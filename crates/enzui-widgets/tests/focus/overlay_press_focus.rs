//! A press on an overlay (a menu row) takes focus away from the control
//! below that held it: a focused editor, trigger or parent menu row never
//! claims focus alongside the row pressed, which would leave assistive
//! technology with two focus owners.

use enzui_core::collect::{CollectedNode, SemanticsCollector};
use enzui_core::context_menu::{InvocationKind, Point as MenuPoint};
use enzui_core::menu_model::{MenuEntryModel, MenuModel, MenuState};
use enzui_core::semantics::Role;
use enzui_widgets::context_menu_host::{ContextMenu, ContextMenuEvent, ContextMenuItem};
use enzui_widgets::menu_button::{MenuButtonEvent, apply_event, enz_menu_button};
use enzui_widgets::text_area::enz_text_area;
use iced::advanced::renderer::Headless;
use iced::widget::{column, text_editor};
use iced::{Element, Length, Point, Size, mouse};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

#[derive(Debug, Clone)]
enum Msg {
    Edit(text_editor::Action),
    Menu(MenuButtonEvent),
    Context(ContextMenuEvent),
    Open,
    Opened(iced::Point),
}

/// An app the press harness drives.
trait Harness {
    fn view(&self) -> Element<'_, Msg, iced::Theme, Renderer>;
    fn update(&mut self, message: Msg);
}

struct App {
    body: text_editor::Content,
    model: MenuModel,
    menu: MenuState,
}

impl Harness for App {
    fn update(&mut self, message: Msg) {
        match message {
            Msg::Edit(action) => {
                let _ = self.body.perform(action);
            }
            Msg::Menu(event) => {
                let _ = apply_event(&mut self.model, &mut self.menu, &event);
            }
            _ => {}
        }
    }

    fn view(&self) -> Element<'_, Msg, iced::Theme, Renderer> {
        column![
            enz_menu_button("Format", &self.model, &self.menu, Msg::Menu).into_element(),
            enz_text_area(&self.body, Msg::Edit)
                .fill()
                .fill_height()
                .into_element(),
        ]
        .padding(20)
        .spacing(16)
        .height(Length::Fill)
        .into()
    }
}

/// Presses the first node matching `pick`, applies what it published, and
/// returns how many nodes claim focus right after the press.
fn label(text: &'static str) -> impl Fn(&CollectedNode) -> bool {
    move |node: &CollectedNode| node.semantics.label.as_deref() == Some(text)
}

fn press(
    app: &mut impl Harness,
    renderer: &mut Renderer,
    cache: &mut Cache,
    pick: impl Fn(&CollectedNode) -> bool,
) -> usize {
    let size = Size::new(800.0, 600.0);
    let mut ui = UserInterface::build(app.view(), size, std::mem::take(cache), renderer);
    let mut bus = iced::advanced::shell::Bus::new();
    let _ = ui.update(
        &iced::advanced::graphics::core::window::Headless,
        &iced::advanced::graphics::core::shell::Waker::noop(),
        &[],
        mouse::Cursor::Unavailable,
        renderer,
        &mut bus,
    );
    let mut collector = SemanticsCollector::new();
    ui.operate(renderer, &mut collector);
    let target = collector
        .collected()
        .iter()
        .find(|node| pick(node))
        .expect("the target is published")
        .bounds;
    let at = Point::new(target.center_x(), target.center_y());
    let _ = ui.update(
        &iced::advanced::graphics::core::window::Headless,
        &iced::advanced::graphics::core::shell::Waker::noop(),
        &[
            iced::Event::Mouse(mouse::Event::CursorMoved { position: at }),
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        ],
        mouse::Cursor::Available(at),
        renderer,
        &mut bus,
    );
    let mut collector = SemanticsCollector::new();
    ui.operate(renderer, &mut collector);
    let focused = collector
        .collected()
        .iter()
        .filter(|node| node.semantics.focused)
        .count();
    let _ = ui.update(
        &iced::advanced::graphics::core::window::Headless,
        &iced::advanced::graphics::core::shell::Waker::noop(),
        &[iced::Event::Mouse(mouse::Event::ButtonReleased(
            mouse::Button::Left,
        ))],
        mouse::Cursor::Available(at),
        renderer,
        &mut bus,
    );
    *cache = ui.into_cache();
    for message in bus.messages().to_vec() {
        app.update(message);
    }
    focused
}

