//! Point-anchored popup placement and coordinate translation.

use super::geometry::{
    clamp_into, finite_or_zero, sanitize_rect, saturate_finite, saturate_point, saturate_rect,
};
use super::{Resolved, Side};
use iced_core::{Point, Rectangle, Size, Vector};

/// Places a popup at a point, flipping each overflowing axis and clamping it
/// within the viewport margin.
///
/// Non-finite or negative geometry is sanitized, and the returned bounds are finite.
#[must_use]
pub fn place_at_point(
    anchor: Point,
    popup: Size,
    viewport: Rectangle,
    edge_margin: f32,
) -> Resolved {
    let viewport = sanitize_rect(viewport);
    let margin = if edge_margin.is_finite() && edge_margin >= 0.0 {
        edge_margin
    } else {
        0.0
    };
    let w = finite_or_zero(popup.width).max(0.0);
    let h = finite_or_zero(popup.height).max(0.0);
    let ax = finite_or_zero(anchor.x);
    let ay = finite_or_zero(anchor.y);

    let (x, flipped_x, shifted_x) = place_point_axis(ax, w, viewport.x, viewport.width, margin);
    let (y, flipped_y, shifted_y) = place_point_axis(ay, h, viewport.y, viewport.height, margin);

    let bounds = saturate_rect(Rectangle {
        x,
        y,
        width: w,
        height: h,
    });

    let arrow = saturate_point(Point::new(
        clamp_into(ax, bounds.x, bounds.x + bounds.width),
        clamp_into(ay, bounds.y, bounds.y + bounds.height),
    ));

    let flipped = flipped_x || flipped_y;

    Resolved {
        bounds,
        side: match (flipped_x, flipped_y) {
            (false, false) => Side::Bottom,
            (false, true) => Side::Top,
            (true, false) => Side::Bottom,
            (true, true) => Side::Top,
        },
        flipped,
        shifted: shifted_x || shifted_y,
        shrunk: false,
        occluded: bounds.intersects(&Rectangle::new(anchor, Size::ZERO)),
        arrow_origin: arrow,
    }
}

fn place_point_axis(
    anchor: f32,
    size: f32,
    field_origin: f32,
    field_extent: f32,
    margin: f32,
) -> (f32, bool, bool) {
    let far_limit = saturate_finite(field_origin + field_extent - size - margin);
    let near_limit = saturate_finite(field_origin + margin);

    let (placed, did_flip) = if anchor + size <= field_origin + field_extent - margin {
        (anchor, false)
    } else {
        (saturate_finite(anchor - size), true)
    };

    // Reversed clamp bounds pin a popup larger than the field to the near edge.
    let clamped = saturate_finite(placed.min(far_limit).max(near_limit));
    (clamped, did_flip, clamped != placed)
}

/// Translates a resolved placement into another coordinate space.
///
/// Overflowing infinities saturate to the signed `f32` bounds and NaN maps to zero.
#[must_use]
pub fn translated(resolved: Resolved, translation: Vector) -> Resolved {
    Resolved {
        bounds: saturate_rect(resolved.bounds + translation),
        arrow_origin: saturate_point(resolved.arrow_origin + translation),
        ..resolved
    }
}
