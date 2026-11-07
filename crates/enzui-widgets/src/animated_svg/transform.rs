//! 2D transforms: SVG transform lists, CSS transform functions, their
//! interpolation, and transform origins.

#![allow(
    clippy::string_slice,
    reason = "transform lists are cut at ASCII delimiters and number ends found by search"
)]

use super::value::{format_number, number_at};

/// An affine transform `[a b c d e f]`, mapping `(x, y)` to
/// `(a x + c y + e, b x + d y + f)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Affine(pub(super) [f64; 6]);

impl Affine {
    pub(super) const IDENTITY: Self = Self([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub(super) fn translate(x: f64, y: f64) -> Self {
        Self([1.0, 0.0, 0.0, 1.0, x, y])
    }

    pub(super) fn scale(x: f64, y: f64) -> Self {
        Self([x, 0.0, 0.0, y, 0.0, 0.0])
    }

    /// A rotation by `degrees` about the origin.
    pub(super) fn rotate(degrees: f64) -> Self {
        let (sin, cos) = degrees.to_radians().sin_cos();
        Self([cos, sin, -sin, cos, 0.0, 0.0])
    }

    pub(super) fn skew(x_degrees: f64, y_degrees: f64) -> Self {
        Self([
            1.0,
            y_degrees.to_radians().tan(),
            x_degrees.to_radians().tan(),
            1.0,
            0.0,
            0.0,
        ])
    }

    /// `self` then `other` applied inside it: the matrix product
    /// `self × other`, as SVG transform lists compose left to right.
    pub(super) fn then(self, other: Self) -> Self {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = other.0;
        Self([
            a * a2 + c * b2,
            b * a2 + d * b2,
            a * c2 + c * d2,
            b * c2 + d * d2,
            a * e2 + c * f2 + e,
            b * e2 + d * f2 + f,
        ])
    }

    /// `self` with its effect centred on `(x, y)`.
    pub(super) fn about(self, x: f64, y: f64) -> Self {
        Self::translate(x, y)
            .then(self)
            .then(Self::translate(-x, -y))
    }

    pub(super) fn apply(self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }

    /// The transform as an SVG `matrix(…)`.
    pub(super) fn to_text(self) -> String {
        let [a, b, c, d, e, f] = self.0.map(format_number);
        format!("matrix({a} {b} {c} {d} {e} {f})")
    }

    /// The transform interpolated by decomposition (CSS Transforms "2D
    /// matrix decomposition"): translation, rotation, scale and skew each
    /// interpolated, so a rotation turns rather than shrinking through the
    /// middle.
    pub(super) fn interpolate(self, to: Self, t: f64) -> Self {
        let (Some(mut from), Some(mut to)) = (decompose(self), decompose(to)) else {
            return if t < 0.5 { self } else { to };
        };
        // Turn the short way round, flipping a scale sign if that is shorter.
        if (from.scale[0] < 0.0 && to.scale[1] < 0.0) || (from.scale[1] < 0.0 && to.scale[0] < 0.0)
        {
            from.scale = [-from.scale[0], -from.scale[1]];
            from.angle += if from.angle < 0.0 { 180.0 } else { -180.0 };
        }
        if from.angle == 0.0 {
            from.angle = 360.0;
        }
        if to.angle == 0.0 {
            to.angle = 360.0;
        }
        if (from.angle - to.angle).abs() > 180.0 {
            if from.angle > to.angle {
                from.angle -= 360.0;
            } else {
                to.angle -= 360.0;
            }
        }
        let mix = |a: f64, b: f64| a + (b - a) * t;
        recompose(&Decomposed {
            translate: [
                mix(from.translate[0], to.translate[0]),
                mix(from.translate[1], to.translate[1]),
            ],
            scale: [
                mix(from.scale[0], to.scale[0]),
                mix(from.scale[1], to.scale[1]),
            ],
            angle: mix(from.angle, to.angle),
            m: std::array::from_fn(|i| mix(from.m[i], to.m[i])),
        })
    }
}

struct Decomposed {
    translate: [f64; 2],
    scale: [f64; 2],
    angle: f64,
    /// The remaining 2×2 skew matrix.
    m: [f64; 4],
}

fn decompose(matrix: Affine) -> Option<Decomposed> {
    let [mut a, mut b, mut c, mut d, e, f] = matrix.0;
    let det = a * d - b * c;
    if det.abs() < 1e-12 {
        return None;
    }
    let mut sx = a.hypot(b);
    let mut sy = c.hypot(d);
    if det < 0.0 {
        if a < d {
            sx = -sx;
        } else {
            sy = -sy;
        }
    }
    if sx != 0.0 {
        a /= sx;
        b /= sx;
    }
    if sy != 0.0 {
        c /= sy;
        d /= sy;
    }
    let angle = b.atan2(a).to_degrees();
    if angle != 0.0 {
        let (sin, cos) = (-angle).to_radians().sin_cos();
        let (m11, m12, m21, m22) = (a, b, c, d);
        a = cos * m11 + sin * m21;
        b = cos * m12 + sin * m22;
        c = -sin * m11 + cos * m21;
        d = -sin * m12 + cos * m22;
    }
    Some(Decomposed {
        translate: [e, f],
        scale: [sx, sy],
        angle,
        m: [a, b, c, d],
    })
}

fn recompose(parts: &Decomposed) -> Affine {
    let [m11, m12, m21, m22] = parts.m;
    let skewed = Affine([m11, m12, m21, m22, 0.0, 0.0]);
    Affine::translate(parts.translate[0], parts.translate[1])
        .then(Affine::rotate(parts.angle))
        .then(skewed)
        .then(Affine::scale(parts.scale[0], parts.scale[1]))
}

/// An SVG `transform` attribute, or `None` when unreadable.
pub(super) fn parse_svg_transform(text: &str) -> Option<Affine> {
    let transform: svgtypes::Transform = text.parse().ok()?;
    Some(Affine([
        transform.a,
        transform.b,
        transform.c,
        transform.d,
        transform.e,
        transform.f,
    ]))
}

/// The box percentages in a CSS transform resolve against.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) struct Reference {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) width: f64,
    pub(super) height: f64,
}

/// One CSS transform function, its arguments resolved to user units and
/// degrees except percentages, which stay relative.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Function {
    Translate(Length, Length),
    Scale(f64, f64),
    Rotate(f64),
    Skew(f64, f64),
    Matrix(Affine),
}

