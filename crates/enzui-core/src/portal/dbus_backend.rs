//! D-Bus portal backend with cancellable live request objects.
//!
//! Readiness uses bounded D-Bus activation and follows portal-owner changes. Response subscriptions are established before method calls, and accepted requests reserve queue capacity for their eventual close.

use std::collections::{HashMap, HashSet};
use std::os::fd::AsFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use tokio::sync::mpsc;

use ashpd::zbus;
use ashpd::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::activation::ActivationContext;
use crate::app_data::AppId;
use crate::portal::backend::{
    BackendError, BackendReply, MAX_REPLY_BODY_BYTES, PortalBackend, PortalOperation, RequestId,
};
use crate::portal::chooser::ChooserOptions;
use crate::portal::notification::{
    DesktopNotification, MAX_NOTIFICATION_ACTION_PARAMETERS, MAX_NOTIFICATION_ACTION_VALUE_BYTES,
    MAX_NOTIFICATION_ACTION_WIRE_BYTES, MAX_NOTIFICATION_ID_BYTES,
    NOTIFICATION_ACTION_QUEUE_CAPACITY, NotificationActionTarget, NotificationBackend,
    NotificationBackendAction, NotificationBackendCommand, NotificationBackendError,
    NotificationBackendEvent, NotificationBackendOutcome, NotificationBackendStats,
    NotificationDisplayHint, NotificationOperationId, NotificationPriority,
    UnsupportedNotificationFeature,
};
use crate::portal::open_uri::{
    OpenUri, OpenUriBackend, OpenUriReply, RetainedDirectory, RetainedFile,
};
use crate::portal::parent::ParentHandle;
use crate::portal::print::{PrintReceipt, PrintRequest, PrintToken};
use crate::portal::runtime::{RuntimeInitError, build_runtime};
use crate::portal::{CancelReason, PortalOutcome, PortalResponse};
use application_identity::ConnectionConfig;
pub use application_identity::PortalIdentity;

const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";

/// Classifies D-Bus call failures for operator-facing diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DbusFailure {
    /// The bus is reachable, but no process owns the portal service name.
    NoPortalImplementation,
    /// The bus refused the method call before any portal interaction.
    AccessDenied,
    /// A transport, decoding, interface, or unrecognized D-Bus failure.
    TransportFailure,
}

impl DbusFailure {
    const SERVICE_UNKNOWN: &'static str = "org.freedesktop.DBus.Error.ServiceUnknown";
    const NAME_HAS_NO_OWNER: &'static str = "org.freedesktop.DBus.Error.NameHasNoOwner";
    const ACCESS_DENIED: &'static str = "org.freedesktop.DBus.Error.AccessDenied";

    /// Classifies a D-Bus method-error name.
    ///
    /// Missing and unrecognized names map to [`Self::TransportFailure`].
    #[must_use]
    pub fn classify(name: Option<&str>) -> Self {
        match name {
            Some(Self::SERVICE_UNKNOWN | Self::NAME_HAS_NO_OWNER) => Self::NoPortalImplementation,
            Some(Self::ACCESS_DENIED) => Self::AccessDenied,
            _ => Self::TransportFailure,
        }
    }

    /// Returns an operator-facing explanation of this failure class.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::NoPortalImplementation => {
                "no portal implementation (install/enable xdg-desktop-portal)"
            }
            Self::AccessDenied => "portal refused the caller (access denied at the bus)",
            Self::TransportFailure => "transport failure (broken connection or malformed reply)",
        }
    }
}
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const FILE_CHOOSER_IFACE: &str = "org.freedesktop.portal.FileChooser";
const PRINT_IFACE: &str = "org.freedesktop.portal.Print";
const OPEN_URI_IFACE: &str = "org.freedesktop.portal.OpenURI";
const NOTIFICATION_IFACE: &str = "org.freedesktop.portal.Notification";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";
const ACTIVATION_ATTEMPT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const PORTAL_READINESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
const ACTIVATION_RETRY_DELAYS: [std::time::Duration; 3] = [
    std::time::Duration::ZERO,
    std::time::Duration::from_millis(100),
    std::time::Duration::from_millis(250),
];

// Includes one reserved Close slot for every admitted request.
const COMMAND_QUEUE_CAPACITY: usize = 64;

// Maximum lifetime allowed for the worker after startup publication.
const SHUTDOWN_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

const PER_CLOSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const NOTIFICATION_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

// Wall-clock ceiling the owner-side watchdog spends waiting for the runtime,
// connection, and activation-aware readiness to settle. Expiry publishes
// `unavailable_reason` into `SharedState`, which the classifier turns into a
// terminal `Availability::Unavailable`. It is never spent on the caller thread.
const OWNER_STARTUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

enum ActivationResult {
    Owned(String),
    Unavailable(String),
}

#[derive(Clone, Copy)]
struct ReadinessConfig {
    total_timeout: std::time::Duration,
    owner_query_delay: std::time::Duration,
}

impl Default for ReadinessConfig {
    fn default() -> Self {
        Self {
            total_timeout: PORTAL_READINESS_TIMEOUT,
            owner_query_delay: std::time::Duration::ZERO,
        }
    }
}

type OwnerChangeStream = zbus::proxy::SignalStream<'static>;

struct OpenUriActivation {
    token: String,
    expires_at: std::time::Instant,
}

enum BackendCommand {
    Start {
        id: RequestId,
        generation: u64,
        portal_owner: String,
        operation: PortalOperation,
        request_path: OwnedObjectPath,
        options: ChooserOptions,
        // Empty means an explicitly unparented portal request.
        parent: ParentHandle,
    },
    StartOpenUri {
        id: RequestId,
        generation: u64,
        portal_owner: String,
        request_path: OwnedObjectPath,
        uri: OpenUri,
        parent: ParentHandle,
        activation: Option<OpenUriActivation>,
    },
    StartOpenFile {
        id: RequestId,
        generation: u64,
        portal_owner: String,
        request_path: OwnedObjectPath,
        /// `OpenFile` (a file, or a directory to open itself), or
        /// `OpenDirectory` to show a file in its folder.
        member: &'static str,
        file: Arc<std::fs::File>,
        parent: ParentHandle,
        activation: Option<OpenUriActivation>,
    },
    StartPrint {
        id: RequestId,
        generation: u64,
        portal_owner: String,
        prepare_path: OwnedObjectPath,
        print_path: OwnedObjectPath,
        request: PrintRequest,
    },
    Notification {
        id: NotificationOperationId,
        generation: u64,
        portal_owner: String,
        command: NotificationBackendCommand,
    },
    Close {
        id: RequestId,
    },
}

#[cfg(test)]
#[derive(Default)]
pub(super) struct DrainLog {
    pub wire_attempts: Vec<RequestId>,
    pub receipts_only: Vec<RequestId>,
    pub worker_exited: bool,
    pub response_dropped: Vec<(u64, std::time::Instant)>,
    pub delayed_inserts: Vec<RequestId>,
}

#[derive(Clone, Copy)]
enum ActiveRequestKind {
    File,
    OpenUri,
    Print,
}

struct ActiveRequest {
    request_path: OwnedObjectPath,
    generation: u64,
    portal_owner: String,
    kind: ActiveRequestKind,
    // Owned response task, aborted and joined before wire Close.
    task: tokio::task::JoinHandle<()>,
}

#[cfg(test)]
struct AbortDropProbe {
    id: RequestId,
    shared: Arc<Mutex<SharedState>>,
}

#[cfg(test)]
impl Drop for AbortDropProbe {
    fn drop(&mut self) {
        let delay = lock_shared(&self.shared).response_abort_drop_delay_ms;
        if delay == 0 {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(delay));
        let drain_log = Arc::clone(&lock_shared(&self.shared).drain_log);
        let (lock, cvar) = &*drain_log;
        let mut log = lock.lock().unwrap();
        log.response_dropped
            .push((self.id.get(), std::time::Instant::now()));
        cvar.notify_all();
    }
}

// Snapshot of connection and activation-aware owner readiness; `Initialising` remains retryable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Availability {
    Initialising,
    Available {
        unique_name: String,
        portal_owner: String,
        generation: u64,
    },
    Unavailable,
}

