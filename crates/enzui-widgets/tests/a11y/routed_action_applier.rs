//! The shared applier turns a routed accessibility action into the widget's own
//! message, through the real runtime path an application takes.
//!
//! Every test drives the whole chain: a real `accesskit` request enters an
//! [`ActionRouter`], the resolved action is paired with the publishing window,
//! [`apply_accessibility_action`] produces the runtime task, the task's
//! `Action::WidgetIn` operations are extracted and run over a live tree, and the
//! message the widget publishes is asserted. Each routed assertion is
//! calibrated against the pointer path on the same fixture first, so a chain
//! that broke for an unrelated reason cannot read as success.
//!
//! Asserting the produced `Task` alone would pass for an applier that focuses a
//! button instead of pressing it: the task exists either way, and only the
//! message distinguishes them.

use enzui_core::a11y_action::{AccessibilityAction, ActionRouter};
use enzui_core::app_runtime::RoutedAccessibilityAction;
use enzui_core::collect::{NodeIdMap, SemanticsCollector, node_id_of_widget};
use enzui_widgets::a11y_actions::apply_accessibility_action;
use enzui_widgets::stepper::enz_stepper;
use iced::advanced::widget::{Id, Operation};
use iced::{Element, Event, Point, Size, Task, mouse, window};
use iced_test::runtime::Action;
use iced_test::runtime::task::into_stream;
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Renderer = iced_test::renderer::Renderer;

struct Harness<'a, Message> {
    ui: UserInterface<'a, Message, iced::Theme, Renderer>,
    renderer: Renderer,
}

impl<'a, Message> Harness<'a, Message> {
    fn new(view: Element<'a, Message, iced::Theme, Renderer>) -> Self {
        let mut renderer = iced_test::futures::futures::executor::block_on(
            <Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
            ),
        )
        .expect("headless renderer");
        let ui = UserInterface::build(
            view,
            Size::new(400.0, 300.0),
            Cache::default(),
            &mut renderer,
        );
        Self { ui, renderer }
    }

    /// Run an operation to completion, following `Outcome::Chain` as the winit
    /// runtime's widget-action loop does.
    fn run(&mut self, mut operation: Box<dyn Operation<()>>) {
        loop {
            self.ui.operate(&self.renderer, &mut operation);
            match operation.finish() {
                iced::advanced::widget::operation::Outcome::Chain(next) => operation = next,
                _ => break,
            }
        }
    }

    /// One event pass with no input event, with the cursor parked far outside
    /// every widget so no hover or click can be mistaken for the routed action.
    fn pump_without_input(&mut self) -> Vec<Message> {
        let mut messages = iced::advanced::shell::Bus::new();
        let _ = self.ui.update(
            &window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[Event::Window(window::Event::RedrawRequested(
                std::time::Instant::now(),
            ))],
            mouse::Cursor::Available(Point::new(5000.0, 5000.0)),
            &mut self.renderer,
            &mut messages,
        );
        messages.into_iter().collect()
    }

    fn publish_semantics(&mut self) -> NodeIdMap {
        let mut collector = SemanticsCollector::new();
        self.ui
            .operate(&self.renderer, &mut collector as &mut dyn Operation);
        collector.to_tree_update_with_ids().1
    }
}

/// Resolve a real AccessKit request against `map` and return the resolved
/// actions, exactly as the bridge's request handler does.
fn resolve(
    map: NodeIdMap,
    action: accesskit::Action,
    node: accesskit::NodeId,
    data: Option<&accesskit::ActionData>,
) -> Vec<AccessibilityAction> {
    let router = ActionRouter::new();
    router.set_map(enzui_core::a11y_action::A11Y_UNKEYED_WINDOW, map);
    router.handle(
        enzui_core::a11y_action::A11Y_UNKEYED_WINDOW,
        action,
        node,
        data,
    );
    router.drain()
}

