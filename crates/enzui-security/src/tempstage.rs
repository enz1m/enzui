//! The shared staged temp file for the framework's atomic writers.
//!
//! # Who publishes through here, mechanically
//!
//! Three private staging primitives, one ladder:
//!
//! * [`TempStage`] — pathname staging next to the destination. Its default
//!   constructor is owner-only (`0600`) for state files; the dedicated
//!   [`TempStage::create_document`] constructor is fixed at `0644` for the
//!   user-authored theme document. Used by `enzui_core::atomic_state`,
//!   `enzui_core::crash_report` and `enzui_core::theme_file`. A theme editor's `source` module is a
//!   no-clobber backup flow rather than a replace-the-destination one, so it
//!   composes this module's naming, unlink and [`link_new_by_identity`] pieces
//!   instead of the whole stage.
//! * [`RootTempStage`] — descriptor-relative file staging inside a verified
//!   [`SecureRoot`](crate::secure_dir::SecureRoot). Used by
//!   `enzui_resources::http_cache`, which must not resolve its cache root
//!   by pathname again after adoption.
//! * [`DirectoryStage`] — exclusive private-directory allocation with
//!   descriptor-relative recursive Drop cleanup. Used by
//!   `enzui_resources::service_download` (and by the `enzui-webkit`
//!   repository's web-view probe) for scratch subtrees a
//!   subprocess or native service writes into. A subtree whose identity
//!   changed after retention is not traversed and its owned ancestor is
//!   retained; each per-entry `fstatat`→`unlinkat` step is the same strict
//!   narrowing [`SecureRoot::unlink_entry_if_identity`](crate::secure_dir::SecureRoot::unlink_entry_if_identity)
//!   documents (POSIX has no unlink-by-inode).
//!
//! # The gap this closes
//!
//! Every temp-then-`rename(2)` writer in the workspace has to get the *same*
//! four things right, and each one that re-derives them gets a different subset
//! right:
//!
//! * Collision. The temp name must be unique per call, not per process.
//!   A `<file>.tmp-<pid>` name is not unique *within* a process, so two
//!   concurrent writers of one destination derive the same path. Paired with
//!   the "unlink the stale temp and retry" branch that a PID-only name forces,
//!   the interleaving
//!
//!   ```text
//!   A: create tmp        (O_EXCL succeeds)
//!   B: create tmp        -> AlreadyExists -> unlink A's tmp -> create fresh
//!   A: write + fsync + rename(tmp -> dest)   <-- renames B's file, not A's
//!   ```
//!
//!   The lost race publishes a partial or foreign file as the destination,
//!   and the unlink silently destroys the other writer's still-open temp with
//!   no error anywhere. `enzui_core::atomic_state` diagnosed and fixed this;
//!   the since-removed agent grant store still carried the PID-only name and
//!   the unlink-and-retry, where the lost write was a consent revocation.
//! * Mode. Creation starts restrictive (`0600`), then the still-open
//!   descriptor is normalized to the constructor's exact final mode before the
//!   first write. This defeats both lax and restrictive process umasks without
//!   a world-writable window or a pathname-based `chmod`. State stays `0600`;
//!   the deliberately separate document constructor is exactly `0644`. There
//!   is no public arbitrary-mode constructor.
//! * Failure. A collision after unique naming is hostile or a broken
//!   filesystem, never stale, so it must be refused — never unlinked and
//!   retried.
//! * Cleanup. Every early return between "temp created" and "temp renamed"
//!   must unlink the temp, and so must an unwind. Hand-written `let _ =
//!   remove_file(&tmp)` on each error branch is one `?` away from leaking a
//!   private file into a shared directory forever.
//!
//! [`TempStage`] does all four once. On Unix it retains the resolved parent
//! directory and performs creation, publication, cleanup, and the durability
//! barrier through that descriptor. The name is unique per call, the create is
//! `O_CREAT | O_EXCL | O_NOFOLLOW` at the constructor's fixed mode, and a
//! collision is an error. Unless [`TempStage::commit`] published it, [`Drop`]
//! unlinks the temp only after revalidating its descriptor identity; an
//! inconclusive revalidation leaves the `0600` state or `0644` document temp
//! rather than unlinking a name that may denote a foreign inode.
//!
//! # Why the temp is a *sibling*
//!
//! [`rename(2)`](std::fs::rename) is only atomic within one filesystem. A
//! temp under `/tmp` (or any other base) turns publication into a cross-device
//! `EXDEV` failure — or, with a copying fallback, into a non-atomic copy a
//! reader can observe torn. The staged file is therefore always created next to
//! its destination.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The unique staging basename for one in-flight write of `name`.
///
/// The naming rule itself, shared by the pathname staging owner
/// ([`TempStage`]) and the descriptor-relative one ([`RootTempStage`]) so a
/// second writer cannot re-derive a subtly different one: a process-wide
/// counter for in-process uniqueness, the PID for cross-process uniqueness,
/// and the nanosecond clock so a temp abandoned by an earlier run with a
/// recycled PID and counter is implausible.
#[must_use]
pub fn unique_temp_file_name(name: &OsStr) -> OsString {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut tmp_name = name.to_os_string();
    tmp_name.push(format!(".tmp-{}-{seq}-{nanos}", std::process::id()));
    tmp_name
}

/// A unique temp sibling name for one in-flight write of `dest`.
///
/// `<file>.tmp-<pid>-<seq>-<nanos>`: the process-wide atomic counter makes the
/// name unique within the process (the case a PID cannot cover), the PID
/// makes it unique across processes, and the nanosecond clock makes a
/// collision with a temp abandoned by an earlier run with a recycled PID and
/// counter implausible.
///
/// Exposed so a test can prove the name is *not* derivable from the PID alone.
///
/// # Errors
/// [`io::ErrorKind::InvalidInput`] when `dest` has no final path component and
/// therefore no sibling to name.
pub fn unique_temp_sibling(dest: &Path) -> io::Result<PathBuf> {
    let file_name = dest
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    Ok(dest.with_file_name(unique_temp_file_name(file_name)))
}

pub(crate) fn write_staged(file: &mut File, bytes: &[u8]) -> io::Result<()> {
    #[cfg(any(test, feature = "fault-injection"))]
    if let Some(e) = fault::take_write() {
        return Err(e);
    }
    file.write_all(bytes)
}

pub(crate) fn sync_staged(file: &File) -> io::Result<()> {
    #[cfg(any(test, feature = "fault-injection"))]
    if let Some(e) = fault::take_sync() {
        return Err(e);
    }
    file.sync_all()
}

#[cfg(unix)]
fn rename_staged_at(parent: &File, tmp: &OsStr, dest: &OsStr) -> io::Result<()> {
    #[cfg(any(test, feature = "fault-injection"))]
    if let Some(e) = fault::take_rename() {
        return Err(e);
    }
    rustix::fs::renameat(parent, tmp, parent, dest).map_err(io::Error::from)
}

#[cfg(not(unix))]
fn rename_staged(tmp: &Path, dest: &Path) -> io::Result<()> {
    #[cfg(any(test, feature = "fault-injection"))]
    if let Some(e) = fault::take_rename() {
        return Err(e);
    }
    std::fs::rename(tmp, dest)
}

