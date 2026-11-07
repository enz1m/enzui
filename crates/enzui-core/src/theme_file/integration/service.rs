//! Bounded shared-theme maintenance.
//!
//! Timer, probe, and load work stays off the application message path. Timer
//! wakeups coalesce, each candidate owns at most one probe, one file load may be
//! active, and output retains only the latest theme and diagnostic state.
//! Dropping the stream does not join blocking workers.

use super::{MIN_PROBE_TIMEOUT, ThemeEvent, ThemeWatchDiagnostic};
use crate::file_tree_watch::{AncestorWatch, DirectoryWatchBackend, EventSink, RawEvent};
use crate::file_watch::{FileWatchBackend, FileWatchOutcome, FileWatcher, Probe, ProbeOp};
use crate::theme_file::{ProbeOverride, ThemeLoad, ThemeLoadOp, ThemePaths, ThemeRuntime};
use enzui_theme::EnzThemeMode;
use iced_futures::BoxStream;
use iced_futures::futures::Stream;
use iced_futures::subscription::{self, EventStream, Recipe};
use std::collections::VecDeque;
use std::hash::Hash;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

// A clone held by a worker proves its blocking call may still exist until that
// worker is dropped, so an owned slot is reclaimed only once the clone is gone.
#[derive(Debug)]
pub(super) struct WorkerLease;

fn lease_alive(lease: &Arc<WorkerLease>) -> bool {
    Arc::strong_count(lease) > 1
}

#[derive(Debug)]
pub(super) struct ProbeOwnership {
    pub(super) generation: u64,
    pub(super) started: Instant,
    pub(super) timed_out: bool,
    pub(super) lease: Arc<WorkerLease>,
}

#[derive(Debug)]
struct LoadOwnership {
    generation: u64,
    started: Instant,
    timed_out: bool,
    lease: Arc<WorkerLease>,
}

pub(super) struct ProbeLaunch {
    pub(super) batch: u64,
    pub(super) index: usize,
    pub(super) op: ProbeOp,
    pub(super) lease: Arc<WorkerLease>,
}

pub(super) struct LoadLaunch {
    pub(super) op: ThemeLoadOp,
    pub(super) lease: Arc<WorkerLease>,
}

pub(super) struct TickStep {
    pub(super) probes: Vec<ProbeLaunch>,
    pub(super) load: Option<LoadLaunch>,
    pub(super) events: Vec<ThemeEvent>,
}

pub(super) struct WatchService {
    paths: ThemePaths,
    watchers: Vec<FileWatcher>,
    probe_owners: Vec<Option<ProbeOwnership>>,
    probe_generation: u64,
    load_owner: Option<LoadOwnership>,
    reload_due: Option<Instant>,
    probe_errors: Vec<Option<ThemeWatchDiagnostic>>,
    load_error: Option<ThemeWatchDiagnostic>,
    internal_error: Option<ThemeWatchDiagnostic>,
    watch_error: Option<ThemeWatchDiagnostic>,
    poll_interval: Duration,
    debounce: Duration,
    // Change-detection shadow. Its resolved theme is never read; only its
    // document and status decide delivery, and its generation counter gates a
    // completion older than the newest ACCEPTED load (see `ThemeRuntime::accept`).
    tracker: ThemeRuntime,
}

impl WatchService {
    pub(super) fn new(
        paths: ThemePaths,
        backend: Arc<dyn FileWatchBackend>,
        probe: ProbeOverride,
        initial_os_mode: EnzThemeMode,
        poll_interval: Duration,
        debounce: Duration,
    ) -> Self {
        let watchers: Vec<FileWatcher> = paths
            .watched()
            .into_iter()
            .map(|path| FileWatcher::from_arc(Arc::clone(&backend), path))
            .collect();
        let probe_owners = watchers.iter().map(|_| None).collect::<Vec<_>>();
        let probe_errors = watchers.iter().map(|_| None).collect::<Vec<_>>();
        Self {
            paths,
            watchers,
            probe_owners,
            probe_generation: 0,
            load_owner: None,
            reload_due: None,
            probe_errors,
            load_error: None,
            internal_error: None,
            watch_error: None,
            poll_interval,
            debounce,
            tracker: ThemeRuntime::new(probe, initial_os_mode),
        }
    }

