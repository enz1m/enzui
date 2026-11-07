//! Disk-backed scratch space for tests.
//!
//! The system temporary directory is a RAM-backed filesystem on the project
//! builders. Integration-test targets get `CARGO_TARGET_TMPDIR` from Cargo at
//! compile time, so this helper uses that path when it is available. Cargo does
//! not provide the variable to `src/` unit-test targets (nor export it to test
//! processes at runtime); those targets derive a profile-local scratch root
//! from their active executable under `target/.../deps` instead. Neither path
//! falls back to the system temporary directory or writes into the source tree.
//!
//! Validation is factored into pure seams (`derive_unit_test_scratch_root`,
//! `choose_scratch_source`, `check_root_shape`) so calibrated unit controls
//! can exercise every refusal branch — including `/tmp`, `/var/tmp`, and
//! `/dev/shm` descendants — without mutating process-global environment or
//! writing anything under the system temporary directory.

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
const COMPILE_TIME_TARGET_TMPDIR: Option<&str> = option_env!("CARGO_TARGET_TMPDIR");

/// System roots that must never host test scratch, in match order.
const SYSTEM_TEMP_ROOTS: [&str; 3] = ["/tmp", "/var/tmp", "/dev/shm"];

/// A uniquely owned scratch directory removed when the guard is dropped.
///
/// This type deliberately does not implement `Clone`: exactly one guard owns
/// cleanup. Call [`ScratchDir::path`] and copy only the `PathBuf` when a path
/// must be moved into a closure.
#[derive(Debug)]
pub struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    /// Create a unique directory in the active Cargo target tree.
    ///
    /// `create_dir` (rather than remove-then-create or `create_dir_all`) makes
    /// ownership atomic: parallel tests and sibling test processes can never
    /// claim or erase one another's fixtures, even if labels coincide.
    #[allow(
        dead_code,
        reason = "crates include this file by `#[path]` and each uses part of it"
    )]
    pub fn new(label: &str) -> Self {
        let root = scratch_root();
        let path = create_unique_dir_with(&root, || unique_leaf(label));
        Self { path }
    }

    /// Create a unique directory with mode 0700, suitable for secrets and
    /// Unix-domain sockets.
    ///
    /// Uses [`std::os::unix::fs::DirBuilderExt::mode`] so creation and
    /// permission are atomic — the directory never exists with permissive
    /// bits.  A pre-existing path or symlink at the candidate location is
    /// never removed; ownership is established by `create_dir` succeeding
    /// (same as [`ScratchDir::new`]).
    ///
    /// The leaf name is deliberately short (`<tag><pid>.<seq>`) so that the
    /// full socket path stays within the 108-byte `sun_path` limit even on
    /// deep work-tree paths.
    #[cfg(unix)]
    #[allow(
        dead_code,
        reason = "crates include this file by `#[path]` and each uses part of it"
    )]
    pub fn new_restricted(tag: &str) -> Self {
        let tag = validate_restricted_tag(tag).unwrap_or_else(|message| panic!("{message}"));
        let root = scratch_root();
        let path = create_unique_restricted_dir_with(&root, || short_leaf(tag));
        Self { path }
    }

    /// Borrow the owned scratch-directory path.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Deref for ScratchDir {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        self.path()
    }
}

impl AsRef<Path> for ScratchDir {
    fn as_ref(&self) -> &Path {
        self.path()
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
            && !std::thread::panicking()
        {
            panic!(
                "test scratch: cannot remove owned directory {}: {error}",
                self.path.display()
            );
        }
    }
}

fn scratch_root() -> PathBuf {
    let (root, origin) = choose_scratch_source(COMPILE_TIME_TARGET_TMPDIR, || {
        let executable = std::env::current_exe().map_err(|error| {
            format!("test scratch: cannot locate active test executable: {error}")
        })?;
        derive_unit_test_scratch_root(&executable)
    })
    .unwrap_or_else(|message| panic!("{message}"));
    validate_scratch_root(root, origin)
}

