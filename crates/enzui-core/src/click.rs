//! Renderer-independent multi-click classification and single-click dispatch policy.
//!
//! Callers provide monotonic timestamps and choose immediate or deferred single-click handling explicitly.

use iced_core::{Point, time::Instant};
use std::time::Duration;

/// Maximum interval between consecutive presses where the desktop sets none.
///
/// The 400 ms value follows GTK and Qt desktop defaults. [`ClickConfig::new`]
/// uses the desktop's double-click time
/// ([`desktop_settings::input`](crate::desktop_settings::input)); deferred
/// dispatch waits for that full interval.
pub const DEFAULT_MULTI_CLICK_INTERVAL: Duration =
    iced_core::preferences::Preferences::DEFAULT.double_click_time;

/// Exclusive movement radius between consecutive presses where the desktop
/// sets none; iced classifies clicks with the same value.
pub const DEFAULT_CLICK_DEADBAND: f32 =
    iced_core::preferences::Preferences::DEFAULT.double_click_distance;

/// Relationship between a single-click action and a possible later double click.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleClickPolicy {
    /// Dispatches every single-click action immediately, allowing a later double-click action to be additional.
    Immediate,
    /// Defers the single-click action until the interval expires and cancels it when a double click completes.
    Deferred,
}

/// Timing, movement, and single-click dispatch settings for a detector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClickConfig {
    /// Maximum elapsed time between consecutive presses (inclusive).
    pub interval: Duration,
    /// Maximum distance between consecutive press positions (exclusive).
    pub deadband: f32,
    /// Whether the first single-click action is immediate or deferred.
    pub single_click: SingleClickPolicy,
}

impl ClickConfig {
    /// Build the desktop's double-click time and distance with an explicit
    /// single-click policy.
    #[must_use]
    pub fn new(single_click: SingleClickPolicy) -> Self {
        let input = crate::desktop_settings::input();
        Self {
            interval: input.double_click_time,
            deadband: input.double_click_distance,
            single_click,
        }
    }

    /// Override the maximum interval between consecutive presses.
    #[must_use]
    pub const fn interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// Sets the exclusive movement radius.
    ///
    /// Negative values and NaN prevent consecutive classification; positive infinity removes the movement bound.
    #[must_use]
    pub const fn deadband(mut self, deadband: f32) -> Self {
        self.deadband = deadband;
        self
    }
}

/// Iced-compatible single-, double-, or triple-click classification for the current press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickCount {
    /// The start of a click sequence.
    Single,
    /// A double-click classification.
    Double,
    /// A triple-click classification.
    Triple,
}

impl ClickCount {
    fn next(self) -> Self {
        match self {
            Self::Single => Self::Double,
            Self::Double => Self::Triple,
            // Match iced’s post-triple sequence by alternating subsequent rapid presses between double and triple.
            Self::Triple => Self::Double,
        }
    }
}

/// What the caller should do with its single-click action for this press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleClickDisposition {
    /// Dispatch the single-click action synchronously for this press.
    DispatchNow,
    /// Holds the action until it matures or a double click cancels it.
    Defer,
    /// Cancel the pending action because this press completed a double click.
    CancelDeferred,
    /// No single-click action is pending or newly requested.
    None,
}

/// Classification and action-policy result for one press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClickReport {
    /// Single-, double-, or triple-click classification for this press.
    pub count: ClickCount,
    /// How this press relates to the caller's single-click action.
    pub single_click: SingleClickDisposition,
    /// Whether a previously deferred single click matured before this press.
    ///
    /// Dispatch it in addition to the disposition for the current press.
    pub matured_deferred_single_click: bool,
}

#[derive(Debug, Clone, Copy)]
struct PreviousPress {
    at: Instant,
    position: Point,
    count: ClickCount,
}

/// A stateful, renderer-independent multi-click detector.
#[derive(Debug, Clone)]
pub struct ClickDetector {
    config: ClickConfig,
    previous: Option<PreviousPress>,
    deferred_single_at: Option<Instant>,
}

impl ClickDetector {
    /// Creates a detector with shared timing defaults and an explicit single-click policy.
    #[must_use]
    pub fn new(single_click: SingleClickPolicy) -> Self {
        Self::with_config(ClickConfig::new(single_click))
    }

    /// Construct with an explicit complete policy.
    #[must_use]
    pub fn with_config(config: ClickConfig) -> Self {
        Self {
            config,
            previous: None,
            deferred_single_at: None,
        }
    }

    /// Return this detector's policy.
    #[must_use]
    pub const fn config(&self) -> ClickConfig {
        self.config
    }

    /// Classifies a press using its monotonic timestamp and position.
    ///
    /// A timestamp earlier than the preceding press starts a new sequence.
    #[must_use]
    pub fn press(&mut self, at: Instant, position: Point) -> ClickReport {
        let matured_deferred_single_click = self.deferred_has_matured(at);
        if matured_deferred_single_click {
            self.deferred_single_at = None;
        }

        let count = self
            .previous
            .filter(|previous| self.is_consecutive(*previous, at, position))
            .map_or(ClickCount::Single, |previous| previous.count.next());

        self.previous = Some(PreviousPress {
            at,
            position,
            count,
        });

        let single_click = match self.config.single_click {
            SingleClickPolicy::Immediate => SingleClickDisposition::DispatchNow,
            SingleClickPolicy::Deferred => match count {
                ClickCount::Single => {
                    self.deferred_single_at = Some(at);
                    SingleClickDisposition::Defer
                }
                ClickCount::Double => {
                    self.deferred_single_at = None;
                    SingleClickDisposition::CancelDeferred
                }
                ClickCount::Triple => SingleClickDisposition::None,
            },
        };

        ClickReport {
            count,
            single_click,
            matured_deferred_single_click,
        }
    }

    /// Resolves a matured deferred single click exactly once.
    ///
    /// A consecutive second press cancels it.
    #[must_use]
    pub fn take_deferred_single_click(&mut self, at: Instant) -> bool {
        if self.deferred_has_matured(at) {
            self.deferred_single_at = None;
            true
        } else {
            false
        }
    }

    fn is_consecutive(&self, previous: PreviousPress, at: Instant, position: Point) -> bool {
        let Some(elapsed) = at.checked_duration_since(previous.at) else {
            return false;
        };

        elapsed <= self.config.interval
            && previous.position.distance(position) < self.config.deadband
    }

    fn deferred_has_matured(&self, at: Instant) -> bool {
        self.deferred_single_at
            .and_then(|started| at.checked_duration_since(started))
            .is_some_and(|elapsed| elapsed > self.config.interval)
    }
}
