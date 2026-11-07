//! Pane placement that mirrors geometry while preserving logical focus and accessibility order.

use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

pub(crate) struct MirrorRow<'a, Message> {
    // Permanent logical storage keeps widget state attached to its child across direction changes.
    children: Vec<Element<'a, Message>>,
    mirrored: bool,
    spacing: f32,
    width: Length,
    height: Length,
    align: iced::Alignment,
}

impl<'a, Message> MirrorRow<'a, Message> {
    pub(crate) fn new(logical: Vec<Element<'a, Message>>, mirrored: bool) -> Self {
        Self {
            children: logical,
            mirrored,
            spacing: 0.0,
            // Fit permits child-size promotion; Shrink would compress filling panes.
            width: Length::Fit,
            height: Length::Fit,
            align: iced::Alignment::Center,
        }
    }

    pub(crate) fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = if spacing.is_finite() && spacing >= 0.0 {
            spacing
        } else {
            0.0
        };
        self
    }

    pub(crate) fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }

    pub(crate) fn height(mut self, height: Length) -> Self {
        self.height = height;
        self
    }

    fn logical_child_at(&self, visual: usize) -> usize {
        if self.mirrored {
            self.children.len().saturating_sub(1) - visual
        } else {
            visual
        }
    }

    // Flex requires parallel visual slices; restore logical storage before any other traversal.
    fn with_visual_order<R>(
        &mut self,
        tree: &mut Tree,
        f: impl FnOnce(&mut Vec<Element<'a, Message>>, &mut Tree) -> R,
    ) -> R {
        if !self.mirrored {
            return f(&mut self.children, tree);
        }
        self.children.reverse();
        tree.children.reverse();
        let out = f(&mut self.children, tree);
        self.children.reverse();
        tree.children.reverse();
        out
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for MirrorRow<'_, Message> {
    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);

        if self.width.is_fit() || self.height.is_fit() {
            for child in &self.children {
                let size = child.as_widget().size();
                self.width = self.width.stack(size.width);
                self.height = self.height.cross(size.height);
            }
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let (width, height, spacing, align) = (self.width, self.height, self.spacing, self.align);
        let mirrored = self.mirrored;
        self.with_visual_order(tree, |children, tree| {
            layout::flex::resolve(
                layout::flex::Axis::Horizontal,
                renderer,
                limits,
                width,
                height,
                iced::Padding::ZERO,
                spacing,
                align,
                children,
                &mut tree.children,
            )
        })
        .with_drawing_cull(false)
        .with_child_drawing_culls()
        // Visual layout nodes must be applied to their matching logically stored trees.
        .with_reversed_children(mirrored)
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
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            for (child, (layout, state)) in self
                .children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
            {
                child
                    .as_widget_mut()
                    .operate(state, layout, viewport, renderer, operation);
            }
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
        for (child, (layout, state)) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
        {
            child
                .as_widget_mut()
                .update(state, event, layout, cursor, renderer, shell, viewport);
            // Ordinary input can create frame interest before the first redraw route runs.
            state.refresh_frame_interest(child.as_widget());
        }
        tree.refresh_frame_children();
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
        layout.draw_culled(tree, renderer, viewport, |renderer, viewport| {
            let n = self.children.len();
            for visual in 0..n {
                let logical = self.logical_child_at(visual);
                let (Some(child), Some(state)) =
                    (self.children.get(logical), tree.children.get(logical))
                else {
                    continue;
                };
                let child_layout = layout.child(&tree.children, logical);
                let bounds = child_layout.bounds();
                // A child with nothing laid out (a sidebar entering from a
                // zero reservation) may still paint past its bounds for the
                // recording; culling it against the viewport would drop that.
                if bounds.width <= 0.0 || bounds.height <= 0.0 {
                    child.as_widget().draw(
                        state,
                        renderer,
                        theme,
                        style,
                        child_layout,
                        cursor,
                        viewport,
                    );
                    continue;
                }
                child_layout.draw_culled(state, renderer, viewport, |renderer, viewport| {
                    child.as_widget().draw(
                        state,
                        renderer,
                        theme,
                        style,
                        child_layout,
                        cursor,
                        viewport,
                    );
                });
            }
        });
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        (0..self.children.len())
            .filter_map(|logical| {
                let child = self.children.get(logical)?;
                let state = tree.children.get(logical)?;
                Some(child.as_widget().mouse_interaction(
                    state,
                    layout.child(&tree.children, logical),
                    cursor,
                    viewport,
                    renderer,
                ))
            })
            .max()
            .unwrap_or_default()
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
        let overlays: Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> = self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
            .flat_map(|(child, (layout, state))| {
                child.as_widget_mut().overlay(
                    state,
                    layout,
                    renderer,
                    viewport,
                    translation,
                    window,
                )
            })
            .collect();
        overlays
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        !tree.frame_children.is_empty()
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
        // Visit only interested descendants; a blanket walk scales seam motion with app content.
        let Tree {
            children,
            frame_children,
            ..
        } = tree;
        let mut route = 0;
        while route < frame_children.len() {
            let logical = frame_children[route];
            let (Some(child), true) = (self.children.get_mut(logical), logical < children.len())
            else {
                let _ = frame_children.remove(route);
                continue;
            };
            let child_layout = layout.child(children, logical);
            let state = &mut children[logical];
            child.as_widget_mut().update_frame(
                state,
                event,
                child_layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
            state.refresh_frame_interest(child.as_widget());
            if state.has_frame_interest() {
                route += 1;
            } else {
                let _ = frame_children.remove(route);
            }
        }
    }
}

impl<'a, Message: 'a> From<MirrorRow<'a, Message>> for Element<'a, Message> {
    fn from(value: MirrorRow<'a, Message>) -> Self {
        Element::new(value)
    }
}
