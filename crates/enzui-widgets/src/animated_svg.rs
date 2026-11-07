//! An SVG that plays its own animation: SMIL (`<animate>`, `<set>`,
//! `<animateTransform>`, `<animateMotion>`) and CSS (`@keyframes` with the
//! `animation` properties).
//!
//! [`AnimatedSvg::parse`] reads the document once; [`enz_animated_svg`]
//! shows it. A document with no animation draws as a still picture.
//!
//! # What plays
//!
//! SMIL animation as SVG 1.1 and SMIL Animation define it:
//! - `values`, `from`, `to` and `by`; `keyTimes` and `keySplines`; the
//!   `discrete`, `linear`, `paced` and `spline` calc modes; `additive` and
//!   `accumulate`; several animations on one attribute composed in priority
//!   order.
//! - `begin` and `end` lists of offsets, syncbases (`other.end+1s`), repeats
//!   (`other.repeat(2)`) and pointer events (`click`, `mouseover`,
//!   `button.mousedown`, …); `dur`, `repeatCount`, `repeatDur`, `min`,
//!   `max`, `restart` and `fill`.
//! - Numbers, lengths, lists, points, view boxes, dash arrays, colours and
//!   path data (`d`, between paths of the same commands) interpolate;
//!   everything else changes discretely.
//! - `animateTransform` of every type, and `animateMotion` along `path`,
//!   `mpath` or `values`, with `keyPoints` and `rotate="auto"`.
//!
//! CSS animation from `<style>` rules and `style` attributes: the
//! `animation` shorthand and longhands, every timing function (keywords,
//! `cubic-bezier()`, `steps()`, `linear()`), per-keyframe timing functions,
//! direction, fill mode and play state, and transforms with
//! `transform-origin` and `transform-box`. Stylesheets are applied with
//! selector specificity and `!important`.
//!
//! # Cost
//!
//! A frame is drawn only when what it shows changes. A value moving
//! continuously asks for the next display frame; one that changes in steps
//! (`set`, `discrete`, `steps()`) wakes the app only at its next step; one
//! that has finished asks for nothing. The animation pauses while scrolled out
//! of view or while the window is not drawing, and resumes where it stopped.
//! Each distinct frame is rasterised once and kept (up to a memory budget per
//! picture), so a looping animation costs rasterising only during its first
//! loop.
//!
//! With reduced motion (the desktop setting or the theme's policy) the
//! picture shows its first frame and never wakes. A named picture
//! (`.label("…")`) publishes one [`Role::Image`] node; without a label it is
//! decorative.

mod css;
mod dom;
mod easing;
mod geometry;
mod render;
mod scene;
mod smil;
mod timing;
mod transform;
mod value;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Weak};

use enzui_core::collect::SemanticsNode;
use enzui_core::semantics::{Role, Semantics};
use iced::advanced::image::{self as iced_image, Renderer as _};
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, renderer};
use iced::time::{Duration, Instant};
use iced::{ContentFit, Element, Event, Length, Rectangle, Size, mouse, window};

use scene::{Next, Scene};
use timing::Timeline;

/// Document time is sampled on this grid (per second), so a looping
/// animation revisits exactly the same states and its frames are reused.
const TIME_GRID: f64 = 240.0;

/// Rasterised frames kept per picture, in bytes. Admission stops when full
/// (nothing is evicted: a loop is played in order, where evicting the oldest
/// frame would always drop the next one needed).
const FRAME_BUDGET: usize = 48 * 1024 * 1024;

/// The longest a single frame may be, in pixels per side.
const MAX_EDGE: f32 = 8192.0;

/// Why a document could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnimatedSvgError {
    /// The bytes are not UTF-8 (or not gzip of it).
    NotText,
    /// The text is not an SVG document this reader understands.
    Unreadable(String),
}

impl std::fmt::Display for AnimatedSvgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotText => f.write_str("the document is not text"),
            Self::Unreadable(reason) => write!(f, "not a readable SVG: {reason}"),
        }
    }
}

impl std::error::Error for AnimatedSvgError {}

/// A parsed SVG document and its animations. Cheap to clone; every clone is
/// the same animation (showing it in two places plays it twice, each from
/// its own start).
#[derive(Debug, Clone)]
pub struct AnimatedSvg {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    scene: Scene,
    /// The document's own size, in its units.
    size: Size,
}

impl AnimatedSvg {
    /// Read an SVG document (`.svg`, or gzip-compressed `.svgz`).
    ///
    /// # Errors
    /// [`AnimatedSvgError`] when the bytes are not an SVG document.
    pub fn parse(bytes: &[u8]) -> Result<Self, AnimatedSvgError> {
        let inflated;
        let bytes = if bytes.starts_with(&[0x1f, 0x8b]) {
            use std::io::Read as _;
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(bytes)
                .take(64 * 1024 * 1024)
                .read_to_end(&mut out)
                .map_err(|_| AnimatedSvgError::NotText)?;
            inflated = out;
            &inflated[..]
        } else {
            bytes
        };
        let text = std::str::from_utf8(bytes).map_err(|_| AnimatedSvgError::NotText)?;
        Self::from_text(text)
    }

