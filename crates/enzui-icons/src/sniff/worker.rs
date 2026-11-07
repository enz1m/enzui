//! The explicit off-UI-thread sniff worker.
//!
//! Blocking sniff work runs on a real background thread rather than inside a
//! `Task::perform` future that would block whatever thread the runtime polls it
//! on. The worker owns a boxed [`Sniffer`], receives
//! [`SniffRequest`](crate::sniff::worker::SniffRequest)s over a channel, runs
//! the blocking sniff, and sends
//! [`SniffCompletion`](crate::sniff::worker::SniffCompletion)s back.
//!
//! It carries no policy: caching, coalescing and invalidation live in
//! [`SniffCache`] on the UI side, which is why they
//! are testable without a thread. The worker only moves the blocking call off
//! the UI thread and returns each opaque attempt identity unchanged, so the
//! UI-side cache can drop stale completions.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::{SniffAttempt, SniffOutcome, Sniffer};

/// Construct a deadline from a caller-controlled [`Duration`] without the
/// `Instant + Duration` panic. Returns `None` when the sum overflows the
/// platform's `Instant` ceiling — callers read `None` as "no representable
/// deadline" and continue until their independent completion condition fires.
///
/// This is the Clamp-only subset of
/// [`enzui_core::monotonic_ms::checked_instant_deadline`]. The full
/// vocabulary lives in core, but `enzui-core` depends on
/// `enzui-icons`, so importing core here would create a cycle.
fn checked_deadline(now: Instant, delta: Duration) -> Option<Instant> {
    now.checked_add(delta)
}

/// Default bound on the number of queued sniff requests.
///
/// The request queue is bounded: an unbounded channel lets a backlog (a
/// directory of a million files scrolled past) retain a path copy per queued
/// request and makes teardown drain arbitrarily much work. A full queue reports
/// [`Dispatch::Full`] and the request is dropped by the caller, who degrades.
pub const DEFAULT_REQUEST_CAPACITY: usize = 128;

/// Default bound on the number of buffered completions.
///
/// Also bounded, for the same reason: a UI that stops draining must not let
/// completions accumulate without limit. When the buffer is full the worker
/// applies backpressure (retrying with a short sleep) while staying responsive
/// to the stop signal, rather than blocking forever.
pub const DEFAULT_COMPLETION_CAPACITY: usize = 128;

/// How long [`SniffWorker::shutdown`] waits for the worker to finish before
/// detaching it.
///
/// Sized for "the in-flight sniff finishes", not "the backlog drains": the
/// backlog is discarded, never executed, at shutdown.
pub const DEFAULT_TEARDOWN_BUDGET: Duration = Duration::from_millis(2_000);

/// A request to execute one exact cache attempt.
#[derive(Clone, Debug)]
pub struct SniffRequest {
    /// The exact cache attempt to execute and return unchanged.
    pub attempt: SniffAttempt,
}

/// The result of a sniff, carrying back the exact attempt identity so
/// [`SniffCache::complete`](super::SniffCache::complete) can reject stale,
/// superseded, or duplicate terminal messages.
#[derive(Clone, Debug)]
pub struct SniffCompletion {
    /// The exact attempt supplied with the request.
    pub attempt: SniffAttempt,
    /// The outcome of the sniff.
    pub outcome: SniffOutcome,
}

/// The outcome of trying to queue a sniff request onto the bounded request
/// channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dispatch {
    /// The request was accepted into the queue.
    Queued,
    /// The bounded queue is full. The request was not queued; the caller
    /// must degrade rather than retain or retry-spin.
    Full,
    /// The worker has shut down (its receiver is gone). The request was not
    /// queued.
    Disconnected,
}

impl Dispatch {
    /// Whether the request was queued.
    pub fn is_queued(self) -> bool {
        matches!(self, Dispatch::Queued)
    }
}

/// What a [`SniffWorker::shutdown`] attempt did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TeardownOutcome {
    /// The OS thread was joined inside the budget: its stack is gone and the
    /// boxed [`Sniffer`] it owned has been dropped. Includes a worker that had
    /// already exited on its own.
    Joined,
    /// The thread finished but its body panicked; it was still joined.
    Panicked,
    /// The thread did not finish inside the budget, so it was detached rather
    /// than blocking the caller. The stop flag stays raised, so it still exits.
    TimedOut,
    /// A previous teardown already joined (or detached) this worker.
    AlreadyTornDown,
}

