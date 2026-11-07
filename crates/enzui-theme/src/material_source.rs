//! The live desktop accessibility-preference feed a material obeys.
//!
//! Backends read on feed-owned worker threads; [`MaterialPreferenceFeed::apply`]
//! publishes synchronously and returns a task to invalidate the host runtime's
//! windows when the composed policy changes. The caller must dispatch that task.
//! Constructing a feed replaces the process-wide policy owner and clears its
//! published layers; superseded feeds cannot publish into the new owner's store.
//!
//! Workers attempt signal subscription before the initial read. Without signals
//! they poll; an idle live-signal wait does not trigger a read. Updates coalesce
//! into the latest answer and latest failure, delivered in their retained order,
//! then a pending signal-loss notification. An initial observation can be replaced
//! before the stream consumes it. Backend diagnostics are not size-bounded here.
//!
//! Delivered reads replace the system layer, including unanswered dimensions.
//! Unsupported reads clear it; failed reads preserve the last publication.
//! User overrides win per dimension. Material rendering also joins the live
//! theme's reduced-motion policy through [`crate::material::policy_with_theme_motion`].
//! Linux transports are available through `enzui_core::material_preferences`.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use crate::material::{MaterialPolicy, MaterialPolicyOwner, PreferenceSet, install_policy_owner};
use iced_futures::Subscription;
use iced_futures::futures::Stream;
use iced_runtime::Task;
use std::hash::{Hash, Hasher};

/// Default 30-second delay between reads without signals, also passed to signal waits.
pub const DEFAULT_PREFERENCE_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// What one attempt to read the desktop's accessibility preferences produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaterialPreferenceRead {
    /// The backend supplied per-dimension answers; absent dimensions remain unanswered.
    Delivered(PreferenceSet),
    /// This desktop has no such source: no portal, no namespace, no key. Not
    /// an error, and not a refusal — the system layer is released.
    Unsupported,
    /// The source exists but the attempt failed. The last delivered answers
    /// stay in force and the detail is reported.
    Failed {
        /// Backend-supplied diagnostic; the backend must bound and sanitize it.
        detail: String,
    },
}

impl MaterialPreferenceRead {
    /// The delivered answers, if this read produced any.
    #[must_use]
    pub fn preferences(&self) -> Option<PreferenceSet> {
        match self {
            Self::Delivered(preferences) => Some(*preferences),
            _ => None,
        }
    }
}

/// One observation from a [`MaterialPreferenceFeed`] subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaterialPreferenceEvent {
    /// The first read after attempting signal subscription. A later answer or
    /// failure can replace it before the stream consumes its retained slot.
    Initial(MaterialPreferenceRead),
    /// A read differing from the previous one, triggered by a change signal
    /// or by polling while no signal handle exists.
    Changed(MaterialPreferenceRead),
    /// The live signal ended. The feed continues by polling, and says so
    /// rather than going quiet.
    SignalsLost,
}

/// The outcome of one bounded wait on a live signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreferenceSignal {
    /// The source says something changed; re-read.
    Changed,
    /// The timeout elapsed with no change.
    Idle,
    /// The signal source ended; the feed degrades to polling.
    Closed,
}

/// A live-signal handle owned by the feed's worker thread.
///
/// Implementations must return from [`wait_for_change`](Self::wait_for_change)
/// within the supplied timeout. Stream drop cannot interrupt a wait already
/// in progress; it closes the worker control without joining the thread.
pub trait MaterialPreferenceSignals: Send {
    /// Waits up to `timeout` for the source to report a change.
    fn wait_for_change(&mut self, timeout: Duration) -> PreferenceSignal;
}

/// A source of desktop accessibility preferences.
///
/// Subscription setup, reads, and signal waits run on a feed-owned worker.
/// In-flight backend calls cannot be interrupted, so implementations should
/// use transport deadlines. Stream drop closes delivery without joining the
/// worker; a read can still begin if cancellation races its preceding check.
pub trait MaterialPreferenceBackend: std::fmt::Debug + Send + Sync + 'static {
    /// A stable identity, used to key the subscription.
    ///
    /// Streams sharing an identity admit one worker at a time. A waiting
    /// replacement does not displace a still-live incumbent; it starts after
    /// the incumbent exits. Distinct backends with the same identity also
    /// hash alike, so an iced recipe rebuild can retain the original backend.
    fn identity(&self) -> String;

    /// Performs one read.
    fn read(&self) -> MaterialPreferenceRead;

    /// Establishes a live signal, when the source has one.
    ///
    /// Called *before* the first [`read`](Self::read). The default returns
    /// `None`: the feed then polls at
    /// [`poll_interval`](Self::poll_interval).
    fn subscribe(&self) -> Option<Box<dyn MaterialPreferenceSignals>> {
        None
    }

    /// Delay between reads without signals, also supplied as the signal-wait timeout.
    ///
    /// The feed clamps this to at least one millisecond. A live signal returning
    /// [`PreferenceSignal::Idle`] causes another wait rather than a timed read.
    fn poll_interval(&self) -> Duration {
        DEFAULT_PREFERENCE_POLL_INTERVAL
    }
}

