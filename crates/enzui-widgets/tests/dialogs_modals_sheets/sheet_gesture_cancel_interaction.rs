use enzui_widgets::sheet::{SheetGesture, enz_sheet};
use iced::advanced::graphics::core::shell::Waker;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::Tree;
use iced::advanced::{Shell, layout, mouse};
use iced::widget::Space;
use iced::window::Headless;
use iced::{Element, Event, Length, Point, Rectangle, Size};

const W: f32 = 400.0;
const H: f32 = 600.0;
const SHEET_HEIGHT: f32 = 400.0;
const PRESS_AT: Point = Point::new(200.0, 300.0);
const MOVE_A: Point = Point::new(200.0, 340.0);
const MOVE_B: Point = Point::new(200.0, 400.0);

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Drag(SheetGesture),
}

fn built() -> Element<'static, Msg> {
    enz_sheet(Space::new().width(Length::Fill).height(Length::Fill))
        .height(SHEET_HEIGHT)
        .on_drag(Msg::Drag)
        .into_element()
}

fn make_renderer() -> iced::Renderer {
    iced_test::futures::futures::executor::block_on(
        <iced::Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer")
}

fn deliver(
    element: &mut Element<'_, Msg>,
    tree: &mut Tree,
    node: &layout::Node,
    renderer: &iced::Renderer,
    event: Event,
    at: Point,
) -> Vec<Msg> {
    let mut messages = iced::advanced::shell::Bus::new();
    let window = Headless;
    let mut shell = Shell::new(&window, Waker::noop(), &mut messages);
    element.as_widget_mut().update(
        tree,
        &event,
        iced::advanced::Layout::of(node),
        mouse::Cursor::Available(at),
        renderer,
        &mut shell,
        &Rectangle {
            x: 0.0,
            y: 0.0,
            width: W,
            height: H,
        },
    );
    messages.into_iter().collect()
}

#[test]
fn sheet_cancel_swallows_the_live_swipe() {
    let mut element: Element<'static, Msg> = built();
    let mut tree = Tree::new(&element);
    element.as_widget_mut().diff(&mut tree);
    let renderer = make_renderer();
    let node = element.as_widget_mut().measure_snapshot(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, Size::new(W, H)),
    );

    let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
    let _ = deliver(&mut element, &mut tree, &node, &renderer, press, PRESS_AT);
    let move_a = Event::Mouse(mouse::Event::CursorMoved { position: MOVE_A });
    let armed = deliver(&mut element, &mut tree, &node, &renderer, move_a, MOVE_A);
    assert!(
        armed
            .iter()
            .any(|m| matches!(m, Msg::Drag(SheetGesture::Moved { .. }))),
        "sanity: the move must publish while the swipe is live; got {armed:?}"
    );

    assert!(
        tree.cancel_interactions() > 0,
        "sheet gesture opt-in must publish at least one Interactive slot"
    );

    let move_b = Event::Mouse(mouse::Event::CursorMoved { position: MOVE_B });
    let post = deliver(&mut element, &mut tree, &node, &renderer, move_b, MOVE_B);
    assert!(
        post.iter()
            .all(|m| !matches!(m, Msg::Drag(SheetGesture::Moved { .. }))),
        "after cancel_interactions the swipe is over; got {post:?}"
    );
}
