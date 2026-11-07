//! The drop shadow and edge lines of a window that draws its own frame.
//!
//! The platform paints the shadow outside the window's bounds as the
//! compositor knows them, so a compositor's own active-window border or
//! highlight wraps the window and not its shadow. Two hairlines keep windows
//! of the same colour apart where they overlap: a dark one just outside the
//! edge (part of the shadow) and, in dark mode, a light one just inside.

/// How a self-drawn window's shadow and edge lines look; the theme file's
/// `[window_shadow]` table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowShadow {
    /// How far the soft shadow reaches past the edge, in logical px; `0`
    /// draws none (the outline still shows).
    pub size: f32,
    /// How far the shadow falls below the window, in logical px.
    pub offset: f32,
    /// How dark the shadow is under the active window, `0` to `1`.
    pub opacity: f32,
    /// How dark it is under every other window.
    pub inactive_opacity: f32,
    /// The dark hairline just outside the edge, `0` to `1`.
    pub outline: f32,
    /// The light hairline just inside the edge in dark mode, `0` to `1`.
    pub highlight: f32,
}

impl Default for WindowShadow {
    /// A subtle shadow: enough to lift a window off one of the same colour
    /// without darkening what is around it.
    fn default() -> Self {
        Self {
            size: 20.0,
            offset: 2.0,
            opacity: 0.22,
            inactive_opacity: 0.11,
            outline: 0.07,
            highlight: 0.06,
        }
    }
}

/// One layer of the shadow, in CSS `box-shadow` terms: black at `alpha`, the
/// window's outline moved down by `offset_y`, grown by `spread` and blurred by
/// `blur`, in logical px.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowShadowLayer {
    /// The layer's opacity.
    pub alpha: f32,
    /// How far the layer falls below the window.
    pub offset_y: f32,
    /// How far the layer's edge fades.
    pub blur: f32,
    /// How far the outline grows before blurring.
    pub spread: f32,
}

impl WindowShadow {
    /// The values clamped to their ranges, non-finite ones at the default.
    #[must_use]
    pub fn normalized(self) -> Self {
        let fallback = Self::default();
        let clamp = |value: f32, fallback: f32, min: f32, max: f32| {
            if value.is_finite() {
                value.clamp(min, max)
            } else {
                fallback
            }
        };
        Self {
            size: clamp(self.size, fallback.size, 0.0, 64.0),
            offset: clamp(self.offset, fallback.offset, -32.0, 32.0),
            opacity: clamp(self.opacity, fallback.opacity, 0.0, 1.0),
            inactive_opacity: clamp(self.inactive_opacity, fallback.inactive_opacity, 0.0, 1.0),
            outline: clamp(self.outline, fallback.outline, 0.0, 1.0),
            highlight: clamp(self.highlight, fallback.highlight, 0.0, 1.0),
        }
    }

    /// Whether nothing is drawn outside the window.
    #[must_use]
    pub fn is_none(self) -> bool {
        let s = self.normalized();
        (s.size == 0.0 || (s.opacity == 0.0 && s.inactive_opacity == 0.0)) && s.outline == 0.0
    }

    /// The layers under the window, `active` or not, drawn first to last: a
    /// wide soft shadow, a tight one close to the edge, and the hairline.
    #[must_use]
    pub fn layers(self, active: bool) -> Vec<WindowShadowLayer> {
        let s = self.normalized();
        let (opacity, size) = if active {
            (s.opacity, s.size)
        } else {
            (s.inactive_opacity, s.size * 0.75)
        };
        let mut layers = Vec::new();
        if size > 0.0 && opacity > 0.0 {
            layers.push(WindowShadowLayer {
                alpha: opacity,
                offset_y: s.offset,
                blur: size,
                spread: 0.0,
            });
            layers.push(WindowShadowLayer {
                alpha: opacity * 0.6,
                offset_y: s.offset.clamp(0.0, 1.0),
                blur: (size / 4.0).max(2.0),
                spread: 0.0,
            });
        }
        if s.outline > 0.0 {
            layers.push(WindowShadowLayer {
                alpha: s.outline,
                offset_y: 0.0,
                blur: 0.0,
                spread: 1.0,
            });
        }
        layers
    }
}
