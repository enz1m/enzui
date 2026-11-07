//! Explicit wheel chaining hands off at edges; default scroll cards contain it.
//! Touch handoff and direction reversal use the retained iced gesture owner.

use enzui_widgets::scroll_view::enz_scroll_view;
use iced::advanced::widget::operation::scrollable::{AbsoluteOffset, Scrollable};
use iced::advanced::widget::{Id, Operation};
use iced::widget::{column, container, text};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector, mouse};

use iced_test::core::shell;
use iced_test::core::window;
use iced_test::runtime::user_interface::{Cache, UserInterface};

#[derive(Debug, Clone)]
enum Msg {}

// ---------------------------------------------------------------------------
// Harness (same pattern as scroll_view_routing.rs)
// ---------------------------------------------------------------------------

struct Host {
    renderer: iced_test::renderer::Renderer,
    cache: Cache,
}

impl Host {
    fn new() -> Self {
        let renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        Self {
            renderer,
            cache: Cache::default(),
        }
    }

    fn build<'a>(
        &mut self,
        view: Element<'a, Msg>,
        surface: Size,
    ) -> UserInterface<'a, Msg, iced::Theme, iced_test::renderer::Renderer> {
        UserInterface::build(
            view,
            surface,
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        )
    }

    fn run_op(&mut self, view: Element<'_, Msg>, surface: Size, mut op: Box<dyn Operation<()>>) {
        let mut ui = self.build(view, surface);
        ui.operate(&self.renderer, &mut *op);
        let mut outcome = op.finish();
        for _ in 0..8 {
            match outcome {
                iced::advanced::widget::operation::Outcome::Chain(mut next) => {
                    ui.operate(&self.renderer, &mut *next);
                    outcome = next.finish();
                }
                _ => break,
            }
        }
        self.cache = ui.into_cache();
    }

    fn send_events(
        &mut self,
        view: Element<'_, Msg>,
        surface: Size,
        events: &[Event],
        cursor: mouse::Cursor,
    ) -> Vec<iced::event::Status> {
        let mut ui = self.build(view, surface);
        let mut msgs = iced::advanced::shell::Bus::new();
        let (_state, statuses) = ui.update(
            &window::Headless,
            &shell::Waker::noop(),
            events,
            cursor,
            &mut self.renderer,
            &mut msgs,
        );
        self.cache = ui.into_cache();
        statuses
    }

    fn probe_translation(
        &mut self,
        view: Element<'_, Msg>,
        surface: Size,
        target: Id,
    ) -> Option<Vector> {
        struct Probe {
            target: Id,
            translation: Option<Vector>,
        }
        impl Operation<()> for Probe {
            fn scrollable(
                &mut self,
                id: Option<&Id>,
                _bounds: Rectangle,
                _content_bounds: Size,
                translation: Vector,
                _state: &mut dyn Scrollable,
            ) {
                if id == Some(&self.target) {
                    self.translation = Some(translation);
                }
            }
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<()>)) {
                operate(self);
            }
        }
        let mut probe = Probe {
            target,
            translation: None,
        };
        let mut ui = self.build(view, surface);
        ui.operate(&self.renderer, &mut probe as &mut dyn Operation<()>);
        self.cache = ui.into_cache();
        probe.translation
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const INNER_H: f32 = 200.0;
const OUTER_H: f32 = 400.0;
const VIEW_W: f32 = 300.0;
const ROW_H: f32 = 30.0;
/// How many rows to put in each scrollable (enough to overflow both).
const ROWS: usize = 40; // 40 * 30 = 1200px content

// ---------------------------------------------------------------------------
// View builders
// ---------------------------------------------------------------------------

fn inner_id() -> Id {
    Id::new("inner-scroll")
}
fn outer_id() -> Id {
    Id::new("outer-scroll")
}

