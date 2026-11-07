//! A chip input turns separated or submitted text into chips, removes the
//! last chip on Backspace in an empty draft, and names each remove button.

use enzui_core::collect::SemanticsCollector;
use enzui_core::semantics::Role;
use enzui_widgets::chip_input::{
    ChipInputEvent, DEFAULT_SEPARATORS, apply_chip_event, enz_chip_input,
};
use iced::advanced::widget::Id;
use iced::advanced::widget::operation::{Outcome, focusable};
use iced::keyboard::key::Named;
use iced::{Element, Size};
use iced_test::Simulator;
use iced_test::simulator::click;

#[test]
fn chip_editor_commits_removes_and_preserves_accessible_focus_1() {
    // separators_and_commit_make_chips_once
    {
        let mut chips = vec!["rust".to_owned()];
        let mut draft = String::new();
        apply_chip_event(
            &mut chips,
            &mut draft,
            &ChipInputEvent::Draft("gui, linux,iced".to_owned()),
            DEFAULT_SEPARATORS,
        );
        assert_eq!(chips, ["rust", "gui", "linux"]);
        assert_eq!(
            draft, "iced",
            "the text after the last separator stays a draft"
        );
        apply_chip_event(
            &mut chips,
            &mut draft,
            &ChipInputEvent::Commit,
            DEFAULT_SEPARATORS,
        );
        assert_eq!(chips, ["rust", "gui", "linux", "iced"]);
        assert_eq!(draft, "");
        draft.push_str(" rust ");
        apply_chip_event(
            &mut chips,
            &mut draft,
            &ChipInputEvent::Commit,
            DEFAULT_SEPARATORS,
        );
        assert_eq!(chips.len(), 4, "a chip already there is not added again");
        apply_chip_event(
            &mut chips,
            &mut draft,
            &ChipInputEvent::Remove(1),
            DEFAULT_SEPARATORS,
        );
        assert_eq!(chips, ["rust", "linux", "iced"]);
    }

    // enter_commits_and_backspace_in_an_empty_draft_removes_the_last_chip
    {
        let chips = vec!["rust".to_owned(), "gui".to_owned()];
        let mut ui = focused(&chips, "linux");
        let _ = ui.tap_key(Named::Enter);
        assert_eq!(
            ui.into_messages().collect::<Vec<_>>(),
            [ChipInputEvent::Commit]
        );

        let mut ui = focused(&chips, "");
        let _ = ui.tap_key(Named::Backspace);
        assert_eq!(
            ui.into_messages().collect::<Vec<_>>(),
            [ChipInputEvent::Remove(1)]
        );

        let mut ui = focused(&chips, "li");
        let _ = ui.tap_key(Named::Backspace);
        let messages: Vec<_> = ui.into_messages().collect();
        assert!(
            !messages.contains(&ChipInputEvent::Remove(1)),
            "with a draft, Backspace edits it: {messages:?}"
        );

        let mut ui = Simulator::with_size(
            Default::default(),
            Size::new(480.0, 120.0),
            view(&chips, ""),
        );
        let _ = ui.tap_key(Named::Backspace);
        assert!(
            !ui.into_messages()
                .any(|event| matches!(event, ChipInputEvent::Remove(_))),
            "Backspace elsewhere is not the chip input's"
        );

        let none: Vec<String> = Vec::new();
        let mut ui = focused(&none, "");
        let _ = ui.tap_key(Named::Backspace);
        assert!(
            !ui.into_messages()
                .any(|event| matches!(event, ChipInputEvent::Remove(_))),
            "nothing to remove"
        );
    }

    // each_chip_has_a_named_remove_button
    {
        let chips = vec!["rust".to_owned(), "gui".to_owned()];
        let mut ui = Simulator::with_size(
            Default::default(),
            Size::new(480.0, 120.0),
            view(&chips, ""),
        );
        let mut collector = SemanticsCollector::new();
        ui.operate(&mut collector);
        let strip = |label: &Option<String>| {
            label
                .clone()
                .unwrap_or_default()
                .replace(['\u{2068}', '\u{2069}'], "")
        };
        let group = collector
            .collected()
            .iter()
            .find(|node| node.semantics.role == Role::Group)
            .expect("the group");
        assert_eq!(strip(&group.semantics.label), "Tags");
        let buttons: Vec<_> = collector
            .collected()
            .iter()
            .filter(|node| node.semantics.role == Role::Button)
            .map(|node| (strip(&node.semantics.label), node.bounds))
            .collect();
        assert_eq!(
            buttons
                .iter()
                .map(|(label, _)| label.as_str())
                .collect::<Vec<_>>(),
            ["Remove rust", "Remove gui"]
        );
        ui.point_at(buttons[1].1.center());
        let _ = ui.simulate(click());
        assert_eq!(
            ui.into_messages().collect::<Vec<_>>(),
            [ChipInputEvent::Remove(1)]
        );
    }

    // the_input_keeps_focus_when_a_chip_is_added
    {
        let before = vec!["rust".to_owned()];
        let mut ui = focused(&before, "");
        let _ = ui.typewrite("gui,");
        let mut chips = before.clone();
        let mut draft = String::new();
        for event in ui.drain().collect::<Vec<_>>() {
            apply_chip_event(&mut chips, &mut draft, &event, DEFAULT_SEPARATORS);
        }
        assert_eq!(chips, ["rust", "gui"]);
        let after = chips;
        // The application rebuilds with the new chip; the input stays focused.
        let mut ui = ui.rebuild(view(&after, ""));
        let _ = ui.typewrite("x");
        assert_eq!(
            ui.into_messages().collect::<Vec<_>>(),
            [ChipInputEvent::Draft("x".to_owned())]
        );
    }
}

