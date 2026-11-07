use super::*;
use crate::accessibility_metrics::DEFAULT_MIN_HIT_TARGET;
use crate::focus_button::focus_button_maybe_with;
use iced::widget::{column, container};
use std::rc::Rc;
use std::time::Instant;

/// Direction requested by the grabber or by content at its scroll boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SheetResizeDirection {
    /// Move to the next taller resolved detent.
    Expand,
    /// Move to the next shorter resolved detent, or dismiss at the floor.
    Collapse,
}

/// Input accepted by [`SheetPresenter`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SheetPresenterEvent {
    /// A direct pointer or touch gesture from the sheet.
    Gesture(SheetGesture),
    /// Pointer, keyboard, or assistive-technology activation of the grabber.
    GrabberActivated,
    /// The content scroll position changed, in logical pixels from its top.
    ContentScrolled(f32),
    /// Content at its top edge handed an unconsumed scroll to the sheet.
    ScrollAtTop(SheetResizeDirection),
}

/// A committed detent transition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SheetDetentChange {
    /// The previously selected ladder index.
    pub previous: usize,
    /// The newly selected ladder index.
    pub selected: usize,
    /// The newly selected resolved height in logical pixels.
    pub height: f32,
}

/// An application-visible result from [`SheetPresenter::update`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SheetPresenterOutcome {
    /// The selected resolved detent changed.
    DetentChanged(SheetDetentChange),
    /// A downward commit below the smallest stop requested dismissal.
    Dismissed(DismissCause),
}

/// Policy for handing content scrolling at its top edge to the sheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SheetScrollToExpand {
    /// Keep all scrolling inside the content.
    Disabled,
    /// Resize the sheet when content at its top cannot consume the gesture.
    #[default]
    Enabled,
}

/// Builds the batteries-included resizable sheet controlled by `presenter`.
///
/// Route the returned events to [`SheetPresenter::update`]. Use [`enz_sheet`]
/// instead when the surface has one static height or the application owns a
/// custom gesture coordinator.
#[must_use]
pub fn enz_resizable_sheet<'a, Message: Clone + 'a>(
    presenter: &SheetPresenter,
    content: impl Into<Element<'a, Message>>,
    on_event: impl Fn(SheetPresenterEvent) -> Message + Clone + 'a,
    at: Instant,
) -> Element<'a, Message> {
    presenter.view_scrollable(content, on_event, at)
}

/// Stateful controller for a resizable detent sheet.
///
/// The presenter owns the validated ladder, selected stop, signed live drag,
/// measured viewport, scroll boundary, and settle animation. Build the sheet
/// with [`view`](Self::view), route its events back through
/// [`update`](Self::update), and handle only the typed outcome.
#[derive(Debug)]
pub struct SheetPresenter {
    ladder: DetentLadder,
    selected: usize,
    viewport: SheetViewport,
    drag: SheetDrag,
    drag_origin_height: f32,
    settle: enzui_core::sheet::SheetSettle,
    policy: DragPolicy,
    scroll_to_expand: SheetScrollToExpand,
    content_scroll_top: f32,
    dismissed: bool,
    grabber_cycle_direction: SheetResizeDirection,
    grabber_id: iced::advanced::widget::Id,
}

impl SheetPresenter {
    /// Creates a presenter at `selected`, clamped to the validated ladder.
    #[must_use]
    pub fn new(
        ladder: DetentLadder,
        selected: usize,
        viewport: SheetViewport,
        reduced_motion: bool,
    ) -> Self {
        let selected = selected.min(ladder.len().saturating_sub(1));
        let viewport = sanitize_viewport(viewport);
        let height = viewport.height(&ladder, selected);
        Self {
            ladder,
            selected,
            viewport,
            drag: SheetDrag::resting(),
            drag_origin_height: height,
            settle: enzui_core::sheet::SheetSettle::new(height, reduced_motion),
            policy: DragPolicy::default(),
            scroll_to_expand: SheetScrollToExpand::Enabled,
            content_scroll_top: 0.0,
            dismissed: false,
            grabber_cycle_direction: SheetResizeDirection::Expand,
            grabber_id: iced::advanced::widget::Id::unique(),
        }
    }