/// Apply `resolved` in `window` through the shared applier and return the widget
/// operations it produced, failing if any operation would reach every window.
fn operations_of<Message: std::fmt::Debug + 'static>(
    window: window::Id,
    resolved: Vec<AccessibilityAction>,
) -> Vec<Box<dyn Operation<()>>> {
    let mut operations = Vec::new();
    for action in resolved {
        let routed = RoutedAccessibilityAction::new(window, action);
        let task: Task<Message> = apply_accessibility_action(&routed);
        let Some(mut stream) = into_stream(task) else {
            continue;
        };
        let waker = iced::futures::task::noop_waker();
        let mut context = std::task::Context::from_waker(&waker);
        // A widget task's stream stays pending after its actions: the receiver
        // half is only closed by running the operation, which happens later.
        loop {
            match iced::futures::StreamExt::poll_next_unpin(&mut stream, &mut context) {
                std::task::Poll::Ready(Some(Action::WidgetIn {
                    window: named,
                    operation,
                })) => {
                    assert_eq!(
                        named, window,
                        "the applier must operate in the publishing window only",
                    );
                    operations.push(operation);
                }
                std::task::Poll::Ready(Some(other)) => {
                    panic!("the applier must produce window-scoped work: {other:?}")
                }
                _ => break,
            }
        }
    }
    operations
}

// -----------------------------------------------------------------------------
// Activate — the widget's real activation callback, not a focus move
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum FormMsg {
    Name,
    Picked,
}

fn focus_owners<Message>(harness: &mut Harness<'_, Message>) -> Vec<enzui_core::semantics::Role> {
    let mut collector = SemanticsCollector::new();
    harness
        .ui
        .operate(&harness.renderer, &mut collector as &mut dyn Operation);
    collector
        .collected()
        .iter()
        .filter(|node| node.semantics.focused)
        .map(|node| node.semantics.role)
        .collect()
}

/// A native click takes focus as a pointer press does: the control it opens
/// becomes the one focus owner, so Escape reaches its options and not the
/// form field that was focused before.
#[test]
fn a_routed_click_moves_focus_to_its_control() {
    use enzui_core::semantics::Role;
    let view: Element<'static, FormMsg, iced::Theme, Renderer> = iced::widget::column![
        enzui_widgets::text_field::enz_text_field_owned("Name", "", "Ada", |_| FormMsg::Name)
            .id(Id::new("applier-name")),
        enzui_widgets::combo_box::enz_combo_box(
            "Style",
            vec!["Plain".into(), "Script".into()],
            None,
            |_| FormMsg::Picked,
        )
        .id(Id::new("applier-combo")),
    ]
    .into();
    let mut harness = Harness::new(view);
    harness.run(Box::new(
        iced::advanced::widget::operation::focusable::focus(Id::new("applier-name")),
    ));
    assert_eq!(focus_owners(&mut harness), vec![Role::TextInput]);

    let map = harness.publish_semantics();
    let node = node_id_of_widget(&Id::new("applier-combo"));
    let resolved = resolve(map, accesskit::Action::Click, node, None);
    assert!(!resolved.is_empty(), "the click resolves to the combo box");
    for operation in operations_of::<FormMsg>(window::Id::unique(), resolved) {
        harness.run(operation);
    }
    assert_eq!(focus_owners(&mut harness), vec![Role::ComboBox]);
}

#[derive(Debug, Clone)]
enum BarMsg {
    Query,
    Pick,
    View,
    Command,
    Menu,
}

fn calendar_bar() -> Element<'static, BarMsg, iced::Theme, Renderer> {
    use enzui_widgets::action_bar::ActionItem;
    use enzui_widgets::autocomplete_field::{AutocompleteOption, enz_autocomplete_field};
    enzui_widgets::title_bar::enz_title_bar()
        .leading(
            enzui_widgets::combo_box::enz_combo_box(
                "Calendar view",
                vec!["Week".into(), "Month".into()],
                None,
                |_| BarMsg::View,
            )
            .id(Id::new("applier-view"))
            .width(iced::Length::Fixed(100.0)),
        )
        .center(
            iced::widget::container(
                enz_autocomplete_field(
                    "",
                    "",
                    Vec::<AutocompleteOption<u64>>::new(),
                    |_| BarMsg::Query,
                    |_| BarMsg::Pick,
                )
                .placeholder("Search events")
                .width(iced::Length::Fill),
            )
            .width(iced::Length::Fixed(340.0)),
        )
        .actions(
            ["today", "previous", "next", "new", "settings"]
                .into_iter()
                .map(|id| ActionItem::icon(id, "plus", id, BarMsg::Command))
                .collect(),
            BarMsg::Menu,
        )
        .into_element()
}

