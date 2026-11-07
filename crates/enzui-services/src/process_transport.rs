//! Bounded execution of helper processes.
//!
//! Requests use structured arguments, explicit input and environment policy, output limits,
//! cancellation, and a deadline. Failures report completeness per stream. One process-wide
//! reaper retains delayed exact waits; its 64 slots cover running helpers and pending cleanup.
//! Exhausted capacity or unavailable reaping refuses new helpers before spawn with
//! [`ProcessErrorKind::Setup`]. Reaper initialization failure disables admission for the
//! process lifetime.
//!
//! [`run_sensitive`] returns captures in wipe-on-drop custody. Owned request inputs,
//! arguments, environment values, scratch buffers and failure captures are wiped
//! on release; operating-system, standard-library and helper copies are outside this custody.

use crate::cancellation::{CancelCause, CancelToken};
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(5);
const TERM_GRACE: Duration = Duration::from_millis(100);
const DIAGNOSTIC_LIMIT: usize = 1024;

// Bounds post-termination residual-group and exact-child observation.
const CLEANUP_CEILING: Duration = Duration::from_secs(1);

const LEADER_REAPED_NOTE: &str = "the direct leader was reaped through its exact handle";

#[cfg(test)]
#[derive(Debug, Default)]
struct ProbeState {
    would_block: usize,
    stdout_would_block: usize,
    stderr_would_block: usize,
    stdout_bytes: usize,
    stderr_bytes: usize,
    stdout_eof: bool,
    stderr_eof: bool,
}

#[cfg(test)]
#[derive(Debug, Clone, Default)]
struct ReaderProbe {
    wipes: Arc<std::sync::Mutex<Option<enzui_security::secret::observer::Snapshot>>>,
    started: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    active: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    joined: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    progress: std::sync::Arc<(std::sync::Mutex<ProbeState>, std::sync::Condvar)>,
}

#[cfg(test)]
impl ReaderProbe {
    fn worker_started(&self) {
        let _state = self
            .progress
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.started
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.active
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.progress.1.notify_all();
    }

    fn observed_would_block(&self) {
        let mut state = self
            .progress
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.would_block += 1;
        self.progress.1.notify_all();
    }

    fn observed_stream_would_block(&self, stream: OutputStream) {
        let mut state = self
            .progress
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.would_block += 1;
        match stream {
            OutputStream::Stdout => state.stdout_would_block += 1,
            OutputStream::Stderr => state.stderr_would_block += 1,
        }
        self.progress.1.notify_all();
    }

    fn observed_stream_bytes(&self, stream: OutputStream, count: usize) {
        let mut state = self
            .progress
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match stream {
            OutputStream::Stdout => state.stdout_bytes += count,
            OutputStream::Stderr => state.stderr_bytes += count,
        }
        self.progress.1.notify_all();
    }

    fn observed_stream_eof(&self, stream: OutputStream) {
        let mut state = self
            .progress
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match stream {
            OutputStream::Stdout => state.stdout_eof = true,
            OutputStream::Stderr => state.stderr_eof = true,
        }
        self.progress.1.notify_all();
    }

    fn wait_for_stream_would_block(&self, stream: OutputStream, context: &str) {
        self.wait_until(context, |_, state| match stream {
            OutputStream::Stdout => state.stdout_would_block >= 1,
            OutputStream::Stderr => state.stderr_would_block >= 1,
        });
    }

    fn wait_for_both_readers_would_block(&self, context: &str) {
        self.wait_until(context, |_, state| {
            state.stdout_would_block >= 1 && state.stderr_would_block >= 1
        });
    }

    fn wait_for_started(&self, expected: usize, context: &str) {
        self.wait_until(context, |probe, _| {
            probe.started.load(std::sync::atomic::Ordering::Relaxed) >= expected
        });
    }

    fn wait_for_would_block(&self, expected: usize, context: &str) {
        self.wait_until(context, |_, state| state.would_block >= expected);
    }

    fn would_block_count(&self) -> usize {
        self.progress
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .would_block
    }

    fn has_would_blocked(&self) -> bool {
        self.would_block_count() != 0
    }

