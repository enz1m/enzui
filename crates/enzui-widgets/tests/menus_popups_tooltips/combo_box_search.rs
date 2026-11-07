//! A searchable combo box filters its options as the user types, highlights
//! the first match, reports indices of the full list, and clears the query
//! after a selection.
//!
//! The host applies every message to its own query and selection and keeps
//! one `Cache` across frames, as an application does.

use enzui_core::collect::SemanticsCollector;
use enzui_core::semantics::Role;
use enzui_widgets::combo_box::enz_combo_box;
use iced::advanced::widget::operation::{Outcome, focusable};
use iced::keyboard::key::Named;
use iced::keyboard::{Key, Modifiers};
use iced::{Element, Event, Point, Size, Theme};
use iced_test::runtime::user_interface::{Cache, UserInterface};

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Query(String),
    Pick(usize),
}

const OPTIONS: [&str; 6] = ["Apple", "Banana", "Cherry", "Mango", "Orange", "Açaí"];

struct Host {
    renderer: iced_test::renderer::Renderer,
    cache: Cache,
    query: String,
    picked: Option<usize>,
    messages: Vec<Msg>,
}

impl Host {
    fn new() -> Self {
        let renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                None,
            ),
        )
        .expect("headless renderer");
        let mut host = Self {
            renderer,
            cache: Cache::default(),
            query: String::new(),
            picked: None,
            messages: Vec::new(),
        };
        host.focus();
        host
    }

    fn view(&self) -> Element<'static, Msg> {
        enz_combo_box(
            "Fruit",
            OPTIONS.iter().map(|o| (*o).to_owned()).collect(),
            None,
            |_| Msg::Pick(usize::MAX),
        )
        .selected_index(self.picked)
        .on_select_index(Msg::Pick)
        .searchable(self.query.clone(), Msg::Query)
        .id(iced::advanced::widget::Id::new("fruit"))
        .into()
    }

    fn ui(&mut self) -> UserInterface<'static, Msg, Theme, iced_test::renderer::Renderer> {
        UserInterface::build(
            self.view(),
            Size::new(400.0, 600.0),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        )
    }

    /// One frame of `events`; the messages are applied before returning.
    fn frame(&mut self, events: &[Event]) -> Vec<Msg> {
        let mut bus = iced::advanced::shell::Bus::new();
        let mut ui = self.ui();
        let _ = ui.update(
            &iced::advanced::graphics::core::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            iced::advanced::mouse::Cursor::Available(Point::new(5000.0, 5000.0)),
            &mut self.renderer,
            &mut bus,
        );
        self.cache = ui.into_cache();
        let messages: Vec<Msg> = bus.into_iter().collect();
        for message in &messages {
            match message {
                Msg::Query(query) => self.query.clone_from(query),
                Msg::Pick(index) => self.picked = Some(*index),
            }
        }
        self.messages.extend(messages.iter().cloned());
        messages
    }

    fn focus(&mut self) {
        let mut ui = self.ui();
        let mut operation: Box<dyn iced::advanced::widget::Operation> =
            Box::new(focusable::focus(iced::advanced::widget::Id::new("fruit")));
        loop {
            ui.operate(&self.renderer, operation.as_mut());
            match operation.finish() {
                Outcome::Chain(next) => operation = next,
                _ => break,
            }
        }
        self.cache = ui.into_cache();
    }

    fn type_text(&mut self, text: &str) {
        for character in text.chars() {
            let typed = character.to_string();
            let _ = self.frame(&[Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: Key::Character(typed.clone().into()),
                modified_key: Key::Character(typed.clone().into()),
                physical_key: iced::keyboard::key::Physical::Unidentified(
                    iced::keyboard::key::NativeCode::Unidentified,
                ),
                location: iced::keyboard::Location::Standard,
                modifiers: Modifiers::empty(),
                text: Some(typed.into()),
                repeat: false,
            })]);
        }
    }

    fn key(&mut self, named: Named) -> Vec<Msg> {
        self.frame(&[Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: Key::Named(named),
            modified_key: Key::Named(named),
            physical_key: iced::keyboard::key::Physical::Unidentified(
                iced::keyboard::key::NativeCode::Unidentified,
            ),
            location: iced::keyboard::Location::Standard,
            modifiers: Modifiers::empty(),
            text: None,
            repeat: false,
        })])
    }

    /// The options the open list shows, and which one is highlighted.
    fn listed(&mut self) -> (Vec<String>, Option<String>) {
        let mut ui = self.ui();
        let mut collector = SemanticsCollector::new();
        ui.operate(&self.renderer, &mut collector);
        self.cache = ui.into_cache();
        let nodes = collector.collected();
        let options = nodes
            .iter()
            .filter(|n| n.semantics.role == Role::ListBoxOption)
            .filter_map(|n| n.semantics.label.clone())
            .collect();
        let highlighted = nodes
            .iter()
            .find(|n| n.semantics.role == Role::ComboBox)
            .and_then(|combo| combo.semantics.active_descendant.clone())
            .and_then(|id| {
                nodes
                    .iter()
                    .find(|n| n.id.as_ref() == Some(&id))
                    .and_then(|n| n.semantics.label.clone())
            });
        (options, highlighted)
    }
}

#[test]
fn search_combo_filters_commits_full_indices_and_handles_empty_queries_1() {
    // typing_filters_and_highlights_the_first_match
    {
        let mut host = Host::new();
        host.type_text("an");
        assert_eq!(host.query, "an");
        let (options, highlighted) = host.listed();
        assert_eq!(
            options,
            ["Banana", "Mango", "Orange"],
            "the list opened, filtered"
        );
        assert_eq!(highlighted.as_deref(), Some("Banana"));
        host.type_text("g");
        let (options, highlighted) = host.listed();
        assert_eq!(options, ["Mango", "Orange"]);
        assert_eq!(highlighted.as_deref(), Some("Mango"), "the new first match");
    }

    // choosing_reports_the_full_list_index_and_clears_the_query
    {
        let mut host = Host::new();
        host.type_text("an");
        let _ = host.key(Named::ArrowDown);
        let chosen = host.key(Named::Enter);
        assert_eq!(
            chosen,
            [Msg::Pick(3), Msg::Query(String::new())],
            "Mango is index 3"
        );
        assert_eq!(host.query, "");
    }

    // backspace_and_escape_edit_the_query_before_closing
    {
        let mut host = Host::new();
        host.type_text("ma");
        assert_eq!(host.key(Named::Backspace), [Msg::Query("m".to_owned())]);
        assert_eq!(host.key(Named::Escape), [Msg::Query(String::new())]);
        let (options, _) = host.listed();
        assert_eq!(
            options.len(),
            OPTIONS.len(),
            "Escape cleared the filter, the list stays"
        );
        assert_eq!(
            host.key(Named::Escape),
            [],
            "a second Escape closes the list"
        );
        assert!(host.listed().0.is_empty());
    }

    // no_match_keeps_the_field_working
    {
        let mut host = Host::new();
        host.type_text("zz");
        let (options, _) = host.listed();
        assert!(options.is_empty());
        assert_eq!(host.key(Named::Backspace), [Msg::Query("z".to_owned())]);
    }
}

#[test]
fn the_search_ignores_case_and_accents() {
    let mut host = Host::new();
    host.type_text("ACAI");
    let (options, highlighted) = host.listed();
    assert_eq!(options, ["Açaí"]);
    assert_eq!(highlighted.as_deref(), Some("Açaí"));
}
