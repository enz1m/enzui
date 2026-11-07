//! Rate-limit activity messages while forwarding events to the wrapped content.
//!
//! The default interval is 250 ms. A qualifying event publishes immediately
//! when no recent pulse exists; events within the interval queue one trailing
//! pulse and request a redraw at the deadline. That pulse requires a later
//! redraw whose timestamp reaches the interval from the last pulse; runtime
//! scheduling can delay it, so it is not a deadline guarantee for a hide timer.
//!
//! Mouse moves, button presses, wheel scrolls, touch presses and touch moves
//! qualify only while the supplied cursor is over the content bounds. Touch
//! positions are not used independently of that cursor. Releases do not pulse.
//! Layout, paint, semantics and child events remain forwarded.
//!
//! [`EnzActivityPulse::on_idle`] adds the other half of auto-hiding chrome:
//! one message once the pointer has rested for a while, so a floating
//! titlebar over a video or photo can fade until the pointer moves again.
//! The rest is counted from the first frame, so chrome also hides when the
//! pointer never moves.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Shell, Widget, mouse, overlay, renderer};
use iced::time::{Duration, Instant};
use iced::touch;
use iced::{Element, Event, Length, Rectangle, Size, Vector, window};

/// Default minimum time between two pulses.
pub const DEFAULT_INTERVAL: Duration = Duration::from_millis(250);

/// Wrap `content` so that pointer activity over it publishes `on_activity`
/// at most once per [`DEFAULT_INTERVAL`].
pub fn enz_activity_pulse<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
    on_activity: Message,
) -> EnzActivityPulse<'a, Message, Theme, Renderer> {
    EnzActivityPulse {
        content: content.into(),
        on_activity,
        interval: DEFAULT_INTERVAL,
        on_idle: None,
    }
}

/// A rest long enough to hide auto-hiding chrome over media.
pub const DEFAULT_IDLE: Duration = Duration::from_millis(2500);

/// A transparent wrapper that reports pointer activity, rate-limited. See the
/// [module docs](self).
pub struct EnzActivityPulse<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
    on_activity: Message,
    interval: Duration,
    on_idle: Option<(Duration, Message)>,
}

impl<Message, Theme, Renderer> std::fmt::Debug for EnzActivityPulse<'_, Message, Theme, Renderer> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnzActivityPulse")
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

impl<'a, Message, Theme, Renderer> EnzActivityPulse<'a, Message, Theme, Renderer> {
    /// Set the interval used by subsequent activity and trailing-pulse checks.
    /// Zero permits every qualifying event to publish. Changing it preserves pulse
    /// state and does not reschedule an outstanding redraw.
    #[must_use]
    pub fn interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Publish `message` once the pointer has rested over the content for
    /// `after` ([`DEFAULT_IDLE`] suits media chrome). The next activity
    /// publishes the activity message and starts a new rest.
    #[must_use]
    pub fn on_idle(mut self, after: Duration, message: Message) -> Self {
        self.on_idle = Some((after, message));
        self
    }

    /// Convert into an [`Element`].
    pub fn into_element(self) -> Element<'a, Message, Theme, Renderer>
    where
        Message: Clone + 'a,
        Theme: 'a,
        Renderer: iced::advanced::Renderer + 'a,
    {
        self.into()
    }
}

#[derive(Debug, Default)]
struct State {
    last_pulse: Option<Instant>,
    trailing: bool,
    /// When the current rest began; `None` before the first frame.
    rest_from: Option<Instant>,
    /// Whether the current rest has yet to be reported.
    resting: bool,
}

fn is_activity(event: &Event) -> bool {
    matches!(
        event,
        Event::Mouse(
            mouse::Event::CursorMoved { .. }
                | mouse::Event::ButtonPressed(_)
                | mouse::Event::WheelScrolled { .. }
        ) | Event::Touch(touch::Event::FingerPressed { .. } | touch::Event::FingerMoved { .. })
    )
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for EnzActivityPulse<'_, Message, Theme, Renderer>
where
    Message: Clone,
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        // A rest is under way from the first frame, which therefore has to
        // reach this widget.
        tree::State::new(State {
            resting: self.on_idle.is_some(),
            ..State::default()
        })
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        layout::pass_through(tree, 0);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }
    fn frame_interest(&self, tree: &Tree) -> bool {
        let state = tree.state.downcast_ref::<State>();
        state.trailing || state.resting || crate::frame_routing::interest_opaque(tree)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_mut::<State>();
        if let Some((after, idle)) = &self.on_idle {
            let activity = is_activity(event) && cursor.is_over(layout.bounds());
            match event {
                _ if activity => {
                    let now = Instant::now();
                    state.rest_from = Some(now);
                    if !state.resting {
                        state.resting = true;
                        shell.schedule_redraw_at(now + *after);
                        tree.raise_frame_interest();
                    }
                }
                Event::Window(window::Event::RedrawRequested(now)) => {
                    let from = *state.rest_from.get_or_insert(*now);
                    if state.resting {
                        if now.saturating_duration_since(from) >= *after {
                            state.resting = false;
                            shell.publish(idle.clone());
                        } else {
                            shell.schedule_redraw_at(from + *after);
                        }
                    }
                }
                _ => {}
            }
        }
        let state = tree.state.downcast_mut::<State>();
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            if state.trailing
                && state
                    .last_pulse
                    .is_none_or(|at| now.saturating_duration_since(at) >= self.interval)
            {
                state.trailing = false;
                state.last_pulse = Some(*now);
                shell.publish(self.on_activity.clone());
            }
        } else if is_activity(event) && cursor.is_over(layout.bounds()) {
            let now = Instant::now();
            match state.last_pulse {
                Some(at) if now.saturating_duration_since(at) < self.interval => {
                    if !state.trailing {
                        state.trailing = true;
                        shell.schedule_redraw_at(at + self.interval);
                        tree.raise_frame_interest();
                    }
                }
                _ => {
                    state.last_pulse = Some(now);
                    shell.publish(self.on_activity.clone());
                }
            }
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.content, tree, 0);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
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
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
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
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<EnzActivityPulse<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: Clone + 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn from(pulse: EnzActivityPulse<'a, Message, Theme, Renderer>) -> Self {
        Element::new(pulse)
    }
}
