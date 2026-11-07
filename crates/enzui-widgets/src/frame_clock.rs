//! Animate draw-time styles using elapsed seconds without an application tick.
//!
//! [`EnzFrameClock`] publishes a phase through [`FrameClockPhase`]. Clone the same
//! handle into a style closure that reads it on every draw; layout changes require
//! a separate mechanism. Time comes from redraw-event timestamps, beginning at
//! the first animating frame. Stilling preserves the last sample; resuming includes
//! the time spent still or offscreen, rather than resetting the clock.
//!
//! The clock defaults to active and follows theme reduced motion.
//! [`EnzFrameClock::active`] and [`EnzFrameClock::reduced_motion`] override these
//! choices. Explicit inactivity or reduced motion schedules no clock frames.
//! Visible clocks following a reduced-motion theme schedule a low-rate restart
//! probe so a policy change can restart animation. Offscreen clocks request no
//! frames, but can still update their phase on redraws delivered by the runtime.
//! Window focus loss alone does not stop the clock.
//!
//! The wrapped child still receives events and can request frames of its own.
//! Avoid a parallel application timer for the same animation. Elapsed seconds use
//! `f32`; reduce by the animation period when long-running precision matters.
//! For standard loading visuals, use [`crate::spinner`] or [`crate::skeleton`].

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{self, Widget};
use iced::advanced::{Shell, overlay, renderer};
use iced::mouse;
use iced::time::Instant;
use iced::{Element, Event, Length, Rectangle, Size, Vector, window};
use std::cell::Cell;
use std::rc::Rc;

/// Share the clock’s elapsed seconds with draw-time style closures.
///
/// Clone the handle created by [`frame_clock_phase`] into the closure and pass
/// the same handle to [`enz_frame_clock`]. The sample is zero before the first
/// animating frame and stays at its last value while still. Resuming includes
/// elapsed time since the clock started, including any still interval.
pub type FrameClockPhase = Rc<Cell<f32>>;

/// Create a shared phase handle initialized to zero.
///
/// See the [module example](self) for its use in a draw-time style closure.
#[must_use]
pub fn frame_clock_phase() -> FrameClockPhase {
    Rc::new(Cell::new(0.0))
}

#[derive(Debug)]
struct FrameClockState {
    started_at: Option<Instant>,
    frame_at: Option<Instant>,
    // Update has no theme; unknown policy stays still until draw observes it.
    theme_reduced_motion: Cell<Option<bool>>,
    visible: bool,
}

impl Default for FrameClockState {
    fn default() -> Self {
        Self {
            started_at: None,
            frame_at: None,
            theme_reduced_motion: Cell::new(None),
            visible: true,
        }
    }
}

impl FrameClockState {
    fn elapsed_secs(&self) -> f32 {
        match (self.started_at, self.frame_at) {
            (Some(start), Some(now)) => now.saturating_duration_since(start).as_secs_f32(),
            _ => 0.0,
        }
    }
}

/// Publish elapsed seconds for an element’s draw-time animation.
///
/// Construct with [`enz_frame_clock`]. Layout and child interactions are delegated
/// to the wrapped element; frame scheduling follows activity, visibility and motion policy.
pub struct EnzFrameClock<'a, Message> {
    inner: Element<'a, Message>,
    active: bool,
    reduced_motion: Option<bool>,
    phase: FrameClockPhase,
    cadence: Option<iced::time::Duration>,
}

/// Wrap an element in a clock publishing to its style closure’s phase handle.
///
/// The clock defaults to active and follows theme reduced motion. Pass the same
/// handle the closure reads; see the [module example](self).
#[must_use]
pub fn enz_frame_clock<'a, Message>(
    inner: impl Into<Element<'a, Message>>,
    phase: FrameClockPhase,
) -> EnzFrameClock<'a, Message> {
    EnzFrameClock {
        inner: inner.into(),
        active: true,
        reduced_motion: None,
        phase,
        cadence: None,
    }
}

