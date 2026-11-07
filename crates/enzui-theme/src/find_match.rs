//! Palette-derived fills and contrast-safe glyph ink for search matches.
//!
//! [`FindMatchTokens`] distinguishes the active match from subordinate matches;
//! [`FindMatchTokens::ink_on`] resolves the supplied glyph ink against the
//! selected fill.

use iced::Color;

use crate::palette::{
    Palette, RaisedInk, legible_ink_on, lift_brand_to_text_contrast, mix_srgb, raise_to_target,
    wcag_contrast,
};

/// How prominent a painted match is: the one the "3 of 12" counter points at,
/// or one of the subordinate others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MatchEmphasis {
    /// The current match — the one navigation steps through and replace acts on.
    Active,
    /// Any other match in the document: visible, deliberately subordinate.
    Inactive,
}

/// The raster margin added to [`crate::ContrastFloor::Text`] when resolving a match
/// glyph's ink over the shared composited match fill.
pub const MATCH_INK_RASTER_MARGIN: f32 = 0.1;

/// The contrast target [`FindMatchTokens::ink_on`] resolves against: the
/// [`crate::ContrastFloor::Text`] WCAG minimum lifted by [`MATCH_INK_RASTER_MARGIN`].
///
/// This is the semantic contrast policy for shared composited match ink. A
/// text-editor match paints coloured glyphs over a coloured fill, and the user
/// sees anti-aliased pixels rather than the token maths, so widgets that render
/// syntax-toned glyphs inside a find or search match should target this ratio
/// rather than `ContrastFloor::Text.minimum_ratio()` directly.
pub const MATCH_INK_TEXT_TARGET: f32 = 4.5 + MATCH_INK_RASTER_MARGIN;

/// Fraction of `accent` in the subordinate match fill.
/// The result is the opaque equivalent of a 35% accent wash over `background`,
/// giving ink resolution an exact backdrop.
pub const INACTIVE_TINT: f32 = 0.35;

/// The find/search match fills, and the resolver that keeps glyphs on them
/// legible.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FindMatchTokens {
    /// The opaque fill behind the current match.
    pub active_fill: Color,
    /// The opaque fill behind every other match.
    pub inactive_fill: Color,
}

impl FindMatchTokens {
    /// Resolves the active control fill from `accent` and the inactive fill from
    /// [`INACTIVE_TINT`] of `accent` over `background`.
    #[must_use]
    pub fn from_palette(palette: &Palette) -> Self {
        Self {
            active_fill: palette.filled_color(palette.accent).color,
            inactive_fill: Color {
                a: 1.0,
                ..mix_srgb(palette.accent, palette.background, 1.0 - INACTIVE_TINT)
            },
        }
    }

    /// The fill painted behind a match of this emphasis.
    pub fn fill(&self, emphasis: MatchEmphasis) -> Color {
        match emphasis {
            MatchEmphasis::Active => self.active_fill,
            MatchEmphasis::Inactive => self.inactive_fill,
        }
    }

    /// Returns glyph ink for this match emphasis, using `ink` as the unmatched
    /// foreground.
    pub fn ink_on(&self, ink: Color, emphasis: MatchEmphasis) -> Color {
        let fill = self.fill(emphasis);
        let opaque = Color { a: 1.0, ..ink };

        match raise_to_target(opaque, fill, MATCH_INK_TEXT_TARGET) {
            RaisedInk::AlreadyLegible(c) | RaisedInk::Raised(c) => Color { a: 1.0, ..c },
            RaisedInk::Impossible { .. } => {
                let lifted = lift_brand_to_text_contrast(opaque, fill, MATCH_INK_TEXT_TARGET);

                if wcag_contrast(lifted, fill) >= MATCH_INK_TEXT_TARGET {
                    Color { a: 1.0, ..lifted }
                } else {
                    legible_ink_on(fill)
                }
            }
        }
    }
}
