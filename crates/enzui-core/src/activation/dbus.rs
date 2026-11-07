//! `org.freedesktop.Application` transport for an existing enzUI instance.

use super::{
    ActivationBroker, ActivationContext, ApplicationIntent, InboundActivationToken,
    InboundTokenError,
};
use crate::portal::CompletionSignal;
use iced_futures::Subscription;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;
use zbus::zvariant::OwnedValue;

pub mod search;

/// Maximum method calls retained while the iced application is not draining.
pub const APPLICATION_CALL_CAPACITY: usize = 64;
/// Maximum URIs retained by one Open call.
pub const APPLICATION_URI_CAPACITY: usize = 64;
/// Maximum UTF-8 bytes retained for one URI or action name.
pub const APPLICATION_TEXT_CAPACITY: usize = 16 * 1024;
/// Maximum dynamic parameters retained by one ActivateAction call.
pub const APPLICATION_PARAMETER_CAPACITY: usize = 16;
/// Maximum serialized bytes retained across ActivateAction parameters.
pub const APPLICATION_PARAMETER_BYTES: usize = 64 * 1024;
/// Deadline for connecting, exporting the object, and acquiring the app name.
pub const APPLICATION_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Deadline for the bus to start an installed application and deliver a
/// launch to it.
pub const APPLICATION_ACTIVATION_TIMEOUT: Duration = Duration::from_secs(20);

pub use super::application_object_path;

/// Event-driven wake emitted when an application call enters the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ApplicationWake {
    /// Completion generation observed by the subscription.
    pub generation: u64,
}

#[derive(Clone)]
struct WakeKey {
    key: u64,
    signal: CompletionSignal,
}

impl std::hash::Hash for WakeKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&self.key, state);
    }
}

struct QueuedCall {
    intent: ApplicationIntent,
    parameters: Vec<OwnedValue>,
    activation_token: Option<InboundActivationToken>,
    activation_error: Option<InboundTokenError>,
    received_at: Instant,
}

impl fmt::Debug for QueuedCall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("QueuedCall")
            .field("intent", &self.intent)
            .field("parameter_count", &self.parameters.len())
            .field("has_activation", &self.activation_token.is_some())
            .field("activation_error", &self.activation_error)
            .finish()
    }
}

/// One desktop request ready for application routing.
pub struct ApplicationCall {
    /// Activate, Open, or ActivateAction intent.
    pub intent: ApplicationIntent,
    /// Parameters supplied to ActivateAction, retained in wire order.
    pub parameters: Vec<OwnedValue>,
    /// Compositor context parsed from `platform_data`, if valid.
    pub context: Option<ActivationContext>,
    /// A malformed activation token was discarded without rejecting the intent.
    pub context_error: Option<InboundTokenError>,
}

impl fmt::Debug for ApplicationCall {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ApplicationCall")
            .field("intent", &self.intent)
            .field("parameter_count", &self.parameters.len())
            .field("has_context", &self.context.is_some())
            .field("context_error", &self.context_error)
            .finish()
    }
}

#[derive(Default)]
struct CallState {
    queue: VecDeque<QueuedCall>,
    refused: u64,
}

#[derive(Clone)]
struct ApplicationObject {
    state: Arc<Mutex<CallState>>,
    completion: CompletionSignal,
}

fn lock(state: &Mutex<CallState>) -> MutexGuard<'_, CallState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn admit_queued_context(
    broker: &mut ActivationBroker,
    activation_token: Option<InboundActivationToken>,
    activation_error: Option<InboundTokenError>,
    received_at: Instant,
    now: Instant,
) -> (Option<ActivationContext>, Option<InboundTokenError>) {
    if let Some(error) = activation_error {
        return (None, Some(error));
    }
    match activation_token
        .map(|token| broker.admit_token(token, received_at))
        .transpose()
    {
        Ok(Some(context)) if context.is_expired(now) => (None, Some(InboundTokenError::Expired)),
        Ok(context) => (context, None),
        Err(error) => (None, Some(error)),
    }
}

