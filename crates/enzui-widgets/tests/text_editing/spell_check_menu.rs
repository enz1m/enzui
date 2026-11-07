//! Spell checking in a text area: a red wave under each misspelled word, a
//! right click on one offers its suggestions, choosing one replaces the word
//! through the edit callback, and "Ignore" and "Add to Dictionary" accept it.

use enzui_core::collect::{CollectedNode, SemanticsCollector};
use enzui_core::semantics::Role;
use enzui_core::spelling::{SpellCheck, SpellChecker};
use enzui_widgets::text_area::{TextAreaTokens, enz_text_area};
use iced::advanced::renderer::Headless as _;
use iced::advanced::text::editor::Position;
use iced::advanced::widget::{Id, Operation};
use iced::widget::text_editor::{Action, Content};
use iced::{Element, Event, Point, Size, mouse, window};
use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, UserInterface};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
enum Msg {
    Edit(Action),
}

/// Knows a handful of words; suggests `cat` and `coat` for anything else.
#[derive(Default)]
struct Tiny {
    added: Mutex<Vec<String>>,
}

impl SpellChecker for Tiny {
    fn check(&self, word: &str) -> bool {
        ["the", "cat", "sat", "on", "a", "mat"].contains(&word)
            || self.added.lock().unwrap().iter().any(|added| added == word)
    }
    fn suggest(&self, _word: &str) -> Vec<String> {
        vec!["cat".into(), "coat".into()]
    }
    fn add_to_dictionary(&self, word: &str) {
        self.added.lock().unwrap().push(word.to_owned());
    }
    fn language(&self) -> &str {
        "en_ZZ"
    }
}

struct Host {
    renderer: Renderer,
    cache: Cache,
    content: Content,
    check: SpellCheck,
    tiny: Arc<Tiny>,
}

impl Host {
    fn new(text: &str) -> Self {
        let tiny = Arc::new(Tiny::default());
        Self {
            renderer: crate::honest_frame_driver::new_renderer(),
            cache: Cache::default(),
            content: Content::with_text(text),
            check: SpellCheck::shared(Arc::<Tiny>::clone(&tiny)),
            tiny,
        }
    }

    /// One frame: the menu's items and the field's bounds.
    fn frame(
        &mut self,
        cursor: mouse::Cursor,
        events: &[Event],
    ) -> (Vec<CollectedNode>, iced::Rectangle) {
        let element: Element<'_, Msg> = enz_text_area(&self.content, Msg::Edit)
            .id(Id::new("reply"))
            .spell_check(&self.check)
            .into();
        let mut ui = UserInterface::build(
            element,
            Size::new(500.0, 300.0),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        let mut messages = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            cursor,
            &mut self.renderer,
            &mut messages,
        );
        let mut collector = SemanticsCollector::new();
        ui.operate(&self.renderer, &mut collector as &mut dyn Operation);
        self.cache = ui.into_cache();
        for message in messages {
            let Msg::Edit(action) = message;
            let _ = self.content.perform(action);
        }
        let nodes = collector.into_collected();
        let editor = nodes
            .iter()
            .rfind(|node| node.semantics.role == Role::TextInput)
            .map(|node| node.bounds)
            .unwrap_or_default();
        let items = nodes
            .into_iter()
            .filter(|node| node.semantics.role == Role::MenuItem)
            .collect();
        (items, editor)
    }

    /// Draws the area and counts the red pixels in the band just under the
    /// word from `start` to `end` on line 0.
    fn red_under(&mut self, start: usize, end: usize) -> usize {
        const SIZE: Size = Size::new(500.0, 300.0);
        let element: Element<'_, Msg> = enz_text_area(&self.content, Msg::Edit)
            .id(Id::new("reply"))
            .spell_check(&self.check)
            .into();
        let mut ui = UserInterface::build(
            element,
            SIZE,
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        ui.draw(
            &mut self.renderer,
            &iced::Theme::Dark,
            &iced::advanced::renderer::Style {
                text_color: iced::Color::WHITE,
            },
            mouse::Cursor::Unavailable,
        );
        let mut collector = SemanticsCollector::new();
        ui.operate(&self.renderer, &mut collector as &mut dyn Operation);
        self.cache = ui.into_cache();
        let field = collector
            .into_collected()
            .into_iter()
            .find(|node| node.semantics.role == Role::TextInput)
            .unwrap()
            .bounds;
        let rect = self.content.highlight_ranges(&[(
            Position {
                line: 0,
                index: start,
            },
            Position {
                line: 0,
                index: end,
            },
        )])[0][0];
        let pixels = self
            .renderer
            .screenshot(Size::new(500, 300), 1.0, iced::Color::BLACK);
        let padding = TextAreaTokens::from_palette(&enzui_theme::Palette::dark()).padding;
        let (x0, y0) = (field.x + padding + rect.x, field.y + padding + rect.y);
        let rows = (y0 + rect.height - 5.0) as usize..(y0 + rect.height + 1.0) as usize;
        let columns = x0 as usize..(x0 + rect.width) as usize;
        rows.flat_map(|y| columns.clone().map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let i = (y * SIZE.width as usize + x) * 4;
                let (r, g, b) = (pixels[i], pixels[i + 1], pixels[i + 2]);
                r > 120 && r > g.saturating_add(60) && r > b.saturating_add(60)
            })
            .count()
    }