    fn wait_until(&self, context: &str, ready: impl Fn(&Self, &ProbeState) -> bool) {
        let until = Instant::now() + Duration::from_secs(2);
        let mut state = self
            .progress
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !ready(self, &state) {
            let remaining = until.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "{context}");
            let waited = self
                .progress
                .1
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = waited.0;
            assert!(!waited.1.timed_out() || ready(self, &state), "{context}");
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Default)]
struct SignalProbe {
    attempts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    injected_failures: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
#[derive(Debug, Clone, Default)]
struct CleanupSeam {
    elapsed_nanos: std::sync::Arc<std::sync::atomic::AtomicU64>,
    child_elapsed_nanos: std::sync::Arc<std::sync::atomic::AtomicU64>,
    polls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    transfers: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    observed_reaps: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    residual_never_clears: bool,
    child_never_exits: bool,
    exact_child_kill_fails: bool,
    observed_wait_failures: usize,
    observed_wait_failed_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    reaper_wait_failures: usize,
    reaper_wait_failed_calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    reaper_elapsed_nanos: std::sync::Arc<std::sync::atomic::AtomicU64>,
    reaper_wait_recovered: std::sync::Arc<AtomicBool>,
    reaper_poll_step: Duration,
    reaper_waited_pid: std::sync::Arc<std::sync::atomic::AtomicU32>,
    reaper_reaped_pid: std::sync::Arc<std::sync::atomic::AtomicU32>,
    reaper_no_child: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl CleanupSeam {
    fn elapsed(&self) -> Duration {
        Duration::from_nanos(
            self.elapsed_nanos
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    fn advance(&self, step: Duration) {
        let step = u64::try_from(step.as_nanos()).unwrap_or(u64::MAX);
        self.elapsed_nanos
            .fetch_add(step, std::sync::atomic::Ordering::Relaxed);
    }

    fn record_poll(&self) {
        self.polls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn child_elapsed(&self) -> Duration {
        Duration::from_nanos(
            self.child_elapsed_nanos
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    fn advance_child(&self, step: Duration) {
        let step = u64::try_from(step.as_nanos()).unwrap_or(u64::MAX);
        self.child_elapsed_nanos
            .fetch_add(step, std::sync::atomic::Ordering::Relaxed);
    }

    fn record_observed_reap(&self) {
        self.observed_reaps
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn record_transfer(&self) {
        self.transfers
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    fn observed_wait(&self, child: &mut Child) -> io::Result<ExitStatus> {
        let failed = self
            .observed_wait_failed_calls
            .load(std::sync::atomic::Ordering::Relaxed);
        if failed < self.observed_wait_failures {
            self.observed_wait_failed_calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Err(io::Error::other(
                "injected wait failure after observed exit",
            ));
        }
        child.wait()
    }

    fn reaper_try_wait(&self, child: &mut Child) -> io::Result<Option<ExitStatus>> {
        self.reaper_waited_pid.store(child.id(), Ordering::Relaxed);
        let failed = self.reaper_wait_failed_calls.load(Ordering::Relaxed);
        if !self.reaper_wait_recovered.load(Ordering::Acquire) && failed < self.reaper_wait_failures
        {
            self.reaper_wait_failed_calls
                .fetch_add(1, Ordering::Relaxed);
            let step = self.reaper_poll_step.max(POLL_INTERVAL);
            self.reaper_elapsed_nanos.fetch_add(
                u64::try_from(step.as_nanos()).unwrap_or(u64::MAX),
                Ordering::Release,
            );
            return Err(io::Error::other("injected rescue wait failure"));
        }
        let result = child.try_wait();
        if matches!(&result, Ok(Some(_))) {
            self.reaper_reaped_pid.store(child.id(), Ordering::Release);
        } else if result.as_ref().is_err_and(is_permanent_no_child) {
            self.reaper_no_child.fetch_add(1, Ordering::Release);
        }
        result
    }
}

#[cfg(test)]
struct WorkerActivity(ReaderProbe);

#[cfg(test)]
impl WorkerActivity {
    fn enter(probe: ReaderProbe) -> Self {
        probe.worker_started();
        Self(probe)
    }
}

#[cfg(test)]
impl Drop for WorkerActivity {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering;
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Which captured pipe an output-related failure names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutputStream {
    /// The helper's standard output.
    Stdout,
    /// The helper's standard error.
    Stderr,
}

/// One bounded chunk observed while a helper is running.
#[derive(Clone, PartialEq, Eq)]
pub struct ProcessOutputChunk {
    /// The pipe that produced the bytes.
    pub stream: OutputStream,
    /// At most 8 KiB read from the pipe.
    pub bytes: Vec<u8>,
}

impl fmt::Debug for ProcessOutputChunk {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessOutputChunk")
            .field("stream", &self.stream)
            .field("bytes", &RedactedBytes(self.bytes.len()))
            .finish()
    }
}

/// Completeness of one captured stream after a failed invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CaptureCompleteness {
    /// The reader reached end of file within the limit, so the capture is whole.
    Complete,
    /// The stream exceeded its limit; exactly `limit + 1` prefix bytes are retained.
    OverflowTruncated,
    /// The reader stopped before end of file and retained an arbitrary bounded prefix.
    StoppedPartial,
}

impl CaptureCompleteness {
    /// Returns whether the capture contains every byte written to the stream.
    #[must_use]
    pub fn is_complete(self) -> bool {
        matches!(self, Self::Complete)
    }
}

/// What the helper receives as standard input.
#[derive(Clone, PartialEq, Eq)]
pub enum StdinPolicy {
    /// Connect stdin to the null device. The helper observes immediate EOF.
    Null,
    /// Write these bytes through a pipe, then close it.
    Bytes(Vec<u8>),
}

/// Environment authority used to spawn the helper child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvPolicy {
    /// Clears the child environment and applies only explicit additions.
    ///
    /// A bare executable also requires an explicitly supplied `PATH` made only
    /// of nonempty absolute directories.
    Cleared,
    /// Inherits the parent environment, then applies explicit additions.
    ///
    /// A bare executable is resolved once through the resulting `PATH`, whose
    /// components must all be nonempty absolute directories.
    Inherit,
}

/// A structured helper invocation; arguments are passed directly without a shell.
#[derive(Clone)]
pub struct ProcessRequest {
    executable: OsString,
    argv: Vec<OsString>,
    stdin: StdinPolicy,
    stdout_limit: usize,
    stderr_limit: usize,
    deadline: Duration,
    env_policy: EnvPolicy,
    env: Vec<(OsString, OsString)>,
    current_dir: Option<PathBuf>,
    background: Option<BackgroundScope>,
}

impl Drop for ProcessRequest {
    fn drop(&mut self) {
        if let StdinPolicy::Bytes(bytes) = &mut self.stdin {
            drop(enzui_security::secret::Secret::new(std::mem::take(bytes)));
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            for argument in &mut self.argv {
                drop(enzui_security::secret::Secret::new(
                    std::mem::take(argument).into_vec(),
                ));
            }
            for (_, value) in &mut self.env {
                drop(enzui_security::secret::Secret::new(
                    std::mem::take(value).into_vec(),
                ));
            }
        }
    }
}

impl ProcessRequest {
    /// Creates a request with a cleared environment and absolute-only executable authority.
    #[must_use]
    pub fn new(
        executable: impl Into<OsString>,
        stdin: StdinPolicy,
        stdout_limit: usize,
        stderr_limit: usize,
        deadline: Duration,
    ) -> Self {
        Self {
            executable: executable.into(),
            argv: Vec::new(),
            stdin,
            stdout_limit,
            stderr_limit,
            deadline,
            env_policy: EnvPolicy::Cleared,
            env: Vec::new(),
            current_dir: None,
            background: None,
        }
    }

    /// Appends one structured argument.
    #[must_use]
    pub fn arg(mut self, argument: impl Into<OsString>) -> Self {
        self.argv.push(argument.into());
        self
    }

    /// Appends structured arguments in order.
    #[must_use]
    pub fn args<I, S>(mut self, arguments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.argv.extend(arguments.into_iter().map(Into::into));
        self
    }

    /// The requested executable before bare-name authority resolution.
    #[must_use]
    pub fn executable(&self) -> &std::ffi::OsStr {
        &self.executable
    }

    /// The structured argument vector.
    #[must_use]
    pub fn argv(&self) -> &[OsString] {
        &self.argv
    }

    /// The configured standard-input policy.
    #[must_use]
    pub fn stdin(&self) -> &StdinPolicy {
        &self.stdin
    }

    /// The maximum successful standard-output length.
    #[must_use]
    pub fn stdout_limit(&self) -> usize {
        self.stdout_limit
    }

    /// The maximum successful standard-error length.
    #[must_use]
    pub fn stderr_limit(&self) -> usize {
        self.stderr_limit
    }

    /// The total running deadline, measured from immediately before spawn.
    #[must_use]
    pub fn deadline(&self) -> Duration {
        self.deadline
    }

    /// Adds or replaces one child variable.
    ///
    /// In the cleared lane, a valid explicit `PATH` authorizes one-time lookup
    /// of a bare executable name before the child working directory is applied.
    #[must_use]
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        let key = key.into();
        let value = value.into();
        if let Some(existing) = self.env.iter_mut().find(|(name, _)| *name == key) {
            existing.1 = value;
        } else {
            self.env.push((key, value));
        }
        self
    }

    /// Adds variables in order, with later duplicate keys replacing earlier values.
    #[must_use]
    pub fn envs<I, K, V>(mut self, vars: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<OsString>,
        V: Into<OsString>,
    {
        for (key, value) in vars {
            self = self.env(key, value);
        }
        self
    }

    /// Opts into inheriting the parent environment.
    ///
    /// Bare executable lookup still requires every resulting `PATH` component
    /// to be a nonempty absolute directory.
    #[must_use]
    pub fn inherit_environment(mut self) -> Self {
        self.env_policy = EnvPolicy::Inherit;
        self
    }

    /// The configured environment authority for the child.
    #[must_use]
    pub fn env_policy(&self) -> &EnvPolicy {
        &self.env_policy
    }

    /// Returns additions, complete under `Cleared` and overlays under `Inherit`.
    #[must_use]
    pub fn env_vars(&self) -> &[(OsString, OsString)] {
        &self.env
    }

    /// Sets an explicit child working directory applied after executable resolution.
    #[must_use]
    pub fn current_dir(mut self, directory: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(directory.into());
        self
    }

    /// Returns the configured child working directory, if any.
    #[must_use]
    pub fn working_directory(&self) -> Option<&Path> {
        self.current_dir.as_deref()
    }

    /// Apply best-effort background scheduling to a sustained helper workload.
    ///
    /// Requests a transient user scope and the supplied CPU/I/O policy, falling
    /// back to installed scheduling tools if scopes are unavailable or disabled.
    /// See [`background`] for environment and scheduling limitations.
    #[must_use]
    pub fn background(mut self, scope: BackgroundScope) -> Self {
        self.background = Some(scope);
        self
    }

    /// The background policy, if [`Self::background`] was set.
    #[must_use]
    pub fn background_scope(&self) -> Option<&BackgroundScope> {
        self.background.as_ref()
    }
}

struct RedactedValues(usize);

impl fmt::Debug for RedactedValues {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "<{} values redacted>", self.0)
    }
}

// Redaction boundary for captured helper output: exposes only a stream's byte
// length, never its contents, lossy text, or decimal byte-vector.
struct RedactedBytes(usize);

impl fmt::Debug for RedactedBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "<{} bytes redacted>", self.0)
    }
}

impl fmt::Debug for StdinPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => formatter.write_str("Null"),
            Self::Bytes(bytes) => formatter
                .debug_struct("Bytes")
                .field("len", &bytes.len())
                .finish(),
        }
    }
}

