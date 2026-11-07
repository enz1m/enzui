//! Bounded latest-frame planning for media generations.
//!
//! Source replacement keeps the displayed frame while discarding queued work from older generations.

use crate::frame_plan;
use crate::media::Generation;

/// A bounded latest-frame plan keyed by media [`Generation`].
pub type FramePlan = frame_plan::FramePlan<Generation>;

/// The verdict of offering a frame to a media [`FramePlan`].
pub type FrameVerdict = frame_plan::FrameVerdict<Generation>;

pub use crate::frame_plan::{FrameBounds, FrameSeq};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::Generation;

    fn g(n: u64) -> Generation {
        let mut generation = Generation::FIRST;
        for _ in 0..n {
            generation = generation.next();
        }
        generation
    }
    fn s(n: u64) -> FrameSeq {
        FrameSeq(n)
    }

    #[test]
    fn saturation_drops_oldest_keeps_newest() {
        let mut plan = FramePlan::new(g(0));
        assert_eq!(
            plan.offer(s(1), g(0)),
            FrameVerdict::AcceptedInFlight { seq: s(1) }
        );
        assert_eq!(plan.offer(s(2), g(0)), FrameVerdict::Queued { seq: s(2) });
        assert_eq!(
            plan.offer(s(3), g(0)),
            FrameVerdict::ReplacedQueued {
                seq: s(3),
                dropped: s(2)
            }
        );
        assert_eq!(
            plan.offer(s(4), g(0)),
            FrameVerdict::ReplacedQueued {
                seq: s(4),
                dropped: s(3)
            }
        );
        assert!(
            plan.retained() <= FrameBounds::MAX_RETAINED,
            "retained must never exceed the bound"
        );
        assert_eq!(
            plan.queued_seq(),
            Some(s(4)),
            "the NEWEST frame survives saturation, not an older one"
        );
        assert_eq!(plan.in_flight_seq(), Some(s(1)));
    }
}
