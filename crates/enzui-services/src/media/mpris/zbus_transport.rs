//! zbus-backed MPRIS transport.
//!
//! Inbound calls are queued for the application thread, and outbound publications use bounded latest-value wakes so the bus task never mutates reducer state.
//!
//! One generation owns the bus name, the call queue and the signal pump.
//! [`ZbusMprisConnection::begin_shutdown`](crate::media::mpris::zbus_transport::ZbusMprisConnection::begin_shutdown)
//! ends that generation, after which
//! inbound calls are refused and publications are inert; a reconnect is a new
//! generation whose properties the caller republishes.

use super::{MprisCall, MprisConnection, MprisProperties, MprisRefusal};
use iced_futures::Subscription;
use iced_futures::futures::future::{Either, select};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::Notify;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// The well-known object path every MPRIS player serves, per the spec.
pub const MPRIS_OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";

/// The root interface name.
pub const MPRIS_ROOT_INTERFACE: &str = "org.mpris.MediaPlayer2";

/// The player interface name.
pub const MPRIS_PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";

/// Maximum inbound calls retained while the application is not draining.
pub const MPRIS_CALL_QUEUE_CAPACITY: usize = 64;

/// Maximum pending wake generations for inbound calls.
pub const MPRIS_CALL_WAKE_CAPACITY: usize = 1;

/// Maximum pending wakes for outbound property notifications.
pub const MPRIS_SIGNAL_QUEUE_CAPACITY: usize = 1;

/// Maximum newest refusal diagnostics retained; older entries are counted as dropped.
pub const MPRIS_REFUSAL_LOG_CAPACITY: usize = 32;

/// Deadline for bus connection, name acquisition, and owner verification.
pub const MPRIS_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// Deadline for releasing the served object, the bus name, and the socket.
pub const MPRIS_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// Maximum bus names that may carry outstanding MPRIS release debt at once.
///
/// A finished release frees its entry; an unfinished one is retained so a
/// reconnect cannot race a name this process may still own. Once the table is
/// full, connecting a further name fails with
/// [`ZbusTransportError::ReleaseRegistryFull`] rather than growing the table or
/// evicting a name whose release did not finish.
pub const MPRIS_RELEASE_REGISTRY_CAPACITY: usize = 32;

// Backstop for a thread waiting on a release it does not run. It is longer than
// the release's own deadline so the task's typed timeout normally wins.
const RELEASE_OWNER_TIMEOUT: Duration = Duration::from_secs(MPRIS_SHUTDOWN_TIMEOUT.as_secs() * 2);

const RELEASE_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Errors that prevent the transport from connecting and serving MPRIS.
#[derive(Debug)]
pub enum ZbusTransportError {
    /// The selected D-Bus endpoint was unavailable or refused the connection.
    Connect(String),
    /// The complete connection/name-owner transaction exceeded
    /// [`MPRIS_CONNECT_TIMEOUT`].
    ConnectTimeout {
        /// The deadline that expired.
        deadline: Duration,
    },
    /// The requested bus name could not be configured, acquired, or verified.
    NameNotAcquired(String),
    /// The object could not be served at [`MPRIS_OBJECT_PATH`].
    Serve(String),
    /// The caller cancelled the transaction through [`MprisConnectCancel`]
    /// before it finished.
    ConnectCancelled,
    /// A previous generation of this bus name has not finished releasing it, so
    /// the connect refused instead of racing the bus for ownership.
    NameRetiring {
        /// The settled release outcome, or `None` while the release is still
        /// running.
        outcome: Option<MprisShutdownOutcome>,
    },
    /// Every slot for tracking release debt is taken, so this name's eventual
    /// shutdown could not be accounted for.
    ReleaseRegistryFull {
        /// The configured [`MPRIS_RELEASE_REGISTRY_CAPACITY`].
        capacity: usize,
    },
    /// The background runtime could not be created.
    Runtime(String),
}

impl core::fmt::Display for ZbusTransportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ZbusTransportError::Connect(e) => {
                write!(f, "could not connect to the D-Bus session bus: {e}")
            }
            ZbusTransportError::ConnectTimeout { deadline } => write!(
                f,
                "timed out after {deadline:?} connecting to the D-Bus session bus and acquiring the MPRIS name"
            ),
            ZbusTransportError::NameNotAcquired(e) => {
                write!(f, "could not acquire the MPRIS bus name: {e}")
            }
            ZbusTransportError::Serve(e) => {
                write!(f, "could not serve the MPRIS object: {e}")
            }
            ZbusTransportError::ConnectCancelled => write!(
                f,
                "the MPRIS connect transaction was cancelled before the bus name was acquired"
            ),
            ZbusTransportError::NameRetiring { outcome: None } => write!(
                f,
                "the previous MPRIS generation is still releasing this bus name"
            ),
            ZbusTransportError::NameRetiring {
                outcome: Some(outcome),
            } => write!(
                f,
                "the previous MPRIS generation stopped releasing this bus name: {}",
                outcome.incomplete_reason().map_or(
                    "it stopped without a recorded reason",
                    MprisShutdownIncomplete::as_str
                )
            ),
            ZbusTransportError::ReleaseRegistryFull { capacity } => write!(
                f,
                "this process already tracks {capacity} unfinished MPRIS bus name releases"
            ),
            ZbusTransportError::Runtime(e) => {
                write!(f, "could not create the MPRIS background runtime: {e}")
            }
        }
    }
}

impl std::error::Error for ZbusTransportError {}

// Treat missing owner information as not owned so connection verification fails closed.
#[must_use]
fn owns_name(reported_owner: Option<&str>, our_unique_name: Option<&str>) -> bool {
    match (reported_owner, our_unique_name) {
        (Some(owner), Some(ours)) => owner == ours,
        _ => false,
    }
}

/// Thread-safe properties served to D-Bus clients, initially representing no loaded source.
#[derive(Debug, Clone)]
pub struct SharedProperties {
    inner: Arc<Mutex<PublishedState>>,
}

#[derive(Debug)]
struct PublishedState {
    properties: MprisProperties,
    last_discontinuity: Option<enzui_core::media::PositionDiscontinuity>,
    pending_seek: bool,
}

impl SharedProperties {
    /// A property set describing "no source loaded".
    #[must_use]
    pub fn empty() -> SharedProperties {
        SharedProperties {
            inner: Arc::new(Mutex::new(PublishedState {
                properties: MprisProperties::from_snapshot(
                    &crate::media::MediaState::new().snapshot(),
                ),
                last_discontinuity: None,
                pending_seek: false,
            })),
        }
    }

    /// Replaces the property snapshot without announcing a new seek.
    ///
    /// A pending seek retains the newest position until emitted. Source
    /// replacement discards a pending seek belonging to the previous track.
    pub fn set(&self, properties: &MprisProperties) {
        self.set_with_discontinuity(properties, None);
    }

    fn set_with_discontinuity(
        &self,
        properties: &MprisProperties,
        discontinuity: Option<enzui_core::media::PositionDiscontinuity>,
    ) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if guard.properties.generation != properties.generation {
            guard.pending_seek = false;
            guard.last_discontinuity = None;
        }
        if discontinuity.is_some() && discontinuity != guard.last_discontinuity {
            guard.pending_seek = true;
            guard.last_discontinuity = discontinuity;
        }
        guard.properties = properties.clone();
    }

    fn take_seeked(&self) -> Option<i64> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut guard.pending_seek).then_some(guard.properties.position_micros)
    }

    /// Reads the current property snapshot, recovering a poisoned lock.
    #[must_use]
    pub fn get(&self) -> MprisProperties {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .properties
            .clone()
    }
}

/// Liveness of one MPRIS transport generation.
///
/// A generation stops admitting inbound calls and publications at its first
/// terminal state. Reconnecting produces a new generation, and the caller
/// republishes properties into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MprisTransportState {
    /// The transport owns its bus name and admits calls and publications.
    Live,
    /// The bus connection or the well-known name went away without a local
    /// shutdown.
    Lost,
    /// [`ZbusMprisConnection::begin_shutdown`] ended this generation locally.
    Retired,
}