impl Availability {
    // Report available only when connection, unique name, and a live portal generation are present.
    pub(super) fn classify(
        available: bool,
        unique_name: Option<&str>,
        portal_present: Option<bool>,
        unavailable_reason: Option<&str>,
        portal_owner: Option<&str>,
        generation: u64,
    ) -> Self {
        if unavailable_reason.is_some() || portal_present == Some(false) {
            Self::Unavailable
        } else if available && portal_present == Some(true) {
            match (unique_name, portal_owner) {
                (Some(unique_name), Some(portal_owner)) => Self::Available {
                    unique_name: unique_name.to_owned(),
                    portal_owner: portal_owner.to_owned(),
                    generation,
                },
                _ => Self::Unavailable,
            }
        } else {
            Self::Initialising
        }
    }
}

#[derive(Default)]
struct SharedState {
    replies: HashMap<RequestId, BackendReply>,
    active: HashMap<RequestId, ActiveRequest>,
    open_uri_replies: HashMap<RequestId, OpenUriReply>,
    print_replies: HashMap<RequestId, PortalOutcome<PrintReceipt>>,
    notification_events: std::collections::VecDeque<NotificationBackendEvent>,
    notification_dropped_actions: u64,
    notification_rejected_actions: u64,
    notification_action_stream_ready: Option<bool>,
    // Notify this signal whenever a reply, print result, or availability outcome becomes observable.
    completion: crate::portal::completion::CompletionSignal,
    available: bool,
    unavailable_reason: Option<String>,
    unique_name: Option<String>,
    // `None` means activation-aware readiness is still in flight.
    portal_present: Option<bool>,
    portal_owner: Option<String>,
    portal_generation: u64,
    identity_owners: HashMap<String, bool>,
    #[cfg(test)]
    active_insert_delay_ms: u64,
    #[cfg(test)]
    response_abort_drop_delay_ms: u64,
    // Retain the actionable class and raw D-Bus name of the latest call failure.
    last_failure: Option<(DbusFailure, Option<String>)>,
    #[cfg(test)]
    drain_log: std::sync::Arc<(std::sync::Mutex<DrainLog>, std::sync::Condvar)>,
}

// On poison, discard pending state, abort active callbacks, and publish unavailability.
fn lock_shared(shared: &Mutex<SharedState>) -> std::sync::MutexGuard<'_, SharedState> {
    match shared.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            let mut state = poisoned.into_inner();
            #[allow(
                clippy::iter_over_hash_type,
                reason = "every entry gets the same treatment, so the order does not matter"
            )]
            for (_, active) in state.active.drain() {
                active.task.abort();
            }
            state.replies.clear();
            state.open_uri_replies.clear();
            state.print_replies.clear();
            state.notification_events.clear();
            state.notification_action_stream_ready = Some(false);
            state.available = false;
            state.portal_present = Some(false);
            state.unavailable_reason = Some("portal worker state lock was poisoned".into());
            state
        }
    }
}

/// A [`PortalBackend`] that talks to the real XDG Desktop Portal over D-Bus.
///
/// Available only with the `portal-dbus` feature.
pub struct DbusBackend {
    application_id: Option<AppId>,
    // Allocate handle tokens process-wide so backend instances cannot share request paths.
    token_counter: Arc<AtomicU64>,
    // Production instances share one allocator so request ids cannot collide across backends.
    id_counter: Arc<AtomicU64>,
    shared: Arc<Mutex<SharedState>>,
    // Synchronous methods use only nonblocking sends or pre-reserved permits.
    cmd_tx: mpsc::Sender<BackendCommand>,
    // Reserve each admitted request’s Close slot against later queue pressure.
    close_permits: HashMap<RequestId, mpsc::OwnedPermit<BackendCommand>>,
    // Retire ids after terminal poll or accepted Close; monotonic allocation prevents stale reuse.
    in_flight: HashSet<RequestId>,
    in_flight_open_uris: HashSet<RequestId>,
    in_flight_prints: HashSet<RequestId>,
    notification_in_flight: HashSet<NotificationOperationId>,
    closed_count: usize,
    probed: bool,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    // Sole owner of the runtime and worker; frontend drop never waits for a running owner.
    shutdown_owner: Option<std::thread::JoinHandle<()>>,
}

static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(0);

// Allocate request ids process-wide so stale handles cannot collide across backend instances.
static GLOBAL_ID_COUNTER: LazyLock<Arc<AtomicU64>> = LazyLock::new(|| Arc::new(AtomicU64::new(1)));

// Allocate monotonically without wrap; return `ResourceExhausted` at the `u64` limit.
fn allocate_request_id(counter: &AtomicU64) -> Result<RequestId, BackendError> {
    counter
        .try_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
            current.checked_add(1)
        })
        .map(RequestId::new)
        .map_err(|_| BackendError::ResourceExhausted)
}

impl DbusBackend {
    /// Creates a D-Bus portal backend on the ambient session bus.
    ///
    /// Returns once the owner and startup-watchdog threads have been spawned; runtime creation,
    /// D-Bus connection, bounded service activation, and owner subscription continue in the background.
    /// Requests return [`BackendError::Initialising`] until readiness settles.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeInitError`] only when the operating system refuses to
    /// spawn either thread. Every other startup failure (runtime creation,
    /// bus connection, activation, owner subscription) is reported through
    /// [`BackendError::Unavailable`] on later backend operations.
    pub fn new() -> Result<Self, RuntimeInitError> {
        Self::with_address(None)
    }

    /// Creates a D-Bus portal backend for an optional explicit bus address.
    ///
    /// `None` selects the ambient session bus. See [`Self::new`] for the
    /// nonblocking-construction contract and how deferred failures settle.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeInitError`] only when the owner or startup-watchdog thread
    /// cannot be spawned; other startup failures settle into [`BackendError::Unavailable`].
    pub fn with_address(address: Option<String>) -> Result<Self, RuntimeInitError> {
        Self::with_config(ConnectionConfig {
            address,
            application: None,
        })
    }

    /// Creates an application-bound portal connection on the session bus.
    ///
    /// Host applications register through the host Registry before any portal
    /// method. Sandboxed applications use the portal's existing identity.
    /// Exported notification actions require this constructor or its explicit-address variant.
    ///
    /// # Errors
    /// Returns [`RuntimeInitError`] if the owner threads cannot be created.
    /// Identity registration failures settle as unavailable backend operations.
    pub fn for_application(
        application_id: AppId,
        identity: PortalIdentity,
    ) -> Result<Self, RuntimeInitError> {
        Self::for_application_with_address(application_id, identity, None)
    }

    /// Creates an application-bound connection on an explicit bus, or the session bus for `None`.
    ///
    /// Use the same id for the notification client, installed desktop entry and
    /// `org.freedesktop.Application` service. See [`Self::for_application`].
    ///
    /// # Errors
    /// Returns [`RuntimeInitError`] if owner threads cannot be created;
    /// registration and connection failures settle asynchronously as unavailable.
    pub fn for_application_with_address(
        application_id: AppId,
        identity: PortalIdentity,
        address: Option<String>,
    ) -> Result<Self, RuntimeInitError> {
        Self::with_config(ConnectionConfig {
            address,
            application: Some((application_id, identity)),
        })
    }

