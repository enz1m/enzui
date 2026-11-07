//! Reveal-fade timing ([`TooltipFade`]) and the tooltip colour/layout [`TooltipTokens`].

use super::*;

/// Default reveal-fade duration before adopting theme motion: 120 ms.
pub const TOOLTIP_FADE: Duration = Duration::from_millis(120);

/// Opacity reveal and hide timing sampled at explicit instants.
/// Reduced motion uses zero duration. The default duration is [`TOOLTIP_FADE`];
/// [`Self::sync_motion`] can adopt a different raw theme duration.
#[derive(Debug, Clone)]
pub struct TooltipFade {
    anim: Animation<bool>,
    reduced_motion: bool,
    // Reduced motion must not erase the configured theme duration.
    raw_duration: Duration,
}

impl Default for TooltipFade {
    fn default() -> Self {
        Self::new(false)
    }
}

impl TooltipFade {
    /// Create a hidden fade. `reduced_motion` disables interpolation.
    pub fn new(reduced_motion: bool) -> Self {
        Self::with_duration(reduced_motion, TOOLTIP_FADE)
    }

    /// Create a hidden fade with an unreduced duration; zero means instant.
    /// The raw duration is retained across reduced-motion toggles; interpolation
    /// saturates at [`enzui_theme::MAX_MOTION_MS`].
    #[must_use]
    pub fn with_duration(reduced_motion: bool, raw: Duration) -> Self {
        let duration = if reduced_motion { Duration::ZERO } else { raw };
        let mut me = Self::build(reduced_motion, duration);
        me.raw_duration = raw;
        me
    }

    /// Adopt the theme's raw fast duration at `at`, retaining the current target.
    ///
    /// Negative or non-finite milliseconds are ignored; finite values follow
    /// [`enzui_theme::sane_motion_ms`]. A duration change rebuilds from hidden,
    /// so a shown target starts another reveal when interpolation is enabled.
    /// Use the event's timestamp for `at`.
    pub fn sync_motion(&mut self, motion: &enzui_theme::MotionScale, at: Instant) {
        if !(motion.fast_ms.is_finite() && motion.fast_ms >= 0.0) {
            return;
        }
        let raw = Duration::from_secs_f32(enzui_theme::sane_motion_ms(motion.fast_ms) / 1000.0);
        if raw == self.raw_duration {
            return;
        }
        let target = self.anim.value();
        let reduced = self.reduced_motion;
        *self = Self::with_duration(reduced, raw);
        self.anim.go_mut(target, at);
    }

    /// Configured duration before reduced-motion policy is applied.
    #[must_use]
    pub const fn raw_duration(&self) -> Duration {
        self.raw_duration
    }

    fn build(reduced_motion: bool, duration: Duration) -> Self {
        Self {
            anim: crate::motion::build_anim(
                false,
                duration.as_secs_f32() * 1000.0,
                enzui_theme::MotionScale::default()
                    .easing_or(Some(enzui_theme::MotionEasing::EaseInOut)),
            ),
            reduced_motion,
            raw_duration: duration,
        }
    }

    /// Change reduced-motion mode at `at`, retaining the raw duration and target.
    /// A mode change rebuilds from hidden. Enabling reduction snaps to the target;
    /// disabling it can restart a shown reveal. Use the event's timestamp for `at`.
    pub fn set_reduced_motion(&mut self, reduced_motion: bool, at: Instant) {
        if reduced_motion != self.reduced_motion {
            let target = self.anim.value();
            *self = Self::with_duration(reduced_motion, self.raw_duration);
            self.anim.go_mut(target, at);
        }
    }

    /// Whether reduced motion is active.
    pub fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }

    /// Begin revealing (target opacity `1.0`) as of `at`.
    pub fn show(&mut self, at: Instant) {
        self.anim.go_mut(true, at);
    }

    /// Begin hiding (target opacity `0.0`) as of `at`.
    pub fn hide(&mut self, at: Instant) {
        self.anim.go_mut(false, at);
    }

    /// The current opacity at instant `at`, in `0.0..=1.0`.
    pub fn opacity(&self, at: Instant) -> f32 {
        self.anim.interpolate(0.0, 1.0, at).clamp(0.0, 1.0)
    }

    /// Whether the fade is still in flight at `at` (the widget should keep
    /// requesting redraws until this is false).
    pub fn is_animating(&self, at: Instant) -> bool {
        self.anim.is_animating(at)
    }

    /// Whether the target is shown or opacity still exceeds [`f32::EPSILON`].
    pub fn is_visible(&self, at: Instant) -> bool {
        self.anim.value() || self.opacity(at) > f32::EPSILON
    }
}

/// Pinned bubble appearance and geometry in logical pixels.
///
/// Defaults: the theme's `tooltip` corner radius (6 px), 8 px horizontal and
/// 4 px vertical padding, 12 px text before the UI text scale, a 6 px gap and
/// a 240 px bubble cap.
/// Invalid geometry falls back to compiled defaults. Zero is retained except
/// for text size, which uses the Body default. The cap bounds text plus padding
/// only when it exceeds total horizontal padding.
/// Container colours are clamped, but the text's own ink resolver receives
/// `text_color` directly; supply finite colour channels in `0.0..=1.0`.
#[derive(Debug, Clone, Copy)]
pub struct TooltipTokens {
    /// Tooltip background color.
    pub background: Color,
    /// Tooltip text color.
    pub text_color: Color,
    /// Tooltip border radius (px).
    pub radius: f32,
    /// Horizontal padding (px).
    pub padding_x: f32,
    /// Vertical padding (px).
    pub padding_y: f32,
    /// Text size in logical pixels; zero uses the Body typography default.
    pub text_size: f32,
    /// Gap between the trigger and the tooltip (px).
    pub gap: f32,
    /// Bubble width cap including padding; defaults to 240 px.
    /// When the cap cannot contain horizontal padding, text remains unconstrained.
    pub max_width: f32,
    /// Shadow blur in logical pixels; defaults to 8 and is capped at 256.
    /// Negative or non-finite values become zero.
    pub shadow_blur: f32,
    /// Signed shadow offset in logical pixels; defaults to 2 and clamps to ±256.
    /// Non-finite values become zero.
    pub shadow_offset_y: f32,
    /// Drop-shadow color. Default `rgba(0, 0, 0, 0.20)`.
    pub shadow_color: Color,
}

impl TooltipTokens {
    /// Derive tooltip tokens from a theme palette.
    ///
    /// Uses the elevated surface and primary text in both light and dark modes.
    /// Layout defaults are the values recorded on [`TooltipTokens`].
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            background: p.surface_elevated,
            text_color: p.text,
            radius: enzui_theme::text_defaults::shape_radius(|s| s.tooltip),
            padding_x: 8.0,
            padding_y: 4.0,
            text_size: enzui_theme::text_scale::ui_px(12.0),
            gap: 6.0,
            max_width: 240.0,
            shadow_blur: 8.0,
            shadow_offset_y: 2.0,
            shadow_color: Color {
                a: 0.20,
                ..p.shadow
            },
        }
    }

    /// Convenience: tokens for dark theme.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Convenience: tokens for light theme.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}
