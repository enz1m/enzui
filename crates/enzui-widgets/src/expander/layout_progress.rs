use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{self, Widget};
use iced::advanced::{mouse, renderer};
use iced::widget::{column, text};
use iced::{Element, Event, Length, Point, Rectangle, Size, window};
use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, State, UserInterface};
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

struct Probe(Rc<Cell<f32>>);

impl<Message> Widget<Message, iced::Theme, Renderer> for Probe {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(10.0), Length::Fixed(10.0))
    }

    fn layout(&mut self, tree: &mut widget::Tree, _: &Renderer, _: &layout::Limits) {
        layout::Node::new(Size::new(10.0, 10.0)).apply(tree);
    }

    fn draw(
        &self,
        _: &widget::Tree,
        _: &mut Renderer,
        _: &iced::Theme,
        _: &renderer::Style,
        layout: Layout,
        _: mouse::Cursor,
        _: &Rectangle,
    ) {
        self.0.set(layout.bounds().y);
    }
}

fn view(expanded: bool, probe: Rc<Cell<f32>>) -> Element<'static, (), iced::Theme, Renderer> {
    let body = column((0..6).map(|row| text(format!("Row {row}")).into()));
    column![
        super::enz_expander("Section", expanded, |_| ())
            .reduced_motion(Some(false))
            .content(body.into()),
        Element::new(Probe(probe)),
    ]
    .into()
}

struct Live {
    ui: Option<UserInterface<'static, (), iced::Theme, Renderer>>,
    renderer: Renderer,
    probe: Rc<Cell<f32>>,
}

impl Live {
    const BOUNDS: Size = Size::new(400.0, 600.0);

    fn new(expanded: bool) -> Self {
        let mut renderer = iced::futures::executor::block_on(
            <Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                None,
            ),
        )
        .expect("headless renderer");
        let probe = Rc::new(Cell::new(f32::NAN));
        let ui = UserInterface::build(
            view(expanded, Rc::clone(&probe)),
            Self::BOUNDS,
            Cache::default(),
            &mut renderer,
        );
        let mut live = Self {
            ui: Some(ui),
            renderer,
            probe,
        };
        live.draw();
        live
    }

    fn draw(&mut self) {
        let theme = iced::Theme::Dark;
        let style = renderer::Style {
            text_color: iced::Color::WHITE,
        };
        let cursor = mouse::Cursor::Available(Point::new(5000.0, 5000.0));
        let ui = self.ui.as_mut().expect("live ui");
        ui.draw(&mut self.renderer, &theme, &style, cursor);
    }

    fn rebuild(&mut self, expanded: bool) {
        let cache = self.ui.take().expect("live ui").into_cache();
        self.ui = Some(UserInterface::build(
            view(expanded, Rc::clone(&self.probe)),
            Self::BOUNDS,
            cache,
            &mut self.renderer,
        ));
        self.draw();
    }

    fn deliver(&mut self, event: Event) -> (bool, bool, f32) {
        let mut messages = iced::advanced::shell::Bus::new();
        let (state, _) = self.ui.as_mut().expect("live ui").update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[event],
            mouse::Cursor::Available(Point::new(5000.0, 5000.0)),
            &mut self.renderer,
            &mut messages,
        );
        self.draw();
        let asked = matches!(
            state,
            State::Updated {
                redraw_request: window::RedrawRequest::NextFrame | window::RedrawRequest::At(_),
                ..
            }
        );
        (state.has_layout_changed(), asked, self.probe.get())
    }

    fn frame(&mut self, at: Instant) -> (bool, bool, f32) {
        self.deliver(Event::Window(window::Event::RedrawRequested(at)))
    }
}

fn cursor_moved() -> Event {
    Event::Mouse(mouse::Event::CursorMoved {
        position: Point::new(4000.0, 4000.0),
    })
}

#[test]
fn an_opening_relays_out_once_per_frame_and_lands_on_the_settled_height() {
    let settled = Live::new(true).probe.get();
    let mut live = Live::new(false);
    let collapsed = live.probe.get();
    assert!(
        settled > collapsed,
        "the body has height: {collapsed} -> {settled}"
    );

    live.rebuild(true);
    let (_, asked, _) = live.deliver(cursor_moved());
    assert!(asked, "the event after the toggle asks for the first frame");

    let start = Instant::now();
    let _ = live.frame(start);
    let (relaid, asked, mid) = live.frame(start + Duration::from_millis(60));
    assert!(relaid && asked, "a new frame instant relays out mid-flight");
    assert!(collapsed < mid && mid < settled, "mid-flight height: {mid}");

    let (relaid, asked, y) = live.deliver(cursor_moved());
    assert!(!relaid, "an event without a new instant keeps the layout");
    assert!(asked, "the travel still asks for frames");
    assert_eq!(y, mid);

    let (relaid, _, y) = live.frame(start + Duration::from_millis(60));
    assert!(!relaid, "a repeated instant keeps the layout");
    assert_eq!(y, mid);

    let (relaid, asked, y) = live.frame(start + Duration::from_secs(1));
    assert!(relaid, "the settling frame relays out");
    assert!(!asked, "a settled expander asks for no frame");
    assert_eq!(y, settled, "the settled frame shows the full body");

    let (relaid, asked, _) = live.frame(start + Duration::from_secs(2));
    assert!(!relaid && !asked, "a settled expander invalidates nothing");
}