    fn with_config(config: ConnectionConfig) -> Result<Self, RuntimeInitError> {
        let application_id = config.application.as_ref().map(|(id, _)| id.clone());
        let shared = Arc::new(Mutex::new(SharedState::default()));

        // Own the command channel here so the owner thread does not need to publish `cmd_tx`
        // back to the caller: `new()` returns without waiting for owner startup.
        let (cmd_tx, cmd_rx) = mpsc::channel(COMMAND_QUEUE_CAPACITY);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let shared_for_owner = Arc::clone(&shared);

        // Consume thread-local runtime failure injection before spawning the owner.
        #[cfg(test)]
        let injected_rt_fail = crate::portal::runtime::fail::take_injected();
        #[cfg(not(test))]
        let injected_rt_fail: Option<std::io::Error> = None;

        #[cfg(test)]
        let readiness_config = readiness_hook::take();
        #[cfg(not(test))]
        let readiness_config = ReadinessConfig::default();

        // Spawn the owner-side startup watchdog before the owner itself so an owner that panics
        // before publishing anything still settles as Unavailable within the ceiling.
        //
        // The watchdog holds a `Weak<Mutex<SharedState>>` so a backend dropped before the ceiling
        // fires does not keep `SharedState` alive for the remainder of the sleep — the Arc leak
        // check `arc_leak_check_exact_count_no_sleep` reads the strong count right after drain.
        let shared_for_watchdog = Arc::downgrade(&shared);
        if let Err(err) = std::thread::Builder::new()
            .name("portal-startup-watchdog".into())
            .spawn(move || {
                std::thread::sleep(OWNER_STARTUP_TIMEOUT);
                let Some(shared) = shared_for_watchdog.upgrade() else {
                    return;
                };
                let mut state = lock_shared(&shared);
                if state.portal_present.is_some() || state.unavailable_reason.is_some() {
                    return;
                }
                state.available = false;
                state.portal_present = Some(false);
                state.unavailable_reason = Some("portal owner startup exceeded ceiling".to_owned());
                let completion = state.completion.clone();
                drop(state);
                completion.notify();
            })
        {
            return Err(RuntimeInitError::new(err));
        }

        let shutdown_owner = match std::thread::Builder::new()
            .name("portal-shutdown-owner".into())
            .spawn(move || {
                Self::run_owner(
                    cmd_rx,
                    shared_for_owner,
                    config,
                    shutdown_rx,
                    readiness_config,
                    injected_rt_fail,
                );
            }) {
            Ok(handle) => handle,
            Err(err) => return Err(RuntimeInitError::new(err)),
        };

        Ok(Self {
            application_id,
            token_counter: {
                let base = TOKEN_COUNTER.fetch_add(1_000_000, Ordering::SeqCst);
                Arc::new(AtomicU64::new(base))
            },
            id_counter: Arc::clone(&GLOBAL_ID_COUNTER),
            shared,
            cmd_tx,
            close_permits: HashMap::new(),
            in_flight: HashSet::new(),
            in_flight_open_uris: HashSet::new(),
            in_flight_prints: HashSet::new(),
            notification_in_flight: HashSet::new(),
            closed_count: 0,
            probed: false,
            shutdown_tx,
            shutdown_owner: Some(shutdown_owner),
        })
    }

    // Derive the portal’s deterministic request path so Response subscription can precede the method call.
    fn request_path(unique_name: &str, handle_token: &str) -> OwnedObjectPath {
        let sanitized = unique_name.trim_start_matches(':').replace('.', "_");
        let path = format!("/org/freedesktop/portal/desktop/request/{sanitized}/{handle_token}");
        OwnedObjectPath::try_from(path).expect("computed request path is always valid")
    }

    // Snapshot availability without waiting; an unsettled probe remains retryable.
    fn probe_availability(&mut self) -> Availability {
        self.probed = true;
        let s = lock_shared(&self.shared);
        Availability::classify(
            s.available,
            s.unique_name.as_deref(),
            s.portal_present,
            s.unavailable_reason.as_deref(),
            s.portal_owner.as_deref(),
            s.portal_generation,
        )
    }

    // Reserve start and Close slots atomically; release the first if the second is unavailable.
    fn reserve_start_and_close(
        &self,
    ) -> Result<
        (
            mpsc::OwnedPermit<BackendCommand>,
            mpsc::OwnedPermit<BackendCommand>,
        ),
        BackendError,
    > {
        use mpsc::error::TrySendError;

        let start = self
            .cmd_tx
            .clone()
            .try_reserve_owned()
            .map_err(|error| match error {
                TrySendError::Full(_) => BackendError::ResourceExhausted,
                TrySendError::Closed(_) => BackendError::Unavailable,
            })?;
        let close = self
            .cmd_tx
            .clone()
            .try_reserve_owned()
            .map_err(|error| match error {
                TrySendError::Full(_) => BackendError::ResourceExhausted,
                TrySendError::Closed(_) => BackendError::Unavailable,
            })?;
        Ok((start, close))
    }

    // Own the runtime and worker on this thread for their full lifetimes.
    //
    // The caller owns `cmd_tx` and returns without waiting for this thread. Runtime creation,
    // bus connection, activation, and owner subscription may all fail; each failure publishes a durable
    // `unavailable_reason` into `SharedState` so `Availability::classify` reports Unavailable,
    // rather than being surfaced synchronously through the constructor.
    fn run_owner(
        cmd_rx: mpsc::Receiver<BackendCommand>,
        shared: Arc<Mutex<SharedState>>,
        config: ConnectionConfig,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
        readiness_config: ReadinessConfig,
        injected_rt_fail: Option<std::io::Error>,
    ) {
        if let Some(err) = injected_rt_fail {
            Self::publish_startup_failure(&shared, &format!("portal runtime init failed: {err}"));
            return;
        }

        let rt = match build_runtime() {
            Ok(rt) => rt,
            Err(err) => {
                Self::publish_startup_failure(
                    &shared,
                    &format!("portal runtime init failed: {err}"),
                );
                return;
            }
        };

        let mut owner_shutdown = shutdown_rx.clone();
        let worker = rt.spawn(Self::command_loop(
            cmd_rx,
            shared,
            config,
            shutdown_rx,
            readiness_config,
        ));

        rt.block_on(async move {
            use futures_util::future::{Either, select};

            let shutdown = async move {
                while !*owner_shutdown.borrow() {
                    if owner_shutdown.changed().await.is_err() {
                        break;
                    }
                }
            };
            futures_util::pin_mut!(shutdown);
            match select(worker, shutdown).await {
                Either::Left((_finished, _shutdown)) => {}
                Either::Right(((), worker)) => {
                    let _ = tokio::time::timeout(SHUTDOWN_DRAIN_TIMEOUT, worker).await;
                }
            }
        });
    }