/// A length that may be a percentage of the reference box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Length {
    Px(f64),
    Percent(f64),
}

impl Length {
    fn resolve(self, size: f64) -> f64 {
        match self {
            Self::Px(value) => value,
            Self::Percent(value) => value / 100.0 * size,
        }
    }

    fn mix(self, to: Self, t: f64, size: f64) -> Self {
        match (self, to) {
            (Self::Px(a), Self::Px(b)) => Self::Px(a + (b - a) * t),
            (Self::Percent(a), Self::Percent(b)) => Self::Percent(a + (b - a) * t),
            (a, b) => Self::Px(a.resolve(size) + (b.resolve(size) - a.resolve(size)) * t),
        }
    }
}

impl Function {
    fn matrix(&self, reference: Reference) -> Affine {
        match *self {
            Self::Translate(x, y) => {
                Affine::translate(x.resolve(reference.width), y.resolve(reference.height))
            }
            Self::Scale(x, y) => Affine::scale(x, y),
            Self::Rotate(angle) => Affine::rotate(angle),
            Self::Skew(x, y) => Affine::skew(x, y),
            Self::Matrix(matrix) => matrix,
        }
    }

    /// The identity of this function's kind, for interpolating against a
    /// shorter list.
    fn identity(&self) -> Self {
        match self {
            Self::Translate(..) => Self::Translate(Length::Px(0.0), Length::Px(0.0)),
            Self::Scale(..) => Self::Scale(1.0, 1.0),
            Self::Rotate(_) => Self::Rotate(0.0),
            Self::Skew(..) => Self::Skew(0.0, 0.0),
            Self::Matrix(_) => Self::Matrix(Affine::IDENTITY),
        }
    }

