//! A document at one instant: every animation applied, written out as plain
//! SVG for the static renderer.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use super::css::{self, CssAnimation, Sampled, Styles};
use super::dom::{Document, ElementOutput, NodeId};
use super::geometry::bounding_box;
use super::smil::{self, Animation, Effect, Output, simple_time};
use super::timing::{Phase, Timeline, Timing};
use super::transform::{Affine, CssTransform, Reference, origin, parse_svg_transform};
use super::value::{Kind, Value};

/// How far past the current instant intervals are generated each frame.
const LOOKAHEAD: f64 = 1.0;

/// Properties written into `style`; every other animated name is an
/// attribute.
const PROPERTIES: &[&str] = &[
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "opacity",
    "visibility",
    "display",
    "color",
    "stop-color",
    "stop-opacity",
    "flood-color",
    "flood-opacity",
    "lighting-color",
    "font-size",
    "font-family",
    "font-weight",
    "font-style",
    "font-variant",
    "font-stretch",
    "letter-spacing",
    "word-spacing",
    "text-anchor",
    "text-decoration",
    "clip-path",
    "clip-rule",
    "mask",
    "filter",
    "marker-start",
    "marker-mid",
    "marker-end",
    "paint-order",
    "mix-blend-mode",
    "isolation",
    "overflow",
    "shape-rendering",
    "text-rendering",
    "image-rendering",
    "color-interpolation",
    "color-interpolation-filters",
    "dominant-baseline",
    "baseline-shift",
    "alignment-baseline",
    "mask-type",
];

/// Geometry properties CSS may set, which the renderer reads only as
/// attributes.
const GEOMETRY: &[&str] = &[
    "cx", "cy", "r", "rx", "ry", "x", "y", "width", "height", "d",
];

/// Elements left out of every frame: they draw nothing.
const SKIPPED: &[&str] = &[
    "style",
    "script",
    "animate",
    "set",
    "animateTransform",
    "animateMotion",
    "animateColor",
];

fn is_property(name: &str) -> bool {
    PROPERTIES.contains(&name)
}

/// When the scene next changes after a sampled instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Next {
    /// Something moves continuously: every frame.
    Continuous,
    /// At this document time.
    At(f64),
    /// Never: everything has settled.
    Never,
}

impl Next {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Continuous, _) | (_, Self::Continuous) => Self::Continuous,
            (Self::At(a), Self::At(b)) => Self::At(a.min(b)),
            (Self::At(a), Self::Never) | (Self::Never, Self::At(a)) => Self::At(a),
            (Self::Never, Self::Never) => Self::Never,
        }
    }
}

/// A parsed animated document.
#[derive(Debug)]
pub(super) struct Scene {
    pub(super) document: Document,
    styles: Styles,
    css: Vec<(NodeId, Vec<CssAnimation>)>,
    pub(super) animations: Vec<Animation>,
    pub(super) timings: Vec<Timing>,
    base: Vec<Option<ElementOutput>>,
    /// The nearest viewport, for `transform-box: view-box`.
    view_box: Reference,
    /// Elements some animation waits on a pointer event from, with the id
    /// they carry in every frame (their own, or one given to them).
    targets: Vec<(NodeId, String)>,
}

/// The document at one instant.
#[derive(Debug, Clone)]
pub(super) struct Frame {
    /// The elements this instant changes, written out by [`Scene::text`]
    /// only when the frame is not already rasterised.
    outputs: HashMap<NodeId, ElementOutput>,
    /// Equal for two frames that would draw the same.
    pub(super) key: u64,
    pub(super) next: Next,
}