/// Select a scratch-root source and its diagnostic origin label.
///
/// This is a pure seam: no filesystem, no process environment, no globals.
/// `unit_test_root` is called only when the compile-time variable is absent,
/// so calibrated unit controls can drive every branch without invoking
/// `current_exe()`.
fn choose_scratch_source(
    compile_time: Option<&str>,
    unit_test_root: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<(PathBuf, &'static str), String> {
    match compile_time {
        Some("") => Err(
            "test scratch: compile-time CARGO_TARGET_TMPDIR is empty; refusing system temp".into(),
        ),
        Some(value) => Ok((PathBuf::from(value), "compile-time CARGO_TARGET_TMPDIR")),
        None => Ok((unit_test_root()?, "unit-test executable target tree")),
    }
}

/// Derive the unit-test profile scratch root from an executable path.
///
/// A Cargo unit-test executable normally lives at
/// `<target>/<profile>/deps/<test-binary>` (or the equivalent beneath a
/// target-triple directory), so its profile directory is an unambiguous
/// disk-backed root belonging to the active build, not a source checkout
/// guessed from `CARGO_MANIFEST_DIR`. This function is pure so calibrated
/// unit controls can exercise every failure branch (missing parent, non-`deps`
/// parent, missing profile parent) without touching the filesystem.
fn derive_unit_test_scratch_root(executable: &Path) -> Result<PathBuf, String> {
    let deps = executable.parent().ok_or_else(|| {
        format!(
            "test scratch: active test executable has no parent: {}",
            executable.display()
        )
    })?;
    if deps.file_name().and_then(|name| name.to_str()) != Some("deps") {
        return Err(format!(
            "test scratch: active unit-test executable is not in a Cargo target deps directory: {}",
            executable.display()
        ));
    }
    let profile = deps.parent().ok_or_else(|| {
        format!(
            "test scratch: Cargo target deps directory has no profile parent: {}",
            deps.display()
        )
    })?;
    Ok(profile.join("test-scratch"))
}

fn validate_scratch_root(root: PathBuf, origin: &str) -> PathBuf {
    check_root_shape(&root, origin).unwrap_or_else(|message| panic!("{message}"));
    std::fs::create_dir_all(&root).unwrap_or_else(|error| {
        panic!(
            "test scratch: {origin} root {} is unusable: {error}",
            root.display()
        )
    });
    let canonical = std::fs::canonicalize(&root).unwrap_or_else(|error| {
        panic!(
            "test scratch: cannot resolve {origin} root {}: {error}",
            root.display()
        )
    });
    check_root_shape(&canonical, origin).unwrap_or_else(|message| panic!("{message}"));
    let metadata = std::fs::metadata(&canonical).unwrap_or_else(|error| {
        panic!(
            "test scratch: cannot inspect {origin} root {}: {error}",
            canonical.display()
        )
    });
    assert!(
        metadata.is_dir(),
        "test scratch: {origin} root {} is not a directory",
        canonical.display()
    );
    canonical
}

/// Refuse any candidate root that is non-absolute or under system temp storage.
///
/// This runs *before* `create_dir_all` in `validate_scratch_root`, so a
/// refused root can never leave a directory behind. It is a pure seam:
/// calibrated unit controls exercise each system-temp branch (`/tmp`,
/// `/var/tmp`, `/dev/shm`) without writing anything to disk.
fn check_root_shape(root: &Path, origin: &str) -> Result<(), String> {
    if !root.is_absolute() {
        return Err(format!(
            "test scratch: {origin} root is not absolute: {}",
            root.display()
        ));
    }
    if let Some(system_root) = SYSTEM_TEMP_ROOTS
        .iter()
        .map(Path::new)
        .find(|system_root| root.starts_with(system_root))
    {
        return Err(format!(
            "test scratch: {origin} root {} is under system temporary storage {}; refusing it",
            root.display(),
            system_root.display()
        ));
    }
    Ok(())
}

#[allow(
    dead_code,
    reason = "crates include this file by `#[path]` and each uses part of it"
)]
fn create_unique_dir_with(root: &Path, mut next_leaf: impl FnMut() -> String) -> PathBuf {
    for _ in 0..1024 {
        let path = root.join(next_leaf());
        match std::fs::create_dir(&path) {
            Ok(()) => return path,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!(
                "test scratch: cannot create unique directory {}: {error}",
                path.display()
            ),
        }
    }
    panic!(
        "test scratch: could not allocate a unique directory below {}",
        root.display()
    );
}

/// Like [`create_unique_dir_with`], but creates the directory with mode 0700
/// atomically via [`DirBuilderExt::mode`].
#[cfg(unix)]
#[allow(
    dead_code,
    reason = "crates include this file by `#[path]` and each uses part of it"
)]
fn create_unique_restricted_dir_with(
    root: &Path,
    mut next_leaf: impl FnMut() -> String,
) -> PathBuf {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    for _ in 0..1024 {
        let path = root.join(next_leaf());
        match builder.create(&path) {
            Ok(()) => return path,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!(
                "test scratch: cannot create restricted directory {}: {error}",
                path.display()
            ),
        }
    }
    panic!(
        "test scratch: could not allocate a unique restricted directory below {}",
        root.display()
    );
}

/// Validate the compact tag used by [`ScratchDir::new_restricted`].
///
/// The ASCII allow-list is intentionally narrower than the platform's notion
/// of a normal component. It rejects separators on both Unix and Windows,
/// drive/prefix syntax, `.`/`..`, and every absolute or multi-component path
/// before it can be joined to the trusted scratch root.
#[cfg(unix)]
#[allow(
    dead_code,
    reason = "crates include this file by `#[path]` and each uses part of it"
)]
fn validate_restricted_tag(tag: &str) -> Result<&str, String> {
    if tag.is_empty()
        || !tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(format!(
            "test scratch: restricted tag {tag:?} must be one non-empty ASCII path component containing only letters, digits, '-' or '_'"
        ));
    }
    Ok(tag)
}

/// A deliberately short leaf for Unix-socket-hosting directories.
///
/// Format: `<tag><pid>.<seq>` — keeps the full socket path within the
/// 108-byte `sun_path` limit on deep work-tree paths.
#[cfg(unix)]
#[allow(
    dead_code,
    reason = "crates include this file by `#[path]` and each uses part of it"
)]
fn short_leaf(tag: &str) -> String {
    let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
    format!("{tag}{}.{sequence}", std::process::id())
}

#[allow(
    dead_code,
    reason = "crates include this file by `#[path]` and each uses part of it"
)]
fn unique_leaf(label: &str) -> String {
    let label: String = label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let sequence = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("test scratch: clock is before the Unix epoch")
        .as_nanos();
    format!("{label}-{}-{nanos}-{sequence}", std::process::id())
}
