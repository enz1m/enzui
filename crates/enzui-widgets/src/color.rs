//! Shared color manipulation utilities for enzUI-ICED widgets.

use iced::Color;

/// Multiply RGB channels by `1 - amount`, preserving alpha.
/// Use amounts in `0..=1`: zero preserves RGB and one produces black. The amount
/// is not clamped. This differs from gradient-background value scaling, which
/// divides brightness by `1 + amount`.
///
/// # Panics
/// In debug builds, panics if the resulting RGB channels are outside `0..=1`
/// or non-finite.
pub fn darken(color: Color, amount: f32) -> Color {
    Color::from_rgba(
        color.r * (1.0 - amount),
        color.g * (1.0 - amount),
        color.b * (1.0 - amount),
        color.a,
    )
}

/// Interpolate RGB toward `toward`, preserving `base` alpha.
/// Finite amounts clamp to `0..=1`; non-finite amounts use zero.
///
/// # Panics
/// In debug builds, panics if the resulting RGB channels are outside `0..=1`
/// or non-finite.
pub fn mix(base: Color, toward: Color, amount: f32) -> Color {
    let t = if amount.is_finite() {
        amount.clamp(0.0, 1.0)
    } else {
        0.0
    };
    Color::from_rgba(
        base.r + (toward.r - base.r) * t,
        base.g + (toward.g - base.g) * t,
        base.b + (toward.b - base.b) * t,
        base.a,
    )
}

/// Replace alpha without clamping or validation, preserving RGB.
pub fn with_alpha(color: Color, alpha: f32) -> Color {
    Color { a: alpha, ..color }
}

/// Resolve disabled text ink against the opaque surface behind the text.
/// The preferred colour is the dark text rung or the light palette disabled ink.
/// It is retained when contrast is at least 3:1 and at most half the enabled-text
/// contrast; otherwise [`Palette::disabled_on`](enzui_theme::palette::Palette::disabled_on)
/// resolves the fallback. Custom palettes and backdrops may not admit that band.
/// Apply the resolved ink directly; further alpha dimming changes its contrast.
pub fn disabled_text_ink(p: &enzui_theme::palette::Palette, backdrop: Color) -> Color {
    let preferred = if p.is_dark() {
        Color::from_rgb(
            0x87 as f32 / 255.0,
            0x81 as f32 / 255.0,
            0x8C as f32 / 255.0,
        )
    } else {
        p.disabled
    };
    p.disabled_on_with_preferred(preferred, backdrop)
}

/// Average RGB channels and return an opaque colour, discarding both input alphas.
///
/// # Panics
/// In debug builds, panics if the resulting RGB channels are outside `0..=1`
/// or non-finite.
pub fn blend(base: Color, target: Color) -> Color {
    Color::from_rgba(
        (base.r + target.r) / 2.0,
        (base.g + target.g) / 2.0,
        (base.b + target.b) / 2.0,
        1.0,
    )
}

pub(crate) fn disabled_alpha(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        enzui_theme::DEFAULT_DISABLED_OPACITY
    }
}

pub(crate) fn with_opacity(color: Color, opacity: f32) -> Color {
    with_alpha(color, color.a * opacity)
}
