//! A translucent SVG composites as its alpha says on the GPU renderer: half
//! white over black is mid grey, not a quarter.

use iced::advanced::renderer::Headless;
use iced_test::runtime::user_interface::{Cache, UserInterface};

#[test]
fn half_transparent_svg_white_over_black_is_mid_grey() {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            Some("wgpu"),
        ),
    );
    let Some(renderer) = renderer.as_mut() else {
        eprintln!("SKIPPED: no wgpu adapter");
        return;
    };
    let svg = br#"<svg width="8" height="8" xmlns="http://www.w3.org/2000/svg"><rect width="8" height="8" fill="white" fill-opacity="0.5"/></svg>"#;
    let element: iced::Element<'_, ()> =
        iced::widget::svg(iced::widget::svg::Handle::from_memory(svg.to_vec()))
            .width(8)
            .height(8)
            .into();
    let mut ui = UserInterface::build(
        element,
        iced::Size::new(8.0, 8.0),
        Cache::default(),
        renderer,
    );
    ui.draw(
        renderer,
        &iced::Theme::Dark,
        &iced::advanced::renderer::Style {
            text_color: iced::Color::WHITE,
        },
        iced::mouse::Cursor::Unavailable,
    );
    let pixels = renderer.screenshot(iced::Size::new(8, 8), 1.0, iced::Color::BLACK);
    let centre = &pixels[(4 * 8 + 4) * 4..(4 * 8 + 4) * 4 + 3];
    assert!(
        centre.iter().all(|c| c.abs_diff(128) <= 2),
        "half white over black: {centre:?}"
    );
}

#[test]
fn ai_glyphs_paint_the_requested_tint() {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as Headless>::new(Default::default(), Some("software")),
    )
    .expect("software renderer");

    for name in ["ai.assistant", "enzim-ai-chat"] {
        for (theme, tint, clear) in [
            (
                iced::Theme::Dark,
                iced::Color::from_rgb8(255, 110, 0),
                iced::Color::BLACK,
            ),
            (
                iced::Theme::Light,
                iced::Color::from_rgb8(0, 74, 189),
                iced::Color::WHITE,
            ),
        ] {
            let icon: iced::Element<'_, ()> = enzui_widgets::icon::enz_icon(name)
                .size(20.0)
                .tint(tint)
                .into_element();
            let mut ui = UserInterface::build(
                icon,
                iced::Size::new(20.0, 20.0),
                Cache::default(),
                &mut renderer,
            );
            ui.draw(
                &mut renderer,
                &theme,
                &iced::advanced::renderer::Style { text_color: clear },
                iced::mouse::Cursor::Unavailable,
            );
            let pixels = renderer.screenshot(iced::Size::new(30, 30), 1.5, clear);
            let sample = enzui_widgets::color_presence::sample(
                &pixels,
                30,
                iced::Rectangle::with_size(iced::Size::new(30.0, 30.0)),
                tint,
                8,
                clear,
            );
            assert!(sample.matching > 10, "{name}: {}", sample.describe());
        }
    }
}
