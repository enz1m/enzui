//! Popup placement and pointer selection.

use super::*;

pub(crate) struct ComboPopupOverlay<'a, 'b, Message> {
    pub(crate) popup: Element<'a, Message>,
    pub(crate) tree: &'b mut iced::advanced::widget::Tree,
    pub(crate) scroll_id: &'b Id,
    pub(crate) anchor: iced::Rectangle,
    pub(crate) direction: enzui_core::placement::Direction,
    pub(crate) state: &'b mut ComboState,
    pub(crate) options: &'b [String],
    pub(crate) row_keys: &'b [String],
    pub(crate) on_select: &'b dyn Fn(String) -> Message,
    pub(crate) on_select_indexed: Option<&'b dyn Fn(usize) -> Message>,
    pub(crate) row_height: f32,
    pub(crate) window: PopupWindow,
}

// Row rectangles include placement but exclude scroll translation; indices address realized rows.
pub(crate) fn row_rects_from_layout(
    layout: iced::advanced::Layout,
    children: &[iced::advanced::widget::Tree],
    want: usize,
    row_height: f32,
) -> Option<Vec<Rectangle>> {
    fn is_row(h: f32, row_height: f32) -> bool {
        (h - row_height).abs() < 0.5
    }
    fn walk(
        layout: iced::advanced::Layout,
        children: &[iced::advanced::widget::Tree],
        want: usize,
        row_height: f32,
        depth: usize,
    ) -> Option<Vec<Rectangle>> {
        if depth > 8 {
            return None;
        }
        let children: Vec<(iced::advanced::Layout, &iced::advanced::widget::Tree)> =
            layout.iter(children).collect();
        if children.len() >= want {
            let heights: Vec<f32> = children.iter().map(|(c, _)| c.bounds().height).collect();
            for start in 0..=heights.len() - want {
                if (start..start + want).all(|i| is_row(heights[i], row_height)) {
                    return Some(
                        children[start..start + want]
                            .iter()
                            .map(|(c, _)| c.bounds())
                            .collect(),
                    );
                }
            }
        }
        children
            .into_iter()
            .find_map(|(child, tree)| walk(child, &tree.children, want, row_height, depth + 1))
    }
    if want == 0 {
        return None;
    }
    walk(layout, children, want, row_height, 0)
}

// Pointer positions and row rectangles must share the same layout space.
pub(crate) fn row_at(
    layout: iced::advanced::Layout,
    children: &[iced::advanced::widget::Tree],
    want: usize,
    row_height: f32,
    point: iced::Point,
) -> Option<usize> {
    let rects = row_rects_from_layout(layout, children, want, row_height)?;
    rects.iter().position(|r| r.contains(point))
}

pub(super) fn popup_placement(
    direction: enzui_core::placement::Direction,
) -> enzui_core::placement::Placement {
    use enzui_core::placement::{Alignment, Overflow, Placement, Side};

    Placement::new(Side::Bottom)
        .alignment(Alignment::Start)
        .direction(direction)
        .gap(POPUP_GAP)
        .overflow(Overflow::Shrink)
}

pub(super) fn popup_scroll_geometry<Message>(
    popup: &mut Element<'_, Message>,
    tree: &mut Tree,
    layout: iced::advanced::Layout,
    renderer: &iced::Renderer,
    scroll_id: &Id,
) -> Option<(Rectangle, Vector)> {
    struct Probe<'a> {
        target: &'a Id,
        found: Option<(Rectangle, Vector)>,
    }
    impl Operation for Probe<'_> {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn scrollable(
            &mut self,
            id: Option<&Id>,
            bounds: Rectangle,
            _content_bounds: iced::Size,
            translation: Vector,
            _state: &mut dyn Scrollable,
        ) {
            if id == Some(self.target) {
                self.found = Some((bounds, translation));
            }
        }
    }
    let mut probe = Probe {
        target: scroll_id,
        found: None,
    };
    popup
        .as_widget_mut()
        .operate(tree, layout, &layout.bounds(), renderer, &mut probe);
    probe.found
}

impl<Message> crate::overlay_adapter::LegacyOverlay<Message, iced::Theme, iced::Renderer>
    for ComboPopupOverlay<'_, '_, Message>
where
    Message: Clone,
{
    fn anchor(&self) -> Option<iced::Rectangle> {
        Some(self.anchor)
    }

    fn grabs(&self) -> bool {
        true
    }

    fn layout(
        &mut self,
        renderer: &iced::Renderer,
        bounds: iced::Size,
    ) -> iced::advanced::layout::Node {
        crate::anchored_overlay::resolve_popup_layout(
            &mut self.popup,
            self.tree,
            renderer,
            bounds,
            self.anchor,
            &popup_placement(self.direction),
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

        // Scrollable provisionally captures touch presses before distinguishing taps from drags.
        if shell.is_event_captured()
            && !matches!(
                event,
                iced::Event::Touch(iced::touch::Event::FingerPressed { .. })
            )
        {
            self.state.pressed_row = None;
            return;
        }

        let Some((viewport, translation)) = popup_scroll_geometry(
            &mut self.popup,
            self.tree,
            layout.to_layout(),
            renderer,
            self.scroll_id,
        ) else {
            return;
        };
        // Gate hits by the scroll viewport and translate them into document space.
        let option_at = |position: iced::Point| {
            viewport
                .contains(position)
                .then(|| {
                    row_at(
                        layout.to_layout(),
                        &self.tree.children,
                        self.window.len,
                        self.row_height,
                        position + translation,
                    )
                })
                .flatten()
                .and_then(|bp| self.window.option_at(bp))
        };

        if matches!(
            event,
            iced::Event::Mouse(
                iced::mouse::Event::CursorMoved { .. } | iced::mouse::Event::ButtonPressed(_)
            ) | iced::Event::Touch(
                iced::touch::Event::FingerMoved { .. } | iced::touch::Event::FingerPressed { .. }
            )
        ) {
            self.state.keyboard_navigation = false;
        }
        // An unavailable cursor preserves hover until a known position or explicit close clears it.
        if let Some(position) = cursor.position() {
            let hovered = option_at(position);
            if self.state.hovered_row != hovered {
                self.state.hovered_row = hovered;
                shell.request_redraw();
            }
        }

        if let Some(Some(position)) = enzui_core::focus::pointer_press_position(event, cursor)
            && let Some(option_index) = option_at(position)
        {
            self.state.pressed_row = Some(option_index);
            self.state.active = Some(option_index);
            self.state.active_key = self.row_keys.get(option_index).cloned();
            // Stop the trigger from interpreting an option press as an outside dismissal.
            shell.capture_event();
            shell.request_redraw();
            return;
        }

        if let Some(Some(position)) = enzui_core::focus::pointer_release_position(event, cursor) {
            let pressed = self.state.pressed_row.take();
            let released_on = option_at(position);
            match (pressed, released_on) {
                (Some(p), Some(r)) if p == r => {
                    if let Some(on_index) = self.on_select_indexed {
                        shell.publish(on_index(r));
                    } else if let Some(option) = self.options.get(r) {
                        shell.publish((self.on_select)(option.clone()));
                    }
                    self.state.open = false;
                    self.state.active = None;
                    self.state.active_key = None;
                    self.state.hovered_row = None;
                    shell.capture_event();
                    shell.invalidate_layout();
                    shell.request_redraw();
                }
                _ => {}
            }
        }
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
        // A non-None interaction shields underlying hover targets without swallowing outside presses.
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
}