impl MprisTransportState {
    /// Whether this generation still admits calls and publications.
    #[must_use]
    pub fn is_live(self) -> bool {
        matches!(self, MprisTransportState::Live)
    }
}

impl core::fmt::Display for MprisTransportState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            MprisTransportState::Live => "live",
            MprisTransportState::Lost => "lost",
            MprisTransportState::Retired => "retired",
        })
    }
}

const LIVENESS_LIVE: u8 = 0;
const LIVENESS_LOST: u8 = 1;
const LIVENESS_RETIRED: u8 = 2;

// The interface clones, the call queue, the wake sender and the signal queue
// share one token, so no component can keep serving a generation another
// component has already ended.
#[derive(Debug)]
struct Liveness {
    state: AtomicU8,
}

impl Liveness {
    fn new() -> Liveness {
        Liveness {
            state: AtomicU8::new(LIVENESS_LIVE),
        }
    }

    fn state(&self) -> MprisTransportState {
        match self.state.load(Ordering::Acquire) {
            LIVENESS_LOST => MprisTransportState::Lost,
            LIVENESS_RETIRED => MprisTransportState::Retired,
            _ => MprisTransportState::Live,
        }
    }

    fn is_live(&self) -> bool {
        self.state.load(Ordering::Acquire) == LIVENESS_LIVE
    }

    // Only the caller that ended a live generation gets `true`, so the terminal
    // work runs once even when loss and shutdown race.
    fn end(&self, terminal: MprisTransportState) -> bool {
        let raw = match terminal {
            MprisTransportState::Live => return false,
            MprisTransportState::Lost => LIVENESS_LOST,
            MprisTransportState::Retired => LIVENESS_RETIRED,
        };
        self.state
            .compare_exchange(LIVENESS_LIVE, raw, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
}

#[derive(Debug, Default)]
struct ConnectCancelInner {
    cancelled: AtomicBool,
    woken: Notify,
}

/// Cancels an in-flight MPRIS connect transaction from another thread.
///
/// Clones share one cancellation, so a shutdown path can hold a clone and
/// abandon a connect that has not yet acquired the bus name.
#[derive(Debug, Clone, Default)]
pub struct MprisConnectCancel {
    inner: Arc<ConnectCancelInner>,
}

impl MprisConnectCancel {
    /// A token that has not been cancelled.
    #[must_use]
    pub fn new() -> MprisConnectCancel {
        MprisConnectCancel::default()
    }

    /// Cancels the transaction; already-finished connects are unaffected.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::Release);
        // `notify_one` stores a permit, so a cancellation racing the connect
        // task between its flag check and its await is not lost.
        self.inner.woken.notify_one();
    }

    /// Whether [`MprisConnectCancel::cancel`] has been called.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::Acquire)
    }

    async fn cancelled(&self) {
        loop {
            let woken = self.inner.woken.notified();
            if self.is_cancelled() {
                return;
            }
            woken.await;
        }
    }
}

/// Counts calls to property publication.
#[derive(Debug, Clone, Default)]
pub struct PublishCounter(Arc<AtomicU64>);

impl PublishCounter {
    /// The count so far.
    #[must_use]
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    fn bump(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// A snapshot of the bounded inbound-call queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MprisQueueStats {
    /// Calls currently waiting for the application.
    pub pending: usize,
    /// Hard maximum for [`MprisQueueStats::pending`].
    pub capacity: usize,
    /// Calls appended to the FIFO (not including coalesced setters).
    pub queued: u64,
    /// Adjacent setters replaced by a newer value.
    pub coalesced: u64,
    /// Calls refused without entering the FIFO, for any typed reason.
    pub refused: u64,
    /// Queued calls discarded because the generation ended before the
    /// application drained them.
    pub discarded: u64,
    /// Most recent typed refusal, or `None` while every call has been accepted.
    pub last_refusal: Option<MprisQueueRefusal>,
}

/// Typed reason an inbound MPRIS request did not enter the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MprisQueueRefusal {
    /// The bounded queue already retained its maximum number of calls.
    Full {
        /// The configured queue capacity.
        capacity: usize,
    },
    /// The generation ended, so no accepted call could still reach the
    /// application.
    PlayerGone {
        /// The terminal state that ended the generation.
        state: MprisTransportState,
    },
    /// The application took the sole call-wake receiver and dropped it, so an
    /// accepted call would wait for a consumer that no longer exists.
    NoSubscriber,
}

impl core::fmt::Display for MprisQueueRefusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MprisQueueRefusal::Full { capacity } => {
                write!(f, "MPRIS call queue is full (capacity {capacity})")
            }
            MprisQueueRefusal::PlayerGone { state } => {
                write!(f, "the MPRIS player generation is {state}")
            }
            MprisQueueRefusal::NoSubscriber => write!(
                f,
                "the MPRIS call-wake receiver was taken and dropped, so calls are no longer drained"
            ),
        }
    }
}

impl std::error::Error for MprisQueueRefusal {}

/// One coalesced notification that the inbound call queue may be nonempty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MprisCallWake {
    /// Wrapping generation recorded after the accepted call.
    pub generation: u64,
    /// Transport liveness observed with this wake. Anything other than
    /// [`MprisTransportState::Live`] is the last wake a receiver delivers.
    pub state: MprisTransportState,
}

/// A snapshot of inbound-call wake delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MprisCallWakeStats {
    /// Whether a live receiver has an unobserved accepted-call generation.
    pub pending: usize,
    /// Hard maximum for [`MprisCallWakeStats::pending`].
    pub capacity: usize,
    /// Accepted calls that established a new pending wake.
    pub queued: u64,
    /// Accepted calls folded into an already-pending wake.
    pub coalesced: u64,
    /// Accepted calls whose wake found no live receiver.
    ///
    /// Only a receiver the application never took can be missing here: once it
    /// has been taken and dropped, admission refuses the call with
    /// [`MprisQueueRefusal::NoSubscriber`] instead of accepting and counting it.
    pub subscriber_closed: u64,
    /// Latest accepted-call generation, including calls with no subscriber.
    pub generation: u64,
}

#[derive(Debug, Default)]
struct CallWakeCounters {
    generation: AtomicU64,
    queued: AtomicU64,
    coalesced: AtomicU64,
    subscriber_closed: AtomicU64,
    // Set when the application takes the sole receiver, which is what makes a
    // later upgrade failure mean the consumer is gone rather than not yet
    // adopted.
    taken: AtomicBool,
}

#[derive(Debug, Default)]
struct CallWakeState {
    observed: u64,
    next_waiter: u64,
    waiters: HashMap<u64, Waker>,
}

#[derive(Debug)]
struct CallWakeInner {
    counters: Arc<CallWakeCounters>,
    state: Mutex<CallWakeState>,
    liveness: Arc<Liveness>,
}

#[derive(Debug, Clone)]
struct CallWakeSender {
    counters: Arc<CallWakeCounters>,
    receiver: Weak<CallWakeInner>,
}

impl CallWakeSender {
    fn notify_accepted(&self) {
        let Some(receiver) = self.receiver.upgrade() else {
            self.counters.generation.fetch_add(1, Ordering::AcqRel);
            bump_saturating(&self.counters.subscriber_closed);
            return;
        };

        let wakers = {
            // Advance generations under the waiter-registration lock so either side of the race observes the other.
            let mut state = receiver
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let previous = self.counters.generation.fetch_add(1, Ordering::AcqRel);
            if state.observed == previous {
                bump_saturating(&self.counters.queued);
            } else {
                bump_saturating(&self.counters.coalesced);
            }
            state
                .waiters
                .drain()
                .map(|(_, waker)| waker)
                .collect::<Vec<_>>()
        };
        for waker in wakers {
            waker.wake();
        }
    }

    fn mark_taken(&self) {
        self.counters.taken.store(true, Ordering::Release);
    }

