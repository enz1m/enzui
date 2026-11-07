//! An eight-entry categorical colour sequence that cycles by series index.
//!
//! # Example
//!
//! ```
//! use enzui_widgets::chart_palette::ChartPalette;
//! use enzui_theme::Palette;
//!
//! let p = ChartPalette::from_palette(&Palette::dark());
//! assert_eq!(p.series_color(0), Palette::dark().accent);
//! assert_eq!(p.series_color(8), p.series_color(0));
//! ```
//!
//! Entry zero snapshots the supplied palette's accent; seven fixed RGB hues
//! follow it. A custom accent can duplicate a fixed hue. The sequence does not
//! update when a theme changes; rebuild it or resolve it during drawing.
//! Callers must supply the same resolved colours to charts and legend entries.

use enzui_theme::Palette;
use iced::Color;

/// Number of entries before the sequence repeats: 8.
pub const PALETTE_LEN: usize = 8;

const FIXED_HUES: [(u8, u8, u8); PALETTE_LEN - 1] = [
    (0x22, 0xc5, 0x5e),
    (0xf5, 0x9e, 0x0b),
    (0xef, 0x44, 0x44),
    (0xa8, 0x55, 0xf7),
    (0x06, 0xb6, 0xd4),
    (0xec, 0x48, 0x99),
    (0x84, 0xcc, 0x16),
];

/// A copyable snapshot of one accent and seven fixed categorical hues.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChartPalette {
    colors: [Color; PALETTE_LEN],
}

impl ChartPalette {
    /// Snapshot the supplied accent followed by seven fixed, opaque RGB hues.
    pub fn from_palette(p: &Palette) -> Self {
        let mut colors = [p.accent; PALETTE_LEN];
        for (slot, (r, g, b)) in colors.iter_mut().skip(1).zip(FIXED_HUES) {
            *slot = Color::from_rgb8(r, g, b);
        }
        Self { colors }
    }

    /// Preset built from the built-in dark palette (its accent is entry 0).
    /// Prefer [`ChartPalette::from_palette`] with the live theme's palette when
    /// one is available, so the accent tracks a custom theme.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Preset built from the built-in light palette (its accent is entry 0).
    /// Prefer [`ChartPalette::from_palette`] with the live theme's palette when
    /// one is available.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Resolve a series index modulo [`PALETTE_LEN`]; every `usize` is accepted.
    pub fn series_color(&self, index: usize) -> Color {
        self.colors[index % PALETTE_LEN]
    }

    /// The palette entries in order.
    pub fn colors(&self) -> &[Color; PALETTE_LEN] {
        &self.colors
    }

    /// Use a supplied colour with strictly positive alpha, otherwise the indexed hue.
    /// Partially transparent colours and positive infinite alpha are accepted without
    /// channel validation; zero, negative or NaN alpha falls back.
    pub fn resolve_or_series(&self, color: Option<Color>, index: usize) -> Color {
        match color {
            Some(c) if c.a > 0.0 => c,
            _ => self.series_color(index),
        }
    }
}

impl Default for ChartPalette {
    fn default() -> Self {
        Self::dark()
    }
}
