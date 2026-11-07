//! Live desktop font preferences: read them, follow them, and fold them into
//! this theme's [`FontCatalog`](crate::FontCatalog) and the renderer's default
//! font.
//!
//! [`DesktopFontFeed::from_env`] polls GTK and KDE settings files.
//! [`DesktopFontFeed::with_backend`] accepts an injected source, including the
//! portal backend from `enzui-core/desktop-font-portal`. Application
//! overrides win over desktop families; size and style stay in the theme.
//!
//! Apply observations with [`DesktopFontFeed::update`], resolve a fresh theme,
//! and dispatch the task returned by [`DesktopFontFeed::sync_theme`]. Catalog
//! and shared text defaults change synchronously; renderer defaults and window
//! invalidation require the host runtime to execute the task.
//!
//! Reads and signal setup run on a worker and cannot be interrupted by the
//! feed. Dropping its stream suppresses later output and does not join the
//! worker; another stream with the same identity waits for that worker to exit.
//! Signal setup is attempted before the first read; without a signal the worker
//! polls. Latest policy and failure observations occupy separate retained slots,
//! so a stalled consumer can miss intermediate reads, including the initial one.
//! Unsupported observations clear the platform layer; failures retain it.
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use crate::font_preferences::{DesktopFontPreferences, FontPreferencePolicy};
use crate::{EnzTheme, FontFamily, FontRole, RawFontDescription};
use iced_core::Font;
use iced_futures::Subscription;
use iced_futures::futures::Stream;
use iced_runtime::Task;

/// Default interval between reads when no live signal is available.
pub const DEFAULT_FONT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Maximum settings-file prefix read in bytes; larger files are truncated
/// for parsing rather than rejected.
pub const MAX_SETTINGS_FILE_BYTES: u64 = 256 * 1024;

/// The renderer default text size published alongside the resolved UI font.
///
/// This is iced's own default (`iced::Settings::default_text_size`), kept
/// deliberately: this module changes which *family* stock text renders with,
/// and global text scaling is a separate concern. A host that wants the
/// theme's body rung instead calls
/// [`DesktopFontFeed::renderer_text_size`].
pub const DEFAULT_RENDERER_TEXT_SIZE: f32 = 16.0;

/// What one attempt to read the desktop's font preferences produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontPreferenceRead {
    /// The source answered. The snapshot may still be empty (the desktop
    /// expressed no font preference) or carry a malformed description.
    Delivered(DesktopFontPreferences),
    /// This desktop has no such source: no settings file, no portal
    /// implementation, no namespace. Not an error — the lower layers stand,
    /// and a source that disappears mid-session releases what it delivered so
    /// the resolved family does not depend on history.
    Unsupported,
    /// The source exists but the attempt failed (an unreadable file, a bus
    /// error). The last delivered snapshot stays in force — a failure is not
    /// an answer — and the detail is reported.
    Failed {
        /// Backend-supplied failure detail; the feed does not sanitize or bound it.
        detail: String,
    },
}

impl FontPreferenceRead {
    /// The delivered snapshot, if this read produced one.
    #[must_use]
    pub fn preferences(&self) -> Option<&DesktopFontPreferences> {
        match self {
            FontPreferenceRead::Delivered(preferences) => Some(preferences),
            _ => None,
        }
    }
}

/// One observation from a [`DesktopFontFeed`] subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontPreferenceEvent {
    /// The first read, taken after attempting signal setup.
    /// A later read can replace it before the consumer polls.
    Initial(FontPreferenceRead),
    /// A later read, taken because the source signalled a change or the poll
    /// interval elapsed. Only emitted when the snapshot actually differs.
    Changed(FontPreferenceRead),
    /// The live signal ended; emitted before retained reads, then polling continues.
    SignalsLost,
}

/// A live-signal handle owned by the feed's worker thread.
///
/// Implementations block inside [`wait_for_change`](Self::wait_for_change) for
/// at most the supplied timeout so the worker stays cancellable.
pub trait FontPreferenceSignals: Send {
    /// Waits up to `timeout` for the source to report a change.
    fn wait_for_change(&mut self, timeout: Duration) -> FontSignal;

