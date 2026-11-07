//! Measured dismissal support for transient editors embedded in larger views.

use std::cell::Cell;

use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{self, Operation, Tree, operation, tree};
use iced::advanced::{Shell, Widget, mouse, overlay};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// Shared cell populated with a child's real layout bounds.
pub type DismissBounds = Cell<Option<Rectangle>>;

/// Record the child's supplied layout rectangle without emitting messages.
///
/// The cell is populated during event dispatch, operations or drawing after
/// layout. Values use logical layout coordinates; a scrolling ancestor may
/// transform them for presentation. Wrap the allocated viewport rather than
/// its scrolling content to measure the visible body. Layout, focus, semantics
/// and child frame interest are forwarded unchanged.
pub fn track_bounds<'a, Message: 'a>(
    child: impl Into<Element<'a, Message>>,
    bounds: &'a DismissBounds,
) -> Element<'a, Message> {
    Element::new(BoundsTracker {
        child: child.into(),
        bounds,
    })
}

struct BoundsTracker<'a, Message> {
    child: Element<'a, Message>,
    bounds: &'a DismissBounds,
}
impl<Message> Widget<Message, iced::Theme, iced::Renderer> for BoundsTracker<'_, Message> {
    fn tag(&self) -> tree::Tag {
        self.child.as_widget().tag()
    }
    fn state(&self) -> tree::State {
        self.child.as_widget().state()
    }
    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }
    fn frame_interest(&self, tree: &Tree) -> bool {
        self.child.as_widget().frame_interest(tree)
    }
    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }
    fn is_void(&self) -> bool {
        self.child.as_widget().is_void()
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
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
        self.bounds.set(Some(layout.bounds()));
        self.child
            .as_widget_mut()
            .update_frame(tree, event, layout, cursor, renderer, shell, viewport);
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
        self.bounds.set(Some(layout.bounds()));
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.bounds.set(Some(layout.bounds()));
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
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
        self.bounds.set(Some(layout.bounds()));
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
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
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

#[derive(Default)]
struct ScopeState {
    had_focus: bool,
    pending_focus_loss: bool,
}
struct FocusProbe<'a> {
    target: &'a widget::Id,
    editor_bounds: Option<Rectangle>,
    focused: bool,
    /// A focused control inside the measured editor, such as the editor's
    /// own trailing actions: focus has not left the editor composite.
    focused_inside: bool,
}
impl<T> Operation<T> for FocusProbe<'_> {
    fn focusable(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        state: &mut dyn operation::Focusable,
    ) {
        if id.is_some_and(|id| id == self.target) {
            self.focused = state.is_focused();
        } else if state.is_focused()
            && self
                .editor_bounds
                .is_some_and(|editor| editor.contains(bounds.center()))
        {
            self.focused_inside = true;
        }
    }
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }
}

/// Wrap the complete application subtree while a measured editor is active.
/// Pointer presses outside `editor_bounds` and completed keyboard,
/// programmatic, or accessibility focus transfers publish `on_dismiss` without
/// consuming the destination interaction. A bare Tab key is not itself a
/// dismissal: the real traversal operation must first move focus, so closing
/// the editor cannot steal focus from the destination control.
pub(crate) fn dismiss_scope<'a, Message: Clone + 'a>(
    child: impl Into<Element<'a, Message>>,
    active: bool,
    editor_bounds: &'a DismissBounds,
    focus_target: widget::Id,
    on_dismiss: Message,
) -> Element<'a, Message> {
    Element::new(DismissScope {
        child: child.into(),
        active,
        editor_bounds,
        focus_target,
        on_dismiss,
    })
}

struct DismissScope<'a, Message> {
    child: Element<'a, Message>,
    active: bool,
    editor_bounds: &'a DismissBounds,
    focus_target: widget::Id,
    on_dismiss: Message,
}
impl<Message: Clone> DismissScope<'_, Message> {
    fn observe(&mut self, tree: &mut Tree, layout: Layout, renderer: &iced::Renderer) {
        if !self.active {
            let state = tree.state.downcast_mut::<ScopeState>();
            state.had_focus = false;
            state.pending_focus_loss = false;
            return;
        }
        let mut probe = FocusProbe {
            target: &self.focus_target,
            editor_bounds: self.editor_bounds.get(),
            focused: false,
            focused_inside: false,
        };
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            &layout.bounds(),
            renderer,
            &mut probe,
        );
        let state = tree.state.downcast_mut::<ScopeState>();
        if probe.focused || probe.focused_inside {
            state.had_focus = true;
        } else if state.had_focus {
            state.had_focus = false;
            state.pending_focus_loss = true;
        }
    }
}
impl<Message: Clone> Widget<Message, iced::Theme, iced::Renderer> for DismissScope<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ScopeState>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(ScopeState::default())
    }
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
        if !self.active {
            let state = tree.state.downcast_mut::<ScopeState>();
            state.had_focus = false;
            state.pending_focus_loss = false;
        }
    }
    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
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
        if !self.active {
            let state = tree.state.downcast_mut::<ScopeState>();
            state.had_focus = false;
            state.pending_focus_loss = false;
        } else if tree.state.downcast_mut::<ScopeState>().pending_focus_loss {
            tree.state.downcast_mut::<ScopeState>().pending_focus_loss = false;
            shell.publish(self.on_dismiss.clone());
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
        if !self.active {
            return;
        }
        let outside = matches!(event, Event::Mouse(mouse::Event::ButtonPressed(_)))
            && cursor.position().is_some_and(|point| {
                self.editor_bounds
                    .get()
                    .is_some_and(|bounds| !bounds.contains(point))
            });
        if outside {
            shell.publish(self.on_dismiss.clone());
        }
        self.observe(tree, layout, renderer);
        if outside {
            // This press already reports the focus loss it caused.
            tree.state.downcast_mut::<ScopeState>().pending_focus_loss = false;
        }
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        op: &mut dyn Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, viewport, renderer, op);
        self.observe(tree, layout, renderer);
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
        self.child.as_widget().draw(
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
        self.child.as_widget().mouse_interaction(
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
