use enzui_theme::palette::Palette;
use enzui_widgets::search_field::enz_search_field;
use iced::advanced::{
    renderer::Headless,
    widget::{Id, operation},
};
use iced::{Color, Element, Event, Length, Size, mouse, window};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

fn view() -> Element<'static, ()> {
    let mut palette = Palette::dark();
    palette.text_muted = Color::from_rgb(1.0, 0.0, 0.0);
    enz_search_field("", |_| ())
        .placeholder("Search placeholder")
        .id(Id::new("composition-field"))
        .palette(palette)
        .width(Length::Fixed(240.0))
        .into()
}

fn event(
    ui: &mut UserInterface<'_, (), iced::Theme, Renderer>,
    renderer: &mut Renderer,
    event: Event,
) {
    let mut messages = iced::advanced::shell::Bus::new();
    ui.update(
        &window::Headless,
        &iced::advanced::graphics::core::shell::Waker::noop(),
        &[event],
        mouse::Cursor::Unavailable,
        renderer,
        &mut messages,
    );
}

fn placeholder_pixels(
    ui: &mut UserInterface<'_, (), iced::Theme, Renderer>,
    renderer: &mut Renderer,
    theme: &iced::Theme,
) -> usize {
    ui.draw(
        renderer,
        theme,
        &iced::advanced::renderer::Style {
            text_color: Color::WHITE,
        },
        mouse::Cursor::Unavailable,
    );
    let rgba = renderer.screenshot(Size::new(240, 40), 1.0, Color::BLACK);
    (5..27)
        .flat_map(|y| (40..225).map(move |x| (y * 240 + x) * 4))
        .filter(|&index| rgba[index] > 100 && rgba[index + 1] < 40 && rgba[index + 2] < 40)
        .count()
}

#[test]
fn focused_preedit_hides_placeholder_across_rebuild_and_empty_preedit_restores_it() {
    for theme in [iced::Theme::Dark, iced::Theme::Light] {
        let _scope = enzui_widgets::focus_retention::FocusScope::new();
        let mut renderer = iced::futures::executor::block_on(<Renderer as Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ))
        .unwrap();
        let size = Size::new(240.0, 40.0);
        let mut ui = UserInterface::build(view(), size, Cache::default(), &mut renderer);
        ui.operate(
            &renderer,
            &mut operation::focusable::focus::<()>(Id::new("composition-field")),
        );
        assert!(placeholder_pixels(&mut ui, &mut renderer, &theme) > 10);
        event(
            &mut ui,
            &mut renderer,
            Event::InputMethod(iced::advanced::input_method::Event::Preedit(
                "に".into(),
                None,
            )),
        );
        assert_eq!(placeholder_pixels(&mut ui, &mut renderer, &theme), 0);
        let cache = ui.into_cache();
        let mut ui = UserInterface::build(view(), size, cache, &mut renderer);
        assert_eq!(placeholder_pixels(&mut ui, &mut renderer, &theme), 0);
        event(
            &mut ui,
            &mut renderer,
            Event::InputMethod(iced::advanced::input_method::Event::Preedit(
                "".into(),
                None,
            )),
        );
        assert!(placeholder_pixels(&mut ui, &mut renderer, &theme) > 10);
    }
}