    /// Wait for a change while observing the feed's cancellation handle.
    ///
    /// The default checks cancellation before waiting for at most
    /// `min(timeout, slice)`; cancellation during that wait does not interrupt it.
    /// Backends can override this to wake directly when cancellation fires.
    fn wait_for_change_until_cancelled(
        &mut self,
        timeout: Duration,
        slice: Duration,
        cancel: &SignalCancel,
    ) -> FontSignal {
        if cancel.is_cancelled() {
            return FontSignal::Idle;
        }
        self.wait_for_change(timeout.min(slice))
    }
}

/// Minimum timeout offered to a live signal's cancellation-aware wait.
/// The worker uses the larger of this value and the backend's poll interval;
/// the default wait implementation still limits each wait to one poll slice.
pub const FONT_SIGNAL_SAFETY_INTERVAL: Duration = Duration::from_secs(60);

/// Ends a [`FontPreferenceSignals`] wait when its feed closes.
///
/// Awaitable from async code ([`Self::cancelled`]) and checkable from blocking
/// code ([`Self::is_cancelled`]).
#[derive(Clone, Debug, Default)]
pub struct SignalCancel(Arc<Mutex<CancelState>>);

#[derive(Debug, Default)]
struct CancelState {
    cancelled: bool,
    wakers: Vec<Waker>,
}

impl SignalCancel {
    /// Whether the feed has closed.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.lock().cancelled
    }

    /// Resolves once the feed has closed.
    pub fn cancelled(&self) -> impl std::future::Future<Output = ()> + '_ {
        std::future::poll_fn(move |context| {
            let mut state = self.lock();
            if state.cancelled {
                return std::task::Poll::Ready(());
            }
            if !state.wakers.iter().any(|w| w.will_wake(context.waker())) {
                state.wakers.push(context.waker().clone());
            }
            std::task::Poll::Pending
        })
    }

    /// Ends every current and future wait on this handle.
    pub fn cancel(&self) {
        let wakers = {
            let mut state = self.lock();
            state.cancelled = true;
            std::mem::take(&mut state.wakers)
        };
        for waker in wakers {
            waker.wake();
        }
    }

    fn lock(&self) -> MutexGuard<'_, CancelState> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The outcome of one bounded wait on a live signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontSignal {
    /// The source says something changed; re-read.
    Changed,
    /// The timeout elapsed with no change.
    Idle,
    /// The signal source ended; the feed degrades to polling.
    Closed,
}

/// A blocking desktop-font source used by the feed's worker.
///
/// Signal setup and reads cannot be interrupted by the feed. Backend-level
/// I/O deadlines bound how long an abandoned worker retains its identity.
pub trait DesktopFontBackend: std::fmt::Debug + Send + Sync + 'static {
    /// Stable subscription and worker-admission identity.
    /// Include configuration that must distinguish sources: backend instances
    /// sharing this identity share a subscription and admit one worker at a time.
    fn identity(&self) -> String;

    /// Performs one read.
    ///
    /// Blocking is expected; nothing cancels it, so prefer a backend-level
    /// deadline where the transport offers one.
    fn read(&self) -> FontPreferenceRead;

    /// Attempt signal setup before the first read; `None` selects polling.
    /// Setup may block and is not interrupted when the stream is dropped.
    fn subscribe(&self) -> Option<Box<dyn FontPreferenceSignals>> {
        None
    }

    /// Delay between signal-free reads and the default live-wait slice.
    /// The worker clamps this to at least one millisecond.
    fn poll_interval(&self) -> Duration {
        DEFAULT_FONT_POLL_INTERVAL
    }
}

/// A shareable backend handle that can key a subscription.
///
/// Identity is the backend's [`identity`](DesktopFontBackend::identity), so a
/// feed rebuilt every `subscription()` call does not restart the worker.
#[derive(Clone)]
pub struct FontBackendHandle(Arc<dyn DesktopFontBackend>);

impl FontBackendHandle {
    /// Wraps a backend for use by a [`DesktopFontFeed`].
    #[must_use]
    pub fn new(backend: Arc<dyn DesktopFontBackend>) -> Self {
        Self(backend)
    }

    /// The wrapped backend.
    #[must_use]
    pub fn backend(&self) -> &Arc<dyn DesktopFontBackend> {
        &self.0
    }
}

impl std::fmt::Debug for FontBackendHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("FontBackendHandle")
            .field(&self.0.identity())
            .finish()
    }
}

impl Hash for FontBackendHandle {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.identity().hash(state);
    }
}

