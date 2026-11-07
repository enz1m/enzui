//! `EnzAppShell::sidebar_available` — a destination that owns no sidebar
//! collapses the pane as motion, and publishes no toggle for it.
//!
//! The gates measure the trajectory, not the endpoint: an instant collapse and
//! an animated one agree about every frame except the ones in between. The
//! negative control is a route that supplies no sidebar slot at all, which must
//! still cut instantly.

use super::motion_fixture as fixture;
use enzui_core::collect::SemanticsCollector;
use enzui_core::semantics::Role;
use enzui_theme::Palette;
use enzui_widgets::app_shell::{ShellLayout, ShellTokens, enz_app_shell};
use enzui_widgets::nav_split::NavSplitState;
use enzui_widgets::toolbar_icon::ToolbarIconTokens;
use iced::widget::container;
use iced::{Color, Element, Length, Size};
use iced_test::renderer::Renderer;
use iced_test::runtime::user_interface::{Cache, UserInterface};

type Msg = fixture::Msg;

const DESKTOP: Size = Size {
    width: 1400.0,
    height: 900.0,
};

// The two ways a route can say "no sidebar here": the declaration, and
// `slot: false` for a caller that supplies no `.sidebar(..)` at all.
fn shell(available: bool, slot: bool) -> Element<'static, Msg> {
    shell_with_header(available, slot, false)
}

// `header` gives the sidebar its own titled band, which carries the toggle.
fn shell_with_header(available: bool, slot: bool, header: bool) -> Element<'static, Msg> {
    let mut toggle_tokens = ToolbarIconTokens::from_palette(&Palette::dark());
    toggle_tokens.icon_color = Color::from_rgb(1.0, 0.0, 1.0);
    let tokens = ShellTokens {
        toggle_icon_tokens: Some(toggle_tokens),
        ..ShellTokens::default()
    };
    let mut shell = enz_app_shell(|| iced::widget::Space::new().into())
        .tokens(tokens)
        .layout(ShellLayout::TitlebarSidebarContent)
        .state(NavSplitState::new())
        .titlebar(|| iced::widget::Space::new().into())
        .on_toggle_sidebar(fixture::Msg::Toggle)
        .sidebar_available(available);
    if header {
        shell = shell.sidebar_titlebar(|| {
            enzui_widgets::title_bar::enz_title_bar()
                .title("Resources")
                .into_element()
        });
    }
    if slot {
        shell = shell.sidebar(|| {
            container(iced::widget::Space::new())
                .width(Length::Fill)
                .height(Length::Fill)
                .style(|_| container::Style {
                    background: Some(fixture::SIDEBAR_INK.into()),
                    ..container::Style::default()
                })
                .into()
        });
    }
    shell.into()
}

