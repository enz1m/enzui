//! An animated image plays its frames on their delays, and stops waking
//! when it is hidden, paused or asked for reduced motion.

use enzui_core::collect::SemanticsCollector;
use enzui_core::semantics::Role;
use enzui_widgets::animated_image::{AnimatedFrames, AnimatedImageError, enz_animated_image};
use iced::advanced::renderer::Headless;
use iced::time::{Duration, Instant};
use iced::widget::{Space, column};
use iced::{Element, Event, Size, window};
use iced_test::runtime::user_interface::{Cache, State, UserInterface};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

/// A 4×4 GIF of solid frames, each shown for its delay in milliseconds.
fn gif(frames: &[([u8; 4], u32)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut bytes);
        encoder
            .set_repeat(image::codecs::gif::Repeat::Infinite)
            .expect("repeat");
        for (color, delay) in frames {
            let buffer = image::RgbaImage::from_pixel(4, 4, image::Rgba(*color));
            encoder
                .encode_frame(image::Frame::from_parts(
                    buffer,
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(*delay, 1),
                ))
                .expect("frame");
        }
    }
    bytes
}

fn three() -> AnimatedFrames {
    AnimatedFrames::decode(&gif(&[(RED, 100), (GREEN, 200), (BLUE, 10)])).expect("decode")
}

#[test]
fn animated_images_obey_timing_visibility_motion_and_decode_limits_1() {
    // decoding_reads_every_frame_and_its_delay
    {
        let frames = three();
        assert_eq!(frames.len(), 3);
        assert_eq!((frames.size().width, frames.size().height), (4, 4));
        assert_eq!(frames.delay(0), Some(Duration::from_millis(100)));
        assert_eq!(frames.delay(1), Some(Duration::from_millis(200)));
        assert_eq!(
            frames.delay(2),
            Some(Duration::from_millis(100)),
            "a 10 ms frame plays at 100 ms, as in browsers"
        );
        assert!(matches!(
            AnimatedFrames::decode(b"not an image"),
            Err(AnimatedImageError::Unreadable(_))
        ));
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(2, 2, image::Rgba(RED))
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("png");
        assert_eq!(AnimatedFrames::decode(&png).expect("still").len(), 1);
    }

    // frames_advance_on_their_delays
    {
        let mut harness = Harness::new(three());
        let start = Instant::now();
        let (request, color) = harness.present(start);
        assert!(close(color, RED), "first frame: {color:?}");
        assert_eq!(
            request,
            window::RedrawRequest::At(start + Duration::from_millis(100)),
            "wake when the first frame ends"
        );
        let (_, color) = harness.present(start + Duration::from_millis(150));
        assert!(close(color, GREEN), "second frame: {color:?}");
        let (_, color) = harness.present(start + Duration::from_millis(320));
        assert!(close(color, BLUE), "third frame: {color:?}");
        let (_, color) = harness.present(start + Duration::from_millis(420));
        assert!(close(color, RED), "it loops: {color:?}");
    }

    // a_hidden_or_paused_image_does_not_wake
    {
        let start = Instant::now();
        let mut hidden = Harness::new(three());
        hidden.offset = 500.0;
        assert_eq!(
            hidden.present(start).0,
            window::RedrawRequest::Wait,
            "out of view"
        );

        let mut paused = Harness::new(three());
        paused.playing = false;
        let (request, color) = paused.present(start);
        assert_eq!(request, window::RedrawRequest::Wait, "paused");
        let (_, later) = paused.present(start + Duration::from_millis(150));
        assert!(close(color, RED) && close(later, RED), "holds its frame");
    }

    // reduced_motion_shows_the_first_frame_and_never_wakes
    {
        let reduced = iced_core::preferences::Preferences {
            reduced_motion: true,
            ..iced_core::preferences::get()
        };
        iced_core::preferences::scoped(reduced, || {
            let mut harness = Harness::new(three());
            let start = Instant::now();
            let (request, _) = harness.present(start);
            assert_eq!(request, window::RedrawRequest::Wait);
            let (request, color) = harness.present(start + Duration::from_millis(150));
            assert_eq!(request, window::RedrawRequest::Wait);
            assert!(close(color, RED), "still the first frame: {color:?}");
        });
    }
}

