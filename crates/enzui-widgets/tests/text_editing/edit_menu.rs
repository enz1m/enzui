//! The built-in edit menu of text fields, and undo/redo in single-line fields.

use enzui_core::collect::{CollectedNode, SemanticsCollector};
use enzui_core::semantics::Role;
use enzui_widgets::text_field::enz_text_field;
use iced::advanced::clipboard::Content as ClipContent;
use iced::advanced::widget::{Id, Operation};
use iced::keyboard::{Key, Modifiers, key::Code, key::Physical};
use iced::{Element, Event, Point, Size, mouse, window};
use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, State, UserInterface};

#[derive(Debug, Clone)]
enum Msg {
    Input(String),
}

struct Host {
    renderer: Renderer,
    cache: Cache,
    value: String,
    read_only: bool,
}

/// What one frame produced.
struct Frame {
    clipboard_write: Option<ClipContent>,
    menu_items: Vec<CollectedNode>,
    /// How many nodes claim focus.
    focused: usize,
}

impl Host {
    fn new(value: &str) -> Self {
        Self {
            renderer: crate::honest_frame_driver::new_renderer(),
            cache: Cache::default(),
            value: value.to_owned(),
            read_only: false,
        }
    }

    fn frame(&mut self, cursor: mouse::Cursor, events: &[Event]) -> Frame {
        let value = self.value.clone();
        let read_only = self.read_only;
        let element: Element<'_, Msg> = enz_text_field("", "", &value, Msg::Input)
            .id(Id::new("field"))
            .read_only(read_only)
            .into();
        let mut ui = UserInterface::build(
            element,
            Size::new(500.0, 400.0),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        let mut messages = iced::advanced::shell::Bus::new();
        let (state, _) = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            cursor,
            &mut self.renderer,
            &mut messages,
        );
        let clipboard_write = match state {
            State::Updated { clipboard, .. } => clipboard.write,
            State::Outdated => None,
        };
        let mut collector = SemanticsCollector::new();
        ui.operate(&self.renderer, &mut collector as &mut dyn Operation);
        self.cache = ui.into_cache();
        for message in messages {
            let Msg::Input(value) = message;
            self.value = value;
        }
        let collected = collector.into_collected();
        let focused = collected
            .iter()
            .filter(|node| node.semantics.focused)
            .count();
        let menu_items = collected
            .into_iter()
            .filter(|node| node.semantics.role == Role::MenuItem)
            .collect();
        Frame {
            clipboard_write,
            menu_items,
            focused,
        }
    }
}

fn labels(frame: &Frame) -> Vec<String> {
    frame
        .menu_items
        .iter()
        .filter_map(|node| node.semantics.label.clone())
        .collect()
}

fn item_center(frame: &Frame, label: &str) -> Point {
    let node = frame
        .menu_items
        .iter()
        .find(|node| node.semantics.label.as_deref() == Some(label))
        .unwrap_or_else(|| panic!("no {label:?} entry in {:?}", labels(frame)));
    node.bounds.center()
}

fn right_click() -> Vec<Event> {
    vec![
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)),
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Right)),
    ]
}

fn left_click() -> Vec<Event> {
    vec![
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
    ]
}

const FIELD: mouse::Cursor = mouse::Cursor::Available(Point { x: 40.0, y: 16.0 });

fn choose(host: &mut Host, label: &str) -> Frame {
    let menu = host.frame(FIELD, &right_click());
    let entry = mouse::Cursor::Available(item_center(&menu, label));
    // The command reaches the field in the same frame as the click: no later
    // event is needed to carry it out.
    host.frame(entry, &left_click())
}

#[test]
fn right_click_opens_the_edit_menu_with_every_command() {
    let mut host = Host::new("hello world");
    let frame = host.frame(FIELD, &right_click());
    assert_eq!(
        labels(&frame),
        [
            "Undo",
            "Redo",
            "Cut",
            "Copy",
            "Paste",
            "Delete",
            "Select All"
        ],
        "the localized commands, in order"
    );

    let frame = host.frame(
        FIELD,
        &[Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: Key::Named(iced::keyboard::key::Named::Escape),
            modified_key: Key::Named(iced::keyboard::key::Named::Escape),
            physical_key: Physical::Code(Code::Escape),
            location: iced::keyboard::Location::Standard,
            modifiers: Modifiers::empty(),
            text: None,
            repeat: false,
        })],
    );
    assert!(frame.menu_items.is_empty(), "Escape closes the menu");
}

