//! Synchronous clipboard requests dispatched on the application's Wayland connection.
//!
//! Protocol objects and byte transfers run on workers, but request methods wait
//! on the calling thread. Use [`crate::bridge`] to keep those waits off the UI.
//! Commands have unbounded queue admission. Result timeouts do not cancel queued
//! work; an offer can take effect after its caller has timed out.
//!
//! Native drag commands share the dispatch worker with bounded admission.
//! Wake writes are nonblocking. Drop sends Stop before allowing a 500 ms join.

use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use wayland_client::Connection;
use wayland_client::globals::registry_queue_init;

use crate::error::ClipboardError;
use crate::lease::DisplayLease;
use crate::mime::{Kind, MimeType};
use crate::payload::{Pasted, Payload};
use crate::protocol::{SeatSelector, SeatStatus, State};
use crate::transfer::{TransferLimits, set_non_blocking, transfer_pipe};

const DROP_JOIN_BUDGET: Duration = Duration::from_millis(500);

enum Command {
    Offer {
        payload: Payload,
        selector: SeatSelector,
        reply: Sender<Result<(), ClipboardError>>,
    },
    OfferPrimary {
        payload: Payload,
        selector: SeatSelector,
        reply: Sender<Result<(), ClipboardError>>,
    },
    PastePrimary {
        preferences: Vec<MimeType>,
        selector: SeatSelector,
        reply: Sender<Result<Pasted, ClipboardError>>,
    },
    Clear {
        selector: SeatSelector,
        reply: Sender<Result<(), ClipboardError>>,
    },
    Paste {
        preferences: Vec<MimeType>,
        selector: SeatSelector,
        reply: Sender<Result<Pasted, ClipboardError>>,
    },
    Seats {
        reply: Sender<Result<Vec<SeatStatus>, ClipboardError>>,
    },
    Drag(crate::drag::DragCommand, NativePermit),
    Stop,
}

struct NativePermit(Arc<AtomicUsize>);

impl Drop for NativePermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The application's clipboard on its own Wayland connection.
///
/// Request methods block the caller; see the module's queue and timeout limits.
/// A worker uses its own event queue on the supplied application connection.
///
/// ```no_run
/// use enzui_clipboard::{Kind, Payload, TransferLimits, WaylandClipboard};
/// use enzui_clipboard::bootstrap::AdoptionSeal;
///
/// # fn example(connection: &wayland_client::Connection, seal: &AdoptionSeal) -> Result<(), Box<dyn std::error::Error>> {
/// let clipboard = WaylandClipboard::adopt(connection, TransferLimits::default(), seal)?;
/// clipboard.offer(Payload::text("hello"))?;
/// let pasted = clipboard.paste(Kind::Text)?;
/// assert_eq!(pasted.text(), Some("hello"));
/// # Ok(())
/// # }
/// ```
pub struct WaylandClipboard {
    commands: Sender<Command>,
    wake: Arc<OwnedFd>,
    native_pending: Arc<AtomicUsize>,
    limits: TransferLimits,
    worker: Option<JoinHandle<()>>,
}

impl WaylandClipboard {
    /// Starts a clipboard on an existing application connection.
    ///
    /// Clones the connection and opens a separate event queue in the same client.
    /// Startup waits up to `startup_timeout`; a stalled worker may outlive failure.
    ///
    /// # Errors
    ///
    /// Returns descriptor, thread-spawn, global-discovery or connection failures,
    /// or `StartupStalled` when readiness is not received within the startup wait.
    pub fn adopt(
        connection: &Connection,
        limits: TransferLimits,
        _seal: &crate::bootstrap::AdoptionSeal,
    ) -> Result<Self, ClipboardError> {
        Self::start(connection.clone(), limits, None, Reading::Worker)
    }

    /// Starts a clipboard on a connection whose socket another thread reads.
    ///
    /// `connection` shares its backend with a thread that reads the socket
    /// and dispatches the backend's queue, as the toolkit's guest connection
    /// does. The worker reads nothing itself: it sleeps until its own queue
    /// receives events, a request arrives or a deadline passes, so traffic for
    /// the rest of the application never wakes it. A share of `lease` is held
    /// as [`Self::adopt_foreign_display`] holds it.
    ///
    /// Without that reader the clipboard sees no events after startup.
    ///
    /// # Errors
    ///
    /// Returns the same setup and startup errors as [`Self::adopt`].
    pub fn adopt_read_elsewhere(
        connection: Connection,
        limits: TransferLimits,
        _seal: &crate::bootstrap::AdoptionSeal,
        lease: DisplayLease,
    ) -> Result<Self, ClipboardError> {
        Self::start(connection, limits, Some(lease), Reading::Elsewhere)
    }

