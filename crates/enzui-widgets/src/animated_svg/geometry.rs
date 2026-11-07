//! Path geometry: motion paths sampled by distance, and the object bounding
//! boxes CSS `transform-box: fill-box` resolves against.

use kurbo::{BezPath, ParamCurve, ParamCurveArclen, ParamCurveDeriv, PathSeg, Point, Shape};

use super::dom::{Document, NodeId};
use super::transform::{Affine, Reference, parse_svg_transform};

/// Arc-length accuracy, in user units.
const ACCURACY: f64 = 1e-3;

/// A path parsed for sampling by distance along it.
#[derive(Debug, Clone)]
pub(super) struct MotionPath {
    segments: Vec<PathSeg>,
    /// Each segment's cumulative end distance.
    ends: Vec<f64>,
}

impl MotionPath {
    /// SVG path data, `None` when unreadable or empty.
    pub(super) fn parse(data: &str) -> Option<Self> {
        Self::from_path(&bez_path(data)?)
    }

    /// A polyline through `points`.
    pub(super) fn polyline(points: &[(f64, f64)]) -> Option<Self> {
        let mut path = BezPath::new();
        let (first, rest) = points.split_first()?;
        path.move_to(*first);
        for point in rest {
            path.line_to(*point);
        }
        Self::from_path(&path)
    }

    fn from_path(path: &BezPath) -> Option<Self> {
        let segments: Vec<PathSeg> = path.segments().collect();
        if segments.is_empty() {
            return None;
        }
        let mut total = 0.0;
        let ends = segments
            .iter()
            .map(|segment| {
                total += segment.arclen(ACCURACY);
                total
            })
            .collect();
        Some(Self { segments, ends })
    }

    pub(super) fn length(&self) -> f64 {
        self.ends.last().copied().unwrap_or(0.0)
    }

    /// The point and tangent angle (degrees) `fraction` of the way along.
    pub(super) fn at(&self, fraction: f64) -> ((f64, f64), f64) {
        let distance = fraction.clamp(0.0, 1.0) * self.length();
        let index = self
            .ends
            .iter()
            .position(|end| *end >= distance)
            .unwrap_or(self.segments.len() - 1);
        let start = if index == 0 {
            0.0
        } else {
            self.ends[index - 1]
        };
        let segment = self.segments[index];
        let local = segment
            .inv_arclen(distance - start, ACCURACY)
            .clamp(0.0, 1.0);
        let point = segment.eval(local);
        let tangent = match segment {
            PathSeg::Line(line) => line.deriv().eval(local),
            PathSeg::Quad(quad) => quad.deriv().eval(local),
            PathSeg::Cubic(cubic) => cubic.deriv().eval(local),
        };
        let angle = if tangent.x == 0.0 && tangent.y == 0.0 {
            0.0
        } else {
            tangent.y.atan2(tangent.x).to_degrees()
        };
        ((point.x, point.y), angle)
    }

    /// The fraction of the length at the start of each of `count` vertices
    /// of a polyline built from `count` points: for paced keyed motion.
    pub(super) fn vertex_fractions(&self) -> Vec<f64> {
        let length = self.length();
        std::iter::once(0.0)
            .chain(
                self.ends
                    .iter()
                    .map(|end| if length > 0.0 { end / length } else { 0.0 }),
            )
            .collect()
    }
}

/// SVG path data as a kurbo path (absolute, arcs as curves).
pub(super) fn bez_path(data: &str) -> Option<BezPath> {
    use svgtypes::SimplePathSegment as S;
    let mut path = BezPath::new();
    let mut started = false;
    for segment in svgtypes::SimplifyingPathParser::from(data) {
        match segment.ok()? {
            S::MoveTo { x, y } => {
                path.move_to((x, y));
                started = true;
            }
            S::LineTo { x, y } if started => path.line_to((x, y)),
            S::CurveTo {
                x1,
                y1,
                x2,
                y2,
                x,
                y,
            } if started => path.curve_to((x1, y1), (x2, y2), (x, y)),
            S::Quadratic { x1, y1, x, y } if started => path.quad_to((x1, y1), (x, y)),
            S::ClosePath if started => path.close_path(),
            _ => return None,
        }
    }
    (!path.elements().is_empty()).then_some(path)
}