/// Test-only fault injection for the shared staging owners, available inside
/// this crate's own tests and to a consuming crate that enables the
/// `fault-injection` feature in its `[dev-dependencies]`.
///
/// A staged publication has failure points a real filesystem will not produce
/// on demand — retained-parent open, descriptor-mode normalization, write,
/// pre-rename `fsync`, and rename. Each is armed here for one shot on the
/// calling thread so a test can assert the error is surfaced, the previous
/// destination survives, and no
/// owned staging residue is left behind.
///
/// [`fault::arm_after_sync`] adds the hostile substitution window between the
/// staged-file durability barrier and publication.
/// [`fault::arm_after_parent_sync`] adds a rendezvous after pathname
/// publication's parent-directory barrier but before its final destination
/// identity check.
/// [`fault::arm_root_identity_failure`] adds the earlier window between
/// descriptor-relative creation and `fstat` of the held descriptor, proving
/// that a failed identity acquisition never authorizes name-only cleanup.
///
/// A product build enables no feature and is not `cfg(test)`, so the seams
/// compile to exactly descriptor `set_permissions` / `write_all` / `sync_all` /
/// `renameat` / parent-directory `fsync` / `fstat`.
#[cfg(any(test, feature = "fault-injection"))]
pub mod fault {
    use std::cell::{Cell, RefCell};
    use std::ffi::OsStr;
    use std::io;
    use std::path::Path;

    type NamedAction = Box<dyn FnOnce(&OsStr)>;
    type PathAction = Box<dyn FnOnce(&Path)>;

    thread_local! {
        static WRITE_ARMED: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
        static SYNC_ARMED: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
        static RENAME_ARMED: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
        static MODE_ARMED: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
        static PARENT_OPEN_ARMED: Cell<Option<io::ErrorKind>> = const { Cell::new(None) };
        static BEFORE_CREATE: RefCell<Option<PathAction>> = const { RefCell::new(None) };
        static AFTER_SYNC: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static BEFORE_PARENT_SYNC: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static AFTER_PARENT_SYNC: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static ROOT_IDENTITY_FAILURE: RefCell<Option<(io::ErrorKind, NamedAction)>> =
            const { RefCell::new(None) };
        static DIRECTORY_CLEANUP: RefCell<Option<(std::path::PathBuf, PathAction)>> =
            const { RefCell::new(None) };
        static CLEANUP_BUDGET: Cell<Option<(usize, usize)>> = const { Cell::new(None) };
    }

    /// Arm one forced retained-parent open failure of `kind` on this thread.
    pub fn arm_parent_open(kind: io::ErrorKind) {
        PARENT_OPEN_ARMED.with(|a| a.set(Some(kind)));
    }

    /// Arm one action after pathname-stage name selection and before its
    /// exclusive create, on this thread.
    ///
    /// This lets a consuming test plant a regular entry or symlink at the exact
    /// unpredictable name and prove the real route refuses rather than removes
    /// it. Product builds do not contain this seam.
    pub fn arm_before_create(action: Box<dyn FnOnce(&Path)>) {
        BEFORE_CREATE.with(|a| *a.borrow_mut() = Some(action));
    }

    /// Arm one action to run after the staged `fsync` and before
    /// publication, on this thread.
    ///
    /// The window a fault *kind* cannot express: the interesting hostile event
    /// between the durability barrier and the rename is not an error return
    /// but a name substitution — someone unlinking the staging entry and
    /// planting their own file at that name. A test arms the substitution
    /// here so it lands in the real window inside
    /// either staging owner's `commit`, rather than before the call where the
    /// `fsync` has not happened yet.
    pub fn arm_after_sync(action: Box<dyn FnOnce()>) {
        AFTER_SYNC.with(|a| *a.borrow_mut() = Some(action));
    }

    /// Arm one action after pathname publication and before its parent
    /// directory is synchronized, on this thread.
    pub fn arm_before_parent_sync(action: Box<dyn FnOnce()>) {
        BEFORE_PARENT_SYNC.with(|a| *a.borrow_mut() = Some(action));
    }

    /// Arm one rendezvous after [`super::TempStage::commit`]'s blocking parent
    /// directory sync returns and before its final destination identity check.
    ///
    /// A test can pause the real commit path here while a concurrent actor
    /// replaces or unlinks the published name. Product builds do not contain
    /// this seam.
    pub fn arm_after_parent_sync(action: Box<dyn FnOnce()>) {
        AFTER_PARENT_SYNC.with(|a| *a.borrow_mut() = Some(action));
    }

    /// Arm a forced held-descriptor identity failure for
    /// [`super::RootTempStage::create`], after running `action` with the exact
    /// staging basename.
    ///
    /// This models the dangerous ordering where exclusive creation succeeds,
    /// another actor substitutes the name, and the subsequent `fstat` fails.
    /// Because no held `(device, inode)` was acquired, creation must return the
    /// error without unlinking the current name.
    pub fn arm_root_identity_failure(kind: io::ErrorKind, action: Box<dyn FnOnce(&OsStr)>) {
        ROOT_IDENTITY_FAILURE.with(|slot| *slot.borrow_mut() = Some((kind, action)));
    }

    /// Arm one action after a directory stage has retained the identity at
    /// `target`, but before Drop traverses or removes that directory.
    ///
    /// Product builds do not contain this seam.
    pub fn arm_directory_cleanup(target: std::path::PathBuf, action: Box<dyn FnOnce(&Path)>) {
        DIRECTORY_CLEANUP.with(|slot| *slot.borrow_mut() = Some((target, action)));
    }

    /// Override the descriptor-relative cleanup depth and per-Drop entry
    /// budgets for the calling thread until [`clear_cleanup_budget`] runs.
    pub fn arm_cleanup_budget(max_depth: usize, max_entries: usize) {
        CLEANUP_BUDGET.with(|slot| slot.set(Some((max_depth, max_entries))));
    }

    /// Restore the production cleanup budget defaults on the calling thread.
    pub fn clear_cleanup_budget() {
        CLEANUP_BUDGET.with(|slot| slot.set(None));
    }

    pub(super) fn cleanup_budget_override() -> Option<(usize, usize)> {
        CLEANUP_BUDGET.with(std::cell::Cell::get)
    }

    /// Arm one forced staged-write failure of `kind` on this thread.
    pub fn arm_write(kind: io::ErrorKind) {
        WRITE_ARMED.with(|a| a.set(Some(kind)));
    }

    /// Arm one forced staged-`fsync` failure of `kind` on this thread.
    pub fn arm_sync(kind: io::ErrorKind) {
        SYNC_ARMED.with(|a| a.set(Some(kind)));
    }

    /// Arm one forced publication-`rename` failure of `kind` on this thread.
    pub fn arm_rename(kind: io::ErrorKind) {
        RENAME_ARMED.with(|a| a.set(Some(kind)));
    }

    /// Arm one forced descriptor-mode normalization failure on this thread.
    pub fn arm_mode(kind: io::ErrorKind) {
        MODE_ARMED.with(|a| a.set(Some(kind)));
    }

