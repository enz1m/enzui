//! Owned activation context for child launches, portals and existing-instance routing.
//!
//! Received tokens are redacted, expiring and moved through explicit handoffs.
//! They are desktop focus policy inputs, not application identities or guarantees
//! of focus. Feature `activation-native-iced` adds native request and window handoff.

use iced_core::window::Id as WindowId;

#[doc(inline)]
pub use iced_core::window::ActivationPlatform;
use iced_runtime::Task;
use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

#[cfg(feature = "application-dbus")]
pub mod dbus;

/// The object path an app serves its D-Bus interfaces at: its ID with dots
/// as slashes and dashes as underscores, `/org/enzui/Mail` for
/// `org.enzui.Mail`.
#[must_use]
pub fn application_object_path(application_id: &str) -> String {
    format!("/{}", application_id.replace('.', "/").replace('-', "_"))
}
#[cfg(feature = "application-dbus")]
pub mod lifecycle;

/// The default lifetime allowed for a token handoff.
pub const DEFAULT_TOKEN_LIFETIME: Duration = Duration::from_secs(10);
/// Maximum simultaneous native token requests owned by one broker.
pub const DEFAULT_PENDING_LIMIT: usize = 32;
/// Maximum activation-token bytes retained from an IPC transport.
pub const MAX_ACTIVATION_TOKEN_BYTES: usize = 4096;

/// Monotonic identity of one activation request or inbound handoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ActivationGeneration(u64);

impl ActivationGeneration {
    /// Returns the process-local generation number.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Opaque activation context accepted from an IPC transport.
///
/// Conversions from strings preserve the Wayland compatibility convention.
/// Use [`Self::for_platform`] for X11 and [`Self::native`] for an untyped native signal.
#[derive(PartialEq, Eq)]
pub struct InboundActivationToken(iced_core::window::ActivationToken);

impl From<String> for InboundActivationToken {
    fn from(raw: String) -> Self {
        Self::for_platform(ActivationPlatform::Wayland, raw)
    }
}

impl From<&str> for InboundActivationToken {
    fn from(raw: &str) -> Self {
        Self::from(raw.to_owned())
    }
}

impl InboundActivationToken {
    /// Captures both startup variables without clearing the process environment.
    ///
    /// Empty, unset and non-UTF-8 variables are absent. Capture before entering
    /// the GUI runtime, which consumes its native startup environment.
    ///
    /// # Errors
    ///
    /// Context exceeding [`MAX_ACTIVATION_TOKEN_BYTES`] is refused.
    pub fn from_env() -> Result<Option<Self>, InboundTokenError> {
        let read = |name| std::env::var(name).ok().filter(|value| !value.is_empty());
        Self::from_platform_data(read("XDG_ACTIVATION_TOKEN"), read("DESKTOP_STARTUP_ID"))
    }

    /// Retains opaque context for one explicitly identified protocol.
    #[must_use]
    pub fn for_platform(platform: ActivationPlatform, raw: String) -> Self {
        Self(iced_core::window::ActivationToken::for_platform(
            platform, raw,
        ))
    }

    /// Retains a native notification token whose signal does not identify its protocol.
    ///
    /// Native windows select their own protocol. Select a platform explicitly
    /// before forwarding this value over D-Bus or passing it to a child.
    #[must_use]
    pub fn native(raw: String) -> Self {
        Self(iced_core::window::ActivationToken::from_raw(raw))
    }

    /// Validates the standard Wayland and X11 fields of desktop platform data.
    ///
    /// Both fields are retained when present; their combined size is bounded.
    ///
    /// # Errors
    ///
    /// Empty fields and context larger than [`MAX_ACTIVATION_TOKEN_BYTES`] are refused.
    pub fn from_platform_data(
        wayland: Option<String>,
        x11: Option<String>,
    ) -> Result<Option<Self>, InboundTokenError> {
        let token = iced_core::window::ActivationToken::from_platform_data(wayland, x11).map(Self);
        if let Some(token) = &token {
            token.validate()?;
        }
        Ok(token)
    }

    /// The Wayland value for a trusted IPC handoff.
    #[must_use]
    pub fn xdg_activation_token(&self) -> Option<&str> {
        self.0.xdg_activation_token()
    }

    /// The X11 value for a trusted IPC handoff.
    #[must_use]
    pub fn desktop_startup_id(&self) -> Option<&str> {
        self.0.desktop_startup_id()
    }

    /// Selects one protocol, discarding any alternative field.
    ///
    /// An untyped native value is bound to the explicit platform. Returns
    /// `None` when context explicitly belongs only to the other protocol.
    #[must_use]
    pub fn select_platform(self, platform: ActivationPlatform) -> Option<Self> {
        self.0
            .into_raw_for(platform)
            .map(|raw| Self::for_platform(platform, raw))
    }

    fn raw_values(&self) -> [Option<&str>; 3] {
        [
            self.0.xdg_activation_token(),
            self.0.desktop_startup_id(),
            self.0.untyped_token(),
        ]
    }