/// A titlebar with a leading combo box and a centre autocomplete too narrow to
/// show its field. The combo opens and closes with Escape, keeping focus; a
/// native click on the collapsed search then leaves one focus owner (#3546).
#[test]
fn a_routed_click_on_a_collapsed_search_leaves_one_focus_owner() {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");
    let size = Size::new(300.0, 300.0);
    let mut cache = Cache::default();
    let mut now = std::time::Instant::now();
    // One rebuilt turn: the event, then a few frames, as a running app does.
    let mut turn = |event: Option<Event>,
                    at: Point,
                    operations: Vec<Box<dyn Operation<()>>>,
                    renderer: &mut Renderer|
     -> (usize, accesskit::TreeUpdate, NodeIdMap) {
        let mut ui =
            UserInterface::build(calendar_bar(), size, std::mem::take(&mut cache), renderer);
        for mut operation in operations {
            loop {
                ui.operate(renderer, &mut operation);
                match operation.finish() {
                    iced::advanced::widget::operation::Outcome::Chain(next) => operation = next,
                    _ => break,
                }
            }
        }
        let mut events: Vec<Event> = event.into_iter().collect();
        for _ in 0..4 {
            now += std::time::Duration::from_millis(16);
            events.push(Event::Window(window::Event::RedrawRequested(now)));
        }
        for event in events {
            let mut bus = iced::advanced::shell::Bus::new();
            let _ = ui.update(
                &window::Headless,
                &iced::advanced::graphics::core::shell::Waker::noop(),
                &[event],
                mouse::Cursor::Available(at),
                renderer,
                &mut bus,
            );
        }
        cache = ui.into_cache();
        let mut ui =
            UserInterface::build(calendar_bar(), size, std::mem::take(&mut cache), renderer);
        let mut collector = SemanticsCollector::new();
        ui.operate(renderer, &mut collector as &mut dyn Operation);
        cache = ui.into_cache();
        let owners = collector
            .collected()
            .iter()
            .filter(|node| node.semantics.focused)
            .count();
        let (update, map) = collector.to_tree_update_with_ids();
        (owners, update, map)
    };
    let away = Point::new(5000.0, 5000.0);
    let (_, update, _) = turn(None, away, Vec::new(), &mut renderer);
    let combo = node_bounds(&update, "Calendar view");
    let press = |kind| Event::Mouse(kind);
    let _ = turn(
        Some(press(mouse::Event::CursorMoved { position: combo })),
        combo,
        Vec::new(),
        &mut renderer,
    );
    let _ = turn(
        Some(press(mouse::Event::ButtonPressed(mouse::Button::Left))),
        combo,
        Vec::new(),
        &mut renderer,
    );
    let _ = turn(
        Some(press(mouse::Event::ButtonReleased(mouse::Button::Left))),
        combo,
        Vec::new(),
        &mut renderer,
    );
    let escape = iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape);
    let (owners, update, map) = turn(
        Some(Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: escape.clone(),
            modified_key: escape,
            physical_key: iced::keyboard::key::Physical::Unidentified(
                iced::keyboard::key::NativeCode::Unidentified,
            ),
            location: iced::keyboard::Location::Standard,
            modifiers: iced::keyboard::Modifiers::default(),
            text: None,
            repeat: false,
        })),
        away,
        Vec::new(),
        &mut renderer,
    );
    assert_eq!(owners, 1, "the combo keeps focus after Escape");

    let node = update
        .nodes
        .iter()
        .find(|(_, node)| {
            node.label()
                .is_some_and(|label| label.starts_with("Search events"))
        })
        .map(|(id, _)| *id)
        .expect("the collapsed search is published");
    let resolved = resolve(map, accesskit::Action::Click, node, None);
    assert!(!resolved.is_empty(), "the click resolves");
    let operations = operations_of::<BarMsg>(window::Id::unique(), resolved);
    let (owners, _, _) = turn(None, away, operations, &mut renderer);
    assert_eq!(owners, 1, "after the click");
    let (owners, _, _) = turn(
        Some(press(mouse::Event::CursorMoved { position: away })),
        away,
        Vec::new(),
        &mut renderer,
    );
    assert_eq!(owners, 1, "after the next input");
}