/// Drives one image through presented frames and reads its pixels.
struct Harness {
    renderer: iced_test::renderer::Renderer,
    cache: Option<Cache>,
    frames: AnimatedFrames,
    playing: bool,
    /// Pushes the image this far below the 64 px window.
    offset: f32,
}

impl Harness {
    fn new(frames: AnimatedFrames) -> Self {
        let renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as Headless>::new(Default::default(), None),
        )
        .expect("renderer");
        Self {
            renderer,
            cache: Some(Cache::default()),
            frames,
            playing: true,
            offset: 0.0,
        }
    }

    fn view(&self) -> Element<'static, ()> {
        column![
            Space::new().height(self.offset),
            enz_animated_image(&self.frames)
                .width(32)
                .height(32)
                .playing(self.playing)
                .alt_text("Loading animation"),
        ]
        .into()
    }

    /// Present a frame at `at`: returns the redraw it asks for and the
    /// centre pixel's colour.
    fn present(&mut self, at: Instant) -> (window::RedrawRequest, [u8; 4]) {
        let mut ui = UserInterface::build(
            self.view(),
            Size::new(64.0, 64.0),
            self.cache.take().expect("cache"),
            &mut self.renderer,
        );
        let mut bus = iced::advanced::shell::Bus::new();
        let (state, _) = ui.update(
            &iced::window::Headless,
            &iced_test::core::shell::Waker::noop(),
            &[Event::Window(window::Event::RedrawRequested(at))],
            iced::mouse::Cursor::Unavailable,
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
            iced::mouse::Cursor::Unavailable,
        );
        let pixels = self
            .renderer
            .screenshot(Size::new(64, 64), 1.0, iced::Color::WHITE);
        self.cache = Some(ui.into_cache());
        let at = |x: usize, y: usize| {
            let i = (y * 64 + x) * 4;
            [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
        };
        (request, at(16, 16))
    }
}

fn close(a: [u8; 4], b: [u8; 4]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) < 24)
}

#[test]
fn animated_images_obey_timing_visibility_motion_and_decode_limits_2() {
    // a_named_animation_is_an_image_node
    {
        let element: Element<'_, ()> = enz_animated_image(&three())
            .alt_text("Loading animation")
            .into_element();
        let mut ui = iced_test::Simulator::new(element);
        let mut collector = SemanticsCollector::new();
        ui.operate(&mut collector);
        let images: Vec<_> = collector
            .collected()
            .iter()
            .filter(|n| n.semantics.role == Role::Image)
            .filter_map(|n| n.semantics.label.clone())
            .collect();
        assert_eq!(images, ["Loading animation"]);
    }

    // replaced_frames_start_from_their_first_frame
    {
        let mut harness = Harness::new(three());
        let start = Instant::now();
        let _ = harness.present(start);
        let (_, color) = harness.present(start + Duration::from_millis(150));
        assert!(close(color, GREEN), "playing its second frame: {color:?}");

        harness.frames = harness.frames.clone();
        let (_, color) = harness.present(start + Duration::from_millis(160));
        assert!(close(color, GREEN), "a clone keeps its place: {color:?}");

        harness.frames =
            AnimatedFrames::decode(&gif(&[(BLUE, 100), (RED, 100), (GREEN, 100)])).expect("decode");
        let (_, color) = harness.present(start + Duration::from_millis(170));
        assert!(
            close(color, BLUE),
            "the new animation starts at its first frame: {color:?}"
        );
    }

    // an_animation_past_its_limits_is_refused
    {
        use enzui_widgets::animated_image::{AnimatedImageError, AnimationLimits};
        let bytes = gif(&[(RED, 100), (GREEN, 200), (BLUE, 10)]);
        let limited =
            |limits| AnimatedFrames::decode_bounded(&bytes, limits).map(|frames| frames.len());
        let roomy = AnimationLimits::default();
        assert_eq!(limited(roomy), Ok(3));
        for tight in [
            AnimationLimits {
                max_frames: 2,
                ..roomy
            },
            AnimationLimits {
                max_total_bytes: 3,
                ..roomy
            },
            AnimationLimits {
                max_edge: 1,
                ..roomy
            },
        ] {
            assert_eq!(
                limited(tight),
                Err(AnimatedImageError::TooLarge),
                "{tight:?}"
            );
        }
    }
}
