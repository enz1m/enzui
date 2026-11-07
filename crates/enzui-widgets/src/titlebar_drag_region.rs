//! Interpret unclaimed titlebar pointer gestures as window commands.
//!
//! The child receives events first. A left press records its origin; motion
//! of at least [`DRAG_DEADBAND`] logical pixels publishes one Drag command.
//! Double presses recognized by the shared [`ClickDetector`] publish
//! ToggleMaximize instead of arming that press. Right press publishes
//! ShowSystemMenu. The application maps these commands to runtime window tasks.
//!
//! Drag and double-click handling default to enabled, independently of the
//! enabled system-menu gesture. Without [`TitlebarDragRegion::on_gesture`],
//! recognized mouse gestures are still captured but no message is published.
//! Touch supports drag only and uses one pending origin without finger-id
//! tracking. Child cursor interactions take precedence; blank titlebar bounds
//! report the arrow and shield lower stacked content from hover. Layout,
//! painting and accessibility delegate to the child.
//!
//! Release, another mouse button, cursor leave, window unfocus and touch
//! lift/loss disarm a pending press even when the child captures the event.
//! This prevents a later motion from requesting a compositor move after its
//! live pointer grab ends. The compositor may decline a valid move request.
//!
//! Mouse press origins use the supplied cursor, while motion uses the raw
//! event position. Place the region outside coordinate-translating scroll
//! subtrees so these positions share one space. Optional caller-owned
//! [`TitlebarDragState`] avoids an extra retained tree node; its handle must
//! survive view rebuilds to retain gesture classification.

use crate::title_bar_gesture::WindowCommand;
use enzui_core::click::{ClickCount, ClickDetector, DEFAULT_CLICK_DEADBAND, SingleClickPolicy};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector, touch};

/// Minimum held-pointer travel for a window drag, in logical pixels.
///
/// Uses the shared click detector's default radius (6); drag promotion includes
/// the boundary while consecutive-click distance excludes it.
pub const DRAG_DEADBAND: f32 = DEFAULT_CLICK_DEADBAND;

fn ends_the_pointer_grab(event: &Event) -> bool {
    match event {
        Event::Mouse(mouse::Event::ButtonReleased(_)) => true,

        Event::Mouse(mouse::Event::ButtonPressed(button)) => *button != mouse::Button::Left,

        Event::Mouse(mouse::Event::CursorLeft) => true,
        Event::Window(iced::window::Event::Unfocused) => true,

        Event::Touch(touch::Event::FingerLifted { .. } | touch::Event::FingerLost { .. }) => true,
        _ => false,
    }
}

#[derive(Debug)]
struct State {
    clicks: ClickDetector,

    press_origin: Option<Point>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            clicks: ClickDetector::new(SingleClickPolicy::Immediate),
            press_origin: None,
        }
    }
}

/// Caller-owned gesture state for a titlebar region that must not add another
/// retained widget-tree node. Sharing is explicit because the handle must live
/// across application `view` rebuilds for double-click classification.
#[derive(Clone, Debug, Default)]
pub struct TitlebarDragState(std::rc::Rc<std::cell::RefCell<State>>);

/// A transparent wrapper turning pointer gestures over its child into
/// [`WindowCommand`]s. See the module docs.
pub struct TitlebarDragRegion<'a, Message> {
    child: Element<'a, Message>,
    on_gesture: Option<Box<dyn Fn(WindowCommand) -> Message + 'a>>,
    draggable: bool,
    system_menu: bool,
    shared_state: Option<TitlebarDragState>,
}

/// Wrap `child` in a titlebar drag region. Emits nothing until
/// [`on_gesture`](TitlebarDragRegion::on_gesture) is set.
pub fn titlebar_drag_region<'a, Message>(
    child: impl Into<Element<'a, Message>>,
) -> TitlebarDragRegion<'a, Message> {
    TitlebarDragRegion {
        child: child.into(),
        on_gesture: None,
        draggable: true,
        system_menu: true,
        shared_state: None,
    }
}

impl<'a, Message> TitlebarDragRegion<'a, Message> {
    /// Keep gesture state in a caller-owned handle and delegate this wrapper's
    /// tree identity directly to its child. Use this for additional titlebar
    /// regions inside a larger composite that already owns durable state.
    #[must_use]
    pub fn shared_state(mut self, state: &TitlebarDragState) -> Self {
        self.shared_state = Some(state.clone());
        self
    }

    /// Map each recognized command to a message.
    ///
    /// Without a callback, recognized mouse gestures are still captured and
    /// gesture state still advances, but no message is published.
    #[must_use]
    pub fn on_gesture(mut self, f: impl Fn(WindowCommand) -> Message + 'a) -> Self {
        self.on_gesture = Some(Box::new(f));
        self
    }

    /// Enable drag and double-click maximize handling (default `true`).
    ///
    /// Disabling leaves those events to the child. Right-press system menu
    /// handling remains controlled independently by [`Self::system_menu`].
    #[must_use]
    pub fn draggable(mut self, draggable: bool) -> Self {
        self.draggable = draggable;
        self
    }

    /// Enable or disable the right-press system menu gesture (default
    /// enabled). Independent of [`draggable`](Self::draggable): a
    /// non-draggable zone may still legitimately offer the window menu.
    #[must_use]
    pub fn system_menu(mut self, enabled: bool) -> Self {
        self.system_menu = enabled;
        self
    }

    fn with_state_mut<T>(&self, tree: &mut Tree, f: impl FnOnce(&mut State) -> T) -> T {
        if let Some(shared) = &self.shared_state {
            f(&mut shared.0.borrow_mut())
        } else {
            f(tree.state.downcast_mut::<State>())
        }
    }

