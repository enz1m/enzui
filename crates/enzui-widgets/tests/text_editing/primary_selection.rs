//! Selected text becomes the primary selection, and a middle click pastes the
//! primary selection at the pointer: in single-line fields, in text areas,
//! never from a secure field, and not at all when the desktop turns middle-click
//! paste off.

use enzui_widgets::secure_field::enz_secure_field;
use enzui_widgets::text_area::enz_text_area;
use enzui_widgets::text_field::enz_text_field;
use iced::advanced::clipboard::{self, Content as ClipContent, Kind};
use iced::advanced::widget::{Id, Operation};
use iced::keyboard::{Key, Modifiers};
use iced::widget::text_editor::{Action, Content};
use iced::{Element, Event, Point, Size, mouse, window};
use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, State, UserInterface};

#[derive(Debug, Clone)]
enum Msg {
    Input(String),
    Edit(Action),
    Reveal,
}

/// What one update asked of the clipboard.
#[derive(Debug, Default)]
struct Requests {
    primary_write: Option<ClipContent>,
    primary_reads: Vec<Kind>,
}

struct Host {
    renderer: Renderer,
    cache: Cache,
}

impl Host {
    fn new() -> Self {
        Self {
            renderer: crate::honest_frame_driver::new_renderer(),
            cache: Cache::default(),
        }
    }

    fn frame(
        &mut self,
        element: Element<'_, Msg>,
        focus: Option<&'static str>,
        cursor: mouse::Cursor,
        events: &[Event],
    ) -> (Vec<Msg>, Requests) {
        let mut ui = UserInterface::build(
            element,
            Size::new(500.0, 160.0),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        if let Some(id) = focus {
            // Focus is a two-pass operation: follow its chain to the end.
            let mut operation: Box<dyn Operation<()>> = Box::new(
                iced::advanced::widget::operation::focusable::focus(Id::new(id)),
            );
            loop {
                ui.operate(&self.renderer, &mut operation);
                match operation.finish() {
                    iced::advanced::widget::operation::Outcome::Chain(next) => operation = next,
                    _ => break,
                }
            }
        }
        let mut messages = iced::advanced::shell::Bus::new();
        let (state, _) = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            cursor,
            &mut self.renderer,
            &mut messages,
        );
        self.cache = ui.into_cache();
        let requests = match state {
            State::Updated { clipboard, .. } => Requests {
                primary_write: clipboard.primary_write,
                primary_reads: clipboard.primary_reads,
            },
            State::Outdated => Requests::default(),
        };
        (messages.into_iter().collect(), requests)
    }
}

/// Ctrl, as a keyboard reports it before the key: text inputs read the held
/// modifiers from this event, not from the key press.
fn ctrl() -> Event {
    Event::Keyboard(iced::keyboard::Event::ModifiersChanged(Modifiers::COMMAND))
}

fn select_all() -> Event {
    Event::Keyboard(iced::keyboard::Event::KeyPressed {
        modified_key: Key::Character("a".into()),
        key: Key::Character("a".into()),
        physical_key: iced::keyboard::key::Physical::Code(iced::keyboard::key::Code::KeyA),
        location: iced::keyboard::Location::Standard,
        modifiers: Modifiers::COMMAND,
        text: None,
        repeat: false,
    })
}

fn middle_press() -> Event {
    Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Middle))
}

fn primary_reply(text: &str) -> Event {
    Event::Clipboard(clipboard::Event::ReadPrimary(Ok(std::sync::Arc::new(
        ClipContent::Text(text.into()),
    ))))
}

fn moved() -> Event {
    Event::Mouse(mouse::Event::CursorMoved {
        position: Point::new(1.0, 1.0),
    })
}

/// The middle-click setting is process-wide; every test here holds this lock
/// so the one that turns it off cannot race the others.
static SETTING: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SETTING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Over the field or area, which both start at the top-left corner.
const INSIDE: mouse::Cursor = mouse::Cursor::Available(Point { x: 40.0, y: 16.0 });

