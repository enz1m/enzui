//! Responsive content with a cache for repeated layout at the same width.
//!
//! Width and height default to Fill. The closure receives the available maximum
//! size and runs on the first layout of each new widget instance. Later layouts
//! reuse content only when the width bits match the most recent build. Returning
//! to an earlier width after a different width rebuilds; height alone does not.
//!
//! Content is measured again even on a cache hit. Closures must produce stable
//! content for a given width during the instance’s lifetime and must not rely on
//! height changes or mutable external state to trigger a rebuild. Construct a new
//! instance when captured application state changes.

use iced::advanced::layout::Measure as _;
use iced::advanced::layout::{self, Layout};
use iced::advanced::mouse;
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget;
use iced::advanced::widget::Tree;
use iced::advanced::{self, Shell};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

#[derive(Debug, Default, Clone, Copy)]
struct ResponsiveState {
    last_built_width: Option<u32>,
}

/// A width-aware container with layout caching. See [module docs](self).
pub struct EnzResponsive<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    view: Box<dyn Fn(Size) -> Element<'a, Message, Theme, Renderer> + 'a>,
    width: Length,
    height: Length,
    content: Element<'a, Message, Theme, Renderer>,
    built_this_cycle: bool,
}

/// Build width-responsive content with both dimensions set to Fill.
/// The closure receives available size; caching compares width only, as described
/// in the module. A new instance always invokes it on its first layout.
pub fn enz_responsive<'a, Message, Theme, Renderer, E>(
    view: impl Fn(Size) -> E + 'a,
) -> EnzResponsive<'a, Message, Theme, Renderer>
where
    Renderer: advanced::Renderer,
    E: Into<Element<'a, Message, Theme, Renderer>>,
{
    EnzResponsive {
        view: Box::new(move |size| view(size).into()),
        width: Length::Fill,
        height: Length::Fill,
        content: Element::new(iced::widget::Space::new()),
        built_this_cycle: false,
    }
}

impl<'a, Message, Theme, Renderer> EnzResponsive<'a, Message, Theme, Renderer>
where
    Renderer: advanced::Renderer,
{
    /// Sets the width of the responsive container.
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Sets the height of the responsive container.
    #[must_use]
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }
}

impl<Message, Theme, Renderer> advanced::Widget<Message, Theme, Renderer>
    for EnzResponsive<'_, Message, Theme, Renderer>
where
    Renderer: advanced::Renderer,
{
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<ResponsiveState>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(ResponsiveState::default())
    }

    fn diff(&mut self, _tree: &mut Tree) {
        // Defer reconciliation until layout replaces the placeholder, preserving child state.
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let limits = limits.width(self.width).height(self.height);
        let size = limits.max();
        let width_bits = size.width.to_bits();
        let state = tree.state.downcast_ref::<ResponsiveState>();
        if self.built_this_cycle && state.last_built_width == Some(width_bits) {
            let node = self.content.as_widget_mut().measure(
                &mut tree.children[0],
                renderer,
                &limits.loose(),
            );
            return layout::cached_factory(&limits, Size::new(self.width, self.height), node)
                .apply(tree);
        }
        self.content = (self.view)(size);
        tree.diff_children(std::slice::from_mut(&mut self.content));
        tree.state
            .downcast_mut::<ResponsiveState>()
            .last_built_width = Some(width_bits);
        self.built_this_cycle = true;

        let node =
            self.content
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, &limits.loose());
        layout::cached_factory(&limits, Size::new(self.width, self.height), node).apply(tree);
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
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("the layout has its one child");
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.content, tree, 0);
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
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_opaque(
            &mut self.content,
            tree,
            0,
            event,
            layout
                .children(&tree.children)
                .next()
                .expect("the layout has its one child"),
            cursor,
            renderer,
            shell,
            viewport,
        );
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
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout
                .children(&tree.children)
                .next()
                .expect("the layout has its one child"),
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
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout
                .children(&tree.children)
                .next()
                .expect("the layout has its one child"),
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("the layout has its one child");
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            child_layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'a, Message, Theme, Renderer>> {
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("the layout has its one child");
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

impl<'a, Message, Theme, Renderer> EnzResponsive<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: advanced::Renderer + 'a,
{
    /// Convert into an [`Element`].
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message, Theme, Renderer> {
        Element::from(self)
    }
}

impl<'a, Message, Theme, Renderer> From<EnzResponsive<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: advanced::Renderer + 'a,
{
    fn from(responsive: EnzResponsive<'a, Message, Theme, Renderer>) -> Self {
        Self::new(responsive)
    }
}
