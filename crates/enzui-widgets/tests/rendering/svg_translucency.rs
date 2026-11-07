//! A translucent SVG composites as its alpha says on the GPU renderer: half
//! white over black is mid grey, not a quarter.

use iced::advanced::renderer::Headless as _;
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
