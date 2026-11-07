use std::cell::Cell;
use std::rc::Rc;

use enzui_widgets::viewport_gate::ViewportGate;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::Tree;
use iced::advanced::{Shell, Widget, mouse, renderer};
use iced::widget::{Space, column, scrollable};
use iced::window::RedrawRequest;
use iced::{Element, Event, Length, Point, Rectangle, Size, Theme};
use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, State, UserInterface};

const PANEL_H: f32 = 300.0;
const SPACER_H: f32 = 400.0;
const LEAF_H: f32 = 120.0;
const WHEEL_LINES: f32 = -3.0;
struct AnimLeaf {
    redraws: Rc<Cell<u32>>,
}

impl Widget<(), Theme, Renderer> for AnimLeaf {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(LEAF_H))
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &Renderer, limits: &layout::Limits) {
        layout::Node::new(Size::new(limits.max().width, LEAF_H)).apply(tree);
    }

    fn draw(
        &self,
        _tree: &Tree,
        _renderer: &mut Renderer,
        _theme: &Theme,
        _style: &renderer::Style,
        _layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
    }

    fn frame_interest(&self, _tree: &Tree) -> bool {
        true
    }

    fn update(
        &mut self,
        _tree: &mut Tree,
        event: &Event,
        _layout: Layout,
        _cursor: mouse::Cursor,
        _renderer: &Renderer,
        shell: &mut Shell<'_, ()>,
        _viewport: &Rectangle,
    ) {
        if matches!(
            event,
            Event::Window(iced::window::Event::RedrawRequested(_))
        ) {
            self.redraws.set(self.redraws.get() + 1);
            shell.request_redraw();
        }
    }
}

fn panel_with(
    redraws: &Rc<Cell<u32>>,
    id: iced::advanced::widget::Id,
    spacer: f32,
    height: Length,
) -> Element<'static, (), Theme, Renderer> {
    let gated = ViewportGate::new(Element::new(AnimLeaf {
        redraws: Rc::clone(redraws),
    }));
    let content = column![
        Space::new()
            .width(Length::Fill)
            .height(Length::Fixed(spacer)),
        Element::from(gated),
    ];
    scrollable(content)
        .id(id)
        .width(Length::Fill)
        .height(height)
        .into()
}

fn panel(
    redraws: &Rc<Cell<u32>>,
    id: iced::advanced::widget::Id,
) -> Element<'static, (), Theme, Renderer> {
    panel_with(redraws, id, SPACER_H, Length::Fixed(PANEL_H))
}

fn view(
    scrolled: &Rc<Cell<u32>>,
    still: &Rc<Cell<u32>>,
    _collapsible: &Rc<Cell<f32>>,
) -> Element<'static, (), Theme, Renderer> {
    column![
        panel(scrolled, iced::advanced::widget::Id::new("scrolled")),
        panel(still, iced::advanced::widget::Id::new("still")),
    ]
    .into()
}

fn renderer() -> Renderer {
    iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer")
}

fn redraw_of(state: State) -> RedrawRequest {
    match state {
        State::Updated { redraw_request, .. } => redraw_request,
        State::Outdated => panic!("the tree went outdated; the probe measured nothing"),
    }
}
type ViewFn =
    fn(&Rc<Cell<u32>>, &Rc<Cell<u32>>, &Rc<Cell<f32>>) -> Element<'static, (), Theme, Renderer>;
#[allow(dead_code, reason = "fixture fields are written but never read")]
struct Run {
    scrolled: Rc<Cell<u32>>,
    still: Rc<Cell<u32>>,
    renderer: Renderer,
    ui: Option<UserInterface<'static, (), Theme, Renderer>>,
    clock: u64,
    t0: iced::time::Instant,
    view: ViewFn,
    window: Size,
    collapsible: Rc<Cell<f32>>,
}

impl Run {
    fn new() -> Self {
        Self::with(view, Size::new(400.0, 2.0 * PANEL_H + 20.0))
    }

    fn with(view: ViewFn, window: Size) -> Self {
        let scrolled = Rc::new(Cell::new(0));
        let still = Rc::new(Cell::new(0));
        let collapsible = Rc::new(Cell::new(COLLAPSIBLE_H));
        let mut renderer = renderer();
        let ui = UserInterface::build(
            view(&scrolled, &still, &collapsible),
            window,
            Cache::default(),
            &mut renderer,
        );

        Self {
            scrolled,
            still,
            renderer,
            ui: Some(ui),
            clock: 0,
            t0: iced::time::Instant::now(),
            view,
            window,
            collapsible,
        }
    }