    /// Whether any armed fault or rendezvous is still waiting to be consumed.
    ///
    /// The mechanism half of an injection test: "the writer failed" alone
    /// cannot distinguish "the staged operation ran and surfaced the fault"
    /// from "the writer never reached that operation", because a writer that
    /// skips it leaves the switch armed.
    #[must_use]
    pub fn is_armed() -> bool {
        WRITE_ARMED.with(|a| a.get().is_some())
            || SYNC_ARMED.with(|a| a.get().is_some())
            || RENAME_ARMED.with(|a| a.get().is_some())
            || MODE_ARMED.with(|a| a.get().is_some())
            || PARENT_OPEN_ARMED.with(|a| a.get().is_some())
            || BEFORE_CREATE.with(|a| a.borrow().is_some())
            || AFTER_SYNC.with(|a| a.borrow().is_some())
            || BEFORE_PARENT_SYNC.with(|a| a.borrow().is_some())
            || AFTER_PARENT_SYNC.with(|a| a.borrow().is_some())
            || ROOT_IDENTITY_FAILURE.with(|a| a.borrow().is_some())
            || DIRECTORY_CLEANUP.with(|a| a.borrow().is_some())
    }

    /// Disarm every pending fault or rendezvous on this thread.
    pub fn clear() {
        WRITE_ARMED.with(|a| a.set(None));
        SYNC_ARMED.with(|a| a.set(None));
        RENAME_ARMED.with(|a| a.set(None));
        MODE_ARMED.with(|a| a.set(None));
        PARENT_OPEN_ARMED.with(|a| a.set(None));
        BEFORE_CREATE.with(|a| *a.borrow_mut() = None);
        AFTER_SYNC.with(|a| *a.borrow_mut() = None);
        BEFORE_PARENT_SYNC.with(|a| *a.borrow_mut() = None);
        AFTER_PARENT_SYNC.with(|a| *a.borrow_mut() = None);
        ROOT_IDENTITY_FAILURE.with(|a| *a.borrow_mut() = None);
        DIRECTORY_CLEANUP.with(|a| *a.borrow_mut() = None);
        CLEANUP_BUDGET.with(|a| a.set(None));
    }

    pub(super) fn take_parent_open() -> Option<io::Error> {
        PARENT_OPEN_ARMED.with(|a| {
            a.take()
                .map(|k| io::Error::new(k, "injected retained-parent open failure"))
        })
    }

    /// Consume the pending root-stage identity failure, after running its
    /// substitution action with the newly-created staging basename.
    pub(super) fn take_root_identity_failure(name: &OsStr) -> Option<io::Error> {
        ROOT_IDENTITY_FAILURE.with(|slot| {
            let (kind, action) = slot.borrow_mut().take()?;
            action(name);
            Some(io::Error::new(
                kind,
                "injected staged descriptor identity failure",
            ))
        })
    }

    /// Run and consume the pending pre-create action, if armed.
    pub(super) fn run_before_create(path: &Path) {
        let action = BEFORE_CREATE.with(|a| a.borrow_mut().take());
        if let Some(action) = action {
            action(path);
        }
    }

    /// Run and consume the pending post-`fsync` action, if armed.
    pub(super) fn run_after_sync() {
        let action = AFTER_SYNC.with(|a| a.borrow_mut().take());
        if let Some(action) = action {
            action();
        }
    }

    /// Run and consume the pending pre-parent-sync action, if armed.
    pub(super) fn run_before_parent_sync() {
        let action = BEFORE_PARENT_SYNC.with(|a| a.borrow_mut().take());
        if let Some(action) = action {
            action();
        }
    }

    /// Run and consume the pending post-parent-sync rendezvous, if armed.
    pub(super) fn run_after_parent_sync() {
        let action = AFTER_PARENT_SYNC.with(|a| a.borrow_mut().take());
        if let Some(action) = action {
            action();
        }
    }

    pub(super) fn run_directory_cleanup(path: &Path) {
        let action = DIRECTORY_CLEANUP.with(|slot| {
            let mut armed = slot.borrow_mut();
            if armed.as_ref().is_some_and(|(target, _)| target == path) {
                armed.take().map(|(_, action)| action)
            } else {
                None
            }
        });
        if let Some(action) = action {
            action(path);
        }
    }

    /// Consume a pending forced write failure, if armed.
    pub(super) fn take_write() -> Option<io::Error> {
        WRITE_ARMED.with(|a| {
            a.take()
                .map(|k| io::Error::new(k, "injected staged write failure"))
        })
    }

    /// Consume a pending forced sync failure, if armed.
    pub(super) fn take_sync() -> Option<io::Error> {
        SYNC_ARMED.with(|a| {
            a.take()
                .map(|k| io::Error::new(k, "injected staged fsync failure"))
        })
    }

    /// Consume a pending forced rename failure, if armed.
    pub(super) fn take_rename() -> Option<io::Error> {
        RENAME_ARMED.with(|a| {
            a.take()
                .map(|k| io::Error::new(k, "injected publication rename failure"))
        })
    }

    /// Consume a pending forced descriptor-mode failure, if armed.
    pub(super) fn take_mode() -> Option<io::Error> {
        MODE_ARMED.with(|a| {
            a.take()
                .map(|k| io::Error::new(k, "injected descriptor mode failure"))
        })
    }
}

/// The two reviewed creation modes. Kept private so callers cannot turn the
/// staging owner into an unchecked arbitrary-mode file creator.
#[derive(Clone, Copy)]
enum CreationMode {
    OwnerOnly,
    ReadableDocument,
}

impl CreationMode {
    #[cfg(unix)]
    const fn unix_mode(self) -> u32 {
        match self {
            Self::OwnerOnly => 0o600,
            Self::ReadableDocument => 0o644,
        }
    }
}

// `/tmp` cannot be retained as a private `SecureRoot`; the uid fallback uses
// this exact-path creator and keeps the returned descriptor through cleanup.
pub(crate) fn create_private_exclusive(path: &Path) -> io::Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(crate::platform::libc_o_nofollow());
    }
    options.open(path)
}

/// An exclusively-created temp sibling that unlinks itself unless committed.
///
/// On Unix, creation, publication, cleanup, and directory synchronization stay
/// relative to the parent descriptor retained by the constructor. Write through
/// [`TempStage::file_mut`], then publish with [`TempStage::commit`]; an early
/// `?` or unwind drops the stage and removes its owned temp.
#[derive(Debug)]
pub struct TempStage {
    #[cfg(not(unix))]
    dest_path: PathBuf,
    tmp_path: PathBuf,
    file: Option<File>,
    #[cfg(unix)]
    parent_path: PathBuf,
    #[cfg(unix)]
    parent: File,
    #[cfg(unix)]
    dest_name: OsString,
    #[cfg(unix)]
    tmp_name: OsString,
}

