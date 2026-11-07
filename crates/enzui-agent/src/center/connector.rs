//! The opt-in production connector to a running `enzui-center` daemon.
//!
//! [`CentralConnector::spawn`] connects an enrolled app outward to the daemon's
//! Unix socket on a background thread, negotiates a mutual v3/v2 handshake, sends
//! its registration, and then relays dispatched invocations and cancels to the
//! app through a bounded channel while writing the app's invocation events back.
//! Everything blocking happens off the caller's thread. A dropped connection is
//! retried with bounded backoff; a rejected credential, proof, or registration
//! stops the connector rather than hammering the daemon. The app owns the
//! [`crate::center::InvocationTable`] and decides how to answer each call; this
//! connector is transport only.
//!
//! # Cargo feature
//!
//! Reachable only with the crate's `central-connector` feature. An app must
//! additionally opt in explicitly, so no environment variable alone can expose
//! app data.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::peer::{self, ParentRefusal, SocketError};
use crate::center::CentralError;
use crate::center::auth::{self, AuthError};
use crate::center::credential::{self, AppCredential};
use crate::center::protocol::{
    AppRegistration, AppToBroker, AppToBrokerV3, BrokerToApp, BrokerToAppV3,
    CommitReportDisposition, FunctionRisk, Invocation, InvocationEvent, Uuid, WireRole,
};

mod commit;
use crate::center::unix_connect::{self, ConnectError, ShutdownWake};
use crate::center::wire::{self, WireError};
use crate::random;
use crate::runtime_dir;
use commit::CommitLedger;
pub use commit::{CommitReportError, CommitReportOutcome};

use std::os::unix::net::UnixStream;

// One budget for the entire setup: connect, authentication and registration
// draw on it together. A per-syscall timeout bounds no exchange, and a
// per-phase one leaves every earlier phase unbounded.
const SETUP_TIMEOUT: Duration = Duration::from_secs(5);

// `setsockopt` reads a zero timeout as "block forever", so a nearly exhausted
// budget rounds up to this instead of down to zero.
const MIN_SETUP_TIMEOUT: Duration = Duration::from_millis(1);

// Wait between attempts to hand a critical event to a full event queue.
const CRITICAL_EVENT_RETRY: Duration = Duration::from_millis(20);

// Ceiling on invocations bound to one generation, which is what keeps the
// binding's memory finite. Reaching it auto-fails the newest dispatch rather
// than delivering an invocation whose events could not be bound.
const MAX_LIVE_INVOCATIONS: usize = 1024;

// The daemon's one stable rejection message for a second live instance holding
// this credential. It is the only rejection this connector retries; matching it
// literally keeps every other (permanent) rejection fail-closed.
const DUPLICATE_INSTANCE_REJECTION: &str = "an instance of this app is already connected";

/// Backoff schedule for reconnection after a transient disconnect.
#[derive(Clone, Debug)]
pub struct ReconnectPolicy {
    /// The first backoff after a disconnect.
    pub initial_backoff: Duration,
    /// The ceiling the backoff doubles toward.
    pub max_backoff: Duration,
    /// The maximum number of reconnect attempts, or `None` for unbounded.
    pub max_attempts: Option<u32>,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial_backoff: Duration::from_millis(250),
            max_backoff: Duration::from_secs(10),
            max_attempts: None,
        }
    }
}

/// Everything the connector needs to reach and register with the daemon.
#[derive(Clone, Debug)]
pub struct ConnectorConfig {
    /// The daemon's Unix socket path.
    pub socket_path: PathBuf,
    /// The version-1 app credential file path.
    pub credential_path: PathBuf,
    /// The registration to send after authentication. Its app id must equal the
    /// credential id.
    pub registration: AppRegistration,
    /// Bound on undrained inbound invocation/cancel events. An invocation that
    /// arrives when the queue is full is auto-failed so the daemon is not left
    /// waiting.
    pub event_capacity: usize,
    /// Bound on outbound invocation events queued by the app.
    pub command_capacity: usize,
    /// Reconnection schedule.
    pub reconnect: ReconnectPolicy,
}

impl ConnectorConfig {
    /// A configuration with default queue bounds and reconnect policy.
    #[must_use]
    pub fn new(
        socket_path: PathBuf,
        credential_path: PathBuf,
        registration: AppRegistration,
    ) -> Self {
        Self {
            socket_path,
            credential_path,
            registration,
            event_capacity: 64,
            command_capacity: 64,
            reconnect: ReconnectPolicy::default(),
        }
    }
}

