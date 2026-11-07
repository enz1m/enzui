//! Shared geometry primitives for rectangle- and point-anchored placement.

use super::{Alignment, Side};
use iced_core::{Padding, Point, Rectangle};

pub(crate) fn align(start: f32, size: f32, len: f32, alignment: Alignment) -> f32 {
    match alignment {
        Alignment::Start => start,
        Alignment::Center => start + (size - len) / 2.0,
        Alignment::End => start + size - len,
    }
}

// Oversized spans pin to the range start so their leading content remains visible.
pub(crate) fn clamp_span(pos: f32, len: f32, min: f32, size: f32) -> f32 {
    let max = min + size - len;

    if max <= min { min } else { pos.clamp(min, max) }
}

pub(crate) fn arrow_origin(bounds: Rectangle, anchor: Rectangle, side: Side) -> Point {
    match side {
        Side::Top => Point::new(
            clamp_into(anchor.center_x(), bounds.x, bounds.x + bounds.width),
            bounds.y + bounds.height,
        ),
        Side::Bottom => Point::new(
            clamp_into(anchor.center_x(), bounds.x, bounds.x + bounds.width),
            bounds.y,
        ),
        Side::Left => Point::new(
            bounds.x + bounds.width,
            clamp_into(anchor.center_y(), bounds.y, bounds.y + bounds.height),
        ),
        Side::Right => Point::new(
            bounds.x,
            clamp_into(anchor.center_y(), bounds.y, bounds.y + bounds.height),
        ),
    }
}

pub(crate) fn ratio(available: f32, needed: f32) -> f32 {
    if needed <= 0.0 {
        1.0
    } else {
        (available / needed).clamp(0.0, 1.0)
    }
}

pub(crate) fn clamp_into(v: f32, lo: f32, hi: f32) -> f32 {
    if hi <= lo { lo } else { v.clamp(lo, hi) }
}

pub(crate) fn finite_or_zero(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

// Map NaN to zero and saturate infinities to signed `f32` bounds so arithmetic
// overflow cannot escape in resolved geometry.
#[inline]
#[must_use]
pub(crate) fn saturate_finite(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(-f32::MAX, f32::MAX)
    }
}

#[inline]
pub(crate) fn saturate_rect(r: Rectangle) -> Rectangle {
    Rectangle {
        x: saturate_finite(r.x),
        y: saturate_finite(r.y),
        width: saturate_finite(r.width).max(0.0),
        height: saturate_finite(r.height).max(0.0),
    }
}

#[inline]
pub(crate) fn saturate_point(p: Point) -> Point {
    Point::new(saturate_finite(p.x), saturate_finite(p.y))
}

// Insets are non-negative thicknesses; non-finite or negative values become zero.
pub(crate) fn sane_inset(v: f32) -> f32 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

pub(crate) fn sanitize_rect(r: Rectangle) -> Rectangle {
    Rectangle {
        x: finite_or_zero(r.x),
        y: finite_or_zero(r.y),
        width: finite_or_zero(r.width).max(0.0),
        height: finite_or_zero(r.height).max(0.0),
    }
}

// Guard finite subtraction overflow so inset rectangles retain finite,
// non-negative extents.
pub(crate) fn inset(r: Rectangle, p: Padding) -> Rectangle {
    let width = finite_or_zero(r.width - p.left - p.right).max(0.0);
    let height = finite_or_zero(r.height - p.top - p.bottom).max(0.0);

    Rectangle {
        x: finite_or_zero(r.x + p.left),
        y: finite_or_zero(r.y + p.top),
        width,
        height,
    }
}
