//! `ContextMenu<Target, Command>` hands back what was chosen and what it was
//! opened on, through the real menu surface, submenus included.

use enzui_core::collect::SemanticsCollector;
use enzui_core::context_menu::{InvocationKind, Point as CorePoint};
use enzui_widgets::context_menu_host::{ContextMenu, ContextMenuEvent, ContextMenuItem};
use iced::{Element, Event, Length, Point, Size, mouse};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

#[derive(Debug, Clone, PartialEq)]
enum Cmd {
    Rename,
    Delete,
    MoveTo(&'static str),
    Connect,
    Disconnect,
    Favorite,
}

fn items() -> Vec<ContextMenuItem<Cmd>> {
    vec![
        ContextMenuItem::command(Cmd::Rename, "Rename").icon("pencil"),
        ContextMenuItem::submenu(
            "Move to",
            vec![
                ContextMenuItem::command(Cmd::MoveTo("Work"), "Work"),
                ContextMenuItem::command(Cmd::MoveTo("Home"), "Home"),
            ],
        ),
        ContextMenuItem::separator(),
        ContextMenuItem::command(Cmd::Delete, "Delete").destructive(true),
    ]
}

/// Click the row labelled `label` in `menu`'s surface and return the events
/// the host published.
fn click(menu: &ContextMenu<usize, Cmd>, label: &str) -> Vec<ContextMenuEvent> {
    let events = pointer(menu, label, true);
    assert!(!events.is_empty(), "no {label:?} row to click");
    events
}

/// Move the pointer onto the row labelled `label`, pressing it when `press`,
/// and return the events the host published; none when no such row shows.
fn pointer(menu: &ContextMenu<usize, Cmd>, label: &str, press: bool) -> Vec<ContextMenuEvent> {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            None,
        ),
    )
    .expect("headless renderer");
    let page = iced::widget::Space::new()
        .width(Length::Fill)
        .height(Length::Fill);
    let view: Element<'_, ContextMenuEvent> = menu.host(page, |event| event).into();
    let size = Size::new(800.0, 600.0);
    let mut ui = UserInterface::build(view, size, Cache::new(), &mut renderer);
    let mut messages = iced::advanced::shell::Bus::new();
    // An update pass builds the menu surface.
    let _ = ui.update(
        &iced::advanced::graphics::core::window::Headless,
        &iced::advanced::graphics::core::shell::Waker::noop(),
        &[],
        mouse::Cursor::Unavailable,
        &mut renderer,
        &mut messages,
    );
    let mut collector = SemanticsCollector::new();
    ui.operate(&renderer, &mut collector);
    let Some(row) = collector
        .collected()
        .iter()
        .find(|node| node.semantics.label.as_deref() == Some(label))
        .map(|node| node.bounds)
    else {
        return Vec::new();
    };
    let at = Point::new(row.center_x(), row.center_y());
    let mut events = vec![Event::Mouse(mouse::Event::CursorMoved { position: at })];
    if press {
        events.extend([
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        ]);
    }
    let _ = ui.update(
        &iced::advanced::graphics::core::window::Headless,
        &iced::advanced::graphics::core::shell::Waker::noop(),
        &events,
        mouse::Cursor::Available(at),
        &mut renderer,
        &mut messages,
    );
    messages.messages().to_vec()
}

/// Feed `events` to the menu, returning the command chosen, if any.
fn apply(
    menu: &mut ContextMenu<usize, Cmd>,
    events: Vec<ContextMenuEvent>,
) -> Option<(usize, Cmd)> {
    let mut chosen = None;
    for event in events {
        let (picked, _focus) = menu.update::<()>(&event);
        chosen = chosen.or(picked);
    }
    chosen
}

#[test]
fn a_chosen_row_comes_back_as_its_command_and_target() {
    let at = InvocationKind::Pointer(CorePoint::new(100.0, 80.0));
    let mut menu: ContextMenu<usize, Cmd> = ContextMenu::new();
    assert!(!menu.is_open());
    menu.open(7, at, items());
    assert_eq!(menu.target(), Some(&7));

    let events = click(&menu, "Rename");
    assert_eq!(apply(&mut menu, events), Some((7, Cmd::Rename)));
    assert!(!menu.is_open(), "choosing closes the menu");
    assert_eq!(menu.target(), None);

    // A command inside a submenu comes back the same way.
    menu.open(3, at, items());
    let events = click(&menu, "Move to");
    assert_eq!(
        apply(&mut menu, events),
        None,
        "opening a submenu chooses nothing"
    );
    assert!(menu.is_open());
    let events = click(&menu, "Home");
    assert_eq!(apply(&mut menu, events), Some((3, Cmd::MoveTo("Home"))));
}