    /// Sets drag thresholds and whether collapse below the floor may dismiss.
    pub fn set_drag_policy(&mut self, policy: DragPolicy) {
        self.policy = policy;
    }

    /// Settles between stops on `spring` instead of the theme's curve, or
    /// `None` for the curve. A released drag hands the spring its velocity,
    /// so a flick carries on into the stop and a slow release eases in.
    pub fn set_spring(&mut self, spring: Option<enzui_theme::Spring>) {
        self.settle.set_spring(spring);
    }

    /// Enables or disables scroll-to-expand handoff.
    pub fn set_scroll_to_expand(&mut self, policy: SheetScrollToExpand) {
        self.scroll_to_expand = policy;
    }

    /// Adopts runtime duration and easing tokens.
    pub fn sync_motion(&mut self, motion: &enzui_theme::MotionScale) {
        self.settle.sync_motion(motion);
    }

    /// Changes reduced-motion policy, completing an active settle when enabled.
    pub fn set_reduced_motion(&mut self, reduced_motion: bool) {
        self.settle.set_reduced_motion(reduced_motion);
    }

    /// Replaces the measured viewport and cancels direct manipulation.
    ///
    /// The visible height is preserved as the new settle origin, while the
    /// selected detent is re-resolved against the new geometry.
    pub fn set_viewport(&mut self, viewport: SheetViewport, at: Instant) {
        let from = self.height(at);
        self.drag.cancel();
        self.viewport = sanitize_viewport(viewport);
        self.settle.begin_from(from, self.resting_height(), at);
    }

    /// Selects a stop and settles to its current resolved height.
    pub fn select(&mut self, selected: usize, at: Instant) -> Option<SheetDetentChange> {
        let selected = selected.min(self.ladder.len().saturating_sub(1));
        let from = self.height(at);
        self.drag.cancel();
        self.grabber_cycle_direction = SheetResizeDirection::Expand;
        self.commit_stop(selected, from, at)
    }

    /// Resets direct manipulation and selects the tallest distinct stop.
    pub fn reset_to_largest(&mut self, at: Instant) -> Option<SheetDetentChange> {
        self.drag.cancel();
        self.dismissed = false;
        self.select(self.ladder.len().saturating_sub(1), at)
    }

    /// The validated detent ladder.
    #[must_use]
    pub const fn ladder(&self) -> &DetentLadder {
        &self.ladder
    }

    /// The selected ladder index.
    #[must_use]
    pub const fn selected(&self) -> usize {
        self.selected
    }

    /// The measured viewport used for rendering and release resolution.
    #[must_use]
    pub const fn viewport(&self) -> SheetViewport {
        self.viewport
    }

    /// The signed live displacement; negative values are upward.
    #[must_use]
    pub const fn displacement(&self) -> f32 {
        self.drag.distance()
    }

    /// The signed live velocity in logical pixels per second.
    #[must_use]
    pub const fn velocity(&self) -> f32 {
        self.drag.signed_velocity()
    }

    /// Whether direct manipulation is active.
    #[must_use]
    pub const fn is_dragging(&self) -> bool {
        self.drag.is_active()
    }

    /// Whether a settle still needs animation frames at `at`.
    #[must_use]
    pub fn is_settling(&self, at: Instant) -> bool {
        self.settle.is_animating(at)
    }

    /// The settle animation used by the presenter.
    #[must_use]
    pub const fn settle(&self) -> &enzui_core::sheet::SheetSettle {
        &self.settle
    }

    /// The selected stop's resolved resting height.
    #[must_use]
    pub fn resting_height(&self) -> f32 {
        self.viewport.height(&self.ladder, self.selected)
    }