/// The reporter's probe: the Calendar titlebar inside an app shell with an
/// overlay sidebar, at 375 px.
fn calendar_shell() -> Element<'static, BarMsg, iced::Theme, Renderer> {
    use enzui_widgets::action_bar::ActionItem;
    use enzui_widgets::app_shell::{ShellLayout, ShellTokens, SidebarHeader, enz_app_shell};
    use enzui_widgets::autocomplete_field::{AutocompleteOption, enz_autocomplete_field};
    use enzui_widgets::nav_split::{NavSplitState, SidebarPresentation};
    use enzui_widgets::text_field::TextFieldSize;
    enz_app_shell(|| iced::widget::Space::new().into())
        .layout(ShellLayout::TitlebarSidebarContent)
        .tokens(ShellTokens::default().sidebar_header(SidebarHeader::Inside))
        .state(NavSplitState::new())
        .sidebar(|| iced::widget::Space::new().into())
        .sidebar_label("Calendars")
        .on_sidebar_state(|_| BarMsg::Menu)
        .forced(SidebarPresentation::Overlay)
        .on_window_gesture(|_| BarMsg::Menu)
        .titlebar(|| {
            enzui_widgets::title_bar::enz_title_bar()
                .leading(
                    enzui_widgets::combo_box::enz_combo_box(
                        "",
                        vec!["Week".into(), "Month".into()],
                        None,
                        |_| BarMsg::View,
                    )
                    .aria_label("Calendar view")
                    .placeholder("Week")
                    .size(TextFieldSize::Sm)
                    .width(iced::Length::Fixed(100.0)),
                )
                .title("Calendar")
                .center(
                    iced::widget::container(
                        enz_autocomplete_field(
                            "",
                            "",
                            Vec::<AutocompleteOption<u64>>::new(),
                            |_| BarMsg::Query,
                            |_| BarMsg::Pick,
                        )
                        .placeholder("Search events")
                        .size(TextFieldSize::Sm)
                        .width(iced::Length::Fill),
                    )
                    .width(iced::Length::Fixed(340.0)),
                )
                .actions(
                    ["today", "previous", "next", "new", "search", "settings"]
                        .into_iter()
                        .map(|id| ActionItem::icon(id, "plus", id, BarMsg::Command))
                        .collect(),
                    BarMsg::Menu,
                )
                .into_element()
        })
        .into_element()
}

