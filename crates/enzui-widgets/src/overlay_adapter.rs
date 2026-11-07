//! Adapt content-owning overlays to iced’s layout-free [`Overlay`] interface.
//!
//! [`element`] and [`Adapted::new`] measure once against the supplied window size.
//! Drawing, events, operations and cursor queries reuse that node until the adapter
//! is recreated; nested overlays receive the same window size. The node belongs
//! to the adapter rather than a widget tree. Use [`layout::Placed::to_layout`] when
//! passing a measured node to its owning widget.
//!
//! Wrappers without their own geometry can implement [`Overlay`] directly.

use iced::advanced::layout::{self};
use iced::advanced::overlay::{self, Overlay};
use iced::advanced::renderer;
use iced::advanced::widget;
use iced::advanced::{Shell, mouse};
use iced::{Event, Rectangle, Size};

/// An overlay with a window-relative layout node and content operations.
/// Optional methods default to no work, no cursor interaction and no nested overlays.
/// The default stacking index is 1.
pub trait LegacyOverlay<Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    /// Lays the overlay out within the window `bounds`.
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node;

    /// Draws the overlay at `layout`.
    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: layout::Placed<'_>,
        cursor: mouse::Cursor,
    );

    /// Applies a [`widget::Operation`].
    fn operate(
        &mut self,
        _layout: layout::Placed<'_>,
        _renderer: &Renderer,
        _operation: &mut dyn widget::Operation,
    ) {
    }

    /// Processes a runtime [`Event`].
    fn update(
        &mut self,
        _event: &Event,
        _layout: layout::Placed<'_>,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        _shell: &mut Shell<'_, Message>,
    ) {
    }

    /// The [`mouse::Interaction`] over the overlay.
    fn mouse_interaction(
        &self,
        _layout: layout::Placed<'_>,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        mouse::Interaction::None
    }

    /// Nested overlays, if any, created against the same `window`.
    fn overlay<'a>(
        &'a mut self,
        _layout: layout::Placed<'_>,
        _renderer: &Renderer,
        _window: Size,
    ) -> Vec<overlay::Element<'a, Message, Theme, Renderer>> {
        Vec::new()
    }

    /// The stacking index: higher is drawn on top.
    fn index(&self) -> f32 {
        1.0
    }

    /// The area the overlay draws in, given the `node` it was laid out to,
    /// in window coordinates; by default the node's bounds. An overlay laid
    /// out as a window-sized node holding its surfaces reports
    /// [`children_area`] instead.
    fn area(&self, node: &layout::Node) -> Option<Rectangle> {
        Some(node.bounds())
    }

    /// What the overlay is placed against, in window coordinates: the
    /// button, field or point it opened from (`Overlay::anchor`).
    fn anchor(&self) -> Option<Rectangle> {
        None
    }

    /// Whether the overlay takes the input grab when shown in a popup
    /// surface (`Overlay::grabs`): menus opened by a click do, hover
    /// feedback does not.
    fn grabs(&self) -> bool {
        false
    }
}

/// The viewport an overlay draws its content with: everywhere it can be
/// placed. Overlays shown in a popup surface may lie above or left of the
/// window, at negative coordinates, where a viewport starting at the origin
/// would clip their text away.
pub(crate) const UNBOUNDED_VIEWPORT: Rectangle = Rectangle {
    x: -1.0e6,
    y: -1.0e6,
    width: 2.0e6,
    height: 2.0e6,
};

/// The area of `node`'s children, for an overlay laid out as a window-sized
/// node that holds its surfaces.
#[must_use]
pub fn children_area(node: &layout::Node) -> Option<Rectangle> {
    let origin = node.bounds().position();
    node.children()
        .iter()
        .map(|child| child.bounds() + iced::Vector::new(origin.x, origin.y))
        .reduce(|area, child| area.union(&child))
}

/// A [`LegacyOverlay`] with the node it was laid out to at creation.
pub struct Adapted<O> {
    overlay: O,
    node: layout::Node,
    window: Size,
}

impl<O> Adapted<O> {
    /// Lays `overlay` out within `window` and keeps the node for its
    /// lifetime.
    pub fn new<Message, Theme, Renderer>(mut overlay: O, renderer: &Renderer, window: Size) -> Self
    where
        O: LegacyOverlay<Message, Theme, Renderer>,
        Renderer: iced::advanced::Renderer,
    {
        let node = overlay.layout(renderer, window);

        Self {
            overlay,
            node,
            window,
        }
    }
}

impl<O, Message, Theme, Renderer> Overlay<Message, Theme, Renderer> for Adapted<O>
where
    O: LegacyOverlay<Message, Theme, Renderer>,
    Renderer: iced::advanced::Renderer,
{
    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        cursor: mouse::Cursor,
    ) {
        self.overlay.draw(
            renderer,
            theme,
            style,
            layout::Placed::of(&self.node),
            cursor,
        );
    }

    fn operate(&mut self, renderer: &Renderer, operation: &mut dyn widget::Operation) {
        self.overlay
            .operate(layout::Placed::of(&self.node), renderer, operation);
    }

    fn update(
        &mut self,
        event: &Event,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        self.overlay.update(
            event,
            layout::Placed::of(&self.node),
            cursor,
            renderer,
            shell,
        );
        // A popup keeps the wheel turned over it, whether or not its list
        // can still move: scrolling past the end of a menu does not scroll
        // the page under it.
        if self.overlay.grabs()
            && matches!(event, Event::Mouse(mouse::Event::WheelScrolled { .. }))
            && let Some(area) = self.overlay.area(&self.node)
            && cursor.is_over(area)
        {
            shell.capture_event();
        }
    }

    fn mouse_interaction(&self, cursor: mouse::Cursor, renderer: &Renderer) -> mouse::Interaction {
        self.overlay
            .mouse_interaction(layout::Placed::of(&self.node), cursor, renderer)
    }

    fn overlay<'a>(
        &'a mut self,
        renderer: &Renderer,
    ) -> Vec<overlay::Element<'a, Message, Theme, Renderer>> {
        let Self {
            overlay,
            node,
            window,
        } = self;

        overlay.overlay(layout::Placed::of(node), renderer, *window)
    }

    fn index(&self) -> f32 {
        self.overlay.index()
    }

    fn node(&self) -> Option<&layout::Node> {
        Some(&self.node)
    }

    fn area(&self) -> Option<Rectangle> {
        self.overlay.area(&self.node)
    }

    fn anchor(&self) -> Option<Rectangle> {
        self.overlay.anchor()
    }

    fn grabs(&self) -> bool {
        self.overlay.grabs()
    }
}

/// Wraps a [`LegacyOverlay`] as an [`overlay::Element`], laying it out within
/// `window` now.
pub fn element<'a, O, Message, Theme, Renderer>(
    overlay: O,
    renderer: &Renderer,
    window: Size,
) -> overlay::Element<'a, Message, Theme, Renderer>
where
    O: LegacyOverlay<Message, Theme, Renderer> + 'a,
    Renderer: iced::advanced::Renderer,
{
    overlay::Element::new(Box::new(Adapted::new(overlay, renderer, window)))
}
