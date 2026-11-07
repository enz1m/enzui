//! A one-time code is edited cell by cell: a click on a filled cell selects
//! its character so the next key replaces it, and a full code takes no more.

use enzui_widgets::input_otp::enz_input_otp;
use iced::{Element, Event, Point, Size, keyboard, mouse, window};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

#[derive(Debug, Clone)]
enum Msg {
    Otp(String),
}

/// The app around the field: its value, and the widget state kept between
/// rebuilds as the runtime keeps it.
struct App {
    value: String,
    cache: Cache,
    renderer: Renderer,
}

impl App {
    fn new() -> Self {
        Self {
            value: String::new(),
            cache: Cache::default(),
            renderer: iced_test::futures::futures::executor::block_on(
                <Renderer as iced::advanced::renderer::Headless>::new(
                    iced::advanced::renderer::Settings::default(),
                    None,
                ),
            )
            .expect("headless renderer"),
        }
    }

    /// One frame: build the view from the value, deliver `events` at
    /// `cursor`, apply what it published.
    fn frame(&mut self, events: &[Event], cursor: mouse::Cursor) {
        let view: Element<'_, Msg, iced::Theme, Renderer> =
            enz_input_otp(&self.value, Msg::Otp).into_element();
        let mut ui = UserInterface::build(
            view,
            Size::new(600.0, 100.0),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        let mut messages = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            cursor,
            &mut self.renderer,
            &mut messages,
        );
        self.cache = ui.into_cache();
        for Msg::Otp(value) in messages.messages().iter().cloned() {
            self.value = value;
        }
    }

    fn click_cell(&mut self, cell: usize) {
        // 40 px cells, 8 px gaps, 16 px between the groups of three.
        let left = [0.0, 48.0, 96.0, 152.0, 200.0, 248.0][cell];
        let at = Point::new(left + 20.0, 20.0);
        self.frame(
            &[
                Event::Mouse(mouse::Event::CursorMoved { position: at }),
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            ],
            mouse::Cursor::Available(at),
        );
    }

    fn type_text(&mut self, text: &str) {
        for ch in text.chars() {
            let key = keyboard::Key::Character(ch.to_string().into());
            self.frame(
                &[Event::Keyboard(keyboard::Event::KeyPressed {
                    key: key.clone(),
                    modified_key: key,
                    physical_key: keyboard::key::Physical::Unidentified(
                        keyboard::key::NativeCode::Unidentified,
                    ),
                    location: keyboard::Location::Standard,
                    modifiers: keyboard::Modifiers::empty(),
                    text: Some(ch.to_string().into()),
                    repeat: false,
                })],
                mouse::Cursor::Unavailable,
            );
        }
    }
}

#[test]
fn a_click_on_a_filled_cell_selects_it_for_replacement() {
    let mut app = App::new();
    app.click_cell(0);
    app.type_text("123456");
    assert_eq!(app.value, "123456");

    // A full code takes no more.
    app.type_text("7");
    assert_eq!(app.value, "123456");

    // The second cell, clicked, is replaced and the edit moves on.
    app.click_cell(1);
    app.type_text("9");
    assert_eq!(app.value, "193456");
    app.type_text("8");
    assert_eq!(app.value, "198456", "the next key replaces the next cell");

    // A click on an empty cell of a partial code types at the end.
    app.value = String::from("12");
    app.click_cell(4);
    app.type_text("3");
    assert_eq!(app.value, "123");
}
