//! Suggestion placement and pointer commits over a focusless listbox.

use super::*;

pub(super) struct SuggestionOverlay<'a, 'b, T, Message> {
    pub(super) popup: Element<'a, Message>,
    pub(super) tree: &'b mut iced::advanced::widget::Tree,
    pub(super) anchor: Rectangle,
    pub(super) direction: enzui_core::placement::Direction,
    pub(super) state: &'b mut AutocompleteState,
    pub(super) options: &'b [AutocompleteOption<T>],
    pub(super) row_keys: &'b [String],
    pub(super) on_select: &'b dyn Fn(&T) -> Message,
    pub(super) row_height: f32,
}

impl<T, Message> crate::overlay_adapter::LegacyOverlay<Message, iced::Theme, iced::Renderer>
    for SuggestionOverlay<'_, '_, T, Message>
where
    Message: Clone,
{
    fn anchor(&self) -> Option<iced::Rectangle> {
        Some(self.anchor)
    }

    fn layout(
        &mut self,
        renderer: &iced::Renderer,
        bounds: iced::Size,
    ) -> iced::advanced::layout::Node {
        use enzui_core::placement::{Alignment, Overflow, Placement, Side};

        // Flip and shrink rather than covering the input the suggestions belong to.
        let placement = Placement::new(Side::Bottom)
            .alignment(Alignment::Start)
            .direction(self.direction)
            .gap(POPUP_GAP)
            .overflow(Overflow::Shrink);

        crate::anchored_overlay::resolve_popup_layout(
            &mut self.popup,
            self.tree,
            renderer,
            bounds,
            self.anchor,
            &placement,
            Some(self.anchor.width),
        )
    }

    fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::layout::Placed<'_>,
        cursor: iced::advanced::mouse::Cursor,
    ) {
        let layout = layout.to_layout();
        self.state
            .reveal
            .draw(renderer, theme, layout.bounds(), self.anchor, |renderer| {
                self.popup.as_widget().draw(
                    self.tree,
                    renderer,
                    theme,
                    style,
                    layout,
                    cursor,
                    &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
                );
            });
    }

    fn operate(
        &mut self,
        layout: iced::advanced::layout::Placed<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        self.popup.as_widget_mut().operate(
            self.tree,
            layout.to_layout(),
            &layout.bounds(),
            renderer,
            operation,
        );
    }

    fn mouse_interaction(
        &self,
        layout: iced::advanced::layout::Placed<'_>,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
        if cursor
            .position()
            .and_then(|p| {
                row_at(
                    layout.to_layout(),
                    &self.tree.children,
                    self.options.len(),
                    self.row_height,
                    p,
                )
            })
            .is_some()
        {
            return iced::advanced::mouse::Interaction::Pointer;
        }
        let inner = self.popup.as_widget().mouse_interaction(
            self.tree,
            layout.to_layout(),
            cursor,
            &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
            renderer,
        );
        let over_popup = cursor
            .position()
            .is_some_and(|p| layout.bounds().contains(p));
        // Idle blocks sibling tooltip hover inside the popup while preserving pass-through outside it.
        if inner == iced::advanced::mouse::Interaction::None && over_popup {
            iced::advanced::mouse::Interaction::Idle
        } else {
            inner
        }
    }

    fn index(&self) -> f32 {
        enzui_core::overlay_layer::OverlayLayer::Popup.z_index()
    }

    fn overlay<'c>(
        &'c mut self,
        layout: iced::advanced::layout::Placed<'_>,
        renderer: &iced::Renderer,
        window: iced::Size,
    ) -> Vec<iced::advanced::overlay::Element<'c, Message, iced::Theme, iced::Renderer>> {
        self.popup.as_widget_mut().overlay(
            self.tree,
            layout.to_layout(),
            renderer,
            &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
            iced::Vector::ZERO,
            window,
        )
    }

    fn update(
        &mut self,
        event: &iced::Event,
        layout: iced::advanced::layout::Placed<'_>,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
    ) {
        self.popup.as_widget_mut().update(
            self.tree,
            event,
            layout.to_layout(),
            cursor,
            renderer,
            shell,
            &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
        );

        let want = self.options.len();
        let row_h = self.row_height;

        // An unavailable cursor leaves hover unresolved; clearing it can flicker between motion events.
        if let Some(position) = cursor.position() {
            let hovered = row_at(
                layout.to_layout(),
                &self.tree.children,
                want,
                row_h,
                position,
            );
            if self.state.hovered_row != hovered {
                self.state.hovered_row = hovered;
                shell.request_redraw();
            }
        }

        if let Some(Some(position)) = enzui_core::focus::pointer_press_position(event, cursor)
            && let Some(index) = row_at(
                layout.to_layout(),
                &self.tree.children,
                want,
                row_h,
                position,
            )
        {
            self.state.pressed_row = Some(index);
            self.state.active = Some(index);
            self.state.active_key = self.row_keys.get(index).cloned();
            // A suggestion press stays within the input control and must not move focus away.
            shell.capture_event();
            shell.request_redraw();
            return;
        }

        if let Some(Some(position)) = enzui_core::focus::pointer_release_position(event, cursor) {
            let pressed = self.state.pressed_row.take();
            let released_on = row_at(
                layout.to_layout(),
                &self.tree.children,
                want,
                row_h,
                position,
            );
            if let (Some(p), Some(r)) = (pressed, released_on)
                && p == r
                && let Some(option) = self.options.get(r)
            {
                shell.publish((self.on_select)(&option.payload));
                self.state.dismissed = true;
                self.state.skip_next_reopen = true;
                self.state.active = None;
                self.state.active_key = None;
                self.state.hovered_row = None;
                shell.capture_event();
                shell.invalidate_layout();
                shell.request_redraw();
            }
        }
    }
}
