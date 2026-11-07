//! Hit testing for committed annotations in image coordinates.
//!
//! Hit tolerance combines image-space aiming slop with half the stroke width.
//! Filled interiors count, open shapes do not, and overlapping results are
//! returned in top-most-first paint order.

use iced_core::Point;

use crate::image_space::ImageProjection;

use super::{Annotation, AnnotationId, ShapeGeometry, StrokeStyle};

/// Default aiming tolerance in display pixels.
pub const DEFAULT_SCREEN_SLOP: f32 = 4.0;

/// Converts display-pixel aiming tolerance to image units using the projection’s geometric-mean scale.
///
/// Negative or non-finite values use `DEFAULT_SCREEN_SLOP`; zero is preserved.
#[must_use]
pub fn image_slop(screen_slop: f32, projection: ImageProjection) -> f32 {
    let slop = if screen_slop.is_finite() && screen_slop >= 0.0 {
        screen_slop
    } else {
        DEFAULT_SCREEN_SLOP
    };
    let s = projection.scale();
    let mean = (s.x.abs() * s.y.abs()).sqrt();
    if mean > 0.0 { slop / mean } else { slop }
}

/// Returns the top-most annotation covering an image-space point within image-space slop.
#[must_use]
pub fn hit_test(annotations: &[Annotation], point: Point, slop: f32) -> Option<&Annotation> {
    hit_test_index(annotations, point, slop).map(|i| &annotations[i])
}

/// Returns the index of the top-most annotation covering an image-space point.
#[must_use]
pub fn hit_test_index(annotations: &[Annotation], point: Point, slop: f32) -> Option<usize> {
    // Iterate backward because later annotations are painted on top.
    annotations
        .iter()
        .enumerate()
        .rev()
        .find(|(_, a)| covers(a, point, slop))
        .map(|(i, _)| i)
}

/// Returns the id of the top-most annotation covering an image-space point.
#[must_use]
pub fn hit_test_id(annotations: &[Annotation], point: Point, slop: f32) -> Option<AnnotationId> {
    hit_test(annotations, point, slop).map(|a| a.id().clone())
}

/// Returns every covering annotation in top-most-first order.
#[must_use]
pub fn hit_test_all(annotations: &[Annotation], point: Point, slop: f32) -> Vec<&Annotation> {
    annotations
        .iter()
        .rev()
        .filter(|a| covers(a, point, slop))
        .collect()
}

/// Returns whether annotation ink covers the image-space point within `slop`.
#[must_use]
pub fn covers(annotation: &Annotation, point: Point, slop: f32) -> bool {
    let style = annotation.style();
    let band = band(style, slop);
    let geometry = annotation.geometry();

    if style.fill().is_some() && encloses(geometry, point) {
        return true;
    }

    outline_distance(geometry, point) <= band
}

// Hit band equals sanitized aiming slop plus half the stroke width.
fn band(style: StrokeStyle, slop: f32) -> f32 {
    let slop = if slop.is_finite() && slop >= 0.0 {
        slop
    } else {
        0.0
    };
    slop + style.width().max(0.0) / 2.0
}

// Only rectangle and ellipse geometry encloses an interior.
fn encloses(geometry: &ShapeGeometry, point: Point) -> bool {
    match geometry {
        ShapeGeometry::Rectangle(r) => {
            point.x >= r.x
                && point.x <= r.x + r.width
                && point.y >= r.y
                && point.y <= r.y + r.height
        }
        ShapeGeometry::Ellipse { center, radii } => {
            normalized_radius(*center, *radii, point) <= 1.0
        }
        // Freehand is an open polyline; closing it would incorrectly make its interior hittable.
        ShapeGeometry::Line { .. }
        | ShapeGeometry::Arrow { .. }
        | ShapeGeometry::Freehand { .. } => false,
    }
}

// Return unsigned distance to the stroked path in the geometry’s coordinate units.
fn outline_distance(geometry: &ShapeGeometry, point: Point) -> f32 {
    match geometry {
        ShapeGeometry::Rectangle(r) => {
            let corners = [
                Point::new(r.x, r.y),
                Point::new(r.x + r.width, r.y),
                Point::new(r.x + r.width, r.y + r.height),
                Point::new(r.x, r.y + r.height),
            ];
            (0..4)
                .map(|i| segment_distance(corners[i], corners[(i + 1) % 4], point))
                .fold(f32::INFINITY, f32::min)
        }
        ShapeGeometry::Ellipse { center, radii } => ellipse_distance(*center, *radii, point),
        // Arrow hit testing uses only the stored shaft; the rendered head has no separate hit geometry.
        ShapeGeometry::Line { from, to } | ShapeGeometry::Arrow { from, to } => {
            segment_distance(*from, *to, point)
        }
        ShapeGeometry::Freehand { points } => match points.len() {
            0 => f32::INFINITY,
            1 => distance(points[0], point),
            _ => points
                .windows(2)
                .map(|w| segment_distance(w[0], w[1], point))
                .fold(f32::INFINITY, f32::min),
        },
    }
}

// Degenerate segments use point distance instead of dividing by zero.
fn segment_distance(a: Point, b: Point, p: Point) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len_sq = dx * dx + dy * dy;
    if len_sq <= f32::EPSILON {
        return distance(a, p);
    }
    // Clamp projection to the segment so points beyond an endpoint use endpoint distance.
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len_sq).clamp(0.0, 1.0);
    distance(Point::new(a.x + t * dx, a.y + t * dy), p)
}

// Normalize each axis by its radius; an off-axis point is outside a zero-radius axis.
fn normalized_radius(center: Point, radii: iced_core::Vector, point: Point) -> f32 {
    let nx = norm_axis(point.x - center.x, radii.x.abs());
    let ny = norm_axis(point.y - center.y, radii.y.abs());
    (nx * nx + ny * ny).sqrt()
}

fn norm_axis(delta: f32, radius: f32) -> f32 {
    if radius > 0.0 {
        delta / radius
    } else if delta == 0.0 {
        0.0
    } else {
        f32::INFINITY
    }
}

// Use radial ellipse distance: exact for circles and conservative for ellipse hit testing, avoiding a quartic solve per pointer move.
fn ellipse_distance(center: Point, radii: iced_core::Vector, point: Point) -> f32 {
    let r = normalized_radius(center, radii, point);
    if !r.is_finite() {
        // A degenerate ellipse axis collapses the shape to a segment.
        let (rx, ry) = (radii.x.abs(), radii.y.abs());
        return segment_distance(
            Point::new(center.x - rx, center.y - ry),
            Point::new(center.x + rx, center.y + ry),
            point,
        );
    }
    if r <= f32::EPSILON {
        return radii.x.abs().min(radii.y.abs());
    }
    distance(center, point) * (r - 1.0).abs() / r
}

fn distance(a: Point, b: Point) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    (dx * dx + dy * dy).sqrt()
}
