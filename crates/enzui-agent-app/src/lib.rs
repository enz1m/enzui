//! The agent-process half every installed enzUI app shares.
//!
//! An app an agent can use while no window is open runs as one process:
//!
//! - `--agent-background` (the package's user unit) starts the app's own
//!   state and reducer with no display, renderer or window, through
//!   [`enzui_agent::center::background::run`];
//! - a desktop launch while that worker runs is forwarded to it over
//!   `org.freedesktop.Application`, and the worker opens its window over the
//!   state the agent left instead of a second process starting;
//! - a background launch while a window is open exits at once: that window
//!   already serves the broker.
//!
//! This crate is the glue between those pieces, so an app writes only what is
//! its own: how it boots, what keeps it busy and what it presents.
//! [`AgentProcess`] is the per-process state an app keeps beside its domain
//! model; [`claim_desktop`] and [`claim_background`] take or reuse the single
//! instance; [`print_manifest`] answers `--export-agent-manifest`; and
//! [`disposition`] is the usual answer to
//! [`BackgroundApp::disposition`]; [`activation_origin`] tells a background
//! start that the OS made for a Search query apart from the others.
//!
//! An installed app runs in its user unit however it was started. Its
//! session D-Bus service file ([`installed::dbus_service`]) makes its own name
//! activatable, so a desktop launch, a `DBusActivatable` desktop entry and a
//! direct run of the installed executable all reach the unit's one process:
//! [`claim_desktop`] hands the launch to the bus, which starts the unit when
//! nothing runs. That needs no agent credential; the credential only connects
//! the worker to the broker.
//!
//! A build that is not the installed executable (development, visual QA)
//! claims the single instance only when it is provisioned for agent use, and
//! otherwise stays an independent process.

#![cfg_attr(docsrs, feature(doc_cfg))]
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use enzui_agent::center::background::Disposition;
use enzui_agent::center::task_host::TaskHost;
use enzui_agent::center::{
    AppFunctionRegistry, AppMetadata, CenterHost, CenterProvision, CenterStatus, installed,
};
pub use enzui_core::activation::ApplicationIntent;
use enzui_core::activation::dbus::APPLICATION_CALL_CAPACITY;
pub use enzui_core::activation::dbus::ApplicationStart;
pub use enzui_core::activation::dbus::search::{
    SearchActivation, SearchInbox, SearchProvider, SearchQuery, SearchRequest, SearchResult,
    search_provider_ini,
};
pub use enzui_core::activation::dbus::{
    ApplicationCall, ApplicationService, ApplicationServiceError, ApplicationWake,
};
pub use enzui_core::activation::lifecycle::{
    CallerPolicy, LifecycleCallers, StopChoice, StopConsequence, StopControl, StopRequest,
    WorkKind, activity_consequences, activity_work,
};
use enzui_core::activation::{ActivationBroker, InboundActivationToken};
use iced_futures::Subscription;
use iced_runtime::Task;

pub use enzui_agent::center::background::{
    BackgroundApp, BackgroundExit, DEFAULT_IDLE_EXIT, LaunchMode, run as run_background,
};
pub use origin::{ActivationOrigin, activation_origin};

mod origin;

/// The agent-facing process state an app keeps beside its domain model.
pub struct AgentProcess<M: Clone + Send + 'static> {
    service: Option<ApplicationService>,
    activation: ActivationBroker,
    tasks: Option<Arc<TaskHost<M>>>,
    windowless: bool,
    present: Option<ApplicationIntent>,
    stop: Option<StopControl>,
    search: Option<SearchInbox>,
}

impl<M: Clone + Send + 'static> Default for AgentProcess<M> {
    fn default() -> Self {
        Self {
            service: None,
            activation: ActivationBroker::default(),
            tasks: None,
            windowless: false,
            present: None,
            stop: None,
            search: None,
        }
    }
}

impl<M: Clone + Send + 'static> std::fmt::Debug for AgentProcess<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentProcess")
            .field("owns_instance", &self.service.is_some())
            .field("windowless", &self.windowless)
            .field("present", &self.present)
            .finish_non_exhaustive()
    }
}