    fn mix(&self, to: &Self, t: f64, reference: Reference) -> Option<Self> {
        let mix = |a: f64, b: f64| a + (b - a) * t;
        Some(match (self, to) {
            (Self::Translate(ax, ay), Self::Translate(bx, by)) => Self::Translate(
                ax.mix(*bx, t, reference.width),
                ay.mix(*by, t, reference.height),
            ),
            (Self::Scale(ax, ay), Self::Scale(bx, by)) => Self::Scale(mix(*ax, *bx), mix(*ay, *by)),
            (Self::Rotate(a), Self::Rotate(b)) => Self::Rotate(mix(*a, *b)),
            (Self::Skew(ax, ay), Self::Skew(bx, by)) => Self::Skew(mix(*ax, *bx), mix(*ay, *by)),
            _ => return None,
        })
    }
}

/// A CSS `transform` value: `none` is the empty list.
#[derive(Debug, Clone, PartialEq, Default)]
pub(super) struct CssTransform(pub(super) Vec<Function>);

impl CssTransform {
    /// A CSS transform list, or `None` when unreadable.
    pub(super) fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text == "none" || text.is_empty() {
            return Some(Self(Vec::new()));
        }
        let mut functions = Vec::new();
        let mut rest = text;
        while !rest.trim().is_empty() {
            rest = rest.trim_start();
            let open = rest.find('(')?;
            let name = rest[..open].trim().to_ascii_lowercase();
            let close = rest[open..].find(')')? + open;
            let arguments = arguments(&rest[open + 1..close])?;
            functions.extend(function(&name, &arguments)?);
            rest = &rest[close + 1..];
        }
        Some(Self(functions))
    }

    pub(super) fn matrix(&self, reference: Reference) -> Affine {
        self.0.iter().fold(Affine::IDENTITY, |matrix, function| {
            matrix.then(function.matrix(reference))
        })
    }

    /// The value between `self` and `to` at `t`, as CSS Transforms
    /// interpolate: function by function when the lists match (a shorter
    /// list padded with identities), else by matrix decomposition.
    pub(super) fn interpolate(&self, to: &Self, t: f64, reference: Reference) -> Self {
        let length = self.0.len().max(to.0.len());
        let pick = |list: &Self, other: &Self, index: usize| {
            list.0
                .get(index)
                .cloned()
                .or_else(|| other.0.get(index).map(Function::identity))
        };
        let pairwise: Option<Vec<Function>> = (0..length)
            .map(|index| {
                let (a, b) = (pick(self, to, index)?, pick(to, self, index)?);
                a.mix(&b, t, reference)
            })
            .collect();
        match pairwise {
            Some(functions) => Self(functions),
            None => Self(vec![Function::Matrix(
                self.matrix(reference).interpolate(to.matrix(reference), t),
            )]),
        }
    }
}

/// Comma- or space-separated arguments with units: `(number, unit)`.
fn arguments(text: &str) -> Option<Vec<(f64, String)>> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < text.len() {
        let character = text[index..].chars().next()?;
        if character.is_whitespace() || character == ',' {
            index += character.len_utf8();
            continue;
        }
        let (number, length) = number_at(text, index)?;
        index += length;
        let unit_end = text[index..]
            .find(|c: char| !(c.is_ascii_alphabetic() || c == '%'))
            .map_or(text.len(), |offset| index + offset);
        out.push((number, text[index..unit_end].to_ascii_lowercase()));
        index = unit_end;
    }
    Some(out)
}

fn angle((value, unit): &(f64, String)) -> Option<f64> {
    Some(match unit.as_str() {
        "deg" | "" => *value,
        "rad" => value.to_degrees(),
        "grad" => value * 0.9,
        "turn" => value * 360.0,
        _ => return None,
    })
}

