//! Timing functions: CSS easing (`cubic-bezier`, `steps`, `linear()` and the
//! keywords) and SMIL `keySplines`.

/// A CSS timing function.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Easing {
    /// `cubic-bezier(x1, y1, x2, y2)`; `linear` is the straight one.
    Bezier([f64; 4]),
    /// `steps(count, position)`.
    Steps(u32, StepPosition),
    /// `linear(…)`: piecewise-linear `(input, output)` stops.
    Points(Vec<(f64, f64)>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StepPosition {
    Start,
    End,
    None,
    Both,
}

impl Easing {
    pub(super) const LINEAR: Self = Self::Bezier([0.0, 0.0, 1.0, 1.0]);
    pub(super) const EASE: Self = Self::Bezier([0.25, 0.1, 0.25, 1.0]);

    /// A CSS timing function, or `None` when unreadable.
    pub(super) fn parse(text: &str) -> Option<Self> {
        let text = text.trim().to_ascii_lowercase();
        Some(match text.as_str() {
            "linear" => Self::LINEAR,
            "ease" => Self::EASE,
            "ease-in" => Self::Bezier([0.42, 0.0, 1.0, 1.0]),
            "ease-out" => Self::Bezier([0.0, 0.0, 0.58, 1.0]),
            "ease-in-out" => Self::Bezier([0.42, 0.0, 0.58, 1.0]),
            "step-start" => Self::Steps(1, StepPosition::Start),
            "step-end" => Self::Steps(1, StepPosition::End),
            _ => {
                let (name, rest) = text.split_once('(')?;
                let inner = rest.strip_suffix(')')?;
                let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
                match name.trim() {
                    "cubic-bezier" => {
                        let values: Vec<f64> = parts
                            .iter()
                            .map(|p| p.parse().ok())
                            .collect::<Option<_>>()?;
                        let [x1, y1, x2, y2] = <[f64; 4]>::try_from(values).ok()?;
                        if !(0.0..=1.0).contains(&x1) || !(0.0..=1.0).contains(&x2) {
                            return None;
                        }
                        Self::Bezier([x1, y1, x2, y2])
                    }
                    "steps" => {
                        let count: u32 = parts.first()?.parse().ok()?;
                        let position = match parts.get(1).copied().unwrap_or("end") {
                            "start" | "jump-start" => StepPosition::Start,
                            "end" | "jump-end" => StepPosition::End,
                            "jump-none" => StepPosition::None,
                            "jump-both" => StepPosition::Both,
                            _ => return None,
                        };
                        let minimum = if position == StepPosition::None { 2 } else { 1 };
                        if count < minimum {
                            return None;
                        }
                        Self::Steps(count, position)
                    }
                    "linear" => Self::Points(linear_stops(&parts)?),
                    _ => return None,
                }
            }
        })
    }

    /// The eased output for input progress `t` in `[0, 1]`.
    pub(super) fn apply(&self, t: f64) -> f64 {
        match self {
            // Control points on the diagonal: exactly linear.
            Self::Bezier([x1, y1, x2, y2]) if x1 == y1 && x2 == y2 => t.clamp(0.0, 1.0),
            Self::Bezier([x1, y1, x2, y2]) => bezier(t, *x1, *y1, *x2, *y2),
            Self::Steps(count, position) => {
                let count = f64::from(*count);
                let mut step = (t * count).floor();
                if matches!(position, StepPosition::Start | StepPosition::Both) {
                    step += 1.0;
                }
                let jumps = match position {
                    StepPosition::Start | StepPosition::End => count,
                    StepPosition::None => count - 1.0,
                    StepPosition::Both => count + 1.0,
                };
                if t >= 1.0 {
                    return 1.0;
                }
                (step / jumps).clamp(0.0, 1.0)
            }
            Self::Points(points) => {
                let t = t.clamp(0.0, 1.0);
                let index = points
                    .iter()
                    .rposition(|(input, _)| *input <= t)
                    .unwrap_or(0);
                let (x0, y0) = points[index];
                match points.get(index + 1) {
                    Some(&(x1, y1)) if x1 > x0 => y0 + (y1 - y0) * (t - x0) / (x1 - x0),
                    _ => y0,
                }
            }
        }
    }
}

/// `linear()` stops with missing inputs filled in as CSS Easing 2 says.
fn linear_stops(parts: &[&str]) -> Option<Vec<(f64, f64)>> {
    let mut stops: Vec<(Option<f64>, f64)> = Vec::new();
    for part in parts {
        let mut words = part.split_whitespace();
        let output: f64 = words.next()?.parse().ok()?;
        let inputs: Vec<f64> = words
            .map(|w| {
                w.strip_suffix('%')
                    .and_then(|v| v.parse::<f64>().ok())
                    .map(|v| v / 100.0)
            })
            .collect::<Option<_>>()?;
        if inputs.is_empty() {
            stops.push((None, output));
        }
        for input in inputs.into_iter().take(2) {
            stops.push((Some(input), output));
        }
    }
    if stops.len() < 2 {
        return None;
    }
    if stops[0].0.is_none() {
        stops[0].0 = Some(0.0);
    }
    let last = stops.len() - 1;
    if stops[last].0.is_none() {
        stops[last].0 = Some(1.0);
    }
    // Inputs never go backwards.
    let mut largest = f64::NEG_INFINITY;
    for stop in &mut stops {
        if let Some(input) = stop.0.as_mut() {
            *input = input.max(largest);
            largest = *input;
        }
    }
    // Runs without inputs spread evenly between their neighbours.
    let mut index = 0;
    while index < stops.len() {
        if stops[index].0.is_some() {
            index += 1;
            continue;
        }
        let start = index - 1;
        let end = (index..stops.len()).find(|i| stops[*i].0.is_some())?;
        let (a, b) = (stops[start].0?, stops[end].0?);
        for (i, stop) in stops.iter_mut().enumerate().take(end).skip(index) {
            stop.0 = Some(a + (b - a) * (i - start) as f64 / (end - start) as f64);
        }
        index = end;
    }
    stops
        .into_iter()
        .map(|(input, output)| Some((input?, output)))
        .collect()
}

/// The cubic Bézier from (0, 0) to (1, 1) through `(x1, y1)` and `(x2, y2)`
/// at input `t`, its parameter found by Newton steps with a bisection
/// fallback.
pub(super) fn bezier(t: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    if t <= 0.0 {
        return if x1 > 0.0 {
            y1 / x1 * t
        } else if y1 == 0.0 && x2 > 0.0 {
            y2 / x2 * t
        } else {
            0.0
        };
    }
    if t >= 1.0 {
        return 1.0;
    }
    let curve = |s: f64, a: f64, b: f64| {
        let r = 1.0 - s;
        3.0 * r * r * s * a + 3.0 * r * s * s * b + s * s * s
    };
    let slope = |s: f64, a: f64, b: f64| {
        let r = 1.0 - s;
        3.0 * r * r * a + 6.0 * r * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    let mut s = t;
    for _ in 0..8 {
        let error = curve(s, x1, x2) - t;
        if error.abs() < 1e-7 {
            return curve(s, y1, y2);
        }
        let d = slope(s, x1, x2);
        if d.abs() < 1e-6 {
            break;
        }
        s -= error / d;
    }
    let (mut low, mut high) = (0.0, 1.0);
    s = t;
    for _ in 0..40 {
        if curve(s, x1, x2) < t {
            low = s;
        } else {
            high = s;
        }
        s = 0.5 * (low + high);
    }
    curve(s, y1, y2)
}
