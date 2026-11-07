//! SMIL animation elements: what each one animates and its value at a point
//! in its interval.

use super::dom::{Document, NodeId};
use super::easing::bezier;
use super::geometry::MotionPath;
use super::timing::{Phase, Restart, Timing, parse_clock, parse_conditions};
use super::transform::Affine;
use super::value::{Kind, Value, kind_of};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum CalcMode {
    Discrete,
    Linear,
    Paced,
    Spline,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum TransformType {
    Translate,
    Scale,
    Rotate,
    SkewX,
    SkewY,
}

impl TransformType {
    /// The matrix of this type with `params`.
    pub(super) fn matrix(self, params: &[f64]) -> Affine {
        let p = |i: usize| params.get(i).copied().unwrap_or(0.0);
        match self {
            Self::Translate => Affine::translate(p(0), p(1)),
            Self::Scale => Affine::scale(p(0), params.get(1).copied().unwrap_or_else(|| p(0))),
            Self::Rotate => Affine::rotate(p(0)).about(p(1), p(2)),
            Self::SkewX => Affine::skew(p(0), 0.0),
            Self::SkewY => Affine::skew(0.0, p(0)),
        }
    }

    /// `params` padded to this type's full parameter list.
    fn pad(self, params: &[f64]) -> Option<Vec<f64>> {
        let p = |i: usize| params.get(i).copied();
        Some(match self {
            Self::Translate => vec![p(0)?, p(1).unwrap_or(0.0)],
            Self::Scale => vec![p(0)?, p(1).unwrap_or(p(0)?)],
            Self::Rotate => vec![p(0)?, p(1).unwrap_or(0.0), p(2).unwrap_or(0.0)],
            Self::SkewX | Self::SkewY => vec![p(0)?],
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Rotate {
    Fixed(f64),
    Auto,
    AutoReverse,
}

/// The values an animation runs through.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Values {
    /// `values="…"` or `from` + `to`.
    List(Vec<Value>),
    /// `to` alone: from the underlying value.
    To(Value),
    /// `by` alone: from zero, added to the underlying value.
    By(Value),
    /// `from` + `by`.
    FromBy(Value, Value),
}

#[derive(Debug, Clone)]
pub(super) enum Effect {
    /// `animate`, `set` and `animateColor` on an attribute or property.
    Attribute {
        name: String,
        kind: Kind,
        values: Values,
    },
    /// `animateTransform`: values are this type's parameters.
    Transform { kind: TransformType, values: Values },
    /// `animateMotion` along `path`.
    Motion {
        path: MotionPath,
        /// For a `values` list, the fraction of the path at each value.
        vertices: Option<Vec<f64>>,
        key_points: Option<Vec<f64>>,
        rotate: Rotate,
    },
}

#[derive(Debug, Clone)]
pub(super) struct Animation {
    pub(super) target: NodeId,
    pub(super) effect: Effect,
    pub(super) calc_mode: CalcMode,
    pub(super) key_times: Option<Vec<f64>>,
    pub(super) key_splines: Option<Vec<[f64; 4]>>,
    pub(super) additive: bool,
    pub(super) accumulate: bool,
    /// A `set` element: its one value holds for the whole interval.
    pub(super) set: bool,
}

/// Every animation element of `document`, in document order, with its
/// timing.
pub(super) fn collect(document: &Document) -> (Vec<Animation>, Vec<Timing>) {
    let elements: Vec<NodeId> = document
        .elements()
        .filter(|node| {
            matches!(
                document.svg_name(*node),
                Some("animate" | "set" | "animateTransform" | "animateMotion" | "animateColor")
            )
        })
        .collect();
    // Kept first, so a syncbase index counts only animations that exist.
    let kept: Vec<(NodeId, Animation)> = elements
        .iter()
        .filter_map(|&node| {
            let target = match document.attribute(node, "href") {
                Some(reference) => document.resolve_reference(reference),
                None => document.nodes[node].parent,
            };
            let target = target.filter(|t| document.is_element(*t))?;
            Some((node, build(document, node, target)?))
        })
        .collect();
    let index_of = |id: &str| {
        let node = *document.ids.get(id)?;
        kept.iter().position(|(n, _)| *n == node)
    };
    let node_of = |id: &str| document.ids.get(id).copied();
    let mut timings = Vec::with_capacity(kept.len());
    for (node, animation) in &kept {
        let (node, target) = (*node, animation.target);
        let attribute = |name: &str| document.attribute(node, name);
        let conditions = |name: &str, default: &str| {
            parse_conditions(
                attribute(name).unwrap_or(default),
                target,
                &node_of,
                &index_of,
            )
        };
        let dur = attribute("dur").and_then(parse_clock).filter(|d| *d >= 0.0);
        let repeat = |name: &str, clock: bool| {
            attribute(name).map(|v| {
                if v.trim() == "indefinite" {
                    None
                } else if clock {
                    parse_clock(v)
                } else {
                    v.trim().parse::<f64>().ok().filter(|c| *c > 0.0)
                }
            })
        };
        timings.push(Timing {
            begin: conditions("begin", "0s"),
            end: attribute("end").map_or_else(Vec::new, |_| conditions("end", "")),
            dur,
            repeat_count: repeat("repeatCount", false),
            repeat_dur: repeat("repeatDur", true),
            min: attribute("min")
                .and_then(parse_clock)
                .unwrap_or(0.0)
                .max(0.0),
            max: attribute("max").and_then(parse_clock),
            restart: match attribute("restart").map(str::trim) {
                Some("whenNotActive") => Restart::WhenNotActive,
                Some("never") => Restart::Never,
                _ => Restart::Always,
            },
            freeze: attribute("fill").map(str::trim) == Some("freeze"),
        });
    }
    let animations = kept.into_iter().map(|(_, animation)| animation).collect();
    (animations, timings)
}

fn build(document: &Document, node: NodeId, target: NodeId) -> Option<Animation> {
    let name = document.svg_name(node)?;
    let attribute = |n: &str| document.attribute(node, n);
    let list = split_list;
    let key_times = attribute("keyTimes").map(|text| {
        list(text)
            .iter()
            .filter_map(|v| v.parse::<f64>().ok())
            .collect::<Vec<_>>()
    });
    let key_splines = attribute("keySplines").map(|text| {
        list(text)
            .iter()
            .filter_map(|spline| {
                let numbers: Vec<f64> = spline
                    .split(|c: char| c.is_whitespace() || c == ',')
                    .filter(|s| !s.is_empty())
                    .filter_map(|s| s.parse().ok())
                    .collect();
                <[f64; 4]>::try_from(numbers).ok()
            })
            .collect::<Vec<_>>()
    });
    let set = name == "set";
    let mut calc_mode = match attribute("calcMode").map(str::trim) {
        Some("discrete") => CalcMode::Discrete,
        Some("paced") => CalcMode::Paced,
        Some("spline") => CalcMode::Spline,
        Some("linear") => CalcMode::Linear,
        _ if name == "animateMotion" => CalcMode::Paced,
        _ => CalcMode::Linear,
    };
    if set {
        calc_mode = CalcMode::Discrete;
    }
    let values_with = |parse: &dyn Fn(&str) -> Option<Value>| -> Option<Values> {
        if set {
            return Some(Values::List(vec![parse(attribute("to")?)?]));
        }
        if let Some(values) = attribute("values") {
            let parsed: Vec<Value> = list(values)
                .iter()
                .map(|v| parse(v))
                .collect::<Option<_>>()?;
            return (!parsed.is_empty()).then_some(Values::List(parsed));
        }
        let from = attribute("from").map(parse);
        let to = attribute("to").map(parse);
        let by = attribute("by").map(parse);
        Some(match (from, to, by) {
            (Some(from), Some(to), _) => Values::List(vec![from?, to?]),
            (Some(from), None, Some(by)) => Values::FromBy(from?, by?),
            (None, Some(to), _) => Values::To(to?),
            (None, None, Some(by)) => Values::By(by?),
            _ => return None,
        })
    };
    let additive_attribute = attribute("additive").map(str::trim) == Some("sum");
    let accumulate = attribute("accumulate").map(str::trim) == Some("sum");
    let effect = match name {
        "animateTransform" => {
            let kind = match attribute("type").map_or("translate", str::trim) {
                "translate" => TransformType::Translate,
                "scale" => TransformType::Scale,
                "rotate" => TransformType::Rotate,
                "skewX" => TransformType::SkewX,
                "skewY" => TransformType::SkewY,
                _ => return None,
            };
            let parse = |text: &str| -> Option<Value> {
                let numbers: Vec<f64> = text
                    .split(|c: char| c.is_whitespace() || c == ',')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.parse().ok())
                    .collect::<Option<_>>()?;
                params(kind.pad(&numbers)?)
            };
            Effect::Transform {
                kind,
                values: values_with(&parse)?,
            }
        }
        "animateMotion" => motion(document, node, &values_with)?,
        _ => {
            let attribute_name = attribute("attributeName")?.trim();
            let local = attribute_name
                .rsplit(':')
                .next()
                .unwrap_or(attribute_name)
                .to_owned();
            let kind = kind_of(&local);
            let parse = |text: &str| Some(Value::parse(text, kind));
            Effect::Attribute {
                name: local,
                kind,
                values: values_with(&parse)?,
            }
        }
    };
    let by_only = matches!(
        &effect,
        Effect::Attribute {
            values: Values::By(_),
            ..
        } | Effect::Transform {
            values: Values::By(_),
            ..
        }
    );
    Some(Animation {
        target,
        effect,
        calc_mode,
        key_times,
        key_splines,
        additive: additive_attribute || by_only,
        accumulate,
        set,
    })
}

/// Reads an animation element's values with a per-value parser.
type ValuesWith<'a> = dyn Fn(&dyn Fn(&str) -> Option<Value>) -> Option<Values> + 'a;

/// Transform parameters as a numeric value every list of the same type
/// interpolates with.
fn params(numbers: Vec<f64>) -> Option<Value> {
    let skeleton: Arc<[String]> = (0..=numbers.len())
        .map(|i| {
            if i == 0 || i == numbers.len() {
                String::new()
            } else {
                " ".to_owned()
            }
        })
        .collect();
    Some(Value::Numeric { numbers, skeleton })
}

fn motion(document: &Document, node: NodeId, values_with: &ValuesWith<'_>) -> Option<Effect> {
    let attribute = |n: &str| document.attribute(node, n);
    let rotate = match attribute("rotate").map(str::trim) {
        Some("auto") => Rotate::Auto,
        Some("auto-reverse") => Rotate::AutoReverse,
        Some(angle) => Rotate::Fixed(angle.parse().unwrap_or(0.0)),
        None => Rotate::Fixed(0.0),
    };
    let key_points = attribute("keyPoints").map(|text| {
        text.split(';')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .filter_map(|v| v.parse::<f64>().ok())
            .collect::<Vec<_>>()
    });
    // An `mpath` child wins over `path`, which wins over values.
    let mpath = document
        .element_children(node)
        .find(|child| document.svg_name(*child) == Some("mpath"))
        .and_then(|mpath| document.resolve_reference(document.attribute(mpath, "href")?))
        .and_then(|path| document.attribute(path, "d"));
    if let Some(data) = mpath.or_else(|| attribute("path")) {
        return Some(Effect::Motion {
            path: MotionPath::parse(data)?,
            vertices: None,
            key_points,
            rotate,
        });
    }
    let point = |text: &str| -> Option<Value> {
        let numbers: Vec<f64> = text
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|s| !s.is_empty())
            .map(|s| s.parse().ok())
            .collect::<Option<_>>()?;
        (numbers.len() == 2).then(|| params(numbers))?
    };
    let points: Vec<(f64, f64)> = match values_with(&point)? {
        Values::List(list) => list
            .iter()
            .filter_map(|v| v.numbers().map(|n| (n[0], n[1])))
            .collect(),
        Values::To(to) => {
            let to = to.numbers()?;
            vec![(0.0, 0.0), (to[0], to[1])]
        }
        Values::By(by) => {
            let by = by.numbers()?;
            vec![(0.0, 0.0), (by[0], by[1])]
        }
        Values::FromBy(from, by) => {
            let (from, by) = (from.numbers()?, by.numbers()?);
            vec![(from[0], from[1]), (from[0] + by[0], from[1] + by[1])]
        }
    };
    let path = MotionPath::polyline(&points)?;
    let vertices = Some(path.vertex_fractions());
    Some(Effect::Motion {
        path,
        vertices,
        key_points,
        rotate,
    })
}

/// How a sampled value changes over the simple duration.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Change {
    Continuous,
    /// Only at these fractions of the simple duration.
    At(Vec<f64>),
}

/// An animation's contribution at one instant.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Output {
    Value(Value),
    Transform(Affine),
}

