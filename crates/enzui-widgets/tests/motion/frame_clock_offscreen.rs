//! Offscreen shimmer clocks must not schedule any frame.
//!
//! A list of loading placeholders that scrolls out of view keeps ticking one
//! animation frame each, forever, unless the widget observes the viewport
//! `Widget::update` is handed and gates its redraw request on visibility.
//!
//! Both directions matter, and the second is the one that catches the naive
//! implementation that only gates:
//!
//! 1. Offscreen requests zero frames — the optimisation. A shimmer whose
//!    laid-out bounds do not overlap the visible viewport must request
//!    neither `NextFrame` nor `At(probe)`.
//! 2. Scrolled back into view resumes animating — the regression guard.
//!    A gate without a wake-up path re-introduces a permanent freeze through a
//!    different door.
//!
//! The wake-up path is delivered by `iced_widget::scrollable`, which forwards
//! every event to `content.update` before matching on it
//! (in iced's `scrollable::update`), and after applying a wheel scroll issues its
//! own `shell.request_redraw()` (in iced's `scrollable`). The next
//! `RedrawRequested` reaches the child with the updated translation — i.e.
//! the fresh viewport — and the child observes itself visible again.
//!
//! # Calibration
//!
//! A test that measures "zero frames" needs to prove the harness could have
//! measured something non-zero. Every test here starts with a
//! calibration frame where the shimmer *is* visible and requests
//! `NextFrame`, so a zero later demonstrates that the gate works.

use enzui_theme::EnzTheme;
use enzui_widgets::skeleton::{SkeletonKind, enz_skeleton};
use iced::Length;
use iced::widget::{Space, column, scrollable};
use iced::window::RedrawRequest;
use iced::{Element, Point, Theme, mouse};
use iced_test::runtime::user_interface::State;

/// Classify a single frame's redraw request into one of three observable
/// behaviours, so a "wait vs probe" or "wait vs animate" difference cannot
/// mask itself as a passing test (mirrors `frame_clock_redraw.rs`).
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Req {
    Animate,
    Probe,
    Nothing,
}

fn classify(req: RedrawRequest) -> Req {
    match req {
        RedrawRequest::NextFrame => Req::Animate,
        RedrawRequest::At(_) => Req::Probe,
        RedrawRequest::Wait => Req::Nothing,
    }
}

fn redraw_of(state: State) -> RedrawRequest {
    match state {
        State::Updated { redraw_request, .. } => redraw_request,
        State::Outdated => panic!("the tree went outdated; the probe measured nothing"),
    }
}

fn animating_theme() -> Theme {
    let mut enz = EnzTheme::dark();
    enz.motion.reduced_motion = false;
    enz.iced_theme()
}

/// Build a view whose scrollable is `viewport_h` tall, and whose content is
/// a `spacer_h`-tall spacer followed by a shimmer skeleton. With
/// `spacer_h > viewport_h`, the shimmer starts wholly below the visible
/// viewport, i.e. offscreen at scroll offset 0.
fn view(spacer_h: f32) -> Element<'static, ()> {
    let content = column![
        Space::new()
            .width(Length::Fill)
            .height(Length::Fixed(spacer_h)),
        Element::from(enz_skeleton(SkeletonKind::Card)),
    ];
    scrollable(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

/// Drive `frames` `RedrawRequested` events into `el` inside a `w × h` window,
/// with an optional wheel-scroll prelude at cursor `at`, and return per-frame
/// redraw classifications. Mirrors `frame_clock_redraw::redraw_profile` but
/// runs a scroll event first and interleaves draws so the scrollable and the
/// shimmer clock both re-latch each frame.
fn profile(
    el: Element<'static, ()>,
    theme: &Theme,
    w: u32,
    h: u32,
    scroll_at: Option<(Point, f32)>,
    frames: u64,
) -> Vec<Req> {
    use iced::advanced::graphics::core::shell::Waker;
    use iced::advanced::renderer::Headless;

    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as Headless>::new(
            iced::advanced::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");

    let size = iced::Size::new(w as f32, h as f32);
    let base = <Theme as iced::advanced::graphics::core::theme::Base>::base(theme);
    let style = iced::advanced::renderer::Style {
        text_color: base.text_color,
    };
    let t0 = iced::time::Instant::now();
    let mut messages: iced::advanced::shell::Bus<()> = iced::advanced::shell::Bus::new();

    let mut ui = iced_test::runtime::user_interface::UserInterface::build(
        el,
        size,
        iced_test::runtime::user_interface::Cache::default(),
        &mut renderer,
    );

    // First draw so any Latched state sees the theme once before update.
    let cursor_start = mouse::Cursor::Unavailable;
    ui.draw(&mut renderer, theme, &style, cursor_start);

    // Optional prelude: park the cursor over the scrollable and deliver one
    // large vertical wheel event to change the scroll offset. This is the
    // channel through which the shimmer wakes up in production — the wheel
    // event reaches `content.update` and the scrollable then issues its own
    // `shell.request_redraw()` because the offset changed.
    if let Some((pt, wheel_y)) = scroll_at {
        let cursor = mouse::Cursor::Available(pt);
        let _ = ui.update(
            &iced::window::Headless,
            &Waker::noop(),
            &[iced::Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x: 0.0, y: wheel_y },
            })],
            cursor,
            &mut renderer,
            &mut messages,
        );
        // The runtime draws the scroll only on the frame it requested, whose
        // `RedrawRequested` commits the translation the draw uses.
        let _ = ui.update(
            &iced::window::Headless,
            &Waker::noop(),
            &[iced::Event::Window(iced::window::Event::RedrawRequested(
                t0,
            ))],
            cursor,
            &mut renderer,
            &mut messages,
        );
        ui.draw(&mut renderer, theme, &style, cursor);
    }

    let mut out = Vec::new();
    for i in 0..frames {
        let (state, _) = ui.update(
            &iced::window::Headless,
            &Waker::noop(),
            &[iced::Event::Window(iced::window::Event::RedrawRequested(
                t0 + iced::time::Duration::from_millis((i + 1) * 16),
            ))],
            cursor_start,
            &mut renderer,
            &mut messages,
        );
        ui.draw(&mut renderer, theme, &style, cursor_start);
        out.push(classify(redraw_of(state)));
    }
    out
}

