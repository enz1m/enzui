//! Withdraw a subtree from ordinary input, operations, overlays and hover paint.
//!
//! Inert children retain their layout and persistent widget state, and continue
//! drawing with an unavailable cursor. Operations, including focus traversal
//! and accessibility collection, do not visit the child while inert.
//!
//! Targeted animation frames and frame interest still pass through, allowing
//! visible motion to finish. Frames receive an unavailable cursor while inert.
//! Ordinary event dispatch is blocked, including redraw delivered through that
//! path. The wrapper supplies no dimming or scrim.

use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};
/// Optionally suppress ordinary input, operations and overlays without replacing child state.
/// Keep this wrapper present and change its flag across rebuilds to preserve tree
/// identity. Retained focus, selection and press state are not reset. Drawing
/// and targeted frames blind the cursor while inert; frame dispatch remains enabled.
pub struct Inert<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    inert: bool,
    hide_paint: bool,
}

impl<'a, Message, Theme, Renderer> Inert<'a, Message, Theme, Renderer> {
    /// Wrap the child and block ordinary input, operations and overlays when the flag is true.
    pub fn new(child: impl Into<Element<'a, Message, Theme, Renderer>>, inert: bool) -> Self {
        Self {
            child: child.into(),
            inert,
            hide_paint: false,
        }
    }

    pub(crate) fn without_paint(mut self) -> Self {
        self.hide_paint = true;
        self
    }

    /// Whether ordinary interaction with the child is currently suppressed.
    #[must_use]
    pub const fn is_inert(&self) -> bool {
        self.inert
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for Inert<'_, Message, Theme, Renderer>
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
        if self.inert {
            return;
        }
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }
    // Visible animations may continue while their ordinary input is blocked.
    fn frame_interest(&self, tree: &Tree) -> bool {
        self.child.as_widget().frame_interest(tree)
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
        let cursor = if self.inert {
            mouse::Cursor::Unavailable
        } else {
            cursor
        };
        self.child
            .as_widget_mut()
            .update_frame(tree, event, layout, cursor, renderer, shell, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        if self.inert {
            return mouse::Interaction::default();
        }
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
        if self.inert && self.hide_paint {
            return;
        }
        // Unavailable cannot be recovered with Cursor::land, unlike a levitated cursor.
        let cursor = if self.inert {
            mouse::Cursor::Unavailable
        } else {
            cursor
        };
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
        if self.inert {
            return;
        }
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
        if self.inert {
            return Vec::new();
        }
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message, Theme, Renderer> From<Inert<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(widget: Inert<'a, Message, Theme, Renderer>) -> Self {
        Self::new(widget)
    }
}
