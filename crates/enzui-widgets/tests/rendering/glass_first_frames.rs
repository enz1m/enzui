//! Glass that opts into adaptive ink appears in its measured ink on the real renderer: over a
//! still backdrop it stays hidden until its first reading lands, and the
//! first frame it shows is already the settled look, with no switch a few
//! frames later.

use enzui_theme::{EnzTheme, GlassInk, MaterialStyle};
use iced::advanced::renderer::Headless;
use iced::time::{Duration, Instant};
use iced::widget::{Space, container, stack};
use iced::{Color, Element, Event, Length, Size, Theme, mouse, window};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;
const W: u32 = 240;
const H: u32 = 120;
const BACKDROP: Color = Color::from_rgb(0.12, 0.16, 0.45);

fn view(style: MaterialStyle) -> Element<'static, (), Theme, Renderer> {
    stack![
        container(Space::new().width(Length::Fill).height(Length::Fill)).style(|_| {
            container::Style {
                background: Some(BACKDROP.into()),
                ..Default::default()
            }
        }),
        container(
            enzui_widgets::glass::enz_glass(
                container(enzui_widgets::text::enz_text("Glass").into_element()).padding(16),
            )
            .style(style)
            .ink(GlassInk::Adaptive)
            .into_element(),
        )
        .center(Length::Fill),
    ]
    .into()
}

#[test]
fn adaptive_glass_shows_first_in_its_measured_ink() {
    let Some(mut renderer) = iced_test::futures::futures::executor::block_on(Renderer::new(
        Default::default(),
        Some("wgpu"),
    )) else {
        println!("SKIPPED: no wgpu adapter, glass cannot draw");
        return;
    };
    let theme = EnzTheme::light().iced_theme();
    let base = <Theme as iced::advanced::graphics::core::theme::Base>::base(&theme);
    let mut cache = Cache::default();
    let t0 = Instant::now();
    let mut frames = Vec::new();
    for i in 0..16 {
        let mut ui = UserInterface::build(
            view(MaterialStyle::Regular),
            Size::new(W as f32, H as f32),
            cache,
            &mut renderer,
        );
        let _ = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[Event::Window(window::Event::RedrawRequested(
                t0 + Duration::from_millis(16 * i),
            ))],
            mouse::Cursor::Unavailable,
            &mut renderer,
            &mut iced::advanced::shell::Bus::new(),
        );
        ui.draw(
            &mut renderer,
            &theme,
            &iced::advanced::renderer::Style {
                text_color: base.text_color,
            },
            mouse::Cursor::Unavailable,
        );
        cache = ui.into_cache();
        frames.push(renderer.screenshot(Size::new(W, H), 1.0, base.background_color));
    }
    let centre = |frame: &[u8]| {
        let i = (((H / 2) * W + W / 2) * 4) as usize;
        (frame[i], frame[i + 1], frame[i + 2])
    };
    let backdrop = (
        (BACKDROP.r * 255.0).round() as u8,
        (BACKDROP.g * 255.0).round() as u8,
        (BACKDROP.b * 255.0).round() as u8,
    );
    let near = |a: (u8, u8, u8), b: (u8, u8, u8)| {
        a.0.abs_diff(b.0) <= 2 && a.1.abs_diff(b.1) <= 2 && a.2.abs_diff(b.2) <= 2
    };
    let settled = frames.last().expect("frames");
    assert!(
        !near(centre(settled), backdrop),
        "the glass shows once measured: {:?}",
        centre(settled)
    );
    let first_shown = frames
        .iter()
        .position(|frame| !near(centre(frame), backdrop))
        .expect("the glass shows");
    assert!(
        frames[first_shown] == *settled,
        "frame {first_shown}, the first to show the glass, is its settled look"
    );
}
