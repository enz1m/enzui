use super::*;
use crate::menu_model::MenuEntryModel;
use iced_core::keyboard::key::Named;
use iced_core::keyboard::{Key, Modifiers};

fn bar() -> MenuBarModel {
    MenuBarModel::new(vec![
        MenuBarMenu::new(
            "File",
            MenuModel::new(vec![
                MenuEntryModel::command("new", "New"),
                MenuEntryModel::submenu("recent", "Open Recent"),
                MenuEntryModel::command("quit", "Quit"),
            ]),
        )
        .submenu(
            "recent",
            MenuModel::new(vec![
                MenuEntryModel::command("a.txt", "a.txt"),
                MenuEntryModel::command("b.txt", "b.txt"),
            ]),
        ),
        MenuBarMenu::new(
            "Edit",
            MenuModel::new(vec![
                MenuEntryModel::command("undo", "Undo"),
                MenuEntryModel::command("redo", "Redo"),
            ]),
        ),
    ])
}

/// Presses `key` and applies what it asks for, returning a chosen command.
fn press(bar: &mut MenuBarModel, key: Named) -> Option<EntryId> {
    let event = bar.event_for_key(&Key::Named(key), Modifiers::empty())?;
    bar.apply(&event)
}

fn active(bar: &MenuBarModel) -> Option<String> {
    let menu = &bar.menus()[bar.open_menu()?];
    let level = match bar.open_submenu() {
        Some(parent) => menu.submenu_model(parent)?,
        None => menu.model(),
    };
    level.active().map(|id| id.as_str().to_owned())
}

#[test]
fn menu_navigation_routes_keyboard_pointer_and_access_keys() {
    // keys_walk_menus_submenus_and_choose_a_command
    {
        let mut bar = bar();
        assert_eq!(
            bar.event_for_key(&Key::Named(Named::ArrowDown), Modifiers::empty()),
            None,
            "a closed bar leaves arrows to the window"
        );
        assert_eq!(press(&mut bar, Named::F10), None);
        assert_eq!(
            (bar.open_menu(), active(&bar).as_deref()),
            (Some(0), Some("new"))
        );

        let _ = press(&mut bar, Named::ArrowDown);
        assert_eq!(active(&bar).as_deref(), Some("recent"));
        let _ = press(&mut bar, Named::ArrowRight);
        assert_eq!(bar.open_submenu().map(EntryId::as_str), Some("recent"));
        assert_eq!(active(&bar).as_deref(), Some("a.txt"));
        let _ = press(&mut bar, Named::ArrowDown);
        assert_eq!(
            press(&mut bar, Named::Enter).as_ref().map(EntryId::as_str),
            Some("b.txt")
        );
        assert_eq!(bar.open_menu(), None, "choosing a command closes the bar");
    }

    // left_and_right_move_between_menus_and_out_of_submenus
    {
        let mut bar = bar();
        let _ = press(&mut bar, Named::F10);
        let _ = press(&mut bar, Named::ArrowLeft);
        assert_eq!(
            bar.open_menu(),
            Some(1),
            "Left from the first menu wraps to the last"
        );
        assert_eq!(active(&bar).as_deref(), Some("undo"));
        let _ = press(&mut bar, Named::ArrowRight);
        assert_eq!(bar.open_menu(), Some(0));

        let _ = press(&mut bar, Named::ArrowDown);
        let _ = press(&mut bar, Named::ArrowRight);
        assert!(bar.open_submenu().is_some());
        let _ = press(&mut bar, Named::ArrowLeft);
        assert_eq!(
            (bar.open_menu(), bar.open_submenu()),
            (Some(0), None),
            "Left leaves the submenu, not the menu"
        );
        assert_eq!(press(&mut bar, Named::Escape), None);
        assert_eq!(bar.open_menu(), None);
    }

    // pointer_events_open_highlight_and_choose
    {
        let mut bar = bar();
        let _ = bar.apply(&MenuBarEvent::OpenUnhighlighted(1));
        assert_eq!((bar.open_menu(), active(&bar)), (Some(1), None));
        let _ = bar.apply(&MenuBarEvent::Highlight(EntryId::new("redo")));
        assert_eq!(active(&bar).as_deref(), Some("redo"));
        assert_eq!(
            bar.apply(&MenuBarEvent::Activate(EntryId::new("redo"))),
            Some(EntryId::new("redo"))
        );
        assert_eq!(
            bar.event_for_key(&Key::Named(Named::F10), Modifiers::CTRL),
            None,
            "chords stay with the application"
        );
    }

    // access_keys_need_alt_on_titles_and_not_in_an_open_menu
    {
        let mut model = MenuBarModel::new(vec![
            MenuBarMenu::new(
                "_File",
                MenuModel::new(vec![
                    MenuEntryModel::command("save", "_Save"),
                    MenuEntryModel::command("off", "_Disabled").enabled(false),
                ]),
            ),
            MenuBarMenu::new(
                "_View",
                MenuModel::new(vec![MenuEntryModel::command("zoom", "_Zoom")]),
            ),
        ]);
        let key = |c: &str| Key::Character(c.into());
        assert_eq!(
            model.event_for_key(&key("v"), Modifiers::empty()),
            None,
            "no Alt, no menu"
        );
        assert_eq!(
            model.event_for_key(&key("v"), Modifiers::ALT),
            Some(MenuBarEvent::Open(1))
        );
        assert_eq!(
            model.event_for_key(&key("v"), Modifiers::ALT | Modifiers::CTRL),
            None
        );
        let _ = model.apply(&MenuBarEvent::Open(0));
        assert_eq!(
            model.event_for_key(&key("S"), Modifiers::empty()),
            Some(MenuBarEvent::Activate(EntryId::new("save")))
        );
        assert_eq!(
            model.event_for_key(&key("d"), Modifiers::empty()),
            None,
            "a disabled row takes no key"
        );
        assert_eq!(
            model.event_for_key(&key("z"), Modifiers::empty()),
            None,
            "another menu's row"
        );
    }
}
