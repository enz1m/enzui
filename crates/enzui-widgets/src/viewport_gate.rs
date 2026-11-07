//! Visibility gating for self-animating child widgets.
//!
//! Positive-area overlap keeps animation enabled; touching edges and empty
//! viewports do not. The gate restores the prior redraw request while hidden,
//! preserving requests from siblings, and forwards ordinary events, drawing
//! and operations to the child regardless of visibility.
//!
//! Visibility starts enabled and refreshes on event or frame dispatch. Hidden
//! children leave targeted frame routes; geometry changes need an external
//! routed frame with the new viewport to resume animation. The iced runtime
//! and scrollable in the enzICED fork supply those routes.

use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};
/// Whether bounds and viewport overlap with positive area.
/// Touching edges and zero-area rectangles do not count.
pub fn is_visible(bounds: Rectangle, viewport: &Rectangle) -> bool {
    bounds.intersects(viewport)
}

/// Suppress a hidden child's redraw requests and cached frame interest.
/// Ordinary events and operations still reach the child; messages and layout
/// invalidation are unaffected. Visibility starts enabled until dispatch first
/// observes the viewport. The wrapper does not clip drawing or gate overlays.
pub struct ViewportGate<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
}

#[derive(Debug)]
struct State {
    visible: bool,
}

impl Default for State {
    fn default() -> Self {
        Self { visible: true }
    }
}

impl<'a, Message, Theme, Renderer> ViewportGate<'a, Message, Theme, Renderer> {
    /// Wrap a child without changing its sizing or introducing a polling timer.
    pub fn new(child: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        Self {
            child: child.into(),
        }
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for ViewportGate<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<State>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn is_void(&self) -> bool {
        self.child.as_widget().is_void()
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        tree.state.downcast_ref::<State>().visible
            && tree.children.first().is_none_or(Tree::has_frame_interest)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child.as_widget_mut().operate(
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
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let visible = is_visible(layout.bounds(), viewport);
        tree.state.downcast_mut::<State>().visible = visible;
        // Hidden children must not erase redraw requests already made by siblings.
        let before = shell.redraw_request();

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
        if !visible {
            Shell::replace_redraw_request(shell, before);
        }
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
        let visible = is_visible(layout.bounds(), viewport);
        tree.state.downcast_mut::<State>().visible = visible;
        // Hidden children must not erase redraw requests already made by siblings.
        let before = shell.redraw_request();
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
        if !visible {
            Shell::replace_redraw_request(shell, before);
        }
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
        renderer: &Renderer,
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
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
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

impl<'a, Message, Theme, Renderer> From<ViewportGate<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: ViewportGate<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}