/// The reporter's cycle, twice, over `view`: the leading combo and the collapsed search
/// are each opened through AT-SPI, Escape closes what opened, and pointer
/// motion follows each step. One focus owner throughout.
fn combo_escape_and_search_cycles(
    view: fn() -> Element<'static, BarMsg, iced::Theme, Renderer>,
    size: Size,
    inline: bool,
) {
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");
    let mut theme = enzui_theme::EnzTheme::light();
    theme.window_buttons = enzui_theme::WindowButtons::All;
    let _scope = enzui_theme::text_defaults::TextDefaultsScope::enter(&theme);
    let mut cache = Cache::default();
    let mut now = std::time::Instant::now();
    let mut turn = |event: Option<Event>,
                    at: Point,
                    operations: Vec<Box<dyn Operation<()>>>,
                    renderer: &mut Renderer|
     -> (
        Vec<enzui_core::semantics::Role>,
        accesskit::TreeUpdate,
        NodeIdMap,
    ) {
        let mut ui = UserInterface::build(view(), size, std::mem::take(&mut cache), renderer);
        for mut operation in operations {
            loop {
                ui.operate(renderer, &mut operation);
                match operation.finish() {
                    iced::advanced::widget::operation::Outcome::Chain(next) => operation = next,
                    _ => break,
                }
            }
        }
        let mut events: Vec<Event> = event.into_iter().collect();
        for _ in 0..4 {
            now += std::time::Duration::from_millis(16);
            events.push(Event::Window(window::Event::RedrawRequested(now)));
        }
        for event in events {
            let mut bus = iced::advanced::shell::Bus::new();
            let _ = ui.update(
                &window::Headless,
                &iced::advanced::graphics::core::shell::Waker::noop(),
                &[event],
                mouse::Cursor::Available(at),
                renderer,
                &mut bus,
            );
        }
        cache = ui.into_cache();
        let mut ui = UserInterface::build(view(), size, std::mem::take(&mut cache), renderer);
        let mut collector = SemanticsCollector::new();
        ui.operate(renderer, &mut collector as &mut dyn Operation);
        cache = ui.into_cache();
        let owners = collector
            .collected()
            .iter()
            .filter(|node| node.semantics.focused)
            .map(|node| node.semantics.role)
            .collect::<Vec<_>>();
        let (update, map) = collector.to_tree_update_with_ids();
        (owners, update, map)
    };
    let away = Point::new(5000.0, 5000.0);
    let escape = || {
        let key = iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape);
        Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: iced::keyboard::key::Physical::Unidentified(
                iced::keyboard::key::NativeCode::Unidentified,
            ),
            location: iced::keyboard::Location::Standard,
            modifiers: iced::keyboard::Modifiers::default(),
            text: None,
            repeat: false,
        })
    };
    let native_click = |update: &accesskit::TreeUpdate, map: NodeIdMap, label: &str| {
        let node = update
            .nodes
            .iter()
            .find(|(_, node)| node.label().is_some_and(|l| l.starts_with(label)))
            .map_or_else(|| panic!("{label} is published"), |(id, _)| *id);
        operations_of::<BarMsg>(
            window::Id::unique(),
            resolve(map, accesskit::Action::Click, node, None),
        )
    };
    let moved = |at: Point| Event::Mouse(mouse::Event::CursorMoved { position: at });
    let (_, mut update, mut map) = turn(None, away, Vec::new(), &mut renderer);
    let mut step = 0;
    let mut check = |owners: Vec<enzui_core::semantics::Role>, what: &str| {
        step += 1;
        assert!(
            owners.len() <= 1,
            "step {step} ({what}): focus owners {owners:?}"
        );
    };
    for _ in 0..2 {
        for target in ["Calendar view", "Search events"] {
            let operations = native_click(&update, map, target);
            let (owners, _, _) = turn(None, away, operations, &mut renderer);
            // Each opening lands: the combo takes focus, and the search, too
            // narrow to expand here, opens its field as a popover.
            let expected = if target == "Search events" && !inline {
                enzui_core::semantics::Role::EditableComboBox
            } else {
                enzui_core::semantics::Role::ComboBox
            };
            if target == "Search events" && inline {
                assert_eq!(
                    owners.len(),
                    1,
                    "{target} opens and takes focus: {owners:?}"
                );
            } else {
                assert_eq!(owners, vec![expected], "{target} opens and takes focus");
            }
            check(owners, target);
            let (owners, _, _) = turn(
                Some(moved(Point::new(150.0, 200.0))),
                Point::new(150.0, 200.0),
                Vec::new(),
                &mut renderer,
            );
            check(owners, "pointer");
            let (owners, u, m) = turn(Some(escape()), away, Vec::new(), &mut renderer);
            check(owners, "Escape");
            let (owners, u2, m2) = turn(
                Some(moved(Point::new(151.0, 200.0))),
                Point::new(151.0, 200.0),
                Vec::new(),
                &mut renderer,
            );
            check(owners, "pointer");
            let _ = (u, m);
            update = u2;
            map = m2;
        }
    }
}

/// The reporter's probe at 375 px with every window button: the search is
/// too narrow to expand and opens as a popover.
#[test]
fn native_combo_escape_and_search_cycles_leave_one_focus_owner() {
    combo_escape_and_search_cycles(calendar_shell, Size::new(375.0, 800.0), false);
}

/// The bare titlebar at 300 px: the search is compact but expands inline,
/// and an Escape meant for the combo must not close it onto its trigger.
#[test]
fn an_escape_for_another_control_leaves_an_open_inline_search_alone() {
    combo_escape_and_search_cycles(calendar_bar, Size::new(300.0, 300.0), true);
}

/// The centre of the published node labelled `label`.
fn node_bounds(update: &accesskit::TreeUpdate, label: &str) -> Point {
    let (_, node) = update
        .nodes
        .iter()
        .find(|(_, node)| node.label().is_some_and(|l| l.starts_with(label)))
        .unwrap_or_else(|| panic!("{label} is published"));
    let bounds = node.bounds().expect("bounds");
    Point::new(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    )
}

// -----------------------------------------------------------------------------
// SetValue / Increment — the value moves, and a refused payload does not
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum StepperMsg {
    Changed(i32),
}

const STEPPER_ID: &str = "applier-stepper";

fn stepper_view(value: i32) -> Element<'static, StepperMsg, iced::Theme, Renderer> {
    enz_stepper(value, StepperMsg::Changed)
        .min(0)
        .max(10)
        .id(STEPPER_ID)
        .into()
}

