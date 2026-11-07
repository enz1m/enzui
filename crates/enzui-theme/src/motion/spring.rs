//! Physical springs: motion that keeps its velocity when it is redirected.
//!
//! A [`Spring`] is a damped harmonic oscillator of unit mass, described by
//! its stiffness and damping ratio, the parameters Material 3 and Android
//! use. [`Spring::from_duration_bounce`] builds one from the perceptual
//! duration and bounce SwiftUI uses instead; both describe the same physics.
//!
//! The motion is solved in closed form, never stepped: [`Spring::evaluate`]
//! gives the exact displacement and velocity at any time, so the result does
//! not depend on the frame rate, a late frame lands where an on-time one
//! would have, and a render lane sampling keyframes presents the same curve
//! the UI thread draws.
//!
//! A spring has no fixed duration. [`Spring::settle_time`] finds the instant
//! after which it stays within a tolerance of its target, from the decay
//! envelope, so an animation can land exactly on its target then and ask for
//! no further frames.
//!
//! # Tokens
//!
//! [`ExpressiveMotion::spring`] and [`ExpressiveMotion::standard_spring`] are
//! the Material 3 motion-scheme springs: spatial springs underdamped (they
//! overshoot and settle), effects springs critically damped (they never
//! overshoot from rest).
//!
//! # Example
//! ```
//! use enzui_theme::motion::{ExpressiveMotion, Spring};
//!
//! let spring = ExpressiveMotion::FastSpatial.spring();
//! // 300 px from the target, at rest.
//! let (x, v) = spring.evaluate(300.0, 0.0, 0.1);
//! assert!(x < 300.0 && v < 0.0, "pulled towards the target");
//! let settle = spring.settle_time(300.0, 0.0, 0.01, 0.1);
//! let (x, _) = spring.evaluate(300.0, 0.0, settle);
//! assert!(x.abs() <= 0.01);
//! ```

use super::ExpressiveMotion;

/// The longest a spring is allowed to take to settle, in seconds: far beyond
/// any designed motion, a bound for a nearly undamped one.
pub const MAX_SETTLE_SECS: f32 = 20.0;

/// Stiffness bounds admitted by [`Spring::checked`].
const STIFFNESS_RANGE: std::ops::RangeInclusive<f32> = 0.5..=100_000.0;

/// Damping-ratio bounds admitted by [`Spring::checked`]. Below 0.05 a spring
/// rings for many seconds; above 20 it barely moves.
const DAMPING_RANGE: std::ops::RangeInclusive<f32> = 0.05..=20.0;

/// Ratios this close to 1 use the critically damped solution, which the
/// other two forms approach but divide by zero at.
const CRITICAL_BAND: f64 = 1e-4;

/// A damped spring of unit mass. See the [module](self) documentation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    stiffness: f32,
    damping_ratio: f32,
}

impl Spring {
    /// SwiftUI's `.smooth`: half a second, no bounce.
    pub const SMOOTH: Self = Self::from_parts(157.913_67, 1.0);
    /// SwiftUI's `.snappy`: half a second with a little bounce.
    pub const SNAPPY: Self = Self::from_parts(157.913_67, 0.85);
    /// SwiftUI's `.bouncy`: half a second with a visible bounce.
    pub const BOUNCY: Self = Self::from_parts(157.913_67, 0.7);

    const fn from_parts(stiffness: f32, damping_ratio: f32) -> Self {
        Self {
            stiffness,
            damping_ratio,
        }
    }

    /// A spring from trusted constants: `stiffness` in N/m for a unit mass
    /// and `damping_ratio` (`1.0` critically damped, below it bouncy).
    /// [`Self::checked`] admits untrusted input.
    #[must_use]
    pub const fn new(stiffness: f32, damping_ratio: f32) -> Self {
        Self::from_parts(stiffness, damping_ratio)
    }

    /// A spring from untrusted input: `None` unless both values are finite,
    /// the stiffness lies in `0.5..=100000` and the damping ratio in
    /// `0.05..=20`.
    #[must_use]
    pub fn checked(stiffness: f32, damping_ratio: f32) -> Option<Self> {
        (STIFFNESS_RANGE.contains(&stiffness) && DAMPING_RANGE.contains(&damping_ratio))
            .then_some(Self::from_parts(stiffness, damping_ratio))
    }