impl TeardownOutcome {
    /// Whether the OS thread was actually joined.
    pub fn is_joined(self) -> bool {
        matches!(self, TeardownOutcome::Joined | TeardownOutcome::Panicked)
    }
}

/// Name given to the background sniff worker thread.
///
/// Linux truncates a thread's `comm` to 15 bytes, so this 18-byte name is
/// reported by the kernel as `enzui-sniff-wor`. That is fine for a human
/// reading `top`/`gdb`, but anything *matching* on it must account for the
/// truncation — see this module's tests.
const WORKER_THREAD_NAME: &str = "enzui-sniff-worker";

/// The worker thread could not be created.
///
/// Deliberately not a [`SniffError`](super::SniffError) variant: that enum is a
/// per-path outcome, stored in [`SniffOutcome::Failed`] and cached negatively,
/// so thread-table exhaustion there would be cached as some file's sniff result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SniffWorkerError {
    /// The OS refused to create the worker thread.
    ///
    /// Thread creation is an OS resource request, not a programming error:
    /// it fails on thread/fd exhaustion, under a restrictive `RLIMIT_NPROC`,
    /// and in containers with low pid limits. Those are runtime conditions a
    /// caller can report and degrade on, so they are returned rather than
    /// panicked.
    Spawn {
        /// Classifying OS error kind. The originating [`std::io::Error`] is
        /// neither `Clone` nor `Eq` while this type is both, so the kind is
        /// retained rather than the error itself.
        kind: std::io::ErrorKind,
    },
}

impl std::fmt::Display for SniffWorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SniffWorkerError::Spawn { kind } => {
                write!(f, "sniff worker thread could not be spawned: {kind}")
            }
        }
    }
}

impl std::error::Error for SniffWorkerError {}

/// A background worker that runs blocking sniffs off the UI thread.
///
/// Construct it with [`SniffWorker::spawn`], feed it [`SniffRequest`]s with
/// [`SniffWorker::request`] (bounded: a full queue reports [`Dispatch::Full`]),
/// and drain [`SniffCompletion`]s with [`SniffWorker::completions`].
///
/// Teardown is out of band and bounded. [`SniffWorker::shutdown`] raises an
/// [`AtomicBool`] stop flag, which a full queue cannot crowd out, disconnects
/// the request channel, and joins within [`DEFAULT_TEARDOWN_BUDGET`]. The queued
/// backlog is abandoned rather than drained, releasing the path copy each
/// discarded request held.
///
/// A worker that overruns the budget, parked in a stalled read, is detached and
/// reported as [`TeardownOutcome::TimedOut`]: the caller is never wedged, but
/// the detached thread still owns its boxed [`Sniffer`] until its read returns
/// and the raised stop flag ends it. [`Drop`] runs the same sequence.
pub struct SniffWorker {
    tx: Option<SyncSender<SniffRequest>>,
    completions: Receiver<SniffCompletion>,
    stop: Arc<AtomicBool>,
    started: Arc<AtomicUsize>,
    discarded: Arc<AtomicUsize>,
    handle: Option<JoinHandle<()>>,
    budget: Duration,
    outcome: Option<TeardownOutcome>,
}

impl SniffWorker {
    /// Spawn a worker thread that sniffs with `sniffer`.
    ///
    /// The `sniffer` is moved onto the worker thread and the UI thread never
    /// touches it, so the blocking [`Sniffer::sniff`] call can never run on the
    /// UI thread; that is why `sniffer` must be `Send`.
    ///
    /// # Errors
    ///
    /// Returns [`SniffWorkerError::Spawn`] when the OS refuses to create the
    /// thread, which is a runtime resource condition rather than a programming
    /// error. No thread exists on that path, so there is nothing to unwind.
    pub fn spawn<S>(sniffer: S) -> Result<Self, SniffWorkerError>
    where
        S: Sniffer + Send + 'static,
    {
        // Production always spawns; the gate exists so the failure path below is
        // reachable from a test. See `spawn_with_gate`.
        Self::spawn_with_gate(sniffer, || Ok(()))
    }

