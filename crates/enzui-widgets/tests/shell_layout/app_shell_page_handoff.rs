use std::cell::Cell;

use enzui_widgets::app_shell::{ShellLayout, enz_app_shell};
use enzui_widgets::expander::enz_expander;
use enzui_widgets::nav_split::NavSplitState;
use enzui_widgets::sidebar_content::enz_sidebar_content;
use enzui_widgets::sidebar_row::enz_sidebar_row;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::Headless as _;
use iced::advanced::widget::operation::{Animation, scrollable};
use iced::advanced::widget::{Id, Operation, Tree};
use iced::advanced::{Widget, renderer};
use iced::time::{Duration, Instant};
use iced::widget::{button, column, container, text};
use iced::{Color, Element, Event, Length, Point, Rectangle, Size, Theme, mouse, window};
use iced_test::runtime::user_interface::{Cache, State, UserInterface};

const SIZE: Size = Size::new(900.0, 600.0);
const PERIOD: Duration = Duration::from_micros(16_667);
const HEADER: &str = "Appearance";
const ACTION: &str = "Incoming action";
const INCOMING_SCROLL: &str = "incoming-scroll";
// Inside the content pane and clear of the sidebar for the default shell tokens.
const PAGE_PROBE: Rectangle = Rectangle {
    x: 420.0,
    y: 120.0,
    width: 440.0,
    height: 420.0,
};
const SIDEBAR_PROBE: Rectangle = Rectangle {
    x: 16.0,
    y: 60.0,
    width: 220.0,
    height: 500.0,
};

thread_local! {
    static INCOMING_BUILDS: Cell<usize> = const { Cell::new(0) };
    static INCOMING_LAYOUTS: Cell<usize> = const { Cell::new(0) };
    static INCOMING_SNAPS: Cell<usize> = const { Cell::new(0) };
}

#[derive(Debug, Clone)]
enum Msg {
    Open(bool),
    Nav,
    PagePressed,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Model {
    open: bool,
    incoming: bool,
}

struct IncomingLeaf;

impl scrollable::Scrollable for IncomingLeaf {
    fn snap_to(
        &mut self,
        offset: scrollable::RelativeOffset<Option<f32>>,
        animation: Animation,
        _: Rectangle,
        _: Size,
    ) {
        assert_eq!(offset.y, Some(0.0));
        assert_eq!(animation, Animation::Instant);
        INCOMING_SNAPS.set(INCOMING_SNAPS.get() + 1);
    }

    fn scroll_to(
        &mut self,
        _: scrollable::AbsoluteOffset<Option<f32>>,
        _: Animation,
        _: Rectangle,
        _: Size,
    ) {
    }

    fn scroll_by(&mut self, _: scrollable::AbsoluteOffset, _: Animation, _: Rectangle, _: Size) {}
}

impl Widget<Msg, Theme, iced::Renderer> for IncomingLeaf {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fixed(24.0))
    }

    fn layout(&mut self, tree: &mut Tree, _: &iced::Renderer, limits: &layout::Limits) {
        INCOMING_LAYOUTS.set(INCOMING_LAYOUTS.get() + 1);
        layout::Node::new(limits.resolve(Length::Fill, 24.0, Size::ZERO)).apply(tree);
    }

    fn operate(
        &mut self,
        _: &mut Tree,
        layout: Layout,
        _: &Rectangle,
        _: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.scrollable(
            Some(&Id::from(INCOMING_SCROLL)),
            layout.bounds(),
            layout.size(),
            iced::Vector::ZERO,
            self,
        );
    }

    fn draw(
        &self,
        _: &Tree,
        _: &mut iced::Renderer,
        _: &Theme,
        _: &renderer::Style,
        _: Layout,
        _: mouse::Cursor,
        _: &Rectangle,
    ) {
    }
}

fn page(model: Model) -> Element<'static, Msg> {
    let (fill, body): (Color, Element<'static, Msg>) = if model.incoming {
        INCOMING_BUILDS.set(INCOMING_BUILDS.get() + 1);
        (
            Color::from_rgb(0.1, 0.3, 0.9),
            column![
                Element::new(IncomingLeaf),
                button(text(ACTION)).on_press(Msg::PagePressed),
            ]
            .into(),
        )
    } else {
        (Color::from_rgb(0.9, 0.3, 0.1), text("Outgoing page").into())
    };
    container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style::default().background(fill))
        .into()
}

fn view(model: Model) -> Element<'static, Msg> {
    enz_app_shell(move || page(model))
        .layout(ShellLayout::TitlebarSidebarContent)
        .state(NavSplitState::new())
        .on_sidebar_state(|_| Msg::Nav)
        .sidebar(move || {
            enz_sidebar_content()
                .push(
                    enz_expander(HEADER, model.open, Msg::Open)
                        .content(
                            (0..5)
                                .fold(column![].spacing(2), |rows, index| {
                                    rows.push(enz_sidebar_row(
                                        format!("Section {index}"),
                                        Msg::Open(true),
                                    ))
                                })
                                .into(),
                        )
                        .into_element(),
                )
                .push(enz_sidebar_row("Network", Msg::Open(false)))
                .into()
        })
        .into()
}

