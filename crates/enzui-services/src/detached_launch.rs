//! Launching a long-lived desktop program the app does not wait for.
//!
//! [`crate::process_transport`] runs *helpers*: bounded, captured, killed at a
//! deadline. A terminal, an editor or any other program the user asked to
//! open is the opposite: it outlives the request and must not be killed when
//! a deadline passes or the app's process group is signalled. This module
//! launches such a program and reports only what the app can honestly know:
//!
//! - it could not be started ([`LaunchError::Spawn`]);
//! - it exited unsuccessfully within the settle window
//!   ([`LaunchError::ExitedEarly`]), which is how a wrong option or a missing
//!   display shows up;
//! - it exited successfully within the window ([`Launched::Handoff`], a
//!   client that handed off to a running server), or is still running
//!   ([`Launched::Running`]).
//!
//! The child runs in its own process group with stdin, stdout and stderr on
//! `/dev/null` (a pipe nobody reads would stall or kill it later), a cleared
//! environment plus the pairs the caller forwards, and an explicit working
//! directory. Arguments are separate [`OsString`]s; nothing is re-split or
//! passed through a shell, so a hostile path is only ever one argument or the
//! working directory. A still-running child is handed to one shared reaper
//! thread so it never lingers as a zombie.
//!
//! The executable must already be resolved to an absolute path; resolution
//! policy (which `PATH`, which candidates) is the caller's.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

/// How long a launch is watched for an early failure by default.
pub const DEFAULT_SETTLE: Duration = Duration::from_millis(750);

/// The most children the shared reaper holds at once; beyond it a launch is
/// refused rather than left to become a zombie.
pub const MAX_REAPED_CHILDREN: usize = 256;

const POLL: Duration = Duration::from_millis(10);
const REAP_POLL: Duration = Duration::from_millis(500);

/// One program to launch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetachedLaunch {
    executable: PathBuf,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
    working_directory: PathBuf,
    settle: Duration,
}

impl DetachedLaunch {
    /// Launch the absolute `executable` in `working_directory`.
    #[must_use]
    pub fn new(executable: impl Into<PathBuf>, working_directory: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            args: Vec::new(),
            env: Vec::new(),
            working_directory: working_directory.into(),
            settle: DEFAULT_SETTLE,
        }
    }

    /// Append one argument, exactly.
    #[must_use]
    pub fn arg(mut self, argument: impl Into<OsString>) -> Self {
        self.args.push(argument.into());
        self
    }

    /// Append arguments, each exactly one argv item.
    #[must_use]
    pub fn args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(arguments.into_iter().map(Into::into));
        self
    }

    /// Forward environment pairs; nothing else from the app's environment is.
    #[must_use]
    pub fn envs<I, K, V>(mut self, vars: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<OsString>,
        V: Into<OsString>,
    {
        self.env
            .extend(vars.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    /// How long to watch for an early exit.
    #[must_use]
    pub fn settle(mut self, settle: Duration) -> Self {
        self.settle = settle;
        self
    }

    /// The executable.
    #[must_use]
    pub fn executable(&self) -> &std::path::Path {
        &self.executable
    }

    /// The arguments after the program name.
    #[must_use]
    pub fn argv(&self) -> &[OsString] {
        &self.args
    }

    /// The working directory.
    #[must_use]
    pub fn working_directory(&self) -> &std::path::Path {
        &self.working_directory
    }
}

/// A launch that did not fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Launched {
    /// Still running after the settle window; the reaper owns it now.
    Running,
    /// Exited successfully within the window, typically a client handing the
    /// request to an already-running instance.
    Handoff,
}

/// Why a launch failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchError {
    /// The executable is not an absolute path.
    NotAbsolute,
    /// The working directory is missing or not a directory.
    WorkingDirectory,
    /// The OS refused to start it.
    Spawn(String),
    /// It exited unsuccessfully within the settle window: the exit code, or
    /// `None` when a signal ended it.
    ExitedEarly(Option<i32>),
    /// The reaper is full or could not be started.
    ReaperUnavailable,
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAbsolute => f.write_str("the program path is not absolute"),
            Self::WorkingDirectory => f.write_str("the folder is not available"),
            Self::Spawn(detail) => write!(f, "it could not be started: {detail}"),
            Self::ExitedEarly(Some(code)) => write!(f, "it exited with status {code}"),
            Self::ExitedEarly(None) => f.write_str("it was terminated"),
            Self::ReaperUnavailable => f.write_str("too many launched programs are still running"),
        }
    }
}

impl std::error::Error for LaunchError {}

/// Launch `request` and watch it for its settle window. Blocks for at most
/// that window, so call it off the UI thread.
///
/// # Errors
///
/// See [`LaunchError`].
pub fn launch(request: &DetachedLaunch) -> Result<Launched, LaunchError> {
    if !request.executable.is_absolute() {
        return Err(LaunchError::NotAbsolute);
    }
    if !request.working_directory.is_dir() {
        return Err(LaunchError::WorkingDirectory);
    }
    let reaper = reaper().ok_or(LaunchError::ReaperUnavailable)?;
    if reaper.held() >= MAX_REAPED_CHILDREN {
        return Err(LaunchError::ReaperUnavailable);
    }
    let mut command = Command::new(&request.executable);
    command
        .args(&request.args)
        .env_clear()
        .envs(request.env.iter().map(|(k, v)| (k, v)))
        .current_dir(&request.working_directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own group: a signal to the app's group (Ctrl+C in the terminal
        // that started the app, a helper teardown) does not reach it.
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| LaunchError::Spawn(error.to_string()))?;
    match watch(&mut child, request.settle) {
        Some(status) if status.success() => Ok(Launched::Handoff),
        Some(status) => Err(LaunchError::ExitedEarly(status.code())),
        None => {
            reaper.adopt(child);
            Ok(Launched::Running)
        }
    }
}

/// The child's exit status if it exits within `settle`.
fn watch(child: &mut Child, settle: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + settle;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL),
            // Still running, or unobservable: either way the reaper owns it.
            _ => return None,
        }
    }
}

struct Reaper {
    adopt: Mutex<Sender<Child>>,
    held: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl Reaper {
    fn held(&self) -> usize {
        self.held.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn adopt(&self, child: Child) {
        self.held.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let sender = self
            .adopt
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // The thread outlives every sender; a send cannot fail while it runs.
        let _ = sender.send(child);
    }
}

fn reaper() -> Option<&'static Reaper> {
    static REAPER: OnceLock<Option<Reaper>> = OnceLock::new();
    REAPER
        .get_or_init(|| {
            let (adopt, children) = mpsc::channel();
            let held = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let count = std::sync::Arc::clone(&held);
            thread::Builder::new()
                .name("enzui-detached-reaper".into())
                .spawn(move || reap(&children, &count))
                .ok()?;
            Some(Reaper {
                adopt: Mutex::new(adopt),
                held,
            })
        })
        .as_ref()
}

fn reap(children: &Receiver<Child>, held: &std::sync::atomic::AtomicUsize) {
    let mut live: Vec<Child> = Vec::new();
    loop {
        let incoming = if live.is_empty() {
            match children.recv() {
                Ok(child) => Some(child),
                Err(_) => return,
            }
        } else {
            children.recv_timeout(REAP_POLL).ok()
        };
        live.extend(incoming);
        live.extend(children.try_iter());
        let before = live.len();
        live.retain_mut(|child| matches!(child.try_wait(), Ok(None)));
        held.fetch_sub(before - live.len(), std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
#[path = "detached_launch_tests.rs"]
mod tests;