    /// Starts a clipboard on the `wl_display` of a running toolkit window.
    ///
    /// `display` is the pointer a windowing library reports as its Wayland
    /// display handle, for example `RawDisplayHandle::Wayland`; the runtime
    /// borrows it and never closes it. A share of `lease` is held by the
    /// thread that dispatches on the display, which outlives the returned
    /// clipboard if the join budget expires and detaches that thread.
    /// Startup failure can also leave a worker retaining the lease.
    ///
    /// # Errors
    ///
    /// Returns the same setup and startup errors as [`Self::adopt`].
    ///
    /// # Safety
    ///
    /// `display` must be a live `wl_display` from libwayland, and `lease` must
    /// keep it valid for as long as any share of that lease exists. A lease on
    /// anything other than the display's owner reintroduces the lifetime this
    /// parameter exists to close.
    #[expect(
        unsafe_code,
        reason = "adopting a toolkit's existing wl_display is FFI by \
                  construction; the alternative is a second connection, which \
                  is exactly the focus/serial mismatch this transport exists \
                  to fix"
    )]
    pub unsafe fn adopt_foreign_display(
        display: std::ptr::NonNull<std::ffi::c_void>,
        limits: TransferLimits,
        _seal: &crate::bootstrap::AdoptionSeal,
        lease: DisplayLease,
    ) -> Result<Self, ClipboardError> {
        // SAFETY: delegated to this function's own contract; the backend does
        // not take ownership of the display.
        let backend = unsafe {
            wayland_client::backend::Backend::from_foreign_display(display.as_ptr().cast())
        };

        Self::start(
            Connection::from_backend(backend),
            limits,
            Some(lease),
            Reading::Worker,
        )
    }

    /// The startup and transfer limits this clipboard runs under.
    pub fn limits(&self) -> TransferLimits {
        self.limits
    }

    /// Returns native drag access on this same display and dispatch worker.
    ///
    /// Requests share 64 admission slots. The returned handle does not keep the
    /// display alive after this clipboard shuts down; later requests fail.
    pub fn native_drag(&self) -> crate::drag::NativeDragClient {
        let pending = Arc::clone(&self.native_pending);
        let commands = self.commands.clone();
        let wake = Arc::clone(&self.wake);
        crate::drag::NativeDragClient {
            submit: Arc::new(move |command| {
                pending
                    .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                        (count < 64).then_some(count + 1)
                    })
                    .map_err(|_| ClipboardError::Busy)?;
                let permit = NativePermit(Arc::clone(&pending));
                commands
                    .send(Command::Drag(command, permit))
                    .map_err(|_| ClipboardError::Stopped)?;
                let _ = rustix::io::write(wake.as_fd(), &[1]);
                Ok(())
            }),
            timeout: self
                .limits
                .receive_timeout
                .saturating_add(Duration::from_secs(1)),
        }
    }

    /// Claims the selection on every focused seat, promising `payload`.
    ///
    /// Waits for selection confirmation on all selected seats. Bytes remain in
    /// this client and require its worker to stay alive to serve later pastes.
    ///
    /// # Errors
    ///
    /// Returns the same focus, protocol, confirmation and wait errors as [`Self::offer_on`].
    pub fn offer(&self, payload: Payload) -> Result<(), ClipboardError> {
        self.offer_on(payload, SeatSelector::Focused)
    }

    /// Request selection ownership on the seats resolved by `selector`.
    ///
    /// `Focused` and `Any` target all focused seats with an input serial; `Named`
    /// targets one seat and still requires focus. Waits for `send_timeout`, which
    /// is separate from the compositor-confirmation budget `claim_timeout`.
    /// A caller timeout does not cancel the claim.
    ///
    /// # Errors
    ///
    /// Returns seat/focus errors, `MimeUnavailable` for an empty payload,
    /// `ClaimNotHonoured` without matching selection confirmation, connection
    /// failures, `Stopped`, or `TransferStalled` when the result wait expires.
    pub fn offer_on(&self, payload: Payload, selector: SeatSelector) -> Result<(), ClipboardError> {
        let cleanup = payload.clone();
        if let Some(source) = payload.deferred_source() {
            let wake = Arc::clone(&self.wake);
            source.install_wake(Arc::new(move || {
                let _ = rustix::io::write(wake.as_fd(), &[1]);
            }))?;
        }
        let (reply, answer) = channel();
        let result = self
            .send(Command::Offer {
                payload,
                selector,
                reply,
            })
            .and_then(|()| self.wait(answer, self.limits.send_timeout));
        if result.is_err() {
            cleanup.revoke_deferred();
        }
        result
    }

    /// Request selection removal on focused seats with an input serial.
    ///
    /// Waits up to `send_timeout` for the command result; success means the
    /// protocol request was flushed, without waiting for compositor confirmation.
    ///
    /// # Errors
    ///
    /// Returns seat/focus or connection errors, `Stopped`, or `TransferStalled`
    /// when the result wait expires.
    pub fn clear(&self) -> Result<(), ClipboardError> {
        let (reply, answer) = channel();

        self.send(Command::Clear {
            selector: SeatSelector::Focused,
            reply,
        })?;

        self.wait(answer, self.limits.send_timeout)
    }

    /// Read the selection using `kind` preferences from a focused seat.
    ///
    /// # Errors
    ///
    /// Returns the negotiation, transfer and worker errors from [`Self::paste_preferring`].
    ///
    /// # Panics
    ///
    /// Panics if `receive_timeout` plus one second overflows [`Duration`].
    pub fn paste(&self, kind: Kind) -> Result<Pasted, ClipboardError> {
        self.paste_preferring(kind.preferences(), SeatSelector::Focused)
    }

    /// Reads the selection using an explicit preference order.
    ///
    /// The first preference the current selection actually offers wins; if none
    /// does, the error includes the advertised types. Waits for `receive_timeout`
    /// plus one second after command submission, without cancelling a timed-out read.
    ///
    /// # Errors
    ///
    /// Returns seat/focus errors, `NoSelection`, `MimeUnavailable`, transfer or
    /// connection failures, `Stopped`, or `TransferStalled` on result timeout.
    ///
    /// # Panics
    ///
    /// Panics if adding one second to `receive_timeout` overflows [`Duration`].
    pub fn paste_preferring(
        &self,
        preferences: Vec<MimeType>,
        selector: SeatSelector,
    ) -> Result<Pasted, ClipboardError> {
        let (reply, answer) = channel();

        self.send(Command::Paste {
            preferences,
            selector,
            reply,
        })?;

        self.wait(answer, self.limits.receive_timeout + Duration::from_secs(1))
    }

    /// Offers `payload` as the primary selection (the text a middle click
    /// pastes) on focused seats.
    ///
    /// Waits up to `send_timeout` for the request to be flushed; the primary
    /// selection has no claim confirmation.
    ///
    /// # Errors
    ///
    /// Returns `NoPrimarySelection` when the compositor lacks the protocol,
    /// seat/focus errors, `MimeUnavailable` for an empty payload, connection
    /// failures, `Stopped`, or `TransferStalled` when the wait expires.
    pub fn offer_primary(&self, payload: Payload) -> Result<(), ClipboardError> {
        let (reply, answer) = channel();
        self.send(Command::OfferPrimary {
            payload,
            selector: SeatSelector::Focused,
            reply,
        })?;
        self.wait(answer, self.limits.send_timeout)
    }

    /// Reads the primary selection using `kind` preferences from a focused seat.
    ///
    /// # Errors
    ///
    /// Returns `NoPrimarySelection`, `NoSelection`, `MimeUnavailable`, seat/focus,
    /// transfer or connection failures, `Stopped`, or `TransferStalled`.
    ///
    /// # Panics
    ///
    /// Panics if `receive_timeout` plus one second overflows [`Duration`].
    pub fn paste_primary(&self, kind: Kind) -> Result<Pasted, ClipboardError> {
        self.paste_primary_preferring(kind.preferences(), SeatSelector::Focused)
    }

    /// Reads the primary selection using an explicit preference order, like
    /// [`Self::paste_preferring`].
    ///
    /// # Errors
    ///
    /// As [`Self::paste_primary`].
    ///
    /// # Panics
    ///
    /// Panics if `receive_timeout` plus one second overflows [`Duration`].
    pub fn paste_primary_preferring(
        &self,
        preferences: Vec<MimeType>,
        selector: SeatSelector,
    ) -> Result<Pasted, ClipboardError> {
        let (reply, answer) = channel();
        self.send(Command::PastePrimary {
            preferences,
            selector,
            reply,
        })?;
        self.wait(answer, self.limits.receive_timeout + Duration::from_secs(1))
    }

    /// The current state of every seat: focus, ownership and offered types.
    ///
    /// Reports locally tracked state, which can lag compositor changes.
    /// Advertised types are retained across focus loss until a selection event
    /// replaces them; ownership includes a source still awaiting confirmation.
    /// Waits up to two seconds after submission.
    ///
    /// # Errors
    ///
    /// Returns a stored connection failure, `Stopped`, or `TransferStalled` on timeout.
    pub fn seats(&self) -> Result<Vec<SeatStatus>, ClipboardError> {
        let (reply, answer) = channel();

        self.send(Command::Seats { reply })?;
        self.wait(answer, Duration::from_secs(2))
    }

    /// Whether any seat has a locally tracked selection source, including pending claims.
    ///
    /// Does not require current focus or independently verify compositor ownership.
    ///
    /// # Errors
    ///
    /// Returns the same worker and wait errors as [`Self::seats`].
    pub fn owns_selection(&self) -> Result<bool, ClipboardError> {
        Ok(self.seats()?.iter().any(|seat| seat.owns_selection))
    }

    fn start(
        connection: Connection,
        limits: TransferLimits,
        lease: Option<DisplayLease>,
        reading: Reading,
    ) -> Result<Self, ClipboardError> {
        let leased = LeasedConnection { connection, lease };

        Self::start_with_worker(
            leased,
            limits,
            move |leased, commands, wake, waker, ready| {
                let reading = match reading {
                    Reading::Worker => None,
                    Reading::Elsewhere => Some(std::task::Waker::from(Arc::new(PipeWaker(waker)))),
                };
                worker(leased, limits, commands, wake, reading, ready);
            },
        )
    }

    fn start_with_worker<R, F>(
        resources: R,
        limits: TransferLimits,
        run: F,
    ) -> Result<Self, ClipboardError>
    where
        R: Send + 'static,
        F: FnOnce(R, Receiver<Command>, OwnedFd, Arc<OwnedFd>, Sender<Result<(), ClipboardError>>)
            + Send
            + 'static,
    {
        let (wake_read, wake_write) = transfer_pipe()?;
        set_non_blocking(wake_write.as_fd())?;
        let wake_write = Arc::new(wake_write);
        let worker_wake = Arc::clone(&wake_write);
        let (commands, command_rx) = channel();
        let (ready, started) = channel();

        let worker = std::thread::Builder::new()
            .name(String::from("enzui-clipboard"))
            .spawn(move || run(resources, command_rx, wake_read, worker_wake, ready))
            .map_err(|error| ClipboardError::Unavailable {
                reason: error.to_string(),
            })?;

        match started.recv_timeout(limits.startup_timeout) {
            Ok(Ok(())) => Ok(Self {
                commands,
                wake: wake_write,
                native_pending: Arc::new(AtomicUsize::new(0)),
                limits,
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = join_with_budget(worker, DROP_JOIN_BUDGET);
                Err(error)
            }
            Err(RecvTimeoutError::Timeout) => {
                drop(worker);
                Err(ClipboardError::StartupStalled {
                    after: limits.startup_timeout,
                })
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = join_with_budget(worker, DROP_JOIN_BUDGET);
                Err(ClipboardError::Unavailable {
                    reason: String::from("the clipboard worker stopped before it started"),
                })
            }
        }
    }

    fn send(&self, command: Command) -> Result<(), ClipboardError> {
        self.commands
            .send(command)
            .map_err(|_| ClipboardError::Stopped)?;

        // Wake an idle worker parked in poll even while a transfer is in flight.
        let _ = rustix::io::write(self.wake.as_fd(), &[1u8]);

        Ok(())
    }

    fn wait<T>(
        &self,
        answer: Receiver<Result<T, ClipboardError>>,
        timeout: Duration,
    ) -> Result<T, ClipboardError> {
        match answer.recv_timeout(timeout) {
            Ok(outcome) => outcome,
            Err(RecvTimeoutError::Timeout) => {
                Err(ClipboardError::TransferStalled { after: timeout })
            }
            Err(RecvTimeoutError::Disconnected) => Err(ClipboardError::Stopped),
        }
    }

    fn stop_and_join_worker(&mut self) -> bool {
        let _ = self.send(Command::Stop);

        self.worker
            .take()
            .is_none_or(|worker| join_with_budget(worker, DROP_JOIN_BUDGET))
    }
}

