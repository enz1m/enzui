//! Settled labels and buttons leave the frame routes, so an idle list is not
//! walked on every presented frame. Icons stay routed: they pin hover status
//! on frames, which a scroll under a still pointer needs.

use enzui_widgets::enz_text::enz_text;
use enzui_widgets::focus_button::focus_button;
use enzui_widgets::label::enz_label;
use iced::{Element, Size};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

fn listeners_after_settling(element: Element<'static, (), iced::Theme, Renderer>) -> usize {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");
    let mut ui = UserInterface::build(
        element,
        Size::new(300.0, 200.0),
        Cache::default(),
        &mut renderer,
    );
    for _ in 0..3 {
        let mut messages = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &iced::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[iced::Event::Window(iced::window::Event::RedrawRequested(
                std::time::Instant::now(),
            ))],
            iced::mouse::Cursor::Unavailable,
            &mut renderer,
            &mut messages,
        );
    }
    ui.frame_route_counts().1
}

// Sabotage: return `true` from `FocusButton::frame_interest` and the button
// stays routed; drop `text::SuppressAnnounce`'s forwarding and the label does.
#[test]
fn settled_labels_and_buttons_hold_no_frame_route() {
    for (name, element) in [
        ("silent text", enz_text("Workspaces").announce(false).into()),
        ("label", enz_label("module_000").into_element()),
        (
            "button",
            focus_button(iced::widget::text("Open"), ()).into(),
        ),
    ] {
        assert_eq!(
            listeners_after_settling(element),
            0,
            "a settled {name} still listens for every frame"
        );
    }
}
