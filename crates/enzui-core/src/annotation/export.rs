//! Shared path emission and renderer-neutral annotation export.
//!
//! `StrokePath` is the common flattened geometry for screen and raster output.
//! `RasterCanvas` stores unencoded RGBA pixels, and export pages use painted
//! bounds so stroke edges are not clipped.

use iced_core::{Color, Point, Rectangle, Size};

use super::{Annotation, ShapeGeometry};
use crate::image_space::ImageProjection;

/// Number of line segments used to flatten an ellipse for export.
pub const ELLIPSE_SEGMENTS: usize = 64;

/// Ordered display-space vertices forming an open or closed contour.
#[derive(Debug, Clone, PartialEq)]
pub struct SubPath {
    /// The vertices, in draw order.
    pub points: Vec<Point>,
    /// Whether the final vertex connects to the first.
    pub closed: bool,
}

impl SubPath {
    fn open(points: Vec<Point>) -> Self {
        Self {
            points,
            closed: false,
        }
    }

    fn closed(points: Vec<Point>) -> Self {
        Self {
            points,
            closed: true,
        }
    }
}

/// Display-space contours and optional fill region flattened from an annotation shape.
#[derive(Debug, Clone, PartialEq)]
pub struct StrokePath {
    /// Every contour that gets stroked.
    pub contours: Vec<SubPath>,
    /// Closed fill contour for rectangles and ellipses; `None` for open shapes.
    pub fill_region: Option<Vec<Point>>,
}

impl StrokePath {
    /// Flattens display-space geometry into contours, sizing arrow heads from the display stroke width.
    #[must_use]
    pub fn of(geometry: &ShapeGeometry, width: f32) -> Self {
        match geometry {
            ShapeGeometry::Rectangle(r) => {
                let pts = vec![
                    Point::new(r.x, r.y),
                    Point::new(r.x + r.width, r.y),
                    Point::new(r.x + r.width, r.y + r.height),
                    Point::new(r.x, r.y + r.height),
                ];
                Self {
                    contours: vec![SubPath::closed(pts.clone())],
                    fill_region: Some(pts),
                }
            }
            ShapeGeometry::Ellipse { center, radii } => {
                let pts: Vec<Point> = (0..ELLIPSE_SEGMENTS)
                    .map(|i| {
                        let t = i as f32 / ELLIPSE_SEGMENTS as f32 * std::f32::consts::TAU;
                        Point::new(center.x + radii.x * t.cos(), center.y + radii.y * t.sin())
                    })
                    .collect();
                Self {
                    contours: vec![SubPath::closed(pts.clone())],
                    fill_region: Some(pts),
                }
            }
            ShapeGeometry::Line { from, to } => Self {
                contours: vec![SubPath::open(vec![*from, *to])],
                fill_region: None,
            },
            ShapeGeometry::Arrow { from, to } => {
                let mut contours = vec![SubPath::open(vec![*from, *to])];
                // Size both arrow barbs from the display stroke width and place them at ±30° from the reversed shaft.
                let head = arrow_head_len(width);
                let (dx, dy) = (to.x - from.x, to.y - from.y);
                let len = (dx * dx + dy * dy).sqrt();
                if len > f32::EPSILON {
                    let (ux, uy) = (dx / len, dy / len);
                    for sign in [1.0_f32, -1.0] {
                        let (c, s) = (0.866_025_4_f32, 0.5_f32 * sign);
                        let rx = -ux * c - -uy * s;
                        let ry = -uy * c + -ux * s;
                        contours.push(SubPath::open(vec![
                            *to,
                            Point::new(to.x + rx * head, to.y + ry * head),
                        ]));
                    }
                }
                Self {
                    contours,
                    fill_region: None,
                }
            }
            ShapeGeometry::Freehand { points } => Self {
                contours: vec![SubPath::open(points.clone())],
                fill_region: None,
            },
        }
    }
}

