//! Theme-scaled stroke widths: hairlines for rules and grid gaps, and the
//! keyboard focus ring.
//!
//! [`StrokeToken::Hairline`] defaults to one logical pixel and
//! [`StrokeToken::FocusRing`] to two; both resolve through [`StrokeScale`].
//!
//! # Example
//!
//! ```
//! use enzui_theme::{StrokeScale, StrokeToken};
//!
//! assert_eq!(StrokeToken::Hairline.px(), 1.0);
//!
//! let dense = StrokeScale {
//!     hairline: 0.5,
//!     ..StrokeScale::default()
//! };
//! assert_eq!(StrokeToken::Hairline.resolve(&dense), 0.5);
//! ```

/// Theme-adjustable logical-pixel stroke widths.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeScale {
    /// Hairline width in logical pixels; defaults to `1.0`.
    pub hairline: f32,
    /// Keyboard focus ring width in logical pixels; defaults to `2.0`. Dense
    /// cells (calendar days, dropdown rows) draw three quarters of it.
    pub focus_ring: f32,
}

impl Default for StrokeScale {
    fn default() -> Self {
        Self {
            hairline: 1.0,
            focus_ring: 2.0,
        }
    }
}

/// A named stroke extent resolved through [`StrokeScale`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrokeToken {
    /// Reads [`StrokeScale::hairline`], whose default is one logical pixel.
    Hairline,
    /// Reads [`StrokeScale::focus_ring`], whose default is two logical pixels.
    FocusRing,
}

impl StrokeToken {
    /// Resolve against a concrete [`StrokeScale`] (the theme-driven path).
    ///
    /// Prefer this over [`StrokeToken::px`] when a theme is in scope, so a theme
    /// that rescales its stroke flows through. A non-finite or negative scale
    /// value is clamped to `0.0` so a malformed theme can never hand geometry a
    /// nonsense extent.
    pub fn resolve(self, scale: &StrokeScale) -> f32 {
        let v = match self {
            StrokeToken::Hairline => scale.hairline,
            StrokeToken::FocusRing => scale.focus_ring,
        };
        if v.is_finite() && v >= 0.0 { v } else { 0.0 }
    }

    /// Resolves against the default [`StrokeScale`]. Use [`Self::resolve`] when
    /// a live theme is available so custom stroke widths are honored.
    pub fn px(self) -> f32 {
        self.resolve(&StrokeScale::default())
    }
}
