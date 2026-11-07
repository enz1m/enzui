//! A text area's host hooks: key bindings that also carry the edit menu's
//! commands, drag notifications and a line-number gutter.

use enzui_core::collect::{CollectedNode, SemanticsCollector};
use enzui_core::semantics::Role;
use enzui_widgets::text_area::enz_text_area;
use iced::advanced::widget::{Id, Operation};
use iced::keyboard::{Key, Modifiers, key::Code, key::Physical};
use iced::widget::text_editor::{Action, Binding, Content, KeyPress};
use iced::{Element, Event, Point, Size, mouse, window};
use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, State, UserInterface};

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Edit(Action),
    HostCopy,
    HostSelectAll,
    Dragged,
}

#[derive(Default, Clone, Copy)]
struct Hooks {
    keys: bool,
    drag: bool,
    gutter: bool,
}

struct Host {
    renderer: Renderer,
    cache: Cache,
    content: Content,
    hooks: Hooks,
}

struct Frame {
    messages: Vec<Msg>,
    clipboard_write: bool,
    menu_items: Vec<CollectedNode>,
}

/// The host's document operations: Copy and Select All go to the host.
fn host_keys(kp: KeyPress) -> Option<Binding<Msg>> {
    if !kp.modifiers.command() {
        return None;
    }
    match kp.key.as_ref() {
        Key::Character("c") => Some(Binding::Custom(Msg::HostCopy)),
        Key::Character("a") => Some(Binding::Custom(Msg::HostSelectAll)),
        _ => None,
    }
}

impl Host {
    fn new(text: &str, hooks: Hooks) -> Self {
        Self {
            renderer: crate::honest_frame_driver::new_renderer(),
            cache: Cache::default(),
            content: Content::with_text(text),
            hooks,
        }
    }

    fn frame(&mut self, cursor: mouse::Cursor, events: &[Event]) -> Frame {
        let mut area = enz_text_area(&self.content, Msg::Edit)
            .id(Id::new("area"))
            .area_height(200.0);
        if self.hooks.keys {
            area = area.key_binding(host_keys);
        }
        if self.hooks.drag {
            area = area.on_drag_update(|_| Msg::Dragged);
        }
        if self.hooks.gutter {
            area = area.gutter(iced::advanced::text::editor::gutter::Gutter::new(60.0));
        }
        let element: Element<'_, Msg> = area.into_element();
        let mut ui = UserInterface::build(
            element,
            Size::new(500.0, 300.0),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        let mut bus = iced::advanced::shell::Bus::new();
        let (state, _) = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            cursor,
            &mut self.renderer,
            &mut bus,
        );
        let clipboard_write = match state {
            State::Updated { clipboard, .. } => clipboard.write.is_some(),
            State::Outdated => false,
        };
        let mut collector = SemanticsCollector::new();
        ui.operate(&self.renderer, &mut collector as &mut dyn Operation);
        self.cache = ui.into_cache();
        let messages: Vec<Msg> = bus.into_iter().collect();
        for message in &messages {
            if let Msg::Edit(action) = message {
                self.content.perform(action.clone());
            }
        }
        Frame {
            messages,
            clipboard_write,
            menu_items: collector
                .into_collected()
                .into_iter()
                .filter(|node| node.semantics.role == Role::MenuItem)
                .collect(),
        }
    }
}

const AREA: mouse::Cursor = mouse::Cursor::Available(Point { x: 200.0, y: 40.0 });

fn press(button: mouse::Button) -> Event {
    Event::Mouse(mouse::Event::ButtonPressed(button))
}

fn release(button: mouse::Button) -> Event {
    Event::Mouse(mouse::Event::ButtonReleased(button))
}

fn moved(to: Point) -> Event {
    Event::Mouse(mouse::Event::CursorMoved { position: to })
}

fn key(c: char, modifiers: Modifiers) -> Event {
    let code = match c {
        'a' => Code::KeyA,
        'c' => Code::KeyC,
        _ => Code::KeyX,
    };
    Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: Key::Character(c.to_string().into()),
        modified_key: Key::Character(c.to_string().into()),
        physical_key: Physical::Code(code),
        location: iced::keyboard::Location::Standard,
        modifiers,
        text: (!modifiers.command()).then(|| c.to_string().into()),
        repeat: false,
    })
}

