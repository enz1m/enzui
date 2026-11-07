//! Tracks regular-file metadata changes against per-watcher baselines.
//!
//! Probes run through an injected backend. `FileWatcher` rejects results from
//! another watcher or an already-admitted generation, and callers must
//! acknowledge their own saves or deletions to advance the baseline. A caller
//! that already loaded a document should seed its exact load-time identity with
//! [`FileWatcher::with_baseline`]; an unseeded first probe cannot prove that the
//! file stayed unchanged since the load.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A metadata snapshot used to compare a file across probes.
///
/// Metadata-only monitoring cannot distinguish a write or inode reuse that
/// preserves every represented field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileIdentity {
    /// `(dev, ino)` — an `(0, 0)` sentinel on non-Unix.
    pub inode: (u64, u64),
    /// Whole seconds since the Unix epoch. `i64` because pre-epoch mtimes
    /// exist and are legal.
    pub mtime_secs: i64,
    /// Subsecond nanoseconds; metadata-derived values are normalized to `0..1_000_000_000`.
    pub mtime_nsec: u32,
    /// Whole seconds of the Unix metadata-change timestamp, or `0` when unavailable.
    pub ctime_secs: i64,
    /// Subsecond metadata-change nanoseconds, or `0` when unavailable.
    pub ctime_nsec: u32,
    /// File size in bytes.
    pub len: u64,
}

impl FileIdentity {
    /// Constructs a synthetic identity without a distinct metadata-change timestamp.
    #[must_use]
    pub fn new(inode: (u64, u64), mtime_secs: i64, mtime_nsec: u32, len: u64) -> Self {
        Self::new_with_change_time(inode, mtime_secs, mtime_nsec, mtime_secs, mtime_nsec, len)
    }

    /// Constructs an identity with separate modification and metadata-change timestamps.
    #[must_use]
    pub fn new_with_change_time(
        inode: (u64, u64),
        mtime_secs: i64,
        mtime_nsec: u32,
        ctime_secs: i64,
        ctime_nsec: u32,
        len: u64,
    ) -> Self {
        Self {
            inode,
            mtime_secs,
            mtime_nsec,
            ctime_secs,
            ctime_nsec,
            len,
        }
    }

    /// Reads the identity out of an [`std::fs::Metadata`].
    #[cfg(unix)]
    #[must_use]
    pub fn from_metadata(md: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        let normalize_nsec = |nsec: i64| {
            if (0..1_000_000_000).contains(&nsec) {
                nsec as u32
            } else {
                0
            }
        };
        Self {
            inode: (md.dev(), md.ino()),
            mtime_secs: md.mtime(),
            mtime_nsec: normalize_nsec(md.mtime_nsec()),
            ctime_secs: md.ctime(),
            ctime_nsec: normalize_nsec(md.ctime_nsec()),
            len: md.len(),
        }
    }

    /// Builds an identity from metadata, using `(0, 0)` for the device and inode on
    /// non-Unix targets.
    #[cfg(not(unix))]
    #[must_use]
    pub fn from_metadata(md: &std::fs::Metadata) -> Self {
        // Preserve pre-epoch modification times by converting the error duration with a
        // negative sign.
        let (secs, nsec) = match md.modified() {
            Ok(t) => match t.duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => (d.as_secs() as i64, d.subsec_nanos()),
                Err(e) => {
                    let d = e.duration();
                    // Normalize negative fractional times by rounding seconds toward negative infinity.
                    if d.subsec_nanos() == 0 {
                        (-(d.as_secs() as i64), 0)
                    } else {
                        (-(d.as_secs() as i64) - 1, 1_000_000_000 - d.subsec_nanos())
                    }
                }
            },
            Err(_) => (0, 0),
        };
        Self {
            inode: (0, 0),
            mtime_secs: secs,
            mtime_nsec: nsec,
            ctime_secs: 0,
            ctime_nsec: 0,
            len: md.len(),
        }
    }
}

