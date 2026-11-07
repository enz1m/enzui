//! The `Widget` implementations of the action bar and of the wrapper that
//! dismisses its overflow when a child activates.

use super::*;

impl<'a, Message: Clone + 'a> Widget<Message, iced::Theme, iced::Renderer>
    for ActionBarWidget<'a, Message>
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<ActionBarState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(ActionBarState {
            signature: self.content_signature(),
            suppress_stale_open: false,
            instance: NEXT_ACTION_BAR_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        })
    }

    fn diff(&mut self, _tree: &mut Tree) {
        // Reconcile during layout; diffing placeholder content would destroy retained child focus.
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width(),
            height: Length::Shrink,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let width = self.width();
        let mut limits = limits.width(Length::Fill).height(Length::Shrink);
        let available = limits.max().width;

        let signature = self.content_signature();
        let state = tree.state.downcast_mut::<ActionBarState>();
        if state.signature != signature {
            state.signature = signature;
            // Require a closed acknowledgement before reopening changed descriptors.
            state.suppress_stale_open = self.overflow_open;
        }
        if !self.overflow_open {
            state.suppress_stale_open = false;
        }
        if state.instance == 0 {
            state.instance =
                NEXT_ACTION_BAR_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        let instance = state.instance;
        let effective_open = self.overflow_open && !state.suppress_stale_open;

        let plan = self.plan_measured(renderer, available);
        if let Some(content) = self.build_owned(available, effective_open, instance, &plan) {
            self.content = content;
            if width == Length::Shrink {
                limits = limits.width(Length::Fixed(Self::measure(&mut self.content, renderer)));
            }
            tree.diff_children(std::slice::from_mut(&mut self.content));
            return layout::passthrough(&limits, Size::new(width, Length::Shrink), |limits| {
                self.content
                    .as_widget_mut()
                    .measure(&mut tree.children[0], renderer, limits)
            })
            .apply(tree);
        }
        let stable_expanded = !effective_open
            && !matches!(&self.collector, Collector::LabelledUntilCompact { .. })
            && plan
                .overflow
                .iter()
                .all(|item| item.policy == OverflowPolicy::AlwaysOverflow)
            && plan.inline.len()
                == self
                    .actions
                    .iter()
                    .filter(|item| {
                        item.descriptor.visible
                            && item.descriptor.policy != OverflowPolicy::AlwaysOverflow
                    })
                    .count()
            && plan
                .inline
                .iter()
                .all(|item| item.presentation == Presentation::Expanded);
        self.content = self.build(available, effective_open, instance, &plan);
        if width == Length::Shrink {
            limits = limits.width(Length::Fixed(Self::measure(&mut self.content, renderer)));
        }
        let minimum = stable_expanded
            .then(|| Self::measure(&mut self.content, renderer))
            .filter(|width| width.is_finite() && *width > 0.0);
        tree.diff_children(std::slice::from_mut(&mut self.content));
        if let Some(minimum) = minimum.filter(|width| *width <= available) {
            return layout::aligned(
                &limits,
                width,
                Length::Shrink,
                iced::Padding::ZERO,
                [Alignment::from(self.align_x), Alignment::Start],
                |limits| {
                    self.content
                        .as_widget_mut()
                        .measure(&mut tree.children[0], renderer, limits)
                },
            )
            .with_reflow_min_width(minimum)
            .expect("aligned finite action allocation")
            .apply(tree);
        }
        let mut node =
            self.content
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, &limits.loose());
        let size = limits.resolve(width, Length::Shrink, node.size());
        node.align_mut(Alignment::from(self.align_x), Alignment::Start, size);
        layout::Node::with_children(size, vec![node]).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
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
        renderer: &iced::Renderer,
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

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
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

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
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

impl<'a, Message: Clone + 'a> Widget<Message, iced::Theme, iced::Renderer>
    for DismissOnChildActivation<'a, Message>
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

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
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

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let mut watch = ArmWatch {
            inner: operation,
            armed: &mut self.routed,
        };
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, &mut watch);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        self.child.as_widget().frame_interest(tree)
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
        // Merge all child effects before appending the close message.
        let mut local_msgs: iced::advanced::shell::Bus<Message> = iced::advanced::shell::Bus::new();
        let mut local_shell = shell.local(&mut local_msgs);
        self.child.as_widget_mut().update(
            tree,
            event,
            layout,
            cursor,
            renderer,
            &mut local_shell,
            viewport,
        );
        let published = !local_shell.is_empty();
        shell.merge(local_shell, |m| m);
        if published && (is_activation_event(event) || std::mem::take(&mut self.routed)) {
            shell.publish(self.on_dismiss.clone());
        }
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
