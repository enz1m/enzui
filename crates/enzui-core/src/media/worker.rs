//! Decode worker with cooperative shutdown and explicit thread joining.
//!
//! [`MediaWorker::shutdown_and_join`] signals before joining and returns the worker body's outcome. Implicit drop uses the same ordering but detaches after 500 ms rather than blocking indefinitely.

#[cfg(test)]
use std::sync::Condvar;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::media::lifecycle::{Phase, WorkerLifecycle};
use crate::media::remote_gate::GateRefusal;
use crate::media::{Generation, MediaError};

/// A cooperative stop signal that worker bodies must poll and honor promptly.
#[derive(Debug, Clone)]
pub struct StopSignal {
    stop: Arc<AtomicBool>,
}

impl StopSignal {
    /// Creates a `StopSignal` backed by the given atomic flag.
    ///
    /// The caller retains the `Arc` and stores `true` to request shutdown.
    #[must_use]
    pub fn from_atomic(stop: Arc<AtomicBool>) -> Self {
        StopSignal { stop }
    }

    /// Whether shutdown has been requested. A worker body should return as soon
    /// as this is `true`.
    #[must_use]
    pub fn should_stop(&self) -> bool {
        // Acquire pairs with the Release store in `request_stop`, so everything
        // the shutting-down side wrote before signalling is visible here.
        self.stop.load(Ordering::Acquire)
    }
}

/// A typed failure produced by a media worker.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WorkerFailure {
    /// A destination, redirect, deadline, cancellation or budget refusal.
    RemotePolicy(GateRefusal),
    /// A decoder/backend media failure.
    Media(MediaError),
    /// The worker panicked before reporting a typed failure; its payload is discarded to avoid leaking source data.
    Panic,
}

impl From<GateRefusal> for WorkerFailure {
    fn from(value: GateRefusal) -> Self {
        Self::RemotePolicy(value)
    }
}

impl From<MediaError> for WorkerFailure {
    fn from(value: MediaError) -> Self {
        Self::Media(value)
    }
}

impl core::fmt::Display for WorkerFailure {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            WorkerFailure::RemotePolicy(refusal) => write!(f, "{refusal}"),
            WorkerFailure::Media(error) => write!(f, "{error}"),
            WorkerFailure::Panic => write!(f, "media worker thread panicked"),
        }
    }
}

impl std::error::Error for WorkerFailure {}

/// The result of a completed join, including the worker body's own iteration count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinOutcome {
    /// Iterations reported by the worker body when it returned.
    pub iterations: u64,
    /// Whether this call was the one that performed the join. A second teardown
    /// reports `false` and does nothing.
    pub joined_now: bool,
    /// Typed failure produced by the worker body, if it could not start or
    /// continue decoding. Remote policy refusals retain their exact variant.
    pub failure: Option<WorkerFailure>,
}

/// Why a worker could not be started or replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerError {
    /// A replacement was attempted while the outgoing worker was still in this
    /// phase. Shut it down and join it first.
    NotJoined(Phase),
    /// The OS refused to spawn a thread.
    SpawnFailed,
}

impl core::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            WorkerError::NotJoined(phase) => write!(
                f,
                "cannot replace a worker in {phase:?}; shut it down and join it first"
            ),
            WorkerError::SpawnFailed => write!(f, "the OS refused to spawn a decode thread"),
        }
    }
}

impl std::error::Error for WorkerError {}

#[cfg(test)]
#[derive(Clone, Debug, Default)]
struct DropTestState {
    stop_acknowledged: bool,
    drop_waiting: bool,
    worker_released: bool,
    drop_released: bool,
    join_completed: bool,
}

#[cfg(test)]
type DropTestHandshake = Arc<(Mutex<DropTestState>, Condvar)>;

/// A generation-scoped decode thread whose body cooperatively polls [`StopSignal`] and is explicitly joined.
#[derive(Debug)]
pub struct MediaWorker {
    generation: Generation,
    lifecycle: WorkerLifecycle,
    stop: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
    handle: Option<JoinHandle<u64>>,
    outcome: Option<JoinOutcome>,
    failure: Arc<Mutex<Option<WorkerFailure>>>,
    #[cfg(test)]
    drop_test_handshake: Option<DropTestHandshake>,
}

impl MediaWorker {
    /// Spawns a worker for `generation`.
    ///
    /// The body receives a stop signal and progress counter and returns its completed iteration count.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError::SpawnFailed`] if the thread cannot be spawned.
    pub fn spawn<F>(generation: Generation, body: F) -> Result<MediaWorker, WorkerError>
    where
        F: FnOnce(StopSignal, Arc<AtomicU64>) -> u64 + Send + 'static,
    {
        Self::spawn_fallible(generation, move |signal, progress| {
            Ok(body(signal, progress))
        })
    }