#[test]
fn select_all_then_copy_puts_the_value_on_the_clipboard() {
    let mut host = Host::new("hello world");
    choose(&mut host, "Select All");
    let frame = choose(&mut host, "Copy");
    assert_eq!(
        frame.clipboard_write,
        Some(ClipContent::Text("hello world".into()))
    );
    assert!(frame.menu_items.is_empty(), "choosing closes the menu");

    choose(&mut host, "Select All");
    choose(&mut host, "Delete");
    assert_eq!(
        host.value, "",
        "Delete removes the selection through on_input"
    );
    choose(&mut host, "Undo");
    assert_eq!(host.value, "hello world", "Undo restores it");
}

#[test]
fn a_read_only_field_offers_copy_and_select_all() {
    let mut host = Host::new("fixed");
    host.read_only = true;
    let frame = host.frame(FIELD, &right_click());
    assert_eq!(labels(&frame), ["Copy", "Select All"]);
}

fn key(c: char, modifiers: Modifiers, text: bool) -> Event {
    let code = match c {
        'z' => Code::KeyZ,
        'y' => Code::KeyY,
        _ => Code::KeyA,
    };
    Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: Key::Character(c.to_string().into()),
        modified_key: Key::Character(c.to_string().into()),
        physical_key: if text {
            Physical::Code(Code::KeyQ)
        } else {
            Physical::Code(code)
        },
        location: iced::keyboard::Location::Standard,
        modifiers,
        text: text.then(|| c.to_string().into()),
        repeat: false,
    })
}

fn modifiers(modifiers: Modifiers) -> Event {
    Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers))
}

#[test]
fn single_line_fields_undo_and_redo_typing() {
    let mut host = Host::new("");
    host.frame(FIELD, &left_click());
    host.frame(FIELD, &[key('a', Modifiers::empty(), true)]);
    host.frame(FIELD, &[key('b', Modifiers::empty(), true)]);
    assert_eq!(host.value, "ab");

    host.frame(
        FIELD,
        &[
            modifiers(Modifiers::COMMAND),
            key('z', Modifiers::COMMAND, false),
        ],
    );
    assert_eq!(host.value, "", "one typing run is one undo step");
    host.frame(
        FIELD,
        &[
            modifiers(Modifiers::COMMAND | Modifiers::SHIFT),
            key('z', Modifiers::COMMAND | Modifiers::SHIFT, false),
        ],
    );
    assert_eq!(host.value, "ab", "Ctrl+Shift+Z redoes");
    host.frame(
        FIELD,
        &[
            modifiers(Modifiers::COMMAND),
            key('z', Modifiers::COMMAND, false),
        ],
    );
    host.frame(
        FIELD,
        &[
            modifiers(Modifiers::COMMAND),
            key('y', Modifiers::COMMAND, false),
        ],
    );
    assert_eq!(host.value, "ab", "Ctrl+Y redoes too");

    // A value the application set itself is not undone into.
    host.value = String::from("replaced");
    host.frame(
        FIELD,
        &[
            modifiers(Modifiers::COMMAND),
            key('z', Modifiers::COMMAND, false),
        ],
    );
    assert_eq!(host.value, "replaced");
}

#[test]
fn pressing_a_menu_row_leaves_one_focused_node() {
    let mut host = Host::new("Unicode café");
    let _ = host.frame(FIELD, &left_click());
    let menu = host.frame(FIELD, &right_click());
    assert!(menu.focused <= 1, "with the menu open: {}", menu.focused);
    let row = mouse::Cursor::Available(item_center(&menu, "Select All"));
    let pressed = host.frame(
        row,
        &[Event::Mouse(mouse::Event::ButtonPressed(
            mouse::Button::Left,
        ))],
    );
    assert!(pressed.focused <= 1, "pressing a row: {}", pressed.focused);
    let released = host.frame(
        row,
        &[Event::Mouse(mouse::Event::ButtonReleased(
            mouse::Button::Left,
        ))],
    );
    let done = host.frame(row, &[]);
    assert!(
        released.focused <= 1 && done.focused == 1,
        "the field has focus back once the menu closes: {} then {}",
        released.focused,
        done.focused
    );
}
