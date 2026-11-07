//! Hover delays, control defaults and cancellation through native widget overlays.

use enzui_widgets::tooltip::{FadeTooltip, TOOLTIP_DELAY, TOOLTIP_FADE, enz_fade_tooltip};
use iced::advanced::graphics::core::shell::Waker;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::Tree;
use iced::advanced::{Shell, layout, mouse};
use iced::widget::{button, tooltip::Position};
use iced::window::Headless;
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector, time, window};

const W: f32 = 320.0;
const H: f32 = 240.0;
const OVER: Point = Point::new(10.0, 10.0);
const AWAY: Point = Point::new(300.0, 220.0);

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Pressed,
}

fn tooltip_over_button() -> Element<'static, Msg> {
    let inner: Element<'static, Msg> = button("press")
        .on_press(Msg::Pressed)
        .width(Length::Fixed(40.0))
        .height(Length::Fixed(20.0))
        .into();
    enz_fade_tooltip(inner, "tip", Position::Bottom)
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
    deliver_trace(element, tree, node, renderer, event, at).0
}

fn deliver_trace(
    element: &mut Element<'_, Msg>,
    tree: &mut Tree,
    node: &layout::Node,
    renderer: &iced::Renderer,
    event: Event,
    at: Point,
) -> (Vec<Msg>, window::RedrawRequest) {
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
    let redraw = shell.redraw_request();
    (messages.into_iter().collect(), redraw)
}

fn overlay_is_published(
    element: &mut Element<'_, Msg>,
    tree: &mut Tree,
    node: &layout::Node,
    renderer: &iced::Renderer,
) -> bool {
    let viewport = Rectangle {
        x: 0.0,
        y: 0.0,
        width: W,
        height: H,
    };
    !element
        .as_widget_mut()
        .overlay(
            tree,
            iced::advanced::Layout::of(node),
            renderer,
            &viewport,
            Vector::ZERO,
            viewport.size(),
        )
        .is_empty()
}

fn redraw_at(now: time::Instant) -> Event {
    Event::Window(window::Event::RedrawRequested(now))
}

fn move_to(p: Point) -> Event {
    Event::Mouse(mouse::Event::CursorMoved { position: p })
}

fn settle() -> time::Duration {
    TOOLTIP_DELAY + TOOLTIP_FADE + TOOLTIP_FADE + time::Duration::from_millis(4)
}

#[test]
fn cancellation_reaches_tooltip_and_button_and_suppresses_button_release() {
    let mut element = tooltip_over_button();
    let mut tree = Tree::new(&element);
    element.as_widget_mut().diff(&mut tree);
    let renderer = make_renderer();
    let node = element.as_widget_mut().measure_snapshot(
        &mut tree,
        &renderer,
        &layout::Limits::new(Size::ZERO, Size::new(W, H)),
    );

    let t0 = time::Instant::now();
    let _ = deliver(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        redraw_at(t0),
        OVER,
    );
    let _ = deliver(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        move_to(OVER),
        OVER,
    );

    let t1 = t0 + settle();
    let _ = deliver(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        redraw_at(t1),
        OVER,
    );
    assert!(
        overlay_is_published(&mut element, &mut tree, &node, &renderer),
        "sanity: the bubble must be visible before cancellation — hover \
         and clock were driven through the real update path"
    );

    let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
    let btn_press_at = Point::new(10.0, 10.0);
    let armed = deliver(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        press,
        btn_press_at,
    );
    assert!(
        armed.iter().all(|m| !matches!(m, Msg::Pressed)),
        "sanity: `on_press` only fires on RELEASE; got {armed:?}"
    );

    let cancelled = tree.cancel_interactions();
    assert!(
        cancelled >= 2,
        "cancel_interactions must reach BOTH the tooltip's FadeState AND the \
         button's State (2 opt-ins); got {cancelled}. A count of 1 means \
         only one opt-in is loaded — the paired button positive control is \
         what proves the walk actually ran."
    );

    // Mouse leave also hides the tooltip; this does not isolate cancellation of its fade.
    let t2 = t1 + settle();
    let _ = deliver(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        move_to(AWAY),
        AWAY,
    );
    let _ = deliver(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        redraw_at(t2),
        AWAY,
    );
    assert!(
        !overlay_is_published(&mut element, &mut tree, &node, &renderer),
        "the bubble must be absent after cancellation, mouse leave and settling"
    );

    let release = Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left));
    let post_release = deliver(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        release,
        btn_press_at,
    );
    assert!(
        post_release.iter().all(|m| !matches!(m, Msg::Pressed)),
        "the cancelled button must suppress its armed release; got {post_release:?}"
    );
}

fn content() -> Element<'static, Msg> {
    iced::widget::Space::new().width(40).height(20).into()
}