/// Where in its simple duration an animation is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct SimpleTime {
    /// Fraction of the simple duration, `0..=1`.
    pub(super) fraction: f64,
    /// Completed repeats.
    pub(super) iteration: f64,
}

/// The simple time for `phase` with simple duration `dur`.
pub(super) fn simple_time(phase: Phase, dur: Option<f64>) -> Option<SimpleTime> {
    let (elapsed, frozen) = match phase {
        Phase::Idle => return None,
        Phase::Active { elapsed, .. } => (elapsed, false),
        Phase::Frozen { active, .. } => (active, true),
    };
    let Some(dur) = dur.filter(|d| *d > 0.0) else {
        return Some(SimpleTime {
            fraction: if frozen && dur == Some(0.0) { 1.0 } else { 0.0 },
            iteration: 0.0,
        });
    };
    let mut iteration = (elapsed / dur).floor();
    let mut fraction = elapsed / dur - iteration;
    if frozen && fraction < 1e-9 && iteration > 0.0 {
        iteration -= 1.0;
        fraction = 1.0;
    }
    Some(SimpleTime {
        fraction,
        iteration,
    })
}

impl Animation {
    /// The value at `time` through the values, with `base` the underlying
    /// value (for `to` animations and additive zero), and how it changes.
    pub(super) fn sample(
        &self,
        time: SimpleTime,
        base: Option<&Value>,
    ) -> Option<(Output, Change)> {
        match &self.effect {
            Effect::Attribute { values, .. } | Effect::Transform { values, .. } => {
                let list = self.expand(values, base)?;
                let (mut value, change) = self.keyed(&list, time.fraction)?;
                if self.accumulate
                    && time.iteration > 0.0
                    && !matches!(values, Values::To(_))
                    && let Some(sum) = list
                        .last()
                        .and_then(|last| last.scale(time.iteration))
                        .and_then(|s| value.add(&s))
                {
                    value = sum;
                }
                match &self.effect {
                    Effect::Transform { kind, .. } => {
                        Some((Output::Transform(kind.matrix(value.numbers()?)), change))
                    }
                    _ => Some((Output::Value(value), change)),
                }
            }
            Effect::Motion {
                path,
                vertices,
                key_points,
                rotate,
            } => {
                let p = time.fraction;
                let (fraction, change) = match (key_points, &self.key_times) {
                    (Some(points), Some(times))
                        if points.len() == times.len()
                            && points.len() >= 2
                            && self.calc_mode != CalcMode::Paced =>
                    {
                        let list: Vec<Value> = points
                            .iter()
                            .map(|k| params(vec![*k]))
                            .collect::<Option<_>>()?;
                        let (value, change) = self.keyed(&list, p)?;
                        (value.numbers()?[0], change)
                    }
                    _ => match (vertices, self.calc_mode) {
                        (
                            Some(vertices),
                            CalcMode::Linear | CalcMode::Spline | CalcMode::Discrete,
                        ) if vertices.len() >= 2 => {
                            let list: Vec<Value> = vertices
                                .iter()
                                .map(|v| params(vec![*v]))
                                .collect::<Option<_>>()?;
                            let (value, change) = self.keyed(&list, p)?;
                            (value.numbers()?[0], change)
                        }
                        _ => (p, Change::Continuous),
                    },
                };
                let ((mut x, mut y), tangent) = path.at(fraction);
                if self.accumulate && time.iteration > 0.0 {
                    // Each repeat starts where the path's last one ended.
                    let ((start_x, start_y), _) = path.at(0.0);
                    let ((end_x, end_y), _) = path.at(1.0);
                    x += time.iteration * (end_x - start_x);
                    y += time.iteration * (end_y - start_y);
                }
                let angle = match rotate {
                    Rotate::Fixed(angle) => *angle,
                    Rotate::Auto => tangent,
                    Rotate::AutoReverse => tangent + 180.0,
                };
                Some((
                    Output::Transform(Affine::translate(x, y).then(Affine::rotate(angle))),
                    change,
                ))
            }
        }
    }