/// An event surfaced to the app from the connector's background thread.
#[derive(Clone, Debug)]
pub enum ConnectorEvent {
    /// The socket connected and the mutual handshake succeeded.
    Connected {
        /// The display name the daemon reported for this app credential.
        display_name: String,
    },
    /// The registration was accepted; the app may now receive invocations.
    Registered,
    /// The daemon dispatched a call. The app should open it in its
    /// [`crate::center::InvocationTable`] and answer with invocation events.
    Invoke(Box<Invocation>),
    /// The daemon asked to cancel an in-flight invocation.
    Cancel {
        /// The invocation to cancel.
        invocation_id: Uuid,
        /// The daemon-supplied reason.
        reason: String,
    },
    /// The connection dropped; the connector will reconnect unless stopped.
    Disconnected {
        /// A short, secret-free reason.
        reason: String,
    },
    /// The connector stopped for good: an unrecoverable rejection, reconnect
    /// exhaustion, or an explicit shutdown.
    Stopped {
        /// A short, secret-free reason.
        reason: String,
    },
    /// A commit report queued by [`CentralConnector::report_commit`] ended.
    /// Each queued report ends exactly once.
    CommitReport {
        /// The reported invocation.
        invocation_id: Uuid,
        /// The broker's answer, or why none can come.
        outcome: CommitReportOutcome,
    },
}

/// Why an outbound invocation event could not be queued.
#[derive(Debug, PartialEq, Eq)]
pub enum SendError {
    /// The outbound queue is full; the app should apply backpressure.
    QueueFull,
    /// The connector has stopped and will accept no more events.
    Stopped,
}

impl core::fmt::Display for SendError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::QueueFull => write!(f, "outbound event queue is full"),
            Self::Stopped => write!(f, "connector has stopped"),
        }
    }
}

impl std::error::Error for SendError {}

/// A handle to a running connector. Dropping it requests shutdown.
pub struct CentralConnector {
    events: Option<Receiver<ConnectorEvent>>,
    commands: SyncSender<Command>,
    shared: Arc<Shared>,
    supervisor: Option<JoinHandle<()>>,
}

// The queue outlives a connection, so an entry alone cannot say which one it
// belongs to. An `App` event is bound by its invocation id through
// `SessionScope`; a `Connector` frame answers an invocation the app never
// received, which is therefore not tracked as live, so it carries its
// generation instead.
enum Command {
    App(InvocationEvent),
    Connector {
        generation: u64,
        event: InvocationEvent,
    },
    // Written only on the connection `generation` names.
    CommitReport {
        generation: u64,
        invocation_id: Uuid,
    },
    // Wakes a writer blocked on an empty queue so it observes its link ended.
    // Carries nothing; a writer of a later connection discards a stale one.
    Wake,
}

struct Shared {
    // The shutdown request, and the descriptor a setup phase with no socket of
    // its own can wait on: closing a published stream only interrupts a
    // supervisor that already has one.
    wake: ShutdownWake,
    active_stream: Mutex<Option<UnixStream>>,
    // Connections served, so each session's outbound frames bind to one of them.
    generations: AtomicU64,
    wire_version: AtomicU64,
    commits: Mutex<CommitLedger>,
}

impl Shared {
    fn new() -> std::io::Result<Self> {
        Ok(Self {
            wake: ShutdownWake::new()?,
            active_stream: Mutex::new(None),
            generations: AtomicU64::new(0),
            wire_version: AtomicU64::new(0),
            commits: Mutex::new(CommitLedger::default()),
        })
    }

    fn commits(&self) -> std::sync::MutexGuard<'_, CommitLedger> {
        self.commits
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn is_shutdown(&self) -> bool {
        self.wake.is_tripped()
    }

    fn wake_and_stop(&self) {
        self.wire_version.store(0, Ordering::SeqCst);
        self.wake.signal();
        if let Ok(mut guard) = self.active_stream.lock()
            && let Some(stream) = guard.take()
        {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }

    // Publishes a connected socket to the shutdown handle before the first
    // authentication byte, so shutdown can interrupt setup as well as a served
    // session. The flag is read under the lock `wake_and_stop` takes, so a
    // racing shutdown either observes the stored stream or is observed here:
    // one of the two closes the socket, never neither.
    fn publish(&self, stream: &UnixStream) -> Result<(), SetupError> {
        let clone = stream
            .try_clone()
            .map_err(|error| SetupError::Transient(format!("socket clone failed: {error}")))?;
        let mut guard = self
            .active_stream
            .lock()
            .map_err(|_| SetupError::Transient("connector state unavailable".into()))?;
        if self.is_shutdown() {
            let _ = clone.shutdown(std::net::Shutdown::Both);
            return Err(SetupError::Transient("shutdown requested".into()));
        }
        *guard = Some(clone);
        Ok(())
    }

    fn next_generation(&self) -> u64 {
        self.generations
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1)
    }
}

// The outbound binding for one connection generation.
//
// Each session's scope starts empty, so every invocation from an earlier
// connection is unbound in this one: an app result produced after a
// reconnection is dropped rather than written on a link that never dispatched
// it.
struct SessionScope {
    generation: u64,
    live: Mutex<HashSet<String>>,
}

impl SessionScope {
    fn new(generation: u64) -> Self {
        Self {
            generation,
            live: Mutex::new(HashSet::new()),
        }
    }

    // `false` means the live-invocation ceiling is reached, so the caller must
    // refuse the dispatch rather than deliver an unbindable invocation.
    fn bind(&self, id: &Uuid) -> bool {
        match self.live.lock() {
            Ok(mut live) => {
                if live.len() >= MAX_LIVE_INVOCATIONS && !live.contains(id.as_str()) {
                    return false;
                }
                live.insert(id.as_str().to_owned());
                true
            }
            Err(_) => false,
        }
    }

