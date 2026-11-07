//! Internal surface-edge colour interpolation that preserves the fill alpha.
//!
//! Edge depth is clamped to 0..1, with non-finite values using 0.12.
//! Hover edges interpolate RGB toward the supplied ink by 0.25.

use iced::Color;
pub(crate) const DEFAULT_EDGE_DEPTH: f32 = 0.12;
pub(crate) const HOVER_EDGE_SHARE: f32 = 0.25;
pub(crate) const fn hover_edge_color(fill: Color, ink: Color) -> Color {
    let t = HOVER_EDGE_SHARE;
    Color::from_rgba(
        fill.r + (ink.r - fill.r) * t,
        fill.g + (ink.g - fill.g) * t,
        fill.b + (ink.b - fill.b) * t,
        fill.a,
    )
}
pub(crate) const fn edge_color(surface: Color, depth: f32) -> Color {
    let depth = if depth.is_finite() {
        depth.clamp(0.0, 1.0)
    } else {
        DEFAULT_EDGE_DEPTH
    };
    let keep = 1.0 - depth;
    Color::from_rgba(
        surface.r * keep,
        surface.g * keep,
        surface.b * keep,
        surface.a,
    )
}
