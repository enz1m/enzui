//! Locale-provided labels and accessibility semantics for calendar controls.
//!
//! Applications may override the English defaults without requiring a bundled locale database.

use crate::civil_date::{
    DAYS_PER_WEEK, Date, DateBounds, DateRange, DayCell, MonthGrid, RangeRole, Weekday, YearMonth,
};
use crate::semantics::{GridPosition, Role, Semantics};

/// Supplies localizable calendar labels and announcements.
///
/// Implementations provide full month and weekday names; the remaining methods have defaults and may be overridden independently.
pub trait CalendarLabels {
    /// Full month name, e.g. "March". `month` is `1..=12`.
    fn month_name(&self, month: u32) -> String;

    /// Returns the full weekday name used for announcements.
    fn weekday_name(&self, weekday: Weekday) -> String;

    /// Describes a calendar cell's selection, range or navigation status.
    ///
    /// Defaults use English wording; locale adapters override the full vocabulary.
    fn day_status(&self, status: CalendarDayStatus) -> String {
        crate::localization::english_text(status.message_key())
    }

    /// Returns the fallback accessible name for an unlabeled date-picker trigger.
    fn date_field_name(&self) -> String {
        String::from("Date")
    }

    /// Default accessible name for a date-range picker trigger.
    fn date_range_field_name(&self) -> String {
        String::from("Date range")
    }

    /// Returns a short weekday label, defaulting to the first three characters of the full name.
    fn weekday_short(&self, weekday: Weekday) -> String {
        self.weekday_name(weekday).chars().take(3).collect()
    }

    /// Supplies compact weekday headings when the abbreviated name cannot fit a cell.
    fn weekday_narrow(&self, weekday: Weekday) -> String {
        self.weekday_short(weekday)
    }

    /// Formats a day numeral; the default uses ASCII digits.
    fn day_number(&self, day: u32) -> String {
        day.to_string()
    }

    /// Formats a chooser year without grouping; the default uses ASCII digits.
    fn year_number(&self, year: i32) -> String {
        year.to_string()
    }

    /// Returns a short chooser month label, defaulting to the first three characters of the full name.
    fn month_short(&self, month: u32) -> String {
        self.month_name(month).chars().take(3).collect()
    }

    /// Formats a month header with locale-specific month/year ordering.
    fn month_year(&self, month: YearMonth) -> String {
        format!("{} {}", self.month_name(month.month()), month.year())
    }

    /// Formats a compact field value, defaulting to zero-padded ISO 8601.
    ///
    /// This is separate from the spoken [`Self::full_date`] form.
    fn display_date(&self, date: Date) -> String {
        date.to_string()
    }

    /// Formats a range from [`Self::display_date`].
    ///
    /// A single-day range renders as one date without a separator.
    fn display_range(&self, range: DateRange) -> String {
        if range.start() == range.end() {
            self.display_date(range.start())
        } else {
            format!(
                "{} {} {}",
                self.display_date(range.start()),
                self.range_separator(),
                self.display_date(range.end())
            )
        }
    }

    /// Returns the localized separator used by [`Self::display_range`].
    fn range_separator(&self) -> String {
        String::from("-")
    }

    /// Announces a range using full spoken dates.
    ///
    /// A single-day range explicitly says that it is one day.
    fn full_range(&self, range: DateRange) -> String {
        if range.start() == range.end() {
            format!("{}, one day", self.full_date(range.start()))
        } else {
            format!(
                "{} to {}",
                self.full_date(range.start()),
                self.full_date(range.end())
            )
        }
    }

    /// Announces a selected anchor and prompts for the end date without treating the anchor as a committed value.
    fn pending_range(&self, anchor: Date) -> String {
        format!("{} selected, choose an end date", self.full_date(anchor))
    }

    /// Announces host data for a day.
    ///
    /// Returns `None` for [`DayMarker::None`]; implementations may localize plural forms.
    fn day_marker(&self, marker: DayMarker) -> Option<String> {
        match marker {
            DayMarker::None => None,
            DayMarker::Present => Some(String::from("has events")),
            DayMarker::Count(1) => Some(String::from("1 event")),
            DayMarker::Count(n) => Some(format!("{n} events")),
        }
    }

    /// Formats a full date for spoken announcement.
    fn full_date(&self, date: Date) -> String {
        format!(
            "{}, {} {} {}",
            self.weekday_name(date.weekday()),
            date.day(),
            self.month_name(date.month()),
            date.year()
        )
    }
}

/// A status included in a calendar day's accessibility description.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarDayStatus {
    /// The day is selected.
    Selected,
    /// Both range endpoints fall on this day.
    SingleDayRange,
    /// The first day in the selected range.
    RangeStart,
    /// A day between the selected range endpoints.
    InRange,
    /// The last day in the selected range.
    RangeEnd,
    /// The day matches the caller's current date.
    Today,
    /// The day belongs to an adjacent month.
    AdjacentMonth,
}