    // Run Close calls concurrently with individual timeouts; the owner bounds the shutdown drain.
    async fn command_loop(
        mut cmd_rx: mpsc::Receiver<BackendCommand>,
        shared: Arc<Mutex<SharedState>>,
        config: ConnectionConfig,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
        readiness_config: ReadinessConfig,
    ) {
        // On connection failure, keep draining commands so accepted closes can retire local tasks.
        let mut owner_task = None;
        let mut notification_action_task = None;
        let setup = tokio::time::timeout(readiness_config.total_timeout, async {
            let connection = Self::connect(config.address.clone()).await?;
            let changes = Self::owner_change_stream(&connection).await?;
            let actions = Self::notification_action_stream(&connection).await.ok();
            let activation = Self::activate_portal(&connection, readiness_config).await;
            Ok::<_, zbus::Error>((connection, changes, actions, activation))
        })
        .await;
        let connection = match setup {
            Ok(Ok((connection, mut changes, actions, activation))) => {
                {
                    let mut state = lock_shared(&shared);
                    state.available = true;
                    state.unique_name = connection.unique_name().map(ToString::to_string);
                    state.notification_action_stream_ready = Some(actions.is_some());
                }
                match activation {
                    ActivationResult::Owned(owner) => {
                        Self::admit_application_owner(
                            &connection,
                            &shared,
                            owner,
                            config.application.as_ref(),
                        )
                        .await;
                    }
                    ActivationResult::Unavailable(reason) => {
                        let mut state = lock_shared(&shared);
                        state.portal_present = Some(false);
                        state.unavailable_reason = Some(reason);
                        let completion = state.completion.clone();
                        drop(state);
                        completion.notify();
                    }
                }
                let monitor_connection = connection.clone();
                let monitor_shared = Arc::clone(&shared);
                let monitor_application = config.application.clone();
                owner_task = Some(tokio::spawn(async move {
                    use futures_util::StreamExt;
                    while let Some(signal) = changes.next().await {
                        let Ok((name, old_owner, new_owner)) =
                            signal.body().deserialize::<(String, String, String)>()
                        else {
                            continue;
                        };
                        if name != PORTAL_DEST {
                            continue;
                        }
                        if !old_owner.is_empty() {
                            let replaced =
                                Self::publish_owner_loss(&monitor_shared, Some(&old_owner)).await;
                            if replaced && !new_owner.is_empty() {
                                Self::admit_application_owner(
                                    &monitor_connection,
                                    &monitor_shared,
                                    new_owner,
                                    monitor_application.as_ref(),
                                )
                                .await;
                                continue;
                            }
                            if !replaced {
                                continue;
                            }
                        } else if !new_owner.is_empty() {
                            let current = lock_shared(&monitor_shared).portal_owner.clone();
                            if Self::owner_change_matches(
                                current.as_deref(),
                                &old_owner,
                                &new_owner,
                            ) {
                                Self::admit_application_owner(
                                    &monitor_connection,
                                    &monitor_shared,
                                    new_owner,
                                    monitor_application.as_ref(),
                                )
                                .await;
                            }
                            continue;
                        }
                        if new_owner.is_empty() {
                            match Self::activate_portal(&monitor_connection, readiness_config).await
                            {
                                ActivationResult::Owned(owner) => {
                                    Self::admit_application_owner(
                                        &monitor_connection,
                                        &monitor_shared,
                                        owner,
                                        monitor_application.as_ref(),
                                    )
                                    .await;
                                }
                                ActivationResult::Unavailable(reason) => {
                                    let mut state = lock_shared(&monitor_shared);
                                    if state.portal_owner.is_none() {
                                        state.portal_present = Some(false);
                                        state.unavailable_reason = Some(reason);
                                    }
                                    let completion = state.completion.clone();
                                    drop(state);
                                    completion.notify();
                                }
                            }
                        }
                    }
                    Self::publish_owner_loss(&monitor_shared, None).await;
                    let mut state = lock_shared(&monitor_shared);
                    state.available = false;
                    state.portal_present = Some(false);
                    state.unavailable_reason = Some("portal owner monitor disconnected".to_owned());
                    let completion = state.completion.clone();
                    drop(state);
                    completion.notify();
                }));
                if let Some(mut actions) = actions {
                    let action_shared = Arc::clone(&shared);
                    notification_action_task = Some(tokio::spawn(async move {
                        use futures_util::StreamExt;
                        while let Some(signal) = actions.next().await {
                            if let Ok(signal) = signal {
                                Self::publish_notification_action(&action_shared, signal);
                            }
                        }
                        let mut state = lock_shared(&action_shared);
                        state.notification_action_stream_ready = Some(false);
                        state.notification_events.retain(|event| {
                            !matches!(event, NotificationBackendEvent::ActionDeliveryUnavailable)
                        });
                        state
                            .notification_events
                            .push_back(NotificationBackendEvent::ActionDeliveryUnavailable);
                        let completion = state.completion.clone();
                        drop(state);
                        completion.notify();
                    }));
                }
                Some(connection)
            }
            Ok(Err(error)) => {
                let mut state = lock_shared(&shared);
                state.available = false;
                state.portal_present = Some(false);
                state.unavailable_reason = Some(format!("portal readiness failed: {error}"));
                let completion = state.completion.clone();
                drop(state);
                completion.notify();
                None
            }
            Err(_) => {
                let mut state = lock_shared(&shared);
                state.available = false;
                state.portal_present = Some(false);
                state.unavailable_reason =
                    Some("portal readiness exceeded its total deadline".to_owned());
                let completion = state.completion.clone();
                drop(state);
                completion.notify();
                None
            }
        };

        let mut close_tasks = tokio::task::JoinSet::new();
        let mut shutting_down = false;

        loop {
            let command = if shutting_down {
                cmd_rx.recv().await
            } else {
                use futures_util::future::{Either, select};

                let (command, saw_shutdown) = {
                    let command = cmd_rx.recv();
                    let shutdown = shutdown_rx.changed();
                    futures_util::pin_mut!(command, shutdown);
                    match select(command, shutdown).await {
                        Either::Left((command, _)) => (command, false),
                        Either::Right((_shutdown, _)) => (None, true),
                    }
                };
                if saw_shutdown {
                    shutting_down = true;
                    cmd_rx.close();
                    continue;
                }
                command
            };
            let Some(cmd) = command else {
                break;
            };
            match cmd {
                BackendCommand::Start {
                    id,
                    generation,
                    portal_owner,
                    operation,
                    request_path,
                    options,
                    parent,
                } => {
                    let generation_is_live = {
                        let state = lock_shared(&shared);
                        state.portal_owner.is_some() && state.portal_generation == generation
                    };
                    if !generation_is_live {
                        let mut state = lock_shared(&shared);
                        state.replies.insert(id, Self::failed_reply());
                        let completion = state.completion.clone();
                        drop(state);
                        completion.notify();
                        continue;
                    }
                    let Some(ref connection) = connection else {
                        let mut s = lock_shared(&shared);
                        s.replies.insert(id, Self::failed_reply());
                        let completion = s.completion.clone();
                        drop(s);
                        completion.notify();
                        continue;
                    };
                    let conn = connection.clone();
                    let shared_task = Arc::clone(&shared);
                    let shared_diag = Arc::clone(&shared);
                    let path_for_task = request_path.clone();
                    let owner_for_task = portal_owner.clone();
                    // Hold the response task until its active record exists, preventing an immediate reply from being discarded.
                    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
                    #[cfg(test)]
                    let probe_shared = Arc::clone(&shared_task);
                    let task = tokio::spawn(async move {
                        #[cfg(test)]
                        let _abort_probe = AbortDropProbe {
                            id,
                            shared: probe_shared,
                        };
                        let _ = ready_rx.await;
                        let reply = Self::run_file_chooser(
                            &conn,
                            &owner_for_task,
                            operation,
                            &path_for_task,
                            &shared_diag,
                            &options,
                            &parent,
                        )
                        .await;
                        let mut s = lock_shared(&shared_task);
                        // Park a reply only while its request remains active.
                        if s.active
                            .get(&id)
                            .is_some_and(|active| active.generation == generation)
                        {
                            s.replies.insert(id, reply);
                            s.active.remove(&id);
                        }
                        // Notify after every response attempt, including discarded late replies.
                        let completion = s.completion.clone();
                        drop(s);
                        completion.notify();
                    });
                    #[cfg(test)]
                    {
                        let delay = lock_shared(&shared).active_insert_delay_ms;
                        if delay > 0 {
                            let drain_log = Arc::clone(&lock_shared(&shared).drain_log);
                            let (lock, cvar) = &*drain_log;
                            lock.lock().unwrap().delayed_inserts.push(id);
                            cvar.notify_all();
                            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                        }
                    }
                    let mut pending_task = Some(task);
                    let inserted = {
                        let mut state = lock_shared(&shared);
                        if state.portal_owner.is_some() && state.portal_generation == generation {
                            state.active.insert(
                                id,
                                ActiveRequest {
                                    request_path,
                                    generation,
                                    portal_owner,
                                    kind: ActiveRequestKind::File,
                                    task: pending_task.take().expect("pending task"),
                                },
                            );
                            true
                        } else {
                            false
                        }
                    };
                    if inserted {
                        let _ = ready_tx.send(());
                    } else {
                        let task = pending_task.take().expect("uninserted task");
                        task.abort();
                        let _ = task.await;
                        let mut state = lock_shared(&shared);
                        state.replies.insert(id, Self::failed_reply());
                        let completion = state.completion.clone();
                        drop(state);
                        completion.notify();
                    }
                }
                BackendCommand::StartOpenUri {
                    id,
                    generation,
                    portal_owner,
                    request_path,
                    uri,
                    parent,
                    activation,
                } => {
                    let generation_is_live = {
                        let state = lock_shared(&shared);
                        state.portal_owner.is_some() && state.portal_generation == generation
                    };
                    if !generation_is_live {
                        let mut state = lock_shared(&shared);
                        state.open_uri_replies.insert(id, OpenUriReply::Unavailable);
                        let completion = state.completion.clone();
                        drop(state);
                        completion.notify();
                        continue;
                    }
                    let Some(ref connection) = connection else {
                        let mut s = lock_shared(&shared);
                        s.open_uri_replies.insert(id, OpenUriReply::Failed);
                        let completion = s.completion.clone();
                        drop(s);
                        completion.notify();
                        continue;
                    };
                    let conn = connection.clone();
                    let shared_task = Arc::clone(&shared);
                    let shared_diag = Arc::clone(&shared);
                    let path_for_task = request_path.clone();
                    let owner_for_task = portal_owner.clone();
                    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
                    let task = tokio::spawn(async move {
                        let _ = ready_rx.await;
                        let reply = Self::run_open_uri(
                            &conn,
                            &owner_for_task,
                            &path_for_task,
                            &shared_diag,
                            &uri,
                            &parent,
                            activation.as_ref(),
                        )
                        .await;
                        let mut state = lock_shared(&shared_task);
                        if state
                            .active
                            .get(&id)
                            .is_some_and(|active| active.generation == generation)
                        {
                            state.open_uri_replies.insert(id, reply);
                            state.active.remove(&id);
                        }
                        let completion = state.completion.clone();
                        drop(state);
                        completion.notify();
                    });
                    let mut pending_task = Some(task);
                    let inserted = {
                        let mut state = lock_shared(&shared);
                        if state.portal_owner.is_some() && state.portal_generation == generation {
                            state.active.insert(
                                id,
                                ActiveRequest {
                                    request_path,
                                    generation,
                                    portal_owner,
                                    kind: ActiveRequestKind::OpenUri,
                                    task: pending_task.take().expect("pending task"),
                                },
                            );
                            true
                        } else {
                            false
                        }
                    };
                    if inserted {
                        let _ = ready_tx.send(());
                    } else {
                        let task = pending_task.take().expect("uninserted task");
                        task.abort();
                        let _ = task.await;
                        let mut state = lock_shared(&shared);
                        state.open_uri_replies.insert(id, OpenUriReply::Unavailable);
                        let completion = state.completion.clone();
                        drop(state);
                        completion.notify();
                    }
                }
                BackendCommand::StartOpenFile {
                    id,
                    generation,
                    portal_owner,
                    request_path,
                    member,
                    file,
                    parent,
                    activation,
                } => {
                    let generation_is_live = {
                        let state = lock_shared(&shared);
                        state.portal_owner.is_some() && state.portal_generation == generation
                    };
                    if !generation_is_live {
                        let mut state = lock_shared(&shared);
                        state.open_uri_replies.insert(id, OpenUriReply::Unavailable);
                        let completion = state.completion.clone();
                        drop(state);
                        completion.notify();
                        continue;
                    }
                    let Some(ref connection) = connection else {
                        let mut s = lock_shared(&shared);
                        s.open_uri_replies.insert(id, OpenUriReply::Failed);
                        let completion = s.completion.clone();
                        drop(s);
                        completion.notify();
                        continue;
                    };
                    let conn = connection.clone();
                    let shared_task = Arc::clone(&shared);
                    let shared_diag = Arc::clone(&shared);
                    let path_for_task = request_path.clone();
                    let owner_for_task = portal_owner.clone();
                    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
                    let task = tokio::spawn(async move {
                        let _ = ready_rx.await;
                        // The task owns the retained descriptor for the whole call, so
                        // the exact fd survives until the portal has consumed it.
                        let reply = Self::run_open_file(
                            &conn,
                            &owner_for_task,
                            &path_for_task,
                            &shared_diag,
                            (member, &*file),
                            &parent,
                            activation.as_ref(),
                        )
                        .await;
                        let mut state = lock_shared(&shared_task);
                        if state
                            .active
                            .get(&id)
                            .is_some_and(|active| active.generation == generation)
                        {
                            state.open_uri_replies.insert(id, reply);
                            state.active.remove(&id);
                        }
                        let completion = state.completion.clone();
                        drop(state);
                        completion.notify();
                    });
                    let mut pending_task = Some(task);
                    let inserted = {
                        let mut state = lock_shared(&shared);
                        if state.portal_owner.is_some() && state.portal_generation == generation {
                            state.active.insert(
                                id,
                                ActiveRequest {
                                    request_path,
                                    generation,
                                    portal_owner,
                                    kind: ActiveRequestKind::OpenUri,
                                    task: pending_task.take().expect("pending task"),
                                },
                            );
                            true
                        } else {
                            false
                        }
                    };
                    if inserted {
                        let _ = ready_tx.send(());
                    } else {
                        let task = pending_task.take().expect("uninserted task");
                        task.abort();
                        let _ = task.await;
                        let mut state = lock_shared(&shared);
                        state.open_uri_replies.insert(id, OpenUriReply::Unavailable);
                        let completion = state.completion.clone();
                        drop(state);
                        completion.notify();
                    }
                }
                BackendCommand::StartPrint {
                    id,
                    generation,
                    portal_owner,
                    prepare_path,
                    print_path,
                    request,
                } => {
                    let generation_is_live = {
                        let state = lock_shared(&shared);
                        state.portal_owner.is_some() && state.portal_generation == generation
                    };
                    if !generation_is_live {
                        let mut state = lock_shared(&shared);
                        state.print_replies.insert(id, PortalOutcome::Unavailable);
                        let completion = state.completion.clone();
                        drop(state);
                        completion.notify();
                        continue;
                    }
                    let Some(ref connection) = connection else {
                        let mut s = lock_shared(&shared);
                        s.print_replies.insert(id, PortalOutcome::Failed);
                        let completion = s.completion.clone();
                        drop(s);
                        completion.notify();
                        continue;
                    };
                    let conn = connection.clone();
                    let shared_task = Arc::clone(&shared);
                    let owner_for_task = portal_owner.clone();
                    let initial_path = if request.token.is_some() {
                        print_path.clone()
                    } else {
                        prepare_path.clone()
                    };
                    let prepare_for_task = prepare_path.clone();
                    // Hold the print task until its active record exists.
                    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
                    let task = tokio::spawn(async move {
                        let _ = ready_rx.await;
                        let outcome = Self::run_print(
                            &conn,
                            &owner_for_task,
                            id,
                            &prepare_for_task,
                            &print_path,
                            &shared_task,
                            request,
                        )
                        .await;
                        let mut s = lock_shared(&shared_task);
                        if s.active
                            .get(&id)
                            .is_some_and(|active| active.generation == generation)
                        {
                            s.print_replies.insert(id, outcome);
                            s.active.remove(&id);
                        }
                        let completion = s.completion.clone();
                        drop(s);
                        completion.notify();
                    });
                    let mut pending_task = Some(task);
                    let inserted = {
                        let mut state = lock_shared(&shared);
                        if state.portal_owner.is_some() && state.portal_generation == generation {
                            state.active.insert(
                                id,
                                ActiveRequest {
                                    request_path: initial_path,
                                    generation,
                                    portal_owner,
                                    kind: ActiveRequestKind::Print,
                                    task: pending_task.take().expect("pending task"),
                                },
                            );
                            true
                        } else {
                            false
                        }
                    };
                    if inserted {
                        let _ = ready_tx.send(());
                    } else {
                        let task = pending_task.take().expect("uninserted task");
                        task.abort();
                        let _ = task.await;
                        let mut state = lock_shared(&shared);
                        state.print_replies.insert(id, PortalOutcome::Unavailable);
                        let completion = state.completion.clone();
                        drop(state);
                        completion.notify();
                    }
                }
                BackendCommand::Notification {
                    id,
                    generation,
                    portal_owner,
                    command,
                } => {
                    let generation_is_live = {
                        let state = lock_shared(&shared);
                        state.portal_owner.as_deref() == Some(portal_owner.as_str())
                            && state.portal_generation == generation
                    };
                    let outcome = if !generation_is_live {
                        NotificationBackendOutcome::Unavailable
                    } else if let Some(ref connection) = connection {
                        match tokio::time::timeout(
                            NOTIFICATION_CALL_TIMEOUT,
                            Self::run_notification_command(
                                connection,
                                &portal_owner,
                                &command,
                                config.application.is_some(),
                            ),
                        )
                        .await
                        {
                            Ok(outcome) => outcome,
                            Err(_) => NotificationBackendOutcome::Failed,
                        }
                    } else {
                        NotificationBackendOutcome::Unavailable
                    };
                    let mut state = lock_shared(&shared);
                    let still_current = state.portal_owner.as_deref()
                        == Some(portal_owner.as_str())
                        && state.portal_generation == generation;
                    state
                        .notification_events
                        .push_back(NotificationBackendEvent::Operation {
                            operation: id,
                            owner_generation: generation,
                            outcome: if still_current {
                                outcome
                            } else {
                                NotificationBackendOutcome::Unavailable
                            },
                        });
                    let completion = state.completion.clone();
                    drop(state);
                    completion.notify();
                }
                BackendCommand::Close { id } => {
                    // Abort and join the response task before issuing wire Close.
                    let active = {
                        let mut s = lock_shared(&shared);
                        s.active.remove(&id)
                    };
                    if let Some(active) = active {
                        active.task.abort();
                        let _ = active.task.await;
                        // Spawn each wire Close independently so one stalled call cannot block the remaining drain.
                        if let Some(ref connection) = connection {
                            let conn = connection.clone();
                            let path = active.request_path.clone();
                            #[cfg(test)]
                            let shared_log = Arc::clone(&shared);
                            close_tasks.spawn(async move {
                                let _ = tokio::time::timeout(
                                    PER_CLOSE_TIMEOUT,
                                    Self::close_request(&conn, &active.portal_owner, &path),
                                )
                                .await;
                                #[cfg(test)]
                                {
                                    let drain_log = Arc::clone(&lock_shared(&shared_log).drain_log);
                                    let (lock, cvar) = &*drain_log;
                                    let mut log = lock.lock().unwrap();
                                    log.wire_attempts.push(id);
                                    cvar.notify_all();
                                }
                            });
                        } else {
                            #[cfg(test)]
                            {
                                let drain_log = Arc::clone(&lock_shared(&shared).drain_log);
                                let (lock, cvar) = &*drain_log;
                                let mut log = lock.lock().unwrap();
                                log.receipts_only.push(id);
                                cvar.notify_all();
                            }
                        }
                    } else {
                        #[cfg(test)]
                        {
                            let drain_log = Arc::clone(&lock_shared(&shared).drain_log);
                            let (lock, cvar) = &*drain_log;
                            let mut log = lock.lock().unwrap();
                            log.receipts_only.push(id);
                            cvar.notify_all();
                        }
                    }
                }
            }
            while close_tasks.try_join_next().is_some() {}
        }

        if let Some(task) = owner_task {
            task.abort();
            let _ = task.await;
        }
        if let Some(task) = notification_action_task {
            task.abort();
            let _ = task.await;
        }

        // Join remaining Close tasks; the owner's worker deadline bounds this wait.
        while close_tasks.join_next().await.is_some() {}

        // Abort and join all remaining response tasks before the worker exits.
        {
            let remaining: Vec<_> = {
                let mut s = lock_shared(&shared);
                s.active.drain().map(|(_, ar)| ar.task).collect()
            };
            for task in remaining {
                task.abort();
                let _ = task.await;
            }
        }

        #[cfg(test)]
        {
            let drain_log = Arc::clone(&lock_shared(&shared).drain_log);
            let (lock, cvar) = &*drain_log;
            let mut log = lock.lock().unwrap();
            log.worker_exited = true;
            cvar.notify_all();
        }
    }