impl fmt::Debug for ProcessRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessRequest")
            .field("executable", &self.executable)
            .field("argv", &RedactedValues(self.argv.len()))
            .field("stdin", &self.stdin)
            .field("stdout_limit", &self.stdout_limit)
            .field("stderr_limit", &self.stderr_limit)
            .field("deadline", &self.deadline)
            .field("env_policy", &self.env_policy)
            .field("env", &RedactedValues(self.env.len()))
            .field("has_current_dir", &self.current_dir.is_some())
            .field("background", &self.background)
            .finish()
    }
}

/// A complete bounded response; a nonzero exit is still a complete transport response.
pub struct ProcessOutput {
    /// The reaped direct child's exit status.
    pub status: ExitStatus,
    /// Complete standard output, no longer than its requested limit.
    pub stdout: Vec<u8>,
    /// Complete standard error, no longer than its requested limit.
    pub stderr: Vec<u8>,
}

impl fmt::Debug for ProcessOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessOutput")
            .field("status", &self.status)
            .field("stdout", &RedactedBytes(self.stdout.len()))
            .field("stderr", &RedactedBytes(self.stderr.len()))
            .finish()
    }
}

/// Typed reason a helper did not produce a complete response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessErrorKind {
    /// A limit was `usize::MAX`, for which retaining limit+1 is impossible.
    InvalidLimit(OutputStream),
    /// Cancellation before or during execution; each stream reports its own completeness.
    Cancelled(CancelCause),
    /// The total deadline elapsed; each stream reports its own completeness.
    Deadline,
    /// The request lacked authority to resolve its executable.
    ///
    /// Relative paths and invalid search directories are refused before
    /// resource acquisition without exposing caller-controlled bytes.
    ExecutableAuthority,
    /// The executable could not be resolved through an authorized path or spawned.
    Spawn,
    /// Rescue admission, pipe setup, or worker establishment failed.
    Setup,
    /// Writing the explicit stdin bytes failed.
    StdinWrite,
    /// Reading one output pipe failed.
    Read(OutputStream),
    /// An output reader panicked. The sibling is still terminated and joined.
    ReaderPanicked(OutputStream),
    /// A non-reader worker panicked.
    WorkerPanicked,
    /// One stream exceeded its limit; its sibling retains independently typed capture.
    OutputLimitExceeded(OutputStream),
    /// Querying or reaping the child failed after reader results were collected.
    Wait,
    /// Group signalling failed or identity was unproven; cleanup uses the exact child handle.
    GroupSignal,
    /// Residual members remained at the cleanup bound or group scanning failed.
    ResidualCleanup,
    /// Cleanup exceeded the caller bound; the process-wide reaper owns the pending exact wait.
    DirectChildCleanup,
}

#[derive(Debug)]
struct TeardownError {
    kind: ProcessErrorKind,
    detail: String,
}

impl TeardownError {
    fn wait(detail: String) -> Self {
        Self {
            kind: ProcessErrorKind::Wait,
            detail,
        }
    }

    fn cleanup(detail: String) -> Self {
        Self {
            kind: ProcessErrorKind::ResidualCleanup,
            detail: bounded_diagnostic(detail),
        }
    }

    fn direct_cleanup(detail: String) -> Self {
        Self {
            kind: ProcessErrorKind::DirectChildCleanup,
            detail: bounded_diagnostic(detail),
        }
    }

    fn group_signal(detail: String) -> Self {
        Self {
            kind: ProcessErrorKind::GroupSignal,
            detail: bounded_diagnostic(detail),
        }
    }

    fn detail(&self) -> &str {
        &self.detail
    }

    fn with_note(mut self, note: &str) -> Self {
        self.detail = bounded_diagnostic(format!("{}; {note}", self.detail));
        self
    }

    fn with_secondary(mut self, secondary: Self) -> Self {
        self.detail = bounded_diagnostic(format!(
            "{}; additionally, {:?}: {}",
            self.detail, secondary.kind, secondary.detail
        ));
        self
    }
}

/// A failed invocation with bounded captures and per-stream completeness.
#[derive(Clone, PartialEq, Eq)]
pub struct ProcessError {
    kind: ProcessErrorKind,
    stdout: Vec<u8>,
    stdout_completeness: CaptureCompleteness,
    stderr: Vec<u8>,
    stderr_completeness: CaptureCompleteness,
    diagnostic: Option<String>,
}

impl Drop for ProcessError {
    fn drop(&mut self) {
        drop(enzui_security::secret::Secret::new(std::mem::take(
            &mut self.stdout,
        )));
        drop(enzui_security::secret::Secret::new(std::mem::take(
            &mut self.stderr,
        )));
    }
}

impl fmt::Debug for ProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessError")
            .field("kind", &self.kind)
            .field("stdout", &RedactedBytes(self.stdout.len()))
            .field("stdout_completeness", &self.stdout_completeness)
            .field("stderr", &RedactedBytes(self.stderr.len()))
            .field("stderr_completeness", &self.stderr_completeness)
            .field("diagnostic", &self.diagnostic)
            .finish()
    }
}

impl ProcessError {
    /// The typed terminal reason.
    #[must_use]
    pub fn kind(&self) -> &ProcessErrorKind {
        &self.kind
    }

    /// Returns bounded stdout; consult `stdout_completeness` before treating it as whole.
    #[must_use]
    pub fn stdout(&self) -> &[u8] {
        &self.stdout
    }

    /// Returns bounded stderr; consult `stderr_completeness` before treating it as whole.
    #[must_use]
    pub fn stderr(&self) -> &[u8] {
        &self.stderr
    }

