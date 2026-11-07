use enzui_widgets::clickable::{ClickableTokens, enz_clickable_with_status};
use enzui_widgets::focus_button::FocusStatus;
use iced::advanced::renderer::Headless as _;
use iced::{Color, Element, Event, Point, Theme, mouse};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Activate,
}

fn ink(status: FocusStatus) -> Color {
    match status {
        FocusStatus::Active => Color::from_rgb8(80, 110, 150),
        FocusStatus::Hovered => Color::from_rgb8(100, 130, 170),
        FocusStatus::Pressed => Color::from_rgb8(60, 90, 130),
        FocusStatus::Focused => Color::from_rgb8(100, 110, 180),
        FocusStatus::Disabled => Color::from_rgb8(50, 70, 90),
    }
}

struct Paint {
    ui: iced_test::runtime::user_interface::UserInterface<
        'static,
        Msg,
        Theme,
        iced_test::renderer::Renderer,
    >,
    renderer: iced_test::renderer::Renderer,
    theme: Theme,
    built: Rc<Cell<usize>>,
    seen: Rc<RefCell<Vec<FocusStatus>>>,
    messages: iced::advanced::shell::Bus<Msg>,
}
impl Paint {
    fn new(theme: Theme, enabled: bool) -> Self {
        let built = Rc::new(Cell::new(0));
        let seen = Rc::new(RefCell::new(Vec::new()));
        let count = Rc::clone(&built);
        let states = Rc::clone(&seen);
        let body: Element<'static, Msg> = enz_clickable_with_status(
            move |status| {
                count.set(count.get() + 1);
                iced::widget::container(iced::widget::Space::new().width(56).height(56))
                    .style(move |_| {
                        let current = status.get();
                        states.borrow_mut().push(current);
                        iced::widget::container::Style::default()
                            .background(ink(current))
                            .border(iced::Border::default().rounded(28))
                    })
                    .into()
            },
            Msg::Activate,
        )
        .enabled(enabled)
        .aria_label("Capture")
        .id(iced::advanced::widget::Id::new("stateful-disc"))
        .radius(28.0)
        .tokens(ClickableTokens {
            background: Color::TRANSPARENT,
            hover_overlay: Color::TRANSPARENT,
            pressed_overlay: Color::TRANSPARENT,
            disabled_alpha: 1.0,
            ..ClickableTokens::from_palette(&enzui_widgets::theme_lookup::palette(&theme))
        })
        .into();
        let mut renderer = iced_test::futures::futures::executor::block_on(
            <iced_test::renderer::Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        let ui = iced_test::runtime::user_interface::UserInterface::build(
            body,
            iced::Size::new(56.0, 56.0),
            iced_test::runtime::user_interface::Cache::default(),
            &mut renderer,
        );
        Self {
            ui,
            renderer,
            theme,
            built,
            seen,
            messages: iced::advanced::shell::Bus::new(),
        }
    }
    fn events(&mut self, events: &[Event], cursor: mouse::Cursor) -> iced::window::RedrawRequest {
        let (state, _) = self.ui.update(
            &iced::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            cursor,
            &mut self.renderer,
            &mut self.messages,
        );
        match state {
            iced_test::runtime::user_interface::State::Updated { redraw_request, .. } => {
                redraw_request
            }
            iced_test::runtime::user_interface::State::Outdated => panic!("unexpected relayout"),
        }
    }
    fn paint(&mut self, cursor: mouse::Cursor, expected: FocusStatus) {
        self.events(
            &[Event::Window(iced::window::Event::RedrawRequested(
                std::time::Instant::now(),
            ))],
            cursor,
        );
        let base = <Theme as iced::advanced::graphics::core::theme::Base>::base(&self.theme);
        self.ui.draw(
            &mut self.renderer,
            &self.theme,
            &iced::advanced::renderer::Style {
                text_color: base.text_color,
            },
            cursor,
        );
        let bytes = self
            .renderer
            .screenshot(iced::Size::new(56, 56), 1.0, base.background_color);
        let center = (28 * 56 + 28) * 4;
        let color = ink(expected);
        let desired = [color.r * 255.0, color.g * 255.0, color.b * 255.0];
        for (actual, expected) in bytes[center..center + 3].iter().zip(desired) {
            assert!(
                (f32::from(*actual) - expected).abs() <= 1.0,
                "content paint disagrees for {expected:?}"
            );
        }
        assert_eq!(self.seen.borrow().last(), Some(&expected));
        assert_eq!(
            self.built.get(),
            1,
            "state changes must not rebuild the content tree"
        );
    }
}