/// Returns the display-space arrow-head length for a display stroke width.
#[must_use]
pub fn arrow_head_len(width: f32) -> f32 {
    (width * 3.0).max(6.0)
}

/// Native PDF markup subtype for an annotation, following PDF 32000-1 §12.5.6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfMarkupSubtype {
    /// `/Square` — a rectangle. PDF 32000-1 §12.5.6.8.
    Square,
    /// `/Circle` — an ellipse inscribed in the annotation rectangle. §12.5.6.8.
    Circle,
    /// `/Line` — a straight segment, optionally with a line ending. §12.5.6.7.
    Line,
    /// `/Ink` — one or more freehand paths. §12.5.6.13.
    Ink,
}

impl PdfMarkupSubtype {
    /// The literal PDF name, e.g. `"Square"`, written after `/Subtype /`.
    #[must_use]
    pub fn pdf_name(self) -> &'static str {
        match self {
            Self::Square => "Square",
            Self::Circle => "Circle",
            Self::Line => "Line",
            Self::Ink => "Ink",
        }
    }
}

/// Renderer-neutral data for a native PDF markup annotation in page coordinates.
///
/// `of` converts image coordinates, whose y axis grows downward, to PDF page
/// coordinates, whose y axis grows upward.
#[derive(Debug, Clone, PartialEq)]
pub struct PdfAnnotation {
    /// The markup subtype.
    pub subtype: PdfMarkupSubtype,
    /// PDF `/Rect` as normalized `[x0, y0, x1, y1]`, sized from painted bounds.
    pub rect: [f32; 4],
    /// The geometry payload, subtype-specific.
    pub geometry: PdfGeometry,
    /// Stroke colour as `/C` expects: RGB in `0.0..=1.0`.
    pub color: [f32; 3],
    /// Stroke width in page units, for the `/BS` border-style dictionary.
    pub border_width: f32,
}

/// The subtype-specific coordinate payload of a [`PdfAnnotation`], in page
/// coordinates (y up).
#[derive(Debug, Clone, PartialEq)]
pub enum PdfGeometry {
    /// `/Square` or `/Circle`, whose geometry is fully represented by `/Rect`.
    Rect,
    /// `/Line` endpoints, with an optional line-ending arrow head.
    Line {
        /// The `/L` endpoints, `[x0, y0, x1, y1]`, in page coordinates.
        l: [f32; 4],
        /// Whether PDF draws an arrow head at the line tip through `/LE`.
        arrow: bool,
    },
    /// `/InkList` containing one interleaved coordinate path.
    Ink {
        /// The single ink path as interleaved page coordinates,
        /// `[x0, y0, x1, y1, …]`.
        path: Vec<f32>,
    },
}

impl PdfAnnotation {
    /// Converts an image-space annotation into PDF page-space dictionary data.
    ///
    /// `page_height` defines the y-axis flip, and `/Rect` uses painted bounds.
    #[must_use]
    pub fn of(annotation: &Annotation, page_height: f32) -> Self {
        // y up: a page-space y is `page_height - image_y`.
        let fy = |y: f32| page_height - y;
        let pb = annotation.painted_bounds();
        // /Rect normalized so y0 < y1 after the flip.
        let (ry0, ry1) = (fy(pb.y + pb.height), fy(pb.y));
        let rect = [pb.x, ry0, pb.x + pb.width, ry1];
        let color = {
            let c = annotation.style().color();
            [c.r, c.g, c.b]
        };
        let (subtype, geometry) = match annotation.geometry() {
            ShapeGeometry::Rectangle(_) => (PdfMarkupSubtype::Square, PdfGeometry::Rect),
            ShapeGeometry::Ellipse { .. } => (PdfMarkupSubtype::Circle, PdfGeometry::Rect),
            ShapeGeometry::Line { from, to } => (
                PdfMarkupSubtype::Line,
                PdfGeometry::Line {
                    l: [from.x, fy(from.y), to.x, fy(to.y)],
                    arrow: false,
                },
            ),
            ShapeGeometry::Arrow { from, to } => (
                PdfMarkupSubtype::Line,
                PdfGeometry::Line {
                    l: [from.x, fy(from.y), to.x, fy(to.y)],
                    arrow: true,
                },
            ),
            ShapeGeometry::Freehand { points } => {
                let mut path = Vec::with_capacity(points.len() * 2);
                for p in points {
                    path.push(p.x);
                    path.push(fy(p.y));
                }
                (PdfMarkupSubtype::Ink, PdfGeometry::Ink { path })
            }
        };
        Self {
            subtype,
            rect,
            geometry,
            color,
            border_width: annotation.style().width(),
        }
    }
}