impl CalendarDayStatus {
    pub(crate) fn message_key(self) -> &'static str {
        match self {
            Self::Selected => "enzui-date-selected",
            Self::SingleDayRange => "enzui-date-single-range",
            Self::RangeStart => "enzui-date-range-start",
            Self::InRange => "enzui-date-in-range",
            Self::RangeEnd => "enzui-date-range-end",
            Self::Today => "enzui-date-today",
            Self::AdjacentMonth => "enzui-date-adjacent-month",
        }
    }
}

/// Default English calendar labels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EnglishLabels;

impl CalendarLabels for EnglishLabels {
    fn month_name(&self, month: u32) -> String {
        const NAMES: [&str; 12] = [
            "January",
            "February",
            "March",
            "April",
            "May",
            "June",
            "July",
            "August",
            "September",
            "October",
            "November",
            "December",
        ];
        NAMES
            .get(month.wrapping_sub(1) as usize)
            .map_or_else(|| format!("Month {month}"), ToString::to_string)
    }

    fn weekday_name(&self, weekday: Weekday) -> String {
        match weekday {
            Weekday::Monday => "Monday",
            Weekday::Tuesday => "Tuesday",
            Weekday::Wednesday => "Wednesday",
            Weekday::Thursday => "Thursday",
            Weekday::Friday => "Friday",
            Weekday::Saturday => "Saturday",
            Weekday::Sunday => "Sunday",
        }
        .to_owned()
    }
}

/// Describes whether a day has host data and, optionally, its count.
///
/// [`Self::Present`] represents unknown quantity; [`Self::count`] normalizes zero to [`Self::None`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DayMarker {
    /// No host data to paint or announce.
    #[default]
    None,
    /// The day holds at least one item; the host does not know (or does not
    /// wish to publish) how many.
    Present,
    /// A known item count; use [`DayMarker::count`] when zero should normalize to [`DayMarker::None`].
    Count(u32),
}

impl DayMarker {
    /// Creates a marker from a count, mapping zero to [`DayMarker::None`].
    #[must_use]
    pub const fn count(n: u32) -> Self {
        if n == 0 { Self::None } else { Self::Count(n) }
    }

    /// Creates [`DayMarker::Present`] when `present` is true and [`DayMarker::None`] otherwise.
    #[must_use]
    pub const fn present(present: bool) -> Self {
        if present { Self::Present } else { Self::None }
    }

    /// Returns whether this marker represents any host data.
    #[must_use]
    pub const fn is_present(self) -> bool {
        !matches!(self, Self::None)
    }

    /// The count, when one is known.
    #[must_use]
    pub const fn known_count(self) -> Option<u32> {
        match self {
            Self::Count(n) => Some(n),
            Self::None | Self::Present => None,
        }
    }
}

/// Describes a day cell’s focus, selection, range, today, and host-data state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DayState {
    /// The cell holds the focus cursor.
    pub focused: bool,
    /// The cell is the selected date (single-date pickers).
    pub selected: bool,
    /// The cell's position in a selected range, if any.
    pub range: Option<RangeRole>,
    /// Whether the cell represents today as a decoration rather than a selection.
    pub today: bool,
    /// Host data advertised by the day, defaulting to [`DayMarker::None`].
    pub marker: DayMarker,
}

impl DayState {
    /// Whether anything about this cell is worth announcing beyond its date.
    #[must_use]
    pub fn is_marked(self) -> bool {
        self.selected
            || self.today
            || self.marker.is_present()
            || matches!(
                self.range,
                Some(RangeRole::Start | RangeRole::Middle | RangeRole::End | RangeRole::Single)
            )
    }

    /// Returns this state with an independent host-data marker attached.
    #[must_use]
    pub const fn with_marker(mut self, marker: DayMarker) -> Self {
        self.marker = marker;
        self
    }
}

/// Resolves day-cell state from focus, selection, range, today, and month membership.
///
/// Selection, range, and today are suppressed for adjacent-month padding cells.
#[must_use]
pub fn day_state(
    date: Date,
    focused: Option<Date>,
    selected: Option<Date>,
    range: Option<DateRange>,
    today: Option<Date>,
    in_month: bool,
) -> DayState {
    DayState {
        focused: focused == Some(date),
        selected: in_month && selected == Some(date),
        range: if in_month {
            range
                .map(|r| r.role(date))
                .filter(|r| *r != RangeRole::Outside)
        } else {
            None
        },
        today: in_month && today == Some(date),
        marker: DayMarker::None,
    }
}

/// Semantics for the grid container itself.
pub fn grid_semantics(month: YearMonth, labels: &dyn CalendarLabels) -> Semantics {
    Semantics::new(Role::Grid).label(labels.month_year(month))
}

/// Returns semantics for a weekday header, using the full weekday as its accessible name.
pub fn column_header_semantics(weekday: Weekday, labels: &dyn CalendarLabels) -> Semantics {
    Semantics::new(Role::ColumnHeader).label(labels.weekday_name(weekday))
}

