use enzui_core::{focus_traversal, reducer_task};
use enzui_test_support::chained_operation::operate_chained;
use enzui_widgets::focus_button::{FocusStatus, focus_button};
use iced::advanced::renderer::Headless;
use iced::advanced::widget::{Id, Operation, operation};
use iced::widget::{column, container, row, scrollable, space, text};
use iced::{Color, Element, Point, Rectangle, Size, Theme, Vector};
use iced_test::futures::futures::{StreamExt, stream};
use iced_test::runtime::user_interface::{Cache, UserInterface};
use std::task::{Context, Poll, Waker};

const SIZE: Size = Size::new(600.0, 400.0);
const INK: Color = Color::from_rgb(1.0, 0.0, 1.0);
const RING: Color = Color::from_rgb(0.0, 1.0, 1.0);

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Hit,
}

fn stop(name: &'static str, width: f32, height: f32) -> Element<'static, Msg> {
    focus_button(text(name), Msg::Hit)
        .id(Id::new(name))
        .width(width)
        .height(height)
        .style_focus(|_, status| iced::widget::button::Style {
            background: Some(INK.into()),
            text_color: Color::BLACK,
            border: iced::Border {
                color: RING,
                width: if matches!(status, FocusStatus::Focused) {
                    2.0
                } else {
                    0.0
                },
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn both(content: Element<'static, Msg>, w: f32, h: f32) -> iced::widget::Scrollable<'static, Msg> {
    scrollable(content)
        .direction(scrollable::Direction::Both {
            vertical: scrollable::Scrollbar::default(),
            horizontal: scrollable::Scrollbar::default(),
        })
        .width(w)
        .height(h)
}

fn view(anonymous: bool, oversized: bool) -> Element<'static, Msg> {
    let deep = row![
        space().width(290),
        stop(
            "deep",
            if oversized { 260.0 } else { 64.0 },
            if oversized { 150.0 } else { 36.0 }
        )
    ];
    let inner_content = column![
        stop("first", 64.0, 36.0),
        space().height(224),
        deep,
        space().height(300)
    ]
    .width(700);
    let mut inner = both(inner_content.into(), 180.0, 110.0);
    if !anonymous {
        inner = inner.id(Id::new("inner"));
    }
    let outer_content = column![
        space().height(420),
        row![space().width(360), inner],
        space().height(500)
    ]
    .width(1000);
    let outer = both(outer_content.into(), 260.0, 180.0).id(Id::new("outer"));
    let other = both(
        container(text("unrelated")).width(500).height(500).into(),
        100.0,
        180.0,
    )
    .id(Id::new("other"));
    container(row![outer, other].spacing(24))
        .padding([23, 17])
        .into()
}

struct Host {
    renderer: iced_test::renderer::Renderer,
    ui: UserInterface<'static, Msg, Theme, iced_test::renderer::Renderer>,
    bus: iced::advanced::shell::Bus<Msg>,
    window: iced::window::Id,
    now: std::time::Instant,
}
impl Host {
    fn new(anonymous: bool, oversized: bool) -> Self {
        let mut renderer = crate::honest_frame_driver::new_renderer();
        let ui = UserInterface::build(
            view(anonymous, oversized),
            SIZE,
            Cache::default(),
            &mut renderer,
        );
        Self {
            renderer,
            ui,
            bus: iced::advanced::shell::Bus::new(),
            window: iced::window::Id::unique(),
            now: std::time::Instant::now(),
        }
    }
    fn run(&mut self, mut op: Box<dyn Operation<()>>) {
        operate_chained(|op| self.ui.operate(&self.renderer, op), op.as_mut());
    }
    fn event(&mut self, event: iced::Event, cursor: iced::mouse::Cursor) -> iced::event::Status {
        let (_, statuses) = self.ui.update(
            &iced::advanced::graphics::core::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[event],
            cursor,
            &mut self.renderer,
            &mut self.bus,
        );
        statuses[0]
    }
    fn route(&mut self, event: iced::Event) -> usize {
        let status = self.event(event.clone(), iced::mouse::Cursor::Unavailable);
        let subscription = focus_traversal::wrap_subscription(
            iced::Subscription::<Msg>::none(),
            focus_traversal::TraversalBinding::Tab,
        );
        let mut count = 0;
        for recipe in iced_test::futures::subscription::into_recipes(subscription) {
            let feed = iced_test::futures::subscription::Event::Interaction {
                window: self.window,
                event: event.clone(),
                status,
            };
            let mut messages = recipe.stream(stream::iter([feed]).boxed());
            let mut cx = Context::from_waker(Waker::noop());
            while let Poll::Ready(Some(message)) = messages.as_mut().poll_next(&mut cx) {
                let task = focus_traversal::reduce(message, |_| iced::Task::<Msg>::none());
                let actions = reducer_task::drain_ready(task);
                assert!(
                    actions.operations().is_empty(),
                    "no cross-window widget actions"
                );
                for (window, op) in actions.into_window_operations() {
                    assert_eq!(window, self.window);
                    self.run(op);
                    count += 1;
                }
            }
        }
        count
    }
    fn snapshot(&mut self) -> Probe {
        let mut probe = Probe::default();
        self.ui.operate(&self.renderer, &mut probe);
        probe
    }
    /// Draws once every reveal and focus fade has settled, on a clock of its
    /// own so two paints of the same state agree.
    fn paint(&mut self) -> Vec<u8> {
        for _ in 0..30 {
            self.now += std::time::Duration::from_millis(50);
            self.event(
                iced::Event::Window(iced::window::Event::RedrawRequested(self.now)),
                iced::mouse::Cursor::Unavailable,
            );
        }
        self.ui.draw(
            &mut self.renderer,
            &Theme::Dark,
            &iced::advanced::renderer::Style {
                text_color: Color::WHITE,
            },
            iced::mouse::Cursor::Unavailable,
        );
        self.renderer.screenshot(
            Size::new(SIZE.width as u32, SIZE.height as u32),
            1.0,
            Color::BLACK,
        )
    }
    fn visible_and_clickable(&mut self) {
        let pixels = self.paint();
        assert_eq!(pixels, self.paint(), "same-input frame control");
        let cyan = count(&pixels, RING);
        assert!(
            (80..=2000).contains(&cyan),
            "visible focused border through both clips: {cyan}"
        );
        let pink = count(&pixels, INK);
        assert!(
            (200..=180 * 110).contains(&pink),
            "bounded visible button fill: {pink}"
        );
        let at = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .position(|px| px[..3] == INK.into_rgba8()[..3])
            .unwrap();
        let point = Point::new(
            (at % SIZE.width as usize) as f32 + 1.0,
            (at / SIZE.width as usize) as f32 + 3.0,
        );
        let cursor = iced::mouse::Cursor::Available(point);
        self.event(
            iced::Event::Mouse(iced::mouse::Event::CursorMoved { position: point }),
            cursor,
        );
        self.event(
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)),
            cursor,
        );
        self.event(
            iced::Event::Mouse(iced::mouse::Event::ButtonReleased(
                iced::mouse::Button::Left,
            )),
            cursor,
        );
        assert!(
            self.bus.drain().any(|(message, _)| message == Msg::Hit),
            "visible pixels must accept activation"
        );
    }
}
fn count(pixels: &[u8], color: Color) -> usize {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[..3] == color.into_rgba8()[..3])
        .count()
}
fn key(named: iced::keyboard::key::Named, shift: bool) -> iced::Event {
    let key = iced::keyboard::Key::Named(named);
    iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: iced::keyboard::key::Physical::Unidentified(
            iced::keyboard::key::NativeCode::Unidentified,
        ),
        location: iced::keyboard::Location::Standard,
        modifiers: if shift {
            iced::keyboard::Modifiers::SHIFT
        } else {
            iced::keyboard::Modifiers::empty()
        },
        text: None,
        repeat: false,
    })
}
#[derive(Default, Debug)]
struct Probe {
    views: Vec<(Option<Id>, Vector)>,
    focused: Option<Id>,
    target: Option<Rectangle>,
}
impl Operation<()> for Probe {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<()>)) {
        operate(self);
    }
    fn scrollable(
        &mut self,
        id: Option<&Id>,
        _: Rectangle,
        _: Size,
        translation: Vector,
        _: &mut dyn operation::Scrollable,
    ) {
        self.views.push((id.cloned(), translation));
    }
    fn focusable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        state: &mut dyn operation::Focusable,
    ) {
        if state.is_focused() {
            self.focused = id.cloned();
            self.target = Some(bounds);
        }
    }
}

