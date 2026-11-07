//! Layout wrappers and press ownership for document-tab cells.

use super::*;
use iced::advanced::layout::Measure as _;

pub(crate) struct TitleReserveCap<'a, Message> {
    pub(crate) child: Element<'a, Message>,
    pub(crate) close_reserve: f32,
    pub(crate) ceiling_cap: f32,
}

impl<Message: Clone> iced::advanced::Widget<Message, iced::Theme, iced::Renderer>
    for TitleReserveCap<'_, Message>
{
    fn frame_interest(&self, tree: &iced::advanced::widget::Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
    }

    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> iced::Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        iced::advanced::layout::reserved_width(
            limits,
            self.child.as_widget().size(),
            self.close_reserve,
            MIN_TITLE_CAP,
            self.ceiling_cap,
            |limits| {
                self.child
                    .as_widget_mut()
                    .measure(&mut tree.children[0], renderer, limits)
            },
        )
        .apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("reserved width child");
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
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("reserved width child");
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
    }

    fn update_frame(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        crate::frame_routing::forward_frame_opaque(
            &mut self.child,
            tree,
            0,
            event,
            layout
                .children(&tree.children)
                .next()
                .expect("reserved width child"),
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout
                .children(&tree.children)
                .next()
                .expect("reserved width child"),
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout
                .children(&tree.children)
                .next()
                .expect("reserved width child"),
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        renderer: &iced::Renderer,
        viewport: &iced::Rectangle,
        translation: iced::Vector,
        window: iced::Size,
    ) -> Vec<iced::advanced::overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("reserved width child");
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

impl<'a, Message: Clone + 'a> From<TitleReserveCap<'a, Message>> for Element<'a, Message> {
    fn from(w: TitleReserveCap<'a, Message>) -> Self {
        Element::new(w)
    }
}

pub(crate) struct TitleWidthPin<'a, Message> {
    pub(crate) child: Element<'a, Message>,
    pub(crate) title: String,
    pub(crate) title_size: f32,
    pub(crate) title_font: iced::Font,
    pub(crate) title_cap: f32,
    pub(crate) pinned_width: f32,
}

impl<Message: Clone> iced::advanced::Widget<Message, iced::Theme, iced::Renderer>
    for TitleWidthPin<'_, Message>
{
    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn frame_interest(&self, tree: &iced::advanced::widget::Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
    }

    fn size(&self) -> iced::Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        use iced::advanced::graphics::text::Paragraph as GraphicsParagraph;
        use iced::advanced::text::Paragraph as _;
        let child_node =
            self.child
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, limits);
        let child_size = child_node.size();
        // The composed child is already capped, so measure raw title advance separately.
        let intrinsic_title_w = GraphicsParagraph::with_text(iced::advanced::text::Text {
            content: self.title.as_str(),
            bounds: iced::Size::new(f32::INFINITY, f32::INFINITY),
            size: iced::Pixels(self.title_size),
            line_height: iced::widget::text::LineHeight::default(),
            font: self.title_font,
            align_x: iced::advanced::text::Alignment::Left,
            align_y: iced::alignment::Vertical::Top,
            shaping: iced::widget::text::Shaping::Auto,
            wrapping: iced::widget::text::Wrapping::None,
            ellipsis: iced::widget::text::Ellipsis::None,
            hint_factor: None,
        })
        .min_bounds()
        .width;
        let width = if intrinsic_title_w <= self.title_cap {
            child_size.width
        } else {
            self.pinned_width
                .min(limits.max().width)
                .max(child_size.width)
        };
        iced::advanced::layout::Node::with_children(
            iced::Size::new(width, child_size.height),
            vec![child_node],
        )
        .apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        let inner = layout
            .children(&tree.children)
            .next()
            .expect("TitleWidthPin always wraps its child in a single sub-layout");
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            inner,
            viewport,
            renderer,
            operation,
        );
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        let inner = layout
            .children(&tree.children)
            .next()
            .expect("TitleWidthPin always wraps its child in a single sub-layout");
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            inner,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
    }

    fn update_frame(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        let inner = layout
            .children(&tree.children)
            .next()
            .expect("TitleWidthPin always wraps its child in a single sub-layout");
        crate::frame_routing::forward_frame_opaque(
            &mut self.child,
            tree,
            0,
            event,
            inner,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        let inner = layout
            .children(&tree.children)
            .next()
            .expect("TitleWidthPin always wraps its child in a single sub-layout");
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            inner,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
        let inner = layout
            .children(&tree.children)
            .next()
            .expect("TitleWidthPin always wraps its child in a single sub-layout");
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            inner,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        renderer: &iced::Renderer,
        viewport: &iced::Rectangle,
        translation: iced::Vector,
        window: iced::Size,
    ) -> Vec<iced::advanced::overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        let inner = layout
            .children(&tree.children)
            .next()
            .expect("TitleWidthPin always wraps its child in a single sub-layout");
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            inner,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: Clone + 'a> From<TitleWidthPin<'a, Message>> for Element<'a, Message> {
    fn from(w: TitleWidthPin<'a, Message>) -> Self {
        Element::new(w)
    }
}
#[derive(Default)]
pub(crate) struct TabGestureState {
    middle_pressed_here: bool,
    right_pressed_here: bool,
}