/// Returns semantics for a day cell with the full date as its name and adornments in its description.
pub fn day_cell_semantics(
    cell: DayCell,
    state: DayState,
    position: GridPosition,
    labels: &dyn CalendarLabels,
) -> Semantics {
    let mut notes: Vec<String> = Vec::new();
    if state.selected {
        notes.push(labels.day_status(CalendarDayStatus::Selected));
    }
    match state.range {
        Some(RangeRole::Single) => notes.push(labels.day_status(CalendarDayStatus::SingleDayRange)),
        Some(RangeRole::Start) => notes.push(labels.day_status(CalendarDayStatus::RangeStart)),
        Some(RangeRole::Middle) => notes.push(labels.day_status(CalendarDayStatus::InRange)),
        Some(RangeRole::End) => notes.push(labels.day_status(CalendarDayStatus::RangeEnd)),
        Some(RangeRole::Outside) | None => {}
    }
    if state.today {
        notes.push(labels.day_status(CalendarDayStatus::Today));
    }
    // Include host data in semantics so the painted marker has an accessible equivalent.
    if let Some(note) = labels.day_marker(state.marker) {
        notes.push(note);
    }
    if !cell.in_month {
        // Announced because an unmarked cell from the adjacent month is
        // otherwise indistinguishable from one in the current month, and a
        // user who cannot see the dimming will page unexpectedly.
        notes.push(labels.day_status(CalendarDayStatus::AdjacentMonth));
    }

    let mut semantics = Semantics::new(Role::GridCell)
        .label(labels.full_date(cell.date))
        .grid_position(position)
        .disabled(!cell.selectable)
        .focused(state.focused)
        .selected(state.selected || state.range.is_some());

    if cell.selectable {
        semantics = semantics.default_action();
    }
    if !notes.is_empty() {
        semantics = semantics.description(notes.join(", "));
    }
    semantics
}

/// Returns semantics for a month-navigation arrow, naming its destination month.
pub fn month_nav_semantics(
    from: YearMonth,
    months: i32,
    bounds: DateBounds,
    labels: &dyn CalendarLabels,
) -> Semantics {
    let target = from.advance(months);
    Semantics::new(Role::Button)
        .label(labels.month_year(target))
        .default_action()
        .disabled(!bounds.month_has_selectable(target))
}

/// Returns semantics for a date-field trigger; an unset date publishes no value.
pub fn date_field_semantics(
    label: Option<&str>,
    value: Option<Date>,
    open: bool,
    enabled: bool,
    labels: &dyn CalendarLabels,
) -> Semantics {
    Semantics::new(Role::DateInput)
        .label(label.map_or_else(|| labels.date_field_name(), str::to_owned))
        .value_opt(value.map(|d| labels.full_date(d)))
        .expanded(open)
        .default_action()
        .disabled(!enabled)
}

/// Returns semantics for a date-range trigger.
///
/// Only a committed range becomes the value; a pending anchor becomes a description.
pub fn date_range_field_semantics(
    label: Option<&str>,
    value: Option<DateRange>,
    pending: Option<Date>,
    open: bool,
    enabled: bool,
    labels: &dyn CalendarLabels,
) -> Semantics {
    let semantics = Semantics::new(Role::DateInput)
        .label(label.map_or_else(|| labels.date_range_field_name(), str::to_owned))
        .value_opt(value.map(|r| labels.full_range(r)))
        .expanded(open)
        .default_action()
        .disabled(!enabled);

    match pending {
        Some(anchor) => semantics.description(labels.pending_range(anchor)),
        None => semantics,
    }
}

/// The grid position of cell `index` in a [`MonthGrid`].
#[must_use]
pub fn position_of_index(index: usize) -> GridPosition {
    GridPosition {
        row: index / DAYS_PER_WEEK,
        column: index % DAYS_PER_WEEK,
        row_count: crate::civil_date::MONTH_CELLS / DAYS_PER_WEEK,
        column_count: DAYS_PER_WEEK,
    }
}

/// Returns semantics for every grid cell, including its grid position.
#[must_use]
pub fn grid_cell_semantics(
    grid: &MonthGrid,
    focused: Option<Date>,
    selected: Option<Date>,
    range: Option<DateRange>,
    today: Option<Date>,
    labels: &dyn CalendarLabels,
) -> Vec<Semantics> {
    grid_cell_semantics_marked(grid, focused, selected, range, today, labels, |_| {
        DayMarker::None
    })
}

/// Returns semantics for every grid cell, adding announcements from `marker`.
#[must_use]
pub fn grid_cell_semantics_marked(
    grid: &MonthGrid,
    focused: Option<Date>,
    selected: Option<Date>,
    range: Option<DateRange>,
    today: Option<Date>,
    labels: &dyn CalendarLabels,
    marker: impl Fn(Date) -> DayMarker,
) -> Vec<Semantics> {
    grid.cells()
        .iter()
        .enumerate()
        .map(|(index, cell)| {
            let state = day_state(cell.date, focused, selected, range, today, cell.in_month)
                .with_marker(marker(cell.date));
            day_cell_semantics(*cell, state, position_of_index(index), labels)
        })
        .collect()
}
