//! A menu bar publishes its menus to assistive technology and answers the
//! desktop menu-bar keys, with menus and submenus drawn from the model.

use enzui_core::collect::{CollectedNode, collect_frame};
use enzui_core::menu_bar::{MenuBarEvent, MenuBarMenu, MenuBarModel};
use enzui_core::menu_model::{EntryId, MenuEntryModel, MenuModel};
use enzui_core::semantics::Role;
use enzui_widgets::menu_bar::enz_menu_bar;
use iced::keyboard::{self, Key, key::Named};
use iced::{Event, Point, Size, mouse};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

fn model() -> MenuBarModel {
    MenuBarModel::new(vec![
        MenuBarMenu::new(
            "File",
            MenuModel::new(vec![
                MenuEntryModel::command("new", "New").shortcut("Ctrl+N"),
                MenuEntryModel::submenu("recent", "Open Recent"),
                MenuEntryModel::separator("rule"),
                MenuEntryModel::command("quit", "Quit").shortcut("Ctrl+Q"),
            ]),
        )
        .submenu(
            "recent",
            MenuModel::new(vec![MenuEntryModel::command("notes", "notes.txt")]),
        ),
        MenuBarMenu::new(
            "Edit",
            MenuModel::new(vec![MenuEntryModel::command("undo", "Undo")]),
        ),
    ])
}

fn nodes(model: &MenuBarModel) -> Vec<CollectedNode> {
    let bar = enz_menu_bar(model, |event| event).into_element();
    collect_frame(bar, Size::new(640.0, 400.0))
        .collected()
        .to_vec()
}

fn named(nodes: &[CollectedNode], role: Role) -> Vec<String> {
    nodes
        .iter()
        .filter(|node| node.semantics.role == role)
        .filter_map(|node| node.semantics.label.clone())
        .collect()
}

#[test]
fn the_bar_and_its_menus_publish_their_roles() {
    let mut model = model();
    let closed = nodes(&model);
    assert_eq!(
        closed
            .iter()
            .filter(|n| n.semantics.role == Role::MenuBar)
            .count(),
        1
    );
    assert_eq!(named(&closed, Role::MenuItem), ["File", "Edit"]);
    assert!(
        closed.iter().all(|n| n.semantics.expanded != Some(true)),
        "no menu is open"
    );

    let _ = model.apply(&MenuBarEvent::Open(0));
    let _ = model.apply(&MenuBarEvent::OpenSubmenu(EntryId::new("recent")));
    let open = nodes(&model);
    let file = open
        .iter()
        .find(|n| n.semantics.label.as_deref() == Some("File"))
        .expect("the File title");
    assert_eq!(file.semantics.expanded, Some(true));
    let items = named(&open, Role::MenuItem);
    for row in ["New", "Open Recent", "Quit", "notes.txt"] {
        assert!(items.iter().any(|item| item == row), "{row} in {items:?}");
    }
    assert!(
        open.iter().any(|n| n.semantics.role == Role::Separator),
        "the rule between groups"
    );
    assert!(
        open.iter()
            .filter(|n| n.semantics.role == Role::Menu)
            .count()
            >= 2,
        "the menu and its submenu"
    );
    let new = open
        .iter()
        .find(|n| n.semantics.label.as_deref() == Some("New"))
        .expect("the New row");
    assert_eq!(new.semantics.keyboard_shortcut.as_deref(), Some("Ctrl+N"));
}

#[test]
fn f10_and_arrows_reach_the_model() {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");
    let press = |named: Named| {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Named(named),
            modified_key: Key::Named(named),
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
            text: None,
            repeat: false,
        })
    };
    let mut model = model();
    let mut published = Vec::new();
    for key in [
        Named::F10,
        Named::ArrowRight,
        Named::ArrowDown,
        Named::Enter,
    ] {
        let mut ui = UserInterface::build(
            enz_menu_bar(&model, |event| event).into_element(),
            Size::new(640.0, 400.0),
            Cache::default(),
            &mut renderer,
        );
        let mut messages = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &iced::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[press(key)],
            mouse::Cursor::Available(Point::new(-10.0, -10.0)),
            &mut renderer,
            &mut messages,
        );
        drop(ui);
        for event in messages {
            let chosen = model.apply(&event);
            published.push((event, chosen));
        }
    }
    assert_eq!(
        published,
        [
            (MenuBarEvent::Open(0), None),
            (MenuBarEvent::Switch(1), None),
            (
                MenuBarEvent::Navigate(enzui_core::context_menu::MenuMove::Next),
                None
            ),
            (
                MenuBarEvent::Activate(EntryId::new("undo")),
                Some(EntryId::new("undo"))
            ),
        ]
    );
}