    fn emit(&self, command: WindowCommand, shell: &mut Shell<'_, Message>) {
        if let Some(f) = &self.on_gesture {
            shell.publish(f(command));
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for TitlebarDragRegion<'_, Message> {
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.shared_state
            .as_ref()
            .map_or_else(iced::advanced::widget::tree::Tag::of::<State>, |_| {
                self.child.as_widget().tag()
            })
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.shared_state.as_ref().map_or_else(
            || iced::advanced::widget::tree::State::new(State::default()),
            |_| self.child.as_widget().state(),
        )
    }

    fn diff(&mut self, tree: &mut Tree) {
        if self.shared_state.is_some() {
            self.child.as_widget_mut().diff(tree);
        } else {
            tree.diff_children(std::slice::from_mut(&mut self.child));
        }
    }

    fn is_void(&self) -> bool {
        self.child.as_widget().is_void()
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        if self.shared_state.is_some() {
            self.child.as_widget_mut().layout(tree, renderer, limits);
        } else {
            self.child
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, limits);
            iced::advanced::layout::pass_through(tree, 0);
        }
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        if self.shared_state.is_some() {
            self.child
                .as_widget_mut()
                .operate(tree, layout, viewport, renderer, operation);
        } else {
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
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
        if self.shared_state.is_some() {
            self.child
                .as_widget_mut()
                .update(tree, event, layout, cursor, renderer, shell, viewport);
            tree.refresh_frame_interest(self.child.as_widget());
        } else {
            self.child.as_widget_mut().update(
                &mut tree.children[0],
                event,
                layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
            crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
        }

        // Disarm before the capture bail: a captured release still ends the compositor grab.
        if ends_the_pointer_grab(event) {
            self.with_state_mut(tree, |state| state.press_origin = None);
        }

        if shell.is_event_captured() {
            return;
        }

        let bounds = layout.bounds();

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if !self.draggable {
                    return;
                }
                let Some(position) = cursor.position_over(bounds) else {
                    return;
                };

                let double = self.with_state_mut(tree, |state| {
                    let click = state.clicks.press(iced::time::Instant::now(), position);
                    if click.count == ClickCount::Double {
                        state.press_origin = None;
                        true
                    } else {
                        state.press_origin = Some(position);
                        false
                    }
                });
                if double {
                    self.emit(WindowCommand::ToggleMaximize, shell);
                }
                shell.capture_event();
            }

            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if !self.draggable {
                    return;
                }
                let promote = self.with_state_mut(tree, |state| {
                    let Some(origin) = state.press_origin else {
                        return false;
                    };
                    if origin.distance(*position) < DRAG_DEADBAND {
                        return false;
                    }

                    // A successful move can hand off the grab without delivering a release.
                    state.press_origin = None;
                    true
                });
                if promote {
                    self.emit(WindowCommand::Drag, shell);
                    shell.capture_event();
                }
            }

            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                if !self.system_menu || cursor.position_over(bounds).is_none() {
                    return;
                }
                self.emit(WindowCommand::ShowSystemMenu, shell);
                shell.capture_event();
            }

            Event::Touch(touch::Event::FingerPressed { position, .. }) => {
                let handles_event = self.draggable && bounds.contains(*position);
                if handles_event {
                    self.with_state_mut(tree, |state| state.press_origin = Some(*position));
                }
            }
            Event::Touch(touch::Event::FingerMoved { position, .. }) => {
                let dragging_disabled = !self.draggable;
                if dragging_disabled {
                    return;
                }
                let promote = self.with_state_mut(tree, |state| {
                    let Some(origin) = state.press_origin else {
                        return false;
                    };
                    if origin.distance(*position) < DRAG_DEADBAND {
                        return false;
                    }
                    // A successful move can hand off the grab without delivering a release.
                    state.press_origin = None;
                    true
                });
                if promote {
                    self.emit(WindowCommand::Drag, shell);
                    shell.capture_event();
                }
            }

            _ => {}
        }
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        if self.shared_state.is_some() {
            crate::frame_routing::interest_transparent(&self.child, tree)
        } else {
            crate::frame_routing::interest_opaque(tree)
        }
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if self.shared_state.is_some() {
            crate::frame_routing::forward_frame_transparent(
                &mut self.child,
                tree,
                event,
                layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
        } else {
            crate::frame_routing::forward_frame_opaque(
                &mut self.child,
                tree,
                0,
                event,
                layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
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
        let child_tree = if self.shared_state.is_some() {
            tree
        } else {
            &tree.children[0]
        };
        self.child
            .as_widget()
            .draw(child_tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let child_tree = if self.shared_state.is_some() {
            tree
        } else {
            &tree.children[0]
        };
        let interaction = self
            .child
            .as_widget()
            .mouse_interaction(child_tree, layout, cursor, viewport, renderer);
        if interaction == mouse::Interaction::None
            && (self.draggable || self.system_menu)
            && cursor.is_over(layout.bounds())
            && cursor.is_over(*viewport)
        {
            mouse::Interaction::Idle
        } else {
            interaction
        }
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
        let child_tree = if self.shared_state.is_some() {
            tree
        } else {
            &mut tree.children[0]
        };
        self.child.as_widget_mut().overlay(
            child_tree,
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<TitlebarDragRegion<'a, Message>> for Element<'a, Message> {
    fn from(w: TitlebarDragRegion<'a, Message>) -> Self {
        Element::new(w)
    }
}
