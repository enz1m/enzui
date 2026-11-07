//! A write to the screen asks for a content-only frame, which any other
//! paint request in the same update turns back into a full one.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use enzui_terminal::{Cell, Rgba, TerminalScreen, enz_terminal};
use iced::advanced::renderer::Headless as _;
use iced::{Event, Point, mouse, window};
use iced_test::core::{shell, window::Headless};
use iced_test::runtime::user_interface::{Cache, State, UserInterface};

const WHITE: Rgba = Rgba::rgb(255, 255, 255);

fn content_only(screen: &TerminalScreen, events: &[Event], cursor: Point) -> bool {
    let mut renderer = iced_test::futures::futures::executor::block_on(iced::Renderer::new(
        iced::advanced::renderer::Settings::default(),
        Some("tiny-skia"),
    ))
    .expect("the software renderer is always available");
    let element: iced::Element<'_, ()> = iced::widget::column![
        enz_terminal(screen).height(150),
        iced::widget::button("chrome").on_press(()),
    ]
    .into();
    let mut ui = UserInterface::build(
        element,
        iced::Size::new(300.0, 200.0),
        Cache::default(),
        &mut renderer,
    );
    let mut messages = shell::Bus::default();
    let mut update = |events: &[Event], cursor: Point| {
        ui.update(
            &Headless,
            &shell::Waker::noop(),
            events,
            mouse::Cursor::Available(cursor),
            &mut renderer,
            &mut messages,
        )
        .0
    };
    // The first event registers the view with the screen, which wakes it
    // from then on.
    // The pointer starts over the terminal.
    let _ = update(
        &[Event::Window(window::Event::RedrawRequested(
            iced::time::Instant::now(),
        ))],
        Point::new(5.0, 5.0),
    );
    screen.update(|w| w.set_cell(0, 0, Cell::new('x', WHITE)));
    match update(events, cursor) {
        State::Updated {
            is_content_only, ..
        } => is_content_only,
        State::Outdated => false,
    }
}

#[test]
fn a_screen_write_alone_asks_for_a_content_only_frame() {
    let screen = TerminalScreen::new(10, 2, WHITE);
    assert!(content_only(&screen, &[Event::Waken], Point::new(5.0, 5.0)));
}

#[test]
fn a_hover_in_the_same_update_asks_for_a_full_frame() {
    let screen = TerminalScreen::new(10, 2, WHITE);
    let over_button = Point::new(10.0, 165.0);
    assert!(!content_only(
        &screen,
        &[
            Event::Waken,
            Event::Mouse(mouse::Event::CursorMoved {
                position: over_button
            }),
        ],
        over_button,
    ));
}
