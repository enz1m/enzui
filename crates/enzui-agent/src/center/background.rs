//! One application process for agent work, with or without its window.
//!
//! An installed app's user unit starts it with [`BACKGROUND_FLAG`]. The process
//! then runs the application's own state, reducer and subscriptions without
//! initializing graphics, opening a display connection or creating a window:
//! [`run`] drives the app's [`BackgroundApp::subscription`] on a headless
//! executor, exactly as the GUI runtime would, and submits every reducer task
//! to the app's [`TaskHost`]. Nothing here is a second implementation of an app
//! function; the same typed registry and handlers serve both modes.
//!
//! [`run`] returns when the application asks for its window
//! ([`Disposition::Present`]), asks to exit, or has stayed
//! [`Disposition::Idle`] for the idle timeout. On return every subscription has
//! been dropped, so the receivers it held (the center host's events, the task
//! host's results, desktop activation wakes) are free for the GUI runtime the
//! application starts next with the same state. Messages a subscription had
//! already taken from its source before the stop are applied first, so none
//! is lost between the two runtimes. Work still running in the task host keeps
//! running and is delivered through its GUI subscription.
//!
//! # What the application owes
//!
//! - Report [`Disposition::Working`] while any center call, task-host work
//!   ([`TaskHost::has_work`]) or app-owned operation is live. Idle exit is safe
//!   only when none is.
//! - Return [`Disposition::Present`] only for explicit interaction: a desktop
//!   launch forwarded through the single-instance bridge, or a call whose
//!   contract is to show the user something (a review). Agent reads and
//!   navigation never open a window.
//! - Answer [`BackgroundApp::wake_at`] with the nearest call deadline so
//!   expired calls retire without a broker event.
//!
//! Single-instance ownership (`org.freedesktop.Application`) is the
//! application's: acquire it before calling [`run`], and exit at once when an
//! existing instance already owns it. That instance serves the broker itself.

use super::task_host::{TaskHost, TaskHostError};
use iced_futures::Subscription;
use iced_futures::futures::StreamExt;
use iced_futures::futures::channel::mpsc;
use iced_futures::subscription::into_recipes;
use iced_runtime::Task;
use std::ffi::OsStr;
use std::time::{Duration, Instant};

pub use super::installed::{BACKGROUND_FLAG, EXPORT_MANIFEST_FLAG};

/// How long an application with nothing to do stays resident in background
/// mode before it exits, unless it chooses another timeout.
pub const DEFAULT_IDLE_EXIT: Duration = Duration::from_secs(30);

/// What a launch was asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    /// An ordinary desktop launch; any arguments are the application's own.
    Gui,
    /// Broker activation through the installed user unit.
    Background,
    /// Print the package catalog generated from the live registry.
    ExportManifest,
}

/// A mode flag was combined with other arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchError {
    /// The mode flag that must be passed alone.
    pub flag: &'static str,
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} takes no other arguments", self.flag)
    }
}

impl std::error::Error for LaunchError {}

impl LaunchMode {
    /// Classifies the arguments after the program name.
    ///
    /// A mode flag must be the only argument: a unit's command line is fixed
    /// by the package, so anything beside the flag is a mistake rather than an
    /// option, and a GUI path argument must never be read as a mode.
    ///
    /// # Errors
    /// A mode flag beside any other argument.
    pub fn from_args<I, S>(args: I) -> Result<Self, LaunchError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let args: Vec<S> = args.into_iter().collect();
        let flag = |name: &'static str| args.iter().any(|arg| arg.as_ref() == OsStr::new(name));
        for (name, mode) in [
            (BACKGROUND_FLAG, Self::Background),
            (EXPORT_MANIFEST_FLAG, Self::ExportManifest),
        ] {
            if flag(name) {
                return if args.len() == 1 {
                    Ok(mode)
                } else {
                    Err(LaunchError { flag: name })
                };
            }
        }
        Ok(Self::Gui)
    }
}

/// What the application needs after a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// A call, task or app-owned operation is live; stay resident.
    Working,
    /// Nothing is live; exit after the idle timeout unless work arrives.
    Idle,
    /// Hand the same state to the GUI runtime now.
    Present,
    /// Exit now, for example when the broker has permanently stopped.
    Exit,
}

/// Why [`run`] returned, carrying the application state back.
#[derive(Debug)]
pub enum BackgroundExit<S> {
    /// The application asked for its window; start the GUI with this state.
    Present(S),
    /// Nothing was live for the whole idle timeout.
    Idle(S),
    /// The application asked to exit.
    Exit(S),
}

/// The background runtime could not start.
#[derive(Debug)]
pub enum BackgroundError {
    /// The headless executor could not be created.
    Runtime(std::io::Error),
}

impl std::fmt::Display for BackgroundError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime(error) => write!(f, "background runtime failed to start: {error}"),
        }
    }
}

impl std::error::Error for BackgroundError {}

/// The application, as the background runtime drives it.
pub trait BackgroundApp {
    /// The application's reducer message.
    type Message: Clone + Send + std::fmt::Debug + 'static;

    /// The application's own reducer.
    fn update(&mut self, message: Self::Message) -> Task<Self::Message>;

    /// The subscriptions background mode needs: the center host's events, the
    /// task host's results and the single-instance wake, at least. Anything
    /// that needs a display or window does not belong here.
    fn subscription(&self) -> Subscription<Self::Message>;

    /// What the application needs now.
    fn disposition(&self) -> Disposition;

    /// The next instant the application must run without an event: the
    /// nearest center call deadline, typically.
    fn wake_at(&self) -> Option<Instant> {
        None
    }

