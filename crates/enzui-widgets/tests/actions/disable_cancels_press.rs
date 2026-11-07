//! A press whose release arrives while its control is disabled ends there:
//! after re-enabling, the old press can neither drag a range slider nor
//! complete a stepper step.

use enzui_widgets::range_slider::enz_range_slider;
use enzui_widgets::stepper::enz_stepper;
use iced::{Element, Event, Point, Size, mouse, window};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Range((f32, f32)),
    Step(i32),
}

/// Runs one event batch per view over one retained tree, returning the
/// messages each batch published.
fn frames(steps: Vec<(Element<'static, Msg>, Vec<Event>, Point)>) -> Vec<Vec<Msg>> {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            None,
        ),
    )
    .expect("headless renderer");
    let mut cache = Cache::new();
    let mut published = Vec::new();
    for (view, events, at) in steps {
        let mut ui = UserInterface::build(view, Size::new(300.0, 60.0), cache, &mut renderer);
        let mut messages = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &events,
            mouse::Cursor::Available(at),
            &mut renderer,
            &mut messages,
        );
        published.push(messages.into_iter().collect());
        cache = ui.into_cache();
    }
    published
}

fn press(at: Point) -> Vec<Event> {
    vec![
        Event::Mouse(mouse::Event::CursorMoved { position: at }),
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
    ]
}

fn release() -> Vec<Event> {
    vec![Event::Mouse(mouse::Event::ButtonReleased(
        mouse::Button::Left,
    ))]
}

#[test]
fn a_range_slider_disabled_mid_drag_does_not_resume_it() {
    let slider = |enabled: bool| -> Element<'static, Msg> {
        enz_range_slider(0.0..=100.0, (20.0, 80.0), Msg::Range)
            .enabled(enabled)
            .into_element()
    };
    let start = Point::new(60.0, 12.0);
    let later = Point::new(200.0, 12.0);
    let published = frames(vec![
        (slider(true), press(start), start),
        (slider(false), release(), start),
        (
            slider(true),
            vec![Event::Mouse(mouse::Event::CursorMoved { position: later })],
            later,
        ),
    ]);
    assert!(
        !published[0].is_empty(),
        "the press moves a thumb: {published:?}"
    );
    assert_eq!(
        published[1..],
        [vec![], vec![]],
        "no drag after re-enabling"
    );
}

#[test]
fn a_stepper_disabled_mid_press_does_not_step_on_a_later_release() {
    let stepper = |enabled: bool| -> Element<'static, Msg> {
        enz_stepper(5, Msg::Step)
            .min(0)
            .max(10)
            .enabled(enabled)
            .into()
    };
    let minus = Point::new(14.0, 16.0);
    let published = frames(vec![
        (stepper(true), press(minus), minus),
        (stepper(false), release(), minus),
        (stepper(true), release(), minus),
    ]);
    assert_eq!(published, [vec![], vec![], vec![]], "the old press is gone");

    // Positive control: the same press and release while enabled steps.
    let published = frames(vec![
        (stepper(true), press(minus), minus),
        (stepper(true), release(), minus),
    ]);
    assert_eq!(published, [vec![], vec![Msg::Step(4)]]);
}