/// The router refuses an oversize payload rather than truncating it. Applying
/// anything for that refusal would write a value the client was told was
/// rejected — and the accepted `SetValue` above is the positive control proving
/// this fixture can publish a change at all.
#[test]
fn a_refused_set_value_changes_no_value() {
    let mut harness = Harness::new(stepper_view(3));
    let node = node_id_of_widget(&Id::new(STEPPER_ID));

    // One byte past the router's payload cap; the cap itself is accepted.
    let oversize = accesskit::ActionData::Value("9".repeat(64 * 1024 + 1).into());
    let map = harness.publish_semantics();
    let resolved = resolve(map, accesskit::Action::SetValue, node, Some(&oversize));
    assert!(
        resolved
            .iter()
            .any(|action| matches!(action, AccessibilityAction::SetValueRefused { .. })),
        "the fixture must produce a refusal, not an accepted value ({} actions \
         resolved, none refused)",
        resolved.len(),
    );

    for operation in operations_of::<StepperMsg>(window::Id::unique(), resolved) {
        harness.run(operation);
    }
    assert_eq!(
        harness.pump_without_input(),
        Vec::new(),
        "a refused payload must leave the value untouched",
    );
}

#[derive(Debug, Clone, PartialEq)]
enum RangeMsg {
    Range((f32, f32)),
}

/// A typed value reaches a range slider's thumb; "NaN" parses as a number
/// but is refused rather than published.
#[test]
fn a_routed_typed_value_moves_a_range_thumb_and_refuses_nan() {
    let node = node_id_of_widget(&Id::new("applier-range-high"));
    let mut seen = Vec::new();
    for typed in ["NaN", "2"] {
        let view: Element<'static, RangeMsg, iced::Theme, Renderer> =
            enzui_widgets::range_slider::enz_range_slider(0.0..=3.0, (0.0, 1.0), RangeMsg::Range)
                .id("applier-range")
                .into_element();
        let mut harness = Harness::new(view);
        let map = harness.publish_semantics();
        let data = accesskit::ActionData::Value(typed.into());
        let resolved = resolve(map, accesskit::Action::SetValue, node, Some(&data));
        for operation in operations_of::<RangeMsg>(window::Id::unique(), resolved) {
            harness.run(operation);
        }
        seen.push(harness.pump_without_input());
    }
    assert_eq!(seen, vec![Vec::new(), vec![RangeMsg::Range((0.0, 2.0))]]);
}

// -----------------------------------------------------------------------------
// Expand — the disclosure's own toggle, not a row activation
// -----------------------------------------------------------------------------

// -----------------------------------------------------------------------------
// ShowContextMenu — the pointer-only right-click, reachable without a pointer
// -----------------------------------------------------------------------------

// -----------------------------------------------------------------------------
// SetTextSelection — the caret moves in the field and the next frame says so
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum FieldMsg {
    Edited,
}

const FIELD_ID: &str = "applier-field";

/// The field's published selection, as (run index, character) pairs for the
/// anchor and the caret, and its text runs.
fn field_selection(
    update: &accesskit::TreeUpdate,
) -> (Vec<accesskit::NodeId>, Option<accesskit::TextSelection>) {
    let field = node_id_of_widget(&Id::new(FIELD_ID));
    let (_, node) = update
        .nodes
        .iter()
        .find(|(id, _)| *id == field)
        .expect("the field is published");
    let runs = node
        .children()
        .iter()
        .copied()
        .filter(|child| {
            update
                .nodes
                .iter()
                .any(|(id, n)| id == child && n.role() == accesskit::Role::TextRun)
        })
        .collect();
    (runs, node.text_selection().copied())
}

