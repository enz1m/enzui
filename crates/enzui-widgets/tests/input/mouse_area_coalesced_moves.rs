//! Coalesced pointer moves reach the application once per presented frame,
//! with the latest position.

use iced::{Element, Event, Length, Point, mouse};
use iced_test::Simulator;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Moved(Point),
}

fn press_area() -> Element<'static, Msg> {
    iced::widget::mouse_area(
        iced::widget::Space::new()
            .width(Length::Fill)
            .height(Length::Fill),
    )
    .on_move_coalesced(Msg::Moved)
    .on_press(Msg::Moved(Point::ORIGIN))
    .into()
}

// Sabotage: flush only on frames and the press arrives before its move.
#[test]
fn a_press_is_reported_after_the_move_that_led_to_it() {
    let mut ui = Simulator::new(press_area());
    let position = Point::new(30.0, 40.0);
    ui.point_at(position);
    let _ = ui.simulate([
        Event::Mouse(mouse::Event::CursorMoved { position }),
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
    ]);
    let messages: Vec<Msg> = ui.into_messages().collect();
    assert_eq!(messages, [Msg::Moved(position), Msg::Moved(Point::ORIGIN)]);
}