impl Drop for WaylandClipboard {
    fn drop(&mut self) {
        let _ = self.stop_and_join_worker();
    }
}

fn join_with_budget(worker: JoinHandle<()>, budget: Duration) -> bool {
    let Some(deadline) = Instant::now().checked_add(budget) else {
        drop(worker);
        return false;
    };

    while !worker.is_finished() {
        let now = Instant::now();
        if now >= deadline {
            drop(worker);
            return false;
        }

        std::thread::sleep(Duration::from_millis(1).min(deadline.duration_since(now)));
    }

    let _ = worker.join();
    true
}

impl std::fmt::Debug for WaylandClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaylandClipboard")
            .field("limits", &self.limits)
            .field("running", &self.worker.is_some())
            .finish()
    }
}

struct LeasedConnection<C = Connection, L = Option<DisplayLease>> {
    connection: C,
    // Declared after `connection`: destroying a borrowed display's event queue
    // and proxies needs that display still open.
    lease: L,
}

/// Who reads the clipboard connection's socket.
#[derive(Clone, Copy)]
enum Reading {
    /// The worker polls and reads the socket itself.
    Worker,
    /// Another thread reads it; the worker waits for its own queue's events.
    Elsewhere,
}

/// Wakes the worker through its wake pipe when its queue receives events.
struct PipeWaker(Arc<OwnedFd>);

