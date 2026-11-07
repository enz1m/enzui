//! Renderer-independent placement for popups anchored to rectangles or points.
//!
//! Rectangle placement supports side fallbacks, cross-axis alignment, viewport
//! insets, overflow handling, and writing direction. All inputs share one logical
//! coordinate space and outputs are sanitized to finite values.
//!
//! ```
//! use enzui_core::placement::{Placement, Side};
//! use iced_core::{Rectangle, Size};
//! let viewport = Rectangle::new([0.0, 0.0].into(), Size::new(800.0, 600.0));
//! let anchor = Rectangle::new([100.0, 40.0].into(), Size::new(80.0, 24.0));
//! let resolved = Placement::new(Side::Bottom).gap(6.0)
//!     .resolve(anchor, Size::new(120.0, 40.0), viewport);
//! assert_eq!(resolved.side, Side::Bottom);
//! ```

use iced_core::{Padding, Point, Rectangle, Size};

mod direction;
mod geometry;
mod point;

pub use direction::*;
use geometry::{
    align, arrow_origin, clamp_span, finite_or_zero, inset, ratio, sane_inset, sanitize_rect,
    saturate_point, saturate_rect,
};
pub use point::*;

/// Which side of the anchor the popup is placed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    /// Above the anchor.
    Top,
    /// Below the anchor.
    Bottom,
    /// To the left of the anchor.
    Left,
    /// To the right of the anchor.
    Right,
}

impl Side {
    /// The directly opposite side.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }

    /// Whether this side stacks the popup vertically (above/below the anchor).
    ///
    /// Vertical sides align along the x axis; horizontal sides along y.
    #[must_use]
    pub const fn is_vertical(self) -> bool {
        matches!(self, Self::Top | Self::Bottom)
    }

    /// The two sides perpendicular to this one, in `(start, end)` order.
    ///
    /// Used to build a fallback order that tries the perpendicular axis after
    /// the preferred axis is exhausted.
    #[must_use]
    pub const fn perpendicular(self) -> (Self, Self) {
        if self.is_vertical() {
            (Self::Left, Self::Right)
        } else {
            (Self::Top, Self::Bottom)
        }
    }
}

/// Alignment of a popup along the anchor cross axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Alignment {
    /// Flush with the anchor's leading edge.
    Start,
    /// Centred on the anchor.
    #[default]
    Center,
    /// Flush with the anchor's trailing edge.
    End,
}

/// Selects whether cross-axis positioning is relative to the anchor or viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CrossAxis {
    /// Align along the anchor using the supplied [`Alignment`].
    #[default]
    Anchor,
    /// Centers along the inset viewport’s cross axis instead of aligning to the anchor.
    ViewportCenter,
}

impl Alignment {
    /// Swaps [`Alignment::Start`] and [`Alignment::End`] while preserving [`Alignment::Center`].
    #[must_use]
    pub const fn mirrored(self) -> Self {
        match self {
            Self::Start => Self::End,
            Self::Center => Self::Center,
            Self::End => Self::Start,
        }
    }
}

/// What to do when the popup cannot fit on any candidate side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Overflow {
    /// Preserves intrinsic size and clamps into the viewport, permitting anchor overlap.
    #[default]
    Clamp,
    /// Shrinks the popup to the largest space available on the selected side.
    Shrink,
}

/// Final popup geometry and the fallback adjustments used to produce it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resolved {
    /// The popup's final rectangle, in the same space as the inputs.
    pub bounds: Rectangle,
    /// The side the popup actually landed on, after any fallback.
    pub side: Side,
    /// Whether the preferred side was unavailable and a fallback was used.
    pub flipped: bool,
    /// Whether the popup had to be nudged along its cross axis to fit.
    pub shifted: bool,
    /// Whether the popup was shrunk from its intrinsic size ([`Overflow::Shrink`]).
    pub shrunk: bool,
    /// Whether the popup overlaps its anchor.
    ///
    /// This occurs only with [`Overflow::Clamp`] when no candidate side fits.
    pub occluded: bool,
    /// Where an arrow/beak should point, on the popup edge facing the anchor.
    ///
    /// Already clamped into the popup's own edge so the arrow cannot detach
    /// from its bubble when the popup has been shifted along the cross axis.
    pub arrow_origin: Point,
}

/// Policy for resolving an anchored popup within a viewport.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    preferred: Side,
    alignment: Alignment,
    cross_axis: CrossAxis,
    direction: Direction,
    gap: f32,
    insets: Padding,
    overflow: Overflow,
    fallbacks: Option<Vec<Side>>,
    allow_shift: bool,
}

