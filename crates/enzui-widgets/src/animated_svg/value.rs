//! Animated values: parsed once, interpolated, added and measured.

#![allow(
    clippy::string_slice,
    reason = "number scanning advances by whole chars and ASCII digits"
)]

use std::fmt::Write as _;
use std::sync::Arc;

/// An attribute or property value an animation produces.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Value {
    /// Numbers set in a fixed text skeleton: `skeleton[0] n0 skeleton[1] n1 …
    /// skeleton[n]`. Lengths with units, number lists, points, view boxes,
    /// dash arrays and normalised path data all take this form, and two of
    /// them interpolate when their skeletons are the same.
    Numeric {
        numbers: Vec<f64>,
        skeleton: Arc<[String]>,
    },
    /// An sRGB colour, components 0..255 and alpha 0..1.
    Color([f64; 4]),
    /// Anything else: interpolated only discretely.
    Text(String),
}

/// What kind of value an attribute or property holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    /// A colour or paint (`none` and `url(…)` stay discrete).
    Paint,
    /// Path data, normalised to absolute commands before comparing.
    Path,
    /// A number, a length or a list of them.
    Numeric,
    /// Only discrete changes.
    Discrete,
}

/// The kind of the attribute or property `name`.
pub(super) fn kind_of(name: &str) -> Kind {
    match name {
        "fill" | "stroke" | "stop-color" | "flood-color" | "lighting-color" | "color" => {
            Kind::Paint
        }
        "d" | "path" => Kind::Path,
        "visibility"
        | "display"
        | "href"
        | "fill-rule"
        | "clip-rule"
        | "stroke-linecap"
        | "stroke-linejoin"
        | "class"
        | "text-anchor"
        | "font-family"
        | "font-style"
        | "font-weight"
        | "dominant-baseline"
        | "pointer-events"
        | "overflow"
        | "mask"
        | "clip-path"
        | "filter"
        | "marker-start"
        | "marker-mid"
        | "marker-end"
        | "preserveAspectRatio"
        | "spreadMethod"
        | "gradientUnits"
        | "patternUnits"
        | "image-rendering"
        | "shape-rendering"
        | "text-rendering"
        | "mix-blend-mode"
        | "isolation"
        | "paint-order"
        | "vector-effect" => Kind::Discrete,
        _ => Kind::Numeric,
    }
}

impl Value {
    /// `text` read as a value of `kind`.
    pub(super) fn parse(text: &str, kind: Kind) -> Self {
        let text = text.trim();
        match kind {
            Kind::Paint => {
                parse_color(text).map_or_else(|| Self::Text(text.to_owned()), Self::Color)
            }
            Kind::Path => normalise_path(text).map_or_else(
                || Self::Text(text.to_owned()),
                |path| numeric(&path).unwrap_or(Self::Text(path)),
            ),
            Kind::Numeric => numeric(text).unwrap_or_else(|| Self::Text(text.to_owned())),
            Kind::Discrete => Self::Text(text.to_owned()),
        }
    }

    /// The value between `self` (at 0) and `to` (at 1), or `None` when the two
    /// cannot be interpolated.
    pub(super) fn interpolate(&self, to: &Self, t: f64) -> Option<Self> {
        match (self, to) {
            (
                Self::Numeric {
                    numbers: a,
                    skeleton,
                },
                Self::Numeric {
                    numbers: b,
                    skeleton: other,
                },
            ) if skeleton == other => Some(Self::Numeric {
                numbers: a.iter().zip(b).map(|(a, b)| a + (b - a) * t).collect(),
                skeleton: Arc::clone(skeleton),
            }),
            (Self::Color(a), Self::Color(b)) => Some(Self::Color(std::array::from_fn(|i| {
                a[i] + (b[i] - a[i]) * t
            }))),
            _ => None,
        }
    }