    pub(crate) async fn connect_pub(
        address: Option<String>,
    ) -> Result<zbus::Connection, zbus::Error> {
        Self::connect(address).await
    }

    // Publish a terminal startup failure so `Availability::classify` reports Unavailable.
    fn publish_startup_failure(shared: &Arc<Mutex<SharedState>>, reason: &str) {
        let mut state = lock_shared(shared);
        state.available = false;
        state.portal_present = Some(false);
        state.unavailable_reason = Some(reason.to_owned());
        let completion = state.completion.clone();
        drop(state);
        completion.notify();
    }

    async fn owner_change_stream(
        connection: &zbus::Connection,
    ) -> Result<OwnerChangeStream, zbus::Error> {
        let proxy = zbus::Proxy::new(
            connection,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
        )
        .await?;
        proxy
            .receive_signal_with_args("NameOwnerChanged", &[(0, PORTAL_DEST)])
            .await
    }

    async fn portal_owner(
        connection: &zbus::Connection,
        config: ReadinessConfig,
    ) -> Result<String, zbus::Error> {
        if !config.owner_query_delay.is_zero() {
            tokio::time::sleep(config.owner_query_delay).await;
        }
        connection
            .call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetNameOwner",
                &(PORTAL_DEST,),
            )
            .await?
            .body()
            .deserialize::<String>()
    }

    async fn activate_portal(
        connection: &zbus::Connection,
        config: ReadinessConfig,
    ) -> ActivationResult {
        match tokio::time::timeout(
            config.total_timeout,
            Self::activation_exchange(connection, config),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => ActivationResult::Unavailable(
                "portal activation/readiness exceeded its total deadline".to_owned(),
            ),
        }
    }

    async fn activation_exchange(
        connection: &zbus::Connection,
        config: ReadinessConfig,
    ) -> ActivationResult {
        if let Ok(owner) = Self::portal_owner(connection, config).await {
            return ActivationResult::Owned(owner);
        }

        let mut last_error = None;
        for delay in ACTIVATION_RETRY_DELAYS {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
                if let Ok(owner) = Self::portal_owner(connection, config).await {
                    return ActivationResult::Owned(owner);
                }
            }
            let call = tokio::time::timeout(
                ACTIVATION_ATTEMPT_TIMEOUT,
                connection.call_method(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    Some("org.freedesktop.DBus"),
                    "StartServiceByName",
                    &(PORTAL_DEST, 0_u32),
                ),
            )
            .await;
            match call {
                Ok(Ok(_)) => {
                    if let Ok(owner) = Self::portal_owner(connection, config).await {
                        return ActivationResult::Owned(owner);
                    }
                    last_error = Some("portal activation returned without an owner".to_owned());
                }
                Ok(Err(error)) => {
                    let (kind, raw_name) = Self::classify_call_error(&error);
                    if kind == DbusFailure::NoPortalImplementation {
                        return ActivationResult::Unavailable(
                            "no activatable portal implementation on the bus".to_owned(),
                        );
                    }
                    last_error = Some(match raw_name {
                        Some(name) => format!("portal activation failed: {name}"),
                        None => format!("portal activation failed: {error}"),
                    });
                }
                Err(_) => last_error = Some("portal activation timed out".to_owned()),
            }
        }
        ActivationResult::Unavailable(
            last_error.unwrap_or_else(|| "portal activation failed".to_owned()),
        )
    }

    fn owner_change_matches(current: Option<&str>, old_owner: &str, new_owner: &str) -> bool {
        if old_owner.is_empty() {
            current.is_none() || current == Some(new_owner)
        } else {
            current == Some(old_owner)
        }
    }

    fn publish_owner(shared: &Arc<Mutex<SharedState>>, owner: String) {
        let mut state = lock_shared(shared);
        if state.portal_owner.as_deref() == Some(owner.as_str()) {
            return;
        }
        state.portal_generation = state.portal_generation.saturating_add(1);
        state.portal_owner = Some(owner);
        state.portal_present = Some(true);
        state.unavailable_reason = None;
        let generation = state.portal_generation;
        state
            .notification_events
            .retain(|event| !matches!(event, NotificationBackendEvent::OwnerChanged { .. }));
        state
            .notification_events
            .push_back(NotificationBackendEvent::OwnerChanged {
                generation,
                available: true,
            });
        let completion = state.completion.clone();
        drop(state);
        completion.notify();
    }

    async fn publish_owner_loss(
        shared: &Arc<Mutex<SharedState>>,
        expected_owner: Option<&str>,
    ) -> bool {
        let (active, completion) = {
            let mut state = lock_shared(shared);
            if expected_owner.is_some_and(|owner| state.portal_owner.as_deref() != Some(owner)) {
                return false;
            }
            state.portal_owner = None;
            state.portal_present = None;
            state.unavailable_reason = None;
            let generation = state.portal_generation;
            state
                .notification_events
                .retain(|event| !matches!(event, NotificationBackendEvent::OwnerChanged { .. }));
            state
                .notification_events
                .push_back(NotificationBackendEvent::OwnerChanged {
                    generation,
                    available: false,
                });
            let active = state.active.drain().collect::<Vec<_>>();
            let completion = state.completion.clone();
            (active, completion)
        };
        for (_, request) in &active {
            request.task.abort();
        }
        let mut terminal = Vec::with_capacity(active.len());
        for (id, request) in active {
            let kind = request.kind;
            let _ = request.task.await;
            terminal.push((id, kind));
        }
        let mut state = lock_shared(shared);
        for (id, kind) in terminal {
            match kind {
                ActiveRequestKind::File => {
                    state.replies.insert(id, Self::failed_reply());
                }
                ActiveRequestKind::OpenUri => {
                    state.open_uri_replies.insert(id, OpenUriReply::Unavailable);
                }
                ActiveRequestKind::Print => {
                    state.print_replies.insert(id, PortalOutcome::Unavailable);
                }
            }
        }
        drop(state);
        completion.notify();
        true
    }

    async fn connect(address: Option<String>) -> Result<zbus::Connection, zbus::Error> {
        match address {
            Some(addr) => {
                zbus::connection::Builder::address(addr.as_str())?
                    .build()
                    .await
            }
            None => zbus::Connection::session().await,
        }
    }

    async fn close_request(
        connection: &zbus::Connection,
        portal_owner: &str,
        request_path: &OwnedObjectPath,
    ) -> Result<(), zbus::Error> {
        connection
            .call_method(
                Some(portal_owner),
                request_path.as_str(),
                Some(REQUEST_IFACE),
                "Close",
                &(),
            )
            .await
            .map(|_| ())
    }

    fn failed_reply() -> BackendReply {
        BackendReply {
            response: PortalResponse::Other,
            uris: vec![],
        }
    }

    pub(crate) fn start_print(
        &mut self,
        request: PrintRequest,
    ) -> Result<RequestId, PortalOutcome<PrintReceipt>> {
        let Availability::Available {
            unique_name,
            portal_owner,
            generation,
        } = self.probe_availability()
        else {
            return Err(PortalOutcome::Unavailable);
        };

        // Capacity refusal is terminal for Print and immediately drops its owned document fd.
        let (start_permit, close_permit) =
            self.reserve_start_and_close()
                .map_err(|error| match error {
                    BackendError::Unavailable => PortalOutcome::Unavailable,
                    _ => PortalOutcome::Failed,
                })?;

        let id = allocate_request_id(&self.id_counter).map_err(|_| PortalOutcome::Failed)?;
        let prepare_n = self.token_counter.fetch_add(1, Ordering::SeqCst);
        let print_n = self.token_counter.fetch_add(1, Ordering::SeqCst);
        let prepare_path =
            Self::request_path(&unique_name, &format!("enzui_print_prepare_{prepare_n}"));
        let print_path = Self::request_path(&unique_name, &format!("enzui_print_{print_n}"));

        // Publish frontend state before sending through the reserved permit.
        self.in_flight_prints.insert(id);
        self.close_permits.insert(id, close_permit);
        start_permit.send(BackendCommand::StartPrint {
            id,
            generation,
            portal_owner,
            prepare_path,
            print_path,
            request,
        });
        Ok(id)
    }

    pub(crate) fn poll_print(&mut self, id: RequestId) -> Option<PortalOutcome<PrintReceipt>> {
        if !self.in_flight_prints.contains(&id) {
            return None;
        }
        let outcome = lock_shared(&self.shared).print_replies.remove(&id);
        if outcome.is_some() {
            self.in_flight_prints.remove(&id);
            self.close_permits.remove(&id);
            self.closed_count = self.closed_count.saturating_add(1);
        }
        outcome
    }

    pub(crate) fn close_print(&mut self, id: RequestId, reason: CancelReason) {
        if !self.in_flight_prints.contains(&id) {
            let _ = reason;
            return;
        }
        // Use the reserved Close permit; preserve state if the worker is gone.
        if self.cmd_tx.is_closed() {
            return;
        }
        let Some(permit) = self.close_permits.remove(&id) else {
            return;
        };
        permit.send(BackendCommand::Close { id });
        self.in_flight_prints.remove(&id);
        self.closed_count = self.closed_count.saturating_add(1);
        // Discard any parked outcome when the caller abandons the request.
        lock_shared(&self.shared).print_replies.remove(&id);
    }

    fn classify_call_error(e: &zbus::Error) -> (DbusFailure, Option<String>) {
        let name = match e {
            zbus::Error::MethodError(name, _, _) => Some(name.as_str().to_owned()),
            _ => None,
        };
        (DbusFailure::classify(name.as_deref()), name)
    }
}

