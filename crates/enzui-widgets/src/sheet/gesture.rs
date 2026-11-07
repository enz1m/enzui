//! Pointer/touch gesture adaptation and top-edge scroll arbitration for sheets.

use super::*;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::Id;
use iced::advanced::widget::operation::Focusable;

struct HitFocusable {
    point: iced::Point,
    hit: bool,
}

impl<T> Operation<T> for HitFocusable {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn focusable(&mut self, _id: Option<&Id>, bounds: Rectangle, _state: &mut dyn Focusable) {
        self.hit |= bounds.contains(self.point);
    }
}

// ---------------------------------------------------------------------------
// The gesture adapter
// ---------------------------------------------------------------------------

/// Live gesture state kept by the runtime between frames.
///
/// This is the *adapter's* state, not the drag's: the drag itself lives in the
/// host's [`SheetDrag`]. What is here is only what is needed to turn a stream
/// of raw events into `(distance, dt)` — where the gesture began, who owns it,
/// and what time it is.
#[derive(Debug, Default)]
pub(crate) struct SheetGestureState {
    /// Ownership and lifecycle of the gesture in flight.
    ///
    /// A shared `SwipeGesture` owns the origin and optional touch finger rather
    /// than duplicating that state across event sites. Central ownership avoids
    /// drift between the carousel and sheet by handling: press-admission, second-finger rejection, foreign-
    /// finger decline, `CursorLeft`/`Unfocused` cancellation, and origin/owner
    /// queries.
    ///
    /// The physics is not here. `SwipeGesture` does not clamp `distance`
    /// upward-past-rest and does not fold the release sample through `moved()`
    /// the way `SheetDrag` does. Migrating the physics onto the
    /// primitive would give a subtly different release feel; the primitive is
    /// therefore used for ownership only, and the sheet still computes
    /// `(distance, dt)` for `SheetGesture::Moved`/`Released` itself and hands
    /// them to `SheetDrag`, whose `release` folds the terminal through
    /// `moved()` — the drag-and-release-decay-by-identical-physics rule
    /// pinned by `drag_and_release_decay_by_identical_physics`.
    swipe: SwipeGesture,
    /// The timestamp of the most recent `RedrawRequested`.
    ///
    /// The frame clock is the time source, exactly as the `row_motion` and
    /// `frame_clock` modules use it, and for the same reason: a `dt` derived
    /// from `Instant::now()` inside `update` is unreproducible under test,
    /// while a `dt` derived from the frame timestamps iced already supplies
    /// makes the whole gesture a pure function of the event stream. `None`
    /// before the first frame.
    frame_at: Option<Instant>,
    /// The frame instant of the previous emitted sample, so `dt` is the time
    /// between *samples* rather than since the press.
    sampled_at: Option<Instant>,
    /// The distance carried by the most recent `Moved` (or `0.0` at the
    /// press). The release edge falls back to this when the lift event carries
    /// no fresh position — a mouse `ButtonReleased` after the cursor has
    /// already left the surface — so `Released` always reports a coherent
    /// terminal even when there is nothing new to observe.
    last_distance: f32,
    resize_owned: bool,
}

/// The swipe policy used for the sheet's gesture ownership: vertical axis, no
/// edge strip (the whole surface is grabbable), any threshold.
///
/// The threshold value is irrelevant here because the sheet does not consult
/// `SwipePolicy::commits` — the commit decision lives in `SheetDrag` +
/// `DragPolicy`. The primitive serves press-admission and finger
/// identity only, so the policy only has to declare the axis so
/// `SwipePolicy::accepts_press` sees vertical bounds correctly and
/// `SwipeGesture::press` records the origin. A distance-only policy with the
/// dismiss threshold is what a full-consumption migration would use, and this
/// stays consistent with that shape.
pub(crate) fn sheet_swipe_policy() -> SwipePolicy {
    SwipePolicy::new(
        SwipeAxis::Vertical,
        FinitePositive::new_or(
            enzui_core::sheet::DEFAULT_DISMISS_THRESHOLD,
            enzui_core::sheet::DEFAULT_DISMISS_THRESHOLD,
        ),
    )
}

impl SheetGestureState {
    /// Seconds since the previous sample, per the frame clock.
    ///
    /// `0.0` when there is no previous sample or no frame has arrived yet —
    /// which [`SheetDrag::moved`] already handles by keeping the previous
    /// velocity while still advancing the position. Several moves inside one
    /// frame therefore report `0.0` and the velocity stays the per-frame
    /// travel, which is the physically meaningful quantity.
    fn advance(&mut self) -> f32 {
        let dt = match (self.sampled_at, self.frame_at) {
            (Some(prev), Some(now)) => now.saturating_duration_since(prev).as_secs_f32(),
            _ => 0.0,
        };
        self.sampled_at = self.frame_at;
        dt
    }

