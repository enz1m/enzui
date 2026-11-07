// motion split production module: spring animation

//! Values that move on a physical [`Spring`], keeping their velocity when
//! they are redirected.
//!
//! [`SpringAnimation`] animates any [`SpringValue`] (a number, a point, a
//! vector, a size, a rectangle or a colour). Changing its target mid-flight
//! continues from where the value is, at the speed it is moving, so a
//! reversal turns around smoothly instead of stopping dead, and a value
//! released from a drag carries the pointer's velocity
//! ([`SpringAnimation::release`], with
//! [`VelocityTracker`](enzui_core::velocity::VelocityTracker)).
//!
//! [`SpringTravel`] is the same physics for a `0..1` travel, the shape
//! [`PanelMotion`](super::PanelMotion) and [`motion`](super::motion) drive:
//! opt in with [`PanelMotion::set_spring`](super::PanelMotion::set_spring) or
//! [`Motion::spring`](super::Motion::spring).
//!
//! Every reading is solved in closed form at the instant asked for, so the
//! result does not depend on frame timing. A settled animation reads exactly
//! its target and asks for no frames. Lane keyframes are sampled from the
//! same solution at 240 per second.

use enzui_theme::{MotionScale, Spring};
use iced::advanced::renderer::{NodeId, Track, TrackAnimation};
use iced::time::{Duration, Instant};

use super::ExpressiveSample;

/// Keyframes per second of spring travel handed to a render lane: a spring
/// still oscillating is sampled finely enough that the lane's linear
/// interpolation stays on the curve at any refresh rate.
pub const SPRING_TRACK_RATE: f32 = 240.0;

/// The most keyframes one lane animation carries.
const MAX_TRACK_SAMPLES: usize = 2400;

/// A value a [`SpringAnimation`] can move: up to four independent components,
/// each driven by the same spring.
pub trait SpringValue: Copy + PartialEq {
    /// The components, unused ones zero.
    fn components(self) -> [f32; 4];
    /// The value from its components.
    fn from_components(components: [f32; 4]) -> Self;
}

impl SpringValue for f32 {
    fn components(self) -> [f32; 4] {
        [self, 0.0, 0.0, 0.0]
    }
    fn from_components(c: [f32; 4]) -> Self {
        c[0]
    }
}

impl SpringValue for iced::Vector {
    fn components(self) -> [f32; 4] {
        [self.x, self.y, 0.0, 0.0]
    }
    fn from_components(c: [f32; 4]) -> Self {
        Self::new(c[0], c[1])
    }
}

impl SpringValue for iced::Point {
    fn components(self) -> [f32; 4] {
        [self.x, self.y, 0.0, 0.0]
    }
    fn from_components(c: [f32; 4]) -> Self {
        Self::new(c[0], c[1])
    }
}

impl SpringValue for iced::Size {
    fn components(self) -> [f32; 4] {
        [self.width, self.height, 0.0, 0.0]
    }
    fn from_components(c: [f32; 4]) -> Self {
        Self::new(c[0], c[1])
    }
}

impl SpringValue for iced::Rectangle {
    fn components(self) -> [f32; 4] {
        [self.x, self.y, self.width, self.height]
    }
    fn from_components(c: [f32; 4]) -> Self {
        Self::new(iced::Point::new(c[0], c[1]), iced::Size::new(c[2], c[3]))
    }
}