    fn release(&self, id: &Uuid) {
        if let Ok(mut live) = self.live.lock() {
            live.remove(id.as_str());
        }
    }

    // Releases the binding when the admitted event is terminal.
    fn admit(&self, command: Command) -> Option<InvocationEvent> {
        let event = match command {
            Command::App(event) => {
                let bound = match self.live.lock() {
                    Ok(live) => live.contains(event.invocation_id().as_str()),
                    Err(_) => false,
                };
                if !bound {
                    return None;
                }
                event
            }
            Command::Connector { generation, event } if generation == self.generation => event,
            Command::Connector { .. } | Command::CommitReport { .. } | Command::Wake => {
                return None;
            }
        };
        if event.is_terminal() {
            self.release(event.invocation_id());
        }
        Some(event)
    }
}

impl CentralConnector {
    /// Spawns the connector's background thread.
    ///
    /// Validates the credential and registration up front so an obviously
    /// misconfigured app fails immediately rather than on a background thread.
    ///
    /// # Errors
    /// Returns [`CentralError`] when the credential cannot be loaded, the
    /// registration's app id does not match the credential id, or the current
    /// uid cannot be established. Failing to start the background thread or the
    /// interruption channel that makes [`shutdown`](Self::shutdown) prompt is
    /// [`CentralError::Spawn`], whose message names which of the two failed.
    pub fn spawn(config: ConnectorConfig) -> Result<Self, CentralError> {
        let credential = credential::load_app_credential(&config.credential_path)?;
        if credential.id != config.registration.app.id {
            return Err(CentralError::AppIdMismatch {
                credential: credential.id,
                registration: config.registration.app.id,
            });
        }
        let uid = runtime_dir::current_uid().ok_or(CentralError::OwnerUnverifiable)?;

        let (event_tx, events) = sync_channel(config.event_capacity.max(1));
        let (commands, command_rx) = sync_channel(config.command_capacity.max(1));
        // Built before the thread starts: a connector whose setup could not be
        // interrupted would be one `shutdown` cannot join.
        let shared = Arc::new(Shared::new().map_err(interruption_unavailable)?);
        shared.commits().side_effects = config
            .registration
            .functions
            .iter()
            .filter(|function| {
                matches!(
                    function.risk,
                    FunctionRisk::Mutating
                        | FunctionRisk::Destructive
                        | FunctionRisk::ExternalSideEffect
                )
            })
            .map(|function| function.id.clone())
            .collect();

        let supervisor = {
            let shared = Arc::clone(&shared);
            let command_tx = commands.clone();
            std::thread::Builder::new()
                .name("enzui-center-connector".into())
                .spawn(move || {
                    supervise(
                        config, credential, uid, event_tx, command_rx, command_tx, shared,
                    );
                })
                .map_err(CentralError::Spawn)?
        };

        Ok(Self {
            events: Some(events),
            commands,
            shared,
            supervisor: Some(supervisor),
        })
    }

    /// The authenticated wire version of the registered connection, or `None`.
    ///
    /// Version 3 proves commit-report capabilities; this is a snapshot, not
    /// authorization to send extension frames or a durable completion receipt.
    #[must_use]
    pub fn negotiated_wire_version(&self) -> Option<u32> {
        if self.shared.is_shutdown() {
            return None;
        }
        match self.shared.wire_version.load(Ordering::SeqCst) {
            2 => Some(2),
            3 => Some(3),
            _ => None,
        }
    }

    /// The receiver of connector events, for a polling drain loop.
    ///
    /// # Panics
    /// Panics if the receiver was already moved out by
    /// [`take_event_receiver`](Self::take_event_receiver).
    #[must_use]
    pub fn events(&self) -> &Receiver<ConnectorEvent> {
        self.events
            .as_ref()
            .expect("event receiver was taken by a consumer")
    }

    /// Moves the event receiver out for exactly one consumer, such as the ICED
    /// subscription adapter.
    ///
    /// Returns `None` on a second call. The connector keeps sending events and
    /// shutting down through the retained command and control channels; the
    /// returned receiver ends when the connector stops.
    pub fn take_event_receiver(&mut self) -> Option<Receiver<ConnectorEvent>> {
        self.events.take()
    }

    /// Queues one invocation event for the daemon.
    ///
    /// The event is bound to the connection that dispatched its invocation: if
    /// that connection has closed by the time the event is written, it is
    /// dropped rather than sent on the reconnected one, so a result can never
    /// answer a call the daemon no longer has.
    ///
    /// # Errors
    /// Returns [`SendError::QueueFull`] under backpressure or
    /// [`SendError::Stopped`] once the connector has stopped.
    pub fn send_event(&self, event: InvocationEvent) -> Result<(), SendError> {
        match self.commands.try_send(Command::App(event)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(SendError::QueueFull),
            Err(TrySendError::Disconnected(_)) => Err(SendError::Stopped),
        }
    }