impl Scene {
    pub(super) fn parse(text: &str) -> Result<Self, String> {
        let document = Document::parse(text)?;
        let styles = css::cascade(&document);
        let css = document
            .elements()
            .filter_map(|node| {
                let list = css::animations(&styles.declarations[node]);
                let list: Vec<CssAnimation> = list
                    .into_iter()
                    .filter(|a| styles.keyframes(&a.name).is_some())
                    .collect();
                (!list.is_empty()).then_some((node, list))
            })
            .collect();
        let (animations, timings) = smil::collect(&document);
        let view_box = view_box(&document);
        let mut targets: Vec<(NodeId, String)> = timings
            .iter()
            .flat_map(|t| t.begin.iter().chain(&t.end))
            .filter_map(|c| match c {
                super::timing::Condition::Event { node, .. } => Some(*node),
                _ => None,
            })
            .map(|node| {
                let id = document
                    .attribute(node, "id")
                    .map_or_else(|| format!("enz-event-target-{node}"), str::to_owned);
                (node, id)
            })
            .collect();
        targets.sort();
        targets.dedup();
        let mut scene = Self {
            document,
            styles,
            css,
            animations,
            timings,
            base: Vec::new(),
            view_box,
            targets,
        };
        scene.base = (0..scene.document.nodes.len())
            .map(|node| scene.base_output(node))
            .collect();
        Ok(scene)
    }

