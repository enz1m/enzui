//! Bounded latest-frame delivery from a decode worker to the UI.
//!
//! [`BoundedFrameSink`] owns exactly the buffers retained by [`FramePlan`], drops evicted buffers immediately, and delegates generation and ordering decisions to that shared plan.

use crate::frame_plan::{FramePlan, FrameSeq, FrameVerdict};
use crate::media::Generation;
use crate::media::rgba::Rgba8Frame;

/// The buffer-ownership result of delivering a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// The frame was retained. Nothing was freed.
    Retained,
    /// The frame was retained and an older queued buffer was freed.
    RetainedFreeingOlder {
        /// The sequence number whose buffer was released.
        freed: FrameSeq,
    },
    /// The frame was refused and its buffer dropped immediately, because it came
    /// from a superseded generation.
    DiscardedStale {
        /// The stale generation the frame carried.
        generation: Generation,
    },
    /// The frame was refused and its buffer dropped immediately, because it is
    /// older than a frame already retained for this generation.
    DiscardedOutOfOrder {
        /// The out-of-order frame's sequence number.
        seq: FrameSeq,
    },
}

/// Owns the decoded buffers represented by a bounded [`FramePlan`].
#[derive(Debug)]
pub struct BoundedFrameSink {
    plan: FramePlan<Generation>,
    displayed: Option<Rgba8Frame>,
    in_flight: Option<Rgba8Frame>,
    queued: Option<Rgba8Frame>,
    accepted: u64,
    dropped_for_bound: u64,
    discarded_stale: u64,
    discarded_out_of_order: u64,
}

impl BoundedFrameSink {
    /// A sink for a source at `generation`, holding nothing.
    #[must_use]
    pub fn new(generation: Generation) -> BoundedFrameSink {
        BoundedFrameSink {
            plan: FramePlan::new(generation),
            displayed: None,
            in_flight: None,
            queued: None,
            accepted: 0,
            dropped_for_bound: 0,
            discarded_stale: 0,
            discarded_out_of_order: 0,
        }
    }

    /// Offers a frame to the plan and immediately applies its buffer-retention verdict.
    pub fn deliver(&mut self, frame: Rgba8Frame) -> Delivery {
        let seq = frame.seq();
        let generation = frame.generation();

        match self.plan.offer(seq, generation) {
            FrameVerdict::DiscardedStaleGeneration { generation } => {
                self.discarded_stale += 1;
                drop(frame);
                Delivery::DiscardedStale { generation }
            }
            FrameVerdict::DiscardedOutOfOrder { seq } => {
                self.discarded_out_of_order += 1;
                drop(frame);
                Delivery::DiscardedOutOfOrder { seq }
            }
            FrameVerdict::AcceptedInFlight { .. } => {
                self.accepted += 1;
                // The plan may have implicitly recreated on a newer generation,
                // which clears its slots; mirror that for the buffers.
                self.sync_slots_to_plan();
                self.in_flight = Some(frame);
                Delivery::Retained
            }
            FrameVerdict::Queued { .. } => {
                self.accepted += 1;
                self.queued = Some(frame);
                Delivery::Retained
            }
            FrameVerdict::ReplacedQueued { dropped, .. } => {
                self.accepted += 1;
                self.dropped_for_bound += 1;
                // Replacing the queued frame frees its buffer and enforces the memory bound.
                let displaced = self.queued.replace(frame);
                debug_assert!(
                    displaced.is_some(),
                    "the plan reported ReplacedQueued but the sink held no queued buffer"
                );
                drop(displaced);
                Delivery::RetainedFreeingOlder { freed: dropped }
            }
        }
    }

    fn sync_slots_to_plan(&mut self) {
        if self.plan.displayed_seq().is_none() {
            self.displayed = None;
        }
        if self.plan.in_flight_seq().is_none() {
            self.in_flight = None;
        }
        if self.plan.queued_seq().is_none() {
            self.queued = None;
        }
    }