/// A shareable backend handle that can key a subscription.
#[derive(Clone)]
pub struct PreferenceBackendHandle(Arc<dyn MaterialPreferenceBackend>);

impl PreferenceBackendHandle {
    /// Wraps a backend for use by a [`MaterialPreferenceFeed`].
    #[must_use]
    pub fn new(backend: Arc<dyn MaterialPreferenceBackend>) -> Self {
        Self(backend)
    }

    /// The wrapped backend.
    #[must_use]
    pub fn backend(&self) -> &Arc<dyn MaterialPreferenceBackend> {
        &self.0
    }
}

impl std::fmt::Debug for PreferenceBackendHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("PreferenceBackendHandle")
            .field(&self.0.identity())
            .finish()
    }
}

impl Hash for PreferenceBackendHandle {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.identity().hash(state);
    }
}

/// Why the feed is serving what it is serving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreferenceFeedStatus {
    /// No platform backend is enabled; the feed can still publish user overrides.
    Disabled,
    /// A source is enabled; its first read has not arrived yet.
    AwaitingFirstRead,
    /// A read was delivered.
    Delivered {
        /// How many of the three dimensions the desktop actually answered.
        /// Zero is a real outcome: a portal that exports none of the keys.
        answered: usize,
    },
    /// The source reported that this desktop has no preferences to give. The
    /// system layer is released.
    Unsupported,
    /// The source failed. The last delivered answers remain in force.
    Failed {
        /// Backend-supplied detail.
        detail: String,
    },
}

/// Owns one application runtime's accessibility-preference transport and its
/// publication into the process-wide material policy store.
///
/// Constructing one installs a new owner and clears the process-wide layers,
/// superseding any earlier feed or owner. Hold it in application state and
/// offer its subscription while needed. Dropping the current owner clears the
/// layers; dropping a superseded feed leaves the current owner's policy intact.
#[derive(Debug)]
pub struct MaterialPreferenceFeed {
    backend: Option<PreferenceBackendHandle>,
    owner: MaterialPolicyOwner,
    status: PreferenceFeedStatus,
    user_override: PreferenceSet,
    generation: u64,
    signals_lost: bool,
}