    /// Elements waiting on pointer events, with the id each carries.
    pub(super) fn event_targets(&self) -> impl Iterator<Item = (NodeId, String)> + '_ {
        self.targets.iter().cloned()
    }

    /// Whether anything in the document can ever change.
    pub(super) fn is_animated(&self) -> bool {
        !self.animations.is_empty() || !self.css.is_empty()
    }

    /// The `transform-box` reference of `node`.
    fn reference(&self, node: NodeId) -> Reference {
        match self.styles.get(node, "transform-box").map(str::trim) {
            Some("fill-box" | "stroke-box" | "content-box" | "border-box") => {
                bounding_box(&self.document, node).unwrap_or_default()
            }
            _ => self.view_box,
        }
    }

    /// The transform `node` has before any animation, origin applied, or
    /// `None` when it keeps its own attribute untouched.
    fn base_transform(&self, node: NodeId, css_transform: Option<&CssTransform>) -> Option<Affine> {
        let origin_text = self
            .styles
            .get(node, "transform-origin")
            .or_else(|| self.document.attribute(node, "transform-origin"));
        let css_static = self
            .styles
            .get(node, "transform")
            .and_then(CssTransform::parse);
        let css_transform = css_transform.or(css_static.as_ref());
        if css_transform.is_none()
            && origin_text.is_none()
            && self.styles.get(node, "transform-box").is_none()
        {
            return None;
        }
        let reference = self.reference(node);
        let matrix = match css_transform {
            Some(css) => css.matrix(reference),
            None => self
                .document
                .attribute(node, "transform")
                .and_then(parse_svg_transform)
                .unwrap_or(Affine::IDENTITY),
        };
        let (x, y) = origin(origin_text.unwrap_or("0 0"), reference);
        Some(matrix.about(x, y))
    }

    /// `node` written out with no animation applied.
    fn base_output(&self, node: NodeId) -> Option<ElementOutput> {
        let name = self.document.svg_name(node)?;
        if SKIPPED.contains(&name) {
            return Some(ElementOutput {
                omit: true,
                ..ElementOutput::default()
            });
        }
        let declarations = &self.styles.declarations[node];
        let mut output = ElementOutput::default();
        let mut style = String::new();
        for (property, value) in declarations {
            if GEOMETRY.contains(&property.as_str()) {
                output
                    .attributes
                    .push((property.clone(), Some(geometry_value(property, value))));
            } else if is_property(property) {
                push_declaration(&mut style, property, value);
            }
        }
        // Always written, so a stylesheet the frame leaves out cannot apply.
        output.style = Some(style);
        if self.document.attribute(node, "id").is_none()
            && let Some((_, id)) = self.targets.iter().find(|(n, _)| *n == node)
        {
            output.attributes.push(("id".into(), Some(id.clone())));
        }
        output.attributes.push(("class".into(), None));
        if let Some(matrix) = self.base_transform(node, None) {
            output
                .attributes
                .push(("transform".into(), Some(matrix.to_text())));
            output.attributes.push(("transform-origin".into(), None));
        }
        Some(output)
    }

    /// The underlying value of `name` on `node`, for `to` animations and
    /// additive sums.
    fn underlying(&self, node: NodeId, name: &str, kind: Kind) -> Option<Value> {
        let text = if is_property(name) || GEOMETRY.contains(&name) {
            self.styles
                .get(node, name)
                .or_else(|| self.document.attribute(node, name))
        } else {
            self.document.attribute(node, name)
        }
        .map(|t| {
            if GEOMETRY.contains(&name) {
                geometry_value(name, t)
            } else {
                t.to_owned()
            }
        })
        .or_else(|| initial(name).map(str::to_owned))?;
        Some(Value::parse(&text, kind))
    }

    /// The document at `time`, its intervals generated in `timeline`.
    pub(super) fn frame(&self, timeline: &mut Timeline, time: f64) -> Frame {
        timeline.extend(&self.timings, time + LOOKAHEAD);
        let mut next = Next::Never;
        // Per element: property and attribute overrides, and transforms.
        let mut properties: HashMap<NodeId, Vec<(String, String)>> = HashMap::new();
        let mut attributes: HashMap<NodeId, Vec<(String, String)>> = HashMap::new();
        let mut css_transforms: HashMap<NodeId, CssTransform> = HashMap::new();

        // CSS animations first: SMIL, applied after, wins over them.
        for (node, list) in &self.css {
            for animation in list {
                let Some(keyframes) = self.styles.keyframes(&animation.name) else {
                    continue;
                };
                // Waiting out its delay: the next change is its start.
                if time < animation.delay && !animation.paused {
                    next = next.merge(Next::At(animation.delay));
                }
                let Some(progress) = animation.progress(time) else {
                    continue;
                };
                let mut names: Vec<&str> = keyframes
                    .stops
                    .iter()
                    .flat_map(|s| s.declarations.iter().map(|(n, _)| n.as_str()))
                    .collect();
                names.sort_unstable();
                names.dedup();
                for name in names {
                    let reference = self.reference(*node);
                    // Keyframes that leave an end out run from the element's
                    // own value, or the property's initial one.
                    let base = self
                        .styles
                        .get(*node, name)
                        .or_else(|| self.document.attribute(*node, name))
                        .or_else(|| initial(name));
                    let Some((value, change)) =
                        css::sample(keyframes, animation, name, progress, base, reference)
                    else {
                        continue;
                    };
                    next = next.merge(css_next(time, progress, &change));
                    match value {
                        Sampled::Transform(transform) => {
                            css_transforms.insert(*node, transform);
                        }
                        Sampled::Text(text) if GEOMETRY.contains(&name) => {
                            let list = attributes.entry(*node).or_default();
                            list.retain(|(n, _)| n != name);
                            list.push((name.to_owned(), geometry_value(name, &text)));
                        }
                        Sampled::Text(text) => {
                            let list = properties.entry(*node).or_default();
                            list.retain(|(n, _)| n != name);
                            list.push((name.to_owned(), text));
                        }
                    }
                }
            }
        }

        // SMIL: in each sandwich, earlier-begun animations sit lower.
        let mut active: Vec<(f64, usize, Phase)> = Vec::new();
        for index in 0..self.animations.len() {
            let phase = timeline.phase(&self.timings, index, time);
            match phase {
                Phase::Idle => {}
                Phase::Active { begin, .. } | Phase::Frozen { begin, .. } => {
                    active.push((begin, index, phase));
                }
            }
            if let Some(end) = timeline.active_end(index, time) {
                next = next.merge(Next::At(end));
            }
        }
        if let Some(begin) = timeline.next_begin(&self.timings, time) {
            next = next.merge(Next::At(begin));
        }
        active.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut values: HashMap<(NodeId, String), Value> = HashMap::new();
        let mut transforms: HashMap<NodeId, Affine> = HashMap::new();
        let mut motions: HashMap<NodeId, Affine> = HashMap::new();
        for (begin, index, phase) in active {
            let animation = &self.animations[index];
            let dur = self.timings[index].dur;
            let Some(simple) = simple_time(phase, dur) else {
                continue;
            };
            let target = animation.target;
            let base = match &animation.effect {
                Effect::Attribute { name, kind, .. } => values
                    .get(&(target, name.clone()))
                    .cloned()
                    .or_else(|| self.underlying(target, name, *kind)),
                _ => None,
            };
            let Some((output, change)) = animation.sample(simple, base.as_ref()) else {
                continue;
            };
            if let Phase::Active { .. } = phase {
                next = next.merge(smil_next(begin, simple, dur, &change));
            }
            match (&animation.effect, output) {
                (Effect::Attribute { name, kind, .. }, Output::Value(value)) => {
                    let key = (target, name.clone());
                    let combined = if animation.additive {
                        let under = values
                            .get(&key)
                            .cloned()
                            .or_else(|| self.underlying(target, name, *kind));
                        under.and_then(|u| u.add(&value)).unwrap_or(value)
                    } else {
                        value
                    };
                    values.insert(key, combined);
                }
                (Effect::Transform { .. }, Output::Transform(matrix)) => {
                    let combined = match (animation.additive, transforms.get(&target)) {
                        (true, Some(under)) => under.then(matrix),
                        (true, None) => self.transform_of(target, &css_transforms).then(matrix),
                        (false, _) => matrix,
                    };
                    transforms.insert(target, combined);
                }
                (Effect::Motion { .. }, Output::Transform(matrix)) => {
                    let combined = match (animation.additive, motions.get(&target)) {
                        (true, Some(under)) => under.then(matrix),
                        _ => matrix,
                    };
                    motions.insert(target, combined);
                }
                _ => {}
            }
        }
        // Sorted, so attributes are written in one order every frame.
        let mut values: Vec<((NodeId, String), Value)> = values.into_iter().collect();
        values.sort_by(|a, b| a.0.cmp(&b.0));
        for ((node, name), value) in values {
            let text = if name == "d" {
                super::value::denormalise_path(&value.to_text())
            } else {
                value.to_text()
            };
            if is_property(&name) {
                let list = properties.entry(node).or_default();
                list.retain(|(n, _)| *n != name);
                list.push((name, text));
            } else {
                let list = attributes.entry(node).or_default();
                list.retain(|(n, _)| *n != name);
                list.push((name, text));
            }
        }

        // Write out every changed element.
        let mut changed: Vec<NodeId> = properties
            .keys()
            .chain(attributes.keys())
            .chain(css_transforms.keys())
            .chain(transforms.keys())
            .chain(motions.keys())
            .copied()
            .collect();
        changed.sort_unstable();
        changed.dedup();
        let mut outputs: HashMap<NodeId, ElementOutput> = HashMap::new();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for node in changed {
            let mut output = self.base[node].clone().unwrap_or_default();
            if output.omit {
                continue;
            }
            if let Some(list) = properties.get(&node) {
                let mut style = String::new();
                for (property, value) in &self.styles.declarations[node] {
                    if is_property(property) && !list.iter().any(|(n, _)| n == property) {
                        push_declaration(&mut style, property, value);
                    }
                }
                for (property, value) in list {
                    push_declaration(&mut style, property, value);
                }
                output.style = Some(style);
            }
            if let Some(list) = attributes.get(&node) {
                for (name, value) in list {
                    output.attributes.retain(|(n, _)| n != name);
                    output.attributes.push((name.clone(), Some(value.clone())));
                }
            }
            let animated_transform = css_transforms.contains_key(&node)
                || transforms.contains_key(&node)
                || motions.contains_key(&node);
            if animated_transform {
                let base = transforms
                    .get(&node)
                    .copied()
                    .unwrap_or_else(|| self.transform_of(node, &css_transforms));
                let matrix = motions.get(&node).map_or(base, |motion| motion.then(base));
                output
                    .attributes
                    .retain(|(n, _)| n != "transform" && n != "transform-origin");
                output
                    .attributes
                    .push(("transform".into(), Some(matrix.to_text())));
                output.attributes.push(("transform-origin".into(), None));
            }
            node.hash(&mut hasher);
            output.style.hash(&mut hasher);
            for (name, value) in &output.attributes {
                name.hash(&mut hasher);
                value.hash(&mut hasher);
            }
            outputs.insert(node, output);
        }
        Frame {
            outputs,
            key: hasher.finish(),
            next,
        }
    }

    /// `frame` written out as SVG text.
    pub(super) fn text(&self, frame: &Frame) -> String {
        self.document
            .serialize(&|node| frame.outputs.get(&node).or(self.base[node].as_ref()))
    }

    /// `node`'s transform before SMIL transforms: the animated or static CSS
    /// transform with its origin, or its attribute.
    fn transform_of(&self, node: NodeId, css_transforms: &HashMap<NodeId, CssTransform>) -> Affine {
        self.base_transform(node, css_transforms.get(&node))
            .or_else(|| {
                self.document
                    .attribute(node, "transform")
                    .and_then(parse_svg_transform)
            })
            .unwrap_or(Affine::IDENTITY)
    }
}