/// A colour moves in its stored (sRGB-encoded) components; a spring that
/// overshoots is clamped back into range.
impl SpringValue for iced::Color {
    fn components(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
    fn from_components(c: [f32; 4]) -> Self {
        Self::from_rgba(
            c[0].clamp(0.0, 1.0),
            c[1].clamp(0.0, 1.0),
            c[2].clamp(0.0, 1.0),
            c[3].clamp(0.0, 1.0),
        )
    }
}

impl SpringValue for [f32; 2] {
    fn components(self) -> [f32; 4] {
        [self[0], self[1], 0.0, 0.0]
    }
    fn from_components(c: [f32; 4]) -> Self {
        [c[0], c[1]]
    }
}

impl SpringValue for [f32; 4] {
    fn components(self) -> [f32; 4] {
        self
    }
    fn from_components(c: [f32; 4]) -> Self {
        c
    }
}

/// A value moving on a [`Spring`]. See the [motion module](crate::motion) documentation.
///
/// Keep one in application or widget state; read it with
/// [`Self::value`] at each frame instant and request frames while
/// [`Self::is_animating`].
#[derive(Debug, Clone, PartialEq)]
pub struct SpringAnimation<T: SpringValue> {
    spring: Spring,
    /// The spring actually run: [`Self::spring`] slowed by motion policy.
    running: Spring,
    target: T,
    /// Displacement from the target and velocity when the motion began.
    displacement: [f32; 4],
    velocity: [f32; 4],
    start: Option<Instant>,
    /// How long after `start` the motion lands.
    settle: Duration,
    precision: f32,
    reduced: bool,
    /// Land the moment the target is first reached instead of passing it.
    clamp: bool,
    generation: u64,
}

impl<T: SpringValue> SpringAnimation<T> {
    /// An animation resting at `value`, settling within 0.01 units (a
    /// hundredth of a pixel for a position).
    #[must_use]
    pub fn new(value: T, spring: Spring) -> Self {
        Self {
            spring,
            running: spring,
            target: value,
            displacement: [0.0; 4],
            velocity: [0.0; 4],
            start: None,
            settle: Duration::ZERO,
            precision: 0.01,
            reduced: false,
            clamp: false,
            generation: 0,
        }
    }

    /// How close to the target counts as arrived, in the value's units. The
    /// speed must also have fallen under ten times this per second. Smaller
    /// runs the tail longer; a non-finite or non-positive value is ignored.
    #[must_use]
    pub fn precision(mut self, precision: f32) -> Self {
        if precision.is_finite() && precision > 0.0 {
            self.precision = precision;
        }
        self
    }

    /// Stop on reaching the target instead of passing it and swinging back,
    /// from the next change on: for motion whose overshoot would look wrong,
    /// such as a panel leaving the screen, which would peek back in.
    #[must_use]
    pub fn clamped(mut self, clamp: bool) -> Self {
        self.clamp = clamp;
        self
    }

    /// [`Self::clamped`] on an animation in place.
    pub fn set_clamped(&mut self, clamp: bool) {
        self.clamp = clamp;
    }

    /// The spring, as designed (before motion policy).
    #[must_use]
    pub fn spring(&self) -> Spring {
        self.spring
    }

    /// Where the value is heading.
    #[must_use]
    pub fn target(&self) -> T {
        self.target
    }

    /// Increments whenever the motion is redirected, so a render lane can
    /// tell a new motion from a republication of the running one.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Follow the theme's motion policy: a slower `motion-normal` plays the
    /// spring proportionally slower, reduced motion lands every change at
    /// once. Takes effect from the next change.
    pub fn sync_motion(&mut self, scale: &MotionScale) {
        match scale.spring_slowdown() {
            Some(slowdown) => {
                self.reduced = false;
                self.running = self.spring.slowed(slowdown);
            }
            None => {
                self.reduced = true;
                self.running = self.spring;
            }
        }
    }

    /// Use another spring from `now` on, continuing from the current value
    /// and velocity.
    pub fn set_spring(&mut self, spring: Spring, now: Instant) {
        if spring == self.spring {
            return;
        }
        let ratio = self.running.stiffness() / self.spring.stiffness();
        self.spring = spring;
        self.running = Spring::new(spring.stiffness() * ratio, spring.damping_ratio());
        let (value, velocity) = self.state(now);
        self.begin(value, velocity, self.target, now);
    }

    /// Head for `target` from where the value is at `now`, keeping its
    /// velocity. The same target again changes nothing.
    pub fn set_target(&mut self, target: T, now: Instant) {
        if target == self.target {
            return;
        }
        let (value, velocity) = self.state(now);
        self.begin(value, velocity, target, now);
    }

    /// Start from `value` moving at `velocity` (units per second) towards
    /// `target`: the release of a drag, with the pointer's velocity.
    pub fn release(&mut self, value: T, velocity: T, target: T, now: Instant) {
        self.begin(value.components(), velocity.components(), target, now);
    }

    /// Jump to `value` and rest there.
    pub fn snap(&mut self, value: T) {
        self.target = value;
        self.displacement = [0.0; 4];
        self.velocity = [0.0; 4];
        self.start = None;
        self.settle = Duration::ZERO;
        self.generation = self.generation.wrapping_add(1);
    }

