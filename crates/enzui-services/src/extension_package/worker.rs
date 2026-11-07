//! The worker protocol: one bounded JSON request on standard input, one
//! bounded JSON response on standard output, diagnostics on standard error.
//!
//! The host side ([`invoke`]) checks the extension is active and intact,
//! runs its worker through the bounded process transport, optionally inside
//! a [`Sandbox`], and turns every way it can end into a [`WorkerFailure`].
//! The author side ([`serve`], and [`serve_builtin`] for workers compiled
//! into the host) reads, answers and exits.

use super::manifest::WorkerSpec;
use super::store::{Installed, verify};
use crate::process_transport::{self, ProcessErrorKind, ProcessRequest, StdinPolicy};
use enzui_core::blocking_task::{BlockingTask, BlockingTaskCancellation, BlockingTaskOwner};
use enzui_core::cancellation::{CancelCause, CancelToken};
use enzui_core::operation_activity::ActivityKey;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The argument a host passes its own executable to run a builtin worker.
pub const BUILTIN_FLAG: &str = "--enzui-extension-builtin";

/// Bounds on one worker run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkerLimits {
    /// The encoded request.
    pub request_bytes: usize,
    /// The response on standard output.
    pub response_bytes: usize,
    /// Diagnostics kept from standard error.
    pub stderr_bytes: usize,
    /// The whole run, start to reap.
    pub deadline: Duration,
}

impl Default for WorkerLimits {
    fn default() -> Self {
        Self {
            request_bytes: 2 << 20,
            response_bytes: 64 << 10,
            stderr_bytes: 4096,
            deadline: Duration::from_secs(60),
        }
    }
}

/// How one worker run ended without a response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkerFailure {
    /// The extension is disabled, damaged or incompatible: the problem.
    Inactive(String),
    /// Cancelled; the worker's process group was stopped and reaped.
    Cancelled,
    /// The deadline passed first.
    TimedOut,
    /// The request is larger than [`WorkerLimits::request_bytes`].
    RequestTooLarge,
    /// The worker wrote more than [`WorkerLimits::response_bytes`].
    ResponseTooLarge,
    /// The worker exited unsuccessfully; its trimmed diagnostics.
    Failed(String),
    /// The worker answered something that is not a response.
    Protocol,
    /// The worker or the sandbox could not be started.
    Unavailable(String),
}

impl std::fmt::Display for WorkerFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Inactive(problem) => f.write_str(problem),
            Self::Cancelled => f.write_str("The extension action was cancelled"),
            Self::TimedOut => f.write_str("The extension took too long and was stopped"),
            Self::RequestTooLarge => f.write_str("Too much was selected for the extension"),
            Self::ResponseTooLarge => f.write_str("The extension answered with too much data"),
            Self::Failed(detail) if detail.is_empty() => {
                f.write_str("The extension action failed; check its settings")
            }
            Self::Failed(detail) => write!(f, "The extension action failed: {detail}"),
            Self::Protocol => f.write_str("The extension returned invalid protocol data"),
            Self::Unavailable(detail) => write!(f, "The extension could not start: {detail}"),
        }
    }
}

impl std::error::Error for WorkerFailure {}

/// Where a worker runs.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum Isolation {
    /// As the user, with the user's full permissions. Disclose this when
    /// asking the user to install a native extension.
    #[default]
    Ambient,
    /// Inside a bubblewrap sandbox.
    Sandboxed(Sandbox),
}

/// A bubblewrap sandbox: the system read-only, home, `/tmp` and `/run`
/// (and so the session bus and display) replaced by empty folders, no
/// network unless asked, a cleared environment, and only the extension's
/// own folder plus the listed paths visible.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Sandbox {
    /// Paths the worker may read.
    pub read_only: Vec<PathBuf>,
    /// Paths the worker may read and write.
    pub read_write: Vec<PathBuf>,
    /// Whether the worker may use the network.
    pub network: bool,
}