    fn subscriber_departed(&self) -> bool {
        self.counters.taken.load(Ordering::Acquire) && self.receiver.strong_count() == 0
    }

    // Bring every registered waiter to its terminal wake; an ended generation
    // has no later edge left to deliver.
    fn wake_terminal(&self) {
        let Some(receiver) = self.receiver.upgrade() else {
            return;
        };
        let wakers = {
            let mut state = receiver
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state
                .waiters
                .drain()
                .map(|(_, waker)| waker)
                .collect::<Vec<_>>()
        };
        for waker in wakers {
            waker.wake();
        }
    }

    fn stats(&self) -> MprisCallWakeStats {
        let generation = self.counters.generation.load(Ordering::Acquire);
        let pending = self.receiver.upgrade().map_or(0, |receiver| {
            let state = receiver
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            usize::from(state.observed != generation)
        });
        MprisCallWakeStats {
            pending,
            capacity: MPRIS_CALL_WAKE_CAPACITY,
            queued: self.counters.queued.load(Ordering::Relaxed),
            coalesced: self.counters.coalesced.load(Ordering::Relaxed),
            subscriber_closed: self.counters.subscriber_closed.load(Ordering::Relaxed),
            generation,
        }
    }
}

/// Receives coalesced notifications that the inbound call queue may have work.
///
/// Drain [`ZbusMprisConnection::recv_call`] after each wake.
#[derive(Debug)]
pub struct MprisCallWakeReceiver {
    inner: Arc<CallWakeInner>,
}

impl MprisCallWakeReceiver {
    /// Builds an iced subscription for inbound-call wakes.
    ///
    /// Rebuilding preserves identity, and calls accepted before polling remain
    /// observable. The stream ends after one terminal wake, so a retired or
    /// lost transport does not keep an idle runtime awake.
    pub fn subscription(&self) -> Subscription<MprisCallWake> {
        Subscription::run_with(
            MprisCallWakeSubscription {
                inner: Arc::clone(&self.inner),
            },
            |data| {
                let inner = Arc::clone(&data.inner);
                let seen = inner
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .observed;
                iced_futures::futures::stream::unfold(Some((inner, seen)), async |cursor| {
                    let (inner, seen) = cursor?;
                    let wake = MprisCallWakeWait::new(Arc::clone(&inner), seen).await;
                    let next = wake.state.is_live().then_some((inner, wake.generation));
                    Some((wake, next))
                })
            },
        )
    }
}

#[derive(Clone)]
struct MprisCallWakeSubscription {
    inner: Arc<CallWakeInner>,
}

impl std::hash::Hash for MprisCallWakeSubscription {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&Arc::as_ptr(&self.inner), state);
    }
}

struct MprisCallWakeWait {
    inner: Arc<CallWakeInner>,
    seen: u64,
    waiter: Option<u64>,
}

impl MprisCallWakeWait {
    fn new(inner: Arc<CallWakeInner>, seen: u64) -> Self {
        Self {
            inner,
            seen,
            waiter: None,
        }
    }
}

impl Future for MprisCallWakeWait {
    type Output = MprisCallWake;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let mut state = this
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let generation = this.inner.counters.generation.load(Ordering::Acquire);
        // An ended generation is itself an edge: no further call can arrive.
        let liveness = this.inner.liveness.state();
        if !liveness.is_live() || generation != this.seen {
            state.observed = generation;
            if let Some(waiter) = this.waiter.take() {
                state.waiters.remove(&waiter);
            }
            return Poll::Ready(MprisCallWake {
                generation,
                state: liveness,
            });
        }

        let waiter = *this.waiter.get_or_insert_with(|| {
            state.next_waiter = state.next_waiter.wrapping_add(1);
            state.next_waiter
        });
        state.waiters.insert(waiter, cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for MprisCallWakeWait {
    fn drop(&mut self) {
        if let Some(waiter) = self.waiter.take() {
            self.inner
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .waiters
                .remove(&waiter);
        }
    }
}

fn call_wake_channel(liveness: Arc<Liveness>) -> (CallWakeSender, MprisCallWakeReceiver) {
    let counters = Arc::new(CallWakeCounters::default());
    let inner = Arc::new(CallWakeInner {
        counters: Arc::clone(&counters),
        state: Mutex::new(CallWakeState::default()),
        liveness,
    });
    (
        CallWakeSender {
            counters,
            receiver: Arc::downgrade(&inner),
        },
        MprisCallWakeReceiver { inner },
    )
}

#[derive(Debug)]
struct CallQueueState {
    calls: VecDeque<MprisCall>,
    queued: u64,
    coalesced: u64,
    refused: u64,
    discarded: u64,
    last_refusal: Option<MprisQueueRefusal>,
}

// Answer the D-Bus call with the typed reason and retain it for diagnostics.
fn refuse(state: &mut CallQueueState, refusal: MprisQueueRefusal) -> MprisQueueRefusal {
    state.refused = state.refused.saturating_add(1);
    state.last_refusal = Some(refusal);
    refusal
}

#[derive(Debug, Clone)]
struct CallQueue {
    state: Arc<Mutex<CallQueueState>>,
    wakes: CallWakeSender,
    liveness: Arc<Liveness>,
}

impl CallQueue {
    fn new(wakes: CallWakeSender, liveness: Arc<Liveness>) -> Self {
        Self {
            state: Arc::new(Mutex::new(CallQueueState {
                calls: VecDeque::with_capacity(MPRIS_CALL_QUEUE_CAPACITY),
                queued: 0,
                coalesced: 0,
                refused: 0,
                discarded: 0,
                last_refusal: None,
            })),
            wakes,
            liveness,
        }
    }

    fn push(&self, call: MprisCall) -> Result<(), MprisQueueRefusal> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // Admission is decided under the lock that retirement also takes, so a
        // call accepted beside `retire` cannot survive it.
        let liveness = self.liveness.state();
        if !liveness.is_live() {
            return Err(refuse(
                &mut state,
                MprisQueueRefusal::PlayerGone { state: liveness },
            ));
        }
        // A receiver that was adopted and then dropped leaves nobody to drain
        // the FIFO, so admitting the call would queue it forever.
        if self.wakes.subscriber_departed() {
            return Err(refuse(&mut state, MprisQueueRefusal::NoSubscriber));
        }

        // Coalesce only adjacent absolute setters of the same property;
        // toggles and seeks must retain multiplicity and order.
        let coalescible = matches!(
            call,
            MprisCall::SetVolume(_) | MprisCall::SetMute(_) | MprisCall::SetRate(_)
        );
        let same_setter = matches!(
            (state.calls.back(), &call),
            (Some(MprisCall::SetVolume(_)), MprisCall::SetVolume(_))
                | (Some(MprisCall::SetMute(_)), MprisCall::SetMute(_))
                | (Some(MprisCall::SetRate(_)), MprisCall::SetRate(_))
        );
        if coalescible && same_setter {
            if let Some(last) = state.calls.back_mut() {
                *last = call;
            }
            state.coalesced = state.coalesced.saturating_add(1);
            drop(state);
            self.wakes.notify_accepted();
            return Ok(());
        }

        if state.calls.len() == MPRIS_CALL_QUEUE_CAPACITY {
            return Err(refuse(
                &mut state,
                MprisQueueRefusal::Full {
                    capacity: MPRIS_CALL_QUEUE_CAPACITY,
                },
            ));
        }

        state.calls.push_back(call);
        state.queued = state.queued.saturating_add(1);
        drop(state);
        self.wakes.notify_accepted();
        Ok(())
    }

    // Discard the queue so an ended generation cannot hand work to a reducer
    // that has stopped draining, and leave the terminal reason readable.
    fn retire(&self, terminal: MprisTransportState) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dropped = u64::try_from(state.calls.len()).unwrap_or(u64::MAX);
        state.calls.clear();
        state.discarded = state.discarded.saturating_add(dropped);
        state.last_refusal = Some(MprisQueueRefusal::PlayerGone { state: terminal });
    }