#[test]
fn default_tab_route_reveals_both_axes_and_anonymous_ancestors() {
    for anonymous in [false, true] {
        let mut host = Host::new(anonymous, false);
        let mut other_window = Host::new(anonymous, false);
        let other_before = other_window.paint();
        assert_eq!(
            count(&host.paint(), INK),
            0,
            "initial target is outside both clips"
        );
        assert_eq!(host.route(key(iced::keyboard::key::Named::Tab, true)), 1);
        let snapshot = host.snapshot();
        assert_eq!(snapshot.focused, Some(Id::new("deep")));
        let outer = snapshot.views[0].1;
        let inner = snapshot.views[1].1;
        assert!(
            outer.x > 0.0 && outer.y > 0.0 && inner.x > 0.0 && inner.y > 0.0,
            "all four offsets must move: {snapshot:?}"
        );
        assert_eq!(
            outer,
            Vector::new(280.0, 350.0),
            "minimum outer movement at a nonzero origin"
        );
        assert_eq!(inner, Vector::new(174.0, 186.0), "minimum inner movement");
        assert_eq!(snapshot.views[2].1, Vector::ZERO);
        host.visible_and_clickable();
        assert_eq!(host.route(key(iced::keyboard::key::Named::Tab, false)), 1);
        assert_eq!(host.snapshot().focused, Some(Id::new("first")));
        host.visible_and_clickable();
        assert_eq!(host.route(key(iced::keyboard::key::Named::Tab, false)), 1);
        assert_eq!(host.snapshot().focused, Some(Id::new("deep")));
        host.visible_and_clickable();
        assert_eq!(
            other_before,
            other_window.paint(),
            "unrelated window unchanged"
        );
        assert_eq!(other_window.snapshot().focused, None);
    }
}
