//! Backend trait and the two portable implementations.

use super::sink::EventSink;
use super::types::{RawEvent, RawEventKind, TokenId, WatchRefusal, WatchToken};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Marker trait for backend-owned watch handles.
///
/// Backend implementations must arrange for handle drop to cancel their watch.
/// The blanket implementation accepts any `Send + 'static` type; the marker
/// itself does not enforce cancellation.
pub trait WatchHandle: Send + 'static {}
impl<T: Send + 'static> WatchHandle for T {}

/// Owned trait object for a backend handle.
pub type BoxHandle = Box<dyn WatchHandle>;

/// A backend that installs directory watches and pushes events into a sink.
///
/// Returned handles cancel their watches on drop. Installation may synchronously
/// emit events; ongoing production belongs to the backend, and sink calls may
/// block on the service's mutex.
pub trait DirectoryWatchBackend: Send + Sync + 'static {
    /// Install a watch on `path`.
    ///
    /// Stamp every event with the supplied token and route it through `sink`.
    /// Synchronous installation-time delivery is supported.
    ///
    /// # Errors
    ///
    /// [`WatchRefusal::Unsupported`] for a static unavailable stance;
    /// [`WatchRefusal::BackendError`] for OS-layer failures.
    /// [`WatchRefusal::CapacityExceeded`] is service-side and never
    /// returned by a backend.
    fn watch(
        &self,
        path: &Path,
        token: WatchToken,
        sink: Arc<dyn EventSink>,
    ) -> Result<BoxHandle, WatchRefusal>;
}

/// The static unavailable backend.
///
/// Every [`watch`](DirectoryWatchBackend::watch) call refuses with
/// [`WatchRefusal::Unsupported`]. Hosts can use it to select manual refresh.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopBackend;

impl DirectoryWatchBackend for NoopBackend {
    fn watch(
        &self,
        _path: &Path,
        _token: WatchToken,
        _sink: Arc<dyn EventSink>,
    ) -> Result<BoxHandle, WatchRefusal> {
        Err(WatchRefusal::Unsupported(
            "no directory-watch backend wired in this build",
        ))
    }
}

/// In-memory directory watches with synchronous event injection and no disk access.
///
/// Clones share registrations. Token identities keep watches from different
/// services independent; dropping a handle removes only its registration.
#[derive(Clone, Debug, Default)]
pub struct InMemoryDirectoryBackend {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    entries: Vec<Entry>,
    forced_refusals: VecDeque<WatchRefusal>,
    served: usize,
}

struct Entry {
    token_id: TokenId,
    token: WatchToken,
    path: PathBuf,
    sink: Arc<dyn EventSink>,
    alive: Arc<AtomicBool>,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("token_id", &self.token_id)
            .field("path", &self.path)
            .field("alive", &self.alive.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

struct InMemoryHandle {
    backend: Arc<Mutex<Inner>>,
    token_id: TokenId,
    alive: Arc<AtomicBool>,
}

impl Drop for InMemoryHandle {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        let mut inner = self.backend.lock().unwrap_or_else(PoisonError::into_inner);
        inner.entries.retain(|e| e.token_id != self.token_id);
    }
}

impl InMemoryDirectoryBackend {
    /// Empty backend.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a refusal for the next `watch` call.
    pub fn force_next_refusal(&self, refusal: WatchRefusal) {
        self.lock().forced_refusals.push_back(refusal);
    }

    /// Number of `watch` calls served (accepted + refused).
    #[must_use]
    pub fn served(&self) -> usize {
        self.lock().served
    }

    /// Paths currently watched by this backend, in insertion order.
    #[must_use]
    pub fn active_paths(&self) -> Vec<PathBuf> {
        self.lock()
            .entries
            .iter()
            .filter(|e| e.alive.load(Ordering::SeqCst))
            .map(|e| e.path.clone())
            .collect()
    }

    /// Number of live watches this backend holds.
    #[must_use]
    pub fn active_watches(&self) -> usize {
        self.lock()
            .entries
            .iter()
            .filter(|e| e.alive.load(Ordering::SeqCst))
            .count()
    }

    /// Push one raw event into the sink of every live watch on `path`.
    ///
    /// Delivers to *all* subscribers of the path (including watches held
    /// by other services sharing this backend). Each subscriber's service
    /// admits only the events stamped with its own [`TokenId`]. Delivery occurs
    /// synchronously on the caller's thread.
    pub fn emit(&self, path: &Path, kind: RawEventKind) {
        // Sink callbacks may reenter backend state; push after releasing its mutex.
        let targets: Vec<(WatchToken, Arc<dyn EventSink>)> = {
            let inner = self.lock();
            inner
                .entries
                .iter()
                .filter(|e| e.path == path && e.alive.load(Ordering::SeqCst))
                .map(|e| (e.token.clone(), Arc::clone(&e.sink)))
                .collect()
        };
        for (token, sink) in targets {
            sink.push(RawEvent {
                token,
                kind: kind.clone(),
            });
        }
    }

    /// Build one raw event for a specific live [`TokenId`] and push it
    /// into that watch's sink. Returns `false` if the token id is not
    /// held by this backend (already dropped, never installed, or from a
    /// different backend).
    pub fn event_for_id(&self, token_id: TokenId, kind: RawEventKind) -> bool {
        let target: Option<(WatchToken, Arc<dyn EventSink>)> = {
            let inner = self.lock();
            inner
                .entries
                .iter()
                .find(|e| e.token_id == token_id && e.alive.load(Ordering::SeqCst))
                .map(|e| (e.token.clone(), Arc::clone(&e.sink)))
        };
        let Some((token, sink)) = target else {
            return false;
        };
        sink.push(RawEvent { token, kind });
        true
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl DirectoryWatchBackend for InMemoryDirectoryBackend {
    fn watch(
        &self,
        path: &Path,
        token: WatchToken,
        sink: Arc<dyn EventSink>,
    ) -> Result<BoxHandle, WatchRefusal> {
        let mut inner = self.lock();
        inner.served += 1;
        if let Some(refusal) = inner.forced_refusals.pop_front() {
            return Err(refusal);
        }
        let alive = Arc::new(AtomicBool::new(true));
        let entry = Entry {
            token_id: token.token_id,
            token: token.clone(),
            path: path.to_path_buf(),
            sink,
            alive: Arc::clone(&alive),
        };
        inner.entries.push(entry);
        Ok(Box::new(InMemoryHandle {
            backend: Arc::clone(&self.inner),
            token_id: token.token_id,
            alive,
        }))
    }
}