impl std::task::Wake for PipeWaker {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        let _ = rustix::io::write(self.0.as_fd(), &[1u8]);
    }
}

fn worker(
    leased: LeasedConnection,
    limits: TransferLimits,
    commands: Receiver<Command>,
    wake: OwnedFd,
    reading: Option<std::task::Waker>,
    ready: Sender<Result<(), ClipboardError>>,
) {
    run_holding_lease(leased, move |connection| {
        dispatch(connection, limits, commands, wake, reading, ready);
    });
}

fn run_holding_lease<C, L, T>(resources: LeasedConnection<C, L>, body: impl FnOnce(&C) -> T) -> T {
    // Function arguments outlive the body call; field order then releases the
    // connection before its display lease, including during unwinding.
    let _keep_alive = &resources.lease;
    body(&resources.connection)
}

fn dispatch(
    connection: &Connection,
    limits: TransferLimits,
    commands: Receiver<Command>,
    wake: OwnedFd,
    // Set when another thread reads the socket: the waker its queue wakes.
    reading: Option<std::task::Waker>,
    ready: Sender<Result<(), ClipboardError>>,
) {
    // The wake pipe is drained without blocking; a blocking read here would
    // park the worker on its own control channel.
    if let Err(error) = set_non_blocking(wake.as_fd()) {
        let _ = ready.send(Err(error));
        return;
    }

    let (globals, mut queue) = match registry_queue_init::<State>(connection) {
        Ok(parts) => parts,
        Err(error) => {
            let _ = ready.send(Err(ClipboardError::Unavailable {
                reason: error.to_string(),
            }));
            return;
        }
    };

    let qh = queue.handle();

    let mut state = match State::new(Connection::clone(connection), &globals, &qh, limits) {
        Ok(state) => state,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };

    // Two round trips: the first delivers seat capabilities and names, the
    // second delivers what those replies asked for — keyboard focus and any
    // selection that already exists.
    for _ in 0..2 {
        if let Err(error) = queue.roundtrip(&mut state) {
            let _ = ready.send(Err(ClipboardError::Unavailable {
                reason: error.to_string(),
            }));
            return;
        }
    }

    if ready.send(Ok(())).is_err() {
        return;
    }

    loop {
        state.expire_claims();
        state.expire_timed_sources();
        state.expire_drags();

        if queue.dispatch_pending(&mut state).is_err() {
            state.fail(ClipboardError::Disconnected);
            break;
        }

        match commands.try_recv() {
            Ok(Command::Stop) => break,
            Ok(command) => {
                handle(&mut state, &qh, command);
                continue;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
        }

        if queue.dispatch_pending(&mut state).is_err() {
            state.fail(ClipboardError::Disconnected);
            break;
        }

        if connection.flush().is_err() {
            state.fail(ClipboardError::Disconnected);
            break;
        }

        let deadline = [
            state.next_claim_deadline(),
            state.drag_deadline(),
            state.next_source_deadline(),
        ]
        .into_iter()
        .flatten()
        .min();

        if let Some(waker) = &reading {
            // Dispatches what the reader routed here and leaves the waker
            // for the next event, so only this queue's events wake the
            // worker.
            let mut context = std::task::Context::from_waker(waker);
            if let std::task::Poll::Ready(Err(_)) =
                queue.poll_dispatch_pending(&mut context, &mut state)
            {
                state.fail(ClipboardError::Disconnected);
                break;
            }
            match wait_on(None, wake.as_fd(), deadline) {
                Wake::Command => drain(wake.as_fd()),
                Wake::Wayland | Wake::Idle => {}
                Wake::Failed => {
                    state.fail(ClipboardError::Disconnected);
                    break;
                }
            }
            continue;
        }

        let Some(guard) = queue.prepare_read() else {
            continue;
        };

        match wait_on(Some(guard.connection_fd()), wake.as_fd(), deadline) {
            Wake::Wayland => {
                if guard.read().is_err() {
                    state.fail(ClipboardError::Disconnected);
                    break;
                }
            }
            Wake::Command => {
                drop(guard);
                drain(wake.as_fd());
            }
            Wake::Idle => drop(guard),
            Wake::Failed => {
                drop(guard);
                state.fail(ClipboardError::Disconnected);
                break;
            }
        }
    }

    state.shutdown();

    let reason = state.fatal().unwrap_or(ClipboardError::Stopped);

    while let Ok(command) = commands.try_recv() {
        refuse(command, reason.clone());
    }
}

fn handle(state: &mut State, qh: &wayland_client::QueueHandle<State>, command: Command) {
    match command {
        Command::Offer {
            payload,
            selector,
            reply,
        } => {
            // `offer` answers through the reply channel once the compositor
            // confirms or refuses the claim, so only an immediate failure is
            // reported here.
            if let Err(error) = state.offer(payload, &selector, qh, reply.clone()) {
                let _ = reply.send(Err(error));
            }
        }
        Command::Clear { selector, reply } => {
            let _ = reply.send(state.clear(&selector));
        }
        Command::OfferPrimary {
            payload,
            selector,
            reply,
        } => {
            let _ = reply.send(state.offer_primary(payload, &selector, qh));
        }
        Command::PastePrimary {
            preferences,
            selector,
            reply,
        } => {
            if let Err(error) = state.paste_primary(&preferences, &selector, reply.clone()) {
                let _ = reply.send(Err(error));
            }
        }
        Command::Paste {
            preferences,
            selector,
            reply,
        } => {
            // On success the transfer thread owns the reply channel; only a
            // refusal is answered here.
            if let Err(error) = state.paste(&preferences, &selector, reply.clone()) {
                let _ = reply.send(Err(error));
            }
        }
        Command::Seats { reply } => {
            let _ = reply.send(match state.fatal() {
                Some(error) => Err(error),
                None => Ok(state.statuses()),
            });
        }
        Command::Drag(command, _permit) => state.drag_command(command, qh),
        Command::Stop => {}
    }
}

fn refuse(command: Command, reason: ClipboardError) {
    match command {
        Command::Offer { reply, .. }
        | Command::OfferPrimary { reply, .. }
        | Command::Clear { reply, .. } => {
            let _ = reply.send(Err(reason));
        }
        Command::Paste { reply, .. } | Command::PastePrimary { reply, .. } => {
            let _ = reply.send(Err(reason));
        }
        Command::Seats { reply } => {
            let _ = reply.send(Err(reason));
        }
        Command::Drag(command, _permit) => command.refuse(reason),
        Command::Stop => {}
    }
}

enum Wake {
    Wayland,
    Command,
    Idle,
    Failed,
}

// Commands are enqueued before their wake byte; pending claims supply the only idle deadline.
fn wait_on(
    socket: Option<BorrowedFd<'_>>,
    wake: BorrowedFd<'_>,
    deadline: Option<Instant>,
) -> Wake {
    let timeout = deadline.map(|deadline| {
        let remaining = deadline.saturating_duration_since(Instant::now());
        Timespec {
            tv_sec: i64::try_from(remaining.as_secs()).unwrap_or(i64::MAX),
            tv_nsec: i64::from(remaining.subsec_nanos()),
        }
    });

    let mut fds = [
        PollFd::new(&wake, PollFlags::IN),
        PollFd::new(&wake, PollFlags::IN),
    ];
    if let Some(socket) = &socket {
        fds[1] = PollFd::new(socket, PollFlags::IN);
    }
    let watched = if socket.is_some() { 2 } else { 1 };

    match poll(&mut fds[..watched], timeout.as_ref()) {
        Ok(0) => Wake::Idle,
        Ok(_) => {
            let socket = if socket.is_some() {
                fds[1].revents()
            } else {
                PollFlags::empty()
            };
            if socket.intersects(PollFlags::IN) {
                Wake::Wayland
            } else if socket.intersects(PollFlags::ERR | PollFlags::HUP) {
                Wake::Failed
            } else {
                Wake::Command
            }
        }
        Err(rustix::io::Errno::INTR) => Wake::Idle,
        Err(_) => Wake::Failed,
    }
}

fn drain(wake: BorrowedFd<'_>) {
    let mut scratch = [0u8; 64];

    while let Ok(count) = rustix::io::read(wake, &mut scratch[..]) {
        if count < scratch.len() {
            break;
        }
    }
}

impl crate::bridge::ClipboardTransport for WaylandClipboard {
    fn paste(
        &self,
        preferences: &[crate::mime::MimeType],
    ) -> Result<crate::payload::Pasted, crate::error::ClipboardError> {
        self.paste_preferring(preferences.to_vec(), SeatSelector::Focused)
    }

    fn offer(&self, payload: crate::payload::Payload) -> Result<(), crate::error::ClipboardError> {
        WaylandClipboard::offer(self, payload)
    }

    fn has_focus(&self) -> Result<bool, crate::error::ClipboardError> {
        Ok(self.seats()?.iter().any(|seat| seat.focused))
    }

    fn native_drag(&self) -> Option<crate::drag::NativeDragClient> {
        Some(WaylandClipboard::native_drag(self))
    }

    fn paste_primary(
        &self,
        preferences: &[crate::mime::MimeType],
    ) -> Result<crate::payload::Pasted, crate::error::ClipboardError> {
        self.paste_primary_preferring(preferences.to_vec(), SeatSelector::Focused)
    }

    fn offer_primary(
        &self,
        payload: crate::payload::Payload,
    ) -> Result<(), crate::error::ClipboardError> {
        WaylandClipboard::offer_primary(self, payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_TIMEOUT: Duration = Duration::from_secs(5);
    const DROP_RETURN_TIMEOUT: Duration = Duration::from_secs(2);

    enum FixtureWait {
        Transfer(Receiver<()>),
    }

    struct WorkerFixture {
        clipboard: WaylandClipboard,
        #[allow(
            dead_code,
            reason = "held so the probe descriptor stays open for the fixture's life"
        )]
        wake_probe: Option<OwnedFd>,
        started: Receiver<()>,
        reclaimed: Receiver<bool>,
    }

    fn worker_fixture(wait: FixtureWait, observe_wake: bool) -> WorkerFixture {
        let (wake_read, wake_write) = transfer_pipe().expect("fixture wake pipe");
        let wake_probe = observe_wake
            .then(|| rustix::io::dup(wake_read.as_fd()).expect("duplicate fixture wake fd"));
        let (commands, command_rx) = channel();
        let (started_tx, started) = channel();
        let (reclaimed_tx, reclaimed) = channel();

        let worker = std::thread::Builder::new()
            .name(String::from("clipboard-drop-fixture"))
            .spawn(move || {
                let _ = started_tx.send(());
                let transfer_returned = match wait {
                    FixtureWait::Transfer(release) => release.recv_timeout(FIXTURE_TIMEOUT).is_ok(),
                };
                let stopped = transfer_returned
                    && matches!(command_rx.recv_timeout(FIXTURE_TIMEOUT), Ok(Command::Stop));
                drop(command_rx);
                drop(wake_read);
                let _ = reclaimed_tx.send(stopped);
            })
            .expect("spawn fixture worker");

        WorkerFixture {
            clipboard: WaylandClipboard {
                commands,
                wake: Arc::new(wake_write),
                native_pending: Arc::new(AtomicUsize::new(0)),
                limits: TransferLimits::default(),
                worker: Some(worker),
            },
            wake_probe,
            started,
            reclaimed,
        }
    }

    #[test]
    fn native_drag_command_admission_releases_slots_and_reports_shutdown() {
        let (wake_read, wake_write) = transfer_pipe().unwrap();
        set_non_blocking(wake_write.as_fd()).unwrap();
        let (commands, command_rx) = channel();
        let clipboard = WaylandClipboard {
            commands,
            wake: Arc::new(wake_write),
            native_pending: Arc::new(AtomicUsize::new(0)),
            limits: TransferLimits::default(),
            worker: None,
        };
        let native = clipboard.native_drag();
        for _ in 0..64 {
            let (reply, _) = channel();
            (native.submit)(crate::drag::DragCommand::Listen(reply)).unwrap();
        }
        let (reply, _) = channel();
        assert!(matches!(
            (native.submit)(crate::drag::DragCommand::Listen(reply)),
            Err(ClipboardError::Busy)
        ));
        let command = command_rx.recv_timeout(FIXTURE_TIMEOUT).unwrap();
        refuse(command, ClipboardError::Stopped);
        let (reply, _) = channel();
        (native.submit)(crate::drag::DragCommand::Listen(reply)).unwrap();
        drop(command_rx);
        assert_eq!(clipboard.native_pending.load(Ordering::Acquire), 0);
        assert!(matches!(native.listen(), Err(ClipboardError::Stopped)));
        drop(clipboard);
        drop(wake_read);
    }

    #[test]
    fn drop_detaches_a_parked_transfer_then_the_worker_reclaims_itself() {
        let (release_tx, release_rx) = channel();
        let WorkerFixture {
            clipboard,
            wake_probe: _,
            started,
            reclaimed,
        } = worker_fixture(FixtureWait::Transfer(release_rx), false);
        let started = started.recv_timeout(FIXTURE_TIMEOUT).is_ok();
        let (drop_done_tx, drop_done) = channel();
        let reaper = std::thread::Builder::new()
            .name(String::from("clipboard-drop-reaper"))
            .spawn(move || {
                let before = Instant::now();
                drop(clipboard);
                let _ = drop_done_tx.send(before.elapsed());
            })
            .expect("spawn drop reaper");

        let first_drop_result = drop_done.recv_timeout(DROP_RETURN_TIMEOUT);
        let returned_before_release = first_drop_result.is_ok();
        let _ = release_tx.send(());
        let elapsed = match first_drop_result {
            Ok(elapsed) => elapsed,
            Err(_) => drop_done
                .recv_timeout(FIXTURE_TIMEOUT)
                .expect("Drop did not return after the parked transfer was released"),
        };
        let stopped = reclaimed.recv_timeout(FIXTURE_TIMEOUT).unwrap_or(false);
        let reaper_joined = join_with_budget(reaper, FIXTURE_TIMEOUT);

        assert!(started, "the transfer fixture was not parked");
        assert!(
            returned_before_release,
            "Drop waited past its bounded join budget for a parked transfer"
        );
        assert!(
            elapsed >= DROP_JOIN_BUDGET,
            "Drop detached before offering the worker its join budget: {elapsed:?}"
        );
        assert!(
            elapsed < DROP_RETURN_TIMEOUT,
            "Drop exceeded its bounded return deadline: {elapsed:?}"
        );
        assert!(
            stopped,
            "the detached worker did not observe Stop and reclaim its owned resources"
        );
        assert!(reaper_joined, "the Drop reaper did not terminate");
    }
}
