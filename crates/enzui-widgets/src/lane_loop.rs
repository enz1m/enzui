//! A child that loops a scale and opacity curve on the compositor.
//!
//! The child is painted once, into a retained node; the compositor's lane
//! then plays the curve over several periods at a time, so a looping
//! decoration (a status dot breathing, a halo rippling out) costs the UI
//! thread one wake every few seconds instead of a frame per vsync. The
//! loop rests under reduced motion and while the child is off screen.
//!
//! A child that grows past its place (a halo) is laid out at its full size
//! and centred on a smaller [`footprint`](LaneLoop::footprint), so it
//! paints beyond the space it takes.

use iced::advanced::layout::Measure as _;
use iced::advanced::renderer::{NodeId, Track, TrackAnimation};
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, renderer};
use iced::time::{Duration, Instant};
use iced::{Element, Length, Point, Rectangle, Renderer, Size, window};
use std::cell::Cell;

/// How many periods one published lane animation covers; the UI thread
/// wakes once per stretch to publish the next.
const PERIODS_PER_STRETCH: u32 = 4;

/// Keyframes sampled per period: enough that the lane's straight segments
/// follow an eased curve.
const SAMPLES_PER_PERIOD: u32 = 12;

/// How often a resting loop under a theme's reduced motion checks whether
/// motion is allowed again.
const REDUCED_MOTION_PROBE: Duration = Duration::from_secs(2);

/// The scale and opacity a [`LaneLoop`] shows at a point of its period.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LoopSample {
    pub(crate) scale: f32,
    pub(crate) opacity: f32,
}

/// Loops `curve` (sampled over `0..1` of each period) on `child`, once
/// every `period`. `rest` is what it shows when it does not move.
pub(crate) struct LaneLoop<'a, Message> {
    child: Element<'a, Message>,
    period: Duration,
    curve: fn(f32) -> LoopSample,
    rest: LoopSample,
    reduced_motion: Option<bool>,
    footprint: Option<Size>,
}

impl<'a, Message> LaneLoop<'a, Message> {
    pub(crate) fn new(
        child: impl Into<Element<'a, Message>>,
        period: Duration,
        curve: fn(f32) -> LoopSample,
        rest: LoopSample,
    ) -> Self {
        Self {
            child: child.into(),
            period,
            curve,
            rest,
            reduced_motion: None,
            footprint: None,
        }
    }

    /// Take only `size` in layout, with the child centred on it.
    pub(crate) fn footprint(mut self, size: Size) -> Self {
        self.footprint = Some(size);
        self
    }

    /// Pin the motion policy instead of following the theme's.
    pub(crate) fn reduced_motion(mut self, reduced: Option<bool>) -> Self {
        self.reduced_motion = reduced;
        self
    }

    fn reduced(&self, state: &LoopState) -> bool {
        self.reduced_motion
            .unwrap_or_else(|| state.theme_reduced.get())
    }

    /// The sample at `now`, for a loop that started at `started`.
    fn sample_at(&self, started: Instant, now: Instant) -> LoopSample {
        let period = self.period.as_secs_f32().max(f32::EPSILON);
        let phase = (now.saturating_duration_since(started).as_secs_f32() / period).fract();
        (self.curve)(phase)
    }

    /// The child's layout: the node itself, or its one child under a
    /// footprint.
    fn child_layout(&self, layout: Layout, children: &[Tree]) -> Layout {
        if self.footprint.is_some() {
            layout
                .children(children)
                .next()
                .expect("a footprint lays its child out under it")
        } else {
            layout
        }
    }

    fn track(node: NodeId, center: Point, sample: LoopSample) -> Track {
        Track {
            scale: sample.scale,
            opacity: sample.opacity,
            scale_origin: center,
            ..Track::identity(node)
        }
    }

    /// One stretch of [`PERIODS_PER_STRETCH`] periods from `phase`, as
    /// keyframes; each period ends where the next begins.
    fn stretch(&self, node: NodeId, center: Point, phase: f32) -> Vec<(f32, Track)> {
        let samples = PERIODS_PER_STRETCH * SAMPLES_PER_PERIOD;
        (0..=samples)
            .map(|i| {
                let t = i as f32 / samples as f32;
                let at = (phase + t * PERIODS_PER_STRETCH as f32).fract();
                (t, Self::track(node, center, (self.curve)(at)))
            })
            .collect()
    }
}