    /// Delay the running motion by `delay`, for a staggered start; `false`
    /// when the clock cannot move that far.
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

    fn begin(&mut self, value: [f32; 4], velocity: [f32; 4], target: T, now: Instant) {
        let to = target.components();
        self.target = target;
        self.generation = self.generation.wrapping_add(1);
        let finite = value.iter().chain(&velocity).all(|c| c.is_finite());
        if self.reduced || !finite {
            self.snap_keep_generation();
            return;
        }
        self.displacement = [0, 1, 2, 3].map(|i| value[i] - to[i]);
        self.velocity = velocity;
        let settle = (0..4)
            .map(|i| {
                let settle = self.running.settle_time(
                    self.displacement[i],
                    self.velocity[i],
                    self.precision,
                    self.precision * 10.0,
                );
                let crossing = if self.clamp {
                    self.running
                        .first_crossing(self.displacement[i], self.velocity[i])
                } else {
                    None
                };
                crossing.map_or(settle, |c| c.min(settle))
            })
            .fold(0.0, f32::max);
        if settle <= 0.0 {
            self.snap_keep_generation();
            return;
        }
        self.start = Some(now);
        self.settle = Duration::from_secs_f32(settle);
    }

    fn snap_keep_generation(&mut self) {
        let generation = self.generation;
        self.snap(self.target);
        self.generation = generation;
    }

    /// Seconds into the running motion at `now`, or `None` once landed.
    fn elapsed(&self, now: Instant) -> Option<f32> {
        let start = self.start?;
        let elapsed = now.saturating_duration_since(start);
        (elapsed < self.settle).then_some(elapsed.as_secs_f32())
    }

    fn state(&self, now: Instant) -> ([f32; 4], [f32; 4]) {
        let to = self.target.components();
        let Some(t) = self.elapsed(now) else {
            return (to, [0.0; 4]);
        };
        let mut value = [0.0; 4];
        let mut velocity = [0.0; 4];
        for i in 0..4 {
            let (x, v) = self
                .running
                .evaluate(self.displacement[i], self.velocity[i], t);
            value[i] = to[i] + x;
            velocity[i] = v;
        }
        (value, velocity)
    }

    /// The value at `now`; exactly the target once landed.
    #[must_use]
    pub fn value(&self, now: Instant) -> T {
        T::from_components(self.state(now).0)
    }

    /// The velocity at `now`, in units per second; zero once landed.
    #[must_use]
    pub fn velocity(&self, now: Instant) -> T {
        T::from_components(self.state(now).1)
    }

    /// Whether `now` still needs a frame: `false` once landed, so a settled
    /// animation costs nothing.
    #[must_use]
    pub fn is_animating(&self, now: Instant) -> bool {
        self.elapsed(now).is_some()
    }

    /// When the running motion lands, or `None` at rest.
    #[must_use]
    pub fn lands_at(&self) -> Option<Instant> {
        self.start.and_then(|start| start.checked_add(self.settle))
    }