    /// Forget the gesture in flight.
    fn clear(&mut self) {
        self.swipe.clear();
        self.sampled_at = None;
        self.last_distance = 0.0;
        self.resize_owned = false;
    }
}

/// Turns real pointer and touch events into [`SheetGesture`]s.
///
/// Transparent to layout, drawing, semantics and focus: every other trait
/// method delegates to the wrapped surface. Its only job is adapting real
/// pointer and touch events into the `SheetDrag` path; without that adapter,
/// only synthetic button messages can drive a sheet gesture.
pub(crate) struct SheetGestureArea<'a, Message> {
    pub(crate) content: Element<'a, Message>,
    pub(crate) on_drag: Box<dyn Fn(SheetGesture) -> Message + 'a>,
    pub(crate) grabber_drag_band: Option<f32>,
    pub(crate) on_scroll_boundary: Option<Box<dyn Fn(SheetResizeDirection) -> Message + 'a>>,
    pub(crate) scroll_handoff: bool,
}

impl<Message> SheetGestureArea<'_, Message> {
    /// Publish one gesture step and ask for the frame that shows it.
    fn emit(&self, gesture: SheetGesture, shell: &mut Shell<'_, Message>) {
        shell.publish((self.on_drag)(gesture));
        shell.request_redraw();
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for SheetGestureArea<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<SheetGestureState>()
    }

    fn state(&self) -> tree::State {
        tree::State::interactive(SheetGestureState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn is_void(&self) -> bool {
        self.content.as_widget().is_void()
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let content = self
            .content
            .as_widget_mut()
            .measure(&mut tree.children[0], renderer, limits);
        layout::Node::with_children(content.size(), vec![content]).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        if let Some(child) = layout.children(&tree.children).next() {
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                child,
                viewport,
                renderer,
                operation,
            );
        }
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
        let handoff_point = match event {
            Event::Touch(touch::Event::FingerPressed { position, .. }) => Some(*position),
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => cursor.position(),
            _ => None,
        };
        let handoff_candidate = if self.scroll_handoff
            && let Some(position) = handoff_point
            && layout.bounds().contains(position)
            && let Some(child) = layout.children(&tree.children).next()
        {
            let mut hit = HitFocusable {
                point: position,
                hit: false,
            };
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                child,
                &child.bounds(),
                renderer,
                &mut hit,
            );
            !hit.hit
        } else {
            false
        };

        let suppress_child_event = {
            let state = tree.state.downcast_ref::<SheetGestureState>();
            state.resize_owned
                && (matches!(
                    event,
                    Event::Mouse(mouse::Event::CursorMoved { .. })
                        | Event::Touch(touch::Event::FingerMoved { .. })
                ) || (state.last_distance.abs() > 4.0
                    && matches!(
                        event,
                        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                            | Event::Touch(touch::Event::FingerLifted { .. })
                    )))
        };

        if suppress_child_event
            && let Some(cancel) = match event {
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    Some(Event::Mouse(mouse::Event::CursorLeft))
                }
                Event::Touch(touch::Event::FingerLifted { id, position }) => {
                    Some(Event::Touch(touch::Event::FingerLost {
                        id: *id,
                        position: *position,
                    }))
                }
                _ => None,
            }
            && let Some(child) = layout.children(&tree.children).next()
        {
            self.content.as_widget_mut().update(
                &mut tree.children[0],
                &cancel,
                child,
                cursor,
                renderer,
                shell,
                viewport,
            );
        }

        // Content gets first refusal unless the grabber or a top-edge scroll
        // handoff already owns the sequence.
        if !suppress_child_event && let Some(child) = layout.children(&tree.children).next() {
            self.content.as_widget_mut().update(
                &mut tree.children[0],
                event,
                child,
                cursor,
                renderer,
                shell,
                viewport,
            );
        }

        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<SheetGestureState>();

        match event {
            // The frame clock. Latched unconditionally — including while no
            // gesture is in flight — so the first sample of the *next* drag
            // has a time to measure from.
            Event::Window(iced::window::Event::RedrawRequested(now)) => {
                state.frame_at = Some(*now);
            }

            // ----- mouse -----
            //
            // Press/finger identity, second-finger rejection,
            // `CursorLeft` and `Unfocused` cancellation go through
            // `SwipeGesture` (see `sheet_swipe_policy`). The physics — the
            // `(distance, dt)` values carried by `SheetGesture::Moved` and
            // `Released` — stays sheet-owned, because `SheetDrag`'s upward
            // clamp and release fold are behaviours the primitive does
            // not express.
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(position) = cursor.position() else {
                    return;
                };
                let grabber_hit = self.grabber_drag_band.is_some_and(|height| {
                    position.x >= bounds.x
                        && position.x <= bounds.x + bounds.width
                        && position.y >= bounds.y
                        && position.y <= bounds.y + height
                });
                if (shell.is_event_captured() || self.grabber_drag_band.is_some())
                    && !grabber_hit
                    && !handoff_candidate
                {
                    return;
                }
                let policy = sheet_swipe_policy();
                // `press` declines when a gesture is already in flight and
                // when the point is outside `bounds`. It admits the mouse
                // (`finger = None`) with the press point as origin.
                if state
                    .swipe
                    .press(&policy, position, None, bounds, Direction::LeftToRight)
                    == crate::swipe::SwipePhase::Began
                {
                    state.resize_owned = grabber_hit || handoff_candidate;
                    state.sampled_at = state.frame_at;
                    // The press point is distance 0; a release before any
                    // move reports it, so `distance` never carries stale
                    // travel.
                    state.last_distance = 0.0;
                    self.emit(SheetGesture::Pressed, shell);
                }
            }

            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                // A drag owned by a finger is not moved by the mouse. The
                // primitive already answers this: `owner()` is `Some(_)` for a
                // finger-owned gesture, `None` for a mouse press. Gate on the
                // mouse-owned case.
                if state.swipe.owner().is_some() {
                    return;
                }
                let Some(origin) = state.swipe.origin() else {
                    return;
                };
                // Raw, unsanitised: an upward pull is negative and a
                // non-finite position stays non-finite. `SheetDrag::moved`
                // owns both rules, and duplicating them here is how they
                // drift.
                let distance = position.y - origin.y;
                let dt = state.advance();
                state.last_distance = distance;
                self.emit(SheetGesture::Moved { distance, dt }, shell);
            }

            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                // A mouse release says nothing about a finger's drag.
                if !state.swipe.is_active() || state.swipe.owner().is_some() {
                    return;
                }
                // The release edge. `ButtonReleased` carries no
                // position of its own, so the terminal point is where the
                // cursor last was — `last_distance`, updated by every
                // `CursorMoved`. What is genuinely new at the release is the
                // elapsed *time*: `advance()` is the frame gap since the last
                // sample, so a button held still for several frames before
                // lifting recomputes velocity `(last_distance - offset) / dt`
                // over a large `dt` with no travel — decaying the stale flick
                // to rest — while a lift right after a fast move keeps its
                // speed. Ordering: the terminal is computed before
                // `state.clear()` (which resets `last_distance`), or the
                // release would carry a zero. `mouse_release_folds_the_terminal_edge`
                // guards the ordering: swapping the two reports
                // `distance = 0` and the whole release-fold rule fails.
                let terminal = SheetGesture::Released {
                    distance: state.last_distance,
                    dt: state.advance(),
                };
                state.clear();
                self.emit(terminal, shell);
            }

            Event::Mouse(mouse::Event::CursorLeft) => {
                // Only cancel a mouse-owned gesture: a finger's drag survives
                // a cursor exit. `cancel(None)` declines when the owner is a
                // finger, and declines when nothing is in flight, so this arm
                // needs no further gating.
                let cancelled = state.swipe.cancel(None) == crate::swipe::SwipePhase::Cancelled;
                if cancelled {
                    state.sampled_at = None;
                    state.last_distance = 0.0;
                    self.emit(SheetGesture::Cancelled, shell);
                }
            }

            // Losing the window abandons whatever is in flight, mouse or
            // finger: no release is coming, and a drag left half-committed
            // would resolve on the next unrelated event.
            Event::Window(iced::window::Event::Unfocused) => {
                let handles_event = state.swipe.cancel_any() == crate::swipe::SwipePhase::Cancelled;
                if handles_event {
                    state.sampled_at = None;
                    state.last_distance = 0.0;
                    self.emit(SheetGesture::Cancelled, shell);
                }
            }

            // ----- touch (a bottom sheet is a touch-first pattern) -----
            Event::Touch(touch::Event::FingerPressed { id, position }) => {
                let grabber_hit = self.grabber_drag_band.is_some_and(|height| {
                    position.x >= bounds.x
                        && position.x <= bounds.x + bounds.width
                        && position.y >= bounds.y
                        && position.y <= bounds.y + height
                });
                if (shell.is_event_captured() || self.grabber_drag_band.is_some())
                    && !grabber_hit
                    && !handoff_candidate
                {
                    return;
                }
                // A drag is owned by exactly one finger. A second finger
                // touching down while one is already held must not steal it:
                // the owner's origin would move under it, and the thief's lift
                // would resolve a drag it never started. `SwipeGesture::press`
                // declines when a gesture is already in flight (whoever owns
                // it), so a second finger and a mouse-and-then-finger both hit
                // the same declined path.
                let policy = sheet_swipe_policy();
                if state.swipe.press(
                    &policy,
                    *position,
                    Some(*id),
                    bounds,
                    Direction::LeftToRight,
                ) == crate::swipe::SwipePhase::Began
                {
                    state.resize_owned = grabber_hit || handoff_candidate;
                    state.sampled_at = state.frame_at;
                    state.last_distance = 0.0;
                    self.emit(SheetGesture::Pressed, shell);
                }
            }

            Event::Touch(touch::Event::FingerMoved { id, position }) => {
                if state.swipe.owner() != Some(*id) {
                    return;
                }
                let Some(origin) = state.swipe.origin() else {
                    return;
                };
                let distance = position.y - origin.y;
                let dt = state.advance();
                state.last_distance = distance;
                self.emit(SheetGesture::Moved { distance, dt }, shell);
            }

            Event::Touch(touch::Event::FingerLifted { id, position }) => {
                // Only the finger that started the drag may finish it.
                if state.swipe.owner() != Some(*id) {
                    return;
                }
                // The release edge. `FingerLifted` carries the
                // finger's final `position` — on a touch-only device this is
                // the only terminal sample available and must not be discarded.
                // Its distance below the press origin plus the frame
                // gap since the last sample is what decays a stale flick when
                // the finger rested before lifting.
                //
                // Ordering: compute `terminal` from `state.swipe.origin()`
                // before clearing the state — `clear` resets the primitive
                // and `last_distance`, and reading either afterwards would
                // silently zero the terminal. See
                // `finger_lift_terminal_uses_the_origin_before_clear`.
                let terminal = match state.swipe.origin() {
                    Some(origin) => SheetGesture::Released {
                        distance: position.y - origin.y,
                        dt: state.advance(),
                    },
                    None => SheetGesture::Released {
                        distance: state.last_distance,
                        dt: 0.0,
                    },
                };
                state.clear();
                self.emit(terminal, shell);
            }

            Event::Mouse(mouse::Event::WheelScrolled { delta })
                if self.scroll_handoff && !shell.is_event_captured() =>
            {
                let Some(on_scroll_boundary) = &self.on_scroll_boundary else {
                    return;
                };
                if !cursor.is_over(bounds) {
                    return;
                }
                let y = match delta {
                    mouse::ScrollDelta::Lines { y, .. } | mouse::ScrollDelta::Pixels { y, .. } => {
                        *y
                    }
                };
                let direction = if y > 0.0 {
                    SheetResizeDirection::Expand
                } else if y < 0.0 {
                    SheetResizeDirection::Collapse
                } else {
                    return;
                };
                shell.publish(on_scroll_boundary(direction));
                shell.capture_event();
                shell.request_redraw();
            }

            // A CANCELLED touch is not a release: the compositor took the
            // sequence, so the sheet springs back rather than committing.
            Event::Touch(touch::Event::FingerLost { id, .. }) => {
                // collapsible_match wants `swipe.cancel()` in a match guard, but
                // `cancel()` MUTATES the swipe state; keep it in the body.
                #[allow(
                    clippy::collapsible_match,
                    reason = "the condition mutates state, so it cannot be a match guard"
                )]
                if state.swipe.cancel(Some(*id)) == crate::swipe::SwipePhase::Cancelled {
                    state.sampled_at = None;
                    state.last_distance = 0.0;
                    self.emit(SheetGesture::Cancelled, shell);
                }
            }

            _ => {}
        }
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
        if let Some(child) = layout.children(&tree.children).next() {
            self.content.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                child,
                cursor,
                viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let child = layout
            .children(&tree.children)
            .next()
            .map(|child| {
                self.content.as_widget().mouse_interaction(
                    &tree.children[0],
                    child,
                    cursor,
                    viewport,
                    renderer,
                )
            })
            .unwrap_or_default();

        // A control inside the sheet keeps its own cursor: a text field must
        // still read as a text field even though the surface around it is
        // grabbable.
        if child != mouse::Interaction::None {
            return child;
        }

        let state = tree.state.downcast_ref::<SheetGestureState>();
        if state.swipe.is_active() {
            mouse::Interaction::Grabbing
        } else if cursor.is_over(layout.bounds()) {
            // A grab cursor over the sheet, matching the drag affordance.
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::None
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: iced::Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        let Some(child) = layout.children(&tree.children).next() else {
            return Vec::new();
        };
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            child,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<EnzSheet<'a, Message>> for Element<'a, Message> {
    fn from(sheet: EnzSheet<'a, Message>) -> Self {
        sheet.into_element()
    }
}

impl tree::Interactive for SheetGestureState {
    fn cancel_interaction(&mut self) {
        self.clear();
    }
}
