use enzui_core::{Action, Icon, collect::SemanticsCollector, semantics::Role};
use enzui_theme::EnzTheme;
use enzui_widgets::header_bar::{HeaderAction, enz_header_bar};
use iced::advanced::renderer::Headless;
use iced::{Element, Size};
use iced_test::runtime::{
    core::{shell::Waker, window::Headless as Window},
    user_interface::{Cache, UserInterface},
};

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Back,
    New,
    Save,
    Close,
}

fn view(label: &str, show_title: bool) -> Element<'_, Msg> {
    enz_header_bar("A rather long document title")
        .subtitle("Subtitle here")
        .show_title(show_title)
        .show_back_button(true)
        .on_back(Msg::Back)
        .start_actions(vec![HeaderAction::new(
            Action::new("file.new", Msg::New)
                .with_label(label)
                .with_icon(Icon::named("action.add")),
        )])
        .end_actions(vec![
            HeaderAction::new(
                Action::new("file.save", Msg::Save)
                    .with_label("Save")
                    .with_icon(Icon::named("action.save")),
            ),
            HeaderAction::icon_only(
                Action::new("window.close", Msg::Close)
                    .with_label("Close")
                    .with_icon(Icon::named("action.close")),
            ),
        ])
        .into()
}

#[test]
fn header_bar_narrow_actions_remain_separate_visible_and_activate_their_own_command() {
    for enz in [EnzTheme::dark(), EnzTheme::light()] {
        for show_title in [false, true] {
            for label in ["New", "Create a new document"] {
                for width in [160.0, 176.0, 200.0, 240.0, 375.0, 640.0] {
                    let _scope = enzui_core::widget_runtime::WidgetRuntimeScope::new();
                    let mut renderer = iced_test::futures::futures::executor::block_on(
                        <iced_test::renderer::Renderer as Headless>::new(Default::default(), None),
                    )
                    .expect("renderer");
                    let mut ui = UserInterface::build(
                        view(label, show_title),
                        Size::new(width, 48.0),
                        Cache::default(),
                        &mut renderer,
                    );
                    let mut collector = SemanticsCollector::new();
                    ui.operate(&renderer, &mut collector);
                    let buttons: Vec<_> = collector
                        .collected()
                        .iter()
                        .filter(|n| n.semantics.role == Role::Button)
                        .cloned()
                        .collect();
                    assert_eq!(buttons.len(), 4, "all four controls remain reachable");
                    for (i, b) in buttons.iter().enumerate() {
                        assert!(
                            b.bounds.width >= 12.0 && b.bounds.height >= 20.0,
                            "positive action bounds: width={width}, label={label}, node={b:?}"
                        );
                        assert!(
                            b.bounds.x >= 0.0 && b.bounds.x + b.bounds.width <= width + 0.1,
                            "action inside header"
                        );
                        for c in &buttons[i + 1..] {
                            let overlap = (b.bounds.x + b.bounds.width)
                                .min(c.bounds.x + c.bounds.width)
                                - b.bounds.x.max(c.bounds.x);
                            assert!(
                                overlap <= 0.1,
                                "{width}px {label}: controls overlap by {overlap}px: {:?} vs {:?}",
                                b.bounds,
                                c.bounds
                            );
                        }
                    }
                    let theme = enz.iced_theme();
                    let base =
                        <iced::Theme as iced::advanced::graphics::core::theme::Base>::base(&theme);
                    ui.draw(
                        &mut renderer,
                        &theme,
                        &iced::advanced::renderer::Style {
                            text_color: base.text_color,
                        },
                        iced::mouse::Cursor::Unavailable,
                    );
                    let pixels = renderer.screenshot(
                        Size::new(width as u32, 48),
                        1.0,
                        base.background_color,
                    );
                    ui.draw(
                        &mut renderer,
                        &theme,
                        &iced::advanced::renderer::Style {
                            text_color: base.text_color,
                        },
                        iced::mouse::Cursor::Unavailable,
                    );
                    assert!(
                        pixels
                            == renderer.screenshot(
                                Size::new(width as u32, 48),
                                1.0,
                                base.background_color
                            ),
                        "repeat control"
                    );
                    let background = [
                        enz.palette.surface_alt.r,
                        enz.palette.surface_alt.g,
                        enz.palette.surface_alt.b,
                    ]
                    .map(|v| (v * 255.0).round() as u8);
                    for (b, expected) in
                        buttons
                            .iter()
                            .zip([Msg::Back, Msg::New, Msg::Save, Msg::Close])
                    {
                        let mut ink = 0;
                        for y in b.bounds.y.ceil() as usize + 2
                            ..(b.bounds.y + b.bounds.height).floor() as usize - 2
                        {
                            for x in b.bounds.x.ceil() as usize + 1
                                ..(b.bounds.x + b.bounds.width).floor() as usize - 1
                            {
                                let i = (y * width as usize + x) * 4;
                                let delta = (0..3)
                                    .map(|c| u32::from(pixels[i + c].abs_diff(background[c])))
                                    .sum::<u32>();
                                if delta > 30 {
                                    ink += 1;
                                }
                            }
                        }
                        assert!(
                            ink > 3,
                            "positive visible action ink: {ink}, {expected:?}, width={width}"
                        );
                        let cursor = iced::mouse::Cursor::Available(b.bounds.center());
                        let mut messages = iced::advanced::shell::Bus::new();
                        ui.update(
                            &Window,
                            &Waker::noop(),
                            &[
                                iced::Event::Mouse(iced::mouse::Event::CursorMoved {
                                    position: b.bounds.center(),
                                }),
                                iced::Event::Mouse(iced::mouse::Event::ButtonPressed(
                                    iced::mouse::Button::Left,
                                )),
                                iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                                    iced::mouse::Button::Left,
                                )),
                            ],
                            cursor,
                            &mut renderer,
                            &mut messages,
                        );
                        assert_eq!(
                            messages.into_iter().collect::<Vec<_>>(),
                            vec![expected.clone()],
                            "each control activates only its own command"
                        );
                        let mut focus = iced::advanced::widget::operation::focusable::focus::<()>(
                            b.id.clone().expect("routable action id"),
                        );
                        ui.operate(&renderer, &mut focus);
                        let mut messages = iced::advanced::shell::Bus::new();
                        let key = iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter);
                        ui.update(
                            &Window,
                            &Waker::noop(),
                            &[iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                                key: key.clone(),
                                modified_key: key,
                                physical_key: iced::keyboard::key::Physical::Unidentified(
                                    iced::keyboard::key::NativeCode::Unidentified,
                                ),
                                location: iced::keyboard::Location::Standard,
                                modifiers: iced::keyboard::Modifiers::empty(),
                                text: None,
                                repeat: false,
                            })],
                            iced::mouse::Cursor::Unavailable,
                            &mut renderer,
                            &mut messages,
                        );
                        assert_eq!(
                            messages.into_iter().collect::<Vec<_>>(),
                            vec![expected],
                            "focused Enter activates only its own command"
                        );
                    }
                }
            }
        }
    }
}
