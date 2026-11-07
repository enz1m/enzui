//! Bounded execution for synchronous work that must not occupy an async executor.
//!
//! An owner starts a fixed number of threads and admits work through a fixed
//! FIFO. Every submission returns a generation-stamped future. Saturation,
//! cancellation, shutdown, and worker panic are terminal settlements rather
//! than missing replies. A terminal future does not release its queue or worker
//! slot before the underlying closure is discarded or exits.
//!
//! Deliver a settlement through the runtime (`iced::Task::perform`). An owner
//! that keeps the task to poll it synchronously awaits
//! [`BlockingTask::settled`] to learn when to poll; a polling timer is an
//! application update, and a view rebuild, on every tick.

use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::num::NonZeroUsize;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, PoisonError, Weak};
use std::task::{Context, Poll, Waker};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Default number of simultaneously running blocking calls.
pub const DEFAULT_WORKER_COUNT: NonZeroUsize = NonZeroUsize::new(2).unwrap();

/// Default number of blocking calls waiting behind the live workers.
pub const DEFAULT_QUEUE_CAPACITY: NonZeroUsize = NonZeroUsize::new(16).unwrap();

/// Maximum time ordinary owner destruction waits for responsive workers.
pub const DEFAULT_SHUTDOWN_WAIT: Duration = Duration::from_millis(100);

/// Why a blocking submission produced no value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BlockingTaskError {
    /// Every queue slot was occupied when the caller submitted the work.
    Busy,
    /// The caller cancelled this generation before it settled.
    Cancelled,
    /// The owner shut down before this generation settled.
    Shutdown,
    /// The blocking function panicked.
    Panicked,
}

impl fmt::Display for BlockingTaskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str("blocking worker queue is busy"),
            Self::Cancelled => f.write_str("blocking task was cancelled"),
            Self::Shutdown => f.write_str("blocking task owner shut down"),
            Self::Panicked => f.write_str("blocking task panicked"),
        }
    }
}

impl std::error::Error for BlockingTaskError {}

/// The single terminal answer for one submitted generation.
#[derive(Debug)]
pub struct BlockingTaskSettlement<T> {
    generation: u64,
    result: Result<T, BlockingTaskError>,
}

impl<T> BlockingTaskSettlement<T> {
    /// Returns the generation supplied at submission.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Borrows the completed value or terminal refusal.
    ///
    /// # Errors
    ///
    /// The task's terminal [`BlockingTaskError`].
    pub const fn result(&self) -> Result<&T, BlockingTaskError> {
        match &self.result {
            Ok(value) => Ok(value),
            Err(error) => Err(*error),
        }
    }

    /// Consumes the settlement and returns its value or terminal refusal.
    ///
    /// # Errors
    ///
    /// The task's terminal [`BlockingTaskError`].
    pub fn into_result(self) -> Result<T, BlockingTaskError> {
        self.result
    }

    /// Constructs a settlement from a generation and a terminal error.
    ///
    /// Use when a caller can determine the terminal outcome before pool
    /// submission, e.g. pre-admission cancellation.
    #[must_use]
    pub const fn from_error(generation: u64, error: BlockingTaskError) -> Self {
        Self {
            generation,
            result: Err(error),
        }
    }
}

#[derive(Default)]
struct CancellationState {
    cancelled: std::sync::atomic::AtomicBool,
}

/// Cooperative cancellation state passed into blocking work.
#[derive(Clone)]
pub struct BlockingTaskContext {
    cancellation: Arc<CancellationState>,
    owner: Weak<Shared>,
}

impl BlockingTaskContext {
    /// Returns whether cancellation or owner shutdown has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancellation
            .cancelled
            .load(std::sync::atomic::Ordering::Acquire)
            || self
                .owner
                .upgrade()
                .is_none_or(|shared| shared.lock().shutdown)
    }
}

struct CompletionState<T> {
    settlement: Option<BlockingTaskSettlement<T>>,
    terminal: bool,
    waker: Option<Waker>,
    observers: Vec<Waker>,
}

struct Completion<T> {
    state: Mutex<CompletionState<T>>,
    owner: Weak<Shared>,
}

