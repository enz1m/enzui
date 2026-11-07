// motion split production module: expressive travel

//! A travel driven by an [`ExpressiveMotion`] curve, ready for a retained
//! render lane.
//!
//! [`ExpressiveTravel`] keeps two channels from one evaluation: the
//! spatial value, which a spatial token lets overshoot its target, and the
//! bounded value, clamped to `[0, 1]`, for opacity, clip, reservation, hit
//! testing and inert exits. A consumer maps the pair to a
//! `iced::advanced::renderer::Track` in one closure, used both for the
//! frame the UI thread draws ([`ExpressiveTravel::sample`]) and for the
//! keyframes the lane advances ([`ExpressiveTravel::track_animation`]), so the
//! two cannot disagree beyond the lane's interpolation between keyframes.
//!
//! The travel is opt-in. Nothing in the framework uses it by default, and the
//! theme's ordinary easing policy is unchanged.

use enzui_theme::{ExpressiveMotion, MotionChannel, MotionScale};
use iced::advanced::renderer::{NodeId, Track, TrackAnimation};
use iced::time::{Duration, Instant};

/// How many keyframes a lane receives for the rest of a travel.
///
/// Twice the panels' 24: the fast spatial curve rises steeply, and 24 points
/// over a 300 px travel left the lane's linear interpolation 1.1 px off the
/// curve near the start; 48 keep it within half a pixel.
pub const EXPRESSIVE_TRACK_SAMPLES: usize = 48;

/// One value of an expressive travel: where it is, and the bounded fraction
/// every non-spatial channel reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExpressiveSample {
    /// The spatial value, which may pass the target and come back.
    pub spatial: f32,
    /// The same instant clamped to `[0, 1]`.
    pub bounded: f32,
}

/// A travel between `0` and `1` along an [`ExpressiveMotion`] curve.
///
/// Retargeting continues from the value last presented, so a reversal
/// mid-overshoot starts where the user sees the element, not at an endpoint.
/// After its duration the travel reads exactly its target and asks for no
/// more frames.
#[derive(Debug, Clone, PartialEq)]
pub struct ExpressiveTravel {
    motion: ExpressiveMotion,
    from: f32,
    target: bool,
    /// When the travel began; `None` for a travel settled without a clock.
    start: Option<Instant>,
    duration: Duration,
    generation: u64,
}

impl ExpressiveTravel {
    /// A travel resting at `target`.
    #[must_use]
    pub fn at_rest(motion: ExpressiveMotion, target: bool, now: Instant) -> Self {
        let mut travel = Self::settled(motion, target);
        travel.start = Some(now);
        travel
    }

    /// A travel resting at `target`, with no frame instant yet.
    #[must_use]
    pub fn settled(motion: ExpressiveMotion, target: bool) -> Self {
        Self {
            motion,
            from: endpoint(target),
            target,
            start: None,
            duration: Duration::ZERO,
            generation: 0,
        }
    }

    /// How long the current travel runs, from its start.
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.duration
    }

    /// The token this travel follows.
    #[must_use]
    pub fn motion(&self) -> ExpressiveMotion {
        self.motion
    }

    /// The side the travel is heading to.
    #[must_use]
    pub fn target(&self) -> bool {
        self.target
    }

    /// Increments on every retarget, so a lane can tell a new travel from a
    /// republication of the running one.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Head for `target` from the value presented at `now`, over the token's
    /// duration under `scale`. Reduced motion lands at once.
    ///
    /// A flip to the target it is already heading for changes nothing, so a
    /// view rebuilding with the same state never restarts a travel.
    pub fn set_target(&mut self, target: bool, now: Instant, scale: &MotionScale) {
        if target == self.target {
            return;
        }
        let from = self.sample(now).spatial;
        self.retarget_from(from, target, now, scale);
    }

    /// Head for `target` from the spatial value `from` — the one a frame
    /// presented — starting at `at`. Unlike [`Self::set_target`] this always
    /// starts a travel: the caller has already decided there is one.
    pub fn retarget_from(&mut self, from: f32, target: bool, at: Instant, scale: &MotionScale) {
        self.from = if from.is_finite() {
            from
        } else {
            endpoint(target)
        };
        self.target = target;
        self.start = Some(at);
        self.duration = Duration::from_secs_f32(scale.expressive_ms(self.motion) / 1000.0);
        self.generation = self.generation.wrapping_add(1);
    }

    /// Delay the travel by `delay`, for a staggered start; `false` when the
    /// clock cannot move that far.
    pub fn shift(&mut self, delay: Duration) -> bool {
        match self.start.map(|start| start.checked_add(delay)) {
            Some(Some(start)) => {
                self.start = Some(start);
                true
            }
            Some(None) => false,
            None => true,
        }
    }

    /// Normalised time through the travel at `now`, in `[0, 1]`.
    fn time(&self, now: Instant) -> f32 {
        let Some(start) = self.start.filter(|_| !self.duration.is_zero()) else {
            return 1.0;
        };
        let elapsed = now.saturating_duration_since(start).as_secs_f32();
        (elapsed / self.duration.as_secs_f32()).clamp(0.0, 1.0)
    }

    /// Both channels at normalised time `t` of this travel.
    fn at_time(&self, t: f32) -> ExpressiveSample {
        let to = endpoint(self.target);
        let spatial = self.from + (to - self.from) * self.motion.sample(t, MotionChannel::Spatial);
        let bounded_from = self.from.clamp(0.0, 1.0);
        let bounded = (bounded_from
            + (to - bounded_from) * self.motion.sample(t, MotionChannel::Bounded))
        .clamp(0.0, 1.0);
        ExpressiveSample { spatial, bounded }
    }

    /// Both channels at `now`.
    #[must_use]
    pub fn sample(&self, now: Instant) -> ExpressiveSample {
        self.at_time(self.time(now))
    }

    /// Whether `now` still needs a frame: `false` once the travel has landed,
    /// so a settled travel costs nothing.
    #[must_use]
    pub fn is_animating(&self, now: Instant) -> bool {
        self.time(now) < 1.0
    }

    /// The rest of the travel from `now` as a lane animation for `node`, its
    /// keyframes built by `value` from the same samples [`Self::sample`]
    /// returns. `None` once settled, so a lane never holds a finished travel.
    pub fn track_animation(
        &self,
        node: NodeId,
        now: Instant,
        samples: usize,
        value: impl Fn(ExpressiveSample) -> Track,
    ) -> Option<TrackAnimation> {
        if !self.is_animating(now) {
            return None;
        }
        let samples = samples.max(1);
        let begin = self.time(now);
        let remaining = self.duration.mul_f32(1.0 - begin);
        if remaining.is_zero() {
            return None;
        }
        let keyframes = (0..=samples)
            .map(|index| {
                let at = index as f32 / samples as f32;
                (at, value(self.at_time(begin + (1.0 - begin) * at)))
            })
            .collect();
        Some(TrackAnimation {
            node,
            start: now,
            duration: remaining,
            keyframes,
            generation: self.generation,
        })
    }

    /// Publish the rest of the travel to the render lane; see
    /// [`Self::track_animation`]. Call on every advancing frame.
    pub fn publish<Message>(
        &self,
        shell: &mut iced::advanced::Shell<'_, Message>,
        node: NodeId,
        now: Instant,
        value: impl Fn(ExpressiveSample) -> Track,
    ) {
        if let Some(animation) = self.track_animation(node, now, EXPRESSIVE_TRACK_SAMPLES, value) {
            shell.publish_track_animation(animation);
        }
    }
}

fn endpoint(target: bool) -> f32 {
    if target { 1.0 } else { 0.0 }
}
