//! Scrollbar tokens: dimensions and opacities for overlay scrollbar chrome.
//!
//! `width` is the painted thumb; `hit_width` is the invisible grab/reveal band.
//! They must stay independent. The bar is always an overlay;
//! `content_gutter` is opt-in trailing clearance.
//!
//! # Example
//!
//! ```
//! use enzui_theme::ScrollbarTokens;
//!
//! let sb = ScrollbarTokens::DESKTOP_POINTER;
//! assert!(sb.width < sb.hit_width);
//! assert!(sb.alpha < sb.alpha_hover);
//! assert!(sb.content_gutter >= sb.width);
//! ```

/// Scrollbar design tokens — the dimensions and opacities of the overlay
/// scrollbar chrome.
///
/// All width values are in logical pixels. Alpha values are in `0.0..=1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarTokens {
    /// Painted thumb width, in logical pixels. Default 5 px.
    ///
    /// Purely visual. This is not the grab target, which is
    /// [`hit_width`](Self::hit_width), so narrowing it can never make the bar
    /// harder to grab.
    pub width: f32,

    /// Width of the full-length reveal and drag band, in logical pixels.
    /// Defaults to 15 px, is anchored at the trailing edge, and must remain at
    /// least as wide as [`Self::width`]. [`Self::DESKTOP_POINTER`] offers a wider
    /// 28 px target without widening the painted thumb.
    pub hit_width: f32,

    /// Minimum visual and draggable thumb length along the scroll axis, in
    /// logical pixels. Default 24 px.
    ///
    /// The proportional length still wins for ordinary content. For extreme
    /// content ratios this floor keeps the thumb visible and grabbable; tracks
    /// shorter than the floor clamp the thumb to the track itself. Position is
    /// mapped over the remaining travel, so both endpoints stay exact.
    pub min_thumb_length: f32,

    /// Scroller opacity once revealed. Default 0.40 — GTK4 Adwaita's
    /// overlay-indicator `gtkalpha($fg_color, 0.4)`.
    pub alpha: f32,

    /// Scroller opacity while the thumb is being dragged. Default 0.60.
    pub alpha_hover: f32,

    /// Optional trailing content clearance in logical pixels.
    /// It composes as a maximum with caller padding and must reach the painted
    /// thumb's inner edge, `(hit_width + width) / 2`.
    pub content_gutter: f32,

    /// Border radius of the scroller thumb, in logical pixels.
    ///
    /// Defaults to half [`width`](Self::width), producing a capsule. Normalization
    /// preserves smaller radii and clamps larger ones to half the width.
    pub border_radius: f32,
}

impl ScrollbarTokens {
    /// The default token set, as an associated `const`.
    pub const DEFAULT: Self = Self {
        width: 5.0,
        hit_width: 15.0,
        min_thumb_length: 24.0,
        alpha: 0.40,
        alpha_hover: 0.60,
        content_gutter: 10.0,
        border_radius: 2.5,
    };

    /// Desktop pointer preset with a 28 px grab band and a 5 px painted thumb.
    ///
    /// Sets a 24 px minimum thumb length and a 16.5 px optional content gutter.
    /// Pass this to a scroll surface's `scrollbar_tokens` override. The full
    /// grab band overlays content; reserve extra clearance near trailing controls.
    pub const DESKTOP_POINTER: Self = Self {
        hit_width: 28.0,
        content_gutter: (28.0 + Self::DEFAULT.width) / 2.0,
        ..Self::DEFAULT
    };

    /// Normalizes every public field for layout and paint.
    ///
    /// Non-finite values use [`Self::DEFAULT`]; extents clamp at zero, alphas at
    /// `0..=1`, `hit_width >= width`, `alpha_hover >= alpha`, the gutter clears
    /// the thumb, and the radius is at most half the painted width.
    #[must_use]
    pub fn normalized(self) -> Self {
        fn extent(value: f32, fallback: f32) -> f32 {
            if value.is_finite() {
                value.max(0.0)
            } else {
                fallback
            }
        }

        fn opacity(value: f32, fallback: f32) -> f32 {
            if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                fallback
            }
        }

        let width = extent(self.width, Self::DEFAULT.width);
        let hit_width = extent(self.hit_width, Self::DEFAULT.hit_width).max(width);
        let alpha = opacity(self.alpha, Self::DEFAULT.alpha);
        let alpha_hover = opacity(self.alpha_hover, Self::DEFAULT.alpha_hover).max(alpha);
        let content_gutter = extent(self.content_gutter, Self::DEFAULT.content_gutter)
            .max((hit_width + width) / 2.0);

        Self {
            width,
            hit_width,
            min_thumb_length: extent(self.min_thumb_length, Self::DEFAULT.min_thumb_length),
            alpha,
            alpha_hover,
            content_gutter,
            border_radius: extent(self.border_radius, Self::DEFAULT.border_radius).min(width / 2.0),
        }
    }
}

impl Default for ScrollbarTokens {
    fn default() -> Self {
        Self::DEFAULT
    }
}
