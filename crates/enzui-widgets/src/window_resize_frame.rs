//! Pointer resize regions around a child occupying a client-decorated window.
//!
//! [`crate::app_shell`] installs this frame for client-decorated layouts.
//! Direct callers should make the child fill the window so its bounds describe
//! the window perimeter.
//! Layout, drawing, overlays and accessibility operations pass through to the child.
//!
//! Left presses declined by the child resize within a 6 px perimeter strip.
//! Corner directions extend 16 px along that strip; points merely inside a
//! 16 px corner square still need to lie within the strip. Flush chrome can
//! preempt the child within the outer 3 px via [`WindowResizeFrame::flush_chrome`].
//! Reaches clamp to half the smaller dimension; inclusive boundaries can meet
//! at a tiny window's midpoint, where the classifier chooses one direction.
//!
//! Presses request iced's native resize on the dispatching window unless
//! [`WindowResizeFrame::on_gesture`] maps them to application messages instead.
//! Builder disablement and native non-resizable, maximized or fullscreen state
//! suppress resize input. Hover uses native state cached by the last ordinary
//! update; before that update, the cache assumes resizing is permitted.
//!
//! The frame adds no accessibility node or focus stop and handles no touch
//! resize gesture. Resize handoff notifies the child of pointer loss before requesting
//! native resizing. Related: [`mod@crate::titlebar_drag_region`].

use crate::title_bar_gesture::{ResizeEdge, WindowCommand};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

/// Perimeter resize-strip thickness in logical pixels: 6.
pub const DEFAULT_RESIZE_EDGE_THICKNESS: f32 = 6.0;

/// Corner-direction reach along the perimeter strip in logical pixels: 16.
pub const DEFAULT_RESIZE_CORNER_EXTENT: f32 = 16.0;

/// Flush-chrome preemption reach in logical pixels: 3, intersected with the resize strip.
pub const DEFAULT_RESIZE_SLIVER_THICKNESS: f32 = 3.0;

/// Transparent child wrapper emitting resize requests from its perimeter.
pub struct WindowResizeFrame<'a, Message> {
    child: Element<'a, Message>,
    on_gesture: Option<Box<dyn Fn(WindowCommand) -> Message + 'a>>,
    enabled: bool,
    edge_thickness: f32,
    corner_extent: f32,
    flush_chrome: bool,
    tiled: iced::window::TiledEdges,
}

/// Wrap content in an enabled resize frame with child-first input by default.
/// Resize presses use the native current-window request channel unless
/// [`WindowResizeFrame::on_gesture`] supplies an application message mapping.
pub fn window_resize_frame<'a, Message>(
    child: impl Into<Element<'a, Message>>,
) -> WindowResizeFrame<'a, Message> {
    WindowResizeFrame {
        child: child.into(),
        on_gesture: None,
        enabled: true,
        edge_thickness: DEFAULT_RESIZE_EDGE_THICKNESS,
        corner_extent: DEFAULT_RESIZE_CORNER_EXTENT,
        tiled: iced::window::TiledEdges::default(),
        flush_chrome: false,
    }
}

#[derive(Debug, Clone, Copy)]
struct RuntimeState {
    can_resize: bool,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self { can_resize: true }
    }
}

impl<'a, Message> WindowResizeFrame<'a, Message> {
    /// Map resize presses to application messages instead of native window requests.
    /// The host is responsible for applying the resulting commands.
    #[must_use]
    pub fn on_gesture(mut self, f: impl Fn(WindowCommand) -> Message + 'a) -> Self {
        self.on_gesture = Some(Box::new(f));
        self
    }

    /// Enable resize input and cursors; defaults to `true`.
    /// Native non-resizable, maximized or fullscreen state still suppresses them.
    /// Disabling leaves child input and rendering active.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Edges the compositor tiled against a neighbour or the screen: they do
    /// not resize. The app shell sets this from
    /// [`iced::window::decorations`].
    #[must_use]
    pub fn tiled(mut self, tiled: iced::window::TiledEdges) -> Self {
        self.tiled = tiled;
        self
    }

    /// Preempt child left presses in the outer 3 px of the resize strip.
    /// Defaults to `false`, which lets child input take precedence everywhere.
    /// The preemption reach clamps with the ordinary strip on small bounds.
    #[must_use]
    pub fn flush_chrome(mut self, flush: bool) -> Self {
        self.flush_chrome = flush;
        self
    }

    /// Classify a point in the perimeter strip, returning `None` outside it or the bounds.
    /// Corner directions extend farther along the strip than edge thickness. Reaches
    /// clamp to half the smaller dimension; inclusive midpoint ties prefer corners
    /// in northwest, northeast, southwest, southeast order.
    /// This geometry query ignores builder enablement and native window state.
    #[must_use]
    pub fn classify(&self, bounds: Rectangle, point: Point) -> Option<ResizeEdge> {
        if !bounds.contains(point) {
            return None;
        }
        let edge = self.edge_reach(bounds);
        let corner = self
            .corner_extent
            .max(edge)
            .min(bounds.width / 2.0)
            .min(bounds.height / 2.0);

        let in_left = point.x <= bounds.x + edge;
        let in_right = point.x >= bounds.x + bounds.width - edge;
        let in_top = point.y <= bounds.y + edge;
        let in_bottom = point.y >= bounds.y + bounds.height - edge;

        if !(in_left || in_right || in_top || in_bottom) {
            return None;
        }

        let near_left = point.x <= bounds.x + corner;
        let near_right = point.x >= bounds.x + bounds.width - corner;
        let near_top = point.y <= bounds.y + corner;
        let near_bottom = point.y >= bounds.y + bounds.height - corner;

        Some(if near_top && near_left {
            ResizeEdge::NorthWest
        } else if near_top && near_right {
            ResizeEdge::NorthEast
        } else if near_bottom && near_left {
            ResizeEdge::SouthWest
        } else if near_bottom && near_right {
            ResizeEdge::SouthEast
        } else if in_top {
            ResizeEdge::North
        } else if in_bottom {
            ResizeEdge::South
        } else if in_left {
            ResizeEdge::West
        } else {
            ResizeEdge::East
        })
    }

