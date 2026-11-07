//! A live chart's timeline slides on the render lane: between samples the
//! UI thread is asked for no frame, every replayed frame draws what a full
//! redraw draws (across a new sample too), and the slide moves the picture
//! step by step.

use enzui_core::chart::{Series, SeriesId};
use enzui_frame_check::FrameCheck;
use enzui_widgets::area_chart::{ChartCurve, enz_area_chart};
use iced::time::{Duration, Instant};
use iced::widget::container;
use iced::{Element, Size};

/// A live chart of `samples` one second apart, the newest taken at `at`.
fn chart(samples: &[f64], at: Instant) -> Element<'static, ()> {
    let n = samples.len();
    let points: Vec<(f64, f64)> = samples
        .iter()
        .enumerate()
        .map(|(i, y)| ((i as f64) - (n as f64 - 1.0), *y))
        .collect();
    container(
        enz_area_chart("CPU")
            .push(Series::from_points(SeriesId(0), points))
            .curve(ChartCurve::Smooth)
            .live(Duration::from_secs(10), at)
            .width(360.0)
            .height(160.0)
            .into_element(),
    )
    .padding(16)
    .into()
}

/// Sabotage: publish only each frame's offset, not the drift -- the lane
/// has nothing to play and every step needs the UI thread; draw the areas
/// at the wall clock instead of the presented frame's instant -- a replayed
/// frame and a full redraw disagree; or measure the slide from each sample
/// rather than from one origin -- the last sample's slide, still playing
/// when the next sample's areas are recorded, throws them a sample back.
#[test]
fn a_live_chart_slides_without_the_ui_thread() {
    let mut theme = enzui_theme::EnzTheme::dark();
    theme.motion.reduced_motion = false;
    let samples = [10.0, 30.0, 20.0, 60.0, 40.0, 50.0];
    let start = Instant::now();
    let Some(mut check) = FrameCheck::new(Size::new(400.0, 200.0), theme.iced_theme(), move || {
        chart(&samples, start)
    }) else {
        eprintln!("SKIPPED: no wgpu adapter");
        return;
    };
    let (requests, changed, uncarried) = (
        check.frame_requests(),
        check.changed(),
        check.uncarried_tracks(),
    );
    // Most of a sample interval at 144 Hz, then the next sample arrives
    // and the timeline goes on sliding without a jump.
    check.frames(120);
    let next = [10.0, 30.0, 20.0, 60.0, 40.0, 50.0, 70.0];
    let at = start + Duration::from_secs(1);
    check.view(move || chart(&next, at));
    check.frames(60);
    check.assert_replays_match();
    let moved = check.changed() - changed;
    assert!(moved >= 10, "the timeline moved on only {moved} frames");
    assert!(
        check.frame_requests() - requests <= 8,
        "the slide asked the UI thread for {} frames",
        check.frame_requests() - requests
    );
    assert_eq!(
        check.uncarried_tracks() - uncarried,
        0,
        "a step moved the timeline with no drift the lane could play"
    );
}