impl<M: Clone + Send + 'static> AgentProcess<M> {
    /// A desktop process, owning the single instance when it claimed it.
    #[must_use]
    pub fn desktop(service: Option<ApplicationService>) -> Self {
        Self {
            service,
            ..Self::default()
        }
    }

    /// A windowless worker that runs its reducer tasks in `tasks`, owning the
    /// single instance when `service` is given ([`claim_background`]). Tests
    /// and harnesses without a session bus pass `None`.
    #[must_use]
    pub fn windowless(service: Option<ApplicationService>, tasks: Arc<TaskHost<M>>) -> Self {
        Self {
            service,
            tasks: Some(tasks),
            windowless: true,
            ..Self::default()
        }
    }

    /// Release desktop launch, search and lifecycle ownership while retaining reducer tasks.
    ///
    /// Use when a closed window must retire work without receiving new launches.
    /// Pending presentation requests and activation contexts are discarded.
    pub fn release_application(&mut self) {
        self.stop = None;
        self.search = None;
        self.present = None;
        self.activation = ActivationBroker::default();
        self.service = None;
    }

    /// Whether the process has no window.
    #[must_use]
    pub fn is_windowless(&self) -> bool {
        self.windowless
    }

    /// The task host a windowless start ran its reducer tasks in.
    #[must_use]
    pub fn tasks(&self) -> Option<&Arc<TaskHost<M>>> {
        self.tasks.as_ref()
    }

    /// Whether reducer work started without a window is still running or
    /// undelivered.
    #[must_use]
    pub fn has_task_work(&self) -> bool {
        self.tasks.as_ref().is_some_and(|tasks| tasks.has_work())
    }

    /// Asks a windowless worker to open its window. The first request
    /// decides what the window shows; later ones are kept by the window
    /// itself. A desktop process ignores it.
    pub fn request_present(&mut self, intent: ApplicationIntent) {
        if self.windowless {
            self.present.get_or_insert(intent);
        }
    }

    /// Whether a windowless worker has been asked to open its window.
    #[must_use]
    pub fn wants_present(&self) -> bool {
        self.present.is_some()
    }

    /// Whether a windowless worker holds a desktop launch it has not taken
    /// with [`Self::take_application_calls`].
    #[must_use]
    pub fn launch_waiting(&self) -> bool {
        self.windowless
            && self
                .service
                .as_ref()
                .is_some_and(|service| service.queued() > 0)
    }

    /// The handover to the window runtime: the launch the window should
    /// honour first, and the renderer work the windowless start deferred
    /// (fonts, theme queries), which the window runtime completes so the work
    /// waiting on it resumes.
    pub fn begin_presenting<T: Send + 'static>(&mut self) -> (Option<ApplicationIntent>, Task<T>) {
        self.windowless = false;
        let deferred = self
            .tasks
            .as_ref()
            .map_or_else(Task::none, |tasks| tasks.take_deferred());
        (self.present.take(), deferred)
    }

    /// Answers the OS's graceful stop (`application-lifecycle-v1`) on the
    /// single instance this process owns, for `callers` (normally
    /// [`LifecycleCallers::installed`]).
    /// Without an owned instance there is nothing to serve and nothing
    /// changes.
    ///
    /// # Errors
    /// The interface could not be served.
    pub fn serve_lifecycle(
        &mut self,
        callers: impl Into<LifecycleCallers>,
    ) -> Result<(), ApplicationServiceError> {
        if let Some(service) = &self.service {
            self.stop = Some(service.serve_lifecycle(callers)?);
        }
        Ok(())
    }

    /// Uses `control` for graceful stop: tests and harnesses that drive the
    /// lifecycle without a session bus.
    pub fn use_stop_control(&mut self, control: StopControl) {
        self.stop = Some(control);
    }

    /// Publishes what stopping now would interrupt. Cheap to call after every
    /// update: only a change in what would be interrupted reaches the OS as a
    /// new revision.
    pub fn publish_stop_consequences(&self, consequences: Vec<StopConsequence>) {
        if let Some(stop) = &self.stop {
            stop.publish(consequences);
        }
    }

    /// Publishes the app's live work for the OS's background-apps list (see
    /// [`StopControl::publish_work`]); [`activity_work`] reads it from an
    /// activity center. Cheap to call after every update. An app that never
    /// calls it reports its work as unknown.
    pub fn publish_work(&self, work: impl IntoIterator<Item = WorkKind>) {
        if let Some(stop) = &self.stop {
            stop.publish_work(work);
        }
    }

    /// A stop the OS asked for, once, if it still matches what is published;
    /// a stop made against an earlier list is dropped here.
    pub fn take_stop(&self) -> Option<StopRequest> {
        let stop = self.stop.as_ref()?;
        stop.take_stop().filter(|request| stop.is_current(request))
    }

    /// Serves `provider` to the desktop's Search on the single instance this
    /// process owns; without one there is nothing to serve. The user's
    /// choices arrive through the same wake as launches: take them with
    /// [`Self::take_searches`]. The provider must never bring up UI.
    ///
    /// # Errors
    /// The interface could not be served.
    pub fn serve_search(
        &mut self,
        provider: SearchProvider,
    ) -> Result<(), ApplicationServiceError> {
        if let Some(service) = &self.service {
            self.search = Some(service.serve_search(provider)?);
        }
        Ok(())
    }

    /// Results and searches the user chose in the desktop's Search, for the
    /// app to open, raising its window with each one's activation context. A
    /// windowless worker is also asked to open its window, and the app opens
    /// them once it has.
    pub fn take_searches(&mut self) -> Vec<SearchActivation> {
        let Some(search) = &self.search else {
            return Vec::new();
        };
        let activations = search.drain(
            &mut self.activation,
            Instant::now(),
            enzui_core::activation::dbus::search::SEARCH_ACTIVATION_CAPACITY,
        );
        if !activations.is_empty() {
            self.request_present(ApplicationIntent::Activate);
        }
        activations
    }

    /// Desktop launches forwarded to this instance. A windowless worker turns
    /// the first into a presentation request and returns nothing; a desktop
    /// process returns them for the app to route (open a file, raise the
    /// window).
    ///
    /// This compatibility method keeps only intents. Use
    /// [`Self::take_application_calls`] for activation context and action parameters.
    pub fn take_launches(&mut self) -> Vec<ApplicationIntent> {
        let calls = self.take_application_calls();
        if self.windowless {
            return Vec::new();
        }
        calls.into_iter().map(|call| call.intent).collect()
    }

    /// Drains desktop requests with their owned activation context and action parameters.
    ///
    /// A windowless worker also requests presentation for the first intent.
    /// Keep the returned calls until their target window exists, then consume
    /// each context through the application's window or child routing policy.
    pub fn take_application_calls(&mut self) -> Vec<ApplicationCall> {
        let Some(service) = self.service.as_mut() else {
            return Vec::new();
        };
        let calls = service.drain(
            &mut self.activation,
            Instant::now(),
            APPLICATION_CALL_CAPACITY,
        );
        if self.windowless
            && let Some(first) = calls.first()
        {
            self.request_present(first.intent.clone());
        }
        calls
    }

    /// The single-instance wake and the delivery of reducer tasks started
    /// without a window, for both the windowless runtime and the GUI.
    ///
    /// `wake` maps the single-instance wake to the message an app routes to
    /// [`Self::take_application_calls`]; like every iced subscription mapper it must
    /// capture nothing (`|_| Message::ApplicationWake`). `refused` reports a
    /// task the host could not run.
    ///
    /// The first wake arrives as soon as the subscription runs and carries the
    /// calls queued before it: the desktop launch the session bus started this
    /// process for. An app must route it even while it is still starting up;
    /// [`disposition`] presents a windowless worker that still holds a launch.
    pub fn subscription<W, R>(&self, wake: W, refused: R) -> Subscription<M>
    where
        W: Fn(ApplicationWake) -> M + Clone + Send + 'static,
        R: Fn(String) -> M + Send + 'static,
    {
        let launches = self
            .service
            .as_ref()
            .map_or_else(Subscription::none, |service| {
                service.wake_subscription(1).map(wake)
            });
        let tasks = self
            .tasks
            .as_ref()
            .map_or_else(Subscription::none, |tasks| {
                tasks.subscription(1, move |delivered| match delivered {
                    Ok(message) => message,
                    Err(error) => refused(error.to_string()),
                })
            });
        Subscription::batch([launches, tasks])
    }
}