    fn worker_timeout(&self) -> Duration {
        self.poll_interval.saturating_mul(2).max(MIN_PROBE_TIMEOUT)
    }

    // The earliest instant a tick has work: a debounced reload falling due,
    // or a load or probe reaching its timeout.
    pub(super) fn next_deadline(&self) -> Option<Instant> {
        let timeout = self.worker_timeout();
        let load = self
            .load_owner
            .as_ref()
            .filter(|owner| !owner.timed_out)
            .and_then(|owner| owner.started.checked_add(timeout));
        let probes = self
            .probe_owners
            .iter()
            .flatten()
            .filter(|owner| !owner.timed_out)
            .filter_map(|owner| owner.started.checked_add(timeout));
        self.reload_due.into_iter().chain(load).chain(probes).min()
    }

    fn launch_load(&mut self, now: Instant) -> Option<LoadLaunch> {
        if self.load_owner.is_some() {
            return None;
        }
        let generation = self.tracker.next_generation();
        let lease = Arc::new(WorkerLease);
        self.load_owner = Some(LoadOwnership {
            generation,
            started: now,
            timed_out: false,
            lease: Arc::clone(&lease),
        });
        Some(LoadLaunch {
            op: ThemeLoadOp::new(&self.paths, generation),
            lease,
        })
    }

    pub(super) fn begin_initial_load(&mut self, now: Instant) -> Option<LoadLaunch> {
        self.launch_load(now)
    }

    pub(super) fn on_tick(&mut self, now: Instant) -> TickStep {
        let before = self.watch_error.clone();
        let timeout = self.worker_timeout();

        if let Some(owner) = &mut self.load_owner {
            if !owner.timed_out {
                if now.saturating_duration_since(owner.started) >= timeout {
                    owner.timed_out = true;
                    self.load_error = Some(ThemeWatchDiagnostic::LoadStalled);
                }
            } else if !lease_alive(&owner.lease) {
                self.load_owner = None;
                self.reload_due = Some(now);
                self.load_error = Some(ThemeWatchDiagnostic::LoadAbandoned);
            }
        }

        let mut load = None;
        if self.reload_due.is_some_and(|due| now >= due)
            && let Some(launch) = self.launch_load(now)
        {
            self.reload_due = None;
            load = Some(launch);
        }

        for (index, slot) in self.probe_owners.iter_mut().enumerate() {
            let Some(owner) = slot else {
                continue;
            };
            if !owner.timed_out {
                if now.saturating_duration_since(owner.started) >= timeout {
                    owner.timed_out = true;
                    self.probe_errors[index] = Some(ThemeWatchDiagnostic::ProbeTimedOut {
                        path: self.watchers[index].path().to_path_buf(),
                    });
                }
                continue;
            }
            if !lease_alive(&owner.lease) {
                *slot = None;
                self.probe_errors[index] = Some(ThemeWatchDiagnostic::ProbeAbandoned {
                    path: self.watchers[index].path().to_path_buf(),
                });
            }
        }

        let vacant = self
            .probe_owners
            .iter()
            .enumerate()
            .filter_map(|(index, owner)| owner.is_none().then_some(index))
            .collect::<Vec<_>>();
        let mut probes = Vec::new();
        if !vacant.is_empty() {
            self.probe_generation = self.probe_generation.wrapping_add(1);
            let batch = self.probe_generation;
            for index in vacant {
                let lease = Arc::new(WorkerLease);
                self.probe_owners[index] = Some(ProbeOwnership {
                    generation: batch,
                    started: now,
                    timed_out: false,
                    lease: Arc::clone(&lease),
                });
                let op = self.watchers[index].prepare_probe();
                probes.push(ProbeLaunch {
                    batch,
                    index,
                    op,
                    lease,
                });
            }
        }

        self.refresh_watch_error();
        let events = self.diagnostic_change(before.as_ref());
        TickStep {
            probes,
            load,
            events,
        }
    }

