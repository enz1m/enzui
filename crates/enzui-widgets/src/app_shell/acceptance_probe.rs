//! Opt-in native acceptance events; completion of this schedule is not presentation acceptance.
use iced::advanced::graphics::presentation::watch;
use iced::{Subscription, Task};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

/// A fixed workload arm; the application must separately attest its actual content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arm {
    /// Selects the application's populated workload.
    Busy,
    /// Selects the application's inexpensive comparison workload.
    Cheap,
}
/// Schedule floors are independent of observed frame counts and cadence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Plan {
    /// Independent repeated cycle groups; minimum five.
    pub repetitions: u16,
    /// Close/open pairs per repetition; minimum ten.
    pub cycles: u16,
    /// Requested blocking time on the UI reducer thread; minimum 100 milliseconds.
    pub stall_ms: u64,
    /// Settled delay after each acknowledged stall, in milliseconds.
    pub hold_ms: u64,
}
impl Default for Plan {
    fn default() -> Self {
        Self {
            repetitions: 5,
            cycles: 10,
            stall_ms: 100,
            hold_ms: 800,
        }
    }
}
impl Plan {
    /// Requires 5..=20 repetitions, 10..=100 cycles, and 100..=1000 ms stalls.
    /// Holds must last at least 100 ms beyond the stall and at most 5000 ms.
    pub fn valid(self) -> bool {
        self.repetitions >= 5
            && self.repetitions <= 20
            && self.cycles >= 10
            && self.cycles <= 100
            && self.stall_ms >= 100
            && self.stall_ms <= 1000
            && self.hold_ms >= self.stall_ms + 100
            && self.hold_ms <= 5000
    }
    /// Total toggle edges; every edge requires its own applied receipt and stall.
    pub fn edges(self) -> u64 {
        u64::from(self.repetitions) * u64::from(self.cycles) * 2
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Config {
    app: &'static str,
    arm: Arm,
    output: PathBuf,
    plan: Plan,
    observed_rows: usize,
    clock_id: i32,
}
impl Config {
    fn requested(app: &'static str) -> Option<Self> {
        if std::env::var("ENZUI_ANIMATION_ACCEPTANCE_APP")
            .ok()
            .as_deref()
            != Some(app)
        {
            return None;
        }
        let arm = match std::env::var("ENZUI_ANIMATION_ACCEPTANCE_ARM")
            .ok()?
            .as_str()
        {
            "busy" => Arm::Busy,
            "cheap" => Arm::Cheap,
            _ => return None,
        };
        let output = PathBuf::from(std::env::var_os("ENZUI_ANIMATION_ACCEPTANCE_OUT")?);
        let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?)
            .canonicalize()
            .ok()?;
        let parent = output.parent()?.canonicalize().ok()?;
        if !parent.starts_with(&runtime) {
            return None;
        }
        let plan = Plan::default();
        let clock_id = std::env::var("ENZUI_ANIMATION_ACCEPTANCE_CLOCK_ID")
            .map_or(Some(1), |value| value.parse::<i32>().ok())?;
        enzui_core::monotonic_ms::presentation_clock_ns_for(clock_id).ok()?;
        Some(Self {
            app,
            arm,
            output,
            plan,
            observed_rows: 0,
            clock_id,
        })
    }
}

#[derive(Debug, Clone, Copy)]
enum Signal {
    Ack(u64),
    Applied(u64, u64),
    Cancel,
}
#[derive(Debug)]
struct Run {
    log: Mutex<std::fs::File>,
    started: Instant,
    control: mpsc::SyncSender<Signal>,
    plan: Plan,
    clock_id: i32,
    watch: Option<Arc<watch::Watch>>,
}
impl Run {
    fn emit(&self, kind: &str, edge: u64, elapsed_ns: u128) -> Option<u64> {
        let mut file = self.log.lock().ok()?;
        match enzui_core::monotonic_ms::presentation_clock_ns_for(self.clock_id) {
            Ok((clock_id, at_ns)) => {
                writeln!(file,
                    "{{\"record\":\"acceptance_{kind}\",\"edge\":{edge},\"clock_id\":{clock_id},\"at_ns\":{at_ns},\"since_run_ns\":{},\"elapsed_ns\":{elapsed_ns}}}",
                    self.started.elapsed().as_nanos()
                ).ok()?;
                file.flush().ok()?;
                u64::try_from(at_ns).ok()
            }
            Err(_) => {
                let _ = writeln!(
                    file,
                    "{{\"record\":\"acceptance_clock_error\",\"edge\":{edge}}}"
                );
                let _ = file.flush();
                None
            }
        }
    }
}
/// One UI-dispatched event. Cloning an event never acknowledges it.
#[derive(Debug, Clone)]
pub struct Event {
    run: Arc<Run>,
    edge: u64,
    kind: Kind,
}
#[derive(Debug, Clone, Copy)]
enum Kind {
    Toggle,
    Stall,
    Finish,
}
/// Kept alive until the application's ordinary toggle reducer has returned.
#[derive(Debug)]
pub struct Receipt {
    run: Arc<Run>,
    edge: u64,
}
impl Drop for Receipt {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let _ = self.run.emit("reducer_panicked", self.edge, 0);
            return;
        }
        let Some(applied_at) = self.run.emit("toggle_applied", self.edge, 0) else {
            return;
        };
        let _ = self
            .run
            .control
            .try_send(Signal::Applied(self.edge, applied_at));
    }
}
/// Dispatch outcomes do not imply that a compositor presented any frame.
#[derive(Debug)]
pub enum Dispatch<Message> {
    /// Run the ordinary toggle reducer while keeping this receipt alive.
    Toggle(Receipt),
    /// Return this task after the stall: it preserves windows because no model changed.
    /// Batch it with any independently queued application work instead of dropping that work.
    Continue(Task<Message>),
    /// Close the owned test application; native acceptance is decided separately.
    Finish,
}