/// How a desktop launch relates to an agent worker.
pub enum DesktopClaim {
    /// This process owns the instance; keep the service in its state.
    Owner(ApplicationService),
    /// A running instance took the launch; exit successfully.
    Forwarded,
    /// Not provisioned for agents, or no session bus: run independently.
    Independent,
}

/// Claims the single instance for a desktop launch of `app_id`, or forwards
/// `intent` to the instance already running.
///
/// The installed executable delivers `intent` once through D-Bus activation
/// ([`ApplicationService::activate_installed_with_token`]), and the bus starts
/// the app's unit when nothing runs. When the bus cannot start it (a package
/// without the enzUI user unit, or a unit that fails), the launch claims the
/// single instance itself and runs its window, forwarding instead if the unit
/// came up after all. Other builds converge only when provisioned for agent
/// use. Both Wayland and X11 startup variables are forwarded without clearing
/// a primary owner's native startup environment.
///
/// # Errors
/// Startup context is oversized, or the running instance refused the launch
/// or vanished. Never start a second instance beside one that refused.
pub fn claim_desktop(
    app_id: &str,
    intent: ApplicationIntent,
) -> Result<DesktopClaim, ApplicationServiceError> {
    let installed = installed_activation(app_id);
    if !installed && CenterProvision::discover(app_id).is_none() {
        return Ok(DesktopClaim::Independent);
    }
    let mut token =
        InboundActivationToken::from_env().map_err(|_| ApplicationServiceError::InvalidRequest)?;
    if installed {
        match ApplicationService::activate_installed_with_token(app_id, &intent, token) {
            Ok(()) => return Ok(DesktopClaim::Forwarded),
            // Without a session bus no instance can be reached behind one.
            Err(ApplicationServiceError::Connect(_) | ApplicationServiceError::Runtime(_)) => {
                return Ok(DesktopClaim::Independent);
            }
            // The bus could not start the unit. Claiming the name below either
            // makes this launch the instance or forwards to a unit that won.
            Err(ApplicationServiceError::Activation(_)) => {
                token = InboundActivationToken::from_env()
                    .map_err(|_| ApplicationServiceError::InvalidRequest)?;
            }
            Err(error) => return Err(error),
        }
    }
    match ApplicationService::connect_or_forward_with_token(app_id, Some(&intent), &mut token) {
        Ok(ApplicationStart::Owner(service)) => Ok(DesktopClaim::Owner(service)),
        Ok(ApplicationStart::Reused) => Ok(DesktopClaim::Forwarded),
        Err(ApplicationServiceError::Connect(_) | ApplicationServiceError::Runtime(_)) => {
            Ok(DesktopClaim::Independent)
        }
        Err(error) => Err(error),
    }
}

