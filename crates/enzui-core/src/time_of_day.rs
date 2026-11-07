//! Renderer-neutral time-of-day formatting and parsing for the four Unicode
//! hour cycles.
//!
//! Callers choose the hour cycle and may replace the default English day-period labels; this module performs no locale or platform lookup.
//!
//! [`TimeFormat::parse`] supplies a shared grammar for typed text and
//! assistive-technology `SetValue` payloads. It rejects configurations flagged
//! by [`TimeFormat::is_reversible`]; arbitrary custom labels can still make
//! formatted output ambiguous or unreadable.

use core::fmt::Write as _;

/// The number of seconds in a day. A time of day is `0..SECONDS_PER_DAY`.
pub const SECONDS_PER_DAY: u32 = 24 * 60 * 60;

/// Which morning/afternoon half a 12-hour-style time falls in.
///
/// Only [`HourCycle::H11`] and [`HourCycle::H12`] produce one; the 24-hour
/// cycles return `None` from [`HourCycle::resolve`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DayPeriod {
    /// Before noon (`00:00..12:00`).
    Am,
    /// Noon and after (`12:00..24:00`).
    Pm,
}

/// Caller-supplied labels for the two day periods.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayPeriodLabels {
    am: String,
    pm: String,
}

impl DayPeriodLabels {
    /// The English `AM` / `PM`.
    #[must_use]
    pub fn english() -> Self {
        Self {
            am: String::from("AM"),
            pm: String::from("PM"),
        }
    }

    /// Custom labels, e.g. lower-case `a.m.`/`p.m.` or a localized pair.
    #[must_use]
    pub fn new(am: impl Into<String>, pm: impl Into<String>) -> Self {
        Self {
            am: am.into(),
            pm: pm.into(),
        }
    }

    /// The label for a period.
    #[must_use]
    pub fn label(&self, period: DayPeriod) -> &str {
        match period {
            DayPeriod::Am => &self.am,
            DayPeriod::Pm => &self.pm,
        }
    }
}

impl Default for DayPeriodLabels {
    fn default() -> Self {
        Self::english()
    }
}

/// A Unicode hour cycle selected by locale or user preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HourCycle {
    /// Twelve-hour cycle using `1..=12`; midnight is 12 AM and noon is 12 PM.
    H12,
    /// Twelve-hour cycle using `0..=11`; midnight is 0 AM and noon is 0 PM.
    H11,
    /// Twenty-four-hour cycle using `0..=23`, with midnight rendered as 00.
    H23,
    /// Twenty-four-hour cycle using `1..=24`; midnight and its hour render with 24.
    H24,
}

impl HourCycle {
    /// Whether this cycle renders a day period (AM/PM).
    #[must_use]
    pub const fn has_day_period(self) -> bool {
        matches!(self, Self::H11 | Self::H12)
    }

    /// Resolves a 24-hour clock hour to its displayed hour and optional day period.
    ///
    /// Reduces `hour24` modulo 24. H12 maps midnight and noon to 12; H11 maps
    /// them to 0; H23 and H24 emit no period.
    #[must_use]
    pub const fn resolve(self, hour24: u32) -> (u32, Option<DayPeriod>) {
        let hour24 = hour24 % 24;
        let period = if hour24 < 12 {
            DayPeriod::Am
        } else {
            DayPeriod::Pm
        };
        match self {
            Self::H23 => (hour24, None),
            Self::H24 => (if hour24 == 0 { 24 } else { hour24 }, None),
            Self::H11 => (hour24 % 12, Some(period)),
            Self::H12 => (
                if hour24.is_multiple_of(12) {
                    12
                } else {
                    hour24 % 12
                },
                Some(period),
            ),
        }
    }
}

/// Controls leading-zero padding for the hour field; minutes and seconds are always two digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HourDigits {
    /// Two digits, zero-padded: `09`.
    Padded,
    /// As many digits as needed, no leading zero: `9`.
    AsNeeded,
}

/// How much of the time to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Precision {
    /// Hour and minute: `13:45`.
    HourMinute,
    /// Hour, minute and second: `13:45:07`.
    HourMinuteSecond,
}