// A sidebar hidden by preference keeps its toggle — that is how it comes back
// — so availability cannot be expressed as `.forced(Hidden)`.
#[test]
fn sidebar_routes_and_render_lane_remain_live_across_visibility_changes_1() {
    // a_route_without_a_sidebar_publishes_no_toggle
    {
        fn toggles(available: bool) -> usize {
            let mut renderer = fixture::new_renderer();
            let mut ui = UserInterface::build(
                shell(available, true),
                DESKTOP,
                Cache::default(),
                &mut renderer,
            );
            let mut collector = SemanticsCollector::new();
            ui.operate(&renderer, {
                let op: &mut dyn iced::advanced::widget::Operation = &mut collector;
                op
            });
            collector
                .into_collected()
                .into_iter()
                .filter(|n| {
                    n.semantics.role == Role::Button
                        && n.semantics
                            .label
                            .as_deref()
                            .is_some_and(|l| l.ends_with("sidebar"))
                })
                .count()
        }
        assert_eq!(
            toggles(true),
            1,
            "control: a route WITH a sidebar publishes exactly one toggle"
        );
        assert_eq!(
            toggles(false),
            0,
            "a route with no sidebar must publish no control for one"
        );
    }

    // a_toggle_travels_with_a_sidebar_that_arrives_or_leaves
    {
        for (from, to) in [(false, true), (true, false)] {
            let mut renderer = fixture::new_renderer();
            let theme = fixture::theme(false);
            let mut ui = UserInterface::build(
                shell_with_header(from, true, true),
                DESKTOP,
                Cache::default(),
                &mut renderer,
            );
            fixture::paint(&mut ui, &mut renderer, &theme);
            let cache = ui.into_cache();

            let _ = enzui_widgets::app_shell::build_probe::take_motion_edge();
            let mut ui = UserInterface::build(
                shell_with_header(to, true, true),
                DESKTOP,
                cache,
                &mut renderer,
            );
            let start = enzui_widgets::app_shell::build_probe::take_motion_edge()
                .expect("an availability change is a transition");
            let frame = |ui: &mut UserInterface<'_, Msg, iced::Theme, Renderer>,
                         renderer: &mut Renderer,
                         ms: u64| {
                let _ = ui.update(
                    &iced::window::Headless,
                    &iced::advanced::graphics::core::shell::Waker::noop(),
                    &[iced::Event::Window(iced::window::Event::RedrawRequested(
                        start + iced::time::Duration::from_millis(ms),
                    ))],
                    iced::mouse::Cursor::Unavailable,
                    renderer,
                    &mut iced::advanced::shell::Bus::new(),
                );
                fixture::paint(ui, renderer, &theme);
            };

            frame(&mut ui, &mut renderer, 0);
            frame(&mut ui, &mut renderer, 90);
            let sidebar = fixture::sidebar_extent(&mut renderer, &theme, DESKTOP);
            let toggle = fixture::toggle_ink(&mut renderer, &theme, DESKTOP);
            assert!(
                sidebar > 30.0 && sidebar < 230.0,
                "{from} -> {to}: the frame is mid-travel ({sidebar} px of sidebar)"
            );
            let toggle = toggle.unwrap_or_else(|| {
                panic!("{from} -> {to}: the toggle is painted mid-travel ({sidebar} px of sidebar)")
            });
            assert!(
                toggle.x + toggle.width <= sidebar + 1.0,
                "{from} -> {to}: the toggle {toggle:?} rides inside the {sidebar} px band"
            );
            assert_eq!(
                sidebar_toggles(&mut ui, &renderer),
                0,
                "{from} -> {to}: a travelling toggle takes no input"
            );

            // A running app's frames to the end of the travel: a docked travel
            // advances at most a step per frame.
            for ms in (106..=2_000).step_by(16) {
                frame(&mut ui, &mut renderer, ms);
            }
            assert_eq!(
                sidebar_toggles(&mut ui, &renderer),
                usize::from(to),
                "{from} -> {to}: the settled route publishes its toggle"
            );
        }
    }
}

fn sidebar_toggles(
    ui: &mut UserInterface<'_, Msg, iced::Theme, Renderer>,
    renderer: &Renderer,
) -> usize {
    let mut collector = SemanticsCollector::new();
    ui.operate(renderer, {
        let op: &mut dyn iced::advanced::widget::Operation = &mut collector;
        op
    });
    collector
        .into_collected()
        .into_iter()
        .filter(|n| {
            n.semantics.role == Role::Button
                && n.semantics
                    .label
                    .as_deref()
                    .is_some_and(|l| l.ends_with("sidebar"))
        })
        .count()
}

/// What the render lane presents at `at` from one recorded frame: the
/// frame's animations sampled at `at`, and its static tracks for every node
/// no animation moves.
fn lane_tracks(
    animations: &[iced::advanced::renderer::TrackAnimation],
    tracks: &[iced::advanced::renderer::Track],
    at: iced::time::Instant,
) -> Vec<iced::advanced::renderer::Track> {
    let mut sampled: Vec<_> = animations
        .iter()
        .filter_map(|animation| animation.sample(at))
        .collect();
    for track in tracks {
        if !sampled.iter().any(|existing| existing.node == track.node) {
            sampled.push(*track);
        }
    }
    sampled
}