    /// Called when [`Self::wake_at`] passes.
    fn wake(&mut self) -> Task<Self::Message> {
        Task::none()
    }

    /// A reducer task could not be submitted. The task has been dropped; the
    /// default logs and continues, and the application may fail the call that
    /// needed it instead.
    fn refused(&mut self, error: TaskHostError) -> Task<Self::Message> {
        log::warn!(target: "enzui::agent", "background task refused: {error}");
        Task::none()
    }
}

/// The headless executor: a current-thread tokio runtime the loop blocks on,
/// so subscription futures run only while the loop waits.
struct Executor(tokio::runtime::Handle);

impl iced_futures::Executor for Executor {
    fn new() -> Result<Self, std::io::Error> {
        tokio::runtime::Handle::try_current()
            .map(Self)
            .map_err(std::io::Error::other)
    }

    fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) {
        // Detached: the tracker cancels a subscription through its own handle.
        drop(self.0.spawn(future));
    }

    // Nothing in the subscription runtime blocks on this executor; the loop
    // itself blocks on the runtime that owns the handle. A caller that does
    // gets its own short-lived runtime rather than a nested `block_on`.
    fn block_on<T>(&self, future: impl Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a current-thread runtime for block_on")
            .block_on(future)
    }

    fn enter<R>(&self, f: impl FnOnce() -> R) -> R {
        let _guard = self.0.enter();
        f()
    }
}

enum Turn<M> {
    Message(M),
    Wake,
    Idle,
    WorkChanged,
    Closed,
}

/// Runs `app` without a display until it presents, exits or idles out.
///
/// `boot` is the application's background startup task; it must not open a
/// window (the task host refuses window actions and aborts that stream).
/// Every event is delivered through the application's own subscriptions and
/// every reducer task through `tasks`, so the loop blocks between events and
/// wakes only for a message, a requested deadline or the idle timeout.
///
/// # Errors
/// Returns executor startup failures before running anything.
pub fn run<A: BackgroundApp>(
    mut app: A,
    boot: Task<A::Message>,
    tasks: &TaskHost<A::Message>,
    idle_exit: Duration,
) -> Result<BackgroundExit<A>, BackgroundError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(BackgroundError::Runtime)?;
    let (sender, mut receiver) = mpsc::unbounded::<A::Message>();
    let mut subscriptions = iced_futures::Runtime::new(Executor(runtime.handle().clone()), sender);

    submit(&mut app, tasks, boot);
    subscriptions.track(into_recipes(app.subscription()));

    let mut idle_since: Option<Instant> = None;
    let exit = loop {
        let now = Instant::now();
        match app.disposition() {
            Disposition::Present => break Stop::Present,
            Disposition::Exit => break Stop::Exit,
            Disposition::Working => idle_since = None,
            Disposition::Idle => {
                let since = *idle_since.get_or_insert(now);
                if now.duration_since(since) >= idle_exit {
                    break Stop::Idle;
                }
            }
        }
        let idle_at = idle_since.map(|since| since + idle_exit);
        let wake_at = app.wake_at();

        let turn = runtime.block_on(async {
            let deadline = match (idle_at, wake_at) {
                (Some(idle), Some(wake)) => Some(idle.min(wake)),
                (idle, wake) => idle.or(wake),
            };
            let sleep = async {
                match deadline {
                    Some(at) => tokio::time::sleep_until(at.into()).await,
                    None => std::future::pending().await,
                }
            };
            let work_changed = tasks.work_changed();
            tokio::select! {
                message = receiver.next() => match message {
                    Some(message) => Turn::Message(message),
                    None => Turn::Closed,
                },
                // The host's own counters change without a message (a
                // delivered result's lease is released just after the app
                // received it), so an app idle by `has_work` must be looked at
                // again then rather than left blocked.
                () = work_changed => Turn::WorkChanged,
                () = sleep => {
                    if wake_at.is_some_and(|wake| Instant::now() >= wake) {
                        Turn::Wake
                    } else {
                        Turn::Idle
                    }
                }
            }
        });

        let task = match turn {
            Turn::Message(message) => app.update(message),
            Turn::Wake => app.wake(),
            Turn::Idle | Turn::WorkChanged => continue,
            Turn::Closed => break Stop::Exit,
        };
        submit(&mut app, tasks, task);
        subscriptions.track(into_recipes(app.subscription()));
    };

    // Dropping the tracker and then the executor drops every subscription
    // stream, returning the receivers they held before the GUI subscribes.
    drop(subscriptions);
    // What the streams forwarded before the stop is no longer in any source
    // the GUI subscribes to (a task result has left the task host's queue):
    // the app takes it here, and the GUI starts from that state.
    while let Ok(message) = receiver.try_recv() {
        let task = app.update(message);
        submit(&mut app, tasks, task);
    }
    drop(runtime);
    Ok(match exit {
        Stop::Present => BackgroundExit::Present(app),
        Stop::Exit => BackgroundExit::Exit(app),
        Stop::Idle => BackgroundExit::Idle(app),
    })
}

/// Why the loop stopped; the app is handed back once it has taken what the
/// subscriptions already forwarded.
enum Stop {
    Present,
    Exit,
    Idle,
}

fn submit<A: BackgroundApp>(app: &mut A, tasks: &TaskHost<A::Message>, task: Task<A::Message>) {
    // A refusal is reported to the app, whose reply goes through the same
    // gate; a host that keeps refusing must not recurse without bound.
    let mut pending = Some(task);
    for _ in 0..4 {
        let Some(task) = pending.take() else {
            return;
        };
        if let Err(error) = tasks.submit(task) {
            pending = Some(app.refused(error));
        }
    }
}

#[cfg(test)]
#[path = "background_tests.rs"]
mod tests;
