//! One instance per user: a second launch hands its files to the window
//! already running instead of starting a second copy.
//!
//! [`SingleInstance::start`] is the first thing `main` does. The first
//! process owns the application and gets what it was launched with; a later
//! launch forwards its files and exits. The owner returns
//! [`SingleInstance::subscription`] from its subscription and calls
//! [`SingleInstance::drain`] when it wakes: that yields what to open and the
//! task that raises its window.
//!
//! ```ignore
//! fn main() -> iced::Result {
//!     let (instance, launch) = match SingleInstance::start("org.example.Notes") {
//!         InstanceStart::Owner(instance, launch) => (instance, launch),
//!         InstanceStart::Forwarded => return Ok(()),
//!     };
//!     // Move both into the application state; open `launch` first.
//! }
//! // subscription: self.instance.subscription(|_| Message::Launched)
//! // update, on Message::Launched:
//! //     let (opened, raise) = self.instance.drain(self.window);
//! //     for intent in opened { self.open(intent) }
//! //     return raise;
//! ```
//!
//! Without a session bus the application runs on its own: it owns itself and
//! no later launch reaches it. Behind the `application-dbus` feature.

use std::time::Instant;

use enzui_core::activation::dbus::{
    ApplicationService, ApplicationServiceError, ApplicationStart, ApplicationWake,
};
use enzui_core::activation::lifecycle::{LifecycleCallers, StopControl};
use enzui_core::activation::{
    ActivationBroker, ApplicationIntent, InboundActivationToken, launch_request_from_env,
};
use iced::{Subscription, Task, window};

/// How many forwarded launches one [`SingleInstance::drain`] takes.
const DRAIN_LIMIT: usize = 16;

/// What [`SingleInstance::start`] found.
#[expect(
    clippy::large_enum_variant,
    reason = "built once in `main` and moved into the application; boxing would only add a deref for every caller"
)]
pub enum InstanceStart {
    /// This process owns the application, launched with this intent:
    /// [`ApplicationIntent::Open`] with the files named on the command line,
    /// or [`ApplicationIntent::Activate`].
    Owner(SingleInstance, ApplicationIntent),
    /// Another process owns it and has the launch; exit.
    Forwarded,
}

impl std::fmt::Debug for InstanceStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Owner(instance, intent) => f
                .debug_tuple("Owner")
                .field(instance)
                .field(intent)
                .finish(),
            Self::Forwarded => f.write_str("Forwarded"),
        }
    }
}

/// The owning process's end of single-instance launching.
pub struct SingleInstance {
    service: Option<ApplicationService>,
    broker: ActivationBroker,
}

impl std::fmt::Debug for SingleInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SingleInstance")
            .field("shared", &self.service.is_some())
            .finish_non_exhaustive()
    }
}

impl SingleInstance {
    /// Claims `application_id` on the session bus, or forwards this launch
    /// to the process that holds it. Uses the same id as the desktop entry
    /// and the window.
    #[must_use]
    pub fn start(application_id: &str) -> InstanceStart {
        let launch = launch_request_from_env();
        Self::start_with_intent(application_id, &launch.intent)
    }

    /// Claims the session-bus identity or forwards an explicit intent, retaining
    /// startup activation context from the environment before the GUI consumes it.
    #[must_use]
    pub fn start_with_intent(application_id: &str, intent: &ApplicationIntent) -> InstanceStart {
        let mut token = InboundActivationToken::from_env().ok().flatten();
        let started = ApplicationService::connect_or_forward_with_token(
            application_id,
            Some(intent),
            &mut token,
        );
        Self::from_start(started, intent.clone())
    }

    // A refused forward must never start a second owner of the same identity.
    fn from_start(
        started: Result<ApplicationStart, enzui_core::activation::dbus::ApplicationServiceError>,
        intent: ApplicationIntent,
    ) -> InstanceStart {
        use enzui_core::activation::dbus::ApplicationServiceError as Error;
        match started {
            Ok(ApplicationStart::Reused) => InstanceStart::Forwarded,
            Ok(ApplicationStart::Owner(service)) => {
                InstanceStart::Owner(Self::with_service(Some(service)), intent)
            }
            Err(error @ (Error::Forward(_) | Error::NameNotAcquired(_))) => {
                log::warn!(
                    target: "enzui::single_instance",
                    "the running instance did not take this launch: {error}"
                );
                InstanceStart::Forwarded
            }
            Err(error) => {
                log::warn!(target: "enzui::single_instance", "running on its own: {error}");
                InstanceStart::Owner(Self::with_service(None), intent)
            }
        }
    }

    /// [`Self::start`] on the bus at `address` with an explicit launch
    /// `intent`, for a sandbox or a test bus.
    #[must_use]
    pub fn start_on(
        application_id: &str,
        address: &str,
        intent: &ApplicationIntent,
    ) -> InstanceStart {
        let mut token = None;
        let started = ApplicationService::connect_or_forward_with_token_to_address(
            application_id,
            address,
            Some(intent),
            &mut token,
        );
        Self::from_start(started, intent.clone())
    }