fn focus(host: &mut Host) {
    let _ = host.frame(
        AREA,
        &[press(mouse::Button::Left), release(mouse::Button::Left)],
    );
}

#[test]
fn host_key_bindings_take_copy_and_select_all_and_leave_typing_to_the_field() {
    let mut host = Host::new(
        "hello",
        Hooks {
            keys: true,
            ..Hooks::default()
        },
    );
    focus(&mut host);
    let frame = host.frame(
        AREA,
        &[
            Event::Keyboard(iced::keyboard::Event::ModifiersChanged(Modifiers::CTRL)),
            key('c', Modifiers::CTRL),
            key('a', Modifiers::CTRL),
        ],
    );
    assert_eq!(frame.messages, [Msg::HostCopy, Msg::HostSelectAll]);
    assert!(!frame.clipboard_write, "the field did not copy on its own");

    let _ = host.frame(
        AREA,
        &[
            Event::Keyboard(iced::keyboard::Event::ModifiersChanged(Modifiers::empty())),
            key('x', Modifiers::empty()),
        ],
    );
    assert!(host.content.text().contains('x'), "typing still edits");
}

#[test]
fn the_edit_menu_carries_its_commands_through_the_host_bindings() {
    let mut host = Host::new(
        "hello",
        Hooks {
            keys: true,
            ..Hooks::default()
        },
    );
    focus(&mut host);
    let menu = host.frame(
        AREA,
        &[press(mouse::Button::Right), release(mouse::Button::Right)],
    );
    let select_all = menu
        .menu_items
        .iter()
        .find(|node| node.semantics.label.as_deref() == Some("Select All"))
        .expect("a Select All entry")
        .bounds
        .center();
    let chosen = host.frame(
        mouse::Cursor::Available(select_all),
        &[press(mouse::Button::Left), release(mouse::Button::Left)],
    );
    assert!(
        chosen.messages.contains(&Msg::HostSelectAll),
        "{:?}",
        chosen.messages
    );
}

#[test]
fn drag_steps_notify_the_host_instead_of_publishing_drag_actions() {
    let mut host = Host::new(
        "a line of text to select",
        Hooks {
            drag: true,
            ..Hooks::default()
        },
    );
    let start = Point::new(30.0, 40.0);
    let end = Point::new(200.0, 40.0);
    let _ = host.frame(
        mouse::Cursor::Available(start),
        &[moved(start), press(mouse::Button::Left)],
    );
    // The editor performs the drag itself, at the next redraw or release, and
    // tells the host about it.
    let frame = host.frame(
        mouse::Cursor::Available(end),
        &[moved(end), release(mouse::Button::Left)],
    );
    assert!(
        frame.messages.contains(&Msg::Dragged),
        "{:?}",
        frame.messages
    );
    assert!(
        !frame
            .messages
            .iter()
            .any(|m| matches!(m, Msg::Edit(Action::Drag(_)))),
        "{:?}",
        frame.messages
    );
}

#[test]
fn a_gutter_moves_the_text_right_of_its_numbers() {
    // The same press lands in the text this much further left with a gutter.
    let click_x = |gutter: bool| {
        let mut host = Host::new(
            "abcdefghijklmnopqrstuvwxyz",
            Hooks {
                gutter,
                ..Hooks::default()
            },
        );
        let at = Point::new(70.0, 40.0);
        let frame = host.frame(
            mouse::Cursor::Available(at),
            &[
                moved(at),
                press(mouse::Button::Left),
                release(mouse::Button::Left),
            ],
        );
        frame
            .messages
            .iter()
            .find_map(|message| match message {
                Msg::Edit(Action::Click(point, _)) => Some(point.x),
                _ => None,
            })
            .expect("the press reached the text")
    };
    let (plain, with_gutter) = (click_x(false), click_x(true));
    assert_eq!(plain - with_gutter, 60.0, "{plain} vs {with_gutter}");
}