    fn pop(&self) -> Option<MprisCall> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .calls
            .pop_front()
    }

    fn stats(&self) -> MprisQueueStats {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        MprisQueueStats {
            pending: state.calls.len(),
            capacity: MPRIS_CALL_QUEUE_CAPACITY,
            queued: state.queued,
            coalesced: state.coalesced,
            refused: state.refused,
            discarded: state.discarded,
            last_refusal: state.last_refusal,
        }
    }
}

/// A snapshot of outbound property-notification pressure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MprisSignalStats {
    /// Notification wakes currently retained for the signal pump.
    pub pending: usize,
    /// Hard maximum for [`MprisSignalStats::pending`].
    pub capacity: usize,
    /// Wakes accepted by the bounded queue.
    pub queued: u64,
    /// Publications folded into an already-pending wake.
    pub coalesced: u64,
    /// Publications that found the signal worker gone.
    ///
    /// This is separate from [`MprisSignalStats::coalesced`]: a closed worker
    /// cannot eventually emit the newest snapshot. The first such publication
    /// ends the generation, so this counter stops at the transition instead of
    /// climbing while callers believe the player is live.
    pub worker_closed: u64,
    /// Publications refused because the generation had already ended.
    ///
    /// An ended generation emits no change signal, so these publications leave
    /// the shared snapshot and [`ZbusMprisConnection::publish_count`] alone.
    pub inert: u64,
}

#[derive(Debug)]
struct SignalQueue {
    sender: Option<mpsc::Sender<SignalRequest>>,
    queued: AtomicU64,
    coalesced: AtomicU64,
    worker_closed: AtomicU64,
    inert: AtomicU64,
}

impl SignalQueue {
    fn new(sender: mpsc::Sender<SignalRequest>) -> Self {
        Self {
            sender: Some(sender),
            queued: AtomicU64::new(0),
            coalesced: AtomicU64::new(0),
            worker_closed: AtomicU64::new(0),
            inert: AtomicU64::new(0),
        }
    }

    fn publish(&self, liveness: &Liveness, wakes: &CallWakeSender) {
        let Some(sender) = self.sender.as_ref() else {
            self.worker_gone(liveness, wakes);
            return;
        };
        match sender.try_send(SignalRequest::PropertiesChanged) {
            Ok(()) => bump_saturating(&self.queued),
            Err(mpsc::error::TrySendError::Full(_)) => bump_saturating(&self.coalesced),
            Err(mpsc::error::TrySendError::Closed(_)) => self.worker_gone(liveness, wakes),
        }
    }

    // The pump is this transport's only emitter, so losing it ends the
    // generation rather than leaving callers publishing into nothing.
    fn worker_gone(&self, liveness: &Liveness, wakes: &CallWakeSender) {
        bump_saturating(&self.worker_closed);
        if liveness.end(MprisTransportState::Lost) {
            wakes.wake_terminal();
        }
    }

    fn record_inert(&self) {
        bump_saturating(&self.inert);
    }

    // Request `Stop` when the bounded queue has room and close the channel in
    // either case, so a full queue cannot swallow the stop.
    fn stop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.try_send(SignalRequest::Stop);
        }
    }

    fn stats(&self) -> MprisSignalStats {
        MprisSignalStats {
            pending: self.sender.as_ref().map_or(0, |sender| {
                MPRIS_SIGNAL_QUEUE_CAPACITY.saturating_sub(sender.capacity())
            }),
            capacity: MPRIS_SIGNAL_QUEUE_CAPACITY,
            queued: self.queued.load(Ordering::Relaxed),
            coalesced: self.coalesced.load(Ordering::Relaxed),
            worker_closed: self.worker_closed.load(Ordering::Relaxed),
            inert: self.inert.load(Ordering::Relaxed),
        }
    }
}

fn bump_saturating(counter: &AtomicU64) {
    let _ = counter.try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(1))
    });
}

// Keep refusal diagnostics in arrival order, evicting the oldest with a saturating drop count.
#[derive(Debug)]
struct RefusalLog {
    entries: Vec<String>,
    capacity: usize,
    dropped: u64,
}

impl RefusalLog {
    fn new(capacity: usize) -> RefusalLog {
        RefusalLog {
            entries: Vec::with_capacity(capacity),
            capacity,
            dropped: 0,
        }
    }

    fn record(&mut self, entry: String) {
        if self.capacity == 0 {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        if self.entries.len() == self.capacity {
            self.entries.remove(0);
            self.dropped = self.dropped.saturating_add(1);
        }
        self.entries.push(entry);
    }

    fn entries(&self) -> &[String] {
        &self.entries
    }

    fn dropped(&self) -> u64 {
        self.dropped
    }
}

/// What a settled MPRIS shutdown achieved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MprisShutdownOutcome {
    /// Both MPRIS interfaces were removed from the object server.
    pub object_removed: bool,
    /// The well-known bus name is no longer owned by this connection.
    pub name_released: bool,
    /// The bus socket was closed.
    pub connection_closed: bool,
    /// Shutdown exceeded [`MPRIS_SHUTDOWN_TIMEOUT`] and stopped where it was.
    pub timed_out: bool,
}

impl MprisShutdownOutcome {
    /// Whether the object, the name and the socket are all gone within the
    /// bound, so a reconnect cannot collide with this generation.
    #[must_use]
    pub fn is_complete(self) -> bool {
        self.incomplete_reason().is_none()
    }

    /// Why this release is terminal but incomplete, or `None` when it finished.
    ///
    /// The reasons are ordered by what a reconnect must act on first: a name
    /// that may still be owned, then a served object, then a socket that can
    /// still carry traffic, and last a bound that expired with every step done.
    #[must_use]
    pub fn incomplete_reason(self) -> Option<MprisShutdownIncomplete> {
        if !self.name_released {
            return Some(MprisShutdownIncomplete::NameRetained);
        }
        if !self.object_removed {
            return Some(MprisShutdownIncomplete::ObjectRetained);
        }
        if !self.connection_closed {
            return Some(MprisShutdownIncomplete::ConnectionOpen);
        }
        if self.timed_out {
            return Some(MprisShutdownIncomplete::TimedOut);
        }
        None
    }
}

/// Why a settled MPRIS release did not finish its work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MprisShutdownIncomplete {
    /// The well-known name was not released, so another generation must not
    /// assume it is free.
    NameRetained,
    /// The served object was not removed, so calls may still reach it.
    ObjectRetained,
    /// The bus socket is still open, so this connection can still carry
    /// traffic for a generation that has ended.
    ConnectionOpen,
    /// The release exceeded [`MPRIS_SHUTDOWN_TIMEOUT`] and stopped where it was.
    TimedOut,
}

impl MprisShutdownIncomplete {
    /// A short description for a diagnostic message.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            MprisShutdownIncomplete::NameRetained => "the bus name was not released",
            MprisShutdownIncomplete::ObjectRetained => "the served object was not removed",
            MprisShutdownIncomplete::ConnectionOpen => "the bus connection is still open",
            MprisShutdownIncomplete::TimedOut => "the release timed out",
        }
    }
}

impl core::fmt::Display for MprisShutdownIncomplete {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a tracked MPRIS bus-name release has reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MprisReleaseStatus {
    /// The release is still running, so the name is not yet free.
    Releasing {
        /// How many owners are still driving it. Every owner records a terminal
        /// outcome as it goes away, so zero cannot persist.
        owners: u32,
    },
    /// The release is terminal. The outcome states what it achieved, including
    /// an incomplete release that left the name owned.
    Settled(MprisShutdownOutcome),
}

#[derive(Debug, Default)]
struct ShutdownProgress {
    object_removed: AtomicBool,
    name_released: AtomicBool,
    connection_closed: AtomicBool,
    timed_out: AtomicBool,
    settled: AtomicBool,
    owners: AtomicU32,
}

impl ShutdownProgress {
    fn is_settled(&self) -> bool {
        self.settled.load(Ordering::Acquire)
    }