impl<T> Completion<T> {
    fn new(owner: Weak<Shared>) -> Self {
        Self {
            state: Mutex::new(CompletionState {
                settlement: None,
                terminal: false,
                waker: None,
                observers: Vec::new(),
            }),
            owner,
        }
    }

    fn settle(&self, settlement: BlockingTaskSettlement<T>) {
        let (wake, observers) = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.terminal {
                return;
            }
            state.terminal = true;
            state.settlement = Some(settlement);
            (state.waker.take(), std::mem::take(&mut state.observers))
        };
        if let Some(owner) = self.owner.upgrade() {
            let mut state = owner.lock();
            state.settled = state.settled.saturating_add(1);
        }
        for waker in wake.into_iter().chain(observers) {
            waker.wake();
        }
    }

    fn is_terminal(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .terminal
    }
}

/// A handle that settles one generation as cancelled.
#[derive(Clone)]
pub struct BlockingTaskCancellation<T> {
    generation: u64,
    cancellation: Arc<CancellationState>,
    completion: Arc<Completion<T>>,
}

impl<T> BlockingTaskCancellation<T> {
    /// Requests cooperative cancellation and publishes the terminal settlement.
    pub fn cancel(&self) {
        self.cancellation
            .cancelled
            .store(true, std::sync::atomic::Ordering::Release);
        let _ = catch_unwind(AssertUnwindSafe(|| {
            self.completion.settle(BlockingTaskSettlement {
                generation: self.generation,
                result: Err(BlockingTaskError::Cancelled),
            });
        }));
    }
}

/// A generation-stamped future returned for every submission attempt.
///
/// Dropping the future cancels it. Accepted work that is already inside a
/// blocking call cannot be forcibly interrupted; it receives a cooperative
/// [`BlockingTaskContext`] and its value is discarded after cancellation.
#[must_use = "poll the task or inspect its cancellation handle so the generation settles"]
pub struct BlockingTask<T> {
    generation: u64,
    cancellation: Arc<CancellationState>,
    completion: Arc<Completion<T>>,
    consumed: bool,
}

impl<T> BlockingTask<T> {
    /// Returns a clonable cancellation handle for this generation.
    #[must_use]
    pub fn cancellation(&self) -> BlockingTaskCancellation<T> {
        BlockingTaskCancellation {
            generation: self.generation,
            cancellation: Arc::clone(&self.cancellation),
            completion: Arc::clone(&self.completion),
        }
    }
}

impl<T> BlockingTask<T> {
    /// A future that resolves once this generation settles, leaving the value
    /// in the task.
    ///
    /// An owner that keeps the task and polls it synchronously awaits this
    /// (through `iced::Task::perform`) to learn when to poll, instead of
    /// polling on a timer.
    pub fn settled(&self) -> BlockingTaskSettled<T> {
        BlockingTaskSettled {
            completion: Arc::clone(&self.completion),
        }
    }
}

/// Resolves when a [`BlockingTask`] settles; see [`BlockingTask::settled`].
#[must_use = "a settlement signal does nothing unless awaited"]
pub struct BlockingTaskSettled<T> {
    completion: Arc<Completion<T>>,
}

impl<T> fmt::Debug for BlockingTaskSettled<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BlockingTaskSettled")
            .field("settled", &self.completion.is_terminal())
            .finish()
    }
}

impl<T> Future for BlockingTaskSettled<T> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        let mut state = self
            .completion
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if state.terminal {
            return Poll::Ready(());
        }
        if !state
            .observers
            .iter()
            .any(|waker| waker.will_wake(context.waker()))
        {
            state.observers.push(context.waker().clone());
        }
        Poll::Pending
    }
}

impl<T> Future for BlockingTask<T> {
    type Output = BlockingTaskSettlement<T>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let settlement = {
            let mut state = self
                .completion
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(settlement) = state.settlement.take() {
                Some(settlement)
            } else {
                if state
                    .waker
                    .as_ref()
                    .is_none_or(|waker| !waker.will_wake(context.waker()))
                {
                    state.waker = Some(context.waker().clone());
                }
                None
            }
        };
        match settlement {
            Some(settlement) => {
                self.consumed = true;
                Poll::Ready(settlement)
            }
            None => Poll::Pending,
        }
    }
}

