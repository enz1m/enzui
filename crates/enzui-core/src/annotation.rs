//! Renderer-neutral annotation geometry in image coordinates.
//!
//! Geometry and stroke widths use document space and are projected only for display. Geometry convenience constructors reject non-finite coordinates, while [`AnnotationIds`] derives widget identities from caller-supplied document namespaces and disambiguated keys.

use iced_core::{Color, Point, Rectangle, Vector, widget::Id};

use crate::identity::{KeyedIds, OwnerId};
use crate::image_space::ImageProjection;

pub mod capture;
pub mod export;
pub mod history;
pub mod hit;
pub mod ink;

/// Stable annotation identity with a document-local key and a namespaced widget id.
///
/// Equality and hashing use the widget id.
#[derive(Debug, Clone)]
pub struct AnnotationId {
    key: String,
    id: Id,
}

impl AnnotationId {
    /// Returns the document-local reconciliation key.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Returns the routable widget id derived from the document prefix and disambiguated key.
    ///
    /// Separate live documents must use distinct prefixes to avoid collisions.
    #[must_use]
    pub fn widget_id(&self) -> &Id {
        &self.id
    }
}

impl PartialEq for AnnotationId {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for AnnotationId {}

impl std::hash::Hash for AnnotationId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// Mints annotation identities within one document namespace.
///
/// Use [`Self::keyed`] for restored keys and [`Self::mint`] for new annotations; both paths disambiguate collisions.
#[derive(Debug, Clone)]
pub struct AnnotationIds {
    owner: OwnerId,
    ids: KeyedIds,
    minted: u64,
}

const ANNOTATION_KIND: &str = "enz-annotation";

impl AnnotationIds {
    /// Creates a deriver for a stable document prefix, such as a record id or file path.
    pub fn new(prefix: impl Into<String>) -> Self {
        let prefix = prefix.into();
        Self {
            owner: OwnerId::of_kind(ANNOTATION_KIND, &prefix),
            ids: KeyedIds::new(prefix),
            minted: 0,
        }
    }

    /// The prefix this deriver namespaces with.
    #[must_use]
    pub fn prefix(&self) -> &str {
        self.ids.prefix()
    }

    /// The injective owner namespace this deriver mints ids under.
    #[must_use]
    pub fn owner(&self) -> &OwnerId {
        &self.owner
    }

    /// Creates an identity for a caller-supplied stable key.
    ///
    /// Repeated keys are disambiguated by occurrence.
    pub fn keyed(&mut self, key: &str) -> AnnotationId {
        // `key_for` and `id_for` both advance the occurrence counter, so the
        // key is taken once and the `Id` derived from it — the pattern
        // `KeyedIds::key_for` documents.
        let key = self.ids.key_for(key);
        let id = self.owner.id(&key);
        AnnotationId { key, id }
    }

    /// Mints a new identity using a monotonic `n{number}` key.
    ///
    /// Generated keys are disambiguated against caller-supplied keys.
    pub fn mint(&mut self) -> AnnotationId {
        self.minted += 1;
        let generated = format!("n{}", self.minted);
        self.keyed(&generated)
    }

    /// How many ids [`mint`](Self::mint) has generated.
    #[must_use]
    pub fn minted(&self) -> u64 {
        self.minted
    }

    /// Reserves one occurrence of `key` so later keyed or minted identities cannot reuse it.
    ///
    /// Call this when a fresh deriver adopts identities created by an earlier deriver.
    pub fn reserve(&mut self, key: &str) -> String {
        self.ids.key_for(key)
    }

    /// Reserves the keys of all supplied annotations.
    pub fn reserve_existing<'a>(&mut self, annotations: impl IntoIterator<Item = &'a Annotation>) {
        for a in annotations {
            self.reserve(a.id().key());
        }
    }
}

/// Identifies an annotation shape without its geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShapeKind {
    /// An axis-aligned rectangle.
    Rectangle,
    /// An axis-aligned ellipse.
    Ellipse,
    /// A straight segment.
    Line,
    /// Arrow geometry identified by a tail and tip.
    Arrow,
    /// A sampled freehand polyline.
    Freehand,
}