    fn owners(&self) -> u32 {
        self.owners.load(Ordering::Acquire)
    }

    // Terminal with no further owner: whatever the flags already say is the
    // whole outcome.
    fn record_settled(&self) {
        self.settled.store(true, Ordering::Release);
    }

    // Terminal because nobody is left to finish it, which a reader must be able
    // to tell apart from a completed release.
    fn record_timed_out(&self) {
        self.timed_out.store(true, Ordering::Release);
        self.settled.store(true, Ordering::Release);
    }

    fn outcome(&self) -> MprisShutdownOutcome {
        MprisShutdownOutcome {
            object_removed: self.object_removed.load(Ordering::Acquire),
            name_released: self.name_released.load(Ordering::Acquire),
            connection_closed: self.connection_closed.load(Ordering::Acquire),
            timed_out: self.timed_out.load(Ordering::Acquire),
        }
    }
}

// Outstanding release debt only: an entry exists while a release runs and
// stays afterwards only when the release did not finish, so a later connect
// cannot race a name this process may still own.
static RELEASES: Mutex<BTreeMap<String, Arc<ShutdownProgress>>> = Mutex::new(BTreeMap::new());

fn releases() -> std::sync::MutexGuard<'static, BTreeMap<String, Arc<ShutdownProgress>>> {
    RELEASES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// Bus names are caller-supplied, so the table never grows past its cap and
// never evicts a name whose release is unfinished.
fn try_track_release(bus_name: &str, progress: Arc<ShutdownProgress>) -> bool {
    let mut releases = releases();
    if let Some(tracked) = releases.get_mut(bus_name) {
        *tracked = progress;
        return true;
    }
    if releases.len() >= MPRIS_RELEASE_REGISTRY_CAPACITY {
        return false;
    }
    let _ = releases.insert(bus_name.to_owned(), progress);
    true
}

// A finished release proves the name is free, so its entry goes; an unfinished
// one stays as debt. The identity check keeps a late finish from discarding a
// newer generation's entry for the same name.
fn finish_release(bus_name: &str, progress: &ShutdownProgress) {
    if !progress.outcome().is_complete() {
        return;
    }
    let mut releases = releases();
    if let Some(tracked) = releases.get(bus_name)
        && std::ptr::eq(Arc::as_ptr(tracked), progress)
    {
        let _ = releases.remove(bus_name);
    }
}

/// How many bus names currently carry outstanding release debt.
///
/// Bounded by [`MPRIS_RELEASE_REGISTRY_CAPACITY`]. A finished release leaves no
/// entry, so a process that shuts its players down cleanly reports zero.
#[must_use]
pub fn tracked_releases() -> usize {
    releases().len()
}

/// The tracked release status of `bus_name`, or `None` when no release debt is
/// outstanding for it.
///
/// A transport dropped inside a tokio context cannot report its own
/// settlement, so this is where that outcome stays readable.
#[must_use]
pub fn release_status(bus_name: &str) -> Option<MprisReleaseStatus> {
    releases().get(bus_name).map(|progress| {
        if progress.is_settled() {
            MprisReleaseStatus::Settled(progress.outcome())
        } else {
            MprisReleaseStatus::Releasing {
                owners: progress.owners(),
            }
        }
    })
}

/// Discards the release debt for `bus_name` once the bus reports no owner.
///
/// `reported_owner` is the bus's own answer for the name, so `None` is the
/// observation that proves it free; any owner keeps the debt, and so does a
/// release that is still running. Returns whether the debt was discarded.
/// Without such an observation an unfinished release keeps refusing reconnects
/// for the life of the process, which is the safe default.
pub fn discard_release_after_owner_check(bus_name: &str, reported_owner: Option<&str>) -> bool {
    if reported_owner.is_some() {
        return false;
    }
    let mut releases = releases();
    if releases
        .get(bus_name)
        .is_some_and(|progress| progress.is_settled())
    {
        return releases.remove(bus_name).is_some();
    }
    false
}

// Refuse a name this process may still own, so two generations never ask the
// bus to arbitrate between them. Debt is never cleared here: an unfinished
// release answers every attempt the same way.
fn admit_bus_name(bus_name: &str) -> Result<(), ZbusTransportError> {
    let mut releases = releases();
    let Some(progress) = releases.get(bus_name) else {
        if releases.len() >= MPRIS_RELEASE_REGISTRY_CAPACITY {
            return Err(ZbusTransportError::ReleaseRegistryFull {
                capacity: MPRIS_RELEASE_REGISTRY_CAPACITY,
            });
        }
        return Ok(());
    };
    if !progress.is_settled() {
        return Err(ZbusTransportError::NameRetiring { outcome: None });
    }

    let outcome = progress.outcome();
    if !outcome.is_complete() {
        return Err(ZbusTransportError::NameRetiring {
            outcome: Some(outcome),
        });
    }
    let _ = releases.remove(bus_name);
    Ok(())
}

// Custody of a release: an owner that goes away, including through a panic,
// records a terminal outcome instead of leaving the release forever pending.
struct ReleaseCustody {
    bus_name: String,
    progress: Arc<ShutdownProgress>,
}

impl ReleaseCustody {
    fn claim(bus_name: String, progress: Arc<ShutdownProgress>) -> ReleaseCustody {
        let _ = progress.owners.fetch_add(1, Ordering::AcqRel);
        ReleaseCustody { bus_name, progress }
    }
}

impl Drop for ReleaseCustody {
    fn drop(&mut self) {
        let remaining = self
            .progress
            .owners
            .fetch_sub(1, Ordering::AcqRel)
            .saturating_sub(1);
        if remaining != 0 {
            return;
        }
        if !self.progress.is_settled() {
            self.progress.record_timed_out();
        }
        finish_release(&self.bus_name, &self.progress);
    }
}

// Wait for a release this thread does not run, then record the bound, so no
// reader is left waiting on an owner that is gone.
fn await_settled(progress: &ShutdownProgress, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while !progress.is_settled() && std::time::Instant::now() < deadline {
        std::thread::sleep(RELEASE_POLL_INTERVAL);
    }
    if !progress.is_settled() {
        progress.record_timed_out();
    }
}

/// A bounded handle to the settlement of [`ZbusMprisConnection::begin_shutdown`].
///
/// The release runs on the transport's own runtime whether or not this handle
/// is polled, and settles within [`MPRIS_SHUTDOWN_TIMEOUT`].
#[derive(Debug)]
pub struct MprisShutdown {
    settled: oneshot::Receiver<MprisShutdownOutcome>,
    progress: Arc<ShutdownProgress>,
    tracked: bool,
}

impl MprisShutdown {
    /// Whether shutdown has settled, without awaiting or blocking.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        self.progress.is_settled()
    }

    /// Whether [`release_status`] can report this release.
    ///
    /// False when every slot up to [`MPRIS_RELEASE_REGISTRY_CAPACITY`] was
    /// taken, in which case this handle is the only way to observe the release.
    #[must_use]
    pub fn is_tracked(&self) -> bool {
        self.tracked
    }

    /// Blocks the calling thread until shutdown settles.
    ///
    /// # Panics
    ///
    /// Panics if called from a thread that is driving a tokio runtime; await
    /// the handle there instead.
    pub fn wait_blocking(self) -> MprisShutdownOutcome {
        let MprisShutdown {
            settled, progress, ..
        } = self;
        settled
            .blocking_recv()
            .unwrap_or_else(|_| progress.outcome())
    }
}

impl Future for MprisShutdown {
    type Output = MprisShutdownOutcome;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let polled = Pin::new(&mut this.settled).poll(cx);
        match polled {
            // A dropped sender means the runtime went away mid-release, so the
            // recorded progress is all the outcome there will ever be.
            Poll::Ready(result) => Poll::Ready(result.unwrap_or_else(|_| this.progress.outcome())),
            Poll::Pending => Poll::Pending,
        }
    }
}