    /// `self + other`, or `None` when the two do not add.
    pub(super) fn add(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (
                Self::Numeric {
                    numbers: a,
                    skeleton,
                },
                Self::Numeric {
                    numbers: b,
                    skeleton: other,
                },
            ) if skeleton == other => Some(Self::Numeric {
                numbers: a.iter().zip(b).map(|(a, b)| a + b).collect(),
                skeleton: Arc::clone(skeleton),
            }),
            (Self::Color(a), Self::Color(b)) => Some(Self::Color([
                a[0] + b[0],
                a[1] + b[1],
                a[2] + b[2],
                a[3].max(b[3]),
            ])),
            _ => None,
        }
    }

    /// `self` repeated `times` over (for accumulation), or `None`.
    pub(super) fn scale(&self, times: f64) -> Option<Self> {
        match self {
            Self::Numeric { numbers, skeleton } => Some(Self::Numeric {
                numbers: numbers.iter().map(|n| n * times).collect(),
                skeleton: Arc::clone(skeleton),
            }),
            Self::Color(c) => Some(Self::Color([
                c[0] * times,
                c[1] * times,
                c[2] * times,
                c[3],
            ])),
            Self::Text(_) => None,
        }
    }

    /// The distance from `self` to `other`, for paced animation.
    pub(super) fn distance(&self, other: &Self) -> Option<f64> {
        match (self, other) {
            (
                Self::Numeric {
                    numbers: a,
                    skeleton,
                },
                Self::Numeric {
                    numbers: b,
                    skeleton: other,
                },
            ) if skeleton == other => Some(
                a.iter()
                    .zip(b)
                    .map(|(a, b)| (a - b) * (a - b))
                    .sum::<f64>()
                    .sqrt(),
            ),
            (Self::Color(a), Self::Color(b)) => Some(
                (0..3)
                    .map(|i| (a[i] - b[i]) * (a[i] - b[i]))
                    .sum::<f64>()
                    .sqrt(),
            ),
            _ => None,
        }
    }

    /// The numbers, for values a caller reads as numbers.
    pub(super) fn numbers(&self) -> Option<&[f64]> {
        match self {
            Self::Numeric { numbers, .. } => Some(numbers),
            _ => None,
        }
    }

    /// The value written back as SVG text.
    pub(super) fn to_text(&self) -> String {
        match self {
            Self::Numeric { numbers, skeleton } => {
                let mut text = String::new();
                for (index, part) in skeleton.iter().enumerate() {
                    text.push_str(part);
                    if let Some(number) = numbers.get(index) {
                        write_number(*number, &mut text);
                    }
                }
                text
            }
            Self::Color([r, g, b, a]) => {
                let channel = |c: f64| c.round().clamp(0.0, 255.0) as u8;
                if *a >= 1.0 {
                    format!("#{:02x}{:02x}{:02x}", channel(*r), channel(*g), channel(*b))
                } else {
                    format!(
                        "rgba({},{},{},{})",
                        channel(*r),
                        channel(*g),
                        channel(*b),
                        format_number(a.clamp(0.0, 1.0))
                    )
                }
            }
            Self::Text(text) => text.clone(),
        }
    }
}

/// `number` written with at most six decimals and no exponent.
pub(super) fn write_number(number: f64, text: &mut String) {
    let rounded = (number * 1e6).round() / 1e6;
    let rounded = if rounded == 0.0 { 0.0 } else { rounded };
    let _ = write!(text, "{rounded}");
}

pub(super) fn format_number(number: f64) -> String {
    let mut text = String::new();
    write_number(number, &mut text);
    text
}

/// The numbers in `text` and the skeleton around them; `None` without a
/// number. Separators between list items normalise to one space, or to `, `
/// inside parentheses, where CSS needs the comma.
fn numeric(text: &str) -> Option<Value> {
    let mut numbers = Vec::new();
    let mut skeleton = Vec::new();
    let mut current = String::new();
    let mut depth = 0_u32;
    // A run of separators not yet written: whether it held a comma.
    let mut separator: Option<bool> = None;
    let mut index = 0;
    while index < text.len() {
        let character = text[index..].chars().next()?;
        if character.is_whitespace() || character == ',' {
            separator = Some(separator.unwrap_or(false) || character == ',');
            index += character.len_utf8();
            continue;
        }
        if let Some(comma) = separator.take() {
            let leading = current.is_empty() && skeleton.is_empty();
            let closing = character == ')';
            if !leading && !closing {
                current.push_str(if comma && depth > 0 { ", " } else { " " });
            }
        }
        if let Some((number, length)) = number_at(text, index) {
            skeleton.push(std::mem::take(&mut current));
            numbers.push(number);
            index += length;
            continue;
        }
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        current.push(character);
        index += character.len_utf8();
    }
    skeleton.push(current);
    if numbers.is_empty() {
        return None;
    }
    Some(Value::Numeric {
        numbers,
        skeleton: skeleton.into(),
    })
}

/// A number starting at byte `start` of `text`, if one does: its value and
/// length. A sign counts only where a number may begin.
pub(super) fn number_at(text: &str, start: usize) -> Option<(f64, usize)> {
    let bytes = text.as_bytes();
    let previous = start.checked_sub(1).map(|i| bytes[i]);
    // A digit or dot glued to a letter belongs to a name (`translate3d`).
    if previous.is_some_and(|b| b.is_ascii_alphabetic() || b == b'_' || b == b'#') {
        return None;
    }
    let mut end = start;
    if matches!(bytes.get(end), Some(b'+' | b'-')) {
        end += 1;
    }
    let digits_start = end;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    if bytes.get(end) == Some(&b'.') && bytes.get(end + 1).is_some_and(u8::is_ascii_digit) {
        end += 1;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
    }
    if end == digits_start {
        return None;
    }
    if matches!(bytes.get(end), Some(b'e' | b'E')) {
        let mut exponent = end + 1;
        if matches!(bytes.get(exponent), Some(b'+' | b'-')) {
            exponent += 1;
        }
        if bytes.get(exponent).is_some_and(u8::is_ascii_digit) {
            while bytes.get(exponent).is_some_and(u8::is_ascii_digit) {
                exponent += 1;
            }
            end = exponent;
        }
    }
    text[start..end]
        .parse()
        .ok()
        .map(|number| (number, end - start))
}

