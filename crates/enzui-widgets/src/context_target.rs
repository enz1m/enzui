//! Transparent right-click capture surface with optional routed context-menu actions.
//!
//! Width and height default to Fill, and the resolved frame is the hit region.
//! The child updates first; a captured pointer event suppresses this target's
//! handler. Disabling the target blocks new pointer and routed requests while
//! retaining ordinary child behavior.
//!
//! Callbacks receive logical window coordinates: the pointer position for a
//! right-button press, or the frame centre for a routed request. Translation
//! comes from the last overlay traversal, so ancestors must delegate overlays;
//! a scroll and press in one event batch can use the preceding translation.
//! Mount the menu host at the page root to use these points directly.
//!
//! [`EnzContextTarget::id`] enables a separate context-menu semantics node and
//! routing carrier. [`EnzContextTarget::label`] names that affordance without
//! replacing child semantics. A routed request stays in tree state until an
//! event/frame pass dispatches it; an already armed request can still dispatch
//! through the frame path after the builder is disabled.

use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

#[derive(Debug, Default)]
struct State {
    context_menu_pending: bool,
    // Overlay traversal supplies the content-to-window correction missing from update.
    translation: Vector,
}

/// Capture secondary-button presses within the resolved frame.
/// The lazy callback receives logical window coordinates using the most recent
/// overlay translation; ancestors must delegate overlay traversal.
pub fn enz_context_target<'a, Message, F>(
    child: impl Into<Element<'a, Message>>,
    on_right_click: F,
) -> EnzContextTarget<'a, Message>
where
    Message: 'a,
    F: Fn(Point) -> Message + 'a,
{
    EnzContextTarget {
        child: child.into(),
        on_right_click: Box::new(on_right_click),
        width: Length::Fill,
        height: Length::Fill,
        enabled: true,
        id: None,
        label: None,
    }
}

/// Capture secondary-button presses with a cloned fixed message.
pub fn enz_context_target_msg<'a, Message>(
    child: impl Into<Element<'a, Message>>,
    on_right_click: Message,
) -> EnzContextTarget<'a, Message>
where
    Message: 'a + Clone,
{
    enz_context_target(child, move |_pos| on_right_click.clone())
}

/// Builder returned by [`enz_context_target`].
#[must_use]
pub struct EnzContextTarget<'a, Message> {
    child: Element<'a, Message>,
    on_right_click: Box<dyn Fn(Point) -> Message + 'a>,
    width: Length,
    height: Length,
    enabled: bool,
    id: Option<iced::advanced::widget::Id>,
    label: Option<String>,
}

impl<'a, Message> EnzContextTarget<'a, Message> {
    /// Set the capture-frame width; defaults to [`Length::Fill`].
    /// Shrink uses the child's measured width; parent layout limits still apply.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Set the capture-frame height; defaults to [`Length::Fill`].
    /// The resolved frame, rather than the child alone, supplies hit-test bounds.
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Accept new context requests when true; defaults to true.
    /// False leaves child behavior intact and offers no context-menu node or carrier.
    /// An already armed request remains in tree state and can dispatch on a frame pass.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Set the stable ID required for routed context-menu actions.
    /// Defaults to no ID, which offers no context-menu node or carrier.
    pub fn id(mut self, id: impl Into<iced::advanced::widget::Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Name the context-menu affordance; used only alongside [`Self::id`].
    /// Defaults to no name and does not replace child semantics.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzContextTarget<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn is_void(&self) -> bool {
        self.child.as_widget().is_void()
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        layout::frame(limits, self.size(), |limits| {
            self.child
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
        if self.enabled
            && let Some(id) = &self.id
        {
            let semantics =
                enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Group)
                    .label_opt(self.label.clone())
                    .context_menu();
            let mut node = enzui_core::collect::SemanticsNode::new(semantics);
            operation.custom(Some(id), layout.bounds(), &mut node);
            let mut request = crate::context_menu_action::ContextMenuRequest::new(id.clone());
            operation.custom(Some(id), layout.bounds(), &mut request);
            if request.armed() {
                tree.state.downcast_mut::<State>().context_menu_pending = true;
            }
        }

        let child_layout = child_layout(layout, tree);
        self.child.as_widget_mut().operate(
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
        let child_layout = child_layout(layout, tree);
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);

        if !self.enabled {
            return;
        }
        // Routed work survives child event capture; clear the latch before dispatch.
        {
            let state = tree.state.downcast_mut::<State>();
            if state.context_menu_pending {
                state.context_menu_pending = false;
                let translation = state.translation;
                let centre = to_window(layout.bounds().center(), translation);
                shell.publish((self.on_right_click)(centre));
                shell.request_redraw();
            }
        }

        if shell.is_event_captured() {
            return;
        }

        if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) = event {
            let bounds = layout.bounds();
            if let Some(position) = cursor.position_over(bounds) {
                let translation = tree.state.downcast_ref::<State>().translation;
                shell.publish((self.on_right_click)(to_window(position, translation)));
                shell.capture_event();
            }
        }
    }

    // Enabled identified targets stay routed: operate cannot refresh ancestor frame caches.
    fn frame_interest(&self, tree: &Tree) -> bool {
        (self.enabled && self.id.is_some())
            || tree.state.downcast_ref::<State>().context_menu_pending
            || crate::frame_routing::interest_opaque(tree)
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
            child_layout(layout, tree),
            cursor,
            renderer,
            shell,
            viewport,
        );
        let state = tree.state.downcast_mut::<State>();
        if state.context_menu_pending {
            state.context_menu_pending = false;
            let centre = to_window(layout.bounds().center(), state.translation);
            shell.publish((self.on_right_click)(centre));
            shell.request_redraw();
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
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            child_layout(layout, tree),
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
            child_layout(layout, tree),
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
        tree.state.downcast_mut::<State>().translation = translation;

        let child_layout = child_layout(layout, tree);
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            child_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

fn to_window(point: Point, translation: Vector) -> Point {
    point + translation
}

// Cached layouts may lag the frame; fall back to the wrapper layout until reconciliation.
fn child_layout(layout: Layout, tree: &Tree) -> Layout {
    layout.children(&tree.children).next().unwrap_or(layout)
}

impl<'a, Message> EnzContextTarget<'a, Message>
where
    Message: 'a,
{
    /// Finish the builder with an explicit element type for macro call sites.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzContextTarget<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(w: EnzContextTarget<'a, Message>) -> Self {
        Element::new(w)
    }
}
