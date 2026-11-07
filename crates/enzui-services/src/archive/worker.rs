//! The bounded owner extractions run on.

use super::{ArchiveError, ExtractProgress, ExtractReport, ExtractRequest};
use enzui_core::blocking_task::{BlockingTask, BlockingTaskCancellation, BlockingTaskOwner};
use enzui_core::operation_activity::ActivityKey;
use std::collections::BTreeMap;
use std::num::NonZeroUsize;

/// What one extraction settles to.
pub type ExtractOutcome = Result<ExtractReport, ArchiveError>;

/// Runs extractions off the UI thread, two at a time with a short queue
/// that refuses a burst as busy, and keeps each in-flight one's
/// cancellation and [`ExtractProgress`] under its activity row's key.
pub struct ArchiveWorker {
    owner: BlockingTaskOwner,
    running: BTreeMap<
        ActivityKey,
        (
            u64,
            BlockingTaskCancellation<ExtractOutcome>,
            ExtractProgress,
        ),
    >,
    generation: u64,
}

impl std::fmt::Debug for ArchiveWorker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArchiveWorker")
            .field("in_flight", &self.running.len())
            .finish()
    }
}

const WORKERS: usize = 2;
const QUEUE: usize = 4;

impl ArchiveWorker {
    /// A worker whose threads are named after `thread_name`.
    #[must_use]
    pub fn new(thread_name: &str) -> Self {
        let queue = const { NonZeroUsize::new(QUEUE).expect("nonzero") };
        let owner = BlockingTaskOwner::new(
            thread_name,
            const { NonZeroUsize::new(WORKERS).expect("nonzero") },
            queue,
        )
        .unwrap_or_else(|_| BlockingTaskOwner::unavailable(queue));
        Self {
            owner,
            running: BTreeMap::new(),
            generation: 0,
        }
    }

    /// Starts `request` under `key`; await the task (or its `settled`
    /// signal) and call [`Self::settled`] once it resolves.
    pub fn extract(
        &mut self,
        key: ActivityKey,
        request: ExtractRequest,
    ) -> BlockingTask<ExtractOutcome> {
        self.generation = self.generation.wrapping_add(1);
        let progress = ExtractProgress::default();
        let counters = progress.clone();
        let task = self.owner.submit(self.generation, move |context| {
            super::extract(&request, &counters, &|| context.is_cancelled())
        });
        // A key started again replaces its entry; the earlier task's late
        // settlement then names a generation that is no longer there.
        self.running
            .insert(key, (self.generation, task.cancellation(), progress));
        task
    }

    /// The live counters of the extraction behind `key`.
    #[must_use]
    pub fn progress(&self, key: ActivityKey) -> Option<&ExtractProgress> {
        self.running.get(&key).map(|(_, _, progress)| progress)
    }

    /// Requests cancellation of the extraction behind `key`; `false` when
    /// none runs under it. Its staging folder is removed as it stops.
    pub fn cancel(&mut self, key: ActivityKey) -> bool {
        self.running
            .remove(&key)
            .map(|(_, cancellation, _)| cancellation.cancel())
            .is_some()
    }

    /// Forgets the extraction under `key` that settled with `generation`
    /// ([`BlockingTaskSettlement::generation`](enzui_core::blocking_task::BlockingTaskSettlement::generation)).
    /// A stale settlement, from a task the key has since been reused for,
    /// leaves the newer extraction in place.
    pub fn settled(&mut self, key: ActivityKey, generation: u64) {
        if self
            .running
            .get(&key)
            .is_some_and(|(live, _, _)| *live == generation)
        {
            self.running.remove(&key);
        }
    }

    /// Extractions started and not yet settled or cancelled.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.running.len()
    }
}