impl<T> Drop for BlockingTask<T> {
    fn drop(&mut self) {
        if !self.consumed && !self.completion.is_terminal() {
            BlockingTaskCancellation {
                generation: self.generation,
                cancellation: Arc::clone(&self.cancellation),
                completion: Arc::clone(&self.completion),
            }
            .cancel();
        }
    }
}

trait ShutdownControl: Send + Sync {
    fn settle(&self, error: BlockingTaskError);
}

struct WorkControl<T> {
    generation: u64,
    completion: Arc<Completion<T>>,
}

impl<T: Send + 'static> ShutdownControl for WorkControl<T> {
    fn settle(&self, error: BlockingTaskError) {
        self.completion.settle(BlockingTaskSettlement {
            generation: self.generation,
            result: Err(error),
        });
    }
}

fn settle_control(control: &dyn ShutdownControl, error: BlockingTaskError) {
    let _ = catch_unwind(AssertUnwindSafe(|| control.settle(error)));
}

trait Runnable: Send {
    /// Runs the task; returns whether its function panicked (the panic is
    /// contained and settled as [`BlockingTaskError::Panicked`]).
    fn run(self: Box<Self>) -> bool;
    fn shutdown_control(&self) -> Arc<dyn ShutdownControl>;
}

struct Work<T, F> {
    generation: u64,
    owner: Weak<Shared>,
    cancellation: Arc<CancellationState>,
    completion: Arc<Completion<T>>,
    control: Arc<WorkControl<T>>,
    function: Option<F>,
}

impl<T, F> Work<T, F> {
    fn terminal_reason(&self) -> Option<BlockingTaskError> {
        // Cancellation outranks shutdown so a live owner never settles Shutdown.
        if self
            .cancellation
            .cancelled
            .load(std::sync::atomic::Ordering::Acquire)
        {
            Some(BlockingTaskError::Cancelled)
        } else if self
            .owner
            .upgrade()
            .is_none_or(|shared| shared.lock().shutdown)
        {
            Some(BlockingTaskError::Shutdown)
        } else {
            None
        }
    }
}

impl<T, F> Runnable for Work<T, F>
where
    T: Send + 'static,
    F: FnOnce(BlockingTaskContext) -> T + Send + 'static,
{
    fn run(mut self: Box<Self>) -> bool {
        if self.completion.is_terminal() {
            return false;
        }
        let context = BlockingTaskContext {
            cancellation: Arc::clone(&self.cancellation),
            owner: Weak::clone(&self.owner),
        };
        if let Some(error) = self.terminal_reason() {
            settle_control(&*self.control, error);
            return false;
        }
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            self.function
                .take()
                .expect("blocking function consumed once")(context)
        }));
        match outcome {
            Err(_) => {
                self.completion.settle(BlockingTaskSettlement {
                    generation: self.generation,
                    result: Err(BlockingTaskError::Panicked),
                });
                true
            }
            Ok(value) => {
                if let Some(error) = self.terminal_reason() {
                    self.completion.settle(BlockingTaskSettlement {
                        generation: self.generation,
                        result: Err(error),
                    });
                    let _ = catch_unwind(AssertUnwindSafe(|| drop(value)));
                } else {
                    self.completion.settle(BlockingTaskSettlement {
                        generation: self.generation,
                        result: Ok(value),
                    });
                }
                false
            }
        }
    }

    fn shutdown_control(&self) -> Arc<dyn ShutdownControl> {
        Arc::<WorkControl<T>>::clone(&self.control)
    }
}

struct PoolState {
    queue: VecDeque<Box<dyn Runnable>>,
    shutdown: bool,
    running: Vec<Arc<dyn ShutdownControl>>,
    exited: usize,
    submitted: u64,
    accepted: u64,
    settled: u64,
    busy: u64,
    detached_workers: usize,
    panicked_workers: usize,
    /// Wakers of [`WaitingBlockingTask`]s parked on a full queue.
    capacity_waiters: Vec<Waker>,
}