impl ShapeKind {
    /// A canonical lowercase name for export formats that name annotation types.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Rectangle => "rectangle",
            Self::Ellipse => "ellipse",
            Self::Line => "line",
            Self::Arrow => "arrow",
            Self::Freehand => "freehand",
        }
    }
    /// A translated shape name for display and accessibility.
    #[must_use]
    pub fn localized_name(self) -> String {
        crate::localization::text(match self {
            Self::Rectangle => "enzui-annotation-rectangle",
            Self::Ellipse => "enzui-annotation-ellipse",
            Self::Line => "enzui-annotation-line",
            Self::Arrow => "enzui-annotation-arrow",
            Self::Freehand => "enzui-annotation-freehand",
        })
    }
}

/// Annotation geometry in image coordinates.
///
/// The convenience constructors reject non-finite input; callers constructing variants directly must preserve that invariant.
#[derive(Debug, Clone, PartialEq)]
pub enum ShapeGeometry {
    /// An axis-aligned rectangle; directly constructed values must have non-negative extents.
    Rectangle(Rectangle),
    /// An axis-aligned ellipse: a centre and a per-axis radius.
    Ellipse {
        /// Centre, in image coordinates.
        center: Point,
        /// Per-axis radii in image units; directly constructed values must be non-negative.
        radii: Vector,
    },
    /// A straight segment from `from` to `to`.
    Line {
        /// Start, in image coordinates.
        from: Point,
        /// End, in image coordinates.
        to: Point,
    },
    /// A shaft from `from` to `to`; a nondegenerate arrowhead is generated at `to` rather than stored in the geometry.
    Arrow {
        /// Tail, in image coordinates.
        from: Point,
        /// Tip, in image coordinates.
        to: Point,
    },
    /// A sampled freehand polyline in image coordinates, typically produced by [`ink::StrokeSampler`].
    Freehand {
        /// Points in draw order; directly constructed values must contain at least one finite point.
        points: Vec<Point>,
    },
}

impl ShapeGeometry {
    /// Creates the normalized rectangle spanning two corners.
    ///
    /// Returns `None` if either corner is non-finite.
    #[must_use]
    pub fn rectangle_from_corners(a: Point, b: Point) -> Option<Self> {
        if !(finite(a) && finite(b)) {
            return None;
        }
        Some(Self::Rectangle(Rectangle {
            x: a.x.min(b.x),
            y: a.y.min(b.y),
            width: (b.x - a.x).abs(),
            height: (b.y - a.y).abs(),
        }))
    }

    /// Creates an ellipse inscribed in the rectangle spanning two corners.
    ///
    /// Returns `None` if either corner is non-finite.
    #[must_use]
    pub fn ellipse_from_corners(a: Point, b: Point) -> Option<Self> {
        let Some(Self::Rectangle(r)) = Self::rectangle_from_corners(a, b) else {
            return None;
        };
        Some(Self::Ellipse {
            center: Point::new(r.x + r.width / 2.0, r.y + r.height / 2.0),
            radii: Vector::new(r.width / 2.0, r.height / 2.0),
        })
    }

    /// A straight segment. Returns [`None`] if either end is non-finite.
    #[must_use]
    pub fn line(from: Point, to: Point) -> Option<Self> {
        (finite(from) && finite(to)).then_some(Self::Line { from, to })
    }

    /// An arrow from `from`, tipped at `to`. Returns [`None`] if either end is
    /// non-finite.
    #[must_use]
    pub fn arrow(from: Point, to: Point) -> Option<Self> {
        (finite(from) && finite(to)).then_some(Self::Arrow { from, to })
    }

    /// Creates a freehand polyline from sampled points.
    ///
    /// Returns `None` for an empty path or any non-finite point.
    #[must_use]
    pub fn freehand(points: Vec<Point>) -> Option<Self> {
        (!points.is_empty() && points.iter().copied().all(finite))
            .then_some(Self::Freehand { points })
    }

    /// This geometry's [`ShapeKind`].
    #[must_use]
    pub fn kind(&self) -> ShapeKind {
        match self {
            Self::Rectangle(_) => ShapeKind::Rectangle,
            Self::Ellipse { .. } => ShapeKind::Ellipse,
            Self::Line { .. } => ShapeKind::Line,
            Self::Arrow { .. } => ShapeKind::Arrow,
            Self::Freehand { .. } => ShapeKind::Freehand,
        }
    }