    /// Reports acceptance of an invocation.
    ///
    /// # Errors
    /// See [`send_event`](Self::send_event).
    pub fn accept(&self, invocation_id: Uuid) -> Result<(), SendError> {
        self.send_event(InvocationEvent::Accepted { invocation_id })
    }

    /// Reports progress for an invocation.
    ///
    /// # Errors
    /// See [`send_event`](Self::send_event).
    pub fn progress(
        &self,
        invocation_id: Uuid,
        completed: f32,
        message: Option<String>,
    ) -> Result<(), SendError> {
        self.send_event(InvocationEvent::Progress {
            invocation_id,
            completed,
            message,
        })
    }

    /// Completes an invocation with its result.
    ///
    /// # Errors
    /// See [`send_event`](Self::send_event).
    pub fn complete(
        &self,
        invocation_id: Uuid,
        result: serde_json::Value,
    ) -> Result<(), SendError> {
        self.send_event(InvocationEvent::Completed {
            invocation_id,
            result,
        })
    }

    /// Fails an invocation with a code and message.
    ///
    /// # Errors
    /// See [`send_event`](Self::send_event).
    pub fn fail(
        &self,
        invocation_id: Uuid,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Result<(), SendError> {
        self.send_event(InvocationEvent::Failed {
            invocation_id,
            code: code.into(),
            message: message.into(),
        })
    }

    /// Confirms cancellation of an invocation.
    ///
    /// # Errors
    /// See [`send_event`](Self::send_event).
    pub fn cancelled(&self, invocation_id: Uuid) -> Result<(), SendError> {
        self.send_event(InvocationEvent::Cancelled { invocation_id })
    }

    /// Reports that the side effect of a call dispatched on this connector
    /// committed, including a call that ended (cancelled, past its deadline)
    /// after its effect was already made.
    ///
    /// Only a side-effect function's call can be reported, and only on the
    /// version-3 connection that dispatched it. The answer arrives as
    /// [`ConnectorEvent::CommitReport`]: the broker's acknowledgement, or
    /// [`CommitReportOutcome::ConnectionClosed`] when that connection ends
    /// first. A [`CommitReportDisposition::StorageUnavailable`] answer may be
    /// retried on the same connection. Queueing is not persistence, and the
    /// broker's original result for the call never changes.
    ///
    /// # Errors
    /// [`CommitReportError`] says why nothing was queued.
    pub fn report_commit(&self, invocation_id: &Uuid) -> Result<(), CommitReportError> {
        if self.shared.is_shutdown() {
            return Err(CommitReportError::Stopped);
        }
        let generation = self.shared.commits().queue(invocation_id, Instant::now())?;
        let command = Command::CommitReport {
            generation,
            invocation_id: invocation_id.clone(),
        };
        match self.commands.try_send(command) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.shared.commits().withdraw(invocation_id.as_str());
                Err(match error {
                    TrySendError::Full(_) => CommitReportError::QueueFull,
                    TrySendError::Disconnected(_) => CommitReportError::Stopped,
                })
            }
        }
    }

    /// Requests shutdown and joins the background thread.
    ///
    /// Unblocks the whole connection setup as well as a served session: an
    /// in-progress connect, an unauthenticated handshake, and any socket read
    /// or write. No task is left running after this returns, including when
    /// the daemon's accept queue is full and no socket has been admitted yet.
    pub fn shutdown(mut self) {
        self.stop_and_join();
    }

    fn stop_and_join(&mut self) {
        self.shared.wake_and_stop();
        if let Some(handle) = self.supervisor.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for CentralConnector {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

/// A connector with no thread behind it, for driving a host against a queue
/// of `capacity` that only `drain` empties.
#[cfg(test)]
pub(crate) fn detached(capacity: usize) -> (CentralConnector, DetachedQueue) {
    let (commands, queue) = sync_channel(capacity);
    let connector = CentralConnector {
        events: None,
        commands,
        shared: Arc::new(Shared::new().expect("connector shared state")),
        supervisor: None,
    };
    (connector, DetachedQueue(queue))
}

/// The far end of a [`detached`] connector's queue.
#[cfg(test)]
pub(crate) struct DetachedQueue(Receiver<Command>);

#[cfg(test)]
impl DetachedQueue {
    /// The app events queued so far, in order.
    pub(crate) fn drain(&self) -> Vec<InvocationEvent> {
        self.0
            .try_iter()
            .filter_map(|command| match command {
                Command::App(event) => Some(event),
                _ => None,
            })
            .collect()
    }
}

// A wake the setup can be interrupted through is a startup prerequisite like
// the thread itself, so this keeps one public variant while naming which half
// failed, and preserves the io kind for a caller that matches on it.
fn interruption_unavailable(error: std::io::Error) -> CentralError {
    let kind = error.kind();
    CentralError::Spawn(std::io::Error::new(
        kind,
        format!("shutdown interruption channel unavailable: {error}"),
    ))
}

fn supervise(
    config: ConnectorConfig,
    credential: AppCredential,
    uid: u32,
    event_tx: SyncSender<ConnectorEvent>,
    mut command_rx: Receiver<Command>,
    command_tx: SyncSender<Command>,
    shared: Arc<Shared>,
) {
    let mut backoff = config.reconnect.initial_backoff;
    let mut attempt = 0u32;

    loop {
        if shared.is_shutdown() {
            let _ = event_tx.try_send(ConnectorEvent::Stopped {
                reason: "shutdown requested".into(),
            });
            return;
        }

        match connect_and_register(&config, &credential, uid, &shared) {
            Ok((stream, display_name)) => {
                attempt = 0;
                backoff = config.reconnect.initial_backoff;
                let _ = event_tx.try_send(ConnectorEvent::Connected { display_name });
                let _ = event_tx.try_send(ConnectorEvent::Registered);
                command_rx = serve_session(stream, &event_tx, command_rx, &command_tx, &shared);
                clear_active(&shared);
                if shared.is_shutdown() {
                    let _ = event_tx.try_send(ConnectorEvent::Stopped {
                        reason: "shutdown requested".into(),
                    });
                    return;
                }
                // The app must cancel the work whose invocation ids died with
                // this connection, so a disconnect is never dropped.
                send_critical(
                    &event_tx,
                    ConnectorEvent::Disconnected {
                        reason: "connection closed".into(),
                    },
                    &shared,
                );
            }
            Err(SetupError::Fatal(reason)) => {
                // `Stopped` stays best-effort: `shutdown` joins this thread, so
                // waiting for an app that stopped draining would wedge that join.
                let _ = event_tx.try_send(ConnectorEvent::Stopped { reason });
                return;
            }
            Err(SetupError::Transient(reason)) => {
                send_critical(&event_tx, ConnectorEvent::Disconnected { reason }, &shared);
            }
        }

        attempt += 1;
        if let Some(max) = config.reconnect.max_attempts
            && attempt >= max
        {
            let _ = event_tx.try_send(ConnectorEvent::Stopped {
                reason: "reconnect attempts exhausted".into(),
            });
            return;
        }
        if !sleep_with_shutdown(&shared, backoff) {
            let _ = event_tx.try_send(ConnectorEvent::Stopped {
                reason: "shutdown requested".into(),
            });
            return;
        }
        backoff = (backoff * 2).min(config.reconnect.max_backoff);
    }
}

// Delivers a critical connector event without dropping it on a full queue.
//
// A cancel or disconnect a busy app never sees leaves that app running work the
// daemon has abandoned, so these wait for room. `false` means the event can
// never arrive: the consumer is gone, or shutdown ended the wait.
fn send_critical(
    event_tx: &SyncSender<ConnectorEvent>,
    event: ConnectorEvent,
    shared: &Shared,
) -> bool {
    let mut pending = event;
    loop {
        match event_tx.try_send(pending) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(returned)) => {
                if shared.is_shutdown() {
                    return false;
                }
                pending = returned;
                std::thread::sleep(CRITICAL_EVENT_RETRY);
            }
        }
    }
}