struct Shared {
    state: Mutex<PoolState>,
    wake: Condvar,
    worker_capacity: usize,
    queue_capacity: usize,
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, PoolState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn submit<T, F>(self: &Arc<Self>, generation: u64, function: F) -> BlockingTask<T>
    where
        T: Send + 'static,
        F: FnOnce(BlockingTaskContext) -> T + Send + 'static,
    {
        let cancellation = Arc::new(CancellationState::default());
        let completion = Arc::new(Completion::new(Arc::downgrade(self)));
        let task = BlockingTask {
            generation,
            cancellation: Arc::clone(&cancellation),
            completion: Arc::clone(&completion),
            consumed: false,
        };
        let control = Arc::new(WorkControl {
            generation,
            completion: Arc::clone(&completion),
        });
        let work: Box<dyn Runnable> = Box::new(Work {
            generation,
            owner: Arc::downgrade(self),
            cancellation,
            completion: Arc::clone(&completion),
            control,
            function: Some(function),
        });

        let refusal = {
            let mut state = self.lock();
            state.submitted = state.submitted.saturating_add(1);
            if state.shutdown {
                Some(BlockingTaskError::Shutdown)
            } else if state.queue.len() >= self.queue_capacity {
                state.busy = state.busy.saturating_add(1);
                Some(BlockingTaskError::Busy)
            } else {
                state.accepted = state.accepted.saturating_add(1);
                state.queue.push_back(work);
                None
            }
        };
        match refusal {
            Some(error) => completion.settle(BlockingTaskSettlement {
                generation,
                result: Err(error),
            }),
            None => self.wake.notify_one(),
        }
        task
    }

    /// Queues `function` when a slot is free, or hands it back with the reason.
    ///
    /// Admission is decided under the pool lock, so a waiter that wins a slot
    /// cannot lose it to a concurrent submitter.
    fn try_submit<T, F>(
        self: &Arc<Self>,
        generation: u64,
        function: F,
    ) -> Result<BlockingTask<T>, (BlockingTaskError, F)>
    where
        T: Send + 'static,
        F: FnOnce(BlockingTaskContext) -> T + Send + 'static,
    {
        let task = {
            let mut state = self.lock();
            if state.shutdown {
                return Err((BlockingTaskError::Shutdown, function));
            }
            if state.queue.len() >= self.queue_capacity {
                return Err((BlockingTaskError::Busy, function));
            }
            let cancellation = Arc::new(CancellationState::default());
            let completion = Arc::new(Completion::new(Arc::downgrade(self)));
            let control = Arc::new(WorkControl {
                generation,
                completion: Arc::clone(&completion),
            });
            state.submitted = state.submitted.saturating_add(1);
            state.accepted = state.accepted.saturating_add(1);
            state.queue.push_back(Box::new(Work {
                generation,
                owner: Arc::downgrade(self),
                cancellation: Arc::clone(&cancellation),
                completion: Arc::clone(&completion),
                control,
                function: Some(function),
            }));
            BlockingTask {
                generation,
                cancellation,
                completion,
                consumed: false,
            }
        };
        self.wake.notify_one();
        Ok(task)
    }

    /// Parks `waker` until a queue slot frees; `true` when one is free already
    /// (or the owner shut down), so the caller retries instead of sleeping.
    fn wait_for_capacity(&self, waker: &Waker) -> bool {
        let mut state = self.lock();
        if state.shutdown || state.queue.len() < self.queue_capacity {
            return true;
        }
        if !state.capacity_waiters.iter().any(|w| w.will_wake(waker)) {
            state.capacity_waiters.push(waker.clone());
        }
        false
    }

    fn shutdown(&self) {
        let (pending, running) = {
            let mut state = self.lock();
            if state.shutdown {
                return;
            }
            state.shutdown = true;
            for waiter in state.capacity_waiters.drain(..) {
                waiter.wake();
            }
            (
                state.queue.drain(..).collect::<Vec<_>>(),
                state.running.clone(),
            )
        };
        for work in pending {
            let control = work.shutdown_control();
            settle_control(&*control, BlockingTaskError::Shutdown);
            let _ = catch_unwind(AssertUnwindSafe(|| drop(work)));
        }
        for control in running {
            settle_control(&*control, BlockingTaskError::Shutdown);
        }
        self.wake.notify_all();
    }
}

