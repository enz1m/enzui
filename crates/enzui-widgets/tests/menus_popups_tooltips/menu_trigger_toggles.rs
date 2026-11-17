//! Pressing the trigger of an open menu closes it, as a platform menu button
//! does: the press publishes the dismissal and never reaches the trigger, so
//! its release cannot reopen the menu. No app-side setting is involved.

use enzui_core::placement::{Placement, Side};
use enzui_widgets::anchored_overlay::dismissible_anchored_menu;
use enzui_widgets::button::enz_button;
use enzui_widgets::preferences_row::enz_preferences_row;
use iced::Element;
use iced_test::Simulator;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Open,
    Dismiss,
    Pick(usize),
}

fn menu(open: bool) -> Element<'static, Msg> {
    let popup = open.then(|| iced::widget::text("Popup body").into());
    iced::widget::container(dismissible_anchored_menu(
        enz_button("Trigger", Msg::Open),
        popup,
        Placement::new(Side::Bottom),
        || Msg::Dismiss,
    ))
    .padding(20)
    .into()
}

fn picker(open: bool) -> Element<'static, Msg> {
    iced::widget::container(enz_preferences_row("Sort by").value_menu(
        "Name",
        open,
        Msg::Open,
        Msg::Dismiss,
        vec![
            ("Name".to_owned(), Msg::Pick(0)),
            ("Size".to_owned(), Msg::Pick(1)),
        ],
    ))
    .width(400)
    .into()
}

fn click(view: Element<'static, Msg>, label: &str) -> Vec<Msg> {
    let mut ui = Simulator::with_size(iced::Settings::default(), (500.0, 400.0), view);
    let _ = ui.click(label).expect("the trigger is laid out");
    ui.into_messages().collect()
}

#[test]
fn a_closed_trigger_opens_its_menu() {
    assert_eq!(click(menu(false), "Trigger"), [Msg::Open]);
}

#[test]
fn an_open_trigger_closes_its_menu() {
    assert_eq!(click(menu(true), "Trigger"), [Msg::Dismiss]);
}

#[test]
fn a_settings_picker_closes_when_pressed_again() {
    assert_eq!(click(picker(false), "Name"), [Msg::Open]);
    assert_eq!(click(picker(true), "Name"), [Msg::Dismiss]);
}