    /// The height to render at `at`, including direct manipulation and settling.
    #[must_use]
    pub fn height(&self, at: Instant) -> f32 {
        if self.drag.is_active() {
            let resting = self.resting_height();
            let upper = self
                .next_above()
                .map_or(resting, |index| self.viewport.height(&self.ladder, index));
            let lower = self
                .next_below()
                .map_or(0.0, |index| self.viewport.height(&self.ladder, index));
            (self.drag_origin_height - self.drag.distance())
                .clamp(lower.min(upper), upper.max(lower))
        } else if self.settle.is_animating(at) {
            self.settle.height(at)
        } else {
            self.resting_height()
        }
    }

    /// Whether the current geometry has more than one distinct resting height.
    #[must_use]
    pub fn is_resizable(&self) -> bool {
        if self.ladder.len() < 2 {
            return false;
        }
        if !self.viewport.has_usable_height() {
            return !self.viewport.is_measured();
        }
        let first = self.viewport.height(&self.ladder, 0);
        (1..self.ladder.len()).any(|index| self.viewport.height(&self.ladder, index) != first)
    }

    /// Handles one widget event and returns at most one committed outcome.
    pub fn update(
        &mut self,
        event: SheetPresenterEvent,
        at: Instant,
    ) -> Option<SheetPresenterOutcome> {
        if self.dismissed {
            return None;
        }
        match event {
            SheetPresenterEvent::Gesture(SheetGesture::Pressed) => {
                if self.is_resizable() {
                    self.drag_origin_height = self.height(at);
                    self.drag.press();
                }
                None
            }
            SheetPresenterEvent::Gesture(SheetGesture::Moved { distance, dt }) => {
                self.drag.moved(distance, dt);
                None
            }
            SheetPresenterEvent::Gesture(SheetGesture::Released { distance, dt }) => {
                self.release(Some((distance, dt)), at)
            }
            SheetPresenterEvent::Gesture(SheetGesture::Cancelled) => {
                let from = self.height(at);
                let velocity = -self.drag.release_velocity(None);
                self.drag.cancel();
                self.settle
                    .begin_from_with_velocity(from, self.resting_height(), velocity, at);
                None
            }
            SheetPresenterEvent::GrabberActivated => self
                .cycle_grabber(at)
                .map(SheetPresenterOutcome::DetentChanged),
            SheetPresenterEvent::ContentScrolled(top) => {
                self.content_scroll_top = finite_non_negative(top);
                None
            }
            SheetPresenterEvent::ScrollAtTop(direction) => {
                if self.scroll_to_expand == SheetScrollToExpand::Enabled
                    && self.content_scroll_top <= f32::EPSILON
                {
                    self.step_or_dismiss(direction, at)
                } else {
                    None
                }
            }
        }
    }