    pub(crate) fn byte_len(&self) -> usize {
        self.raw_values().into_iter().flatten().map(str::len).sum()
    }

    fn validate(&self) -> Result<(), InboundTokenError> {
        validate_token_values(self.raw_values().into_iter().flatten())
    }

    #[cfg(feature = "portal-dbus")]
    fn as_raw(&self) -> &str {
        self.0
            .untyped_token()
            .or_else(|| self.0.xdg_activation_token())
            .or_else(|| self.0.desktop_startup_id())
            .unwrap_or_default()
    }

    /// Exposes one raw value, preferring Wayland in combined platform data.
    ///
    /// Use [`Self::select_platform`] before a protocol-specific handoff.
    #[must_use]
    pub fn into_raw(self) -> String {
        self.0.into_raw()
    }
}

#[cfg(any(feature = "application-dbus", feature = "portal-dbus"))]
pub(crate) fn inbound_platform_data(
    data: &HashMap<String, zbus::zvariant::OwnedValue>,
) -> Result<Option<InboundActivationToken>, InboundTokenError> {
    let read = |key| {
        data.get(key)
            .map(|value| <&str>::try_from(value).map_err(|_| InboundTokenError::Malformed))
            .transpose()
    };
    let wayland = read("activation-token")?;
    let x11 = read("desktop-startup-id")?;
    validate_token_values([wayland, x11].into_iter().flatten())?;
    InboundActivationToken::from_platform_data(wayland.map(str::to_owned), x11.map(str::to_owned))
}

impl fmt::Debug for InboundActivationToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("InboundActivationToken([redacted])")
    }
}

/// Failure reported by the native activation adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeActivationError {
    /// The requested window no longer exists.
    WindowClosed,
    /// The native backend does not support the activation operation or context protocol.
    Unsupported,
    /// The bounded native request queue is full.
    Busy,
    /// The native request did not complete before its deadline.
    Expired,
}

#[cfg(feature = "activation-native-iced")]
impl From<iced_core::window::ActivationError> for NativeActivationError {
    fn from(error: iced_core::window::ActivationError) -> Self {
        match error {
            iced_core::window::ActivationError::WindowClosed => Self::WindowClosed,
            iced_core::window::ActivationError::Unsupported => Self::Unsupported,
            iced_core::window::ActivationError::Busy => Self::Busy,
            iced_core::window::ActivationError::Expired => Self::Expired,
        }
    }
}

/// A token request completed by the iced window runtime.
pub struct ActivationResponse {
    generation: ActivationGeneration,
    source_window: WindowId,
    result: Result<InboundActivationToken, NativeActivationError>,
}

impl fmt::Debug for ActivationResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActivationResponse")
            .field("generation", &self.generation)
            .field("source_window", &self.source_window)
            .field("result", &self.result.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

#[derive(Debug)]
struct PendingRequest {
    source_window: WindowId,
    expires_at: Instant,
    abort: iced_runtime::task::Handle,
}

/// Refusal to begin another token request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationRequestError {
    /// The configured pending-request bound has been reached.
    Busy,
    /// The generation counter cannot advance without reusing an identity.
    GenerationExhausted,
}

/// Why a native token response did not become usable launch context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationResponseError {
    /// No live request owns this response generation.
    StaleGeneration,
    /// The response named a different source window than its request.
    WrongWindow,
    /// The local handoff deadline passed before the response was accepted.
    Expired,
    /// The native backend refused or could not start the request.
    Native(NativeActivationError),
    /// The returned token violated the retained transport bound.
    InvalidToken(InboundTokenError),
}

/// Why an activation context could not be handed off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationHandoffError {
    /// The local handoff deadline has passed.
    Expired,
    /// A native token must be bound to an explicit protocol before a cross-process handoff.
    UnspecifiedPlatform,
    /// The retained context belongs only to the other window protocol.
    WrongPlatform,
}

/// One generation-bound token ready for exactly one handoff.
pub struct ActivationContext {
    generation: ActivationGeneration,
    source_window: Option<WindowId>,
    expires_at: Instant,
    token: InboundActivationToken,
}

impl ActivationContext {
    /// Returns the generation used to reject stale completions.
    #[must_use]
    pub fn generation(&self) -> ActivationGeneration {
        self.generation
    }

    /// Returns the source window for a locally generated token.
    #[must_use]
    pub fn source_window(&self) -> Option<WindowId> {
        self.source_window
    }