/// Row-major straight-alpha RGBA pixels without image-container encoding.
#[derive(Debug, Clone, PartialEq)]
pub struct RasterCanvas {
    width: u32,
    height: u32,
    // Row-major straight-alpha RGBA bytes.
    pixels: Vec<u8>,
}

/// Maximum RGBA allocation accepted by `RasterCanvas::try_new`, in bytes.
pub const MAX_CANVAS_BYTES: usize = 512 * 1024 * 1024;

/// Reports a canvas request whose byte size overflows or exceeds `MAX_CANVAS_BYTES`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanvasTooLarge {
    /// The requested width, in pixels.
    pub width: u32,
    /// The requested height, in pixels.
    pub height: u32,
}

impl core::fmt::Display for CanvasTooLarge {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "raster canvas {}x{} exceeds the {}-byte ceiling",
            self.width, self.height, MAX_CANVAS_BYTES
        )
    }
}

impl std::error::Error for CanvasTooLarge {}

/// Reason an annotation export produced no pixels.
///
/// Fallible export functions return this type; their `Option` counterparts map
/// every refusal to `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportRefusal {
    /// The annotation set was empty, so there was nothing to draw.
    NothingToDraw,
    /// A scale or page dimension was non-finite, or the scale was not positive.
    NonFiniteGeometry,
    /// The page byte size overflowed or exceeded `MAX_CANVAS_BYTES`; unrepresentable dimensions are reported as `u32::MAX`.
    CanvasTooLarge(CanvasTooLarge),
}

