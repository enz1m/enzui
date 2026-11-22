use enzui_widgets::scroll_view::{EnzScrollView, enz_scroll_view};
use iced::widget::Space;
use iced::widget::scrollable::{AbsoluteOffset, Viewport};
use iced::{Element, Length, Point, Size, mouse};
use iced_test::runtime::user_interface::{Cache, UserInterface};

const START_Y: f32 = 300.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Seen {
    y: f32,
    height: f32,
}

fn view(vertical_only: bool, viewport_height: f32) -> Element<'static, Seen> {
    let content = Space::new()
        .width(Length::Fill)
        .height(Length::Fixed(1600.0));
    let observe = |viewport: Viewport| Seen {
        y: viewport.absolute_offset().y,
        height: viewport.bounds().height,
    };
    let scroll: EnzScrollView<'static, Seen> = enz_scroll_view(content)
        .height(Length::Fixed(viewport_height))
        .initial_offset(AbsoluteOffset { x: 0.0, y: START_Y });
    if vertical_only {
        scroll.on_vertical_scroll(observe).into()
    } else {
        scroll.on_scroll(observe).into()
    }
}

fn run(vertical_only: bool, steps: &[(f32, f32, Option<iced::Event>)]) -> Vec<Vec<Seen>> {
    let mut renderer = crate::honest_frame_driver::new_renderer();
    let mut cache = Cache::default();
    let mut frames = Vec::new();
    let start = iced::time::Instant::now();
    for (index, (width, height, event)) in steps.iter().enumerate() {
        let size = Size::new(*width, *height);
        let mut ui = UserInterface::build(
            view(vertical_only, *height),
            size,
            std::mem::take(&mut cache),
            &mut renderer,
        );
        let mut events = vec![iced::Event::Window(iced::window::Event::RedrawRequested(
            start + iced::time::Duration::from_millis(100 * index as u64),
        ))];
        events.extend(event.clone());
        let mut bus = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &iced::window::Headless,
            &iced_test::core::shell::Waker::noop(),
            &events,
            mouse::Cursor::Available(Point::new(20.0, 20.0)),
            &mut renderer,
            &mut bus,
        );
        frames.push(bus.messages().to_vec());
        cache = ui.into_cache();
    }
    frames
}

fn wheel(lines: f32) -> Option<iced::Event> {
    Some(iced::Event::Mouse(mouse::Event::WheelScrolled {
        delta: mouse::ScrollDelta::Lines { x: 0.0, y: -lines },
    }))
}

#[test]
fn a_width_only_relayout_does_not_notify_a_vertical_observer() {
    let steps: Vec<_> = [600.0, 600.0, 560.0, 520.0, 480.0, 520.0, 600.0]
        .into_iter()
        .map(|width| (width, 400.0, None))
        .collect();
    let every = run(false, &steps);
    let vertical = run(true, &steps);

    let width_messages: usize = every[2..].iter().map(Vec::len).sum();
    assert!(
        width_messages >= 4,
        "on_scroll must still report width changes: {width_messages}"
    );
    assert!(
        vertical[2..].iter().all(Vec::is_empty),
        "a width-only relayout must not notify: {:?}",
        vertical
    );
}

#[test]
fn vertical_movement_and_viewport_height_still_notify() {
    let steps = [
        (600.0, 400.0, None),
        (600.0, 400.0, None),
        (580.0, 400.0, wheel(2.0)),
        (560.0, 400.0, None),
        (540.0, 400.0, None),
        (520.0, 400.0, None),
        (520.0, 360.0, None),
    ];
    let reported = run(true, &steps);

    let scrolled = reported[2..6]
        .iter()
        .flatten()
        .last()
        .expect("a wheel scroll must notify");
    assert!(
        scrolled.y > START_Y,
        "the wheel must move down: {reported:?}"
    );
    let resized = reported[6]
        .last()
        .expect("a viewport-height change must notify");
    assert_eq!(resized.height, 360.0, "{reported:?}");
}