    /// Returns the tight geometry bounds in the current coordinate space, excluding stroke width.
    ///
    /// Use [`Annotation::painted_bounds`] for the ink’s full extent.
    #[must_use]
    pub fn bounds(&self) -> Rectangle {
        match self {
            Self::Rectangle(r) => *r,
            Self::Ellipse { center, radii } => Rectangle {
                x: center.x - radii.x,
                y: center.y - radii.y,
                width: radii.x * 2.0,
                height: radii.y * 2.0,
            },
            Self::Line { from, to } | Self::Arrow { from, to } => {
                bbox([*from, *to].iter().copied())
            }
            Self::Freehand { points } => bbox(points.iter().copied()),
        }
    }

    /// Projects this geometry into display coordinates.
    #[must_use]
    pub fn project(&self, projection: ImageProjection) -> Self {
        self.map_points(
            |p| projection.image_to_display(p),
            |r| {
                Vector::new(
                    r.x * projection.scale().x.abs(),
                    r.y * projection.scale().y.abs(),
                )
            },
        )
    }

    /// Converts display-space geometry back into image coordinates.
    #[must_use]
    pub fn unproject(&self, projection: ImageProjection) -> Self {
        self.map_points(
            |p| projection.display_to_image(p),
            |r| {
                Vector::new(
                    r.x / projection.scale().x.abs(),
                    r.y / projection.scale().y.abs(),
                )
            },
        )
    }

    // Re-normalize mapped rectangle corners so axis-flipping transforms cannot create negative extents.
    fn map_points(&self, point: impl Fn(Point) -> Point, radii: impl Fn(Vector) -> Vector) -> Self {
        match self {
            Self::Rectangle(r) => {
                let a = point(Point::new(r.x, r.y));
                let b = point(Point::new(r.x + r.width, r.y + r.height));
                Self::Rectangle(Rectangle {
                    x: a.x.min(b.x),
                    y: a.y.min(b.y),
                    width: (b.x - a.x).abs(),
                    height: (b.y - a.y).abs(),
                })
            }
            Self::Ellipse { center, radii: r } => Self::Ellipse {
                center: point(*center),
                radii: radii(*r),
            },
            Self::Line { from, to } => Self::Line {
                from: point(*from),
                to: point(*to),
            },
            Self::Arrow { from, to } => Self::Arrow {
                from: point(*from),
                to: point(*to),
            },
            Self::Freehand { points } => Self::Freehand {
                points: points.iter().copied().map(point).collect(),
            },
        }
    }
}

/// Renderer-neutral annotation stroke and optional fill, with width in document units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeStyle {
    width: f32,
    color: Color,
    fill: Option<Color>,
}

impl StrokeStyle {
    /// Creates a stroke in document units.
    ///
    /// Returns `None` for a negative or non-finite width; zero is accepted.
    #[must_use]
    pub const fn new(width: f32, color: Color) -> Option<Self> {
        if width.is_finite() && width >= 0.0 {
            Some(Self {
                width,
                color,
                fill: None,
            })
        } else {
            None
        }
    }

    /// The same stroke with an interior fill.
    #[must_use]
    pub fn with_fill(mut self, fill: Color) -> Self {
        self.fill = Some(fill);
        self
    }

    /// Returns the stroke width in document units (image pixels).
    #[must_use]
    pub fn width(self) -> f32 {
        self.width
    }

    /// Returns the stroke color.
    pub fn color(self) -> Color {
        self.color
    }

    /// The interior fill, if any.
    #[must_use]
    pub fn fill(self) -> Option<Color> {
        self.fill
    }

    /// Returns the display-pixel width using the projection’s geometric-mean axis scale.
    #[must_use]
    pub fn screen_width(self, projection: ImageProjection) -> f32 {
        let s = projection.scale();
        self.width * (s.x.abs() * s.y.abs()).sqrt()
    }
}

/// A committed annotation with stable identity, image-space geometry, and document-space style.
#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    id: AnnotationId,
    geometry: ShapeGeometry,
    style: StrokeStyle,
}

