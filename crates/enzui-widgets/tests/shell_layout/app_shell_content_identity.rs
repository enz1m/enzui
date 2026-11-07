use enzui_core::collect::SemanticsCollector;
use enzui_widgets::app_shell::{ShellLayout, ShellTokens, SidebarHeader, enz_app_shell};
use enzui_widgets::button::enz_button;
use enzui_widgets::keyed_column::keyed_column;
use enzui_widgets::nav_split::NavSplitState;
use enzui_widgets::scroll_view::enz_scroll_view;
use iced::advanced::widget::{Id, Operation, Tree, tree};
use iced::advanced::{Layout, Widget, layout, renderer};
use iced::widget::{column, text};
use iced::{Element, Event, Length, Point, Rectangle, Size, Theme, Vector, mouse};
use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, UserInterface};

const SCROLLER: &str = "content-identity.scroll";
const LOCAL_STATE: &str = "content-identity.local-state";
const SIDEBAR_ACTION: &str = "content-identity.sidebar-action";

#[derive(Debug, Clone)]
enum Msg {
    Nav(NavSplitState),
    Sidebar,
}

// Unlike a text input's focus/caret, this state has no shell checkpoint fallback.
struct LocalState;
impl Widget<Msg, Theme, Renderer> for LocalState {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(1.0), Length::Fixed(1.0))
    }
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<u64>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(0_u64)
    }
    fn layout(&mut self, tree: &mut Tree, _: &Renderer, _: &layout::Limits) {
        layout::Node::new(Size::new(1.0, 1.0)).apply(tree);
    }
    fn draw(
        &self,
        _: &Tree,
        _: &mut Renderer,
        _: &Theme,
        _: &renderer::Style,
        _: Layout,
        _: mouse::Cursor,
        _: &Rectangle,
    ) {
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        _viewport: &Rectangle,
        _: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.custom(
            Some(&Id::new(LOCAL_STATE)),
            layout.bounds(),
            tree.state.downcast_mut::<u64>(),
        );
    }
}

#[derive(Default)]
struct Probe {
    offset: Option<f32>,
    width: Option<f32>,
    local: Option<u64>,
    set_local: Option<u64>,
    sidebar_focusable: bool,
}
impl Operation for Probe {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }
    fn scrollable(
        &mut self,
        id: Option<&Id>,
        bounds: Rectangle,
        _: Size,
        translation: Vector,
        _: &mut dyn iced::advanced::widget::operation::Scrollable,
    ) {
        if id == Some(&Id::new(SCROLLER)) {
            self.offset = Some(translation.y);
            self.width = Some(bounds.width);
        }
    }
    fn custom(&mut self, id: Option<&Id>, _: Rectangle, state: &mut dyn std::any::Any) {
        if id == Some(&Id::new(LOCAL_STATE)) {
            let value = state.downcast_mut::<u64>().unwrap();
            if let Some(next) = self.set_local {
                *value = next;
            }
            self.local = Some(*value);
        }
    }
    fn focusable(
        &mut self,
        id: Option<&Id>,
        _: Rectangle,
        _: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        self.sidebar_focusable |= id == Some(&Id::new(SIDEBAR_ACTION));
    }
}