fn valid_application_intent(intent: &ApplicationIntent) -> bool {
    match intent {
        ApplicationIntent::Activate => true,
        ApplicationIntent::Open(uris) => {
            uris.len() <= APPLICATION_URI_CAPACITY
                && uris
                    .iter()
                    .all(|uri| !uri.is_empty() && uri.len() <= APPLICATION_TEXT_CAPACITY)
        }
        ApplicationIntent::ActivateAction { name } => {
            !name.is_empty() && name.len() <= APPLICATION_TEXT_CAPACITY
        }
    }
}

impl ApplicationObject {
    fn enqueue(
        &self,
        intent: ApplicationIntent,
        parameters: Vec<OwnedValue>,
        platform_data: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        let valid_intent = valid_application_intent(&intent);
        let parameter_bytes = parameters.iter().try_fold(0_usize, |total, parameter| {
            let context = zbus::zvariant::serialized::Context::new_dbus(zbus::zvariant::LE, 0);
            zbus::zvariant::serialized_size(context, parameter)
                .ok()
                .and_then(|size| total.checked_add(*size))
        });
        if !valid_intent
            || parameters.len() > APPLICATION_PARAMETER_CAPACITY
            || parameter_bytes.is_none_or(|bytes| bytes > APPLICATION_PARAMETER_BYTES)
        {
            return Err(zbus::fdo::Error::InvalidArgs(
                "application activation payload exceeds its bound".into(),
            ));
        }

        let mut state = lock(&self.state);
        if state.queue.len() >= APPLICATION_CALL_CAPACITY {
            state.refused = state.refused.saturating_add(1);
            return Err(zbus::fdo::Error::LimitsExceeded(
                "application activation queue is full".into(),
            ));
        }
        let (activation_token, activation_error) =
            match super::inbound_platform_data(&platform_data) {
                Ok(token) => (token, None),
                Err(error) => (None, Some(error)),
            };
        state.queue.push_back(QueuedCall {
            intent,
            parameters,
            activation_token,
            activation_error,
            received_at: Instant::now(),
        });
        drop(state);
        self.completion.notify();
        Ok(())
    }
}

#[zbus::interface(name = "org.freedesktop.Application", spawn = false)]
impl ApplicationObject {
    #[zbus(name = "Activate")]
    fn activate(&self, platform_data: HashMap<String, OwnedValue>) -> zbus::fdo::Result<()> {
        self.enqueue(ApplicationIntent::Activate, Vec::new(), platform_data)
    }

    #[zbus(name = "Open")]
    fn open(
        &self,
        uris: Vec<String>,
        platform_data: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        self.enqueue(ApplicationIntent::Open(uris), Vec::new(), platform_data)
    }

    #[zbus(name = "ActivateAction")]
    fn activate_action(
        &self,
        action_name: String,
        parameter: Vec<OwnedValue>,
        platform_data: HashMap<String, OwnedValue>,
    ) -> zbus::fdo::Result<()> {
        self.enqueue(
            ApplicationIntent::ActivateAction { name: action_name },
            parameter,
            platform_data,
        )
    }
}

/// Failure to establish an application activation endpoint.
#[derive(Debug)]
pub enum ApplicationServiceError {
    /// The supplied application id cannot be used as a D-Bus well-known name.
    InvalidApplicationId(String),
    /// A Tokio runtime could not be created.
    Runtime(String),
    /// The session bus could not be reached or the object could not be served.
    Connect(String),
    /// Another process already owns the application id.
    NameNotAcquired(String),
    /// A launch intent or activation token exceeded its transport bounds.
    InvalidRequest,
    /// The existing instance refused delivery or disappeared.
    Forward(String),
    /// Connection and name acquisition exceeded the fixed deadline.
    Timeout,
    /// The bus could not start the installed application, or it did not
    /// answer the launch within [`APPLICATION_ACTIVATION_TIMEOUT`].
    Activation(String),
}