impl TempStage {
    /// Create a unique, private (`0600`), exclusively-created temp sibling of
    /// `dest`.
    ///
    /// # Errors
    /// [`io::ErrorKind::InvalidInput`] when `dest` has no file name, any open
    /// failure — notably [`io::ErrorKind::AlreadyExists`], which with a unique
    /// name means a hostile pre-plant rather than a stale leftover — or a
    /// descriptor permission-normalization failure.
    pub fn create(dest: &Path) -> io::Result<Self> {
        Self::create_with_mode(dest, CreationMode::OwnerOnly)
    }

    /// Create a unique, exclusively-created temp sibling for a user-authored
    /// document whose published contract is exactly `0644`.
    ///
    /// This is deliberately a dedicated constructor, not `create_with_mode(u32)`:
    /// framework state and cache callers cannot accidentally weaken their
    /// `0600` default, while document staging cannot pass a group/world-writable
    /// mode. On Unix the open descriptor is normalized to `0644` before the
    /// first byte is written; no pathname is reopened or chmodded.
    ///
    /// # Errors
    /// The same naming, exclusive-open and descriptor-mode failures as
    /// [`TempStage::create`].
    pub fn create_document(dest: &Path) -> io::Result<Self> {
        Self::create_with_mode(dest, CreationMode::ReadableDocument)
    }

    fn create_with_mode(dest: &Path, mode: CreationMode) -> io::Result<Self> {
        let tmp_path = unique_temp_sibling(dest)?;
        #[cfg(any(test, feature = "fault-injection"))]
        fault::run_before_create(&tmp_path);
        #[cfg(unix)]
        let stage = {
            use std::os::fd::AsFd;

            let parent_path = normalized_parent(dest);
            let parent = open_parent_directory(&parent_path)?;
            let dest_name = dest
                .file_name()
                .expect("unique_temp_sibling validated the destination")
                .to_os_string();
            let tmp_name = tmp_path
                .file_name()
                .expect("a temp sibling has a file name")
                .to_os_string();
            let file = crate::secure_dir::create_entry_private_at(parent.as_fd(), &tmp_name)?;
            Self {
                tmp_path,
                file: Some(file),
                parent_path,
                parent,
                dest_name,
                tmp_name,
            }
        };
        #[cfg(not(unix))]
        let stage = Self {
            dest_path: dest.to_path_buf(),
            file: Some(create_private_exclusive(&tmp_path)?),
            tmp_path,
        };
        stage.normalize_mode(mode)?;
        #[cfg(unix)]
        stage.ensure_parent_reachable()?;
        Ok(stage)
    }

    #[cfg(unix)]
    fn ensure_parent_reachable(&self) -> io::Result<()> {
        ensure_path_resolves_to_file(&self.parent_path, &self.parent)
    }

    fn normalize_mode(&self, mode: CreationMode) -> io::Result<()> {
        #[cfg(any(test, feature = "fault-injection"))]
        if let Some(error) = fault::take_mode() {
            return Err(error);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file = self.file.as_ref().expect("staged temp file is open");
            file.set_permissions(std::fs::Permissions::from_mode(mode.unix_mode()))
        }
        #[cfg(not(unix))]
        {
            let _ = mode;
            Ok(())
        }
    }

    /// The staged temp's path. Useful for diagnostics; the file itself should
    /// be reached through [`TempStage::file_mut`] so no second path resolution
    /// is introduced.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.tmp_path
    }

    /// The open descriptor for the staged temp.
    ///
    /// # Panics
    /// Never: the handle is only taken by [`TempStage::commit`], which consumes
    /// `self`.
    pub fn file_mut(&mut self) -> &mut File {
        self.file.as_mut().expect("staged temp file is open")
    }

    /// Write `bytes` into the staged temp.
    ///
    /// The whole-buffer counterpart of [`TempStage::file_mut`], and the seam a
    /// test uses to force the *write* half of a publication to fail.
    ///
    /// # Errors
    /// Any [`Write::write_all`] failure. The stage is left uncommitted, so the
    /// caller's `?` unlinks the temp on the way out.
    ///
    /// # Panics
    /// Never: the handle is only taken by [`TempStage::commit`].
    pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        let file = self.file.as_mut().expect("staged temp file is open");
        write_staged(file, bytes)
    }

    /// `fsync` the staged data, atomically replace the destination supplied to
    /// [`TempStage::create`], then `fsync` the parent directory.
    ///
    /// On Unix, the parent directory is retained from creation through
    /// publication, cleanup, and synchronization. The parent pathname must
    /// still resolve to that directory before publication and before success is
    /// reported. The staged entry and published destination are revalidated
    /// against the held file descriptor, so a replacement observed at either
    /// check is refused.
    ///
    /// On an ordinary write/`fsync`/`rename` failure before publication the
    /// original temp is removed (by the drop that follows the early return) so
    /// a partial private file is never left behind. That cleanup is
    /// identity-bound, though: the drop unlinks the staging name only while it
    /// still denotes the exclusively-created inode, so when the revalidating
    /// `stat` is inconclusive it fails closed and leaves the staged inode rather
    /// than risk unlinking a substituted name. Once rename succeeds the staging
    /// pathname is cleared before the parent sync, so a barrier or final-check
    /// failure cannot make [`Drop`] unlink the published destination.
    ///
    /// POSIX has no rename-by-inode, so a same-user actor can still substitute
    /// an entry between its identity check and the name-based syscall. The final
    /// check narrows that race but does not grant permanent ownership of a
    /// pathname; a caller needing continued identity must retain and revalidate
    /// its own descriptor.
    ///
    /// # Errors
    /// A propagated `sync_all`, identity-validation, `renameat`, or
    /// (non-tolerated) retained-parent sync failure. A parent or destination
    /// replaced before the final check is an [`io::ErrorKind::PermissionDenied`]
    /// identity refusal. Durability errors are surfaced rather than swallowed;
    /// see [`crate::durable::sync_parent_dir`] for the tolerance policy.
    pub fn commit(mut self) -> io::Result<()> {
        let Some(file) = self.file.as_ref() else {
            return Err(io::Error::other("the staged file was already closed"));
        };
        sync_staged(file)?;
        #[cfg(any(test, feature = "fault-injection"))]
        fault::run_after_sync();
        #[cfg(unix)]
        {
            self.ensure_parent_reachable()?;
            ensure_entry_names_file(&self.parent, &self.tmp_name, file)?;
            rename_staged_at(&self.parent, &self.tmp_name, &self.dest_name)?;
            ensure_entry_names_file(&self.parent, &self.dest_name, file)?;

            // Publication and cleanup stay relative to this retained parent,
            // so replacing its pathname cannot redirect either operation.
            self.tmp_path = PathBuf::new();
            self.tmp_name = OsString::new();
            #[cfg(any(test, feature = "fault-injection"))]
            fault::run_before_parent_sync();
            crate::durable::sync_retained_dir(&self.parent, &self.parent_path)?;
            #[cfg(any(test, feature = "fault-injection"))]
            fault::run_after_parent_sync();
            self.ensure_parent_reachable()?;
            ensure_entry_names_file(&self.parent, &self.dest_name, file)?;
        }
        #[cfg(not(unix))]
        {
            ensure_path_names_file(&self.tmp_path, file)?;
            rename_staged(&self.tmp_path, &self.dest_path)?;
            ensure_path_names_file(&self.dest_path, file)?;
            self.tmp_path = PathBuf::new();
            #[cfg(any(test, feature = "fault-injection"))]
            fault::run_before_parent_sync();
            crate::durable::sync_parent_dir(&self.dest_path)?;
            #[cfg(any(test, feature = "fault-injection"))]
            fault::run_after_parent_sync();
            ensure_path_names_file(&self.dest_path, file)?;
        }
        self.file.take();
        Ok(())
    }
}

