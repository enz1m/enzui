//! Behavioural tests for [`EnzContextMenuHost`].

use enzui_core::collect::{CollectedNode, SemanticsCollector};
use enzui_core::menu_model::{CloseCause, EntryId, MenuEntryModel, MenuModel};
use enzui_core::semantics::Role;
use enzui_widgets::context_menu_host::{
    ContextMenuEvent, ContextMenuState, apply_event, enz_context_menu_host,
    enz_point_context_menu_host, focus_restore_operation,
};
use iced::advanced::widget::{Id, Operation, operation};
use iced::keyboard::key::Named;
use iced::widget::{column, container, scrollable, text};
use iced::{Element, Event, Length, Padding, Point, Rectangle, Size, mouse};

type Renderer = iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, UserInterface};

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Menu(ContextMenuEvent),
    TargetPressed,
    BackgroundPressed,
}

/// The target's size. Fixed so its bounds are deterministic, and non-square so
/// an x/y transposition in the conversion cannot hide.
const TARGET_W: f32 = 200.0;
const TARGET_H: f32 = 80.0;

const TARGET_ID: &str = "ctx-target";
const BACKGROUND_ID: &str = "background";
const SCROLL_ID: &str = "scroller";

/// Height of the spacer above the host in the scrolled fixture.
const SPACER_ABOVE: f32 = 400.0;
/// Visible height of the scroll viewport in the scrolled fixture.
const SCROLL_VIEWPORT_H: f32 = 300.0;

/// Somewhere far from every widget. `Simulator::click` parks the cursor *on* the
/// widget it clicked, and `FocusButton::draw` tests hover before focus, so a
/// draw- or status-based focus assertion taken with the cursor still on the
/// control reads `Hovered` and passes for the wrong reason.
fn parked() -> Point {
    Point::new(5000.0, 5000.0)
}

/// Four entries: a command, a separator, a disabled command, a command.
fn sample_model() -> MenuModel {
    MenuModel::new(vec![
        MenuEntryModel::command("copy", "Copy").shortcut("Ctrl+C"),
        MenuEntryModel::separator("rule"),
        MenuEntryModel::command("paste", "Paste").enabled(false),
        MenuEntryModel::command("select-all", "Select All"),
    ])
}

/// Which fixture a [`Host`] drives.
#[derive(Debug, Clone, Copy)]
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
enum Fixture {
    /// The host at a known non-zero offset, above a full-bleed focusable
    /// background. The offset is what makes a target-local/viewport confusion
    /// visible as an exact shift instead of hiding behind equal numbers.
    Offset {
        left: f32,
        top: f32,
    },
    Scrolled,
}

fn view<'a>(
    fixture: Fixture,
    model: &'a MenuModel,
    state: &'a ContextMenuState,
    live_target: bool,
) -> Element<'a, Msg> {
    let target = enzui_widgets::focus_button::focus_button(text("target"), Msg::TargetPressed)
        .id(Id::new(TARGET_ID))
        .width(Length::Fixed(TARGET_W))
        .height(Length::Fixed(TARGET_H));

    let host = if live_target {
        enz_point_context_menu_host(target, model, state, Msg::Menu)
    } else {
        enz_context_menu_host(target, model, state, Msg::Menu)
    }
    .invoker(Id::new(TARGET_ID))
    .label("Row actions");

    match fixture {
        Fixture::Offset { left, top } => {
            let background = enzui_widgets::focus_button::focus_button(
                text("background"),
                Msg::BackgroundPressed,
            )
            .id(Id::new(BACKGROUND_ID))
            .width(Length::Fill)
            .height(Length::Fill);

            column![
                container(host).padding(Padding {
                    top,
                    left,
                    right: 0.0,
                    bottom: 0.0,
                }),
                background,
            ]
            .into()
        }
        Fixture::Scrolled => scrollable(column![
            container(text("above")).height(Length::Fixed(SPACER_ABOVE)),
            host,
            container(text("below")).height(Length::Fixed(800.0)),
        ])
        .id(Id::new(SCROLL_ID))
        .height(Length::Fixed(SCROLL_VIEWPORT_H))
        .into(),
    }
}