impl core::fmt::Display for ExportRefusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NothingToDraw => write!(f, "no annotations to export"),
            Self::NonFiniteGeometry => {
                write!(f, "a non-finite or non-positive export dimension")
            }
            Self::CanvasTooLarge(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ExportRefusal {}

impl From<CanvasTooLarge> for ExportRefusal {
    fn from(e: CanvasTooLarge) -> Self {
        Self::CanvasTooLarge(e)
    }
}

// Compute checked RGBA bytes before allocation; zero dimensions follow the constructor’s one-pixel clamp.
fn canvas_byte_budget(width: u32, height: u32) -> Result<usize, CanvasTooLarge> {
    let w = width.max(1) as usize;
    let h = height.max(1) as usize;
    w.checked_mul(h)
        .and_then(|pixels| pixels.checked_mul(4))
        .filter(|&bytes| bytes <= MAX_CANVAS_BYTES)
        .ok_or(CanvasTooLarge { width, height })
}

impl RasterCanvas {
    /// Allocates a transparent RGBA canvas after clamping zero dimensions to one.
    ///
    /// # Errors
    ///
    /// Returns `CanvasTooLarge` if `width * height * 4` overflows or exceeds
    /// `MAX_CANVAS_BYTES`.
    pub fn try_new(width: u32, height: u32) -> Result<Self, CanvasTooLarge> {
        let bytes = canvas_byte_budget(width, height)?;
        let w = width.max(1);
        let h = height.max(1);
        Ok(Self {
            width: w,
            height: h,
            pixels: vec![0; bytes],
        })
    }

    /// Creates a transparent canvas, clamping each zero dimension to one and falling back to `1x1` when allocation is refused.
    ///
    /// Use `try_new` when allocation refusal must be distinguished from zero-dimension clamping.
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: a 1x1 canvas is within every ceiling"
    )]
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self::try_new(width, height)
            .unwrap_or_else(|_| Self::try_new(1, 1).expect("1x1 is within the ceiling"))
    }

    /// Width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The raw RGBA bytes, ready to hand to a PNG encoder.
    #[must_use]
    pub fn as_rgba(&self) -> &[u8] {
        &self.pixels
    }

    /// Consume the canvas, returning its RGBA bytes.
    #[must_use]
    pub fn into_rgba(self) -> Vec<u8> {
        self.pixels
    }

    /// The straight-alpha RGBA of pixel `(x, y)`, or `None` if out of bounds.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        Some([
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ])
    }

    // Drop out-of-bounds writes so stroke overlap clips at the page edge instead of wrapping.
    fn blend(&mut self, x: i64, y: i64, color: Color) {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return;
        }
        let i = ((y as usize) * (self.width as usize) + (x as usize)) * 4;
        let sa = color.a.clamp(0.0, 1.0);
        if sa <= 0.0 {
            return;
        }
        let src = [color.r, color.g, color.b];
        let da = f32::from(self.pixels[i + 3]) / 255.0;
        let out_a = sa + da * (1.0 - sa);
        for (c, &s) in src.iter().enumerate() {
            let d = f32::from(self.pixels[i + c]) / 255.0;
            let out = if out_a > 0.0 {
                (s * sa + d * da * (1.0 - sa)) / out_a
            } else {
                0.0
            };
            self.pixels[i + c] = (out.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
        self.pixels[i + 3] = (out_a.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
}

/// Exports image-space annotations to a tight RGBA raster at `scale` pixels per image pixel.
///
/// Returns `None` for empty input, invalid scale, or a page exceeding the canvas
/// byte limit.
#[must_use]
pub fn export_annotations(annotations: &[Annotation], scale: f32) -> Option<RasterCanvas> {
    try_export_annotations(annotations, scale).ok()
}

/// Fallible form of `export_annotations`.
///
/// # Errors
///
/// Returns the applicable `ExportRefusal`; `export_annotations` maps it to `None`.
pub fn try_export_annotations(
    annotations: &[Annotation],
    scale: f32,
) -> Result<RasterCanvas, ExportRefusal> {
    if annotations.is_empty() {
        return Err(ExportRefusal::NothingToDraw);
    }
    let (page, projection) =
        export_page(annotations, scale).ok_or(ExportRefusal::NonFiniteGeometry)?;
    try_render_annotations(annotations, projection, page)
}

/// Renders image-space annotations through `projection` into a page-sized RGBA raster.
///
/// Returns `None` for empty input, non-finite page dimensions, or a page beyond
/// `MAX_CANVAS_BYTES`.
#[must_use]
pub fn render_annotations(
    annotations: &[Annotation],
    projection: ImageProjection,
    page: Size,
) -> Option<RasterCanvas> {
    try_render_annotations(annotations, projection, page).ok()
}

/// Fallible form of `render_annotations`.
///
/// Per-segment raster work is bounded by page geometry rather than the magnitude
/// of off-page projected coordinates.
///
/// # Errors
///
/// Returns the applicable `ExportRefusal`.
pub fn try_render_annotations(
    annotations: &[Annotation],
    projection: ImageProjection,
    page: Size,
) -> Result<RasterCanvas, ExportRefusal> {
    if annotations.is_empty() {
        return Err(ExportRefusal::NothingToDraw);
    }
    // Reject non-finite dimensions before float-to-integer conversion can saturate them.
    if !(page.width.is_finite() && page.height.is_finite()) {
        return Err(ExportRefusal::NonFiniteGeometry);
    }
    let w = page.width.ceil().max(1.0);
    let h = page.height.ceil().max(1.0);
    // Refuse dimensions beyond `u32` representation and report saturated dimensions in the error.
    if w > u32::MAX as f32 || h > u32::MAX as f32 {
        return Err(ExportRefusal::CanvasTooLarge(CanvasTooLarge {
            width: w as u32,
            height: h as u32,
        }));
    }
    let mut canvas = RasterCanvas::try_new(w as u32, h as u32)?;

    for annotation in annotations {
        let shape = annotation.to_display(projection);
        let path = StrokePath::of(&shape.geometry, shape.width);
        if let (Some(region), Some(fill)) = (&path.fill_region, shape.fill) {
            fill_polygon(&mut canvas, region, fill);
        }
        for contour in &path.contours {
            stroke_polyline(
                &mut canvas,
                &contour.points,
                contour.closed,
                shape.width,
                shape.color,
            );
        }
    }
    Ok(canvas)
}

/// Computes a tight painted page and projection for the annotations.
///
/// `scale` is display pixels per image pixel. Returns `None` for empty input or
/// a non-positive or non-finite scale.
#[must_use]
pub fn export_page(annotations: &[Annotation], scale: f32) -> Option<(Size, ImageProjection)> {
    if annotations.is_empty() || !(scale.is_finite() && scale > 0.0) {
        return None;
    }
    let bounds = combined_painted_bounds(annotations)?;
    // Scale painted bounds and offset their top-left to the page origin.
    let page = Size::new(
        (bounds.width * scale).ceil().max(1.0),
        (bounds.height * scale).ceil().max(1.0),
    );
    let origin = Point::new(-bounds.x * scale, -bounds.y * scale);
    let projection = ImageProjection::from_origin_scale(origin, scale)?;
    Some((page, projection))
}

/// The union of every annotation's [`painted_bounds`](Annotation::painted_bounds),
/// in image space. [`None`] for an empty set.
#[must_use]
pub fn combined_painted_bounds(annotations: &[Annotation]) -> Option<Rectangle> {
    let mut it = annotations.iter();
    let first = it.next()?.painted_bounds();
    let (mut min_x, mut min_y) = (first.x, first.y);
    let (mut max_x, mut max_y) = (first.x + first.width, first.y + first.height);
    for a in it {
        let b = a.painted_bounds();
        min_x = min_x.min(b.x);
        min_y = min_y.min(b.y);
        max_x = max_x.max(b.x + b.width);
        max_y = max_y.max(b.y + b.height);
    }
    Some(Rectangle {
        x: min_x,
        y: min_y,
        width: max_x - min_x,
        height: max_y - min_y,
    })
}

// Stroke each segment with a disc brush whose radius is half the display width.
fn stroke_polyline(
    canvas: &mut RasterCanvas,
    points: &[Point],
    closed: bool,
    width: f32,
    color: Color,
) {
    if points.is_empty() {
        return;
    }
    let radius = (width / 2.0).max(0.5);
    let segments = points.windows(2);
    for seg in segments {
        stamp_segment(canvas, seg[0], seg[1], radius, color);
    }
    if closed && points.len() >= 2 {
        stamp_segment(canvas, points[points.len() - 1], points[0], radius, color);
    }
    if points.len() == 1 {
        stamp_disc(canvas, points[0], radius, color);
    }
}

#[cfg(test)]
mod work {
    use std::cell::Cell;

    thread_local! {
        static STEPS: Cell<u64> = const { Cell::new(0) };
    }

    pub(super) fn step() {
        STEPS.with(|c| c.set(c.get().saturating_add(1)));
    }
}

#[derive(Clone, Copy)]
struct P64 {
    x: f64,
    y: f64,
}

fn outcode(x: f64, y: f64, lo_x: f64, lo_y: f64, hi_x: f64, hi_y: f64) -> u8 {
    let mut code = 0u8;
    if x < lo_x {
        code |= 1;
    } else if x > hi_x {
        code |= 2;
    }
    if y < lo_y {
        code |= 4;
    } else if y > hi_y {
        code |= 8;
    }
    code
}

// Clip in `f64` screen space against the radius-inflated canvas, using homogeneous line intersections to avoid overflow and cancellation at extreme coordinates.
fn clip_segment_to_canvas(
    canvas: &RasterCanvas,
    a: Point,
    b: Point,
    radius: f32,
) -> Option<(P64, P64)> {
    if !(a.x.is_finite() && a.y.is_finite() && b.x.is_finite() && b.y.is_finite()) {
        return None;
    }
    let r = f64::from(radius).max(0.5);
    let (lo_x, lo_y) = (-r, -r);
    let (hi_x, hi_y) = (
        f64::from(canvas.width()) + r,
        f64::from(canvas.height()) + r,
    );
    // `f64` represents products of two `f32` significands exactly.
    let (ax0, ay0) = (f64::from(a.x), f64::from(a.y));
    let (bx0, by0) = (f64::from(b.x), f64::from(b.y));
    // Solve edge crossings from `A*x + B*y = C` instead of interpolating huge endpoints.
    let a_coef = by0 - ay0;
    let b_coef = ax0 - bx0;
    let c_coef = ax0 * by0 - bx0 * ay0;
    let (mut ax, mut ay) = (ax0, ay0);
    let (mut bx, mut by) = (bx0, by0);
    let mut code_a = outcode(ax, ay, lo_x, lo_y, hi_x, hi_y);
    let mut code_b = outcode(bx, by, lo_x, lo_y, hi_x, hi_y);
    // Eight iterations cover the four-edge clip and bound non-finite failure paths.
    for _ in 0..8 {
        if code_a | code_b == 0 {
            if !(ax.is_finite() && ay.is_finite() && bx.is_finite() && by.is_finite()) {
                return None;
            }
            return Some((P64 { x: ax, y: ay }, P64 { x: bx, y: by }));
        }
        if code_a & code_b != 0 {
            return None;
        }
        let out = if code_a != 0 { code_a } else { code_b };
        // Pin the binding coordinate to the page edge and solve the other from the homogeneous line equation.
        let (x, y) = if out & 8 != 0 {
            ((c_coef - b_coef * hi_y) / a_coef, hi_y)
        } else if out & 4 != 0 {
            ((c_coef - b_coef * lo_y) / a_coef, lo_y)
        } else if out & 2 != 0 {
            (hi_x, (c_coef - a_coef * hi_x) / b_coef)
        } else {
            (lo_x, (c_coef - a_coef * lo_x) / b_coef)
        };
        if !(x.is_finite() && y.is_finite()) {
            return None;
        }
        if out == code_a {
            ax = x;
            ay = y;
            code_a = outcode(ax, ay, lo_x, lo_y, hi_x, hi_y);
        } else {
            bx = x;
            by = y;
            code_b = outcode(bx, by, lo_x, lo_y, hi_x, hi_y);
        }
    }
    None
}

// Clipped endpoints and dab count are bounded by the radius-inflated canvas.
struct SegmentPlan {
    p0: P64,
    p1: P64,
    n: i64,
}

// Plan dabs from clipped `f64` endpoints, rejecting non-finite values and capping the loop by page geometry.
fn plan_segment_dabs(
    canvas: &RasterCanvas,
    a: Point,
    b: Point,
    radius: f32,
) -> Option<SegmentPlan> {
    let (p0, p1) = clip_segment_to_canvas(canvas, a, b, radius)?;
    let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
    let len = (dx * dx + dy * dy).sqrt();
    let step = (f64::from(radius) * 0.5).max(0.5);
    let planned = (len / step).ceil();
    // Reject non-finite length or count before converting to the loop-bound integer.
    if !(len.is_finite() && planned.is_finite()) {
        return None;
    }
    // Cap dabs by the inflated canvas diagonal because no visible run can be longer.
    let r = f64::from(radius).max(0.5);
    let (w, h) = (
        f64::from(canvas.width()) + 2.0 * r,
        f64::from(canvas.height()) + 2.0 * r,
    );
    let n_cap = ((w * w + h * h).sqrt() / step).ceil() as i64 + 2;
    let n = (planned.max(1.0) as i64).clamp(1, n_cap);
    Some(SegmentPlan { p0, p1, n })
}

// Sample only the clipped visible segment so dab work is bounded by the canvas.
fn stamp_segment(canvas: &mut RasterCanvas, a: Point, b: Point, radius: f32, color: Color) {
    let Some(plan) = plan_segment_dabs(canvas, a, b, radius) else {
        return;
    };
    let SegmentPlan { p0, p1, n } = plan;
    let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
    for i in 0..=n {
        #[cfg(test)]
        work::step();
        let t = i as f64 / n as f64;
        stamp_disc(
            canvas,
            Point::new((p0.x + dx * t) as f32, (p0.y + dy * t) as f32),
            radius,
            color,
        );
    }
}

// Intersect the brush scan box with the canvas before iterating.
fn stamp_disc(canvas: &mut RasterCanvas, c: Point, radius: f32, color: Color) {
    if !(c.x.is_finite() && c.y.is_finite() && radius.is_finite()) {
        return;
    }
    let r = radius.max(0.5);
    let (last_x, last_y) = (
        i64::from(canvas.width()) - 1,
        i64::from(canvas.height()) - 1,
    );
    // `as i64` saturates on overflow, so a huge coordinate clamps rather than
    // wrapping into the canvas from the far side.
    let x0 = ((c.x - r).floor() as i64).max(0);
    let x1 = ((c.x + r).ceil() as i64).min(last_x);
    let y0 = ((c.y - r).floor() as i64).max(0);
    let y1 = ((c.y + r).ceil() as i64).min(last_y);
    let r2 = r * r;
    for y in y0..=y1 {
        for x in x0..=x1 {
            #[cfg(test)]
            work::step();
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let (ddx, ddy) = (px - c.x, py - c.y);
            if ddx * ddx + ddy * ddy <= r2 {
                canvas.blend(x, y, color);
            }
        }
    }
}

fn fill_polygon(canvas: &mut RasterCanvas, points: &[Point], color: Color) {
    if points.len() < 3 {
        return;
    }
    let (last_x, last_y) = (
        i64::from(canvas.width()) - 1,
        i64::from(canvas.height()) - 1,
    );
    // Clamp scanlines to the canvas before iteration so off-page coordinates cannot increase raster work.
    let min_y = (points
        .iter()
        .map(|p| p.y)
        .fold(f32::INFINITY, f32::min)
        .floor() as i64)
        .max(0);
    let max_y = (points
        .iter()
        .map(|p| p.y)
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil() as i64)
        .min(last_y);
    for y in min_y..=max_y {
        #[cfg(test)]
        work::step();
        let sy = y as f32 + 0.5;
        let mut xs: Vec<f32> = Vec::new();
        for i in 0..points.len() {
            let a = points[i];
            let b = points[(i + 1) % points.len()];
            let (y0, y1) = (a.y, b.y);
            // A scanline crosses edge a-b iff sy is between the endpoints,
            // half-open so a vertex is counted once.
            if (y0 <= sy && sy < y1) || (y1 <= sy && sy < y0) {
                let t = (sy - y0) / (y1 - y0);
                xs.push(a.x + t * (b.x - a.x));
            }
        }
        xs.sort_by(|p, q| p.partial_cmp(q).unwrap_or(std::cmp::Ordering::Equal));
        let mut i = 0;
        while i + 1 < xs.len() {
            // Clamp each crossing pair to the page before blending the span.
            let x0 = (xs[i].ceil() as i64).max(0);
            let x1 = (xs[i + 1].floor() as i64).min(last_x);
            for x in x0..=x1 {
                #[cfg(test)]
                work::step();
                canvas.blend(x, y, color);
            }
            i += 2;
        }
    }
}
