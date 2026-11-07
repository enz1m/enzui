//! Verifies that every window appears once under its AT-SPI application root.
//!
//! Windows receive adapter ids at creation but register on their first frame,
//! so publication order may differ from id order. The tests publish real enzUI
//! semantic frames through the vendored adapter used by the product path.

use accesskit_atspi_common::{
    Adapter, AdapterCallback, AppContext, Event, InterfaceSet, NodeId as AtspiNodeId, NodeIdOrRoot,
    ObjectEvent, PlatformRoot, WindowBounds, next_adapter_id,
};
use enzui_core::collect::SemanticsCollector;
use enzui_widgets::text::enz_text;
use iced::advanced::widget::Operation;
use iced::{Element, Size, window};
use iced_test::runtime::user_interface::{Cache, UserInterface};
use std::sync::{Arc, Mutex, RwLock};

type Renderer = iced_test::renderer::Renderer;

const VIEW_W: f32 = 640.0;
const VIEW_H: f32 = 240.0;

type Msg = ();

#[derive(Clone, Default)]
struct ChildAdditions(Arc<Mutex<Vec<(usize, usize)>>>);

impl ChildAdditions {
    fn announced(&self) -> Vec<(usize, usize)> {
        self.0.lock().expect("child additions").clone()
    }
}

impl AdapterCallback for ChildAdditions {
    fn register_interfaces(&self, _: &Adapter, _: AtspiNodeId, _: InterfaceSet) {}

    fn unregister_interfaces(&self, _: &Adapter, _: AtspiNodeId, _: InterfaceSet) {}

    fn emit_event(&self, adapter: &Adapter, event: Event) {
        if let Event::Object {
            target: NodeIdOrRoot::Root,
            event: ObjectEvent::ChildAdded(index, _),
        } = event
        {
            self.0
                .lock()
                .expect("child additions")
                .push((index, adapter.id()));
        }
    }
}

struct NoOpActionHandler;

impl accesskit::ActionHandler for NoOpActionHandler {
    fn do_action(&mut self, _: accesskit::ActionRequest) {}
}

struct DaemonWindow {
    adapter_id: usize,
    window: window::Id,
    title: &'static str,
}

fn created(titles: [&'static str; 3]) -> [DaemonWindow; 3] {
    titles.map(|title| DaemonWindow {
        adapter_id: next_adapter_id(),
        window: window::Id::unique(),
        title,
    })
}

fn frame(window: &DaemonWindow) -> accesskit::TreeUpdate {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");
    let view: Element<'_, Msg> = iced::widget::column![enz_text(window.title)].into();
    let mut ui = UserInterface::build(
        view,
        Size::new(VIEW_W, VIEW_H),
        Cache::default(),
        &mut renderer,
    );
    let mut collector = SemanticsCollector::new();
    ui.operate(&renderer, &mut collector as &mut dyn Operation);
    collector
        .to_tree_update_for_window(window.window, Some(window.title))
        .0
}

fn publish(
    window: &DaemonWindow,
    app_context: &Arc<RwLock<AppContext>>,
    events: &ChildAdditions,
) -> Adapter {
    Adapter::with_id(
        window.adapter_id,
        app_context,
        events.clone(),
        frame(window),
        false,
        WindowBounds::default(),
        NoOpActionHandler,
    )
}

fn children(root: &PlatformRoot) -> Vec<(usize, i32, String)> {
    let count = root.child_count().expect("application child count");
    (0..count as usize)
        .map(|index| {
            let (adapter_id, _) = root
                .child_id_at_index(index)
                .expect("child id")
                .expect("child at index");
            let node = root
                .child_at_index(index)
                .expect("child")
                .expect("child at index");
            (
                adapter_id,
                node.index_in_parent().expect("index in parent"),
                node.name().expect("child name"),
            )
        })
        .collect()
}

#[test]
fn a_single_window_daemon_publishes_one_frame() {
    // Two windows exist and never publish, so the surviving window's adapter id
    // is not zero -- the shape the single-window control in the report had.
    let [_desktop, _sidebar, pane] = created(["Desktop", "Sidebar", "Pane"]);
    let app_context = AppContext::new(None);
    let events = ChildAdditions::default();

    let _published_pane = publish(&pane, &app_context, &events);

    let root = PlatformRoot::new(&app_context);
    assert_eq!(
        children(&root),
        vec![(pane.adapter_id, 0, "Pane".to_owned())]
    );
    assert_eq!(events.announced(), vec![(0, pane.adapter_id)]);
}