fn clear_active(shared: &Shared) {
    shared.wire_version.store(0, Ordering::SeqCst);
    if let Ok(mut guard) = shared.active_stream.lock() {
        *guard = None;
    }
}

// Sleeps up to `total`, returning `false` if shutdown was requested meanwhile.
fn sleep_with_shutdown(shared: &Shared, total: Duration) -> bool {
    let step = Duration::from_millis(100);
    let mut remaining = total;
    while remaining > Duration::ZERO {
        if shared.is_shutdown() {
            return false;
        }
        let slice = remaining.min(step);
        std::thread::sleep(slice);
        remaining = remaining.saturating_sub(slice);
    }
    !shared.is_shutdown()
}

// A setup failure classified by whether reconnecting could ever help: `Fatal`
// is an unrecoverable rejection (bad proof, wrong role, refused registration)
// that reconnecting only repeats; `Transient` is worth retrying.
//
// The reasons are short, secret-free classifications, so deriving `Debug` for
// assertions cannot disclose credential material.
#[derive(Debug)]
pub(crate) enum SetupError {
    Fatal(String),
    Transient(String),
}

fn connect_and_register(
    config: &ConnectorConfig,
    credential: &AppCredential,
    uid: u32,
    shared: &Shared,
) -> Result<(UnixStream, String), SetupError> {
    connect_and_register_within(
        config,
        credential,
        uid,
        shared,
        Instant::now() + SETUP_TIMEOUT,
    )
}

