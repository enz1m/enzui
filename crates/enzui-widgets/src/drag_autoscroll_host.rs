//! Auto-scroll a contained scrollable while a pointer drag stays near its edges.
//!
//! The content must already contain a scrollable with the supplied `scroll_id`.
//! The host forwards ordinary scrolling and child events, and applies
//! [`AutoScroll`] deltas through scroll operations using redraw-event timestamps.
//! A left press must start inside the host and travel beyond its deadband before
//! auto-scrolling can begin. Match [`EnzDragAutoScrollHost::deadband`] to the drag
//! source’s threshold; the default is four logical pixels.
//!
//! Once active, returning toward the press origin does not cancel the drag.
//! Leaving the edge zone clears its frame origin, so re-entry does not accumulate
//! idle time. Left-button release, window focus loss and window cursor exit end the gesture.
//! Cursor movement can request an initial frame; subsequent ticks request frames
//! only after actual scroll progress, or on a zero-time priming tick with headroom.
//! A clamped scroll boundary therefore ends the continuing frame loop.
//!
//! Operation-driven scrolling does not invoke the scrollable’s `on_scroll` callback.
//! If the application owns the offset, feed [`EnzDragAutoScrollHost::on_scrolled`]
//! back into the model so viewport filtering and retained scroll state agree.
//! This callback reports the new absolute vertical offset only, even when the
//! policy also produces horizontal movement.

use enzui_core::drag_autoscroll::AutoScroll;
use iced::advanced::layout::Measure as _;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::operation::scrollable::{AbsoluteOffset, Scrollable};
use iced::advanced::widget::{Id, Operation, Tree, tree};
use iced::advanced::{Shell, Widget, overlay, renderer};
use iced::mouse;
use iced::time::{Duration, Instant};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector, window};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum DragLifecycle {
    #[default]
    Idle,
    Pressed {
        origin: Point,
    },
    // Clearing the frame origin suspends scrolling without ending the drag.
    Dragging {
        scrolling_since: Option<Instant>,
    },
}

#[derive(Debug, Default)]
struct HostState {
    phase: DragLifecycle,
    // Uses ancestor-translated coordinates, matching the host’s layout bounds.
    last_pointer: Option<Point>,
}

impl HostState {
    fn is_dragging(&self) -> bool {
        matches!(self.phase, DragLifecycle::Dragging { .. })
    }

    fn end_gesture(&mut self) {
        self.phase = DragLifecycle::Idle;
    }

    fn leave_hot_zone(&mut self) {
        if let DragLifecycle::Dragging { scrolling_since } = &mut self.phase {
            *scrolling_since = None;
        }
    }
}

/// Apply edge-scroll policy to a drag over a contained scrollable.
///
/// Construct with [`enz_drag_autoscroll_host`]. The target ID must identify a
/// scrollable within the supplied content; unmatched targets cannot scroll.
pub struct EnzDragAutoScrollHost<'a, Message> {
    content: Element<'a, Message, iced::Theme, iced::Renderer>,
    scroll_id: Id,
    policy: AutoScroll,
    deadband: f32,
    width: Length,
    height: Length,
    on_scrolled: Option<Box<dyn Fn(f32) -> Message + 'a>>,
}

// The priming probe uses only delta direction; this interval never changes an offset.
const NOMINAL_FRAME: Duration = Duration::from_millis(16);

/// Construct an [`EnzDragAutoScrollHost`]. See the module docs.
#[must_use]
pub fn enz_drag_autoscroll_host<'a, Message>(
    content: impl Into<Element<'a, Message, iced::Theme, iced::Renderer>>,
    scroll_id: Id,
    policy: AutoScroll,
) -> EnzDragAutoScrollHost<'a, Message> {
    EnzDragAutoScrollHost {
        content: content.into(),
        scroll_id,
        policy,
        deadband: enzui_core::desktop_settings::input().drag_threshold,
        width: Length::Shrink,
        height: Length::Shrink,
        on_scrolled: None,
    }
}

impl<'a, Message> EnzDragAutoScrollHost<'a, Message> {
    fn observed_offset(
        &mut self,
        tree: &mut Tree,
        child_layout: Layout,
        renderer: &iced::Renderer,
    ) -> Option<Vector> {
        self.probe_scroller(tree, child_layout, renderer)
            .map(|s| s.translation)
    }