    /// Returns whether the local handoff deadline has passed.
    #[must_use]
    pub fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires_at
    }

    /// The startup variables a launched desktop handler receives from this
    /// context: `XDG_ACTIVATION_TOKEN` (Wayland) and `DESKTOP_STARTUP_ID`
    /// (X11). Empty once the handoff deadline has passed or for an untyped
    /// native token, which has no defined environment form.
    pub(crate) fn launch_environment(&self, now: Instant) -> Vec<(&'static str, String)> {
        if self.is_expired(now) || self.token.0.untyped_token().is_some() {
            return Vec::new();
        }
        let mut pairs = Vec::new();
        if let Some(value) = self.token.xdg_activation_token() {
            pairs.push(("XDG_ACTIVATION_TOKEN", value.to_owned()));
        }
        if let Some(value) = self.token.desktop_startup_id() {
            pairs.push(("DESKTOP_STARTUP_ID", value.to_owned()));
        }
        pairs
    }

    #[cfg(feature = "portal-dbus")]
    pub(crate) fn token_raw(&self) -> &str {
        self.token.as_raw()
    }

    #[cfg(feature = "portal-dbus")]
    pub(crate) fn expires_at(&self) -> Instant {
        self.expires_at
    }

    /// Consumes this expiring context for one trusted transport handoff.
    ///
    /// # Errors
    ///
    /// [`ActivationHandoffError::Expired`] after the handoff deadline.
    pub fn into_token(
        self,
        now: Instant,
    ) -> Result<InboundActivationToken, ActivationHandoffError> {
        if self.is_expired(now) {
            Err(ActivationHandoffError::Expired)
        } else {
            Ok(self.token)
        }
    }

    /// Selects one native protocol without extending the handoff deadline.
    ///
    /// Use this to bind an untyped notification token before a child or D-Bus handoff.
    ///
    /// # Errors
    ///
    /// [`ActivationHandoffError::WrongPlatform`] if context names only the other protocol.
    pub fn select_platform(
        mut self,
        platform: ActivationPlatform,
    ) -> Result<Self, ActivationHandoffError> {
        self.token = self
            .token
            .select_platform(platform)
            .ok_or(ActivationHandoffError::WrongPlatform)?;
        Ok(self)
    }

    /// Creates a window with this context as its one startup handoff.
    ///
    /// # Errors
    ///
    /// [`ActivationHandoffError::Expired`] when the handoff deadline has passed.
    #[cfg(feature = "activation-native-iced")]
    pub fn open_window(
        self,
        settings: iced_core::window::Settings,
        now: Instant,
    ) -> Result<(WindowId, Task<WindowId>), ActivationHandoffError> {
        self.into_token(now)
            .map(|token| iced_runtime::window::open_with_activation(settings, token.0))
    }

    /// Submits this context to activate an existing iced window.
    ///
    /// `Ok(())` means the request reached the native backend. The compositor
    /// may still deny focus according to its own policy.
    #[cfg(feature = "activation-native-iced")]
    pub fn activate(self, target: WindowId, now: Instant) -> Task<Result<(), ActivateWindowError>> {
        match self.into_token(now) {
            Ok(token) => iced_runtime::window::activate(target, token.0)
                .map(|result| result.map_err(|error| ActivateWindowError::Native(error.into()))),
            Err(error) => Task::done(Err(ActivateWindowError::Handoff(error))),
        }
    }
}

impl fmt::Debug for ActivationContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ActivationContext")
            .field("generation", &self.generation)
            .field("source_window", &self.source_window)
            .field("expires_at", &self.expires_at)
            .field("token", &"[redacted]")
            .finish()
    }
}

/// Failure to activate an existing window with received context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivateWindowError {
    /// The context was no longer eligible for handoff.
    Handoff(ActivationHandoffError),
    /// The native window operation could not be submitted.
    Native(NativeActivationError),
}

/// Owner of generation assignment, expiry, and response correlation.
#[derive(Debug)]
pub struct ActivationBroker {
    next_generation: u64,
    lifetime: Duration,
    #[allow(
        dead_code,
        reason = "read only with the `activation-native-iced` feature"
    )]
    pending_limit: usize,
    pending: BTreeMap<ActivationGeneration, PendingRequest>,
}

impl Default for ActivationBroker {
    fn default() -> Self {
        Self::new(DEFAULT_TOKEN_LIFETIME, DEFAULT_PENDING_LIMIT)
    }
}

impl Drop for ActivationBroker {
    fn drop(&mut self) {
        for request in self.pending.values() {
            request.abort.abort();
        }
    }
}

impl ActivationBroker {
    /// Creates a broker with explicit handoff lifetime and pending bound.
    #[must_use]
    pub fn new(lifetime: Duration, pending_limit: usize) -> Self {
        Self {
            next_generation: 0,
            lifetime,
            pending_limit,
            pending: BTreeMap::new(),
        }
    }

    fn next_generation(&mut self) -> Result<ActivationGeneration, ActivationRequestError> {
        self.next_generation = self
            .next_generation
            .checked_add(1)
            .ok_or(ActivationRequestError::GenerationExhausted)?;
        Ok(ActivationGeneration(self.next_generation))
    }

