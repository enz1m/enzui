//! Per-edge rectangle overflow measurements for layout auditing.
//!
//! # Example
//!
//! ```
//! use enzui_widgets::bounds_probe::bounds_probe;
//! use iced::Rectangle;
//! let child = Rectangle::new([-4.0, 12.0].into(), [100.0, 40.0].into());
//! let bounds = Rectangle::new([0.0, 0.0].into(), [200.0, 100.0].into());
//! let probe = bounds_probe(child, bounds, false);
//! assert!(probe.spilling_x);
//! assert_eq!(probe.spill_left, 4.0);
//! ```
//!
//! This is pure geometry; [`crate::layout_spill_probe::LayoutSpillProbe`]
//! publishes real widget bounds for collection. Values use logical pixels.
//! Boolean verdicts use the fixed [`SPILL_EPSILON`]; when display scale is
//! available, compare `spill_amount` with [`spill_epsilon_for_scale`] or use
//! [`crate::layout_spill_probe::spill_amount`].
//!
//! Allowed spill and rectangles with non-finite fields return [`BoundsProbe::NONE`].
//! Negative dimensions are not normalized, and finite fields can overflow edge
//! arithmetic. Supply valid rectangles; this helper does not inspect paint.

use iced::Rectangle;

/// Per-edge spill of a child rectangle against a container rectangle.
///
/// A positive value on an edge means the child extends past that edge by
/// that many pixels; zero means it's inside (or aligned) on that edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundsProbe {
    /// Pixels the child extends past the container's left edge (0 if inside).
    pub spill_left: f32,
    /// Pixels the child extends past the container's right edge (0 if inside).
    pub spill_right: f32,
    /// Pixels the child extends past the container's top edge (0 if inside).
    pub spill_top: f32,
    /// Pixels the child extends past the container's bottom edge (0 if inside).
    pub spill_bottom: f32,
    /// The worst single-edge spill (max of the four).
    pub spill_amount: f32,
    /// True when left or right spill exceeds [`SPILL_EPSILON`].
    pub spilling_x: bool,
    /// True when top or bottom spill exceeds [`SPILL_EPSILON`].
    pub spilling_y: bool,
    /// True when any edge is spilling.
    pub spilling: bool,
}

impl BoundsProbe {
    /// The all-zero, non-spilling result (also what `allow_spill = true` and a
    /// fully-contained child both produce).
    pub const NONE: BoundsProbe = BoundsProbe {
        spill_left: 0.0,
        spill_right: 0.0,
        spill_top: 0.0,
        spill_bottom: 0.0,
        spill_amount: 0.0,
        spilling_x: false,
        spilling_y: false,
        spilling: false,
    };
}

/// Fixed spill-verdict tolerance in logical pixels: 0.5.
/// An edge equal to the tolerance is accepted; smaller raw edge amounts remain
/// in the result. Use [`spill_epsilon_for_scale`] for scale-aware interpretation.
pub const SPILL_EPSILON: f32 = 0.5;

/// Resolve logical-pixel tolerance as `0.5 / max(scale, 1.0)`.
/// Scales below one, including negative values and NaN, use 0.5. Positive
/// infinity yields zero. This does not change [`bounds_probe`]'s boolean flags.
pub fn spill_epsilon_for_scale(scale: f32) -> f32 {
    SPILL_EPSILON / scale.max(1.0)
}

/// Measure each child edge beyond its container in logical pixels.
/// `allow_spill` or a non-finite rectangle field returns [`BoundsProbe::NONE`].
/// Dimensions are not normalized; boolean verdicts use a strict comparison
/// against [`SPILL_EPSILON`], while edge amounts retain sub-tolerance overflow.
pub fn bounds_probe(child: Rectangle, bounds: Rectangle, allow_spill: bool) -> BoundsProbe {
    if allow_spill || !finite(child) || !finite(bounds) {
        return BoundsProbe::NONE;
    }
    let spill_left = (bounds.x - child.x).max(0.0);
    let spill_top = (bounds.y - child.y).max(0.0);
    let spill_right = ((child.x + child.width) - (bounds.x + bounds.width)).max(0.0);
    let spill_bottom = ((child.y + child.height) - (bounds.y + bounds.height)).max(0.0);
    let spill_amount = spill_left.max(spill_right).max(spill_top).max(spill_bottom);
    let spilling_x = spill_left > SPILL_EPSILON || spill_right > SPILL_EPSILON;
    let spilling_y = spill_top > SPILL_EPSILON || spill_bottom > SPILL_EPSILON;
    BoundsProbe {
        spill_left,
        spill_right,
        spill_top,
        spill_bottom,
        spill_amount,
        spilling_x,
        spilling_y,
        spilling: spilling_x || spilling_y,
    }
}

#[inline]
fn finite(r: Rectangle) -> bool {
    r.x.is_finite() && r.y.is_finite() && r.width.is_finite() && r.height.is_finite()
}
