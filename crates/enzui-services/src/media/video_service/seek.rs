//! Bounded request receipts and outcomes for video seeking.

use super::{SeekMode, Shared};
use crate::media::Generation;
use std::collections::VecDeque;
use std::time::Duration;

/// Tracked seeks awaiting completion or outcome collection per service.
pub const VIDEO_SEEK_CAPACITY: usize = 64;

/// Identity and intent of an admitted seek, preserved in its outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeekRequest {
    /// Source generation at admission.
    pub generation: Generation,
    /// Monotonically increasing identity within this service.
    pub id: u64,
    /// Caller position before clamping to the source duration.
    pub requested: Duration,
    /// Requested precision.
    pub mode: SeekMode,
}

/// Why a tracked seek was not admitted; no outcome is reserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekAdmissionError {
    /// The command queue or uncollected outcome budget is full.
    Full,
    /// The worker has closed.
    Closed,
}

/// Why an admitted seek did not settle successfully.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekRejection {
    /// The source is opening or has no live pipeline.
    NotReady,
    /// A newer queued seek replaced this request.
    Superseded,
    /// The pipeline was replaced, unloaded, reopened, or retired.
    SourceChanged,
    /// The backend cannot represent or refused the target.
    BackendRefused,
    /// No completion arrived within the seek deadline.
    TimedOut,
    /// The backend completed outside the accurate-seek tolerance.
    PositionMismatch,
    /// The backend supplied no position on completion.
    PositionUnavailable,
    /// The worker closed before completing this request.
    Closed,
}

/// One terminal result for a tracked seek, including the queried position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeekOutcome {
    /// Receipt returned at admission.
    pub request: SeekRequest,
    /// Effective target when known; otherwise the original request.
    pub target: Duration,
    /// Backend position queried at completion or rejection, when available.
    pub actual: Option<Duration>,
    /// Successful settlement or its typed refusal.
    pub result: Result<(), SeekRejection>,
}

#[derive(Debug, Default)]
pub(super) struct Receipts {
    next: u64,
    pending: Vec<SeekRequest>,
    completed: VecDeque<SeekOutcome>,
}

impl Receipts {
    pub(super) fn reserve(
        &mut self,
        generation: Generation,
        requested: Duration,
        mode: SeekMode,
    ) -> Result<SeekRequest, SeekAdmissionError> {
        if self.pending.len() + self.completed.len() >= VIDEO_SEEK_CAPACITY {
            return Err(SeekAdmissionError::Full);
        }
        self.next = self.next.checked_add(1).ok_or(SeekAdmissionError::Full)?;
        let request = SeekRequest {
            generation,
            id: self.next,
            requested,
            mode,
        };
        self.pending.push(request);
        Ok(request)
    }

    pub(super) fn release(&mut self, request: SeekRequest) {
        self.pending.retain(|pending| *pending != request);
    }

    fn complete(&mut self, outcome: SeekOutcome) -> bool {
        let Some(index) = self
            .pending
            .iter()
            .position(|request| *request == outcome.request)
        else {
            return false;
        };
        self.pending.swap_remove(index);
        self.completed.push_back(outcome);
        true
    }

    pub(super) fn take(&mut self) -> Vec<SeekOutcome> {
        self.completed.drain(..).collect()
    }
}

impl Shared {
    pub(super) fn seek_outcome(&self, outcome: SeekOutcome) {
        let wake = {
            let mut slot = self.lock();
            if !slot.seeks.complete(outcome) {
                return;
            }
            slot.wake()
        };
        if let Some(wake) = wake {
            wake.wake();
        }
    }

    pub(super) fn cancel_seeks(&self, generation: Option<Generation>, reason: SeekRejection) {
        let wake = {
            let mut slot = self.lock();
            let requests: Vec<_> = slot
                .seeks
                .pending
                .iter()
                .copied()
                .filter(|request| {
                    generation.is_none_or(|generation| request.generation == generation)
                })
                .collect();
            if requests.is_empty() {
                return;
            }
            for request in requests {
                slot.seeks.complete(SeekOutcome {
                    request,
                    target: request.requested,
                    actual: None,
                    result: Err(reason),
                });
            }
            slot.wake()
        };
        if let Some(wake) = wake {
            wake.wake();
        }
    }
}