fn push_declaration(style: &mut String, property: &str, value: &str) {
    if !style.is_empty() {
        style.push(';');
    }
    style.push_str(property);
    style.push(':');
    style.push_str(value);
}

/// A CSS geometry value as the attribute takes it: `px` dropped, `path()`
/// unwrapped.
fn geometry_value(name: &str, value: &str) -> String {
    let value = value.trim();
    if name == "d" {
        return value
            .strip_prefix("path(")
            .and_then(|v| v.strip_suffix(')'))
            .map_or(value, |v| v.trim().trim_matches(['"', '\'']))
            .to_owned();
    }
    value.strip_suffix("px").unwrap_or(value).trim().to_owned()
}

/// The initial value of a property an animation reads without the element
/// setting it.
fn initial(name: &str) -> Option<&'static str> {
    Some(match name {
        "opacity" | "fill-opacity" | "stroke-opacity" | "stop-opacity" | "flood-opacity" => "1",
        "fill" => "black",
        "stroke" => "none",
        "stroke-width" => "1",
        "stroke-dashoffset" | "x" | "y" | "cx" | "cy" | "r" | "rx" | "ry" | "width" | "height"
        | "x1" | "y1" | "x2" | "y2" => "0",
        "visibility" => "visible",
        "display" => "inline",
        "stop-color" => "black",
        "transform" => "none",
        "stroke-dasharray" => "none",
        "stroke-miterlimit" => "4",
        "font-size" => "16",
        _ => return None,
    })
}

