//! A range slider: two named thumbs, each a Tab stop with its own keys, that
//! never cross; a press moves the nearer thumb.

use enzui_core::collect::SemanticsCollector;
use enzui_core::placement::Direction;
use enzui_core::semantics::Role;
use enzui_widgets::range_slider::{THUMB_WIDTH, enz_range_slider};
use iced::advanced::widget::Operation;
use iced::advanced::widget::operation::{Outcome, focusable};
use iced::keyboard::key::Named;
use iced::{Element, Event, Point, Size, mouse};
use iced_test::Simulator;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Range((f32, f32)),
}

const WIDTH: f32 = 300.0;

fn view(values: (f32, f32), direction: Direction) -> Element<'static, Msg> {
    enz_range_slider(0.0..=100.0, values, Msg::Range)
        .step(5.0)
        .label("Price")
        .value_format(|value| format!("€{value:.0}"))
        .direction(direction)
        .into_element()
}

fn simulator(values: (f32, f32), direction: Direction) -> Simulator<'static, Msg> {
    Simulator::with_size(
        Default::default(),
        Size::new(WIDTH, 60.0),
        view(values, direction),
    )
}

/// The x of `value` on the track, left to right.
fn x_of(value: f32) -> f32 {
    THUMB_WIDTH / 2.0 + value / 100.0 * (WIDTH - THUMB_WIDTH)
}

fn run(ui: &mut Simulator<'_, Msg>, mut operation: Box<dyn Operation>) {
    loop {
        ui.operate(operation.as_mut());
        match operation.finish() {
            Outcome::Chain(next) => operation = next,
            _ => break,
        }
    }
}

#[test]
fn range_slider_keeps_thumbs_ordered_for_pointer_keyboard_and_rtl() {
    // each_thumb_is_a_named_slider_limited_by_the_other
    {
        let mut ui = simulator((20.0, 60.0), Direction::LeftToRight);
        let mut collector = SemanticsCollector::new();
        ui.operate(&mut collector);
        let sliders: Vec<_> = collector
            .collected()
            .iter()
            .filter(|node| node.semantics.role == Role::Slider)
            .map(|node| {
                let numeric = node.semantics.numeric.expect("numeric");
                (
                    node.semantics
                        .label
                        .clone()
                        .unwrap_or_default()
                        .replace(['\u{2068}', '\u{2069}'], ""),
                    node.semantics.value.clone().unwrap_or_default(),
                    (numeric.value, numeric.min, numeric.max),
                )
            })
            .collect();
        assert_eq!(
            sliders,
            [
                (
                    "Price minimum".to_owned(),
                    "€20".to_owned(),
                    (20.0, 0.0, 60.0)
                ),
                (
                    "Price maximum".to_owned(),
                    "€60".to_owned(),
                    (60.0, 20.0, 100.0)
                ),
            ]
        );
    }

    // tab_reaches_each_thumb_and_arrows_stop_at_the_other
    {
        let keys = |values: (f32, f32), tabs: usize, named: Named| {
            let mut ui = simulator(values, Direction::LeftToRight);
            for _ in 0..tabs {
                run(&mut ui, Box::new(focusable::focus_next()));
            }
            let _ = ui.tap_key(named);
            ui.into_messages().collect::<Vec<_>>()
        };
        assert_eq!(
            keys((20.0, 60.0), 1, Named::ArrowRight),
            [Msg::Range((25.0, 60.0))],
            "the first stop is the low thumb"
        );
        assert_eq!(
            keys((20.0, 60.0), 2, Named::ArrowLeft),
            [Msg::Range((20.0, 55.0))],
            "the second stop is the high thumb"
        );
        assert_eq!(
            keys((60.0, 60.0), 1, Named::ArrowRight),
            [],
            "the low thumb cannot pass the high one"
        );
        assert_eq!(
            keys((20.0, 60.0), 1, Named::End),
            [Msg::Range((60.0, 60.0))],
            "End takes the low thumb up to the high one"
        );
        assert_eq!(
            keys((20.0, 60.0), 2, Named::Home),
            [Msg::Range((20.0, 20.0))],
            "Home takes the high thumb down to the low one"
        );
    }

    // a_press_moves_the_nearer_thumb_and_a_drag_stops_at_the_other
    {
        assert_eq!(
            press_drag((20.0, 60.0), Direction::LeftToRight, x_of(80.0), x_of(90.0)),
            [Msg::Range((20.0, 80.0)), Msg::Range((20.0, 90.0))],
            "nearer the high thumb"
        );
        assert_eq!(
            press_drag((20.0, 60.0), Direction::LeftToRight, x_of(30.0), x_of(95.0)),
            [Msg::Range((30.0, 60.0)), Msg::Range((60.0, 60.0))],
            "the low thumb drags no further than the high one"
        );
    }

    // right_to_left_puts_the_minimum_on_the_right
    {
        assert_eq!(
            press_drag(
                (20.0, 60.0),
                Direction::RightToLeft,
                x_of(100.0),
                x_of(100.0)
            ),
            [Msg::Range((0.0, 60.0))],
            "the right end is the minimum"
        );
        let mut ui = simulator((20.0, 60.0), Direction::RightToLeft);
        run(&mut ui, Box::new(focusable::focus_next()));
        let _ = ui.tap_key(Named::ArrowLeft);
        assert_eq!(
            ui.into_messages().collect::<Vec<_>>(),
            [Msg::Range((25.0, 60.0))],
            "Left increases in right-to-left text"
        );
    }
}

fn press_drag(values: (f32, f32), direction: Direction, from: f32, to: f32) -> Vec<Msg> {
    let mut ui = simulator(values, direction);
    ui.point_at(Point::new(from, 12.0));
    let _ = ui.simulate([
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        Event::Mouse(mouse::Event::CursorMoved {
            position: Point::new(to, 12.0),
        }),
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
    ]);
    ui.into_messages().collect()
}
