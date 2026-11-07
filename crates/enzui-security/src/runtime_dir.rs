//! Where runtime objects go: the 0700, per-user directory for sockets,
//! credential files, per-session state and the uid probe
//!
//! # The gap this closes
//!
//! The framework already honours three of the four XDG base directories —
//! `$XDG_DATA_HOME`, `$XDG_CONFIG_HOME`, `$XDG_STATE_HOME` (see `crate::persist`)
//! — but it never mentioned `$XDG_RUNTIME_DIR`, the *one* base directory the
//! spec defines for exactly these objects: sockets, credentials, per-session
//! state. Everything runtime-shaped landed in [`std::env::temp_dir`] instead.
//!
//! `$XDG_RUNTIME_DIR` is the only base directory guaranteed to be owned by the process's uid
//! and mode 0700. `/tmp` is mode 1777: the sticky bit stops *another*
//! user deleting the process's files, it does not stop them **creating a path
//! first**. That pre-creation window is what makes the credential-file and socket
//! checks (`secret::load_secret` and the center connector's
//! `validate_socket_path`) carry an honest "TOCTOU is structurally
//! present" caveat: the window is real *in a world-writable directory*, and
//! absent against other users in a 0700 one they cannot enter. This module
//! is the hardening step those checks were written waiting for; it does not add
//! a check, it removes the directory that made the caveat necessary.
//!
//! # The fallback ladder, and why every rung is stated
//!
//! [`runtime_dir`] resolves in this order:
//!
//! 1. **`$XDG_RUNTIME_DIR`**, when it is set, exists, is a directory (not a
//!    symlink), is owned by the process's uid, and is mode 0700 — used directly
//!    ([`RuntimeDirOrigin::XdgRuntimeDir`]).
//! 2. Otherwise a per-uid 0700 subdirectory created and owned by the process under the
//!    system temp base (`/tmp/enzui-<uid>/`), together with the
//!    [`FallbackReason`] that forced the degrade
//!    ([`RuntimeDirOrigin::Fallback`]).
//!
//! Falling silently back to *today's behaviour* — a bare path in a 1777
//! directory — is deliberately not an option: the degraded answer still
//! carries the permission on the *directory*, because it is created with `mkdir` 0700 and
//! then re-`chmod` it 0700 regardless of umask, and verify it if it already
//! exists. A `/tmp/enzui-<uid>` an attacker pre-created with the wrong owner or
//! a group bit is refused, not trusted.
//!
//! The fallback base is not [`std::env::temp_dir`]: its security comes from
//! the 0700 subdirectory created and verified here, so honouring `$TMPDIR`
//! (which could redirect it somewhere unexpected) buys nothing. `/tmp` is the writable, world-agnostic
//! base on Unix.
//!
//! # Making the refusal visible
//!
//! A `$XDG_RUNTIME_DIR` that fails the owner or mode check is a system in a
//! state the user should hear about, not a shrug. [`FallbackReason::is_refusal`]
//! separates the *security refusals* ([`FallbackReason::WrongOwner`],
//! [`FallbackReason::InsecureMode`]) from the *benign absences*
//! ([`FallbackReason::Unset`], [`FallbackReason::Missing`]); a caller that logs
//! [`RuntimeDir::origin`] surfaces the difference. The probe — the framework's
//! one composed application — does exactly that.

use std::path::{Path, PathBuf};

/// Where this process's runtime objects should live, plus how that answer was
/// reached.
///
/// Construct with [`runtime_dir`]; place a named object with [`RuntimeDir::join`].
#[derive(Debug, Clone)]
pub struct RuntimeDir {
    path: PathBuf,
    origin: RuntimeDirOrigin,
}

impl RuntimeDir {
    /// The resolved directory. Guaranteed (on Unix) to be owned by the process's uid and mode
    /// 0700 by the time this returns — either because `$XDG_RUNTIME_DIR`
    /// passed those checks, or because the fallback was created and verified to
    /// carry them.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// How the directory was chosen — the direct `$XDG_RUNTIME_DIR` or a
    /// fallback with its reason.
    #[must_use]
    pub fn origin(&self) -> &RuntimeDirOrigin {
        &self.origin
    }

    /// A path for a named object inside the runtime directory.
    #[must_use]
    pub fn join(&self, name: impl AsRef<Path>) -> PathBuf {
        self.path.join(name)
    }