/// A recorded frame of a sidebar reveal carries its travel: replayed with its
/// published curve at a later instant, it shows the sidebar where a full
/// redraw at that instant puts it. (A lane presenting a travel the UI thread
/// repaints holds a late frame instead; the recording stays replayable.)
///
/// Sabotage: stop the seam group extrapolating -- the replay holds the
/// sidebar at its recorded width. Record the travelling panel without its
/// off-window part -- the replay shows the panel cut at its recorded width.
#[test]
fn a_late_ui_frame_leaves_the_render_lane_moving_the_sidebar() {
    use iced::advanced::Renderer as _;
    use iced_test::runtime::user_interface::{State, replay_retained};

    let theme = fixture::theme(false);
    let style = iced::advanced::renderer::Style {
        text_color: <iced::Theme as iced::advanced::graphics::core::theme::Base>::base(&theme)
            .text_color,
    };
    let frame = |ui: &mut UserInterface<'_, Msg, iced::Theme, Renderer>,
                 renderer: &mut Renderer,
                 at: iced::time::Instant| {
        let (state, _) = ui.update(
            &iced::window::Headless,
            &iced::advanced::graphics::core::shell::Waker::noop(),
            &[iced::Event::Window(iced::window::Event::RedrawRequested(
                at,
            ))],
            iced::mouse::Cursor::Unavailable,
            renderer,
            &mut iced::advanced::shell::Bus::new(),
        );
        state
    };
    // Settle with the sidebar unavailable, then make it available: the
    // reveal's edge is stamped by that rebuild.
    let start = |renderer: &mut Renderer| {
        let mut ui = UserInterface::build(
            shell_with_header(false, true, true),
            DESKTOP,
            Cache::default(),
            renderer,
        );
        fixture::paint(&mut ui, renderer, &theme);
        let cache = ui.into_cache();
        let _ = enzui_widgets::app_shell::build_probe::take_motion_edge();
        let ui = UserInterface::build(
            shell_with_header(true, true, true),
            DESKTOP,
            cache,
            renderer,
        );
        let edge = enzui_widgets::app_shell::build_probe::take_motion_edge()
            .expect("an availability change is a transition");
        (ui, edge)
    };
    let late = iced::time::Duration::from_millis(100);
    // The render lane is the wgpu backend's retained path.
    let wgpu = || {
        iced_test::futures::futures::executor::block_on(
            <Renderer as iced::advanced::renderer::Headless>::new(
                iced::advanced::renderer::Settings::default(),
                Some("wgpu"),
            ),
        )
    };
    let Some(mut renderer) = wgpu() else {
        eprintln!("SKIPPED: no wgpu adapter");
        return;
    };

    // The truth: the UI thread on time, drawing every frame.
    let (mut ui, edge) = start(&mut renderer);
    let mut at = edge;
    while at <= edge + late {
        let _ = frame(&mut ui, &mut renderer, at);
        fixture::paint(&mut ui, &mut renderer, &theme);
        at += iced::time::Duration::from_micros(6_944);
    }
    let _ = frame(&mut ui, &mut renderer, edge + late);
    fixture::paint(&mut ui, &mut renderer, &theme);
    let truth = fixture::sidebar_extent(&mut renderer, &theme, DESKTOP);
    drop(ui);

    // The late UI thread: its last frame before `late` is either the
    // transition's first, with nothing of the sidebar reserved yet, or the
    // one after it.
    for frames_before in 0..2 {
        let mut renderer = wgpu().expect("the adapter found above");
        let (mut ui, edge) = start(&mut renderer);
        let mut at = edge;
        for _ in 0..frames_before {
            let _ = frame(&mut ui, &mut renderer, at);
            fixture::paint(&mut ui, &mut renderer, &theme);
            at += iced::time::Duration::from_micros(6_944);
        }
        let state = frame(&mut ui, &mut renderer, at);
        let scene = ui
            .draw_retained(
                &mut renderer,
                &theme,
                &style,
                iced::mouse::Cursor::Unavailable,
            )
            .expect("the shell frame is recorded");
        let recorded = fixture::sidebar_extent(&mut renderer, &theme, DESKTOP);
        let State::Updated {
            tracks, animations, ..
        } = state
        else {
            panic!("the frame updated the interface");
        };
        assert!(
            !animations.is_empty(),
            "the recorded frame publishes the travel for the render lane"
        );
        renderer.reset(iced::Rectangle::with_size(DESKTOP));
        assert!(
            replay_retained(
                &mut renderer,
                DESKTOP,
                &scene,
                &lane_tracks(&animations, &tracks, edge + late)
            ),
            "the recorded frame replays"
        );
        let replayed = fixture::sidebar_extent(&mut renderer, &theme, DESKTOP);

        assert!(
            truth > recorded + 40.0,
            "the travel moves on after the recorded frame ({recorded} -> {truth} px)"
        );
        assert!(
            (replayed - truth).abs() <= 2.0,
            "after {frames_before} frames, the lane shows {replayed} px of sidebar where a \
             full redraw shows {truth} px (recorded at {recorded} px)"
        );
    }
}

