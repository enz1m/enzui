//! Theme-scaled motion durations, easing, reduced-motion policy, and elevation.
//!
//! Unscaled defaults are `Instant = 0 ms`, `Fast = 120 ms`, `Normal = 180 ms`,
//! and `Slow = 260 ms`. [`MotionScale::default`] applies the process motion
//! multiplier to nonzero durations, and [`MotionScale::ms`] returns zero for
//! every token when reduced motion is enabled.
//!
//! [`expressive`] adds overshooting spatial curves and bounded effects curves
//! for shells that opt into an expressive style; nothing uses them by default.

pub mod expressive;
pub mod spring;
pub use expressive::{CubicBezier, ExpressiveMotion, MotionChannel};
pub use spring::Spring;

/// A named motion duration. Resolve against a [`MotionScale`] so theme timing
/// and reduced-motion policy apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MotionToken {
    /// `motion-instant` — 0 ms (no animation; state snaps).
    Instant,
    /// `motion-fast`; unscaled default 120 ms.
    Fast,
    /// `motion-normal`; the default token, with unscaled default 180 ms.
    #[default]
    Normal,
    /// `motion-slow`; unscaled default 260 ms.
    Slow,
}

/// The easing-curve half of the motion policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MotionEasing {
    /// No easing — constant rate. Use only for a continuously running
    /// indicator (spinner, marquee), where acceleration reads as stutter.
    Linear,
    /// Accelerate from rest — suits an element leaving the screen.
    EaseIn,
    /// Decelerate into rest: the default, for anything the user initiated.
    ///
    /// A transition that starts immediately and settles gently reads as
    /// responsive, which is why almost every animated widget wants it.
    #[default]
    EaseOut,
    /// Accelerate then decelerate — the curve the tooltip reveal uses, where
    /// both ends of a fade should be soft.
    EaseInOut,
}

impl MotionEasing {
    /// Evaluates the easing curve after clamping finite progress to `[0, 1]`.
    /// A `NaN` input produces `NaN`.
    #[must_use]
    pub fn apply(self, t: f32) -> f32 {
        use core::f32::consts::PI;
        let t = t.clamp(0.0, 1.0);
        match self {
            MotionEasing::Linear => t,
            MotionEasing::EaseIn => 1.0 - f32::cos((t * PI) / 2.0),
            MotionEasing::EaseOut => f32::sin((t * PI) / 2.0),
            MotionEasing::EaseInOut => -(f32::cos(PI * t) - 1.0) / 2.0,
        }
    }
}

/// Environment variable for a process-wide motion-duration multiplier.
///
/// It is read once. Missing, non-Unicode, nonnumeric, non-finite, and
/// non-positive values all use `1.0`; a value that would stretch the slowest
/// default past [`MAX_MOTION_MS`] is lowered to the largest one that does not.
pub const MOTION_SCALE_ENV: &str = "ENZUI_MOTION_SCALE";

/// The unscaled `motion-normal` duration, ms; the reference other scaled
/// durations follow.
pub const DEFAULT_NORMAL_MS: f32 = 180.0;

/// Maximum motion duration in milliseconds (one hour).
///
/// Larger finite values saturate to this limit before duration conversion.
pub const MAX_MOTION_MS: f32 = 3_600_000.0;

/// Clamps milliseconds to `0.0..=`[`MAX_MOTION_MS`].
///
/// Negative and non-finite values become zero.
#[must_use]
pub fn sane_motion_ms(ms: f32) -> f32 {
    if ms.is_finite() {
        ms.clamp(0.0, MAX_MOTION_MS)
    } else {
        0.0
    }
}

/// The application-wide motion policy: the four named durations, the easing
/// curve, and the reduced-motion switch. When
/// [`reduced_motion`](Self::reduced_motion) is set, every token resolves to 0 ms,
/// so a single theme-level flag disables animation everywhere rather than each
/// widget re-checking a local field.
#[derive(Debug, Clone, Copy, PartialEq)]
#[must_use]
pub struct MotionScale {
    /// `motion-fast` duration, ms.
    pub fast_ms: f32,
    /// `motion-normal` duration, ms.
    pub normal_ms: f32,
    /// `motion-slow` duration, ms.
    pub slow_ms: f32,
    /// Theme-default easing; callers may override it through [`Self::easing_or`].
    pub easing: MotionEasing,
    /// When true, all durations resolve to 0 ms (honours the OS
    /// prefers-reduced-motion setting).
    pub reduced_motion: bool,
}