// Returns `true` when the worker caught at least one job panic.
fn worker_main(shared: Arc<Shared>) -> bool {
    struct WorkerExit(Arc<Shared>);

    impl Drop for WorkerExit {
        fn drop(&mut self) {
            let mut state = self.0.lock();
            state.exited = state.exited.saturating_add(1);
            self.0.wake.notify_all();
        }
    }

    let _exit = WorkerExit(Arc::clone(&shared));
    let mut had_panic = false;
    loop {
        let work = {
            let mut state = shared.lock();
            while !state.shutdown && state.queue.is_empty() {
                state = shared
                    .wake
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            if state.shutdown {
                return had_panic;
            }
            let work = state.queue.pop_front().expect("nonempty worker queue");
            let control = work.shutdown_control();
            state.running.push(Arc::clone(&control));
            // A slot just freed: every parked submitter retries admission.
            for waiter in state.capacity_waiters.drain(..) {
                waiter.wake();
            }
            (work, control)
        };
        let (work, control) = work;
        match catch_unwind(AssertUnwindSafe(|| work.run())) {
            // The task's own panic is contained by `run`; the join still
            // reports this worker as having run a panicking task.
            Ok(task_panicked) => had_panic |= task_panicked,
            Err(_) => {
                had_panic = true;
                settle_control(&*control, BlockingTaskError::Panicked);
            }
        }
        let mut state = shared.lock();
        state
            .running
            .retain(|candidate| !Arc::ptr_eq(candidate, &control));
    }
}

/// A submission waiting for queue space; see
/// [`BlockingTaskDispatcher::submit_waiting`].
#[must_use = "poll the task so the function is admitted and settles"]
pub struct WaitingBlockingTask<T, F> {
    shared: Arc<Shared>,
    generation: u64,
    pending: Option<F>,
    task: Option<BlockingTask<T>>,
}

// Nothing is pinned structurally: `F` is only ever moved out by value.
impl<T, F> Unpin for WaitingBlockingTask<T, F> {}

impl<T, F> Future for WaitingBlockingTask<T, F>
where
    T: Send + 'static,
    F: FnOnce(BlockingTaskContext) -> T + Send + 'static,
{
    type Output = BlockingTaskSettlement<T>;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;
        loop {
            if let Some(task) = this.task.as_mut() {
                return Pin::new(task).poll(context);
            }
            let function = this
                .pending
                .take()
                .expect("WaitingBlockingTask polled after it settled");
            match this.shared.try_submit(this.generation, function) {
                Ok(task) => this.task = Some(task),
                Err((BlockingTaskError::Busy, function)) => {
                    this.pending = Some(function);
                    if !this.shared.wait_for_capacity(context.waker()) {
                        return Poll::Pending;
                    }
                }
                Err((error, _function)) => {
                    return Poll::Ready(BlockingTaskSettlement {
                        generation: this.generation,
                        result: Err(error),
                    });
                }
            }
        }
    }
}

impl<T, F> fmt::Debug for WaitingBlockingTask<T, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WaitingBlockingTask")
            .field("generation", &self.generation)
            .field("admitted", &self.task.is_some())
            .finish_non_exhaustive()
    }
}

/// A non-owning, clonable submission handle.
///
/// It becomes permanently shut down when its [`BlockingTaskOwner`] is shut
/// down or dropped.
#[derive(Clone)]
pub struct BlockingTaskDispatcher {
    shared: Arc<Shared>,
}

impl BlockingTaskDispatcher {
    /// Submit one synchronous function without waiting for queue space.
    ///
    /// The returned future is immediately ready with [`BlockingTaskError::Busy`]
    /// when the fixed queue is full, or [`BlockingTaskError::Shutdown`] after
    /// owner shutdown. Otherwise exactly one settlement carries `generation`.
    pub fn submit<T, F>(&self, generation: u64, function: F) -> BlockingTask<T>
    where
        T: Send + 'static,
        F: FnOnce(BlockingTaskContext) -> T + Send + 'static,
    {
        self.shared.submit(generation, function)
    }

