//! Opt-in tracing for shell layout durations and frame intervals, logged at
//! info under `enzui::trace::frame_cost`.
//!
//! Set `ENZUI_FRAME_COST` to a nonempty value other than `0` to enable tracing.
//! Presence of `ENZUI_DISABLE_RESPONSIVE_ISLAND` routes docked motion through
//! root invalidation for comparison. Both settings are cached on first use.
//! Disabled timing performs no clock reads. Pair duration reports with
//! [`super::build_probe`] counters to distinguish builds, diffs, and layout work.

use std::sync::OnceLock;
use std::time::Instant;

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("ENZUI_FRAME_COST").is_ok_and(|value| value != "0" && !value.is_empty())
    })
}

pub(crate) fn isolation_disabled() -> bool {
    static DISABLED: OnceLock<bool> = OnceLock::new();
    *DISABLED.get_or_init(|| std::env::var_os("ENZUI_DISABLE_RESPONSIVE_ISLAND").is_some())
}

/// Start timing a root layout pass. `None` when tracing is off, and the
/// clock is then never read.
#[inline]
#[must_use]
pub fn begin() -> Option<Instant> {
    enabled().then(Instant::now)
}

/// Finish timing a root layout pass and print it.
#[inline]
pub fn end(started: Option<Instant>, rebuilt: bool, inline_progress: f32, width: f32) {
    finish(started, "root", rebuilt, inline_progress, width);
}

/// Finish timing a local retained-island layout pass.
#[inline]
pub fn end_responsive(started: Option<Instant>, inline_progress: f32, width: f32) {
    finish(started, "island", false, inline_progress, width);
}

fn finish(started: Option<Instant>, kind: &str, rebuilt: bool, inline_progress: f32, width: f32) {
    if let Some(started) = started {
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        log::info!(
            target: "enzui::trace::frame_cost",
            "pass ms={ms:.3} rebuilt={} inline_progress={inline_progress:.4} \
             width={width:.1} kind={kind}",
            u8::from(rebuilt)
        );
    }
}

/// Note that the runtime handed the shell a `RedrawRequested`, so the
/// passes that follow can be attributed to one frame.
#[inline]
pub fn note_frame(at: iced::time::Instant) {
    if !enabled() {
        return;
    }
    use std::cell::Cell;
    thread_local! {
        static PREVIOUS: Cell<Option<iced::time::Instant>> = const { Cell::new(None) };
    }
    let dt = PREVIOUS.replace(Some(at)).map_or(f64::NAN, |previous| {
        at.duration_since(previous).as_secs_f64() * 1000.0
    });
    log::info!(target: "enzui::trace::frame_cost", "frame dt_ms={dt:.2}");
}