    /// `true` when the fallback was taken *because a set `$XDG_RUNTIME_DIR` failed a
    /// security check* (wrong owner or insecure mode) — the case a user should
    /// hear about. `false` for the direct case and for benign absences.
    #[must_use]
    pub fn is_refusal(&self) -> bool {
        matches!(&self.origin, RuntimeDirOrigin::Fallback(r) if r.is_refusal())
    }
}

/// How [`runtime_dir`] arrived at its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeDirOrigin {
    /// `$XDG_RUNTIME_DIR` was set, owned by the process's uid and mode 0700; used directly.
    XdgRuntimeDir,
    /// `$XDG_RUNTIME_DIR` was unusable; a per-uid 0700 directory created
    /// under the system temp base is used instead, for this reason.
    Fallback(FallbackReason),
}

/// Why [`runtime_dir`] declined to use `$XDG_RUNTIME_DIR` and fell back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FallbackReason {
    /// `$XDG_RUNTIME_DIR` was not set, or was empty. Benign: not every session
    /// exports one.
    Unset,
    /// `$XDG_RUNTIME_DIR` was set but the path does not exist. Benign.
    Missing,
    /// `$XDG_RUNTIME_DIR` was set but is not a directory (or is a symlink at
    /// the final component). A refusal.
    NotADirectory,
    /// `$XDG_RUNTIME_DIR` was set but is owned by another uid. A refusal: a
    /// runtime directory not owned by the process is exactly the cross-uid exposure this
    /// module exists to avoid.
    WrongOwner {
        /// The uid that owns the directory.
        dir_uid: u32,
        /// The process's own uid.
        our_uid: u32,
    },
    /// `$XDG_RUNTIME_DIR` was set but grants group or other access
    /// (`mode & 0o077 != 0`). A refusal.
    InsecureMode {
        /// The offending mode (permission bits only).
        mode: u32,
    },
    /// `$XDG_RUNTIME_DIR` was set but the process's own uid could not be established, so it
    /// cannot verify ownership. It falls back rather than trust an unverifiable
    /// directory. A refusal.
    OwnerUnverifiable,
}

impl FallbackReason {
    /// `true` for the reasons that mean "a directory was there but failed a
    /// security check" — the ones a user should hear about — and `false` for
    /// benign absence.
    #[must_use]
    pub fn is_refusal(&self) -> bool {
        match self {
            FallbackReason::Unset | FallbackReason::Missing => false,
            FallbackReason::NotADirectory
            | FallbackReason::WrongOwner { .. }
            | FallbackReason::InsecureMode { .. }
            | FallbackReason::OwnerUnverifiable => true,
        }
    }
}

impl std::fmt::Display for FallbackReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FallbackReason::Unset => write!(f, "$XDG_RUNTIME_DIR is not set"),
            FallbackReason::Missing => write!(f, "$XDG_RUNTIME_DIR does not exist"),
            FallbackReason::NotADirectory => {
                write!(f, "$XDG_RUNTIME_DIR is not a directory (or is a symlink)")
            }
            FallbackReason::WrongOwner { dir_uid, our_uid } => write!(
                f,
                "$XDG_RUNTIME_DIR is owned by uid {dir_uid}, not our uid {our_uid}"
            ),
            FallbackReason::InsecureMode { mode } => write!(
                f,
                "$XDG_RUNTIME_DIR mode {mode:04o} grants group/other access (must be 0700)"
            ),
            FallbackReason::OwnerUnverifiable => {
                write!(f, "could not determine our uid to verify $XDG_RUNTIME_DIR")
            }
        }
    }
}

/// The fallback directory could not be created or secured. This is the loud
/// terminal state: neither `$XDG_RUNTIME_DIR` nor a self-created 0700 directory
/// is available, so there is *nowhere safe* for a runtime object.
#[derive(Debug)]
pub struct RuntimeDirError {
    /// Why `$XDG_RUNTIME_DIR` itself was not used.
    pub reason: FallbackReason,
    /// The I/O error that prevented creating or securing the fallback.
    pub source: std::io::Error,
}

impl std::fmt::Display for RuntimeDirError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "no secure runtime directory available ({}); the fallback could not \
             be secured: {}",
            self.reason, self.source
        )
    }
}

