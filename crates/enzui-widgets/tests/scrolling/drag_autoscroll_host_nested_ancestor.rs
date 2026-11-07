//! `EnzDragAutoScrollHost` under a scrolling ancestor.
//!
//! An outer 200×100 `scrollable` holds a 60 px spacer above the host, so its
//! content is 160 px and it scrolls exactly 60. With the outer scrolled to 60
//! the host occupies content-space y 60–160 while appearing on screen at y
//! 0–100, so every window y here is 60 less than the content y the host
//! reasons in.

use enzui_core::drag_autoscroll::AutoScroll;
use enzui_widgets::drag_autoscroll_host::enz_drag_autoscroll_host;

use iced::advanced::widget::operation::scrollable::{AbsoluteOffset, Scrollable};
use iced::advanced::widget::{Id, Operation};
use iced::mouse;
use iced::time::{Duration, Instant};
use iced::widget::{column, container, scrollable, text};
use iced::window;
use iced::{Element, Event, Length, Point, Rectangle, Size, Theme, Vector};

use iced_test::runtime::user_interface::{Cache, UserInterface};

const SURFACE_W: f32 = 200.0;
const SURFACE_H: f32 = 100.0;
// The spacer above the host, and therefore the exact distance the outer
// scrollable can travel.
const ANCESTOR_SPACER_H: f32 = 60.0;
const ANCESTOR_OFFSET: f32 = 60.0;

fn inner_scroll_id() -> Id {
    Id::new("host-scroll")
}

fn outer_scroll_id() -> Id {
    Id::new("page-scroll")
}

// 20 px edge zone, 400 content-px/s at the edge.
fn standard_policy() -> AutoScroll {
    AutoScroll::new(20.0, 400.0)
}

// Outer scrollable → spacer + autoscroll host → inner scrollable of 30 rows.
struct Nested {
    renderer: iced_test::renderer::Renderer,
    cache: Cache,
}

impl Nested {
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

    fn view(&self) -> Element<'static, ()> {
        let mut rows = column![].width(Length::Fill);
        for index in 0..30 {
            rows = rows.push(
                container(text(format!("row-{index}")))
                    .width(Length::Fill)
                    .height(Length::Fixed(50.0)),
            );
        }
        let inner = scrollable(rows)
            .id(inner_scroll_id())
            .width(Length::Fixed(SURFACE_W))
            .height(Length::Fixed(SURFACE_H));
        let host = enz_drag_autoscroll_host(inner, inner_scroll_id(), standard_policy())
            .width(Length::Fixed(SURFACE_W))
            .height(Length::Fixed(SURFACE_H))
            .into_element();

        scrollable(column![
            container(text("page header"))
                .width(Length::Fill)
                .height(Length::Fixed(ANCESTOR_SPACER_H)),
            host,
        ])
        .id(outer_scroll_id())
        .width(Length::Fixed(SURFACE_W))
        .height(Length::Fixed(SURFACE_H))
        .into()
    }

    fn build(&mut self) -> UserInterface<'static, (), Theme, iced_test::renderer::Renderer> {
        let view = self.view();
        UserInterface::build(
            view,
            Size::new(SURFACE_W, SURFACE_H),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        )
    }

    fn feed(&mut self, cursor: mouse::Cursor, event: Event) {
        let mut ui = self.build();
        let mut messages: iced::advanced::shell::Bus<()> = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[event],
            cursor,
            &mut self.renderer,
            &mut messages,
        );
        self.cache = ui.into_cache();
    }

    // Scrolls the ancestor without the nested scrollable claiming a wheel
    // event first.
    fn scroll_ancestor_to(&mut self, y: f32) {
        let mut op = iced::advanced::widget::operation::scrollable::scroll_to::<()>(
            outer_scroll_id(),
            AbsoluteOffset { x: 0.0, y }.into(),
            iced::advanced::widget::operation::Animation::Instant,
        );
        let mut ui = self.build();
        ui.operate(&self.renderer, &mut op as &mut dyn Operation<()>);
        self.cache = ui.into_cache();
        // Present the frame the runtime draws after the scroll: scrollables
        // commit the translation pointer events are mapped through only then.
        self.feed(
            mouse::Cursor::Unavailable,
            Event::Window(window::Event::RedrawRequested(Instant::now())),
        );
    }

    // The live translation of one scrollable in the tree, read through a probe
    // `Operation` rather than any stored field.
    fn observed_translation_y(&mut self, target: Id) -> f32 {
        struct Probe {
            target: Id,
            y: Option<f32>,
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
                    self.y = Some(translation.y);
                }
            }
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<()>)) {
                operate(self);
            }
        }
        let mut probe = Probe { target, y: None };
        let mut ui = self.build();
        ui.operate(&self.renderer, &mut probe as &mut dyn Operation<()>);
        self.cache = ui.into_cache();
        probe.y.expect("the probed scrollable must exist")
    }

    // `press_at` and `hold` are window coordinates. The travel between them
    // clears the 4 px deadband in either coordinate space.
    fn arm_drag_to(&mut self, press_at: Point, hold: Point) {
        self.feed(cursor_at(press_at), cursor_moved(press_at));
        self.feed(
            cursor_at(press_at),
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
        );
        self.feed(cursor_at(hold), cursor_moved(hold));
    }

    // The first tick primes the frame clock with a zero interval; only the
    // second can move content. The interval is explicit, so no tick depends on
    // wall-clock progress between calls.
    fn run_ticks(&mut self, cursor_position: Point, base: Instant) {
        self.feed(
            cursor_at(cursor_position),
            Event::Window(window::Event::RedrawRequested(base)),
        );
        self.feed(
            cursor_at(cursor_position),
            Event::Window(window::Event::RedrawRequested(
                base + Duration::from_millis(100),
            )),
        );
    }
}

fn cursor_at(position: Point) -> mouse::Cursor {
    mouse::Cursor::Available(position)
}

fn cursor_moved(position: Point) -> Event {
    Event::Mouse(mouse::Event::CursorMoved { position })
}

// Content-space geometry: the host is y 60–160, so its bottom zone is 140–160
// and its top zone 60–80. The window point (100, 95) is 155 in content space,
// inside the bottom zone, while the raw window y 95 is in neither zone.
#[test]
fn nested_host_scrolls_when_the_adjusted_pointer_is_in_the_edge_zone() {
    let mut rig = Nested::new();
    rig.scroll_ancestor_to(ANCESTOR_OFFSET);

    let press_at = Point::new(100.0, 50.0);
    let hold = Point::new(100.0, 95.0);
    rig.arm_drag_to(press_at, hold);
    let before = rig.observed_translation_y(inner_scroll_id());
    assert_eq!(before, 0.0, "premise: the inner list has not moved yet");

    let base = Instant::now();
    rig.run_ticks(hold, base);

    let after = rig.observed_translation_y(inner_scroll_id());
    assert!(
        after > 0.0,
        "a drag held in the host's edge zone (content y {}, host 60-160) must \
         auto-scroll the inner list; the raw window y {} is not in that zone, \
         so a payload-based host reads 0 — got {after}",
        hold.y + ANCESTOR_OFFSET,
        hold.y
    );
}