fn prepare(element: &mut Element<'_, Msg>, renderer: &iced::Renderer) -> (Tree, layout::Node) {
    let mut tree = Tree::new(&*element);
    element.as_widget_mut().diff(&mut tree);
    let node = element.as_widget_mut().measure_snapshot(
        &mut tree,
        renderer,
        &layout::Limits::new(Size::ZERO, Size::new(W, H)),
    );
    (tree, node)
}

#[test]
fn delayed_tooltip_uses_one_deadline_and_cancels_pending_or_visible_hints() {
    let renderer = make_renderer();
    let delay = time::Duration::from_millis(600);
    let early = time::Duration::from_millis(599);
    let origin = time::Instant::now() + time::Duration::from_secs(60);
    for reduced in [true, false] {
        for cancellation in [
            move_to(AWAY),
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
            Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 },
            }),
            Event::Window(window::Event::Unfocused),
        ] {
            let mut element: Element<'_, Msg> =
                FadeTooltip::new(content(), "Delayed", Position::Bottom)
                    .delay(delay)
                    .reduced_motion(reduced)
                    .into();
            let (mut tree, node) = prepare(&mut element, &renderer);
            deliver(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                redraw_at(origin),
                OVER,
            );
            let (_, schedule) = deliver_trace(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                move_to(OVER),
                OVER,
            );
            assert_eq!(schedule, window::RedrawRequest::At(origin + delay));
            assert!(!overlay_is_published(
                &mut element,
                &mut tree,
                &node,
                &renderer
            ));
            deliver(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                redraw_at(origin + early),
                OVER,
            );
            let (_, schedule) = deliver_trace(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                move_to(OVER),
                OVER,
            );
            assert_eq!(
                schedule,
                window::RedrawRequest::At(origin + delay),
                "motion inside the control must not restart the timer"
            );
            assert!(!overlay_is_published(
                &mut element,
                &mut tree,
                &node,
                &renderer
            ));
            deliver(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                cancellation.clone(),
                if matches!(cancellation, Event::Mouse(mouse::Event::CursorMoved { .. })) {
                    AWAY
                } else {
                    OVER
                },
            );
            deliver(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                redraw_at(origin + delay * 2),
                OVER,
            );
            assert!(
                !overlay_is_published(&mut element, &mut tree, &node, &renderer),
                "a cancelled deadline revealed a hint: {cancellation:?}"
            );
            deliver(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                move_to(AWAY),
                AWAY,
            );
            deliver(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                move_to(OVER),
                OVER,
            );
            deliver(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                redraw_at(origin + delay * 3),
                OVER,
            );
            assert!(
                overlay_is_published(&mut element, &mut tree, &node, &renderer),
                "fresh hover must still work"
            );
            deliver(
                &mut element,
                &mut tree,
                &node,
                &renderer,
                cancellation,
                AWAY,
            );
            assert!(
                !overlay_is_published(&mut element, &mut tree, &node, &renderer),
                "visible hints must withdraw on interaction"
            );
        }
    }
    let mut element: Element<'_, Msg> =
        FadeTooltip::new(content(), "Tree cancellation", Position::Bottom)
            .delay(delay)
            .into();
    let (mut tree, node) = prepare(&mut element, &renderer);
    deliver(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        redraw_at(origin),
        OVER,
    );
    deliver(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        move_to(OVER),
        OVER,
    );
    assert!(tree.cancel_interactions() > 0);
    let (_, schedule) = deliver_trace(
        &mut element,
        &mut tree,
        &node,
        &renderer,
        redraw_at(origin + delay * 2),
        OVER,
    );
    assert_eq!(schedule, window::RedrawRequest::Wait);
    assert!(!overlay_is_published(
        &mut element,
        &mut tree,
        &node,
        &renderer
    ));
    let mut idle: Element<'_, Msg> = FadeTooltip::new(content(), "Idle window", Position::Bottom)
        .delay(delay)
        .into();
    let (mut tree, node) = prepare(&mut idle, &renderer);
    deliver(
        &mut idle,
        &mut tree,
        &node,
        &renderer,
        redraw_at(time::Instant::now() - time::Duration::from_secs(5)),
        OVER,
    );
    let before = time::Instant::now();
    let (_, schedule) = deliver_trace(&mut idle, &mut tree, &node, &renderer, move_to(OVER), OVER);
    let after = time::Instant::now();
    assert!(
        matches!(schedule, window::RedrawRequest::At(at) if at >= before + delay && at <= after + delay)
    );
    for delay in [time::Duration::ZERO, time::Duration::MAX] {
        let mut element: Element<'_, Msg> =
            FadeTooltip::new(content(), "Boundary", Position::Bottom)
                .delay(delay)
                .into();
        let (mut tree, node) = prepare(&mut element, &renderer);
        deliver(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            redraw_at(origin),
            OVER,
        );
        deliver(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            move_to(OVER),
            OVER,
        );
        assert_eq!(
            overlay_is_published(&mut element, &mut tree, &node, &renderer),
            delay.is_zero()
        );
    }
}