/// Claims the single instance for a background start of `app_id`: the unit
/// started by the broker or by D-Bus activation for a desktop launch.
///
/// Returns `None` when an instance already runs, so this launch should exit
/// successfully. No credential is needed; without one the worker serves
/// desktop launches and refuses agent access.
///
/// # Errors
/// The endpoint is unavailable. A background worker without the single
/// instance would race a desktop launch.
pub fn claim_background(app_id: &str) -> Result<Option<ApplicationService>, String> {
    match ApplicationService::connect_or_forward(app_id, None, None) {
        Ok(ApplicationStart::Owner(service)) => Ok(Some(service)),
        Ok(ApplicationStart::Reused) => Ok(None),
        Err(error) => Err(format!("single-instance endpoint unavailable: {error}")),
    }
}

/// Whether this process runs the installed executable of `app_id` and the
/// session bus can activate it: a `dbus-1/services/APP_ID.service` in the
/// user's or the system's data directories whose `Exec` is this executable.
#[must_use]
pub fn installed_activation(app_id: &str) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    installed_activation_in(&dbus_service_dirs(), app_id, &exe)
}

/// The session bus's service directories, most specific first.
fn dbus_service_dirs() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    home.into_iter()
        .chain(
            system
                .split(':')
                .map(PathBuf::from)
                .filter(|path| path.is_absolute()),
        )
        .map(|data| data.join("dbus-1/services"))
        .collect()
}

/// [`installed_activation`] over explicit service directories and executable.
fn installed_activation_in(
    dirs: &[std::path::PathBuf],
    app_id: &str,
    exe: &std::path::Path,
) -> bool {
    let Ok(exe) = std::fs::canonicalize(exe) else {
        return false;
    };
    let file = format!("{app_id}.service");
    // The bus uses the first service file for a name, so only that one counts.
    let Some(text) = dirs
        .iter()
        .find_map(|dir| std::fs::read_to_string(dir.join(&file)).ok())
    else {
        return false;
    };
    let mut in_section = false;
    let mut name = None;
    let mut program = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line == "[D-BUS Service]";
        } else if in_section {
            if let Some(value) = line.strip_prefix("Name=") {
                name = Some(value.to_owned());
            } else if let Some(value) = line.strip_prefix("Exec=") {
                program = first_argument(value).map(std::path::PathBuf::from);
            }
        }
    }
    name.as_deref() == Some(app_id)
        && program
            .and_then(|program| std::fs::canonicalize(program).ok())
            .is_some_and(|program| program == exe)
}