impl MaterialPreferenceFeed {
    /// Installs a new policy owner without a platform backend.
    ///
    /// Clears published layers and supersedes the previous owner. User
    /// overrides can still be published through this feed.
    ///
    /// # Panics
    ///
    /// Panics if the process-wide policy-owner generation is exhausted.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            backend: None,
            owner: install_policy_owner(),
            status: PreferenceFeedStatus::Disabled,
            user_override: PreferenceSet::NOTHING,
            generation: 0,
            signals_lost: false,
        }
    }

    /// Installs a new policy owner over an explicit backend.
    ///
    /// Clears published layers and supersedes the previous owner; reading begins
    /// only when the subscription stream is polled.
    ///
    /// # Panics
    ///
    /// Panics if the process-wide policy-owner generation is exhausted.
    #[must_use]
    pub fn with_backend(backend: Arc<dyn MaterialPreferenceBackend>) -> Self {
        Self {
            backend: Some(PreferenceBackendHandle::new(backend)),
            owner: install_policy_owner(),
            status: PreferenceFeedStatus::AwaitingFirstRead,
            user_override: PreferenceSet::NOTHING,
            generation: 0,
            signals_lost: false,
        }
    }

    /// Whether a platform source is enabled at all.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.backend.is_some()
    }

    /// Why the feed is serving what it is serving.
    #[must_use]
    pub fn status(&self) -> &PreferenceFeedStatus {
        &self.status
    }

    /// Whether this feed has processed a signal-loss event.
    #[must_use]
    pub fn signals_lost(&self) -> bool {
        self.signals_lost
    }

    /// Saturating count of policy changes caused by processed backend reads.
    /// Starts at zero and excludes changes through user-override setters.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// The process-wide policy currently published by its active owner.
    ///
    /// A superseded feed reads the new owner's policy, not its own last answers.
    ///
    /// A widget resolves one step further, joining this with the live theme's
    /// motion policy through
    /// [`policy_with_theme_motion`](crate::material::policy_with_theme_motion),
    /// so a diagnostic that must match what a material obeys reads that
    /// instead.
    pub fn policy(&self) -> MaterialPolicy {
        crate::material::current_policy()
    }

    /// The override last requested through this feed.
    /// Supersession can prevent it from becoming the process-wide override.
    pub fn user_override(&self) -> PreferenceSet {
        self.user_override
    }

    /// The backend observation subscription, or none for a disabled feed.
    ///
    /// Recipe identity depends on the backend identity. An unpolled stream
    /// opens no transport; dropping the stream signals cancellation without
    /// waiting for an in-flight backend call. Spawn failure or a backend panic
    /// ends the stream without generating a failure observation; any previously
    /// published policy remains in force.
    pub fn subscription(&self) -> Subscription<MaterialPreferenceEvent> {
        let Some(handle) = self.backend.clone() else {
            return Subscription::none();
        };

        Subscription::run_with(handle, |handle| {
            PreferenceStream::new(Arc::clone(handle.backend()))
        })
    }

    /// Folds one observation and publishes it. Returns whether the composed
    /// policy changed. Status can change even when a superseded owner refuses
    /// publication; this method alone schedules no window invalidation.
    pub fn update(&mut self, event: MaterialPreferenceEvent) -> bool {
        let read = match event {
            MaterialPreferenceEvent::SignalsLost => {
                self.signals_lost = true;
                return false;
            }
            MaterialPreferenceEvent::Initial(read) | MaterialPreferenceEvent::Changed(read) => read,
        };

        let changed = match read {
            MaterialPreferenceRead::Delivered(preferences) => {
                self.status = PreferenceFeedStatus::Delivered {
                    answered: preferences.answered(),
                };
                self.owner.publish_system(preferences)
            }
            MaterialPreferenceRead::Unsupported => {
                self.status = PreferenceFeedStatus::Unsupported;
                self.owner.publish_system(PreferenceSet::NOTHING)
            }
            // Transport failure must not clear an already published preference.
            MaterialPreferenceRead::Failed { detail } => {
                self.status = PreferenceFeedStatus::Failed { detail };
                false
            }
        };

        if changed {
            self.generation = self.generation.saturating_add(1);
        }

        changed
    }

    /// Folds one observation and returns the invalidation the runtime owes it.
    ///
    /// Writes the store synchronously. A changed composed policy returns an
    /// all-window invalidation task for the caller to dispatch; this does not
    /// synchronously render every window or invalidate a different runtime.
    pub fn apply<Message: Send + 'static>(
        &mut self,
        event: MaterialPreferenceEvent,
    ) -> Task<Message> {
        if self.update(event) {
            iced_runtime::window::invalidate_all()
        } else {
            Task::none()
        }
    }

    /// Replaces the in-application user override, and returns the invalidation
    /// the runtime owes it.
    ///
    /// Answers win over the desktop per dimension; unanswered dimensions defer
    /// to it. A superseded feed records the request locally but cannot publish
    /// it. This method does not increment [`Self::generation`].
    pub fn set_user_override<Message: Send + 'static>(
        &mut self,
        preferences: PreferenceSet,
    ) -> Task<Message> {
        self.user_override = preferences;

        if self.owner.set_user_override(preferences) {
            iced_runtime::window::invalidate_all()
        } else {
            Task::none()
        }
    }

    /// Clears this feed's requested override and, if current, the published override.
    /// Does not increment [`Self::generation`].
    pub fn clear_user_override<Message: Send + 'static>(&mut self) -> Task<Message> {
        self.user_override = PreferenceSet::NOTHING;

        if self.owner.clear_user_override() {
            iced_runtime::window::invalidate_all()
        } else {
            Task::none()
        }
    }
}

impl Default for MaterialPreferenceFeed {
    fn default() -> Self {
        Self::disabled()
    }
}

