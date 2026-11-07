//! Page translation, focus semantics and child-first Escape handling.

use super::*;
use iced::advanced::layout::Measure as _;
pub(crate) struct Slide<'a, Message> {
    pub(crate) child: Element<'a, Message>,
    pub(crate) offset: f32,
    pub(crate) route_key: Option<String>,
    pub(crate) id: Option<iced::advanced::widget::Id>,
    pub(crate) accessible_name: String,
}
#[derive(Default)]
pub(crate) struct SlideState {
    pub(crate) theme_reduced_motion: Cell<Option<bool>>,
    previous_key: std::cell::RefCell<Option<String>>,
    focus: enzui_core::focus::FocusState,
}

impl iced::advanced::widget::operation::Focusable for SlideState {
    fn is_focused(&self) -> bool {
        self.focus.is_focused()
    }
    fn focus(&mut self) {
        self.focus.focus();
    }
    fn unfocus(&mut self) {
        self.focus.unfocus();
    }
}

impl<Message> Slide<'_, Message> {
    pub(crate) fn effective_offset(&self, state: &SlideState) -> f32 {
        // Before draw, preserve the host-resolved offset; the theme can suppress later layouts.
        if state.theme_reduced_motion.get().unwrap_or(false) {
            0.0
        } else {
            self.offset
        }
    }
}
pub(crate) fn child_layout(layout: Layout, tree: &Tree) -> Layout {
    layout
        .children(&tree.children)
        .next()
        .expect("a Slide node always has exactly one child")
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for Slide<'_, Message> {
    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn is_void(&self) -> bool {
        self.child.as_widget().is_void()
    }
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<SlideState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(SlideState::default())
    }
    fn diff(&mut self, tree: &mut Tree) {
        let Some(new_key) = self.route_key.clone() else {
            tree.diff_children(std::slice::from_mut(&mut self.child));
            return;
        };
        let state = tree.state.downcast_ref::<SlideState>();
        let previous_key = state.previous_key.borrow().clone();
        let previous_keys: Vec<String> = previous_key.into_iter().collect();
        let new_keys = vec![new_key.clone()];

        let mut new_children = [&mut self.child];
        crate::keyed_diff::reconcile_children_by_key(
            &mut tree.children,
            &previous_keys,
            &new_keys,
            &mut new_children,
            |child_tree, child| {
                child_tree.diff(child.as_widget_mut());
            },
            |child| child.as_widget().tag(),
            |child| Tree::new(child.as_widget()),
        );

        *state.previous_key.borrow_mut() = Some(new_key);
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let state = tree.state.downcast_ref::<SlideState>();
        let child = self
            .child
            .as_widget_mut()
            .measure(&mut tree.children[0], renderer, limits);
        let size = limits.max();
        // Keep translation below the parent node that containers reposition.
        layout::Node::with_children(
            size,
            vec![child.translate(Vector::new(self.effective_offset(state), 0.0))],
        )
        .apply(tree);
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
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let child_layout = child_layout(layout, tree);
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            child_layout,
            cursor,
            viewport,
            renderer,
        )
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
        let state = tree.state.downcast_ref::<SlideState>();
        // Layout has no theme; this policy takes effect on the next layout pass.
        state
            .theme_reduced_motion
            .set(Some(crate::theme_lookup::reduced_motion(theme)));
        let child_layout = child_layout(layout, tree);
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            child_layout,
            cursor,
            viewport,
        );
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        if let Some(id) = self.id.as_ref() {
            let state = tree.state.downcast_mut::<SlideState>();
            operation.focusable(Some(id), layout.bounds(), state);
            let focused = {
                let state = tree.state.downcast_ref::<SlideState>();
                state.focus.is_focused()
            };
            let mut node = enzui_core::collect::SemanticsNode::new(
                enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Group)
                    .label(self.accessible_name.clone())
                    .focused(focused),
            );
            operation.custom(Some(id), layout.bounds(), &mut node);
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

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
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

impl<'a, Message: 'a> From<Slide<'a, Message>> for Element<'a, Message> {
    fn from(widget: Slide<'a, Message>) -> Self {
        Self::new(widget)
    }
}
pub(crate) struct BackKeyCapture<'a, Message> {
    pub(crate) child: Element<'a, Message>,
    pub(crate) on_pop: Option<Message>,
    pub(crate) pop_on_escape: bool,
    pub(crate) back_enabled: bool,
}

impl<Message: Clone> Widget<Message, iced::Theme, iced::Renderer> for BackKeyCapture<'_, Message> {
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

    fn is_void(&self) -> bool {
        self.child.as_widget().is_void()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
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
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);

        if shell.is_event_captured() {
            return;
        }

        let Event::Keyboard(iced::keyboard::Event::KeyPressed { key, repeat, .. }) = event else {
            return;
        };
        if event_for_key_repeat(key, self.pop_on_escape, self.back_enabled, *repeat).is_some()
            && let Some(message) = self.on_pop.clone()
        {
            shell.publish(message);
            shell.capture_event();
        }
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
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
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

impl<'a, Message: Clone + 'a> From<BackKeyCapture<'a, Message>> for Element<'a, Message> {
    fn from(widget: BackKeyCapture<'a, Message>) -> Self {
        Self::new(widget)
    }
}