#[test]
fn text_field_offers_its_selection_and_pastes_the_primary_selection() {
    let _serial = serial();
    let mut host = Host::new();
    fn field(value: &str) -> Element<'_, Msg> {
        enz_text_field("", "", value, Msg::Input)
            .id(Id::new("field"))
            .into()
    }

    let (_, requests) = host.frame(
        field("hello"),
        Some("field"),
        INSIDE,
        &[ctrl(), select_all()],
    );
    assert_eq!(
        requests.primary_write,
        Some(ClipContent::Text("hello".into()))
    );

    // The same selection is not offered again on the next event.
    let (_, requests) = host.frame(field("hello"), None, INSIDE, &[moved()]);
    assert_eq!(requests.primary_write, None);

    let (_, requests) = host.frame(field("hello"), None, INSIDE, &[middle_press()]);
    assert_eq!(requests.primary_reads, vec![Kind::Text]);
    let (messages, _) = host.frame(field("hello"), None, INSIDE, &[primary_reply("X")]);
    let inputs: Vec<&str> = messages
        .iter()
        .filter_map(|m| match m {
            Msg::Input(value) => Some(value.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(inputs.len(), 1, "one paste: {messages:?}");
    assert!(
        inputs[0].contains('X') && inputs[0].len() == 6,
        "pasted into the value: {inputs:?}"
    );

    // A reply nobody asked for pastes nothing.
    let (messages, _) = host.frame(field("hello"), None, INSIDE, &[primary_reply("Y")]);
    assert!(messages.is_empty(), "{messages:?}");
}

#[test]
fn secure_field_never_offers_its_selection() {
    let _serial = serial();
    let mut host = Host::new();
    let element = enz_secure_field("secret", Msg::Input, false, Msg::Reveal)
        .id(Id::new("secret"))
        .into();
    let (_, requests) = host.frame(element, Some("secret"), INSIDE, &[ctrl(), select_all()]);
    assert_eq!(requests.primary_write, None);
}

#[test]
fn text_area_offers_its_selection_and_pastes_at_the_pointer() {
    let _serial = serial();
    let mut host = Host::new();
    let mut content = Content::with_text("first line\nsecond");
    let mut frame = |content: &mut Content, focus, events: &[Event]| {
        let element = enz_text_area(content, Msg::Edit).id("area").into_element();
        let (messages, requests) = host.frame(element, focus, INSIDE, events);
        for message in &messages {
            if let Msg::Edit(action) = message {
                content.perform(action.clone());
            }
        }
        (messages, requests)
    };

    frame(&mut content, Some("area"), &[select_all()]);
    // The selection the application just applied is offered on the next event.
    let (_, requests) = frame(&mut content, None, &[moved()]);
    assert_eq!(
        requests.primary_write,
        Some(ClipContent::Text("first line\nsecond".into()))
    );

    let (messages, requests) = frame(&mut content, None, &[middle_press()]);
    assert_eq!(requests.primary_reads, vec![Kind::Text]);
    assert!(
        matches!(messages.as_slice(), [Msg::Edit(Action::Click(_, _))]),
        "the caret moves to the pointer first: {messages:?}"
    );
    frame(&mut content, None, &[primary_reply("Z")]);
    let first_line = content.text().lines().next().unwrap_or_default().to_owned();
    assert!(
        first_line.contains('Z') && !first_line.starts_with('Z') && !first_line.ends_with('Z'),
        "pasted inside the first line, where the pointer is: {:?}",
        content.text()
    );
}

#[test]
fn middle_click_paste_follows_the_desktop_setting() {
    let _serial = serial();
    let mut host = Host::new();
    clipboard::set_primary_paste_enabled(false);
    let element = enz_text_field("", "", "hello", Msg::Input)
        .id(Id::new("off"))
        .into();
    let (_, requests) = host.frame(element, Some("off"), INSIDE, &[middle_press()]);
    clipboard::set_primary_paste_enabled(true);
    assert!(
        requests.primary_reads.is_empty(),
        "disabled middle-click paste must not read"
    );
}