/// An open menu hides the window's text beneath it: popups draw in a layer
/// above the content, so no glyph of the page shows through the menu.
#[test]
fn an_open_menu_hides_the_text_beneath_it() {
    use iced::widget::{column, text};
    let mut model = model();
    let _ = model.apply(&MenuBarEvent::Open(0));
    let model: &'static MenuBarModel = Box::leak(Box::new(model));
    let red = iced::Color::from_rgb(1.0, 0.0, 0.0);
    let page = column![
        enz_menu_bar(model, |event| event).into_element(),
        column((0..8).map(|_| {
            text("WWWWWWWWWWWWWWWWWWWWWWWWWWWWWW")
                .size(18)
                .color(red)
                .into()
        })),
    ];
    let (w, h) = (480, 320);
    let pixels = crate::corner_probe::render_with_overlays(page.into(), w, h, 1.0);
    let red_at = |x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        pixels[i] > 200 && pixels[i + 1] < 60 && pixels[i + 2] < 60
    };
    // The page's red text is drawn where the menu is not.
    assert!(
        (300..470).any(|x| (60..200).any(|y| red_at(x, y))),
        "the probe text renders"
    );
    // Inside the File menu (x 0..240, y 33..133): no red at all.
    let bleeding = (10..225)
        .flat_map(|x| (40..125).map(move |y| (x, y)))
        .filter(|(x, y)| red_at(*x, *y))
        .count();
    assert_eq!(bleeding, 0, "page text shows through the open menu");
}

/// Access keys: Alt and a title's letter open its menu, and a row's letter
/// chooses it; the markers are not shown or read out.
#[test]
fn access_keys_open_menus_and_choose_rows() {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");
    let mut model = MenuBarModel::new(vec![
        MenuBarMenu::new(
            "_File",
            MenuModel::new(vec![
                MenuEntryModel::command("new", "_New"),
                MenuEntryModel::command("open", "_Open…"),
            ]),
        ),
        MenuBarMenu::new(
            "_Edit",
            MenuModel::new(vec![MenuEntryModel::command("undo", "_Undo")]),
        ),
    ]);
    let closed = nodes(&model);
    assert_eq!(
        named(&closed, Role::MenuItem),
        ["File", "Edit"],
        "no markers"
    );
    let press = |letter: &str, modifiers: keyboard::Modifiers| {
        Event::Keyboard(keyboard::Event::KeyPressed {
            key: Key::Character(letter.into()),
            modified_key: Key::Character(letter.into()),
            physical_key: keyboard::key::Physical::Unidentified(
                keyboard::key::NativeCode::Unidentified,
            ),
            location: keyboard::Location::Standard,
            modifiers,
            text: Some(letter.into()),
            repeat: false,
        })
    };
    let mut published = Vec::new();
    for event in [
        press("e", keyboard::Modifiers::ALT),
        press("f", keyboard::Modifiers::ALT),
        press("o", keyboard::Modifiers::empty()),
    ] {
        let mut ui = UserInterface::build(
            enz_menu_bar(&model, |event| event).into_element(),
            Size::new(640.0, 400.0),
            Cache::default(),
            &mut renderer,
        );
        let mut messages = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &iced::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[event],
            mouse::Cursor::Available(Point::new(-10.0, -10.0)),
            &mut renderer,
            &mut messages,
        );
        drop(ui);
        for event in messages {
            let chosen = model.apply(&event);
            published.push((event, chosen));
        }
    }
    assert_eq!(
        published,
        [
            (MenuBarEvent::Open(1), None),
            (MenuBarEvent::Open(0), None),
            (
                MenuBarEvent::Activate(EntryId::new("open")),
                Some(EntryId::new("open"))
            ),
        ]
    );
    let _ = model.apply(&MenuBarEvent::Open(0));
    let rows = named(&nodes(&model), Role::MenuItem);
    assert!(rows.iter().any(|row| row == "Open…"), "{rows:?}");
}