fn view<'a>(chips: &'a [String], draft: &'a str) -> Element<'a, ChipInputEvent> {
    enz_chip_input(chips, draft, |event| event)
        .label("Tags")
        .placeholder("Add a tag")
        .id(Id::new("tags"))
        .into_element()
}

fn focused<'a>(chips: &'a [String], draft: &'a str) -> Simulator<'a, ChipInputEvent> {
    let mut ui = Simulator::with_size(
        Default::default(),
        Size::new(480.0, 120.0),
        view(chips, draft),
    );
    let mut operation: Box<dyn iced::advanced::widget::Operation> =
        Box::new(focusable::focus(Id::new("tags")));
    loop {
        ui.operate(operation.as_mut());
        match operation.finish() {
            Outcome::Chain(next) => operation = next,
            _ => break,
        }
    }
    ui
}

/// Records the id of every text input an operation visits.
#[derive(Default)]
struct InputIds(Vec<Option<Id>>);

impl iced::advanced::widget::Operation for InputIds {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn iced::advanced::widget::Operation)) {
        operate(self);
    }

    fn text_input(
        &mut self,
        id: Option<&Id>,
        _bounds: iced::Rectangle,
        _state: &mut dyn iced::advanced::widget::operation::TextInput,
    ) {
        self.0.push(id.cloned());
    }
}

#[test]
fn an_unnamed_chip_input_still_has_a_stable_id() {
    // The app shell keeps a focused text input across rebuilds only by id;
    // one without an id stopped the app when clicked inside a shell.
    let chips = vec!["rust".to_owned()];
    for (label, placeholder) in [(Some("Tags"), ""), (None, "Add a tag")] {
        let mut input = enz_chip_input(&chips, "", |event| event).placeholder(placeholder);
        if let Some(label) = label {
            input = input.label(label);
        }
        let mut ui = Simulator::with_size(
            Default::default(),
            Size::new(480.0, 120.0),
            input.into_element(),
        );
        let mut ids = InputIds::default();
        ui.operate(&mut ids);
        assert_eq!(ids.0.len(), 1, "one text input");
        assert!(
            ids.0[0].is_some(),
            "label {label:?} / placeholder {placeholder:?}: the input has an id"
        );
    }
}