#[test]
fn a_routed_text_selection_moves_the_caret() {
    let view: Element<'static, FieldMsg, iced::Theme, Renderer> =
        enzui_widgets::text_field::enz_text_field("Name", "", "Hello world", |_| FieldMsg::Edited)
            .id(Id::new(FIELD_ID))
            .into();
    let mut harness = Harness::new(view);
    let mut collector = SemanticsCollector::new();
    harness
        .ui
        .operate(&harness.renderer, &mut collector as &mut dyn Operation);
    let (update, map) = collector.to_tree_update_with_ids();
    let (runs, before) = field_selection(&update);
    assert_eq!(runs.len(), 1);
    assert_ne!(before.map(|s| s.focus.character_index), Some(5));

    let position = |character_index| accesskit::TextPosition {
        node: runs[0],
        character_index,
    };
    let resolved = resolve(
        map,
        accesskit::Action::SetTextSelection,
        node_id_of_widget(&Id::new(FIELD_ID)),
        Some(&accesskit::ActionData::SetTextSelection(
            accesskit::TextSelection {
                anchor: position(2),
                focus: position(5),
            },
        )),
    );
    for operation in operations_of::<FieldMsg>(window::Id::unique(), resolved) {
        harness.run(operation);
    }

    let mut collector = SemanticsCollector::new();
    harness
        .ui
        .operate(&harness.renderer, &mut collector as &mut dyn Operation);
    let (update, _) = collector.to_tree_update_with_ids();
    let (_, after) = field_selection(&update);
    let after = after.expect("a selection");
    assert_eq!(
        (after.anchor.character_index, after.focus.character_index),
        (2, 5)
    );
}

// -----------------------------------------------------------------------------
// Schedule events: a canvas row reaches its activation through its own id
// -----------------------------------------------------------------------------

#[test]
fn a_routed_click_activates_a_schedule_event() {
    use enzui_core::schedule::EventId;
    use enzui_widgets::schedule::{ScheduleBlock, ScheduleColumn, ScheduleEvent, enz_schedule};

    let mut block = ScheduleBlock::timed(EventId(41), 0, 540, 600);
    block.title = "QA schedule event".into();
    let schedule = || {
        enz_schedule(
            vec![ScheduleColumn::new("Wed", "21")],
            vec![block.clone()],
            |e| e,
        )
        .label("QA schedule")
        .initial_scroll_minute(Some(480))
    };
    let id = schedule().event_widget_id(EventId(41));
    let mut harness = Harness::new(schedule().into());
    let _ = harness.pump_without_input();
    let map = harness.publish_semantics();
    let resolved = resolve(map, accesskit::Action::Click, node_id_of_widget(&id), None);
    assert!(!resolved.is_empty(), "the event's node is routable");
    for operation in operations_of::<ScheduleEvent>(window::Id::unique(), resolved) {
        harness.run(operation);
    }
    assert_eq!(
        harness.pump_without_input(),
        vec![ScheduleEvent::BlockActivated {
            id: EventId(41),
            at: None
        }],
        "the routed click activates the event it names"
    );
}

#[derive(Debug, Clone, PartialEq)]
enum PickerMsg {
    Left(enzui_widgets::emoji_picker::EmojiPickerEvent),
    Right(enzui_widgets::emoji_picker::EmojiPickerEvent),
}

#[test]
fn a_routed_click_picks_an_emoji_and_a_category_in_the_picker_it_names() {
    use enzui_widgets::emoji_picker::{
        EmojiCategory, EmojiPickerEvent, EmojiRecents, category_id, emoji_id, enz_emoji_picker,
    };
    let recents: &'static EmojiRecents = Box::leak(Box::new(EmojiRecents::new()));
    let view: Element<'static, PickerMsg, iced::Theme, Renderer> = iced::widget::column![
        enz_emoji_picker("rocket", EmojiCategory::Smileys, recents, PickerMsg::Left)
            .id_prefix("left")
            .grid_height(40.0),
        enz_emoji_picker("rocket", EmojiCategory::Smileys, recents, PickerMsg::Right)
            .id_prefix("right")
            .grid_height(40.0),
    ]
    .into();
    let mut harness = Harness::new(view);
    let _ = harness.pump_without_input();
    for (id, expected) in [
        (
            emoji_id("right", "🚀"),
            PickerMsg::Right(EmojiPickerEvent::Picked("🚀".into())),
        ),
        (
            category_id("left", EmojiCategory::Symbols),
            PickerMsg::Left(EmojiPickerEvent::Category(EmojiCategory::Symbols)),
        ),
    ] {
        let map = harness.publish_semantics();
        let resolved = resolve(map, accesskit::Action::Click, node_id_of_widget(&id), None);
        assert!(!resolved.is_empty(), "{expected:?}: the button is routable");
        for operation in operations_of::<PickerMsg>(window::Id::unique(), resolved) {
            harness.run(operation);
        }
        assert_eq!(
            harness.pump_without_input(),
            vec![expected],
            "the routed click presses the button it names, in its own picker"
        );
    }
}