    fn probe_scroller(
        &mut self,
        tree: &mut Tree,
        child_layout: Layout,
        renderer: &iced::Renderer,
    ) -> Option<ScrollerGeometry> {
        struct Probe {
            target: Id,
            found: Option<ScrollerGeometry>,
        }
        impl<T> Operation<T> for Probe {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
                operate(self);
            }
            fn scrollable(
                &mut self,
                id: Option<&Id>,
                bounds: Rectangle,
                content_bounds: Size,
                translation: Vector,
                _state: &mut dyn Scrollable,
            ) {
                if id == Some(&self.target) {
                    self.found = Some(ScrollerGeometry {
                        bounds,
                        content_bounds: Rectangle::new(bounds.position(), content_bounds),
                        translation,
                    });
                }
            }
        }
        let mut probe = Probe {
            target: self.scroll_id.clone(),
            found: None,
        };
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            child_layout,
            &child_layout.bounds(),
            renderer,
            &mut probe as &mut dyn Operation,
        );
        probe.found
    }

    /// Set the pointer-travel threshold in logical pixels. Defaults to four.
    ///
    /// Travel must strictly exceed this distance. Zero allows any movement; negative
    /// or non-finite values restore the default. Match the drag source’s threshold.
    #[must_use]
    pub fn deadband(mut self, px: f32) -> Self {
        self.deadband = if px.is_finite() && px >= 0.0 {
            px
        } else {
            enzui_core::desktop_settings::input().drag_threshold
        };
        self
    }

    /// Widget width (default [`Length::Shrink`]).
    #[must_use]
    pub fn width(mut self, w: impl Into<Length>) -> Self {
        self.width = w.into();
        self
    }

    /// Widget height (default [`Length::Shrink`]).
    #[must_use]
    pub fn height(mut self, h: impl Into<Length>) -> Self {
        self.height = h.into();
        self
    }

    /// Publish the new absolute vertical offset after a tick changes scroll translation.
    ///
    /// Operation-driven scrolling bypasses the scrollable’s `on_scroll`. Callers whose
    /// models own the offset must store this value and use it for viewport filtering.
    /// Horizontal movement is not reported by this scalar callback.
    #[must_use]
    pub fn on_scrolled(mut self, f: impl Fn(f32) -> Message + 'a) -> Self {
        self.on_scrolled = Some(Box::new(f));
        self
    }
}

#[derive(Debug, Clone, Copy)]
struct ScrollerGeometry {
    bounds: Rectangle,
    content_bounds: Rectangle,
    translation: Vector,
}

impl ScrollerGeometry {
    fn can_move(self, delta: Vector) -> bool {
        fn headroom(pos: f32, viewport: f32, content: f32, d: f32) -> bool {
            if !(d.is_finite() && pos.is_finite() && viewport.is_finite() && content.is_finite()) {
                return false;
            }
            let max = (content - viewport).max(0.0);
            if d > 0.0 {
                pos < max
            } else if d < 0.0 {
                pos > 0.0
            } else {
                false
            }
        }
        headroom(
            self.translation.x,
            self.bounds.width,
            self.content_bounds.width,
            delta.x,
        ) || headroom(
            self.translation.y,
            self.bounds.height,
            self.content_bounds.height,
            delta.y,
        )
    }
}

struct ScrollBy {
    target: Id,
    offset: AbsoluteOffset,
}

impl<T> Operation<T> for ScrollBy {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        content_bounds: Size,
        _translation: Vector,
        state: &mut dyn Scrollable,
    ) {
        if id == Some(&self.target) {
            state.scroll_by(
                self.offset,
                iced::advanced::widget::operation::Animation::Instant,
                bounds,
                content_bounds,
            );
        }
    }
}

impl<'a, Message> EnzDragAutoScrollHost<'a, Message> {
    fn tick(
        &mut self,
        tree: &mut Tree,
        now: Instant,
        bounds: Rectangle,
        child_layout: Layout,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        let now = &now;
        let state = tree.state.downcast_mut::<HostState>();
        let DragLifecycle::Dragging { scrolling_since } = state.phase else {
            return;
        };
        let Some(pointer) = state.last_pointer else {
            return;
        };

        let in_zone = pointer_in_zone(pointer, bounds, self.policy.edge());
        if !in_zone {
            // Discard elapsed time outside the zone to avoid a catch-up jump on re-entry.
            state.leave_hot_zone();
            return;
        }

        let elapsed =
            scrolling_since.map_or(Duration::ZERO, |prev| now.saturating_duration_since(prev));
        let is_priming_tick = scrolling_since.is_none();
        state.phase = DragLifecycle::Dragging {
            scrolling_since: Some(*now),
        };

        let delta = self.policy.delta(pointer, bounds, elapsed);
        let mut moved = false;
        if delta != Vector::ZERO {
            let before = self.observed_offset(tree, child_layout, renderer);
            let mut op = ScrollBy {
                target: self.scroll_id.clone(),
                offset: AbsoluteOffset {
                    x: delta.x,
                    y: delta.y,
                },
            };
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                child_layout,
                &child_layout.bounds(),
                renderer,
                &mut op as &mut dyn Operation,
            );
            let after = self.observed_offset(tree, child_layout, renderer);
            moved = match (before, after) {
                (Some(b), Some(a)) => {
                    // Scroll operations bypass the scrollable’s ordinary notification callback.
                    if b != a
                        && let Some(publish) = &self.on_scrolled
                    {
                        shell.publish(publish(a.y));
                    }
                    b != a
                }
                _ => false,
            };
        }

