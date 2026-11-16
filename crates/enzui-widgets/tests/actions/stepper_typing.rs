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

#[test]
fn an_uneditable_stepper_ignores_typing() {
    assert!(type_into(whole(false), "42", Some(Named::Enter)).is_empty());
}

/// Minutes shown as a clock time: the readout is not the number.
fn clock(editable: Option<bool>) -> Element<'static, Msg> {
    let stepper = enz_stepper(540, Msg::Changed)
        .max(23 * 60)
        .step(60)
        .value_text("9:00 AM")
        .aria_label("Start");
    let stepper = match editable {
        Some(e) => stepper.editable(e),
        None => stepper,
    };
    iced::widget::column![stepper, iced::widget::Space::new().height(40)].into()
}

#[test]
fn a_value_text_readout_is_not_typed_over_by_default() {
    assert!(type_into(clock(None), "600", Some(Named::Enter)).is_empty());
}

#[test]
fn a_value_text_readout_can_opt_back_into_typing() {
    assert_eq!(
        type_into(clock(Some(true)), "600", Some(Named::Enter)),
        [Msg::Changed(600)]
    );
}

/// `h:mm` read back as minutes, the inverse of the readout.
fn parse_clock(text: &str) -> Option<i32> {
    let (h, m) = text.trim().split_once(':')?;
    Some(h.trim().parse::<i32>().ok()? * 60 + m.trim().parse::<i32>().ok()?)
}

fn parsed_clock() -> Element<'static, Msg> {
    iced::widget::column![
        enz_stepper(540, Msg::Changed)
            .max(23 * 60)
            .step(60)
            .value_text("9:00")
            .parse_value_text(parse_clock)
            .aria_label("Start"),
        iced::widget::Space::new().height(40),
    ]
    .into()
}

#[test]
fn a_parsed_readout_is_typed_as_written() {
    assert_eq!(
        type_into(parsed_clock(), "10:30", Some(Named::Enter)),
        [Msg::Changed(630)]
    );
}

#[test]
fn a_parsed_readout_is_clamped_to_the_range() {
    assert_eq!(
        type_into(parsed_clock(), "23:45", Some(Named::Enter)),
        [Msg::Changed(23 * 60)]
    );
}

#[test]
fn unreadable_text_keeps_the_value() {
    assert!(type_into(parsed_clock(), "soon", Some(Named::Enter)).is_empty());
}

#[test]
fn backspace_edits_the_shown_text() {
    // The click selects "9:00"; Backspace clears it, then "8:15" is typed.
    let mut ui = Simulator::new(parsed_clock());
    ui.point_at(Point::new(50.0, 16.0));
    let _ = ui.simulate(click());
    let _ = ui.tap_key(Named::Backspace);
    let _ = ui.typewrite("8:15");
    let _ = ui.tap_key(Named::Enter);
    assert_eq!(ui.into_messages().collect::<Vec<_>>(), [Msg::Changed(495)]);
}