fn connect_and_register_within(
    config: &ConnectorConfig,
    credential: &AppCredential,
    uid: u32,
    shared: &Shared,
    deadline: Instant,
) -> Result<(UnixStream, String), SetupError> {
    for version in [3, 2] {
        peer::validate_socket_trust(&config.socket_path)
            .map_err(|error| classify_socket(&error))?;
        let stream = unix_connect::connect_within(&config.socket_path, deadline, &shared.wake)
            .map_err(classify_connect)?;
        peer::verify_peer_uid(&stream, uid).map_err(|error| classify_socket(&error))?;
        let published = PublishedSetup::publish(shared, &stream)?;
        let mut bounded = DeadlineStream::new(&stream, deadline);
        let client_nonce = mint_nonce().map_err(SetupError::Transient)?;
        let authenticate = if version == 3 {
            auth::authenticate_v3
        } else {
            auth::authenticate
        };
        let authenticated = authenticate(
            &mut bounded,
            credential.secret.expose_secret(),
            WireRole::App,
            &credential.id,
            uid,
            &client_nonce,
        );
        let authenticated = match authenticated {
            Err(AuthError::V3Unsupported) if version == 3 => continue,
            Err(AuthError::Wire(WireError::Malformed | WireError::InvalidLength { .. })) => {
                return Err(SetupError::Fatal("malformed authentication frame".into()));
            }
            other => other.map_err(classify_auth)?,
        };
        let display_name =
            register_authenticated(&mut bounded, &config.registration, authenticated)?;
        stream
            .set_read_timeout(None)
            .and_then(|()| stream.set_write_timeout(None))
            .map_err(|error| SetupError::Transient(format!("socket timeout: {error}")))?;
        shared.wire_version.store(version, Ordering::SeqCst);
        published.into_session();
        return Ok((stream, display_name));
    }
    unreachable!("version-2 authentication cannot request a fallback")
}

// A published setup socket, taken back down unless the setup reaches a session.
//
// Publication lets `shutdown` interrupt an unauthenticated handshake, so every
// exit before a session must withdraw it. Binding that to drop keeps `?` usable
// in between: an abandoned publication would otherwise absorb the next
// shutdown, closing a dead handle and leaving the live session running.
struct PublishedSetup<'a> {
    shared: &'a Shared,
    served: bool,
}

impl<'a> PublishedSetup<'a> {
    fn publish(shared: &'a Shared, stream: &UnixStream) -> Result<Self, SetupError> {
        shared.publish(stream)?;
        Ok(Self {
            shared,
            served: false,
        })
    }

    // The session owns the socket from here on.
    fn into_session(mut self) {
        self.served = true;
    }
}

impl Drop for PublishedSetup<'_> {
    fn drop(&mut self) {
        if !self.served {
            clear_active(self.shared);
        }
    }
}

// A `Read`/`Write` view of the setup socket under one total deadline.
//
// A per-syscall timeout bounds no exchange: a peer sending one byte per second
// never exceeds any single read's timeout, yet holds an unauthenticated
// connection open indefinitely. An exhausted budget here is `TimedOut` instead
// of another syscall.
struct DeadlineStream<'a> {
    stream: &'a UnixStream,
    deadline: Instant,
}

impl<'a> DeadlineStream<'a> {
    fn new(stream: &'a UnixStream, deadline: Instant) -> Self {
        Self { stream, deadline }
    }

    fn remaining(&self) -> std::io::Result<Duration> {
        let now = Instant::now();
        if now >= self.deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "handshake budget exhausted",
            ));
        }
        Ok(self
            .deadline
            .saturating_duration_since(now)
            .max(MIN_SETUP_TIMEOUT))
    }
}

impl std::io::Read for DeadlineStream<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.remaining()?;
        self.stream.set_read_timeout(Some(remaining))?;
        let mut stream = self.stream;
        stream.read(buf)
    }
}

impl std::io::Write for DeadlineStream<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let remaining = self.remaining()?;
        self.stream.set_write_timeout(Some(remaining))?;
        let mut stream = self.stream;
        stream.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut stream = self.stream;
        stream.flush()
    }
}

fn register_authenticated<S: std::io::Read + std::io::Write>(
    stream: &mut S,
    registration: &AppRegistration,
    authenticated: auth::Authenticated,
) -> Result<String, SetupError> {
    wire::write_message(
        stream,
        &AppToBroker::Register {
            registration: registration.clone(),
        },
    )
    .map_err(classify_wire)?;

    match wire::read_message::<_, BrokerToApp>(stream).map_err(classify_wire)? {
        BrokerToApp::Registered => Ok(authenticated.display_name),
        // The daemon emits one rejection envelope for several causes. Only a
        // duplicate live instance is worth retrying: it clears once the daemon
        // reaps the previous connection. Credential-id and validation
        // rejections are permanent, so classify by the daemon's one stable
        // duplicate message and fail closed on everything else (an unknown or
        // changed message included) rather than retrying a doomed registration.
        BrokerToApp::RegistrationRejected { message } => {
            if message == DUPLICATE_INSTANCE_REJECTION {
                Err(SetupError::Transient(
                    "registration rejected: duplicate instance".into(),
                ))
            } else {
                Err(SetupError::Fatal(format!(
                    "registration rejected: {message}"
                )))
            }
        }
        BrokerToApp::Invoke { .. } | BrokerToApp::Cancel { .. } => Err(SetupError::Transient(
            "unexpected pre-registration frame".into(),
        )),
    }
}

