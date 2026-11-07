//! RGB token-distance cases for calendar and key-capture-chip hover fills.
//!
//! The manual registry compares each token fill with its declared background.
//! It does not derive widget coverage, sample rendered pixels, composite alpha
//! or establish perceptual contrast. Runtime backgrounds and caller overrides
//! require separate fixtures. Stored RGB channels are compared without validation.

use enzui_theme::Palette;
use iced::Color;

/// Sum absolute differences of stored RGB channels; alpha is ignored.
/// This is a float-channel distance, without colour-space conversion or quantization.
#[must_use]
pub fn channel_distance(a: Color, b: Color) -> f32 {
    (a.r - b.r).abs() + (a.g - b.g).abs() + (a.b - b.b).abs()
}

/// A declared background/fill pair and its minimum RGB channel distance.
#[derive(Debug, Clone, Copy)]
pub struct HoverVisibilityCase {
    /// Widget name for diagnostics.
    pub widget: &'static str,
    /// Interaction state name for diagnostics.
    pub state: &'static str,
    /// Declared background token; this is not sampled from rendering.
    pub known_parent: Color,
    /// Resolved interaction fill token.
    pub state_fill: Color,
    /// Minimum accepted sum of RGB channel differences; current cases use 0.02.
    pub min_distance: f32,
}

/// Resolve the manual calendar and key-capture-chip hover cases from a palette.
#[must_use]
pub fn registry(palette: &Palette) -> Vec<HoverVisibilityCase> {
    let mut out = Vec::new();
    {
        let t = crate::calendar::CalendarTokens::from_palette(palette);
        out.push(HoverVisibilityCase {
            widget: "calendar day-cell",
            state: "hover",
            known_parent: t.surface,
            state_fill: t.hover_background,
            min_distance: 0.02,
        });
    }
    {
        let t = crate::key_capture_chip::KeyCaptureChipTokens::from_palette(palette);
        out.push(HoverVisibilityCase {
            widget: "key-capture chip",
            state: "hover",
            known_parent: t.background,
            state_fill: t.hover_background,
            min_distance: 0.02,
        });
    }

    out
}
