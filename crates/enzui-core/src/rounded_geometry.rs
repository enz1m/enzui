//! Hit testing for rounded rectangles that matches the renderer’s corner geometry.

use iced_core::border::Radius;
use iced_core::{Point, Rectangle};

/// Tests whether a point lies inside rounded bounds.
///
/// Invalid radii become zero, oversized radii clamp to half the shorter side, and zero radii match `Rectangle::contains`.
pub fn point_in_rounded_rect(point: Point, bounds: Rectangle, radius: Radius) -> bool {
    if !bounds.contains(point) {
        return false;
    }

    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return false;
    }

    let max_r = 0.5 * bounds.width.min(bounds.height);
    let clamp = |r: f32| {
        let r = if r.is_finite() && r > 0.0 { r } else { 0.0 };
        r.min(max_r)
    };

    let tl = clamp(radius.top_left);
    let tr = clamp(radius.top_right);
    let br = clamp(radius.bottom_right);
    let bl = clamp(radius.bottom_left);

    let excluded_by_corner = |cx: f32, cy: f32, r: f32, beyond_x: bool, beyond_y: bool| -> bool {
        if r <= 0.0 {
            return false;
        }
        let outside_x = if beyond_x { point.x > cx } else { point.x < cx };
        let outside_y = if beyond_y { point.y > cy } else { point.y < cy };
        if outside_x && outside_y {
            let dx = point.x - cx;
            let dy = point.y - cy;
            (dx * dx + dy * dy) > (r * r)
        } else {
            false
        }
    };

    let left = bounds.x;
    let right = bounds.x + bounds.width;
    let top = bounds.y;
    let bottom = bounds.y + bounds.height;

    if excluded_by_corner(left + tl, top + tl, tl, false, false) {
        return false;
    }
    if excluded_by_corner(right - tr, top + tr, tr, true, false) {
        return false;
    }
    if excluded_by_corner(right - br, bottom - br, br, true, true) {
        return false;
    }
    if excluded_by_corner(left + bl, bottom - bl, bl, false, true) {
        return false;
    }

    true
}