fn serve_session(
    stream: UnixStream,
    event_tx: &SyncSender<ConnectorEvent>,
    command_rx: Receiver<Command>,
    command_tx: &SyncSender<Command>,
    shared: &Shared,
) -> Receiver<Command> {
    // Nothing can reference this generation before the reader below binds the
    // first invocation dispatched on it.
    let scope = Arc::new(SessionScope::new(shared.next_generation()));
    let reports = shared.wire_version.load(Ordering::SeqCst) == 3;
    shared.commits().begin(scope.generation, reports);
    let link_dead = Arc::new(AtomicBool::new(false));
    let Ok(writer_stream) = stream.try_clone() else {
        return command_rx;
    };

    let writer_link_dead = Arc::clone(&link_dead);
    let writer_scope = Arc::clone(&scope);
    let writer = std::thread::Builder::new()
        .name("enzui-center-writer".into())
        .spawn(move || writer_loop(writer_stream, command_rx, &writer_link_dead, &writer_scope));
    let Ok(writer) = writer else {
        let (_tx, rx) = sync_channel(1);
        return rx;
    };

    reader_loop(stream, event_tx, shared, command_tx, &scope, reports);

    link_dead.store(true, Ordering::SeqCst);
    // A full queue needs no wake: the writer returns to its `link_dead` check
    // after the command it is about to take.
    let _ = command_tx.try_send(Command::Wake);
    if let Ok(guard) = shared.active_stream.lock()
        && let Some(active) = guard.as_ref()
    {
        let _ = active.shutdown(std::net::Shutdown::Both);
    }
    let Ok(command_rx) = writer.join() else {
        unreachable!("writer thread must not panic")
    };
    // A report this connection never answered can never be answered: the
    // broker binds it to this connection.
    let closed = shared.commits().end(scope.generation);
    for invocation_id in closed {
        send_critical(
            event_tx,
            ConnectorEvent::CommitReport {
                invocation_id,
                outcome: CommitReportOutcome::ConnectionClosed,
            },
            shared,
        );
    }
    command_rx
}

// One frame from the broker. A version-3 session also carries commit-report
// answers; a version-2 session parses exactly the version-2 frames.
enum Inbound {
    Broker(BrokerToApp),
    CommitReportAck {
        invocation_id: Uuid,
        disposition: CommitReportDisposition,
    },
}

fn read_inbound(stream: &mut UnixStream, reports: bool) -> Result<Inbound, WireError> {
    if !reports {
        return wire::read_message::<_, BrokerToApp>(stream).map(Inbound::Broker);
    }
    Ok(match wire::read_message::<_, BrokerToAppV3>(stream)? {
        BrokerToAppV3::Registered => Inbound::Broker(BrokerToApp::Registered),
        BrokerToAppV3::RegistrationRejected { message } => {
            Inbound::Broker(BrokerToApp::RegistrationRejected { message })
        }
        BrokerToAppV3::Invoke { invocation } => Inbound::Broker(BrokerToApp::Invoke { invocation }),
        BrokerToAppV3::Cancel {
            invocation_id,
            reason,
        } => Inbound::Broker(BrokerToApp::Cancel {
            invocation_id,
            reason,
        }),
        BrokerToAppV3::CommitReportAck {
            invocation_id,
            disposition,
        } => Inbound::CommitReportAck {
            invocation_id,
            disposition,
        },
    })
}

fn reader_loop(
    mut stream: UnixStream,
    event_tx: &SyncSender<ConnectorEvent>,
    shared: &Shared,
    connector_tx: &SyncSender<Command>,
    scope: &SessionScope,
    reports: bool,
) {
    loop {
        if shared.is_shutdown() {
            return;
        }
        match read_inbound(&mut stream, reports) {
            Ok(Inbound::CommitReportAck {
                invocation_id,
                disposition,
            }) => {
                // An answer to a report this connection did not queue is
                // ignored rather than attributed to some other call.
                if shared
                    .commits()
                    .answer(invocation_id.as_str(), scope.generation)
                    && !send_critical(
                        event_tx,
                        ConnectorEvent::CommitReport {
                            invocation_id,
                            outcome: CommitReportOutcome::Broker(disposition),
                        },
                        shared,
                    )
                {
                    return;
                }
            }
            Ok(Inbound::Broker(BrokerToApp::Invoke { invocation })) => {
                let invocation_id = invocation.invocation_id.clone();
                // Bind before the app can see the invocation, so an immediate
                // answer is already bound to this session.
                if !scope.bind(&invocation_id) {
                    auto_fail(
                        &invocation_id,
                        "live invocation limit reached",
                        connector_tx,
                        scope.generation,
                    );
                    continue;
                }
                shared.commits().record(&invocation, scope.generation);
                let event = ConnectorEvent::Invoke(Box::new(invocation));
                match event_tx.try_send(event) {
                    Ok(()) => {}
                    Err(TrySendError::Full(_)) => {
                        scope.release(&invocation_id);
                        auto_fail(
                            &invocation_id,
                            "inbound queue is full",
                            connector_tx,
                            scope.generation,
                        );
                    }
                    Err(TrySendError::Disconnected(_)) => return,
                }
            }
            Ok(Inbound::Broker(BrokerToApp::Cancel {
                invocation_id,
                reason,
            })) => {
                if !send_critical(
                    event_tx,
                    ConnectorEvent::Cancel {
                        invocation_id,
                        reason,
                    },
                    shared,
                ) {
                    return;
                }
            }
            Ok(Inbound::Broker(
                BrokerToApp::Registered | BrokerToApp::RegistrationRejected { .. },
            )) => return,
            Err(error) if error.is_timeout() => continue,
            Err(_) => return,
        }
    }
}