    /// Submit one synchronous function, waiting without a thread for queue space.
    ///
    /// Where [`Self::submit`] refuses a full queue with
    /// [`BlockingTaskError::Busy`], this future parks until a worker takes
    /// queued work and then queues `function`, so the worker and queue bounds
    /// still hold while nothing is refused. Dropping it before admission
    /// discards `function` unrun; after owner shutdown it settles
    /// [`BlockingTaskError::Shutdown`].
    pub fn submit_waiting<T, F>(&self, generation: u64, function: F) -> WaitingBlockingTask<T, F>
    where
        T: Send + 'static,
        F: FnOnce(BlockingTaskContext) -> T + Send + 'static,
    {
        WaitingBlockingTask {
            shared: Arc::clone(&self.shared),
            generation,
            pending: Some(function),
            task: None,
        }
    }

    /// Returns current capacities, occupancy, and saturating lifetime counters.
    #[must_use]
    pub fn snapshot(&self) -> BlockingTaskSnapshot {
        let state = self.shared.lock();
        BlockingTaskSnapshot {
            worker_capacity: self.shared.worker_capacity,
            queue_capacity: self.shared.queue_capacity,
            queued: state.queue.len(),
            running: state.running.len(),
            submitted: state.submitted,
            accepted: state.accepted,
            settled: state.settled,
            busy: state.busy,
            detached_workers: state.detached_workers,
            panicked_workers: state.panicked_workers,
            shutdown: state.shutdown,
        }
    }
}

impl fmt::Debug for BlockingTaskDispatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockingTaskDispatcher")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

/// Bounded diagnostics for one blocking-task owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockingTaskSnapshot {
    /// Maximum number of simultaneously running calls.
    pub worker_capacity: usize,
    /// Maximum number of calls waiting to run.
    pub queue_capacity: usize,
    /// Calls currently waiting to run.
    pub queued: usize,
    /// Calls currently running.
    pub running: usize,
    /// Submission attempts, including immediate refusals.
    pub submitted: u64,
    /// Attempts admitted to a worker or its queue.
    pub accepted: u64,
    /// Attempts that have reached one terminal settlement.
    ///
    /// Settlement does not release [`Self::queued`] or [`Self::running`]; those
    /// remain charged until the queued closure is discarded or the running call exits.
    pub settled: u64,
    /// Attempts refused because the queue was full.
    pub busy: u64,
    /// Workers detached after a bounded shutdown wait expired.
    pub detached_workers: usize,
    /// Workers whose thread unwound or whose loop caught at least one job panic.
    pub panicked_workers: usize,
    /// Whether shutdown has begun.
    pub shutdown: bool,
}

impl BlockingTaskSnapshot {
    /// Maximum number of admitted calls that can be live at once.
    #[must_use]
    pub const fn live_capacity(&self) -> usize {
        self.worker_capacity + self.queue_capacity
    }

    /// Number of admitted calls still occupying a worker or queue slot.
    ///
    /// This is independent of [`Self::settled`]: early cancellation or shutdown
    /// cannot free capacity while the underlying work still occupies it.
    #[must_use]
    pub const fn live(&self) -> usize {
        self.running + self.queued
    }
}

/// Outcome of a bounded shutdown wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BlockingShutdown {
    /// Every worker exited and was joined.
    Joined {
        /// Number of joined worker threads.
        workers: usize,
        /// Workers whose thread unwound or whose loop caught at least one job panic.
        panicked: usize,
    },
    /// At least one running blocking call outlived the wait budget.
    TimedOut {
        /// Number of worker threads that had not exited at the deadline.
        remaining: usize,
    },
}

/// Owner of a fixed worker set and a bounded pending queue.
///
/// Shutdown discards queued calls and asks running calls to stop through their
/// context. Running occupancy remains charged until the call exits. Because an
/// arbitrary synchronous call cannot be forcibly interrupted, bounded shutdown
/// detaches only workers that outlive its deadline. Dropping an owner applies
/// [`DEFAULT_SHUTDOWN_WAIT`] rather than detaching responsive workers.
pub struct BlockingTaskOwner {
    dispatcher: BlockingTaskDispatcher,
    handles: Vec<JoinHandle<bool>>,
}