// An interface that is already absent counts as removed, so a shutdown racing
// an earlier removal still settles.
fn removed(result: zbus::Result<bool>) -> bool {
    matches!(result, Ok(_) | Err(zbus::Error::InterfaceNotFound))
}

/// A D-Bus implementation of [`MprisConnection`] with shared property snapshots and bounded notification wakes.
///
/// The transport owns a tokio runtime, so it must be constructed from a thread
/// that is not already driving one. [`ZbusMprisConnection::begin_shutdown`] is
/// the app-facing obligation: it retires the generation without blocking and
/// yields a handle that settles when the object and the name are gone.
///
/// Dropping the transport always retires it and starts the same release. Off a
/// runtime thread the drop waits for settlement; inside a tokio context it
/// returns first and a named owner thread outlives the transport to finish the
/// release or record the bound it stopped at. A release that finishes frees
/// the name and the slot that tracked it; one that does not stays readable
/// through [`release_status`] and refuses every reconnect to that name with
/// [`ZbusTransportError::NameRetiring`] instead of racing the bus for it.
#[derive(Debug)]
pub struct ZbusMprisConnection {
    properties: SharedProperties,
    signals: SignalQueue,
    calls: CallQueue,
    call_wakes: CallWakeSender,
    call_wake_receiver: Option<MprisCallWakeReceiver>,
    published: PublishCounter,
    refusals: RefusalLog,
    bus_name: String,
    liveness: Arc<Liveness>,
    connection: zbus::Connection,
    signal_pump: Option<JoinHandle<()>>,
    release_task: Option<JoinHandle<()>>,
    shutdown: Option<Arc<ShutdownProgress>>,
    // Keep the runtime last so senders and tasks close before runtime teardown during field drop.
    runtime: Option<Runtime>,
}

#[derive(Debug)]
enum SignalRequest {
    PropertiesChanged,
    Stop,
}

impl ZbusMprisConnection {
    /// Connects to the session bus, acquires the app's MPRIS name, and serves its interfaces.
    ///
    /// `app_name` is both the `org.mpris.MediaPlayer2.<app_name>` bus-name
    /// suffix and the user-facing `Identity` property, so it must be a valid
    /// D-Bus name element (ASCII letters, digits, `_` and `-`, not starting
    /// with a digit) and should read as the player's name.
    ///
    /// Blocks the calling thread for up to [`MPRIS_CONNECT_TIMEOUT`], so it
    /// belongs on a worker thread rather than in a reducer; use
    /// [`Self::connect_cancellable`] when a close may need to abandon the
    /// attempt.
    ///
    /// # Errors
    ///
    /// Returns [`ZbusTransportError`] for connection, ownership, serving, or
    /// runtime failure, [`ZbusTransportError::NameRetiring`] when a previous
    /// generation of this name has not finished releasing it, and
    /// [`ZbusTransportError::ReleaseRegistryFull`] when no slot is left to
    /// account for this name's own release.
    ///
    /// # Panics
    ///
    /// Panics if called from a thread that is already driving a tokio runtime,
    /// because the connection transaction blocks on an owned runtime.
    pub fn connect(app_name: &str) -> Result<ZbusMprisConnection, ZbusTransportError> {
        Self::connect_inner(app_name, None, None)
    }

    /// Connects to a specific bus address instead of the session bus.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::connect`].
    ///
    /// # Panics
    ///
    /// Panics under the same condition as [`Self::connect`].
    pub fn connect_to_address(
        app_name: &str,
        address: &str,
    ) -> Result<ZbusMprisConnection, ZbusTransportError> {
        Self::connect_inner(app_name, Some(address.to_owned()), None)
    }

    /// Connects to the session bus with a cancellable transaction.
    ///
    /// [`MprisConnectCancel::cancel`] from another thread abandons the attempt
    /// with [`ZbusTransportError::ConnectCancelled`]; cancelling before the
    /// name is acquired leaves no owner behind.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::connect`], plus
    /// [`ZbusTransportError::ConnectCancelled`].
    ///
    /// # Panics
    ///
    /// Panics under the same condition as [`Self::connect`].
    pub fn connect_cancellable(
        app_name: &str,
        cancel: &MprisConnectCancel,
    ) -> Result<ZbusMprisConnection, ZbusTransportError> {
        Self::connect_inner(app_name, None, Some(cancel.clone()))
    }

    /// Connects to a specific bus address with a cancellable transaction.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::connect_cancellable`].
    ///
    /// # Panics
    ///
    /// Panics under the same condition as [`Self::connect`].
    pub fn connect_to_address_cancellable(
        app_name: &str,
        address: &str,
        cancel: &MprisConnectCancel,
    ) -> Result<ZbusMprisConnection, ZbusTransportError> {
        Self::connect_inner(app_name, Some(address.to_owned()), Some(cancel.clone()))
    }