    /// An instance no later launch reaches, as when there is no session bus.
    #[must_use]
    pub fn standalone() -> Self {
        Self::with_service(None)
    }

    fn with_service(service: Option<ApplicationService>) -> Self {
        Self {
            service,
            broker: ActivationBroker::default(),
        }
    }

    /// Whether later launches reach this process.
    #[must_use]
    pub fn is_shared(&self) -> bool {
        self.service.is_some()
    }

    /// Serves the OS lifecycle interface (`application-lifecycle-v1`) on this
    /// instance's own application connection, answering only `callers`
    /// (normally [`LifecycleCallers::installed`]) with the caller
    /// authentication [`ApplicationService::serve_lifecycle`] describes.
    /// Publish pending work and stop consequences through the returned
    /// control; an accepted stop wakes [`Self::subscription`] like a forwarded
    /// launch, so check [`StopControl::take_stop`] when it fires.
    ///
    /// `Ok(None)` when the instance runs on its own: without a session bus
    /// there is no OS to report to.
    ///
    /// # Errors
    /// The interface could not be served, or is already served.
    pub fn serve_lifecycle(
        &self,
        callers: impl Into<LifecycleCallers>,
    ) -> Result<Option<StopControl>, ApplicationServiceError> {
        self.service
            .as_ref()
            .map(|service| service.serve_lifecycle(callers))
            .transpose()
    }

    /// [`Self::serve_lifecycle`] answering only processes running this same
    /// executable: tests and harnesses that play the OS from the app's own
    /// binary.
    ///
    /// # Errors
    /// The interface could not be served, or is already served.
    pub fn serve_lifecycle_admitting_own_executable(
        &self,
    ) -> Result<Option<StopControl>, ApplicationServiceError> {
        self.service
            .as_ref()
            .map(ApplicationService::serve_lifecycle_admitting_own_executable)
            .transpose()
    }

    /// A message whenever a later launch has been forwarded here
    /// (`|_| Message::Launched`); answer it with [`Self::drain`].
    pub fn subscription<Message>(
        &self,
        on_launch: fn(ApplicationWake) -> Message,
    ) -> Subscription<Message>
    where
        Message: Send + 'static,
    {
        match &self.service {
            // `map` takes only zero-sized closures; the constructor rides
            // along as subscription data instead.
            Some(service) => service
                .wake_subscription(1)
                .with(on_launch)
                .map(|(on_launch, wake)| on_launch(wake)),
            None => Subscription::none(),
        }
    }

    /// The forwarded launches: what each asked to open, and the task that
    /// raises `window` (the most recently opened one when `None`) with the
    /// launcher's activation token, so the compositor brings it forward.
    pub fn drain<Message>(
        &mut self,
        window: Option<window::Id>,
    ) -> (Vec<ApplicationIntent>, Task<Message>)
    where
        Message: Send + 'static,
    {
        let Some(service) = self.service.as_mut() else {
            return (Vec::new(), Task::none());
        };
        let calls = service.drain(&mut self.broker, Instant::now(), DRAIN_LIMIT);
        let mut intents = Vec::with_capacity(calls.len());
        let mut raise = Vec::new();
        for call in calls {
            intents.push(call.intent);
            let mut context = call.context;
            let target = match window {
                Some(window) => Task::done(Some(window)),
                None => window::latest(),
            };
            raise.push(target.then(move |window| match (window, context.take()) {
                (Some(window), Some(context)) => context.activate(window, Instant::now()).discard(),
                (Some(window), None) => window::gain_focus(window),
                (None, _) => Task::none(),
            }));
        }
        (intents, Task::batch(raise))
    }
}

#[cfg(test)]
#[path = "../../enzui-core/tests/support/private_bus.rs"]
mod private_bus;

#[cfg(test)]
mod fallback_tests {
    use super::*;
    use enzui_core::activation::dbus::ApplicationServiceError as Error;