/// A valid time represented as seconds since midnight.
///
/// Constructors reject out-of-range components rather than wrapping them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeOfDay {
    secs: u32,
}

impl TimeOfDay {
    /// Midnight, `00:00:00`.
    pub const MIDNIGHT: Self = Self { secs: 0 };
    /// Noon, `12:00:00`.
    pub const NOON: Self = Self { secs: 12 * 60 * 60 };

    /// Reject any time that is not a real clock time: `hour 0..=23`,
    /// `minute 0..=59`, `second 0..=59`.
    #[must_use]
    pub const fn new(hour: u32, minute: u32, second: u32) -> Option<Self> {
        if hour > 23 || minute > 59 || second > 59 {
            return None;
        }
        Some(Self {
            secs: hour * 3600 + minute * 60 + second,
        })
    }

    /// An `(hour, minute)` time at the top of the second.
    #[must_use]
    pub const fn hm(hour: u32, minute: u32) -> Option<Self> {
        Self::new(hour, minute, 0)
    }

    /// Build from seconds since midnight, rejecting `>= SECONDS_PER_DAY`.
    #[must_use]
    pub const fn from_seconds_since_midnight(secs: u32) -> Option<Self> {
        if secs >= SECONDS_PER_DAY {
            return None;
        }
        Some(Self { secs })
    }

    /// Seconds since midnight, `0..SECONDS_PER_DAY`.
    #[must_use]
    pub const fn seconds_since_midnight(self) -> u32 {
        self.secs
    }

    /// The 24-hour clock hour, `0..=23`.
    #[must_use]
    pub const fn hour(self) -> u32 {
        self.secs / 3600
    }

    /// The minute within the hour, `0..=59`.
    #[must_use]
    pub const fn minute(self) -> u32 {
        (self.secs / 60) % 60
    }

    /// The second within the minute, `0..=59`.
    #[must_use]
    pub const fn second(self) -> u32 {
        self.secs % 60
    }
}

/// Configures pure formatting of a [`TimeOfDay`] for an [`HourCycle`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeFormat {
    cycle: HourCycle,
    precision: Precision,
    hour_digits: HourDigits,
    separator: char,
    period_labels: DayPeriodLabels,
    period_space: bool,
}

impl TimeFormat {
    /// A formatter for `cycle` with conventional defaults: hour+minute
    /// precision, `:` separator, English AM/PM labels with a leading space, and
    /// the leading-zero policy each cycle conventionally uses — the 12-hour
    /// cycles unpadded (`9:00 AM`), the 24-hour cycles padded (`09:00`).
    #[must_use]
    pub fn new(cycle: HourCycle) -> Self {
        let hour_digits = if cycle.has_day_period() {
            HourDigits::AsNeeded
        } else {
            HourDigits::Padded
        };
        Self {
            cycle,
            precision: Precision::HourMinute,
            hour_digits,
            separator: ':',
            period_labels: DayPeriodLabels::english(),
            period_space: true,
        }
    }

    /// Set the precision (hour+minute vs hour+minute+second).
    #[must_use]
    pub fn with_precision(mut self, precision: Precision) -> Self {
        self.precision = precision;
        self
    }

    /// Set the hour-field leading-zero policy.
    #[must_use]
    pub fn with_hour_digits(mut self, hour_digits: HourDigits) -> Self {
        self.hour_digits = hour_digits;
        self
    }

    /// Set the field separator (default `:`).
    #[must_use]
    pub fn with_separator(mut self, separator: char) -> Self {
        self.separator = separator;
        self
    }

    /// Sets the day-period labels.
    #[must_use]
    pub fn with_day_period_labels(mut self, labels: DayPeriodLabels) -> Self {
        self.period_labels = labels;
        self
    }

    /// Sets whether a space precedes the day period; ignored for cycles without a day period.
    #[must_use]
    pub fn with_period_space(mut self, space: bool) -> Self {
        self.period_space = space;
        self
    }

    /// The configured hour cycle.
    #[must_use]
    pub const fn cycle(&self) -> HourCycle {
        self.cycle
    }