/// Why the feed is serving what it is serving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontFeedStatus {
    /// No platform source is enabled.
    Disabled,
    /// A source is enabled; its first read has not arrived yet.
    AwaitingFirstRead,
    /// A snapshot is in force.
    Delivered {
        /// Roles whose delivered description resolves to no family. These fall
        /// through to the theme/generic layers.
        malformed: Vec<FontRole>,
    },
    /// The source reported that this desktop has no font preferences to give.
    /// The platform layer is released: the theme's families apply.
    Unsupported,
    /// The source failed. The last delivered snapshot remains in force.
    Failed {
        /// Backend-supplied detail.
        detail: String,
    },
}

/// Owns the desktop font preference source, its precedence and the renderer
/// default it keeps in step.
#[derive(Debug)]
pub struct DesktopFontFeed {
    backend: Option<FontBackendHandle>,
    policy: FontPreferencePolicy,
    status: FontFeedStatus,
    generation: u64,
    text_size: Option<f32>,
    synced: Option<(Font, f32)>,
    signals_lost: bool,
}

impl DesktopFontFeed {
    /// A feed with no platform source: the theme and generic layers stand.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            backend: None,
            policy: FontPreferencePolicy::new(),
            status: FontFeedStatus::Disabled,
            generation: 0,
            text_size: None,
            synced: None,
            signals_lost: false,
        }
    }

    /// A feed over an explicit backend.
    #[must_use]
    pub fn with_backend(backend: Arc<dyn DesktopFontBackend>) -> Self {
        Self {
            backend: Some(FontBackendHandle::new(backend)),
            policy: FontPreferencePolicy::new(),
            status: FontFeedStatus::AwaitingFirstRead,
            generation: 0,
            text_size: None,
            synced: None,
            signals_lost: false,
        }
    }

    /// Publishes `size` as the renderer default text size instead of
    /// the desktop UI size (or [`DEFAULT_RENDERER_TEXT_SIZE`] without a size preference).
    ///
    /// Non-finite or non-positive values are ignored, so a bad computation
    /// cannot make stock text unrenderable.
    #[must_use]
    pub fn renderer_text_size(mut self, size: f32) -> Self {
        if size.is_finite() && size > 0.0 {
            self.text_size = Some(size);
        }
        self
    }

    /// A dependency-free selection of the desktops' own settings files.
    ///
    /// `ThemeIntegration` selects the portal with watched file fallback by default
    /// when its portal feature is enabled. A manual host that wants the
    /// freedesktop settings portal as well (GNOME's live font keys) enables
    /// `enzui-core/desktop-font-portal` and hands
    /// `enzui_core::font_preferences::portal_with_file_fallback` to
    /// [`with_backend`](Self::with_backend), which keeps this source as its
    /// fallback.
    #[must_use]
    pub fn from_env() -> Self {
        Self::with_backend(Arc::new(SettingsFileFontBackend::from_env()))
    }

    /// Whether a platform source is enabled at all.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.backend.is_some()
    }

    /// The precedence state, for a host that wants to inspect provenance.
    #[must_use]
    pub fn policy(&self) -> &FontPreferencePolicy {
        &self.policy
    }

    /// Why the feed is serving what it serves.
    #[must_use]
    pub fn status(&self) -> &FontFeedStatus {
        &self.status
    }

    /// Whether a signal-loss observation has been applied; sticky for this feed.
    #[must_use]
    pub fn signals_lost(&self) -> bool {
        self.signals_lost
    }

    /// Count of stored platform-snapshot and application-override changes.
    /// Starts at zero and saturates; status-only or theme changes do not count.
    /// A changed snapshot counts even if an override hides its family.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Sets or clears the explicit application override for `role`, the layer
    /// that outranks every desktop answer.
    ///
    /// Returns whether the override changed.
    pub fn set_override(&mut self, role: FontRole, family: Option<FontFamily>) -> bool {
        let changed = self.policy.set_override(role, family);
        if changed {
            self.generation = self.generation.saturating_add(1);
        }
        changed
    }

    /// Subscribe to worker observations, or return no recipe when disabled.
    ///
    /// The worker starts when the stream is first polled. Spawn failure or a
    /// backend panic closes the stream without a synthesized failure observation.
    /// Already retained reads can still be delivered before it ends; no platform
    /// layer is cleared automatically.
    pub fn subscription(&self) -> Subscription<FontPreferenceEvent> {
        let Some(handle) = self.backend.clone() else {
            return Subscription::none();
        };
        Subscription::run_with(handle, |handle| {
            FontPreferenceStream::spawn(Arc::clone(handle.backend()))
        })
    }

    /// Apply an observation and report whether the stored platform snapshot changed.
    /// Status and signal-loss updates alone return false; no task is dispatched.
    pub fn update(&mut self, event: FontPreferenceEvent) -> bool {
        let read = match event {
            FontPreferenceEvent::SignalsLost => {
                self.signals_lost = true;
                return false;
            }
            FontPreferenceEvent::Initial(read) | FontPreferenceEvent::Changed(read) => read,
        };
        let changed = match read {
            FontPreferenceRead::Delivered(preferences) => {
                let malformed = preferences.malformed_roles();
                let changed = self.policy.set_platform(Some(preferences));
                self.status = FontFeedStatus::Delivered { malformed };
                changed
            }
            FontPreferenceRead::Unsupported => {
                let changed = self.policy.set_platform(None);
                self.status = FontFeedStatus::Unsupported;
                changed
            }
            // Transport failure preserves the last platform answer.
            FontPreferenceRead::Failed { detail } => {
                self.status = FontFeedStatus::Failed { detail };
                false
            }
        };
        if changed {
            self.generation = self.generation.saturating_add(1);
        }
        changed
    }

    /// Apply font precedence to a fresh theme and prepare renderer synchronization.
    ///
    /// The catalog must come from a theme document or built-in theme, rather
    /// than an earlier policy application, so withdrawn preferences can fall
    /// back. Shared text defaults are published synchronously; the returned
    /// task must be dispatched to apply renderer defaults or invalidate host
    /// windows. Unchanged defaults return an idle task.
    pub fn sync_theme<Message: Send + 'static>(&mut self, theme: &mut EnzTheme) -> Task<Message> {
        let _ = self.policy.apply(&mut theme.font_catalog);
        self.renderer_sync(theme)
    }

    /// Publish shared text defaults and prepare a renderer-default task.
    ///
    /// `theme` must already have font precedence applied. Its UI family and
    /// regular weight become the requested default, paired with the configured
    /// text size. Dispatch the task to supersede `iced::Settings::default_font`;
    /// use [`set_override`](Self::set_override) to retain an application choice.
    /// Shared role families and weight rungs publish immediately. A change to
    /// those alone returns a task to invalidate windows in the host runtime.
    /// The feed records the requested renderer default before task execution;
    /// discarding the task does not reset that record or cause a later retry.
    pub fn renderer_sync<Message: Send + 'static>(&mut self, theme: &EnzTheme) -> Task<Message> {
        let defaults_changed = crate::text_defaults::publish(theme);
        let font = theme.resolve_font(
            FontRole::Ui,
            theme.typography.iced_weight(crate::WeightRung::Regular),
            iced_core::font::Style::Normal,
            iced_core::font::Stretch::Normal,
        );
        let size = self.text_size.unwrap_or_else(|| {
            if self
                .policy
                .platform()
                .and_then(|fonts| fonts.parsed(FontRole::Ui))
                .and_then(|font| font.size)
                .is_some()
            {
                theme.typography.body_size
            } else {
                DEFAULT_RENDERER_TEXT_SIZE
            }
        });
        if self.synced == Some((font, size)) {
            return if defaults_changed {
                iced_runtime::window::invalidate_all()
            } else {
                Task::none()
            };
        }
        self.synced = Some((font, size));
        iced_runtime::font::set_defaults(font, size)
    }

    /// The renderer default last requested by this feed.
    /// This records task construction, not runtime execution.
    #[must_use]
    pub fn synced_renderer_default(&self) -> Option<(Font, f32)> {
        self.synced
    }
}

