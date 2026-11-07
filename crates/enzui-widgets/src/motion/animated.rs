// motion split production module: implicit spring animation

//! A child that moves and fades to wherever the view puts it, on a spring,
//! without the application animating anything.
//!
//! [`animated`] wraps any element. The view sets where the child should be
//! ([`Animated::offset`], [`Animated::opacity`]); when a later view sets a
//! different value, the child travels there on its [`Spring`] and keeps its
//! velocity if the target changes again mid-flight. The widget keeps its own
//! clock: the application publishes no per-frame messages, so its view is not
//! rebuilt while the child moves, and the render lane can move the recorded
//! child on its own.
//!
//! [`Animated::enters_from`] gives the child an entrance: when it is first
//! mounted it starts from that offset and opacity and springs to where the
//! view puts it (a toast rising into its stack, a row sliding in).
//!
//! The offset is visual only. Layout places the child where it would be
//! without it, and pointer input and overlays follow it to where it is shown.

use super::{SpringAnimation, SpringValue, next_scene_node};
use enzui_theme::{ExpressiveMotion, MotionScale, Spring};
use iced::advanced::layout::{self, Layout, Measure as _};
use iced::advanced::renderer::{self, NodeId, Track};
use iced::advanced::widget::{self, Tree, Widget};
use iced::advanced::{Renderer as _, Shell, overlay};
use iced::time::Instant;
use iced::{Element, Event, Length, Rectangle, Size, Vector, mouse, window};
use std::cell::Cell;

/// Wraps `child` so that it springs to the offset and opacity each view
/// gives it. See the [motion documentation](crate::motion).
pub fn animated<'a, Message>(child: impl Into<Element<'a, Message>>) -> Animated<'a, Message> {
    Animated {
        child: child.into(),
        pose: Pose::REST,
        fades: false,
        spring: ExpressiveMotion::DefaultSpatial.spring(),
        enters_from: None,
    }
}

/// A child that springs to its offset and opacity; built by [`animated`].
pub struct Animated<'a, Message> {
    child: Element<'a, Message>,
    pose: Pose,
    fades: bool,
    spring: Spring,
    enters_from: Option<Pose>,
}

impl<Message> Animated<'_, Message> {
    /// Where the child is drawn, relative to where layout puts it.
    #[must_use]
    pub fn offset(mut self, offset: Vector) -> Self {
        self.pose.x = offset.x;
        self.pose.y = offset.y;
        self
    }

    /// How opaque the child is drawn, `0.0..=1.0`.
    #[must_use]
    pub fn opacity(mut self, opacity: f32) -> Self {
        self.pose.opacity = opacity.clamp(0.0, 1.0);
        self.fades = true;
        self
    }

    /// When first mounted, start at `offset` and `opacity` and spring to the
    /// pose the view gives; without it the child appears where it is put.
    /// Under reduced motion it appears in place.
    #[must_use]
    pub fn enters_from(mut self, offset: Vector, opacity: f32) -> Self {
        let opacity = opacity.clamp(0.0, 1.0);
        self.fades |= opacity < 1.0;
        self.enters_from = Some(Pose {
            x: offset.x,
            y: offset.y,
            opacity,
        });
        self
    }

    /// The spring the child travels on; the theme's default spatial spring
    /// otherwise. The theme's `motion-normal` speed slows or speeds it, and
    /// reduced motion lands every change at once.
    #[must_use]
    pub fn spring(mut self, spring: Spring) -> Self {
        self.spring = spring;
        self
    }
}

/// Offset and opacity, the values a spring moves together.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pose {
    x: f32,
    y: f32,
    opacity: f32,
}

impl Pose {
    const REST: Self = Self {
        x: 0.0,
        y: 0.0,
        opacity: 1.0,
    };

    fn offset(self) -> Vector {
        Vector::new(self.x, self.y)
    }
}

impl SpringValue for Pose {
    fn components(self) -> [f32; 4] {
        [self.x, self.y, self.opacity, 0.0]
    }

    fn from_components(c: [f32; 4]) -> Self {
        Self {
            x: c[0],
            y: c[1],
            opacity: c[2],
        }
    }
}

struct State {
    node: NodeId,
    animation: SpringAnimation<Pose>,
    /// A target the view set, applied at the next frame's instant.
    pending: Option<Pose>,
    /// The latest frame instant: the clock layout, draw and input read.
    now: Cell<Instant>,
    /// The theme's motion policy, from the last draw.
    scale: Cell<Option<MotionScale>>,
    /// The pose the last draw painted, which a replay moves from.
    recorded: Cell<Option<Pose>>,
}

impl State {
    fn pose(&self) -> Pose {
        self.animation.value(self.now.get())
    }

    /// Where pointer input finds the child: where it was painted, or, before
    /// it has been painted at all, where it is going.
    fn input_offset(&self) -> Vector {
        match self.recorded.get() {
            Some(_) => self.pose().offset(),
            None => self
                .pending
                .unwrap_or_else(|| self.animation.target())
                .offset(),
        }
    }
}

impl<Message> Animated<'_, Message> {
    fn track(&self, node: NodeId, pose: Pose) -> Track {
        Track {
            node,
            translation: pose.offset(),
            opacity: if self.fades { pose.opacity } else { 1.0 },
            scale: 1.0,
            scale_origin: iced::Point::ORIGIN,
            rotation: 0.0,
            clip: None,
        }
    }
}

