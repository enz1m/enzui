//! Proleptic Gregorian civil dates, ranges, bounds, and fixed month grids.
//!
//! Day arithmetic uses an exact serial representation; constructors reject invalid civil dates, while selection bounds provide clamping policy.

use core::fmt;

/// Smallest year accepted by [`Date::new`] and [`YearMonth::new`].
///
/// Serial conversion remains exact beyond this constructor bound so adjacent month-grid cells do not collapse at the boundary.
pub const MIN_YEAR: i32 = -9999;

/// Largest year [`Date::new`] will accept. See [`MIN_YEAR`] for why this is a
/// constructor bound rather than a representational one.
pub const MAX_YEAR: i32 = 9999;

/// Number of cells in a fixed six-row, seven-column month grid.
///
/// The fixed height keeps month-grid geometry stable as months change.
pub const MONTH_CELLS: usize = 42;

/// Number of days in a week.
pub const DAYS_PER_WEEK: usize = 7;

/// A weekday with Monday-first ISO ordering.
///
/// Display order is configured independently through [`MonthGrid`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Weekday {
    /// Monday.
    Monday,
    /// Tuesday.
    Tuesday,
    /// Wednesday.
    Wednesday,
    /// Thursday.
    Thursday,
    /// Friday.
    Friday,
    /// Saturday.
    Saturday,
    /// Sunday.
    Sunday,
}

impl Weekday {
    /// All seven, Monday first.
    pub const ALL: [Self; DAYS_PER_WEEK] = [
        Self::Monday,
        Self::Tuesday,
        Self::Wednesday,
        Self::Thursday,
        Self::Friday,
        Self::Saturday,
        Self::Sunday,
    ];

    /// Monday-first index, `0..=6`.
    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Build from a Monday-first index, wrapping.
    #[must_use]
    pub const fn from_index(index: usize) -> Self {
        Self::ALL[index % DAYS_PER_WEEK]
    }

    /// Returns this weekday’s zero-based offset from a configurable first weekday.
    #[must_use]
    pub const fn offset_from(self, first: Weekday) -> usize {
        (self.index() + DAYS_PER_WEEK - first.index()) % DAYS_PER_WEEK
    }

    /// The next weekday, wrapping Sunday to Monday.
    #[must_use]
    pub const fn succ(self) -> Self {
        Self::from_index(self.index() + 1)
    }
}

/// A calendar month independent of any selected date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct YearMonth {
    year: i32,
    // Maintained in `1..=12` by constructors and internal arithmetic.
    month: u32,
}

impl YearMonth {
    /// Reject an out-of-range year or a month outside `1..=12`.
    #[must_use]
    pub const fn new(year: i32, month: u32) -> Option<Self> {
        if year < MIN_YEAR || year > MAX_YEAR || month < 1 || month > 12 {
            return None;
        }
        Some(Self { year, month })
    }

    /// The year.
    #[must_use]
    pub const fn year(self) -> i32 {
        self.year
    }

    /// The month, `1..=12`.
    #[must_use]
    pub const fn month(self) -> u32 {
        self.month
    }

    /// Reports whether `year` is a proleptic Gregorian leap year.
    #[must_use]
    pub const fn is_leap_year(year: i32) -> bool {
        (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
    }

    /// Whether this month's year is a leap year.
    #[must_use]
    pub const fn is_leap(self) -> bool {
        Self::is_leap_year(self.year)
    }

    /// How many days this month has, `28..=31`.
    #[must_use]
    pub const fn days(self) -> u32 {
        match self.month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            _ => {
                if self.is_leap() {
                    29
                } else {
                    28
                }
            }
        }
    }

    /// The first day of this month.
    #[must_use]
    pub const fn first_day(self) -> Date {
        Date {
            year: self.year,
            month: self.month,
            day: 1,
        }
    }

    /// The last day of this month.
    #[must_use]
    pub const fn last_day(self) -> Date {
        Date {
            year: self.year,
            month: self.month,
            day: self.days(),
        }
    }