/// Runs stalls on the calling application update thread, never on the scheduler worker.
/// Toggle receipts must span the real reducer, not merely message construction.
pub fn dispatch<Message>(event: Event) -> Dispatch<Message> {
    match event.kind {
        Kind::Toggle => Dispatch::Toggle(Receipt {
            run: event.run,
            edge: event.edge,
        }),
        Kind::Stall => {
            let _ = event.run.emit("stall_begin", event.edge, 0);
            let start = Instant::now();
            std::thread::sleep(Duration::from_millis(event.run.plan.stall_ms));
            let _ = event
                .run
                .emit("stall_end", event.edge, start.elapsed().as_nanos());
            let _ = event.run.control.try_send(Signal::Ack(event.edge));
            Dispatch::Continue(iced::window::invalidate(iced::window::Invalidation::none()))
        }
        Kind::Finish => {
            let _ = event
                .run
                .emit("schedule_finished_not_accepted", event.edge, 0);
            Dispatch::Finish
        }
    }
}

struct Cancel(mpsc::SyncSender<Signal>);
impl Drop for Cancel {
    fn drop(&mut self) {
        let _ = self.0.try_send(Signal::Cancel);
    }
}
fn wait(receiver: &mpsc::Receiver<Signal>, duration: Duration) -> bool {
    matches!(
        receiver.recv_timeout(duration),
        Err(mpsc::RecvTimeoutError::Timeout)
    )
}
fn acknowledged(receiver: &mpsc::Receiver<Signal>, edge: u64) -> bool {
    matches!(receiver.recv_timeout(Duration::from_secs(5)),Ok(Signal::Ack(value)) if value == edge)
}
fn await_presentation(
    receiver: &mpsc::Receiver<Signal>,
    timeout: Duration,
    mut poll: impl FnMut() -> Option<watch::Receipt>,
) -> Option<watch::Receipt> {
    let deadline = Instant::now().checked_add(timeout)?;
    loop {
        if !matches!(receiver.try_recv(), Err(mpsc::TryRecvError::Empty)) {
            return None;
        }
        if let Some(receipt) = poll() {
            return Some(receipt);
        }
        let remaining = deadline.checked_duration_since(Instant::now())?;
        if !wait(receiver, remaining.min(Duration::from_millis(1))) {
            return None;
        }
    }
}

fn drive(run: Arc<Run>, receiver: mpsc::Receiver<Signal>, mut send: impl FnMut(Event) -> bool) {
    if !wait(&receiver, Duration::from_millis(1500)) {
        return;
    }
    let mut window = None;
    for edge in 1..=run.plan.edges() {
        if !send(Event {
            run: Arc::clone(&run),
            edge,
            kind: Kind::Toggle,
        }) {
            let _ = run.emit("toggle_unacknowledged", edge, 0);
            return;
        }
        let applied_at = match receiver.recv_timeout(Duration::from_secs(5)) {
            Ok(Signal::Applied(applied, at)) if applied == edge => at,
            _ => {
                let _ = run.emit("toggle_unacknowledged", edge, 0);
                return;
            }
        };
        let confirmed = await_presentation(&receiver, Duration::from_secs(5), || {
            run.watch
                .as_ref()?
                .after(u32::try_from(run.clock_id).ok()?, applied_at, window)
        });
        let Some(confirmed) = confirmed else {
            let _ = run.emit("presentation_unconfirmed", edge, 0);
            return;
        };
        window = Some(confirmed.key.window_id);
        if !send(Event {
            run: Arc::clone(&run),
            edge,
            kind: Kind::Stall,
        }) || !acknowledged(&receiver, edge)
        {
            let _ = run.emit("stall_unacknowledged", edge, 0);
            return;
        }
        if !wait(&receiver, Duration::from_millis(run.plan.hold_ms)) {
            return;
        }
    }
    if !send(Event {
        run: Arc::clone(&run),
        edge: run.plan.edges(),
        kind: Kind::Finish,
    }) {
        let _ = run.emit("finish_undelivered", run.plan.edges(), 0);
    }
}