    #[allow(
        dead_code,
        reason = "called only with the `activation-native-iced` feature"
    )]
    fn request_with(
        &mut self,
        source_window: WindowId,
        now: Instant,
        build_task: impl FnOnce(ActivationGeneration) -> Task<ActivationResponse>,
    ) -> Result<(ActivationGeneration, Task<ActivationResponse>), ActivationRequestError> {
        self.prune(now);
        if self.pending.len() >= self.pending_limit {
            return Err(ActivationRequestError::Busy);
        }

        let generation = self.next_generation()?;
        let expires_at = now.checked_add(self.lifetime).unwrap_or(now);
        let (task, abort) = build_task(generation).abortable();
        self.pending.insert(
            generation,
            PendingRequest {
                source_window,
                expires_at,
                abort,
            },
        );

        Ok((generation, task))
    }

    /// Starts a native request tied to `source_window`.
    ///
    /// Call this from the update handling the eligible user gesture. A later
    /// background callback has no equivalent interaction context.
    /// Successful replies retain the source window's protocol for later handoffs.
    ///
    /// # Errors
    ///
    /// [`ActivationRequestError::Busy`] at the pending-request bound, and `GenerationExhausted`
    /// when no fresh generation is left.
    #[cfg(feature = "activation-native-iced")]
    pub fn request(
        &mut self,
        source_window: WindowId,
        now: Instant,
    ) -> Result<(ActivationGeneration, Task<ActivationResponse>), ActivationRequestError> {
        self.request_with(source_window, now, move |generation| {
            iced_runtime::window::request_activation_token(source_window).map(move |result| {
                ActivationResponse {
                    generation,
                    source_window,
                    result: result.map(InboundActivationToken).map_err(Into::into),
                }
            })
        })
    }

    /// Cancels one generation before it is handed off.
    pub fn cancel(&mut self, generation: ActivationGeneration) -> bool {
        let Some(request) = self.pending.remove(&generation) else {
            return false;
        };
        request.abort.abort();
        true
    }

    /// Removes expired requests and aborts their asynchronous result streams.
    pub fn prune(&mut self, now: Instant) -> usize {
        let before = self.pending.len();
        self.pending.retain(|_, request| {
            let live = now < request.expires_at;
            if !live {
                request.abort.abort();
            }
            live
        });
        before - self.pending.len()
    }

    /// Earliest deadline an application should schedule as a one-shot wake.
    #[must_use]
    pub fn next_expiry(&self) -> Option<Instant> {
        self.pending
            .values()
            .map(|request| request.expires_at)
            .min()
    }

    /// Correlates one asynchronous response and yields single-use context.
    ///
    /// # Errors
    ///
    /// An [`ActivationResponseError`] for a stale generation, the wrong window, an expired request,
    /// a native failure, or an invalid token.
    pub fn finish(
        &mut self,
        response: ActivationResponse,
        now: Instant,
    ) -> Result<ActivationContext, ActivationResponseError> {
        let Some(request) = self.pending.remove(&response.generation) else {
            return Err(ActivationResponseError::StaleGeneration);
        };
        if request.source_window != response.source_window {
            return Err(ActivationResponseError::WrongWindow);
        }
        if now >= request.expires_at {
            return Err(ActivationResponseError::Expired);
        }
        let token = response.result.map_err(ActivationResponseError::Native)?;
        token
            .validate()
            .map_err(ActivationResponseError::InvalidToken)?;

        Ok(ActivationContext {
            generation: response.generation,
            source_window: Some(response.source_window),
            expires_at: request.expires_at,
            token,
        })
    }

    /// Admits an activation token received through a trusted IPC boundary.
    ///
    /// An application id or D-Bus sender name is routing metadata, not token
    /// authority. Empty and oversized values are refused before allocation into
    /// a handoff. This compatibility entry point treats the raw value as Wayland;
    /// use [`Self::admit_token`] for X11 or combined platform data.
    ///
    /// # Errors
    ///
    /// An [`InboundTokenError`] when the token is empty or oversized, or no fresh generation is
    /// left.
    pub fn admit_inbound(
        &mut self,
        raw_token: String,
        now: Instant,
    ) -> Result<ActivationContext, InboundTokenError> {
        self.admit_token(InboundActivationToken::from(raw_token), now)
    }

    /// Admits bounded context while retaining its protocol fields.
    ///
    /// # Errors
    ///
    /// As [`Self::admit_inbound`], including the combined size bound for two fields.
    pub fn admit_token(
        &mut self,
        token: InboundActivationToken,
        now: Instant,
    ) -> Result<ActivationContext, InboundTokenError> {
        token.validate()?;
        let generation = self
            .next_generation()
            .map_err(|_| InboundTokenError::GenerationExhausted)?;
        Ok(ActivationContext {
            generation,
            source_window: None,
            expires_at: now.checked_add(self.lifetime).unwrap_or(now),
            token,
        })
    }

    /// Admits optional platform context from an application or notification callback.
    ///
    /// `None` remains tokenless instead of manufacturing focus authority.
    ///
    /// # Errors
    ///
    /// As [`Self::admit_inbound`], for a present token.
    pub fn admit_optional_inbound(
        &mut self,
        raw_token: Option<String>,
        now: Instant,
    ) -> Result<Option<ActivationContext>, InboundTokenError> {
        raw_token
            .map(|token| self.admit_inbound(token, now))
            .transpose()
    }

    /// Moves portal notification activation data into one bounded context.
    ///
    /// # Errors
    ///
    /// As [`Self::admit_inbound`], for a context carrying a token.
    pub fn admit_notification_context(
        &mut self,
        context: crate::portal::notification::NotificationActivationContext,
        received_at: Instant,
    ) -> Result<Option<ActivationContext>, InboundTokenError> {
        use crate::portal::notification::NotificationActivationContext;

        match context {
            NotificationActivationContext::XdgActivationToken(token)
            | NotificationActivationContext::PlatformData(token) => {
                self.admit_token(token, received_at).map(Some)
            }
            NotificationActivationContext::Unavailable => Ok(None),
        }
    }

    /// Takes activation authority from an already validated notification action.
    ///
    /// # Errors
    ///
    /// [`InboundTokenError::Expired`] when the action's context outlived the handoff lifetime.
    pub fn admit_notification_invocation(
        &mut self,
        invocation: &mut crate::portal::notification::NotificationActionInvocation,
        now: Instant,
    ) -> Result<Option<ActivationContext>, InboundTokenError> {
        let context = std::mem::replace(
            &mut invocation.activation_context,
            crate::portal::notification::NotificationActivationContext::Unavailable,
        );
        match self.admit_notification_context(context, invocation.received_at)? {
            Some(context) if context.is_expired(now) => Err(InboundTokenError::Expired),
            context => Ok(context),
        }
    }

    /// Number of locally requested generations awaiting a response.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

