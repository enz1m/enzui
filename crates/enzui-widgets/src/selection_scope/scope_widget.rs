use super::*;

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for SelectionScopeWidget<'_, Message>
where
    Message: Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ScopeState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(ScopeState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
        let state = tree.state.downcast_mut::<ScopeState>();
        if self.clear_generation != state.clear_generation
            || self.document_key != state.document_key
        {
            state
                .copy_lifetime
                .cancel(enzui_core::cancellation::CancelCause::Cancelled);
            state.copy_lifetime = enzui_core::cancellation::CancelToken::new();
            state.copy_range = None;
            state.model.clear();
            state.logical = None;
            state.projection = None;
            state.published_range = None;
            state.clear_generation = self.clear_generation;
            state.document_key = self.document_key;
            state.context_menu_pending = false;
            state.keyboard_caret = None;
        }
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let _depth = (!self.yielding()).then(ScopeDepth::enter);
        let child_layout =
            self.child
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, limits);
        let size = child_layout.size();
        // A resize may go straight from layout to draw, so refresh live selection geometry here.
        if tree
            .state
            .downcast_ref::<ScopeState>()
            .model
            .has_selection()
        {
            refresh_text_regions(
                &mut self.child,
                &mut tree.children[0],
                Layout::of(&child_layout),
                renderer,
                tree.state.downcast_mut::<ScopeState>(),
                None,
                false,
                scope_document(self.document.as_ref()),
            );
        }

        layout::Node::with_children(size, vec![child_layout]).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let child_layout = layout.children(&tree.children).next().unwrap_or(layout);
        if self.yielding() {
            operation.traverse(&mut |operation| {
                self.child.as_widget_mut().operate(
                    &mut tree.children[0],
                    child_layout,
                    viewport,
                    renderer,
                    operation,
                );
            });
            return;
        }
        let _depth = ScopeDepth::enter();
        refresh_text_regions(
            &mut self.child,
            &mut tree.children[0],
            child_layout,
            renderer,
            tree.state.downcast_mut::<ScopeState>(),
            None,
            false,
            scope_document(self.document.as_ref()),
        );

        {
            let state = tree.state.downcast_mut::<ScopeState>();
            state
                .focus
                .operate(operation, self.id.as_ref(), layout.bounds());
            let mut work = state.work;
            operation.custom(self.id.as_ref(), layout.bounds(), &mut work);
            let merged_context = self.id.is_some()
                && self.context_id.as_ref() == self.id.as_ref()
                && self.handles_context()
                && state.model.has_selection();
            let document_label = self
                .label
                .clone()
                .or_else(|| merged_context.then(|| self.context_label.clone()).flatten())
                .unwrap_or_else(|| {
                    enzui_core::localization::text("enzui-selection-scope-selectable-document")
                });
            let semantics =
                enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Document)
                    .label(document_label)
                    .read_only(true)
                    .focused(state.focus.is_focused());
            let semantics = if merged_context {
                semantics.context_menu()
            } else {
                semantics
            };
            let mut node = enzui_core::collect::SemanticsNode::new(semantics);
            operation.custom(self.id.as_ref(), layout.bounds(), &mut node);
        }

        {
            let state = tree.state.downcast_ref::<ScopeState>();
            crate::text_drag::offer_read_only_selection(
                operation,
                self.id.as_ref(),
                layout.bounds(),
                |point| {
                    selection_contains_point(
                        &state.model,
                        &state.regions,
                        point - (child_layout.position() - Point::ORIGIN),
                    )
                    .then(|| {
                        (
                            selected_scope_text(
                                state,
                                scope_document(self.document.as_ref()),
                                renderer,
                            ),
                            state.copy_lifetime.share(),
                        )
                    })
                },
            );
        }

        if self.handles_context()
            && let Some(id) = &self.context_id
            && let Some(selection) = build_scope_selection(
                tree.state.downcast_ref::<ScopeState>(),
                scope_document(self.document.as_ref()),
                renderer,
                child_layout.position() - Point::ORIGIN,
            )
        {
            if Some(id) != self.id.as_ref() {
                let semantics =
                    enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Group)
                        .label_opt(self.context_label.clone())
                        .context_menu();
                let mut node = enzui_core::collect::SemanticsNode::new(semantics);
                operation.custom(Some(id), selection.bounds, &mut node);
            }

            let mut request = crate::context_menu_action::ContextMenuRequest::new(id.clone());
            operation.custom(Some(id), selection.bounds, &mut request);
            if request.armed() {
                tree.state.downcast_mut::<ScopeState>().context_menu_pending = true;
            }
        }

        operation.traverse(&mut |operation| {
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                child_layout,
                viewport,
                renderer,
                operation,
            );
        });
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
        let child_layout = layout.children(&tree.children).next().unwrap_or(layout);
        let child_offset = child_layout.position() - Point::ORIGIN;
        let yielding = self.yielding();
        let _depth = (!yielding).then(ScopeDepth::enter);
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
        if yielding {
            return;
        }

        if matches!(event, Event::Window(iced::window::Event::Unfocused)) {
            let state = tree.state.downcast_mut::<ScopeState>();
            state.focus.unfocus();
            // The release may occur in another window; blur retires the drag but keeps its range.
            if state.model.is_dragging() {
                let _outcome = state.model.release();
                shell.request_redraw();
            }
        } else if tree.state.downcast_ref::<ScopeState>().focus.is_focused() {
            let mut probe = FocusOwner::default();
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                child_layout,
                &child_layout.bounds(),
                renderer,
                &mut probe,
            );
            if probe.present
                || (shell.is_event_captured()
                    && matches!(
                        event,
                        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                    ))
            {
                tree.state.downcast_mut::<ScopeState>().focus.unfocus();
            }
        }

        let context_menu_pending = {
            let state = tree.state.downcast_mut::<ScopeState>();
            std::mem::take(&mut state.context_menu_pending)
        };
        if context_menu_pending && self.handles_context() {
            let origin = self.focus_origin(tree, child_layout, renderer);
            let state = tree.state.downcast_ref::<ScopeState>();
            if let Some(selection) = build_scope_selection(
                state,
                scope_document(self.document.as_ref()),
                renderer,
                child_offset + state.translation,
            ) {
                let invocation = ContextInvocationKind::Keyboard(context_rect(selection.bounds));
                self.publish_context(shell, selection, invocation, origin);
                shell.request_redraw();
            }
        }

        if shell.is_event_captured() {
            return;
        }
        // Runtime event dispatch does not automatically perform widget operations.
        let collect_keyboard_carets = tree.state.downcast_ref::<ScopeState>().focus.is_focused()
            && matches!(event, Event::Keyboard(key) if keyboard_selection_move(key).is_some());
        refresh_text_regions(
            &mut self.child,
            &mut tree.children[0],
            child_layout,
            renderer,
            tree.state.downcast_mut::<ScopeState>(),
            cursor.position(),
            collect_keyboard_carets,
            scope_document(self.document.as_ref()),
        );
        let publish_origin = if publishes_context(event) {
            self.focus_origin(tree, child_layout, renderer)
        } else {
            None
        };

        let mut release_child_focus = false;
        {
            let state = tree.state.downcast_mut::<ScopeState>();
            if let Some(pos) = cursor.position() {
                let local = pos - child_offset;
                state.over_text = state.regions.iter().any(|r| r.contains(local));
            } else {
                state.over_text = false;
            }

            match event {
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                    if let Some(pos) = cursor.position()
                        && let Some(doc_pos) =
                            point_in_doc_position(&state.regions, pos - child_offset)
                    {
                        state.focus.pointer_focus();
                        state.keyboard_caret = None;
                        release_child_focus = true;
                        let click = state.clicks.press(Instant::now(), pos);
                        if click.count == ClickCount::Double {
                            state.model.select_word_at(doc_pos);
                        } else {
                            state.model.press(doc_pos);
                        }
                        shell.capture_event();
                        shell.request_redraw();
                    } else {
                        state.focus.unfocus();
                    }
                }
                Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                    if state.model.is_dragging()
                        && let Some(pos) = cursor.position()
                        && let Some(doc_pos) =
                            point_to_doc_position(&state.regions, pos - child_offset)
                    {
                        state.model.drag_to(doc_pos);
                        shell.request_redraw();
                    }
                }
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    let handles_event = state.model.is_dragging();
                    if handles_event {
                        let _outcome = state.model.release();
                        shell.request_redraw();
                    }
                }
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                    if self.handles_context()
                        && let Some(position) = cursor.position()
                        && selection_contains_point(
                            &state.model,
                            &state.regions,
                            position - child_offset,
                        )
                        && let Some(selection) = build_scope_selection(
                            state,
                            scope_document(self.document.as_ref()),
                            renderer,
                            child_offset + state.translation,
                        )
                    {
                        let point = position + state.translation;
                        self.publish_context(
                            shell,
                            selection,
                            ContextInvocationKind::Pointer(ContextPoint::new(point.x, point.y)),
                            publish_origin,
                        );
                        shell.capture_event();
                        shell.request_redraw();
                    }
                }
                Event::Keyboard(
                    event @ keyboard::Event::KeyPressed {
                        key,
                        modifiers,
                        physical_key,
                        ..
                    },
                ) if state.focus.is_focused() => {
                    if enzui_core::focus::signals_keyboard_modality(event) {
                        state.focus.ring_visible = true;
                    }
                    if self.handles_context()
                        && state.model.has_selection()
                        && enzui_core::context_menu::opens_context_menu(event)
                        && let Some(selection) = build_scope_selection(
                            state,
                            scope_document(self.document.as_ref()),
                            renderer,
                            child_offset + state.translation,
                        )
                    {
                        let invocation =
                            ContextInvocationKind::Keyboard(context_rect(selection.bounds));
                        self.publish_context(shell, selection, invocation, publish_origin);
                        shell.capture_event();
                        shell.request_redraw();
                        return;
                    }
                    if let Some(movement) = keyboard_selection_move(event)
                        && move_keyboard_selection(state, movement)
                    {
                        shell.capture_event();
                        shell.request_redraw();
                    }
                    let ctrl = modifiers.command();
                    // On a non-Latin layout, Ctrl+C and Ctrl+A are the keys at
                    // the C and A positions.
                    let positioned = enzui_core::shortcut::latin_fallback(key, *physical_key)
                        .map(|latin| keyboard::Key::Character(latin.to_string().into()));
                    match positioned.as_ref().unwrap_or(key) {
                        keyboard::Key::Character(c)
                            if ctrl
                                && c.as_str() == "c"
                                && (state.model.has_selection() || state.logical.is_some()) =>
                        {
                            let text = selected_scope_text(
                                state,
                                scope_document(self.document.as_ref()),
                                renderer,
                            );
                            match &self.on_copy {
                                Some(on_copy) => shell.publish(on_copy(text)),
                                None => shell.write_clipboard(
                                    iced::advanced::graphics::core::clipboard::Content::Text(text),
                                ),
                            }
                            shell.capture_event();
                        }
                        keyboard::Key::Character(c) if ctrl && c.as_str() == "a" => {
                            state.model.select_all();
                            state.keyboard_caret = None;
                            shell.capture_event();
                            shell.request_redraw();
                        }
                        keyboard::Key::Named(keyboard::key::Named::Escape)
                            if state.model.has_selection() =>
                        {
                            state.model.clear();
                            shell.capture_event();
                            shell.request_redraw();
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        if release_child_focus {
            let mut unfocus = widget::operation::focusable::unfocus();
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                child_layout,
                &child_layout.bounds(),
                renderer,
                &mut unfocus,
            );
        }
        refresh_text_regions(
            &mut self.child,
            &mut tree.children[0],
            child_layout,
            renderer,
            tree.state.downcast_mut::<ScopeState>(),
            cursor.position(),
            false,
            scope_document(self.document.as_ref()),
        );
        let state = tree.state.downcast_mut::<ScopeState>();
        let copy_range = state.model.selection().map(|selection| selection.range());
        if copy_range != state.copy_range {
            state
                .copy_lifetime
                .cancel(enzui_core::cancellation::CancelCause::Cancelled);
            state.copy_lifetime = enzui_core::cancellation::CancelToken::new();
            state.copy_range = copy_range;
        }
        // Endpoint changes publish; geometry-only changes keep the same notification range.
        if let Some(on_sel) = &self.on_selection_change {
            let selection = build_scope_selection(
                state,
                scope_document(self.document.as_ref()),
                renderer,
                child_offset,
            );
            let range = selection.as_ref().map(|s| (s.start, s.end));
            if range != state.published_range {
                shell.publish(on_sel(selection));
                state.published_range = range;
            }
        }
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        let state = tree.state.downcast_ref::<ScopeState>();
        (self.handles_context() && self.context_id.is_some() && state.model.has_selection())
            || state.context_menu_pending
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
        let child_layout = layout.children(&tree.children).next().unwrap_or(layout);
        let yielding = self.yielding();
        let _depth = (!yielding).then(ScopeDepth::enter);
        crate::frame_routing::forward_frame_opaque(
            &mut self.child,
            tree,
            0,
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        if yielding {
            return;
        }
        let child_offset = child_layout.position() - Point::ORIGIN;
        let context_menu_pending = {
            let state = tree.state.downcast_mut::<ScopeState>();
            std::mem::take(&mut state.context_menu_pending)
        };
        if context_menu_pending && self.handles_context() {
            let origin = self.focus_origin(tree, child_layout, renderer);
            let state = tree.state.downcast_ref::<ScopeState>();
            if let Some(selection) = build_scope_selection(
                state,
                scope_document(self.document.as_ref()),
                renderer,
                child_offset + state.translation,
            ) {
                let invocation = ContextInvocationKind::Keyboard(context_rect(selection.bounds));
                self.publish_context(shell, selection, invocation, origin);
                shell.request_redraw();
            }
        }
        if shell.is_event_captured() {
            return;
        }
        refresh_text_regions(
            &mut self.child,
            &mut tree.children[0],
            child_layout,
            renderer,
            tree.state.downcast_mut::<ScopeState>(),
            cursor.position(),
            false,
            scope_document(self.document.as_ref()),
        );
        let state = tree.state.downcast_mut::<ScopeState>();
        state.over_text = cursor.position().is_some_and(|position| {
            state
                .regions
                .iter()
                .any(|region| region.contains(position - child_offset))
        });
        if let Some(on_selection_change) = &self.on_selection_change {
            let selection = build_scope_selection(
                state,
                scope_document(self.document.as_ref()),
                renderer,
                child_offset,
            );
            let range = selection
                .as_ref()
                .map(|selection| (selection.start, selection.end));
            if range != state.published_range {
                shell.publish(on_selection_change(selection));
                state.published_range = range;
            }
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
        let child_layout = layout.children(&tree.children).next().unwrap_or(layout);
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            child_layout,
            cursor,
            viewport,
        );
        let state = tree.state.downcast_ref::<ScopeState>();
        let palette = crate::theme_lookup::palette(theme);
        if state.model.has_selection() {
            let fill = selection_fill(&palette);
            let selected = state.model.selected_paragraphs();
            let offset = child_layout.position() - Point::ORIGIN;

            use iced::advanced::graphics::core::Renderer as _;
            for idx in selected {
                if let Some(region) = state.regions.get(idx) {
                    for bounds in region.selection_bounds() {
                        renderer.fill_quad(
                            iced::advanced::renderer::Quad {
                                bounds: bounds + offset,
                                border: iced::Border::default(),
                                shadow: iced::Shadow::default(),
                                snap: true,
                            },
                            iced::Background::Color(fill),
                        );
                    }
                }
            }
        }
        if state.focus.ring_visible {
            use iced::advanced::graphics::core::Renderer as _;
            renderer.fill_quad(
                iced::advanced::renderer::Quad {
                    bounds: layout.bounds(),
                    border: iced::Border {
                        color: palette.focus_ring,
                        width: crate::theme_lookup::focus_ring_width(theme),
                        radius: 2.0.into(),
                    },
                    shadow: iced::Shadow::default(),
                    snap: true,
                },
                iced::Background::Color(Color::TRANSPARENT),
            );
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
        let state = tree.state.downcast_ref::<ScopeState>();
        if state.model.is_dragging() {
            return mouse::Interaction::Text;
        }
        let child_layout = layout.children(&tree.children).next().unwrap_or(layout);
        let child_interaction = self.child.as_widget().mouse_interaction(
            &tree.children[0],
            child_layout,
            cursor,
            viewport,
            renderer,
        );
        if child_interaction != mouse::Interaction::default() {
            return child_interaction;
        }
        let offset = child_layout.position() - Point::ORIGIN;
        if let Some(pos) = cursor.position()
            && state.regions.iter().any(|r| r.contains(pos - offset))
        {
            return mouse::Interaction::Text;
        }

        child_interaction
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
        tree.state.downcast_mut::<ScopeState>().translation = translation;
        let child_layout = layout.children(&tree.children).next().unwrap_or(layout);
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