/// When a SMIL value next changes, from its interval `begin`, its place in
/// the simple duration and how it changes.
fn smil_next(
    begin: f64,
    simple: smil::SimpleTime,
    dur: Option<f64>,
    change: &smil::Change,
) -> Next {
    match change {
        smil::Change::Continuous => Next::Continuous,
        smil::Change::At(fractions) => {
            let Some(dur) = dur.filter(|d| *d > 0.0) else {
                return Next::Never;
            };
            let next_fraction = fractions
                .iter()
                .copied()
                .filter(|f| *f > simple.fraction + 1e-9)
                .fold(1.0_f64, f64::min);
            Next::At(begin + (simple.iteration + next_fraction) * dur)
        }
    }
}

/// When a CSS value that changes in steps next changes.
fn css_next(time: f64, progress: css::Progress, change: &css::Change) -> Next {
    match change {
        css::Change::Continuous if progress.rate != 0.0 => Next::Continuous,
        css::Change::Continuous => Next::Never,
        css::Change::Steps(points) => {
            if progress.rate == 0.0 {
                return Next::Never;
            }
            let p = progress.progress;
            let target = if progress.rate > 0.0 {
                points
                    .iter()
                    .copied()
                    .filter(|c| *c > p + 1e-9)
                    .fold(1.0_f64, f64::min)
            } else {
                points
                    .iter()
                    .copied()
                    .filter(|c| *c < p - 1e-9)
                    .fold(0.0_f64, f64::max)
            };
            Next::At(time + (target - p) / progress.rate)
        }
    }
}

fn view_box(document: &Document) -> Reference {
    let root = document.root;
    let numbers: Vec<f64> = document
        .attribute(root, "viewBox")
        .map(|v| {
            v.split(|c: char| c.is_whitespace() || c == ',')
                .filter(|s| !s.is_empty())
                .filter_map(|s| s.parse::<f64>().ok())
                .collect()
        })
        .unwrap_or_default();
    if let [_, _, width, height] = numbers[..] {
        return Reference {
            x: 0.0,
            y: 0.0,
            width,
            height,
        };
    }
    let length = |name: &str| {
        document
            .attribute(root, name)
            .and_then(|v| v.trim().trim_end_matches("px").parse::<f64>().ok())
            .unwrap_or(100.0)
    };
    Reference {
        x: 0.0,
        y: 0.0,
        width: length("width"),
        height: length("height"),
    }
}