    pub(super) fn on_probe(
        &mut self,
        batch: u64,
        index: usize,
        probe: Probe,
        now: Instant,
    ) -> Vec<ThemeEvent> {
        let before = self.watch_error.clone();
        self.complete_watch(batch, index, probe, now);
        self.refresh_watch_error();
        self.diagnostic_change(before.as_ref())
    }

    pub(super) fn on_load(&mut self, load: ThemeLoad) -> Vec<ThemeEvent> {
        let before = self.watch_error.clone();
        match &self.load_owner {
            Some(owner) if owner.generation == load.generation => {
                self.load_owner = None;
            }
            Some(_) => return Vec::new(),
            #[cfg(not(test))]
            None => return Vec::new(),
            #[cfg(test)]
            None => {}
        }

        let before_doc = self.tracker.document().clone();
        let before_status = self.tracker.status().clone();
        let committed = load.clone();
        let mut events = Vec::new();
        if self.tracker.accept(load) {
            if self.tracker.document() != &before_doc || self.tracker.status() != &before_status {
                events.push(ThemeEvent::Loaded(committed));
            }
            self.load_error = None;
        }
        self.refresh_watch_error();
        events.extend(self.diagnostic_change(before.as_ref()));
        events
    }

    fn refresh_watch_error(&mut self) {
        self.watch_error = self
            .internal_error
            .clone()
            .or_else(|| self.load_error.clone())
            .or_else(|| self.probe_errors.iter().flatten().next().cloned());
    }

    fn diagnostic_change(&self, before: Option<&ThemeWatchDiagnostic>) -> Vec<ThemeEvent> {
        if self.watch_error.as_ref() != before {
            vec![ThemeEvent::WatchDiagnostic(self.watch_error.clone())]
        } else {
            Vec::new()
        }
    }

    fn complete_watch(&mut self, batch: u64, index: usize, probe: Probe, now: Instant) {
        let Some(owner_slot) = self.probe_owners.get_mut(index) else {
            self.internal_error = Some(ThemeWatchDiagnostic::InvalidResultIndex { index });
            return;
        };
        let Some(owner) = owner_slot.as_ref() else {
            return;
        };
        if batch != owner.generation {
            return;
        }
        let was_timed_out = owner.timed_out;
        *owner_slot = None;
        if was_timed_out {
            return;
        }
        self.observe_watch(index, probe, now);
    }

    fn observe_watch(&mut self, index: usize, probe: Probe, now: Instant) {
        let Some(watcher) = self.watchers.get_mut(index) else {
            self.internal_error = Some(ThemeWatchDiagnostic::InvalidResultIndex { index });
            return;
        };
        match watcher.observe(probe) {
            FileWatchOutcome::NoBaseline(_) => {
                self.reload_due = Some(now);
                self.probe_errors[index] = None;
            }
            FileWatchOutcome::Modified { .. }
            | FileWatchOutcome::Replaced { .. }
            | FileWatchOutcome::Deleted { .. }
            | FileWatchOutcome::Appeared { .. } => {
                self.reload_due = crate::monotonic_ms::checked_instant_deadline(
                    now,
                    self.debounce,
                    crate::monotonic_ms::Policy::Clamp,
                );
                self.probe_errors[index] = None;
            }
            FileWatchOutcome::Unchanged => self.probe_errors[index] = None,
            FileWatchOutcome::Superseded => {
                // Not this watcher's current request: it reports nothing about
                // current reality, so leave the reload and error state intact.
            }
            FileWatchOutcome::ProbeFailed(error) => {
                self.probe_errors[index] = Some(ThemeWatchDiagnostic::ProbeFailed {
                    path: watcher.path().to_path_buf(),
                    error,
                });
            }
        }
    }
}