    /// Present the in-flight frame: it becomes displayed, and the queued latest
    /// is promoted into the freed in-flight slot.
    ///
    /// Returns a reference to the newly-displayed frame, or `None` if there was
    /// nothing in flight. The previously-displayed frame's buffer is freed.
    pub fn present(&mut self) -> Option<&Rgba8Frame> {
        let presented_seq = self.plan.present()?;
        let ready = self.in_flight.take();
        debug_assert!(
            ready.is_some(),
            "the plan presented {presented_seq:?} but the sink held no in-flight buffer"
        );
        self.displayed = ready;
        self.in_flight = self.queued.take();
        self.displayed.as_ref()
    }

    /// Advances to a new generation while keeping the displayed frame and dropping old in-flight and queued buffers.
    pub fn replace_source(&mut self, generation: Generation) {
        self.plan.recreate_keeping_displayed(generation);
        self.in_flight = None;
        self.queued = None;
    }

    /// The frame currently displayed, if any.
    #[must_use]
    pub fn displayed(&self) -> Option<&Rgba8Frame> {
        self.displayed.as_ref()
    }

    /// The generation the sink currently accepts frames for.
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.plan.generation()
    }

    /// Returns the number of owned frame buffers, equal to the plan's retained-frame count.
    #[must_use]
    pub fn retained_buffers(&self) -> usize {
        usize::from(self.displayed.is_some())
            + usize::from(self.in_flight.is_some())
            + usize::from(self.queued.is_some())
    }

    /// The total bytes of pixel data the sink is holding.
    #[must_use]
    pub fn retained_bytes(&self) -> usize {
        [
            self.displayed.as_ref(),
            self.in_flight.as_ref(),
            self.queued.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(Rgba8Frame::byte_len)
        .sum()
    }

    /// Returns how many frames were retained at least momentarily.
    #[must_use]
    pub fn accepted(&self) -> u64 {
        self.accepted
    }

    /// How many frames were dropped specifically to keep the latest-only bound.
    #[must_use]
    pub fn dropped_for_bound(&self) -> u64 {
        self.dropped_for_bound
    }

    /// How many frames were discarded as belonging to a superseded generation.
    #[must_use]
    pub fn discarded_stale(&self) -> u64 {
        self.discarded_stale
    }

    /// How many frames were discarded as out-of-order within their generation.
    #[must_use]
    pub fn discarded_out_of_order(&self) -> u64 {
        self.discarded_out_of_order
    }

    /// The plan backing this sink, for callers that need the sequence-level view.
    #[must_use]
    pub fn plan(&self) -> &FramePlan<Generation> {
        &self.plan
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame_plan::FrameBounds;

    fn frame(generation: Generation, seq: u64) -> Rgba8Frame {
        Rgba8Frame::new(generation, FrameSeq(seq), 2, 2, vec![0u8; 16])
            .expect("a 2x2 frame with 16 bytes is valid")
    }

    fn gen_n(n: u64) -> Generation {
        let mut g = Generation::FIRST;
        for _ in 0..n {
            g = g.next();
        }
        g
    }

    #[test]
    fn a_slow_ui_cannot_grow_the_queue() {
        let mut sink = BoundedFrameSink::new(gen_n(0));
        let mut max_buffers_seen = 0usize;
        let mut max_bytes_seen = 0usize;

        for seq in 1..=1000 {
            sink.deliver(frame(gen_n(0), seq));
            max_buffers_seen = max_buffers_seen.max(sink.retained_buffers());
            max_bytes_seen = max_bytes_seen.max(sink.retained_bytes());
            assert!(
                sink.retained_buffers() <= FrameBounds::MAX_RETAINED,
                "retained {} buffers at seq {seq}, bound is {}",
                sink.retained_buffers(),
                FrameBounds::MAX_RETAINED
            );
        }

        assert_eq!(sink.accepted(), 1000, "all 1000 frames were offered");
        assert!(
            max_buffers_seen > 0,
            "the harness must have observed buffers being held, got {max_buffers_seen}"
        );
        assert_eq!(
            sink.dropped_for_bound(),
            998,
            "each frame after the second must displace the queued latest"
        );
        assert_eq!(
            max_bytes_seen, 32,
            "at most two buffers (32 bytes) were ever held with no present"
        );
        assert_eq!(sink.plan().queued_seq(), Some(FrameSeq(1000)));
        assert_eq!(sink.plan().in_flight_seq(), Some(FrameSeq(1)));
    }
}
