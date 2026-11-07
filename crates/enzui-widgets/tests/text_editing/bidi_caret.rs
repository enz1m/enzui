//! Arrow keys move the caret on screen, not in storage order, through
//! right-to-left text in a single-line field.

use enzui_widgets::text_field::enz_text_field;
use iced::keyboard::{
    Event as KeyEvent, Key, Location, Modifiers, key::Code, key::Named, key::Physical,
};
use iced::{Element, Event, Point, Size, mouse, window};
use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, UserInterface};

#[derive(Debug, Clone)]
enum Msg {
    Input(String),
}

struct Host {
    renderer: Renderer,
    cache: Cache,
    value: String,
}

const FIELD: mouse::Cursor = mouse::Cursor::Available(Point { x: 40.0, y: 16.0 });

impl Host {
    fn new(value: &str) -> Self {
        Self {
            renderer: crate::honest_frame_driver::new_renderer(),
            cache: Cache::default(),
            value: value.to_owned(),
        }
    }

    fn frame(&mut self, events: &[Event]) {
        let value = self.value.clone();
        let element: Element<'_, Msg> = enz_text_field("", "", &value, Msg::Input).into();
        let mut ui = UserInterface::build(
            element,
            Size::new(400.0, 100.0),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        let mut messages = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            FIELD,
            &mut self.renderer,
            &mut messages,
        );
        self.cache = ui.into_cache();
        for Msg::Input(value) in messages {
            self.value = value;
        }
    }

    fn press(&mut self, named: Named, code: Code) {
        self.frame(&[Event::Keyboard(KeyEvent::KeyPressed {
            key: Key::Named(named),
            modified_key: Key::Named(named),
            physical_key: Physical::Code(code),
            location: Location::Standard,
            modifiers: Modifiers::empty(),
            text: None,
            repeat: false,
        })]);
    }

    fn type_x(&mut self) {
        self.frame(&[Event::Keyboard(KeyEvent::KeyPressed {
            key: Key::Character("x".into()),
            modified_key: Key::Character("x".into()),
            physical_key: Physical::Code(Code::KeyX),
            location: Location::Standard,
            modifiers: Modifiers::empty(),
            text: Some("x".into()),
            repeat: false,
        })]);
    }

    fn focus(&mut self) {
        self.frame(&[
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        ]);
    }
}

#[test]
fn arrows_follow_the_screen_through_right_to_left_text() {
    // Hebrew runs right to left: its first letter is drawn rightmost, so Left
    // from the start moves one letter further into the word.
    let mut host = Host::new("שלום");
    host.focus();
    host.press(Named::Home, Code::Home);
    host.press(Named::ArrowLeft, Code::ArrowLeft);
    host.type_x();
    assert_eq!(host.value, "שxלום");

    // Right from the logical end (the left edge) steps back into the word.
    let mut host = Host::new("שלום");
    host.focus();
    host.press(Named::End, Code::End);
    host.press(Named::ArrowRight, Code::ArrowRight);
    host.type_x();
    assert_eq!(host.value, "שלוxם");

    // Latin text keeps logical movement.
    let mut host = Host::new("abc");
    host.focus();
    host.press(Named::Home, Code::Home);
    host.press(Named::ArrowRight, Code::ArrowRight);
    host.type_x();
    assert_eq!(host.value, "axbc");
}
