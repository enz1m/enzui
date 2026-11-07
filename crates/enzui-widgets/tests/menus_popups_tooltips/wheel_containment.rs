//! A scrolled list keeps the wheel at its end: scrolling on past the last
//! row does not go on to scroll the page around it. Popups keep every wheel
//! turned over them, whether or not their list can move.

use enzui_widgets::scroll_view::{Axis, contain_wheel, enz_scroll_view, enz_scrollable};
use iced::widget::{Space, column, container, text};
use iced::{Element, Length, Point, Size};
use iced_test::Simulator;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Page,
    List,
    Picked(String),
}

fn wheel(lines: f32) -> iced::Event {
    iced::Event::Mouse(iced::mouse::Event::WheelScrolled {
        delta: iced::mouse::ScrollDelta::Lines { x: 0.0, y: lines },
    })
}

fn page(top: Element<'static, Msg>) -> Element<'static, Msg> {
    enz_scroll_view(column![top, Space::new().height(3000)].width(Length::Fill))
        .on_scroll(|_| Msg::Page)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn list() -> Element<'static, Msg> {
    let mut rows = column![];
    for i in 0..30 {
        rows = rows.push(text(format!("row {i}")));
    }
    contain_wheel(
        enz_scrollable(container(rows).width(Length::Fill))
            .height(120)
            .on_scroll(|_| Msg::List),
        Axis::Vertical,
    )
}

#[test]
fn a_list_scrolled_to_its_end_keeps_the_wheel() {
    let mut ui = Simulator::with_size(
        iced::Settings::default(),
        Size::new(300.0, 400.0),
        page(list()),
    );
    ui.point_at(Point::new(40.0, 60.0));
    // Far past the end: the first turns take the list there, the rest find
    // it pinned.
    for _ in 0..12 {
        let _ = ui.simulate([wheel(-20.0)]);
    }
    let messages: Vec<Msg> = ui.into_messages().collect();
    assert!(
        messages.contains(&Msg::List),
        "the list scrolls: {messages:?}"
    );
    assert!(
        !messages.contains(&Msg::Page),
        "and the page around it stays put: {messages:?}"
    );
}

#[test]
fn a_popup_keeps_the_wheel_turned_over_it() {
    let combo = enzui_widgets::combo_box::enz_combo_box(
        "",
        vec!["Apple".into(), "Banana".into(), "Cherry".into()],
        None,
        Msg::Picked,
    )
    .into_element();
    let mut ui = Simulator::with_size(
        iced::Settings::default(),
        Size::new(300.0, 400.0),
        page(combo),
    );
    ui.point_at(Point::new(60.0, 16.0));
    let _ = ui.simulate([
        iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
        iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
            iced::mouse::Button::Left,
        )),
    ]);
    // Over the open list, which is too short to scroll.
    ui.point_at(Point::new(60.0, 60.0));
    let _ = ui.simulate([wheel(-3.0)]);
    let messages: Vec<Msg> = ui.into_messages().collect();
    assert!(
        !messages.contains(&Msg::Page),
        "the page under the open list stays put: {messages:?}"
    );
}
