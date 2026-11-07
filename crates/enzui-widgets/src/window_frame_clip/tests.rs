use iced::advanced::renderer::Headless as _;
use iced::{Color, Element, Length, Size, mouse, window};
use iced_test::runtime::user_interface::{Cache, UserInterface};

const RED: Color = Color::from_rgb(1.0, 0.0, 0.0);

/// A shell with a full-window scrim stacked above it, as a modal host lays
/// one out, wrapped by the root host the way `app_setup` wraps a window.
fn view(maximized: bool) -> Element<'static, ()> {
    let shell = crate::app_shell::enz_app_shell(|| iced::widget::Space::new().into())
        .layout(crate::app_shell::ShellLayout::TitlebarContent)
        .titlebar(|| iced::widget::text("Title").into())
        .maximized(maximized)
        .on_window_gesture(|_| ())
        .into_element();
    let scrim = iced::widget::container(iced::widget::Space::new())
        .width(Length::Fill)
        .height(Length::Fill)
        .style(|_| iced::widget::container::Style {
            background: Some(RED.into()),
            ..Default::default()
        });
    super::host(iced::widget::stack![shell, scrim].into())
}

/// The scrim's colour at the window's top-left corner pixel and at its centre.
fn corner_and_centre(maximized: bool) -> ([u8; 4], [u8; 4]) {
    let mut renderer = iced::futures::executor::block_on(iced_test::renderer::Renderer::new(
        Default::default(),
        std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
    ))
    .expect("renderer");
    let size = Size::new(200.0, 150.0);
    let mut ui = UserInterface::build(view(maximized), size, Cache::default(), &mut renderer);
    let _ = ui.update(
        &window::Headless,
        &iced::advanced::graphics::core::shell::Waker::noop(),
        &[iced::Event::Window(window::Event::RedrawRequested(
            iced::time::Instant::now(),
        ))],
        mouse::Cursor::Unavailable,
        &mut renderer,
        &mut iced::advanced::shell::Bus::new(),
    );
    ui.draw(
        &mut renderer,
        &iced::Theme::Dark,
        &iced::advanced::renderer::Style::default(),
        mouse::Cursor::Unavailable,
    );
    let pixels = renderer.screenshot(Size::new(200, 150), 1.0, Color::TRANSPARENT);
    let at = |x: usize, y: usize| -> [u8; 4] {
        let i = (y * 200 + x) * 4;
        [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
    };
    (at(0, 0), at(100, 75))
}

#[test]
fn a_layer_above_a_rounded_shell_leaves_the_corners_outside_its_frame_clear() {
    let (corner, centre) = corner_and_centre(false);
    assert_eq!(centre[..3], [255, 0, 0], "the scrim covers the frame");
    assert_eq!(
        corner[3], 0,
        "outside the rounded frame stays clear: {corner:?}"
    );

    let (corner, _) = corner_and_centre(true);
    assert_eq!(
        corner[..3],
        [255, 0, 0],
        "a maximized window has square corners"
    );
}

/// How many runtime actions `task` carries.
fn actions(task: iced::Task<()>) -> usize {
    let Some(mut stream) = iced_test::runtime::task::into_stream(task) else {
        return 0;
    };
    let waker = iced::futures::task::noop_waker();
    let mut context = std::task::Context::from_waker(&waker);
    let mut count = 0;
    while let std::task::Poll::Ready(Some(_)) =
        iced::futures::StreamExt::poll_next_unpin(&mut stream, &mut context)
    {
        count += 1;
    }
    count
}

#[test]
fn a_self_framed_window_gets_the_theme_shadow_once_and_again_when_it_changes() {
    let sync = super::ShadowSync::default();
    let framed = iced::window::Id::unique();
    let plain = iced::window::Id::unique();
    sync.saw(framed, true);
    sync.saw(plain, false);
    assert_eq!(actions(sync.tasks()), 1, "the self-framed window only");
    assert_eq!(actions(sync.tasks()), 0, "nothing changed");

    let mut theme = enzui_theme::EnzTheme::dark();
    theme.window_shadow.opacity = 0.4;
    let _scope = enzui_theme::text_defaults::TextDefaultsScope::enter(&theme);
    assert_eq!(actions(sync.tasks()), 1, "the theme's shadow changed");
}

#[test]
fn text_keeps_the_ancestor_mask() {
    let mut renderer = iced::futures::executor::block_on(iced_test::renderer::Renderer::new(
        Default::default(),
        Some("wgpu"),
    ))
    .expect("wgpu renderer");
    let view: Element<'_, ()> = crate::rounded_clip::rounded_clip(
        iced::widget::container(iced::widget::text("WW").size(80).color(Color::WHITE))
            .width(Length::Fill)
            .height(Length::Fill),
        50.0,
    )
    .into();
    let mut ui = UserInterface::build(
        view,
        Size::new(200.0, 150.0),
        Cache::default(),
        &mut renderer,
    );
    ui.draw(
        &mut renderer,
        &iced::Theme::Dark,
        &Default::default(),
        mouse::Cursor::Unavailable,
    );
    let pixels = renderer.screenshot(Size::new(200, 150), 1.0, Color::TRANSPARENT);
    let ink = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[3] > 16)
        .count();
    assert!(ink > 100, "the glyphs paint a positive control: {ink}");
    let leaks = (0..50)
        .flat_map(|y| (0..50).map(move |x| (x, y)))
        .filter(|(x, y)| {
            ((50 - *x) as f32).hypot((50 - *y) as f32) > 52.0 && pixels[(y * 200 + x) * 4 + 3] > 16
        })
        .count();
    assert_eq!(leaks, 0, "glyphs must not escape the rounded ancestor mask");
}

/// Messages a nudged view publishes over two redraws of one build.
fn nudged(nudge: bool) -> Vec<u8> {
    let mut renderer = iced::futures::executor::block_on(iced_test::renderer::Renderer::new(
        Default::default(),
        std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
    ))
    .expect("renderer");
    let view: Element<'static, u8> = super::nudge(
        iced::widget::Space::new().into(),
        nudge.then(|| -> Box<dyn Fn() -> u8> { Box::new(|| 7) }),
    );
    let mut ui = UserInterface::build(view, Size::new(80.0, 60.0), Cache::default(), &mut renderer);
    let mut bus = iced::advanced::shell::Bus::new();
    for _ in 0..2 {
        let _ = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[iced::Event::Window(window::Event::RedrawRequested(
                iced::time::Instant::now(),
            ))],
            mouse::Cursor::Unavailable,
            &mut renderer,
            &mut bus,
        );
    }
    bus.messages().to_vec()
}

#[test]
fn a_window_waiting_for_glass_asks_once_for_the_update_that_binds_it() {
    assert_eq!(nudged(true), vec![7], "one nudge per view build");
    assert!(nudged(false).is_empty());

    let sync = super::GlassSync::default();
    let window = window::Id::unique();
    assert!(!sync.saw(window, false), "no glass wanted, nothing waits");
    assert!(
        sync.saw(window, true),
        "first wanted outside an update: waits"
    );
    assert!(
        sync.saw(window, true),
        "still waiting until an update binds it"
    );
    let _ = sync.tasks(|message| message);
    assert!(!sync.saw(window, true), "bound once asked");
    assert!(sync.wanted(&enzui_core::app_runtime::ServiceMessage::new(
        super::GlassWanted
    )));
}