/// A remote-host style menu whose rows depend on whether a session is live.
fn inventory(session: bool, favorite_enabled: bool) -> Vec<ContextMenuItem<Cmd>> {
    let mut rows = vec![ContextMenuItem::command(Cmd::Connect, "Connect").key("connect")];
    if session {
        rows.push(ContextMenuItem::command(Cmd::Disconnect, "Disconnect").key("disconnect"));
    }
    rows.extend([
        ContextMenuItem::separator(),
        ContextMenuItem::command(Cmd::Favorite, "Favorite")
            .key("favorite")
            .enabled(favorite_enabled),
        ContextMenuItem::command(Cmd::Delete, "Delete").key("delete"),
        ContextMenuItem::submenu(
            "Move to",
            vec![
                ContextMenuItem::command(Cmd::MoveTo("Work"), "Work").key("work"),
                ContextMenuItem::command(Cmd::MoveTo("Home"), "Home").key("home"),
            ],
        )
        .key("move"),
    ]);
    rows
}

#[test]
fn a_reconciled_menu_keeps_its_highlight_and_refuses_stale_rows() {
    let at = InvocationKind::Pointer(CorePoint::new(100.0, 80.0));
    let mut menu: ContextMenu<usize, Cmd> = ContextMenu::new();
    menu.open(4, at, inventory(false, true));
    let hover = pointer(&menu, "Favorite", false);
    assert_eq!(apply(&mut menu, hover), None);
    assert_eq!(menu.highlighted(), Some(&Cmd::Favorite));

    // A row appears above the highlight: the highlight stays on Favorite and
    // a click queued before the change still means Favorite.
    let queued = click(&menu, "Favorite");
    assert!(menu.reconcile(inventory(true, true)));
    assert_eq!(menu.highlighted(), Some(&Cmd::Favorite));
    assert_eq!(menu.target(), Some(&4));
    assert_eq!(apply(&mut menu, queued), Some((4, Cmd::Favorite)));

    // The highlighted row is disabled: its queued click is refused.
    menu.open(4, at, inventory(true, true));
    let hover = pointer(&menu, "Favorite", false);
    let _ = apply(&mut menu, hover);
    let queued = click(&menu, "Favorite");
    assert!(menu.reconcile(inventory(false, false)));
    assert_ne!(menu.highlighted(), Some(&Cmd::Favorite));
    assert_eq!(apply(&mut menu, queued), None);
    assert!(menu.is_open(), "a refused click leaves the menu showing");

    // A removed row's queued click never lands on its successor.
    let queued = click(&menu, "Connect");
    assert!(
        menu.reconcile(
            inventory(false, true)
                .into_iter()
                .skip(1)
                .collect::<Vec<_>>()
        )
    );
    assert_eq!(apply(&mut menu, queued), None);
    assert!(menu.is_open());
}

#[test]
fn events_from_an_earlier_showing_are_refused() {
    let at = InvocationKind::Pointer(CorePoint::new(100.0, 80.0));
    let mut menu: ContextMenu<usize, Cmd> = ContextMenu::new();
    menu.open(1, at, inventory(false, true));
    let queued = click(&menu, "Favorite");

    // Reopening on another target outdates the queued click.
    menu.open(2, at, inventory(false, true));
    assert_eq!(apply(&mut menu, queued.clone()), None);

    // So does closing and reopening on the same target.
    menu.close();
    assert!(
        !menu.reconcile(inventory(false, true)),
        "nothing to reconcile"
    );
    menu.open(1, at, inventory(false, true));
    assert_eq!(apply(&mut menu, queued), None);
    assert!(menu.is_open());
}