impl fmt::Display for ApplicationServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidApplicationId(error) => {
                write!(formatter, "invalid application id: {error}")
            }
            Self::InvalidRequest => {
                formatter.write_str("application launch payload exceeds its bound")
            }
            Self::Forward(error) => {
                write!(formatter, "could not notify existing application: {error}")
            }
            Self::Activation(error) => {
                write!(
                    formatter,
                    "the installed application did not start: {error}"
                )
            }
            Self::Runtime(error) => write!(formatter, "could not create D-Bus runtime: {error}"),
            Self::Connect(error) => {
                write!(formatter, "could not serve application endpoint: {error}")
            }
            Self::NameNotAcquired(error) => {
                write!(formatter, "could not acquire application name: {error}")
            }
            Self::Timeout => formatter.write_str("application D-Bus connection timed out"),
        }
    }
}

impl std::error::Error for ApplicationServiceError {}

/// Whether this launch acquired the app endpoint or reused its current owner.
#[derive(Debug)]
pub enum ApplicationStart {
    /// Keep this service in the shared GUI/background state until final exit.
    Owner(ApplicationService),
    /// Another process owns the endpoint; this launching process may exit.
    Reused,
}

/// Owner of one `org.freedesktop.Application` endpoint and its bounded queue.
pub struct ApplicationService {
    state: Arc<Mutex<CallState>>,
    completion: CompletionSignal,
    path: zbus::zvariant::OwnedObjectPath,
    connection: zbus::Connection,
    runtime: Runtime,
}

impl fmt::Debug for ApplicationService {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = lock(&self.state);
        formatter
            .debug_struct("ApplicationService")
            .field("queued", &state.queue.len())
            .field("refused", &state.refused)
            .finish()
    }
}

impl ApplicationService {
    /// Connects to the session bus and exclusively owns `application_id`.
    ///
    /// # Errors
    ///
    /// An [`ApplicationServiceError`] when the id is not a valid bus name, the bus cannot be
    /// reached, another process owns the name, or setup times out.
    pub fn connect(application_id: &str) -> Result<Self, ApplicationServiceError> {
        Self::connect_inner(application_id, None)
    }

    /// Connects to an explicit bus address, intended for isolated service tests.
    ///
    /// # Errors
    ///
    /// As [`Self::connect`], on the given bus.
    pub fn connect_to_address(
        application_id: &str,
        address: &str,
    ) -> Result<Self, ApplicationServiceError> {
        Self::connect_inner(application_id, Some(address.to_owned()))
    }

    /// Acquires the app identity or notifies its existing process.
    ///
    /// Call this blocking startup helper before entering the application or an
    /// async runtime.
    ///
    /// Pass no intent for broker background activation: an existing GUI stays
    /// in place and receives no presentation request. An explicit desktop launch
    /// may pass Activate, Open or a parameter-free ActivateAction. The primary
    /// owner handles its own startup intent; only a secondary forwards it.
    /// Forwarding targets the current owner's unique bus name and never queues
    /// a replacement owner or starts another process. Activation tokens remain
    /// compositor-policy inputs; delivery does not guarantee focus. This
    /// compatibility method treats a string as Wayland context; use
    /// [`Self::connect_or_forward_with_token`] for typed X11 or combined data.
    ///
    /// # Errors
    /// Invalid or oversized requests, bus/name failures, bounded timeout, or
    /// refusal/disappearance of an existing owner. A failed notification never
    /// falls through to starting a second app.
    pub fn connect_or_forward(
        application_id: &str,
        intent: Option<&ApplicationIntent>,
        activation_token: Option<&str>,
    ) -> Result<ApplicationStart, ApplicationServiceError> {
        Self::start_inner(
            application_id,
            None,
            intent,
            legacy_activation_token(activation_token)?.as_ref(),
        )
    }