    fn edge_reach(&self, bounds: Rectangle) -> f32 {
        self.edge_thickness
            .min(bounds.width / 2.0)
            .min(bounds.height / 2.0)
    }

    /// Return the flush-chrome preemption reach in logical pixels.
    /// Zero without flush chrome; otherwise at most 3 px, ordinary edge reach and
    /// half either dimension. This geometry query ignores enablement and native state.
    #[must_use]
    pub fn sliver_reach(&self, bounds: Rectangle) -> f32 {
        if !self.flush_chrome {
            return 0.0;
        }
        DEFAULT_RESIZE_SLIVER_THICKNESS.min(self.edge_reach(bounds))
    }

    /// Classify points where the flush-chrome sliver intersects the ordinary resize strip.
    /// Returns `None` without flush chrome or outside that intersection. Like
    /// [`Self::classify`], this geometry query ignores enablement and native state.
    #[must_use]
    pub fn classify_preempt(&self, bounds: Rectangle, point: Point) -> Option<ResizeEdge> {
        let sliver = self.sliver_reach(bounds);
        if sliver <= 0.0 || !bounds.contains(point) {
            return None;
        }
        let in_sliver = point.x <= bounds.x + sliver
            || point.x >= bounds.x + bounds.width - sliver
            || point.y <= bounds.y + sliver
            || point.y >= bounds.y + bounds.height - sliver;
        if !in_sliver {
            return None;
        }
        self.classify(bounds, point)
    }

    fn active_edge(
        &self,
        runtime_enabled: bool,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<ResizeEdge> {
        if !self.enabled || !runtime_enabled {
            return None;
        }
        let position = cursor.position()?;
        self.classify(bounds, position)
            .filter(|edge| !touches_tiled(*edge, self.tiled))
    }

    fn preempt_edge(
        &self,
        runtime_enabled: bool,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<ResizeEdge> {
        if !self.enabled || !runtime_enabled {
            return None;
        }
        let position = cursor.position()?;
        self.classify_preempt(bounds, position)
            .filter(|edge| !touches_tiled(*edge, self.tiled))
    }

    fn notify_child_pointer_loss(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // Pointer grabs revoke ownership without synthesizing a release or window focus loss.
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            &Event::Mouse(mouse::Event::CursorLeft),
            layout,
            mouse::Cursor::Unavailable,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
    }

    fn emit(&self, command: WindowCommand, shell: &mut Shell<'_, Message>) {
        if let Some(f) = &self.on_gesture {
            shell.publish(f(command));
        } else if let WindowCommand::Resize(edge) = command {
            shell.request_window(iced::window::Request::DragResize(edge.direction()));
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for WindowResizeFrame<'_, Message> {
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<RuntimeState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(RuntimeState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn is_void(&self) -> bool {
        self.child.as_widget().is_void()
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

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
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
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let runtime_enabled = shell.window_state().can_resize();
        tree.state.downcast_mut::<RuntimeState>().can_resize = runtime_enabled;
        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event
            && let Some(edge) = self.preempt_edge(runtime_enabled, layout.bounds(), cursor)
        {
            self.notify_child_pointer_loss(tree, layout, renderer, shell, viewport);
            self.emit(WindowCommand::Resize(edge), shell);
            shell.capture_event();
            return;
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
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);

        if shell.is_event_captured() {
            return;
        }

        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event
            && let Some(edge) = self.active_edge(runtime_enabled, layout.bounds(), cursor)
        {
            self.notify_child_pointer_loss(tree, layout, renderer, shell, viewport);
            self.emit(WindowCommand::Resize(edge), shell);
            shell.capture_event();
        }
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
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
        let runtime_enabled = tree.state.downcast_ref::<RuntimeState>().can_resize;
        if let Some(edge) = self.preempt_edge(runtime_enabled, layout.bounds(), cursor) {
            return edge.cursor();
        }
        let child = self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        );
        if child == mouse::Interaction::None
            && let Some(edge) = self.active_edge(runtime_enabled, layout.bounds(), cursor)
        {
            return edge.cursor();
        }
        child
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

impl<'a, Message: 'a> From<WindowResizeFrame<'a, Message>> for Element<'a, Message> {
    fn from(w: WindowResizeFrame<'a, Message>) -> Self {
        Element::new(w)
    }
}

/// Whether a resize from `edge` would move a tiled side.
fn touches_tiled(edge: ResizeEdge, tiled: iced::window::TiledEdges) -> bool {
    match edge {
        ResizeEdge::North => tiled.top,
        ResizeEdge::South => tiled.bottom,
        ResizeEdge::West => tiled.left,
        ResizeEdge::East => tiled.right,
        ResizeEdge::NorthWest => tiled.top || tiled.left,
        ResizeEdge::NorthEast => tiled.top || tiled.right,
        ResizeEdge::SouthWest => tiled.bottom || tiled.left,
        ResizeEdge::SouthEast => tiled.bottom || tiled.right,
    }
}
