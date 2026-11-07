//! The one parent-directory durability barrier the framework's atomic writers
//! share.
//!
//! # Why this lives here, once
//!
//! A temp-then-`rename(2)` writer only becomes durable when the **parent
//! directory** is `fsync`ed after the rename: the rename is directory metadata,
//! and without syncing it a power loss after a returned `Ok` can lose the new
//! directory entry even though the file's own data blocks were flushed. Several
//! writers across the workspace implement that step, and the hazard is that
//! each does it slightly differently — one tolerating the wrong error class,
//! one swallowing a real `EIO`. `enzui-security` is the single crate that
//! both `enzui-core` (the shared theme file) and the optional
//! `enzui-agent` (the consent grant store) already depend on, so the
//! barrier and its tolerance policy live here to be reused verbatim rather than
//! re-derived.
//!
//! # The tolerance policy, and why it is exactly two kinds
//!
//! Some filesystems and platforms legitimately refuse `fsync` on a directory
//! descriptor, reporting `EINVAL` ([`io::ErrorKind::InvalidInput`]) or
//! `ENOTSUP` ([`io::ErrorKind::Unsupported`]). That means "this platform does
//! not require the barrier", not "a write may vanish", so those two — and
//! only those two — are tolerated. Everything else, `EIO` above all (the
//! actual disk-is-failing signal), is propagated so a caller can surface it
//! through its own typed error rather than reporting a durability guarantee it
//! did not achieve. This matches the proven writers
//! (`enzui_core::atomic_state::save_atomic`,
//! `enzui_core::crash_report`) so there is one policy, not three.

use std::fs::File;
use std::io;
use std::path::Path;

/// `fsync` the parent directory of `path` so a just-completed `rename(2)`
/// publishing `path` is durable.
///
/// Call this after the rename has succeeded. Opens the parent directory,
/// syncs it, and returns:
///
/// * `Ok(())` when the sync succeeds or when the platform legitimately refuses
///   a directory `fsync`; a bare relative path syncs the current directory
///   (`EINVAL`/`ENOTSUP` — see the module docs);
/// * `Err(e)` for any other failure (notably `EIO` and permission errors) and
///   for a failure to *open* the parent directory, so the caller can propagate
///   it through its own typed error.
///
/// # Errors
/// Propagates the parent-directory open error, or a non-tolerated `sync_all`
/// error.
pub fn sync_parent_dir(path: &Path) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    // `Path::parent` represents the parent of a bare relative name as an empty
    // path. That parent is the current directory, and its directory entry needs
    // the same post-rename durability barrier as any explicitly named parent.
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let dir = open_parent_dir(parent).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "open parent directory {} for durability sync: {e}",
                parent.display()
            ),
        )
    })?;
    sync_retained_dir(&dir, parent)
}