    /// Read an SVG document from text.
    ///
    /// # Errors
    /// [`AnimatedSvgError::Unreadable`] when it is not an SVG document.
    pub fn from_text(text: &str) -> Result<Self, AnimatedSvgError> {
        let scene = Scene::parse(text).map_err(AnimatedSvgError::Unreadable)?;
        let first = scene.frame(&mut Timeline::new(scene.animations.len()), 0.0);
        let parsed = render::parse(&scene.text(&first)).ok_or_else(|| {
            AnimatedSvgError::Unreadable("the renderer refused the document".into())
        })?;
        let (width, height) = parsed.size();
        Ok(Self {
            inner: Arc::new(Inner {
                scene,
                size: Size::new(width, height),
            }),
        })
    }

    /// Whether anything in the document moves.
    #[must_use]
    pub fn is_animated(&self) -> bool {
        self.inner.scene.is_animated()
    }

    /// The document's own size, in its units.
    #[must_use]
    pub fn size(&self) -> Size {
        self.inner.size
    }
}

/// A picture playing `svg`. See the [module](self) documentation.
pub fn enz_animated_svg(svg: &AnimatedSvg) -> EnzAnimatedSvg {
    EnzAnimatedSvg {
        svg: svg.clone(),
        width: Length::Shrink,
        height: Length::Shrink,
        fit: ContentFit::Contain,
        label: String::new(),
        playing: true,
    }
}

/// Builder for an animated SVG picture.
#[must_use]
pub struct EnzAnimatedSvg {
    svg: AnimatedSvg,
    width: Length,
    height: Length,
    fit: ContentFit,
    label: String,
    playing: bool,
}

impl EnzAnimatedSvg {
    /// The width.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// The height.
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// How the picture fits its box; [`ContentFit::Contain`] by default.
    pub fn content_fit(mut self, fit: ContentFit) -> Self {
        self.fit = fit;
        self
    }

    /// What the picture shows, read by screen readers.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Whether the animation runs; paused, it holds its frame and resumes
    /// from there.
    pub fn playing(mut self, playing: bool) -> Self {
        self.playing = playing;
        self
    }

    /// Build the widget.
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        Element::new(AnimatedSvgWidget { picture: self })
    }
}

impl<'a, Message: 'a> From<EnzAnimatedSvg> for Element<'a, Message> {
    fn from(picture: EnzAnimatedSvg) -> Self {
        picture.into_element()
    }
}

/// A frame's identity: its scene key and its size in pixels.
type FrameKey = (u64, (u32, u32));

/// One rasterised frame.
#[derive(Debug, Clone)]
struct Raster {
    handle: iced_image::Handle,
    /// Event-target boxes in document units, for pointer events.
    targets: Vec<(dom::NodeId, Rectangle)>,
}

#[derive(Debug, Default)]
struct State {
    /// The document playing, held weakly so its address stays unique.
    playing: Option<Weak<Inner>>,
    timeline: Timeline,
    /// Seconds of document time played before `since`.
    played: f64,
    /// When the current run of playing began; `None` while paused.
    since: Option<Instant>,
    /// The frame on screen.
    shown: RefCell<Option<(FrameKey, Raster)>>,
    /// Frames rasterised so far.
    cache: RefCell<HashMap<FrameKey, Raster>>,
    cached_bytes: std::cell::Cell<usize>,
    /// The document time of the frame shown.
    time: f64,
    /// Elements under the pointer, for `mouseover` and `mouseout`.
    hovered: Vec<dom::NodeId>,
    /// Elements a press began on, for `click`.
    pressed: Vec<dom::NodeId>,
    theme_reduced: std::cell::Cell<bool>,
}

struct AnimatedSvgWidget {
    picture: EnzAnimatedSvg,
}

impl AnimatedSvgWidget {
    fn scene(&self) -> &Scene {
        &self.picture.svg.inner.scene
    }

    fn reduced(state: &State) -> bool {
        state.theme_reduced.get() || iced_core::preferences::get().reduced_motion
    }

    /// Where the document is drawn inside `bounds`, in logical pixels.
    fn drawn(&self, bounds: Rectangle) -> Rectangle {
        let size = self.picture.svg.inner.size;
        let fitted = self.picture.fit.fit(size, bounds.size());
        Rectangle::new(
            iced::Point::new(
                bounds.center_x() - fitted.width / 2.0,
                bounds.center_y() - fitted.height / 2.0,
            ),
            fitted,
        )
    }

