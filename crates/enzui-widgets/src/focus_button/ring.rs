//! `focus_button`: ring.

use super::*;

/// Default focus-visible ring width in logical pixels: 2, the stock theme's
/// `stroke.focus_ring`. A theme without a stroke scale draws this.
pub const DEFAULT_FOCUS_RING_WIDTH: f32 = 2.0;

/// The published theme's focus ring width for dense cells (calendar days,
/// dropdown triggers): three quarters of `stroke.focus_ring`.
pub(crate) fn dense_focus_ring_width() -> f32 {
    0.75 * enzui_theme::text_defaults::focus_ring_width()
}

// Preferred-ring contrast threshold; the fallback does not guarantee this ratio.
const MIN_RING_CONTRAST: f32 = 3.0;

/// Theme policy for default focus rings, neutral fills and disabled foregrounds.
/// The `iced::Theme` implementation composites fill over the palette page
/// background. It prefers the declared ring at contrast ratio 3 or above;
/// otherwise it chooses whichever of palette text/background contrasts better.
/// The fallback does not guarantee a minimum ratio or inspect ancestor paint.
pub trait FocusRingTheme {
    /// Resolve ring ink over a solid fill, or the page background when absent.
    /// A gradient is represented by `None`; the theme cannot inspect its colours.
    fn enz_focus_ring_color(&self, fill: Option<Color>) -> Color;

    /// The focus-visible ring width. Defaults to [`DEFAULT_FOCUS_RING_WIDTH`].
    fn enz_focus_ring_width(&self) -> f32 {
        DEFAULT_FOCUS_RING_WIDTH
    }

    /// Resolve disabled ink over an opaque backdrop.
    /// The default preserves `preferred`; `iced::Theme` applies the palette contrast band.
    fn enz_disabled_on(&self, preferred: Color, _backdrop: Color) -> Color {
        preferred
    }

    /// Whether this theme asks for reduced motion; state fades stand still
    /// when it does. Defaults to `false`.
    fn enz_reduced_motion(&self) -> bool {
        false
    }

    /// How long a motion of `token` takes, in milliseconds, and its curve,
    /// before reduced motion. Defaults to the stock motion scale easing out.
    fn enz_motion(&self, token: enzui_theme::MotionToken) -> (f32, enzui_theme::MotionEasing) {
        let scale = enzui_theme::MotionScale {
            reduced_motion: false,
            ..enzui_theme::MotionScale::default()
        };
        (scale.ms(token), scale.easing)
    }

    /// Keeps a copy of this theme in `slot` so the button can resolve its
    /// style when a frame arrives, before it draws; returns whether the copy
    /// changed. The default keeps nothing, and the button's state changes
    /// land at once.
    fn enz_latch(&self, _slot: &mut Option<Box<dyn std::any::Any>>) -> bool {
        false
    }

    /// The theme [`Self::enz_latch`] kept, if any.
    fn enz_latched(_slot: &Option<Box<dyn std::any::Any>>) -> Option<&Self>
    where
        Self: Sized,
    {
        None
    }

    /// Resolve the fill and text of an unstyled button for the interaction state.
    /// `iced::Theme` uses transparent resting/focused fill, live-theme hover/press
    /// washes and disabled ink. Ring paint is resolved separately.
    fn enz_neutral_button_style(&self, status: FocusStatus) -> Style;
}

impl FocusRingTheme for iced::Theme {
    fn enz_focus_ring_width(&self) -> f32 {
        crate::theme_lookup::focus_ring_width(self)
    }

    fn enz_reduced_motion(&self) -> bool {
        crate::theme_lookup::reduced_motion(self)
    }

    fn enz_motion(&self, token: enzui_theme::MotionToken) -> (f32, enzui_theme::MotionEasing) {
        let motion = crate::theme_lookup::motion(self);
        let motion = enzui_theme::MotionScale {
            reduced_motion: false,
            ..motion
        };
        (motion.ms(token), motion.easing)
    }

    fn enz_latch(&self, slot: &mut Option<Box<dyn std::any::Any>>) -> bool {
        let kept = slot
            .as_ref()
            .and_then(|kept| kept.downcast_ref::<iced::Theme>());
        let same = kept.is_some_and(|kept| match (kept, self) {
            (iced::Theme::Custom(a), iced::Theme::Custom(b)) if std::sync::Arc::ptr_eq(a, b) => {
                true
            }
            (kept, theme) => kept == theme,
        });
        if !same {
            *slot = Some(Box::new(self.clone()));
        }
        !same
    }