impl std::error::Error for RuntimeDirError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// The current process's owning uid, derived without `libc` or `unsafe`.
///
/// The workspace forbids `unsafe` (`unsafe_code = "deny"`) and this crate
/// carries no `libc`/`rustix` dependency, so a raw `getuid` binding is not
/// available. Two ways to observe the process's own uid, tried in order:
///
/// 1. **`/proc/self`** (Linux). The `/proc/<pid>` directory is owned by the
///    process's uid, so [`std::fs::metadata`] of `/proc/self` reports it with
///    no filesystem write. This is the common path and it runs on every
///    credential load. What it costs: portability — off Linux, or with `/proc`
///    not mounted, `metadata` fails and detection falls to (2).
/// 2. **A created file, read back with `fstat`.** A file this process just created is, by
///    definition, owned by the process's uid, so its [`std::os::unix::fs::MetadataExt::uid`]
///    *is* the process's uid. Created with `O_EXCL` (`create_new`) under `/tmp` so a
///    path an attacker pre-created can never be opened and read back as *their*
///    uid — on collision the answer is `None`, never a wrong answer.
///
/// Returns `None` if neither works. `None` means **"owner verification
/// unavailable"**, and every caller must treat it as such — failing closed,
/// never as "the check passed". That property is load-bearing: `secret::load_secret`
/// maps it to `SecretError::OwnerUnverifiable` rather than skipping the owner
/// check.
#[cfg(unix)]
#[must_use]
pub fn current_uid() -> Option<u32> {
    use std::os::unix::fs::MetadataExt;

    // 1. /proc/self, no write. `metadata` follows the symlink to /proc/<pid>,
    //    the directory the kernel owns as our uid.
    if let Ok(meta) = std::fs::metadata("/proc/self") {
        return Some(meta.uid());
    }

    // 2. Filesystem probe: create a file we own and read its owner back.
    probe_uid_via_tempfile()
}

/// The (2) rung of [`current_uid`]: create a file with `O_EXCL` and read the
/// uid the kernel stamped on it. Split out so it can be exercised directly on
/// platforms where `/proc/self` short-circuits it away.
#[cfg(unix)]
fn probe_uid_via_tempfile() -> Option<u32> {
    // `/tmp` literal, not `std::env::temp_dir()`: see the module docs on why no
    // production `temp_dir()` call survives.
    probe_uid_under(Path::new("/tmp"), |_| ())
}

// The injected base keeps tests out of `/tmp`; the hook reaches the otherwise
// unobservable create-to-cleanup window.
#[cfg(unix)]
fn probe_uid_under(base: &Path, inside_window: impl FnOnce(&Path)) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static PROBE_SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = PROBE_SEQ.fetch_add(1, Ordering::Relaxed);

    // `O_EXCL`, so a pre-existing (possibly attacker-owned) path fails the open
    // outright rather than handing us its owner's uid.
    let probe = base.join(format!("enzui-uidprobe-{}-{seq}", std::process::id()));
    let file = crate::tempstage::create_private_exclusive(&probe).ok()?;
    let uid = file.metadata().ok().map(|m| m.uid());
    inside_window(&probe);
    // The descriptor stays open across the clean-up: it is what proves the name
    // still denotes the file this probe created rather than a substitute.
    let _ = crate::tempstage::remove_if_path_names_file(&probe, &file);
    drop(file);
    uid
}

#[cfg(not(unix))]
#[must_use]
pub fn current_uid() -> Option<u32> {
    None
}

/// Resolve the directory for this process's runtime objects.
///
/// See the [module docs](self) for the fallback ladder and the security
/// argument. On success the returned [`RuntimeDir`] points at a directory owned
/// by the process's uid and mode 0700; inspect [`RuntimeDir::origin`] to learn whether it is
/// `$XDG_RUNTIME_DIR` or a fallback (and, for a fallback, whether the reason
/// was a refusal worth surfacing).
///
/// # Errors
///
/// [`RuntimeDirError`] when neither `$XDG_RUNTIME_DIR` nor a self-created 0700
/// fallback could be secured — the loud terminal state in which there is
/// nowhere safe to place a runtime object.
#[cfg(unix)]
pub fn runtime_dir() -> Result<RuntimeDir, RuntimeDirError> {
    resolve(
        std::env::var_os("XDG_RUNTIME_DIR"),
        current_uid(),
        Path::new("/tmp"),
    )
}