fn validate_token_values<'a>(
    values: impl IntoIterator<Item = &'a str>,
) -> Result<(), InboundTokenError> {
    let mut retained = 0;
    for token in values {
        validate_token(token)?;
        retained += token.len();
        if retained > MAX_ACTIVATION_TOKEN_BYTES {
            return Err(InboundTokenError::TooLong);
        }
    }
    Ok(())
}

fn validate_token(token: &str) -> Result<(), InboundTokenError> {
    if token.is_empty() {
        Err(InboundTokenError::Empty)
    } else if token.len() > MAX_ACTIVATION_TOKEN_BYTES {
        Err(InboundTokenError::TooLong)
    } else {
        Ok(())
    }
}

/// Rejection of a token received through IPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundTokenError {
    /// The transport value was not a string.
    Malformed,
    /// Empty strings carry no activation authority.
    Empty,
    /// The transport value exceeded the defensive size bound.
    TooLong,
    /// The generation counter cannot advance without reuse.
    GenerationExhausted,
    /// The transport held the token beyond the local handoff lifetime.
    Expired,
}

/// Executable lookup authority for a child launch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ExecutableAuthority {
    /// Requires the program supplied to [`ChildLaunch::new`] to be absolute.
    #[default]
    AbsoluteOnly,
    /// Resolves one bare program name against these absolute directories.
    ///
    /// Resolution happens before the child's working directory is applied. An
    /// empty list, a relative directory, or a program containing path
    /// components is refused.
    SearchPath(Vec<PathBuf>),
}

#[derive(Clone)]
enum ChildEnvironmentValue {
    Explicit(OsString),
    Inherit,
}

/// A child process launch with explicit executable, environment, and activation authority.
pub struct ChildLaunch {
    program: OsString,
    executable_authority: ExecutableAuthority,
    arguments: Vec<OsString>,
    current_dir: Option<PathBuf>,
    environment: Vec<(OsString, ChildEnvironmentValue)>,
    context: Option<ActivationContext>,
}