fn shifted(cursor: mouse::Cursor, offset: Vector) -> mouse::Cursor {
    match cursor {
        mouse::Cursor::Available(position) => mouse::Cursor::Available(position - offset),
        other => other,
    }
}

impl<'a, Message: 'a> Widget<Message, iced::Theme, iced::Renderer> for Animated<'a, Message> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<State>()
    }

    fn state(&self) -> widget::tree::State {
        // The first appearance is where the view put it, not a travel there,
        // unless the child has an entrance.
        widget::tree::State::new(State {
            node: next_scene_node(),
            animation: SpringAnimation::new(self.enters_from.unwrap_or(self.pose), self.spring)
                .precision(1e-3),
            pending: self.enters_from.map(|_| self.pose),
            now: Cell::new(Instant::now()),
            scale: Cell::new(None),
            recorded: Cell::new(None),
        })
    }

    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<State>();
        if state.animation.spring() != self.spring {
            state.animation.set_spring(self.spring, state.now.get());
        }
        let target = state.pending.unwrap_or_else(|| state.animation.target());
        if target != self.pose {
            state.pending = Some(self.pose);
        }
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let child = self
            .child
            .as_widget_mut()
            .measure(&mut tree.children[0], renderer, limits);
        layout::Node::with_children(child.size(), vec![child]).apply(tree);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        let state = tree.state.downcast_ref::<State>();
        state.pending.is_some()
            || state.animation.is_animating(state.now.get())
            || state
                .recorded
                .get()
                .is_some_and(|recorded| recorded != state.pose())
            || tree
                .children
                .first()
                .is_none_or(|child| self.child.as_widget().frame_interest(child))
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
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let node = {
                let state = tree.state.downcast_mut::<State>();
                state.now.set(*now);
                // A target waits for a draw to have read the theme's motion
                // policy, so an entrance before the first draw still honours
                // reduced motion. Out of view no draw comes: the child is
                // already where the view puts it.
                match (state.pending, state.scale.get()) {
                    (Some(target), Some(scale)) => {
                        state.pending = None;
                        state.animation.sync_motion(&scale);
                        state.animation.set_target(target, *now);
                    }
                    (Some(target), None) if !layout.bounds().intersects(viewport) => {
                        state.pending = None;
                        state.animation.snap(target);
                    }
                    (Some(_), None) => shell.request_redraw(),
                    (None, _) => {}
                }
                state.node
            };
            let state = tree.state.downcast_ref::<State>();
            let animating = state.animation.is_animating(*now);
            let pose = state.pose();
            // A recording made mid-travel replays where it was painted unless
            // a track says where the child is now, settled or not.
            let stale = state
                .recorded
                .get()
                .is_some_and(|recorded| recorded != pose);
            if animating {
                crate::motion::request_motion_frame(shell, state.recorded.get().is_some());
                // The lane may carry the rest of the travel without waking
                // this thread: wake where it lands to record it at rest.
                if let Some(lands) = state.animation.lands_at() {
                    shell.schedule_redraw_at(lands);
                }
            } else if stale {
                // Settled away from where it was recorded: one drawn frame
                // records it at rest, so replays stop correcting an old
                // recording with tracks on every frame.
                shell.request_redraw();
            }
            if state.recorded.get().is_some() && (animating || stale) {
                shell.publish_track(self.track(node, pose));
                state
                    .animation
                    .publish(shell, node, *now, |pose| self.track(node, pose));
            }
        }
        let offset = tree.state.downcast_ref::<State>().input_offset();
        let child_layout = layout.child(&tree.children, 0);
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            shifted(cursor, offset),
            renderer,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let offset = tree.state.downcast_ref::<State>().input_offset();
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout.child(&tree.children, 0),
            shifted(cursor, offset),
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let scale = crate::theme_lookup::motion(theme);
        state.scale.set(Some(scale));
        // Under reduced motion a target the next frame will land on (an
        // entrance included) is drawn there already.
        let pose = match state.pending {
            Some(target) if scale.reduced_motion => target,
            _ => state.pose(),
        };
        let offset = pose.offset();
        let child_layout = layout.child(&tree.children, 0);
        let paint = |renderer: &mut iced::Renderer| {
            renderer.with_motion(state.node, offset, |renderer| {
                self.child.as_widget().draw(
                    &tree.children[0],
                    renderer,
                    theme,
                    style,
                    child_layout,
                    shifted(cursor, offset),
                    // Unshifted, so the child's clips travel with it and a replayed frame clips
                    // as a drawn one does.
                    viewport,
                );
            });
        };
        // The whole child is kept where it was painted, so a replay can move
        // in what the visible region cut off when it was recorded.
        let retained = (child_layout.bounds() + offset).union(viewport);
        // An opaque child needs no group; a replay that fades it composites
        // the node itself.
        if self.fades && pose.opacity < 1.0 {
            renderer.with_layer_alpha(*viewport, pose.opacity, |renderer| {
                renderer.with_layer_retained(*viewport, retained, paint);
            });
        } else {
            renderer.with_layer_retained(*viewport, retained, paint);
        }
        state.recorded.set(Some(pose));
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        let child_layout = layout.child(&tree.children, 0);
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            child_layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        let offset = tree.state.downcast_ref::<State>().input_offset();
        let child_layout = layout.child(&tree.children, 0);
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            child_layout,
            renderer,
            viewport,
            translation + offset,
            window,
        )
    }
}

impl<'a, Message: 'a> From<Animated<'a, Message>> for Element<'a, Message> {
    fn from(animated: Animated<'a, Message>) -> Self {
        Element::new(animated)
    }
}