    /// Uses an explicit bus address for isolated application lifecycle tests.
    ///
    /// # Errors
    ///
    /// As `connect_or_forward`, on the given bus.
    pub fn connect_or_forward_to_address(
        application_id: &str,
        address: &str,
        intent: Option<&ApplicationIntent>,
        activation_token: Option<&str>,
    ) -> Result<ApplicationStart, ApplicationServiceError> {
        Self::start_inner(
            application_id,
            Some(address.to_owned()),
            intent,
            legacy_activation_token(activation_token)?.as_ref(),
        )
    }

    /// Delivers `intent` to the installed application through D-Bus
    /// activation: the call goes to the well-known name with auto-start
    /// allowed, so the bus starts the application's service when none runs
    /// and hands the call to it. Two launches at once start one instance.
    /// This compatibility method treats a string as Wayland context; use
    /// [`Self::activate_installed_with_token`] for typed X11 or combined data.
    ///
    /// # Errors
    /// The request exceeds its bounds, the bus is unreachable, or the
    /// application failed to start or answer within
    /// [`APPLICATION_ACTIVATION_TIMEOUT`]. Nothing is retried: the caller
    /// reports the failure rather than starting a second instance.
    pub fn activate_installed(
        application_id: &str,
        intent: &ApplicationIntent,
        activation_token: Option<&str>,
    ) -> Result<(), ApplicationServiceError> {
        Self::activate_inner(
            application_id,
            None,
            intent,
            legacy_activation_token(activation_token)?,
        )
    }

    /// [`Self::activate_installed`] on an explicit bus, for isolated tests.
    ///
    /// # Errors
    ///
    /// As [`Self::activate_installed`], on the given bus.
    pub fn activate_installed_at_address(
        application_id: &str,
        address: &str,
        intent: &ApplicationIntent,
        activation_token: Option<&str>,
    ) -> Result<(), ApplicationServiceError> {
        Self::activate_inner(
            application_id,
            Some(address.to_owned()),
            intent,
            legacy_activation_token(activation_token)?,
        )
    }

    /// Acquires the app identity or forwards typed platform context to its existing owner.
    ///
    /// A primary owner retains `token` for its startup intent. Successful
    /// forwarding consumes it; a failed operation leaves it with the caller.
    /// Untyped native notification context must be selected for a platform first.
    ///
    /// # Errors
    ///
    /// As [`Self::connect_or_forward`], including untyped or invalid context.
    pub fn connect_or_forward_with_token(
        application_id: &str,
        intent: Option<&ApplicationIntent>,
        token: &mut Option<InboundActivationToken>,
    ) -> Result<ApplicationStart, ApplicationServiceError> {
        Self::start_with_token_inner(application_id, None, intent, token)
    }

    /// Forwards typed platform context on an explicit bus.
    ///
    /// # Errors
    ///
    /// As [`Self::connect_or_forward_with_token`], on the given bus.
    pub fn connect_or_forward_with_token_to_address(
        application_id: &str,
        address: &str,
        intent: Option<&ApplicationIntent>,
        token: &mut Option<InboundActivationToken>,
    ) -> Result<ApplicationStart, ApplicationServiceError> {
        Self::start_with_token_inner(application_id, Some(address.to_owned()), intent, token)
    }

    /// Activates an installed application with typed Wayland or X11 platform data.
    ///
    /// Untyped native notification context must be selected for a platform first.
    ///
    /// # Errors
    ///
    /// As [`Self::activate_installed`], including untyped or invalid context.
    pub fn activate_installed_with_token(
        application_id: &str,
        intent: &ApplicationIntent,
        token: Option<InboundActivationToken>,
    ) -> Result<(), ApplicationServiceError> {
        Self::activate_inner(application_id, None, intent, token)
    }

    /// Activates an installed application with typed context on an explicit bus.
    ///
    /// # Errors
    ///
    /// As [`Self::activate_installed_with_token`], on the given bus.
    pub fn activate_installed_with_token_at_address(
        application_id: &str,
        address: &str,
        intent: &ApplicationIntent,
        token: Option<InboundActivationToken>,
    ) -> Result<(), ApplicationServiceError> {
        Self::activate_inner(application_id, Some(address.to_owned()), intent, token)
    }