#[test]
fn control_tooltip_defaults_and_overrides_reach_the_native_overlay() {
    use enzui_widgets::button::ButtonIconPosition;
    use enzui_widgets::{
        enz_button, enz_sidebar_toggle_button, enz_toolbar_icon, enz_window_control_button,
        enz_window_controls,
        tab_pill::{TabPillItem, enz_tab_list},
    };
    let renderer = make_renderer();
    let origin = time::Instant::now() + time::Duration::from_secs(60);
    let menu = enzui_core::menu_model::MenuModel::new(vec![
        enzui_core::menu_model::MenuEntryModel::command("save", "Save"),
    ]);
    let state = enzui_core::menu_model::MenuState::closed();
    let short = time::Duration::from_millis(25);
    const POLICY_CHILD: &str = "ENZUI_TOOLTIP_POLICY_CHILD";
    let child = std::env::var_os(POLICY_CHILD).is_some();
    let default_delay = time::Duration::from_millis(if child { 35 } else { 500 });
    if child {
        enzui_widgets::set_default_tooltip_delay(default_delay);
    }
    let cases: Vec<(&str, Element<'_, Msg>, Option<time::Duration>)> = vec![
        (
            "menu name",
            enzui_widgets::menu_button::enz_kebab_menu_button(&menu, &state, |_| Msg::Pressed)
                .into(),
            Some(default_delay),
        ),
        (
            "menu override",
            enzui_widgets::menu_button::enz_kebab_menu_button(&menu, &state, |_| Msg::Pressed)
                .tooltip_delay(short)
                .into(),
            Some(short),
        ),
        (
            "menu opt-out",
            enzui_widgets::menu_button::enz_kebab_menu_button(&menu, &state, |_| Msg::Pressed)
                .tooltip("")
                .into(),
            None,
        ),
        (
            "switcher override",
            enzui_widgets::enz_view_switcher(
                vec![
                    enzui_widgets::SwitcherPage::new("settings", "Settings").with_icon("settings"),
                ],
                0,
                |_| Msg::Pressed,
            )
            .icons_only(true)
            .tooltip_delay(short)
            .into(),
            Some(short),
        ),
        (
            "sidebar shell",
            enzui_widgets::enz_app_shell(content)
                .layout(enzui_widgets::ShellLayout::TitlebarSidebarContent)
                .sidebar(content)
                .on_toggle_sidebar(Msg::Pressed)
                .into(),
            Some(default_delay),
        ),
        (
            "sidebar shell opt-out",
            enzui_widgets::enz_app_shell(content)
                .layout(enzui_widgets::ShellLayout::TitlebarSidebarContent)
                .sidebar(content)
                .on_toggle_sidebar(Msg::Pressed)
                .sidebar_toggle_tooltip(false)
                .into(),
            None,
        ),
        (
            "sidebar shell override",
            enzui_widgets::enz_app_shell(content)
                .layout(enzui_widgets::ShellLayout::TitlebarSidebarContent)
                .sidebar(content)
                .on_toggle_sidebar(Msg::Pressed)
                .tooltip_delay(short)
                .into(),
            Some(short),
        ),
        (
            "tab row override",
            enzui_widgets::enz_titlebar_tab_row(
                vec![
                    enzui_widgets::TitlebarTabItem::new("doc", "Document", Msg::Pressed)
                        .description("Document path"),
                ],
                0,
                Msg::Pressed,
            )
            .tooltip_delay(short)
            .into(),
            Some(short),
        ),
        (
            "tab description override",
            enzui_widgets::enz_titlebar_tab_row(
                vec![
                    enzui_widgets::TitlebarTabItem::new("doc", "Document", Msg::Pressed)
                        .description("Document path")
                        .tooltip_delay(short),
                ],
                0,
                Msg::Pressed,
            )
            .tooltip_delay(time::Duration::from_millis(600))
            .into(),
            Some(short),
        ),
        (
            "titlebar collector override",
            enzui_widgets::enz_titlebar_actions(
                vec![enzui_widgets::TitlebarActionItem::new(
                    enzui_core::action_overflow::Descriptor::new("save"),
                    "Save",
                    content,
                    Msg::Pressed,
                )],
                Msg::Pressed,
            )
            .force_compact(true)
            .tooltip_delay(short)
            .into(),
            Some(short),
        ),
        (
            "raw default remains immediate",
            FadeTooltip::new(content(), "Raw hint", Position::Bottom).into(),
            Some(time::Duration::ZERO),
        ),
        (
            "labelled menu",
            enzui_widgets::menu_button::enz_menu_button("Save", &menu, &state, |_| Msg::Pressed)
                .into(),
            None,
        ),
        (
            "window cluster",
            enz_window_controls(Msg::Pressed).into(),
            Some(default_delay),
        ),
        (
            "window cluster override",
            enz_window_controls(Msg::Pressed)
                .tooltip_delay(short)
                .into(),
            Some(short),
        ),
        (
            "titlebar controls override",
            enzui_widgets::enz_title_bar()
                .on_gesture(|_| Msg::Pressed)
                .tooltip_delay(short)
                .into(),
            Some(short),
        ),
        (
            "helper",
            enzui_widgets::enz_tooltip(content(), "Hint", Position::Bottom),
            Some(default_delay),
        ),
        (
            "toolbar name",
            enz_toolbar_icon("settings", Msg::Pressed)
                .aria_label("Settings")
                .into(),
            Some(default_delay),
        ),
        (
            "window name",
            enz_window_control_button("window.close", Msg::Pressed)
                .aria_label("Close")
                .into(),
            Some(default_delay),
        ),
        (
            "sidebar name",
            enz_sidebar_toggle_button(Msg::Pressed).into(),
            Some(default_delay),
        ),
        (
            "icon button",
            enz_button("Save", Msg::Pressed)
                .icon("save")
                .icon_position(ButtonIconPosition::Only)
                .into(),
            Some(default_delay),
        ),
        (
            "visible caption",
            enz_button("Save", Msg::Pressed).into(),
            None,
        ),
        (
            "explicit help",
            enz_button("Save", Msg::Pressed)
                .tooltip("Save this document")
                .into(),
            Some(default_delay),
        ),
        (
            "empty override",
            enz_toolbar_icon("settings", Msg::Pressed)
                .aria_label("Settings")
                .tooltip("")
                .into(),
            None,
        ),
        (
            "immediate override",
            enz_toolbar_icon("settings", Msg::Pressed)
                .aria_label("Settings")
                .tooltip_delay(time::Duration::ZERO)
                .into(),
            Some(time::Duration::ZERO),
        ),
        (
            "cluster opt-out",
            enz_window_controls(Msg::Pressed).tooltips(false).into(),
            None,
        ),
        (
            "named tab",
            enz_tab_list(
                [TabPillItem::new("").icon("settings").aria_label("Settings")],
                0,
                |_| Msg::Pressed,
            )
            .into(),
            Some(default_delay),
        ),
        (
            "visible tab",
            enz_tab_list([TabPillItem::new("Settings")], 0, |_| Msg::Pressed).into(),
            None,
        ),
    ];
    for (name, mut element, expected) in cases {
        let (mut tree, node) = prepare(&mut element, &renderer);
        let mut semantics = enzui_core::collect::SemanticsCollector::new();
        element.as_widget_mut().operate(
            &mut tree,
            iced::advanced::Layout::of(&node),
            &Rectangle::with_size(Size::new(W, H)),
            &renderer,
            &mut semantics,
        );
        let over = semantics
            .collected()
            .iter()
            .find(|n| {
                matches!(
                    n.semantics.role,
                    enzui_core::semantics::Role::Button | enzui_core::semantics::Role::Tab
                )
            })
            .map_or(OVER, |n| n.bounds.center());
        let threshold = expected.unwrap_or(default_delay);
        deliver(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            redraw_at(origin),
            over,
        );
        deliver(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            move_to(over),
            over,
        );
        assert_eq!(
            overlay_is_published(&mut element, &mut tree, &node, &renderer),
            expected.is_some_and(|v| v.is_zero()),
            "{name}: initial hover"
        );
        deliver(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            redraw_at(origin + threshold.saturating_sub(time::Duration::from_millis(1))),
            over,
        );
        assert_eq!(
            overlay_is_published(&mut element, &mut tree, &node, &renderer),
            expected.is_some_and(|v| v.is_zero()),
            "{name}: before delay"
        );
        deliver(
            &mut element,
            &mut tree,
            &node,
            &renderer,
            redraw_at(origin + threshold + TOOLTIP_FADE),
            over,
        );
        assert_eq!(
            overlay_is_published(&mut element, &mut tree, &node, &renderer),
            expected.is_some(),
            "{name}: after delay"
        );
    }
    if !child {
        // A separate process isolates the application-wide policy from parallel tests.
        let test = concat!(
            module_path!(),
            "::control_tooltip_defaults_and_overrides_reach_the_native_overlay"
        );
        let test = test.split_once("::").expect("integration test module").1;
        let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", test, "--nocapture"])
            .env(POLICY_CHILD, "1")
            .output()
            .expect("isolated policy test");
        assert!(
            output.status.success(),
            "custom app policy: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("test result: ok. 1 passed"),
            "the isolated policy test must match exactly once"
        );
    }
}
