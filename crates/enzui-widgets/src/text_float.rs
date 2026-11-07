//! Elements a text area's text flows around, like an image floated in a
//! paragraph.
//!
//! A [`TextFloat`] belongs to a line of the text area's content and sits
//! against its left or right edge. The editor lays each wrapped row out in
//! the gap the floats leave at its height and returns to the full width
//! below them, while the content, its cursor and its undo history stay one
//! document. The float is drawn in the text and scrolls with it; the caller
//! owns what it shows and moves its line as lines are inserted and removed
//! above it (the editor's [`EditDelta`](iced::widget::text_editor::EditDelta)
//! says how).

use iced::advanced::text::editor::Exclusion;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::widget::text_editor::Content;
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

/// The space kept between a float and the text beside and below it, in
/// logical pixels.
pub const DEFAULT_FLOAT_MARGIN: f32 = 12.0;

/// Which edge of the text a [`TextFloat`] sits against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FloatSide {
    /// Against the left edge; the text flows on its right.
    #[default]
    Left,
    /// Against the right edge; the text flows on its left.
    Right,
}

/// An element a text area's text flows around; see the [module](self).
#[must_use]
pub struct TextFloat<'a, Message> {
    line: usize,
    side: FloatSide,
    offset_y: f32,
    size: Size,
    margin: f32,
    content: Element<'a, Message>,
}

impl<'a, Message> TextFloat<'a, Message> {
    /// `content`, `size` in logical pixels, at the top of `line` against
    /// `side`.
    pub fn new(
        line: usize,
        side: FloatSide,
        size: Size,
        content: impl Into<Element<'a, Message>>,
    ) -> Self {
        Self {
            line,
            side,
            offset_y: 0.0,
            size,
            margin: DEFAULT_FLOAT_MARGIN,
            content: content.into(),
        }
    }

    /// Move the float this far below the top of its line (default `0`).
    pub fn offset_y(mut self, px: f32) -> Self {
        self.offset_y = if px.is_finite() { px } else { 0.0 };
        self
    }

    /// The space kept between the float and the text beside and below it
    /// (default [`DEFAULT_FLOAT_MARGIN`]).
    pub fn margin(mut self, px: f32) -> Self {
        self.margin = enzui_core::metrics::FiniteNonNeg::new_or(px, DEFAULT_FLOAT_MARGIN).get();
        self
    }

    /// The rectangle the text flows around: the float and its margin on the
    /// sides that face text.
    pub(crate) fn exclusion(&self) -> Exclusion {
        let exclusion = Exclusion::anchored(
            self.line,
            Rectangle::new(
                Point::new(0.0, self.offset_y),
                Size::new(
                    self.size.width + self.margin,
                    self.size.height + self.margin,
                ),
            ),
        );
        match self.side {
            FloatSide::Left => exclusion,
            FloatSide::Right => exclusion.from_trailing_edge(),
        }
    }

    /// Where the float itself is drawn, given where the editor placed its
    /// exclusion.
    fn origin_in(&self, exclusion: Rectangle) -> Point {
        match self.side {
            FloatSide::Left => exclusion.position(),
            FloatSide::Right => Point::new(exclusion.x + self.margin, exclusion.y),
        }
    }
}

/// `editor`, the text area's editor, with `floats` drawn in its text.
pub(crate) fn text_floats<'a, Message: 'a>(
    editor: Element<'a, Message>,
    content: &'a Content,
    floats: Vec<TextFloat<'a, Message>>,
) -> Element<'a, Message> {
    if floats.is_empty() {
        return editor;
    }
    Element::new(TextFloats {
        editor,
        content,
        floats,
    })
}

struct TextFloats<'a, Message> {
    editor: Element<'a, Message>,
    content: &'a Content,
    floats: Vec<TextFloat<'a, Message>>,
}