    /// The rest of the motion from `now` as a lane animation for `node`,
    /// keyframes built by `track` from the same values [`Self::value`]
    /// returns. `None` at rest.
    pub fn track_animation(
        &self,
        node: NodeId,
        now: Instant,
        track: impl Fn(T) -> Track,
    ) -> Option<TrackAnimation> {
        let lands = self.lands_at()?;
        let remaining = lands.saturating_duration_since(now);
        if remaining.is_zero() {
            return None;
        }
        let samples = track_samples(remaining);
        let keyframes = (0..=samples)
            .map(|index| {
                let at = index as f32 / samples as f32;
                (at, track(self.value(now + remaining.mul_f32(at))))
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

    /// Publish the rest of the motion to the render lane; see
    /// [`Self::track_animation`]. Call on every advancing frame.
    pub fn publish<Message>(
        &self,
        shell: &mut iced::advanced::Shell<'_, Message>,
        node: NodeId,
        now: Instant,
        track: impl Fn(T) -> Track,
    ) {
        if let Some(animation) = self.track_animation(node, now, track) {
            shell.publish_track_animation(animation);
        }
    }
}

/// How many keyframe intervals a lane receives for `remaining` travel.
fn track_samples(remaining: Duration) -> usize {
    ((remaining.as_secs_f32() * SPRING_TRACK_RATE).ceil() as usize).clamp(8, MAX_TRACK_SAMPLES)
}

/// A travel between `0` and `1` on a [`Spring`], the spring counterpart of
/// [`ExpressiveTravel`](super::ExpressiveTravel).
///
/// The spatial value overshoots as the spring does; the bounded value is the
/// same instant clamped to `[0, 1]`, for opacity, clip, reservation, hit
/// testing and exits. A reversal keeps the travel's velocity.
#[derive(Debug, Clone, PartialEq)]
pub struct SpringTravel {
    animation: SpringAnimation<f32>,
    target: bool,
}

impl SpringTravel {
    /// A travel resting at `target`.
    #[must_use]
    pub fn settled(spring: Spring, target: bool) -> Self {
        Self {
            animation: SpringAnimation::new(endpoint(target), spring).precision(1e-4),
            target,
        }
    }

    /// The spring this travel follows, as designed.
    #[must_use]
    pub fn spring(&self) -> Spring {
        self.animation.spring()
    }

    /// The side the travel is heading to.
    #[must_use]
    pub fn target(&self) -> bool {
        self.target
    }

    /// Increments on every retarget.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.animation.generation()
    }

    /// How long the current travel runs from its start until it lands.
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.animation.settle
    }

    /// Head for `target` from the value and velocity at `now`. Reduced
    /// motion lands at once; the same target again changes nothing.
    pub fn set_target(&mut self, target: bool, now: Instant, scale: &MotionScale) {
        if target == self.target {
            return;
        }
        self.target = target;
        self.animation.sync_motion(scale);
        // Leaving stops at hidden: past it the panel would only peek back.
        self.animation.set_clamped(!target);
        self.animation.set_target(endpoint(target), now);
    }

    /// Head for `target` from the presented value `from` at `at`, keeping the
    /// velocity the travel had there. Always starts a travel.
    pub fn retarget_from(&mut self, from: f32, target: bool, at: Instant, scale: &MotionScale) {
        let from = if from.is_finite() {
            from
        } else {
            endpoint(target)
        };
        let velocity = self.animation.velocity(at);
        self.target = target;
        self.animation.sync_motion(scale);
        self.animation.set_clamped(!target);
        self.animation.release(from, velocity, endpoint(target), at);
    }

    /// Delay the travel by `delay`; `false` when the clock cannot move that
    /// far.
    pub fn shift(&mut self, delay: Duration) -> bool {
        self.animation.shift(delay)
    }

    /// Both channels at `now`.
    #[must_use]
    pub fn sample(&self, now: Instant) -> ExpressiveSample {
        let spatial = self.animation.value(now);
        ExpressiveSample {
            spatial,
            bounded: spatial.clamp(0.0, 1.0),
        }
    }

    /// The travel's velocity at `now`, in travels per second.
    #[must_use]
    pub fn velocity(&self, now: Instant) -> f32 {
        self.animation.velocity(now)
    }

    /// Whether `now` still needs a frame.
    #[must_use]
    pub fn is_animating(&self, now: Instant) -> bool {
        self.animation.is_animating(now)
    }

    /// Keyframe intervals a lane should receive for the travel left at
    /// `now`: [`SPRING_TRACK_RATE`] per second.
    #[must_use]
    pub fn track_samples(&self, now: Instant) -> usize {
        self.animation.lands_at().map_or(1, |lands| {
            track_samples(lands.saturating_duration_since(now))
        })
    }

    /// The rest of the travel from `now` as a lane animation for `node`;
    /// `None` once landed.
    pub fn track_animation(
        &self,
        node: NodeId,
        now: Instant,
        value: impl Fn(ExpressiveSample) -> Track,
    ) -> Option<TrackAnimation> {
        self.animation.track_animation(node, now, |spatial| {
            value(ExpressiveSample {
                spatial,
                bounded: spatial.clamp(0.0, 1.0),
            })
        })
    }

    /// Publish the rest of the travel to the render lane.
    pub fn publish<Message>(
        &self,
        shell: &mut iced::advanced::Shell<'_, Message>,
        node: NodeId,
        now: Instant,
        value: impl Fn(ExpressiveSample) -> Track,
    ) {
        if let Some(animation) = self.track_animation(node, now, value) {
            shell.publish_track_animation(animation);
        }
    }
}

fn endpoint(target: bool) -> f32 {
    if target { 1.0 } else { 0.0 }
}

#[cfg(test)]
mod tests;
