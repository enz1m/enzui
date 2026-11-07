//! A process-wide app session for the `enzui-center` protocol.
//!
//! The app owns its registry, connector-event messages and ordinary reducers.
//! [`CenterHost::receive_with_ends`] admits calls and names pending work to
//! cancel. Keep [`CenterCall::reference`] beside an authorization cancellation
//! id; schedule [`CenterHost::poll_ended`] from [`CenterHost::next_deadline`]
//! while idle, and recheck [`CenterHost::is_live`] immediately before committing.
//!
//! Headless applications use [`CenterHost::spawn_headless`] and
//! [`CenterHost::poll_headless`] with only `central-connector` enabled. GUI
//! applications enable `iced-consumer` and use [`CenterHost::receive_with_ends`]
//! through their subscription instead. Both paths admit the same typed commands.
//!
//! The host enforces deadlines, result budgets, connection generations,
//! idempotent replay and at most one terminal event. [`CenterHost::complete`]
//! cannot undo an external commit or overwrite an already-terminal broker result.
//! Apps should retain the invocation identity for their own commit audit.
//!
//! [`CenterProvision::discover`] requires an enrolled app credential; an
//! environment override cannot create one. The app is an outbound client,
//! exposes nothing without opt-in, and never reads the center control credential.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(feature = "iced-consumer")]
use iced_futures::Subscription;
use serde_json::Value;

use crate::center::CentralError;
use crate::center::connector::{
    CentralConnector, CommitReportError, CommitReportOutcome, ConnectorConfig, ConnectorEvent,
    SendError,
};
#[cfg(feature = "iced-consumer")]
use crate::center::iced_consumer::{ConnectorEvents, connector_subscription};
use crate::center::lifecycle::{EmitDecision, InvocationTable};
use crate::center::protocol::{AppMetadata, Invocation, InvocationEvent, Uuid};
use crate::center::registry::{AppFunctionRegistry, DispatchError, RegistryError};

/// Environment override for the app credential path.
pub const CREDENTIAL_ENV: &str = "ENZUI_CENTER_CREDENTIAL";
/// Credential path supplied by installed application user units.
pub const APP_CREDENTIAL_ENV: &str = "ENZUI_APP_CREDENTIAL";
/// Environment override for the daemon socket path.
pub const SOCKET_ENV: &str = "ENZUI_CENTER_SOCKET";

/// Stored results replayed for a repeated idempotency key.
const REPLAY_CAPACITY: usize = 64;
/// Events kept in order while the connector's queue is full. Past this the
/// connection is not draining at all, and the session ends.
const OUTBOX_CAPACITY: usize = 256;
/// How soon [`CenterHost::next_deadline`] asks to be polled again while
/// events wait in the outbox.
const OUTBOX_RETRY: Duration = Duration::from_millis(20);
/// The subscription identity; one host per app process.
#[cfg(feature = "iced-consumer")]
const SUBSCRIPTION_ID: u64 = 0x_c3e7_7e12;

/// Where an app finds the daemon and its own enrolled credential.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CenterProvision {
    /// The daemon's Unix socket.
    pub socket_path: PathBuf,
    /// This app's version-1 credential file.
    pub credential_path: PathBuf,
}

impl CenterProvision {
    /// The provisioned paths for `app_id`, or `None` when no credential has
    /// been enrolled for it.
    ///
    /// The credential is `$ENZUI_APP_CREDENTIAL`, else
    /// `$ENZUI_CENTER_CREDENTIAL`, else
    /// `$XDG_STATE_HOME/enzui/apps/<app_id>.credential.json` (default
    /// `$HOME/.local/state`); inside a Flatpak sandbox, whose
    /// `$XDG_STATE_HOME` is the app's own, also the host's state directory
    /// (`$HOST_XDG_STATE_HOME`, default `$HOME/.local/state`), where MCP
    /// Center enrolls it; then the legacy XDG config location. An existing
    /// state credential never falls through to a legacy identity. The socket is
    /// `$ENZUI_CENTER_SOCKET`, else `$XDG_RUNTIME_DIR/enzui/center.sock`. The
    /// credential's contents are checked by the connector, not here.
    #[must_use]
    pub fn discover(app_id: &str) -> Option<Self> {
        Self::discover_with(app_id, |key| std::env::var_os(key))
    }