impl MotionScale {
    /// Resolve a [`MotionToken`] to milliseconds, honouring reduced-motion.
    ///
    /// `Instant` is always 0; any other token is 0 when `reduced_motion` is on,
    /// else its configured duration.
    pub fn ms(&self, token: MotionToken) -> f32 {
        if self.reduced_motion {
            return 0.0;
        }
        match token {
            MotionToken::Instant => 0.0,
            MotionToken::Fast => self.fast_ms,
            MotionToken::Normal => self.normal_ms,
            MotionToken::Slow => self.slow_ms,
        }
    }

    /// Resolve a [`MotionToken`] to seconds (convenience for animation curves
    /// that work in fractional seconds).
    pub fn secs(&self, token: MotionToken) -> f32 {
        self.ms(token) / 1000.0
    }

    /// A copy of this scale with reduced-motion forced on/off — for widgets
    /// that expose a local override on top of the theme default.
    pub fn with_reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = reduced;
        self
    }

    /// A copy of this scale with a different `motion-fast` duration, ms.
    pub fn with_fast_ms(mut self, ms: f32) -> Self {
        self.fast_ms = ms;
        self
    }

    /// A copy of this scale with a different [`MotionEasing`].
    pub fn with_easing(mut self, easing: MotionEasing) -> Self {
        self.easing = easing;
        self
    }

    /// Returns `preferred` when supplied; otherwise returns the theme easing.
    pub fn easing_or(&self, preferred: Option<MotionEasing>) -> MotionEasing {
        preferred.unwrap_or(self.easing)
    }
}

impl Default for MotionScale {
    // Unscaled durations are fast 120, normal 180, and slow 260 ms; easing is
    // `EaseOut` and reduced motion is off. A positive finite
    // `MOTION_SCALE_ENV` value scales all durations. The
    // process environment is read once and cached.
    fn default() -> Self {
        default_with_multiplier(cached_env_multiplier())
    }
}

fn default_with_multiplier(multiplier: f32) -> MotionScale {
    MotionScale {
        fast_ms: 120.0 * multiplier,
        normal_ms: DEFAULT_NORMAL_MS * multiplier,
        slow_ms: DEFAULT_SLOW_MS * multiplier,
        easing: MotionEasing::EaseOut,
        reduced_motion: false,
    }
}

/// The unscaled `motion-slow` duration, ms: the longest default.
const DEFAULT_SLOW_MS: f32 = 260.0;

fn parse_multiplier(value: Option<&std::ffi::OsStr>) -> f32 {
    value
        .and_then(std::ffi::OsStr::to_str)
        .and_then(|value| value.parse::<f32>().ok())
        .filter(|value| value.is_finite() && *value > 0.0)
        .map_or(1.0, |value| value.min(MAX_MOTION_MS / DEFAULT_SLOW_MS))
}

fn cached_multiplier_with(
    cache: &std::sync::OnceLock<f32>,
    read: impl FnOnce() -> Option<std::ffi::OsString>,
) -> f32 {
    *cache.get_or_init(|| parse_multiplier(read().as_deref()))
}

fn cached_env_multiplier() -> f32 {
    static MULTIPLIER: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    cached_multiplier_with(&MULTIPLIER, || std::env::var_os(MOTION_SCALE_ENV))
}

/// One elevation tier's shadow geometry: drop-shadow blur radius and vertical
/// offset, both in pixels. The colour lives on the palette as
/// [`crate::Palette::shadow`], resolved at draw time, so retinting a shadow and
/// re-shaping it are independent changes.
#[derive(Debug, Clone, Copy, PartialEq)]
#[must_use]
pub struct ElevationTier {
    /// Drop-shadow blur radius, px.
    ///
    /// Finite and non-negative: a shadow cannot have negative blur. NaN / ±inf
    /// / negative degrade to `0.0` (no blur) through [`ElevationTier::new`].
    pub blur: f32,
    /// Drop-shadow vertical offset, px.
    ///
    /// Finite, any sign: a negative offset is an upward shadow, which is a
    /// legitimate design choice. NaN / ±inf degrade to `0.0` through
    /// [`ElevationTier::new`].
    pub y_offset: f32,
}

impl ElevationTier {
    /// Flat (no shadow): blur 0, offset 0.
    pub const FLUSH: Self = Self {
        blur: 0.0,
        y_offset: 0.0,
    };

    /// Construct a tier with validated fields.
    pub fn new(blur: f32, y_offset: f32) -> Self {
        Self {
            blur: sane_blur(blur),
            y_offset: sane_offset(y_offset),
        }
    }