impl Drop for DbusBackend {
    fn drop(&mut self) {
        // Drop unused Close permits before beginning the bounded drain.
        self.close_permits.clear();
        let _ = self.shutdown_tx.send(true);

        // Detach a running owner; join only when `is_finished` proves the call cannot block.
        if let Some(owner) = self.shutdown_owner.take()
            && owner.is_finished()
        {
            let _ = owner.join();
        }
    }
}

impl PortalBackend for DbusBackend {
    fn start(&mut self, operation: PortalOperation) -> Result<RequestId, BackendError> {
        self.start_with_options(operation, &ChooserOptions::new(), &ParentHandle::none())
    }

    fn start_with_options(
        &mut self,
        operation: PortalOperation,
        options: &ChooserOptions,
        parent: &ParentHandle,
    ) -> Result<RequestId, BackendError> {
        // Map `Initialising` to retryable deferral and settled `Unavailable` to terminal refusal.
        let (unique_name, portal_owner, generation) = match self.probe_availability() {
            Availability::Available {
                unique_name,
                portal_owner,
                generation,
            } => (unique_name, portal_owner, generation),
            Availability::Initialising => return Err(BackendError::Initialising),
            Availability::Unavailable => return Err(BackendError::Unavailable),
        };

        // Reserve start and Close slots before allocating an id or cloning the payload.
        let (start_permit, close_permit) = self.reserve_start_and_close()?;

        let id = allocate_request_id(&self.id_counter)?;

        let token_n = self.token_counter.fetch_add(1, Ordering::SeqCst);
        let handle_token = format!("enzui_{token_n}");
        let request_path = Self::request_path(&unique_name, &handle_token);

        // Publish frontend bookkeeping before exposing Start to the worker.
        self.in_flight.insert(id);
        self.close_permits.insert(id, close_permit);
        start_permit.send(BackendCommand::Start {
            id,
            generation,
            portal_owner,
            operation,
            request_path,
            options: options.clone(),
            parent: parent.clone(),
        });
        Ok(id)
    }