// Cleanup fails closed at this recursion depth, retaining the ancestor.
// `mkdirat` chains inside a stage are not bounded by `PATH_MAX`.
#[cfg(unix)]
const MAX_CLEANUP_DEPTH: usize = 64;

// Cleanup fails closed once one Drop observes this many entries, bounding a
// shallow tree an attacker inflated horizontally.
#[cfg(unix)]
const MAX_CLEANUP_ENTRIES: usize = 65_536;

#[cfg(unix)]
fn cleanup_budget() -> (usize, usize) {
    #[cfg(any(test, feature = "fault-injection"))]
    if let Some(over) = fault::cleanup_budget_override() {
        return over;
    }
    (MAX_CLEANUP_DEPTH, MAX_CLEANUP_ENTRIES)
}

/// An exclusively-created private directory whose Drop cleanup stays bound to
/// retained directory descriptors.
///
/// Create the stage beneath an already verified
/// [`SecureRoot`](crate::secure_dir::SecureRoot). Drop opens descendants
/// descriptor-relatively without following symlinks and removes a name whose
/// current `fstatat` identity through the parent descriptor still equals the
/// identity observed at retention. That per-entry check is the same strict
/// narrowing [`SecureRoot::unlink_entry_if_identity`](crate::secure_dir::SecureRoot::unlink_entry_if_identity)
/// documents — a residual `fstatat`→`unlinkat` window remains because POSIX
/// has no unlink-by-inode — so what Drop unlinks is a name that denoted the
/// retained identity microseconds earlier, not any name at all. A subtree
/// whose identity changed after retention is not traversed, is not removed,
/// and its owned ancestor is retained. Cleanup also retains the stage if its
/// root becomes foreign-owned or group/other-writable, and refuses to descend
/// beyond 64 levels or observe more than 65 536 entries per Drop.
#[cfg(unix)]
#[derive(Debug)]
pub struct DirectoryStage {
    path: PathBuf,
    parent_fd: std::os::fd::OwnedFd,
    name: OsString,
    root: crate::secure_dir::SecureRoot,
    cleaned: bool,
}

#[cfg(unix)]
impl DirectoryStage {
    /// Exclusively create a mode-`0700` directory named `name` inside `parent`.
    ///
    /// # Errors
    /// Returns an error when `name` is not one basename, the parent identity
    /// changed, or any descriptor-relative create, open, identity check, or
    /// permission normalization fails.
    pub fn create(parent: &crate::secure_dir::SecureRoot, name: &OsStr) -> io::Result<Self> {
        use rustix::fs::{AtFlags, Mode, OFlags};
        use std::os::unix::ffi::OsStrExt;

        let bytes = name.as_bytes();
        if bytes.is_empty() || bytes.contains(&b'/') || bytes == b"." || bytes == b".." {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "directory-stage name must be one basename",
            ));
        }
        parent
            .revalidate()
            .map_err(|error| io::Error::new(io::ErrorKind::PermissionDenied, error.to_string()))?;
        let parent_fd = rustix::fs::openat(
            parent.retained_fd(),
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        rustix::fs::mkdirat(&parent_fd, name, Mode::RUSR | Mode::WUSR | Mode::XUSR)
            .map_err(io::Error::from)?;

        let created = match rustix::fs::statat(&parent_fd, name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(error) => return Err(io::Error::from(error)),
        };
        let identity = (created.st_dev, created.st_ino);
        let child_fd = match rustix::fs::openat(
            &parent_fd,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(error) => {
                let _ = remove_directory_name_if_identity(&parent_fd, name, identity);
                return Err(io::Error::from(error));
            }
        };
        let opened = match rustix::fs::fstat(&child_fd) {
            Ok(stat) => stat,
            Err(error) => {
                let _ = remove_directory_name_if_identity(&parent_fd, name, identity);
                return Err(io::Error::from(error));
            }
        };
        if (opened.st_dev, opened.st_ino) != identity {
            let _ = remove_directory_name_if_identity(&parent_fd, name, identity);
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "created directory changed while its descriptor was retained",
            ));
        }
        if let Err(error) = rustix::fs::fchmod(&child_fd, Mode::RUSR | Mode::WUSR | Mode::XUSR) {
            let _ = remove_directory_name_if_identity(&parent_fd, name, identity);
            return Err(io::Error::from(error));
        }

        let path = parent.path().join(name);
        let root =
            crate::secure_dir::SecureRoot::verify_private_descriptor(path.clone(), child_fd)?;
        Ok(Self {
            path,
            parent_fd,
            name: name.to_os_string(),
            root,
            cleaned: false,
        })
    }

    /// The pathname applications may hand to a subprocess or native service.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The retained root used for descriptor-relative stage operations.
    #[must_use]
    pub const fn root(&self) -> &crate::secure_dir::SecureRoot {
        &self.root
    }

    /// Attempt to remove all owned descendants and the root directory itself.
    ///
    /// Returns `Ok(true)` only when every retained descendant and the exact
    /// root name were removed. Returns `Ok(false)` when identity substitution,
    /// policy drift, or a cleanup bound (64-depth / 65 536-entry) leaves
    /// anything retained. Returns `Err` for a distinguishable descriptor or
    /// I/O failure during the final root removal.
    ///
    /// On `Ok(false)` or `Err`, all descriptors, the root name and the
    /// retained identity remain live so the caller can retry. A partially
    /// completed `false` or `Err` never removes a planted replacement. After
    /// `Ok(true)`, [`Drop`] is an inert no-op.
    ///
    /// # Errors
    /// An `unlinkat` or `fstatat` I/O failure during the final root-directory
    /// removal, distinguishable from the `Ok(false)` identity/budget cases.
    pub fn try_cleanup(&mut self) -> io::Result<bool> {
        if self.cleaned {
            return Ok(true);
        }

        #[cfg(any(test, feature = "fault-injection"))]
        fault::run_directory_cleanup(&self.path);

        if self.root.verify_retained_policy().is_err() {
            return Ok(false);
        }

        use std::os::fd::AsFd;
        if !directory_name_matches(self.parent_fd.as_fd(), &self.name, self.root.identity()) {
            return Ok(false);
        }

        let (max_depth, max_entries) = cleanup_budget();
        let mut entry_budget = max_entries;
        if !clean_directory_contents(
            self.root.retained_fd(),
            &self.path,
            0,
            max_depth,
            &mut entry_budget,
        ) {
            return Ok(false);
        }

        match remove_directory_name_if_identity(&self.parent_fd, &self.name, self.root.identity()) {
            Ok(true) => {
                self.cleaned = true;
                Ok(true)
            }
            Ok(false) => Ok(false),
            Err(e) => Err(e),
        }
    }
}