struct Rig {
    renderer: Renderer,
    cache: Cache,
    nav: NavSplitState,
    header: SidebarHeader,
    size: Size,
    clock: iced::time::Instant,
    page: u64,
    nested_sidebar: bool,
}
impl Rig {
    fn new(header: SidebarHeader) -> Self {
        let renderer = iced_test::futures::futures::executor::block_on(
            <Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        Self {
            renderer,
            cache: Cache::default(),
            nav: NavSplitState::new(),
            header,
            size: Size::new(1024.0, 768.0),
            clock: iced::time::Instant::now(),
            page: 0,
            nested_sidebar: false,
        }
    }
    fn view(&self) -> Element<'static, Msg> {
        let page = self.page;
        let nested_sidebar = self.nested_sidebar;
        enz_app_shell(move || {
            let rows = (0..70).map(|row| text(format!("Row {row}")).height(40).into());
            let content = column![
                Element::new(LocalState),
                enz_scroll_view(iced::widget::Column::with_children(rows).width(Length::Fill))
                    .id(Id::new(SCROLLER))
                    .height(Length::Fill)
            ]
            .height(Length::Fill);
            keyed_column(vec![(page.to_string(), content.into())])
                .height(Length::Fill)
                .into()
        })
        .layout(ShellLayout::TitlebarSidebarContent)
        .tokens(ShellTokens {
            sidebar_header: self.header,
            ..ShellTokens::default()
        })
        .titlebar(|| text("Settings").into())
        .sidebar(move || {
            if nested_sidebar {
                enz_app_shell(|| text("Nested shell content").into())
                    .layout(ShellLayout::ContentOnly)
                    .into()
            } else {
                enz_button("Sidebar action", Msg::Sidebar)
                    .id(Id::new(SIDEBAR_ACTION))
                    .into()
            }
        })
        .state(self.nav)
        .on_sidebar_state(Msg::Nav)
        .into()
    }
    fn build(&mut self) -> UserInterface<'static, Msg, Theme, Renderer> {
        let mut ui = UserInterface::build(
            self.view(),
            self.size,
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        ui.draw(
            &mut self.renderer,
            &Theme::Dark,
            &renderer::Style {
                text_color: iced::Color::WHITE,
            },
            mouse::Cursor::Unavailable,
        );
        ui
    }
    fn events(&mut self, cursor: Point, events: &[Event]) -> Vec<Msg> {
        let mut ui = self.build();
        let mut messages = iced::advanced::shell::Bus::new();
        ui.update(
            &iced::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            mouse::Cursor::Available(cursor),
            &mut self.renderer,
            &mut messages,
        );
        self.cache = ui.into_cache();
        for message in &messages {
            if let Msg::Nav(nav) = message {
                self.nav = *nav;
            }
        }
        messages.into_iter().collect()
    }
    /// Runs `millis` of frames, one every 16 ms as a running app gets them: a
    /// docked travel advances at most a step per frame.
    fn advance(&mut self, millis: u64) {
        let mut left = millis;
        while left > 0 {
            let step = left.min(16);
            left -= step;
            self.clock += iced::time::Duration::from_millis(step);
            self.events(
                Point::new(-1.0, -1.0),
                &[Event::Window(iced::window::Event::RedrawRequested(
                    self.clock,
                ))],
            );
        }
    }
    fn probe(&mut self, set_local: Option<u64>) -> Probe {
        let mut ui = self.build();
        let mut probe = Probe {
            set_local,
            ..Probe::default()
        };
        ui.operate(&self.renderer, &mut probe);
        self.cache = ui.into_cache();
        probe
    }
    fn nodes(&mut self) -> Vec<enzui_core::collect::CollectedNode> {
        let mut ui = self.build();
        let mut collector = SemanticsCollector::new();
        ui.operate(&self.renderer, &mut collector);
        self.cache = ui.into_cache();
        collector.into_collected()
    }
    fn toggle(&mut self) {
        let nodes = self.nodes();
        let toggles: Vec<_> = nodes
            .iter()
            .filter(|node| {
                node.semantics.role == enzui_core::semantics::Role::Button
                    && matches!(
                        node.semantics.label.as_deref(),
                        Some("Hide sidebar" | "Show sidebar")
                    )
            })
            .collect();
        assert_eq!(
            toggles.len(),
            1,
            "one real shell toggle at {:?} with {:?}: {toggles:?}",
            self.size,
            self.nav
        );
        let at = toggles[0].bounds.center();
        let messages = self.events(
            at,
            &[
                Event::Mouse(mouse::Event::CursorMoved { position: at }),
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            ],
        );
        assert_eq!(
            messages
                .iter()
                .filter(|msg| matches!(msg, Msg::Nav(_)))
                .count(),
            1
        );
        self.probe(None);
        self.clock = enzui_widgets::app_shell::build_probe::take_motion_edge()
            .expect("the real toggle must start a presentation transition");
    }
    fn scroll(&mut self, pixels: f32) {
        self.events(
            Point::new(self.size.width - 70.0, 400.0),
            &[Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Pixels { x: 0.0, y: -pixels },
            })],
        );
    }
    fn assert_retained(&mut self, expected: f32) {
        let probe = self.probe(None);
        assert!(
            (probe.offset.expect("content scrollable") - expected).abs() < 1.0,
            "content scroll offset reset: {:?}, expected {expected}",
            probe.offset
        );
        assert_eq!(
            probe.local,
            Some(37),
            "non-scroll widget state was replaced"
        );
    }
}

#[test]
fn content_state_survives_docked_sidebar_endpoint_and_reopen() {
    for header in [
        SidebarHeader::Inside,
        SidebarHeader::SpanningBand,
        SidebarHeader::SplitBand,
    ] {
        let mut rig = Rig::new(header);
        rig.advance(1);
        rig.probe(Some(37));
        rig.scroll(820.0);
        let offset = rig.probe(None).offset.unwrap();
        assert!(
            offset > 600.0,
            "wheel must move content well away from zero"
        );
        assert!(
            rig.probe(None).sidebar_focusable,
            "live sidebar participates in focus traversal"
        );
        let sidebar = rig
            .nodes()
            .into_iter()
            .find(|node| node.semantics.label.as_deref() == Some("Sidebar action"))
            .expect("live sidebar positive control");
        let clicks = [
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
        ];
        assert!(
            rig.events(sidebar.bounds.center(), &clicks)
                .iter()
                .any(|msg| matches!(msg, Msg::Sidebar))
        );
        rig.toggle();
        rig.advance(1);
        rig.advance(500);
        rig.assert_retained(offset);
        assert!(
            !rig.probe(None).sidebar_focusable,
            "hidden sidebar leaves focus traversal"
        );
        assert!(
            !rig.nodes()
                .iter()
                .any(|node| node.semantics.label.as_deref() == Some("Sidebar action"))
        );
        assert!(
            !rig.events(sidebar.bounds.center(), &clicks)
                .iter()
                .any(|msg| matches!(msg, Msg::Sidebar))
        );
        rig.toggle();
        rig.advance(1);
        rig.advance(500);
        rig.assert_retained(offset);
        rig.scroll(140.0);
        assert!(rig.probe(None).offset.unwrap() > offset + 100.0);
        rig.page += 1;
        let fresh = rig.probe(None);
        assert_eq!(
            fresh.offset,
            Some(0.0),
            "real keyed navigation must reset scroll"
        );
        assert_eq!(
            fresh.local,
            Some(0),
            "real keyed navigation must reset local state"
        );
    }
}