    fn enz_latched(slot: &Option<Box<dyn std::any::Any>>) -> Option<&Self> {
        slot.as_ref()?.downcast_ref::<iced::Theme>()
    }

    fn enz_focus_ring_color(&self, fill: Option<Color>) -> Color {
        use crate::accessibility_metrics::{composite_over, contrast_ratio};
        let p = crate::theme_lookup::palette(self);
        // No ancestor backdrop reaches this API; use the palette page for alpha composition.
        let painted = match fill {
            Some(c) => composite_over(c, p.background),
            None => p.background,
        };
        if contrast_ratio(p.focus_ring, painted) >= MIN_RING_CONTRAST {
            return p.focus_ring;
        }
        let text = contrast_ratio(p.text, painted);
        let bg = contrast_ratio(p.background, painted);
        if text >= bg { p.text } else { p.background }
    }

    fn enz_disabled_on(&self, preferred: Color, backdrop: Color) -> Color {
        crate::theme_lookup::palette(self).disabled_on_with_preferred(preferred, backdrop)
    }

    fn enz_neutral_button_style(&self, status: FocusStatus) -> Style {
        let p = crate::theme_lookup::palette(self);
        let base = Style {
            text_color: p.text,
            ..Style::default()
        };
        match status {
            FocusStatus::Active | FocusStatus::Focused => base,
            FocusStatus::Hovered => Style {
                background: Some(Background::Color(p.overlay_hover())),
                ..base
            },
            FocusStatus::Pressed => Style {
                background: Some(Background::Color(p.overlay_pressed())),
                ..base
            },
            FocusStatus::Disabled => Style {
                text_color: p.disabled,
                ..base
            },
        }
    }
}

/// Resolve shared icon-action ring ink and width for the supplied fill and radius.
/// Translucent fill is composited over the palette page background; ancestor
/// surfaces are not sampled. The radius is in logical pixels.
pub fn icon_action_focus_ring(theme: &iced::Theme, fill: Color, radius: f32) -> Border {
    Border {
        radius: radius.into(),
        width: theme.enz_focus_ring_width(),
        color: theme.enz_focus_ring_color(Some(fill)),
    }
}

/// Stroke and geometry for one focus-visible ring.
/// An outset changes bounds without automatically changing the supplied radius.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FocusRingQuad {
    /// The ring stroke colour.
    pub ink: Color,
    /// The ring stroke width in logical px.
    pub width: f32,
    /// The ring corner radius. A grown ring normally passes the control's own
    /// radius plus [`outset`](Self::outset).
    pub radius: iced::border::Radius,
    /// Outward growth applied to all four sides of the subject bounds.
    /// Negative values inset (`image_viewer`).
    pub outset: f32,
    /// Whether the quad snaps to the physical pixel grid.
    pub snap: bool,
}

/// Grow each side of `subject` by `outset` logical pixels.
/// Negative outset insets; width and height clamp at zero, while the origin
/// continues to move by the requested amount.
#[inline]
pub fn focus_ring_bounds(subject: Rectangle, outset: f32) -> Rectangle {
    Rectangle {
        x: subject.x - outset,
        y: subject.y - outset,
        width: (subject.width + 2.0 * outset).max(0.0),
        height: (subject.height + 2.0 * outset).max(0.0),
    }
}

/// Draw a transparent quad with the supplied border and pixel-snap policy.
/// The caller decides focus visibility; bounds use [`focus_ring_bounds`].
#[inline]
pub fn paint_focus_ring<R>(renderer: &mut R, subject: Rectangle, ring: FocusRingQuad)
where
    R: iced::advanced::Renderer,
{
    renderer.fill_quad(
        renderer::Quad {
            bounds: focus_ring_bounds(subject, ring.outset),
            border: Border {
                color: ring.ink,
                width: ring.width,
                radius: ring.radius,
            },
            shadow: iced::Shadow::default(),
            snap: ring.snap,
        },
        Background::Color(Color::TRANSPARENT),
    );
}