#[test]
fn a_menu_row_press_takes_focus_from_the_editor_and_trigger() {
    let mut app = App {
        body: text_editor::Content::with_text("Synthetic editor text"),
        model: MenuModel::new(vec![
            MenuEntryModel::command("plain", "Plain text"),
            MenuEntryModel::command("rich", "Rich text"),
        ]),
        menu: MenuState::closed(),
    };
    let mut renderer =
        iced_test::futures::futures::executor::block_on(Renderer::new(Default::default(), None))
            .expect("renderer");
    let mut cache = Cache::default();
    assert!(
        press(&mut app, &mut renderer, &mut cache, |node| {
            node.semantics.role == Role::TextInput
        }) <= 1,
        "the editor takes focus"
    );
    assert!(
        press(&mut app, &mut renderer, &mut cache, label("Format")) <= 1,
        "the trigger takes focus from the editor"
    );
    assert!(app.menu.is_open(), "the menu opened");
    let focused = press(&mut app, &mut renderer, &mut cache, label("Rich text"));
    assert!(
        focused <= 1,
        "pressing the menu row leaves one focus owner, not {focused}"
    );
}

/// The issue's public consumer: a sidebar row inside a context target, whose
/// typed menu restores focus to the row on dismissal.
struct ReopenRow {
    menu: ContextMenu<u8, bool>,
    /// A tree row restores instead of a sidebar row, as in Notes.
    tree: bool,
    removed: bool,
    pending: Vec<Box<dyn iced::advanced::widget::Operation>>,
}

impl ReopenRow {
    fn update(&mut self, message: Msg) {
        match message {
            Msg::Opened(at) => self.menu.open_restoring(
                0,
                InvocationKind::Pointer(MenuPoint::new(at.x, at.y)),
                vec![ContextMenuItem::command(true, "Remove")],
                if self.tree {
                    enzui_widgets::context_menu_host::FocusRestoreTarget::tree_row_path(
                        "notes",
                        &["queued"],
                    )
                } else {
                    enzui_widgets::context_menu_host::FocusRestoreTarget::Widget(
                        iced::advanced::widget::Id::new("row"),
                    )
                },
            ),
            Msg::Context(event) => {
                let (chosen, focus) = self.menu.update::<Msg>(&event);
                if chosen.is_some() {
                    self.removed = true;
                }
                self.pending.extend(operations(focus));
            }
            _ => {}
        }
    }

    fn view(&self) -> Element<'_, Msg, iced::Theme, Renderer> {
        let body: Element<'_, Msg, iced::Theme, Renderer> = if self.removed {
            iced::widget::text("Removed").into()
        } else {
            let row = if self.tree {
                enzui_widgets::tree_view::enz_tree_view(vec![
                    enzui_widgets::tree_view::EnzTreeItem::leaf("Queued video", 0).key("queued"),
                ])
                .id_prefix("notes")
                .into_element()
            } else {
                enzui_widgets::sidebar_row::enz_sidebar_row("Queued video", Msg::Open)
                    .id(iced::advanced::widget::Id::new("row"))
                    .into_element()
            };
            enzui_widgets::context_target::enz_context_target(row, Msg::Opened)
                .height(Length::Shrink)
                .into_element()
        };
        self.menu
            .host(
                iced::widget::container(body)
                    .padding(24)
                    .center(Length::Fill),
                Msg::Context,
            )
            .into_element()
    }
}

/// The widget operations a task runs.
fn operations(task: iced::Task<Msg>) -> Vec<Box<dyn iced::advanced::widget::Operation>> {
    let Some(mut stream) = iced_test::runtime::task::into_stream(task) else {
        return Vec::new();
    };
    let waker = iced::futures::task::noop_waker();
    let mut context = std::task::Context::from_waker(&waker);
    let mut found = Vec::new();
    while let std::task::Poll::Ready(Some(action)) =
        iced::futures::StreamExt::poll_next_unpin(&mut stream, &mut context)
    {
        if let iced_test::runtime::Action::Widget(operation) = action {
            found.push(operation);
        }
    }
    found
}

/// Runs the app's pending focus work, then `events` at `cursor`; applies
/// what they publish and returns how many nodes claimed focus after the
/// events.
fn drive(
    app: &mut ReopenRow,
    renderer: &mut Renderer,
    cache: &mut Cache,
    events: &[iced::Event],
    cursor: mouse::Cursor,
) -> usize {
    let size = Size::new(600.0, 400.0);
    let pending = std::mem::take(&mut app.pending);
    let mut ui = UserInterface::build(app.view(), size, std::mem::take(cache), renderer);
    for mut operation in pending {
        // As the runtime does: run each operation, then whatever it chains.
        loop {
            ui.operate(renderer, operation.as_mut());
            match operation.finish() {
                iced::advanced::widget::operation::Outcome::Chain(next) => operation = next,
                _ => break,
            }
        }
    }
    let mut bus = iced::advanced::shell::Bus::new();
    let _ = ui.update(
        &iced::advanced::graphics::core::window::Headless,
        &iced::advanced::graphics::core::shell::Waker::noop(),
        events,
        cursor,
        renderer,
        &mut bus,
    );
    let mut collector = SemanticsCollector::new();
    ui.operate(renderer, &mut collector);
    let focused = collector
        .collected()
        .iter()
        .filter(|node| node.semantics.focused)
        .count();
    *cache = ui.into_cache();
    let messages = bus.messages().to_vec();
    for message in messages {
        app.update(message);
    }
    focused
}