    /// Formats `time` using this configuration.
    #[must_use]
    pub fn format(&self, time: TimeOfDay) -> String {
        let (hour, period) = self.cycle.resolve(time.hour());
        let mut out = String::new();
        match self.hour_digits {
            HourDigits::Padded => {
                let _ = write!(out, "{hour:02}");
            }
            HourDigits::AsNeeded => {
                let _ = write!(out, "{hour}");
            }
        }
        let _ = write!(out, "{}{:02}", self.separator, time.minute());
        if self.precision == Precision::HourMinuteSecond {
            let _ = write!(out, "{}{:02}", self.separator, time.second());
        }
        if let Some(period) = period {
            if self.period_space {
                out.push(' ');
            }
            out.push_str(self.period_labels.label(period));
        }
        out
    }

    /// Whether the configuration passes the parser's ambiguity checks.
    ///
    /// Rejects ASCII-digit separators and, for day-period cycles, labels that
    /// are blank or equal after trimming and Unicode lowercasing. This is not
    /// a complete round-trip guarantee for arbitrary labels: surrounding label
    /// whitespace or labels that resemble numeric fields can prevent parsing
    /// or change its result. Labels are also recognized under 24-hour cycles,
    /// where these label checks do not apply.
    ///
    /// [`Self::parse`] returns [`TimeParseError::UnsupportedFormat`] when this
    /// check fails.
    #[must_use]
    pub fn is_reversible(&self) -> bool {
        if self.separator.is_ascii_digit() {
            return false;
        }
        if self.cycle.has_day_period() {
            let am = self.period_labels.label(DayPeriod::Am).trim();
            let pm = self.period_labels.label(DayPeriod::Pm).trim();
            if am.is_empty() || pm.is_empty() || am.to_lowercase() == pm.to_lowercase() {
                return false;
            }
        }
        true
    }

    /// Reads a time back out of text written for this configuration.
    ///
    /// Minute precision omits seconds; second precision preserves them. Custom
    /// labels must remain distinguishable from the numeric body after input
    /// trimming. The parser accepts more spellings than the formatter emits:
    ///
    /// * `:` is always a separator, in addition to [`Self::with_separator`].
    /// * An unseparated run of digits is read positionally: 1–2 digits are an
    ///   hour, 3–4 are `h mm`, 5–6 are `h mm ss`. So `930`, `0930` and `9:30`
    ///   agree, and `9` is 9 o'clock.
    /// * A day-period label may be attached with or without a space, in any
    ///   case, and may be abbreviated to its first character when the two
    ///   labels do not share one. The longer label is matched first, so `M`
    ///   beside `PM` reads the way it was written.
    /// * A label is honoured under the 24-hour cycles too: someone who types
    ///   `2 PM` into a `14:00` field meant 14:00.
    /// * Hour `24` reads as midnight under every cycle, not only the
    ///   [`HourCycle::H24`] that emits it. `24:30` is `00:30`; rejecting the
    ///   whole spelling would make H24's own midnight unreadable.
    ///
    /// Parsing does not quantize minutes or seconds to a control's step size.
    ///
    /// # Errors
    ///
    /// [`TimeParseError::UnsupportedFormat`] when [`Self::is_reversible`] is
    /// false — checked before the text, because no text can be read under such
    /// a configuration. [`TimeParseError::Empty`] for blank input,
    /// [`TimeParseError::Malformed`] for text that is not shaped like a time
    /// (including a seconds field when [`Precision::HourMinute`] is configured,
    /// which would display back as something other than what was typed), and
    /// [`TimeParseError::OutOfRange`] for a time-shaped string whose hour,
    /// minute or second cannot exist.
    pub fn parse(&self, text: &str) -> Result<TimeOfDay, TimeParseError> {
        if !self.is_reversible() {
            return Err(TimeParseError::UnsupportedFormat);
        }
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(TimeParseError::Empty);
        }

        let (body, period) = self.split_day_period(trimmed);
        let body = body.trim();
        if body.is_empty() {
            return Err(TimeParseError::Malformed);
        }