struct Inbox {
    tick_pending: bool,
    probes: VecDeque<(u64, usize, Probe)>,
    loads: VecDeque<ThemeLoad>,
    closed: bool,
    probe_capacity: usize,
}

impl Inbox {
    fn new(probe_capacity: usize) -> Self {
        Self {
            tick_pending: false,
            probes: VecDeque::new(),
            loads: VecDeque::new(),
            closed: false,
            probe_capacity,
        }
    }

    fn has_work(&self) -> bool {
        self.tick_pending || !self.probes.is_empty() || !self.loads.is_empty()
    }
}

// One live probe per candidate bounds `probes`; one-active load ownership bounds
// `loads`. The second slot absorbs a single retired straggler completing late.
pub(super) const LOAD_CAPACITY: usize = 2;

/// The regular tick while a directory watch covers every theme path. Events
/// tick at once; this only catches a change the watch could not report.
pub(super) const LIVE_WATCH_SAFETY_INTERVAL: Duration = Duration::from_secs(60);

/// Which directory watch turns theme-file changes into ticks.
#[derive(Clone)]
pub(super) enum DirectoryWatch {
    /// Poll on the regular interval only.
    Off,
    /// The platform's native watch, created when the service starts.
    Native,
    /// A supplied backend.
    Backend(Arc<dyn DirectoryWatchBackend>),
}

impl DirectoryWatch {
    fn backend(&self) -> Option<Arc<dyn DirectoryWatchBackend>> {
        match self {
            Self::Off => None,
            Self::Backend(backend) => Some(Arc::clone(backend)),
            Self::Native => crate::file_tree_watch::native_directory_watch(),
        }
    }
}

// Any event in a watched directory asks the service for a tick.
struct TickSink(std::sync::Weak<ServiceShared>);

impl EventSink for TickSink {
    fn push(&self, _event: RawEvent) {
        if let Some(shared) = self.0.upgrade() {
            shared.push_tick();
        }
    }
}

// The coalescing output the recipe stream consumes: only the newest theme and
// the newest diagnostic survive, so alternating updates never accumulate.
#[derive(Default)]
struct OutMailbox {
    theme: Option<ThemeLoad>,
    diagnostic: Option<Option<ThemeWatchDiagnostic>>,
    waker: Option<Waker>,
    closed: bool,
}

pub(super) struct ServiceShared {
    inbox: Mutex<Inbox>,
    inbox_wake: Condvar,
    out: Mutex<OutMailbox>,
}

impl ServiceShared {
    pub(super) fn new(probe_capacity: usize) -> Self {
        Self {
            inbox: Mutex::new(Inbox::new(probe_capacity)),
            inbox_wake: Condvar::new(),
            out: Mutex::new(OutMailbox::default()),
        }
    }

    fn lock_inbox(&self) -> MutexGuard<'_, Inbox> {
        self.inbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock_out(&self) -> MutexGuard<'_, OutMailbox> {
        self.out
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn push_tick(&self) {
        let mut inbox = self.lock_inbox();
        if inbox.closed {
            return;
        }
        inbox.tick_pending = true; // coalesced, capacity-one
        drop(inbox);
        self.inbox_wake.notify_all();
    }

    pub(super) fn push_probe(&self, batch: u64, index: usize, probe: Probe) {
        let mut inbox = self.lock_inbox();
        if inbox.closed {
            return;
        }
        if inbox.probes.len() < inbox.probe_capacity {
            inbox.probes.push_back((batch, index, probe));
        }
        drop(inbox);
        self.inbox_wake.notify_all();
    }

    pub(super) fn push_load(&self, load: ThemeLoad) {
        let mut inbox = self.lock_inbox();
        if inbox.closed {
            return;
        }
        if inbox.loads.len() < LOAD_CAPACITY {
            inbox.loads.push_back(load);
        }
        drop(inbox);
        self.inbox_wake.notify_all();
    }

