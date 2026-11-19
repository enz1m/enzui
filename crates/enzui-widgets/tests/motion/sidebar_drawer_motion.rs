use super::motion_fixture::{self as fixture, Msg};
use enzui_widgets::app_shell::{ShellLayout, ShellTokens, enz_app_shell};
use enzui_widgets::nav_split::{NavSplitState, SidebarPresentation};
use iced::advanced::renderer::Headless as _;
use iced::time::{Duration, Instant};
use iced::widget::{Space, container};
use iced::{Color, Element, Length, Size, Theme};
use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, State, UserInterface};

const WIDTH: u32 = 400;
const HEIGHT: u32 = 240;
const SIZE: Size = Size::new(WIDTH as f32, HEIGHT as f32);

fn drawer(layout: ShellLayout, open: bool) -> Element<'static, Msg> {
    let fill = move |color: Color| {
        container(Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| container::Style {
                background: Some(color.into()),
                ..container::Style::default()
            })
            .into()
    };
    enz_app_shell(move || fill(Color::WHITE))
        .layout(layout)
        .tokens(ShellTokens::flat_page().sidebar_width(160.0))
        .state(NavSplitState::for_sidebar_first(open))
        .forced(SidebarPresentation::Overlay)
        .sidebar(move || fill(fixture::SIDEBAR_INK))
        .titlebar(|| Space::new().into())
        .on_toggle_sidebar(Msg::Toggle)
        .into()
}

fn redraw(
    ui: &mut UserInterface<'_, Msg, Theme, Renderer>,
    renderer: &mut Renderer,
    theme: &Theme,
    at: Instant,
) -> State {
    let (state, _) = ui.update(
        &iced::window::Headless,
        &iced::advanced::graphics::core::shell::Waker::noop(),
        &[iced::Event::Window(iced::window::Event::RedrawRequested(
            at,
        ))],
        iced::mouse::Cursor::Unavailable,
        renderer,
        &mut iced::advanced::shell::Bus::new(),
    );
    fixture::paint(ui, renderer, theme);
    state
}

fn sample(renderer: &mut Renderer) -> (f32, f32) {
    let pixels = renderer.screenshot(Size::new(WIDTH, HEIGHT), 1.0, Color::WHITE);
    let row_start = (HEIGHT / 2 * WIDTH * 4) as usize;
    let row = &pixels[row_start..row_start + WIDTH as usize * 4];
    let drawer_width = row
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[0] < 4 && pixel[1] > 251 && pixel[2] > 251)
        .count();
    (
        drawer_width as f32,
        f32::from(row[(WIDTH as usize - 24) * 4]),
    )
}

#[test]
fn modal_drawer_and_scrim_dismiss_together_on_painted_frames() {
    for layout in [
        ShellLayout::TitlebarSidebarContent,
        ShellLayout::FloatingTitlebarSidebar,
    ] {
        let theme = fixture::theme(false);
        let mut renderer = fixture::new_renderer();
        let mut open =
            UserInterface::build(drawer(layout, true), SIZE, Cache::default(), &mut renderer);
        fixture::paint(&mut open, &mut renderer, &theme);
        let now = Instant::now();
        for ms in (0..=800).step_by(16) {
            let _ = redraw(
                &mut open,
                &mut renderer,
                &theme,
                now + Duration::from_millis(ms),
            );
        }
        let (open_width, dimmed) = sample(&mut renderer);
        let mut closed =
            UserInterface::build(drawer(layout, false), SIZE, Cache::default(), &mut renderer);
        fixture::paint(&mut closed, &mut renderer, &theme);
        let (hidden_width, undimmed) = sample(&mut renderer);
        assert!(open_width > 100.0 && hidden_width == 0.0);
        assert!(undimmed - dimmed > 40.0);

        let mut closing = UserInterface::build(
            drawer(layout, false),
            SIZE,
            open.into_cache(),
            &mut renderer,
        );
        let edge =
            enzui_widgets::app_shell::build_probe::take_motion_edge().expect("drawer dismissal");
        let mut moving = 0;
        for ms in (0..=400).step_by(16) {
            let state = redraw(
                &mut closing,
                &mut renderer,
                &theme,
                edge + Duration::from_millis(ms),
            );
            let (width, brightness) = sample(&mut renderer);
            let visible = width / open_width;
            let shade = (undimmed - brightness) / (undimmed - dimmed);
            assert!(
                (visible - shade).abs() < 0.12,
                "{layout:?} at {ms} ms: drawer {visible:.2}, shade {shade:.2}"
            );
            if (0.15..0.85).contains(&visible) {
                assert!(
                    matches!(
                        state,
                        State::Updated {
                            is_repainting: true,
                            ..
                        }
                    ),
                    "{layout:?} at {ms} ms must paint both layers"
                );
                moving += 1;
            }
        }
        assert!(
            moving >= 2,
            "{layout:?} must slide through intermediate frames"
        );
        assert_eq!(sample(&mut renderer), (hidden_width, undimmed));
    }
}
