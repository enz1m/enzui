//! CSS: the cascade for every element, and CSS animations (`@keyframes` and
//! the `animation` properties).

#![allow(
    clippy::string_slice,
    reason = "the stylesheet scanners cut only at ASCII delimiters found by search or at `char_indices` positions"
)]

use super::dom::{Document, NodeId, Selectable};
use super::easing::{Easing, StepPosition};
use super::transform::{CssTransform, Reference};
use super::value::{Value, kind_of};

/// One element's declarations after the cascade, in the order they were last
/// set (so a later longhand overrides an earlier shorthand).
pub(super) type Declarations = Vec<(String, String)>;

/// The cascaded declarations of every node (empty for text nodes), and the
/// document's keyframes.
#[derive(Debug, Clone, Default)]
pub(super) struct Styles {
    pub(super) declarations: Vec<Declarations>,
    pub(super) keyframes: Vec<Keyframes>,
}

impl Styles {
    pub(super) fn get(&self, node: NodeId, name: &str) -> Option<&str> {
        self.declarations
            .get(node)?
            .iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub(super) fn keyframes(&self, name: &str) -> Option<&Keyframes> {
        // The last definition of a name wins.
        self.keyframes.iter().rev().find(|k| k.name == name)
    }
}

/// Cascade every element of `document`.
pub(super) fn cascade(document: &Document) -> Styles {
    let mut sheet_text = String::new();
    for node in document.elements() {
        if document.svg_name(node) == Some("style")
            && document
                .attribute(node, "type")
                .is_none_or(|t| t.trim().is_empty() || t.trim() == "text/css")
        {
            sheet_text.push_str(&document.text_content(node));
            sheet_text.push('\n');
        }
    }
    let keyframes = parse_keyframes(&sheet_text);
    let sheet = simplecss::StyleSheet::parse(&sheet_text);
    let mut declarations = vec![Vec::new(); document.nodes.len()];
    for node in document.elements() {
        let element = Selectable { document, node };
        let mut normal: Vec<(&str, &str)> = Vec::new();
        let mut important: Vec<(&str, &str)> = Vec::new();
        for rule in &sheet.rules {
            if rule.selector.matches(&element) {
                for declaration in &rule.declarations {
                    let list = if declaration.important {
                        &mut important
                    } else {
                        &mut normal
                    };
                    list.push((declaration.name, declaration.value));
                }
            }
        }
        let inline: Vec<simplecss::Declaration<'_>> = document
            .attribute(node, "style")
            .map(|style| simplecss::DeclarationTokenizer::from(style).collect())
            .unwrap_or_default();
        for declaration in &inline {
            let list = if declaration.important {
                &mut important
            } else {
                &mut normal
            };
            list.push((declaration.name, declaration.value));
        }
        let mut out: Declarations = Vec::new();
        for (name, value) in normal.into_iter().chain(important.iter().copied()) {
            let name = name.trim().to_ascii_lowercase();
            out.retain(|(n, _)| *n != name);
            out.push((name, value.trim().to_owned()));
        }
        // An important declaration is not overridden by a later normal one,
        // which the order above already ensures.
        declarations[node] = out;
    }
    Styles {
        declarations,
        keyframes,
    }
}

/// One `@keyframes` rule: stops sorted by offset.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Keyframes {
    pub(super) name: String,
    pub(super) stops: Vec<Stop>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Stop {
    pub(super) offset: f64,
    pub(super) declarations: Declarations,
    /// `animation-timing-function` set in this keyframe: the easing from it
    /// to the next.
    pub(super) easing: Option<Easing>,
}

/// Every `@keyframes` (and vendor-prefixed) rule in `text`.
fn parse_keyframes(text: &str) -> Vec<Keyframes> {
    let text = strip_comments(text);
    let mut out = Vec::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find('@') {
        rest = &rest[at + 1..];
        let keyword_end = rest
            .find(|c: char| c.is_whitespace() || c == '{')
            .unwrap_or(rest.len());
        let keyword = rest[..keyword_end].to_ascii_lowercase();
        if !keyword.ends_with("keyframes") {
            continue;
        }
        let Some(open) = rest.find('{') else {
            break;
        };
        let name = rest[keyword_end..open]
            .trim()
            .trim_matches(['"', '\''])
            .to_owned();
        let Some(close) = matching_brace(rest, open) else {
            break;
        };
        let body = &rest[open + 1..close];
        rest = &rest[close + 1..];
        let mut stops: Vec<Stop> = Vec::new();
        let mut block = body;
        while let Some(open) = block.find('{') {
            let selectors = &block[..open];
            let Some(close) = block[open..].find('}').map(|c| c + open) else {
                break;
            };
            let declarations_text = &block[open + 1..close];
            block = &block[close + 1..];
            let mut declarations: Declarations = Vec::new();
            let mut easing = None;
            for declaration in simplecss::DeclarationTokenizer::from(declarations_text) {
                let name = declaration.name.trim().to_ascii_lowercase();
                if name == "animation-timing-function" {
                    easing = Easing::parse(declaration.value);
                    continue;
                }
                declarations.retain(|(n, _)| *n != name);
                declarations.push((name, declaration.value.trim().to_owned()));
            }
            for selector in selectors.split(',') {
                let offset = match selector.trim().to_ascii_lowercase().as_str() {
                    "from" => 0.0,
                    "to" => 1.0,
                    other => match other
                        .strip_suffix('%')
                        .and_then(|p| p.trim().parse::<f64>().ok())
                    {
                        Some(percent) if (0.0..=100.0).contains(&percent) => percent / 100.0,
                        _ => continue,
                    },
                };
                // Repeated offsets merge, the later declarations winning.
                match stops.iter_mut().find(|s| s.offset == offset) {
                    Some(stop) => {
                        for (name, value) in &declarations {
                            stop.declarations.retain(|(n, _)| n != name);
                            stop.declarations.push((name.clone(), value.clone()));
                        }
                        if easing.is_some() {
                            stop.easing.clone_from(&easing);
                        }
                    }
                    None => stops.push(Stop {
                        offset,
                        declarations: declarations.clone(),
                        easing: easing.clone(),
                    }),
                }
            }
        }
        stops.sort_by(|a, b| a.offset.total_cmp(&b.offset));
        if !name.is_empty() {
            out.push(Keyframes { name, stops });
        }
    }
    out
}

fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find("*/") {
            Some(end) => rest = &rest[start + 2 + end + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0_usize;
    for (index, byte) in text.bytes().enumerate().skip(open) {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Direction {
    Normal,
    Reverse,
    Alternate,
    AlternateReverse,
}

/// One running CSS animation on an element.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct CssAnimation {
    pub(super) name: String,
    pub(super) duration: f64,
    pub(super) easing: Easing,
    pub(super) delay: f64,
    pub(super) iterations: f64,
    pub(super) direction: Direction,
    pub(super) backwards: bool,
    pub(super) forwards: bool,
    pub(super) paused: bool,
}

impl Default for CssAnimation {
    fn default() -> Self {
        Self {
            name: String::new(),
            duration: 0.0,
            easing: Easing::EASE,
            delay: 0.0,
            iterations: 1.0,
            direction: Direction::Normal,
            backwards: false,
            forwards: false,
            paused: false,
        }
    }
}

/// Split at top-level commas (not inside parentheses).
fn split_commas(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0_i32;
    let mut start = 0;
    for (index, character) in text.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(text[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(text[start..].trim());
    parts
}

/// Split at top-level whitespace.
fn split_words(text: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut depth = 0_i32;
    let mut start = None;
    for (index, character) in text.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c.is_whitespace() && depth == 0 => {
                if let Some(s) = start.take() {
                    words.push(&text[s..index]);
                }
                continue;
            }
            _ => {}
        }
        if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(s) = start {
        words.push(&text[s..]);
    }
    words
}

fn css_time(text: &str) -> Option<f64> {
    let text = text.trim();
    if let Some(ms) = text.strip_suffix("ms") {
        return ms.trim().parse::<f64>().ok().map(|v| v / 1000.0);
    }
    text.strip_suffix('s')?.trim().parse().ok()
}

/// The CSS animations `declarations` (one element's cascade) run.
pub(super) fn animations(declarations: &Declarations) -> Vec<CssAnimation> {
    let mut list: Vec<CssAnimation> = Vec::new();
    for (name, value) in declarations {
        let items = split_commas(value);
        match name.as_str() {
            "animation" | "-webkit-animation" => {
                list = items.iter().map(|item| shorthand(item)).collect();
            }
            longhand
                if longhand.starts_with("animation-")
                    || longhand.starts_with("-webkit-animation-") =>
            {
                let property = longhand.trim_start_matches("-webkit-");
                if property == "animation-name" && list.len() < items.len() {
                    list.resize_with(items.len(), CssAnimation::default);
                }
                if list.is_empty() {
                    continue;
                }
                // Lists shorter than the names repeat.
                for (index, animation) in list.iter_mut().enumerate() {
                    let item = items[index % items.len()];
                    apply_longhand(animation, property, item);
                }
            }
            _ => {}
        }
    }
    list.retain(|a| !a.name.is_empty() && a.name != "none");
    list
}

fn apply_longhand(animation: &mut CssAnimation, property: &str, item: &str) {
    match property {
        "animation-name" => item
            .trim()
            .trim_matches(['"', '\''])
            .clone_into(&mut animation.name),
        "animation-duration" => {
            if let Some(d) = css_time(item) {
                animation.duration = d.max(0.0);
            }
        }
        "animation-timing-function" => {
            if let Some(e) = Easing::parse(item) {
                animation.easing = e;
            }
        }
        "animation-delay" => {
            if let Some(d) = css_time(item) {
                animation.delay = d;
            }
        }
        "animation-iteration-count" => {
            animation.iterations = match item.trim() {
                "infinite" => f64::INFINITY,
                n => n.parse().unwrap_or(1.0_f64).max(0.0),
            };
        }
        "animation-direction" => {
            if let Some(d) = direction(item.trim()) {
                animation.direction = d;
            }
        }
        "animation-fill-mode" => fill_mode(animation, item.trim()),
        "animation-play-state" => animation.paused = item.trim() == "paused",
        _ => {}
    }
}

fn direction(word: &str) -> Option<Direction> {
    Some(match word {
        "normal" => Direction::Normal,
        "reverse" => Direction::Reverse,
        "alternate" => Direction::Alternate,
        "alternate-reverse" => Direction::AlternateReverse,
        _ => return None,
    })
}

fn fill_mode(animation: &mut CssAnimation, word: &str) {
    let (backwards, forwards) = match word {
        "none" => (false, false),
        "forwards" => (false, true),
        "backwards" => (true, false),
        "both" => (true, true),
        _ => return,
    };
    animation.backwards = backwards;
    animation.forwards = forwards;
}

/// One item of the `animation` shorthand.
fn shorthand(item: &str) -> CssAnimation {
    let mut animation = CssAnimation::default();
    let mut times = 0;
    for word in split_words(item) {
        let lower = word.to_ascii_lowercase();
        if let Some(time) = css_time(&lower) {
            if times == 0 {
                animation.duration = time.max(0.0);
            } else {
                animation.delay = time;
            }
            times += 1;
        } else if let Some(easing) = Easing::parse(&lower) {
            animation.easing = easing;
        } else if lower == "infinite" {
            animation.iterations = f64::INFINITY;
        } else if let Ok(count) = lower.parse::<f64>() {
            animation.iterations = count.max(0.0);
        } else if let Some(d) = direction(&lower) {
            animation.direction = d;
        } else if matches!(lower.as_str(), "forwards" | "backwards" | "both") {
            fill_mode(&mut animation, &lower);
        } else if lower == "paused" {
            animation.paused = true;
        } else if lower == "running" || lower == "none" && !animation.name.is_empty() {
        } else {
            word.trim_matches(['"', '\''])
                .clone_into(&mut animation.name);
        }
    }
    animation
}

/// Where a CSS animation stands at a document time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Progress {
    /// The directed progress through the keyframes, `0..=1`.
    pub(super) progress: f64,
    /// Seconds per unit of progress, signed by the direction of travel; zero
    /// when frozen or paused.
    pub(super) rate: f64,
}

impl CssAnimation {
    /// The progress at document time `time`, or `None` when the animation
    /// shows nothing then.
    pub(super) fn progress(&self, time: f64) -> Option<Progress> {
        let active = if self.paused { 0.0 } else { time - self.delay };
        let directed = |iteration: f64, p: f64| {
            let odd = iteration.rem_euclid(2.0) >= 1.0;
            match (self.direction, odd) {
                (Direction::Normal, _)
                | (Direction::Alternate, false)
                | (Direction::AlternateReverse, true) => p,
                _ => 1.0 - p,
            }
        };
        let sign = |iteration: f64| {
            if directed(iteration, 1.0) == 1.0 {
                1.0
            } else {
                -1.0
            }
        };
        if active < 0.0 {
            return self.backwards.then(|| Progress {
                progress: directed(0.0, 0.0),
                rate: 0.0,
            });
        }
        let total = self.duration * self.iterations;
        if self.duration <= 0.0 || active >= total {
            if !(self.forwards || self.duration <= 0.0 && self.backwards) {
                return None;
            }
            let iterations = self.iterations;
            let (iteration, p) = if iterations.is_infinite() {
                (0.0, 1.0)
            } else if iterations.fract() == 0.0 && iterations > 0.0 {
                (iterations - 1.0, 1.0)
            } else {
                (iterations.floor(), iterations.fract())
            };
            return Some(Progress {
                progress: directed(iteration, p),
                rate: 0.0,
            });
        }
        let iteration = (active / self.duration).floor();
        let p = active / self.duration - iteration;
        Some(Progress {
            progress: directed(iteration, p),
            rate: if self.paused {
                0.0
            } else {
                sign(iteration) / self.duration
            },
        })
    }
}

/// A sampled CSS animation value.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Sampled {
    Text(String),
    Transform(CssTransform),
}

/// The value of `property` animated by `keyframes` at `progress`, with
/// `base` the element's own value (for keyframes that leave it out), or
/// `None` when the keyframes give nothing to show. Also returns the next
/// progress at which a value that only changes in steps changes, or `None`
/// when it changes continuously.
pub(super) fn sample(
    keyframes: &Keyframes,
    animation: &CssAnimation,
    property: &str,
    progress: Progress,
    base: Option<&str>,
    reference: Reference,
) -> Option<(Sampled, Change)> {
    let mut stops: Vec<(f64, &str, &Easing)> = keyframes
        .stops
        .iter()
        .filter_map(|stop| {
            let value = stop
                .declarations
                .iter()
                .rev()
                .find(|(n, _)| n == property)?;
            Some((
                stop.offset,
                value.1.as_str(),
                stop.easing.as_ref().unwrap_or(&animation.easing),
            ))
        })
        .collect();
    if stops.is_empty() {
        return None;
    }
    if stops[0].0 > 0.0 {
        let base = base.unwrap_or(stops[0].1);
        stops.insert(0, (0.0, base, &animation.easing));
    }
    if stops.last().is_some_and(|s| s.0 < 1.0) {
        let base = base.unwrap_or(stops[stops.len() - 1].1);
        stops.push((1.0, base, &animation.easing));
    }
    let p = progress.progress;
    let index = stops
        .iter()
        .rposition(|s| s.0 <= p)
        .unwrap_or(0)
        .min(stops.len().saturating_sub(2));
    let (o0, v0, easing) = stops[index];
    let (o1, v1, _) = stops.get(index + 1).copied().unwrap_or(stops[index]);
    let span = o1 - o0;
    let local = if span > 0.0 {
        ((p - o0) / span).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let eased = easing.apply(local);
    let (value, continuous) = if property == "transform" {
        let (Some(a), Some(b)) = (CssTransform::parse(v0), CssTransform::parse(v1)) else {
            return Some((
                Sampled::Text(if eased < 0.5 { v0 } else { v1 }.to_owned()),
                Change::Steps(critical(&stops, None)),
            ));
        };
        (
            Sampled::Transform(a.interpolate(&b, eased, reference)),
            true,
        )
    } else {
        let kind = kind_of(property);
        let unwrap = |v: &str| unwrap_path(property, v);
        let (a, b) = (
            Value::parse(&unwrap(v0), kind),
            Value::parse(&unwrap(v1), kind),
        );
        match a.interpolate(&b, eased) {
            Some(mixed) => (Sampled::Text(rewrap_path(property, &mixed.to_text())), true),
            None => (
                Sampled::Text(if eased < 0.5 { v0 } else { v1 }.to_owned()),
                false,
            ),
        }
    };
    let steps = match easing {
        Easing::Steps(..) => true,
        _ => !continuous,
    };
    let change = if !steps {
        Change::Continuous
    } else {
        Change::Steps(critical(&stops, Some((o0, o1, easing, !continuous))))
    };
    Some((value, change))
}

/// How a sampled value changes over progress.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Change {
    Continuous,
    /// Only at these progress values.
    Steps(Vec<f64>),
}

/// The progress values where a stepwise value can change: every stop, every
/// step of the current segment's easing, and the midpoint where a discrete
/// value flips.
fn critical(
    stops: &[(f64, &str, &Easing)],
    segment: Option<(f64, f64, &Easing, bool)>,
) -> Vec<f64> {
    let mut points: Vec<f64> = stops.iter().map(|s| s.0).collect();
    for window in stops.windows(2) {
        let (o0, _, easing) = window[0];
        let o1 = window[1].0;
        if let Easing::Steps(count, position) = easing {
            let jumps = match position {
                StepPosition::None => count.saturating_sub(1),
                _ => *count,
            };
            for k in 1..=*count {
                let _ = jumps;
                points.push(o0 + (o1 - o0) * f64::from(k) / f64::from(*count));
            }
        }
        // A discrete value flips where its easing reaches one half: found by
        // bisection on the easing.
        let midpoint = solve_half(easing);
        points.push(o0 + (o1 - o0) * midpoint);
    }
    let _ = segment;
    points.sort_by(f64::total_cmp);
    points.dedup();
    points
}

fn solve_half(easing: &Easing) -> f64 {
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..40 {
        let mid = 0.5 * (low + high);
        if easing.apply(mid) < 0.5 {
            low = mid;
        } else {
            high = mid;
        }
    }
    high
}

/// CSS `d: path("…")` and `offset-path`-style wrapping, removed for parsing.
fn unwrap_path(property: &str, value: &str) -> String {
    if property == "d"
        && let Some(inner) = value
            .trim()
            .strip_prefix("path(")
            .and_then(|v| v.strip_suffix(')'))
    {
        return inner.trim().trim_matches(['"', '\'']).to_owned();
    }
    value.to_owned()
}

fn rewrap_path(property: &str, value: &str) -> String {
    if property == "d" {
        super::value::denormalise_path(value)
    } else {
        value.to_owned()
    }
}