    /// Advances by whole months, saturating at [`MIN_YEAR`] January and [`MAX_YEAR`] December.
    ///
    /// The combined year-month value is clamped before conversion, so overflow lands on the endpoint month rather than another month in the endpoint year.
    #[must_use]
    pub fn advance(self, months: i32) -> Self {
        let total = i64::from(self.year) * 12 + i64::from(self.month) - 1 + i64::from(months);
        // Clamp the combined month number before splitting it back into a year and month.
        const MIN_TOTAL: i64 = MIN_YEAR as i64 * 12;
        const MAX_TOTAL: i64 = MAX_YEAR as i64 * 12 + 11;
        let total = total.clamp(MIN_TOTAL, MAX_TOTAL);
        let year = total.div_euclid(12) as i32;
        let month = total.rem_euclid(12) as u32 + 1;
        Self { year, month }
    }

    /// The month containing `date`.
    #[must_use]
    pub const fn of(date: Date) -> Self {
        Self {
            year: date.year,
            month: date.month,
        }
    }

    /// Clamps a day number into this month, including leap-year February.
    #[must_use]
    pub const fn clamp_day(self, day: u32) -> Date {
        let days = self.days();
        let day = if day < 1 {
            1
        } else if day > days {
            days
        } else {
            day
        };
        Date {
            year: self.year,
            month: self.month,
            day,
        }
    }
}

impl fmt::Display for YearMonth {
    // Format `YearMonth` as ISO-8601 `YYYY-MM`, preserving four magnitude digits for negative years.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_iso_year(f, self.year)?;
        write!(f, "-{:02}", self.month)
    }
}

// Formats an ISO year with four magnitude digits in addition to any negative sign.
pub(crate) fn write_iso_year(f: &mut fmt::Formatter<'_>, year: i32) -> fmt::Result {
    if year < 0 {
        write!(f, "-{:04}", year.unsigned_abs())
    } else {
        write!(f, "{year:04}")
    }
}

/// A validated civil date in the proleptic Gregorian calendar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    year: i32,
    // Maintained in `1..=12` by validated construction and serial conversion.
    month: u32,
    // Maintained in `1..=year_month.days()` by validated construction and serial conversion.
    day: u32,
}

impl Date {
    /// Creates a date only when the year, month, and day form a valid civil date.
    #[must_use]
    pub const fn new(year: i32, month: u32, day: u32) -> Option<Self> {
        let Some(ym) = YearMonth::new(year, month) else {
            return None;
        };
        if day < 1 || day > ym.days() {
            return None;
        }
        Some(Self { year, month, day })
    }

    /// The year.
    #[must_use]
    pub const fn year(self) -> i32 {
        self.year
    }

    /// The month, `1..=12`.
    #[must_use]
    pub const fn month(self) -> u32 {
        self.month
    }

    /// The day of the month, `1..=31`.
    #[must_use]
    pub const fn day(self) -> u32 {
        self.day
    }

    /// The month this date falls in.
    #[must_use]
    pub const fn year_month(self) -> YearMonth {
        YearMonth::of(self)
    }

    /// Returns days since 1970-01-01 using exact proleptic Gregorian arithmetic.
    #[must_use]
    pub const fn to_days(self) -> i32 {
        let y = if self.month <= 2 {
            self.year - 1
        } else {
            self.year
        };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = (y - era * 400) as i64; // Year within a 400-year era.
        let m = self.month as i64;
        let d = self.day as i64;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // Day within the shifted year.
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // Day within the 400-year era.
        (era as i64 * 146_097 + doe - 719_468) as i32
    }

    /// Converts an `i32` day serial to a civil date without clamping to constructor year bounds.
    ///
    /// Exact, injective conversion is required for consecutive padding dates in endpoint month grids.
    #[must_use]
    pub const fn from_days(days: i32) -> Self {
        let z = days as i64 + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097; // Day within the 400-year era.
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // Year within the 400-year era.
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // Day within the shifted year.
        let mp = (5 * doy + 2) / 153; // 0..=11
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // 1..=31
        let m = if mp < 10 { mp + 3 } else { mp - 9 }; // 1..=12
        let y = if m <= 2 { y + 1 } else { y };
        Self {
            year: y as i32,
            month: m as u32,
            day: d,
        }
    }

