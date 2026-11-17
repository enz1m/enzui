//! `EnzTextField::on_blur`: a click away from a focused field publishes the
//! blur message, so an in-place editor settles without Enter; Escape, the
//! cancel key, does not, and an unfocused field never does.

use enzui_widgets::text_field::enz_text_field;
use iced::widget::{Space, column};
use iced::{Element, Length, keyboard};
use iced_test::simulator::{click, simulator};

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Changed(String),
    Blurred,
}

/// A field across the top 32 px, then empty space to click into.
fn view(read_only: bool) -> Element<'static, Msg> {
    column![
        enz_text_field("", "Name", "draft", Msg::Changed)
            .on_blur(Msg::Blurred)
            .read_only(read_only),
        Space::new().height(Length::Fixed(200.0)),
    ]
    .into()
}

fn blurs(read_only: bool, after_focus: impl FnOnce(&mut iced_test::Simulator<'_, Msg>)) -> usize {
    let mut ui = simulator(view(read_only));
    ui.point_at((60.0, 16.0));
    let _ = ui.simulate(click());
    after_focus(&mut ui);
    ui.into_messages().filter(|m| *m == Msg::Blurred).count()
}

#[test]
fn a_click_elsewhere_publishes_the_blur() {
    let count = blurs(false, |ui| {
        ui.point_at((60.0, 150.0));
        let _ = ui.simulate(click());
    });
    assert_eq!(count, 1);
}

#[test]
fn a_click_inside_the_focused_field_does_not() {
    let count = blurs(false, |ui| {
        ui.point_at((90.0, 16.0));
        let _ = ui.simulate(click());
    });
    assert_eq!(count, 0);
}

#[test]
fn escape_cancels_rather_than_blurs() {
    let count = blurs(false, |ui| {
        let _ = ui.tap_key(keyboard::key::Named::Escape);
    });
    assert_eq!(count, 0);
}

#[test]
fn an_unfocused_field_never_blurs() {
    let mut ui = simulator(view(false));
    ui.point_at((60.0, 150.0));
    let _ = ui.simulate(click());
    assert_eq!(ui.into_messages().filter(|m| *m == Msg::Blurred).count(), 0);
}

#[test]
fn a_read_only_field_does_not_blur_commit() {
    let count = blurs(true, |ui| {
        ui.point_at((60.0, 150.0));
        let _ = ui.simulate(click());
    });
    assert_eq!(count, 0);
}

#[test]
fn a_focus_operation_publishes_one_blur() {
    use iced::advanced::widget::{Id, operation::focusable};
    let first = Id::new("blur-first");
    let second = Id::new("blur-second");
    let view = column![
        enz_text_field("", "First", "draft", Msg::Changed)
            .id(first.clone())
            .on_blur(Msg::Blurred),
        enz_text_field("", "Second", "", Msg::Changed).id(second.clone()),
    ];
    let mut ui = simulator(view);
    ui.operate(&mut focusable::focus::<()>(first));
    ui.operate(&mut focusable::focus::<()>(second));
    for _ in 0..2 {
        let _ = ui.simulate([iced::Event::Window(
            iced::window::Event::RedrawRequested(iced::time::Instant::now()),
        )]);
    }
    assert_eq!(ui.into_messages().filter(|m| *m == Msg::Blurred).count(), 1);
}