        // The zero-time priming tick needs headroom instead of an observed delta.
        let start_the_loop = is_priming_tick && {
            let probe_delta = self.policy.delta(pointer, bounds, NOMINAL_FRAME);
            probe_delta != Vector::ZERO
                && self
                    .probe_scroller(tree, child_layout, renderer)
                    .is_some_and(|g| g.can_move(probe_delta))
        };

        if moved || start_the_loop {
            shell.request_redraw();
        }
    }
}

impl<'a, Message: 'a> Widget<Message, iced::Theme, iced::Renderer>
    for EnzDragAutoScrollHost<'a, Message>
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<HostState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(HostState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        tree.state.downcast_ref::<HostState>().is_dragging()
            || tree.children.first().is_none_or(Tree::has_frame_interest)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        layout::frame(limits, Size::new(self.width, self.height), |limits| {
            self.content
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, limits)
        })
        .apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let child_layout = layout.children(&tree.children).next().expect("one child");
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            child_layout,
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
        let bounds = layout.bounds();
        let child_layout = layout.children(&tree.children).next().expect("one child");

        // Scroll before forwarding the frame so the child presents the newly committed offset.
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            self.tick(tree, *now, bounds, child_layout, renderer, shell);
        }

        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        tree.children[0].refresh_frame_interest(self.content.as_widget());

        let state = tree.state.downcast_mut::<HostState>();

        // Event payload positions lack ancestor translation; sample the supplied cursor instead.
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(pos) = cursor.position_over(bounds) {
                    state.phase = DragLifecycle::Pressed { origin: pos };
                    state.last_pointer = Some(pos);
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let Some(position) = cursor.position() else {
                    state.last_pointer = None;
                    state.leave_hot_zone();
                    return;
                };
                state.last_pointer = Some(position);

                if let DragLifecycle::Pressed { origin } = state.phase
                    && position.x.is_finite()
                    && position.y.is_finite()
                {
                    let dx = position.x - origin.x;
                    let dy = position.y - origin.y;
                    if dx * dx + dy * dy > self.deadband * self.deadband {
                        state.phase = DragLifecycle::Dragging {
                            scrolling_since: None,
                        };
                    }
                }

                if state.is_dragging() {
                    if pointer_in_zone(position, bounds, self.policy.edge()) {
                        shell.request_redraw();
                    } else {
                        state.leave_hot_zone();
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.end_gesture();
            }

            // Pointer/focus loss can send the release elsewhere, so both terminate the gesture.
            Event::Window(window::Event::Unfocused) | Event::Mouse(mouse::Event::CursorLeft) => {
                state.end_gesture();
            }

            Event::Window(window::Event::RedrawRequested(_)) => {}
            _ => {}
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
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout.children(&tree.children).next().expect("one child"),
            cursor,
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
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout.children(&tree.children).next().expect("one child"),
            cursor,
            viewport,
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
        let child_layout = layout.children(&tree.children).next().expect("one child");
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            child_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> EnzDragAutoScrollHost<'a, Message> {
    /// Build the autoscroll element.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message, iced::Theme, iced::Renderer> {
        Element::from(self)
    }
}

impl<'a, Message: 'a> From<EnzDragAutoScrollHost<'a, Message>>
    for Element<'a, Message, iced::Theme, iced::Renderer>
{
    fn from(w: EnzDragAutoScrollHost<'a, Message>) -> Self {
        Element::new(w)
    }
}

fn pointer_in_zone(pointer: Point, bounds: Rectangle, edge: f32) -> bool {
    if !(edge.is_finite() && edge > 0.0) {
        return false;
    }
    if !(pointer.x.is_finite() && pointer.y.is_finite()) {
        return false;
    }
    // Include the far boundary, matching AutoScroll’s full-speed edge policy.
    let inside = pointer.x >= bounds.x
        && pointer.x <= bounds.x + bounds.width.max(0.0)
        && pointer.y >= bounds.y
        && pointer.y <= bounds.y + bounds.height.max(0.0);
    if !inside {
        return false;
    }
    let zx = edge.min(bounds.width * 0.5);
    let zy = edge.min(bounds.height * 0.5);
    let in_x = zx > 0.0 && (pointer.x < bounds.x + zx || pointer.x > bounds.x + bounds.width - zx);
    let in_y = zy > 0.0 && (pointer.y < bounds.y + zy || pointer.y > bounds.y + bounds.height - zy);
    in_x || in_y
}