    /// Moves by serial days, saturating at the `i32` serial endpoints.
    #[must_use]
    pub const fn add_days(self, days: i32) -> Self {
        Self::from_days(self.to_days().saturating_add(days))
    }

    /// Returns the signed serial-day distance from `self` to `other`; the difference must fit `i32`.
    #[must_use]
    pub const fn days_until(self, other: Self) -> i32 {
        other.to_days() - self.to_days()
    }

    /// Returns the weekday, using Thursday as the 1970-01-01 anchor and Euclidean reduction for pre-epoch dates.
    #[must_use]
    pub const fn weekday(self) -> Weekday {
        Weekday::from_index((self.to_days() + 3).rem_euclid(DAYS_PER_WEEK as i32) as usize)
    }

    /// Moves by whole months and clamps the day into the target month.
    #[must_use]
    pub fn add_months(self, months: i32) -> Self {
        self.year_month().advance(months).clamp_day(self.day)
    }

    /// Move by whole years, clamping Feb 29 onto Feb 28 in common years.
    #[must_use]
    pub fn add_years(self, years: i32) -> Self {
        self.add_months(years.saturating_mul(12))
    }
}

impl fmt::Display for Date {
    // Format as `YYYY-MM-DD`; negative years retain at least four magnitude digits.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_iso_year(f, self.year)?;
        write!(f, "-{:02}-{:02}", self.month, self.day)
    }
}

/// An inclusive range of dates with `start <= end` guaranteed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DateRange {
    start: Date,
    end: Date,
}

impl DateRange {
    /// Creates an inclusive range, returning `None` when `start > end`.
    #[must_use]
    pub fn new(start: Date, end: Date) -> Option<Self> {
        (start <= end).then_some(Self { start, end })
    }

    /// Creates an inclusive range after ordering its two endpoints.
    #[must_use]
    pub fn sorted(a: Date, b: Date) -> Self {
        if a <= b {
            Self { start: a, end: b }
        } else {
            Self { start: b, end: a }
        }
    }

    /// A range covering exactly one day.
    #[must_use]
    pub const fn single(day: Date) -> Self {
        Self {
            start: day,
            end: day,
        }
    }

    /// The first day.
    #[must_use]
    pub const fn start(self) -> Date {
        self.start
    }

    /// The last day (inclusive).
    #[must_use]
    pub const fn end(self) -> Date {
        self.end
    }

    /// Whether `date` falls inside, inclusive at both ends.
    #[must_use]
    pub fn contains(self, date: Date) -> bool {
        self.start <= date && date <= self.end
    }

    /// Returns the inclusive day count; the serial-day span must fit `i32`.
    #[must_use]
    pub const fn len_days(self) -> u32 {
        (self.end.days_until(self.start).unsigned_abs()) + 1
    }

    /// Where `date` sits relative to this range, for rendering.
    #[must_use]
    pub fn role(self, date: Date) -> RangeRole {
        if !self.contains(date) {
            return RangeRole::Outside;
        }
        match (date == self.start, date == self.end) {
            // A single-day range needs the distinct role so both outer corners are rounded.
            (true, true) => RangeRole::Single,
            (true, false) => RangeRole::Start,
            (false, true) => RangeRole::End,
            (false, false) => RangeRole::Middle,
        }
    }
}

/// A date’s position in a selected range for fill and corner styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RangeRole {
    /// Not in the range.
    Outside,
    /// The only day in a one-day range: both ends at once.
    Single,
    /// The first day.
    Start,
    /// Between the ends.
    Middle,
    /// The last day.
    End,
}

/// Independently optional inclusive lower and upper selectable dates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DateBounds {
    min: Option<Date>,
    max: Option<Date>,
}

impl DateBounds {
    /// Unbounded in both directions.
    #[must_use]
    pub const fn unbounded() -> Self {
        Self {
            min: None,
            max: None,
        }
    }

