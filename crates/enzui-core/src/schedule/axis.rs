//! Minute-to-pixel transforms and time-band placement for schedules.

/// Maps a half-open visible minute interval onto a top-down pixel span.
///
/// The continuous transform is independent of ruler granularity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScheduleAxis {
    visible_start: i32,
    visible_end: i32,
    granularity_minutes: i32,
    height: f32,
}

impl ScheduleAxis {
    /// Builds an axis, returning `None` for a nonpositive interval or granularity, or invalid height.
    #[must_use]
    pub fn new(
        visible_start: i32,
        visible_end: i32,
        granularity_minutes: i32,
        height: f32,
    ) -> Option<Self> {
        if visible_end <= visible_start {
            return None;
        }
        if granularity_minutes <= 0 {
            return None;
        }
        if !(height.is_finite() && height > 0.0) {
            return None;
        }
        Some(Self {
            visible_start,
            visible_end,
            granularity_minutes,
            height,
        })
    }

    /// The first visible minute (maps to pixel `0`).
    #[must_use]
    pub const fn visible_start(&self) -> i32 {
        self.visible_start
    }

    /// The last visible minute, exclusive (maps to pixel `height`).
    #[must_use]
    pub const fn visible_end(&self) -> i32 {
        self.visible_end
    }

    /// The ruler step in minutes.
    #[must_use]
    pub const fn granularity_minutes(&self) -> i32 {
        self.granularity_minutes
    }

    /// The pixel span the interval maps onto.
    #[must_use]
    pub const fn height(&self) -> f32 {
        self.height
    }

    /// The visible interval length in minutes (always `> 0` by construction).
    #[must_use]
    pub const fn visible_minutes(&self) -> i32 {
        self.visible_end - self.visible_start
    }

    /// Pixels per minute — the transform's scale factor.
    #[must_use]
    pub fn pixels_per_minute(&self) -> f32 {
        self.height / self.visible_minutes() as f32
    }

    /// Maps a minute to an unclamped viewport-local pixel offset.
    #[must_use]
    pub fn y_of(&self, minute: i32) -> f32 {
        (minute - self.visible_start) as f32 * self.pixels_per_minute()
    }

    /// Maps a pixel offset to a fractional minute without snapping.
    #[must_use]
    pub fn minute_of(&self, y: f32) -> f32 {
        self.visible_start as f32 + y / self.pixels_per_minute()
    }

    /// Returns ascending `(minute, y)` rules beginning at the first aligned visible minute.
    #[must_use]
    pub fn rule_positions(&self) -> Vec<(i32, f32)> {
        let g = self.granularity_minutes;
        let first = {
            let rem = self.visible_start.rem_euclid(g);
            if rem == 0 {
                self.visible_start
            } else {
                self.visible_start + (g - rem)
            }
        };
        let mut out = Vec::new();
        let mut m = first;
        while m < self.visible_end {
            out.push((m, self.y_of(m)));
            m += g;
        }
        out
    }
}

/// A minute band with viewport-local pixel edges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BackgroundBand {
    /// The band's start minute (from the caller's origin).
    pub start_minute: i32,
    /// The band's end minute.
    pub end_minute: i32,
    /// Pixel offset of the band's top edge.
    pub top: f32,
    /// Pixel offset of the band's bottom edge.
    pub bottom: f32,
}

impl ScheduleAxis {
    /// Resolves visible off-hours intervals before and after a work interval.
    #[must_use]
    pub fn off_hours_bands(&self, work_start: i32, work_end: i32) -> Vec<BackgroundBand> {
        let mut out = Vec::new();
        let vs = self.visible_start;
        let ve = self.visible_end;
        let top_end = work_start.clamp(vs, ve);
        if top_end > vs {
            out.push(BackgroundBand {
                start_minute: vs,
                end_minute: top_end,
                top: self.y_of(vs),
                bottom: self.y_of(top_end),
            });
        }
        let bot_start = work_end.clamp(vs, ve);
        if bot_start < ve {
            out.push(BackgroundBand {
                start_minute: bot_start,
                end_minute: ve,
                top: self.y_of(bot_start),
                bottom: self.y_of(ve),
            });
        }
        out
    }

    /// Returns the current-time pixel offset inside the half-open visible interval.
    #[must_use]
    pub fn now_indicator_y(&self, now_minute: i32) -> Option<f32> {
        if now_minute >= self.visible_start && now_minute < self.visible_end {
            Some(self.y_of(now_minute))
        } else {
            None
        }
    }

    /// Resolves an injected current time only for its matching visible column.
    ///
    /// The caller owns clock, timezone, and daylight-saving policy.
    #[must_use]
    pub fn now_marker(&self, now: ScheduleNow, column: usize) -> Option<NowMarker> {
        if now.column != column {
            return None;
        }
        let y = self.now_indicator_y(now.minute)?;
        Some(NowMarker {
            minute: now.minute,
            column,
            y,
        })
    }

    /// Relabels primary rules by a signed minute offset without moving their pixels.
    #[must_use]
    pub fn secondary_rule_positions(&self, offset_minutes: i32) -> Vec<(i32, f32)> {
        self.rule_positions()
            .into_iter()
            .map(|(minute, y)| (minute + offset_minutes, y))
            .collect()
    }
}

/// Caller-supplied current minute and visible day column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduleNow {
    /// The current minute, in the same origin the events use.
    pub minute: i32,
    /// Which visible column "now" falls in (`0` for a single-day view).
    pub column: usize,
}

impl ScheduleNow {
    /// A now-value at `minute` in column `column`.
    #[must_use]
    pub const fn new(minute: i32, column: usize) -> Self {
        Self { minute, column }
    }
}

/// Resolved current-time marker with minute, column, and pixel offset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NowMarker {
    /// The minute the marker represents (echoed from the injected
    /// [`ScheduleNow`]).
    pub minute: i32,
    /// The column the marker is drawn in.
    pub column: usize,
    /// The marker's pixel offset from the top of the axis.
    pub y: f32,
}