    /// Spawn a worker that calls `notify` after a completion has been parked in
    /// the bounded completion queue.
    ///
    /// The callback runs on the worker thread and must return promptly. It is
    /// deliberately ordered after the successful queue send: a UI wake can
    /// therefore drain the completion immediately without racing an
    /// about-to-be-enqueued result. Callers should make notification itself
    /// non-blocking (for example, a coalescing `try_send`).
    ///
    /// # Errors
    ///
    /// A [`SniffWorkerError`] when the worker thread cannot be spawned.
    pub fn spawn_with_completion_notify<S, N>(
        sniffer: S,
        notify: N,
    ) -> Result<Self, SniffWorkerError>
    where
        S: Sniffer + Send + 'static,
        N: FnMut() + Send + 'static,
    {
        Self::spawn_named_with_notify(
            sniffer,
            WORKER_THREAD_NAME,
            DEFAULT_REQUEST_CAPACITY,
            DEFAULT_COMPLETION_CAPACITY,
            DEFAULT_TEARDOWN_BUDGET,
            || Ok(()),
            notify,
        )
    }

    /// [`SniffWorker::spawn`] with a gate consulted immediately before the
    /// thread is created.
    ///
    /// Exhausting the OS thread table in a unit test is neither hermetic nor
    /// reliable, so a test injects the `io::Error` the OS would have produced.
    /// The gate's error takes the same early return as a real
    /// [`std::thread::Builder::spawn`] failure.
    fn spawn_with_gate<S>(
        sniffer: S,
        gate: impl FnOnce() -> std::io::Result<()>,
    ) -> Result<Self, SniffWorkerError>
    where
        S: Sniffer + Send + 'static,
    {
        Self::spawn_named(
            sniffer,
            WORKER_THREAD_NAME,
            DEFAULT_REQUEST_CAPACITY,
            DEFAULT_COMPLETION_CAPACITY,
            DEFAULT_TEARDOWN_BUDGET,
            gate,
        )
    }

    /// [`SniffWorker::spawn_with_gate`] with the worker thread name supplied
    /// explicitly.
    ///
    /// A leak test needs a unique name to count only its own threads, since
    /// `cargo test` runs tests concurrently in one process. Production always
    /// passes [`WORKER_THREAD_NAME`].
    fn spawn_named<S>(
        sniffer: S,
        thread_name: &str,
        request_capacity: usize,
        completion_capacity: usize,
        budget: Duration,
        gate: impl FnOnce() -> std::io::Result<()>,
    ) -> Result<Self, SniffWorkerError>
    where
        S: Sniffer + Send + 'static,
    {
        Self::spawn_named_with_notify(
            sniffer,
            thread_name,
            request_capacity,
            completion_capacity,
            budget,
            gate,
            || {},
        )
    }

    fn spawn_named_with_notify<S, N>(
        sniffer: S,
        thread_name: &str,
        request_capacity: usize,
        completion_capacity: usize,
        budget: Duration,
        gate: impl FnOnce() -> std::io::Result<()>,
        notify: N,
    ) -> Result<Self, SniffWorkerError>
    where
        S: Sniffer + Send + 'static,
        N: FnMut() + Send + 'static,
    {
        // Bounded on both ends: a backlog of requests and a flood of completions
        // are each capped at construction, with a minimum of 1 so a zero cannot
        // deadlock the rendezvous.
        let (req_tx, req_rx) = sync_channel::<SniffRequest>(request_capacity.max(1));
        let (done_tx, done_rx) = sync_channel::<SniffCompletion>(completion_capacity.max(1));

        let stop = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicUsize::new(0));
        let discarded = Arc::new(AtomicUsize::new(0));

        let worker_stop = Arc::clone(&stop);
        let worker_started = Arc::clone(&started);
        let worker_discarded = Arc::clone(&discarded);

        let spawned = gate().and_then(|()| {
            std::thread::Builder::new()
                .name(thread_name.to_owned())
                .spawn(move || {
                    worker_loop(
                        sniffer,
                        req_rx,
                        done_tx,
                        worker_stop,
                        worker_started,
                        worker_discarded,
                        notify,
                    );
                })
        });