/// An SVG colour, `None` for `none`, `currentColor`, `url(…)` and anything
/// unreadable.
pub(super) fn parse_color(text: &str) -> Option<[f64; 4]> {
    if text.eq_ignore_ascii_case("currentcolor") || text.starts_with("url(") || text == "none" {
        return None;
    }
    let color: svgtypes::Color = text.parse().ok()?;
    Some([
        f64::from(color.red),
        f64::from(color.green),
        f64::from(color.blue),
        f64::from(color.alpha) / 255.0,
    ])
}

/// Path data rewritten with absolute commands, one letter per segment, so
/// two paths with the same command sequence share a skeleton. Arc flags are
/// written as words, so they never interpolate.
pub(super) fn normalise_path(text: &str) -> Option<String> {
    use svgtypes::PathSegment as S;
    let mut out = String::new();
    let (mut x, mut y) = (0.0_f64, 0.0_f64);
    let (mut start_x, mut start_y) = (0.0_f64, 0.0_f64);
    let point = |out: &mut String, px: f64, py: f64| {
        out.push(' ');
        write_number(px, out);
        out.push(' ');
        write_number(py, out);
    };
    for segment in svgtypes::PathParser::from(text) {
        let segment = segment.ok()?;
        match segment {
            S::MoveTo { abs, x: px, y: py } => {
                let (px, py) = if abs { (px, py) } else { (x + px, y + py) };
                out.push_str(" M");
                point(&mut out, px, py);
                (x, y, start_x, start_y) = (px, py, px, py);
            }
            S::LineTo { abs, x: px, y: py } => {
                let (px, py) = if abs { (px, py) } else { (x + px, y + py) };
                out.push_str(" L");
                point(&mut out, px, py);
                (x, y) = (px, py);
            }
            S::HorizontalLineTo { abs, x: px } => {
                let px = if abs { px } else { x + px };
                out.push_str(" H ");
                write_number(px, &mut out);
                x = px;
            }
            S::VerticalLineTo { abs, y: py } => {
                let py = if abs { py } else { y + py };
                out.push_str(" V ");
                write_number(py, &mut out);
                y = py;
            }
            S::CurveTo {
                abs,
                x1,
                y1,
                x2,
                y2,
                x: px,
                y: py,
            } => {
                let (dx, dy) = if abs { (0.0, 0.0) } else { (x, y) };
                out.push_str(" C");
                point(&mut out, x1 + dx, y1 + dy);
                point(&mut out, x2 + dx, y2 + dy);
                point(&mut out, px + dx, py + dy);
                (x, y) = (px + dx, py + dy);
            }
            S::SmoothCurveTo {
                abs,
                x2,
                y2,
                x: px,
                y: py,
            } => {
                let (dx, dy) = if abs { (0.0, 0.0) } else { (x, y) };
                out.push_str(" S");
                point(&mut out, x2 + dx, y2 + dy);
                point(&mut out, px + dx, py + dy);
                (x, y) = (px + dx, py + dy);
            }
            S::Quadratic {
                abs,
                x1,
                y1,
                x: px,
                y: py,
            } => {
                let (dx, dy) = if abs { (0.0, 0.0) } else { (x, y) };
                out.push_str(" Q");
                point(&mut out, x1 + dx, y1 + dy);
                point(&mut out, px + dx, py + dy);
                (x, y) = (px + dx, py + dy);
            }
            S::SmoothQuadratic { abs, x: px, y: py } => {
                let (px, py) = if abs { (px, py) } else { (x + px, y + py) };
                out.push_str(" T");
                point(&mut out, px, py);
                (x, y) = (px, py);
            }
            S::EllipticalArc {
                abs,
                rx,
                ry,
                x_axis_rotation,
                large_arc,
                sweep,
                x: px,
                y: py,
            } => {
                let (px, py) = if abs { (px, py) } else { (x + px, y + py) };
                out.push_str(" A ");
                write_number(rx, &mut out);
                out.push(' ');
                write_number(ry, &mut out);
                out.push(' ');
                write_number(x_axis_rotation, &mut out);
                // Flags as words: a different flag is a different shape.
                out.push_str(if large_arc { " large" } else { " small" });
                out.push_str(if sweep { "+sweep" } else { "+nosweep" });
                point(&mut out, px, py);
                (x, y) = (px, py);
            }
            S::ClosePath { .. } => {
                out.push_str(" Z");
                (x, y) = (start_x, start_y);
            }
        }
    }
    Some(out.trim_start().to_owned())
}

/// Normalised path text (see [`normalise_path`]) back to SVG path data.
pub(super) fn denormalise_path(text: &str) -> String {
    text.replace(" large+sweep", " 1 1")
        .replace(" large+nosweep", " 1 0")
        .replace(" small+sweep", " 0 1")
        .replace(" small+nosweep", " 0 0")
}