/// A backend that replays a script, for a test that must not need a bus.
///
/// Reads are consumed in order; the last one repeats once the script runs out,
/// so a settled feed stays settled. Each subscription copies the signal script
/// and consumes that copy before returning [`PreferenceSignal::Idle`]. Empty
/// read scripts return [`MaterialPreferenceRead::Unsupported`]. Signal waits
/// return immediately, so polling an exhausted signal script can spin its worker.
#[derive(Debug)]
pub struct ScriptedPreferenceBackend {
    identity: String,
    reads: Mutex<std::collections::VecDeque<MaterialPreferenceRead>>,
    last: Mutex<Option<MaterialPreferenceRead>>,
    signals: Mutex<std::collections::VecDeque<PreferenceSignal>>,
    offers_signals: bool,
    poll_interval: Duration,
}

impl ScriptedPreferenceBackend {
    /// A backend replaying `reads`.
    #[must_use]
    pub fn new(identity: impl Into<String>, reads: Vec<MaterialPreferenceRead>) -> Self {
        Self {
            identity: identity.into(),
            reads: Mutex::new(reads.into()),
            last: Mutex::new(None),
            signals: Mutex::new(std::collections::VecDeque::new()),
            offers_signals: true,
            poll_interval: Duration::from_millis(10),
        }
    }

    /// Supplies the script copied by each subscription, followed by `Idle`.
    #[must_use]
    pub fn with_signals(self, signals: Vec<PreferenceSignal>) -> Self {
        *self
            .signals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = signals.into();
        self
    }

    /// Makes this backend signal-less, so the feed polls.
    #[must_use]
    pub fn without_signals(mut self) -> Self {
        self.offers_signals = false;
        self
    }