/// The object bounding box of `node` in its own user space (before its own
/// transform): what `fill-box` means. Shapes from their geometry attributes,
/// groups from their children; `None` for an element with no geometry.
pub(super) fn bounding_box(document: &Document, node: NodeId) -> Option<Reference> {
    let rect = shape_box(document, node, 0)?;
    Some(Reference {
        x: rect.x0,
        y: rect.y0,
        width: rect.width(),
        height: rect.height(),
    })
}

fn number(document: &Document, node: NodeId, name: &str) -> f64 {
    document
        .attribute(node, name)
        .and_then(|v| {
            let v = v.trim();
            v.strip_suffix("px").unwrap_or(v).trim().parse().ok()
        })
        .unwrap_or(0.0)
}

fn shape_box(document: &Document, node: NodeId, depth: u32) -> Option<kurbo::Rect> {
    if depth > 64 {
        return None;
    }
    let name = document.svg_name(node)?;
    let n = |attribute| number(document, node, attribute);
    let rect = match name {
        "rect" | "image" | "foreignObject" => {
            kurbo::Rect::new(n("x"), n("y"), n("x") + n("width"), n("y") + n("height"))
        }
        "circle" => kurbo::Rect::new(
            n("cx") - n("r"),
            n("cy") - n("r"),
            n("cx") + n("r"),
            n("cy") + n("r"),
        ),
        "ellipse" => kurbo::Rect::new(
            n("cx") - n("rx"),
            n("cy") - n("ry"),
            n("cx") + n("rx"),
            n("cy") + n("ry"),
        ),
        "line" => kurbo::Rect::from_points((n("x1"), n("y1")), (n("x2"), n("y2"))),
        "path" => bez_path(document.attribute(node, "d")?)?.bounding_box(),
        "polyline" | "polygon" => {
            let numbers: Vec<f64> = document
                .attribute(node, "points")?
                .split(|c: char| c.is_whitespace() || c == ',')
                .filter(|s| !s.is_empty())
                .map(|s| s.parse().ok())
                .collect::<Option<_>>()?;
            let points: Vec<Point> = numbers
                .as_chunks::<2>()
                .0
                .iter()
                .map(|p| Point::new(p[0], p[1]))
                .collect();
            let first = *points.first()?;
            points
                .iter()
                .fold(kurbo::Rect::from_points(first, first), |r, p| {
                    r.union_pt(*p)
                })
        }
        "g" | "a" | "svg" | "switch" | "symbol" => {
            let mut union: Option<kurbo::Rect> = None;
            for child in document.element_children(node) {
                let Some(child_box) = shape_box(document, child, depth + 1) else {
                    continue;
                };
                let transform = document
                    .attribute(child, "transform")
                    .and_then(parse_svg_transform)
                    .unwrap_or(Affine::IDENTITY);
                let corners = [
                    (child_box.x0, child_box.y0),
                    (child_box.x1, child_box.y0),
                    (child_box.x0, child_box.y1),
                    (child_box.x1, child_box.y1),
                ]
                .map(|(x, y)| {
                    let (x, y) = transform.apply(x, y);
                    Point::new(x, y)
                });
                let mapped = corners[1..]
                    .iter()
                    .fold(kurbo::Rect::from_points(corners[0], corners[0]), |r, p| {
                        r.union_pt(*p)
                    });
                union = Some(union.map_or(mapped, |u| u.union(mapped)));
            }
            union?
        }
        "use" => {
            let target = document.resolve_reference(document.attribute(node, "href")?)?;
            let inner = shape_box(document, target, depth + 1)?;
            inner + kurbo::Vec2::new(n("x"), n("y"))
        }
        _ => return None,
    };
    Some(rect)
}
