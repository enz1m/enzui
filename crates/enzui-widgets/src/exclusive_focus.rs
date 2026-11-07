//! Repairs multiple focus owners after a captured primary pointer press.
//!
//! After child dispatch, a captured left-button or touch press with at least
//! two registered focus owners clears all of them and replays the press with
//! a fresh shell. Zero or one owner is left untouched by this wrapper; a stack
//! inside it releases focus in the layers a captured press skips, including
//! behind a nonfocusable capturing scrim. State is confined to the wrapped tree.
//!
//! Replay messages are discarded, but child callbacks and other update side
//! effects can run twice. Children must tolerate a repeated press. Replayed
//! layout/frame requests are not forwarded; the wrapper requests redraw.

use enzui_core::focus::{count_focused, release_all_focus};
use iced::advanced::widget::{Operation, Tree, operation};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// Focus repair for layered content whose captured presses can strand multiple owners.
///
/// Repair clears all registered owners and replays the primary press; it does
/// not identify the capturing layer. The child must tolerate repeated press
/// updates, including side effects other than buffered messages.
pub struct ExclusiveFocus<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
}

impl<'a, Message, Theme, Renderer> ExclusiveFocus<'a, Message, Theme, Renderer> {
    /// Wraps `child` with captured-press focus repair.
    pub fn new(child: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        Self {
            child: child.into(),
        }
    }
}

/// Wrap `child` in an [`ExclusiveFocus`].
pub fn exclusive_focus<'a, Message, Theme, Renderer>(
    child: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> ExclusiveFocus<'a, Message, Theme, Renderer> {
    ExclusiveFocus::new(child)
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for ExclusiveFocus<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
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
        let is_press = enzui_core::focus::pointer_press_position(event, cursor).is_some();

        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);

        if !is_press {
            return;
        }

        if !shell.is_event_captured() {
            return;
        }

        let mut count = count_focused();
        self.child.as_widget_mut().operate(
            tree,
            layout,
            &layout.bounds(),
            renderer,
            &mut operation::black_box(&mut count),
        );
        let operation::Outcome::Some(owners) = count.finish() else {
            return;
        };
        if owners < 2 {
            return;
        }

        // Clear all owners before replay so the hit layer can retake focus without stale lower owners.
        let mut release = release_all_focus();
        self.child.as_widget_mut().operate(
            tree,
            layout,
            &layout.bounds(),
            renderer,
            &mut operation::black_box(&mut release),
        );

        // Replay can repeat child side effects, but must not publish its buffered messages.
        let mut discarded: iced::advanced::shell::Bus<Message> = iced::advanced::shell::Bus::new();
        let mut refocus = Shell::new(shell.window(), shell.waker().clone(), &mut discarded);
        self.child.as_widget_mut().update(
            tree,
            event,
            layout,
            cursor,
            renderer,
            &mut refocus,
            viewport,
        );

        shell.request_redraw();
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.child, tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
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
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
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
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
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
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message, Theme, Renderer> From<ExclusiveFocus<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn from(w: ExclusiveFocus<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}
