//! Motion that keeps going at a steady speed: content sliding until the
//! next update moves it on, handed to the render lane whole.
//!
//! A widget whose content drifts (a live chart's timeline) would otherwise
//! ask the UI thread for a frame every time the content moved a pixel and
//! draw the whole window again. Published as one travel, the lane moves the
//! recorded node on its own and the UI thread sleeps until the content
//! changes.

use iced::Vector;
use iced::advanced::renderer::{NodeId, Track, TrackAnimation};
use iced::time::{Duration, Instant};

/// Content leaving `origin` at `speed` logical pixels a second along
/// `direction`, in whole steps of `step` logical pixels (`0` moves
/// continuously).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Drift {
    pub origin: Instant,
    pub speed: f32,
    pub direction: Vector,
    pub step: f32,
}

impl Drift {
    /// Where the content is at `now`.
    ///
    /// The position depends only on the time since the origin, so every
    /// drift published from the same origin agrees on it: one still playing
    /// when the next is published places the content where the next would.
    pub(crate) fn position(&self, now: Instant) -> Vector {
        let travel =
            f64::from(self.speed) * now.saturating_duration_since(self.origin).as_secs_f64();
        let travel = if self.step > 0.0 {
            (travel / f64::from(self.step)).floor() * f64::from(self.step)
        } else {
            travel
        };
        self.direction * travel as f32
    }

    /// `node` drifting from `start` for `duration`, for the lane to play.
    ///
    /// Stepping keeps a slow drift from asking for a frame that moves its
    /// content by a sliver of a pixel: each presented step repaints what
    /// drifts, so a timeline crossing thirteen pixels a second in one-pixel
    /// steps is presented thirteen times a second, not at every refresh.
    /// `generation` tells the lane a new drift from a continuation.
    pub(crate) fn animation(
        &self,
        node: NodeId,
        start: Instant,
        duration: Duration,
        generation: u64,
    ) -> Option<TrackAnimation> {
        let seconds = duration.as_secs_f64();
        if seconds.is_nan() || seconds <= 0.0 || !self.speed.is_finite() {
            return None;
        }
        let track = |at: Instant| Track {
            translation: self.position(at),
            ..Track::identity(node)
        };
        let at = |instant: Instant| {
            (instant.saturating_duration_since(start).as_secs_f64() / seconds).clamp(0.0, 1.0)
                as f32
        };
        let end = start + duration;
        let mut keyframes = vec![(0.0, track(start))];
        if self.step > 0.0 && self.speed > 0.0 {
            // Each step holds until the drift crosses the next: the instant
            // the travel from the origin reaches the next whole step.
            let per_step = f64::from(self.step) / f64::from(self.speed);
            let mut index = (start.saturating_duration_since(self.origin).as_secs_f64() / per_step)
                .floor() as u64
                + 1;
            loop {
                let crossing = self.origin + Duration::from_secs_f64(index as f64 * per_step);
                if crossing >= end || keyframes.len() > 4000 {
                    break;
                }
                let held = keyframes
                    .last()
                    .map_or_else(|| track(start), |(_, track)| *track);
                let next = at(crossing);
                keyframes.push(((next - f32::EPSILON.max(next * 1e-6)).max(0.0), held));
                keyframes.push((next, track(crossing)));
                index += 1;
            }
        }
        keyframes.push((1.0, track(end)));
        Some(TrackAnimation {
            node,
            start,
            duration,
            keyframes,
            generation,
        })
    }
}
