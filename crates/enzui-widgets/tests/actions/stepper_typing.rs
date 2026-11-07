//! A stepper's value can be typed in place: click it, type, then Enter or
//! click away; Escape cancels. Typed values clamp to the range, and an
//! uneditable stepper ignores typing.

use enzui_widgets::stepper::enz_stepper;
use iced::keyboard::key::Named;
use iced::{Element, Point};
use iced_test::Simulator;
use iced_test::simulator::click;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Changed(i32),
}

fn whole(editable: bool) -> Element<'static, Msg> {
    iced::widget::column![
        enz_stepper(7, Msg::Changed)
            .max(100)
            .editable(editable)
            .aria_label("Copies"),
        iced::widget::Space::new().height(40),
    ]
    .into()
}

/// Clicks the value cell, types `text`, then taps `end` (if any), and
/// returns what the stepper published.
fn type_into(view: Element<'static, Msg>, text: &str, end: Option<Named>) -> Vec<Msg> {
    let mut ui = Simulator::new(view);
    ui.point_at(Point::new(50.0, 16.0));
    let _ = ui.simulate(click());
    let _ = ui.typewrite(text);
    if let Some(end) = end {
        let _ = ui.tap_key(end);
    }
    ui.into_messages().collect()
}

#[test]
fn typing_replaces_the_value_and_enter_commits_it() {
    assert_eq!(
        type_into(whole(true), "42", Some(Named::Enter)),
        [Msg::Changed(42)]
    );
}

#[test]
fn escape_cancels_and_nothing_is_published() {
    assert!(type_into(whole(true), "42", Some(Named::Escape)).is_empty());
}