    /// A spring from SwiftUI's perceptual parameters: `duration` is the
    /// period of the undamped oscillation in seconds (how long the motion
    /// feels), and `bounce` runs from `0` (no overshoot) towards `1` (endless
    /// oscillation); a negative bounce is overdamped. `None` outside what
    /// [`Self::checked`] admits.
    #[must_use]
    pub fn from_duration_bounce(duration: f32, bounce: f32) -> Option<Self> {
        if !duration.is_finite() || duration <= 0.0 || !bounce.is_finite() || bounce <= -1.0 {
            return None;
        }
        let omega = std::f32::consts::TAU / duration;
        let damping_ratio = if bounce >= 0.0 {
            1.0 - bounce
        } else {
            1.0 / (1.0 + bounce)
        };
        Self::checked(omega * omega, damping_ratio)
    }

    /// The stiffness, for a unit mass.
    #[must_use]
    pub const fn stiffness(self) -> f32 {
        self.stiffness
    }

    /// The damping ratio: `1.0` is critically damped, less overshoots.
    #[must_use]
    pub const fn damping_ratio(self) -> f32 {
        self.damping_ratio
    }

    /// The perceptual duration (SwiftUI's), in seconds: the period of the
    /// undamped oscillation.
    #[must_use]
    pub fn duration(self) -> f32 {
        std::f32::consts::TAU / self.stiffness.sqrt()
    }

    /// The perceptual bounce (SwiftUI's): `0` for critical damping, positive
    /// for a spring that overshoots, negative for an overdamped one.
    #[must_use]
    pub fn bounce(self) -> f32 {
        if self.damping_ratio <= 1.0 {
            1.0 - self.damping_ratio
        } else {
            1.0 / self.damping_ratio - 1.0
        }
    }

    /// Whether released from rest this spring passes its target.
    #[must_use]
    pub fn overshoots(self) -> bool {
        f64::from(self.damping_ratio) < 1.0 - CRITICAL_BAND
    }

    /// The same spring played `factor` times slower (`2.0` takes twice as
    /// long, with the same shape). A non-finite or non-positive factor
    /// returns the spring unchanged.
    #[must_use]
    pub fn slowed(self, factor: f32) -> Self {
        if !factor.is_finite() || factor <= 0.0 {
            return self;
        }
        Self::from_parts(self.stiffness / (factor * factor), self.damping_ratio)
    }

    /// Displacement from the target and velocity `t` seconds after leaving
    /// `displacement` with `velocity`, both in the caller's units (and units
    /// per second). Exact at any `t`; `t <= 0` returns the start.
    #[must_use]
    pub fn evaluate(self, displacement: f32, velocity: f32, t: f32) -> (f32, f32) {
        // `NaN` reads as the start too.
        if t.is_nan() || t <= 0.0 {
            return (displacement, velocity);
        }
        let (x, v) = self
            .solution(f64::from(displacement), f64::from(velocity))
            .at(f64::from(t));
        (x as f32, v as f32)
    }

    /// How long a motion leaving `displacement` with `velocity` takes before
    /// it stays within `position_epsilon` of its target with speed under
    /// `velocity_epsilon`, in seconds, capped at [`MAX_SETTLE_SECS`].
    ///
    /// Found from the decay envelope rather than the first crossing, so the
    /// motion never leaves the tolerance again afterwards; it may report a
    /// few milliseconds later than the last visible movement, never earlier.
    #[must_use]
    pub fn settle_time(
        self,
        displacement: f32,
        velocity: f32,
        position_epsilon: f32,
        velocity_epsilon: f32,
    ) -> f32 {
        let epsilon_x = f64::from(position_epsilon.abs().max(f32::MIN_POSITIVE));
        let epsilon_v = f64::from(velocity_epsilon.abs().max(f32::MIN_POSITIVE));
        let solution = self.solution(f64::from(displacement), f64::from(velocity));
        let excess = |t: f64| {
            let (x, v) = solution.envelope(t);
            (x / epsilon_x).max(v / epsilon_v)
        };
        let cap = f64::from(MAX_SETTLE_SECS);
        if excess(0.0) <= 1.0 {
            return 0.0;
        }
        if let Some(closed) = solution.closed_settle(epsilon_x, epsilon_v) {
            return closed.clamp(0.0, cap) as f32;
        }
        // The remaining envelopes fall monotonically once past their peak:
        // bisect between a point inside the tolerance and the last outside.
        let mut high = solution.peak_time().max(1e-3);
        while excess(high) > 1.0 {
            high *= 2.0;
            if high >= cap {
                return MAX_SETTLE_SECS;
            }
        }
        let mut low = solution.peak_time().min(high);
        for _ in 0..60 {
            let mid = 0.5 * (low + high);
            if excess(mid) > 1.0 {
                low = mid;
            } else {
                high = mid;
            }
        }
        high as f32
    }