// Answers a dispatch this connector could not hand to the app, so the daemon is
// not left waiting on an invocation the app will never see. Its id was never
// delivered and so is not tracked as live, hence the explicit generation.
fn auto_fail(
    invocation_id: &Uuid,
    message: &str,
    connector_tx: &SyncSender<Command>,
    generation: u64,
) {
    let _ = connector_tx.try_send(Command::Connector {
        generation,
        event: InvocationEvent::Failed {
            invocation_id: invocation_id.clone(),
            code: "app_busy".into(),
            message: message.to_owned(),
        },
    });
}

fn writer_loop(
    mut stream: UnixStream,
    command_rx: Receiver<Command>,
    link_dead: &AtomicBool,
    scope: &SessionScope,
) -> Receiver<Command> {
    loop {
        if link_dead.load(Ordering::SeqCst) {
            return command_rx;
        }
        // Blocks: the reader posts `Command::Wake` once it marks the link
        // dead, and the supervisor holds a sender for as long as it serves.
        match command_rx.recv() {
            Ok(command) => {
                let written = match command {
                    // A report queued for an earlier connection is dropped:
                    // that connection's end already answered it.
                    Command::CommitReport {
                        generation,
                        invocation_id,
                    } if generation == scope.generation => wire::write_message(
                        &mut stream,
                        &AppToBrokerV3::CommitReport { invocation_id },
                    ),
                    Command::CommitReport { .. } | Command::Wake => continue,
                    command => {
                        // A queued event from an earlier connection is dropped
                        // here: its invocation belongs to a link the daemon
                        // already closed.
                        let Some(event) = scope.admit(command) else {
                            continue;
                        };
                        wire::write_message(&mut stream, &AppToBroker::Event { event })
                    }
                };
                if written.is_err() {
                    // The link is dead, and the paired reader is blocked inside
                    // `read` on the same socket. Shutting the socket down wakes
                    // it, so the session ends and the supervisor reconnects
                    // instead of stranding a reader on a write-dead link.
                    link_dead.store(true, Ordering::SeqCst);
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    return command_rx;
                }
            }
            Err(std::sync::mpsc::RecvError) => return command_rx,
        }
    }
}

fn mint_nonce() -> Result<String, String> {
    let bytes: [u8; 32] =
        random::random_array().map_err(|error| format!("entropy unavailable: {error}"))?;
    Ok(hex::encode(bytes))
}

fn classify_socket(error: &SocketError) -> SetupError {
    match error {
        SocketError::WrongOwner { .. }
        | SocketError::PeerUidMismatch { .. }
        | SocketError::IsSymlink
        | SocketError::NotASocket
        | SocketError::InsecureMode { .. }
        | SocketError::OwnerUnverifiable => SetupError::Fatal(format!("socket refused: {error}")),
        // A parent another principal owns, may write, or may merely reach is a
        // misconfiguration reconnecting cannot fix, so it fails closed instead
        // of retrying against a path outside the daemon's contract. Only "the
        // directory could not be read at all" is a transient filesystem state.
        SocketError::InsecureParent(ParentRefusal::Unreadable(_)) => {
            SetupError::Transient(format!("socket unavailable: {error}"))
        }
        SocketError::InsecureParent(_) => SetupError::Fatal(format!("socket refused: {error}")),
        SocketError::NotFound(_) | SocketError::PeerCredentialUnavailable(_) => {
            SetupError::Transient(format!("socket unavailable: {error}"))
        }
    }
}

// A full accept queue drains, a refused connection means a daemon that is not
// listening yet, and an interruption is the shutdown the supervisor observes
// next, so all three retry. A path that cannot be a Unix address never becomes
// one, so it fails closed instead.
fn classify_connect(error: ConnectError) -> SetupError {
    match error {
        ConnectError::Interrupted => SetupError::Transient("shutdown requested".into()),
        ConnectError::TimedOut(pending) => {
            SetupError::Transient(format!("connect timed out: {pending}"))
        }
        ConnectError::Failed(error) => SetupError::Transient(format!("connect failed: {error}")),
        ConnectError::Unaddressable(reason) => {
            SetupError::Fatal(format!("socket refused: {reason}"))
        }
    }
}

fn classify_auth(error: AuthError) -> SetupError {
    match error {
        AuthError::Rejected
        | AuthError::BadBrokerProof
        | AuthError::UnsupportedVersion(_)
        | AuthError::BadChallengeNonce
        | AuthError::BadClientNonce
        | AuthError::BadCapabilities => {
            SetupError::Fatal(format!("authentication failed: {error}"))
        }
        AuthError::V3Unsupported | AuthError::Wire(_) => {
            SetupError::Transient(format!("authentication io: {error}"))
        }
    }
}

fn classify_wire(error: WireError) -> SetupError {
    SetupError::Transient(format!("registration io: {error}"))
}

#[cfg(test)]
#[path = "connector_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "connector_negotiation_tests.rs"]
mod negotiation_tests;

#[cfg(test)]
#[path = "connector_commit_tests.rs"]
mod commit_tests;
