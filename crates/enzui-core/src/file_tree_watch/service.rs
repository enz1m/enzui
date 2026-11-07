//! [`DirectoryWatchService`]: bounded, generation-checked directory-watch
//! service and its event-driven [`Subscription`](iced_futures::Subscription).

use super::backend::{BoxHandle, DirectoryWatchBackend};
use super::sink::EventSink;
use super::types::{
    DirectoryChange, IngestReport, RawEvent, RawEventKind, TokenId, WatchDelivery, WatchId,
    WatchOutcome, WatchRefusal, WatchToken,
};
use iced_futures::BoxStream;
use iced_futures::Subscription;
use iced_futures::futures::stream;
use iced_futures::futures::task::AtomicWaker;
use iced_futures::subscription::{self, EventStream, Hasher, Recipe};
use std::any::TypeId;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::hash::Hash;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::task::Poll;

/// Default cap on live watches per service.
pub const DEFAULT_MAX_WATCHES: usize = 32;
/// Default per-watch cap on buffered change events.
pub const DEFAULT_MAX_PENDING_PER_WATCH: usize = 64;
/// Default per-watch cap on buffered payload bytes (path lengths).
pub const DEFAULT_MAX_PATH_BYTES: usize = 64 * 1024;

/// Bounded, generation-checked directory-watch service.
pub struct DirectoryWatchService {
    shared: Arc<Shared>,
    backend: Arc<dyn DirectoryWatchBackend>,
    limits: Limits,
    subscription_key: u64,
}

impl std::fmt::Debug for DirectoryWatchService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirectoryWatchService")
            .field("limits", &self.limits)
            .field("active_watches", &self.active_watches())
            .field("subscription_key", &self.subscription_key)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Limits {
    pub(super) max_watches: usize,
    pub(super) max_pending_per_watch: usize,
    pub(super) max_path_bytes: usize,
}

pub(super) struct Shared {
    pub(super) inner: Mutex<Inner>,
    pub(super) ready: AtomicBool,
    pub(super) waker: AtomicWaker,
    pub(super) limits: Limits,
    pub(super) service_gone: AtomicBool,
}

pub(super) struct Inner {
    // Placeholder slots admit installation-time events and reserve capacity.
    /// In creation order, so deliveries across watches come out in that order.
    pub(super) watches: BTreeMap<TokenId, Slot>,
    pub(super) id_to_token: HashMap<WatchId, TokenId>,
    pub(super) next_watch_id: u64,
}

pub(super) struct Slot {
    pub(super) id: WatchId,
    pub(super) path: PathBuf,
    pub(super) handle: Option<BoxHandle>,
    pub(super) pending: VecDeque<DirectoryChange>,
    pub(super) pending_bytes: usize,
    pub(super) overflow: bool,
    pub(super) vanished: bool,
    pub(super) diagnostic: Option<String>,
}

// Weak ownership prevents a retained backend sink from keeping the service alive.
struct ServiceSink {
    shared: Weak<Shared>,
}

impl EventSink for ServiceSink {
    fn push(&self, event: RawEvent) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let admitted = {
            let mut guard = shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
            let mut report = IngestReport::default();
            apply_raw(&mut guard, event, &shared.limits, &mut report);
            report.admitted > 0
        };
        if admitted {
            shared.ready.store(true, Ordering::Release);
            shared.waker.wake();
        }
    }
}

impl DirectoryWatchService {
    /// Create a service with the default caps and an owned backend.
    #[must_use]
    pub fn new(backend: impl DirectoryWatchBackend) -> Self {
        Self::with_config(
            Arc::new(backend),
            DEFAULT_MAX_WATCHES,
            DEFAULT_MAX_PENDING_PER_WATCH,
            DEFAULT_MAX_PATH_BYTES,
        )
    }