impl<Message> TextFloats<'_, Message> {
    /// How far the editor's text is scrolled.
    fn scroll(&self) -> Vector {
        self.content.scroll_metrics().offset
    }

    /// The cursor in content coordinates when it is over the editor's
    /// viewport; floats are only reachable through it.
    fn content_cursor(&self, layout: Layout, cursor: mouse::Cursor) -> mouse::Cursor {
        let viewport = layout.bounds();
        match cursor.position() {
            Some(point) if viewport.contains(point) => {
                mouse::Cursor::Available(point + self.scroll())
            }
            _ => mouse::Cursor::Unavailable,
        }
    }

    /// Whether `cursor` is over a float.
    fn over_float(&self, tree: &Tree, layout: Layout, cursor: mouse::Cursor) -> bool {
        let cursor = self.content_cursor(layout, cursor);
        (1..=self.floats.len())
            .any(|index| cursor.is_over(layout.child(&tree.children, index).bounds()))
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for TextFloats<'_, Message> {
    fn diff(&mut self, tree: &mut Tree) {
        let mut children: Vec<&mut Element<'_, Message>> = std::iter::once(&mut self.editor)
            .chain(self.floats.iter_mut().map(|float| &mut float.content))
            .collect();
        tree.diff_children_custom(
            &mut children,
            |tree, child| child.as_widget_mut().diff(tree),
            |child| Tree::new(child.as_widget()),
        );
    }

    fn size(&self) -> Size<Length> {
        self.editor.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        // The editor places the floats' exclusions as it lays its text out,
        // so it goes first.
        self.editor
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        tree.children[0].translation = Vector::ZERO;
        tree.size = tree.children[0].size;

        let placed = self.content.exclusion_bounds();
        for (index, float) in self.floats.iter_mut().enumerate() {
            let child = &mut tree.children[index + 1];
            float.content.as_widget_mut().layout(
                child,
                renderer,
                &layout::Limits::new(Size::ZERO, float.size),
            );
            // A float whose line is gone is not drawn: it is laid out far
            // outside the text.
            let origin = match placed.get(index).copied().flatten() {
                Some(exclusion) => float.origin_in(exclusion),
                None => Point::new(f32::MIN / 4.0, f32::MIN / 4.0),
            };
            child.translation = Vector::new(origin.x, origin.y);
        }
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
        let content_cursor = self.content_cursor(layout, cursor);
        let scroll = self.scroll();
        let content_viewport = *viewport + scroll;
        for (index, float) in self.floats.iter_mut().enumerate() {
            let child = layout.child(&tree.children, index + 1);
            float.content.as_widget_mut().update(
                &mut tree.children[index + 1],
                event,
                child,
                content_cursor,
                renderer,
                shell,
                &content_viewport,
            );
            if shell.is_event_captured() {
                return;
            }
        }
        // A press on a float does not move the caret to the text beside it;
        // moves and releases still reach the editor so a drag selection
        // crosses the float.
        let pressed = matches!(
            event,
            Event::Mouse(mouse::Event::ButtonPressed(_)) | Event::Touch(_)
        );
        let editor_cursor = if pressed && self.over_float(tree, layout, cursor) {
            mouse::Cursor::Unavailable
        } else {
            cursor
        };
        let editor = layout.child(&tree.children, 0);
        self.editor.as_widget_mut().update(
            &mut tree.children[0],
            event,
            editor,
            editor_cursor,
            renderer,
            shell,
            viewport,
        );
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
        use iced::advanced::Renderer as _;
        self.editor.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout.child(&tree.children, 0),
            cursor,
            viewport,
        );
        let bounds = layout.bounds();
        let scroll = self.scroll();
        let content_cursor = self.content_cursor(layout, cursor);
        let Some(clip) = bounds.intersection(viewport) else {
            return;
        };
        renderer.with_layer(clip, |renderer| {
            renderer.with_translation(Vector::new(-scroll.x, -scroll.y), |renderer| {
                for (index, float) in self.floats.iter().enumerate() {
                    float.content.as_widget().draw(
                        &tree.children[index + 1],
                        renderer,
                        theme,
                        style,
                        layout.child(&tree.children, index + 1),
                        content_cursor,
                        &(clip + scroll),
                    );
                }
            });
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
        if self.over_float(tree, layout, cursor) {
            let content_cursor = self.content_cursor(layout, cursor);
            let scroll = self.scroll();
            return self
                .floats
                .iter()
                .enumerate()
                .map(|(index, float)| {
                    float.content.as_widget().mouse_interaction(
                        &tree.children[index + 1],
                        layout.child(&tree.children, index + 1),
                        content_cursor,
                        &(*viewport + scroll),
                        renderer,
                    )
                })
                .max()
                .unwrap_or_default();
        }
        self.editor.as_widget().mouse_interaction(
            &tree.children[0],
            layout.child(&tree.children, 0),
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let editor = layout.child(&tree.children, 0);
        self.editor.as_widget_mut().operate(
            &mut tree.children[0],
            editor,
            viewport,
            renderer,
            operation,
        );
        let scroll = self.scroll();
        for (index, float) in self.floats.iter_mut().enumerate() {
            let child = layout.child(&tree.children, index + 1);
            // Published where the float is on screen, not in the content.
            let child = child.move_to(child.position() - scroll);
            float.content.as_widget_mut().operate(
                &mut tree.children[index + 1],
                child,
                viewport,
                renderer,
                operation,
            );
        }
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        std::iter::once(self.editor.as_widget())
            .chain(self.floats.iter().map(|float| float.content.as_widget()))
            .zip(&tree.children)
            .any(|(widget, tree)| widget.frame_interest(tree))
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
        let editor_layout = layout.child(&tree.children, 0);
        self.editor.as_widget_mut().overlay(
            &mut tree.children[0],
            editor_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}