/// A controlled host driven the way an application drives it.
struct Host {
    fixture: Fixture,
    /// Selects the point-host route, whose wrapped page remains live while its
    /// menu is open. Its early keyboard arm is separate from the ordinary
    /// host's post-target arm, so chord-boundary tests must exercise both.
    live_target: bool,
    model: MenuModel,
    state: ContextMenuState,
    cache: Cache,
    renderer: Renderer,
    size: Size,
    cursor: mouse::Cursor,
    /// Focus operations the *application* would run as a `Task` after handling
    /// a host event. Run at the top of the next frame, which is when a real
    /// runtime runs them.
    pending_focus: Vec<Box<dyn Operation<()>>>,
    /// Every message the tree published, in order.
    messages: Vec<Msg>,
    /// Rows that actually fired, as reported by `apply_event`.
    dispatched: Vec<EntryId>,
    /// `event::Status` per event of the most recent frame. This is exactly what
    /// `keyboard::listen()` gates on, so it decides whether the app-level Tab
    /// dispatcher would ever see the key at all.
    last_statuses: Vec<iced::event::Status>,
    observe_in_place_shape: bool,
    /// Menu publication immediately before and after the most recent
    /// `UserInterface::update`, while the same controlled-state snapshot was
    /// still borrowed by the live tree.
    last_in_place_menu_bounds: (Option<Rectangle>, Option<Rectangle>),
    /// Whether that same update requested a layout revalidation.
    last_layout_changed: bool,
}