    /// Create a service with explicit caps and a shared backend.
    ///
    /// Pending-event and path-byte caps are clamped to at least one. A zero
    /// watch cap refuses all installations; the byte cap also separately
    /// bounds the retained diagnostic for each watch.
    #[must_use]
    pub fn with_config(
        backend: Arc<dyn DirectoryWatchBackend>,
        max_watches: usize,
        max_pending_per_watch: usize,
        max_path_bytes: usize,
    ) -> Self {
        static NEXT_SUB_KEY: AtomicU64 = AtomicU64::new(1);
        let limits = Limits {
            max_watches,
            max_pending_per_watch: max_pending_per_watch.max(1),
            max_path_bytes: max_path_bytes.max(1),
        };
        Self {
            shared: Arc::new(Shared {
                inner: Mutex::new(Inner {
                    watches: BTreeMap::new(),
                    id_to_token: HashMap::new(),
                    next_watch_id: 1,
                }),
                ready: AtomicBool::new(false),
                waker: AtomicWaker::new(),
                limits,
                service_gone: AtomicBool::new(false),
            }),
            backend,
            limits,
            subscription_key: NEXT_SUB_KEY.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Configured watch cap.
    #[must_use]
    pub fn max_watches(&self) -> usize {
        self.limits.max_watches
    }
    /// Configured per-watch pending-event cap.
    #[must_use]
    pub fn max_pending_per_watch(&self) -> usize {
        self.limits.max_pending_per_watch
    }
    /// Configured per-watch pending-payload byte cap.
    #[must_use]
    pub fn max_path_bytes(&self) -> usize {
        self.limits.max_path_bytes
    }

    /// Install a watch on `path`.
    ///
    /// Events emitted during backend installation are retained. Capacity counts
    /// installations still in progress. Concurrent cancellation may remove the
    /// slot before this call returns a successful id.
    ///
    /// # Errors
    ///
    /// Returns [`WatchRefusal::CapacityExceeded`] before invoking the backend
    /// when the service is full, or propagates the backend's refusal.
    ///
    /// # Panics
    ///
    /// Panics when watch-id or token-id allocation is exhausted. A backend
    /// panic propagates and leaves the reserved slot until it is cancelled or
    /// the service is dropped.
    pub fn watch(&self, path: impl Into<PathBuf>) -> Result<WatchId, WatchRefusal> {
        let path = path.into();
        let (id, token_id) = {
            let mut inner = self.lock();
            if inner.watches.len() >= self.limits.max_watches {
                return Err(WatchRefusal::CapacityExceeded {
                    limit: self.limits.max_watches,
                });
            }
            let id = WatchId(inner.next_watch_id);
            inner.next_watch_id = inner
                .next_watch_id
                .checked_add(1)
                .expect("WatchId space exhausted");
            let token_id = TokenId::next();
            inner.id_to_token.insert(id, token_id);
            inner.watches.insert(
                token_id,
                Slot {
                    id,
                    path: path.clone(),
                    handle: None,
                    pending: VecDeque::new(),
                    pending_bytes: 0,
                    overflow: false,
                    vanished: false,
                    diagnostic: None,
                },
            );
            (id, token_id)
        };
        let token = WatchToken {
            watch: id,
            token_id,
        };
        let sink: Arc<dyn EventSink> = Arc::new(ServiceSink {
            shared: Arc::downgrade(&self.shared),
        });
        // Install outside the mutex so synchronous sink callbacks can acquire it.
        let handle = match self.backend.watch(&path, token, sink) {
            Ok(handle) => handle,
            Err(err) => {
                let mut inner = self.lock();
                inner.id_to_token.remove(&id);
                inner.watches.remove(&token_id);
                return Err(err);
            }
        };
        // A cancelled installation's handle may reenter the service when dropped.
        let salvaged: Option<BoxHandle> = {
            let mut inner = self.lock();
            match inner.watches.get_mut(&token_id) {
                Some(slot) => {
                    slot.handle = Some(handle);
                    None
                }
                None => Some(handle),
            }
        };
        drop(salvaged);
        Ok(id)
    }

    /// Cancel a watch, dropping its backend handle.
    pub fn unwatch(&self, id: WatchId) -> bool {
        let slot: Option<Slot> = {
            let mut inner = self.lock();
            let Some(token_id) = inner.id_to_token.remove(&id) else {
                return false;
            };
            inner.watches.remove(&token_id)
        };
        // Backend cancellation may reenter the service; release its mutex first.
        drop(slot);
        true
    }

    /// `true` iff `id` still identifies a live watch.
    #[must_use]
    pub fn is_watching(&self, id: WatchId) -> bool {
        self.lock().id_to_token.contains_key(&id)
    }

    /// Number of reserved slots, including installations in progress.
    #[must_use]
    pub fn active_watches(&self) -> usize {
        self.lock().watches.len()
    }

    /// Reserved paths, including installations in progress, in unspecified order.
    #[must_use]
    pub fn watched_paths(&self) -> Vec<PathBuf> {
        self.lock()
            .watches
            .values()
            .map(|s| s.path.clone())
            .collect()
    }

    /// Ingest a batch of raw events directly.
    ///
    /// Applies the same token checks and bounds as the backend sink. Wakes the
    /// subscription once when at least one event targets a live slot, even if
    /// those events are deduplicated or suppressed by pending overflow.
    pub fn ingest<I>(&self, events: I) -> IngestReport
    where
        I: IntoIterator<Item = RawEvent>,
    {
        let mut inner = self.lock();
        let mut report = IngestReport::default();
        for event in events {
            apply_raw(&mut inner, event, &self.limits, &mut report);
        }
        if report.admitted > 0 {
            drop(inner);
            self.shared.ready.store(true, Ordering::Release);
            self.shared.waker.wake();
        }
        report
    }

    /// Drain every watch's pending state in unspecified watch order.
    ///
    /// Within each watch, changes or overflow precede the latest diagnostic
    /// and then disappearance. Overflow replaces queued changes; disappearance
    /// leaves the watch slot installed. Direct draining competes with the
    /// subscription for the same pending events.
    pub fn take_deliveries(&self) -> Vec<WatchDelivery> {
        let mut inner = self.lock();
        let mut out = Vec::new();
        for slot in inner.watches.values_mut() {
            emit_slot_deliveries(slot, &mut out);
        }
        self.shared.ready.store(false, Ordering::Release);
        out
    }

    /// `true` if any watch has pending state.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.lock()
            .watches
            .values()
            .any(|s| !s.pending.is_empty() || s.overflow || s.vanished || s.diagnostic.is_some())
    }

    /// Access the shared backend.
    #[must_use]
    pub fn backend(&self) -> Arc<dyn DirectoryWatchBackend> {
        Arc::clone(&self.backend)
    }

    /// Event-driven [`Subscription`](iced_futures::Subscription).
    ///
    /// Repeated calls for one service have the same recipe identity, keeping
    /// the running stream while it remains offered to iced. It drains the same
    /// pending state as [`Self::take_deliveries`] and ends when the service drops.
    /// No polling timer is installed. Multiple independently started streams
    /// compete for events and share one waker slot; use one subscription owner.
    pub fn subscription(&self) -> Subscription<Vec<WatchDelivery>> {
        subscription::from_recipe(WatchRecipe {
            key: self.subscription_key,
            shared: Arc::clone(&self.shared),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.shared
            .inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

impl Drop for DirectoryWatchService {
    fn drop(&mut self) {
        let handles: Vec<Option<BoxHandle>> = {
            let mut inner = self.lock();
            inner.id_to_token.clear();
            std::mem::take(&mut inner.watches)
                .into_values()
                .map(|slot| slot.handle)
                .collect()
        };
        self.shared.service_gone.store(true, Ordering::Release);
        self.shared.ready.store(true, Ordering::Release);
        self.shared.waker.wake();
        // Retained subscription state must not keep handles alive; cancellation may reenter.
        drop(handles);
    }
}

fn apply_raw(inner: &mut Inner, event: RawEvent, limits: &Limits, report: &mut IngestReport) {
    let Some(slot) = inner.watches.get_mut(&event.token.token_id) else {
        report.superseded += 1;
        return;
    };
    report.admitted += 1;
    match event.kind {
        RawEventKind::Overflow => {
            slot.overflow = true;
        }
        RawEventKind::Vanished => {
            slot.vanished = true;
        }
        RawEventKind::ProbeFailed(msg) => {
            const MARKER: &str = " [truncated]";
            let cap = limits.max_path_bytes;
            let truncated = if msg.len() > cap {
                if cap >= MARKER.len() {
                    let mut s = msg.clone();
                    s.truncate(msg.floor_char_boundary(cap - MARKER.len()));
                    s.push_str(MARKER);
                    s
                } else {
                    let mut s = msg.clone();
                    s.truncate(msg.floor_char_boundary(cap));
                    s
                }
            } else {
                msg
            };
            slot.diagnostic = Some(truncated);
        }
        RawEventKind::Change(change) => {
            if slot.overflow {
                return;
            }
            let cost = change.byte_cost();
            if slot.pending.len() >= limits.max_pending_per_watch
                || slot.pending_bytes.saturating_add(cost) > limits.max_path_bytes
            {
                slot.overflow = true;
                return;
            }
            if slot.pending.back() == Some(&change) {
                return;
            }
            slot.pending_bytes = slot.pending_bytes.saturating_add(cost);
            slot.pending.push_back(change);
        }
    }
}

fn emit_slot_deliveries(slot: &mut Slot, out: &mut Vec<WatchDelivery>) {
    if slot.overflow {
        out.push(WatchDelivery {
            watch: slot.id,
            path: slot.path.clone(),
            outcome: WatchOutcome::Overflow,
        });
        slot.overflow = false;
        slot.pending.clear();
        slot.pending_bytes = 0;
    } else if !slot.pending.is_empty() {
        let changes: Vec<_> = slot.pending.drain(..).collect();
        slot.pending_bytes = 0;
        out.push(WatchDelivery {
            watch: slot.id,
            path: slot.path.clone(),
            outcome: WatchOutcome::Changes(changes),
        });
    }
    if let Some(diag) = slot.diagnostic.take() {
        out.push(WatchDelivery {
            watch: slot.id,
            path: slot.path.clone(),
            outcome: WatchOutcome::ProbeFailed(diag),
        });
    }
    if slot.vanished {
        out.push(WatchDelivery {
            watch: slot.id,
            path: slot.path.clone(),
            outcome: WatchOutcome::Vanished,
        });
        slot.vanished = false;
    }
}

struct WatchRecipe {
    key: u64,
    shared: Arc<Shared>,
}

impl Recipe for WatchRecipe {
    type Output = Vec<WatchDelivery>;

    fn hash(&self, state: &mut Hasher) {
        TypeId::of::<Self>().hash(state);
        self.key.hash(state);
    }

    fn stream(self: Box<Self>, _input: EventStream) -> BoxStream<Self::Output> {
        let shared = self.shared;
        Box::pin(stream::poll_fn(move |cx| {
            if shared.service_gone.load(Ordering::Acquire) && !has_pending_lock(&shared) {
                return Poll::Ready(None);
            }
            if shared.ready.swap(false, Ordering::AcqRel) {
                let batch = drain_batch(&shared);
                if !batch.is_empty() {
                    return Poll::Ready(Some(batch));
                }
            }
            // Register before rechecking to cover an event racing with the first flag swap.
            shared.waker.register(cx.waker());
            if shared.ready.swap(false, Ordering::AcqRel) {
                let batch = drain_batch(&shared);
                if !batch.is_empty() {
                    return Poll::Ready(Some(batch));
                }
            }
            if shared.service_gone.load(Ordering::Acquire) && !has_pending_lock(&shared) {
                return Poll::Ready(None);
            }
            Poll::Pending
        }))
    }
}

fn drain_batch(shared: &Arc<Shared>) -> Vec<WatchDelivery> {
    let mut inner = shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
    let mut out = Vec::new();
    for slot in inner.watches.values_mut() {
        emit_slot_deliveries(slot, &mut out);
    }
    out
}

fn has_pending_lock(shared: &Arc<Shared>) -> bool {
    let inner = shared.inner.lock().unwrap_or_else(PoisonError::into_inner);
    inner
        .watches
        .values()
        .any(|s| !s.pending.is_empty() || s.overflow || s.vanished || s.diagnostic.is_some())
}