    /// The first time a motion leaving `displacement` with `velocity`
    /// reaches its target, or `None` when it only approaches it (or starts
    /// on it). For a motion that must stop at the target rather than pass
    /// it, such as a panel leaving the screen.
    #[must_use]
    pub fn first_crossing(self, displacement: f32, velocity: f32) -> Option<f32> {
        if displacement == 0.0 || !displacement.is_finite() || !velocity.is_finite() {
            return None;
        }
        let solution = self.solution(f64::from(displacement), f64::from(velocity));
        let side = f64::from(displacement).signum();
        let horizon = f64::from(self.settle_time(displacement, velocity, 1e-6, 1e-6)).max(1e-3);
        // Fine enough to see a crossing between two samples: a twentieth of
        // the oscillation period.
        let omega = f64::from(self.stiffness.max(f32::MIN_POSITIVE)).sqrt();
        let step = (std::f64::consts::TAU / omega / 20.0)
            .min(horizon / 8.0)
            .max(1e-5);
        let mut low = 0.0;
        let mut t = step;
        while t <= horizon + step {
            if solution.at(t).0 * side <= 0.0 {
                let mut high = t;
                for _ in 0..50 {
                    let mid = 0.5 * (low + high);
                    if solution.at(mid).0 * side <= 0.0 {
                        high = mid;
                    } else {
                        low = mid;
                    }
                }
                return Some(high as f32);
            }
            low = t;
            t += step;
        }
        None
    }

    fn solution(self, x0: f64, v0: f64) -> Solution {
        let omega = f64::from(self.stiffness.max(f32::MIN_POSITIVE)).sqrt();
        let zeta = f64::from(self.damping_ratio.max(0.0));
        if (zeta - 1.0).abs() <= CRITICAL_BAND {
            Solution::Critical {
                omega,
                a: x0,
                b: v0 + omega * x0,
            }
        } else if zeta < 1.0 {
            let omega_d = omega * (1.0 - zeta * zeta).sqrt();
            Solution::Under {
                decay: zeta * omega,
                omega_d,
                a: x0,
                b: (v0 + zeta * omega * x0) / omega_d,
            }
        } else {
            let root = (zeta * zeta - 1.0).sqrt();
            let r1 = -omega * (zeta - root);
            let r2 = -omega * (zeta + root);
            let c1 = (v0 - r2 * x0) / (r1 - r2);
            Solution::Over {
                r1,
                r2,
                c1,
                c2: x0 - c1,
            }
        }
    }
}

/// The closed-form motion for one start.
#[derive(Debug, Clone, Copy)]
enum Solution {
    /// `e^(-decay t) (a cos(ωd t) + b sin(ωd t))`.
    Under {
        decay: f64,
        omega_d: f64,
        a: f64,
        b: f64,
    },
    /// `(a + b t) e^(-ω t)`.
    Critical { omega: f64, a: f64, b: f64 },
    /// `c1 e^(r1 t) + c2 e^(r2 t)`.
    Over { r1: f64, r2: f64, c1: f64, c2: f64 },
}

impl Solution {
    fn at(self, t: f64) -> (f64, f64) {
        match self {
            Self::Under {
                decay,
                omega_d,
                a,
                b,
            } => {
                let envelope = (-decay * t).exp();
                let (sin, cos) = (omega_d * t).sin_cos();
                let x = envelope * (a * cos + b * sin);
                let v =
                    envelope * ((b * omega_d - decay * a) * cos - (a * omega_d + decay * b) * sin);
                (x, v)
            }
            Self::Critical { omega, a, b } => {
                let envelope = (-omega * t).exp();
                let x = (a + b * t) * envelope;
                let v = (b - omega * (a + b * t)) * envelope;
                (x, v)
            }
            Self::Over { r1, r2, c1, c2 } => {
                let (e1, e2) = ((r1 * t).exp(), (r2 * t).exp());
                (c1 * e1 + c2 * e2, c1 * r1 * e1 + c2 * r2 * e2)
            }
        }
    }