    // Out-of-band cancellation: the service checks `closed` before any pending
    // work, so this never waits behind a queue or a blocking worker.
    fn close_inbox(&self) {
        self.lock_inbox().closed = true;
        self.inbox_wake.notify_all();
    }

    fn push_out(&self, events: Vec<ThemeEvent>) {
        if events.is_empty() {
            return;
        }
        let waker = {
            let mut out = self.lock_out();
            for event in events {
                match event {
                    ThemeEvent::Loaded(load) => out.theme = Some(load),
                    ThemeEvent::WatchDiagnostic(diagnostic) => out.diagnostic = Some(diagnostic),
                    // Platform preferences have independent subscriptions and
                    // never enter the file-service mailbox.
                    ThemeEvent::OsBootstrap(_)
                    | ThemeEvent::OsChanged(_)
                    | ThemeEvent::ReducedMotionBootstrap(_)
                    | ThemeEvent::ReducedMotionChanged(_)
                    | ThemeEvent::Fonts(_)
                    | ThemeEvent::Desktop(_) => {}
                }
            }
            out.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn close_out(&self) {
        let waker = {
            let mut out = self.lock_out();
            out.closed = true;
            out.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

#[derive(Default)]
struct TimerState {
    closed: bool,
    // Set by the service: tick no later than this.
    wake_at: Option<Instant>,
    // A directory watch covers every theme path, so the regular tick is only
    // a safety net.
    live_watch: bool,
    #[cfg(test)]
    producer_parked: bool,
}

#[derive(Default)]
pub(super) struct TimerControl {
    state: Mutex<TimerState>,
    wake: Condvar,
}

impl TimerControl {
    fn lock(&self) -> MutexGuard<'_, TimerState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn close(&self) {
        self.lock().closed = true;
        self.wake.notify_all();
    }

    fn set_wake_at(&self, at: Option<Instant>) {
        let mut state = self.lock();
        if state.wake_at != at {
            state.wake_at = at;
            drop(state);
            self.wake.notify_all();
        }
    }

    fn set_live_watch(&self, live: bool) {
        let mut state = self.lock();
        if state.live_watch != live {
            state.live_watch = live;
            drop(state);
            self.wake.notify_all();
        }
    }
}

fn run_timer(control: Arc<TimerControl>, interval: Duration, shared: Arc<ServiceShared>) {
    struct TimerExit(Arc<TimerControl>);
    impl Drop for TimerExit {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _exit = TimerExit(Arc::clone(&control));

    let interval = interval.max(Duration::from_millis(1));
    loop {
        let started = Instant::now();
        let mut state = control.lock();
        loop {
            if state.closed {
                return;
            }
            let regular = if state.live_watch {
                interval.max(LIVE_WATCH_SAFETY_INTERVAL)
            } else {
                interval
            };
            let deadline = match (started.checked_add(regular), state.wake_at) {
                (Some(regular), Some(wake_at)) => Some(regular.min(wake_at)),
                (regular, wake_at) => regular.or(wake_at),
            };
            let Some(deadline) = deadline else {
                // An unrepresentable deadline cannot elapse; park until woken.
                #[cfg(test)]
                {
                    state.producer_parked = true;
                    control.wake.notify_all();
                }
                state = control
                    .wake
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                continue;
            };
            let now = Instant::now();
            if now >= deadline {
                if state.wake_at.is_some_and(|wake_at| wake_at <= now) {
                    state.wake_at = None;
                }
                break;
            }
            state = match control.wake.wait_timeout(state, deadline - now) {
                Ok((state, _)) => state,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
        drop(state);
        shared.push_tick();
    }
}

fn spawn_probe(shared: &Arc<ServiceShared>, launch: ProbeLaunch) {
    let ProbeLaunch {
        batch,
        index,
        op,
        lease,
    } = launch;
    let shared = Arc::clone(shared);
    // A failed spawn drops the lease with the un-run closure, so the next tick
    // observes the slot as abandoned and retries it.
    let _ = std::thread::Builder::new()
        .name("enzui-theme-probe".into())
        .spawn(move || {
            let _lease = lease; // released only when this worker ends
            let probe = op.execute();
            shared.push_probe(batch, index, probe);
        });
}

fn spawn_load(shared: &Arc<ServiceShared>, launch: LoadLaunch) {
    let LoadLaunch { op, lease } = launch;
    let shared = Arc::clone(shared);
    let _ = std::thread::Builder::new()
        .name("enzui-theme-load".into())
        .spawn(move || {
            let _lease = lease;
            let load = op.execute();
            shared.push_load(load);
        });
}

fn run_service(
    mut service: WatchService,
    shared: Arc<ServiceShared>,
    timer_control: Arc<TimerControl>,
    timer: Option<JoinHandle<()>>,
    mut dir_watch: Option<AncestorWatch>,
) {
    if let Some(launch) = service.begin_initial_load(Instant::now()) {
        spawn_load(&shared, launch);
    }

    loop {
        let (tick, probes, loads) = {
            let mut inbox = shared.lock_inbox();
            while !inbox.closed && !inbox.has_work() {
                inbox = shared
                    .inbox_wake
                    .wait(inbox)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if inbox.closed {
                break;
            }
            let tick = std::mem::take(&mut inbox.tick_pending);
            let probes = std::mem::take(&mut inbox.probes);
            let loads = std::mem::take(&mut inbox.loads);
            (tick, probes, loads)
        };

        // Loads and probes first, so a transition they record is seen by the
        // tick that launches the reload in the same drain.
        for load in loads {
            let events = service.on_load(load);
            shared.push_out(events);
        }
        for (batch, index, probe) in probes {
            let events = service.on_probe(batch, index, probe, Instant::now());
            shared.push_out(events);
        }
        if tick {
            if let Some(watch) = dir_watch.as_mut() {
                timer_control.set_live_watch(watch.refresh());
            }
            let step = service.on_tick(Instant::now());
            for probe in step.probes {
                spawn_probe(&shared, probe);
            }
            if let Some(load) = step.load {
                spawn_load(&shared, load);
            }
            shared.push_out(step.events);
        }
        timer_control.set_wake_at(service.next_deadline());
    }

    timer_control.close();
    if let Some(timer) = timer {
        let _ = timer.join();
    }
    shared.close_out();
}

pub(super) struct ThemeServiceStream {
    shared: Arc<ServiceShared>,
    timer_control: Arc<TimerControl>,
    service: Option<JoinHandle<()>>,
}

impl ThemeServiceStream {
    pub(super) fn spawn(config: ServiceConfig) -> Self {
        Self::spawn_with(config, |control, interval, shared| {
            std::thread::Builder::new()
                .name("enzui-theme-timer".into())
                .spawn(move || run_timer(control, interval, shared))
        })
    }

    pub(super) fn spawn_with(
        config: ServiceConfig,
        spawn_timer: impl FnOnce(
            Arc<TimerControl>,
            Duration,
            Arc<ServiceShared>,
        ) -> std::io::Result<JoinHandle<()>>,
    ) -> Self {
        let probe_capacity = config.paths.watched().len().max(1);
        let shared = Arc::new(ServiceShared::new(probe_capacity));
        let timer_control = Arc::new(TimerControl::default());

        #[cfg(test)]
        LAST_TIMER_CONTROL.with(|last| {
            *last.borrow_mut() = Some(Arc::downgrade(&timer_control));
        });

        let poll_interval = config.poll_interval;
        let mut dir_watch = config.directory_watch.backend().map(|backend| {
            AncestorWatch::new(
                backend,
                config.paths.watched(),
                Arc::new(TickSink(Arc::downgrade(&shared))),
            )
        });
        if let Some(watch) = dir_watch.as_mut() {
            timer_control.set_live_watch(watch.refresh());
        }
        let service_state = WatchService::new(
            config.paths,
            config.backend,
            config.probe,
            config.initial_os_mode,
            config.poll_interval,
            config.debounce,
        );

        let timer = spawn_timer(
            Arc::clone(&timer_control),
            poll_interval,
            Arc::clone(&shared),
        )
        .ok();
        if timer.is_none() {
            timer_control.close();
            shared.push_out(vec![ThemeEvent::WatchDiagnostic(Some(
                ThemeWatchDiagnostic::WatcherUnavailable {
                    detail: "theme watch timer thread could not be spawned".into(),
                },
            ))]);
        }

        let service = std::thread::Builder::new()
            .name("enzui-theme-service".into())
            .spawn({
                let shared = Arc::clone(&shared);
                let timer_control = Arc::clone(&timer_control);
                move || run_service(service_state, shared, timer_control, timer, dir_watch)
            })
            .ok();
        if service.is_none() {
            timer_control.close();
            shared.push_out(vec![ThemeEvent::WatchDiagnostic(Some(
                ThemeWatchDiagnostic::WatcherUnavailable {
                    detail: "theme maintenance service thread could not be spawned".into(),
                },
            ))]);
            shared.close_out();
        }

        Self {
            shared,
            timer_control,
            service,
        }
    }
}

impl Stream for ThemeServiceStream {
    type Item = ThemeEvent;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut out = self.shared.lock_out();
        if let Some(load) = out.theme.take() {
            return Poll::Ready(Some(ThemeEvent::Loaded(load)));
        }
        if let Some(diagnostic) = out.diagnostic.take() {
            return Poll::Ready(Some(ThemeEvent::WatchDiagnostic(diagnostic)));
        }
        if out.closed {
            return Poll::Ready(None);
        }
        if out
            .waker
            .as_ref()
            .is_none_or(|waker| !waker.will_wake(context.waker()))
        {
            out.waker = Some(context.waker().clone());
        }
        Poll::Pending
    }
}

impl Drop for ThemeServiceStream {
    fn drop(&mut self) {
        self.shared.close_inbox();
        self.timer_control.close();
        if let Some(service) = self.service.take() {
            let _ = service.join();
        }
        self.shared.close_out();
    }
}

#[derive(Clone)]
pub(super) struct ServiceConfig {
    pub(super) paths: ThemePaths,
    pub(super) backend: Arc<dyn FileWatchBackend>,
    pub(super) probe: ProbeOverride,
    pub(super) initial_os_mode: EnzThemeMode,
    pub(super) poll_interval: Duration,
    pub(super) debounce: Duration,
    pub(super) directory_watch: DirectoryWatch,
}

// Stable identity preserves watcher baselines across subscription reconciliation.
pub(super) struct ThemeServiceRecipe {
    pub(super) config: ServiceConfig,
}

impl Recipe for ThemeServiceRecipe {
    type Output = ThemeEvent;

    fn hash(&self, state: &mut subscription::Hasher) {
        std::any::TypeId::of::<Self>().hash(state);
        self.config.poll_interval.hash(state);
        self.config.debounce.hash(state);
        match self.config.probe {
            ProbeOverride::None => 0u8,
            ProbeOverride::Light => 1u8,
            ProbeOverride::Dark => 2u8,
        }
        .hash(state);
        self.config.paths.watched().hash(state);
    }

    fn stream(self: Box<Self>, _input: EventStream) -> BoxStream<Self::Output> {
        iced_futures::boxed_stream(ThemeServiceStream::spawn(self.config))
    }
}

#[cfg(test)]
thread_local! {
    pub(super) static LAST_TIMER_CONTROL: std::cell::RefCell<Option<std::sync::Weak<TimerControl>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
impl ThemeServiceStream {}