/// A page beside the sidebar, as Task Manager's Processes page: a header
/// whose control (Filter) sits at its trailing edge, above a table that fills
/// the width and an inspector of fixed width at the trailing edge.
fn shell_with_inspector(available: bool) -> Element<'static, Msg> {
    let ink = |color: Color| {
        move |_: &iced::Theme| container::Style {
            background: Some(color.into()),
            ..container::Style::default()
        }
    };
    enz_app_shell(move || {
        iced::widget::column![
            iced::widget::row![
                iced::widget::Space::new().width(Length::Fill),
                container(iced::widget::Space::new())
                    .width(Length::Fixed(HEADER_CONTROL_WIDTH))
                    .height(Length::Fixed(24.0))
                    .style(ink(HEADER_CONTROL_INK)),
            ],
            iced::widget::row![
                container(iced::widget::Space::new())
                    .width(Length::Fill)
                    .height(Length::Fill),
                container(iced::widget::Space::new())
                    .width(Length::Fixed(INSPECTOR_WIDTH))
                    .height(Length::Fill)
                    .style(ink(INSPECTOR_INK)),
            ],
        ]
        .into()
    })
    .layout(ShellLayout::TitlebarSidebarContent)
    .state(NavSplitState::new())
    .titlebar(|| iced::widget::Space::new().into())
    .on_toggle_sidebar(fixture::Msg::Toggle)
    .sidebar_available(available)
    .sidebar(|| {
        container(iced::widget::Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(fixture::SIDEBAR_INK.into()),
                ..container::Style::default()
            })
            .into()
    })
    .into()
}

const INSPECTOR_WIDTH: f32 = 180.0;
const INSPECTOR_INK: Color = Color::from_rgb(1.0, 0.5, 0.0);
const HEADER_CONTROL_WIDTH: f32 = 60.0;
const HEADER_CONTROL_INK: Color = Color::from_rgb(0.5, 0.0, 1.0);

/// Every frame of a docked travel the UI thread is on time for shows that
/// frame's own layout: the content beside the sidebar reflows, and the
/// inspector and the page header's control at its trailing edge stay at the
/// window's edge while the sidebar opens or closes (enzUI-ICED #3660). The
/// render lane's side, a late UI frame held rather than moved rigidly, is
/// `iced_graphics`'s `lane::tests::a_late_commit_holds_and_a_stall_carries_on`.
///
/// The frames are presented the way the runtime presents them: a frame that
/// changed nothing but published tracks replays the last recorded scene
/// under those tracks; any other frame is drawn and recorded.
///
/// Sabotage: make the shell ask for a track frame instead of a drawn one
/// while a docked travel lays it out -- the travel's frames replay the first
/// one moved rigidly, and the inspector leaves the window's edge.
#[test]
fn a_docked_travel_keeps_trailing_content_at_the_window_edge() {
    use iced::advanced::Renderer as _;
    use iced_test::runtime::user_interface::{State, replay_retained};

    let theme = fixture::theme(false);
    let style = iced::advanced::renderer::Style {
        text_color: <iced::Theme as iced::advanced::graphics::core::theme::Base>::base(&theme)
            .text_color,
    };
    let Some(mut renderer) = iced_test::futures::futures::executor::block_on(
        <Renderer as iced::advanced::renderer::Headless>::new(
            iced::advanced::renderer::Settings::default(),
            Some("wgpu"),
        ),
    ) else {
        eprintln!("SKIPPED: no wgpu adapter");
        return;
    };

    for (from, to) in [(true, false), (false, true)] {
        let mut ui = UserInterface::build(
            shell_with_inspector(from),
            DESKTOP,
            Cache::default(),
            &mut renderer,
        );
        fixture::paint(&mut ui, &mut renderer, &theme);
        let cache = ui.into_cache();
        let _ = enzui_widgets::app_shell::build_probe::take_motion_edge();
        let mut ui = UserInterface::build(shell_with_inspector(to), DESKTOP, cache, &mut renderer);
        let edge = enzui_widgets::app_shell::build_probe::take_motion_edge()
            .expect("an availability change is a transition");

        let mut scene = None;
        let mut mid_travel = 0;
        let mut at = edge;
        while at <= edge + iced::time::Duration::from_millis(600) {
            let (state, _) = ui.update(
                &iced::window::Headless,
                &iced::advanced::graphics::core::shell::Waker::noop(),
                &[iced::Event::Window(iced::window::Event::RedrawRequested(
                    at,
                ))],
                iced::mouse::Cursor::Unavailable,
                &mut renderer,
                &mut iced::advanced::shell::Bus::new(),
            );
            let State::Updated {
                is_animation_only,
                tracks,
                ..
            } = state
            else {
                panic!("the frame updated the interface");
            };
            match (&scene, is_animation_only) {
                (Some(scene), true) => {
                    renderer.reset(iced::Rectangle::with_size(DESKTOP));
                    assert!(
                        replay_retained(&mut renderer, DESKTOP, scene, &tracks),
                        "the recorded frame replays"
                    );
                }
                _ => {
                    scene = ui.draw_retained(
                        &mut renderer,
                        &theme,
                        &style,
                        iced::mouse::Cursor::Unavailable,
                    );
                }
            }
            let sidebar = fixture::sidebar_extent(&mut renderer, &theme, DESKTOP);
            let inspector = fixture::ink_bounds(&mut renderer, &theme, DESKTOP, INSPECTOR_INK)
                .expect("the inspector is painted");
            let control = fixture::ink_bounds(&mut renderer, &theme, DESKTOP, HEADER_CONTROL_INK)
                .expect("the header control is painted");
            assert!(
                (control.x + control.width - DESKTOP.width).abs() <= 1.0,
                "{from} -> {to}, {} ms: the header control {control:?} sits at the \
                 trailing edge beside {sidebar} px of sidebar",
                (at - edge).as_millis()
            );
            if sidebar > 20.0 && sidebar < DESKTOP.width {
                mid_travel += 1;
            }
            assert!(
                (inspector.x + inspector.width - DESKTOP.width).abs() <= 1.0
                    && (inspector.width - INSPECTOR_WIDTH).abs() <= 1.0,
                "{from} -> {to}, {} ms: the inspector {inspector:?} sits at the window's \
                 trailing edge beside {sidebar} px of sidebar",
                (at - edge).as_millis()
            );
            at += iced::time::Duration::from_micros(16_667);
        }
        assert!(
            mid_travel >= 4,
            "{from} -> {to}: the frames cover the travel ({mid_travel} mid-travel)"
        );
    }
}