impl Sandbox {
    /// The bubblewrap executable, when it is installed and works here
    /// (unprivileged user namespaces may be disabled).
    #[must_use]
    pub fn available() -> Option<PathBuf> {
        static PROBED: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
        PROBED
            .get_or_init(|| {
                let bwrap = std::env::var_os("PATH")
                    .into_iter()
                    .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
                    .chain([PathBuf::from("/usr/bin")])
                    .filter(|dir| dir.is_absolute())
                    .map(|dir| dir.join("bwrap"))
                    .find(|path| path.is_file())?;
                let probe = ProcessRequest::new(
                    bwrap.clone().into_os_string(),
                    StdinPolicy::Null,
                    64,
                    1024,
                    Duration::from_secs(10),
                )
                .args(["--ro-bind", "/", "/", "--unshare-all", "--", "/bin/true"]);
                process_transport::run(probe, &CancelToken::new())
                    .ok()
                    .filter(|output| output.status.success())
                    .map(|_| bwrap)
            })
            .clone()
    }

    fn argv(&self, folder: &Path, cwd: &Path, program: &Path) -> Vec<OsString> {
        let mut argv: Vec<OsString> = [
            "--die-with-parent",
            "--new-session",
            "--unshare-all",
            "--ro-bind",
            "/",
            "/",
            "--tmpfs",
            "/home",
            "--tmpfs",
            "/root",
            "--tmpfs",
            "/tmp",
            "--tmpfs",
            "/run",
            "--tmpfs",
            "/var/tmp",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        if self.network {
            argv.push("--share-net".into());
        }
        let mut bind = |flag: &str, path: &Path| {
            argv.push(flag.into());
            argv.push(path.into());
            argv.push(path.into());
        };
        bind("--ro-bind", folder);
        for path in &self.read_only {
            bind("--ro-bind", path);
        }
        for path in &self.read_write {
            bind("--bind", path);
        }
        // The working folder must be one the sandbox shows.
        let visible = std::iter::once(folder)
            .chain(self.read_only.iter().map(PathBuf::as_path))
            .chain(self.read_write.iter().map(PathBuf::as_path))
            .any(|root| cwd.starts_with(root));
        argv.push("--chdir".into());
        argv.push(if visible { cwd } else { folder }.into());
        argv.extend([
            "--setenv".into(),
            "PATH".into(),
            "/usr/local/bin:/usr/bin:/bin".into(),
        ]);
        for (key, value) in [("HOME", "/tmp"), ("LANG", "C.UTF-8")] {
            argv.extend(["--setenv".into(), key.into(), value.into()]);
        }
        argv.push("--".into());
        argv.push(program.into());
        argv
    }
}

/// Runs `extension`'s worker on `request` with `cwd` as its working folder
/// and returns its decoded response. `store` is the store's root.
///
/// The host must still check the response's effects against the manifest's
/// permissions ([`super::Manifest::grants`]) before applying them.
///
/// # Errors
///
/// See [`WorkerFailure`].
pub fn invoke<C, Request: Serialize, Response: DeserializeOwned>(
    store: &Path,
    extension: &Installed<C>,
    request: &Request,
    cwd: &Path,
    isolation: &Isolation,
    limits: &WorkerLimits,
    cancel: &CancelToken,
) -> Result<Response, WorkerFailure> {
    let manifest = match (&extension.problem, &extension.manifest) {
        (Some(problem), _) => return Err(WorkerFailure::Inactive(problem.to_string())),
        (None, None) => return Err(WorkerFailure::Inactive("The extension is damaged".into())),
        (None, Some(_)) if !extension.state.enabled => {
            return Err(WorkerFailure::Inactive(
                "The extension is turned off".into(),
            ));
        }
        (None, Some(manifest)) => manifest,
    };
    let folder = store.join(&extension.id);
    let (program, args): (PathBuf, Vec<OsString>) = match &manifest.worker {
        WorkerSpec::Native { program, .. } => {
            // The files are checked again just before running: the store may
            // have been edited since the catalog was read.
            verify(&folder, &extension.state)
                .map_err(|error| WorkerFailure::Inactive(error.to_string()))?;
            (folder.join(program), Vec::new())
        }
        WorkerSpec::Builtin { id } => (
            std::env::current_exe()
                .map_err(|error| WorkerFailure::Unavailable(error.to_string()))?,
            vec![BUILTIN_FLAG.into(), id.into()],
        ),
    };
    let bytes = serde_json::to_vec(request).map_err(|_| WorkerFailure::Protocol)?;
    if bytes.len() > limits.request_bytes {
        return Err(WorkerFailure::RequestTooLarge);
    }
    let process = match isolation {
        Isolation::Ambient => ProcessRequest::new(
            program.into_os_string(),
            StdinPolicy::Bytes(bytes),
            limits.response_bytes,
            limits.stderr_bytes,
            limits.deadline,
        )
        .args(args)
        .current_dir(cwd)
        .inherit_environment(),
        Isolation::Sandboxed(sandbox) => {
            let bwrap = Sandbox::available().ok_or_else(|| {
                WorkerFailure::Unavailable("the bubblewrap sandbox is not available".into())
            })?;
            let mut argv = sandbox.argv(&folder, cwd, &program);
            argv.extend(args);
            ProcessRequest::new(
                bwrap.into_os_string(),
                StdinPolicy::Bytes(bytes),
                limits.response_bytes,
                limits.stderr_bytes,
                limits.deadline,
            )
            .args(argv)
        }
    };
    let output = process_transport::run(process, cancel).map_err(|error| match error.kind() {
        ProcessErrorKind::Cancelled(_) => WorkerFailure::Cancelled,
        ProcessErrorKind::Deadline => WorkerFailure::TimedOut,
        ProcessErrorKind::OutputLimitExceeded(process_transport::OutputStream::Stdout) => {
            WorkerFailure::ResponseTooLarge
        }
        ProcessErrorKind::OutputLimitExceeded(process_transport::OutputStream::Stderr) => {
            WorkerFailure::Failed(diagnostics(error.stderr()))
        }
        ProcessErrorKind::ExecutableAuthority | ProcessErrorKind::Spawn => {
            WorkerFailure::Unavailable("the worker program could not be started".into())
        }
        // It exited or closed its input without taking the whole request.
        ProcessErrorKind::StdinWrite => WorkerFailure::Protocol,
        _ => WorkerFailure::Unavailable("the worker could not be run".into()),
    })?;
    if !output.status.success() {
        return Err(WorkerFailure::Failed(diagnostics(&output.stderr)));
    }
    serde_json::from_slice(&output.stdout).map_err(|_| WorkerFailure::Protocol)
}

/// Standard error as at most 512 printable characters.
fn diagnostics(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .take(512)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Serves one request as a native worker: reads it from standard input,
/// writes the handler's response to standard output, or the handler's
/// error to standard error with a failing exit code.
pub fn serve<Request: DeserializeOwned, Response: Serialize>(
    limits: &WorkerLimits,
    handler: impl FnOnce(Request) -> Result<Response, String>,
) -> std::process::ExitCode {
    let result = read_request(std::io::stdin().lock(), limits)
        .and_then(handler)
        .and_then(|response| {
            let encoded = serde_json::to_vec(&response).map_err(|error| error.to_string())?;
            if encoded.len() > limits.response_bytes {
                return Err("the response exceeds the protocol limit".into());
            }
            std::io::stdout()
                .lock()
                .write_all(&encoded)
                .map_err(|error| error.to_string())
        });
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            // The worker reports a failure on stderr: the parent reads it.
            let _ = writeln!(std::io::stderr().lock(), "{error}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Serves a builtin worker when this process was started as one, returning
/// its exit code; `None` for an ordinary launch. Call it first in `main`.
pub fn serve_builtin<Request: DeserializeOwned, Response: Serialize>(
    limits: &WorkerLimits,
    handler: impl FnOnce(&str, Request) -> Result<Response, String>,
) -> Option<std::process::ExitCode> {
    let mut args = std::env::args_os().skip(1);
    if args.next()? != BUILTIN_FLAG {
        return None;
    }
    let id = args.next()?.into_string().ok()?;
    Some(serve(limits, |request| handler(&id, request)))
}

fn read_request<Request: DeserializeOwned>(
    input: impl Read,
    limits: &WorkerLimits,
) -> Result<Request, String> {
    let mut bytes = Vec::new();
    input
        .take(limits.request_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > limits.request_bytes {
        return Err("the request exceeds the protocol limit".into());
    }
    serde_json::from_slice(&bytes).map_err(|error| format!("invalid request: {error}"))
}

/// What one extension run settles to.
pub type RunOutcome<Response> = Result<Response, WorkerFailure>;

/// The generation a run was submitted with, its cancellation and its token.
type Running<Response> = (
    u64,
    BlockingTaskCancellation<RunOutcome<Response>>,
    CancelToken,
);

/// Runs extension actions off the UI thread on a bounded owner, keyed by
/// activity row. Cancelling a key stops its worker process and settles the
/// task as cancelled, so a late success is never applied.
pub struct ExtensionRunner<Response> {
    owner: BlockingTaskOwner,
    running: BTreeMap<ActivityKey, Running<Response>>,
    generation: u64,
}

impl<Response> std::fmt::Debug for ExtensionRunner<Response> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionRunner")
            .field("in_flight", &self.running.len())
            .finish()
    }
}

impl<Response: Send + 'static> ExtensionRunner<Response> {
    /// A runner with `workers` threads named after `thread_name` and a
    /// short queue that refuses a burst.
    #[must_use]
    pub fn new(thread_name: &str, workers: std::num::NonZeroUsize) -> Self {
        let queue = const { std::num::NonZeroUsize::new(4).expect("nonzero") };
        Self {
            owner: BlockingTaskOwner::new(thread_name, workers, queue)
                .unwrap_or_else(|_| BlockingTaskOwner::unavailable(queue)),
            running: BTreeMap::new(),
            generation: 0,
        }
    }

    /// Starts `job` (usually a closure around [`invoke`]) under `key`.
    pub fn run(
        &mut self,
        key: ActivityKey,
        job: impl FnOnce(&CancelToken) -> RunOutcome<Response> + Send + 'static,
    ) -> BlockingTask<RunOutcome<Response>> {
        self.generation = self.generation.wrapping_add(1);
        let cancel = CancelToken::new();
        let token = cancel.share();
        let task = self.owner.submit(self.generation, move |context| {
            if context.is_cancelled() {
                return Err(WorkerFailure::Cancelled);
            }
            job(&token)
        });
        // A key started again replaces its entry; the earlier run's late
        // settlement then names a generation that is no longer there.
        self.running
            .insert(key, (self.generation, task.cancellation(), cancel));
        task
    }

    /// Cancels the run behind `key`; `false` when none runs under it.
    pub fn cancel(&mut self, key: ActivityKey) -> bool {
        self.running
            .remove(&key)
            .map(|(_, task, token)| {
                token.cancel(CancelCause::Cancelled);
                task.cancel();
            })
            .is_some()
    }

    /// Forgets the run under `key` that settled with `generation`
    /// ([`BlockingTaskSettlement::generation`](enzui_core::blocking_task::BlockingTaskSettlement::generation)).
    /// A stale settlement, from a run the key has since been reused for,
    /// leaves the newer run in place.
    pub fn settled(&mut self, key: ActivityKey, generation: u64) {
        if self
            .running
            .get(&key)
            .is_some_and(|(live, _, _)| *live == generation)
        {
            self.running.remove(&key);
        }
    }

    /// Runs started and not yet settled or cancelled.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.running.len()
    }
}

#[cfg(test)]
mod reuse_tests {
    use super::*;

    fn settle<T>(task: BlockingTask<T>) -> u64 {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        runtime.block_on(task).generation()
    }

    /// A key reused while its first run is still recorded: the first run's
    /// late settlement leaves the second run's entry and cancellation alone.
    #[test]
    fn a_stale_settlement_keeps_the_newer_run() {
        let key = ActivityKey::new("reuse-test", 7);
        let mut runner: ExtensionRunner<u32> =
            ExtensionRunner::new("reuse-test", std::num::NonZeroUsize::MIN);
        let first = runner.run(key, |_| Ok(1));
        let (release, wait) = std::sync::mpsc::channel::<()>();
        let second = runner.run(key, move |_| {
            let _ = wait.recv();
            Ok(2)
        });
        let first_generation = settle(first);
        runner.settled(key, first_generation);
        assert_eq!(runner.in_flight(), 1, "the newer run is still tracked");
        let _ = release.send(());
        let second_generation = settle(second);
        assert_ne!(first_generation, second_generation);
        runner.settled(key, second_generation);
        assert_eq!(runner.in_flight(), 0);
    }
}