    /// Make sure the frame at `time` is shown, rasterised at `scale`
    /// physical pixels per logical pixel; returns when it next changes.
    fn show(&self, state: &mut State, bounds: Rectangle, scale: f32, time: f64) -> Next {
        let time = (time * TIME_GRID).floor() / TIME_GRID;
        let frame = self.scene().frame(&mut state.timeline, time);
        state.time = time;
        let drawn = self.drawn(bounds);
        let pixels = (
            (drawn.width * scale).ceil().clamp(1.0, MAX_EDGE) as u32,
            (drawn.height * scale).ceil().clamp(1.0, MAX_EDGE) as u32,
        );
        let key = (frame.key, pixels);
        let current = state.shown.borrow().as_ref().map(|(key, _)| *key);
        if current == Some(key) {
            return frame.next;
        }
        let cached = state.cache.borrow().get(&key).cloned();
        let raster = cached.or_else(|| {
            let raster = self.rasterise(&self.scene().text(&frame), pixels)?;
            let bytes = (pixels.0 as usize) * (pixels.1 as usize) * 4;
            if state.cached_bytes.get() + bytes <= FRAME_BUDGET {
                state.cached_bytes.set(state.cached_bytes.get() + bytes);
                state.cache.borrow_mut().insert(key, raster.clone());
            }
            Some(raster)
        });
        if let Some(raster) = raster {
            *state.shown.borrow_mut() = Some((key, raster));
        }
        frame.next
    }

    fn rasterise(&self, text: &str, (width, height): (u32, u32)) -> Option<Raster> {
        let parsed = render::parse(text)?;
        let size = self.picture.svg.inner.size;
        let (sx, sy) = (
            width as f32 / size.width.max(1e-6),
            height as f32 / size.height.max(1e-6),
        );
        let pixels = parsed.rasterise(width, height, [sx, 0.0, 0.0, sy, 0.0, 0.0])?;
        let targets = parsed.targets(self.scene());
        Some(Raster {
            handle: iced_image::Handle::from_rgba(width, height, pixels),
            targets,
        })
    }

    /// Reset when a different document arrives.
    fn adopt(&self, state: &mut State) {
        let inner = &self.picture.svg.inner;
        let same = state
            .playing
            .as_ref()
            .is_some_and(|playing| std::ptr::addr_eq(playing.as_ptr(), Arc::as_ptr(inner)));
        if !same {
            *state = State {
                playing: Some(Arc::downgrade(inner)),
                timeline: Timeline::new(inner.scene.animations.len()),
                ..State::default()
            };
        }
    }

    /// Document coordinates of a window position over the drawn picture.
    fn to_document(&self, bounds: Rectangle, position: iced::Point) -> Option<(f32, f32)> {
        let drawn = self.drawn(bounds);
        if !drawn.contains(position) {
            return None;
        }
        let size = self.picture.svg.inner.size;
        Some((
            (position.x - drawn.x) / drawn.width * size.width,
            (position.y - drawn.y) / drawn.height * size.height,
        ))
    }

    /// Send pointer events at `position` to the elements under it.
    fn pointer(
        &self,
        state: &mut State,
        bounds: Rectangle,
        position: Option<iced::Point>,
        event: &mouse::Event,
    ) -> bool {
        let under: Vec<dom::NodeId> = match (
            position.and_then(|p| self.to_document(bounds, p)),
            state.shown.borrow().as_ref(),
        ) {
            (Some((x, y)), Some((_, raster))) => raster
                .targets
                .iter()
                .filter(|(_, rect)| rect.contains(iced::Point::new(x, y)))
                .map(|(node, _)| *node)
                .collect(),
            _ => Vec::new(),
        };
        let time = state.time;
        let timings = &self.scene().timings;
        let mut fired = false;
        let mut fire = |state: &mut State, node: dom::NodeId, event: timing::Event| {
            if Timeline::listens(timings, node, event) {
                state.timeline.add_event(time, node, event);
                fired = true;
            }
        };
        match event {
            mouse::Event::CursorMoved { .. } | mouse::Event::CursorLeft => {
                let entered: Vec<_> = under
                    .iter()
                    .filter(|n| !state.hovered.contains(n))
                    .copied()
                    .collect();
                let left: Vec<_> = state
                    .hovered
                    .iter()
                    .filter(|n| !under.contains(n))
                    .copied()
                    .collect();
                for node in entered {
                    fire(state, node, timing::Event::MouseOver);
                }
                for node in left {
                    fire(state, node, timing::Event::MouseOut);
                }
                for node in &under {
                    fire(state, *node, timing::Event::MouseMove);
                }
                state.hovered = under;
            }
            mouse::Event::ButtonPressed(mouse::Button::Left) => {
                for node in &under {
                    fire(state, *node, timing::Event::MouseDown);
                }
                state.pressed = under;
            }
            mouse::Event::ButtonReleased(mouse::Button::Left) => {
                for node in &under {
                    fire(state, *node, timing::Event::MouseUp);
                }
                let pressed = std::mem::take(&mut state.pressed);
                for node in under.iter().filter(|n| pressed.contains(n)) {
                    fire(state, *node, timing::Event::Click);
                }
            }
            _ => {}
        }
        fired
    }
}