/// The pure resolver behind [`runtime_dir`], with its three inputs injected so
/// the fallback ladder is testable at every rung without mutating global
/// process env or holding a real cross-uid directory (the wrong-owner rung is
/// reached by passing an `our_uid` that differs from the fixture's real
/// owner). This is the same injection shape `crate::persist::XdgResolver`
/// uses; production calls [`runtime_dir`], which wires the real env, uid and
/// `/tmp` base.
///
/// # Errors
///
/// [`RuntimeDirError`] — see [`runtime_dir`].
#[cfg(unix)]
#[doc(hidden)]
pub fn resolve(
    xdg_runtime_dir: Option<std::ffi::OsString>,
    our_uid: Option<u32>,
    fallback_base: &Path,
) -> Result<RuntimeDir, RuntimeDirError> {
    let reason = match xdg_runtime_dir {
        None => FallbackReason::Unset,
        Some(v) if v.is_empty() => FallbackReason::Unset,
        Some(v) => {
            let path = PathBuf::from(&v);
            match our_uid {
                None => FallbackReason::OwnerUnverifiable,
                Some(uid) => match verify_owned_0700(&path, uid) {
                    Ok(()) => {
                        return Ok(RuntimeDir {
                            path,
                            origin: RuntimeDirOrigin::XdgRuntimeDir,
                        });
                    }
                    Err(reason) => reason,
                },
            }
        }
    };

    let path = create_fallback(fallback_base, our_uid).map_err(|source| RuntimeDirError {
        reason: reason.clone(),
        source,
    })?;
    Ok(RuntimeDir {
        path,
        origin: RuntimeDirOrigin::Fallback(reason),
    })
}

/// Verify `path` is a directory owned by `our_uid` with no group/other access.
/// Uses `symlink_metadata` so a symlink masquerading as the directory is a
/// [`FallbackReason::NotADirectory`], not silently followed.
#[cfg(unix)]
fn verify_owned_0700(path: &Path, our_uid: u32) -> Result<(), FallbackReason> {
    use std::os::unix::fs::MetadataExt;

    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(FallbackReason::Missing),
        Err(_) => return Err(FallbackReason::NotADirectory),
    };
    if !meta.file_type().is_dir() {
        return Err(FallbackReason::NotADirectory);
    }
    let dir_uid = meta.uid();
    if dir_uid != our_uid {
        return Err(FallbackReason::WrongOwner { dir_uid, our_uid });
    }
    let mode = meta.mode() & 0o7777;
    if mode & 0o077 != 0 {
        return Err(FallbackReason::InsecureMode { mode });
    }
    Ok(())
}

/// Create (or adopt) a per-uid 0700 directory under `base`. On adoption of an
/// existing directory it is re-verified owned-and-0700 and refused otherwise —
/// the whole point is that the *directory* carries the permission.
#[cfg(unix)]
fn create_fallback(base: &Path, our_uid: Option<u32>) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let name = match our_uid {
        Some(uid) => format!("enzui-{uid}"),
        // No uid to key on: fall to a pid-scoped name. Still a 0700 dir we own,
        // just not shared across this uid's processes.
        None => format!("enzui-pid{}", std::process::id()),
    };
    let path = base.join(name);

    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    match builder.create(&path) {
        Ok(()) => {
            // mkdir's mode is masked by umask; force exactly 0700 so a strict
            // umask cannot leave us a directory we cannot enter, and a lax one
            // cannot leave group/other bits.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
            Ok(path)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Adopt only if it is ours and 0700. Without a uid we cannot verify
            // ownership, so we refuse rather than trust.
            match our_uid {
                Some(uid) => verify_owned_0700(&path, uid)
                    .map(|()| path.clone())
                    .map_err(|r| {
                        std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            format!(
                                "fallback runtime directory {} exists but is unsafe: {r}",
                                path.display()
                            ),
                        )
                    }),
                None => Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "fallback runtime directory {} exists but our uid is \
                         unknown, so ownership cannot be verified",
                        path.display()
                    ),
                )),
            }
        }
        Err(e) => Err(e),
    }
}