impl Host {
    fn new(fixture: Fixture, size: Size) -> Self {
        let renderer = iced_test::futures::futures::executor::block_on(
            <Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");

        Self {
            fixture,
            live_target: false,
            model: sample_model(),
            state: ContextMenuState::closed(),
            cache: Cache::default(),
            renderer,
            size,
            cursor: mouse::Cursor::Unavailable,
            pending_focus: Vec::new(),
            messages: Vec::new(),
            dispatched: Vec::new(),
            last_statuses: Vec::new(),
            observe_in_place_shape: false,
            last_in_place_menu_bounds: (None, None),
            last_layout_changed: false,
        }
    }

    /// The standard fixture: target at (100, 50) in a 1024x768 viewport.
    fn offset() -> Self {
        Self::new(
            Fixture::Offset {
                left: 100.0,
                top: 50.0,
            },
            Size::new(1024.0, 768.0),
        )
    }

    fn point_at(&mut self, position: Point) {
        self.cursor = mouse::Cursor::Available(position);
    }

    /// Build the tree, run pending focus work, feed `events`, then apply every
    /// published host event to the model and state.
    fn frame(&mut self, events: &[Event]) -> Vec<ContextMenuEvent> {
        let mut messages: iced::advanced::shell::Bus<Msg> = iced::advanced::shell::Bus::new();
        let cursor = self.cursor;
        let pending: Vec<Box<dyn Operation<()>>> = std::mem::take(&mut self.pending_focus);

        let model = self.model.clone();
        let state = self.state.clone();
        let mut ui = UserInterface::build(
            view(self.fixture, &model, &state, self.live_target),
            self.size,
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        for operation in pending {
            run_operation(&mut ui, &self.renderer, operation);
        }
        let before = self
            .observe_in_place_shape
            .then(|| menu_bounds_in_ui(&mut ui, &self.renderer))
            .flatten();
        let (update_state, statuses) = ui.update(
            &iced::advanced::graphics::core::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            events,
            cursor,
            &mut self.renderer,
            &mut messages,
        );
        let after = self
            .observe_in_place_shape
            .then(|| menu_bounds_in_ui(&mut ui, &self.renderer))
            .flatten();
        self.cache = ui.into_cache();

        self.last_statuses = statuses;
        self.last_in_place_menu_bounds = (before, after);
        self.last_layout_changed = update_state.has_layout_changed();
        self.messages.extend(messages.iter().cloned());

        let mut host_events = Vec::new();
        for message in &messages {
            if let Msg::Menu(event) = message {
                if let Some(op) = focus_restore_operation::<()>(event, Some(&Id::new(TARGET_ID))) {
                    self.pending_focus.push(op);
                }
                if let Some(id) = apply_event(&mut self.model, &mut self.state, event) {
                    self.dispatched.push(id);
                }
                host_events.push(event.clone());
            }
        }
        host_events
    }

    /// Run one event-less frame, so any pending focus `Task` actually lands.
    fn settle(&mut self) {
        let _ = self.frame(&[]);
    }

    /// Put keyboard focus on the host's target, the way a user arriving by Tab
    /// (or an application `focus(id)` `Task`) would.
    fn focus_target(&mut self) {
        self.pending_focus
            .push(Box::new(operation::focusable::focus(Id::new(TARGET_ID))));
        self.settle();
        assert!(
            self.focused_is(TARGET_ID),
            "precondition: the target must hold focus before a keyboard invocation"
        );
    }

    /// Collect the semantics of the live tree, overlay included.
    fn collect(&mut self) -> Vec<CollectedNode> {
        let model = self.model.clone();
        let state = self.state.clone();
        let mut ui = UserInterface::build(
            view(self.fixture, &model, &state, self.live_target),
            self.size,
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        let mut collector = SemanticsCollector::new();
        ui.operate(&self.renderer, &mut collector as &mut dyn Operation);
        self.cache = ui.into_cache();
        collector.into_collected()
    }

    /// The bounds the overlay actually laid the menu surface out at, in screen
    /// space. `None` when no menu is presented.
    fn menu_bounds(&mut self) -> Option<Rectangle> {
        self.collect()
            .into_iter()
            .find(|n| n.semantics.role == Role::Menu)
            .map(|n| n.bounds)
    }

    /// Which control currently holds focus, read out of the live tree.
    fn focused(&mut self) -> Option<String> {
        let model = self.model.clone();
        let state = self.state.clone();
        let mut ui = UserInterface::build(
            view(self.fixture, &model, &state, self.live_target),
            self.size,
            std::mem::take(&mut self.cache),
            &mut self.renderer,
        );
        let mut find = operation::focusable::find_focused();
        let mut boxed: Box<dyn Operation<()>> = Box::new(operation::black_box(&mut find));
        loop {
            ui.operate(&self.renderer, &mut boxed);
            match boxed.finish() {
                operation::Outcome::Chain(next) => boxed = next,
                _ => break,
            }
        }
        drop(boxed);
        self.cache = ui.into_cache();
        match find.finish() {
            operation::Outcome::Some(id) => Some(format!("{id:?}")),
            _ => None,
        }
    }

    /// `focused()` renders an `Id` as `Id(Custom("ctx-target"))`; match on name.
    fn focused_is(&mut self, name: &str) -> bool {
        self.focused()
            .is_some_and(|id| id.contains(&format!("\"{name}\"")))
    }
}

/// Read the menu overlay from one live `UserInterface` without rebuilding it.
/// Calling this on both sides of `UserInterface::update` is what distinguishes
/// an in-place publication edge from the controlled application's later
/// rebuild.
fn menu_bounds_in_ui(
    ui: &mut UserInterface<'_, Msg, iced::Theme, Renderer>,
    renderer: &Renderer,
) -> Option<Rectangle> {
    let mut collector = SemanticsCollector::new();
    ui.operate(renderer, &mut collector as &mut dyn Operation);
    collector
        .into_collected()
        .into_iter()
        .find(|node| node.semantics.role == Role::Menu)
        .map(|node| node.bounds)
}

/// Drain a focus `Operation` against the live tree.
fn run_operation(
    ui: &mut UserInterface<'_, Msg, iced::Theme, Renderer>,
    renderer: &Renderer,
    mut op: Box<dyn Operation<()>>,
) {
    loop {
        ui.operate(renderer, &mut op);
        match op.finish() {
            operation::Outcome::Chain(next) => op = next,
            _ => break,
        }
    }
}

fn press_with_modifiers(named: Named, modifiers: iced::keyboard::Modifiers) -> Vec<Event> {
    let k = iced::keyboard::Key::Named(named);
    vec![Event::Keyboard(iced::keyboard::Event::KeyPressed {
        key: k.clone(),
        modified_key: k,
        physical_key: iced::keyboard::key::Physical::Unidentified(
            iced::keyboard::key::NativeCode::Unidentified,
        ),
        location: iced::keyboard::Location::Standard,
        modifiers,
        text: None,
        repeat: false,
    })]
}

fn press(named: Named, shift: bool) -> Vec<Event> {
    press_with_modifiers(
        named,
        if shift {
            iced::keyboard::Modifiers::SHIFT
        } else {
            iced::keyboard::Modifiers::default()
        },
    )
}

/// Escape dismisses an open menu.
#[test]
fn escape_dismisses_an_open_menu() {
    let mut host = Host::offset();
    host.point_at(parked());
    host.focus_target();
    let _ = host.frame(&press(Named::ContextMenu, false));
    assert!(host.state.is_open(), "precondition: the menu is open");

    let events = host.frame(&press(Named::Escape, false));

    assert_eq!(
        events,
        vec![ContextMenuEvent::Dismissed(CloseCause::Escape)],
        "Escape must dismiss with the Escape cause"
    );
    assert!(!host.state.is_open());
    assert!(host.menu_bounds().is_none());
}