        let handle = match spawned {
            Ok(handle) => handle,
            // No thread was created, so there is nothing to terminate or join.
            // Every channel endpoint and Arc is a local dropped by this return;
            // the ones moved into the closure are dropped when the spawn fails.
            Err(error) => {
                return Err(SniffWorkerError::Spawn { kind: error.kind() });
            }
        };

        Ok(Self {
            tx: Some(req_tx),
            completions: done_rx,
            stop,
            started,
            discarded,
            handle: Some(handle),
            budget,
            outcome: None,
        })
    }

    /// Queue a sniff request without blocking. The queue is bounded, so a
    /// saturated worker reports [`Dispatch::Full`] and the request is dropped
    /// (no path copy is retained), while a shut-down worker reports
    /// [`Dispatch::Disconnected`].
    pub fn request(&self, request: SniffRequest) -> Dispatch {
        match &self.tx {
            Some(tx) => match tx.try_send(request) {
                Ok(()) => Dispatch::Queued,
                Err(TrySendError::Full(_)) => Dispatch::Full,
                Err(TrySendError::Disconnected(_)) => Dispatch::Disconnected,
            },
            None => Dispatch::Disconnected,
        }
    }

    /// How many requests the worker has begun sniffing. The positive control for
    /// "only active work ran": a shutdown that discarded the backlog leaves this
    /// at the number actually started, not the number queued.
    pub fn started(&self) -> usize {
        self.started.load(Ordering::Acquire)
    }

    /// How many queued requests the worker discarded on its way out.
    pub fn discarded(&self) -> usize {
        self.discarded.load(Ordering::Acquire)
    }

    /// Whether the out-of-band stop signal has been raised.
    pub fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    /// The outcome of a completed teardown, if this worker has been torn down.
    pub fn teardown_outcome(&self) -> Option<TeardownOutcome> {
        self.outcome
    }

    /// Block until the worker has begun at least `n` sniffs or `timeout`
    /// elapses; returns whether the target was reached. Lets a caller wait for
    /// the worker to be genuinely busy instead of sleeping a guessed interval.
    pub fn wait_for_started(&self, n: usize, timeout: Duration) -> bool {
        let deadline = checked_deadline(Instant::now(), timeout);
        loop {
            if self.started() >= n {
                return true;
            }
            let now = Instant::now();
            if deadline.is_some_and(|deadline| now >= deadline) {
                return self.started() >= n;
            }
            let sleep_for = deadline.map_or(Duration::from_millis(1), |deadline| {
                deadline.duration_since(now)
            });
            std::thread::sleep(Duration::from_millis(1).min(sleep_for));
        }
    }

    /// Raise the out-of-band stop signal, disconnect the request channel, and
    /// join within the budget — the bounded, backlog-discarding teardown.
    ///
    /// The order is load-bearing:
    ///
    /// 1. raise the stop flag — an [`AtomicBool`], never a queue message, so a
    ///    full request queue cannot crowd it out;
    /// 2. drop the sender — a worker parked in `recv()` wakes with no queue
    ///    slot required;
    /// 3. wait up to the budget, then `join()`. Joining first would block for as
    ///    long as an in-flight stalled read takes; waiting forever would wedge
    ///    the UI, so a worker that overruns is detached ([`TeardownOutcome::TimedOut`]).
    ///
    /// Returns what happened. Idempotent: a second call reports
    /// [`TeardownOutcome::AlreadyTornDown`].
    pub fn shutdown(&mut self) -> TeardownOutcome {
        if self.outcome.is_some() {
            return TeardownOutcome::AlreadyTornDown;
        }

        // 1. Out-of-band signal: an AtomicBool, never a queue message.
        self.stop.store(true, Ordering::Release);
        // 2. Disconnect, so an idle worker wakes without a free queue slot.
        self.tx = None;

        let Some(handle) = self.handle.take() else {
            self.outcome = Some(TeardownOutcome::AlreadyTornDown);
            return TeardownOutcome::AlreadyTornDown;
        };

        // 3. Bounded wait, then join. A worker that overruns is detached, never
        //    waited on forever.
        let deadline = checked_deadline(Instant::now(), self.budget);
        while !handle.is_finished() {
            let now = Instant::now();
            if deadline.is_some_and(|d| now >= d) {
                self.outcome = Some(TeardownOutcome::TimedOut);
                drop(handle);
                return TeardownOutcome::TimedOut;
            }
            let sleep_for = deadline.map_or(Duration::from_millis(1), |d| d.duration_since(now));
            std::thread::sleep(Duration::from_millis(1).min(sleep_for));
        }
        let outcome = match handle.join() {
            Ok(()) => TeardownOutcome::Joined,
            Err(_) => TeardownOutcome::Panicked,
        };
        self.outcome = Some(outcome);
        outcome
    }

    /// The receiving end of completions. The caller polls this (for example,
    /// once per frame via `try_recv`) and feeds each completion into
    /// [`SniffCache::complete`](super::SniffCache::complete).
    pub fn completions(&self) -> &Receiver<SniffCompletion> {
        &self.completions
    }

    /// Tear the worker down on the bounded path and drop the handle. Kept for
    /// call sites that want an owning shutdown; it delegates to
    /// [`SniffWorker::shutdown`], so it also discards the backlog and honours
    /// the budget.
    pub fn join(mut self) {
        let _ = self.shutdown();
    }
}