#[cfg(not(unix))]
pub fn runtime_dir() -> Result<RuntimeDir, RuntimeDirError> {
    // No Unix owner/mode model to honour; the runtime-directory contract is
    // Unix-specific. Callers of this capability are all `#[cfg(unix)]`.
    Err(RuntimeDirError {
        reason: FallbackReason::Unset,
        source: std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "runtime_dir is only implemented on Unix",
        ),
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static N: AtomicU32 = AtomicU32::new(0);

            let mut path = std::env::current_exe().expect("current test executable");
            path.pop();
            path.push(format!(
                "enzui-uidprobe-scratch-{}-{}-{tag}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).expect("create scratch directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // Independent from the file-create mechanism under test.
    fn uid_from_proc() -> Option<u32> {
        std::fs::metadata("/proc/self").ok().map(|m| m.uid())
    }

    struct ConcurrentRename {
        hook: Box<dyn FnOnce(&Path)>,
        worker: std::thread::JoinHandle<()>,
    }

    fn concurrent_rename(staged: PathBuf, marker: PathBuf) -> ConcurrentRename {
        use std::sync::mpsc;
        use std::time::Duration;

        let (path_tx, path_rx) = mpsc::channel::<PathBuf>();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let path = path_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("the uid probe must reach the replacement window");
            std::fs::rename(&staged, path).expect("swap in the foreign entry");
            std::fs::write(marker, b"worker ran").expect("record the replacement");
            done_tx.send(()).expect("report the completed replacement");
        });
        let hook = Box::new(move |path: &Path| {
            path_tx
                .send(path.to_owned())
                .expect("release the replacement worker");
            done_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("the replacement worker must finish inside the window");
        });
        ConcurrentRename { hook, worker }
    }

    #[test]
    fn uid_probe_removes_the_file_it_created() {
        let scratch = Scratch::new("cleanup");
        let sentinel = scratch.join("neighbour");
        std::fs::write(&sentinel, b"unrelated").expect("seed a neighbouring file");

        let mut created = None;
        let uid = probe_uid_under(scratch.path(), |probe| {
            assert!(
                probe.exists(),
                "the probe file must exist inside the window; {} is absent",
                probe.display()
            );
            created = Some(probe.to_owned());
        })
        .expect("an exclusive create in an owned directory must yield our uid");

        if let Some(expected) = uid_from_proc() {
            assert_eq!(
                uid, expected,
                "the created file's owner must be the uid /proc/self reports"
            );
        }
        let created = created.expect("the rendezvous inside the window must run");
        assert!(
            !created.exists(),
            "the probe must remove the file it created; {} survived",
            created.display()
        );
        assert!(
            sentinel.exists(),
            "clean-up must not reach a neighbouring file"
        );
    }

    #[test]
    fn uid_probe_leaves_a_symlink_planted_at_its_name_untouched() {
        let scratch = Scratch::new("planted-symlink");
        let victim = scratch.join("victim");
        std::fs::write(&victim, b"do-not-touch").expect("seed the symlink target");

        let staged_symlink = scratch.join("staged-symlink");
        std::os::unix::fs::symlink(&victim, &staged_symlink).expect("stage a symlink");
        let planted = scratch.join("planted-marker");
        let ConcurrentRename { hook, worker } = concurrent_rename(staged_symlink, planted.clone());

        let uid = probe_uid_under(scratch.path(), hook);
        worker.join().expect("replacement worker");

        assert!(
            uid.is_some(),
            "a substituted name must not cost the uid diagnostic"
        );
        assert!(planted.exists(), "the replacement worker ran");
        let replaced_path = std::fs::read_dir(scratch.path())
            .expect("read scratch directory")
            .map(|entry| entry.expect("read scratch entry").path())
            .find(|path| {
                std::fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink())
            })
            .expect("the planted symlink survives");
        assert!(
            std::fs::symlink_metadata(&replaced_path)
                .expect("lstat the planted symlink")
                .file_type()
                .is_symlink(),
            "the planted symlink must survive clean-up; {} was removed",
            replaced_path.display()
        );
        assert_eq!(
            std::fs::read(&victim).expect("read the symlink target"),
            b"do-not-touch",
            "the symlink target must be untouched"
        );
    }

    #[test]
    fn uid_probe_leaves_a_same_uid_replacement_untouched() {
        let scratch = Scratch::new("replacement");

        let staged_file = scratch.join("staged-file");
        std::fs::write(&staged_file, b"another writer's file").expect("stage a foreign file");
        let marker = scratch.join("replacement-marker");
        let ConcurrentRename { hook, worker } = concurrent_rename(staged_file, marker.clone());

        let uid = probe_uid_under(scratch.path(), hook);
        worker.join().expect("replacement worker");

        assert!(
            uid.is_some(),
            "a substituted name must not cost the uid diagnostic"
        );
        assert!(marker.exists(), "the replacement worker ran");
        let replaced = std::fs::read_dir(scratch.path())
            .expect("read scratch directory")
            .map(|entry| entry.expect("read scratch entry").path())
            .find(|path| std::fs::read(path).is_ok_and(|bytes| bytes == b"another writer's file"))
            .expect("the replacement file survives");
        assert_eq!(
            std::fs::read(&replaced).expect("read the replacement file"),
            b"another writer's file",
            "clean-up must not remove a file this probe did not create"
        );
    }
}