/// Reads the selected workload arm for the named application.
///
/// Reads `ENZUI_ANIMATION_ACCEPTANCE_APP` and `ENZUI_ANIMATION_ACCEPTANCE_ARM`
/// (`busy` or `cheap`). This does not validate the output path or enable the
/// schedule; [`subscription`] performs those checks.
pub fn selected_arm(app: &str) -> Option<Arm> {
    if std::env::var("ENZUI_ANIMATION_ACCEPTANCE_APP")
        .ok()
        .as_deref()
        != Some(app)
    {
        return None;
    }
    match std::env::var("ENZUI_ANIMATION_ACCEPTANCE_ARM")
        .ok()
        .as_deref()
    {
        Some("busy") => Some(Arm::Busy),
        Some("cheap") => Some(Arm::Cheap),
        _ => None,
    }
}

/// Subscribes to an opt-in acceptance schedule once the application is ready.
///
/// `ENZUI_ANIMATION_ACCEPTANCE_APP` must equal `app`, and
/// `ENZUI_ANIMATION_ACCEPTANCE_ARM` must be `busy` or `cheap`.
/// `ENZUI_ANIMATION_ACCEPTANCE_OUT` names a new file whose parent resolves under
/// `XDG_RUNTIME_DIR`. Invalid configuration disables the subscription; an
/// output file that cannot be created produces no events.
///
/// The schedule requires a release build. A debug build records its refusal
/// and emits a finish event. `observed_rows` is recorded as supplied; the
/// application must attest workload contents separately. Completion does not
/// establish presentation acceptance, cadence, or source/build binding.
/// The launcher owns D-Bus isolation.
pub fn subscription(app: &'static str, ready: bool, observed_rows: usize) -> Subscription<Event> {
    if !ready {
        return Subscription::none();
    }
    let Some(mut config) = Config::requested(app) else {
        return Subscription::none();
    };
    config.observed_rows = observed_rows;
    Subscription::run_with(config, |config| {
        let config = config.clone();
        iced::stream::channel(4, async move |mut output| {
            let Ok(file) = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&config.output)
            else {
                std::future::pending::<()>().await;
                return;
            };
            let (control, receiver) = mpsc::sync_channel(4);
            let run = Arc::new(Run {
                log: Mutex::new(file),
                started: Instant::now(),
                control: control.clone(),
                plan: config.plan,
                clock_id: config.clock_id,
                watch: watch::subscribe(),
            });
            if let Ok(mut file) = run.log.lock() {
                let _ = writeln!(
                    file,
                    "{{\"record\":\"acceptance_schedule\",\"app\":\"{}\",\"arm\":\"{}\",\"pid\":{},\"release\":{},\"repetitions\":{},\"cycles\":{},\"stall_ms\":{},\"observed_rows\":{},\"minimum_moving_intervals\":1000,\"native_evidence_required\":true}}",
                    config.app,
                    if config.arm == Arm::Busy {
                        "busy"
                    } else {
                        "cheap"
                    },
                    std::process::id(),
                    !cfg!(debug_assertions),
                    config.plan.repetitions,
                    config.plan.cycles,
                    config.plan.stall_ms,
                    config.observed_rows
                );
            }
            if cfg!(debug_assertions) || !config.plan.valid() {
                let _ = run.emit("refused_profile_or_plan", 0, 0);
                let _ = output.try_send(Event {
                    run,
                    edge: 0,
                    kind: Kind::Finish,
                });
                std::future::pending::<()>().await;
                return;
            }
            let cancel = Cancel(control);
            let error_run = Arc::clone(&run);
            if std::thread::Builder::new()
                .name("enzui-acceptance-events".into())
                .spawn(move || drive(run, receiver, |event| output.try_send(event).is_ok()))
                .is_err()
            {
                let _ = error_run.emit("scheduler_unavailable", 0, 0);
            }
            std::future::pending::<()>().await;
            drop(cancel);
        })
    })
}
