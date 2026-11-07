//! Wavy underlines under the misspelled words of a text area.
//!
//! [`spell_underlines`] wraps the text area's editor. Each draw asks the
//! [`SpellCheck`] which words of the content are misspelled (its verdicts
//! are remembered, so this is a scan, not a dictionary lookup per word) and
//! paints a wave in the theme's error colour under each, where the editor
//! laid the word out. Words the user ignores or adds to the dictionary
//! lose their wave on the next draw.

use enzui_core::spelling::SpellCheck;
use iced::advanced::text::editor::Position;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::widget::text_editor::Content;
use iced::{Color, Element, Event, Length, Rectangle, Size, Vector};

/// One misspelled word: where it starts and ends, and the word.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Misspelling {
    pub start: Position,
    pub end: Position,
    pub word: String,
}

/// The misspelled words of `content`, line by line.
pub(crate) fn misspellings(content: &Content, check: &SpellCheck) -> Vec<Misspelling> {
    let mut found = Vec::new();
    for (line, text) in content.lines().enumerate() {
        let text = text.text;
        for range in check.misspelled(&text) {
            found.push(Misspelling {
                start: Position {
                    line,
                    index: range.start,
                },
                end: Position {
                    line,
                    index: range.end,
                },
                word: text.get(range).unwrap_or_default().to_owned(),
            });
        }
    }
    found
}

/// The misspelled word whose laid-out text contains `point` (relative to the
/// editor's text origin), or holds the caret when `point` is `None`.
pub(crate) fn misspelling_at(
    content: &Content,
    check: &SpellCheck,
    point: Option<iced::Point>,
) -> Option<Misspelling> {
    let words = misspellings(content, check);
    match point {
        Some(point) => {
            let ranges: Vec<_> = words.iter().map(|word| (word.start, word.end)).collect();
            let rects = content.highlight_ranges(&ranges);
            words
                .into_iter()
                .zip(rects)
                .find(|(_, rects)| rects.iter().any(|rect| rect.contains(point)))
                .map(|(word, _)| word)
        }
        None => {
            let caret = content.cursor().position;
            words.into_iter().find(|word| {
                word.start.line == caret.line
                    && word.start.index <= caret.index
                    && caret.index <= word.end.index
            })
        }
    }
}

/// Wraps a text area's `editor`, whose content is `content`, with wavy
/// underlines under the words `check` finds misspelled.
pub(crate) fn spell_underlines<'a, Message: 'a>(
    editor: Element<'a, Message>,
    content: &'a Content,
    check: SpellCheck,
) -> Element<'a, Message> {
    Element::new(SpellUnderlines {
        editor,
        content,
        check,
    })
}

struct SpellUnderlines<'a, Message> {
    editor: Element<'a, Message>,
    content: &'a Content,
    check: SpellCheck,
}

/// The wave's height and length, in logical pixels.
const AMPLITUDE: f32 = 1.5;
const PERIOD: f32 = 4.0;
/// How far the wave's top sits above the bottom of the word's line box.
const LIFT: f32 = 3.0;

/// Paints a triangle wave from `x0` to `x1` with its top at `y`.
fn wave(renderer: &mut iced::Renderer, x0: f32, x1: f32, y: f32, color: Color) {
    use iced::advanced::Renderer as _;
    let step = 0.5;
    let mut x = x0;
    while x < x1 {
        let phase = ((x - x0) % PERIOD) / PERIOD;
        let height = if phase < 0.5 {
            phase * 2.0
        } else {
            2.0 - phase * 2.0
        };
        renderer.fill_quad(
            renderer::Quad {
                bounds: Rectangle {
                    x,
                    y: y + height * AMPLITUDE,
                    width: step + 0.25,
                    height: 1.0,
                },
                ..renderer::Quad::default()
            },
            color,
        );
        x += step;
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for SpellUnderlines<'_, Message> {
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.editor));
    }

    fn size(&self) -> Size<Length> {
        self.editor.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.editor
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        layout::pass_through(tree, 0);
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
        self.editor.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
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
            layout,
            cursor,
            viewport,
        );
        let words = misspellings(self.content, &self.check);
        if words.is_empty() {
            return;
        }
        let ranges: Vec<_> = words.iter().map(|word| (word.start, word.end)).collect();
        let rects = self.content.highlight_ranges(&ranges);
        let bounds = layout.bounds();
        let color = crate::theme_lookup::palette(theme).error;
        renderer.with_layer(bounds, |renderer| {
            for rect in rects.iter().flatten() {
                let x0 = bounds.x + rect.x;
                let bottom = bounds.y + rect.y + rect.height;
                wave(renderer, x0, x0 + rect.width, bottom - LIFT, color);
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
        self.editor.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
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
        self.editor.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        tree.children
            .first()
            .is_none_or(|editor| self.editor.as_widget().frame_interest(editor))
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
        self.editor.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}