impl ChildLaunch {
    /// Creates a launch requiring an absolute executable and clearing the child environment.
    #[must_use]
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            program: program.as_ref().to_owned(),
            executable_authority: ExecutableAuthority::AbsoluteOnly,
            arguments: Vec::new(),
            current_dir: None,
            environment: Vec::new(),
            context: None,
        }
    }

    /// Selects how a non-absolute program may be resolved.
    #[must_use]
    pub fn executable_authority(mut self, authority: ExecutableAuthority) -> Self {
        self.executable_authority = authority;
        self
    }

    /// Appends one argv entry without shell interpretation.
    #[must_use]
    pub fn arg(mut self, argument: impl AsRef<OsStr>) -> Self {
        self.arguments.push(argument.as_ref().to_owned());
        self
    }

    /// Appends argv entries without shell interpretation.
    #[must_use]
    pub fn args(mut self, arguments: impl IntoIterator<Item = impl AsRef<OsStr>>) -> Self {
        self.arguments.extend(
            arguments
                .into_iter()
                .map(|argument| argument.as_ref().to_owned()),
        );
        self
    }

    /// Sets the child's working directory after executable resolution.
    #[must_use]
    pub fn current_dir(mut self, directory: impl AsRef<Path>) -> Self {
        self.current_dir = Some(directory.as_ref().to_owned());
        self
    }

    /// Sets one explicit child environment value in the otherwise cleared environment.
    ///
    /// `XDG_ACTIVATION_TOKEN` and `DESKTOP_STARTUP_ID` are reserved for typed
    /// activation handoff and cause [`ChildLaunchError::EnvironmentAuthority`]
    /// at spawn time.
    #[must_use]
    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.set_environment(
            key.as_ref().to_owned(),
            ChildEnvironmentValue::Explicit(value.as_ref().to_owned()),
        );
        self
    }

    /// Allowlists one variable to copy from the parent into the cleared environment.
    ///
    /// An absent parent variable remains absent. Activation variables remain
    /// reserved for typed handoff.
    #[must_use]
    pub fn inherit_env(mut self, key: impl AsRef<OsStr>) -> Self {
        self.set_environment(key.as_ref().to_owned(), ChildEnvironmentValue::Inherit);
        self
    }

    fn set_environment(&mut self, key: OsString, value: ChildEnvironmentValue) {
        if let Some((_, existing)) = self
            .environment
            .iter_mut()
            .find(|(existing, _)| environment_keys_equal(existing, &key))
        {
            *existing = value;
        } else {
            self.environment.push((key, value));
        }
    }

    /// Attaches one context, replacing and dropping any previous token.
    #[must_use]
    pub fn activation(mut self, context: ActivationContext) -> Self {
        self.context = Some(context);
        self
    }

    /// Spawns the child with only explicitly supplied or allowlisted environment values.
    ///
    /// Live context installs `XDG_ACTIVATION_TOKEN` for Wayland and
    /// `DESKTOP_STARTUP_ID` for X11 only in this child's environment. Combined
    /// platform data retains both fields. Untyped native notification tokens
    /// require [`ActivationContext::select_platform`] before this handoff.
    /// The child must consume its context before launching unrelated descendants.
    ///
    /// # Errors
    ///
    /// Returns [`ChildLaunchError::ExecutableAuthority`] before process creation
    /// when a non-absolute program lacks a valid explicit search policy. Returns
    /// [`ChildLaunchError::EnvironmentAuthority`] when a generic environment
    /// method names an activation variable, and propagates activation expiry or
    /// process-creation failures through their corresponding variants.
    pub fn spawn(self, now: Instant) -> Result<Child, ChildLaunchError> {
        let program = resolve_child_executable(&self.program, &self.executable_authority)?;
        if self
            .environment
            .iter()
            .any(|(key, _)| is_activation_environment_key(key))
        {
            return Err(ChildLaunchError::EnvironmentAuthority);
        }
        let token = self
            .context
            .map(|context| {
                context
                    .into_token(now)
                    .and_then(|token| {
                        if token.0.untyped_token().is_some() {
                            Err(ActivationHandoffError::UnspecifiedPlatform)
                        } else {
                            Ok(token)
                        }
                    })
                    .map_err(ChildLaunchError::Activation)
            })
            .transpose()?;

        let mut command = Command::new(program);
        command.args(&self.arguments).env_clear();
        for (key, value) in self.environment {
            match value {
                ChildEnvironmentValue::Explicit(value) => {
                    command.env(key, value);
                }
                ChildEnvironmentValue::Inherit => {
                    if let Some(value) = std::env::var_os(&key) {
                        command.env(key, value);
                    }
                }
            }
        }
        if let Some(token) = token {
            if let Some(value) = token.xdg_activation_token() {
                command.env("XDG_ACTIVATION_TOKEN", value);
            }
            if let Some(value) = token.desktop_startup_id() {
                command.env("DESKTOP_STARTUP_ID", value);
            }
        }
        if let Some(directory) = self.current_dir {
            command.current_dir(directory);
        }
        command.spawn().map_err(ChildLaunchError::Spawn)
    }
}

fn resolve_child_executable(
    program: &OsStr,
    authority: &ExecutableAuthority,
) -> Result<PathBuf, ChildLaunchError> {
    let program_path = Path::new(program);
    if program_path.is_absolute() {
        return Ok(program_path.to_owned());
    }

    let ExecutableAuthority::SearchPath(directories) = authority else {
        return Err(ChildLaunchError::ExecutableAuthority);
    };
    if directories.is_empty()
        || directories.iter().any(|directory| !directory.is_absolute())
        || !is_bare_program(program_path)
    {
        return Err(ChildLaunchError::ExecutableAuthority);
    }

    for directory in directories {
        let candidate = directory.join(program_path);
        match std::fs::metadata(&candidate) {
            Ok(metadata) if executable_candidate(&metadata) => {
                return std::fs::canonicalize(candidate).map_err(ChildLaunchError::Spawn);
            }
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) => {}
            Err(error) => return Err(ChildLaunchError::Spawn(error)),
        }
    }

    Err(ChildLaunchError::Spawn(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        "explicit executable search path contained no matching program",
    )))
}

fn is_bare_program(program: &Path) -> bool {
    let mut components = program.components();
    matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
}