    /// Upper bounds on `|x|` and `|v|` from `t` on.
    fn envelope(self, t: f64) -> (f64, f64) {
        match self {
            Self::Under {
                decay,
                omega_d,
                a,
                b,
            } => {
                let amplitude = a.hypot(b) * (-decay * t).exp();
                (amplitude, amplitude * omega_d.hypot(decay))
            }
            Self::Critical { omega, a, b } => {
                let envelope = (-omega * t).exp();
                let x = (a.abs() + b.abs() * t) * envelope;
                (x, (b.abs() + omega * x / envelope) * envelope)
            }
            Self::Over { r1, r2, c1, c2 } => {
                let (e1, e2) = ((r1 * t).exp(), (r2 * t).exp());
                (
                    c1.abs() * e1 + c2.abs() * e2,
                    (c1 * r1).abs() * e1 + (c2 * r2).abs() * e2,
                )
            }
        }
    }

    /// The settle time solved directly, where the envelope is a single
    /// exponential.
    fn closed_settle(self, epsilon_x: f64, epsilon_v: f64) -> Option<f64> {
        let Self::Under {
            decay,
            omega_d,
            a,
            b,
        } = self
        else {
            return None;
        };
        let amplitude = a.hypot(b);
        let speed = amplitude * omega_d.hypot(decay);
        let t = ((amplitude / epsilon_x).ln().max((speed / epsilon_v).ln())) / decay;
        Some(t.max(0.0))
    }

    /// From when the envelope only falls.
    fn peak_time(self) -> f64 {
        match self {
            // The position bound (|a| + |b| t) e^(-ωt) peaks here; the
            // velocity bound only falls.
            Self::Critical { omega, a, b } if b.abs() > 0.0 => {
                (1.0 / omega - a.abs() / b.abs()).max(0.0)
            }
            _ => 0.0,
        }
    }
}

impl ExpressiveMotion {
    /// The Material 3 expressive motion-scheme spring for this token:
    /// spatial springs overshoot and settle (damping 0.6 fast, 0.8 default
    /// and slow), effects springs are critically damped.
    #[must_use]
    pub const fn spring(self) -> Spring {
        match self {
            Self::FastSpatial => Spring::from_parts(800.0, 0.6),
            Self::DefaultSpatial => Spring::from_parts(380.0, 0.8),
            Self::SlowSpatial => Spring::from_parts(200.0, 0.8),
            Self::FastEffects => Spring::from_parts(3800.0, 1.0),
            Self::DefaultEffects => Spring::from_parts(1600.0, 1.0),
            Self::SlowEffects => Spring::from_parts(800.0, 1.0),
        }
    }

    /// The Material 3 standard motion-scheme spring for this token's speed:
    /// spatial springs barely overshoot (damping 0.9), effects springs are
    /// critically damped.
    #[must_use]
    pub const fn standard_spring(self) -> Spring {
        match self {
            Self::FastSpatial => Spring::from_parts(1400.0, 0.9),
            Self::DefaultSpatial => Spring::from_parts(700.0, 0.9),
            Self::SlowSpatial => Spring::from_parts(300.0, 0.9),
            Self::FastEffects => Spring::from_parts(3800.0, 1.0),
            Self::DefaultEffects => Spring::from_parts(1600.0, 1.0),
            Self::SlowEffects => Spring::from_parts(800.0, 1.0),
        }
    }
}

impl super::MotionScale {
    /// How much slower than its design a spring plays under this policy:
    /// `None` under reduced motion, where a spring lands at once, else the
    /// theme's `motion-normal` speed relative to its 180 ms default, the same
    /// factor [`Self::expressive_ms`](super::MotionScale::expressive_ms)
    /// applies to curves.
    #[must_use]
    pub fn spring_slowdown(&self) -> Option<f32> {
        if self.reduced_motion {
            return None;
        }
        let ratio = self.normal_ms / super::DEFAULT_NORMAL_MS;
        if ratio.is_finite() && ratio > 0.0 {
            Some(ratio.clamp(0.1, super::MAX_MOTION_MS / super::DEFAULT_NORMAL_MS))
        } else {
            Some(1.0)
        }
    }
}

#[cfg(test)]
mod tests;