/// The typed lifecycle outcome of one probe.
#[must_use = "file-watch outcomes carry changes and failures that must be handled"]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileWatchOutcome {
    /// No baseline was held, so this observation was installed without comparison.
    ///
    /// A present value cannot prove that a file loaded earlier remained unchanged;
    /// seed that load-time identity with [`FileWatcher::with_baseline`] instead.
    NoBaseline(Option<FileIdentity>),
    /// The observed identity matched the baseline exactly. The steady state.
    Unchanged,
    /// The baseline and observation share `(dev, ino)` but differ in modification time,
    /// metadata-change time, or length.
    Modified {
        /// The baseline that was overwritten.
        previous: FileIdentity,
        /// The newly observed identity.
        current: FileIdentity,
    },
    /// The file identity changed, as occurs when temp-file replacement assigns a new `(dev,
    /// ino)`.
    /// Call [`FileWatcher::acknowledge`] after an intentional replacement.
    Replaced {
        /// The baseline that was replaced.
        previous: FileIdentity,
        /// The new file at this path.
        current: FileIdentity,
    },
    /// A previously present file is now absent.
    Deleted {
        /// The baseline that vanished.
        previous: FileIdentity,
    },
    /// The file was absent, and is now present. Informational — this happens when a
    /// `Deleted` is followed by the user restoring or recreating the file. Do not prompt on
    /// this arm.
    Appeared {
        /// The identity observed when the file reappeared.
        current: FileIdentity,
    },
    /// The backend reported an error other than `NotFound`. The string is a short
    /// diagnostic; log it, do not surface it as a "the file was deleted" prompt (which
    /// would be wrong for a transient permissions blip).
    ProbeFailed(String),
    /// The probe was rejected because its watcher provenance was absent or mismatched, or its generation was already admitted.
    Superseded,
}

