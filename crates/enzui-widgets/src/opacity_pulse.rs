//! A loading pulse the render lane plays: the child is painted once inside an
//! opacity composite, and its eased opacity cycle is published as a track
//! animation, so a page of pulsing placeholders costs replays, not redraws.
//!
//! Visible and animating, it asks for frames the way a turning spinner does;
//! offscreen it asks for none, and under reduced motion it holds the loading
//! opacity, probing at a low rate when the theme decides so a policy change
//! restarts it.

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{NodeId, Track, TrackAnimation};
use iced::advanced::widget::{self, Widget};
use iced::advanced::{Shell, overlay, renderer};
use iced::time::{Duration, Instant};
use iced::{Element, Event, Length, Rectangle, Size, Vector, mouse, window};
use std::cell::Cell;

/// Periods one published track animation covers.
const PERIODS_PER_STRETCH: u32 = 2;
/// Keyframes per period, enough for the eased triangle to read as a curve.
const SAMPLES_PER_PERIOD: u32 = 24;

/// Pulses `child`'s opacity between `idle` and `loading` every `period_secs`,
/// as [`crate::skeleton::pulse_alpha_with_period`] samples it. The child
/// paints at full strength.
pub(crate) struct OpacityPulse<'a, Message> {
    child: Element<'a, Message>,
    period_secs: f32,
    idle: f32,
    loading: f32,
    reduced_motion: Option<bool>,
}

impl<'a, Message> OpacityPulse<'a, Message> {
    pub(crate) fn new(
        child: Element<'a, Message>,
        period_secs: f32,
        idle: f32,
        loading: f32,
        reduced_motion: Option<bool>,
    ) -> Self {
        Self {
            child,
            period_secs,
            idle,
            loading,
            reduced_motion,
        }
    }

    fn alpha(&self, elapsed_secs: f32) -> f32 {
        crate::skeleton::pulse_alpha_with_period(
            elapsed_secs,
            self.period_secs,
            self.idle,
            self.loading,
            false,
        )
    }

    /// Whether this frame holds still: explicitly, or by a theme the last
    /// draw read. `None` until a draw has read the theme.
    fn reduced(&self, state: &PulseState) -> Option<bool> {
        self.reduced_motion.or_else(|| state.theme_reduced.get())
    }

    fn track(node: NodeId, alpha: f32) -> Track {
        Track {
            opacity: alpha,
            ..Track::identity(node)
        }
    }
}

#[derive(Debug)]
struct PulseState {
    node: NodeId,
    started_at: Option<Instant>,
    frame_at: Option<Instant>,
    theme_reduced: Cell<Option<bool>>,
    /// Whether the last draw recorded the child for the lane to fade.
    recorded: Cell<bool>,
    visible: bool,
}

impl PulseState {
    fn elapsed_secs(&self) -> f32 {
        match (self.started_at, self.frame_at) {
            (Some(start), Some(now)) => now.saturating_duration_since(start).as_secs_f32(),
            _ => 0.0,
        }
    }
}

impl<'a, Message: 'a> Widget<Message, iced::Theme, iced::Renderer> for OpacityPulse<'a, Message> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<PulseState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(PulseState {
            node: crate::motion::next_scene_node(),
            started_at: None,
            frame_at: None,
            theme_reduced: Cell::new(None),
            recorded: Cell::new(false),
            visible: true,
        })
    }

    fn diff(&mut self, tree: &mut widget::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn is_void(&self) -> bool {
        self.child.as_widget().is_void()
    }

    fn frame_interest(&self, tree: &widget::Tree) -> bool {
        let state = tree.state.downcast_ref::<PulseState>();
        (state.visible && self.reduced_motion != Some(true))
            || tree
                .children
                .first()
                .is_none_or(widget::Tree::has_frame_interest)
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        layout::pass_through(tree, 0);
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let visible = crate::viewport_gate::is_visible(layout.bounds(), viewport);
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let reduced = self.reduced(tree.state.downcast_ref::<PulseState>());
            let state = tree.state.downcast_mut::<PulseState>();
            state.visible = visible;
            match reduced {
                Some(false) => {
                    // Elapsed time runs on across offscreen spells.
                    let started = *state.started_at.get_or_insert(*now);
                    state.frame_at = Some(*now);
                    if visible {
                        crate::motion::request_motion_frame(shell, state.recorded.get());
                        if state.recorded.get() {
                            let node = state.node;
                            let elapsed = state.elapsed_secs();
                            shell.publish_track(Self::track(node, self.alpha(elapsed)));
                            let period = self.period_secs.max(0.1);
                            let samples = SAMPLES_PER_PERIOD * PERIODS_PER_STRETCH;
                            let stretch = period * PERIODS_PER_STRETCH as f32;
                            shell.publish_track_animation(TrackAnimation {
                                node,
                                start: *now,
                                duration: Duration::from_secs_f32(stretch),
                                keyframes: (0..=samples)
                                    .map(|i| {
                                        let at = i as f32 / samples as f32;
                                        (at, Self::track(node, self.alpha(elapsed + at * stretch)))
                                    })
                                    .collect(),
                                // A new stretch is a new travel for the lane.
                                generation: (now.saturating_duration_since(started).as_secs_f32()
                                    / stretch) as u64,
                            });
                            // With the lane fading it, the UI thread sleeps
                            // until the stretch nearly runs out.
                            shell.schedule_redraw_at(
                                *now + Duration::from_secs_f32(stretch).mul_f32(0.9),
                            );
                        }
                    }
                }
                // Explicit stillness schedules nothing.
                Some(true) if self.reduced_motion.is_some() => {}
                // A theme that allows motion again restarts the pulse.
                Some(true) => {
                    if visible {
                        shell.request_redraw_at(
                            *now + Duration::from_secs(crate::spinner::REDUCED_MOTION_PROBE_SECS),
                        );
                    }
                }
                // No draw has read the theme yet: one frame learns it.
                None => {
                    if visible {
                        shell.request_redraw();
                    }
                }
            }
        }
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        tree.children[0].refresh_frame_interest(self.child.as_widget());
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;
        let state = tree.state.downcast_ref::<PulseState>();
        state
            .theme_reduced
            .set(Some(crate::theme_lookup::reduced_motion(theme)));
        let bounds = layout.bounds();
        let reduced = self.reduced(state).unwrap_or(true);
        let alpha = if reduced {
            self.loading
        } else {
            self.alpha(state.elapsed_secs())
        };
        renderer.with_layer_composite(bounds, alpha, 1.0, 0.0, bounds.center(), |renderer| {
            if !reduced {
                renderer.mark_recording(state.node);
            }
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            );
            if !reduced {
                renderer.mark_recording(NodeId::STATIC);
            }
        });
        state.recorded.set(!reduced);
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<OpacityPulse<'a, Message>> for Element<'a, Message> {
    fn from(pulse: OpacityPulse<'a, Message>) -> Self {
        Element::new(pulse)
    }
}