    /// Wraps content in the framework scroll view and reports its live top offset.
    #[must_use]
    pub fn scrollable<'a, Message: 'a>(
        &self,
        content: impl Into<Element<'a, Message>>,
        on_event: impl Fn(SheetPresenterEvent) -> Message + 'a,
    ) -> Element<'a, Message> {
        crate::scroll_view::enz_scroll_view(content)
            .on_scroll(move |viewport| {
                on_event(SheetPresenterEvent::ContentScrolled(
                    viewport.absolute_offset().y,
                ))
            })
            .into()
    }

    /// Builds an operable sheet whose content uses the coordinated scroll view.
    #[must_use]
    pub fn view_scrollable<'a, Message: Clone + 'a>(
        &self,
        content: impl Into<Element<'a, Message>>,
        on_event: impl Fn(SheetPresenterEvent) -> Message + Clone + 'a,
        at: Instant,
    ) -> Element<'a, Message> {
        let scrollable = self.scrollable(content, on_event.clone());
        self.view(scrollable, on_event, at)
    }

    /// Builds the operable, resizable sheet surface.
    ///
    /// The grabber is one named button with a framework-minimum hit target.
    /// Pointer click, touch tap, Enter, Space, and routed accessibility
    /// activation all publish [`SheetPresenterEvent::GrabberActivated`].
    #[must_use]
    pub fn view<'a, Message: Clone + 'a>(
        &self,
        content: impl Into<Element<'a, Message>>,
        on_event: impl Fn(SheetPresenterEvent) -> Message + 'a,
        at: Instant,
    ) -> Element<'a, Message> {
        let on_event: Rc<dyn Fn(SheetPresenterEvent) -> Message + 'a> = Rc::new(on_event);
        let resizable = self.is_resizable();
        let grabber: Element<'a, Message> = if resizable {
            let count = self.distinct_stop_count();
            let position = self.distinct_stop_position();
            let state = enzui_core::localized!("enzui-sheet-current-stop-of", "position" => position, "count" => count);
            let activation_map = Rc::clone(&on_event);
            let pill = container(space::Space::new())
                .width(Length::Fixed(HANDLE_WIDTH))
                .height(Length::Fixed(HANDLE_HEIGHT))
                .style(move |theme: &iced::Theme| container::Style {
                    background: Some(Background::Color(resolve_tokens(None, theme).handle)),
                    border: Border {
                        radius: HANDLE_RADIUS.into(),
                        ..Border::default()
                    },
                    ..container::Style::default()
                });
            focus_button_maybe_with(
                pill,
                Some(Box::new(move || {
                    activation_map(SheetPresenterEvent::GrabberActivated)
                })),
            )
            .id(self.grabber_id.clone())
            .width(Length::Fill)
            .height(Length::Fixed(DEFAULT_MIN_HIT_TARGET.max(44.0)))
            .content_align_x(iced::Alignment::Center)
            .content_align_y(iced::Alignment::Center)
            .semantics(
                Semantics::new(Role::Button)
                    .label(enzui_core::localization::text("enzui-sheet-resize-sheet"))
                    .description(state),
            )
            .outset_focus_ring(true)
            .style_focus(|_theme, _status| iced::widget::button::Style {
                background: None,
                border: Border::default(),
                ..iced::widget::button::Style::default()
            })
            .into()
        } else {
            space::Space::new().height(Length::Fixed(0.0)).into()
        };

        let body: Element<'a, Message> = if resizable {
            iced::widget::stack![
                column![
                    space::Space::new().height(Length::Fixed(HANDLE_HEIGHT + 8.0)),
                    content.into()
                ]
                .width(Length::Fill)
                .height(Length::Fill),
                grabber,
            ]
            .width(Length::Fill)
            .into()
        } else {
            content.into()
        };
        let sheet = enz_sheet(body).height(self.height(at)).show_handle(false);
        if !resizable {
            return sheet.into_element();
        }

        let drag_map = Rc::clone(&on_event);
        let scroll_map = Rc::clone(&on_event);
        sheet
            .on_drag(move |gesture| drag_map(SheetPresenterEvent::Gesture(gesture)))
            .grabber_drag_band(SHEET_PADDING + 44.0)
            .on_scroll_boundary(move |direction| {
                scroll_map(SheetPresenterEvent::ScrollAtTop(direction))
            })
            .scroll_handoff(
                self.scroll_to_expand == SheetScrollToExpand::Enabled
                    && self.content_scroll_top <= f32::EPSILON,
            )
            .into_element()
    }

    fn release(
        &mut self,
        terminal: Option<(f32, f32)>,
        at: Instant,
    ) -> Option<SheetPresenterOutcome> {
        if !self.drag.is_active() {
            return None;
        }
        let from = self.height(at);
        // Height grows upward: the drag's downward speed, negated.
        let velocity = -self.drag.release_velocity(terminal);
        let resolution = self.drag.release_resize(
            &self.ladder,
            self.selected,
            self.policy,
            self.viewport,
            terminal,
        );
        match resolution {
            DragResolution::SpringBack => {
                self.settle
                    .begin_from_with_velocity(from, self.resting_height(), velocity, at);
                None
            }
            DragResolution::Settle(selected) => self
                .commit_stop_moving(selected, from, velocity, at)
                .map(SheetPresenterOutcome::DetentChanged),
            DragResolution::Dismiss => {
                self.dismissed = true;
                Some(SheetPresenterOutcome::Dismissed(DismissCause::Drag))
            }
        }
    }

    fn cycle_grabber(&mut self, at: Instant) -> Option<SheetDetentChange> {
        if let Some(change) = self.step(self.grabber_cycle_direction, at) {
            return Some(change);
        }

        let reversed = match self.grabber_cycle_direction {
            SheetResizeDirection::Expand => SheetResizeDirection::Collapse,
            SheetResizeDirection::Collapse => SheetResizeDirection::Expand,
        };
        let change = self.step(reversed, at)?;
        self.grabber_cycle_direction = reversed;
        Some(change)
    }

    fn step_or_dismiss(
        &mut self,
        direction: SheetResizeDirection,
        at: Instant,
    ) -> Option<SheetPresenterOutcome> {
        if direction == SheetResizeDirection::Collapse
            && self.next_below().is_none()
            && self.policy.dismissible
        {
            self.dismissed = true;
            return Some(SheetPresenterOutcome::Dismissed(DismissCause::Drag));
        }
        self.step(direction, at)
            .map(SheetPresenterOutcome::DetentChanged)
    }

    fn step(&mut self, direction: SheetResizeDirection, at: Instant) -> Option<SheetDetentChange> {
        let selected = match direction {
            SheetResizeDirection::Expand => self.next_above(),
            SheetResizeDirection::Collapse => self.next_below(),
        }?;
        self.commit_stop(selected, self.height(at), at)
    }

    fn commit_stop(
        &mut self,
        selected: usize,
        from: f32,
        at: Instant,
    ) -> Option<SheetDetentChange> {
        let velocity = self.settle.velocity(at);
        self.commit_stop_moving(selected, from, velocity, at)
    }

    /// [`Self::commit_stop`] for a surface moving at `velocity` px/s of height.
    fn commit_stop_moving(
        &mut self,
        selected: usize,
        from: f32,
        velocity: f32,
        at: Instant,
    ) -> Option<SheetDetentChange> {
        if selected == self.selected || selected >= self.ladder.len() {
            return None;
        }
        let previous = self.selected;
        self.selected = selected;
        let height = self.resting_height();
        self.drag_origin_height = height;
        self.settle
            .begin_from_with_velocity(from, height, velocity, at);
        Some(SheetDetentChange {
            previous,
            selected,
            height,
        })
    }

    fn next_above(&self) -> Option<usize> {
        if self.viewport.has_usable_height() {
            self.ladder
                .next_distinct_above(self.selected, self.viewport)
        } else if self.viewport.is_measured() {
            None
        } else {
            self.selected
                .checked_add(1)
                .filter(|next| *next < self.ladder.len())
        }
    }

    fn next_below(&self) -> Option<usize> {
        if self.viewport.has_usable_height() {
            self.ladder
                .next_distinct_below(self.selected, self.viewport)
        } else if self.viewport.is_measured() {
            None
        } else {
            self.selected.checked_sub(1)
        }
    }

    fn distinct_stop_count(&self) -> usize {
        if !self.viewport.has_usable_height() {
            return if self.viewport.is_measured() {
                1
            } else {
                self.ladder.len()
            };
        }
        let mut count = 0;
        let mut previous = None;
        for index in 0..self.ladder.len() {
            let height = self.viewport.height(&self.ladder, index);
            if previous != Some(height) {
                count += 1;
                previous = Some(height);
            }
        }
        count
    }

    fn distinct_stop_position(&self) -> usize {
        if !self.viewport.has_usable_height() {
            return 1;
        }
        let selected_height = self.viewport.height(&self.ladder, self.selected);
        let mut heights = Vec::new();
        for index in 0..=self.selected {
            let height = self.viewport.height(&self.ladder, index);
            if heights.last().copied() != Some(height) {
                heights.push(height);
            }
        }
        heights
            .iter()
            .position(|height| *height == selected_height)
            .map_or(1, |position| position + 1)
    }
}

fn sanitize_viewport(viewport: SheetViewport) -> SheetViewport {
    if viewport.is_measured() {
        SheetViewport::measured(viewport.available(), viewport.content())
            .with_insets(viewport.insets())
    } else {
        SheetViewport::UNKNOWN
    }
}

fn finite_non_negative(value: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}