impl render::Parsed {
    /// The boxes of every element some animation waits on a pointer event
    /// from, in document units.
    fn targets(&self, scene: &Scene) -> Vec<(dom::NodeId, Rectangle)> {
        scene
            .event_targets()
            .filter_map(|(node, id)| {
                let rect = self.node_box(&id)?;
                Some((node, rect))
            })
            .collect()
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for AnimatedSvgWidget {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.picture.width, self.picture.height)
    }

    fn frame_interest(&self, _tree: &Tree) -> bool {
        // Paused pictures still see frames, so the frame that follows a pause
        // (or scrolling out of view) stops their clock there.
        self.scene().is_animated()
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) {
        let image = self.picture.svg.inner.size;
        let (width, height) = (self.picture.width, self.picture.height);
        let size = limits
            .width(width)
            .height(height)
            .resolve(width, height, image);
        let fitted = self.picture.fit.fit(image, size);
        tree.size = Size::new(
            if width == Length::Shrink {
                fitted.width
            } else {
                size.width
            },
            if height == Length::Shrink {
                fitted.height
            } else {
                size.height
            },
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let state = tree.state.downcast_mut::<State>();
        self.adopt(state);
        let bounds = layout.bounds();
        if let Event::Mouse(mouse_event) = event {
            if self.picture.playing && self.pointer(state, bounds, cursor.position(), mouse_event) {
                shell.request_redraw();
            }
            return;
        }
        let Event::Window(window::Event::RedrawRequested(now)) = event else {
            return;
        };
        let scale = renderer
            .scale()
            .map_or(1.0, iced::advanced::renderer::Scale::total);
        let visible = bounds.intersects(viewport);
        let animates = self.picture.playing && self.scene().is_animated() && !Self::reduced(state);
        if !animates || !visible {
            // Paused: keep the time reached, resume from it.
            if let Some(since) = state.since.take() {
                state.played += now.saturating_duration_since(since).as_secs_f64();
            }
            let time = if Self::reduced(state) {
                0.0
            } else {
                state.played
            };
            if visible {
                let _ = self.show(state, bounds, scale, time);
            }
            return;
        }
        let since = *state.since.get_or_insert(*now);
        let time = state.played + now.saturating_duration_since(since).as_secs_f64();
        match self.show(state, bounds, scale, time) {
            Next::Continuous => shell.request_redraw(),
            Next::At(at) => {
                let wait = Duration::from_secs_f64((at - time).max(0.0) + 0.5 / TIME_GRID);
                shell.schedule_redraw_at(*now + wait);
            }
            Next::Never => {}
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        state
            .theme_reduced
            .set(crate::theme_lookup::reduced_motion(theme));
        let bounds = layout.bounds();
        if state.shown.borrow().is_none() {
            // Drawn before any frame event: the first frame, rasterised now.
            use iced::advanced::Renderer as _;
            let scale = renderer
                .scale()
                .map_or(1.0, iced::advanced::renderer::Scale::total);
            let mut fresh = State {
                timeline: Timeline::new(self.scene().animations.len()),
                ..State::default()
            };
            let _ = self.show(&mut fresh, bounds, scale, 0.0);
            *state.shown.borrow_mut() = fresh.shown.into_inner();
        }
        let shown = state.shown.borrow();
        let Some((_, raster)) = shown.as_ref() else {
            return;
        };
        let drawn = self.drawn(bounds);
        let clip = bounds.intersection(viewport).unwrap_or(bounds);
        renderer.draw_image(iced_image::Image::new(raster.handle.clone()), drawn, clip);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<State>();
        let Some((x, y)) = cursor
            .position()
            .and_then(|p| self.to_document(layout.bounds(), p))
        else {
            return mouse::Interaction::default();
        };
        let shown = state.shown.borrow();
        let clickable = shown.as_ref().is_some_and(|(_, raster)| {
            raster.targets.iter().any(|(node, rect)| {
                rect.contains(iced::Point::new(x, y))
                    && Timeline::listens(&self.scene().timings, *node, timing::Event::Click)
            })
        });
        if clickable {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }

    fn operate(
        &mut self,
        _tree: &mut Tree,
        layout: Layout,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        if self.picture.label.trim().is_empty() {
            return;
        }
        let mut node =
            SemanticsNode::new(Semantics::new(Role::Image).label(self.picture.label.clone()));
        operation.custom(None, layout.bounds(), &mut node);
    }
}

#[cfg(test)]
mod tests;
