//! Keyboard and view navigation for calendar surfaces.
//!
//! The controller owns the focused date and visible month, not selection. Arrows may cross months, Home and End are week-relative, and Control-, Alt-, or Logo-modified keys remain unclaimed.

use crate::civil_date::{Date, DateBounds, MonthGrid, Weekday, YearMonth};
use iced_core::keyboard::{self, key};

/// A navigational move within a calendar grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CalendarMove {
    /// One day earlier (Left in an LTR grid).
    PrevDay,
    /// One day later (Right in an LTR grid).
    NextDay,
    /// One week earlier — seven days, i.e. one row up.
    PrevWeek,
    /// One week later — seven days, i.e. one row down.
    NextWeek,
    /// Moves to the configured first weekday of the focused week (Home).
    WeekStart,
    /// The last day of the focused week. End.
    WeekEnd,
    /// The same day-of-month one month earlier, clamped. PageUp.
    PrevMonth,
    /// The same day-of-month one month later, clamped. PageDown.
    NextMonth,
    /// The same date one year earlier, clamped. Shift+PageUp.
    PrevYear,
    /// The same date one year later, clamped. Shift+PageDown.
    NextYear,
}

impl CalendarMove {
    /// Returns whether this is a day or week step rather than a week-edge or page move.
    #[must_use]
    pub const fn is_within_grid(self) -> bool {
        matches!(
            self,
            Self::PrevDay | Self::NextDay | Self::PrevWeek | Self::NextWeek
        )
    }
}

/// Decodes a key press into a calendar move.
///
/// Returns `None` for unclaimed keys and Ctrl-, Alt-, or Logo-modified navigation. RTL mirrors only horizontal arrows; Shift remains available for year paging.
#[must_use]
pub fn move_for_key(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
    rtl: bool,
) -> Option<CalendarMove> {
    let keyboard::Key::Named(named) = key else {
        return None;
    };
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    let (back, forward) = if rtl {
        (CalendarMove::NextDay, CalendarMove::PrevDay)
    } else {
        (CalendarMove::PrevDay, CalendarMove::NextDay)
    };
    Some(match named {
        key::Named::ArrowLeft => back,
        key::Named::ArrowRight => forward,
        key::Named::ArrowUp => CalendarMove::PrevWeek,
        key::Named::ArrowDown => CalendarMove::NextWeek,
        key::Named::Home => CalendarMove::WeekStart,
        key::Named::End => CalendarMove::WeekEnd,
        key::Named::PageUp => {
            if modifiers.shift() {
                CalendarMove::PrevYear
            } else {
                CalendarMove::PrevMonth
            }
        }
        key::Named::PageDown => {
            if modifiers.shift() {
                CalendarMove::NextYear
            } else {
                CalendarMove::NextMonth
            }
        }
        _ => return None,
    })
}

/// Why a requested move did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveRefusal {
    /// The target is outside selectable bounds; focus remains unchanged.
    OutOfBounds,
}

/// Calendar focus and visible-month navigation without a retained grid or date cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonthController {
    focused: Date,
    visible: YearMonth,
    first_weekday: Weekday,
    bounds: DateBounds,
    picker_open: bool,
}

impl MonthController {
    /// Creates a controller with `focused` visible, clamping focus into `bounds`.
    #[must_use]
    pub fn new(focused: Date, first_weekday: Weekday, bounds: DateBounds) -> Self {
        let focused = bounds.clamp(focused);
        Self {
            focused,
            visible: focused.year_month(),
            first_weekday,
            bounds,
            picker_open: false,
        }
    }

    /// The roving-focus cursor.
    #[must_use]
    pub const fn focused(&self) -> Date {
        self.focused
    }

    /// The month currently on screen.
    #[must_use]
    pub const fn visible_month(&self) -> YearMonth {
        self.visible
    }

    /// The configured first day of the week.
    #[must_use]
    pub const fn first_weekday(&self) -> Weekday {
        self.first_weekday
    }

    /// The active selectable bounds.
    #[must_use]
    pub const fn bounds(&self) -> DateBounds {
        self.bounds
    }

    /// Build the grid for the visible month.
    #[must_use]
    pub fn grid(&self) -> MonthGrid {
        MonthGrid::build(self.visible, self.first_weekday, self.bounds)
    }

    /// Where the target of `movement` would land, ignoring bounds.
    #[must_use]
    pub fn target_of(&self, movement: CalendarMove) -> Date {
        match movement {
            CalendarMove::PrevDay => self.focused.add_days(-1),
            CalendarMove::NextDay => self.focused.add_days(1),
            CalendarMove::PrevWeek => self.focused.add_days(-7),
            CalendarMove::NextWeek => self.focused.add_days(7),
            CalendarMove::WeekStart => {
                let offset = self.focused.weekday().offset_from(self.first_weekday);
                self.focused.add_days(-(offset as i32))
            }
            CalendarMove::WeekEnd => {
                let offset = self.focused.weekday().offset_from(self.first_weekday);
                self.focused.add_days(6 - offset as i32)
            }
            CalendarMove::PrevMonth => self.focused.add_months(-1),
            CalendarMove::NextMonth => self.focused.add_months(1),
            CalendarMove::PrevYear => self.focused.add_years(-1),
            CalendarMove::NextYear => self.focused.add_years(1),
        }
    }

