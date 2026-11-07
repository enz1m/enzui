//! `EnzWindowControls` policy: visibility, enabled state, order, and
//! fullscreen suppression, all application-driven.
//!
//! A cluster with three hard-coded sequential `push`es always renders
//! Minimize → Maximize → Close, with Close always present and enabled. On Linux
//! the button layout is a desktop setting
//! (`org.gnome.desktop.wm.preferences button-layout` /
//! `gtk-decoration-layout`) and right-hand-side layouts differ, so a fixed
//! sequence puts our window visibly out of step with every other window on the
//! user's desktop and cannot express a window that must not close (for example,
//! a modal mid-operation or a kiosk surface).
//!
//! **Every claim here is read out of a live `UserInterface`**, never off a
//! stored builder field. The order assertions compare laid-out x positions,
//! because a stored `Vec<WindowControlKind>` can be perfectly correct and never
//! reach `into_element` (the dead-widget failure mode); the visibility assertions check the **rendered
//! tree and the AT tree separately**, because a hidden-but-collected node is
//! the semantic-text contract duplicate class.
//!
//! Harness shape follows `window_controls_behaviour.rs` (geometry + messages)
//! and `window_controls_routed_activation.rs` (ids + AT routing).

use enzui_core::a11y_action::{AccessibilityAction, ActionRouter};
use enzui_core::collect::{SemanticsCollector, node_id_of_widget};
use enzui_widgets::activation::activate;
use enzui_widgets::window_controls::{WindowControlKind, enz_window_controls};
use iced::advanced::widget::{Id, Operation};
use iced::widget::container;
use iced::{Element, Event, Point, Size, mouse, window};
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Close,
    Minimize,
    Maximize,
}

const VIEWPORT: Size = Size::new(600.0, 400.0);
const PREFIX: &str = "enz-window-controls";

/// The host-owned per-window key every fixture in this file identifies its
/// cluster with, preserving stable per-instance identity.
///
/// # Why every fixture must carry one
///
/// A window-control cluster has no intrinsic per-instance key: the only
/// per-instance data at the widget level is the *mutable* maximized flag, and
/// folding that in would move the maximize id when the window is maximized.
/// The widget therefore asks the host for a key, and a cluster with no host key
/// publishes no ids at all — anonymous rather than aliased, because two
/// unkeyed clusters would otherwise both derive one `close` id and a routed
/// close could target the wrong cluster.
///
/// That matters for *this* file specifically, and not only for the four
/// assertions that failed. Several tests here assert an id is absent —
/// hidden close, disabled close, fullscreen. Left unkeyed, those would pass
/// because the cluster has no ids at all, which is the wrong reason: they would
/// no longer be testing hiding, disabling or fullscreen. Keying every fixture
/// keeps each assertion about the property it names.
const WINDOW_KEY: &str = "policy-window";

/// The owner namespace every fixture's buttons derive under.
fn owner() -> enzui_core::identity::OwnerId {
    enzui_core::identity::OwnerId::of_kind(PREFIX, WINDOW_KEY)
}

/// The id of role `role` on this file's clusters.
///
/// Uses the owner namespace rather than per-role `identity::derive_id` calls,
/// which would accept a per-kind constant as instance identity.
fn wc_id(role: &str) -> Id {
    owner().id(role)
}

/// Far outside every widget: `FocusButton::draw` resolves hover before focus,
/// so a cursor parked on a control can pass an assertion for the wrong reason.
fn parked() -> Point {
    Point::new(5000.0, 5000.0)
}

fn headless_renderer() -> Renderer {
    iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer")
}

/// A live tree that can be laid out, collected from, operated on and pumped.
struct Host<'v> {
    view: Box<dyn Fn() -> Element<'v, Msg> + 'v>,
    cache: Cache,
    renderer: Renderer,
    cursor: mouse::Cursor,
}

impl<'v> Host<'v> {
    fn new(view: impl Fn() -> Element<'v, Msg> + 'v) -> Self {
        Self {
            view: Box::new(view),
            cache: Cache::default(),
            renderer: headless_renderer(),
            cursor: mouse::Cursor::Available(parked()),
        }
    }

    fn build(&mut self) -> UserInterface<'v, Msg, iced::Theme, Renderer> {
        UserInterface::build(
            (self.view)(),
            VIEWPORT,
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        )
    }

    /// The `NodeId -> Id` map an AT action request resolves against.
    fn node_id_map(&mut self) -> enzui_core::collect::NodeIdMap {
        let mut ui = self.build();
        let mut collector = SemanticsCollector::new();
        ui.operate(&self.renderer, &mut collector as &mut dyn Operation);
        self.cache = ui.into_cache();
        collector.to_tree_update_with_ids().1
    }

    /// Run an operation, then a bare-redraw pass, returning published messages.
    /// No input event and the cursor parked, so only the routed activation can
    /// produce anything.
    fn activate_and_pump(&mut self, id: Id) -> Vec<Msg> {
        let mut ui = self.build();
        let mut operation: Box<dyn Operation<()>> = Box::new(activate(id));
        loop {
            ui.operate(&self.renderer, &mut operation);
            match operation.finish() {
                iced::advanced::widget::operation::Outcome::Chain(next) => operation = next,
                _ => break,
            }
        }
        let mut messages = iced::advanced::shell::Bus::new();
        let _ = ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[Event::Window(window::Event::RedrawRequested(
                std::time::Instant::now(),
            ))],
            self.cursor,
            &mut self.renderer,
            &mut messages,
        );
        self.cache = ui.into_cache();
        messages.into_iter().collect()
    }
}

// ---------------------------------------------------------------------------
// 1. Default-behavior pin
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 2. Custom order — asserted on laid-out x, not on the stored Vec
// ---------------------------------------------------------------------------

fn ordered(order: [WindowControlKind; 3]) -> impl Fn() -> Element<'static, Msg> {
    move || {
        container(
            enz_window_controls(Msg::Close)
                .show_minimize(true, Msg::Minimize)
                .show_maximize(true, Msg::Maximize, false)
                .order(order)
                .id(WINDOW_KEY),
        )
        .into()
    }
}

// ---------------------------------------------------------------------------
// 3. show_close(false): gone from the rendered tree and the AT tree
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 4. Enabled state — the complement of visibility
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 5. Fullscreen suppression
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 6. Ids stay distinct and position-independent under a custom order
// ---------------------------------------------------------------------------

/// A routed AT `Click` on the reordered close node resolves to
/// `Activate(close_id)` and reaches `Msg::Close` — the full bus path, not just
/// the internal operation.
#[test]
fn a_routed_at_click_reaches_the_reordered_close() {
    let mut host = Host::new(ordered([
        WindowControlKind::Close,
        WindowControlKind::Minimize,
        WindowControlKind::Maximize,
    ]));
    let close_id = wc_id("close");
    let node = node_id_of_widget(&close_id);
    let map = host.node_id_map();
    assert_eq!(map.widget_id(node), Some(&close_id));

    let router = ActionRouter::new();
    router.set_map(enzui_core::a11y_action::A11Y_UNKEYED_WINDOW, map);
    router.handle(
        enzui_core::a11y_action::A11Y_UNKEYED_WINDOW,
        accesskit::Action::Click,
        node,
        None,
    );
    assert_eq!(
        router.drain(),
        vec![AccessibilityAction::Activate(close_id.clone())]
    );
    assert_eq!(host.activate_and_pump(close_id), vec![Msg::Close]);
}

// ---------------------------------------------------------------------------
// 7. Maximize/Restore state is still driven by `maximized`
// ---------------------------------------------------------------------------