    fn draw(&mut self) {
        let style = iced::advanced::renderer::Style {
            text_color: iced::Color::BLACK,
        };
        let theme = Theme::Dark;
        let Self { ui, renderer, .. } = self;
        ui.as_mut().expect("live user interface").draw(
            renderer,
            &theme,
            &style,
            mouse::Cursor::Unavailable,
        );
    }
    fn dispatch(&mut self, events: &[Event], cursor: mouse::Cursor) -> RedrawRequest {
        use iced::advanced::graphics::core::shell::Waker;

        let mut messages: iced::advanced::shell::Bus<()> = iced::advanced::shell::Bus::new();
        let Self { ui, renderer, .. } = self;
        let (state, _) = ui.as_mut().expect("live user interface").update(
            &iced::window::Headless,
            &Waker::noop(),
            events,
            cursor,
            renderer,
            &mut messages,
        );
        self.draw();
        redraw_of(state)
    }
    fn frame(&mut self) -> RedrawRequest {
        self.clock += 1;
        let at = self.t0 + iced::time::Duration::from_millis(self.clock * 16);
        self.dispatch(
            &[Event::Window(iced::window::Event::RedrawRequested(at))],
            mouse::Cursor::Unavailable,
        )
    }
    fn settle_offscreen(&mut self) {
        self.draw();
        let first = self.frame();
        assert!(
            self.scrolled.get() >= 1 && self.still.get() >= 1,
            "calibration: the first frame must reach both gated leaves before \
             they cache themselves offscreen; got scrolled={} still={}",
            self.scrolled.get(),
            self.still.get()
        );
        assert_eq!(
            self.frame(),
            RedrawRequest::Wait,
            "both animators are wholly offscreen, so the runtime must request \
             nothing; the first frame asked for {first:?}"
        );
        let (a, b) = (self.scrolled.get(), self.still.get());
        assert_eq!(
            (self.frame(), self.frame()),
            (RedrawRequest::Wait, RedrawRequest::Wait),
            "offscreen animators must stay quiet"
        );
        assert_eq!(
            (self.scrolled.get(), self.still.get()),
            (a, b),
            "a settled offscreen subtree must be off the frame route entirely"
        );
        self.scrolled.set(0);
        self.still.set(0);
    }
    // Window-wide geometry probes may reach the hidden control once; wheel routing may not.
    fn assert_reentry(&mut self, label: &str, control_budget: u32) {
        let profile = [self.frame(), self.frame(), self.frame()];
        let control_after_probe = self.still.get();

        assert!(
            self.scrolled.get() >= 1,
            "{label}: the scrolled-in animator must receive a real runtime \
             redraw after the transition; it received {} (redraw profile \
             {profile:?})",
            self.scrolled.get()
        );
        assert!(
            profile.contains(&RedrawRequest::NextFrame),
            "{label}: after the transition the runtime must request an \
             animation frame again, not {profile:?}"
        );
        assert_eq!(
            profile[2],
            RedrawRequest::NextFrame,
            "{label}: the re-armed animation must keep running, not fire once; \
             got {profile:?}"
        );
        assert_eq!(
            control_after_probe, control_budget,
            "{label}: the panel that did not move is entitled to \
             {control_budget} frame(s); it received {control_after_probe}"
        );

        let tail = [
            self.frame(),
            self.frame(),
            self.frame(),
            self.frame(),
            self.frame(),
        ];

        assert_eq!(
            self.still.get(),
            control_budget,
            "{label}: the panel that did not move must be off the frame route \
             by now; it received {} redraws in total — a bounded one-shot \
             route has become a permanent fan-out (tail {tail:?})",
            self.still.get()
        );
        assert_eq!(
            tail[4],
            RedrawRequest::NextFrame,
            "{label}: the moved animator must still be animating at the end \
             of the tail; got {tail:?}"
        );
    }
}

#[test]
fn wheel_scroll_reroutes_the_gated_animator_once() {
    let mut run = Run::new();
    run.settle_offscreen();

    let _ = run.dispatch(
        &[Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Lines {
                x: 0.0,
                y: WHEEL_LINES,
            },
        })],
        mouse::Cursor::Available(Point::new(200.0, PANEL_H / 2.0)),
    );

    run.assert_reentry("wheel scroll", 0);
}

const COLLAPSIBLE_H: f32 = 900.0;