#[cfg(unix)]
fn executable_candidate(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn executable_candidate(metadata: &std::fs::Metadata) -> bool {
    metadata.is_file()
}

#[cfg(windows)]
fn environment_keys_equal(left: &OsStr, right: &OsStr) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(not(windows))]
fn environment_keys_equal(left: &OsStr, right: &OsStr) -> bool {
    left == right
}

fn is_activation_environment_key(key: &OsStr) -> bool {
    environment_keys_equal(key, OsStr::new("XDG_ACTIVATION_TOKEN"))
        || environment_keys_equal(key, OsStr::new("DESKTOP_STARTUP_ID"))
}

impl fmt::Debug for ChildLaunch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChildLaunch")
            .field("program", &self.program)
            .field("executable_authority", &self.executable_authority)
            .field("arguments", &self.arguments)
            .field("current_dir", &self.current_dir)
            .field(
                "environment",
                &format_args!("<{} values redacted>", self.environment.len()),
            )
            .field("has_activation", &self.context.is_some())
            .finish()
    }
}

/// Failure to prepare or spawn an explicit child launch.
#[derive(Debug)]
pub enum ChildLaunchError {
    /// The executable was neither absolute nor authorized by a valid explicit search path.
    ExecutableAuthority,
    /// A generic environment method attempted to set or inherit an activation variable.
    EnvironmentAuthority,
    /// The context expired or needs an explicit window protocol before spawning.
    Activation(ActivationHandoffError),
    /// Process creation or explicit executable resolution failed.
    Spawn(std::io::Error),
}

/// Existing-instance request received from a desktop launcher or action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplicationIntent {
    /// Activate the application's primary window.
    Activate,
    /// Open the supplied URI list in an existing window.
    Open(Vec<String>),
    /// Invoke a named desktop or notification action.
    ActivateAction {
        /// Desktop action name.
        name: String,
    },
}

/// What a launch's command line asks for: the [`ApplicationIntent`] for its
/// files and URIs, and the options the application reads itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRequest {
    /// [`ApplicationIntent::Open`] with every file and URI named, or
    /// [`ApplicationIntent::Activate`] when none is.
    pub intent: ApplicationIntent,
    /// Arguments starting with `-` before a `--`, in order, for the
    /// application's own flags.
    pub options: Vec<OsString>,
}

/// Read a launch's arguments (without the program name) as a desktop entry's
/// `%U` or `%F` passes them: URIs (`https://…`, `mailto:…`, `file:///…`)
/// stay as they are, and file paths become `file://` URIs, made absolute
/// against `cwd`. Arguments starting with `-` are options for the
/// application, except `-` itself and everything after `--`.
///
/// Hand the intent to `ApplicationService::connect_or_forward`: a second
/// launch then opens its files in the running instance.
#[must_use]
pub fn launch_request<I>(args: I, cwd: &Path) -> LaunchRequest
where
    I: IntoIterator,
    I::Item: Into<OsString>,
{
    let mut uris = Vec::new();
    let mut options = Vec::new();
    let mut positional_only = false;
    for arg in args {
        let arg: OsString = arg.into();
        if !positional_only {
            if arg == "--" {
                positional_only = true;
                continue;
            }
            if arg.as_encoded_bytes().first() == Some(&b'-') && arg != "-" {
                options.push(arg);
                continue;
            }
        }
        uris.push(match arg.to_str().filter(|text| has_uri_scheme(text)) {
            Some(uri) => uri.to_owned(),
            None => file_uri(&cwd.join(&arg)),
        });
    }
    let intent = if uris.is_empty() {
        ApplicationIntent::Activate
    } else {
        ApplicationIntent::Open(uris)
    };
    LaunchRequest { intent, options }
}

/// [`launch_request`] for this process: its arguments after the program
/// name, against its working directory (`/` when that is unreadable).
#[must_use]
pub fn launch_request_from_env() -> LaunchRequest {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
    launch_request(std::env::args_os().skip(1), &cwd)
}

/// Whether `text` starts with an RFC 3986 scheme and a colon (`https:`,
/// `mailto:`), at least two characters so it is never a lone letter.
fn has_uri_scheme(text: &str) -> bool {
    let Some((scheme, _)) = text.split_once(':') else {
        return false;
    };
    scheme.len() >= 2
        && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// A `file://` URI for `path`, its bytes percent-encoded except the
/// unreserved characters and `/`.
fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'.' | b'_' | b'~') {
            uri.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}

/// An existing-instance request bound to one live window.
pub struct RoutedActivation {
    /// The selected existing window.
    pub window: WindowId,
    /// The application intent to dispatch to that window.
    pub intent: ApplicationIntent,
    /// Optional compositor context from platform data.
    pub context: Option<ActivationContext>,
}

impl fmt::Debug for RoutedActivation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RoutedActivation")
            .field("window", &self.window)
            .field("intent", &self.intent)
            .field("has_context", &self.context.is_some())
            .finish()
    }
}

/// Result of routing a desktop application call.
pub enum ApplicationRoute {
    /// A live window already owns the request.
    Existing(RoutedActivation),
    /// No live window exists; the application may create one with the context.
    Cold {
        /// Intent that launched the application.
        intent: ApplicationIntent,
        /// Optional startup context retained for the first window or child handoff.
        context: Option<ActivationContext>,
    },
}