    /// How much of standard output this error carries.
    #[must_use]
    pub fn stdout_completeness(&self) -> CaptureCompleteness {
        self.stdout_completeness
    }

    /// How much of standard error this error carries.
    #[must_use]
    pub fn stderr_completeness(&self) -> CaptureCompleteness {
        self.stderr_completeness
    }

    /// How much of the named stream this error carries.
    #[must_use]
    pub fn completeness(&self, stream: OutputStream) -> CaptureCompleteness {
        match stream {
            OutputStream::Stdout => self.stdout_completeness,
            OutputStream::Stderr => self.stderr_completeness,
        }
    }

    /// The bounded bytes retained for the named stream, of unspecified
    /// completeness.
    #[must_use]
    pub fn capture(&self, stream: OutputStream) -> &[u8] {
        match stream {
            OutputStream::Stdout => &self.stdout,
            OutputStream::Stderr => &self.stderr,
        }
    }

    /// Returns a stream’s bytes only when the reader proved them complete.
    #[must_use]
    pub fn complete_capture(&self, stream: OutputStream) -> Option<&[u8]> {
        self.completeness(stream)
            .is_complete()
            .then(|| self.capture(stream))
    }

    /// A separately bounded setup/I/O diagnostic, when one exists.
    ///
    /// This is framework prose about the failure, bounded by its own separate
    /// limit and independent of both pipe captures; it is never helper output.
    #[must_use]
    pub fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }
}

impl fmt::Display for ProcessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "helper process failed: {:?}", self.kind)?;
        if let Some(detail) = &self.diagnostic {
            write!(formatter, " ({detail})")?;
        }
        Ok(())
    }
}

impl Error for ProcessError {}

/// Runs one helper synchronously and returns output only after both streams complete.
///
/// # Errors
/// Returns `ProcessError` for cancellation, deadline, overflow, setup, I/O, wait, cleanup, or worker failure.
pub fn run(request: ProcessRequest, cancel: &CancelToken) -> Result<ProcessOutput, ProcessError> {
    run_inner(request, cancel, Faults::default())
}

/// Runs one helper while publishing best-effort output chunks to a bounded channel.
///
/// Chunk delivery never blocks process draining: a full or disconnected channel
/// drops that chunk. The returned [`ProcessOutput`] remains the authoritative
/// complete capture and enforces the request's stream limits.
///
/// # Errors
/// Returns [`ProcessError`] under the same conditions as [`run`].
pub fn run_streaming(
    request: ProcessRequest,
    cancel: &CancelToken,
    output: SyncSender<ProcessOutputChunk>,
) -> Result<ProcessOutput, ProcessError> {
    run_inner_observed(request, cancel, Faults::default(), Some(output))
}

/// Complete helper output whose captured streams are wiped when dropped.
#[derive(Debug)]
pub struct SensitiveProcessOutput {
    /// The reaped direct child's exit status.
    pub status: ExitStatus,
    /// Complete standard output in wipe-on-drop custody.
    pub stdout: enzui_security::secret::Secret,
    /// Complete standard error in wipe-on-drop custody.
    pub stderr: enzui_security::secret::Secret,
}

/// Runs a bounded helper and retains successful captures in wipe-on-drop custody.
///
/// Request input, owned argument/environment values and abandoned or failed captures
/// are also wiped; copies held by the operating system or helper are outside this custody.
///
/// # Errors
/// Returns [`ProcessError`] under the same conditions as [`run`].
pub fn run_sensitive(
    request: ProcessRequest,
    cancel: &CancelToken,
) -> Result<SensitiveProcessOutput, ProcessError> {
    run(request, cancel).map(|output| SensitiveProcessOutput {
        status: output.status,
        stdout: enzui_security::secret::Secret::new(output.stdout),
        stderr: enzui_security::secret::Secret::new(output.stderr),
    })
}

