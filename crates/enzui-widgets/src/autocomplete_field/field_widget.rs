//! Retained autocomplete input state and published semantics.

use super::*;

impl<'a, T, Message> iced::advanced::Widget<Message, iced::Theme, iced::Renderer>
    for AutocompleteWidget<'a, T, Message>
where
    T: 'a,
    Message: Clone + 'a,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<AutocompleteState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(AutocompleteState::default())
    }

    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        // Fixed child slots preserve popup scroll state while the list is hidden.
        if tree.children.len() != 2 {
            tree.children = vec![
                iced::advanced::widget::Tree::new(&self.field),
                iced::advanced::widget::Tree::empty(),
            ];
        }

        {
            let state = tree.state.downcast_mut::<AutocompleteState>();
            if state.last_value != self.value {
                state.last_value.clone_from(&self.value);
                // A commit absorbs one value rewrite; later changes lift dismissal suppression.
                if state.skip_next_reopen {
                    state.skip_next_reopen = false;
                } else {
                    state.dismissed = false;
                }
                state.hovered_row = None;
                state.pressed_row = None;
            }
            self.reconcile_active(state);
        }

        tree.children[0].diff(&mut self.field);
        let (snapshot, focused) = {
            let state = tree.state.downcast_ref::<AutocompleteState>().clone();
            let focused =
                crate::field_chrome::focused_input_in::<iced::Renderer>(&tree.children[0])
                    .unwrap_or(false);
            (state, focused)
        };
        if let Some(mut popup) = self.popup(&snapshot, focused) {
            tree.children[1].diff(&mut popup);
        }
    }

    fn size(&self) -> iced::Size<Length> {
        self.field.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &iced::Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        self.field
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
        self.field.as_widget().draw(
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
        self.field.as_widget().mouse_interaction(
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
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        let focused = probe_focus(
            &mut self.field,
            &mut tree.children[0],
            layout,
            renderer,
            &self.node_id,
        );

        let (active, open) = {
            let state = tree.state.downcast_mut::<AutocompleteState>();
            self.reconcile_active(state);
            let open = self.is_open(state, focused);
            (state.active, open)
        };

        let bounds = self.chrome_bounds(layout, &tree.children[0].children);
        let mut semantics = self
            .semantics
            .clone()
            .focused(focused && self.enabled)
            // Openness is derived; there is no routed Expand/Collapse action.
            .expanded_state_only(open);
        if !self.value.is_empty() {
            semantics = semantics.value(self.value.clone());
        }
        if open && let Some(row_id) = active.and_then(|i| self.row_ids.get(i)) {
            semantics = semantics.active_descendant(row_id.clone());
        }
        let mut node = enzui_core::collect::SemanticsNode::new(semantics);
        operation.custom(Some(&self.node_id), bounds, &mut node);

        if self.is_operable() {
            let mut request = crate::value_action::ValueRequest::new(self.node_id.clone());
            operation.custom(Some(&self.node_id), bounds, &mut request);
            if let Some(crate::value_action::ValueIntent::Set(text)) = request.take() {
                tree.state.downcast_mut::<AutocompleteState>().pending_value = Some(text);
            }
        }

        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            self.field.as_widget_mut().operate(
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
        let window_key = enzui_core::focus::window_key(shell.window());
        tree.state.downcast_mut::<AutocompleteState>().window_key = window_key;
        enzui_core::focus::note_input_event_for(event, window_key);
        if let iced::Event::Window(iced::window::Event::RedrawRequested(_)) = event {
            let focused = probe_focus(
                &mut self.field,
                &mut tree.children[0],
                layout,
                renderer,
                &self.node_id,
            );
            let state = tree.state.downcast_mut::<AutocompleteState>();
            let open = self.is_open(state, focused);
            state.reveal.track(event, open, shell);
        }

        {
            // Always drain the latch so a read-only rebuild cannot postpone an armed edit.
            let pending = tree
                .state
                .downcast_mut::<AutocompleteState>()
                .pending_value
                .take();
            if let Some(text) = pending
                && self.is_operable()
            {
                shell.publish((self.on_input)(text));
                shell.request_redraw();
                tree.state.downcast_mut::<AutocompleteState>().edited = true;
            }
        }

        // Decode menu keys before the text input can capture Enter or Escape.
        let focused = probe_focus(
            &mut self.field,
            &mut tree.children[0],
            layout,
            renderer,
            &self.node_id,
        );
        let open = {
            let state = tree.state.downcast_mut::<AutocompleteState>();
            self.reconcile_active(state);
            self.is_open(state, focused)
        };
        if let iced::Event::Keyboard(kb) = event
            && focused
            && self.is_operable()
            && let Some(action) = key_for_autocomplete(open, kb)
        {
            self.handle_key(tree, action, open, shell, renderer, layout, *viewport);
            if shell.is_event_captured() {
                return;
            }
        }

        // A press on this field's own open suggestions reaches the field below
        // them with the pointer levitating: the field keeps its focus, and
        // with it the popup the press is choosing from.
        if open
            && cursor.is_levitating()
            && matches!(
                event,
                iced::Event::Mouse(iced::mouse::Event::ButtonPressed(_))
            )
        {
            return;
        }

        // The input's only message source is on_input, so local messages identify actual edits.
        let mut child_messages: iced::advanced::shell::Bus<Message> =
            iced::advanced::shell::Bus::new();
        let mut child_shell = shell.local(&mut child_messages);
        self.field.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            &mut child_shell,
            viewport,
        );
        let edited_now = !child_shell.is_empty();
        shell.merge(child_shell, |m| m);
        if edited_now && focused && self.is_operable() {
            tree.state.downcast_mut::<AutocompleteState>().edited = true;
        }
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
        let option_row_height = self.suggestion_row_height();
        let anchor = self.chrome_bounds(layout, &tree.children[0].children) + translation;
        let focused = probe_focus(
            &mut self.field,
            &mut tree.children[0],
            layout,
            renderer,
            &self.node_id,
        );
        let snapshot = tree.state.downcast_ref::<AutocompleteState>().clone();
        let popup = self.popup(&snapshot, focused);

        let iced::advanced::widget::Tree {
            state: tree_state,
            children,
            ..
        } = tree;
        let ta_state = tree_state.downcast_mut::<AutocompleteState>();
        let (field_tree, rest) = children.split_at_mut(1);

        let field_overlay = self.field.as_widget_mut().overlay(
            &mut field_tree[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        );

        let popup_overlay = match (popup, rest.first_mut()) {
            (Some(mut popup), Some(popup_tree)) => {
                popup_tree.diff(&mut popup);
                Some(crate::overlay_adapter::element(
                    SuggestionOverlay {
                        popup,
                        tree: popup_tree,
                        anchor,
                        direction: self.direction,
                        state: ta_state,
                        options: &self.options,
                        row_keys: &self.row_keys,
                        on_select: self.on_select.as_ref(),
                        row_height: option_row_height,
                    },
                    renderer,
                    window,
                ))
            }
            _ => None,
        };

        let mut overlays = field_overlay;
        overlays.extend(popup_overlay);
        overlays
    }
}