/// A forgotten handle still tears its worker down on the same bounded path.
///
/// [`SniffWorker::shutdown`] remains the supported call because it reports the
/// outcome, including a [`TeardownOutcome::TimedOut`] detach, which `Drop`
/// cannot surface. A dropped handle joins a responsive worker and detaches an
/// unresponsive one; a detached thread exits on its own under the raised stop
/// flag.
impl Drop for SniffWorker {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

/// The worker loop: receive requests until stop is signalled or the channel
/// closes, sniff each one (blocking, off the UI thread), and send the completion
/// back over the bounded completion channel.
///
/// The stop flag is checked before blocking on `recv` and again after a
/// request arrives, so no queued request is sniffed after shutdown was
/// requested. On the way out the queued backlog is discarded, releasing the
/// path copies it held rather than draining every filesystem sniff.
fn worker_loop<S: Sniffer, N: FnMut()>(
    sniffer: S,
    requests: Receiver<SniffRequest>,
    completions: SyncSender<SniffCompletion>,
    stop: Arc<AtomicBool>,
    started: Arc<AtomicUsize>,
    discarded: Arc<AtomicUsize>,
    mut notify: N,
) {
    loop {
        if stop.load(Ordering::Acquire) {
            discard_backlog(&requests, &discarded);
            return;
        }
        let Ok(req) = requests.recv() else {
            discard_backlog(&requests, &discarded);
            return;
        };
        // Re-check after a request arrives: a request already sitting in the
        // queue must not be executed once shutdown was requested.
        if stop.load(Ordering::Acquire) {
            drop(req);
            discard_backlog(&requests, &discarded);
            return;
        }

        // The one active unit of work, counted before the blocking sniff so a
        // caller can observe that the worker is genuinely busy.
        started.fetch_add(1, Ordering::Release);
        let outcome = sniffer.sniff(req.attempt.path());
        let mut completion = SniffCompletion {
            attempt: req.attempt,
            outcome,
        };

        // Deliver the active completion over the bounded channel. Try first so
        // the active result is delivered when there is room; back off on a full
        // buffer while staying responsive to the stop flag so a UI that stopped
        // draining cannot wedge teardown.
        loop {
            match completions.try_send(completion) {
                Ok(()) => {
                    notify();
                    break;
                }
                Err(TrySendError::Full(back)) => {
                    if stop.load(Ordering::Acquire) {
                        drop(back);
                        discard_backlog(&requests, &discarded);
                        return;
                    }
                    completion = back;
                    std::thread::sleep(Duration::from_millis(1));
                }
                // Receiver gone: nothing more to deliver.
                Err(TrySendError::Disconnected(_)) => {
                    discard_backlog(&requests, &discarded);
                    return;
                }
            }
        }
    }
}

/// Drop every queued request now, recording how many were released. This is the
/// mechanism that keeps teardown bounded: the backlog is discarded, not
/// executed, and each dropped request frees the [`std::path::PathBuf`] it owned.
fn discard_backlog(requests: &Receiver<SniffRequest>, discarded: &AtomicUsize) {
    let mut n = 0;
    while let Ok(req) = requests.try_recv() {
        drop(req);
        n += 1;
    }
    if n > 0 {
        discarded.fetch_add(n, Ordering::Release);
    }
}

#[cfg(test)]
mod tests;