impl fmt::Debug for ApplicationRoute {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Existing(routed) => formatter.debug_tuple("Existing").field(routed).finish(),
            Self::Cold { intent, context } => formatter
                .debug_struct("Cold")
                .field("intent", intent)
                .field("has_context", &context.is_some())
                .finish(),
        }
    }
}

/// Registry used to route desktop activation to existing application windows.
#[derive(Debug, Default)]
pub struct ExistingInstanceRouter {
    primary: Option<WindowId>,
    resources: HashMap<String, WindowId>,
}

impl ExistingInstanceRouter {
    /// Selects the fallback window for Activate and previously unseen URIs.
    pub fn set_primary(&mut self, window: WindowId) {
        self.primary = Some(window);
    }

    /// Associates an exact URI with the window that already owns it.
    pub fn bind_uri(&mut self, uri: impl Into<String>, window: WindowId) {
        self.resources.insert(uri.into(), window);
    }

    /// Removes all routing entries for a closed window.
    pub fn window_closed(&mut self, window: WindowId) {
        if self.primary == Some(window) {
            self.primary = None;
        }
        self.resources.retain(|_, candidate| *candidate != window);
    }

    /// Routes to an exact URI owner first, then the live primary window.
    pub fn route(
        &mut self,
        intent: ApplicationIntent,
        context: Option<ActivationContext>,
        is_live: impl Fn(WindowId) -> bool,
    ) -> ApplicationRoute {
        self.resources.retain(|_, window| is_live(*window));
        if self.primary.is_some_and(|window| !is_live(window)) {
            self.primary = None;
        }

        let target = match &intent {
            ApplicationIntent::Open(uris) => uris
                .iter()
                .find_map(|uri| self.resources.get(uri).copied())
                .or(self.primary),
            ApplicationIntent::Activate | ApplicationIntent::ActivateAction { .. } => self.primary,
        };

        match target {
            Some(window) => ApplicationRoute::Existing(RoutedActivation {
                window,
                intent,
                context,
            }),
            None => ApplicationRoute::Cold { intent, context },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn write_executable_script(path: &Path, source: &str) {
        use std::os::unix::fs::PermissionsExt;

        std::fs::write(path, source).expect("write child-launch fixture");
        let mut permissions = std::fs::metadata(path)
            .expect("inspect child-launch fixture")
            .permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(path, permissions).expect("make child-launch fixture executable");
    }

    #[cfg(unix)]
    fn wait_child_bounded(mut child: Child, context: &str) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().expect("query child-launch fixture") {
                return status;
            }
            if Instant::now() >= deadline {
                child
                    .kill()
                    .expect("terminate timed-out child-launch fixture");
                let reap_deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    if child
                        .try_wait()
                        .expect("reap timed-out child-launch fixture")
                        .is_some()
                    {
                        break;
                    }
                    assert!(
                        Instant::now() < reap_deadline,
                        "{context}: killed child was not reaped within the cleanup bound"
                    );
                    std::thread::sleep(Duration::from_millis(5));
                }
                panic!("{context}: child exceeded its five-second deadline");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[cfg(unix)]
    #[test]
    fn child_launch_refuses_path_substitution_without_explicit_resolution() {
        use enzui_test_support::test_scratch::ScratchDir;

        if let Some(substitute) = std::env::var_os("ENZUI_CHILD_LAUNCH_PATH_SUBSTITUTE") {
            assert!(
                Path::new(&substitute).is_file(),
                "the ambient PATH substitute positive control exists"
            );
            let default = ChildLaunch::new("viewer")
                .spawn(Instant::now())
                .expect_err("the default launch has no ambient PATH authority");
            assert!(matches!(default, ChildLaunchError::ExecutableAuthority));

            let allowlisted_path = ChildLaunch::new("viewer")
                .inherit_env("PATH")
                .spawn(Instant::now())
                .expect_err("allowlisting PATH does not select executable resolution");
            assert!(matches!(
                allowlisted_path,
                ChildLaunchError::ExecutableAuthority
            ));
            return;
        }

        let scratch = ScratchDir::new("activation-path-refusal");
        let substitute = scratch.path().join("viewer");
        write_executable_script(&substitute, "#!/bin/sh\nexit 41\n");
        let mut command = Command::new(std::env::current_exe().expect("locate unit-test binary"));
        command
            .args([
                "--exact",
                "activation::tests::child_launch_refuses_path_substitution_without_explicit_resolution",
                "--nocapture",
            ])
            .env("PATH", scratch.path())
            .env("ENZUI_CHILD_LAUNCH_PATH_SUBSTITUTE", &substitute);
        let child = command
            .spawn()
            .expect("spawn executable-authority fixture process");
        let status = wait_child_bounded(child, "executable-authority fixture process");
        assert!(status.success(), "nested fixture assertions must pass");
    }
}
