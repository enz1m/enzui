//! Animation-clock holds and geometry observations for sidebar captures.
//!
//! Holds and observations are thread-local. [`arm`] pins the next transition
//! at a fraction of its travel; [`release`] compensates for time held so motion
//! resumes without skipping. Geometry observations are published without a hold.
use std::cell::Cell;
use std::time::Duration;

use iced::time::Instant;

/// Published sidebar geometry from a layout or an unmounted endpoint.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Observation {
    /// Wrapping count of geometry publications on this thread; reset by [`reset`].
    pub sequence: u64,
    /// The eased travel fraction this layout pass read.
    pub progress: f32,
    /// Visible horizontal extent, in logical pixels. For overlays this is
    /// travel multiplied by resting width, without a content reservation.
    pub reserved_width: f32,
    /// The panel's settled width, in logical pixels.
    pub resting_width: f32,
}

/// Whether the shell is holding, about to hold, or ignoring the clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No capture hold; released holds may retain clock compensation.
    Off,
    /// Armed: the next transition edge resolves the pin.
    Armed,
    /// Pinned: every frame presents the same instant.
    Held,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Off,
    Armed { fraction: f32 },
    Held { at: Instant },
}

thread_local! {
    static MODE: Cell<Mode> = const { Cell::new(Mode::Off) };
    // Offset preserves elapsed travel across a hold.
    static OFFSET: Cell<Duration> = const { Cell::new(Duration::ZERO) };
    static LAST_RAW: Cell<Option<Instant>> = const { Cell::new(None) };
    static OBSERVATION: Cell<Option<Observation>> = const { Cell::new(None) };
    static SEQUENCE: Cell<u64> = const { Cell::new(0) };
    static UNPUBLISHED: Cell<u64> = const { Cell::new(0) };
    static REFUSALS: Cell<u64> = const { Cell::new(0) };
    // An unmounted endpoint uses the latest laid-out settled width.
    static LAST_RESTING: Cell<f32> = const { Cell::new(0.0) };
}

/// Ask the shell to pin the next transition at `fraction` of its travel.
///
/// Returns false without changes for a nonfinite fraction, an endpoint or
/// out-of-range value, or an already armed or held clock.
pub fn arm(fraction: f32) -> bool {
    if !fraction.is_finite() || fraction <= 0.0 || fraction >= 1.0 {
        return false;
    }
    if MODE.get() != Mode::Off {
        return false;
    }
    MODE.set(Mode::Armed { fraction });
    true
}

/// The current hold state.
#[must_use]
pub fn state() -> State {
    match MODE.get() {
        Mode::Off => State::Off,
        Mode::Armed { .. } => State::Armed,
        Mode::Held { .. } => State::Held,
    }
}

/// The instant the animation clock is pinned at, if it is pinned.
#[must_use]
pub fn pinned_at() -> Option<Instant> {
    match MODE.get() {
        Mode::Held { at } => Some(at),
        Mode::Off | Mode::Armed { .. } => None,
    }
}

/// Release a hold, resuming the travel from exactly where it was pinned.
///
/// Returns `false` when nothing was held. An armed-but-unresolved hold is
/// cleared, so a capture that never reached its transition cannot silently pin
/// a later one.
pub fn release() -> bool {
    match MODE.replace(Mode::Off) {
        Mode::Held { at } => {
            if let Some(raw) = LAST_RAW.get() {
                OFFSET.set(raw.saturating_duration_since(at));
            }
            true
        }
        Mode::Off => false,
        Mode::Armed { .. } => false,
    }
}

/// Clear every capture-hold effect on this thread, including the accumulated
/// clock offset and the published observation.
pub fn reset() {
    MODE.set(Mode::Off);
    OFFSET.set(Duration::ZERO);
    LAST_RAW.set(None);
    OBSERVATION.set(None);
    SEQUENCE.set(0);
    UNPUBLISHED.set(0);
    REFUSALS.set(0);
    LAST_RESTING.set(0.0);
}

/// Latest geometry publication on this thread, including overlays and settled
/// unmounted endpoints; `None` until a publication or after [`reset`].
#[must_use]
pub fn observation() -> Option<Observation> {
    OBSERVATION.get()
}

/// Number of reveal layouts that could not publish a visible horizontal extent.
#[must_use]
pub fn unpublished() -> u64 {
    UNPUBLISHED.get()
}

/// Holds that could not be resolved because the travel they named was already
/// over.
#[must_use]
pub fn refusals() -> u64 {
    REFUSALS.get()
}

pub(crate) fn note_reserve_layout(progress: f32, reserved_width: f32, resting_width: f32) {
    if resting_width.is_finite() && resting_width > 0.0 {
        LAST_RESTING.set(resting_width);
    }
    let sequence = SEQUENCE.get().wrapping_add(1);
    SEQUENCE.set(sequence);
    OBSERVATION.set(Some(Observation {
        sequence,
        progress,
        reserved_width,
        resting_width,
    }));
}

pub(crate) fn note_unmounted_docked(progress: f32) {
    let resting = LAST_RESTING.get();
    if resting <= 0.0 {
        note_unpublished_reveal();
        return;
    }
    let sequence = SEQUENCE.get().wrapping_add(1);
    SEQUENCE.set(sequence);
    OBSERVATION.set(Some(Observation {
        sequence,
        progress,
        reserved_width: 0.0,
        resting_width: resting,
    }));
}

pub(crate) fn note_overlay_layout(progress: f32, resting_width: f32) {
    if resting_width.is_finite() && resting_width > 0.0 {
        LAST_RESTING.set(resting_width);
    }
    let sequence = SEQUENCE.get().wrapping_add(1);
    SEQUENCE.set(sequence);
    OBSERVATION.set(Some(Observation {
        sequence,
        progress,
        reserved_width: progress * resting_width,
        resting_width,
    }));
}

pub(crate) fn note_unpublished_reveal() {
    UNPUBLISHED.set(UNPUBLISHED.get().wrapping_add(1));
}

pub(crate) fn effective(raw: Instant) -> Instant {
    LAST_RAW.set(Some(raw));
    match MODE.get() {
        Mode::Held { at } => at,
        Mode::Off | Mode::Armed { .. } => raw.checked_sub(OFFSET.get()).unwrap_or(raw),
    }
}

// An elapsed pin would replay frames the compositor already showed.
pub(crate) fn resolve_pin(edge: Instant, travel: Duration, now: Instant) {
    let Mode::Armed { fraction } = MODE.get() else {
        return;
    };
    let at = edge + travel.mul_f32(fraction);
    if at <= now || travel.is_zero() {
        MODE.set(Mode::Off);
        REFUSALS.set(REFUSALS.get().wrapping_add(1));
        return;
    }
    MODE.set(Mode::Held { at });
}

pub(crate) fn clock_is_unshifted() -> bool {
    MODE.with(|mode| matches!(mode.get(), Mode::Off))
        && OFFSET.with(|offset| offset.get().is_zero())
}
