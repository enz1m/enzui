//! Bounded latest-frame retention shared by asynchronous frame producers.
//!
//! Each plan retains at most one displayed, one in-flight, and one queued frame,
//! while generation changes prevent stale frames from being presented.

use std::fmt::Debug;

/// The generation newtype a [`FramePlan`] is keyed on. Each subsystem supplies its own
/// (`web::view_id::Generation`, `media::Generation`); the plan only needs to order them
/// (newer > older) and copy them into frame records.
pub trait FrameGeneration: Copy + Ord + Debug {}

impl<T: Copy + Ord + Debug> FrameGeneration for T {}

/// A monotonic per-source frame sequence number. Higher is newer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrameSeq(pub u64);

impl FrameSeq {
    /// Increments the underlying sequence counter.
    ///
    /// # Panics
    ///
    /// Panics at `u64::MAX` when overflow checks are enabled.
    #[must_use]
    pub fn next(self) -> FrameSeq {
        FrameSeq(self.0 + 1)
    }
}

/// Fixed limits for displayed, in-flight, and latest-queued frame retention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameBounds;

impl FrameBounds {
    /// The maximum number of frames retained at once: one displayed, one in
    /// flight, one latest-queued.
    pub const MAX_RETAINED: usize = 3;
    /// The maximum number of frames queued waiting to become in-flight. Latest-only: a
    /// newer frame replaces the queued one.
    pub const MAX_QUEUED: usize = 1;
    /// The maximum number of frames in flight (accepted, uploading).
    pub const MAX_IN_FLIGHT: usize = 1;
}

/// What happened to a frame offered to [`FramePlan::offer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameVerdict<G: FrameGeneration> {
    /// The frame was accepted as the new in-flight frame (nothing was in flight).
    AcceptedInFlight {
        /// The accepted frame's sequence number.
        seq: FrameSeq,
    },
    /// The frame was queued as the latest (something is already in flight).
    Queued {
        /// The queued frame's sequence number.
        seq: FrameSeq,
    },
    /// The frame replaced a stale queued frame (latest-only coalescing). The
    /// displaced frame's sequence number is carried so the caller can free its
    /// buffer.
    ReplacedQueued {
        /// The newly queued frame's sequence number.
        seq: FrameSeq,
        /// The older queued frame that was dropped.
        dropped: FrameSeq,
    },
    /// The frame was discarded because it is from a superseded generation.
    DiscardedStaleGeneration {
        /// The stale generation the frame carried.
        generation: G,
    },
    /// The frame was discarded because its sequence is not newer than every retained frame in the current generation.
    DiscardedOutOfOrder {
        /// The rejected frame's sequence number.
        seq: FrameSeq,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameRef<G: FrameGeneration> {
    seq: FrameSeq,
    generation: G,
}

/// The latest-frame presentation model for one source, generic over that source’s
/// generation type.
#[derive(Debug, Clone)]
pub struct FramePlan<G: FrameGeneration> {
    generation: G,
    displayed: Option<FrameRef<G>>,
    in_flight: Option<FrameRef<G>>,
    queued: Option<FrameRef<G>>,
}

impl<G: FrameGeneration> FramePlan<G> {
    /// A fresh plan for a source at `generation`, with nothing displayed.
    #[must_use]
    pub fn new(generation: G) -> FramePlan<G> {
        FramePlan {
            generation,
            displayed: None,
            in_flight: None,
            queued: None,
        }
    }

    /// Advance to a new generation, dropping everything — including the displayed frame.
    pub fn recreate(&mut self, generation: G) {
        debug_assert!(
            generation > self.generation,
            "a recreate must advance the generation"
        );
        self.generation = generation;
        self.displayed = None;
        self.in_flight = None;
        self.queued = None;
    }

    /// Advance to a new generation, keeping the displayed frame but dropping the in-flight
    /// and queued frames.
    pub fn recreate_keeping_displayed(&mut self, generation: G) {
        debug_assert!(
            generation > self.generation,
            "a recreate must advance the generation"
        );
        self.generation = generation;
        self.in_flight = None;
        self.queued = None;
    }

    /// The current generation.
    #[must_use]
    pub fn generation(&self) -> G {
        self.generation
    }

    /// The number of frames currently retained (0..=[`FrameBounds::MAX_RETAINED`]).
    #[must_use]
    pub fn retained(&self) -> usize {
        usize::from(self.displayed.is_some())
            + usize::from(self.in_flight.is_some())
            + usize::from(self.queued.is_some())
    }

    /// The sequence number of the displayed frame, if any.
    #[must_use]
    pub fn displayed_seq(&self) -> Option<FrameSeq> {
        self.displayed.map(|f| f.seq)
    }

    /// The sequence number of the in-flight frame, if any.
    #[must_use]
    pub fn in_flight_seq(&self) -> Option<FrameSeq> {
        self.in_flight.map(|f| f.seq)
    }

    /// The sequence number of the queued frame, if any.
    #[must_use]
    pub fn queued_seq(&self) -> Option<FrameSeq> {
        self.queued.map(|f| f.seq)
    }

    // Compare sequences only within the current generation; a retained old-generation
    // display must not reject a restarted low sequence.
    fn newest_seq(&self) -> Option<FrameSeq> {
        [self.displayed, self.in_flight, self.queued]
            .into_iter()
            .flatten()
            .filter(|f| f.generation == self.generation)
            .map(|f| f.seq)
            .max()
    }

    /// Offer a newly-produced frame (identified by `seq` and its source `generation`) to
    /// the plan.
    pub fn offer(&mut self, seq: FrameSeq, generation: G) -> FrameVerdict<G> {
        if generation < self.generation {
            return FrameVerdict::DiscardedStaleGeneration { generation };
        }
        // Replace all retained frames before comparing sequence numbers from a newer generation.
        if generation > self.generation {
            self.recreate(generation);
        }

        if let Some(newest) = self.newest_seq()
            && seq <= newest
        {
            return FrameVerdict::DiscardedOutOfOrder { seq };
        }

        let frame = FrameRef { seq, generation };
        if self.in_flight.is_none() {
            self.in_flight = Some(frame);
            FrameVerdict::AcceptedInFlight { seq }
        } else if let Some(old) = self.queued.replace(frame) {
            FrameVerdict::ReplacedQueued {
                seq,
                dropped: old.seq,
            }
        } else {
            FrameVerdict::Queued { seq }
        }
    }

    /// Mark the in-flight frame as uploaded and ready; it becomes the displayed frame, and
    /// any queued frame is promoted to in-flight.
    pub fn present(&mut self) -> Option<FrameSeq> {
        let ready = self.in_flight.take()?;
        self.displayed = Some(ready);
        self.in_flight = self.queued.take();
        Some(ready.seq)
    }
}

#[cfg(test)]
mod tests;