#[cfg(unix)]
impl Drop for DirectoryStage {
    fn drop(&mut self) {
        let _ = self.try_cleanup();
    }
}

/// Removes the directory `name` in `parent` and everything beneath it, only
/// while the name still denotes `identity`, with [`DirectoryStage`]'s
/// identity checks and bounds. `Ok(false)` when anything was retained.
#[cfg(unix)]
pub(crate) fn remove_tree_if_identity(
    parent: std::os::fd::BorrowedFd<'_>,
    name: &OsStr,
    identity: (u64, u64),
    path: &Path,
) -> io::Result<bool> {
    use rustix::fs::{Mode, OFlags};
    use std::os::fd::AsFd;

    if !directory_name_matches(parent, name, identity) {
        return Ok(false);
    }
    let child = rustix::fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io::Error::from)?;
    let opened = rustix::fs::fstat(&child).map_err(io::Error::from)?;
    if (opened.st_dev, opened.st_ino) != identity {
        return Ok(false);
    }
    let (max_depth, max_entries) = cleanup_budget();
    let mut entry_budget = max_entries;
    if !clean_directory_contents(child.as_fd(), path, 0, max_depth, &mut entry_budget) {
        return Ok(false);
    }
    remove_directory_name_if_identity(parent, name, identity)
}

#[cfg(unix)]
fn clean_directory_contents(
    directory: std::os::fd::BorrowedFd<'_>,
    path: &Path,
    depth: usize,
    max_depth: usize,
    entry_budget: &mut usize,
) -> bool {
    use rustix::fs::{AtFlags, Dir, FileType, Mode, OFlags};
    use std::os::fd::AsFd;
    use std::os::unix::ffi::OsStringExt;

    if depth >= max_depth {
        return false;
    }
    let Ok(duplicate) = rustix::fs::openat(
        directory,
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return false;
    };
    let Ok(mut entries) = Dir::read_from(duplicate) else {
        return false;
    };
    let mut names = Vec::new();
    while let Some(entry) = entries.read() {
        let Ok(entry) = entry else {
            return false;
        };
        let bytes = entry.file_name().to_bytes();
        if bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
            continue;
        }
        if *entry_budget == 0 {
            return false;
        }
        *entry_budget -= 1;
        names.push(OsString::from_vec(bytes.to_vec()));
    }

    let mut complete = true;
    for name in names {
        let observed = match rustix::fs::statat(directory, &name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(rustix::io::Errno::NOENT) => continue,
            Err(_) => {
                complete = false;
                continue;
            }
        };
        let identity = (observed.st_dev, observed.st_ino);
        if FileType::from_raw_mode(observed.st_mode) == FileType::Directory {
            let Ok(child) = rustix::fs::openat(
                directory,
                &name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) else {
                complete = false;
                continue;
            };
            let opened = match rustix::fs::fstat(&child) {
                Ok(stat) => (stat.st_dev, stat.st_ino),
                Err(_) => {
                    complete = false;
                    continue;
                }
            };
            if opened != identity {
                complete = false;
                continue;
            }
            let child_path = path.join(&name);
            #[cfg(any(test, feature = "fault-injection"))]
            fault::run_directory_cleanup(&child_path);
            if !directory_name_matches(directory, &name, identity) {
                complete = false;
                continue;
            }
            if !clean_directory_contents(
                child.as_fd(),
                &child_path,
                depth + 1,
                max_depth,
                entry_budget,
            ) {
                complete = false;
                continue;
            }
            match remove_directory_name_if_identity(directory, &name, identity) {
                Ok(true) => {}
                Ok(false) => complete = false,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(_) => complete = false,
            }
        } else if !entry_name_matches(directory, &name, identity)
            || rustix::fs::unlinkat(directory, &name, AtFlags::empty()).is_err()
        {
            complete = false;
        }
    }
    complete
}

#[cfg(unix)]
fn directory_name_matches(
    parent: std::os::fd::BorrowedFd<'_>,
    name: &OsStr,
    identity: (u64, u64),
) -> bool {
    use rustix::fs::{AtFlags, FileType};
    matches!(
        rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW),
        Ok(stat)
            if FileType::from_raw_mode(stat.st_mode) == FileType::Directory
                && (stat.st_dev, stat.st_ino) == identity
    )
}

#[cfg(unix)]
fn entry_name_matches(
    parent: std::os::fd::BorrowedFd<'_>,
    name: &OsStr,
    identity: (u64, u64),
) -> bool {
    use rustix::fs::AtFlags;
    matches!(
        rustix::fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW),
        Ok(stat) if (stat.st_dev, stat.st_ino) == identity
    )
}

#[cfg(unix)]
fn remove_directory_name_if_identity(
    parent: impl std::os::fd::AsFd,
    name: &OsStr,
    identity: (u64, u64),
) -> io::Result<bool> {
    use rustix::fs::AtFlags;
    let parent = parent.as_fd();
    if !directory_name_matches(parent, name, identity) {
        return Ok(false);
    }
    rustix::fs::unlinkat(parent, name, AtFlags::REMOVEDIR).map_err(io::Error::from)?;
    Ok(true)
}

/// A private, exclusively-created temp entry of a [`SecureRoot`](crate::secure_dir::SecureRoot) that
/// unlinks itself unless it is committed.
///
/// The descriptor-relative twin of [`TempStage`], for a writer that has already
/// paid for a verified, retained directory descriptor. It is a separate type
/// rather than a mode of [`TempStage`] because the two differ in exactly one
/// respect that must not be blurred: this one never resolves a pathname.
/// Creation is `openat(root_fd, tmp, O_CREAT | O_EXCL | O_WRONLY | O_NOFOLLOW,
/// 0600)` and publication is `renameat(root_fd, tmp, root_fd, name)`, so a
/// directory substituted at the root's *pathname* after adoption cannot
/// redirect the write — a guarantee a `&Path`-based stage cannot make, and one
/// this shared owner must therefore not silently downgrade.
///
/// What is shared with [`TempStage`] is everything that was being re-derived:
/// the per-call unique staging name ([`unique_temp_file_name`]), the
/// owner-only exclusive creation, the write/`fsync`/publish ladder with its
/// fault seams, and—after descriptor identity has been acquired—the [`Drop`]
/// that unlinks the staging entry on an ordinary failure path that is not a
/// successful [`RootTempStage::commit`]. That cleanup is identity-bound both
/// ways: if the initial descriptor `fstat` fails there is no identity with
/// which to authorize cleanup, so creation fails closed without unlinking
/// whatever the staging name currently denotes; and if a later revalidation
/// cannot read the entry's identity it likewise leaves the owner-only entry
/// rather than unlink a name that may now denote a foreign inode.
///
/// # Durability
/// Publication is a `renameat` within one directory; the directory-entry
/// barrier that would make it survive a power loss belongs to the caller,
/// which knows whether its artefact is re-derivable (a re-fetchable HTTP cache
/// entry) or not (a state file, which stages through [`TempStage`] and gets
/// [`crate::durable::sync_parent_dir`] inside [`TempStage::commit`]). This type
/// deliberately adds no barrier of its own rather than silently changing the
/// I/O profile of a cache write.
#[cfg(unix)]
#[derive(Debug)]
pub struct RootTempStage<'root> {
    root: &'root crate::secure_dir::SecureRoot,
    dest_name: OsString,
    tmp_name: OsString,
    /// `(device, inode)` of the entry this stage exclusively created, from
    /// `fstat` on the held descriptor. Publication and cleanup both refuse to
    /// act on `tmp_name` unless it still denotes this identity.
    identity: (u64, u64),
    file: Option<File>,
}

