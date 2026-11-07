//! Behavioral tests for EnzSlider: keyboard focus + arrow/Home/End
//! navigation and disabled gating, via the custom focusable slider widget.

use enzui_widgets::slider::enz_slider;
use iced::{Element, Point};
use iced_test::Simulator;
use iced_test::simulator::click;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Vol(f32),
}

/// The pointer row must not be *narrower* than the floor at its bottom edge
/// either: a press just below the hit row (y=26) must be ignored, so the test
/// above cannot pass merely because the row became unbounded.
#[test]
fn press_below_the_hit_row_is_ignored() {
    let view: Element<'static, Msg> = enz_slider(0.0..=100.0, 50.0, Msg::Vol).step(5.0).into();
    let mut ui = Simulator::new(view);

    ui.point_at(Point::new(80.0, 26.0));
    let _ = ui.simulate(click());

    let messages: Vec<Msg> = ui.into_messages().collect();
    assert!(
        messages.is_empty(),
        "a press below the 24px hit row must be ignored, got {messages:?}"
    );
}

/// A drag belongs to the device that started it: while a finger holds it, the
/// mouse can neither move nor take it, and the finger keeps driving it.
#[test]
fn a_finger_drag_is_not_moved_or_taken_by_the_mouse() {
    use iced::touch::{Event as Touch, Finger};
    use iced::{Event, mouse};
    let view: Element<'static, Msg> = enz_slider(0.0..=100.0, 50.0, Msg::Vol).into();
    let finger = Finger(7);
    let row = 12.0;
    let mut ui = Simulator::new(view);
    // The mouse rests on the track, so its press would land there.
    ui.point_at(Point::new(150.0, row));

    let _ = ui.simulate([
        Event::Touch(Touch::FingerPressed {
            id: finger,
            position: Point::new(40.0, row),
        }),
        Event::Mouse(mouse::Event::CursorMoved {
            position: Point::new(150.0, row),
        }),
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        Event::Mouse(mouse::Event::CursorMoved {
            position: Point::new(160.0, row),
        }),
        Event::Touch(Touch::FingerMoved {
            id: finger,
            position: Point::new(60.0, row),
        }),
    ]);
    let messages: Vec<Msg> = ui.into_messages().collect();
    assert_eq!(
        messages.len(),
        2,
        "finger press and finger move only: {messages:?}"
    );
    let [Msg::Vol(down), Msg::Vol(moved)] = messages[..] else {
        unreachable!()
    };
    assert!(down < 50.0 && moved > down && moved < 50.0, "{messages:?}");
}
