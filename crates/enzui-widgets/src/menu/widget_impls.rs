//! The `Widget` implementations behind a menu: the roving-focus list, the
//! hover reporter, the row highlight and the active-row reveal.

use super::*;

impl<'a, Message> iced::advanced::Widget<Message, iced::Theme, iced::Renderer>
    for MenuRovingWidget<'a, Message>
where
    Message: 'a + Clone,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<MenuRovingState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        // The host must relinquish trigger focus separately; this subtree cannot unfocus the trigger.
        let focus_on_mount = matches!(
            &self.mode,
            RovingMode::SelfCursor(options) if options.focus_on_mount
        );
        let mut state = MenuRovingState::default();
        if focus_on_mount {
            use iced::advanced::widget::operation::Focusable;
            state.focus();
        }
        iced::advanced::widget::tree::State::new(state)
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
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        let state = tree.state.downcast_mut::<MenuRovingState>();
        operation.focusable(None, layout.bounds(), state);
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                viewport,
                renderer,
                operation,
            );
        });
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
        enzui_core::focus::note_input_event_for(
            event,
            enzui_core::focus::window_key(shell.window()),
        );
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );

        let bounds = layout.bounds();
        if let Some(decision) = enzui_core::focus::press_focus_from_cursor(event, cursor, bounds) {
            let state = tree.state.downcast_mut::<MenuRovingState>();
            let was = state.focus.is_focused();
            match decision {
                enzui_core::focus::PressFocus::Take => state.focus.focus(),
                enzui_core::focus::PressFocus::Drop => {
                    state.focus.unfocus();
                    state.space_armed = None;
                }
                enzui_core::focus::PressFocus::Leave => {}
            }
            if state.focus.is_focused() != was {
                shell.request_redraw();
            }
        }
        if let RovingMode::SelfCursor(options) = &self.mode
            && let Some(id) = &options.focus_return_to
        {
            let state = tree.state.downcast_ref::<MenuRovingState>();
            if state.focus.is_focused() {
                let window = enzui_core::focus::window_key(shell.window());
                crate::focus_return::arm(window, id.clone());
            }
        }

        if matches!(event, iced::Event::Window(iced::window::Event::Unfocused)) {
            tree.state.downcast_mut::<MenuRovingState>().space_armed = None;
        }

        if let iced::Event::Keyboard(kb) = event {
            let self_roving = matches!(self.mode, RovingMode::SelfCursor(_));
            let current_cursor: Option<EntryId> = if self_roving {
                let state = tree.state.downcast_ref::<MenuRovingState>();
                state.cursor.clone().or_else(|| self.active.clone())
            } else {
                self.active.clone()
            };
            {
                let state = tree.state.downcast_ref::<MenuRovingState>();
                if !state.focus.is_focused() {
                    return;
                }
            }
            use iced::keyboard::key::Named;
            match kb {
                iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(Named::Space),
                    ..
                } => {
                    if enzui_core::focus::activation_from_key(kb) {
                        let armed = current_cursor.as_ref().filter(|key| {
                            self.model
                                .entry(key)
                                .is_some_and(MenuEntryModel::is_focusable)
                        });
                        tree.state.downcast_mut::<MenuRovingState>().space_armed = armed.cloned();
                        if armed.is_some() {
                            shell.capture_event();
                            shell.request_redraw();
                        }
                    }
                    return;
                }
                iced::keyboard::Event::KeyReleased {
                    key: iced::keyboard::Key::Named(Named::Space),
                    ..
                } => {
                    let armed = tree
                        .state
                        .downcast_mut::<MenuRovingState>()
                        .space_armed
                        .take();
                    if let Some(key) = armed {
                        self.activate_key(Some(&key), shell);
                        shell.capture_event();
                        shell.request_redraw();
                    }
                    return;
                }
                iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(Named::Enter),
                    ..
                } if !enzui_core::focus::activation_from_key(kb) => {
                    return;
                }
                _ => {}
            }
            let Some(menu_key) = enzui_core::menu_model::key_for_open_menu(kb) else {
                return;
            };
            use enzui_core::menu_model::MenuKey;
            match menu_key {
                MenuKey::Move(movement) => {
                    let mut model = self.model.clone();
                    if let Some(active) = &current_cursor {
                        model.set_active(active);
                    }
                    if let Some(new_key) = model.navigate(movement) {
                        if Some(&new_key) != current_cursor.as_ref() {
                            self.reveal_row(tree, layout, renderer, &new_key);
                            match &self.mode {
                                RovingMode::Publish(on_navigate) => {
                                    shell.publish((on_navigate)(new_key));
                                }
                                RovingMode::SelfCursor(_) => {
                                    tree.state.downcast_mut::<MenuRovingState>().cursor =
                                        Some(new_key);
                                }
                            }
                        }
                        shell.request_redraw();
                        shell.capture_event();
                    } else if self_roving {
                        shell.capture_event();
                    }
                }
                MenuKey::Activate => {
                    self.activate_key(current_cursor.as_ref(), shell);
                    shell.request_redraw();
                    shell.capture_event();
                }
                MenuKey::Dismiss | MenuKey::OpenSubmenu | MenuKey::CloseSubmenu => {}
            }
        }
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
        let mut highlighted = None;
        if let RovingMode::SelfCursor(_) = &self.mode {
            let state = tree.state.downcast_ref::<MenuRovingState>();
            let effective = state.cursor.as_ref().or(self.active.as_ref());
            if state.focus.is_focused()
                && let Some(key) = effective
                && self
                    .model
                    .entry(key)
                    .is_some_and(MenuEntryModel::is_focusable)
            {
                highlighted = self.rows.row_of(key);
            }
        }
        self.rows.highlight.set(highlighted);

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