    /// Only an unreachable bus runs the app on its own; a running instance
    /// that refused this launch is never joined by a second one.
    #[test]
    fn a_refusing_owner_is_not_joined_by_a_second_instance() {
        for refused in [
            Error::Forward("busy".into()),
            Error::NameNotAcquired("taken".into()),
        ] {
            assert!(matches!(
                SingleInstance::from_start(Err(refused), ApplicationIntent::Activate),
                InstanceStart::Forwarded
            ));
        }
        for unreachable in [Error::Connect("no bus".into()), Error::Timeout] {
            assert!(matches!(
                SingleInstance::from_start(Err(unreachable), ApplicationIntent::Activate),
                InstanceStart::Owner(..)
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::private_bus;
    use super::{InstanceStart, SingleInstance};
    use enzui_core::activation::ApplicationIntent;
    use std::time::{Duration, Instant};

    const APP_ID: &str = "org.enzui.SingleInstanceFixture";

    #[test]
    fn explicit_intents_preserve_environment_activation_context() {
        const APP: &str = "org.enzui.ExplicitIntentFixture";
        const CHILD: &str = "ENZUI_EXPLICIT_INTENT_CHILD";
        let intent = ApplicationIntent::Open(vec!["https://example.test/profile".into()]);
        if std::env::var_os(CHILD).is_some() {
            assert!(matches!(
                SingleInstance::start_with_intent(APP, &intent),
                InstanceStart::Forwarded
            ));
            return;
        }
        let bus = private_bus::PrivateBus::spawn();
        let InstanceStart::Owner(mut first, _) =
            SingleInstance::start_on(APP, &bus.address, &ApplicationIntent::Activate)
        else {
            panic!("the fixture owns its isolated identity");
        };
        let child = std::process::Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--exact",
                "single_instance::tests::explicit_intents_preserve_environment_activation_context",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
            .env("XDG_ACTIVATION_TOKEN", "explicit-intent-fixture-token")
            .env_remove("DESKTOP_STARTUP_ID")
            .output()
            .expect("child test launch");
        assert!(
            child.status.success(),
            "{}",
            String::from_utf8_lossy(&child.stderr)
        );
        let mut broker = enzui_core::activation::ActivationBroker::default();
        let service = first.service.as_mut().expect("native application service");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut calls = loop {
            let calls = service.drain(&mut broker, Instant::now(), 4);
            if !calls.is_empty() || Instant::now() >= deadline {
                break calls;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(calls.len(), 1);
        let call = calls.remove(0);
        assert_eq!(call.intent, intent);
        let token = call
            .context
            .expect("forwarded activation context")
            .into_token(Instant::now())
            .expect("fresh native context");
        assert_eq!(
            token.xdg_activation_token(),
            Some("explicit-intent-fixture-token")
        );
    }

    #[test]
    fn a_second_launch_hands_its_files_to_the_first() {
        let bus = private_bus::PrivateBus::spawn();
        let InstanceStart::Owner(mut first, launch) =
            SingleInstance::start_on(APP_ID, &bus.address, &ApplicationIntent::Activate)
        else {
            panic!("the first launch owns the application");
        };
        assert_eq!(launch, ApplicationIntent::Activate);
        assert!(first.is_shared());

        let files = ApplicationIntent::Open(vec!["file:///home/user/notes.txt".to_owned()]);
        assert!(
            matches!(
                SingleInstance::start_on(APP_ID, &bus.address, &files),
                InstanceStart::Forwarded
            ),
            "the second launch forwards and exits"
        );

        // The owner's subscription reports the forwarded launch.
        let launched = {
            use iced::futures::StreamExt as _;
            let recipes =
                iced::advanced::subscription::into_recipes(first.subscription(|_| "launched"));
            let mut streams: Vec<_> = recipes
                .into_iter()
                .map(|recipe| recipe.stream(Box::pin(iced::futures::stream::pending())))
                .collect();
            iced::futures::executor::block_on(async {
                let next = iced::futures::future::select_all(
                    streams.iter_mut().map(|stream| stream.next()),
                );
                let timeout = async {
                    std::thread::sleep(Duration::from_secs(5));
                };
                iced::futures::pin_mut!(timeout);
                match iced::futures::future::select(next, timeout).await {
                    iced::futures::future::Either::Left(((message, _, _), _)) => message,
                    iced::futures::future::Either::Right(_) => None,
                }
            })
        };
        assert_eq!(launched, Some("launched"));

        let deadline = Instant::now() + Duration::from_secs(5);
        let opened = loop {
            let (opened, _raise) = first.drain::<()>(None);
            if !opened.is_empty() || Instant::now() > deadline {
                break opened;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(opened, vec![files]);

        let alone = SingleInstance::standalone();
        assert!(!alone.is_shared());
    }

    #[test]
    fn the_owner_serves_the_lifecycle_on_its_own_connection() {
        let bus = private_bus::PrivateBus::spawn();
        let InstanceStart::Owner(owner, _) = SingleInstance::start_on(
            "org.enzui.SingleInstanceLifecycleFixture",
            &bus.address,
            &ApplicationIntent::Activate,
        ) else {
            panic!("the first launch owns the application");
        };
        let control = owner
            .serve_lifecycle_admitting_own_executable()
            .expect("served");
        assert!(control.is_some(), "a shared instance serves the lifecycle");
        assert!(
            owner.serve_lifecycle_admitting_own_executable().is_err(),
            "the interface sits on the owner's one connection, served once"
        );
        assert!(
            SingleInstance::standalone()
                .serve_lifecycle_admitting_own_executable()
                .expect("nothing to serve")
                .is_none(),
            "an instance on its own has no OS to report to"
        );
    }
}