    /// Interpolates between tiers after mapping non-finite `t` to zero and
    /// clamping finite `t` to `0..=1`; the result is normalized through
    /// [`ElevationTier::new`].
    pub fn lerp(self, to: ElevationTier, t: f32) -> ElevationTier {
        let t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            0.0
        };
        ElevationTier::new(
            self.blur + (to.blur - self.blur) * t,
            self.y_offset + (to.y_offset - self.y_offset) * t,
        )
    }
}

fn sane_blur(v: f32) -> f32 {
    if v.is_finite() && v >= 0.0 { v } else { 0.0 }
}

fn sane_offset(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

/// The six-level elevation scale (`elevation-0` … `elevation-5`), the semantic
/// depth ladder every raised surface resolves against. Level 0 is flat.
#[derive(Debug, Clone, Copy, PartialEq)]
#[must_use]
pub struct ElevationScale {
    tiers: [ElevationTier; 6],
    /// The level filled buttons are raised to; 0 draws them flat, edged by a
    /// border instead.
    button: u8,
}

impl ElevationScale {
    /// Builds the six-tier scale after normalizing every tier through
    /// [`ElevationTier::new`], so non-finite fields become zero.
    pub fn from_tiers(tiers: [ElevationTier; 6]) -> Self {
        Self {
            tiers: tiers.map(|t| ElevationTier::new(t.blur, t.y_offset)),
            button: DEFAULT_BUTTON_ELEVATION,
        }
    }

    /// The level filled buttons are raised to (0 to 5). At 0 a button is
    /// flat and its edge is drawn as a border; above it the button is
    /// borderless and its shadow sets it apart. Defaults to 1.
    pub fn button(&self) -> u8 {
        self.button
    }

    /// This scale with filled buttons raised to `level` (clamped to 5).
    pub fn with_button(mut self, level: u8) -> Self {
        self.button = level.min(5);
        self
    }

    /// Replace a single tier, returning the modified scale. Levels above 5
    /// clamp to 5, matching [`ElevationScale::tier`]'s read behaviour.
    ///
    /// The tier is validated through [`ElevationTier::new`].
    pub fn with_tier(mut self, level: u8, tier: ElevationTier) -> Self {
        self.tiers[(level as usize).min(5)] = ElevationTier::new(tier.blur, tier.y_offset);
        self
    }

    /// The shadow geometry for a semantic level. Levels above 5 clamp to 5.
    pub fn tier(&self, level: u8) -> ElevationTier {
        self.tiers[(level as usize).min(5)]
    }

    /// The number of levels (always 6).
    pub fn levels(&self) -> usize {
        self.tiers.len()
    }
}

impl Default for ElevationScale {
    fn default() -> Self {
        Self {
            tiers: [
                ElevationTier {
                    blur: 0.0,
                    y_offset: 0.0,
                },
                ElevationTier {
                    blur: 4.0,
                    y_offset: 1.0,
                },
                ElevationTier {
                    blur: 8.0,
                    y_offset: 2.0,
                },
                ElevationTier {
                    blur: 16.0,
                    y_offset: 4.0,
                },
                ElevationTier {
                    blur: 24.0,
                    y_offset: 8.0,
                },
                ElevationTier {
                    blur: 32.0,
                    y_offset: 12.0,
                },
            ],
            button: DEFAULT_BUTTON_ELEVATION,
        }
    }
}

/// The level filled buttons are raised to by default: none, so buttons are
/// flat and set apart by their fill. A theme raises them with
/// `elevation.button`; level 1 is a faint contact shadow.
pub const DEFAULT_BUTTON_ELEVATION: u8 = 0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_extreme_multiplier_keeps_every_duration_finite_and_bounded() {
        let multiplier = parse_multiplier(Some(std::ffi::OsStr::new("3e38")));
        let scale = default_with_multiplier(multiplier);
        for token in [MotionToken::Fast, MotionToken::Normal, MotionToken::Slow] {
            let ms = scale.ms(token);
            assert!(ms.is_finite() && ms <= MAX_MOTION_MS, "{token:?}: {ms}");
        }
        assert_eq!(scale.slow_ms, MAX_MOTION_MS);
        // The expressive channel follows the same admitted scale.
        let ratio = scale.normal_ms / DEFAULT_NORMAL_MS;
        assert!((ratio - multiplier).abs() <= multiplier * 1e-6);
        // Ordinary multipliers pass through untouched.
        assert_eq!(parse_multiplier(Some(std::ffi::OsStr::new("2"))), 2.0);
        assert_eq!(parse_multiplier(Some(std::ffi::OsStr::new("-1"))), 1.0);
    }
}
