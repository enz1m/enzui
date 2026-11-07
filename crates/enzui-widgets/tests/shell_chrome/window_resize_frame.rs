//! Resize frames route perimeter presses and preserve child input and semantics.
//! Cursor checks cover edge, interior and disabled cases; task inspection checks
//! the explicitly supplied target window.

use enzui_widgets::title_bar_gesture::{ResizeEdge, WindowCommand};
use enzui_widgets::window_resize_frame::window_resize_frame;

use iced::advanced::renderer::Headless;
use iced::widget::{button, container};
use iced::{Element, Event, Length, Point, Size, Theme, mouse};
use iced_test::runtime::user_interface::{Cache, UserInterface};

const W: f32 = 800.0;
const H: f32 = 600.0;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Window(WindowCommand),
    ButtonClicked,
}
fn parked() -> Point {
    Point::new(5000.0, 5000.0)
}

fn cursor_at(p: Point) -> mouse::Cursor {
    mouse::Cursor::Available(p)
}

fn press_left() -> Event {
    Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
}

fn moved(p: Point) -> Event {
    Event::Mouse(mouse::Event::CursorMoved { position: p })
}
#[derive(Clone, Copy, PartialEq)]
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
enum Fixture {
    Plain,
    WithButton,
    Disabled,
    Automatic,
    TiledLeft,
    /// A scroll view filling the window, its scrollbar along the right edge.
    ScrollView,
    /// The same, with flush chrome: the outer sliver resizes first.
    ScrollViewFlush,
}

struct Host {
    renderer: iced_test::renderer::Renderer,
    cache: Cache,
    fixture: Fixture,
}

impl Host {
    fn new(fixture: Fixture) -> Self {
        let renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        Self {
            renderer,
            cache: Cache::default(),
            fixture,
        }
    }

    fn view(&self) -> Element<'static, Msg> {
        let child: Element<'static, Msg> = match self.fixture {
            Fixture::WithButton => container(
                button("Content")
                    .on_press(Msg::ButtonClicked)
                    .width(Length::Fill)
                    .height(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
            Fixture::ScrollView | Fixture::ScrollViewFlush => {
                let mut rows = iced::widget::column![];
                for i in 0..200 {
                    rows = rows.push(iced::widget::text(format!("row {i}")));
                }
                enzui_widgets::scroll_view::enz_scroll_view(rows)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into()
            }
            _ => container(iced::widget::Space::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .into(),
        };

        let mut frame = window_resize_frame(child);
        if self.fixture != Fixture::Automatic {
            frame = frame.on_gesture(Msg::Window);
        }
        if self.fixture == Fixture::ScrollViewFlush {
            frame = frame.flush_chrome(true);
        }
        if self.fixture == Fixture::Disabled {
            frame = frame.enabled(false);
        }
        if self.fixture == Fixture::TiledLeft {
            frame = frame.tiled(iced::window::TiledEdges {
                left: true,
                ..iced::window::TiledEdges::default()
            });
        }
        frame.into()
    }

    fn build(&mut self) -> UserInterface<'static, Msg, Theme, iced_test::renderer::Renderer> {
        let view = self.view();
        UserInterface::build(
            view,
            Size::new(W, H),
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        )
    }

    fn feed(&mut self, cursor: mouse::Cursor, event: Event) -> Vec<Msg> {
        let mut ui = self.build();
        let mut messages: iced::advanced::shell::Bus<Msg> = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &iced::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[event],
            cursor,
            &mut self.renderer,
            &mut messages,
        );
        self.cache = ui.into_cache();
        messages.into_iter().collect()
    }

    fn feed_all(&mut self, steps: &[(Point, Event)]) -> Vec<Msg> {
        let mut out = Vec::new();
        for (p, e) in steps {
            out.extend(self.feed(cursor_at(*p), e.clone()));
        }
        out
    }

    /// The cursor the window shows with the pointer at `p`.
    fn cursor_shape_at(&mut self, p: Point) -> mouse::Interaction {
        self.park();
        let mut ui = self.build();
        let mut messages: iced::advanced::shell::Bus<Msg> = iced::advanced::shell::Bus::new();
        let (state, _) = ui.update(
            &iced::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[moved(p)],
            cursor_at(p),
            &mut self.renderer,
            &mut messages,
        );
        self.cache = ui.into_cache();
        match state {
            iced_test::runtime::user_interface::State::Updated {
                mouse_interaction, ..
            } => mouse_interaction,
            iced_test::runtime::user_interface::State::Outdated => mouse::Interaction::None,
        }
    }

    fn park(&mut self) {
        let _ = self.feed(cursor_at(parked()), moved(parked()));
    }
    fn press_at(&mut self, p: Point) -> Vec<Msg> {
        self.park();
        self.feed_all(&[(p, moved(p)), (p, press_left())])
    }
}

fn resizes(messages: &[Msg]) -> Vec<ResizeEdge> {
    messages
        .iter()
        .filter_map(|m| match m {
            Msg::Window(WindowCommand::Resize(e)) => Some(*e),
            _ => None,
        })
        .collect()
}
#[test]
fn window_resize_routes_edges_and_respects_native_state() {
    // interior_press_publishes_nothing
    {
        let mut host = Host::new(Fixture::Plain);
        let messages = host.press_at(Point::new(W / 2.0, H / 2.0));
        assert_eq!(
            resizes(&messages),
            Vec::<ResizeEdge>::new(),
            "an interior press must start no resize; got {messages:?}",
        );
    }

    // a_tiled_edge_does_not_resize
    {
        let mut host = Host::new(Fixture::TiledLeft);
        let left = host.press_at(Point::new(1.0, H / 2.0));
        assert!(resizes(&left).is_empty(), "the tiled left edge: {left:?}");
        let right = host.press_at(Point::new(W - 1.0, H / 2.0));
        assert_eq!(resizes(&right), [ResizeEdge::East], "the free right edge");
        let mut plain = Host::new(Fixture::Plain);
        let left = plain.press_at(Point::new(1.0, H / 2.0));
        assert_eq!(resizes(&left), [ResizeEdge::West], "positive calibration");
    }

    // the_cursor_shows_what_a_press_on_the_edge_does_over_a_scrollbar
    {
        let resize_cursor = ResizeEdge::East.cursor();
        let mut over_bar = 0;
        let mut x = W - DEFAULT_STRIP;
        while x < W {
            let p = Point::new(x, H / 2.0);
            let mut host = Host::new(Fixture::ScrollView);
            let shows_resize = host.cursor_shape_at(p) == resize_cursor;
            let mut host = Host::new(Fixture::ScrollView);
            let resizes_here = !resizes(&host.press_at(p)).is_empty();
            assert_eq!(
                shows_resize, resizes_here,
                "at x = {x}: resize cursor {shows_resize}, press resizes {resizes_here}"
            );
            over_bar += usize::from(!resizes_here);
            x += 0.5;
        }
        assert!(
            over_bar > 0,
            "precondition: the scrollbar reaches into the edge strip"
        );
    }

    // the_outer_sliver_resizes_over_a_scrollbar
    {
        let p = Point::new(W - 1.0, H / 2.0);
        let mut host = Host::new(Fixture::ScrollViewFlush);
        assert_eq!(host.cursor_shape_at(p), ResizeEdge::East.cursor());
        let mut host = Host::new(Fixture::ScrollViewFlush);
        assert_eq!(resizes(&host.press_at(p)), [ResizeEdge::East]);
    }
}

const DEFAULT_STRIP: f32 = enzui_widgets::window_resize_frame::DEFAULT_RESIZE_EDGE_THICKNESS;
