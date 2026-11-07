//! `tree_view`: keyed virt column.
//!
//! Owns keyed virtual-column reconciliation for the tree-view implementation.

use super::*;

// ---------------------------------------------------------------------------
// KeyedVirtColumn — keyed vertical column for virtualized row reconciliation
// ---------------------------------------------------------------------------

/// A vertical column that reconciles children by key rather than by slot
/// index, so widget state (pressed, hovered, animation) follows the logical
/// row across realization-window shifts.
///
/// iced's stock `keyed::Column` uses `diff_children_custom_with_search`, which
/// only splices on length changes and falls through to positional matching for
/// equal-length windows (see `keyed_diff` module docs). The tree view's
/// realization window typically produces the same number of rows at every
/// offset, so equal-length shifts are the common case, and the positional
/// fallback silently re-binds one row's pressed/hover/animation state to a
/// different row.
///
/// This widget instead uses [`crate::keyed_diff::reconcile_children_by_key`],
/// the same algorithm `KeyedRow` and `KeyedGrid` share. A row whose logical
/// index leaves the window has its `Tree` dropped; a row entering the window
/// gets a fresh `Tree`; a row present in both frames keeps its state — no
/// leakage, no position-dependent pairing.
///
/// Layout is a zero-spacing vertical flex (all rows `Fill` width, `Fixed`
/// height), identical to `iced::widget::Column::with_children(rows).spacing(0)`.
pub(crate) struct KeyedVirtColumn<'a, Message> {
    keys: Vec<String>,
    children: Vec<Element<'a, Message>>,
    width: Length,
}

pub(crate) struct KeyedVirtColumnState {
    keys: Vec<String>,
}

impl<'a, Message> KeyedVirtColumn<'a, Message> {
    pub(crate) fn new(keyed_children: Vec<(String, Element<'a, Message>)>) -> Self {
        let (keys, children): (Vec<_>, Vec<_>) = keyed_children.into_iter().unzip();
        Self {
            keys,
            children,
            width: Length::Shrink,
        }
    }

    pub(crate) fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }
}

impl<'a, Message: 'a> Widget<Message, iced::Theme, iced::Renderer>
    for KeyedVirtColumn<'a, Message>
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<KeyedVirtColumnState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(KeyedVirtColumnState {
            keys: self.keys.clone(),
        })
    }

    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<KeyedVirtColumnState>();
        crate::keyed_diff::reconcile_children_by_key(
            &mut tree.children,
            &state.keys,
            &self.keys,
            &mut self.children,
            |child_tree, child| child.as_widget_mut().diff(child_tree),
            |child| child.as_widget().tag(),
            |child| Tree::new(child.as_widget()),
        );
        state.keys.clone_from(&self.keys);
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: Length::Shrink,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        layout::flex::resolve(
            layout::flex::Axis::Vertical,
            renderer,
            limits,
            self.width,
            Length::Shrink,
            iced::Padding::ZERO,
            0.0, // spacing
            iced::Alignment::Start,
            &mut self.children,
            &mut tree.children,
        )
        .apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            self.children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
                .for_each(|(child, (layout, state))| {
                    child
                        .as_widget_mut()
                        .operate(state, layout, viewport, renderer, operation);
                });
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
        for (child, (layout, tree)) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
        {
            child
                .as_widget_mut()
                .update(tree, event, layout, cursor, renderer, shell, viewport);
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
        self.children
            .iter()
            .zip(layout.iter(&tree.children))
            .map(|(child, (layout, tree))| {
                child
                    .as_widget()
                    .mouse_interaction(tree, layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
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
        for (child, (layout, state)) in self.children.iter().zip(layout.iter(&tree.children)) {
            child
                .as_widget()
                .draw(state, renderer, theme, style, layout, cursor, viewport);
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
        overlay::from_children(
            &mut self.children,
            tree,
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<KeyedVirtColumn<'a, Message>> for Element<'a, Message> {
    fn from(col: KeyedVirtColumn<'a, Message>) -> Self {
        Self::new(col)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