fn length((value, unit): &(f64, String)) -> Option<Length> {
    Some(match unit.as_str() {
        "px" | "" => Length::Px(*value),
        "%" => Length::Percent(*value),
        "pt" => Length::Px(value * 4.0 / 3.0),
        "pc" => Length::Px(value * 16.0),
        "in" => Length::Px(value * 96.0),
        "cm" => Length::Px(value * 96.0 / 2.54),
        "mm" => Length::Px(value * 96.0 / 25.4),
        _ => return None,
    })
}

fn function(name: &str, arguments: &[(f64, String)]) -> Option<Vec<Function>> {
    let number = |index: usize| arguments.get(index).map(|(value, _)| *value);
    let zero = Length::Px(0.0);
    Some(vec![match (name, arguments.len()) {
        ("translate", 1) => Function::Translate(length(&arguments[0])?, zero),
        ("translate", 2) => Function::Translate(length(&arguments[0])?, length(&arguments[1])?),
        ("translatex", 1) => Function::Translate(length(&arguments[0])?, zero),
        ("translatey", 1) => Function::Translate(zero, length(&arguments[0])?),
        ("translate3d", 3) => Function::Translate(length(&arguments[0])?, length(&arguments[1])?),
        ("scale", 1) => Function::Scale(number(0)?, number(0)?),
        ("scale", 2) => Function::Scale(number(0)?, number(1)?),
        ("scalex", 1) => Function::Scale(number(0)?, 1.0),
        ("scaley", 1) => Function::Scale(1.0, number(0)?),
        ("scale3d", 3) => Function::Scale(number(0)?, number(1)?),
        ("rotate" | "rotatez", 1) => Function::Rotate(angle(&arguments[0])?),
        // SVG's `rotate(angle cx cy)` written in a style.
        ("rotate", 3) => {
            let (cx, cy) = (number(1)?, number(2)?);
            return Some(vec![
                Function::Translate(Length::Px(cx), Length::Px(cy)),
                Function::Rotate(angle(&arguments[0])?),
                Function::Translate(Length::Px(-cx), Length::Px(-cy)),
            ]);
        }
        ("skew", 1) => Function::Skew(angle(&arguments[0])?, 0.0),
        ("skew", 2) => Function::Skew(angle(&arguments[0])?, angle(&arguments[1])?),
        ("skewx", 1) => Function::Skew(angle(&arguments[0])?, 0.0),
        ("skewy", 1) => Function::Skew(0.0, angle(&arguments[0])?),
        ("matrix", 6) => Function::Matrix(Affine(std::array::from_fn(|i| arguments[i].0))),
        _ => return None,
    }])
}

/// A `transform-origin` against `reference`, in user units.
pub(super) fn origin(text: &str, reference: Reference) -> (f64, f64) {
    let mut x = None;
    let mut y = None;
    let mut parts: Vec<&str> = text.split_whitespace().collect();
    parts.truncate(2);
    let resolve = |part: &str, size: f64| -> Option<f64> {
        match part {
            "left" | "top" => Some(0.0),
            "center" => Some(size / 2.0),
            "right" | "bottom" => Some(size),
            _ => {
                let parsed = arguments(part)?;
                Some(length(parsed.first()?)?.resolve(size))
            }
        }
    };
    for (index, part) in parts.iter().enumerate() {
        let vertical = matches!(*part, "top" | "bottom");
        let horizontal = matches!(*part, "left" | "right");
        if vertical || (index == 1 && !horizontal && x.is_some()) {
            y = resolve(part, reference.height);
        } else if x.is_none() {
            x = resolve(part, reference.width);
        } else {
            y = resolve(part, reference.height);
        }
    }
    let x = x.unwrap_or(
        if parts.len() == 1 && matches!(parts[0], "top" | "bottom") {
            reference.width / 2.0
        } else if parts.is_empty() {
            0.0
        } else {
            reference.width / 2.0
        },
    );
    let y = y.unwrap_or(if parts.is_empty() {
        0.0
    } else {
        reference.height / 2.0
    });
    (reference.x + x, reference.y + y)
}