#[cfg(test)]
impl OutputStream {
    fn sibling(self) -> Self {
        match self {
            Self::Stdout => Self::Stderr,
            Self::Stderr => Self::Stdout,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkerKind {
    Reader(OutputStream),
    Stdin,
}

#[derive(Debug)]
struct ReadResult {
    bytes: Vec<u8>,
    outcome: ReadOutcome,
}

impl Drop for ReadResult {
    fn drop(&mut self) {
        drop(enzui_security::secret::Secret::new(std::mem::take(
            &mut self.bytes,
        )));
    }
}

#[derive(Debug)]
enum ReadOutcome {
    Complete,
    Overflow,
    Stopped,
    Failed(String),
}

#[derive(Debug)]
enum WorkerEvent {
    Read(OutputStream, ReadResult),
    Stdin(Result<(), String>),
    Panicked(WorkerKind),
}

struct WorkerHandle {
    kind: WorkerKind,
    handle: JoinHandle<()>,
    joined: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

// Nonblocking pipe workers observe this stop request between bounded readiness waits.
#[derive(Clone, Default)]
struct WorkerStop(Arc<AtomicBool>);

impl WorkerStop {
    fn request(&self) {
        self.0.store(true, Ordering::Release);
    }

    fn requested(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

struct ProcessOwner {
    child: Option<Child>,
    group: platform::OwnedGroup,
    workers: Vec<WorkerHandle>,
    worker_stop: WorkerStop,
    reaper: Option<ReaperPermit>,
    /// The background scope the helper runs in, killed after the group.
    scope_unit: Option<background::ScopeUnit>,
    #[cfg(test)]
    cleanup_seam: Option<CleanupSeam>,
}

impl ProcessOwner {
    fn new(child: Child, reaper: ReaperPermit, faults: &Faults) -> Self {
        let group = platform::OwnedGroup::capture(&child, faults);
        Self {
            child: Some(child),
            group,
            workers: Vec::new(),
            worker_stop: WorkerStop::default(),
            reaper: Some(reaper),
            scope_unit: None,
            #[cfg(test)]
            cleanup_seam: faults.cleanup_seam.clone(),
        }
    }

    fn child_mut(&mut self) -> &mut Child {
        self.child.as_mut().expect("owned child is present")
    }

    fn push_worker(
        &mut self,
        kind: WorkerKind,
        handle: JoinHandle<()>,
        joined: Option<Arc<std::sync::atomic::AtomicUsize>>,
    ) {
        self.workers.push(WorkerHandle {
            kind,
            handle,
            joined,
        });
    }

    fn setup_error(&self) -> Option<String> {
        self.group.setup_error()
    }

    fn worker_stop(&self) -> WorkerStop {
        self.worker_stop.clone()
    }

    #[cfg(target_os = "linux")]
    fn complete_if_exited(&mut self) -> Result<Option<ExitStatus>, TeardownError> {
        let Some(child) = self.child.as_mut() else {
            return Err(TeardownError::wait("child was already reaped".to_owned()));
        };
        if matches!(
            self.group
                .probe_leader(child)
                .map_err(TeardownError::wait)?,
            platform::LeaderProbe::Running
        ) {
            return Ok(None);
        }

        // Keep the leader waitable through numeric signalling and residual observation, preventing PGID reuse.
        let cleanup = self
            .group
            .kill_residual()
            .and_then(|()| self.group.wait_for_residuals());

        let reaped = self.reap_observed_leader();
        match (cleanup, reaped) {
            (Ok(()), Ok(status)) => Ok(Some(status)),
            (Ok(()), Err(direct)) => Err(direct),
            (Err(cleanup), Ok(_)) => Err(cleanup.with_note(LEADER_REAPED_NOTE)),
            (Err(cleanup), Err(direct)) => Err(cleanup.with_secondary(direct)),
        }
    }

    // Retry an observed leader’s exact wait under the cleanup bound; transfer ownership if status remains unavailable.
    #[cfg(target_os = "linux")]
    fn reap_observed_leader(&mut self) -> Result<ExitStatus, TeardownError> {
        self.group.release_identity();
        let Some(mut child) = self.child.take() else {
            return Err(TeardownError::wait("child was already reaped".to_owned()));
        };
        #[cfg(test)]
        if let Some(seam) = &self.cleanup_seam {
            seam.record_observed_reap();
        }

        #[cfg(test)]
        let seam = self.cleanup_seam.clone();
        #[cfg(test)]
        let outcome = match seam {
            Some(seam) => {
                let clock = seam.clone();
                wait_observed_leader(
                    &mut child,
                    |child| seam.observed_wait(child),
                    || clock.child_elapsed(),
                    |step| clock.advance_child(step),
                )
            }
            None => wait_observed_leader(
                &mut child,
                Child::wait,
                {
                    let start = Instant::now();
                    move || start.elapsed()
                },
                thread::sleep,
            ),
        };
        #[cfg(not(test))]
        let outcome = wait_observed_leader(
            &mut child,
            Child::wait,
            {
                let start = Instant::now();
                move || start.elapsed()
            },
            thread::sleep,
        );

        match outcome {
            Ok(status) => Ok(status),
            Err(detail) => {
                // Transfer the child rather than dropping a handle whose status was not observed.
                let transferred = self.transfer_wait_ownership(child);
                Err(TeardownError::direct_cleanup(bounded_diagnostic(format!(
                    "wait after observed exit kept failing ({detail}); {}",
                    transferred.detail()
                ))))
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    fn complete_if_exited(&mut self) -> Result<Option<ExitStatus>, TeardownError> {
        let Some(child) = self.child.as_mut() else {
            return Err(TeardownError::wait("child was already reaped".to_owned()));
        };
        match self
            .group
            .probe_leader(child)
            .map_err(TeardownError::wait)?
        {
            platform::LeaderProbe::Running => Ok(None),
            platform::LeaderProbe::Reaped(status) => {
                self.child.take();
                Ok(Some(status))
            }
        }
    }

    fn terminate_and_reap(&mut self) -> Result<(), TeardownError> {
        // Stop pipe workers before cleanup because an escaped process may retain peer descriptors.
        self.worker_stop.request();
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };

        // Successful termination requires accepted group kill and observed absence while identity remains pinned.
        let outcome = self.group.terminate(child, TERM_GRACE);

        // After numeric signalling, use only the exact child handle, which cannot alias a reused PID.
        let direct = self.release_via_child_handle();

        // Scope cleanup can reach descendants outside the process group if placement succeeded.
        if let Some(scope) = self.scope_unit.take() {
            scope.kill_remaining();
        }

        // Preserve group failure as primary while reporting a delayed exact-child owner.
        match (outcome, direct) {
            (Ok(()), direct) => direct,
            (Err(error), Ok(())) => Err(error.with_note(LEADER_REAPED_NOTE)),
            (Err(error), Err(direct)) => Err(error.with_secondary(direct)),
        }
    }

    // After numeric identity release, use the exact child handle and transfer delayed wait ownership.
    fn release_via_child_handle(&mut self) -> Result<(), TeardownError> {
        self.group.release_identity();
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        // A kill error is not actionable here: the usual cause is that the
        // child already exited, and the bounded observation below still has to
        // reap it either way.
        #[cfg(test)]
        let inject_kill_failure = self
            .cleanup_seam
            .as_ref()
            .is_some_and(|seam| seam.exact_child_kill_fails);
        #[cfg(not(test))]
        let inject_kill_failure = false;
        if !inject_kill_failure {
            let _ = child.kill();
        }

        #[cfg(test)]
        let stub = self
            .cleanup_seam
            .clone()
            .filter(|seam| seam.child_never_exits);
        #[cfg(test)]
        let reaped = match stub {
            Some(seam) => {
                let wait_seam = seam;
                observe_child_exit(
                    &mut child,
                    |_| false,
                    || wait_seam.child_elapsed(),
                    |step| wait_seam.advance_child(step),
                )
            }
            None => observe_exact_child(&mut child),
        };
        #[cfg(not(test))]
        let reaped = observe_exact_child(&mut child);

        if reaped {
            return Ok(());
        }
        Err(self.transfer_wait_ownership(child))
    }

    fn transfer_wait_ownership(&mut self, child: Child) -> TeardownError {
        #[cfg(test)]
        if let Some(seam) = &self.cleanup_seam {
            seam.record_transfer();
        }
        self.reaper
            .take()
            .expect("each owned helper reserves one rescue slot before spawn")
            .handoff(
                child,
                #[cfg(test)]
                self.cleanup_seam.clone(),
            );
        TeardownError::direct_cleanup(
            "exact child wait ownership transferred to the process-wide rescue reaper; \
             cleanup pending after bounded observation"
                .to_owned(),
        )
    }

    fn join_all(&mut self) -> Vec<WorkerKind> {
        let mut panicked = Vec::new();
        for worker in self.workers.drain(..) {
            if worker.handle.join().is_err() {
                panicked.push(worker.kind);
            }
            if let Some(joined) = worker.joined {
                joined.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        }
        panicked
    }
}

impl Drop for ProcessOwner {
    fn drop(&mut self) {
        let _ = self.terminate_and_reap();
        let _ = self.join_all();
    }
}

fn exact_child_exited(child: &mut Child) -> bool {
    matches!(child.try_wait(), Ok(Some(_)))
}

fn observe_exact_child(child: &mut Child) -> bool {
    let start = Instant::now();
    observe_child_exit(
        child,
        exact_child_exited,
        move || start.elapsed(),
        thread::sleep,
    )
}

fn observe_child_exit(
    child: &mut Child,
    mut exited: impl FnMut(&mut Child) -> bool,
    mut elapsed: impl FnMut() -> Duration,
    mut wait: impl FnMut(Duration),
) -> bool {
    loop {
        if exited(child) {
            return true;
        }
        if elapsed() >= CLEANUP_CEILING {
            return false;
        }
        wait(POLL_INTERVAL);
    }
}

// Retry a failed wait until the cleanup ceiling while retaining the exact handle.
fn wait_observed_leader(
    child: &mut Child,
    mut wait: impl FnMut(&mut Child) -> io::Result<ExitStatus>,
    mut elapsed: impl FnMut() -> Duration,
    mut pause: impl FnMut(Duration),
) -> Result<ExitStatus, String> {
    let mut last;
    loop {
        match wait(child) {
            Ok(status) => return Ok(status),
            Err(error) => last = error.to_string(),
        }
        if elapsed() >= CLEANUP_CEILING {
            return Err(last);
        }
        pause(POLL_INTERVAL);
    }
}

const REAPER_CAPACITY: usize = 64;
const REAPER_VACANT: u8 = 0;
const REAPER_RESERVED: u8 = 1;
const REAPER_PENDING: u8 = 2;
const REAPER_READINESS_CEILING: Duration = Duration::from_secs(2);

// Static custody outlives callers and runtimes, including a worker that fails readiness.
static PROCESS_REAPER: ReaperRegistry = ReaperRegistry::new();

#[derive(Debug)]
struct ReaperRegistry {
    service: std::sync::OnceLock<ReaperService>,
    #[cfg(test)]
    start_fault: ReaperStartFault,
    #[cfg(test)]
    initializations: std::sync::atomic::AtomicUsize,
}

impl ReaperRegistry {
    const fn new() -> Self {
        Self {
            service: std::sync::OnceLock::new(),
            #[cfg(test)]
            start_fault: ReaperStartFault::None,
            #[cfg(test)]
            initializations: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn reserve(&'static self) -> io::Result<ReaperPermit> {
        self.service
            .get_or_init(|| {
                #[cfg(test)]
                self.initializations.fetch_add(1, Ordering::Relaxed);
                ReaperService::start(
                    #[cfg(test)]
                    self.start_fault,
                )
            })
            .reserve()
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReaperStartFault {
    None,
    Spawn,
    Readiness,
}

#[derive(Debug)]
struct RescuedChild {
    child: Child,
    #[cfg(test)]
    seam: Option<CleanupSeam>,
}

#[derive(Debug)]
struct ReaperSlot {
    state: AtomicU8,
    child: std::sync::Mutex<Option<RescuedChild>>,
}

impl ReaperSlot {
    fn poll(&self) {
        if self.state.load(Ordering::Acquire) != REAPER_PENDING {
            return;
        }
        let completed = {
            let mut pending = self
                .child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(owned) = pending.as_mut() else {
                return;
            };
            #[cfg(test)]
            let seam = &owned.seam;
            #[cfg(test)]
            let outcome = reap_exact_child(&mut owned.child, |child| match seam {
                Some(seam) => seam.reaper_try_wait(child),
                None => child.try_wait(),
            });
            #[cfg(not(test))]
            let outcome = reap_exact_child(&mut owned.child, Child::try_wait);
            let completed = outcome.is_some();
            if completed {
                pending.take();
            }
            completed
        };
        if completed {
            self.state.store(REAPER_VACANT, Ordering::SeqCst);
        }
    }
}

#[derive(Debug)]
struct ReaperShared {
    slots: [Arc<ReaperSlot>; REAPER_CAPACITY],
    closing: AtomicBool,
    /// How many times the worker has looked at its slots.
    #[cfg(test)]
    rounds: std::sync::atomic::AtomicUsize,
}

impl ReaperShared {
    fn is_empty(&self) -> bool {
        self.slots
            .iter()
            .all(|slot| slot.state.load(Ordering::SeqCst) == REAPER_VACANT)
    }

    fn run(&self) {
        loop {
            for slot in &self.slots {
                slot.poll();
            }
            // Check vacancies after closing admission; preissued reservations must still be serviced.
            if self.closing.load(Ordering::SeqCst) && self.is_empty() {
                return;
            }
            #[cfg(test)]
            self.rounds.fetch_add(1, Ordering::Relaxed);
            // Only a handed-off child needs polling. Otherwise sleep until a
            // hand-off, a released reservation or shutdown unparks this thread;
            // an unpark that lands before the park leaves its token, so none is
            // missed.
            if self
                .slots
                .iter()
                .any(|slot| slot.state.load(Ordering::Acquire) == REAPER_PENDING)
            {
                thread::park_timeout(POLL_INTERVAL);
            } else {
                thread::park();
            }
        }
    }
}

#[derive(Debug)]
struct ReaperService {
    shared: Arc<ReaperShared>,
    worker: std::sync::Mutex<Option<JoinHandle<()>>>,
    start_error: Option<String>,
}

impl ReaperService {
    fn start(#[cfg(test)] fault: ReaperStartFault) -> Self {
        let shared = Arc::new(ReaperShared {
            slots: std::array::from_fn(|_| {
                Arc::new(ReaperSlot {
                    state: AtomicU8::new(REAPER_VACANT),
                    child: std::sync::Mutex::new(None),
                })
            }),
            closing: AtomicBool::new(false),
            #[cfg(test)]
            rounds: std::sync::atomic::AtomicUsize::new(0),
        });
        let worker_shared = Arc::clone(&shared);
        let (ready_sender, ready_receiver) = mpsc::channel();
        let spawn = || {
            thread::Builder::new()
                .name("enzui-process-rescue-reaper".to_owned())
                .spawn(move || {
                    #[cfg(test)]
                    let send_ready = fault != ReaperStartFault::Readiness;
                    #[cfg(not(test))]
                    let send_ready = true;
                    if send_ready && ready_sender.send(()).is_err() {
                        return;
                    }
                    worker_shared.run();
                })
        };
        #[cfg(test)]
        let spawned = if fault == ReaperStartFault::Spawn {
            Err(io::Error::other("injected rescue thread creation failure"))
        } else {
            spawn()
        };
        #[cfg(not(test))]
        let spawned = spawn();
        let worker = match spawned {
            Ok(worker) => worker,
            Err(error) => {
                return Self {
                    shared,
                    worker: std::sync::Mutex::new(None),
                    start_error: Some(format!("rescue thread creation: {error}")),
                };
            }
        };
        let mut service = Self {
            shared,
            worker: std::sync::Mutex::new(Some(worker)),
            start_error: None,
        };
        #[cfg(test)]
        let readiness = if fault == ReaperStartFault::Readiness {
            Duration::ZERO
        } else {
            REAPER_READINESS_CEILING
        };
        #[cfg(not(test))]
        let readiness = REAPER_READINESS_CEILING;
        if let Err(error) = ready_receiver.recv_timeout(readiness) {
            service.start_error = Some(format!("rescue readiness: {error}"));
            service.request_shutdown();
            service.join_if_finished();
        }
        service
    }

    fn reserve(&'static self) -> io::Result<ReaperPermit> {
        if let Some(error) = &self.start_error {
            self.join_if_finished();
            return Err(io::Error::other(error.clone()));
        }
        let wake = {
            let worker = self
                .worker
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match worker.as_ref() {
                Some(worker) if !worker.is_finished() => worker.thread().clone(),
                _ => return Err(io::Error::other("rescue worker is unavailable")),
            }
        };
        if self.shared.closing.load(Ordering::SeqCst) {
            return Err(io::Error::other("rescue admission is closed"));
        }
        for slot in &self.shared.slots {
            if slot
                .state
                .compare_exchange(
                    REAPER_VACANT,
                    REAPER_RESERVED,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
            {
                let permit = ReaperPermit {
                    slot: Arc::clone(slot),
                    wake,
                    reserved: true,
                };
                if self.shared.closing.load(Ordering::SeqCst) {
                    return Err(io::Error::other("rescue admission is closed"));
                }
                return Ok(permit);
            }
        }
        Err(io::Error::other(
            "rescue capacity exhausted by running helpers or pending cleanup",
        ))
    }

    fn request_shutdown(&self) {
        self.shared.closing.store(true, Ordering::SeqCst);
        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            worker.thread().unpark();
        }
    }

    fn join_if_finished(&self) -> bool {
        let mut worker = self
            .worker
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if worker.as_ref().is_some_and(|worker| !worker.is_finished()) {
            return false;
        }
        if let Some(worker) = worker.take() {
            let _ = worker.join();
        }
        true
    }
}

// Only a vacant slot issues a non-cloneable permit; only a terminal exact wait recycles a published slot.
#[derive(Debug)]
struct ReaperPermit {
    slot: Arc<ReaperSlot>,
    wake: thread::Thread,
    reserved: bool,
}

impl ReaperPermit {
    fn handoff(mut self, child: Child, #[cfg(test)] seam: Option<CleanupSeam>) {
        {
            let mut pending = self
                .slot
                .child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *pending = Some(RescuedChild {
                child,
                #[cfg(test)]
                seam,
            });
        }
        self.reserved = false;
        self.slot.state.store(REAPER_PENDING, Ordering::Release);
        self.wake.unpark();
    }
}

impl Drop for ReaperPermit {
    fn drop(&mut self) {
        if self.reserved {
            self.slot.state.store(REAPER_VACANT, Ordering::SeqCst);
            self.wake.unpark();
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ReapOutcome {
    Reaped,
    NoChild,
}

fn reap_exact_child(
    child: &mut Child,
    try_wait: impl FnOnce(&mut Child) -> io::Result<Option<ExitStatus>>,
) -> Option<ReapOutcome> {
    match try_wait(child) {
        Ok(Some(_)) => Some(ReapOutcome::Reaped),
        Err(error) if is_permanent_no_child(&error) => Some(ReapOutcome::NoChild),
        Ok(None) | Err(_) => None,
    }
}

// `io::ErrorKind` has no ECHILD variant; other wait errors do not release custody.
#[cfg(unix)]
fn is_permanent_no_child(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ECHILD)
}

#[cfg(not(unix))]
fn is_permanent_no_child(_error: &io::Error) -> bool {
    false
}

fn establish_reaper(faults: &Faults) -> io::Result<ReaperPermit> {
    #[cfg(test)]
    if faults.fail_reaper_start {
        return Err(io::Error::other("injected reaper start failure"));
    }
    #[cfg(test)]
    let registry = faults.reaper_registry.unwrap_or(&PROCESS_REAPER);
    #[cfg(not(test))]
    let registry = {
        let _ = faults;
        &PROCESS_REAPER
    };
    registry.reserve()
}

#[derive(Debug, Default, Clone)]
struct Faults {
    #[cfg(test)]
    panic_reader: Option<OutputStream>,
    #[cfg(test)]
    fail_reader: Option<OutputStream>,
    #[cfg(test)]
    fail_setup_after_readers: bool,
    #[cfg(test)]
    fail_wait: bool,
    #[cfg(test)]
    deadline_after_stdin_would_block: bool,
    #[cfg(test)]
    panic_after_readers: bool,
    #[cfg(test)]
    fail_first_group_kill: bool,
    #[cfg(test)]
    fail_every_group_kill: bool,
    #[cfg(test)]
    signal_probe: Option<SignalProbe>,
    #[cfg(test)]
    reader_probe: Option<ReaderProbe>,
    #[cfg(test)]
    stdin_probe: Option<ReaderProbe>,
    #[cfg(test)]
    cleanup_seam: Option<CleanupSeam>,
    #[cfg(test)]
    fail_reaper_start: bool,
    #[cfg(test)]
    reaper_registry: Option<&'static ReaperRegistry>,
    #[cfg(test)]
    fixture_publication: Option<std::path::PathBuf>,
    #[cfg(test)]
    require_reader_would_block: bool,
}

#[cfg(test)]
fn await_fixture_publication(faults: &Faults) {
    let Some(marker) = faults.fixture_publication.as_ref() else {
        return;
    };
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        if std::fs::metadata(marker).is_ok_and(|metadata| metadata.len() > 0) {
            return;
        }
        assert!(
            Instant::now() < until,
            "fixture must publish its PIDs before an injected fault"
        );
        thread::sleep(POLL_INTERVAL);
    }
}

fn executable_authority_error() -> ProcessError {
    // This fixed public diagnostic cannot disclose executable or environment bytes.
    uncaptured_error(
        ProcessErrorKind::ExecutableAuthority,
        "executable authority requires an absolute path or a bare name resolved through a \
         nonempty PATH containing only absolute directories"
            .to_owned(),
    )
}

fn resolve_executable(request: &ProcessRequest) -> Result<PathBuf, ProcessError> {
    let search_path = request
        .env
        .iter()
        .rev()
        .find(|(key, _)| is_path_key(key))
        .map(|(_, value)| value.clone())
        .or_else(|| {
            matches!(request.env_policy, EnvPolicy::Inherit)
                .then(|| std::env::var_os("PATH"))
                .flatten()
        });
    let search_directories = search_path
        .as_ref()
        .map(|path| std::env::split_paths(path).collect::<Vec<_>>());
    if search_directories.as_ref().is_some_and(|directories| {
        directories.is_empty()
            || directories
                .iter()
                .any(|directory| directory.as_os_str().is_empty() || !directory.is_absolute())
    }) {
        return Err(executable_authority_error());
    }

    let executable = Path::new(&request.executable);
    if executable.is_absolute() {
        return Ok(executable.to_owned());
    }
    if !is_bare_executable(executable) {
        return Err(executable_authority_error());
    }
    let directories = search_directories.ok_or_else(executable_authority_error)?;

    for directory in directories {
        let candidate = directory.join(executable);
        match std::fs::metadata(&candidate) {
            Ok(metadata) if executable_candidate(&metadata) => {
                return std::fs::canonicalize(candidate).map_err(|error| {
                    uncaptured_error(
                        ProcessErrorKind::Spawn,
                        bounded_diagnostic(format!("executable resolution: {error}")),
                    )
                });
            }
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) => {}
            Err(error) => {
                return Err(uncaptured_error(
                    ProcessErrorKind::Spawn,
                    bounded_diagnostic(format!("executable resolution: {error}")),
                ));
            }
        }
    }

    Err(uncaptured_error(
        ProcessErrorKind::Spawn,
        "authorized executable search path contained no matching program".to_owned(),
    ))
}

fn is_bare_executable(executable: &Path) -> bool {
    let mut components = executable.components();
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
fn is_path_key(key: &std::ffi::OsStr) -> bool {
    key.to_string_lossy().eq_ignore_ascii_case("PATH")
}

#[cfg(not(windows))]
fn is_path_key(key: &std::ffi::OsStr) -> bool {
    key == std::ffi::OsStr::new("PATH")
}

fn run_inner(
    request: ProcessRequest,
    cancel: &CancelToken,
    faults: Faults,
) -> Result<ProcessOutput, ProcessError> {
    run_inner_observed(request, cancel, faults, None)
}

fn run_inner_observed(
    mut request: ProcessRequest,
    cancel: &CancelToken,
    faults: Faults,
    output: Option<SyncSender<ProcessOutputChunk>>,
) -> Result<ProcessOutput, ProcessError> {
    if request.stdout_limit == usize::MAX {
        return Err(empty_error(ProcessErrorKind::InvalidLimit(
            OutputStream::Stdout,
        )));
    }
    if request.stderr_limit == usize::MAX {
        return Err(empty_error(ProcessErrorKind::InvalidLimit(
            OutputStream::Stderr,
        )));
    }
    if let Some(cause) = cancel.cause() {
        return Err(empty_error(ProcessErrorKind::Cancelled(cause)));
    }
    let executable = resolve_executable(&request)?;

    // Reserve exact-child custody before spawn; cleanup cannot depend on later allocation or admission.
    let reaper = establish_reaper(&faults).map_err(|error| {
        uncaptured_error(
            ProcessErrorKind::Setup,
            bounded_diagnostic(format!(
                "reaper service unavailable, helper not spawned: {error}"
            )),
        )
    })?;

    let started = Instant::now();
    let (mut command, scope_unit, manager_env) = match &request.background {
        Some(scope) => {
            let launch = background::launch(
                scope,
                &executable,
                &request.argv,
                &request.env_policy,
                &request.env,
            );
            (launch.command, launch.scope, launch.manager_env)
        }
        None => {
            let mut command = Command::new(&executable);
            command.args(&request.argv);
            (command, None, None)
        }
    };
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(match &request.stdin {
            StdinPolicy::Null => Stdio::null(),
            StdinPolicy::Bytes(_) => Stdio::piped(),
        });
    // Clear ambient secrets and lookup authority unless inheritance was explicitly selected.
    match request.env_policy {
        EnvPolicy::Cleared => {
            command.env_clear();
        }
        EnvPolicy::Inherit => {}
    }
    command.envs(
        request
            .env
            .iter()
            .map(|(key, value)| (key.as_os_str(), value.as_os_str())),
    );
    if let Some((key, value)) = manager_env {
        command.env(key, value);
    }
    if let Some(directory) = &request.current_dir {
        command.current_dir(directory);
    }
    platform::configure(&mut command);

    let child = command.spawn().map_err(|error| {
        uncaptured_error(
            ProcessErrorKind::Spawn,
            bounded_diagnostic(format!("spawn: {error}")),
        )
    })?;
    let mut owner = ProcessOwner::new(child, reaper, &faults);
    owner.scope_unit = scope_unit;
    #[cfg(test)]
    let started = {
        await_fixture_publication(&faults);
        if faults.fixture_publication.is_some() {
            Instant::now()
        } else {
            started
        }
    };
    if let Some(diagnostic) = owner.setup_error() {
        return Err(uncaptured_error(ProcessErrorKind::Setup, diagnostic));
    }
    let (sender, receiver) = mpsc::channel();

    let stdout = owner.child_mut().stdout.take().ok_or_else(|| {
        uncaptured_error(
            ProcessErrorKind::Setup,
            "stdout pipe was not created".to_owned(),
        )
    })?;
    workers::spawn_reader(
        &mut owner,
        sender.clone(),
        OutputStream::Stdout,
        stdout,
        request.stdout_limit,
        faults.clone(),
        output.clone(),
    )?;

    let stderr = owner.child_mut().stderr.take().ok_or_else(|| {
        uncaptured_error(
            ProcessErrorKind::Setup,
            "stderr pipe was not created".to_owned(),
        )
    })?;
    workers::spawn_reader(
        &mut owner,
        sender.clone(),
        OutputStream::Stderr,
        stderr,
        request.stderr_limit,
        faults.clone(),
        output,
    )?;

    let needs_stdin_event = matches!(request.stdin, StdinPolicy::Bytes(_));
    if let StdinPolicy::Bytes(bytes) = std::mem::replace(&mut request.stdin, StdinPolicy::Null) {
        let bytes = enzui_security::secret::Secret::new(bytes);
        let stdin = owner.child_mut().stdin.take().ok_or_else(|| {
            uncaptured_error(
                ProcessErrorKind::Setup,
                "stdin pipe was not created".to_owned(),
            )
        })?;
        workers::spawn_stdin(&mut owner, sender.clone(), stdin, bytes, faults.clone())?;
    }

    #[cfg(test)]
    if faults.fail_setup_after_readers || faults.panic_after_readers {
        synchronize_post_spawn_fault(&faults, needs_stdin_event);
    }
    #[cfg(test)]
    if faults.fail_setup_after_readers {
        return Err(uncaptured_error(
            ProcessErrorKind::Setup,
            "injected post-spawn setup failure".to_owned(),
        ));
    }
    #[cfg(test)]
    if faults.panic_after_readers {
        panic!("injected owner unwind after spawn");
    }
    drop(sender);

    drive(DriveState {
        owner,
        receiver,
        cancel,
        started,
        deadline: request.deadline,
        stdin_complete: !needs_stdin_event,
        captures: Captures::default(),
        faults,
    })
}

trait NonblockingPipe {
    fn make_nonblocking(&self) -> io::Result<()>;
}

#[cfg(target_os = "linux")]
fn make_nonblocking(fd: &impl std::os::fd::AsFd) -> io::Result<()> {
    use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};

    let flags = fcntl_getfl(fd).map_err(io::Error::from)?;
    fcntl_setfl(fd, flags | OFlags::NONBLOCK).map_err(io::Error::from)
}

macro_rules! impl_nonblocking_pipe {
    ($pipe:ty) => {
        impl NonblockingPipe for $pipe {
            fn make_nonblocking(&self) -> io::Result<()> {
                #[cfg(target_os = "linux")]
                {
                    make_nonblocking(self)
                }
                #[cfg(not(target_os = "linux"))]
                {
                    Ok(())
                }
            }
        }
    };
}

impl_nonblocking_pipe!(ChildStdout);
impl_nonblocking_pipe!(ChildStderr);
impl_nonblocking_pipe!(ChildStdin);

#[cfg(test)]
fn synchronize_post_spawn_fault(faults: &Faults, needs_stdin: bool) {
    if let Some(probe) = &faults.reader_probe {
        probe.wait_for_started(
            2,
            "both output readers must start before the injected fault",
        );
        if faults.require_reader_would_block {
            probe.wait_for_both_readers_would_block(
                "both output readers must observe real WouldBlock before the injected fault",
            );
        }
    }
    if needs_stdin && let Some(probe) = &faults.stdin_probe {
        probe.wait_for_started(1, "stdin writer must start before the injected fault");
        probe.wait_for_would_block(
            1,
            "stdin writer must observe real WouldBlock before the injected fault",
        );
    }
}

pub mod background;
mod capture;
mod workers;

pub use background::{
    BackgroundScope, FallbackReason, Placement, placement, run_on_lowered_thread,
};

include!("process_transport/drive.rs");

#[cfg(target_os = "linux")]
mod platform {
    include!("process_transport/platform_linux.rs");
}

#[cfg(not(target_os = "linux"))]
mod platform {
    include!("process_transport/platform_other.rs");
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests;