/// A UI thread that stalls during a docked travel (the first frames of a page
/// being built) resumes the slide where it held, one step on, instead of
/// jumping ahead: the render lane holds a late layout rather than moving it
/// rigidly, so the travel's clock is delayed by the stall (enzUI-ICED #3660).
///
/// Sabotage: stop delaying the clock after a late frame -- the frame after a
/// 100 ms stall shows the sidebar far past where a frame one step later does.
#[test]
fn a_stalled_docked_travel_resumes_where_it_held() {
    let theme = fixture::theme(false);
    let travel = |late_ms: u64| {
        let mut renderer = fixture::new_renderer();
        let mut ui = UserInterface::build(
            shell_with_inspector(true),
            DESKTOP,
            Cache::default(),
            &mut renderer,
        );
        fixture::paint(&mut ui, &mut renderer, &theme);
        let cache = ui.into_cache();
        let _ = enzui_widgets::app_shell::build_probe::take_motion_edge();
        let mut ui =
            UserInterface::build(shell_with_inspector(false), DESKTOP, cache, &mut renderer);
        let edge = enzui_widgets::app_shell::build_probe::take_motion_edge()
            .expect("an availability change is a transition");
        let mut extent = 0.0;
        for ms in [0, 16, 16 + late_ms] {
            let _ = ui.update(
                &iced::window::Headless,
                &iced::advanced::graphics::core::shell::Waker::noop(),
                &[iced::Event::Window(iced::window::Event::RedrawRequested(
                    edge + iced::time::Duration::from_millis(ms),
                ))],
                iced::mouse::Cursor::Unavailable,
                &mut renderer,
                &mut iced::advanced::shell::Bus::new(),
            );
            fixture::paint(&mut ui, &mut renderer, &theme);
            extent = fixture::sidebar_extent(&mut renderer, &theme, DESKTOP);
        }
        extent
    };
    let stepped = travel(33);
    let stalled = travel(100);
    assert!(
        stepped > 20.0 && stepped < 240.0,
        "the reference frame is mid-travel ({stepped} px)"
    );
    assert!(
        (stalled - stepped).abs() <= 2.0,
        "after a 100 ms stall the sidebar is at {stalled} px, where a frame one \
         step on shows {stepped} px"
    );
}

// Type anchor: the harness renderer type is the one the fixture builds.
const _: fn() -> Renderer = fixture::new_renderer;