    /// The concrete values list for `values`.
    fn expand(&self, values: &Values, base: Option<&Value>) -> Option<Vec<Value>> {
        Some(match values {
            Values::List(list) => list.clone(),
            Values::To(to) => vec![base.cloned().unwrap_or_else(|| to.clone()), to.clone()],
            Values::By(by) => vec![by.scale(0.0)?, by.clone()],
            Values::FromBy(from, by) => vec![from.clone(), from.add(by)?],
        })
    }

    /// The value at fraction `p` through `list` under the calc mode.
    fn keyed(&self, list: &[Value], p: f64) -> Option<(Value, Change)> {
        let n = list.len();
        if n == 0 {
            return None;
        }
        if n == 1 || self.set {
            return Some((list[0].clone(), Change::At(Vec::new())));
        }
        let interpolates = list
            .windows(2)
            .all(|w| w[0].interpolate(&w[1], 0.0).is_some());
        let discrete = self.calc_mode == CalcMode::Discrete || !interpolates;
        if discrete {
            let times = self
                .key_times
                .clone()
                .filter(|t| t.len() == n)
                .unwrap_or_else(|| (0..n).map(|i| i as f64 / n as f64).collect());
            let index = times.iter().rposition(|t| *t <= p).unwrap_or(0);
            return Some((list[index].clone(), Change::At(times)));
        }
        let times = if self.calc_mode == CalcMode::Paced {
            let distances: Option<Vec<f64>> =
                list.windows(2).map(|w| w[0].distance(&w[1])).collect();
            match distances {
                Some(d) if d.iter().sum::<f64>() > 0.0 => {
                    let total: f64 = d.iter().sum();
                    let mut acc = 0.0;
                    std::iter::once(0.0)
                        .chain(d.iter().map(|x| {
                            acc += x;
                            acc / total
                        }))
                        .collect()
                }
                _ => even(n),
            }
        } else {
            self.key_times
                .clone()
                .filter(|t| t.len() == n)
                .unwrap_or_else(|| even(n))
        };
        let segment = times.iter().rposition(|t| *t <= p).unwrap_or(0).min(n - 2);
        let (t0, t1) = (times[segment], times[segment + 1]);
        let mut local = if t1 > t0 {
            ((p - t0) / (t1 - t0)).clamp(0.0, 1.0)
        } else {
            1.0
        };
        if self.calc_mode == CalcMode::Spline
            && let Some([x1, y1, x2, y2]) = self.key_splines.as_ref().and_then(|s| s.get(segment))
        {
            local = bezier(local, *x1, *y1, *x2, *y2);
        }
        Some((
            list[segment].interpolate(&list[segment + 1], local)?,
            Change::Continuous,
        ))
    }
}

fn split_list(text: &str) -> Vec<&str> {
    text.split(';')
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .collect()
}

fn even(n: usize) -> Vec<f64> {
    (0..n).map(|i| i as f64 / (n - 1) as f64).collect()
}
