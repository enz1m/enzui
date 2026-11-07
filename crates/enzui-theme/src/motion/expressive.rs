//! Expressive motion: cubic-Bézier curves whose spatial travel may overshoot
//! its target, with every other channel kept inside `[0, 1]`.
//!
//! The six [`ExpressiveMotion`] tokens are the Material 3 expressive pairs
//! (the curves and timings Caelestia's shell ships): three *spatial* curves,
//! for where something is, which overshoot and settle, and three *effects*
//! curves, for how it looks, which never leave `[0, 1]`.
//!
//! # Channels
//!
//! An overshoot is a position past the target, which only a spatial channel
//! can show. Read a spatial token's curve through [`MotionChannel::Bounded`]
//! for opacity, clip, reservation, hit testing or an inert exit: the value is
//! clamped to `[0, 1]`, so a panel is never more than fully opaque, never
//! clips past its own bounds and never reserves more room than it owns.
//!
//! # One evaluation
//!
//! [`CubicBezier::evaluate`] is the only evaluator. A render lane advancing a
//! retained scene must present these curves through [`ExpressiveMotion::track_points`],
//! sampled from the same function, never a second implementation, so the
//! frame the UI thread draws and the frame the lane draws agree.
//!
//! # Example
//! ```
//! use enzui_theme::motion::{ExpressiveMotion, MotionChannel};
//!
//! let curve = ExpressiveMotion::FastSpatial;
//! // Past the middle of the travel, a fast spatial curve has overshot.
//! assert!(curve.sample(0.5, MotionChannel::Spatial) > 1.0);
//! // The same instant read for opacity stays bounded.
//! assert_eq!(curve.sample(0.5, MotionChannel::Bounded), 1.0);
//! // Both settle exactly on the target.
//! assert_eq!(curve.sample(1.0, MotionChannel::Spatial), 1.0);
//! ```

/// A CSS-style cubic Bézier timing curve from `(0, 0)` to `(1, 1)` through the
/// control points `(x1, y1)` and `(x2, y2)`.
///
/// The control `x`s lie in `[0, 1]`, so time is monotonic and every progress
/// has exactly one value; the `y`s are free, which is what lets a curve
/// overshoot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CubicBezier {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
}

/// Bisection steps when solving the curve's time for a progress: 2^-32 of the
/// travel, below `f32` resolution, in a fixed count so no input can spin.
const SOLVE_STEPS: u32 = 32;

/// The largest |y| a control point may take: far beyond any designed
/// overshoot, small enough that no evaluation can leave finite range.
const MAX_CONTROL_Y: f32 = 16.0;

impl CubicBezier {
    /// A curve from trusted constants. The control `x`s must lie in `[0, 1]`
    /// and every value must be finite; [`Self::checked`] admits untrusted
    /// input.
    #[must_use]
    pub const fn new(x1: f32, y1: f32, x2: f32, y2: f32) -> Self {
        Self { x1, y1, x2, y2 }
    }

    /// A curve from untrusted input (a theme file, a shell setting): `None`
    /// unless every value is finite, both control `x`s lie in `[0, 1]` and
    /// both `y`s lie within ±16.
    #[must_use]
    pub fn checked(x1: f32, y1: f32, x2: f32, y2: f32) -> Option<Self> {
        let x_ok = |x: f32| x.is_finite() && (0.0..=1.0).contains(&x);
        let y_ok = |y: f32| y.is_finite() && y.abs() <= MAX_CONTROL_Y;
        (x_ok(x1) && x_ok(x2) && y_ok(y1) && y_ok(y2)).then_some(Self { x1, y1, x2, y2 })
    }

    /// The control points, `(x1, y1, x2, y2)`.
    #[must_use]
    pub fn controls(self) -> (f32, f32, f32, f32) {
        (self.x1, self.y1, self.x2, self.y2)
    }

    /// The curve's value at progress `t`, which is clamped to `[0, 1]`.
    ///
    /// Exactly `0` at `t <= 0` and exactly `1` at `t >= 1`, so a settled
    /// travel lands on its target with no residue. Between them the value may
    /// leave `[0, 1]` when a control `y` does. A `NaN` progress reads as `0`.
    #[must_use]
    pub fn evaluate(self, t: f32) -> f32 {
        if t.is_nan() || t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        // x(s) rises monotonically from 0 to 1, so bisect for the s it reads t.
        let (mut low, mut high) = (0.0_f32, 1.0_f32);
        for _ in 0..SOLVE_STEPS {
            let mid = 0.5 * (low + high);
            if bezier(mid, self.x1, self.x2) < t {
                low = mid;
            } else {
                high = mid;
            }
        }
        bezier(0.5 * (low + high), self.y1, self.y2)
    }