    fn activate_inner(
        application_id: &str,
        address: Option<String>,
        intent: &ApplicationIntent,
        activation_token: Option<InboundActivationToken>,
    ) -> Result<(), ApplicationServiceError> {
        if !valid_application_intent(intent)
            || activation_token
                .as_ref()
                .is_some_and(|token| !valid_outbound_activation(token))
        {
            return Err(ApplicationServiceError::InvalidRequest);
        }
        let name = zbus::names::WellKnownName::try_from(application_id)
            .map_err(|error| ApplicationServiceError::InvalidApplicationId(error.to_string()))?
            .to_owned();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| ApplicationServiceError::Runtime(error.to_string()))?;
        runtime.block_on(async {
            let delivery = async {
                let builder = match address {
                    Some(address) => zbus::connection::Builder::address(address.as_str()),
                    None => zbus::connection::Builder::session(),
                }
                .map_err(|error| ApplicationServiceError::Connect(error.to_string()))?;
                let connection = builder
                    .build()
                    .await
                    .map_err(|error| ApplicationServiceError::Connect(error.to_string()))?;
                let path = format!("/{}", application_id.replace('.', "/").replace('-', "_"));
                // Addressed to the well-known name, not its current owner, so
                // the bus may auto-start the service.
                let proxy = zbus::Proxy::new(
                    &connection,
                    name,
                    path.as_str(),
                    "org.freedesktop.Application",
                )
                .await
                .map_err(|error| ApplicationServiceError::Activation(error.to_string()))?;
                deliver_intent(&proxy, intent, activation_token.as_ref())
                    .await
                    .map_err(ApplicationServiceError::Activation)
            };
            match tokio::time::timeout(APPLICATION_ACTIVATION_TIMEOUT, delivery).await {
                Ok(result) => result,
                Err(_) => Err(ApplicationServiceError::Activation(format!(
                    "no answer within {} s",
                    APPLICATION_ACTIVATION_TIMEOUT.as_secs()
                ))),
            }
        })
    }

    fn start_with_token_inner(
        application_id: &str,
        address: Option<String>,
        intent: Option<&ApplicationIntent>,
        token: &mut Option<InboundActivationToken>,
    ) -> Result<ApplicationStart, ApplicationServiceError> {
        let outcome = Self::start_inner(application_id, address, intent, token.as_ref())?;
        if matches!(&outcome, ApplicationStart::Reused) {
            let _ = token.take();
        }
        Ok(outcome)
    }

    fn start_inner(
        application_id: &str,
        address: Option<String>,
        intent: Option<&ApplicationIntent>,
        activation_token: Option<&InboundActivationToken>,
    ) -> Result<ApplicationStart, ApplicationServiceError> {
        if intent.is_some_and(|intent| !valid_application_intent(intent))
            || activation_token
                .is_some_and(|token| !valid_outbound_activation(token) || intent.is_none())
        {
            return Err(ApplicationServiceError::InvalidRequest);
        }
        match Self::connect_inner(application_id, address.clone()) {
            Ok(service) => return Ok(ApplicationStart::Owner(service)),
            Err(ApplicationServiceError::NameNotAcquired(_)) => {}
            Err(error) => return Err(error),
        }
        let Some(intent) = intent else {
            return Ok(ApplicationStart::Reused);
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| ApplicationServiceError::Runtime(error.to_string()))?;
        runtime.block_on(async {
            let delivery = async {
                let builder = match address {
                    Some(address) => zbus::connection::Builder::address(address.as_str()),
                    None => zbus::connection::Builder::session(),
                }
                .map_err(|error| ApplicationServiceError::Forward(error.to_string()))?;
                let connection = builder
                    .build()
                    .await
                    .map_err(|error| ApplicationServiceError::Forward(error.to_string()))?;
                let bus = zbus::fdo::DBusProxy::new(&connection)
                    .await
                    .map_err(|error| ApplicationServiceError::Forward(error.to_string()))?;
                let name = zbus::names::BusName::try_from(application_id).map_err(|error| {
                    ApplicationServiceError::InvalidApplicationId(error.to_string())
                })?;
                let owner = bus
                    .get_name_owner(name)
                    .await
                    .map_err(|error| ApplicationServiceError::Forward(error.to_string()))?;
                let path = format!("/{}", application_id.replace('.', "/").replace('-', "_"));
                let proxy = zbus::Proxy::new(
                    &connection,
                    owner.as_str(),
                    path.as_str(),
                    "org.freedesktop.Application",
                )
                .await
                .map_err(|error| ApplicationServiceError::Forward(error.to_string()))?;
                deliver_intent(&proxy, intent, activation_token)
                    .await
                    .map_err(ApplicationServiceError::Forward)?;
                Ok(ApplicationStart::Reused)
            };
            match tokio::time::timeout(APPLICATION_CONNECT_TIMEOUT, delivery).await {
                Ok(result) => result,
                Err(_) => Err(ApplicationServiceError::Timeout),
            }
        })
    }

    fn connect_inner(
        application_id: &str,
        address: Option<String>,
    ) -> Result<Self, ApplicationServiceError> {
        let name = zbus::names::WellKnownName::try_from(application_id)
            .map_err(|error| ApplicationServiceError::InvalidApplicationId(error.to_string()))?;
        let object_path = application_object_path(application_id);
        let path = zbus::zvariant::ObjectPath::try_from(object_path.as_str())
            .map_err(|error| ApplicationServiceError::InvalidApplicationId(error.to_string()))?
            .to_owned();
        let runtime =
            Runtime::new().map_err(|error| ApplicationServiceError::Runtime(error.to_string()))?;
        let state = Arc::new(Mutex::new(CallState::default()));
        let completion = CompletionSignal::new();
        let object = ApplicationObject {
            state: Arc::clone(&state),
            completion: completion.clone(),
        };
        let requested_name = name.to_owned();
        let served_path = zbus::zvariant::OwnedObjectPath::from(path.clone());

        let connection = runtime.block_on(async move {
            let builder = match address {
                Some(address) => zbus::connection::Builder::address(address.as_str())
                    .map_err(|error| ApplicationServiceError::Connect(error.to_string()))?,
                None => zbus::connection::Builder::session()
                    .map_err(|error| ApplicationServiceError::Connect(error.to_string()))?,
            };
            let transaction = async move {
                let connection = builder
                    .allow_name_replacements(false)
                    .replace_existing_names(false)
                    .name(requested_name.clone())
                    .map_err(|error| ApplicationServiceError::NameNotAcquired(error.to_string()))?
                    .serve_at(path, object)
                    .map_err(|error| ApplicationServiceError::Connect(error.to_string()))?
                    .build()
                    .await
                    .map_err(|error| match error {
                        zbus::Error::NameTaken => ApplicationServiceError::NameNotAcquired(
                            "another process owns the application name".into(),
                        ),
                        error => ApplicationServiceError::Connect(error.to_string()),
                    })?;
                let well_known =
                    zbus::names::BusName::try_from(requested_name.as_str()).map_err(|error| {
                        ApplicationServiceError::InvalidApplicationId(error.to_string())
                    })?;
                let reported_owner = zbus::fdo::DBusProxy::new(&connection)
                    .await
                    .map_err(|error| ApplicationServiceError::Connect(error.to_string()))?
                    .get_name_owner(well_known)
                    .await
                    .map_err(|error| ApplicationServiceError::NameNotAcquired(error.to_string()))?;
                let our_unique = connection.unique_name().map(|name| name.as_str());
                if our_unique != Some(reported_owner.as_str()) {
                    return Err(ApplicationServiceError::NameNotAcquired(
                        "another process owns the application name".into(),
                    ));
                }
                Ok(connection)
            };

            match tokio::time::timeout(APPLICATION_CONNECT_TIMEOUT, transaction).await {
                Ok(result) => result,
                Err(_) => Err(ApplicationServiceError::Timeout),
            }
        })?;

        Ok(Self {
            state,
            completion,
            path: served_path,
            connection,
            runtime,
        })
    }

    /// Serves `application-lifecycle-v1` ([`super::lifecycle`]) beside the
    /// application endpoint, answering only `callers` (normally
    /// [`LifecycleCallers::installed`](super::lifecycle::LifecycleCallers::installed)),
    /// verified as [`super::lifecycle::admission`] describes. An accepted
    /// stop wakes [`Self::wake_subscription`]; take it from the returned
    /// control.
    ///
    /// # Errors
    /// The interface could not be added to the object.
    pub fn serve_lifecycle(
        &self,
        callers: impl Into<super::lifecycle::LifecycleCallers>,
    ) -> Result<super::lifecycle::StopControl, ApplicationServiceError> {
        self.serve_lifecycle_with(callers.into().0)
    }

    /// Serves the lifecycle to processes running this same executable only:
    /// tests and harnesses that play the OS from the app's own binary.
    ///
    /// # Errors
    /// The interface could not be added to the object.
    pub fn serve_lifecycle_admitting_own_executable(
        &self,
    ) -> Result<super::lifecycle::StopControl, ApplicationServiceError> {
        self.serve_lifecycle_with(super::lifecycle::admission::Admission::OwnExecutable)
    }

    fn serve_lifecycle_with(
        &self,
        admission: super::lifecycle::admission::Admission,
    ) -> Result<super::lifecycle::StopControl, ApplicationServiceError> {
        let control = super::lifecycle::StopControl::new(self.completion.clone());
        let object = super::lifecycle::LifecycleObject {
            control: control.clone(),
            admission,
        };
        let added = self.runtime.block_on(
            self.connection
                .object_server()
                .at(self.path.as_ref(), object),
        );
        match added {
            Ok(true) => Ok(control),
            Ok(false) => Err(ApplicationServiceError::Connect(
                "the lifecycle interface is already served".into(),
            )),
            Err(error) => Err(ApplicationServiceError::Connect(error.to_string())),
        }
    }

    /// Serves `provider` as this app's [`search`] provider beside the
    /// application endpoint. A chosen result wakes
    /// [`Self::wake_subscription`]; drain it from the returned inbox.
    ///
    /// # Errors
    /// The interface could not be added, or is already served.
    pub fn serve_search(
        &self,
        provider: search::SearchProvider,
    ) -> Result<search::SearchInbox, ApplicationServiceError> {
        let (core, inbox) = search::SearchCore::new(provider, self.completion.clone());
        let server = self.connection.object_server();
        let added = self.runtime.block_on(async {
            let ours = server
                .at(self.path.as_ref(), search::SearchObject(Arc::clone(&core)))
                .await?;
            // GNOME Shell and KRunner ask the same provider over their own
            // interfaces, at the same path.
            let gnome = server
                .at(
                    self.path.as_ref(),
                    search::GnomeSearchObject(Arc::clone(&core)),
                )
                .await?;
            let krunner = server
                .at(self.path.as_ref(), search::KRunnerObject(core))
                .await?;
            zbus::Result::Ok(ours && gnome && krunner)
        });
        match added {
            Ok(true) => Ok(inbox),
            Ok(false) => Err(ApplicationServiceError::Connect(
                "the search provider is already served".into(),
            )),
            Err(error) => Err(ApplicationServiceError::Connect(error.to_string())),
        }
    }

    /// Returns an event edge suitable for custom runtime integration.
    #[must_use]
    pub fn completion_signal(&self) -> CompletionSignal {
        self.completion.clone()
    }

    /// Subscribes to accepted Activate, Open, and ActivateAction calls, and to
    /// search activations when [`Self::serve_search`] serves a provider.
    ///
    /// The stream emits one priming wake and then one coalesced generation per
    /// service event; it does not poll on a frame or timer cadence.
    pub fn wake_subscription(&self, key: u64) -> Subscription<ApplicationWake> {
        let signal = self.completion.clone();
        Subscription::run_with(WakeKey { key, signal }, |data| {
            let signal = data.signal.clone();
            iced_futures::futures::stream::unfold((signal, None::<u64>), async |(signal, seen)| {
                match seen {
                    None => {
                        let generation = signal.generation();
                        Some((ApplicationWake { generation }, (signal, Some(generation))))
                    }
                    Some(seen) => {
                        let generation = signal.wait_after(seen).await;
                        Some((ApplicationWake { generation }, (signal, Some(generation))))
                    }
                }
            })
        })
    }

    /// How many accepted Activate, Open and ActivateAction calls wait for
    /// [`Self::drain`].
    #[must_use]
    pub fn queued(&self) -> usize {
        lock(&self.state).queue.len()
    }

    /// Drains at most `limit` calls and admits their activation context.
    pub fn drain(
        &mut self,
        broker: &mut ActivationBroker,
        now: Instant,
        limit: usize,
    ) -> Vec<ApplicationCall> {
        let queued = {
            let mut state = lock(&self.state);
            let count = limit.min(state.queue.len());
            state.queue.drain(..count).collect::<Vec<_>>()
        };

        queued
            .into_iter()
            .map(|call| {
                let (context, context_error) = admit_queued_context(
                    broker,
                    call.activation_token,
                    call.activation_error,
                    call.received_at,
                    now,
                );
                ApplicationCall {
                    intent: call.intent,
                    parameters: call.parameters,
                    context,
                    context_error,
                }
            })
            .collect()
    }

    /// Number of calls refused because the bounded queue was full.
    #[must_use]
    pub fn refused(&self) -> u64 {
        lock(&self.state).refused
    }
}

