//! Maps points between image and display coordinates for fitted, zoomed, and panned
//! images.

use iced_core::{ContentFit, Point, Rectangle, Size, Vector};

use crate::zoom_pan::{ViewTransform, ViewportGeometry};

/// A resolved image placement for converting between image and display coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImageProjection {
    origin: Point,
    scale: Vector,
}

impl ImageProjection {
    /// Resolves the placement of `image` inside `viewport` under `fit` and `transform`.
    ///
    /// Returns `None` when the inputs cannot produce a finite, positive-area projection.
    #[must_use]
    pub fn new(
        viewport: Rectangle,
        image: Size,
        fit: ContentFit,
        transform: ViewTransform,
    ) -> Option<Self> {
        if !finite_point(viewport.position()) {
            return None;
        }
        let geometry = ViewportGeometry::new(image, viewport.size(), fit);
        Self::from_geometry(viewport.position(), geometry, transform)
    }

    /// Resolves placement from an already-computed [`ViewportGeometry`].
    ///
    /// Returns `None` when the origin or geometry cannot produce a finite, positive-area projection.
    #[must_use]
    pub fn from_geometry(
        origin: Point,
        geometry: ViewportGeometry,
        transform: ViewTransform,
    ) -> Option<Self> {
        if !finite_point(origin) || geometry.is_empty() || !has_area(geometry.image) {
            return None;
        }

        let scaled = geometry.scaled(transform.scale());
        if !has_area(scaled) {
            return None;
        }

        let offset = transform.offset();
        let placement = Point::new(
            origin.x + (geometry.bounds.width - scaled.width) / 2.0 - offset.x,
            origin.y + (geometry.bounds.height - scaled.height) / 2.0 - offset.y,
        );
        if !finite_point(placement) {
            return None;
        }

        // Compute scale per axis because `ContentFit::Fill` may use different horizontal and
        // vertical ratios.
        let scale = Vector::new(
            scaled.width / geometry.image.width,
            scaled.height / geometry.image.height,
        );
        if !(scale.x.is_finite() && scale.y.is_finite() && scale.x > 0.0 && scale.y > 0.0) {
            return None;
        }

        Some(Self {
            origin: placement,
            scale,
        })
    }

    /// Constructs an isotropic projection from an explicit display origin and scale.
    ///
    /// Returns `None` for a non-finite origin or a non-positive or non-finite scale.
    #[must_use]
    pub fn from_origin_scale(origin: Point, scale: f32) -> Option<Self> {
        if !(finite_point(origin) && scale.is_finite() && scale > 0.0) {
            return None;
        }
        Some(Self {
            origin,
            scale: Vector::new(scale, scale),
        })
    }

    /// Where image pixel `point` is drawn, in display coordinates.
    #[must_use]
    pub fn image_to_display(self, point: Point) -> Point {
        Point::new(
            self.origin.x + point.x * self.scale.x,
            self.origin.y + point.y * self.scale.y,
        )
    }

    /// Maps a display point to image coordinates without clamping it to the image bounds.
    #[must_use]
    pub fn display_to_image(self, point: Point) -> Point {
        Point::new(
            (point.x - self.origin.x) / self.scale.x,
            (point.y - self.origin.y) / self.scale.y,
        )
    }

    /// Display-space position of image pixel `(0, 0)`: the top-left corner of the drawn
    /// picture.
    #[must_use]
    pub fn origin(self) -> Point {
        self.origin
    }

    /// Display pixels per image pixel, per axis.
    #[must_use]
    pub fn scale(self) -> Vector {
        self.scale
    }

    /// The rectangle the whole image occupies in display coordinates.
    #[must_use]
    pub fn image_rect(self, image: Size) -> Rectangle {
        let far = self.image_to_display(Point::new(image.width, image.height));
        Rectangle {
            x: self.origin.x,
            y: self.origin.y,
            width: far.x - self.origin.x,
            height: far.y - self.origin.y,
        }
    }

    /// Returns whether `point` lies within the half-open image bounds `[0, width) × [0, height)`.
    #[must_use]
    pub fn contains(self, point: Point, image: Size) -> bool {
        point.x >= 0.0 && point.y >= 0.0 && point.x < image.width && point.y < image.height
    }
}

/// Maps an image point to display coordinates, returning `None` for degenerate geometry.
#[must_use]
pub fn image_to_display(
    point: Point,
    viewport: Rectangle,
    image: Size,
    fit: ContentFit,
    transform: ViewTransform,
) -> Option<Point> {
    ImageProjection::new(viewport, image, fit, transform).map(|p| p.image_to_display(point))
}

/// Maps one display point to image coordinates, returning `None` for degenerate
/// geometry.
#[must_use]
pub fn display_to_image(
    point: Point,
    viewport: Rectangle,
    image: Size,
    fit: ContentFit,
    transform: ViewTransform,
) -> Option<Point> {
    ImageProjection::new(viewport, image, fit, transform).map(|p| p.display_to_image(point))
}

/// Returns the viewport-visible image rectangle, clamped to image bounds.
///
/// Degenerate geometry returns `None`; a fully off-screen image yields a zero-area
/// rectangle.
#[must_use]
pub fn visible_image_rect(
    viewport: Rectangle,
    image: Size,
    fit: ContentFit,
    transform: ViewTransform,
) -> Option<Rectangle> {
    let projection = ImageProjection::new(viewport, image, fit, transform)?;

    let top_left = projection.display_to_image(viewport.position());
    let bottom_right = projection.display_to_image(Point::new(
        viewport.x + viewport.width,
        viewport.y + viewport.height,
    ));

    let x0 = top_left.x.clamp(0.0, image.width);
    let y0 = top_left.y.clamp(0.0, image.height);
    let x1 = bottom_right.x.clamp(0.0, image.width);
    let y1 = bottom_right.y.clamp(0.0, image.height);

    Some(Rectangle {
        x: x0,
        y: y0,
        width: (x1 - x0).max(0.0),
        height: (y1 - y0).max(0.0),
    })
}

fn has_area(size: Size) -> bool {
    size.width.is_finite() && size.height.is_finite() && size.width > 0.0 && size.height > 0.0
}

fn finite_point(p: Point) -> bool {
    p.x.is_finite() && p.y.is_finite()
}
