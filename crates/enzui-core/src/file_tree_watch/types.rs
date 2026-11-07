//! Value types for [`crate::file_tree_watch`].

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Opaque per-service watch id.
///
/// Returned by [`super::DirectoryWatchService::watch`] and consumed by
/// [`super::DirectoryWatchService::unwatch`]. Unique within one service
/// instance; a caller who holds two services must not compare ids across
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WatchId(pub(super) u64);

impl fmt::Display for WatchId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WatchId({})", self.0)
    }
}

/// Globally unique event-provenance token, issued from a process-wide
/// counter. Every watch (across every service and every backend) gets
/// a distinct id; this is the identity the service and backends key on
/// for admission and cleanup. [`WatchId`] alone would let two services
/// sharing a backend alias each other's events.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TokenId(pub(super) u64);

impl fmt::Display for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TokenId({})", self.0)
    }
}

impl TokenId {
    pub(super) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let raw = NEXT.fetch_add(1, Ordering::Relaxed);
        assert!(raw != 0, "TokenId space exhausted");
        Self(raw)
    }
}

/// One admissible child-directory change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DirectoryChange {
    /// A new child entry appeared.
    Created(PathBuf),
    /// An existing child entry disappeared.
    Deleted(PathBuf),
    /// A child entry's data or metadata changed in place.
    Modified(PathBuf),
    /// A rename observed by the backend as one correlated pair. Backends
    /// without correlation must push [`Deleted`](Self::Deleted) and
    /// [`Created`](Self::Created) instead; the service does not synthesise
    /// this arm on partial evidence.
    Renamed {
        /// The name that vanished.
        from: PathBuf,
        /// The name that appeared.
        to: PathBuf,
    },
}

impl DirectoryChange {
    #[must_use]
    pub(super) fn byte_cost(&self) -> usize {
        match self {
            Self::Created(p) | Self::Deleted(p) | Self::Modified(p) => path_byte_cost(p),
            Self::Renamed { from, to } => path_byte_cost(from).saturating_add(path_byte_cost(to)),
        }
    }
}

fn path_byte_cost(path: &std::path::Path) -> usize {
    path.as_os_str().len()
}

/// The caller-actionable outcome the service holds for one watch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchOutcome {
    /// A non-empty change batch with consecutive identical events suppressed.
    /// Bounds are checked before duplicate suppression; a duplicate arriving
    /// at a full buffer can therefore trigger overflow.
    Changes(Vec<DirectoryChange>),
    /// Buffered event count or total bytes exceeded the caps, or the
    /// backend explicitly reported that it lost events. The caller must
    /// perform a fresh listing; the buffer is cleared before this arm is
    /// delivered.
    Overflow,
    /// The watched directory disappeared or became inaccessible. This report
    /// does not itself cancel the service slot.
    Vanished,
    /// The backend reported a non-fatal error. Diagnostic; the watch stays
    /// live. Only the latest diagnostic is retained, capped separately at the
    /// service's path-byte limit.
    ProbeFailed(String),
}

impl WatchOutcome {
    /// `true` for the caller-actionable arms
    /// ([`Changes`](Self::Changes) / [`Overflow`](Self::Overflow) /
    /// [`Vanished`](Self::Vanished)); `false` for
    /// [`ProbeFailed`](Self::ProbeFailed).
    #[must_use]
    pub fn requires_attention(&self) -> bool {
        matches!(self, Self::Changes(_) | Self::Overflow | Self::Vanished)
    }
}

/// One delivery ready for consumption by the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WatchDelivery {
    /// The watch the outcome applies to.
    pub watch: WatchId,
    /// The path the watch was installed on.
    pub path: PathBuf,
    /// The typed outcome.
    pub outcome: WatchOutcome,
}

/// Reason [`super::DirectoryWatchService::watch`] declined to install a
/// watch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WatchRefusal {
    /// The service already holds the configured watch cap.
    CapacityExceeded {
        /// The configured limit.
        limit: usize,
    },
    /// The injected backend is a static unavailable stance.
    Unsupported(&'static str),
    /// The backend attempted to install a watch and failed at the OS layer.
    BackendError(String),
}

impl fmt::Display for WatchRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CapacityExceeded { limit } => {
                write!(f, "directory-watch capacity exceeded (limit {limit})")
            }
            Self::Unsupported(msg) => write!(f, "directory-watch unsupported: {msg}"),
            Self::BackendError(msg) => write!(f, "directory-watch backend error: {msg}"),
        }
    }
}

/// Report from one [`super::DirectoryWatchService::ingest`] call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IngestReport {
    /// Number of events applied to a live watch.
    pub admitted: usize,
    /// Number of events discarded because their token did not identify
    /// a live watch of *this* service (a different service's token, a
    /// dropped watch, or a hostile forgery).
    pub superseded: usize,
}

/// Handed to the backend when a watch is installed. The backend
/// clones this and stamps every [`RawEvent`] it produces with it.
#[derive(Clone)]
pub struct WatchToken {
    pub(super) watch: WatchId,
    pub(super) token_id: TokenId,
}

impl fmt::Debug for WatchToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WatchToken")
            .field("watch", &self.watch)
            .field("token_id", &self.token_id)
            .finish()
    }
}

impl WatchToken {
    /// A token for a watch installed directly on a
    /// [`DirectoryWatchBackend`](super::DirectoryWatchBackend), outside any
    /// [`DirectoryWatchService`](super::DirectoryWatchService). Its
    /// [`token_id`](Self::token_id) is globally unique; its watch id is 0.
    #[must_use]
    pub fn standalone() -> Self {
        Self {
            watch: WatchId(0),
            token_id: TokenId::next(),
        }
    }

    /// The per-service id this token was minted for.
    #[must_use]
    pub fn watch(&self) -> WatchId {
        self.watch
    }

    /// The globally unique provenance id.
    #[must_use]
    pub fn token_id(&self) -> TokenId {
        self.token_id
    }
}

/// A raw event pushed by a backend into the service.
#[derive(Debug, Clone)]
pub struct RawEvent {
    /// Provenance stamp the backend cloned from the token it received.
    pub token: WatchToken,
    /// What happened.
    pub kind: RawEventKind,
}

impl RawEvent {
    /// Construct a change event.
    #[must_use]
    pub fn change(token: WatchToken, change: DirectoryChange) -> Self {
        Self {
            token,
            kind: RawEventKind::Change(change),
        }
    }

    /// Construct an overflow event: the backend lost events for this watch.
    #[must_use]
    pub fn overflow(token: WatchToken) -> Self {
        Self {
            token,
            kind: RawEventKind::Overflow,
        }
    }

    /// Construct a vanished event: the watched directory is gone.
    #[must_use]
    pub fn vanished(token: WatchToken) -> Self {
        Self {
            token,
            kind: RawEventKind::Vanished,
        }
    }

    /// Construct a probe-failure diagnostic.
    #[must_use]
    pub fn probe_failed(token: WatchToken, msg: impl Into<String>) -> Self {
        Self {
            token,
            kind: RawEventKind::ProbeFailed(msg.into()),
        }
    }
}

/// The union of things a backend may report for one watch.
#[derive(Debug, Clone)]
pub enum RawEventKind {
    /// One admissible directory change.
    Change(DirectoryChange),
    /// The backend lost events for this watch; the service records an
    /// overflow and clears the pending buffer at delivery time.
    Overflow,
    /// The watched directory disappeared or became inaccessible.
    Vanished,
    /// A non-fatal backend error. Diagnostic.
    ProbeFailed(String),
}
