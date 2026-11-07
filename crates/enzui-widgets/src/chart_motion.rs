//! Bounded, visibility-aware motion for live charts.
//!
//! A sampler may rebuild a chart only once a second. This wrapper turns each
//! changed data key into one `motion-normal` transition, then stops requesting
//! frames. It does not own a timer or publish application messages. Scheduling
//! is also suppressed while the chart is outside its viewport and when the
//! theme requests reduced motion.

use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::time::Instant;
use iced::{Element, Event, Length, Rectangle, Size, Vector, window};
use std::cell::Cell;
use std::rc::Rc;

/// Draw-time progress shared with a chart Canvas program.
pub(crate) type ChartMotionPhase = Rc<Cell<f32>>;

pub(crate) fn phase() -> ChartMotionPhase {
    Rc::new(Cell::new(1.0))
}

#[derive(Debug)]
struct State {
    key: Option<u64>,
    started_at: Option<Instant>,
    /// When the key last changed.
    changed_at: Option<Instant>,
    duration_ms: Cell<f32>,
    easing: Cell<enzui_theme::MotionEasing>,
}

impl Default for State {
    fn default() -> Self {
        let motion = enzui_theme::MotionScale::default();
        Self {
            key: None,
            started_at: None,
            changed_at: None,
            duration_ms: Cell::new(transition_ms(&motion)),
            easing: Cell::new(motion.easing),
        }
    }
}

/// The transition length under `motion`, sanitized: a non-finite or negative
/// duration (built by hand, since the fields are public) snaps instead of
/// never settling.
fn transition_ms(motion: &enzui_theme::MotionScale) -> f32 {
    enzui_theme::sane_motion_ms(motion.ms(enzui_theme::MotionToken::Normal))
}

/// Transparent wrapper that schedules only the finite transition after `key`
/// changes. The inner chart reads `phase` while painting its newest sample.
pub(crate) struct ChartMotion<'a, Message> {
    inner: Element<'a, Message>,
    key: u64,
    phase: ChartMotionPhase,
}

impl<'a, Message> ChartMotion<'a, Message> {
    pub(crate) fn new(inner: Element<'a, Message>, key: u64, phase: ChartMotionPhase) -> Self {
        Self { inner, key, phase }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for ChartMotion<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.inner));
    }

    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.inner
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.inner.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
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
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let state = tree.state.downcast_mut::<State>();
            match state.key {
                None => {
                    // First paint has no earlier reading to animate from.
                    state.key = Some(self.key);
                    state.started_at = None;
                    self.phase.set(1.0);
                }
                Some(old) if old != self.key => {
                    state.key = Some(self.key);
                    let duration = state.duration_ms.get();
                    // A feed that changes faster than its transition shows
                    // each reading as it arrives: a point replaced before it
                    // lands costs a full redraw every frame and shows no
                    // motion a reader can follow.
                    let feed_is_faster = state.changed_at.is_some_and(|previous| {
                        now.saturating_duration_since(previous).as_secs_f32() * 1000.0 < duration
                    });
                    state.changed_at = Some(*now);
                    if duration > 0.0 && !feed_is_faster {
                        state.started_at = Some(*now);
                        self.phase.set(0.0);
                    } else {
                        state.started_at = None;
                        self.phase.set(1.0);
                    }
                }
                Some(_) => {
                    if let Some(start) = state.started_at {
                        let duration = state.duration_ms.get().max(0.0) / 1000.0;
                        let linear = if duration > 0.0 {
                            now.saturating_duration_since(start).as_secs_f32() / duration
                        } else {
                            1.0
                        };
                        let progress = state.easing.get().apply(linear);
                        self.phase.set(progress);
                        if linear >= 1.0 {
                            state.started_at = None;
                            self.phase.set(1.0);
                        }
                    } else {
                        self.phase.set(1.0);
                    }
                }
            }

            // A live chart costs frames only for the bounded transition and
            // only while at least one pixel intersects the visible viewport.
            if state.started_at.is_some() && layout.bounds().intersects(viewport) {
                shell.request_redraw();
            }
        }

        self.inner.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
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
        let motion = crate::theme_lookup::motion(theme);
        state.duration_ms.set(transition_ms(&motion));
        state.easing.set(motion.easing_or(None));
        if state.duration_ms.get() <= 0.0 {
            self.phase.set(1.0);
        }
        self.inner.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
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
        self.inner.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
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
        self.inner.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<ChartMotion<'a, Message>> for Element<'a, Message> {
    fn from(value: ChartMotion<'a, Message>) -> Self {
        Element::new(value)
    }
}