    /// Applies a movement and returns the new focus or [`MoveRefusal`].
    ///
    /// The visible month follows the resulting focused date.
    ///
    /// # Errors
    ///
    /// [`MoveRefusal::OutOfBounds`] when the movement leaves the selectable range.
    pub fn apply(&mut self, movement: CalendarMove) -> Result<Date, MoveRefusal> {
        let target = self.target_of(movement);
        if !self.bounds.contains(target) {
            return Err(MoveRefusal::OutOfBounds);
        }
        self.focus(target);
        Ok(target)
    }

    /// Moves focus to `date` and makes its month visible.
    ///
    /// Returns [`MoveRefusal::OutOfBounds`] without changing state when the date is not selectable.
    ///
    /// # Errors
    ///
    /// [`MoveRefusal::OutOfBounds`] when `date` is not selectable.
    pub fn focus_date(&mut self, date: Date) -> Result<Date, MoveRefusal> {
        if !self.bounds.contains(date) {
            return Err(MoveRefusal::OutOfBounds);
        }
        self.focus(date);
        Ok(date)
    }

    /// Pages the visible month without moving focus.
    ///
    /// Returns `false` when the target has no selectable day or saturation leaves the month unchanged.
    pub fn page_months(&mut self, months: i32) -> bool {
        if !self.can_page_months(months) {
            return false;
        }
        self.visible = self.visible.advance(months);
        true
    }

    /// Returns whether the target month contains a selectable day.
    ///
    /// A nonzero step that saturates to the current month returns `false`; zero tests the current month.
    #[must_use]
    pub fn can_page_months(&self, months: i32) -> bool {
        let target = self.visible.advance(months);
        // A nonzero step that saturates to the current month is not actionable.
        if months != 0 && target == self.visible {
            return false;
        }
        self.bounds.month_has_selectable(target)
    }

    /// Jumps the visible month to an absolute target without moving focus.
    ///
    /// Returns `false` if the target has no selectable day or is already visible.
    pub fn jump_to_month(&mut self, target: YearMonth) -> bool {
        if !self.can_jump_to_month(target) {
            return false;
        }
        self.visible = target;
        true
    }

    /// Returns whether `target` is selectable and differs from the visible month.
    ///
    /// Use [`Self::month_in_bounds`] for chooser availability because the visible month remains available.
    #[must_use]
    pub fn can_jump_to_month(&self, target: YearMonth) -> bool {
        if target == self.visible {
            return false;
        }
        self.bounds.month_has_selectable(target)
    }

    /// Returns whether `target` contains a selectable day.
    ///
    /// Unlike [`Self::can_jump_to_month`], this returns `true` for the visible month.
    #[must_use]
    pub fn month_in_bounds(&self, target: YearMonth) -> bool {
        self.bounds.month_has_selectable(target)
    }

    /// Resolves a year step to the nearest selectable month in the target year.
    ///
    /// The visible month is kept whenever it is selectable in the target year;
    /// an unrepresentable year or one with no selectable days returns `None`.
    /// For nonzero `delta`, a returned month is accepted by [`Self::jump_to_month`].
    #[must_use]
    pub fn year_step_target(&self, delta: i32) -> Option<YearMonth> {
        let year = self.visible.year().checked_add(delta)?;
        let shown = self.visible.month();
        (1..=12)
            .filter_map(|month| YearMonth::new(year, month))
            .filter(|candidate| self.bounds.month_has_selectable(*candidate))
            .min_by_key(|candidate| candidate.month().abs_diff(shown))
    }

    /// Whether the header's month/year chooser is open
    #[must_use]
    pub const fn picker_open(&self) -> bool {
        self.picker_open
    }

    /// Set the chooser's open state directly
    pub const fn set_picker_open(&mut self, open: bool) {
        self.picker_open = open;
    }

    /// Toggles the month/year chooser and returns its new open state.
    pub const fn toggle_picker(&mut self) -> bool {
        self.picker_open = !self.picker_open;
        self.picker_open
    }

    /// Makes the month containing the focused date visible.
    pub fn reveal_focused(&mut self) {
        self.visible = self.focused.year_month();
    }

    /// Whether the cursor is inside the visible month.
    #[must_use]
    pub fn focus_is_visible(&self) -> bool {
        self.focused.year_month() == self.visible
    }

    /// Replaces selectable bounds and clamps the focused date into them.
    pub fn set_bounds(&mut self, bounds: DateBounds) {
        self.bounds = bounds;
        let clamped = bounds.clamp(self.focused);
        if clamped != self.focused {
            self.focus(clamped);
        }
    }

    /// Changes the first weekday without moving the cursor; subsequent week-edge moves use the new value.
    pub const fn set_first_weekday(&mut self, first: Weekday) {
        self.first_weekday = first;
    }

    fn focus(&mut self, date: Date) {
        self.focused = date;
        self.visible = date.year_month();
    }
}