    /// Spawn a worker whose body can return a typed media failure.
    ///
    /// # Errors
    ///
    /// [`WorkerError::SpawnFailed`] when the thread cannot be spawned.
    pub fn spawn_fallible<F>(generation: Generation, body: F) -> Result<MediaWorker, WorkerError>
    where
        F: FnOnce(StopSignal, Arc<AtomicU64>) -> Result<u64, WorkerFailure> + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(AtomicU64::new(0));
        let failure = Arc::new(Mutex::new(None));
        let signal = StopSignal {
            stop: Arc::clone(&stop),
        };
        let body_progress = Arc::clone(&progress);
        let body_failure = Arc::clone(&failure);

        let handle = thread::Builder::new()
            .name(format!("enzui-media-{}", generation.raw()))
            .spawn(move || match body(signal, body_progress) {
                Ok(iterations) => iterations,
                Err(error) => {
                    *body_failure
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(error);
                    0
                }
            })
            .map_err(|_| WorkerError::SpawnFailed)?;

        Ok(MediaWorker {
            generation,
            lifecycle: WorkerLifecycle::start(),
            stop,
            progress,
            handle: Some(handle),
            outcome: None,
            failure,
            #[cfg(test)]
            drop_test_handshake: None,
        })
    }

    /// The generation this worker decodes for. Frames it produces carry this, so
    /// the sink can discard them once the source is replaced.
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// The worker's lifecycle phase.
    #[must_use]
    pub fn phase(&self) -> Phase {
        self.lifecycle.phase()
    }

    /// Whether the worker has been fully shut down and joined.
    #[must_use]
    pub fn is_joined(&self) -> bool {
        self.lifecycle.is_joined()
    }

    /// Returns whether the OS thread has finished, independently of lifecycle bookkeeping.
    #[must_use]
    pub fn is_thread_finished(&self) -> bool {
        self.handle.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Returns the number of iterations reported by the worker body so far.
    #[must_use]
    pub fn progress(&self) -> u64 {
        self.progress.load(Ordering::Acquire)
    }

    /// Waits until at least `n` iterations are reported or `timeout` elapses.
    ///
    /// An unrepresentable deadline is clamped without panicking.
    pub fn wait_for_progress(&self, n: u64, timeout: Duration) -> bool {
        let started = Instant::now();
        let deadline = crate::monotonic_ms::checked_instant_deadline(
            started,
            timeout,
            crate::monotonic_ms::Policy::Clamp,
        );
        loop {
            if self.progress() >= n {
                return true;
            }
            let now = Instant::now();
            if deadline.is_some_and(|deadline| now >= deadline) {
                return false;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Requests shutdown without waiting, transitioning from running to draining.
    ///
    /// Use [`shutdown_and_join`](MediaWorker::shutdown_and_join) for complete teardown.
    pub fn begin_shutdown(&mut self) -> bool {
        // Release so the worker sees everything written before the signal.
        self.stop.store(true, Ordering::Release);
        self.lifecycle.begin_shutdown()
    }

    /// Signals the worker, then joins its thread.
    ///
    /// Repeated calls return the same outcome with `joined_now` set to `false`.
    pub fn shutdown_and_join(&mut self) -> JoinOutcome {
        if let Some(existing) = &self.outcome {
            return JoinOutcome {
                iterations: existing.iterations,
                joined_now: false,
                failure: existing.failure.clone(),
            };
        }

        self.stop.store(true, Ordering::Release);
        self.lifecycle.begin_shutdown();

        // Observe every join result; map a panic to `WorkerFailure::Panic` without retaining its payload.
        let (iterations, panicked) = match self.handle.take() {
            Some(handle) => match handle.join() {
                Ok(iterations) => (iterations, false),
                Err(_payload) => (0, true),
            },
            None => (0, false),
        };

        let joined_now = self.lifecycle.complete_join();
        let failure = {
            let mut slot = self
                .failure
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // Preserve an explicit `WorkerFailure` (Media/RemotePolicy) already
            // recorded by the body: a body that set a typed failure before
            // panicking retains its exact variant. Only a join panic with an
            // empty failure slot is reported as `Panic`.
            if panicked && slot.is_none() {
                *slot = Some(WorkerFailure::Panic);
            }
            slot.clone()
        };
        let outcome = JoinOutcome {
            iterations,
            joined_now,
            failure,
        };
        self.outcome = Some(outcome.clone());
        outcome
    }

    /// The outcome of the join, if this worker has been joined.
    #[must_use]
    pub fn join_outcome(&self) -> Option<JoinOutcome> {
        self.outcome.clone()
    }

    /// A typed failure reported by the worker body, available as soon as the
    /// body records it and retained in the join outcome.
    #[must_use]
    pub fn failure(&self) -> Option<WorkerFailure> {
        self.failure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Replaces this worker with a fresh one at `generation` after it has been joined.
    ///
    /// # Errors
    ///
    /// Returns [`WorkerError::NotJoined`] if shutdown and joining have not completed.
    pub fn replace<F>(&self, generation: Generation, body: F) -> Result<MediaWorker, WorkerError>
    where
        F: FnOnce(StopSignal, Arc<AtomicU64>) -> u64 + Send + 'static,
    {
        if !self.lifecycle.is_joined() {
            return Err(WorkerError::NotJoined(self.lifecycle.phase()));
        }
        MediaWorker::spawn(generation, body)
    }
}

const DROP_JOIN_BUDGET: Duration = Duration::from_millis(500);

// Drop signals first, joins a promptly finished body, and detaches after `DROP_JOIN_BUDGET`.
impl Drop for MediaWorker {
    fn drop(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        self.stop.store(true, Ordering::Release);

        // The test-only handshake starts the budget only after its synthetic
        // body has acknowledged this signal. Production builds have no seam or
        // extra wait here.
        #[cfg(test)]
        if let Some(shared) = &self.drop_test_handshake {
            let (lock, ready) = &**shared;
            let mut state = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while !state.stop_acknowledged && !state.worker_released {
                state = ready
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        }

        // Clamp an unrepresentable deadline to avoid panicking during `Drop` unwinding.
        let deadline = crate::monotonic_ms::checked_instant_deadline(
            Instant::now(),
            DROP_JOIN_BUDGET,
            crate::monotonic_ms::Policy::Clamp,
        );
        while !handle.is_finished() {
            let now = Instant::now();
            if deadline.is_some_and(|deadline| now >= deadline) {
                drop(handle);
                return;
            }

            // Hold the iteration after bounded join wins so the test can observe it before releasing the body.
            #[cfg(test)]
            if let Some(shared) = &self.drop_test_handshake {
                let (lock, ready) = &**shared;
                let mut state = lock
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !state.drop_waiting {
                    state.drop_waiting = true;
                    ready.notify_all();
                    while !state.drop_released {
                        state = ready
                            .wait(state)
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                    }
                }
            }

            let sleep_for = deadline.map_or(Duration::from_millis(1), |deadline| {
                deadline.duration_since(now)
            });
            thread::sleep(Duration::from_millis(1).min(sleep_for));
        }
        let _ = handle.join();

        #[cfg(test)]
        if let Some(shared) = &self.drop_test_handshake {
            let (lock, ready) = &**shared;
            let mut state = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.join_completed = true;
            ready.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    fn gen_n(n: u64) -> Generation {
        let mut g = Generation::FIRST;
        for _ in 0..n {
            g = g.next();
        }
        g
    }

    fn counting_body(signal: StopSignal, progress: Arc<AtomicU64>) -> u64 {
        let mut iterations = 0u64;
        while !signal.should_stop() {
            iterations += 1;
            progress.store(iterations, Ordering::Release);
            thread::sleep(Duration::from_millis(1));
        }
        iterations
    }

    #[test]
    fn replacing_an_unjoined_worker_is_refused() {
        let mut worker = MediaWorker::spawn(gen_n(0), counting_body).expect("spawn");
        worker.wait_for_progress(1, TEST_TIMEOUT);

        assert_eq!(
            worker.replace(gen_n(1), counting_body).err(),
            Some(WorkerError::NotJoined(Phase::Running)),
            "replacing a Running worker would leak its decode thread"
        );

        worker.begin_shutdown();
        assert_eq!(
            worker.replace(gen_n(1), counting_body).err(),
            Some(WorkerError::NotJoined(Phase::Draining)),
            "a signalled-but-unjoined worker still blocks replacement"
        );

        worker.shutdown_and_join();
        let mut replacement = worker
            .replace(gen_n(1), counting_body)
            .expect("a joined worker may be replaced");
        assert_eq!(replacement.generation(), gen_n(1));
        assert_eq!(replacement.phase(), Phase::Running);
        replacement.shutdown_and_join();
    }
}