impl<'a, Message> EnzFrameClock<'a, Message> {
    /// Whether the animation is live (default `true`).
    ///
    /// `false` is build-time stillness: the phase stops advancing and the
    /// clock schedules nothing at all — not even the reduced-motion restart
    /// probe, because nothing external to this widget instance can revoke a
    /// caller's decision, so probing it would be pure busy-loop cost.
    /// Flip it back by rebuilding the view, which happens anyway
    /// when the reported operation finishes.
    #[must_use]
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Override the theme's reduced-motion policy for this instance, in either
    /// direction.
    ///
    /// `true` stills the clock even under an animating theme; `false` runs it
    /// even under a reduced-motion theme. **Reach for `false` only for motion
    /// that carries information a still frame cannot** — WCAG 2.2.2 exists
    /// because prolonged automatic motion is a real accessibility cost, and a
    /// decorative sweep is never worth overriding a user preference for. With
    /// no call the theme decides, which is the right default.
    #[must_use]
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = Some(reduced);
        self
    }

    /// Limit phase updates and requested redraws to this interval.
    /// Clamped to 16 ms through one second; the default follows window frames.
    #[must_use]
    pub fn cadence(mut self, interval: iced::time::Duration) -> Self {
        self.cadence = Some(interval.clamp(
            iced::time::Duration::from_millis(16),
            iced::time::Duration::from_secs(1),
        ));
        self
    }

    /// Convert into an `iced::Element` — the documented terminal build step.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message>
    where
        Message: 'a,
    {
        Element::new(self)
    }

    fn should_animate(&self, state: &FrameClockState) -> bool {
        if !self.active {
            return false;
        }
        let reduced = self
            .reduced_motion
            .unwrap_or_else(|| state.theme_reduced_motion.get().unwrap_or(true));
        !reduced
    }

    fn is_theme_still(&self, state: &FrameClockState) -> bool {
        self.active
            && self.reduced_motion.is_none()
            && state.theme_reduced_motion.get().unwrap_or(true)
    }

    fn is_visible(bounds: Rectangle, viewport: &Rectangle) -> bool {
        crate::viewport_gate::is_visible(bounds, viewport)
    }
}

impl<'a, Message: 'a> Widget<Message, iced::Theme, iced::Renderer> for EnzFrameClock<'a, Message> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<FrameClockState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(FrameClockState::default())
    }

    fn diff(&mut self, tree: &mut widget::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.inner));
    }

    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }

    fn frame_interest(&self, tree: &widget::Tree) -> bool {
        let state = tree.state.downcast_ref::<FrameClockState>();
        let own_interest = state.visible && self.active && self.reduced_motion != Some(true);

        own_interest
            || tree
                .children
                .first()
                .is_none_or(widget::Tree::has_frame_interest)
    }

    fn is_void(&self) -> bool {
        self.inner.as_widget().is_void()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) {
        self.inner
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
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
        let visible = Self::is_visible(layout.bounds(), viewport);
        tree.state.downcast_mut::<FrameClockState>().visible = visible;

        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let state = tree.state.downcast_mut::<FrameClockState>();
            if self.should_animate(state) {
                // Keep elapsed time continuous across offscreen and still intervals.
                if state.started_at.is_none() {
                    state.started_at = Some(*now);
                }
                let due = self.cadence.is_none_or(|cadence| {
                    state
                        .frame_at
                        .is_none_or(|last| now.saturating_duration_since(last) >= cadence)
                });
                if due {
                    state.frame_at = Some(*now);
                    self.phase.set(state.elapsed_secs());
                }
                if visible {
                    if let Some(cadence) = self.cadence {
                        shell.request_redraw_at(state.frame_at.unwrap_or(*now) + cadence);
                    } else {
                        shell.request_redraw();
                    }
                }
            } else if self.is_theme_still(state) && visible {
                // Draw latches policy for the next update; probing permits a theme-driven restart.
                shell.request_redraw_at(
                    *now + iced::time::Duration::from_secs(
                        crate::spinner::REDUCED_MOTION_PROBE_SECS,
                    ),
                );
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
        tree.children[0].refresh_frame_interest(self.inner.as_widget());
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
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
        let state = tree.state.downcast_ref::<FrameClockState>();

        state
            .theme_reduced_motion
            .set(Some(crate::theme_lookup::reduced_motion(theme)));

        // Publish before the child’s style closure reads the phase.
        self.phase.set(state.elapsed_secs());

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

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.inner.as_widget_mut().operate(
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

impl<'a, Message: 'a> From<EnzFrameClock<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzFrameClock<'a, Message>) -> Self {
        Element::new(w)
    }
}