impl<Message> iced::advanced::Widget<Message, iced::Theme, iced::Renderer>
    for HoverReporter<'_, Message>
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<HoverReporterState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(HoverReporterState::default())
    }

    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn is_void(&self) -> bool {
        self.child.as_widget().is_void()
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
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        // Unavailable cursor passes preserve the latch instead of treating redraws as pointer leave.
        let Some(position) = cursor.position() else {
            return;
        };
        let inside = layout.bounds().contains(position);
        let state = tree.state.downcast_mut::<HoverReporterState>();
        if inside == state.inside {
            return;
        }
        state.inside = inside;
        if inside {
            shell.publish((self.on_hover)(self.key.clone()));
            shell.request_redraw();
        }
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

impl<Message> iced::advanced::Widget<Message, iced::Theme, iced::Renderer>
    for RowHighlight<'_, Message>
{
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
        let node = self
            .child
            .as_widget_mut()
            .measure(&mut tree.children[0], renderer, limits);
        let size = node.size();
        iced::advanced::layout::Node::with_children(size, vec![node]).apply(tree);
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
        let Some(column) = layout.children(&tree.children).next() else {
            return;
        };
        let column_tree = &tree.children[0];
        if let Some(row) = self
            .row
            .get()
            .and_then(|row| column.children(&column_tree.children).nth(row))
        {
            let tokens = self.tokens.unwrap_or_else(|| {
                crate::theme_lookup::resolve(theme, crate::menu_row::MenuRowTokens::from_palette)
            });
            use iced::advanced::Renderer as _;
            renderer.fill_quad(
                iced::advanced::renderer::Quad {
                    bounds: crate::focus_button::inset_background_bounds(
                        row.bounds(),
                        tokens.background_inset_y,
                    ),
                    border: iced::Border {
                        radius: tokens.row_radius.into(),
                        ..iced::Border::default()
                    },
                    shadow: iced::Shadow::default(),
                    snap: true,
                },
                iced::Background::Color(tokens.hover_background),
            );
        }
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            column,
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
        layout
            .children(&tree.children)
            .next()
            .map_or_else(Default::default, |column| {
                self.child.as_widget().mouse_interaction(
                    &tree.children[0],
                    column,
                    cursor,
                    viewport,
                    renderer,
                )
            })
    }

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        viewport: &iced::Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        if let Some(column) = layout.children(&tree.children).next() {
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                column,
                viewport,
                renderer,
                operation,
            );
        }
    }

    fn frame_interest(&self, tree: &iced::advanced::widget::Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
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
        if let Some(column) = layout.children(&tree.children).next() {
            self.child.as_widget_mut().update_frame(
                &mut tree.children[0],
                event,
                column,
                cursor,
                renderer,
                shell,
                viewport,
            );
        }
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
        if let Some(column) = layout.children(&tree.children).next() {
            self.child.as_widget_mut().update(
                &mut tree.children[0],
                event,
                column,
                cursor,
                renderer,
                shell,
                viewport,
            );
        }
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
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
        let Some(column) = layout.children(&tree.children).next() else {
            return Vec::new();
        };
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            column,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<Message> iced::advanced::Widget<Message, iced::Theme, iced::Renderer>
    for ActiveReveal<'_, Message>
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<ActiveRevealState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(ActiveRevealState::default())
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

    fn frame_interest(&self, tree: &iced::advanced::widget::Tree) -> bool {
        tree.children
            .first()
            .is_none_or(|child| self.child.as_widget().frame_interest(child))
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
        self.child.as_widget_mut().update_frame(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
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
        let revealed = tree.state.downcast_ref::<ActiveRevealState>().revealed;
        if self.active_row != revealed {
            if let Some(mut reveal) = self
                .active_row
                .and_then(|row| RevealRow::for_row(&self.scroll_id, layout, &tree.children[0], row))
            {
                self.child.as_widget_mut().operate(
                    &mut tree.children[0],
                    layout,
                    &layout.bounds(),
                    renderer,
                    &mut reveal,
                );
                shell.request_redraw();
            }
            tree.state.downcast_mut::<ActiveRevealState>().revealed = self.active_row;
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
