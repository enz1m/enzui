//! A menu button opens a submenu beside its row: keys, pointer highlight and
//! activation act on the open level, and Left or Escape close it alone. The
//! keyboard enters a submenu on its first row; a pointer opens it, by click
//! or by resting on its row, with no row highlighted.

use enzui_core::collect::{CollectedNode, collect_frame};
use enzui_core::menu_model::{EntryId, MenuEntryModel, MenuModel, MenuState};
use enzui_core::semantics::Role;
use enzui_widgets::menu_button::{MenuButtonEvent, apply_event, enz_menu_button, event_for_key};
use iced::Size;
use iced::keyboard::{self, Key, Modifiers, key::Named};

fn model() -> MenuModel {
    MenuModel::new(vec![
        MenuEntryModel::command("new", "New"),
        MenuEntryModel::submenu("recent", "Open Recent"),
        MenuEntryModel::command("quit", "Quit"),
    ])
    .with_submenu(
        "recent",
        MenuModel::new(vec![
            MenuEntryModel::command("notes", "notes.txt"),
            MenuEntryModel::command("todo", "todo.md"),
        ]),
    )
}

fn press(named: Named) -> keyboard::Event {
    keyboard::Event::KeyPressed {
        key: Key::Named(named),
        modified_key: Key::Named(named),
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers: Modifiers::empty(),
        repeat: false,
        text: None,
    }
}

/// Decode `named` for the menu and apply it, returning the event and the
/// chosen command.
fn key(
    model: &mut MenuModel,
    state: &mut MenuState,
    named: Named,
) -> (Option<MenuButtonEvent>, Option<EntryId>) {
    let event = event_for_key(model, state, true, &press(named));
    let chosen = event
        .as_ref()
        .and_then(|event| apply_event(model, state, event));
    (event, chosen)
}

fn open_on_recent() -> (MenuModel, MenuState) {
    let mut model = model();
    let mut state = MenuState::closed();
    let _ = key(&mut model, &mut state, Named::ArrowDown);
    let _ = key(&mut model, &mut state, Named::ArrowDown);
    assert_eq!(model.active().map(EntryId::as_str), Some("recent"));
    (model, state)
}

fn submenu_active(model: &MenuModel) -> Option<&str> {
    model
        .submenu(&EntryId::new("recent"))
        .and_then(MenuModel::active)
        .map(EntryId::as_str)
}

#[test]
fn submenu_navigation_selection_and_dismissal_follow_the_active_level_1() {
    // right_opens_the_submenu_on_its_first_row
    {
        let (mut model, mut state) = open_on_recent();
        let (event, _) = key(&mut model, &mut state, Named::ArrowRight);
        assert_eq!(
            event,
            Some(MenuButtonEvent::SubmenuEntered(EntryId::new("recent")))
        );
        assert_eq!(state.open_submenu().map(EntryId::as_str), Some("recent"));
        assert_eq!(submenu_active(&model), Some("notes"));
    }

    // keys_act_on_the_open_submenu_and_its_command_closes_the_menu
    {
        let (mut model, mut state) = open_on_recent();
        let _ = key(&mut model, &mut state, Named::ArrowRight);
        let _ = key(&mut model, &mut state, Named::ArrowDown);
        assert_eq!(submenu_active(&model), Some("todo"));
        assert_eq!(
            model.active().map(EntryId::as_str),
            Some("recent"),
            "the menu's highlight stays on the submenu row"
        );
        let (_, chosen) = key(&mut model, &mut state, Named::Enter);
        assert_eq!(chosen.as_ref().map(EntryId::as_str), Some("todo"));
        assert!(!state.is_open(), "choosing a command closes the whole menu");
        assert_eq!(state.open_submenu(), None);
    }

    // left_and_escape_close_only_the_submenu
    {
        for closing in [Named::ArrowLeft, Named::Escape] {
            let (mut model, mut state) = open_on_recent();
            let _ = key(&mut model, &mut state, Named::ArrowRight);
            let (event, _) = key(&mut model, &mut state, closing);
            assert_eq!(event, Some(MenuButtonEvent::SubmenuClosed), "{closing:?}");
            assert!(state.is_open(), "{closing:?}: the menu stays open");
            assert_eq!(state.open_submenu(), None);
            assert_eq!(submenu_active(&model), None);
            assert_eq!(model.active().map(EntryId::as_str), Some("recent"));
        }
    }

    // highlighting_another_row_closes_the_submenu
    {
        let (mut model, mut state) = open_on_recent();
        let _ = key(&mut model, &mut state, Named::ArrowRight);
        let _ = apply_event(
            &mut model,
            &mut state,
            &MenuButtonEvent::Highlight(EntryId::new("todo")),
        );
        assert_eq!(submenu_active(&model), Some("todo"), "a submenu row");
        assert!(state.open_submenu().is_some());
        let _ = apply_event(
            &mut model,
            &mut state,
            &MenuButtonEvent::Highlight(EntryId::new("quit")),
        );
        assert_eq!(state.open_submenu(), None, "another row of the menu");
        assert_eq!(model.active().map(EntryId::as_str), Some("quit"));
    }
}

#[test]
fn submenu_navigation_selection_and_dismissal_follow_the_active_level_2() {
    // a_pointer_opens_the_submenu_with_no_row_highlighted
    {
        for event in [
            MenuButtonEvent::SubmenuOpened(EntryId::new("recent")),
            MenuButtonEvent::Highlight(EntryId::new("recent")),
        ] {
            let mut model = model();
            let mut state = MenuState::closed();
            let _ = state.open();
            let _ = apply_event(&mut model, &mut state, &event);
            assert_eq!(
                state.open_submenu().map(EntryId::as_str),
                Some("recent"),
                "{event:?} opens the submenu"
            );
            assert_eq!(
                submenu_active(&model),
                None,
                "{event:?}: no row highlighted"
            );
        }
    }

    // the_open_submenu_is_drawn_beside_its_row
    {
        let (mut model, mut state) = open_on_recent();
        let closed = nodes(&model, &state);
        assert!(
            !closed
                .iter()
                .any(|n| n.semantics.label.as_deref() == Some("notes.txt")),
            "calibration: no submenu before it opens"
        );
        let _ = key(&mut model, &mut state, Named::ArrowRight);
        let open = nodes(&model, &state);
        let row = |label: &str| {
            open.iter()
                .find(|n| {
                    n.semantics.role == Role::MenuItem
                        && n.semantics.label.as_deref() == Some(label)
                })
                .unwrap_or_else(|| panic!("{label} row"))
                .bounds
        };
        let parent = row("Open Recent");
        let child = row("notes.txt");
        assert!(
            child.x >= parent.x + parent.width - 1.0,
            "the submenu opens to the right: {parent:?} {child:?}"
        );
        assert!(
            (child.y - parent.y).abs() < 12.0,
            "its first row lines up with the submenu row: {parent:?} {child:?}"
        );
        assert_eq!(
            open.iter()
                .filter(|n| n.semantics.role == Role::Menu)
                .count(),
            2,
            "two menus are open"
        );
    }
}

fn nodes(model: &MenuModel, state: &MenuState) -> Vec<CollectedNode> {
    let button = enz_menu_button("File", model, state, |event| event).into_element();
    collect_frame(button, Size::new(640.0, 400.0))
        .collected()
        .to_vec()
}
