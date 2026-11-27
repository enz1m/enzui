//! A preferences row's suffix control sits inside the row before its main
//! control: a press on the suffix is the suffix's, and on a switch row any
//! other press still toggles the switch, not the suffix.

use enzui_widgets::button::enz_button;
use enzui_widgets::preferences_row::enz_preferences_row;
use iced::{Element, Point};
use iced_test::Simulator;
use iced_test::simulator::click;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Toggled(bool),
    Edit,
}

const WIDTH: f32 = 400.0;

fn row() -> Element<'static, Msg> {
    iced::widget::container(
        enz_preferences_row("Home")
            .subtitle("iCloud")
            .suffix(enz_button("Edit", Msg::Edit))
            .switch(true, Msg::Toggled),
    )
    .width(WIDTH)
    .into()
}

fn press(at: Point) -> Vec<Msg> {
    let mut ui = Simulator::with_size(iced::Settings::default(), (WIDTH, 120.0), row());
    ui.point_at(at);
    let _ = ui.simulate(click());
    ui.into_messages().collect()
}

#[test]
fn the_suffix_takes_its_own_press() {
    let mut ui = Simulator::with_size(iced::Settings::default(), (WIDTH, 120.0), row());
    let _ = ui.click("Edit").expect("the suffix is laid out");
    assert_eq!(ui.into_messages().collect::<Vec<_>>(), [Msg::Edit]);
}

#[test]
fn a_press_on_the_label_toggles_the_switch() {
    assert_eq!(press(Point::new(40.0, 20.0)), [Msg::Toggled(false)]);
}

#[test]
fn only_a_completed_touch_toggles_the_row() {
    use iced::Event;
    use iced::touch::{Event as Touch, Finger};

    let mut ui = Simulator::with_size(iced::Settings::default(), (WIDTH, 120.0), row());
    let at = Point::new(40.0, 20.0);
    let finger = Finger(7);
    for end in [
        Touch::FingerLost {
            id: finger,
            position: at,
        },
        Touch::FingerLifted {
            id: finger,
            position: Point::new(65.0, 20.0),
        },
        Touch::FingerLifted {
            id: finger,
            position: at,
        },
    ] {
        let _ = ui.simulate([
            Event::Touch(Touch::FingerPressed {
                id: finger,
                position: at,
            }),
            Event::Touch(Touch::FingerLifted {
                id: Finger(8),
                position: at,
            }),
            Event::Touch(end),
        ]);
    }
    assert_eq!(
        ui.into_messages().collect::<Vec<_>>(),
        [Msg::Toggled(false)]
    );
}