impl FileWatchOutcome {
    /// Returns `true` for `Modified`, `Replaced`, and `Deleted` outcomes.
    #[must_use]
    pub fn requires_attention(&self) -> bool {
        matches!(
            self,
            Self::Modified { .. } | Self::Replaced { .. } | Self::Deleted { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WatchOwner(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OwnerSequence {
    next: u64,
    exhausted: bool,
}

impl OwnerSequence {
    const START: Self = Self {
        next: 0,
        exhausted: false,
    };

    fn mint(&mut self) -> Option<WatchOwner> {
        if self.exhausted {
            return None;
        }
        let owner = WatchOwner(self.next);
        match self.next.checked_add(1) {
            Some(next) => self.next = next,
            // Exhaustion follows the `u64::MAX` mint because no unused successor remains.
            None => self.exhausted = true,
        }
        Some(owner)
    }
}

static OWNER_SEQUENCE: Mutex<OwnerSequence> = Mutex::new(OwnerSequence::START);

fn mint_owner() -> Option<WatchOwner> {
    OWNER_SEQUENCE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .mint()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbeStamp {
    Watcher { owner: WatchOwner, generation: u64 },
    Unidentified,
}

#[derive(Debug)]
struct ProbeIdentity {
    owner: Option<WatchOwner>,
    latest: Option<u64>,
    admitted: Option<u64>,
}

impl ProbeIdentity {
    fn renewed() -> Self {
        Self {
            owner: mint_owner(),
            latest: None,
            admitted: None,
        }
    }

    fn issue(&mut self) -> ProbeStamp {
        let Some(owner) = self.owner else {
            return ProbeStamp::Unidentified;
        };
        let generation = match self.latest {
            None => 0,
            Some(previous) => match previous.checked_add(1) {
                Some(next) => next,
                None => {
                    // Renew the owner instead of wrapping a generation, so no outstanding probe can alias
                    // generation zero.
                    *self = Self::renewed();
                    return self.issue();
                }
            },
        };
        self.latest = Some(generation);
        ProbeStamp::Watcher { owner, generation }
    }

    fn admit(&mut self, stamp: ProbeStamp) -> bool {
        match stamp {
            ProbeStamp::Unidentified => false,
            ProbeStamp::Watcher { owner, generation } => {
                if self.owner != Some(owner) {
                    return false;
                }
                if self.admitted.is_some_and(|applied| generation <= applied) {
                    return false;
                }
                self.admitted = Some(generation);
                true
            }
        }
    }
}

/// The injectable probe backend.
pub trait FileWatchBackend: Send + Sync + 'static {
    /// Returns `Ok(Some(id))` when the path resolves to a watchable regular file,
    /// `Ok(None)` when it does not exist ([`std::io::ErrorKind::NotFound`]), and
    /// `Err(msg)` for a refusal or any other failure.
    ///
    /// # Errors
    ///
    /// A message for a refusal or any failure other than a missing path.
    fn probe(&self, path: &Path) -> Result<Option<FileIdentity>, String>;
}

/// Filesystem-backed probe of a regular file via [`std::fs::metadata`].
///
/// Final symlinks are followed. A dangling link is absent, while a directory or
/// other non-regular target is a probe failure.
#[derive(Clone, Copy, Debug, Default)]
pub struct FsBackend;

impl FileWatchBackend for FsBackend {
    fn probe(&self, path: &Path) -> Result<Option<FileIdentity>, String> {
        match std::fs::metadata(path) {
            Ok(md) if md.is_file() => Ok(Some(FileIdentity::from_metadata(&md))),
            Ok(_) => Err(format!(
                "{}: watched path is not a regular file",
                path.display()
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

/// In-memory probe backend for tests.
#[derive(Clone, Debug, Default)]
pub struct InMemoryBackend {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    files: HashMap<PathBuf, FileIdentity>,
    errors: HashMap<PathBuf, String>,
    probes: usize,
}

impl InMemoryBackend {
    /// A fresh, empty backend. No files known.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Make `path` present with `id`. Replaces any prior entry.
    pub fn set(&self, path: impl Into<PathBuf>, id: FileIdentity) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .files
            .insert(path.into(), id);
    }

    /// Makes `path` absent and returns its previous identity, if present.
    pub fn remove(&self, path: impl AsRef<Path>) -> Option<FileIdentity> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .files
            .remove(path.as_ref())
    }

    /// Force the next `probe(path)` (and every subsequent one until [`Self::clear_error`])
    /// to return `Err(msg)`. Simulates a transient stat failure (`EACCES`, `EIO`) without
    /// needing a real filesystem state where that would happen.
    pub fn force_error(&self, path: impl Into<PathBuf>, msg: impl Into<String>) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .errors
            .insert(path.into(), msg.into());
    }

    /// Clear a forced error set by [`Self::force_error`].
    pub fn clear_error(&self, path: impl AsRef<Path>) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .errors
            .remove(path.as_ref());
    }

    /// Returns the number of probes served since this backend was created.
    #[must_use]
    pub fn probes(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .probes
    }
}

impl FileWatchBackend for InMemoryBackend {
    fn probe(&self, path: &Path) -> Result<Option<FileIdentity>, String> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.probes += 1;
        if let Some(msg) = inner.errors.get(path) {
            return Err(msg.clone());
        }
        Ok(inner.files.get(path).copied())
    }
}

/// A prepared backend probe that can be moved to a worker task before execution.
pub struct ProbeOp {
    backend: Arc<dyn FileWatchBackend>,
    path: PathBuf,
    baseline: Option<FileIdentity>,
    stamp: ProbeStamp,
}

impl fmt::Debug for ProbeOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProbeOp")
            .field("path", &self.path)
            .field("baseline", &self.baseline)
            .field("stamp", &self.stamp)
            .finish_non_exhaustive()
    }
}

impl ProbeOp {
    /// Submits this probe to a bounded blocking-task owner.
    ///
    /// `generation` is returned by the task settlement; the watcher's separate
    /// internal stamp continues to protect [`FileWatcher::observe`].
    pub fn dispatch(
        self,
        worker: &crate::blocking_task::BlockingTaskDispatcher,
        generation: u64,
    ) -> crate::blocking_task::BlockingTask<Probe> {
        worker.submit(generation, move |_| self.execute())
    }

    /// Runs the backend probe synchronously; do not call this on an async executor.
    pub fn execute(self) -> Probe {
        let observed = self.backend.probe(&self.path);
        Probe {
            baseline: self.baseline,
            observed,
            stamp: self.stamp,
        }
    }

    /// The path this probe examines.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

const _: () = {
    fn _assert_send<T: Send + 'static>() {}
    fn _check() {
        _assert_send::<ProbeOp>();
        _assert_send::<Probe>();
    }
};

/// The result of one [`ProbeOp::execute`].
#[derive(Debug, Clone)]
pub struct Probe {
    baseline: Option<FileIdentity>,
    observed: Result<Option<FileIdentity>, String>,
    stamp: ProbeStamp,
}

impl Probe {
    /// Returns the baseline captured when this probe was prepared; outcomes use the
    /// watcher’s current baseline instead.
    #[must_use]
    pub fn baseline_snapshot(&self) -> Option<FileIdentity> {
        self.baseline
    }

    /// Returns the backend observation, with `Ok(None)` representing an absent path.
    ///
    /// # Errors
    ///
    /// The backend's failure message.
    pub fn observed(&self) -> Result<Option<FileIdentity>, &str> {
        self.observed.as_ref().copied().map_err(String::as_str)
    }
}

/// The stateful watcher for one open file.
pub struct FileWatcher {
    backend: Arc<dyn FileWatchBackend>,
    path: PathBuf,
    baseline: Option<FileIdentity>,
    seen: bool,
    identity: Mutex<ProbeIdentity>,
}

impl fmt::Debug for FileWatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileWatcher")
            .field("path", &self.path)
            .field("baseline", &self.baseline)
            .field("seen", &self.seen)
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl FileWatcher {
    /// Creates an unseeded watcher owning its backend.
    ///
    /// Its first admitted probe reports [`FileWatchOutcome::NoBaseline`] and
    /// installs what it observes. Use [`Self::with_baseline`] when the caller has
    /// already loaded the file, or a change before that first probe is unknowable.
    pub fn new(backend: impl FileWatchBackend, path: impl Into<PathBuf>) -> Self {
        Self {
            backend: Arc::new(backend),
            path: path.into(),
            baseline: None,
            seen: false,
            identity: Mutex::new(ProbeIdentity::renewed()),
        }
    }

    /// Creates an unseeded watcher sharing an existing `Arc<dyn FileWatchBackend>`.
    ///
    /// Use [`Self::with_baseline`] if the file has already been loaded.
    pub fn from_arc(backend: Arc<dyn FileWatchBackend>, path: impl Into<PathBuf>) -> Self {
        Self {
            backend,
            path: path.into(),
            baseline: None,
            seen: false,
            identity: Mutex::new(ProbeIdentity::renewed()),
        }
    }

    /// Seeds the watcher with the exact identity captured when the file was loaded.
    ///
    /// Capture the metadata from the opened file rather than looking the path up
    /// again, which could identify a replacement. Any already-prepared probes are
    /// invalidated.
    #[must_use]
    pub fn with_baseline(mut self, baseline: FileIdentity) -> Self {
        self.acknowledge(baseline);
        self
    }

    /// Points this watcher at `path` again (or at a different path) as a fresh observation.
    ///
    /// The baseline is cleared, so the next probe is a
    /// [`FileWatchOutcome::NoBaseline`] rather than a change event. Call
    /// [`Self::acknowledge`] before probing if the new path's contents have already
    /// been loaded.
    pub fn reopen(&mut self, path: impl Into<PathBuf>) {
        self.path = path.into();
        self.baseline = None;
        self.seen = false;
        *self.identity_mut() = ProbeIdentity::renewed();
    }

    // Recover the lock so prior poisoning does not permanently disable probe provenance.
    fn identity_mut(&self) -> std::sync::MutexGuard<'_, ProbeIdentity> {
        self.identity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The watched path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The current baseline, if one has been established.
    #[must_use]
    pub fn baseline(&self) -> Option<FileIdentity> {
        self.baseline
    }

    /// Whether the watcher currently regards the file as absent (either
    /// after a [`FileWatchOutcome::Deleted`] observation or after
    /// [`Self::mark_deleted`]).
    #[must_use]
    pub fn is_absent(&self) -> bool {
        self.seen && self.baseline.is_none()
    }

    /// Acknowledges a trusted present identity after a load or the app's own save.
    ///
    /// Any probe prepared before this call is rejected, so an old observation
    /// cannot undo the acknowledgement.
    pub fn acknowledge(&mut self, new_baseline: FileIdentity) {
        self.baseline = Some(new_baseline);
        self.seen = true;
        *self.identity_mut() = ProbeIdentity::renewed();
    }

    /// After the app's own delete completes, call this. Any already-prepared probe
    /// is invalidated. The next probe of `Ok(None)` reads as
    /// [`FileWatchOutcome::Unchanged`], and a probe that sees the file back reads as
    /// [`FileWatchOutcome::Appeared`].
    pub fn mark_deleted(&mut self) {
        self.baseline = None;
        self.seen = true;
        *self.identity_mut() = ProbeIdentity::renewed();
    }

    /// Prepares a probe for [`ProbeOp::dispatch`] or another dedicated worker.
    pub fn prepare_probe(&self) -> ProbeOp {
        let stamp = self.identity_mut().issue();
        ProbeOp {
            backend: Arc::clone(&self.backend),
            path: self.path.clone(),
            baseline: self.baseline,
            stamp,
        }
    }

    /// Applies an admitted probe to the current watcher state and returns its outcome.
    ///
    /// Present observations replace the baseline, deletion clears it, and failures or
    /// superseded probes leave it unchanged. Provenance is checked before mutation.
    pub fn observe(&mut self, probe: Probe) -> FileWatchOutcome {
        let admitted = self.identity_mut().admit(probe.stamp);
        if !admitted {
            return FileWatchOutcome::Superseded;
        }
        match probe.observed {
            Err(msg) => FileWatchOutcome::ProbeFailed(msg),
            Ok(observed) => match (self.baseline, observed, self.seen) {
                (None, obs, false) => {
                    self.baseline = obs;
                    self.seen = true;
                    FileWatchOutcome::NoBaseline(obs)
                }
                (None, None, true) => FileWatchOutcome::Unchanged,
                (None, Some(cur), true) => {
                    self.baseline = Some(cur);
                    FileWatchOutcome::Appeared { current: cur }
                }
                (Some(prev), None, _) => {
                    self.baseline = None;
                    self.seen = true;
                    FileWatchOutcome::Deleted { previous: prev }
                }
                (Some(prev), Some(cur), _) => {
                    if prev == cur {
                        FileWatchOutcome::Unchanged
                    } else if prev.inode != cur.inode {
                        self.baseline = Some(cur);
                        FileWatchOutcome::Replaced {
                            previous: prev,
                            current: cur,
                        }
                    } else {
                        self.baseline = Some(cur);
                        FileWatchOutcome::Modified {
                            previous: prev,
                            current: cur,
                        }
                    }
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id_at(inode: (u64, u64), mtime_secs: i64, len: u64) -> FileIdentity {
        FileIdentity::new(inode, mtime_secs, 0, len)
    }

    fn watcher(backend: &InMemoryBackend, path: &str) -> FileWatcher {
        FileWatcher::from_arc(Arc::new(backend.clone()), path)
    }

    #[test]
    fn deletion_is_reported_as_deleted() {
        let backend = InMemoryBackend::new();
        backend.set("/f", id_at((10, 42), 100, 1024));
        let mut w = watcher(&backend, "/f");
        let _ = w.observe(w.prepare_probe().execute());

        backend.remove("/f");

        let outcome = w.observe(w.prepare_probe().execute());
        assert!(
            matches!(
                outcome,
                FileWatchOutcome::Deleted { previous } if previous.inode == (10, 42)
            ),
            "external delete must be Deleted with the vanished identity; got {outcome:?}",
        );
        assert!(
            w.is_absent(),
            "the watcher must record absence after Deleted"
        );
        assert!(outcome.requires_attention());
    }
}