fn legacy_activation_token(
    raw: Option<&str>,
) -> Result<Option<InboundActivationToken>, ApplicationServiceError> {
    raw.map(|raw| {
        super::validate_token(raw).map_err(|_| ApplicationServiceError::InvalidRequest)?;
        Ok(InboundActivationToken::from(raw))
    })
    .transpose()
}

fn valid_outbound_activation(token: &InboundActivationToken) -> bool {
    token.validate().is_ok() && token.0.untyped_token().is_none()
}

async fn deliver_intent(
    proxy: &zbus::Proxy<'_>,
    intent: &ApplicationIntent,
    activation_token: Option<&InboundActivationToken>,
) -> Result<(), String> {
    let mut data = HashMap::<String, OwnedValue>::new();
    if let Some(token) = activation_token {
        for (key, raw) in [
            ("activation-token", token.xdg_activation_token()),
            ("desktop-startup-id", token.desktop_startup_id()),
        ] {
            if let Some(raw) = raw {
                let value = OwnedValue::try_from(zbus::zvariant::Value::from(raw))
                    .map_err(|error| error.to_string())?;
                data.insert(key.into(), value);
            }
        }
    }
    let result = match intent {
        ApplicationIntent::Activate => proxy.call_method("Activate", &(data,)).await,
        ApplicationIntent::Open(uris) => proxy.call_method("Open", &(uris, data)).await,
        ApplicationIntent::ActivateAction { name } => {
            proxy
                .call_method("ActivateAction", &(name, Vec::<OwnedValue>::new(), data))
                .await
        }
    };
    result.map(|_| ()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned_string(value: &str) -> OwnedValue {
        OwnedValue::from(zbus::zvariant::Str::from(value))
    }

    #[test]
    fn oversized_action_parameters_are_refused_before_queueing() {
        let state = Arc::new(Mutex::new(CallState::default()));
        let object = ApplicationObject {
            state: Arc::clone(&state),
            completion: CompletionSignal::new(),
        };
        let oversized = "x".repeat(APPLICATION_PARAMETER_BYTES + 1);
        assert!(matches!(
            object.enqueue(
                ApplicationIntent::ActivateAction {
                    name: "open".into(),
                },
                vec![owned_string(&oversized)],
                HashMap::new(),
            ),
            Err(zbus::fdo::Error::InvalidArgs(_))
        ));
        assert!(lock(&state).queue.is_empty());
    }
}