impl Annotation {
    /// An annotation over already-validated image-space geometry.
    pub fn new(id: AnnotationId, geometry: ShapeGeometry, style: StrokeStyle) -> Self {
        Self {
            id,
            geometry,
            style,
        }
    }

    /// Creates an annotation by converting display-space geometry to image space.
    #[must_use]
    pub fn from_display(
        id: AnnotationId,
        geometry: &ShapeGeometry,
        style: StrokeStyle,
        projection: ImageProjection,
    ) -> Self {
        Self::new(id, geometry.unproject(projection), style)
    }

    /// This annotation's stable identity.
    #[must_use]
    pub fn id(&self) -> &AnnotationId {
        &self.id
    }

    /// Returns the geometry in image coordinates.
    #[must_use]
    pub fn geometry(&self) -> &ShapeGeometry {
        &self.geometry
    }

    /// Returns the style, whose width is in document units.
    #[must_use]
    pub fn style(&self) -> StrokeStyle {
        self.style
    }

    /// The shape kind, for descriptions and export switches.
    #[must_use]
    pub fn kind(&self) -> ShapeKind {
        self.geometry.kind()
    }

    /// Returns image-space bounds covering the stroke and any rendered arrowhead.
    ///
    /// Unlike [`ShapeGeometry::bounds`], this includes half the stroke width on each side and the arrowhead emitted by [`crate::annotation::export::StrokePath`].
    #[must_use]
    pub fn painted_bounds(&self) -> Rectangle {
        let b = self.geometry.bounds();
        let mut pad = self.style.width / 2.0;
        if matches!(self.geometry, ShapeGeometry::Arrow { .. }) {
            pad += crate::annotation::export::arrow_head_len(self.style.width);
        }
        Rectangle {
            x: b.x - pad,
            y: b.y - pad,
            width: b.width + pad * 2.0,
            height: b.height + pad * 2.0,
        }
    }

    /// Returns a speakable shape description using whole image-coordinate units.
    ///
    /// Selection is excluded because it is published separately as structured semantics.
    #[must_use]
    pub fn describe(&self) -> String {
        let b = self.geometry.bounds();
        let kind = self.kind().localized_name();
        match &self.geometry {
            ShapeGeometry::Line { from, to } | ShapeGeometry::Arrow { from, to } => {
                crate::localized!("enzui-annotation-endpoints", "kind" => kind, "from-x" => from.x, "from-y" => from.y, "to-x" => to.x, "to-y" => to.y)
            }
            ShapeGeometry::Freehand { points } => {
                crate::localized!("enzui-annotation-points", "kind" => kind, "x" => b.x, "y" => b.y, "count" => points.len())
            }
            ShapeGeometry::Rectangle(_) | ShapeGeometry::Ellipse { .. } => {
                crate::localized!("enzui-annotation-bounds", "kind" => kind, "x" => b.x, "y" => b.y, "width" => b.width, "height" => b.height)
            }
        }
    }

    /// Resolves this annotation into display geometry, width, and colors for one frame.
    #[must_use]
    pub fn to_display(&self, projection: ImageProjection) -> DisplayShape {
        DisplayShape {
            id: self.id.clone(),
            geometry: self.geometry.project(projection),
            width: self.style.screen_width(projection),
            color: self.style.color,
            fill: self.style.fill,
        }
    }
}

/// An annotation resolved into display coordinates for one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayShape {
    /// The annotation this was derived from.
    pub id: AnnotationId,
    /// Geometry in display coordinates.
    pub geometry: ShapeGeometry,
    /// Stroke width in display pixels.
    pub width: f32,
    /// Stroke colour.
    pub color: Color,
    /// Interior fill, if any.
    pub fill: Option<Color>,
}

fn finite(p: Point) -> bool {
    p.x.is_finite() && p.y.is_finite()
}

fn bbox(points: impl Iterator<Item = Point>) -> Rectangle {
    let mut min = Point::new(f32::INFINITY, f32::INFINITY);
    let mut max = Point::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
    for p in points {
        min.x = min.x.min(p.x);
        min.y = min.y.min(p.y);
        max.x = max.x.max(p.x);
        max.y = max.y.max(p.y);
    }
    Rectangle {
        x: min.x,
        y: min.y,
        width: max.x - min.x,
        height: max.y - min.y,
    }
}