/// The program of a D-Bus service `Exec` line: its first argument, read with
/// the shell-style quoting dbus-daemon applies (single quotes literal, double
/// quotes and bare text with backslash escapes), so a quoted path with spaces
/// names its program. `None` for an empty or unterminated line.
fn first_argument(exec: &str) -> Option<String> {
    let mut chars = exec.trim_start().chars();
    let mut argument = String::new();
    let mut quote = None;
    while let Some(ch) = chars.next() {
        match (quote, ch) {
            (None, ch) if ch.is_whitespace() => break,
            (None, '\\') => argument.push(chars.next()?),
            (None, '\'' | '"') => quote = Some(ch),
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('"'), '\\') => match chars.next()? {
                escaped @ ('"' | '\\' | '$' | '`') => argument.push(escaped),
                other => {
                    argument.push('\\');
                    argument.push(other);
                }
            },
            (_, ch) => argument.push(ch),
        }
    }
    (quote.is_none() && !argument.is_empty()).then_some(argument)
}

/// Prints the package catalog generated from the live registry, for
/// `--export-agent-manifest`. Touches no credential, connector or display.
pub fn print_manifest<C: 'static>(
    registry: &AppFunctionRegistry<C>,
    metadata: AppMetadata,
) -> std::process::ExitCode {
    use std::io::Write;
    match installed::manifest(registry, metadata) {
        Ok(bytes) => {
            let mut out = std::io::stdout().lock();
            if out.write_all(&bytes).and_then(|()| out.flush()).is_ok() {
                std::process::ExitCode::SUCCESS
            } else {
                std::process::ExitCode::FAILURE
            }
        }
        Err(error) => {
            // The command's own error output, not a diagnostic.
            let _ = writeln!(std::io::stderr().lock(), "{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// The nearest live call deadline as a monotonic instant, for
/// [`BackgroundApp::wake_at`].
#[must_use]
pub fn deadline_instant<C: 'static>(host: &CenterHost<C>) -> Option<Instant> {
    let deadline = host.next_deadline()?;
    let wait = deadline
        .duration_since(SystemTime::now())
        .unwrap_or_default();
    Some(Instant::now() + wait)
}

/// The usual windowless disposition: present when asked or while a desktop
/// launch waits untaken ([`AgentProcess::launch_waiting`]), stay while a call, a
/// reducer task or `app_busy` work is live, exit once nothing is live and the
/// broker connection is gone (stopped for good, or dropped: broker EOF ends a
/// background worker, and the broker activates a fresh one on demand), and
/// idle otherwise.
#[must_use]
pub fn disposition<C: 'static, M: Clone + Send + 'static>(
    host: Option<&CenterHost<C>>,
    process: &AgentProcess<M>,
    app_busy: bool,
) -> Disposition {
    // A launch the app has not taken (its wake was dropped while it started)
    // still opens the window; without this it waited for the next launch.
    if process.wants_present() || process.launch_waiting() {
        return Disposition::Present;
    }
    let Some(host) = host else {
        // A worker without the broker serves desktop launches: the one the
        // bus started it for is still on its way, so it waits for that like
        // an idle worker, and leaves after the idle timeout if none comes.
        return if process.has_task_work() || app_busy {
            Disposition::Working
        } else {
            Disposition::Idle
        };
    };
    // A stopped connector has already retired every agent call; the app's
    // own work still finishes before the process leaves.
    if host.live_calls() > 0 || process.has_task_work() || app_busy {
        return Disposition::Working;
    }
    if matches!(
        host.status(),
        CenterStatus::Stopped(_) | CenterStatus::Disconnected(_)
    ) {
        // Nothing can reach a worker the broker let go of; retrying would
        // keep a process alive for no one.
        return Disposition::Exit;
    }
    Disposition::Idle
}

#[cfg(test)]
mod tests;