    fn connect_inner(
        app_name: &str,
        address: Option<String>,
        cancel: Option<MprisConnectCancel>,
    ) -> Result<ZbusMprisConnection, ZbusTransportError> {
        let bus_name = format!("org.mpris.MediaPlayer2.{app_name}");
        // A name whose previous generation may still own it is refused here, so
        // the bus never arbitrates between two generations of one player.
        admit_bus_name(&bus_name)?;
        let properties = SharedProperties::empty();
        let published = PublishCounter::default();
        let liveness = Arc::new(Liveness::new());

        let runtime = Runtime::new().map_err(|e| ZbusTransportError::Runtime(e.to_string()))?;

        let (signal_tx, mut signal_rx) =
            mpsc::channel::<SignalRequest>(MPRIS_SIGNAL_QUEUE_CAPACITY);
        let signals = SignalQueue::new(signal_tx);
        // The application drains a fixed-capacity synchronous queue; a separate coalesced wake activates an idle runtime.
        let (call_wakes, call_wake_receiver) = call_wake_channel(Arc::clone(&liveness));
        let calls = CallQueue::new(call_wakes.clone(), Arc::clone(&liveness));
        let calls_for_interface = calls.clone();

        let props_for_task = properties.clone();
        let name_for_task = bus_name.clone();
        let identity_for_task = app_name.to_owned();

        // Complete bus construction on the runtime before returning, so failures cannot yield a partial transport.
        let connection = runtime.block_on(async move {
            let transaction = async move {
                let player = PlayerInterface {
                    properties: props_for_task.clone(),
                    calls: calls_for_interface,
                };
                let root = RootInterface::for_app(&identity_for_task);

                let builder = match &address {
                    Some(addr) => zbus::connection::Builder::address(addr.as_str())
                        .map_err(|e| ZbusTransportError::Connect(e.to_string()))?,
                    None => zbus::connection::Builder::session()
                        .map_err(|e| ZbusTransportError::Connect(e.to_string()))?,
                };

                let connection = builder
                    // Disable replacement and queuing so another instance cannot steal or wait for the same MPRIS name.
                    .allow_name_replacements(false)
                    .replace_existing_names(false)
                    .name(name_for_task.as_str())
                    .map_err(|e| ZbusTransportError::NameNotAcquired(e.to_string()))?
                    .serve_at(MPRIS_OBJECT_PATH, player)
                    .map_err(|e| ZbusTransportError::Serve(e.to_string()))?
                    .serve_at(MPRIS_OBJECT_PATH, root)
                    .map_err(|e| ZbusTransportError::Serve(e.to_string()))?
                    .build()
                    .await
                    .map_err(|e| ZbusTransportError::Connect(e.to_string()))?;

                // Verify the well-known name's owner with the bus; request-name success alone may not mean this connection owns it.
                let reported_owner = match zbus::fdo::DBusProxy::new(&connection).await {
                    Ok(dbus) => match zbus::names::BusName::try_from(name_for_task.as_str()) {
                        Ok(well_known) => dbus
                            .get_name_owner(well_known)
                            .await
                            .map(|o| o.as_str().to_owned())
                            .ok(),
                        Err(_) => None,
                    },
                    Err(e) => return Err(ZbusTransportError::Connect(e.to_string())),
                };
                let our_unique = connection.unique_name().map(|n| n.as_str().to_owned());

                if !owns_name(reported_owner.as_deref(), our_unique.as_deref()) {
                    return Err(ZbusTransportError::NameNotAcquired(format!(
                        "the bus name {name_for_task} is owned by another process; \
                     another MPRIS player for this app is already running"
                    )));
                }

                Ok::<_, ZbusTransportError>(connection)
            };

            let deadline = tokio::time::timeout(MPRIS_CONNECT_TIMEOUT, transaction);
            let expired = || ZbusTransportError::ConnectTimeout {
                deadline: MPRIS_CONNECT_TIMEOUT,
            };
            match cancel {
                None => deadline.await.unwrap_or_else(|_| Err(expired())),
                // Dropping the transaction mid-flight is what keeps a cancelled
                // attempt from leaving a half-acquired name behind.
                Some(cancel) => {
                    let deadline = pin!(deadline);
                    let cancelled = pin!(cancel.cancelled());
                    match select(deadline, cancelled).await {
                        Either::Left((result, _)) => result.unwrap_or_else(|_| Err(expired())),
                        Either::Right(((), _)) => Err(ZbusTransportError::ConnectCancelled),
                    }
                }
            }
        })?;

        let pump_connection = connection.clone();
        let pump_liveness = Arc::clone(&liveness);
        let pump_wakes = call_wakes.clone();
        let signal_pump = runtime.spawn(async move {
            loop {
                let request = pin!(signal_rx.recv());
                let bus_closed = pin!(pump_connection.closed());
                match select(request, bus_closed).await {
                    Either::Left((Some(SignalRequest::PropertiesChanged), _)) => {
                        // A wake queued before the generation ended describes
                        // properties nobody may read now, so it is dropped
                        // rather than emitted.
                        if !pump_liveness.is_live() {
                            break;
                        }
                        if let Ok(iface_ref) = pump_connection
                            .object_server()
                            .interface::<_, PlayerInterface>(MPRIS_OBJECT_PATH)
                            .await
                        {
                            // Emit change signals only for published properties that can vary; constant capability flags are not signalled.
                            let iface = iface_ref.get().await;
                            let emitter = iface_ref.signal_emitter();
                            if let Some(position) = iface.properties.take_seeked() {
                                let _ = PlayerInterface::seeked(emitter, position).await;
                            }
                            let _ = iface.playback_status_changed(emitter).await;
                            let _ = iface.metadata_changed(emitter).await;
                            let _ = iface.volume_changed(emitter).await;
                            let _ = iface.rate_changed(emitter).await;
                            let _ = iface.can_seek_changed(emitter).await;
                            let _ = iface.can_play_changed(emitter).await;
                            let _ = iface.can_pause_changed(emitter).await;
                            let _ = iface.can_go_next_changed(emitter).await;
                            let _ = iface.can_go_previous_changed(emitter).await;
                        }
                    }
                    Either::Left((Some(SignalRequest::Stop) | None, _)) => break,
                    // Losing the bus ends the generation: no signal can be
                    // emitted and no inbound call can arrive on it again.
                    Either::Right(((), _)) => {
                        if pump_liveness.end(MprisTransportState::Lost) {
                            pump_wakes.wake_terminal();
                        }
                        break;
                    }
                }
            }
        });

        Ok(ZbusMprisConnection {
            properties,
            signals,
            calls,
            call_wakes,
            call_wake_receiver: Some(call_wake_receiver),
            published,
            refusals: RefusalLog::new(MPRIS_REFUSAL_LOG_CAPACITY),
            bus_name,
            liveness,
            connection,
            signal_pump: Some(signal_pump),
            release_task: None,
            shutdown: None,
            runtime: Some(runtime),
        })
    }

    /// The bus name this transport owns.
    #[must_use]
    pub fn bus_name(&self) -> &str {
        &self.bus_name
    }

    /// How many times [`MprisConnection::publish`] has run.
    #[must_use]
    pub fn publish_count(&self) -> u64 {
        self.published.get()
    }

    /// The properties currently visible to D-Bus clients.
    #[must_use]
    pub fn published_properties(&self) -> MprisProperties {
        self.properties.get()
    }

    /// Returns retained refusals from oldest to newest.
    ///
    /// At most [`MPRIS_REFUSAL_LOG_CAPACITY`] newest entries are retained.
    #[must_use]
    pub fn refusals(&self) -> &[String] {
        self.refusals.entries()
    }

    /// Returns the saturating count of refusals evicted from the bounded log.
    #[must_use]
    pub fn refusals_dropped(&self) -> u64 {
        self.refusals.dropped()
    }

    /// Inspect inbound-call queue pressure and overflow/coalescing totals.
    ///
    /// The counters are monotonic for this connection and make overload
    /// observable even though MPRIS has no standard diagnostics property.
    #[must_use]
    pub fn queue_stats(&self) -> MprisQueueStats {
        self.calls.stats()
    }

    /// Takes the sole inbound-call wake receiver.
    ///
    /// Returns `None` after it has already been taken. Dropping the taken
    /// receiver leaves nobody to drain the queue, so later calls are refused
    /// immediately with [`MprisQueueRefusal::NoSubscriber`] rather than queued
    /// for a consumer that will never run.
    pub fn take_call_wake_receiver(&mut self) -> Option<MprisCallWakeReceiver> {
        let receiver = self.call_wake_receiver.take()?;
        self.call_wakes.mark_taken();
        Some(receiver)
    }

    /// Inspect inbound-call wake pressure and delivery classification.
    #[must_use]
    pub fn call_wake_stats(&self) -> MprisCallWakeStats {
        self.call_wakes.stats()
    }

    /// Returns outbound notification pressure, distinguishing coalesced wakes from a closed worker.
    #[must_use]
    pub fn signal_stats(&self) -> MprisSignalStats {
        self.signals.stats()
    }

    /// The liveness of this transport generation.
    ///
    /// Anything other than [`MprisTransportState::Live`] means inbound calls
    /// are refused and publications are inert; recovery is a fresh connection.
    #[must_use]
    pub fn transport_state(&self) -> MprisTransportState {
        self.liveness.state()
    }

    /// Retires this generation and starts releasing the object and the bus name.
    ///
    /// Returns without blocking. From this point inbound calls are refused with
    /// [`MprisQueueRefusal::PlayerGone`], queued calls are discarded, every
    /// registered call-wake waiter receives a terminal [`MprisCallWake`], the
    /// signal worker is stopped and further publications are inert. The
    /// returned handle settles when the object is removed and the name
    /// released, or after [`MPRIS_SHUTDOWN_TIMEOUT`].
    ///
    /// Returns `None` when shutdown has already begun or no runtime is left to
    /// release the name. Retirement still happens in both cases, and
    /// [`Self::shutdown_outcome`] or [`release_status`] reports what the
    /// release achieved. A release started while the debt table is full still
    /// runs, but only through the returned handle; see
    /// [`MprisShutdown::is_tracked`].
    pub fn begin_shutdown(&mut self) -> Option<MprisShutdown> {
        if self.shutdown.is_some() {
            return None;
        }
        // Retirement is local and nonblocking, so it precedes runtime
        // admission: a transport whose runtime is already gone still stops
        // serving calls and publications.
        self.retire();

        let Some(runtime) = self.runtime.as_ref() else {
            // Nothing is left to remove the object or release the name, so the
            // recorded outcome says so rather than implying settlement.
            let progress = Arc::new(ShutdownProgress::default());
            progress.record_settled();
            self.shutdown = Some(Arc::clone(&progress));
            let _ = try_track_release(&self.bus_name, progress);
            return None;
        };

        let progress = Arc::new(ShutdownProgress::default());
        let (settled_tx, settled_rx) = oneshot::channel();
        let task_progress = Arc::clone(&progress);
        let connection = self.connection.clone();
        let name = self.bus_name.clone();
        // Custody is claimed and the debt tracked before the task can be
        // polled, so a task that finishes or is dropped immediately cannot race
        // its own bookkeeping.
        let custody = ReleaseCustody::claim(self.bus_name.clone(), Arc::clone(&progress));
        let tracked = try_track_release(&self.bus_name, Arc::clone(&progress));
        let task = runtime.spawn(async move {
            let release = async {
                let player = connection
                    .object_server()
                    .remove::<PlayerInterface, _>(MPRIS_OBJECT_PATH)
                    .await;
                let root = connection
                    .object_server()
                    .remove::<RootInterface, _>(MPRIS_OBJECT_PATH)
                    .await;
                task_progress
                    .object_removed
                    .store(removed(player) && removed(root), Ordering::Release);

                // A closed connection cannot own a well-known name, because the
                // bus drops every name held by a disconnected client.
                let released = match connection.release_name(name.as_str()).await {
                    Ok(_) => true,
                    Err(_) => connection.is_closed(),
                };
                task_progress
                    .name_released
                    .store(released, Ordering::Release);

                // A peer that vanished first leaves nothing to close, and
                // `close` then reports the write error rather than a
                // connection that is still usable.
                let closed = connection.is_closed() || connection.close().await.is_ok();
                task_progress
                    .connection_closed
                    .store(closed, Ordering::Release);
            };
            let timed_out = tokio::time::timeout(MPRIS_SHUTDOWN_TIMEOUT, release)
                .await
                .is_err();
            task_progress.timed_out.store(timed_out, Ordering::Release);
            task_progress.record_settled();
            let _ = settled_tx.send(task_progress.outcome());
            drop(custody);
        });

        self.shutdown = Some(Arc::clone(&progress));
        self.release_task = Some(task);
        Some(MprisShutdown {
            settled: settled_rx,
            progress,
            tracked,
        })
    }

