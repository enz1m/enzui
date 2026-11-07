//! Host-owned timing and directional page translation.

use super::*;

/// Return the entering-page horizontal offset in logical pixels.
/// Forward enters from the right, backward from the left, and `None` returns zero.
/// Finite progress is clamped to `0..=1`; distance is not sanitized. Callers must
/// supply finite values and a nonnegative distance for that sign convention.
#[must_use]
pub fn transition_offset(transition: Transition, progress: f32, distance: f32) -> f32 {
    let direction = match transition {
        Transition::Forward => 1.0,
        Transition::Backward => -1.0,
        Transition::None => return 0.0,
    };
    (1.0 - progress.clamp(0.0, 1.0)) * direction * distance
}

/// Host-owned transition timing sampled at explicit timestamps.
/// Begin with the direction from [`RoutePath::transition_to`], synchronize theme
/// timing and reduction, and rebuild the outlet on animation ticks.
#[derive(Debug, Clone)]
pub struct NavigationTransition {
    direction: Transition,
    started: Option<Instant>,
    raw_duration: Duration,
    easing: enzui_theme::MotionEasing,
    reduced_motion: bool,
}

impl Default for NavigationTransition {
    fn default() -> Self {
        Self::new(false)
    }
}

impl NavigationTransition {
    /// Create a settled transition with 180 ms timing and the default EaseOut curve.
    /// `reduced_motion` makes subsequent transitions settle immediately.
    #[must_use]
    pub fn new(reduced_motion: bool) -> Self {
        Self {
            direction: Transition::None,
            started: None,
            raw_duration: TRANSITION_DURATION,
            easing: enzui_theme::MotionScale::default().easing,
            reduced_motion,
        }
    }

    /// Adopt raw `normal_ms` timing and the easing curve from the host motion scale.
    /// Zero means instant. Negative or non-finite timing retains the previous value;
    /// positive timing is capped at one hour. Easing is adopted even when timing is
    /// invalid. This does not adopt the scale's reduced-motion flag; use
    /// [`Self::set_reduced_motion`] separately.
    pub fn sync_motion(&mut self, motion: &enzui_theme::MotionScale) {
        if motion.normal_ms.is_finite() && motion.normal_ms >= 0.0 {
            let ms = enzui_theme::sane_motion_ms(motion.normal_ms);
            self.raw_duration = Duration::from_secs_f32(ms / 1000.0);
        }
        self.easing = motion.easing;
    }

    /// Return the configured duration, or zero while reduced motion is enabled.
    #[must_use]
    pub fn duration(&self) -> Duration {
        if self.reduced_motion {
            Duration::ZERO
        } else {
            self.raw_duration
        }
    }

    /// Return configured timing before reduced-motion suppression.
    #[must_use]
    pub const fn raw_duration(&self) -> Duration {
        self.raw_duration
    }

    /// Whether this instance suppresses transitions.
    #[must_use]
    pub const fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }

    /// Enable or disable transition suppression.
    /// Enabling settles an in-flight transition. Disabling preserves configured
    /// timing but does not restart the settled transition.
    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        if reduced_motion == self.reduced_motion {
            return;
        }
        self.reduced_motion = reduced_motion;
        if reduced_motion {
            self.started = None;
            self.direction = Transition::None;
        }
    }

    /// Start the supplied direction at `at`; `None` or reduced motion settles immediately.
    pub fn begin(&mut self, direction: Transition, at: Instant) {
        self.direction = direction;
        self.started = if direction == Transition::None || self.reduced_motion {
            None
        } else {
            Some(at)
        };
    }

    /// Return the last supplied direction; it can remain set after animation settles.
    /// Enabling reduced motion resets it to `None`.
    #[must_use]
    pub const fn direction(&self) -> Transition {
        self.direction
    }

    /// Return eased progress in `0..=1`, or one when settled.
    /// Timestamps before the start use zero elapsed time.
    #[must_use]
    pub fn progress(&self, at: Instant) -> f32 {
        let Some(started) = self.started else {
            return 1.0;
        };
        let duration = self.duration();
        if duration.is_zero() {
            return 1.0;
        }
        let elapsed = at.saturating_duration_since(started).as_secs_f32();
        let linear = (elapsed / duration.as_secs_f32()).clamp(0.0, 1.0);
        self.easing.apply(linear)
    }

    /// Whether progress is incomplete at `at`.
    /// While true, the host must rebuild the outlet with later timestamps to advance
    /// the sampled offset; requesting redraw alone does not advance it.
    #[must_use]
    pub fn is_animating(&self, at: Instant) -> bool {
        self.started.is_some() && self.progress(at) < 1.0
    }

    /// Return entering-page translation at `at`, following [`transition_offset`].
    #[must_use]
    pub fn offset(&self, at: Instant, distance: f32) -> f32 {
        transition_offset(self.direction, self.progress(at), distance)
    }
}