        let (hour, minute, second) = self.split_fields(body)?;
        let hour = resolve_typed_hour(hour, period).ok_or(TimeParseError::OutOfRange)?;
        TimeOfDay::new(hour, minute, second).ok_or(TimeParseError::OutOfRange)
    }

    fn split_day_period<'t>(&self, text: &'t str) -> (&'t str, Option<DayPeriod>) {
        let am = self.period_labels.label(DayPeriod::Am);
        let pm = self.period_labels.label(DayPeriod::Pm);
        // Match longer labels first so a suffix label cannot steal another period.
        let mut labels = [(pm, DayPeriod::Pm), (am, DayPeriod::Am)];
        labels.sort_by_key(|(label, _)| core::cmp::Reverse(label.chars().count()));
        for (label, period) in labels {
            if let Some(head) = strip_suffix_ignoring_case(text, label) {
                return (head, Some(period));
            }
        }
        let (am_initial, pm_initial) = (am.chars().next(), pm.chars().next());
        if am_initial.is_some() && am_initial != pm_initial {
            for (initial, period) in [(pm_initial, DayPeriod::Pm), (am_initial, DayPeriod::Am)] {
                let Some(initial) = initial else { continue };
                let mut buffer = [0u8; 4];
                if let Some(head) =
                    strip_suffix_ignoring_case(text, initial.encode_utf8(&mut buffer))
                {
                    return (head, Some(period));
                }
            }
        }
        (text, None)
    }

    fn split_fields(&self, body: &str) -> Result<(u32, u32, u32), TimeParseError> {
        let seconds_allowed = self.precision == Precision::HourMinuteSecond;
        let separator = self.separator;
        let is_separator = |c: char| c == ':' || (c == separator && !c.is_ascii_digit());

        if !body.contains(is_separator) {
            if !body.chars().all(|c| c.is_ascii_digit()) {
                return Err(TimeParseError::Malformed);
            }
            let digits = body.len();
            let (hour, rest) = match digits {
                1..=2 => return Ok((field(body)?, 0, 0)),
                3..=4 => body.split_at(digits - 2),
                5..=6 if seconds_allowed => body.split_at(digits - 4),
                _ => return Err(TimeParseError::Malformed),
            };
            let (minute, second) = rest.split_at(2);
            return Ok((field(hour)?, field(minute)?, field(second).unwrap_or(0)));
        }

        let mut fields = body.split(is_separator);
        let hour = field(fields.next().unwrap_or_default())?;
        let minute = field(fields.next().unwrap_or_default())?;
        let second = match fields.next() {
            Some(_) if !seconds_allowed => return Err(TimeParseError::Malformed),
            Some(text) => field(text)?,
            None => 0,
        };
        if fields.next().is_some() {
            return Err(TimeParseError::Malformed);
        }
        Ok((hour, minute, second))
    }
}

/// Why [`TimeFormat::parse`] could not read a time.
///
/// Unsupported configurations are reported before input errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeParseError {
    /// The text was empty or entirely whitespace.
    Empty,
    /// The text is not shaped like a time under this configuration.
    Malformed,
    /// The text is shaped like a time, but names one that does not exist.
    OutOfRange,
    /// The [`TimeFormat`] itself cannot be read back — see
    /// [`TimeFormat::is_reversible`]. Independent of `text`.
    UnsupportedFormat,
}

fn field(text: &str) -> Result<u32, TimeParseError> {
    if text.is_empty() || text.len() > 2 || !text.chars().all(|c| c.is_ascii_digit()) {
        return Err(TimeParseError::Malformed);
    }
    text.parse().map_err(|_| TimeParseError::Malformed)
}

fn resolve_typed_hour(hour: u32, period: Option<DayPeriod>) -> Option<u32> {
    match period {
        Some(period) => {
            let base = match hour {
                0 | 12 => 0,
                1..=11 => hour,
                _ => return None,
            };
            Some(base + if period == DayPeriod::Pm { 12 } else { 0 })
        }
        None => match hour {
            0..=23 => Some(hour),
            24 => Some(0),
            _ => None,
        },
    }
}

// Empty labels must not match every numeric body.
fn strip_suffix_ignoring_case<'a>(text: &'a str, suffix: &str) -> Option<&'a str> {
    let width = suffix.chars().count();
    if width == 0 {
        return None;
    }
    let start = text.char_indices().rev().nth(width - 1).map(|(at, _)| at)?;
    let (head, tail) = text.split_at(start);
    (tail.to_lowercase() == suffix.to_lowercase()).then_some(head)
}