#[cfg(unix)]
impl<'root> RootTempStage<'root> {
    /// Create a unique, private (`0600`), exclusively-created staging entry for
    /// `dest_name` inside `root`.
    ///
    /// # Errors
    /// Any `openat` or held-descriptor `fstat` failure. An
    /// [`io::ErrorKind::AlreadyExists`] from the exclusive create means a
    /// hostile pre-plant rather than a stale leftover and is therefore
    /// refused, never unlinked and retried. An `fstat` failure likewise
    /// performs no name-only cleanup because no known identity exists to prove
    /// the current name is still ours.
    pub fn create(
        root: &'root crate::secure_dir::SecureRoot,
        dest_name: &OsStr,
    ) -> io::Result<Self> {
        let tmp_name = unique_temp_file_name(dest_name);
        let file = root.create_entry_private(&tmp_name)?;
        // Bind the name to the inode now, while `O_EXCL` still guarantees the
        // entry is the one this call created. If `fstat` fails there is no
        // known held identity with which cleanup can be authorised. Fail
        // closed and leave the current name alone: it may have been
        // substituted after creation, and name-only unlink would delete a
        // foreign entry. Closing `file` still releases our descriptor.
        #[cfg(any(test, feature = "fault-injection"))]
        if let Some(error) = fault::take_root_identity_failure(&tmp_name) {
            return Err(error);
        }
        let identity = crate::secure_dir::open_file_identity(&file)?;
        Ok(Self {
            root,
            dest_name: dest_name.to_os_string(),
            tmp_name,
            identity,
            file: Some(file),
        })
    }

    /// The staging entry's name within the root. Diagnostics only.
    #[must_use]
    pub fn name(&self) -> &OsStr {
        &self.tmp_name
    }

    /// The `(device, inode)` this stage exclusively created and publishes.
    ///
    /// A `renameat` moves a name, not an inode, so after a successful
    /// [`RootTempStage::commit`] this identity is what the destination name
    /// denotes. A caller that must later undo its own publication passes it to
    /// [`SecureRoot::unlink_entry_if_identity`](crate::secure_dir::SecureRoot::unlink_entry_if_identity)
    /// so the rollback cannot remove a concurrent writer's newer replacement.
    #[must_use]
    pub fn identity(&self) -> (u64, u64) {
        self.identity
    }

    /// Write `bytes` into the staged entry.
    ///
    /// # Errors
    /// Any [`Write::write_all`] failure; the stage stays uncommitted, so the
    /// caller's `?` unlinks the staging entry on the way out.
    ///
    /// # Panics
    /// Never: the handle is only taken by [`RootTempStage::commit`].
    pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        let file = self.file.as_mut().expect("staged temp entry is open");
        write_staged(file, bytes)
    }

    /// `fsync` the staged data, then `renameat` it over the destination name
    /// supplied to [`RootTempStage::create`], both relative to the retained
    /// root descriptor.
    ///
    /// On an ordinary write/`fsync`/`renameat` failure before publication the
    /// staging entry is unlinked (by the drop that follows the early return),
    /// so a partial private file is never left behind and the previous
    /// destination survives untouched. That cleanup is identity-bound: it
    /// unlinks the staging name only while it still denotes the
    /// exclusively-created entry, so when the revalidating `fstatat` is
    /// inconclusive it fails closed and leaves the owner-only entry rather than
    /// unlink a name that may now denote a foreign inode. On success the
    /// staging name no longer exists — it *is* the destination — so the drop
    /// guard is disarmed.
    ///
    /// # Identity
    /// `renameat` publishes whatever the *name* denotes at the instant it
    /// runs, and the `fsync` above is an unbounded window in which another
    /// process can unlink the staging entry and plant its own file at that
    /// name. Renaming it would publish a foreign inode as the destination —
    /// exactly the outcome the destination's previous contents are supposed to
    /// survive. So the name is re-bound to the held descriptor's `(device,
    /// inode)` through the retained root fd (never by re-resolving a pathname)
    /// immediately before the rename, and a mismatch is
    /// [`io::ErrorKind::PermissionDenied`]: the old destination is left in
    /// place, the substitute is left untouched (the identity-bound [`Drop`]
    /// will not remove it either), and the staged inode this call created is
    /// never published. That inode is reclaimed when the descriptor closes if
    /// the substituter unlinked its name; if they *renamed* it instead, it
    /// survives under a name they chose, still `0600` and still owned by the process's uid —
    /// which only a process of the same uid could arrange, since the root was
    /// verified owner-only and not group/world-writable before adoption.
    /// The residual window between the check and the `renameat` cannot be
    /// closed by POSIX — there is no rename-by-inode — so this is a strict
    /// narrowing, not an atomic guarantee.
    ///
    /// # Errors
    /// A propagated `sync_all`, `fstatat` or `renameat` failure, or
    /// [`io::ErrorKind::PermissionDenied`] when the staging name no longer
    /// denotes the entry this stage created.
    ///
    /// # Panics
    /// Never: the handle is taken only here.
    pub fn commit(mut self) -> io::Result<()> {
        {
            let file = self.file.as_ref().expect("staged temp entry is open");
            sync_staged(file)?;
        }
        #[cfg(any(test, feature = "fault-injection"))]
        fault::run_after_sync();
        #[cfg(any(test, feature = "fault-injection"))]
        if let Some(e) = fault::take_rename() {
            return Err(e);
        }
        if self.root.entry_identity(&self.tmp_name)? != self.identity {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "staged entry name no longer denotes the exclusively-created file",
            ));
        }
        self.root.rename_entry(&self.tmp_name, &self.dest_name)?;

        // Published: the staging name no longer exists, so the drop guard must
        // not unlink it (and must never unlink the destination).
        self.tmp_name = OsString::new();
        self.file.take();
        Ok(())
    }
}

#[cfg(unix)]
impl Drop for RootTempStage<'_> {
    fn drop(&mut self) {
        // Disarmed by a successful `commit`, which clears the staging name.
        if self.tmp_name.is_empty() {
            return;
        }
        // Only remove the name while it still denotes the entry this stage
        // created: a file, symlink or directory substituted at the staging
        // name during the write belongs to someone else and is left alone.
        // The descriptor stays open across the check so the inode number it
        // was compared against cannot have been recycled.
        let _ = self
            .root
            .unlink_entry_if_identity(&self.tmp_name, self.identity);
        self.file.take();
    }
}