/// Where the node matching `pick` is drawn now.
fn centre_of(
    app: &ReopenRow,
    renderer: &mut Renderer,
    cache: &mut Cache,
    pick: impl Fn(&CollectedNode) -> bool,
) -> Point {
    let ui = UserInterface::build(
        app.view(),
        Size::new(600.0, 400.0),
        std::mem::take(cache),
        renderer,
    );
    let mut ui = ui;
    let mut collector = SemanticsCollector::new();
    ui.operate(renderer, &mut collector);
    let bounds = collector
        .collected()
        .iter()
        .find(|node| pick(node))
        .expect("the target is published")
        .bounds;
    *cache = ui.into_cache();
    Point::new(bounds.center_x(), bounds.center_y())
}

fn click(at: Point, button: mouse::Button) -> [iced::Event; 3] {
    [
        iced::Event::Mouse(mouse::Event::CursorMoved { position: at }),
        iced::Event::Mouse(mouse::Event::ButtonPressed(button)),
        iced::Event::Mouse(mouse::Event::ButtonReleased(button)),
    ]
}

fn key(named: iced::keyboard::key::Named) -> [iced::Event; 2] {
    let key = iced::keyboard::Key::Named(named);
    [
        iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key.clone(),
            physical_key: iced::keyboard::key::Physical::Unidentified(
                iced::keyboard::key::NativeCode::Unidentified,
            ),
            location: iced::keyboard::Location::Standard,
            modifiers: iced::keyboard::Modifiers::default(),
            text: None,
            repeat: false,
        }),
        iced::Event::Keyboard(iced::keyboard::Event::KeyReleased {
            key: key.clone(),
            modified_key: key,
            physical_key: iced::keyboard::key::Physical::Unidentified(
                iced::keyboard::key::NativeCode::Unidentified,
            ),
            location: iced::keyboard::Location::Standard,
            modifiers: iced::keyboard::Modifiers::default(),
        }),
    ]
}

/// Right click the row, Escape, right click it again and press the menu's
/// row: the restored row lets go of focus when the menu row is pressed.
fn reopen_and_press(tree: bool) {
    let mut app = ReopenRow {
        menu: ContextMenu::new(),
        tree,
        removed: false,
        pending: Vec::new(),
    };
    let mut renderer =
        iced_test::futures::futures::executor::block_on(Renderer::new(Default::default(), None))
            .expect("renderer");
    let mut cache = Cache::default();
    let row = centre_of(&app, &mut renderer, &mut cache, label("Queued video"));
    let at = mouse::Cursor::Available(row);

    let _ = drive(
        &mut app,
        &mut renderer,
        &mut cache,
        &click(row, mouse::Button::Right),
        at,
    );
    assert!(app.menu.is_open(), "right click opens the menu");
    let _ = drive(
        &mut app,
        &mut renderer,
        &mut cache,
        &key(iced::keyboard::key::Named::Escape),
        at,
    );
    assert!(!app.menu.is_open(), "Escape closes it");
    // The restore task lands on the next pass: the row holds focus.
    let focused = drive(&mut app, &mut renderer, &mut cache, &[], at);
    assert_eq!(focused, 1, "focus returns to the row");

    let _ = drive(
        &mut app,
        &mut renderer,
        &mut cache,
        &click(row, mouse::Button::Right),
        at,
    );
    assert!(app.menu.is_open(), "right click opens the menu again");
    let remove = centre_of(&app, &mut renderer, &mut cache, label("Remove"));
    let pointer = mouse::Cursor::Available(remove);
    let focused = drive(
        &mut app,
        &mut renderer,
        &mut cache,
        &[
            iced::Event::Mouse(mouse::Event::CursorMoved { position: remove }),
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        ],
        pointer,
    );
    assert!(
        focused <= 1,
        "pressing the reopened menu's row leaves one focus owner, not {focused}"
    );
    let _ = drive(
        &mut app,
        &mut renderer,
        &mut cache,
        &[iced::Event::Mouse(mouse::Event::ButtonReleased(
            mouse::Button::Left,
        ))],
        pointer,
    );
    assert!(app.removed, "Remove acts");
}

#[test]
fn reopening_a_context_menu_after_escape_leaves_one_focus_owner() {
    reopen_and_press(false);
}