    fn discover_with(
        app_id: &str,
        env: impl Fn(&str) -> Option<std::ffi::OsString>,
    ) -> Option<Self> {
        if !super::registry::valid_id(app_id) {
            return None;
        }
        let credential_path = env(APP_CREDENTIAL_ENV)
            .or_else(|| env(CREDENTIAL_ENV))
            .map(PathBuf::from)
            .or_else(|| {
                let name = format!("{app_id}.credential.json");
                let state = xdg_home(&env, "XDG_STATE_HOME", ".local/state");
                let host = env("FLATPAK_ID")
                    .and_then(|_| xdg_home(&env, "HOST_XDG_STATE_HOME", ".local/state"));
                for home in [state, host].into_iter().flatten() {
                    let path = home.join("enzui/apps").join(&name);
                    if !matches!(path.symlink_metadata(), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
                    {
                        return Some(path);
                    }
                }
                xdg_home(&env, "XDG_CONFIG_HOME", ".config")
                    .map(|home| home.join("enzui/apps").join(name))
            })?;
        if !credential_path.is_file() {
            return None;
        }
        let socket_path = env(SOCKET_ENV).map(PathBuf::from).or_else(|| {
            crate::runtime_dir::runtime_dir()
                .ok()
                .map(|dir| dir.join("enzui").join("center.sock"))
        })?;
        Some(Self {
            socket_path,
            credential_path,
        })
    }
}

fn xdg_home(
    env: &impl Fn(&str) -> Option<std::ffi::OsString>,
    key: &str,
    fallback: &str,
) -> Option<PathBuf> {
    env(key)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| {
            env("HOME")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|home| home.join(fallback))
        })
}

/// Why a host could not start.
#[derive(Debug)]
pub enum HostError {
    /// The registry or app metadata failed registration validation.
    Registry(RegistryError),
    /// The connector could not start (credential, owner or spawn).
    Central(CentralError),
}

impl core::fmt::Display for HostError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Registry(error) => write!(f, "center registration is invalid: {error}"),
            Self::Central(error) => write!(f, "center connector could not start: {error}"),
        }
    }
}

impl std::error::Error for HostError {}

/// The connection as the app may present it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CenterStatus {
    /// Started; no handshake has completed yet.
    Connecting,
    /// Authenticated with the daemon.
    Connected,
    /// Registration accepted: calls may arrive.
    Registered,
    /// The connection dropped and is being retried.
    Disconnected(String),
    /// The connector stopped for good.
    Stopped(String),
}

impl CenterStatus {
    /// Whether the agent can call this app now. Show agent-only controls
    /// while this holds and hide them otherwise; the app itself works the
    /// same either way.
    #[must_use]
    pub fn is_available(&self) -> bool {
        matches!(self, Self::Registered)
    }
}

/// One admitted call, answered exactly once through the host.
///
/// Deliberately not `Clone`: [`CenterHost::complete`] and [`CenterHost::fail`]
/// consume it, so an app cannot answer one call twice.
#[derive(Debug, PartialEq, Eq)]
pub struct CenterCall {
    invocation_id: Uuid,
    function_id: String,
    generation: u64,
    reference: CallRef,
}

impl CenterCall {
    /// A cheap identity for matching cancellation to app-owned pending work.
    #[must_use]
    pub fn reference(&self) -> CallRef {
        self.reference.clone()
    }

    /// The registered function this call invokes.
    #[must_use]
    pub fn function_id(&self) -> &str {
        &self.function_id
    }
}

/// A comparable identity for one admission, distinct across hosts and reconnects.
#[derive(Clone, Debug)]
pub struct CallRef(Arc<Uuid>);

impl PartialEq for CallRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for CallRef {}
impl std::hash::Hash for CallRef {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&Arc::as_ptr(&self.0), state);
    }
}
impl CallRef {
    /// The broker's invocation id, suitable for a local commit audit record.
    #[must_use]
    pub fn invocation_id(&self) -> &Uuid {
        &self.0
    }
}

/// Why an admitted call ended before the app answered it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// The caller cancelled its request.
    Cancelled,
    /// The call's absolute deadline was reached.
    Deadline,
    /// The broker or caller connection disappeared.
    Disconnected,
    /// The broker revoked the caller's credential or permission.
    Revoked,
    /// The app connector stopped permanently.
    Stopped,
}

/// An ended call whose pending authorization or work should be cancelled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CenterEnded {
    /// The identity retained beside the app's pending work.
    pub call: CallRef,
    /// The cause observed by the host.
    pub reason: EndReason,
}