    /// Creates bounds unless both endpoints are present and the lower endpoint exceeds the upper one.
    #[must_use]
    pub fn new(min: Option<Date>, max: Option<Date>) -> Option<Self> {
        match (min, max) {
            (Some(lo), Some(hi)) if lo > hi => None,
            _ => Some(Self { min, max }),
        }
    }

    /// No date earlier than `min`.
    #[must_use]
    pub const fn at_least(min: Date) -> Self {
        Self {
            min: Some(min),
            max: None,
        }
    }

    /// No date later than `max`.
    #[must_use]
    pub const fn at_most(max: Date) -> Self {
        Self {
            min: None,
            max: Some(max),
        }
    }

    /// The lower bound, if any.
    #[must_use]
    pub const fn min(self) -> Option<Date> {
        self.min
    }

    /// The upper bound, if any.
    #[must_use]
    pub const fn max(self) -> Option<Date> {
        self.max
    }

    /// Whether `date` is selectable.
    #[must_use]
    pub fn contains(self, date: Date) -> bool {
        self.min.is_none_or(|lo| date >= lo) && self.max.is_none_or(|hi| date <= hi)
    }

    /// Clamp a date into the selectable window.
    #[must_use]
    pub fn clamp(self, date: Date) -> Date {
        let date = self.min.map_or(date, |lo| date.max(lo));
        self.max.map_or(date, |hi| date.min(hi))
    }

    /// Reports whether at least one day in `month` falls within the bounds.
    #[must_use]
    pub fn month_has_selectable(self, month: YearMonth) -> bool {
        self.min.is_none_or(|lo| month.last_day() >= lo)
            && self.max.is_none_or(|hi| month.first_day() <= hi)
    }
}

/// One cell of a month grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayCell {
    /// The date this cell shows. Always a real date, including for cells
    /// belonging to the neighbouring months.
    pub date: Date,
    /// Whether the date belongs to the month being displayed, as opposed to the
    /// leading/trailing days that pad the grid.
    pub in_month: bool,
    /// Whether the date is selectable under the active bounds.
    pub selectable: bool,
}

/// A fixed six-by-seven month grid whose padding cells contain real neighboring dates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonthGrid {
    month: YearMonth,
    first_weekday: Weekday,
    cells: [DayCell; MONTH_CELLS],
}

impl MonthGrid {
    /// Build the grid for `month`, with weeks starting on `first_weekday`.
    #[must_use]
    pub fn build(month: YearMonth, first_weekday: Weekday, bounds: DateBounds) -> Self {
        let first = month.first_day();
        // Leading padding is the first date’s offset from the configured week start.
        let lead = first.weekday().offset_from(first_weekday) as i32;
        let origin = first.to_days() - lead;
        let last_day = month.days();

        let cells = core::array::from_fn(|i| {
            let date = Date::from_days(origin + i as i32);
            let in_month = date.year() == month.year() && date.month() == month.month();
            DayCell {
                date,
                in_month,
                selectable: bounds.contains(date),
            }
        });
        debug_assert!(
            cells
                .iter()
                .filter(|c| c.in_month)
                .count()
                .eq(&(last_day as usize)),
            "every day of {month} must appear exactly once in its grid"
        );

        Self {
            month,
            first_weekday,
            cells,
        }
    }

    /// The month being displayed.
    #[must_use]
    pub const fn month(&self) -> YearMonth {
        self.month
    }

    /// The weekday each row starts on.
    #[must_use]
    pub const fn first_weekday(&self) -> Weekday {
        self.first_weekday
    }

    /// All 42 cells, row-major.
    #[must_use]
    pub const fn cells(&self) -> &[DayCell; MONTH_CELLS] {
        &self.cells
    }

    /// The column headers, left to right, honouring the configured first
    /// weekday.
    #[must_use]
    pub fn weekday_header(&self) -> [Weekday; DAYS_PER_WEEK] {
        let mut wd = self.first_weekday;
        core::array::from_fn(|_| {
            let out = wd;
            wd = wd.succ();
            out
        })
    }

    /// The cell holding `date`, if the grid shows it at all.
    #[must_use]
    pub fn position_of(&self, date: Date) -> Option<usize> {
        self.cells.iter().position(|c| c.date == date)
    }
}
