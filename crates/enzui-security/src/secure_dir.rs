//! Private store roots and retained ordinary-directory capabilities.
//!
//! [`ensure_private_dir`] creates missing components at `0700` and adopts an
//! existing leaf only when it is owned by the current user and not group/other
//! writable. Existing ancestors are trusted; symlinks at the leaf are refused.
//! [`SecureRoot`] adds a retained descriptor for entry operations and private
//! subroots; [`SecureRoot::revalidate`] explicitly checks pathname reachability.
//!
//! On Unix, [`RetainedDir`] retains ordinary directories without owner or mode
//! restrictions. Its basename-only subroot operations do not reopen ancestors.
//! Linux [`RetainedEntry`] moves return [`MoveCustody`] on failure and offer
//! regular-file EXDEV copy/remove. This is not a Trash metadata transaction.
//! Rename/unlink identity checks narrow, but cannot eliminate, the interval
//! before the name-based syscall; callers must exclude hostile directory writers.

use std::path::{Path, PathBuf};

#[cfg(unix)]
mod capability;
#[cfg(unix)]
pub use capability::{SecureRoot, open_file_identity};
#[cfg(unix)]
pub(crate) use capability::{
    create_entry_private_at, entry_identity_at, unlink_entry_if_identity_at,
};
#[cfg(unix)]
mod retained;
#[cfg(unix)]
pub use retained::RetainedDir;
#[cfg(target_os = "linux")]
pub use retained::{EnumerationLimits, MoveCustody, MoveFailure, RetainedEntry, TreeSizeLimits};

/// Why [`ensure_private_dir`] refused a candidate store root.
///
/// [`SecureDirRefusal::CreateFailed`] is benign unavailability (a read-only
/// home, a full disk, a sandbox); every other variant means something was
/// already at the path and failed a security check, which a caller may want to
/// surface rather than treat as an ordinary miss.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecureDirRefusal {
    /// The path (or a missing parent) could not be created.
    CreateFailed,
    /// The final component is not a directory — a symlink (refused before it can
    /// be followed), a regular file, or another non-directory object.
    NotADirectory,
    /// The directory is owned by another uid.
    WrongOwner {
        /// The uid that owns the directory.
        dir_uid: u32,
        /// The process's own uid.
        our_uid: u32,
    },
    /// The directory grants group or other write access
    /// (`mode & 0o022 != 0`), which would let another user plant a symlink or a
    /// substitute entry at an opened name.
    GroupOrWorldWritable {
        /// The offending mode (permission bits only).
        mode: u32,
    },
    /// The process's own uid could not be established, so ownership cannot be verified. It
    /// fails closed rather than trust an unverifiable directory — the same
    /// posture as [`runtime_dir`](crate::runtime_dir).
    OwnerUnverifiable,
    /// Explicit reachability validation found a missing or substituted root pathname.
    /// Descriptor-relative operations remain bound to the retained inode.
    RootIdentityChanged {
        /// `(dev, ino)` captured when [`SecureRoot::open`] verified the leaf.
        original: (u64, u64),
        /// `(dev, ino)` the pathname currently resolves to under `lstat`, or
        /// `None` when the pathname no longer exists / cannot be stat'd.
        current: Option<(u64, u64)>,
    },
}

impl std::fmt::Display for SecureDirRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecureDirRefusal::CreateFailed => write!(f, "the directory could not be created"),
            SecureDirRefusal::NotADirectory => {
                write!(f, "the path is not a directory (or is a symlink)")
            }
            SecureDirRefusal::WrongOwner { dir_uid, our_uid } => write!(
                f,
                "the directory is owned by uid {dir_uid}, not our uid {our_uid}"
            ),
            SecureDirRefusal::GroupOrWorldWritable { mode } => write!(
                f,
                "the directory mode {mode:04o} grants group/other write access"
            ),
            SecureDirRefusal::OwnerUnverifiable => {
                write!(f, "could not determine our uid to verify ownership")
            }
            SecureDirRefusal::RootIdentityChanged { original, current } => match current {
                Some((dev, ino)) => write!(
                    f,
                    "the root pathname now resolves to (dev {dev}, ino {ino}) \
                     instead of the verified (dev {}, ino {})",
                    original.0, original.1
                ),
                None => write!(
                    f,
                    "the root pathname no longer resolves to the verified \
                     (dev {}, ino {})",
                    original.0, original.1
                ),
            },
        }
    }
}

impl std::error::Error for SecureDirRefusal {}

/// Ensure `path` exists as a directory owned by the process that no other user can write,
/// creating it and any missing parents at mode `0o700` first.
///
/// See the [module docs](self) for the create-private / adopt-verified split and
/// the residual TOCTOU exposure. On success the returned path is `path`.
///
/// # Errors
///
/// [`SecureDirRefusal`] when the path could not be created, or when an existing
/// leaf is a symlink or non-directory, is owned by another uid, or grants
/// group/other write. A just-created directory that the platform left
/// group/other-writable in defiance of the `0o700` request lands in the same
/// [`SecureDirRefusal::GroupOrWorldWritable`] arm — the mode is checked after
/// creation, not assumed from the request.
#[cfg(unix)]
pub fn ensure_private_dir(path: &Path) -> Result<PathBuf, SecureDirRefusal> {
    ensure_and_verify_leaf(path).map(|leaf| leaf.path)
}