/// The end of a commit report the app made with [`CenterHost::report_commit`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitReported {
    /// The reported call.
    pub call: CallRef,
    /// The broker's answer, or why none can come.
    pub outcome: CommitReportOutcome,
}

/// An event's admission decision and every call it ended.
#[derive(Debug)]
pub struct CenterUpdate<C> {
    /// A newly admitted call or a status-only update.
    pub turn: CenterTurn<C>,
    /// Pending work to stop, including deadlines reached before this event.
    pub ended: Vec<CenterEnded>,
    /// Commit reports this event answered.
    pub reported: Vec<CommitReported>,
}

/// How the host handled a completion; queueing does not prove broker delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionDisposition {
    /// The completion was queued, or recorded by an offline host.
    Queued,
    /// The outbound queue was unavailable; no completion was queued.
    NotQueued,
    /// This admission had already ended or belongs to another host.
    Ended,
    /// The deadline passed before completion; a failure was attempted instead.
    DeadlineExceeded,
    /// The result exceeded its declared budget; a failure was attempted instead.
    ResultTooLarge,
}

/// A completion identity and disposition for the app's local commit audit.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use]
pub struct CompletionReceipt {
    /// Retain this invocation identity when an external side effect committed.
    pub call: CallRef,
    /// Whether the host queued a completion or why it could not.
    pub disposition: CompletionDisposition,
}

/// A refused call: a short machine-readable code and a sentence for the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CenterFailure {
    /// A short token such as `not_found` or `unavailable`.
    pub code: &'static str,
    /// A sentence with no private data.
    pub message: String,
}

impl CenterFailure {
    /// A failure with `code` and `message`.
    #[must_use]
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// What an app's handler reports for one call.
pub type CenterOutcome = Result<Value, CenterFailure>;

/// An invocation as the daemon would dispatch it, for app tests: a fresh id,
/// a test caller, no deadline and no idempotency key.
///
/// # Panics
///
/// When the OS has no entropy for the invocation id.
#[must_use]
pub fn test_invocation(function_id: &str, arguments: Value) -> Invocation {
    use crate::center::protocol::AgentCaller;
    Invocation {
        invocation_id: Uuid::new_v4().expect("entropy for a test invocation id"),
        function_id: function_id.into(),
        arguments,
        caller: AgentCaller {
            credential_id: "agent.test".into(),
            display_name: "Test agent".into(),
            session_id: Uuid::new_v4().expect("entropy for a test session id"),
        },
        deadline_unix_ms: 0,
        idempotency_key: None,
    }
}

/// What [`CenterHost::receive`] asks of the app.
#[derive(Debug)]
pub enum CenterTurn<C> {
    /// Apply `C` through the app's own messages, then answer `CenterCall`.
    Call(CenterCall, C),
    /// Nothing for the app to do beyond, perhaps, repainting its status.
    Handled,
}

struct Live {
    reference: CallRef,
    invocation_id: Uuid,
    generation: u64,
    deadline: Option<SystemTime>,
    max_result_bytes: u32,
    replay_key: Option<Replay>,
}

/// What a stored result answers: the authenticated caller, the function and
/// the caller's idempotency key, bound to the arguments the call was
/// admitted with.
#[derive(Clone, Debug, PartialEq)]
struct Replay {
    caller: String,
    function: String,
    key: String,
    arguments: Value,
}

impl Replay {
    fn same_request(&self, other: &Self) -> bool {
        self.caller == other.caller && self.function == other.function && self.key == other.key
    }
}

/// The app's process-wide center session.
pub struct CenterHost<C> {
    connector: Option<CentralConnector>,
    #[cfg(feature = "iced-consumer")]
    events: Option<ConnectorEvents>,
    headless_events: Option<std::sync::mpsc::Receiver<ConnectorEvent>>,
    registry: AppFunctionRegistry<C>,
    table: InvocationTable,
    live: HashMap<String, Live>,
    generation: u64,
    replay: VecDeque<(Replay, Value)>,
    status: CenterStatus,
    admitting: bool,
    sent: Vec<InvocationEvent>,
    // Calls with a commit report awaiting its answer, by invocation id. The
    // connector ends each report exactly once, which removes its entry.
    reports: HashMap<String, CallRef>,
    /// Events the connector's full queue could not take yet, in order. Sent
    /// on the next turn; the reducer never waits for room.
    outbox: VecDeque<InvocationEvent>,
    /// The outbox overflowed: the next turn ends the session's calls.
    outbox_overflowed: bool,
}

impl<C: 'static> CenterHost<C> {
    /// Registers `registry` as `app` and starts the connector.
    ///
    /// # Errors
    /// [`HostError::Registry`] when the registration does not validate, or
    /// [`HostError::Central`] when the connector cannot start.
    #[cfg(feature = "iced-consumer")]
    pub fn spawn(
        provision: CenterProvision,
        app: AppMetadata,
        registry: AppFunctionRegistry<C>,
    ) -> Result<Self, HostError> {
        let registration = registry.registration(app).map_err(HostError::Registry)?;
        let config = ConnectorConfig::new(
            provision.socket_path,
            provision.credential_path,
            registration,
        );
        let mut connector = CentralConnector::spawn(config).map_err(HostError::Central)?;
        let events = connector.take_event_receiver().map(ConnectorEvents::new);
        let mut host = Self::offline(registry);
        host.events = events;
        host.connector = Some(connector);
        Ok(host)
    }

