//! An animated SVG draws its frames, wakes the app only when the picture
//! changes, and stops waking when it is hidden, paused or finished.

use enzui_widgets::animated_svg::{AnimatedSvg, enz_animated_svg};
use iced::advanced::renderer::Headless;
use iced::time::{Duration, Instant};
use iced::widget::{Space, column};
use iced::{Element, Event, Point, Size, mouse, window};
use iced_test::runtime::user_interface::{Cache, State, UserInterface};

/// A square fading from red to blue over a second, then holding blue.
const FADE: &str = r#"<svg width="32" height="32" xmlns="http://www.w3.org/2000/svg"><rect width="32" height="32" fill="red"><animate attributeName="fill" from="red" to="blue" dur="1s" fill="freeze"/></rect></svg>"#;

struct Harness {
    renderer: iced_test::renderer::Renderer,
    cache: Option<Cache>,
    svg: AnimatedSvg,
    playing: bool,
    offset: f32,
}

impl Harness {
    fn new(text: &str) -> Self {
        let renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as Headless>::new(Default::default(), None),
        )
        .expect("renderer");
        Self {
            renderer,
            cache: Some(Cache::default()),
            svg: AnimatedSvg::from_text(text).expect("svg"),
            playing: true,
            offset: 0.0,
        }
    }

    fn view(&self) -> Element<'static, ()> {
        column![
            Space::new().height(self.offset),
            enz_animated_svg(&self.svg)
                .width(32)
                .height(32)
                .playing(self.playing),
        ]
        .into()
    }

    /// Deliver `events` then a frame at `at`: the redraw the frame asks for
    /// and the picture's centre colour.
    fn present_after(&mut self, events: &[Event], at: Instant) -> (window::RedrawRequest, [u8; 4]) {
        let mut ui = UserInterface::build(
            self.view(),
            Size::new(64.0, 64.0),
            self.cache.take().expect("cache"),
            &mut self.renderer,
        );
        let mut bus = iced::advanced::shell::Bus::new();
        let cursor = mouse::Cursor::Available(Point::new(16.0, 16.0));
        if !events.is_empty() {
            let _ = ui.update(
                &iced::window::Headless,
                &iced_test::core::shell::Waker::noop(),
                events,
                cursor,
                &mut self.renderer,
                &mut bus,
            );
        }
        let (state, _) = ui.update(
            &iced::window::Headless,
            &iced_test::core::shell::Waker::noop(),
            &[Event::Window(window::Event::RedrawRequested(at))],
            cursor,
            &mut self.renderer,
            &mut bus,
        );
        let request = match state {
            State::Updated { redraw_request, .. } => redraw_request,
            State::Outdated => panic!("outdated"),
        };
        let theme = enzui_theme::EnzTheme::light().iced_theme();
        ui.draw(
            &mut self.renderer,
            &theme,
            &iced::advanced::renderer::Style {
                text_color: iced::Color::BLACK,
            },
            cursor,
        );
        let pixels = self
            .renderer
            .screenshot(Size::new(64, 64), 1.0, iced::Color::WHITE);
        self.cache = Some(ui.into_cache());
        let i = (16 * 64 + 16) * 4;
        (
            request,
            [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]],
        )
    }

    fn present(&mut self, at: Instant) -> (window::RedrawRequest, [u8; 4]) {
        self.present_after(&[], at)
    }
}

fn close(a: [u8; 4], b: [u8; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) < 24)
}

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

#[test]
fn a_continuous_animation_asks_for_frames_until_it_finishes() {
    let mut harness = Harness::new(FADE);
    let start = Instant::now();
    let (request, color) = harness.present(start);
    assert!(close(color, RED), "{color:?}");
    assert_eq!(request, window::RedrawRequest::NextFrame);
    let (request, color) = harness.present(start + Duration::from_millis(500));
    assert_eq!(request, window::RedrawRequest::NextFrame);
    assert!(close(color, [128, 0, 128, 255]), "half way: {color:?}");
    let (request, color) = harness.present(start + Duration::from_millis(1200));
    assert!(close(color, BLUE), "{color:?}");
    assert_eq!(
        request,
        window::RedrawRequest::Wait,
        "finished: no more wakes"
    );
}

#[test]
fn a_hidden_or_paused_picture_does_not_wake_and_resumes_where_it_stopped() {
    let start = Instant::now();
    let mut hidden = Harness::new(FADE);
    hidden.offset = 500.0;
    assert_eq!(
        hidden.present(start).0,
        window::RedrawRequest::Wait,
        "out of view"
    );

    let mut paused = Harness::new(FADE);
    let _ = paused.present(start);
    paused.playing = false;
    let (request, held) = paused.present(start + Duration::from_millis(400));
    assert_eq!(request, window::RedrawRequest::Wait, "paused");
    let (_, still) = paused.present(start + Duration::from_millis(900));
    assert!(close(held, still), "holds its frame: {held:?} {still:?}");
    paused.playing = true;
    // After a 5 s pause it resumes at the 400 ms it had played.
    let (request, resumed) = paused.present(start + Duration::from_secs(5));
    assert!(close(resumed, held), "{resumed:?} {held:?}");
    assert_eq!(request, window::RedrawRequest::NextFrame);
    let (_, later) = paused.present(start + Duration::from_secs(5) + Duration::from_millis(100));
    assert!(
        close(later, [128, 0, 128, 255]),
        "half way at 500 ms played: {later:?}"
    );
}