struct TextBounds(&'static str, Option<Rectangle>);

impl Operation for TextBounds {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn text(&mut self, _: Option<&Id>, bounds: Rectangle, text: &str) {
        if text == self.0 && self.1.is_none() {
            self.1 = Some(bounds);
        }
    }
}

struct Frame {
    page: Vec<u8>,
    sidebar: Vec<u8>,
    incoming_builds: usize,
    incoming_layouts: usize,
}

struct Harness {
    renderer: iced::Renderer,
    theme: Theme,
    model: Model,
    ui: Option<UserInterface<'static, Msg, Theme, iced::Renderer>>,
    now: Instant,
    cursor: mouse::Cursor,
    published: Vec<Msg>,
    asked_for_frame: bool,
}

impl Harness {
    fn new(model: Model) -> Option<Self> {
        let mut renderer = iced_test::futures::futures::executor::block_on(iced::Renderer::new(
            renderer::Settings::default(),
            Some("wgpu"),
        ))?;
        let ui = UserInterface::build(view(model), SIZE, Cache::default(), &mut renderer);
        INCOMING_BUILDS.set(0);
        INCOMING_LAYOUTS.set(0);
        INCOMING_SNAPS.set(0);
        Some(Self {
            renderer,
            theme: enzui_theme::EnzTheme::dark().iced_theme(),
            model,
            ui: Some(ui),
            now: Instant::now(),
            cursor: mouse::Cursor::Unavailable,
            published: Vec::new(),
            asked_for_frame: false,
        })
    }

    fn rebuild(&mut self) {
        let cache = self
            .ui
            .take()
            .map(UserInterface::into_cache)
            .unwrap_or_default();
        self.ui = Some(UserInterface::build(
            view(self.model),
            SIZE,
            cache,
            &mut self.renderer,
        ));
    }

    fn deliver(&mut self, event: Event) {
        let mut bus = iced::advanced::shell::Bus::new();
        let (state, _) = self.ui.as_mut().unwrap().update(
            &window::Headless,
            &iced_test::core::shell::Waker::noop(),
            &[event],
            self.cursor,
            &mut self.renderer,
            &mut bus,
        );
        self.asked_for_frame = matches!(
            state,
            State::Updated {
                redraw_request: window::RedrawRequest::NextFrame,
                ..
            }
        );
        let messages: Vec<Msg> = bus.drain_messages().collect();
        if matches!(state, State::Outdated) && messages.is_empty() {
            self.rebuild();
        }
        for message in messages {
            match message {
                Msg::Open(open) => {
                    self.model.open = open;
                    self.model.incoming = open;
                }
                Msg::Nav => {}
                Msg::PagePressed => self.published.push(Msg::PagePressed),
            }
            self.rebuild();
        }
    }

    fn move_to(&mut self, point: Point) {
        self.cursor = mouse::Cursor::Available(point);
        self.deliver(Event::Mouse(mouse::Event::CursorMoved { position: point }));
    }

    fn click(&mut self, point: Point) {
        self.move_to(point);
        self.deliver(Event::Mouse(mouse::Event::ButtonPressed(
            mouse::Button::Left,
        )));
        self.deliver(Event::Mouse(mouse::Event::ButtonReleased(
            mouse::Button::Left,
        )));
    }

    fn locate(&mut self, label: &'static str) -> Point {
        let mut probe = TextBounds(label, None);
        self.ui
            .as_mut()
            .unwrap()
            .operate(&self.renderer, &mut probe);
        probe
            .1
            .unwrap_or_else(|| panic!("{label} is shown"))
            .center()
    }

    fn frame(&mut self) -> Frame {
        self.now += PERIOD;
        self.deliver(Event::Window(window::Event::RedrawRequested(self.now)));
        let style = renderer::Style {
            text_color: Color::WHITE,
        };
        self.ui
            .as_mut()
            .unwrap()
            .draw(&mut self.renderer, &self.theme, &style, self.cursor);
        let shot = self.renderer.screenshot(
            Size::new(SIZE.width as u32, SIZE.height as u32),
            1.0,
            Color::BLACK,
        );
        Frame {
            page: crop(&shot, PAGE_PROBE),
            sidebar: crop(&shot, SIDEBAR_PROBE),
            incoming_builds: INCOMING_BUILDS.get(),
            incoming_layouts: INCOMING_LAYOUTS.get(),
        }
    }

    fn settle(&mut self) -> Frame {
        let mut frame = self.frame();
        for _ in 0..60 {
            if !self.asked_for_frame {
                break;
            }
            frame = self.frame();
        }
        frame
    }
}

fn crop(shot: &[u8], area: Rectangle) -> Vec<u8> {
    let stride = SIZE.width as usize * 4;
    let mut out = Vec::new();
    for y in area.y as usize..(area.y + area.height) as usize {
        let row = y * stride;
        out.extend_from_slice(
            &shot[row + area.x as usize * 4..row + (area.x + area.width) as usize * 4],
        );
    }
    out
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.iter().zip(b).all(|(a, b)| a.abs_diff(*b) <= 2)
}

fn harness() -> Option<Harness> {
    let harness = Harness::new(Model {
        open: false,
        incoming: false,
    });
    if harness.is_none() {
        eprintln!("SKIPPED: no wgpu adapter");
    }
    harness
}

#[test]
fn sidebar_motion_runs_over_retained_page_before_incoming_page_is_laid_out() {
    let Some(mut ui) = harness() else { return };
    let outgoing = ui.settle();
    let header = ui.locate(HEADER);
    ui.move_to(header);
    let hovered = ui.settle();
    assert!(same(&hovered.page, &outgoing.page));

    ui.click(header);
    assert!(ui.model.incoming, "the header press opened the section");
    let mut snap = scrollable::snap_to(
        Id::from(INCOMING_SCROLL),
        scrollable::RelativeOffset::START.into(),
        Animation::Instant,
    );
    ui.ui.as_mut().unwrap().operate(&ui.renderer, &mut snap);
    assert_eq!(INCOMING_BUILDS.get(), 0);
    assert_eq!(INCOMING_SNAPS.get(), 0);

    let mut held = Vec::new();
    let mut frame = ui.frame();
    while frame.incoming_layouts == 0 && held.len() < 60 {
        held.push(frame);
        frame = ui.frame();
    }
    assert!(
        held.len() >= 6,
        "the expander travel drew only {} frames before the page was handed in",
        held.len()
    );
    for (index, held) in held.iter().enumerate() {
        assert_eq!(
            held.incoming_builds, 0,
            "frame {index} built the incoming page"
        );
        assert_eq!(
            held.incoming_layouts, 0,
            "frame {index} laid out the incoming page"
        );
        assert!(
            same(&held.page, &outgoing.page),
            "frame {index} lost the retained page"
        );
    }
    let moving = held
        .windows(2)
        .filter(|pair| !same(&pair[0].sidebar, &pair[1].sidebar))
        .count();
    assert!(moving >= 5, "only {moving} held frames moved the sidebar");

    assert!(frame.incoming_builds > 0);
    assert!(frame.incoming_layouts > 0);
    assert!(
        !same(&frame.page, &outgoing.page),
        "the incoming page was not drawn"
    );
    let settled = ui.settle();
    assert_eq!(INCOMING_SNAPS.get(), 1);

    let Some(mut direct) = Harness::new(ui.model) else {
        return;
    };
    let _ = direct.settle();
    direct.move_to(header);
    let reference = direct.settle();
    assert!(same(&settled.page, &reference.page));
    assert!(same(&settled.sidebar, &reference.sidebar));
    let action = direct.locate(ACTION);
    assert_eq!(ui.locate(ACTION), action);
    ui.click(action);
    direct.click(action);
    assert_eq!(ui.published.len(), 1, "the handed-in page routes a press");
    assert_eq!(direct.published.len(), 1);
}

#[test]
fn a_press_on_the_retained_page_hands_the_page_in_without_delivering_it() {
    let Some(mut ui) = harness() else { return };
    let _ = ui.settle();
    let header = ui.locate(HEADER);
    ui.move_to(header);
    let _ = ui.settle();
    ui.click(header);
    assert_eq!(ui.frame().incoming_layouts, 0);

    ui.click(Point::new(PAGE_PROBE.x + 40.0, PAGE_PROBE.y + 40.0));
    assert!(
        ui.published.is_empty(),
        "a press aimed at retained paint reached the new page"
    );
    let handed_in = ui.frame();
    assert!(handed_in.incoming_layouts > 0);
}

#[test]
fn a_press_without_a_recorded_page_rebuilds_the_page_at_once() {
    let Some(mut ui) = harness() else { return };
    let _ = ui.settle();
    let header = ui.locate(HEADER);
    // No frame is drawn between leaving the page and pressing, so nothing is retained.
    ui.click(header);
    let frame = ui.frame();
    assert!(frame.incoming_layouts > 0);
}

#[test]
fn a_rebuild_without_pointer_input_lays_out_the_page_at_once() {
    let Some(mut ui) = harness() else { return };
    let _ = ui.settle();
    let header = ui.locate(HEADER);
    ui.move_to(header);
    let _ = ui.settle();
    ui.model = Model {
        open: true,
        incoming: true,
    };
    ui.rebuild();
    let frame = ui.frame();
    assert!(frame.incoming_layouts > 0);
}