pub(crate) fn sync_retained_dir(dir: &File, path: &Path) -> io::Result<()> {
    if let Err(e) = sync_dir(dir) {
        let tolerated = matches!(
            e.kind(),
            io::ErrorKind::InvalidInput | io::ErrorKind::Unsupported
        );
        if !tolerated {
            return Err(io::Error::new(
                e.kind(),
                format!(
                    "sync parent directory {} after publication: {e}",
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

/// Open the parent directory, with a deterministic test-only failure seam.
fn open_parent_dir(parent: &Path) -> io::Result<File> {
    #[cfg(feature = "fault-injection")]
    if let Some(e) = fault::take_open() {
        return Err(e);
    }
    File::open(parent)
}

/// The actual directory sync, wrapped so tests in either consuming crate
/// can force it to fail.
///
/// The consuming crates (`enzui-core`, `enzui-agent`) compile this
/// crate as an ordinary, non-`cfg(test)` dependency, so a `#[cfg(test)]` fault
/// switch here would be invisible to their tests. The switch is therefore
/// gated on the `fault-injection` feature, which those crates turn on only
/// in their `[dev-dependencies]`; a normal (product) build never enables it, so
/// this compiles to exactly `dir.sync_all()`.
fn sync_dir(dir: &File) -> io::Result<()> {
    #[cfg(feature = "fault-injection")]
    if let Some(e) = fault::take() {
        return Err(e);
    }
    dir.sync_all()
}

/// Test-only fault injection for [`sync_parent_dir`], available when the
/// `fault-injection` feature is enabled.
///
/// A consuming crate's test arms a single forced parent-open or directory-sync
/// failure of a chosen [`io::ErrorKind`]; the matching operation consumes it
/// and returns that error. This is how atomic-writer tests induce failures at
/// either half of the barrier and assert they are surfaced — without relying
/// on permissions or a real failing filesystem.
#[cfg(feature = "fault-injection")]
pub mod fault {
    use std::cell::Cell;
    use std::io;

    thread_local! {
        static OPEN_ARMED: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
        static SYNC_ARMED: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
    }

    /// Arm one forced parent-directory open failure of `kind` on this thread.
    pub fn arm_open(kind: io::ErrorKind) {
        OPEN_ARMED.with(|a| a.set(Some(kind)));
    }

    /// Arm one forced parent-directory `fsync` failure of `kind` on this thread.
    pub fn arm(kind: io::ErrorKind) {
        SYNC_ARMED.with(|a| a.set(Some(kind)));
    }

    /// Whether an armed sync failure is still waiting to be consumed.
    ///
    /// The mechanism half of a barrier test: asserting only that a
    /// writer *failed* cannot distinguish "the barrier ran and surfaced the
    /// fault" from "the writer never synced anything", because a writer that
    /// skips [`super::sync_parent_dir`] entirely leaves the switch armed. A
    /// caller that must prove it reached the barrier — or, for a path that
    /// deliberately owes no barrier, that it did not — reads this.
    /// Non-consuming, so it can be read before `clear`.
    #[must_use]
    pub fn is_armed() -> bool {
        SYNC_ARMED.with(|a| a.get().is_some())
    }

    /// Whether an armed parent-open failure is still waiting to be consumed.
    #[must_use]
    pub fn is_open_armed() -> bool {
        OPEN_ARMED.with(|a| a.get().is_some())
    }

    /// Disarm any pending forced failure on this thread.
    pub fn clear() {
        OPEN_ARMED.with(|a| a.set(None));
        SYNC_ARMED.with(|a| a.set(None));
    }

    /// Consume a pending forced sync failure, if armed.
    pub(super) fn take() -> Option<io::Error> {
        SYNC_ARMED.with(|a| {
            a.take()
                .map(|k| io::Error::new(k, "injected parent-dir fsync failure"))
        })
    }

    /// Consume a pending forced open failure, if armed.
    pub(super) fn take_open() -> Option<io::Error> {
        OPEN_ARMED.with(|a| {
            a.take()
                .map(|k| io::Error::new(k, "injected parent-dir open failure"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TmpDir(std::path::PathBuf);

    impl TmpDir {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicU32, Ordering};
            static N: AtomicU32 = AtomicU32::new(0);
            // Unit-test scratch stays beside the test binary under `target/`.
            let mut p = std::env::current_exe().expect("current test executable");
            p.pop();
            p.push(format!(
                "enzui-durable-{}-{}-{tag}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).expect("temp dir");
            Self(p)
        }
    }

    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn syncing_an_existing_parent_succeeds() {
        let d = TmpDir::new("ok");
        let p = d.0.join("file");
        std::fs::write(&p, b"x").expect("write");
        sync_parent_dir(&p).expect("a healthy parent sync must succeed");
    }

    #[test]
    fn a_bare_relative_path_syncs_the_current_directory() {
        sync_parent_dir(Path::new("bare-name")).expect("current directory sync");
    }

    #[cfg(feature = "fault-injection")]
    #[test]
    fn a_bare_relative_path_reaches_the_directory_sync_barrier() {
        fault::arm(io::ErrorKind::Other);
        let out = sync_parent_dir(Path::new("bare-name"));
        let consumed = !fault::is_armed();
        fault::clear();
        assert!(consumed, "the current-directory barrier was not reached");
        assert!(
            out.is_err(),
            "the injected barrier failure was not surfaced"
        );
    }
}