    /// The highest value the curve reaches, found on a fixed grid: `1.0` for a
    /// curve that never overshoots.
    #[must_use]
    pub fn peak(self) -> f32 {
        (0..=256)
            .map(|step| self.evaluate(step as f32 / 256.0))
            .fold(1.0, f32::max)
    }
}

/// One coordinate of a cubic Bézier from 0 to 1 with inner controls `a` and
/// `b`, at curve parameter `s`.
fn bezier(s: f32, a: f32, b: f32) -> f32 {
    let r = 1.0 - s;
    3.0 * r * r * s * a + 3.0 * r * s * s * b + s * s * s
}

/// Which kind of value a curve is driving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionChannel {
    /// A position, offset or scale: the one place an overshoot is meaningful.
    Spatial,
    /// Opacity, clip, reservation, hit testing, an inert exit, or any value
    /// that must stay within `[0, 1]`.
    Bounded,
}

/// The six expressive motion tokens: a spatial and an effects curve at three
/// speeds, each with its duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExpressiveMotion {
    /// Quick spatial travel with a pronounced overshoot; 350 ms.
    FastSpatial,
    /// Ordinary spatial travel with a soft overshoot; 500 ms.
    DefaultSpatial,
    /// Long spatial travel with a soft overshoot; 650 ms.
    SlowSpatial,
    /// A quick bounded change of appearance; 150 ms.
    FastEffects,
    /// An ordinary bounded change of appearance; 200 ms.
    DefaultEffects,
    /// A slow bounded change of appearance, such as a colour; 300 ms.
    SlowEffects,
}

impl ExpressiveMotion {
    /// Every token, spatial first.
    pub const ALL: [Self; 6] = [
        Self::FastSpatial,
        Self::DefaultSpatial,
        Self::SlowSpatial,
        Self::FastEffects,
        Self::DefaultEffects,
        Self::SlowEffects,
    ];

    /// The token's curve.
    #[must_use]
    pub const fn curve(self) -> CubicBezier {
        match self {
            Self::FastSpatial => CubicBezier::new(0.42, 1.67, 0.21, 0.9),
            Self::DefaultSpatial => CubicBezier::new(0.38, 1.21, 0.22, 1.0),
            Self::SlowSpatial => CubicBezier::new(0.39, 1.29, 0.35, 0.98),
            Self::FastEffects => CubicBezier::new(0.31, 0.94, 0.34, 1.0),
            Self::DefaultEffects => CubicBezier::new(0.34, 0.8, 0.34, 1.0),
            Self::SlowEffects => CubicBezier::new(0.34, 0.88, 0.34, 1.0),
        }
    }

    /// The token's unscaled duration, in milliseconds.
    #[must_use]
    pub const fn base_ms(self) -> f32 {
        match self {
            Self::FastSpatial => 350.0,
            Self::DefaultSpatial => 500.0,
            Self::SlowSpatial => 650.0,
            Self::FastEffects => 150.0,
            Self::DefaultEffects => 200.0,
            Self::SlowEffects => 300.0,
        }
    }

    /// Whether this is a spatial token, whose curve may overshoot.
    #[must_use]
    pub const fn is_spatial(self) -> bool {
        matches!(
            self,
            Self::FastSpatial | Self::DefaultSpatial | Self::SlowSpatial
        )
    }

    /// The curve's value at progress `t` for `channel`.
    ///
    /// A spatial token read for [`MotionChannel::Spatial`] may leave `[0, 1]`;
    /// every other read is clamped into it. Exactly `0` and `1` at the ends.
    #[must_use]
    pub fn sample(self, t: f32, channel: MotionChannel) -> f32 {
        let value = self.curve().evaluate(t);
        if channel == MotionChannel::Spatial && self.is_spatial() {
            value
        } else {
            value.clamp(0.0, 1.0)
        }
    }

    /// `samples + 1` evenly spaced `(progress, value)` points over the whole
    /// travel for `channel`, the form a render lane interpolates: the lane
    /// presents exactly what [`Self::sample`] draws, from one evaluator.
    #[must_use]
    pub fn track_points(self, samples: usize, channel: MotionChannel) -> Vec<(f32, f32)> {
        let samples = samples.max(1);
        (0..=samples)
            .map(|index| {
                let at = index as f32 / samples as f32;
                (at, self.sample(at, channel))
            })
            .collect()
    }
}

impl super::MotionScale {
    /// An expressive token's duration under this policy, in milliseconds:
    /// zero under reduced motion, so the value snaps to its target, else the
    /// token's duration scaled as `motion-normal` is scaled from its 180 ms
    /// default.
    #[must_use]
    pub fn expressive_ms(&self, motion: ExpressiveMotion) -> f32 {
        if self.reduced_motion {
            return 0.0;
        }
        let ratio = self.normal_ms / super::DEFAULT_NORMAL_MS;
        super::sane_motion_ms(motion.base_ms() * if ratio.is_finite() { ratio } else { 1.0 })
    }
}

#[cfg(test)]
mod tests;