#[test]
fn clickable_content_status_reaches_real_paint_pointer_keyboard_and_disabled() {
    let outside = mouse::Cursor::Unavailable;
    let inside = mouse::Cursor::Available(Point::new(28.0, 28.0));
    for theme in [
        enzui_theme::EnzTheme::dark().iced_theme(),
        enzui_theme::EnzTheme::light().iced_theme(),
    ] {
        let mut h = Paint::new(theme.clone(), true);
        h.paint(outside, FocusStatus::Active);
        assert_eq!(
            h.events(
                &[Event::Mouse(mouse::Event::CursorMoved {
                    position: Point::new(28.0, 28.0)
                })],
                inside
            ),
            iced::window::RedrawRequest::NextFrame
        );
        h.paint(inside, FocusStatus::Hovered);
        h.events(
            &[Event::Mouse(mouse::Event::ButtonPressed(
                mouse::Button::Left,
            ))],
            inside,
        );
        h.paint(inside, FocusStatus::Pressed);
        assert!(h.messages.is_empty());
        h.events(
            &[Event::Mouse(mouse::Event::ButtonReleased(
                mouse::Button::Left,
            ))],
            inside,
        );
        h.paint(inside, FocusStatus::Hovered);
        assert_eq!(
            h.messages.drain_messages().collect::<Vec<_>>(),
            vec![Msg::Activate]
        );
        h.events(
            &[iced_test::simulator::press_key(
                iced::keyboard::key::Named::Space,
                None,
            )],
            outside,
        );
        h.paint(outside, FocusStatus::Pressed);
        assert!(h.messages.is_empty());
        h.events(
            &[iced_test::simulator::release_key(
                iced::keyboard::key::Named::Space,
            )],
            outside,
        );
        h.paint(outside, FocusStatus::Focused);
        assert_eq!(
            h.messages.drain_messages().collect::<Vec<_>>(),
            vec![Msg::Activate]
        );
        h.events(
            &[iced_test::simulator::press_key(
                iced::keyboard::key::Named::Enter,
                None,
            )],
            outside,
        );
        assert_eq!(
            h.messages.drain_messages().collect::<Vec<_>>(),
            vec![Msg::Activate]
        );
        h.events(
            &[Event::Touch(iced::touch::Event::FingerPressed {
                id: iced::touch::Finger(5),
                position: Point::new(28.0, 28.0),
            })],
            outside,
        );
        h.paint(outside, FocusStatus::Pressed);
        assert!(h.messages.is_empty());
        h.events(
            &[Event::Touch(iced::touch::Event::FingerLifted {
                id: iced::touch::Finger(5),
                position: Point::new(28.0, 28.0),
            })],
            outside,
        );
        h.paint(outside, FocusStatus::Active);
        assert_eq!(
            h.messages.drain_messages().collect::<Vec<_>>(),
            vec![Msg::Activate]
        );
        h.events(
            &[Event::Mouse(mouse::Event::ButtonPressed(
                mouse::Button::Left,
            ))],
            inside,
        );
        h.paint(inside, FocusStatus::Pressed);
        h.events(
            &[
                Event::Mouse(mouse::Event::CursorMoved {
                    position: Point::new(80.0, 80.0),
                }),
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            ],
            outside,
        );
        h.paint(outside, FocusStatus::Active);
        assert!(h.messages.is_empty());
        let mut collector = enzui_core::collect::SemanticsCollector::new();
        h.ui.operate(&h.renderer, &mut collector);
        let buttons: Vec<_> = collector
            .collected()
            .iter()
            .filter(|node| node.semantics.role == enzui_core::semantics::Role::Button)
            .collect();
        assert_eq!(buttons.len(), 1);
        assert!(buttons[0].semantics.supports_default_action);
        let mut operation =
            enzui_widgets::activation::activate(iced::advanced::widget::Id::new("stateful-disc"));
        h.ui.operate(&h.renderer, &mut operation);
        assert!(operation.armed());
        h.events(
            &[Event::Window(iced::window::Event::RedrawRequested(
                std::time::Instant::now(),
            ))],
            outside,
        );
        assert_eq!(
            h.messages.drain_messages().collect::<Vec<_>>(),
            vec![Msg::Activate]
        );
        let mut disabled = Paint::new(theme, false);
        disabled.paint(inside, FocusStatus::Disabled);
        disabled.events(
            &[
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
                iced_test::simulator::press_key(iced::keyboard::key::Named::Space, None),
                iced_test::simulator::release_key(iced::keyboard::key::Named::Space),
            ],
            inside,
        );
        disabled.paint(inside, FocusStatus::Disabled);
        let mut operation =
            enzui_widgets::activation::activate(iced::advanced::widget::Id::new("stateful-disc"));
        disabled.ui.operate(&disabled.renderer, &mut operation);
        assert!(!operation.armed());
        disabled.events(
            &[Event::Window(iced::window::Event::RedrawRequested(
                std::time::Instant::now(),
            ))],
            outside,
        );
        assert!(disabled.messages.is_empty());
    }
}