/// Publish the held inode at a new name without replacing any existing entry.
///
/// Linux resolves the source through `/proc/self/fd`, so a substituted source
/// pathname cannot be published. The descriptor must retain a linked inode.
///
/// # Errors
/// Returns `AlreadyExists` for occupied destinations, `Unsupported` off Linux,
/// or a `linkat` failure, including unavailable procfs or an unlinked source.
#[cfg(target_os = "linux")]
pub fn link_new_by_identity(file: &File, dest: &Path) -> io::Result<()> {
    link_new_at_by_identity(file, rustix::fs::CWD, dest)
}

#[cfg(target_os = "linux")]
pub(crate) fn link_new_at_by_identity(
    file: &File,
    destination: impl std::os::fd::AsFd,
    name: impl rustix::path::Arg,
) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let source = format!("/proc/self/fd/{}", file.as_raw_fd());
    rustix::fs::linkat(
        rustix::fs::CWD,
        source,
        destination,
        name,
        rustix::fs::AtFlags::SYMLINK_FOLLOW,
    )
    .map_err(io::Error::from)
}

/// Publish the open `file` under `dest` as a new name, never clobbering.
///
/// # Errors
/// Always [`io::ErrorKind::Unsupported`]: see the [Linux
/// implementation](fn.link_new_by_identity.html) for why this fails closed
/// rather than degrading to a path-to-path `link(2)`.
#[cfg(not(target_os = "linux"))]
pub fn link_new_by_identity(_file: &File, _dest: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "identity-bound no-clobber publication requires Linux",
    ))
}

/// Verify that `path` still names the inode behind the open `file`.
///
/// This is the read-only companion to [`link_new_by_identity`]. Call it after
/// any blocking post-publication operation (notably a parent-directory
/// `fsync`) before reporting the pathname as a successful publication.
///
/// # Errors
/// On Unix, [`io::ErrorKind::PermissionDenied`] when the identities differ, or
/// a metadata error. On non-Unix targets, [`io::ErrorKind::Unsupported`]
/// because no portable file identity is available.
pub fn verify_path_names_file(file: &File, path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        ensure_path_names_file(path, file)
    }
    #[cfg(not(unix))]
    {
        let _ = (file, path);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "path identity verification requires Unix",
        ))
    }
}

/// Unlink `path`. On Unix, only while it still names the very `file` this
/// transaction created; on a non-Unix target there is no portable inode
/// identity to compare and this reduces to [`std::fs::remove_file`].
///
/// Any exclusive-create-then-clean-up flow has the same window: the file is
/// created `O_EXCL`, some blocking work happens (a copy, an `fsync`), and the
/// cleanup then removes the *pathname*. If the name was unlinked and replaced
/// in that window — with another file or with a symlink to one — the cleanup
/// deletes an inode (or a link to one) the transaction never created. Binding
/// the removal to `(device, inode)` of the held descriptor turns that into a
/// refusal.
///
/// The residual window between the `lstat` and the `unlink(2)` cannot be closed
/// by POSIX (there is no unlink-by-inode), so this is a strict narrowing, not
/// an atomic guarantee: what it removes is always a name that named the staged inode
/// microseconds earlier, rather than any name at all. Keep `file` open until
/// after this call returns; a dropped descriptor has no identity to compare.
///
/// # Errors
/// On Unix, [`io::ErrorKind::PermissionDenied`] when `path` no longer names
/// `file`, or any `stat`/`unlink` failure. On non-Unix targets, only the
/// underlying [`std::fs::remove_file`] error, if any.
pub fn remove_if_path_names_file(path: &Path, file: &File) -> io::Result<()> {
    ensure_path_names_file(path, file)?;
    std::fs::remove_file(path)
}

/// Unlinks one entry through a retained parent after verifying that it names
/// `file`.
///
/// This prevents parent-path redirection but retains the POSIX check-to-unlink
/// window documented by [`remove_if_path_names_file`].
///
/// # Errors
/// Returns an identity mismatch or an underlying descriptor-relative metadata
/// or unlink error.
#[cfg(unix)]
pub fn remove_if_parent_entry_names_file(
    parent: &File,
    name: &OsStr,
    file: &File,
) -> io::Result<()> {
    ensure_entry_names_file(parent, name, file)?;
    rustix::fs::unlinkat(parent, name, rustix::fs::AtFlags::empty()).map_err(io::Error::from)
}

#[cfg(unix)]
fn normalized_parent(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if parent.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        parent.to_path_buf()
    }
}

#[cfg(unix)]
fn open_parent_directory(path: &Path) -> io::Result<File> {
    #[cfg(any(test, feature = "fault-injection"))]
    if let Some(error) = fault::take_parent_open() {
        return Err(io::Error::new(
            error.kind(),
            format!(
                "open parent directory {} for staged publication: {error}",
                path.display()
            ),
        ));
    }
    let fd = rustix::fs::openat(
        rustix::fs::CWD,
        path,
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::DIRECTORY | rustix::fs::OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(io::Error::from)?;
    Ok(File::from(fd))
}

#[cfg(unix)]
fn ensure_path_resolves_to_file(path: &Path, file: &File) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let held = file.metadata()?;
    let named = std::fs::metadata(path)?;
    if held.dev() != named.dev() || held.ino() != named.ino() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "parent path no longer resolves to the retained directory",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn ensure_entry_names_file(parent: &File, name: &OsStr, file: &File) -> io::Result<()> {
    use std::os::fd::AsFd;

    let held = crate::secure_dir::open_file_identity(file)?;
    let named = crate::secure_dir::entry_identity_at(parent.as_fd(), name)?;
    if held != named {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "staged entry no longer names the exclusively-created file",
        ));
    }
    Ok(())
}

/// Whether `path` still resolves to the exact inode behind `file`.
///
/// # Errors
/// [`io::ErrorKind::PermissionDenied`] on a mismatch, or a `stat` failure.
#[cfg(unix)]
fn ensure_path_names_file(path: &Path, file: &File) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let held = file.metadata()?;
    let named = std::fs::symlink_metadata(path)?;
    if held.dev() != named.dev() || held.ino() != named.ino() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "staged path no longer names the exclusively-created file",
        ));
    }
    Ok(())
}

/// Whether `path` still resolves to the exact inode behind `file`.
///
/// # Errors
/// Never on a non-Unix target: there is no portable inode identity to compare.
#[cfg(not(unix))]
fn ensure_path_names_file(_path: &Path, _file: &File) -> io::Result<()> {
    Ok(())
}

impl Drop for TempStage {
    fn drop(&mut self) {
        // Disarmed by a successful `commit`, which clears the path.
        if self.tmp_path.as_os_str().is_empty() {
            return;
        }
        #[cfg(unix)]
        if let Some(file) = self.file.as_ref() {
            use std::os::fd::AsFd;

            if let Ok(identity) = crate::secure_dir::open_file_identity(file) {
                let _ = crate::secure_dir::unlink_entry_if_identity_at(
                    self.parent.as_fd(),
                    &self.tmp_name,
                    identity,
                    rustix::fs::AtFlags::empty(),
                );
            }
        }
        #[cfg(not(unix))]
        if let Some(file) = self.file.as_ref() {
            let _ = remove_if_path_names_file(&self.tmp_path, file);
        }
        self.file.take();
    }
}

#[cfg(test)]
mod tests;