pub(crate) struct TabMouseCapture<'a, Message> {
    pub(crate) child: Element<'a, Message>,
    pub(crate) on_close: Option<Message>,
    pub(crate) on_context: Option<Box<dyn Fn(f32, f32) -> Message + 'a>>,
}

impl<'a, Message: Clone> iced::advanced::Widget<Message, iced::Theme, iced::Renderer>
    for TabMouseCapture<'a, Message>
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<TabGestureState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(TabGestureState::default())
    }

    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn frame_interest(&self, tree: &iced::advanced::widget::Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
    }

    fn size(&self) -> iced::Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
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

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
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
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        use iced::Event as E;
        use iced::advanced::mouse::{self, Button};
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

        let bounds = layout.bounds();
        let over = cursor.position_in(bounds);
        let state = tree.state.downcast_mut::<TabGestureState>();
        match event {
            E::Mouse(mouse::Event::ButtonPressed(Button::Middle)) => {
                state.middle_pressed_here = over.is_some();
            }
            E::Mouse(mouse::Event::ButtonPressed(Button::Right)) => {
                state.right_pressed_here = over.is_some();
            }
            E::Mouse(mouse::Event::ButtonReleased(Button::Middle)) => {
                let armed = std::mem::take(&mut state.middle_pressed_here);
                if armed
                    && over.is_some()
                    && let Some(msg) = self.on_close.clone()
                {
                    shell.publish(msg);
                    shell.capture_event();
                }
            }
            E::Mouse(mouse::Event::ButtonReleased(Button::Right)) => {
                let armed = std::mem::take(&mut state.right_pressed_here);
                if armed
                    && let Some(local) = over
                    && let Some(cb) = self.on_context.as_ref()
                {
                    shell.publish(cb(local.x, local.y));
                    shell.capture_event();
                }
            }
            E::Mouse(mouse::Event::CursorLeft) => {
                state.middle_pressed_here = false;
                state.right_pressed_here = false;
            }
            E::Touch(iced::touch::Event::FingerLost { .. }) => {
                state.middle_pressed_here = false;
                state.right_pressed_here = false;
            }
            _ => {}
        }
    }

    fn update_frame(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
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

    fn mouse_interaction(
        &self,
        tree: &iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
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
        tree: &'b mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        renderer: &iced::Renderer,
        viewport: &iced::Rectangle,
        translation: iced::Vector,
        window: iced::Size,
    ) -> Vec<iced::advanced::overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
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

impl<'a, Message: Clone + 'a> From<TabMouseCapture<'a, Message>> for Element<'a, Message> {
    fn from(w: TabMouseCapture<'a, Message>) -> Self {
        Element::new(w)
    }
}

pub(crate) struct TabHeightFill<'a, Message> {
    pub(crate) child: Element<'a, Message>,
    pub(crate) floor: f32,
}

impl<Message: Clone> iced::advanced::Widget<Message, iced::Theme, iced::Renderer>
    for TabHeightFill<'_, Message>
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    fn frame_interest(&self, tree: &iced::advanced::widget::Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.child, tree)
    }

    fn size(&self) -> iced::Size<Length> {
        iced::Size {
            width: self.child.as_widget().size().width,
            // Static cross-axis layout preserves the parent offer instead of sibling-derived height.
            height: Length::Shrink,
        }
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        let max = limits.max();
        let min = limits.min();
        // Containers loosen minimum height; finite maximum still carries the slot height.
        let target = if max.height.is_finite() {
            max.height
        } else {
            self.floor
        };
        let child_limits = iced::advanced::layout::Limits::with_compression(
            iced::Size::new(min.width, target),
            iced::Size::new(max.width, target),
            limits.compression(),
        );
        self.child
            .as_widget_mut()
            .layout(tree, renderer, &child_limits);
    }

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }

    fn update_frame(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
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

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
    ) {
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        renderer: &iced::Renderer,
        viewport: &iced::Rectangle,
        translation: iced::Vector,
        window: iced::Size,
    ) -> Vec<iced::advanced::overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message: Clone + 'a> From<TabHeightFill<'a, Message>> for Element<'a, Message> {
    fn from(w: TabHeightFill<'a, Message>) -> Self {
        Element::new(w)
    }
}

impl<'a, Message: Clone + 'a> From<EnzDocumentTab<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzDocumentTab<'a, Message>) -> Self {
        w.into_element()
    }
}