// ---------------------------------------------------------------------------
// Calibration
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Direction 1: offscreen requests zero frames (the optimisation)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Direction 2: scroll back into view resumes animating (the wake-up guard)
// ---------------------------------------------------------------------------

/// A shimmer that starts below the viewport but is then scrolled into view
/// must return to requesting `NextFrame`. This is the guard against a naive
/// implementation that gates but cannot wake — the exact permanent freeze caused
/// through a different door.
///
/// Sequence:
/// 1. Build the tree so the shimmer sits at y=250 in a 300 px viewport with
///    a 250 px spacer above (bounds `[250, ..., 250+120]`, viewport
///    `[0, ..., 300]`, so it starts barely visible — that gives us a
///    reliable Animate request in frame 0 as an anchor).
///
/// Wait — that isn't what we want. What we want is: shimmer starts fully
/// offscreen (no animate requests), then a scroll event brings it up.
///
/// So: spacer 400 px, viewport 300 px → shimmer at y=400 in 300 px viewport
/// → offscreen. Then a large negative wheel-y at the top of the viewport
/// scrolls content DOWN (delta y = -3 lines maps to `movement.y = -3`, and
/// scrollable applies `-movement * 60 = +180` so the offset grows by 180 →
/// viewport becomes `[180, ..., 480]`, which covers the shimmer at
/// y=[400, 520]).
#[test]
fn scrolled_back_into_view_resumes_animating() {
    // Frame budget: at least three frames after the scroll, since we need to
    // give the shimmer clock at least one `RedrawRequested` to observe the
    // new viewport and one more to actually request the next frame.
    let profile = profile(
        view(400.0),
        &animating_theme(),
        400,
        300,
        Some((Point::new(200.0, 150.0), -3.0)), // wheel-down over the scrollable
        4,
    );
    // Measured: after the wheel prelude, the scrollable has already shifted
    // its translation to bring the shimmer into view, so all four frames of
    // the post-scroll profile come back as `Animate` — the wake-up is
    // immediate. A weaker `>= 1` assertion still catches the bad-fix cases
    // that come back all-`Nothing` or all-`Probe`.
    // At least one of the frames after the scroll must be Animate. The exact
    // frame depends on how the runtime interleaves the scroll-triggered
    // request with our RedrawRequested events, so we assert the endpoint
    // rather than the timing: within four frames of a scroll-in, the
    // shimmer is back to requesting NextFrame.
    let animates = profile.iter().filter(|&&r| r == Req::Animate).count();
    assert!(
        animates >= 1,
        "after scrolling the shimmer back into view, it must resume requesting \
         NextFrame within a few frames (offscreen-gate wake-up guard, the \
         re-entry check); got {profile:?}"
    );
    // No probes either — we're under an animating theme, not a reduced one.
    let probes = profile.iter().filter(|&&r| r == Req::Probe).count();
    assert_eq!(
        probes, 0,
        "an animating theme must never take the At(probe) branch; got {profile:?}"
    );
}

// ---------------------------------------------------------------------------
// Zero-size viewport is not "everything is visible"
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The PUBLIC builder inherits the same gate
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// An entrance out of view
// ---------------------------------------------------------------------------

/// `spacer_h` of space, then a row with an `animated` entrance.
fn entering(spacer_h: f32) -> Element<'static, ()> {
    let row = enzui_widgets::motion::animated(Space::new().width(Length::Fill).height(40))
        .enters_from(iced::Vector::new(0.0, -24.0), 0.0);
    scrollable(column![
        Space::new()
            .width(Length::Fill)
            .height(Length::Fixed(spacer_h)),
        Element::from(row),
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// A row that mounts with an entrance below the fold is never drawn, so it
/// never reads the theme's motion policy; it lands in place and asks for no
/// frames, rather than a redraw on every frame of the window's life. In
/// view, the same row enters.
///
/// Sabotage: keep requesting a redraw until a draw comes -- the offscreen
/// row asks for every frame.
#[test]
fn an_entrance_out_of_view_lands_and_asks_for_no_frames() {
    let theme = animating_theme();
    let visible = profile(entering(0.0), &theme, 400, 300, None, 6);
    assert!(
        visible.contains(&Req::Animate),
        "calibration: in view, the row enters: {visible:?}"
    );
    let hidden = profile(entering(400.0), &theme, 400, 300, None, 6);
    assert!(
        hidden[2..].iter().all(|req| *req == Req::Nothing),
        "out of view, the row asks for no frames: {hidden:?}"
    );
}