struct LoopState {
    node: NodeId,
    started_at: Option<Instant>,
    frame_at: Option<Instant>,
    theme_reduced: Cell<bool>,
    /// Whether the last draw painted through the retained node.
    recorded: Cell<bool>,
    /// Counts published stretches, so each is a new travel for the lane.
    stretches: u64,
}

impl<Message> Widget<Message, iced::Theme, Renderer> for LaneLoop<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<LoopState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(LoopState {
            node: crate::motion::next_scene_node(),
            started_at: None,
            frame_at: None,
            theme_reduced: Cell::new(false),
            recorded: Cell::new(false),
            stretches: 0,
        })
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        match self.footprint {
            Some(size) => Size::new(Length::Fixed(size.width), Length::Fixed(size.height)),
            None => self.child.as_widget().size(),
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let Some(footprint) = self.footprint else {
            self.child
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, limits);
            layout::pass_through(tree, 0);
            return;
        };
        let child = self.child.as_widget_mut().measure(
            &mut tree.children[0],
            renderer,
            &layout::Limits::new(Size::ZERO, Size::INFINITE),
        );
        let offset = Point::new(
            (footprint.width - child.size().width) / 2.0,
            (footprint.height - child.size().height) / 2.0,
        );
        layout::Node::with_children(footprint, vec![child.move_to(offset)]).apply(tree);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &iced::Event,
        layout: Layout,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let iced::Event::Window(window::Event::RedrawRequested(now)) = event else {
            return;
        };
        let reduced = self.reduced(tree.state.downcast_ref::<LoopState>());
        let state = tree.state.downcast_mut::<LoopState>();
        let started = *state.started_at.get_or_insert(*now);
        state.frame_at = Some(*now);
        if reduced {
            if self.reduced_motion.is_none() {
                shell.schedule_redraw_at(*now + REDUCED_MOTION_PROBE);
            }
            return;
        }
        let bounds = self.child_layout(layout, &tree.children).bounds();
        let state = tree.state.downcast_mut::<LoopState>();
        if !crate::viewport_gate::is_visible(bounds, viewport) {
            return;
        }
        if !state.recorded.get() {
            // The next draw records the node the lane plays.
            shell.request_redraw();
            return;
        }
        let node = state.node;
        let center = bounds.center();
        let period = self.period.as_secs_f32().max(f32::EPSILON);
        let phase = (now.saturating_duration_since(started).as_secs_f32() / period).fract();
        shell.publish_track(Self::track(node, center, (self.curve)(phase)));
        let duration = self.period * PERIODS_PER_STRETCH;
        state.stretches += 1;
        shell.publish_track_animation(TrackAnimation {
            node,
            start: *now,
            duration,
            keyframes: self.stretch(node, center, phase),
            generation: state.stretches,
        });
        // The lane plays the stretch; the UI thread sleeps until it nearly
        // runs out.
        shell.schedule_redraw_at(*now + duration.mul_f32(0.9));
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let state = tree.state.downcast_ref::<LoopState>();
        state
            .theme_reduced
            .set(crate::theme_lookup::reduced_motion(theme));
        let layout = self.child_layout(layout, &tree.children);
        let bounds = layout.bounds();
        let draw_child = |renderer: &mut Renderer| {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            );
        };
        let sample = match (self.reduced(state), state.started_at, state.frame_at) {
            (false, Some(started), Some(now)) => self.sample_at(started, now),
            _ => self.rest,
        };
        if self.reduced(state) {
            state.recorded.set(false);
            if self.rest.opacity <= 0.0 {
                return;
            }
        }
        renderer.with_layer_composite(
            bounds,
            sample.opacity,
            sample.scale,
            0.0,
            bounds.center(),
            |renderer| {
                renderer.mark_recording(state.node);
                draw_child(renderer);
                renderer.mark_recording(NodeId::STATIC);
            },
        );
        state.recorded.set(!self.reduced(state));
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            self.child_layout(layout, &tree.children),
            cursor,
            viewport,
            renderer,
        )
    }
}

impl<'a, Message: 'a> From<LaneLoop<'a, Message>> for Element<'a, Message> {
    fn from(lane: LaneLoop<'a, Message>) -> Self {
        Element::new(lane)
    }
}