    fn poll_reply(&mut self, id: RequestId) -> Option<BackendReply> {
        // Treat unknown and retired ids as absent so stale handles cannot panic or receive late replies.
        if !self.in_flight.contains(&id) {
            return None;
        }
        let reply = lock_shared(&self.shared).replies.remove(&id);
        if reply.is_some() {
            self.in_flight.remove(&id);
            self.close_permits.remove(&id);
            self.closed_count = self.closed_count.saturating_add(1);
        }
        reply
    }

    fn completion_signal(&self) -> Option<crate::portal::completion::CompletionSignal> {
        Some(lock_shared(&self.shared).completion.clone())
    }

    fn close(&mut self, id: RequestId, reason: CancelReason) {
        // Ignore unknown, retired, and repeated closes without issuing a wire call.
        if !self.in_flight.contains(&id) {
            let _ = reason;
            return;
        }
        let _ = reason;
        // Send Close through the slot reserved at start.
        if self.cmd_tx.is_closed() {
            return;
        }
        let Some(permit) = self.close_permits.remove(&id) else {
            return;
        };
        permit.send(BackendCommand::Close { id });
        self.in_flight.remove(&id);
        self.closed_count = self.closed_count.saturating_add(1);
        // Discard a parked reply when its request is closed.
        lock_shared(&self.shared).replies.remove(&id);
    }

    fn describe(&self) -> String {
        let s = lock_shared(&self.shared);
        let availability = Availability::classify(
            s.available,
            s.unique_name.as_deref(),
            s.portal_present,
            s.unavailable_reason.as_deref(),
            s.portal_owner.as_deref(),
            s.portal_generation,
        );
        Self::describe_state(
            self.probed,
            matches!(availability, Availability::Available { .. }),
            if matches!(availability, Availability::Initialising) {
                Some("availability probe is still initialising")
            } else if s.portal_present == Some(false)
                && s.available
                && s.unavailable_reason.is_none()
            {
                Some("no activatable portal implementation on the bus")
            } else {
                s.unavailable_reason.as_deref()
            },
            s.last_failure.as_ref().map(|(k, n)| (*k, n.as_deref())),
            self.in_flight.len()
                + self.in_flight_open_uris.len()
                + self.in_flight_prints.len()
                + self.notification_in_flight.len(),
            self.closed_count,
        )
    }
}

impl NotificationBackend for DbusBackend {
    fn notification_application_id(&self) -> Option<&AppId> {
        self.application_id.as_ref()
    }