impl BlockingTaskOwner {
    /// Starts `worker_count` threads with a queue of `queue_capacity` entries.
    ///
    /// # Errors
    ///
    /// Returns the first OS thread-creation error. Any threads already created
    /// for this owner are shut down and joined before returning.
    pub fn new(
        thread_name: &str,
        worker_count: NonZeroUsize,
        queue_capacity: NonZeroUsize,
    ) -> std::io::Result<Self> {
        let shared = Arc::new(Shared {
            state: Mutex::new(PoolState {
                queue: VecDeque::with_capacity(queue_capacity.get()),
                shutdown: false,
                running: Vec::with_capacity(worker_count.get()),
                exited: 0,
                submitted: 0,
                accepted: 0,
                settled: 0,
                busy: 0,
                detached_workers: 0,
                panicked_workers: 0,
                capacity_waiters: Vec::new(),
            }),
            wake: Condvar::new(),
            worker_capacity: worker_count.get(),
            queue_capacity: queue_capacity.get(),
        });
        let mut handles = Vec::with_capacity(worker_count.get());
        for index in 0..worker_count.get() {
            let worker = Arc::clone(&shared);
            match std::thread::Builder::new()
                .name(format!("{thread_name}-{index}"))
                .spawn(move || worker_main(worker))
            {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    shared.shutdown();
                    for handle in handles {
                        let _ = handle.join();
                    }
                    return Err(error);
                }
            }
        }
        Ok(Self {
            dispatcher: BlockingTaskDispatcher { shared },
            handles,
        })
    }

    /// Starts an owner with [`DEFAULT_WORKER_COUNT`] and [`DEFAULT_QUEUE_CAPACITY`].
    ///
    /// # Errors
    ///
    /// An I/O error when a worker thread cannot be spawned.
    pub fn with_defaults(thread_name: &str) -> std::io::Result<Self> {
        Self::new(thread_name, DEFAULT_WORKER_COUNT, DEFAULT_QUEUE_CAPACITY)
    }

    /// Builds a permanently shut-down owner for graceful thread-spawn fallback.
    ///
    /// Every submission settles immediately as [`BlockingTaskError::Shutdown`].
    #[must_use]
    pub fn unavailable(queue_capacity: NonZeroUsize) -> Self {
        Self {
            dispatcher: BlockingTaskDispatcher {
                shared: Arc::new(Shared {
                    state: Mutex::new(PoolState {
                        queue: VecDeque::new(),
                        shutdown: true,
                        running: Vec::new(),
                        exited: 0,
                        submitted: 0,
                        accepted: 0,
                        settled: 0,
                        busy: 0,
                        detached_workers: 0,
                        panicked_workers: 0,
                        capacity_waiters: Vec::new(),
                    }),
                    wake: Condvar::new(),
                    worker_capacity: 0,
                    queue_capacity: queue_capacity.get(),
                }),
            },
            handles: Vec::new(),
        }
    }

    /// Returns a clonable submission handle tied to this owner's lifetime.
    #[must_use]
    pub fn dispatcher(&self) -> BlockingTaskDispatcher {
        self.dispatcher.clone()
    }

    /// Submit one synchronous function without waiting for queue space.
    pub fn submit<T, F>(&self, generation: u64, function: F) -> BlockingTask<T>
    where
        T: Send + 'static,
        F: FnOnce(BlockingTaskContext) -> T + Send + 'static,
    {
        self.dispatcher.submit(generation, function)
    }

    /// Submit one synchronous function, waiting for queue space instead of
    /// refusing; see [`BlockingTaskDispatcher::submit_waiting`].
    pub fn submit_waiting<T, F>(&self, generation: u64, function: F) -> WaitingBlockingTask<T, F>
    where
        T: Send + 'static,
        F: FnOnce(BlockingTaskContext) -> T + Send + 'static,
    {
        self.dispatcher.submit_waiting(generation, function)
    }

    /// Prevents new admission and settles queued and running generations as shut down.
    ///
    /// This method never waits. A running call remains in [`BlockingTaskSnapshot::running`]
    /// until it actually exits, even though its future has already settled.
    pub fn shutdown(&mut self) {
        self.dispatcher.shared.shutdown();
    }

    /// Shuts down and waits at most `budget` for worker exit.
    ///
    /// A timeout records detached workers in [`BlockingTaskSnapshot::detached_workers`].
    pub fn shutdown_timeout(&mut self, budget: Duration) -> BlockingShutdown {
        self.shutdown();
        let deadline = Instant::now().checked_add(budget);
        let worker_count = self.handles.len();
        let mut state = self.dispatcher.shared.lock();
        while state.exited < worker_count {
            let Some(deadline) = deadline else {
                drop(state);
                return self.finish_timed_out_shutdown();
            };
            let now = Instant::now();
            if now >= deadline {
                drop(state);
                return self.finish_timed_out_shutdown();
            }
            let remaining = deadline.saturating_duration_since(now);
            state = match self.dispatcher.shared.wake.wait_timeout(state, remaining) {
                Ok((state, _)) => state,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
        drop(state);
        self.join_all_handles()
    }

    /// Shuts down, then blocks until every worker has exited and been joined.
    ///
    /// Never returns [`BlockingShutdown::TimedOut`] and never detaches a worker
    /// handle. Call only from a thread that can block indefinitely (an app
    /// close-helper, not the UI or async executor thread).
    pub fn shutdown_and_join(mut self) -> BlockingShutdown {
        self.shutdown();
        let worker_count = self.handles.len();
        let mut state = self.dispatcher.shared.lock();
        while state.exited < worker_count {
            state = self
                .dispatcher
                .shared
                .wake
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        drop(state);
        self.join_all_handles()
    }

    /// Closes admission and joins all workers only when every handle has
    /// already exited, otherwise returns `None` while retaining every handle.
    ///
    /// A helper thread can poll this without blocking. When `None` is returned
    /// all handles remain owned; a later call to [`Self::shutdown_and_join`] or
    /// [`Self::shutdown_timeout`] still has full custody.
    pub fn try_join_shutdown(&mut self) -> Option<BlockingShutdown> {
        self.shutdown();
        if self.handles.iter().all(JoinHandle::is_finished) {
            Some(self.join_all_handles())
        } else {
            None
        }
    }

    // Joins every retained handle and records panic counters.
    fn join_all_handles(&mut self) -> BlockingShutdown {
        let joined = self.handles.len();
        let mut panicked: usize = 0;
        for handle in self.handles.drain(..) {
            match handle.join() {
                Ok(true) | Err(_) => {
                    panicked = panicked.saturating_add(1);
                }
                Ok(false) => {}
            }
        }
        if panicked > 0 {
            let mut state = self.dispatcher.shared.lock();
            state.panicked_workers = state.panicked_workers.saturating_add(panicked);
        }
        BlockingShutdown::Joined {
            workers: joined,
            panicked,
        }
    }

    fn finish_timed_out_shutdown(&mut self) -> BlockingShutdown {
        let mut detached: usize = 0;
        let mut panicked: usize = 0;
        for handle in self.handles.drain(..) {
            if handle.is_finished() {
                match handle.join() {
                    Ok(true) | Err(_) => {
                        panicked = panicked.saturating_add(1);
                    }
                    Ok(false) => {}
                }
            } else {
                detached = detached.saturating_add(1);
            }
        }
        let mut state = self.dispatcher.shared.lock();
        state.detached_workers = state.detached_workers.saturating_add(detached);
        state.panicked_workers = state.panicked_workers.saturating_add(panicked);
        if detached == 0 {
            BlockingShutdown::Joined {
                workers: state.exited,
                panicked,
            }
        } else {
            BlockingShutdown::TimedOut {
                remaining: detached,
            }
        }
    }
}

impl fmt::Debug for BlockingTaskOwner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockingTaskOwner")
            .field("snapshot", &self.dispatcher.snapshot())
            .field("worker_handles", &self.handles.len())
            .finish()
    }
}

impl Drop for BlockingTaskOwner {
    fn drop(&mut self) {
        let _ = self.shutdown_timeout(DEFAULT_SHUTDOWN_WAIT);
    }
}

#[cfg(test)]
mod tests;
