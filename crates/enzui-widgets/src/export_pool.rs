//! Bounded worker pool for portal-parent exports.
//!
//! Two workers and one queue slot bound admitted exports. A full pool refuses
//! work as [`PoolError::Busy`](enzui_core::blocking_task::BlockingTaskError::Busy); cancellation keeps admitted work charged until
//! worker reclamation. [`crate::portal_parent::export_parent_task`] handles
//! pre-admission portal cancellation; direct submissions do not check it.
//! Owner drop shuts down admission and waits for a bounded interval, detaching
//! workers that outlive it. Existing dispatchers then refuse work as `Shutdown`.

use std::num::NonZeroUsize;

use enzui_core::blocking_task::{
    BlockingShutdown, BlockingTask, BlockingTaskContext, BlockingTaskDispatcher, BlockingTaskOwner,
    BlockingTaskSnapshot,
};

/// Maximum concurrent export workers.
///
/// Two admits a re-export during predecessor cancellation without letting a
/// runaway caller accumulate blocked threads against a stalled compositor.
pub const EXPORT_WORKER_COUNT: NonZeroUsize = NonZeroUsize::new(2).unwrap();

/// Queue depth behind the live workers.
///
/// One: admits a single queued re-export behind two running workers. Every
/// request beyond that is refused as [`BlockingTaskError::Busy`](enzui_core::blocking_task::BlockingTaskError::Busy).
pub const EXPORT_QUEUE_CAPACITY: NonZeroUsize = NonZeroUsize::new(1).unwrap();

const THREAD_NAME: &str = "portal-export";

/// A bounded owner of parent-export worker threads.
///
/// Wraps a [`BlockingTaskOwner`] with portal-specific configuration.
/// Bounded shutdown on drop joins responsive workers and detaches only
/// those that outlive the wait budget.
pub struct ExportPoolOwner {
    owner: BlockingTaskOwner,
}

/// A clonable submission handle that becomes shut down when its
/// [`ExportPoolOwner`] is dropped.
#[derive(Clone)]
pub struct ExportPoolDispatcher {
    dispatcher: BlockingTaskDispatcher,
}

impl ExportPoolOwner {
    /// Starts the export worker threads.
    ///
    /// # Errors
    ///
    /// Returns the OS thread-creation error. Any threads already started
    /// for this owner are shut down and joined before returning.
    pub fn start() -> std::io::Result<Self> {
        BlockingTaskOwner::new(THREAD_NAME, EXPORT_WORKER_COUNT, EXPORT_QUEUE_CAPACITY)
            .map(|owner| Self { owner })
    }

    /// Builds a permanently shut-down pool for graceful spawn-failure fallback.
    ///
    /// Every submission settles immediately as [`BlockingTaskError::Shutdown`](enzui_core::blocking_task::BlockingTaskError::Shutdown).
    #[must_use]
    pub fn unavailable() -> Self {
        Self {
            owner: BlockingTaskOwner::unavailable(EXPORT_QUEUE_CAPACITY),
        }
    }

    /// Returns a clonable submission handle.
    #[must_use]
    pub fn dispatcher(&self) -> ExportPoolDispatcher {
        ExportPoolDispatcher {
            dispatcher: self.owner.dispatcher(),
        }
    }

    /// Submits work directly through the owner.
    pub fn submit<T, F>(&self, generation: u64, work: F) -> BlockingTask<T>
    where
        T: Send + 'static,
        F: FnOnce(BlockingTaskContext) -> T + Send + 'static,
    {
        self.owner.submit(generation, work)
    }

    /// Current occupancy and lifetime counters.
    #[must_use]
    pub fn snapshot(&self) -> BlockingTaskSnapshot {
        self.owner.dispatcher().snapshot()
    }

    /// Prevent new admission and settle pending work without waiting for worker exit.
    /// Use [`Self::shutdown_timeout`] to wait with a budget and observe the join outcome.
    pub fn shutdown(&mut self) {
        self.owner.shutdown();
    }

    /// Shuts down with a bounded wait, returning the join outcome.
    pub fn shutdown_timeout(&mut self, budget: std::time::Duration) -> BlockingShutdown {
        self.owner.shutdown_timeout(budget)
    }
}

impl core::fmt::Debug for ExportPoolOwner {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ExportPoolOwner")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}

impl ExportPoolDispatcher {
    /// Submits work through the dispatcher.
    pub fn submit<T, F>(&self, generation: u64, work: F) -> BlockingTask<T>
    where
        T: Send + 'static,
        F: FnOnce(BlockingTaskContext) -> T + Send + 'static,
    {
        self.dispatcher.submit(generation, work)
    }

    /// Current occupancy and lifetime counters.
    #[must_use]
    pub fn snapshot(&self) -> BlockingTaskSnapshot {
        self.dispatcher.snapshot()
    }
}

impl core::fmt::Debug for ExportPoolDispatcher {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ExportPoolDispatcher")
            .field("snapshot", &self.snapshot())
            .finish()
    }
}
pub use enzui_core::blocking_task::BlockingTaskError as PoolError;
pub use enzui_core::blocking_task::BlockingTaskSettlement as PoolSettlement;