    /// Overrides the poll interval.
    #[must_use]
    pub fn poll_every(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// How many scripted reads are left.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.reads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

impl MaterialPreferenceBackend for ScriptedPreferenceBackend {
    fn identity(&self) -> String {
        self.identity.clone()
    }

    fn read(&self) -> MaterialPreferenceRead {
        let next = self
            .reads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front();
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        match next {
            Some(read) => {
                *last = Some(read.clone());
                read
            }
            None => last.clone().unwrap_or(MaterialPreferenceRead::Unsupported),
        }
    }

    fn subscribe(&self) -> Option<Box<dyn MaterialPreferenceSignals>> {
        if !self.offers_signals {
            return None;
        }

        let scripted: Vec<PreferenceSignal> = self
            .signals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .copied()
            .collect();

        Some(Box::new(ScriptedSignals {
            scripted: scripted.into(),
        }))
    }

    fn poll_interval(&self) -> Duration {
        self.poll_interval
    }
}

struct ScriptedSignals {
    scripted: std::collections::VecDeque<PreferenceSignal>,
}

impl MaterialPreferenceSignals for ScriptedSignals {
    fn wait_for_change(&mut self, _timeout: Duration) -> PreferenceSignal {
        self.scripted.pop_front().unwrap_or(PreferenceSignal::Idle)
    }
}

/// A backend that answers from `primary` and falls through to `fallback`.
///
/// Fall-through happens on [`MaterialPreferenceRead::Unsupported`] only: a
/// failed primary keeps its failure. Signal subscription independently selects
/// the primary handle if available, otherwise the fallback; it does not follow
/// whichever backend most recently supplied a read. Polling uses the shorter
/// backend interval.
#[derive(Debug)]
pub struct FallbackPreferenceBackend {
    primary: Arc<dyn MaterialPreferenceBackend>,
    fallback: Arc<dyn MaterialPreferenceBackend>,
}

impl FallbackPreferenceBackend {
    /// Chains two backends.
    #[must_use]
    pub fn new(
        primary: Arc<dyn MaterialPreferenceBackend>,
        fallback: Arc<dyn MaterialPreferenceBackend>,
    ) -> Self {
        Self { primary, fallback }
    }
}

impl MaterialPreferenceBackend for FallbackPreferenceBackend {
    fn identity(&self) -> String {
        format!(
            "fallback:{}|{}",
            self.primary.identity(),
            self.fallback.identity()
        )
    }

    fn read(&self) -> MaterialPreferenceRead {
        match self.primary.read() {
            MaterialPreferenceRead::Unsupported => self.fallback.read(),
            other => other,
        }
    }

    fn subscribe(&self) -> Option<Box<dyn MaterialPreferenceSignals>> {
        self.primary
            .subscribe()
            .or_else(|| self.fallback.subscribe())
    }

    fn poll_interval(&self) -> Duration {
        self.primary
            .poll_interval()
            .min(self.fallback.poll_interval())
    }
}

#[derive(Default)]
struct WorkerState {
    // Separate slots preserve the latest answer and failure under slow consumption.
    answer: Option<(u64, MaterialPreferenceEvent)>,
    diagnostic: Option<(u64, MaterialPreferenceEvent)>,
    signals_lost: bool,
    closed: bool,
    waker: Option<Waker>,
    sequence: u64,
}

#[derive(Default)]
struct WorkerControl {
    state: Mutex<WorkerState>,
    wake: Condvar,
}

impl WorkerControl {
    fn lock(&self) -> MutexGuard<'_, WorkerState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn close(&self) {
        let waker = {
            let mut state = self.lock();
            state.closed = true;
            state.waker.take()
        };
        self.wake.notify_all();

        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn is_closed(&self) -> bool {
        self.lock().closed
    }

    fn emit(&self, event: MaterialPreferenceEvent) {
        let waker = {
            let mut state = self.lock();
            if state.closed {
                return;
            }
            state.sequence = state.sequence.wrapping_add(1);
            let sequence = state.sequence;
            let diagnostic = matches!(
                &event,
                MaterialPreferenceEvent::Initial(MaterialPreferenceRead::Failed { .. })
                    | MaterialPreferenceEvent::Changed(MaterialPreferenceRead::Failed { .. })
            );

            if diagnostic {
                state.diagnostic = Some((sequence, event));
            } else {
                state.answer = Some((sequence, event));
            }

            state.waker.take()
        };

        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn emit_signals_lost(&self) {
        let waker = {
            let mut state = self.lock();
            if state.closed {
                return;
            }
            state.signals_lost = true;
            state.waker.take()
        };

        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn sleep(&self, duration: Duration) -> bool {
        let state = self.lock();
        let (state, _) = self
            .wake
            .wait_timeout_while(state, duration, |state| !state.closed)
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        !state.closed
    }
}

// Reserve under the registry lock before spawning to prevent duplicate identity workers.
const MAX_WORKERS_PER_IDENTITY: usize = 1;

// A backend reporting a zero interval would spin the worker between reads.
const MIN_PREFERENCE_POLL_INTERVAL: Duration = Duration::from_millis(1);

struct WorkerLease;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct WaiterTicket(u64);

impl WaiterTicket {
    fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);

        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Default)]
struct AdmissionSlot {
    workers: Vec<Weak<WorkerLease>>,
    waiting: HashMap<WaiterTicket, Waker>,
}

fn admissions() -> &'static Mutex<HashMap<String, AdmissionSlot>> {
    static ADMISSIONS: OnceLock<Mutex<HashMap<String, AdmissionSlot>>> = OnceLock::new();

    ADMISSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_admissions() -> MutexGuard<'static, HashMap<String, AdmissionSlot>> {
    admissions()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn release_admission(identity: &str) {
    let wakers = {
        let mut map = lock_admissions();
        let Some(slot) = map.get_mut(identity) else {
            return;
        };
        slot.workers.retain(|lease| lease.strong_count() > 0);
        let wakers = slot
            .waiting
            .drain()
            .map(|(_, waker)| waker)
            .collect::<Vec<_>>();
        if slot.workers.is_empty() {
            let _ = map.remove(identity);
        }
        wakers
    };

    for waker in wakers {
        waker.wake();
    }
}

fn remove_waiter(identity: &str, ticket: WaiterTicket) {
    let mut map = lock_admissions();
    let Some(slot) = map.get_mut(identity) else {
        return;
    };
    let _ = slot.waiting.remove(&ticket);
    slot.workers.retain(|lease| lease.strong_count() > 0);
    if slot.workers.is_empty() && slot.waiting.is_empty() {
        let _ = map.remove(identity);
    }
}

struct RunningWorker {
    control: Arc<WorkerControl>,
}

struct PreferenceStream {
    identity: String,
    backend: Arc<dyn MaterialPreferenceBackend>,
    running: Option<RunningWorker>,
    waiter: Option<WaiterTicket>,
}

impl PreferenceStream {
    fn new(backend: Arc<dyn MaterialPreferenceBackend>) -> Self {
        Self {
            identity: backend.identity(),
            backend,
            running: None,
            waiter: None,
        }
    }

    fn start(&mut self, waker: &Waker) -> bool {
        let lease = {
            let mut map = lock_admissions();
            let slot = map.entry(self.identity.clone()).or_default();
            slot.workers.retain(|lease| lease.strong_count() > 0);
            if slot.workers.len() >= MAX_WORKERS_PER_IDENTITY {
                let ticket = *self.waiter.get_or_insert_with(WaiterTicket::next);
                let _ = slot.waiting.insert(ticket, waker.clone());
                return false;
            }
            if let Some(ticket) = self.waiter.take() {
                let _ = slot.waiting.remove(&ticket);
            }
            let lease = Arc::new(WorkerLease);
            slot.workers.push(Arc::downgrade(&lease));
            lease
        };
        let control = Arc::new(WorkerControl::default());
        let spawned = {
            let control = Arc::clone(&control);
            let backend = Arc::clone(&self.backend);
            let identity = self.identity.clone();
            std::thread::Builder::new()
                .name("enzui-material-prefs".into())
                .spawn(move || {
                    let _exit = WorkerExit {
                        control: Arc::clone(&control),
                        lease: Some(lease),
                        identity,
                    };
                    run_worker(backend, control);
                })
                .is_ok()
        };

        if !spawned {
            // Spawn failure ends delivery while leaving the existing policy and status intact.
            control.close();
            release_admission(&self.identity);
        }
        self.running = Some(RunningWorker { control });

        true
    }
}

// Release identity admission on normal return or panic, waking waiting streams.
struct WorkerExit {
    control: Arc<WorkerControl>,
    lease: Option<Arc<WorkerLease>>,
    identity: String,
}

impl Drop for WorkerExit {
    fn drop(&mut self) {
        self.control.close();
        // Drop the lease before reclaiming its admission slot.
        drop(self.lease.take());
        release_admission(&self.identity);
    }
}

fn run_worker(backend: Arc<dyn MaterialPreferenceBackend>, control: Arc<WorkerControl>) {
    // Subscribe before reading so a supporting backend can retain boot-time change signals.
    let mut signals = backend.subscribe();
    let interval = backend.poll_interval().max(MIN_PREFERENCE_POLL_INTERVAL);

    let mut previous = backend.read();
    control.emit(MaterialPreferenceEvent::Initial(previous.clone()));

    while !control.is_closed() {
        let should_read = match signals.as_mut() {
            Some(handle) => match handle.wait_for_change(interval) {
                PreferenceSignal::Changed => true,
                PreferenceSignal::Idle => false,
                PreferenceSignal::Closed => {
                    signals = None;
                    control.emit_signals_lost();
                    false
                }
            },
            None => {
                if !control.sleep(interval) {
                    return;
                }
                true
            }
        };

        // Check cancellation after waiting, before starting another backend read.
        if control.is_closed() {
            return;
        }
        if !should_read {
            continue;
        }

        let read = backend.read();
        if read != previous {
            previous = read.clone();
            control.emit(MaterialPreferenceEvent::Changed(read));
        }
    }
}

impl Drop for PreferenceStream {
    fn drop(&mut self) {
        if let Some(ticket) = self.waiter.take() {
            remove_waiter(&self.identity, ticket);
        }
        let Some(running) = self.running.take() else {
            return;
        };
        running.control.close();
    }
}

impl Stream for PreferenceStream {
    type Item = MaterialPreferenceEvent;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let stream = self.get_mut();
        if stream.running.is_none() && !stream.start(context.waker()) {
            return Poll::Pending;
        }
        let control = &stream
            .running
            .as_ref()
            .expect("a started stream owns its worker control")
            .control;
        let mut state = control.lock();

        // Preserve retained answer/failure order; signal loss follows both slots.
        let answer = state.answer.as_ref().map(|(sequence, _)| *sequence);
        let diagnostic = state.diagnostic.as_ref().map(|(sequence, _)| *sequence);
        let take_answer = match (answer, diagnostic) {
            (Some(a), Some(d)) => a <= d,
            (Some(_), None) => true,
            _ => false,
        };

        if take_answer {
            let (_, event) = state.answer.take().expect("slot was just observed");
            return Poll::Ready(Some(event));
        }

        if let Some((_, event)) = state.diagnostic.take() {
            return Poll::Ready(Some(event));
        }

        if state.signals_lost {
            state.signals_lost = false;
            return Poll::Ready(Some(MaterialPreferenceEvent::SignalsLost));
        }

        if state.closed {
            return Poll::Ready(None);
        }

        if state
            .waker
            .as_ref()
            .is_none_or(|waker| !waker.will_wake(context.waker()))
        {
            state.waker = Some(context.waker().clone());
        }

        Poll::Pending
    }
}

#[cfg(test)]
mod tests;