    fn submit_notification(
        &mut self,
        command: NotificationBackendCommand,
    ) -> Result<NotificationOperationId, NotificationBackendError> {
        if self.notification_in_flight.len() >= COMMAND_QUEUE_CAPACITY {
            return Err(NotificationBackendError::ResourceExhausted);
        }
        let (portal_owner, generation) = match self.probe_availability() {
            Availability::Available {
                portal_owner,
                generation,
                ..
            } => (portal_owner, generation),
            Availability::Initialising => return Err(NotificationBackendError::Initialising),
            Availability::Unavailable => return Err(NotificationBackendError::Unavailable),
        };
        if let NotificationBackendCommand::Send(notification) = &command
            && notification.has_callback_actions()
            && lock_shared(&self.shared).notification_action_stream_ready != Some(true)
        {
            return Err(NotificationBackendError::Unsupported(
                UnsupportedNotificationFeature::ActionDelivery,
            ));
        }
        let permit = self
            .cmd_tx
            .clone()
            .try_reserve_owned()
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => NotificationBackendError::ResourceExhausted,
                mpsc::error::TrySendError::Closed(_) => NotificationBackendError::Unavailable,
            })?;
        let request = allocate_request_id(&self.id_counter).map_err(|error| match error {
            BackendError::ResourceExhausted => NotificationBackendError::ResourceExhausted,
            _ => NotificationBackendError::Failed,
        })?;
        let id = NotificationOperationId::new(request.get());
        self.notification_in_flight.insert(id);
        permit.send(BackendCommand::Notification {
            id,
            generation,
            portal_owner,
            command,
        });
        Ok(id)
    }

    fn poll_notification_event(&mut self) -> Option<NotificationBackendEvent> {
        let event = lock_shared(&self.shared).notification_events.pop_front()?;
        match event {
            NotificationBackendEvent::Operation {
                operation,
                owner_generation,
                outcome,
            } => {
                self.notification_in_flight.remove(&operation);
                Some(NotificationBackendEvent::Operation {
                    operation,
                    owner_generation,
                    outcome,
                })
            }
            other => Some(other),
        }
    }

    fn notification_completion_signal(
        &self,
    ) -> Option<crate::portal::completion::CompletionSignal> {
        Some(lock_shared(&self.shared).completion.clone())
    }

    fn notification_stats(&self) -> NotificationBackendStats {
        let state = lock_shared(&self.shared);
        NotificationBackendStats {
            pending_actions: state
                .notification_events
                .iter()
                .filter(|event| matches!(event, NotificationBackendEvent::Action(_)))
                .count(),
            action_capacity: NOTIFICATION_ACTION_QUEUE_CAPACITY,
            dropped_actions: state.notification_dropped_actions,
            rejected_actions: state.notification_rejected_actions,
        }
    }
}

impl OpenUriBackend for DbusBackend {
    fn start_open_uri(
        &mut self,
        uri: &OpenUri,
        parent: &ParentHandle,
    ) -> Result<RequestId, BackendError> {
        self.start_open_uri_with_activation(uri, parent, None)
    }

    fn start_open_uri_with_activation(
        &mut self,
        uri: &OpenUri,
        parent: &ParentHandle,
        activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        let (unique_name, portal_owner, generation) = match self.probe_availability() {
            Availability::Available {
                unique_name,
                portal_owner,
                generation,
            } => (unique_name, portal_owner, generation),
            Availability::Initialising => return Err(BackendError::Initialising),
            Availability::Unavailable => return Err(BackendError::Unavailable),
        };
        let (start_permit, close_permit) = self.reserve_start_and_close()?;
        let id = allocate_request_id(&self.id_counter)?;
        let token = self.token_counter.fetch_add(1, Ordering::SeqCst);
        let request_path = Self::request_path(&unique_name, &format!("enzui_open_uri_{token}"));

        self.in_flight_open_uris.insert(id);
        self.close_permits.insert(id, close_permit);
        start_permit.send(BackendCommand::StartOpenUri {
            id,
            generation,
            portal_owner,
            request_path,
            uri: uri.clone(),
            parent: parent.clone(),
            activation: activation.map(|context| OpenUriActivation {
                token: context.token_raw().to_owned(),
                expires_at: context.expires_at(),
            }),
        });
        Ok(id)
    }

    fn start_open_file(
        &mut self,
        file: &RetainedFile,
        parent: &ParentHandle,
        activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        self.start_retained_file("OpenFile", file.descriptor(), parent, activation)
    }

    fn start_open_directory(
        &mut self,
        directory: &RetainedDirectory,
        parent: &ParentHandle,
        activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        // `OpenFile` given a directory opens it in the file manager;
        // `OpenDirectory` would open the folder that contains it.
        self.start_retained_file("OpenFile", directory.descriptor(), parent, activation)
    }

    fn start_open_containing_folder(
        &mut self,
        file: &RetainedFile,
        parent: &ParentHandle,
        activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        self.start_retained_file("OpenDirectory", file.descriptor(), parent, activation)
    }

    fn poll_open_uri(&mut self, id: RequestId) -> Option<OpenUriReply> {
        if !self.in_flight_open_uris.contains(&id) {
            return None;
        }
        let reply = lock_shared(&self.shared).open_uri_replies.remove(&id);
        if reply.is_some() {
            self.in_flight_open_uris.remove(&id);
            self.close_permits.remove(&id);
            self.closed_count = self.closed_count.saturating_add(1);
        }
        reply
    }

    fn close_open_uri(&mut self, id: RequestId, reason: CancelReason) {
        if !self.in_flight_open_uris.contains(&id) {
            let _ = reason;
            return;
        }
        if self.cmd_tx.is_closed() {
            return;
        }
        let Some(permit) = self.close_permits.remove(&id) else {
            return;
        };
        permit.send(BackendCommand::Close { id });
        self.in_flight_open_uris.remove(&id);
        self.closed_count = self.closed_count.saturating_add(1);
        lock_shared(&self.shared).open_uri_replies.remove(&id);
    }

    fn open_uri_completion_signal(&self) -> Option<crate::portal::completion::CompletionSignal> {
        Some(lock_shared(&self.shared).completion.clone())
    }

    fn describe_open_uri(&self) -> String {
        PortalBackend::describe(self)
    }
}

impl DbusBackend {
    fn start_retained_file(
        &mut self,
        member: &'static str,
        file: Arc<std::fs::File>,
        parent: &ParentHandle,
        activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        let (unique_name, portal_owner, generation) = match self.probe_availability() {
            Availability::Available {
                unique_name,
                portal_owner,
                generation,
            } => (unique_name, portal_owner, generation),
            Availability::Initialising => return Err(BackendError::Initialising),
            Availability::Unavailable => return Err(BackendError::Unavailable),
        };
        let (start_permit, close_permit) = self.reserve_start_and_close()?;
        let id = allocate_request_id(&self.id_counter)?;
        let token = self.token_counter.fetch_add(1, Ordering::SeqCst);
        let request_path = Self::request_path(&unique_name, &format!("enzui_open_file_{token}"));

        self.in_flight_open_uris.insert(id);
        self.close_permits.insert(id, close_permit);
        start_permit.send(BackendCommand::StartOpenFile {
            id,
            generation,
            portal_owner,
            request_path,
            member,
            file,
            parent: parent.clone(),
            activation: activation.map(|context| OpenUriActivation {
                token: context.token_raw().to_owned(),
                expires_at: context.expires_at(),
            }),
        });
        Ok(id)
    }
}

impl DbusBackend {
    fn describe_state(
        probed: bool,
        available: bool,
        unavailable_reason: Option<&str>,
        last_failure: Option<(DbusFailure, Option<&str>)>,
        in_flight: usize,
        closed: usize,
    ) -> String {
        let failure = match last_failure {
            Some((kind, Some(raw))) => {
                format!(" [last failure: {} ({raw})]", kind.describe())
            }
            Some((kind, None)) => format!(" [last failure: {}]", kind.describe()),
            None => String::new(),
        };
        if !probed {
            return format!(
                "D-Bus (zbus, not yet probed) — {in_flight} in flight, {closed} closed"
            );
        }
        if available {
            format!("D-Bus (zbus, real portal) — {in_flight} in flight, {closed} closed{failure}")
        } else {
            format!(
                "D-Bus (zbus, UNAVAILABLE: {}) — {in_flight} in flight, {closed} closed{failure}",
                unavailable_reason.unwrap_or("unknown"),
            )
        }
    }
}

impl std::fmt::Debug for DbusBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbusBackend")
            .field("file_in_flight", &self.in_flight.len())
            .field("open_uri_in_flight", &self.in_flight_open_uris.len())
            .field("print_in_flight", &self.in_flight_prints.len())
            .field("notification_in_flight", &self.notification_in_flight.len())
            .field("closed_count", &self.closed_count)
            .finish()
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mock_dbus;

mod application_identity;
mod notification_icon;
mod portal_calls;

#[cfg(test)]
pub(super) mod readiness_hook {
    use std::cell::RefCell;

    use super::ReadinessConfig;

    thread_local! {
        static CONFIG: RefCell<Option<ReadinessConfig>> = const { RefCell::new(None) };
    }

    pub(super) fn take() -> ReadinessConfig {
        CONFIG
            .with(|slot| slot.borrow_mut().take())
            .unwrap_or_default()
    }
}

pub mod session;