    /// Starts the same typed host with a receiver owned by a headless event loop.
    ///
    /// This path needs only `central-connector`; it does not initialize iced,
    /// graphics, or a display. Use [`Self::poll_headless`] instead of a GUI
    /// subscription, and keep application work bounded between polls.
    ///
    /// # Errors
    /// Returns invalid registration or connector startup failures.
    pub fn spawn_headless(
        provision: CenterProvision,
        app: AppMetadata,
        registry: AppFunctionRegistry<C>,
    ) -> Result<Self, HostError> {
        let registration = registry.registration(app).map_err(HostError::Registry)?;
        let config = ConnectorConfig::new(
            provision.socket_path,
            provision.credential_path,
            registration,
        );
        let mut connector = CentralConnector::spawn(config).map_err(HostError::Central)?;
        let mut host = Self::offline(registry);
        host.headless_events = connector.take_event_receiver();
        host.connector = Some(connector);
        Ok(host)
    }

    /// Waits for one headless event, stopping at the earliest live deadline.
    ///
    /// Timeouts retire expired calls even when no broker event arrives. A closed
    /// receiver stops admission and retires live calls. Applications must process
    /// `ended` before committing effects and track their own windows/work for
    /// idle exit; an empty result alone does not establish that exit is safe.
    ///
    /// # Errors
    /// Returns `InvalidInput` for a GUI or offline host, leaving it unchanged.
    pub fn poll_headless(&mut self, timeout: Duration) -> std::io::Result<CenterUpdate<C>> {
        let receiver = self.headless_events.as_ref().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "host has no headless receiver",
            )
        })?;
        let wait = self.next_deadline().map_or(timeout, |deadline| {
            timeout.min(
                deadline
                    .duration_since(SystemTime::now())
                    .unwrap_or_default(),
            )
        });
        match receiver.recv_timeout(wait) {
            Ok(event) => Ok(self.receive_with_ends(event)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(CenterUpdate {
                turn: CenterTurn::Handled,
                ended: self.poll_ended(),
                reported: Vec::new(),
            }),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Ok(self.receive_with_ends(ConnectorEvent::Stopped {
                    reason: "headless connector receiver closed".into(),
                }))
            }
        }
    }

    /// Transfers an existing headless receiver to GUI subscription delivery.
    ///
    /// Call once before starting the GUI subscription. Live calls, registry,
    /// broker connection and replay state remain in this host; queued events
    /// move with the receiver. Returns false for an offline or already-GUI host.
    /// This opens no window and consumes no activation context.
    #[cfg(feature = "iced-consumer")]
    pub fn promote_to_gui(&mut self) -> bool {
        if self.events.is_some() {
            return false;
        }
        let Some(receiver) = self.headless_events.take() else {
            return false;
        };
        self.events = Some(ConnectorEvents::new(receiver));
        true
    }

    /// A host with no connector: every event it would send is kept for
    /// [`Self::take_sent`]. For tests of an app's center wiring.
    #[must_use]
    pub fn offline(registry: AppFunctionRegistry<C>) -> Self {
        Self {
            connector: None,
            #[cfg(feature = "iced-consumer")]
            events: None,
            headless_events: None,
            registry,
            table: InvocationTable::new(),
            live: HashMap::new(),
            generation: 0,
            replay: VecDeque::new(),
            status: CenterStatus::Connecting,
            admitting: true,
            sent: Vec::new(),
            reports: HashMap::new(),
            outbox: VecDeque::new(),
            outbox_overflowed: false,
        }
    }

    /// The events an offline host would have sent, oldest first.
    pub fn take_sent(&mut self) -> Vec<InvocationEvent> {
        std::mem::take(&mut self.sent)
    }

    /// The registry this host registered.
    #[must_use]
    pub fn registry(&self) -> &AppFunctionRegistry<C> {
        &self.registry
    }

    /// The connection state.
    #[must_use]
    pub fn status(&self) -> &CenterStatus {
        &self.status
    }

    /// Whether the agent can call this app now: [`CenterStatus::is_available`].
    /// An app without a host (no enzUI agent service on this system) has no
    /// agent at all.
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.status.is_available()
    }

    /// Calls admitted and not yet answered.
    #[must_use]
    pub fn live_calls(&self) -> usize {
        self.live.len()
    }

    /// The connector's events as app messages. Offline hosts subscribe to
    /// nothing.
    #[cfg(feature = "iced-consumer")]
    pub fn subscription<M, F>(&self, map: F) -> Subscription<M>
    where
        M: Send + 'static,
        F: Fn(ConnectorEvent) -> M + Send + 'static,
    {
        match &self.events {
            Some(events) => connector_subscription(events, SUBSCRIPTION_ID, map),
            None => Subscription::none(),
        }
    }

    /// Whether this admission still belongs to this host and precedes its deadline.
    ///
    /// Check immediately before a side effect. This cannot revoke an operation
    /// already submitted to another service or replace processing connector events.
    #[must_use]
    pub fn is_live(&self, call: &CenterCall) -> bool {
        self.matching_live(call).is_some_and(|live| {
            live.deadline
                .is_none_or(|deadline| SystemTime::now() < deadline)
        })
    }

    /// The deadline of this still-admitted call, or `None` for an ended or untimed call.
    #[must_use]
    pub fn deadline(&self, call: &CenterCall) -> Option<SystemTime> {
        self.matching_live(call).and_then(|live| live.deadline)
    }

    /// The earliest admitted deadline, or a prompt retry while events wait for
    /// the connector; schedule an app message to [`Self::poll_ended`].
    #[must_use]
    pub fn next_deadline(&self) -> Option<SystemTime> {
        let retry = (!self.outbox.is_empty()).then(|| SystemTime::now() + OUTBOX_RETRY);
        self.live
            .values()
            .filter_map(|live| live.deadline)
            .chain(retry)
            .min()
    }

    /// Retire expired calls and return their identities once, without a connector event.
    ///
    /// Call from the app's deadline timer so pending prompts can be dismissed.
    /// [`Self::is_live`] rejects expired calls even before this timer runs.
    pub fn poll_ended(&mut self) -> Vec<CenterEnded> {
        self.flush_outbox();
        let mut ended = self.expire_at(SystemTime::now());
        if std::mem::take(&mut self.outbox_overflowed) {
            // The connection stopped draining: its calls cannot be answered.
            self.outbox.clear();
            ended.extend(self.retire_generation(EndReason::Disconnected));
        }
        ended
    }

    fn expire_at(&mut self, now: SystemTime) -> Vec<CenterEnded> {
        let expired: Vec<_> = self
            .live
            .iter()
            .filter(|(_, live)| live.deadline.is_some_and(|deadline| now >= deadline))
            .map(|(id, _)| id.clone())
            .collect();
        let mut ended = Vec::with_capacity(expired.len());
        for id in expired {
            let live = self.live.remove(&id).expect("selected live call");
            ended.push(CenterEnded {
                call: live.reference,
                reason: EndReason::Deadline,
            });
            self.terminal(InvocationEvent::Failed {
                invocation_id: live.invocation_id,
                code: "deadline_exceeded".into(),
                message: "the call's deadline passed before it finished".into(),
            });
        }
        ended
    }

    /// Apply an event, discarding ended-call notices for legacy handlers.
    ///
    /// Side-effecting handlers should use [`Self::receive_with_ends`] instead.
    #[deprecated(
        note = "drops the calls that ended; use `receive_with_ends` and cancel the work it names"
    )]
    pub fn receive(&mut self, event: ConnectorEvent) -> CenterTurn<C> {
        self.receive_with_ends(event).turn
    }

    /// Apply an event and name calls whose pending work must stop.
    ///
    /// Drain connector events and cancel work named in `ended` before dispatching
    /// `turn`. Use [`Self::next_deadline`] and [`Self::poll_ended`] while idle.
    pub fn receive_with_ends(&mut self, event: ConnectorEvent) -> CenterUpdate<C> {
        let mut ended = self.poll_ended();
        match event {
            ConnectorEvent::Connected { .. } => self.status = CenterStatus::Connected,
            ConnectorEvent::Registered => self.status = CenterStatus::Registered,
            ConnectorEvent::Invoke(invocation) => {
                return CenterUpdate {
                    turn: self.admit(*invocation),
                    ended,
                    reported: Vec::new(),
                };
            }
            ConnectorEvent::CommitReport {
                invocation_id,
                outcome,
            } => {
                let reported = self
                    .reports
                    .remove(invocation_id.as_str())
                    .map(|call| CommitReported { call, outcome })
                    .into_iter()
                    .collect();
                return CenterUpdate {
                    turn: CenterTurn::Handled,
                    ended,
                    reported,
                };
            }
            ConnectorEvent::Cancel {
                invocation_id,
                reason,
            } => {
                if self.table.cancel(&invocation_id)
                    && let Some(live) = self.live.remove(invocation_id.as_str())
                {
                    let reason = match reason.as_str() {
                        "credential revoked" | "permission revoked" => EndReason::Revoked,
                        "deadline exceeded" => EndReason::Deadline,
                        "caller disconnected" => EndReason::Disconnected,
                        _ => EndReason::Cancelled,
                    };
                    ended.push(CenterEnded {
                        call: live.reference,
                        reason,
                    });
                    self.emit(InvocationEvent::Cancelled {
                        invocation_id: invocation_id.clone(),
                    });
                    self.table.forget(&invocation_id);
                }
            }
            ConnectorEvent::Disconnected { reason } => {
                ended.extend(self.retire_generation(EndReason::Disconnected));
                self.status = CenterStatus::Disconnected(reason);
            }
            ConnectorEvent::Stopped { reason } => {
                ended.extend(self.retire_generation(EndReason::Stopped));
                self.admitting = false;
                self.status = CenterStatus::Stopped(reason);
            }
        }
        CenterUpdate {
            turn: CenterTurn::Handled,
            ended,
            reported: Vec::new(),
        }
    }

    /// Answer `call` with `outcome`: [`Self::complete`] on `Ok`,
    /// [`Self::fail`] on `Err`.
    pub fn answer(&mut self, call: CenterCall, outcome: CenterOutcome) {
        match outcome {
            Ok(result) => self.complete(call, result),
            Err(failure) => self.fail(call, failure.code, failure.message),
        }
    }

    /// Answer `call` with its result.
    ///
    /// A result after cancellation, disconnect, or deadline is suppressed. If
    /// an external operation already committed, use [`Self::complete_with_receipt`]
    /// and record the receipt locally with the committed outcome. The version-2
    /// broker protocol has no late-commit audit event and cannot replace an
    /// earlier terminal result. Never describe an ended call as proof that an
    /// already-submitted side effect was rolled back.
    pub fn complete(&mut self, call: CenterCall, result: Value) {
        let _ = self.complete_with_receipt(call, result);
    }

    /// Complete a call and return an identity-bearing receipt for local auditing.
    ///
    /// After an external commit, record its actual outcome beside this receipt,
    /// including `Ended`, `DeadlineExceeded`, or `NotQueued`. These dispositions
    /// do not mean the operation was rolled back, and the receipt does not
    /// replace a previous terminal result. To tell the broker that an ended
    /// call's effect committed anyway, pass `receipt.call` to
    /// [`Self::report_commit`].
    pub fn complete_with_receipt(&mut self, call: CenterCall, result: Value) -> CompletionReceipt {
        let reference = call.reference();
        let receipt = |disposition| CompletionReceipt {
            call: reference,
            disposition,
        };
        let Some(live) = self.take_live(&call) else {
            return receipt(CompletionDisposition::Ended);
        };
        let invocation_id = call.invocation_id;
        if live
            .deadline
            .is_some_and(|deadline| SystemTime::now() >= deadline)
        {
            self.terminal(InvocationEvent::Failed {
                invocation_id,
                code: "deadline_exceeded".into(),
                message: "the call's deadline passed before it finished".into(),
            });
            return receipt(CompletionDisposition::DeadlineExceeded);
        }
        let size = serialized_len(&result);
        if size > live.max_result_bytes as usize {
            self.terminal(InvocationEvent::Failed {
                invocation_id,
                code: "result_too_large".into(),
                message: format!(
                    "the result is {size} bytes; the function declares at most {}",
                    live.max_result_bytes
                ),
            });
            return receipt(CompletionDisposition::ResultTooLarge);
        }
        if let Some(key) = live.replay_key {
            if self.replay.len() == REPLAY_CAPACITY {
                self.replay.pop_front();
            }
            self.replay.push_back((key, result.clone()));
        }
        let queued = self.terminal(InvocationEvent::Completed {
            invocation_id,
            result,
        });
        receipt(if queued {
            CompletionDisposition::Queued
        } else {
            CompletionDisposition::NotQueued
        })
    }

    /// Report to the broker that `call`'s side effect committed, including a
    /// call that ended (cancelled, past its deadline) after the effect was made.
    ///
    /// Only a side-effect function's call qualifies, and only on the
    /// version-3 broker connection that dispatched it. The answer comes back
    /// once in [`CenterUpdate::reported`]; a durable disposition means the
    /// broker stored the report, never that the caller received a result, and
    /// the call's original outcome stays as it was. Retry a
    /// `storage_unavailable` answer the same way.
    ///
    /// # Errors
    /// [`CommitReportError`] says why nothing was queued. An offline host
    /// answers [`CommitReportError::Unavailable`].
    pub fn report_commit(&mut self, call: &CallRef) -> Result<(), CommitReportError> {
        let connector = self
            .connector
            .as_ref()
            .ok_or(CommitReportError::Unavailable)?;
        connector.report_commit(call.invocation_id())?;
        self.reports
            .insert(call.invocation_id().as_str().to_owned(), call.clone());
        Ok(())
    }

    /// Answer `call` with a failure. `code` is a short machine-readable token.
    pub fn fail(&mut self, call: CenterCall, code: &str, message: impl Into<String>) {
        if self.take_live(&call).is_none() {
            return;
        }
        self.terminal(InvocationEvent::Failed {
            invocation_id: call.invocation_id,
            code: code.into(),
            message: message.into(),
        });
    }

    /// Refuse every later call; the first step of final shutdown.
    pub fn stop_admission(&mut self) {
        self.admitting = false;
    }

    /// Stop admission, cancel what is still live and join the connector.
    pub fn shutdown(mut self) {
        self.stop_admission();
        let live: Vec<Uuid> = self
            .live
            .drain()
            .map(|(_, live)| live.invocation_id)
            .collect();
        for invocation_id in live {
            self.table.cancel(&invocation_id);
            self.terminal(InvocationEvent::Cancelled { invocation_id });
        }
        if let Some(connector) = self.connector.take() {
            connector.shutdown();
        }
    }

    fn admit(&mut self, invocation: Invocation) -> CenterTurn<C> {
        let invocation_id = invocation.invocation_id.clone();
        if self.table.open(&invocation_id).is_err() {
            // A replayed dispatch of a live id: the first one is answered.
            return CenterTurn::Handled;
        }
        if !self.admitting {
            return self.refuse(invocation_id, "unavailable", "the app is shutting down");
        }
        let deadline = (invocation.deadline_unix_ms != 0)
            .then(|| UNIX_EPOCH + Duration::from_millis(invocation.deadline_unix_ms));
        if deadline.is_some_and(|deadline| SystemTime::now() >= deadline) {
            return self.refuse(
                invocation_id,
                "deadline_exceeded",
                "the call's deadline passed before it was admitted",
            );
        }
        // Keys are the caller's own: another agent's key never reaches this
        // caller's results, and a key reused with other arguments is refused
        // rather than answered with a result for a different request.
        let replay_key = invocation.idempotency_key.clone().map(|key| Replay {
            caller: invocation.caller.credential_id.clone(),
            function: invocation.function_id.clone(),
            key,
            arguments: invocation.arguments.clone(),
        });
        let stored = replay_key.as_ref().and_then(|replay| {
            self.replay
                .iter()
                .find(|(stored, _)| stored.same_request(replay))
                .map(|(stored, result)| (stored.arguments == replay.arguments, result.clone()))
        });
        if let Some((false, _)) = stored {
            return self.refuse(
                invocation_id,
                "idempotency_conflict",
                "the idempotency key was used with other arguments",
            );
        }
        if let Some((true, result)) = stored {
            self.emit(InvocationEvent::Accepted {
                invocation_id: invocation_id.clone(),
            });
            self.terminal(InvocationEvent::Completed {
                invocation_id,
                result,
            });
            return CenterTurn::Handled;
        }
        let message = match self.registry.dispatch(&invocation) {
            Ok(message) => message,
            Err(DispatchError::UnknownFunction { .. }) => {
                return self.refuse(invocation_id, "unknown_function", "no such function");
            }
            Err(error) => {
                return self.refuse(invocation_id, "invalid_arguments", error.to_string());
            }
        };
        let max_result_bytes = self
            .registry
            .descriptor(&invocation.function_id)
            .map_or(crate::center::protocol::DEFAULT_RESULT_BYTES, |d| {
                d.max_result_bytes
            });
        self.emit(InvocationEvent::Accepted {
            invocation_id: invocation_id.clone(),
        });
        let reference = CallRef(Arc::new(invocation_id.clone()));
        self.live.insert(
            invocation_id.as_str().to_owned(),
            Live {
                reference: reference.clone(),
                invocation_id: invocation_id.clone(),
                generation: self.generation,
                deadline,
                max_result_bytes,
                replay_key,
            },
        );
        CenterTurn::Call(
            CenterCall {
                invocation_id,
                function_id: invocation.function_id,
                generation: self.generation,
                reference,
            },
            message,
        )
    }

    fn refuse(
        &mut self,
        invocation_id: Uuid,
        code: &str,
        message: impl Into<String>,
    ) -> CenterTurn<C> {
        self.terminal(InvocationEvent::Failed {
            invocation_id,
            code: code.into(),
            message: message.into(),
        });
        CenterTurn::Handled
    }

    fn matching_live(&self, call: &CenterCall) -> Option<&Live> {
        self.live
            .get(call.invocation_id.as_str())
            .filter(|live| live.generation == call.generation && live.reference == call.reference)
    }

    fn take_live(&mut self, call: &CenterCall) -> Option<Live> {
        self.matching_live(call)?;
        self.live.remove(call.invocation_id.as_str())
    }

    fn retire_generation(&mut self, reason: EndReason) -> Vec<CenterEnded> {
        self.generation += 1;
        self.table.cancel_all_disconnected();
        self.table = InvocationTable::new();
        // Ended in invocation-id order, not hash order, so the app sees the
        // same sequence every run.
        let mut live: Vec<_> = self.live.drain().collect();
        live.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
        live.into_iter()
            .map(|(_, live)| CenterEnded {
                call: live.reference,
                reason,
            })
            .collect()
    }

    fn terminal(&mut self, event: InvocationEvent) -> bool {
        let invocation_id = event.invocation_id().clone();
        let queued = self.emit(event);
        self.table.forget(&invocation_id);
        queued
    }

    fn emit(&mut self, event: InvocationEvent) -> bool {
        match self.table.record(&event) {
            Ok(EmitDecision::Send) => {}
            Ok(EmitDecision::Suppress) | Err(_) => return false,
        }
        if self.connector.is_none() {
            self.sent.push(event);
            return true;
        }
        // Order is kept: nothing overtakes events already waiting.
        self.flush_outbox();
        if self.outbox.is_empty() {
            match self.connector.as_ref().map(|c| c.send_event(event.clone())) {
                Some(Ok(())) => return true,
                Some(Err(SendError::Stopped)) | None => return false,
                Some(Err(SendError::QueueFull)) => {}
            }
        }
        if self.outbox.len() >= OUTBOX_CAPACITY {
            self.outbox_overflowed = true;
            return false;
        }
        self.outbox.push_back(event);
        true
    }

    /// Sends waiting events while the connector has room.
    fn flush_outbox(&mut self) {
        let Some(connector) = self.connector.as_ref() else {
            return;
        };
        while let Some(event) = self.outbox.front() {
            match connector.send_event(event.clone()) {
                Ok(()) => {
                    self.outbox.pop_front();
                }
                Err(SendError::QueueFull) => return,
                Err(SendError::Stopped) => {
                    self.outbox.clear();
                    return;
                }
            }
        }
    }
}

/// The length `value` serializes to, counted without building it.
fn serialized_len(value: &Value) -> usize {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(buf.len());
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).map_or(usize::MAX, |()| counter.0)
}

#[cfg(test)]
#[path = "app_host_tests.rs"]
mod tests;