/// Nested vertical layout: an inner scroll view (INNER_H) inside an outer
/// scroll view (OUTER_H). Both have more content than fits.
fn nested_vertical() -> Element<'static, Msg> {
    let mut inner_col = column![].width(Length::Fill);
    for i in 0..ROWS {
        inner_col = inner_col.push(
            container(text(format!("inner-{i}")))
                .width(Length::Fill)
                .height(Length::Fixed(ROW_H)),
        );
    }
    let inner_sv: Element<'_, Msg> = enz_scroll_view(inner_col)
        .height(Length::Fixed(INNER_H))
        .width(Length::Fill)
        .wheel_chaining(true)
        .id(inner_id())
        .into();

    let mut outer_col = column![].width(Length::Fill);
    outer_col = outer_col.push(inner_sv);
    for i in 0..ROWS {
        outer_col = outer_col.push(
            container(text(format!("outer-{i}")))
                .width(Length::Fill)
                .height(Length::Fixed(ROW_H)),
        );
    }
    enz_scroll_view(outer_col)
        .height(Length::Fixed(OUTER_H))
        .width(Length::Fixed(VIEW_W))
        .wheel_chaining(true)
        .id(outer_id())
        .into()
}

fn wheel_down() -> Vec<Event> {
    vec![Event::Mouse(mouse::Event::WheelScrolled {
        delta: mouse::ScrollDelta::Lines { x: 0.0, y: -3.0 },
    })]
}

fn surface() -> Size {
    Size::new(VIEW_W + 20.0, OUTER_H + 20.0)
}

/// Cursor positioned inside the inner scrollable.
fn cursor_in_inner() -> mouse::Cursor {
    mouse::Cursor::Available(Point::new(VIEW_W / 2.0, INNER_H / 2.0))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Wheel: nested vertical
// ---------------------------------------------------------------------------

/// Inner mid-scroll (not at an edge) → inner must consume (capture) the wheel
/// event. The outer must not scroll.
#[test]
fn wheel_mid_scroll_inner_captures() {
    let mut host = Host::new();
    let s = surface();

    // Scroll the inner to mid-content so it has room in both directions.
    use iced::advanced::widget::operation::scrollable::scroll_to;
    host.run_op(
        nested_vertical(),
        s,
        Box::new(scroll_to::<()>(
            inner_id(),
            AbsoluteOffset { x: 0.0, y: 200.0 }.into(),
            iced::advanced::widget::operation::Animation::Instant,
        )),
    );

    let inner_before = host
        .probe_translation(nested_vertical(), s, inner_id())
        .expect("inner exists");
    assert!(
        inner_before.y > 100.0,
        "inner must be mid-scroll, got {}",
        inner_before.y
    );

    // Outer at top.
    let outer_before = host
        .probe_translation(nested_vertical(), s, outer_id())
        .expect("outer exists");
    assert!(outer_before.y.abs() < 0.01, "outer must start at top");

    // Wheel down: inner has room, must consume.
    host.send_events(nested_vertical(), s, &wheel_down(), cursor_in_inner());

    let inner_after = host
        .probe_translation(nested_vertical(), s, inner_id())
        .expect("inner exists");
    assert!(
        inner_after.y > inner_before.y + 0.01,
        "inner must scroll further down, was {} now {}",
        inner_before.y,
        inner_after.y
    );

    let outer_after = host
        .probe_translation(nested_vertical(), s, outer_id())
        .expect("outer exists");
    assert!(
        outer_after.y.abs() < 0.01,
        "outer must NOT scroll when inner consumed the event, got {}",
        outer_after.y
    );
}

// ---------------------------------------------------------------------------
// Wheel: nested horizontal
// ---------------------------------------------------------------------------

// 40 * 30 = 1200px content

// ---------------------------------------------------------------------------
// Wheel: mixed-axis (inner horizontal inside outer vertical)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Touch: nested vertical
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Multiple nesting levels (three-deep)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// ScrollCard integration: real nested viewport
// ---------------------------------------------------------------------------