impl Default for DesktopFontFeed {
    fn default() -> Self {
        Self::disabled()
    }
}

#[derive(Default)]
struct WorkerState {
    // Separate latest-answer and failure slots keep diagnostics from evicting policy.
    policy_read: Option<(u64, FontPreferenceEvent)>,
    diagnostic_read: Option<(u64, FontPreferenceEvent)>,
    signals_lost: bool,
    closed: bool,
    waker: Option<Waker>,
    generation: u64,
}

#[derive(Default)]
struct WorkerControl {
    state: Mutex<WorkerState>,
    wake: Condvar,
    signal_cancel: SignalCancel,
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
        self.signal_cancel.cancel();
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn is_closed(&self) -> bool {
        self.lock().closed
    }

    fn emit_read(&self, event: FontPreferenceEvent) {
        let waker = {
            let mut state = self.lock();
            if state.closed {
                return;
            }
            state.generation = state.generation.wrapping_add(1);
            let seq = state.generation;
            let is_diagnostic = matches!(
                &event,
                FontPreferenceEvent::Initial(FontPreferenceRead::Failed { .. })
                    | FontPreferenceEvent::Changed(FontPreferenceRead::Failed { .. })
            );
            if is_diagnostic {
                state.diagnostic_read = Some((seq, event));
            } else {
                state.policy_read = Some((seq, event));
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

    // Cancellable sleep: returns false once the stream has been dropped.
    fn sleep(&self, duration: Duration) -> bool {
        let state = self.lock();
        let (state, _) = self
            .wake
            .wait_timeout_while(state, duration, |state| !state.closed)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        !state.closed
    }
}

// Reserving the lease under the registry lock closes the check/spawn race.
const MAX_WORKERS_PER_IDENTITY: usize = 1;

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
            map.remove(identity);
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
    slot.waiting.remove(&ticket);
    slot.workers.retain(|lease| lease.strong_count() > 0);
    if slot.workers.is_empty() && slot.waiting.is_empty() {
        map.remove(identity);
    }
}

struct RunningWorker {
    control: Arc<WorkerControl>,
}

struct FontPreferenceStream {
    identity: String,
    backend: Arc<dyn DesktopFontBackend>,
    running: Option<RunningWorker>,
    waiter: Option<WaiterTicket>,
    seen_policy: u64,
    seen_diagnostic: u64,
    seen_signals_lost: bool,
}

impl FontPreferenceStream {
    fn spawn(backend: Arc<dyn DesktopFontBackend>) -> Self {
        Self {
            identity: backend.identity(),
            backend,
            running: None,
            waiter: None,
            seen_policy: 0,
            seen_diagnostic: 0,
            seen_signals_lost: false,
        }
    }

    fn start(&mut self, waker: &Waker) -> bool {
        let lease = {
            let mut map = lock_admissions();
            let slot = map.entry(self.identity.clone()).or_default();
            slot.workers.retain(|lease| lease.strong_count() > 0);
            if slot.workers.len() >= MAX_WORKERS_PER_IDENTITY {
                let ticket = *self.waiter.get_or_insert_with(WaiterTicket::next);
                slot.waiting.insert(ticket, waker.clone());
                return false;
            }
            if let Some(ticket) = self.waiter.take() {
                slot.waiting.remove(&ticket);
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
                .name("enzui-font-prefs".into())
                .spawn(move || {
                    let _exit = WorkerExit {
                        control: Arc::clone(&control),
                        lease: Some(lease),
                        identity,
                    };
                    run_font_worker(control, backend);
                })
                .is_ok()
        };
        if !spawned {
            // Spawn failure ends the stream without replacing the applied preferences.
            control.close();
            release_admission(&self.identity);
        }
        self.running = Some(RunningWorker { control });
        true
    }
}

impl Stream for FontPreferenceStream {
    type Item = FontPreferenceEvent;

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
        if state.signals_lost && !stream.seen_signals_lost {
            stream.seen_signals_lost = true;
            return Poll::Ready(Some(FontPreferenceEvent::SignalsLost));
        }
        // Answers precede failures, so diagnostics do not mask retained preferences.
        if let Some((seq, event)) = &state.policy_read
            && *seq != stream.seen_policy
        {
            stream.seen_policy = *seq;
            return Poll::Ready(Some(event.clone()));
        }
        if let Some((seq, event)) = &state.diagnostic_read
            && *seq != stream.seen_diagnostic
        {
            stream.seen_diagnostic = *seq;
            return Poll::Ready(Some(event.clone()));
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

impl Drop for FontPreferenceStream {
    fn drop(&mut self) {
        if let Some(ticket) = self.waiter.take() {
            remove_waiter(&self.identity, ticket);
        }
        let Some(running) = self.running.take() else {
            return;
        };
        // Backend I/O is not cancellable, so closing suppresses late output but
        // never waits for the worker.
        running.control.close();
    }
}

struct WorkerExit {
    control: Arc<WorkerControl>,
    lease: Option<Arc<WorkerLease>>,
    identity: String,
}

impl Drop for WorkerExit {
    fn drop(&mut self) {
        self.control.close();
        // The lease must be gone before the slot is reclaimed, or an admitted
        // replacement would count this worker as still running.
        drop(self.lease.take());
        release_admission(&self.identity);
    }
}

fn run_font_worker(control: Arc<WorkerControl>, backend: Arc<dyn DesktopFontBackend>) {
    let interval = backend.poll_interval().max(Duration::from_millis(1));

    // Subscribe before the first read: a change landing between the two is
    // then still signalled instead of being lost until the next poll.
    let mut signals = backend.subscribe();
    let mut last = backend.read();
    control.emit_read(FontPreferenceEvent::Initial(last.clone()));

    while !control.is_closed() {
        let should_read = match signals.as_mut() {
            Some(handle) => match handle.wait_for_change_until_cancelled(
                FONT_SIGNAL_SAFETY_INTERVAL.max(interval),
                interval,
                &control.signal_cancel,
            ) {
                FontSignal::Changed => true,
                FontSignal::Idle => false,
                FontSignal::Closed => {
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
        if control.is_closed() {
            return;
        }
        if !should_read {
            continue;
        }
        let read = backend.read();
        if read != last {
            last = read.clone();
            control.emit_read(FontPreferenceEvent::Changed(read));
        }
    }
}

/// A polling fixture backend that consumes scripted reads and repeats the last.
/// Empty scripts report [`FontPreferenceRead::Unsupported`]. No live signal
/// is provided; the default poll interval is five milliseconds.
#[derive(Debug)]
pub struct ScriptedFontBackend {
    identity: String,
    reads: Mutex<std::collections::VecDeque<FontPreferenceRead>>,
    last: Mutex<FontPreferenceRead>,
    poll_interval: Duration,
}

impl ScriptedFontBackend {
    /// A backend replaying `reads`, then repeating the final one.
    ///
    /// An empty script reports [`FontPreferenceRead::Unsupported`], the same
    /// answer a desktop with no source gives.
    #[must_use]
    pub fn new(identity: impl Into<String>, reads: Vec<FontPreferenceRead>) -> Self {
        Self {
            identity: identity.into(),
            reads: Mutex::new(reads.into_iter().collect()),
            last: Mutex::new(FontPreferenceRead::Unsupported),
            poll_interval: Duration::from_millis(5),
        }
    }

    /// Overrides the poll interval; a test usually wants it short.
    #[must_use]
    pub fn poll_every(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }
}

impl DesktopFontBackend for ScriptedFontBackend {
    fn identity(&self) -> String {
        format!("scripted:{}", self.identity)
    }

    fn read(&self) -> FontPreferenceRead {
        let mut reads = self
            .reads
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(next) = reads.pop_front() {
            *last = next;
        }
        last.clone()
    }

    fn poll_interval(&self) -> Duration {
        self.poll_interval
    }
}

/// Select a primary snapshot containing any raw role description, or try a secondary.
///
/// Malformed descriptions still count as present. If neither snapshot contains
/// an entry, an empty delivered snapshot wins over failure or unsupported,
/// preferring the primary; otherwise the primary result wins. Signals come
/// from the primary when available, then the secondary, independently of which
/// source supplied the latest read. Polling uses the shorter interval.
#[derive(Debug)]
pub struct FallbackFontBackend {
    primary: Arc<dyn DesktopFontBackend>,
    secondary: Arc<dyn DesktopFontBackend>,
}

impl FallbackFontBackend {
    /// Builds a fallback chain of exactly two sources.
    #[must_use]
    pub fn new(
        primary: Arc<dyn DesktopFontBackend>,
        secondary: Arc<dyn DesktopFontBackend>,
    ) -> Self {
        Self { primary, secondary }
    }
}

impl DesktopFontBackend for FallbackFontBackend {
    fn identity(&self) -> String {
        format!(
            "fallback:{}>{}",
            self.primary.identity(),
            self.secondary.identity()
        )
    }

    fn read(&self) -> FontPreferenceRead {
        let primary = self.primary.read();
        if primary.preferences().is_some_and(|p| !p.is_empty()) {
            return primary;
        }
        // A reachable source that answered about no role at all is not a usable
        // answer either, so the secondary still gets its turn.
        let secondary = self.secondary.read();
        if secondary.preferences().is_some_and(|p| !p.is_empty()) {
            return secondary;
        }
        match (&primary, &secondary) {
            // Neither named a font: keep a reachable source's empty answer over
            // an unreachable one's failure, primary first.
            (FontPreferenceRead::Delivered(_), _) => primary,
            (_, FontPreferenceRead::Delivered(_)) => secondary,
            _ => primary,
        }
    }

    fn subscribe(&self) -> Option<Box<dyn FontPreferenceSignals>> {
        match (self.primary.subscribe(), self.secondary.subscribe()) {
            (Some(primary), Some(secondary)) => {
                Some(Box::new(FallbackFontSignals { primary, secondary }))
            }
            (Some(signals), None) | (None, Some(signals)) => Some(signals),
            (None, None) => None,
        }
    }

    fn poll_interval(&self) -> Duration {
        self.primary
            .poll_interval()
            .min(self.secondary.poll_interval())
    }
}

struct FallbackFontSignals {
    primary: Box<dyn FontPreferenceSignals>,
    secondary: Box<dyn FontPreferenceSignals>,
}

impl FontPreferenceSignals for FallbackFontSignals {
    fn wait_for_change(&mut self, timeout: Duration) -> FontSignal {
        let first = timeout / 2;
        match self.primary.wait_for_change(first) {
            FontSignal::Idle => self
                .secondary
                .wait_for_change(timeout.saturating_sub(first)),
            // A lost transport switches the feed to polling, allowing it to reconnect.
            signal => signal,
        }
    }
}

/// Which desktop settings file a description came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SettingsFileKind {
    /// GTK's `settings.ini` (`[Settings] gtk-font-name`), the interface font.
    GtkSettingsIni,
    /// KDE's `kdeglobals` (`[General] font` / `fixed`), the interface and
    /// fixed-width fonts.
    KdeGlobals,
}

impl SettingsFileKind {
    /// The role each key in this file kind maps to.
    ///
    /// These are the desktops' own keys, not a standardised set: GTK publishes
    /// only an interface font, KDE additionally publishes a fixed-width one,
    /// and neither has a document/prose key (GNOME's lives in the portal
    /// namespace read by `portal`).
    #[must_use]
    pub fn keys(self) -> &'static [(&'static str, FontRole)] {
        match self {
            SettingsFileKind::GtkSettingsIni => &[("gtk-font-name", FontRole::Ui)],
            SettingsFileKind::KdeGlobals => {
                &[("font", FontRole::Ui), ("fixed", FontRole::Monospace)]
            }
        }
    }
}

/// Reads desktop font preferences from the desktops' own settings files.
///
/// GTK's `settings.ini` supplies the interface font; KDE's `kdeglobals` also
/// supplies the fixed-width font. Neither supplies a prose font. Candidates
/// are tried in order and the first file containing a supported nonempty key
/// wins, including malformed descriptions; snapshots are not merged.
///
/// Changes are observed by re-reading at the feed's poll interval; these files
/// have no change signal.
#[derive(Debug, Clone)]
pub struct SettingsFileFontBackend {
    candidates: Vec<(PathBuf, SettingsFileKind)>,
    poll_interval: Duration,
}

impl SettingsFileFontBackend {
    /// Builds from explicit candidates, highest precedence first.
    #[must_use]
    pub fn new(candidates: Vec<(PathBuf, SettingsFileKind)>) -> Self {
        Self {
            candidates,
            poll_interval: DEFAULT_FONT_POLL_INTERVAL,
        }
    }

    /// Re-reads this often instead of every [`DEFAULT_FONT_POLL_INTERVAL`].
    ///
    /// Clamped to at least one millisecond. Read time and scheduling add to
    /// this delay, so it does not bound change-detection latency.
    #[must_use]
    pub fn poll_every(mut self, interval: Duration) -> Self {
        self.poll_interval = interval.max(Duration::from_millis(1));
        self
    }

    /// Snapshots the XDG environment once, at construction.
    ///
    /// Order: `$XDG_CONFIG_HOME/gtk-4.0/settings.ini`, the GTK 3 file beside
    /// it, `kdeglobals`, then the same three under each `$XDG_CONFIG_DIRS`
    /// entry. Relative roots are ignored, exactly as the XDG specification
    /// requires.
    #[must_use]
    pub fn from_env() -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let config_home = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| home.map(|home| home.join(".config")));
        let config_dirs: Vec<PathBuf> = match std::env::var_os("XDG_CONFIG_DIRS") {
            Some(value) if !value.is_empty() => std::env::split_paths(&value).collect(),
            _ => vec![PathBuf::from("/etc/xdg")],
        };

        let mut candidates = Vec::new();
        for root in config_home.into_iter().chain(config_dirs) {
            if !root.is_absolute() {
                continue;
            }
            candidates.push((
                root.join("gtk-4.0").join("settings.ini"),
                SettingsFileKind::GtkSettingsIni,
            ));
            candidates.push((
                root.join("gtk-3.0").join("settings.ini"),
                SettingsFileKind::GtkSettingsIni,
            ));
            candidates.push((root.join("kdeglobals"), SettingsFileKind::KdeGlobals));
        }
        Self::new(candidates)
    }

    /// The files this backend reads, in order.
    #[must_use]
    pub fn candidates(&self) -> &[(PathBuf, SettingsFileKind)] {
        &self.candidates
    }
}

impl DesktopFontBackend for SettingsFileFontBackend {
    fn identity(&self) -> String {
        let paths: Vec<String> = self
            .candidates
            .iter()
            .map(|(path, kind)| format!("{kind:?}:{}", path.display()))
            .collect();
        format!("settings-files:{}", paths.join("|"))
    }

    fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    fn read(&self) -> FontPreferenceRead {
        let mut failure: Option<String> = None;
        for (path, kind) in &self.candidates {
            match read_settings_file(path, *kind) {
                Ok(Some(preferences)) => return FontPreferenceRead::Delivered(preferences),
                Ok(None) => {}
                Err(detail) => {
                    // Keep looking: one unreadable candidate must not hide a
                    // readable one further down the list.
                    if failure.is_none() {
                        failure = Some(detail);
                    }
                }
            }
        }
        match failure {
            Some(detail) => FontPreferenceRead::Failed { detail },
            None => FontPreferenceRead::Unsupported,
        }
    }
}

// `Ok(None)` means "this candidate is absent or names no font"; `Err` is a real
// read failure worth reporting.
fn read_settings_file(
    path: &Path,
    kind: SettingsFileKind,
) -> Result<Option<DesktopFontPreferences>, String> {
    use std::io::Read;

    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    match file.metadata() {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    }
    let mut text = String::new();
    if let Err(error) = file
        .by_ref()
        .take(MAX_SETTINGS_FILE_BYTES)
        .read_to_string(&mut text)
    {
        return Err(format!("{}: {error}", path.display()));
    }

    Ok(parse_settings_file(&text, kind))
}

/// Parses one desktop settings file into a preference snapshot.
///
/// Both formats are the same INI shape: `[Section]` headers and `key=value`
/// lines and whole-line `#`/`;` comments. Section and key matching ignores ASCII
/// case; values are trimmed and matching outer quotes removed. The last nonempty
/// value for a role wins. Escapes and inline comments are not interpreted.
/// Returns `None` when no supported key has a nonempty value; malformed font
/// descriptions remain in the snapshot for later diagnosis.
#[must_use]
pub fn parse_settings_file(text: &str, kind: SettingsFileKind) -> Option<DesktopFontPreferences> {
    let section = match kind {
        SettingsFileKind::GtkSettingsIni => "settings",
        SettingsFileKind::KdeGlobals => "general",
    };
    let mut current = String::new();
    let mut preferences = DesktopFontPreferences::new();
    let mut found = false;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = header.trim().to_ascii_lowercase();
            continue;
        }
        if current != section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let Some((_, role)) = kind.keys().iter().find(|(candidate, _)| *candidate == key) else {
            continue;
        };
        let value = unquote(value.trim());
        if value.is_empty() {
            continue;
        }
        // KDE records remain raw; the shared font parser extracts their family.
        preferences.set(*role, Some(RawFontDescription::new(value)));
        found = true;
    }

    found.then_some(preferences)
}

fn unquote(value: &str) -> &str {
    let unquote = |quote| value.strip_prefix(quote)?.strip_suffix(quote);
    if let Some(inner) = unquote('"').or_else(|| unquote('\'')) {
        inner
    } else {
        value
    }
}

#[cfg(test)]
mod tests;