    /// Where the word from `start` to `end` on line 0 is on screen.
    fn word_center(&mut self, start: usize, end: usize) -> Point {
        let (_, editor) = self.frame(mouse::Cursor::Unavailable, &[]);
        let rects = self.content.highlight_ranges(&[(
            Position {
                line: 0,
                index: start,
            },
            Position {
                line: 0,
                index: end,
            },
        )]);
        let rect = rects[0][0];
        // The editor's text starts inside the field's padding.
        let padding = TextAreaTokens::from_palette(&enzui_theme::Palette::dark()).padding;
        Point::new(
            editor.x + padding + rect.center_x(),
            editor.y + padding + rect.center_y(),
        )
    }
}

fn labels(items: &[CollectedNode]) -> Vec<String> {
    items
        .iter()
        .filter_map(|node| node.semantics.label.clone())
        .collect()
}

fn click(button: mouse::Button) -> Vec<Event> {
    vec![
        Event::Mouse(mouse::Event::ButtonPressed(button)),
        Event::Mouse(mouse::Event::ButtonReleased(button)),
    ]
}

/// Right-clicks the word, then chooses `label`.
fn choose(host: &mut Host, word: Point, label: &str) -> Vec<String> {
    let (menu, _) = host.frame(mouse::Cursor::Available(word), &click(mouse::Button::Right));
    let offered = labels(&menu);
    let entry = menu
        .iter()
        .find(|node| node.semantics.label.as_deref() == Some(label))
        .unwrap_or_else(|| panic!("no {label:?} in {offered:?}"))
        .bounds
        .center();
    let _ = host.frame(mouse::Cursor::Available(entry), &click(mouse::Button::Left));
    let _ = host.frame(mouse::Cursor::Unavailable, &[]);
    offered
}

#[test]
fn a_suggestion_replaces_the_misspelled_word() {
    let mut host = Host::new("the caat sat on a mat");
    let word = host.word_center(4, 8);
    let offered = choose(&mut host, word, "coat");
    assert_eq!(
        offered[..4],
        ["cat", "coat", "Add to Dictionary", "Ignore"],
        "suggestions come first: {offered:?}"
    );
    assert_eq!(offered.last().map(String::as_str), Some("Select All"));
    assert_eq!(host.content.text(), "the coat sat on a mat");
}

#[test]
fn a_correct_word_gets_the_plain_edit_menu() {
    let mut host = Host::new("the caat sat on a mat");
    let word = host.word_center(9, 12);
    let (menu, _) = host.frame(mouse::Cursor::Available(word), &click(mouse::Button::Right));
    assert_eq!(labels(&menu)[0], "Undo");
}

#[test]
fn ignore_and_add_accept_the_word() {
    let mut host = Host::new("the caat sat on a matt");
    let caat = host.word_center(4, 8);
    let _ = choose(&mut host, caat, "Ignore");
    assert!(host.check.is_correct("caat"), "ignored for the session");
    assert!(host.tiny.added.lock().unwrap().is_empty(), "not added");
    let matt = host.word_center(18, 22);
    let _ = choose(&mut host, matt, "Add to Dictionary");
    assert_eq!(*host.tiny.added.lock().unwrap(), ["matt"]);
    assert_eq!(host.content.text(), "the caat sat on a matt", "no edit");
    // Neither is offered again.
    let (menu, _) = host.frame(mouse::Cursor::Available(matt), &click(mouse::Button::Right));
    assert_eq!(labels(&menu)[0], "Undo");
}

#[test]
fn a_wave_runs_under_misspelled_words_only() {
    let mut host = Host::new("the caat sat on a mat");
    let _ = host.frame(mouse::Cursor::Unavailable, &[]);
    let under_caat = host.red_under(4, 8);
    assert!(
        under_caat > 10,
        "a red wave under caat: {under_caat} pixels"
    );
    assert_eq!(host.red_under(9, 12), 0, "nothing under sat");
    host.check.ignore("caat");
    assert_eq!(host.red_under(4, 8), 0, "an ignored word loses its wave");
}
