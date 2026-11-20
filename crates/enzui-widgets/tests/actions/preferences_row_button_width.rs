//! The row preserves the natural text width of a padded control.

use enzui_widgets::button::{ButtonKind, ButtonSize, enz_button};
use enzui_widgets::preferences_row::enz_preferences_row;
use iced::advanced::widget::{Id, Operation};
use iced::{Element, Rectangle};
use iced_test::Simulator;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Pressed,
}

const WIDTH: f32 = 692.6;

struct LabelWidth<'a> {
    label: &'a str,
    width: Option<f32>,
}

impl Operation for LabelWidth<'_> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }
    fn text(&mut self, _: Option<&Id>, bounds: Rectangle, text: &str) {
        if text == self.label {
            self.width = Some(bounds.width);
        }
    }
}

fn button(label: &'static str, size: ButtonSize) -> Element<'static, Msg> {
    enz_button(label, Msg::Pressed)
        .kind(ButtonKind::Secondary)
        .size(size)
        .into_element()
}

fn drawn_width(label: &str, view: Element<'static, Msg>) -> f32 {
    let mut ui = Simulator::with_size(iced::Settings::default(), (WIDTH, 120.0), view);
    let mut found = LabelWidth { label, width: None };
    ui.operate(&mut found);
    found.width.expect("the button's label is laid out")
}

#[test]
fn a_row_lays_its_button_out_with_its_whole_label() {
    for (label, size) in [("Rename", ButtonSize::Md), ("Remove", ButtonSize::Sm)] {
        let alone = drawn_width(label, iced::widget::row![button(label, size)].into());
        let in_row = drawn_width(
            label,
            iced::widget::container(
                enz_preferences_row("Device name")
                    .subtitle("How this computer appears to other devices on the network")
                    .control(button(label, size)),
            )
            .width(WIDTH)
            .into(),
        );
        assert!(
            in_row + 0.01 >= alone,
            "{label:?} is cut short in the row ({in_row} of {alone})"
        );
    }
}