/// The pathname-view identity of a verified leaf: what [`ensure_private_dir`]
/// actually observed with `symlink_metadata` before returning.
///
/// Used by [`SecureRoot::open`] so the openat that follows can compare its own
/// `fstat(fd)` identity against the exact pair that passed verification: a
/// same-uid attacker who swapped a private-looking directory into place
/// between `symlink_metadata` and `openat` no longer wins by simply matching
/// the policy — the inode itself has to match.
#[cfg(unix)]
#[derive(Debug, Clone)]
pub(crate) struct VerifiedLeaf {
    /// The verified pathname (unchanged from what the caller passed).
    pub(crate) path: PathBuf,
    /// `st_dev` observed via `symlink_metadata` at verification time.
    pub(crate) dev: u64,
    /// `st_ino` observed via `symlink_metadata` at verification time.
    pub(crate) ino: u64,
    /// The process's own uid, resolved once and passed on so callers do not resolve it
    /// twice for the same operation.
    pub(crate) our_uid: u32,
}

/// Shared verification pipeline: create the chain, then `symlink_metadata`-
/// verify the leaf, then return the observed identity.
///
/// The identity return is load-bearing for [`SecureRoot::open`]: it opens the
/// path *again* to get an fd and compares the fd's `fstat` identity against
/// this pair, refusing any substitution that happened between the two
/// syscalls. Callers that only need the pathname (the legacy
/// [`ensure_private_dir`] contract) discard the extra fields.
#[cfg(unix)]
pub(crate) fn ensure_and_verify_leaf(path: &Path) -> Result<VerifiedLeaf, SecureDirRefusal> {
    let our_uid = crate::runtime_dir::current_uid().ok_or(SecureDirRefusal::OwnerUnverifiable)?;
    create_private_chain(path)?;
    // The leaf is verified whether we just made it or adopted one that was
    // already there: a fresh `mkdir` can still land in a hostile parent, and a
    // race between our create and this check has to be caught by the same
    // `symlink_metadata` that catches a pre-planted link.
    verify_leaf_via_symlink_metadata(path, our_uid)
}

/// Create every missing component of `path` with mode `0o700`, forcing exactly
/// `0o700` on each component created.
///
/// `create_dir_all` is not used: its mode is `0o777 & !umask`, which is the very
/// thing that made the former provisioning path umask-dependent. Nested creation
/// is preserved — each ancestor is created in turn — and a component that
/// already exists is left exactly as it is (no chmod-after).
#[cfg(unix)]
fn create_private_chain(path: &Path) -> Result<(), SecureDirRefusal> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);

    // Ancestors, outermost first. `path.ancestors()` yields leaf-to-root, so
    // reverse it; the empty path and the root itself are no-ops for `mkdir`.
    let mut chain: Vec<&Path> = path.ancestors().collect();
    chain.reverse();
    for component in chain {
        if component.as_os_str().is_empty() {
            continue;
        }
        match builder.create(component) {
            Ok(()) => {
                // Force exactly 0700 on every component we created: `mkdir`'s
                // mode is `0o700 & !umask`, so a mask that clips an owner bit
                // (`umask 277`) would leave `0o500` — a directory we cannot even
                // create the next component inside — while a lax mask is handled
                // by the `0o700` argument itself. Components that already
                // existed are skipped below and never chmod'd.
                std::fs::set_permissions(component, std::fs::Permissions::from_mode(0o700))
                    .map_err(|_| SecureDirRefusal::CreateFailed)?;
            }
            // Already there: adopt it. The leaf's identity/owner/mode is
            // verified by `verify_leaf`; an intermediate component is not (see
            // the residual-exposure note in the module docs).
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(SecureDirRefusal::CreateFailed),
        }
    }
    Ok(())
}

/// Verify the final component is a directory owned by `our_uid` that grants no
/// group/other write, returning the observed identity.
///
/// Uses `symlink_metadata`, so a symlink at the leaf is a
/// [`SecureDirRefusal::NotADirectory`] and is never followed to its target.
/// The `(dev, ino)` return is used by [`SecureRoot::open`] to cross-check the
/// subsequent openat.
#[cfg(unix)]
fn verify_leaf_via_symlink_metadata(
    path: &Path,
    our_uid: u32,
) -> Result<VerifiedLeaf, SecureDirRefusal> {
    use std::os::unix::fs::MetadataExt;

    let meta = std::fs::symlink_metadata(path).map_err(|_| SecureDirRefusal::NotADirectory)?;
    if !meta.file_type().is_dir() {
        return Err(SecureDirRefusal::NotADirectory);
    }
    let dir_uid = meta.uid();
    if dir_uid != our_uid {
        return Err(SecureDirRefusal::WrongOwner { dir_uid, our_uid });
    }
    let mode = meta.mode() & 0o7777;
    if mode & 0o022 != 0 {
        return Err(SecureDirRefusal::GroupOrWorldWritable { mode });
    }
    Ok(VerifiedLeaf {
        path: path.to_path_buf(),
        dev: meta.dev(),
        ino: meta.ino(),
        our_uid,
    })
}

/// Non-Unix fallback: no owner/mode model here, so this creates the directory
/// and confirms a directory is what ended up at the path. The mode, ownership
/// and symlink checks are Unix-specific by construction.
#[cfg(not(unix))]
pub fn ensure_private_dir(path: &Path) -> Result<PathBuf, SecureDirRefusal> {
    std::fs::create_dir_all(path).map_err(|_| SecureDirRefusal::CreateFailed)?;
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => Ok(path.to_path_buf()),
        _ => Err(SecureDirRefusal::NotADirectory),
    }
}