    // End this generation locally: admission, publication and waiter liveness
    // all stop here, whether or not any runtime can release the name.
    fn retire(&mut self) {
        let terminal = if self.liveness.end(MprisTransportState::Retired) {
            MprisTransportState::Retired
        } else {
            self.liveness.state()
        };
        self.calls.retire(terminal);
        self.call_wakes.wake_terminal();
        self.call_wake_receiver = None;
        self.signals.stop();
    }

    /// The recorded outcome of this transport's shutdown.
    ///
    /// `None` before [`Self::begin_shutdown`] and while the release is still
    /// running; afterwards the outcome is terminal, and
    /// [`MprisShutdownOutcome::incomplete_reason`] says what it could not do.
    #[must_use]
    pub fn shutdown_outcome(&self) -> Option<MprisShutdownOutcome> {
        let progress = self.shutdown.as_ref()?;
        progress.is_settled().then(|| progress.outcome())
    }

    // A tokio context can neither block nor drop the runtime, so the release is
    // handed to a thread that outlives this transport and its runtime; that
    // owner finishes the release or records the bound it stopped at.
    fn hand_release_to_owner_thread(&mut self) {
        let Some(runtime) = self.runtime.take() else {
            return;
        };
        let custody = self
            .shutdown
            .clone()
            .map(|progress| ReleaseCustody::claim(self.bus_name.clone(), progress));
        let (handover, receive) = std::sync::mpsc::channel::<Runtime>();
        let owner = std::thread::Builder::new()
            .name("mpris-release".to_owned())
            .spawn(move || {
                let Ok(runtime) = receive.recv() else {
                    return;
                };
                if let Some(custody) = custody.as_ref() {
                    await_settled(&custody.progress, RELEASE_OWNER_TIMEOUT);
                }
                drop(runtime);
                drop(custody);
            });

        // Dropping a runtime in this context would panic, so every path that
        // cannot hand it over retires it in the background instead and records
        // that the release lost its owner.
        let abandoned = match owner {
            Ok(_owner) => handover.send(runtime).err().map(|returned| returned.0),
            Err(_) => Some(runtime),
        };
        if let Some(runtime) = abandoned {
            runtime.shutdown_background();
            if let Some(progress) = self.shutdown.as_deref()
                && !progress.is_settled()
            {
                progress.record_timed_out();
            }
        }
    }

    /// Whether shutdown has finished every step it owns.
    ///
    /// Requires the generation to have ended, the call queue to be empty and
    /// retired, the signal worker to have joined, this connection's socket to
    /// be closed, the object removed, the name released, and the release task
    /// settled. An incomplete release leaves this false for good;
    /// [`Self::shutdown_outcome`] is the terminal answer in that case.
    #[must_use]
    pub fn is_drained(&self) -> bool {
        if self.liveness.is_live() {
            return false;
        }
        self.calls.stats().pending == 0
            && self.signal_worker_finished()
            && self.connection.is_closed()
            && self
                .release_task
                .as_ref()
                .is_some_and(tokio::task::JoinHandle::is_finished)
            && self
                .shutdown
                .as_ref()
                .is_some_and(|progress| progress.is_settled() && progress.outcome().is_complete())
    }

    fn signal_worker_finished(&self) -> bool {
        self.signal_pump
            .as_ref()
            .is_none_or(tokio::task::JoinHandle::is_finished)
    }

    /// Non-blockingly receives the next inbound MPRIS call.
    ///
    /// Drain after wake notification and pass calls to [`MprisAdapter::handle`](super::MprisAdapter::handle).
    #[must_use]
    pub fn recv_call(&self) -> Option<MprisCall> {
        self.calls.pop()
    }
}

impl MprisConnection for ZbusMprisConnection {
    fn publish(&mut self, properties: &MprisProperties) {
        self.publish_with_discontinuity(properties, None);
    }

    fn publish_with_discontinuity(
        &mut self,
        properties: &MprisProperties,
        discontinuity: Option<enzui_core::media::PositionDiscontinuity>,
    ) {
        // An ended generation cannot emit a change signal, so it must not
        // advance the published snapshot or the publish counter either.
        if !self.liveness.is_live() {
            self.signals.record_inert();
            return;
        }
        self.properties
            .set_with_discontinuity(properties, discontinuity);
        self.published.bump();
        // Attempt the shared snapshot update before offering a bounded, coalescing signal wake.
        self.signals.publish(&self.liveness, &self.call_wakes);
    }

    fn refuse(&mut self, call: &MprisCall, refusal: MprisRefusal) {
        // MPRIS defines no refusal signal, so refusals remain local diagnostics.
        self.refusals.record(format!("{call:?}: {refusal}"));
    }
}

impl Drop for ZbusMprisConnection {
    fn drop(&mut self) {
        // Retirement and starting the release are nonblocking, so they run on
        // every path before deciding who can own the rest.
        drop(self.begin_shutdown());

        if tokio::runtime::Handle::try_current().is_ok() {
            self.hand_release_to_owner_thread();
            return;
        }

        // Off a runtime thread this drop owns settlement, and must not let the
        // runtime's own drop cancel the release it just started.
        if let Some(progress) = self.shutdown.clone() {
            await_settled(&progress, RELEASE_OWNER_TIMEOUT);
        }
    }
}

// Serves the minimal `org.mpris.MediaPlayer2` root interface without window-management capabilities.
struct RootInterface {
    identity: String,
}

impl RootInterface {
    // `Identity` is the user-facing player name a desktop shell prints, never
    // the `org.mpris.MediaPlayer2.*` bus name that carries the same player.
    fn for_app(app_name: &str) -> RootInterface {
        RootInterface {
            identity: app_name.to_owned(),
        }
    }
}

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl RootInterface {
    fn raise(&self) {}

    fn quit(&self) {}

    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn can_raise(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        // The `org.mpris.MediaPlayer2.TrackList` interface is not served, so no
        // client can enumerate or select entries.
        false
    }

    #[zbus(property)]
    fn identity(&self) -> String {
        self.identity.clone()
    }

    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        // This interface serves no `OpenUri` method, so any advertised scheme
        // would promise a call no client can make.
        Vec::new()
    }

    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

mod player;
use player::PlayerInterface;

#[cfg(test)]
mod tests;