impl Placement {
    /// A placement preferring `side`, centred, with no gap or insets.
    #[must_use]
    pub fn new(preferred: Side) -> Self {
        Self {
            preferred,
            alignment: Alignment::Center,
            cross_axis: CrossAxis::Anchor,
            direction: Direction::LeftToRight,
            gap: 0.0,
            insets: Padding::ZERO,
            overflow: Overflow::Clamp,
            fallbacks: None,
            allow_shift: true,
        }
    }

    /// Set the cross-axis alignment.
    #[must_use]
    pub fn alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = alignment;
        self.cross_axis = CrossAxis::Anchor;
        self
    }

    /// Centers the popup in the inset viewport on its cross axis while keeping its main axis anchored.
    #[must_use]
    pub fn viewport_centered(mut self) -> Self {
        self.cross_axis = CrossAxis::ViewportCenter;
        self
    }

    /// Sets writing direction; right-to-left mirrors `Start` and `End` only for vertical sides.
    #[must_use]
    pub fn direction(mut self, direction: Direction) -> Self {
        self.direction = direction;
        self
    }

    /// Sets the gap between the anchor edge and popup edge.
    ///
    /// Non-finite and negative values are treated as zero.
    #[must_use]
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = if gap.is_finite() && gap > 0.0 {
            gap
        } else {
            0.0
        };
        self
    }

    /// Sets margins the popup must stay clear of.
    ///
    /// Each non-finite or negative component is treated as zero.
    #[must_use]
    pub fn insets(mut self, insets: Padding) -> Self {
        self.insets = Padding {
            top: sane_inset(insets.top),
            right: sane_inset(insets.right),
            bottom: sane_inset(insets.bottom),
            left: sane_inset(insets.left),
        };
        self
    }

    /// Set the policy for when the popup fits nowhere.
    #[must_use]
    pub fn overflow(mut self, overflow: Overflow) -> Self {
        self.overflow = overflow;
        self
    }

    /// Overrides fallback order after the preferred side.
    ///
    /// By default, the opposite and then perpendicular sides are tried. An empty list disables fallbacks, and duplicate sides are ignored.
    #[must_use]
    pub fn fallbacks(mut self, fallbacks: impl Into<Vec<Side>>) -> Self {
        self.fallbacks = Some(fallbacks.into());
        self
    }

    /// Enables or disables cross-axis shifting, which is enabled by default.
    ///
    /// When disabled, the aligned cross-axis position may remain outside the viewport.
    #[must_use]
    pub fn allow_shift(mut self, allow: bool) -> Self {
        self.allow_shift = allow;
        self
    }

    /// The order of sides this placement will try, preferred side first.
    #[must_use]
    pub fn side_order(&self) -> Vec<Side> {
        let mut order = vec![self.preferred];

        match &self.fallbacks {
            Some(explicit) => {
                for side in explicit {
                    if !order.contains(side) {
                        order.push(*side);
                    }
                }
            }
            None => {
                let (a, b) = self.preferred.perpendicular();
                for side in [self.preferred.opposite(), a, b] {
                    if !order.contains(&side) {
                        order.push(side);
                    }
                }
            }
        }

        order
    }

    /// Resolves popup geometry from an anchor, intrinsic size, and viewport in one
    /// logical coordinate space. Non-finite inputs are sanitized.
    #[must_use]
    pub fn resolve(&self, anchor: Rectangle, popup: Size, viewport: Rectangle) -> Resolved {
        let viewport = sanitize_rect(viewport);
        let anchor = sanitize_rect(anchor);
        let popup = Size::new(
            finite_or_zero(popup.width).max(0.0),
            finite_or_zero(popup.height).max(0.0),
        );

        let field = inset(viewport, self.insets);

        let order = self.side_order();

        for (index, side) in order.iter().enumerate() {
            if self.fits(*side, anchor, popup, field) {
                return self.finish(*side, anchor, popup, field, index > 0, false);
            }
        }

        // Compare sides by visible-area fraction across axes, but require a later side
        // to improve the score by `OVERRIDE_MARGIN` before overriding preference order.
        const OVERRIDE_MARGIN: f32 = 1.25;

        let mut best = self.preferred;
        let mut best_score = f32::NEG_INFINITY;
        for side in &order {
            let score = self.visible_fraction(*side, anchor, popup, field);
            if score > best_score * OVERRIDE_MARGIN || best_score < 0.0 {
                best_score = score;
                best = *side;
            }
        }

        let flipped = best != self.preferred;

        match self.overflow {
            Overflow::Clamp => self.finish(best, anchor, popup, field, flipped, false),
            Overflow::Shrink => {
                // Shrink both the side-facing and cross axes; otherwise an oversized popup still overflows the field.
                let room = self.room(best, anchor, field).max(0.0);
                let shrunk = if best.is_vertical() {
                    Size::new(popup.width.min(field.width), popup.height.min(room))
                } else {
                    Size::new(popup.width.min(room), popup.height.min(field.height))
                };
                self.finish(best, anchor, shrunk, field, flipped, shrunk != popup)
            }
        }
    }

    fn visible_fraction(
        &self,
        side: Side,
        anchor: Rectangle,
        popup: Size,
        field: Rectangle,
    ) -> f32 {
        let room = self.room(side, anchor, field).max(0.0);

        let (need_main, need_cross, cross_space) = if side.is_vertical() {
            (popup.height, popup.width, field.width)
        } else {
            (popup.width, popup.height, field.height)
        };

        ratio(room, need_main) * ratio(cross_space, need_cross)
    }

    fn room(&self, side: Side, anchor: Rectangle, field: Rectangle) -> f32 {
        let raw = match side {
            Side::Top => anchor.y - field.y,
            Side::Bottom => (field.y + field.height) - (anchor.y + anchor.height),
            Side::Left => anchor.x - field.x,
            Side::Right => (field.x + field.width) - (anchor.x + anchor.width),
        };

        raw - self.gap
    }

    fn fits(&self, side: Side, anchor: Rectangle, popup: Size, field: Rectangle) -> bool {
        let needed = if side.is_vertical() {
            popup.height
        } else {
            popup.width
        };

        if self.room(side, anchor, field) < needed {
            return false;
        }

        // Require cross-axis fit because shifting cannot make a popup narrower than the field.
        if side.is_vertical() {
            popup.width <= field.width
        } else {
            popup.height <= field.height
        }
    }

    fn finish(
        &self,
        side: Side,
        anchor: Rectangle,
        popup: Size,
        field: Rectangle,
        flipped: bool,
        shrunk: bool,
    ) -> Resolved {
        let (main_x, main_y) = match side {
            Side::Top => (f32::NAN, anchor.y - popup.height - self.gap),
            Side::Bottom => (f32::NAN, anchor.y + anchor.height + self.gap),
            Side::Left => (anchor.x - popup.width - self.gap, f32::NAN),
            Side::Right => (anchor.x + anchor.width + self.gap, f32::NAN),
        };

        // RTL mirrors only horizontal cross axes, affecting top/bottom placement but not left/right placement.
        let alignment = match (self.direction, side.is_vertical()) {
            (Direction::RightToLeft, true) => self.alignment.mirrored(),
            _ => self.alignment,
        };

        let (cross_x, cross_y) = match (side.is_vertical(), self.cross_axis) {
            (true, CrossAxis::Anchor) => (
                align(anchor.x, anchor.width, popup.width, alignment),
                main_y,
            ),
            (false, CrossAxis::Anchor) => (
                main_x,
                align(anchor.y, anchor.height, popup.height, alignment),
            ),
            (true, CrossAxis::ViewportCenter) => {
                (field.x + (field.width - popup.width) / 2.0, main_y)
            }
            (false, CrossAxis::ViewportCenter) => {
                (main_x, field.y + (field.height - popup.height) / 2.0)
            }
        };

        let mut bounds = Rectangle {
            x: cross_x,
            y: cross_y,
            width: popup.width,
            height: popup.height,
        };

        let mut shifted = false;
        if self.allow_shift {
            if side.is_vertical() {
                let clamped = clamp_span(bounds.x, bounds.width, field.x, field.width);
                shifted = clamped != bounds.x;
                bounds.x = clamped;
            } else {
                let clamped = clamp_span(bounds.y, bounds.height, field.y, field.height);
                shifted = clamped != bounds.y;
                bounds.y = clamped;
            }
        }

        // Clamp the main axis so an impossible placement overlaps the anchor instead of remaining off-screen.
        if side.is_vertical() {
            bounds.y = clamp_span(bounds.y, bounds.height, field.y, field.height);
        } else {
            bounds.x = clamp_span(bounds.x, bounds.width, field.x, field.width);
        }

        // Saturation closes the public invariant when arithmetic on finite inputs
        // overflows: every returned scalar remains within the finite `f32` range.
        let bounds = saturate_rect(bounds);
        let arrow = saturate_point(arrow_origin(bounds, anchor, side));
        let occluded = bounds.intersects(&anchor);

        Resolved {
            bounds,
            side,
            flipped,
            shifted,
            shrunk,
            occluded,
            arrow_origin: arrow,
        }
    }
}