#[test]
fn a_flipped_child_occludes_its_parent_ink() {
    use enzui_core::semantics::Role;
    use iced::advanced::renderer::Headless as _;
    fn capture(
        renderer: &mut Renderer,
        theme: &iced::Theme,
        label: &'static str,
        child: bool,
    ) -> (Vec<u8>, iced::Rectangle) {
        let mut menu = ContextMenu::new();
        menu.open(
            7,
            InvocationKind::Pointer(CorePoint::new(114.0, 189.0)),
            vec![
                ContextMenuItem::command(Cmd::Rename, "Open"),
                ContextMenuItem::submenu(
                    label,
                    vec![
                        ContextMenuItem::command(Cmd::MoveTo("one"), "Child one"),
                        ContextMenuItem::command(Cmd::MoveTo("two"), "Child two"),
                        ContextMenuItem::command(Cmd::MoveTo("three"), "Child three"),
                    ],
                )
                .key("branch")
                .icon("bell-off"),
                ContextMenuItem::separator(),
                ContextMenuItem::command(Cmd::Delete, "Delete"),
            ],
        );
        if child {
            let events = click(&menu, label);
            assert_eq!(apply(&mut menu, events), None);
        }
        let view: Element<'_, ContextMenuEvent> = menu
            .host(
                iced::widget::Space::new()
                    .width(Length::Fill)
                    .height(Length::Fill),
                |event| event,
            )
            .menu_width(240.0)
            .into();
        let mut ui =
            UserInterface::build(view, Size::new(400.0, 700.0), Cache::default(), renderer);
        let _ = ui.update(
            &iced::window::Headless,
            &iced::advanced::shell::Waker::noop(),
            &[],
            mouse::Cursor::Unavailable,
            renderer,
            &mut iced::advanced::shell::Bus::new(),
        );
        let mut collector = SemanticsCollector::new();
        ui.operate(renderer, &mut collector);
        let nodes = collector.collected();
        let row = |label: &str| {
            nodes
                .iter()
                .find(|n| {
                    n.semantics.label.as_deref() == Some(label)
                        && matches!(n.semantics.role, Role::MenuItem)
                })
                .expect("menu item")
                .bounds
        };
        let parent = row(label);
        let overlap = if child {
            let child = row("Child one");
            assert!(child.x < parent.x, "submenu must flip left");
            child
                .intersection(&parent)
                .expect("positive overlapping menu rows")
        } else {
            parent
        };
        ui.draw(
            renderer,
            theme,
            &iced::advanced::renderer::Style {
                text_color: iced::Color::BLACK,
            },
            mouse::Cursor::Unavailable,
        );
        let pixels = renderer.screenshot(Size::new(400, 700), 1.0, iced::Color::BLACK);
        (pixels, overlap)
    }
    let mut renderer = iced_test::futures::futures::executor::block_on(Renderer::new(
        Default::default(),
        Some("wgpu"),
    ))
    .expect("GPU renderer");
    let changed = |a: &[u8], b: &[u8], area: iced::Rectangle| {
        ((area.y.ceil() as usize + 2)..(area.y + area.height).floor() as usize - 2)
            .flat_map(|y| {
                ((area.x.ceil() as usize + 2)..(area.x + area.width).floor() as usize - 2)
                    .map(move |x| (y * 400 + x) * 4)
            })
            .filter(|&i| (0..3).any(|c| a[i + c].abs_diff(b[i + c]) > 3))
            .count()
    };
    for theme in [iced::Theme::Light, iced::Theme::Dark] {
        let (a, area) = capture(&mut renderer, &theme, "MMMMMMMMMMM", false);
        let (b, _) = capture(&mut renderer, &theme, "XXXXXXXXXXX", false);
        assert!(changed(&a, &b, area) > 20, "parent ink positive control");
        let (a, area) = capture(&mut renderer, &theme, "MMMMMMMMMMM", true);
        let (zero, same) = capture(&mut renderer, &theme, "MMMMMMMMMMM", true);
        assert_eq!(area, same);
        assert!(
            area.width * area.height > 1000.0,
            "substantial overlap required"
        );
        assert!(changed(&a, &zero, area) <= 2, "unchanged paint control");
        let (b, same) = capture(&mut renderer, &theme, "XXXXXXXXXXX", true);
        assert_eq!(area, same);
        assert!(
            changed(&a, &b, area) <= 2,
            "parent ink paints through the flipped child: {} pixels",
            changed(&a, &b, area)
        );
    }
}
