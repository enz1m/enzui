use super::{Arguments, Locale, LocalizationError, Localizer};
use crate::calendar_semantics::{CalendarDayStatus, CalendarLabels, DayMarker};
use crate::civil_date::{Date, DateRange, Weekday, YearMonth};
use crate::time_of_day::TimeOfDay;
use fluent_bundle::FluentValue;
use fluent_bundle::types::{FluentNumberOptions, FluentNumberStyle};
use icu_calendar::Gregorian;
use icu_datetime::{FixedCalendarDateTimeFormatter, NoCalendarFormatter, fieldsets};
use icu_decimal::DecimalFormatter;
use icu_decimal::options::{DecimalFormatterOptions, GroupingStrategy};
use intl_memoizer::{Memoizable, concurrent::IntlLangMemoizer};
use writeable::{TryWriteable, Writeable};

// Fluent renders a function's Error value without adding a resolver diagnostic.
thread_local! {
    static NUMBER_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct NumberFailureScope(bool);

impl Drop for NumberFailureScope {
    fn drop(&mut self) {
        NUMBER_FAILURE.with(|failure| failure.set(self.0));
    }
}

/// How many locale formatters of one kind a thread keeps: a table or a file
/// list formats many cells in one or two locales.
const FORMATTERS_KEPT: usize = 8;

type Kept<K, V> = std::cell::RefCell<Vec<(K, std::rc::Rc<V>)>>;

thread_local! {
    static DECIMALS: Kept<Locale, DecimalFormatter> = const { std::cell::RefCell::new(Vec::new()) };
    static DATES: Kept<(Locale, DateStyle), FixedCalendarDateTimeFormatter<Gregorian, fieldsets::YMD>> =
        const { std::cell::RefCell::new(Vec::new()) };
    static FULL_DATES: Kept<Locale, FixedCalendarDateTimeFormatter<Gregorian, fieldsets::YMDE>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The formatter for `key` from this thread's few most recent, made by `make`
/// when it is not among them. Constructing ICU formatters is the costly part
/// of formatting a value.
fn kept<K: PartialEq, V, E>(
    cache: &'static std::thread::LocalKey<Kept<K, V>>,
    key: K,
    make: impl FnOnce() -> Result<V, E>,
) -> Result<std::rc::Rc<V>, E> {
    cache.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(at) = cache.iter().position(|(kept, _)| *kept == key) {
            let entry = cache.remove(at);
            let formatter = std::rc::Rc::clone(&entry.1);
            cache.insert(0, entry);
            return Ok(formatter);
        }
        let formatter = std::rc::Rc::new(make()?);
        cache.insert(0, (key, std::rc::Rc::clone(&formatter)));
        cache.truncate(FORMATTERS_KEPT);
        Ok(formatter)
    })
}

pub(super) fn capture_number_failure<R>(format: impl FnOnce() -> R) -> (R, bool) {
    let _scope = NumberFailureScope(NUMBER_FAILURE.with(|failure| failure.replace(false)));
    let result = format();
    (result, NUMBER_FAILURE.with(std::cell::Cell::get))
}

fn invalid_number() -> FluentValue<'static> {
    NUMBER_FAILURE.with(|failure| failure.set(true));
    FluentValue::Error
}

/// An exact decimal value for locale-aware number display, independent of binary floats.
pub use fixed_decimal::Decimal;

/// The length and vocabulary of a formatted Gregorian date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateStyle {
    /// Locale-ordered numeric date, potentially using a two-digit year.
    Short,
    /// Abbreviated month with day and year.
    Medium,
    /// Full month with day and year.
    Long,
    /// Full weekday, month, day and year.
    Full,
}

fn format_error(error: impl std::fmt::Display) -> LocalizationError {
    LocalizationError::Format(error.to_string())
}

fn icu_date(date: Date) -> Result<icu_calendar::Date<Gregorian>, LocalizationError> {
    icu_calendar::Date::try_new_gregorian(date.year(), date.month() as u8, date.day() as u8)
        .map_err(format_error)
}

impl Locale {
    /// Resolves the locale's hour cycle, including a Unicode `u-hc` override.
    ///
    /// # Errors
    /// Returns an error if CLDR clock preferences cannot be resolved.
    pub fn hour_cycle(&self) -> Result<crate::time_of_day::HourCycle, LocalizationError> {
        self.hour_cycle_with(crate::desktop_settings::clock_24h())
    }

    /// The locale with the desktop's clock format as its `u-hc` keyword,
    /// unless the locale names an hour cycle itself.
    fn clock_locale(&self, desktop_24h: Option<bool>) -> icu_locale::Locale {
        use icu_locale::extensions::unicode::{key, value};
        let mut locale = self.0.clone();
        if locale
            .extensions
            .unicode
            .keywords
            .get(&key!("hc"))
            .is_none()
            && let Some(h24) = desktop_24h
        {
            let cycle = if h24 { value!("h23") } else { value!("h12") };
            let _ = locale.extensions.unicode.keywords.set(key!("hc"), cycle);
        }
        locale
    }

    /// [`Self::hour_cycle`] with the desktop's clock format given: an
    /// explicit `u-hc` in the locale, then the desktop, then CLDR.
    pub(crate) fn hour_cycle_with(
        &self,
        desktop_24h: Option<bool>,
    ) -> Result<crate::time_of_day::HourCycle, LocalizationError> {
        use crate::time_of_day::HourCycle;
        let clock = self.clock_locale(desktop_24h);
        if let Some(cycle) = clock
            .extensions
            .unicode
            .keywords
            .get(&icu_locale::extensions::unicode::key!("hc"))
        {
            match cycle.to_string().as_str() {
                "h11" => return Ok(HourCycle::H11),
                "h12" => return Ok(HourCycle::H12),
                "h23" => return Ok(HourCycle::H23),
                "h24" => return Ok(HourCycle::H24),
                _ => {}
            }
        }
        let formatter = NoCalendarFormatter::try_new(clock.into(), fieldsets::T::short())
            .map_err(format_error)?;
        let time = icu_datetime::input::Time::start_of_day();
        let pattern = formatter
            .format(&time)
            .pattern()
            .write_to_string()
            .into_owned();
        let mut quoted = false;
        for symbol in pattern.chars() {
            if symbol == '\'' {
                quoted = !quoted;
            } else if !quoted {
                match symbol {
                    'K' => return Ok(HourCycle::H11),
                    'h' => return Ok(HourCycle::H12),
                    'H' => return Ok(HourCycle::H23),
                    'k' => return Ok(HourCycle::H24),
                    _ => {}
                }
            }
        }
        Err(LocalizationError::Format(
            "the resolved UTS 35 time pattern has no hour field".to_owned(),
        ))
    }

    /// Formats a Gregorian month and day without a year, using abbreviated month names.
    ///
    /// # Errors
    /// Returns an error for a date outside ICU's range or unavailable locale data.
    pub fn format_month_day(&self, date: Date) -> Result<String, LocalizationError> {
        let date = icu_date(date)?;
        let formatter = FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new(
            self.0.clone().into(),
            fieldsets::MD::medium(),
        )
        .map_err(format_error)?;
        Ok(formatter.format(&date).write_to_string().into_owned())
    }

    /// Formats exact decimal digits with the locale's numbering system and grouping.
    ///
    /// # Errors
    /// Returns an error if CLDR data cannot resolve the formatting preferences.
    pub fn format_decimal(&self, value: &Decimal) -> Result<String, LocalizationError> {
        let formatter = kept(&DECIMALS, self.clone(), || {
            DecimalFormatter::try_new(self.0.clone().into(), Default::default())
                .map_err(format_error)
        })?;
        Ok(formatter.format(value).write_to_string().into_owned())
    }

    /// Formats a Gregorian civil date without converting to a different calendar system.
    ///
    /// # Errors
    /// Returns an error for a date outside ICU's range or unavailable locale data.
    pub fn format_date(&self, date: Date, style: DateStyle) -> Result<String, LocalizationError> {
        let date = icu_date(date)?;
        if style == DateStyle::Full {
            let formatter = kept(&FULL_DATES, self.clone(), || {
                FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new(
                    self.0.clone().into(),
                    fieldsets::YMDE::long(),
                )
                .map_err(format_error)
            })?;
            return Ok(formatter.format(&date).write_to_string().into_owned());
        }
        let fields = match style {
            DateStyle::Short => fieldsets::YMD::short(),
            DateStyle::Medium => fieldsets::YMD::medium(),
            DateStyle::Long | DateStyle::Full => fieldsets::YMD::long(),
        };
        let formatter = kept(&DATES, (self.clone(), style), || {
            FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new(self.0.clone().into(), fields)
                .map_err(format_error)
        })?;
        Ok(formatter.format(&date).write_to_string().into_owned())
    }

    /// Formats local wall-clock time using CLDR hour-cycle, separator and day-period rules.
    ///
    /// Unicode `u-hc` and `u-nu` preferences are honored. This is display formatting;
    /// [`crate::time_of_day::TimeFormat`] remains the explicit editable-field grammar.
    ///
    /// # Errors
    /// Returns an error if the time or formatting preferences cannot be resolved.
    pub fn format_time(&self, time: TimeOfDay, seconds: bool) -> Result<String, LocalizationError> {
        if self
            .0
            .extensions
            .unicode
            .keywords
            .get(&icu_locale::extensions::unicode::key!("hc"))
            .is_some_and(|value| value.to_string() == "h24")
        {
            return Err(LocalizationError::Format("CLDR display formatting does not support H24; use TimeFormat for that explicit grammar".to_owned()));
        }
        let fields = fieldsets::T::short().with_time_precision(if seconds {
            icu_datetime::options::TimePrecision::Second
        } else {
            icu_datetime::options::TimePrecision::Minute
        });
        let formatter = NoCalendarFormatter::try_new(
            self.clock_locale(crate::desktop_settings::clock_24h())
                .into(),
            fields,
        )
        .map_err(format_error)?;
        let time = icu_datetime::input::Time::try_new(
            time.hour() as u8,
            time.minute() as u8,
            time.second() as u8,
            0,
        )
        .map_err(format_error)?;
        Ok(formatter.format(&time).write_to_string().into_owned())
    }

    /// Resolves the first weekday, including Unicode region and `u-fw` overrides.
    ///
    /// # Errors
    /// Returns an error if CLDR week data is unavailable.
    pub fn first_weekday(&self) -> Result<Weekday, LocalizationError> {
        use icu_calendar::types::Weekday as IcuWeekday;
        let information = icu_calendar::week::WeekInformation::try_new(self.0.clone().into())
            .map_err(format_error)?;
        Ok(match information.first_weekday {
            IcuWeekday::Monday => Weekday::Monday,
            IcuWeekday::Tuesday => Weekday::Tuesday,
            IcuWeekday::Wednesday => Weekday::Wednesday,
            IcuWeekday::Thursday => Weekday::Thursday,
            IcuWeekday::Friday => Weekday::Friday,
            IcuWeekday::Saturday => Weekday::Saturday,
            IcuWeekday::Sunday => Weekday::Sunday,
        })
    }
}

struct NumberFormatter(DecimalFormatter);

impl Localizer {
    /// Formats an hour without minutes, using native digits and catalog day-period labels.
    ///
    /// # Errors
    /// Returns an error if hour-cycle or numbering preferences cannot be resolved.
    pub fn format_compact_hour(&self, time: TimeOfDay) -> Result<String, LocalizationError> {
        let localizer = self.snapshot();
        let locale = localizer.locale_for(super::LocaleCategory::Time);
        let (hour, period) = locale.hour_cycle()?.resolve(time.hour());
        let hour = locale.format_decimal(&hour.into())?;
        let Some(period) = period else {
            return Ok(hour);
        };
        let period = localizer.text(match period {
            crate::time_of_day::DayPeriod::Am => "enzui-time-am",
            crate::time_of_day::DayPeriod::Pm => "enzui-time-pm",
        });
        let mut arguments = Arguments::new();
        arguments.set("hour", hour);
        arguments.set("period", period);
        Ok(localizer.text_with("enzui-time-compact-hour", &arguments))
    }

    /// Builds an editable ASCII time grammar with the locale's hour cycle and catalog day-period labels.
    ///
    /// The separator is `:` and day-period labels are suffixes. This grammar is
    /// shared by time-field display and parsing. Day-period labels are trimmed
    /// and must not start with an ASCII digit or colon. Use [`Locale::format_time`] for
    /// CLDR display conventions such as numbering systems and label placement.
    ///
    /// # Errors
    /// Returns an error if clock preferences are unavailable or day-period labels cannot round-trip.
    pub fn time_format(&self) -> Result<crate::time_of_day::TimeFormat, LocalizationError> {
        let localizer = self.snapshot();
        let cycle = localizer
            .locale_for(super::LocaleCategory::Time)
            .hour_cycle()?;
        let am = localizer.text("enzui-time-am");
        let pm = localizer.text("enzui-time-pm");
        let labels = [am.trim(), pm.trim()];
        if labels.iter().any(|label| {
            label.is_empty()
                || label
                    .starts_with(|character: char| character.is_ascii_digit() || character == ':')
        }) {
            return Err(LocalizationError::Format(
                "day-period labels must be nonnumeric suffixes".to_owned(),
            ));
        }
        let format = crate::time_of_day::TimeFormat::new(cycle).with_day_period_labels(
            crate::time_of_day::DayPeriodLabels::new(labels[0], labels[1]),
        );
        if !format.is_reversible() {
            return Err(LocalizationError::Format(
                "day-period labels must remain distinguishable after Unicode lowercasing"
                    .to_owned(),
            ));
        }
        Ok(format)
    }
}

impl Memoizable for NumberFormatter {
    type Args = bool;
    type Error = LocalizationError;

    fn construct(
        language: unic_langid::LanguageIdentifier,
        grouping: bool,
    ) -> Result<Self, Self::Error> {
        let locale: Locale = language.to_string().parse()?;
        let mut options = DecimalFormatterOptions::default();
        options.grouping_strategy = Some(if grouping {
            GroupingStrategy::Auto
        } else {
            GroupingStrategy::Never
        });
        DecimalFormatter::try_new(locale.0.into(), options)
            .map(Self)
            .map_err(format_error)
    }
}

fn valid_number_options(options: &FluentNumberOptions) -> bool {
    [
        options.minimum_integer_digits,
        options.minimum_significant_digits,
        options.maximum_significant_digits,
    ]
    .into_iter()
    .flatten()
    .all(|value| (1..=100).contains(&value))
        && [
            options.minimum_fraction_digits,
            options.maximum_fraction_digits,
        ]
        .into_iter()
        .flatten()
        .all(|value| value <= 100)
        && options
            .minimum_fraction_digits
            .zip(options.maximum_fraction_digits)
            .is_none_or(|(min, max)| min <= max)
        && options
            .minimum_significant_digits
            .zip(options.maximum_significant_digits)
            .is_none_or(|(min, max)| min <= max)
        && options.style == FluentNumberStyle::Decimal
}

pub(super) fn fluent_number(
    value: &FluentValue<'_>,
    memoizer: &IntlLangMemoizer,
) -> Option<String> {
    let FluentValue::Number(number) = value else {
        return None;
    };
    if !valid_number_options(&number.options) || !number.value.is_finite() {
        invalid_number();
        return Some(String::new());
    }
    let mut decimal: Decimal = number.value.to_string().parse().ok()?;
    let rounding = fixed_decimal::SignedRoundingMode::Unsigned(
        fixed_decimal::UnsignedRoundingMode::HalfExpand,
    );
    if number.options.minimum_significant_digits.is_some()
        || number.options.maximum_significant_digits.is_some()
    {
        if let Some(max) = number.options.maximum_significant_digits {
            let position = decimal.nonzero_magnitude_start() - i16::try_from(max).ok()? + 1;
            decimal.round_with_mode(position, rounding);
        }
        if let Some(min) = number.options.minimum_significant_digits {
            let position = decimal.nonzero_magnitude_start() - i16::try_from(min).ok()? + 1;
            decimal.pad_end(position);
        }
    } else {
        if let Some(max) = number.options.maximum_fraction_digits {
            decimal.round_with_mode(-i16::try_from(max).ok()?, rounding);
        }
        if let Some(min) = number.options.minimum_fraction_digits {
            decimal.pad_end(-i16::try_from(min).ok()?);
        }
    }
    if let Some(min) = number.options.minimum_integer_digits {
        decimal.pad_start(i16::try_from(min).ok()?);
    }
    match memoizer.with_try_get::<NumberFormatter, _, _>(number.options.use_grouping, |formatter| {
        formatter.0.format(&decimal).write_to_string().into_owned()
    }) {
        Ok(value) => Some(value),
        Err(_) => {
            invalid_number();
            Some(String::new())
        }
    }
}

pub(super) fn number_value<'a>(
    positional: &[FluentValue<'a>],
    named: &Arguments<'_>,
) -> FluentValue<'a> {
    let [FluentValue::Number(number)] = positional else {
        return invalid_number();
    };
    if !number.value.is_finite() {
        return invalid_number();
    }
    for (name, value) in named.iter() {
        let valid = match (name, value) {
            ("style", FluentValue::String(value)) => value == "decimal",
            ("type", FluentValue::String(value)) => value == "cardinal" || value == "ordinal",
            ("useGrouping", FluentValue::String(value)) => value == "true" || value == "false",
            (
                "minimumIntegerDigits" | "minimumSignificantDigits" | "maximumSignificantDigits",
                FluentValue::Number(value),
            ) => value.value.fract() == 0.0 && (1.0..=100.0).contains(&value.value),
            ("minimumFractionDigits" | "maximumFractionDigits", FluentValue::Number(value)) => {
                value.value.fract() == 0.0 && (0.0..=100.0).contains(&value.value)
            }
            _ => false,
        };
        if !valid {
            return invalid_number();
        }
    }
    let mut number = number.clone();
    number.options.merge(named);
    if !valid_number_options(&number.options) {
        return invalid_number();
    }
    FluentValue::Number(number)
}

/// Calendar vocabulary and Gregorian display formatting from one locale snapshot.
///
/// The snapshot retains its language when the localizer changes; recreate it
/// for a newly selected locale. Editable date fields keep their own parsing grammar.
pub struct LocaleCalendarLabels {
    localizer: Localizer,
    month: FixedCalendarDateTimeFormatter<Gregorian, fieldsets::M>,
    month_short: FixedCalendarDateTimeFormatter<Gregorian, fieldsets::M>,
    weekday: FixedCalendarDateTimeFormatter<Gregorian, fieldsets::E>,
    weekday_short: FixedCalendarDateTimeFormatter<Gregorian, fieldsets::E>,
    weekday_narrow: Vec<String>,
    numeral: DecimalFormatter,
    month_year: FixedCalendarDateTimeFormatter<Gregorian, fieldsets::YM>,
    date: FixedCalendarDateTimeFormatter<Gregorian, fieldsets::YMD>,
    full_date: FixedCalendarDateTimeFormatter<Gregorian, fieldsets::YMDE>,
}

impl LocaleCalendarLabels {
    /// Captures the current selection and loads reusable CLDR calendar formatters.
    ///
    /// # Errors
    /// Returns an error if the locale's Gregorian date data is unavailable.
    pub fn new(localizer: &Localizer) -> Result<Self, LocalizationError> {
        let localizer = localizer.snapshot();
        let locale = localizer.locale_for(super::LocaleCategory::Time);
        let mut narrow = icu_datetime::pattern::FixedCalendarDateTimeNames::<Gregorian, fieldsets::E>::new_without_number_formatting(locale.0.clone().into());
        narrow
            .include_weekday_names(icu_datetime::pattern::WeekdayNameLength::Narrow)
            .map_err(format_error)?;
        let pattern = icu_datetime::pattern::DateTimePattern::try_from_pattern_str("EEEEE")
            .map_err(format_error)?;
        let weekday_narrow = (1..=7)
            .map(|day| {
                let date =
                    icu_calendar::Date::try_new_gregorian(2024, 1, day).map_err(format_error)?;
                narrow
                    .with_pattern_unchecked(&pattern)
                    .format(&date)
                    .try_write_to_string()
                    .map(std::borrow::Cow::into_owned)
                    .map_err(|(error, _)| format_error(error))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut numeral_options = DecimalFormatterOptions::default();
        numeral_options.grouping_strategy = Some(GroupingStrategy::Never);
        let numeral = DecimalFormatter::try_new(locale.0.clone().into(), numeral_options)
            .map_err(format_error)?;
        Ok(Self {
            localizer,
            weekday_narrow,
            numeral,
            month: FixedCalendarDateTimeFormatter::try_new(
                locale.0.clone().into(),
                fieldsets::M::long(),
            )
            .map_err(format_error)?,
            month_short: FixedCalendarDateTimeFormatter::try_new(
                locale.0.clone().into(),
                fieldsets::M::medium(),
            )
            .map_err(format_error)?,
            weekday: FixedCalendarDateTimeFormatter::try_new(
                locale.0.clone().into(),
                fieldsets::E::long(),
            )
            .map_err(format_error)?,
            weekday_short: FixedCalendarDateTimeFormatter::try_new(
                locale.0.clone().into(),
                fieldsets::E::medium(),
            )
            .map_err(format_error)?,
            month_year: FixedCalendarDateTimeFormatter::try_new(
                locale.0.clone().into(),
                fieldsets::YM::long(),
            )
            .map_err(format_error)?,
            date: FixedCalendarDateTimeFormatter::try_new(
                locale.0.clone().into(),
                fieldsets::YMD::short(),
            )
            .map_err(format_error)?,
            full_date: FixedCalendarDateTimeFormatter::try_new(
                locale.0.into(),
                fieldsets::YMDE::long(),
            )
            .map_err(format_error)?,
        })
    }

    fn date_arg(&self, key: &str, date: Date) -> String {
        let mut arguments = Arguments::new();
        arguments.set("date", self.full_date(date));
        self.localizer.text_with(key, &arguments)
    }
}

impl CalendarLabels for LocaleCalendarLabels {
    fn day_status(&self, status: CalendarDayStatus) -> String {
        self.localizer.text(status.message_key())
    }
    fn month_name(&self, month: u32) -> String {
        let Some(date) = Date::new(2024, month, 1).and_then(|date| icu_date(date).ok()) else {
            return month.to_string();
        };
        self.month.format(&date).write_to_string().into_owned()
    }

    fn month_short(&self, month: u32) -> String {
        let Some(date) = Date::new(2024, month, 1).and_then(|date| icu_date(date).ok()) else {
            return month.to_string();
        };
        self.month_short
            .format(&date)
            .write_to_string()
            .into_owned()
    }

    fn weekday_name(&self, weekday: Weekday) -> String {
        let date = icu_calendar::Date::try_new_gregorian(2024, 1, 1 + weekday.index() as u8)
            .expect("the reference week is valid in the Gregorian calendar");
        self.weekday.format(&date).write_to_string().into_owned()
    }

    fn weekday_short(&self, weekday: Weekday) -> String {
        let date = icu_calendar::Date::try_new_gregorian(2024, 1, 1 + weekday.index() as u8)
            .expect("the reference week is valid in the Gregorian calendar");
        self.weekday_short
            .format(&date)
            .write_to_string()
            .into_owned()
    }

    fn weekday_narrow(&self, weekday: Weekday) -> String {
        self.weekday_narrow[weekday.index()].clone()
    }
    fn day_number(&self, day: u32) -> String {
        self.numeral
            .format(&day.into())
            .write_to_string()
            .into_owned()
    }
    fn year_number(&self, year: i32) -> String {
        self.numeral
            .format(&year.into())
            .write_to_string()
            .into_owned()
    }

    fn month_year(&self, month: YearMonth) -> String {
        let Ok(date) = icu_date(month.clamp_day(1)) else {
            return month.to_string();
        };
        self.month_year.format(&date).write_to_string().into_owned()
    }

    fn display_date(&self, date: Date) -> String {
        let Ok(icu) = icu_date(date) else {
            return date.to_string();
        };
        self.date.format(&icu).write_to_string().into_owned()
    }

    fn full_date(&self, date: Date) -> String {
        let Ok(icu) = icu_date(date) else {
            return date.to_string();
        };
        self.full_date.format(&icu).write_to_string().into_owned()
    }

    fn date_field_name(&self) -> String {
        self.localizer.text("enzui-date")
    }
    fn date_range_field_name(&self) -> String {
        self.localizer.text("enzui-date-range")
    }
    fn range_separator(&self) -> String {
        self.localizer.text("enzui-range-separator")
    }

    fn full_range(&self, range: DateRange) -> String {
        if range.start() == range.end() {
            return self.date_arg("enzui-date-single-day", range.start());
        }
        let mut arguments = Arguments::new();
        arguments.set("start", self.full_date(range.start()));
        arguments.set("end", self.full_date(range.end()));
        self.localizer
            .text_with("enzui-date-full-range", &arguments)
    }

    fn pending_range(&self, anchor: Date) -> String {
        self.date_arg("enzui-date-pending-range", anchor)
    }

    fn day_marker(&self, marker: DayMarker) -> Option<String> {
        match marker {
            DayMarker::None => None,
            DayMarker::Present => Some(self.localizer.text("enzui-date-has-events")),
            DayMarker::Count(count) => {
                let mut arguments = Arguments::new();
                arguments.set("count", count);
                Some(self.localizer.text_with("enzui-date-events", &arguments))
            }
        }
    }
}

/// Calendar defaults that follow the current localizer: the scope, else the environment's locale.
///
/// Apps may still replace these defaults with any [`CalendarLabels`] implementation.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalizedCalendarLabels;

thread_local! {
    static CALENDAR: std::cell::RefCell<Option<(Localizer, u64, std::rc::Rc<LocaleCalendarLabels>)>> = const { std::cell::RefCell::new(None) };
}

fn with_calendar<R>(operation: impl FnOnce(&dyn CalendarLabels) -> R) -> R {
    let localizer = super::context::current();
    let identity = localizer.identity();
    let generation = localizer.generation();
    let labels = CALENDAR.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((old_identity, old_generation, labels)) = cache.as_ref()
            && old_identity.identity() == identity
            && *old_generation == generation
        {
            return Some(std::rc::Rc::clone(labels));
        }
        let labels = std::rc::Rc::new(LocaleCalendarLabels::new(&localizer).ok()?);
        *cache = Some((localizer, generation, std::rc::Rc::clone(&labels)));
        Some(labels)
    });
    match labels {
        Some(labels) => operation(labels.as_ref()),
        None => operation(&crate::calendar_semantics::EnglishLabels),
    }
}

impl CalendarLabels for LocalizedCalendarLabels {
    fn day_status(&self, status: CalendarDayStatus) -> String {
        with_calendar(|labels| labels.day_status(status))
    }
    fn month_name(&self, month: u32) -> String {
        with_calendar(|labels| labels.month_name(month))
    }
    fn weekday_name(&self, weekday: Weekday) -> String {
        with_calendar(|labels| labels.weekday_name(weekday))
    }
    fn month_short(&self, month: u32) -> String {
        with_calendar(|labels| labels.month_short(month))
    }
    fn weekday_short(&self, weekday: Weekday) -> String {
        with_calendar(|labels| labels.weekday_short(weekday))
    }
    fn weekday_narrow(&self, weekday: Weekday) -> String {
        with_calendar(|labels| labels.weekday_narrow(weekday))
    }
    fn day_number(&self, day: u32) -> String {
        with_calendar(|labels| labels.day_number(day))
    }
    fn year_number(&self, year: i32) -> String {
        with_calendar(|labels| labels.year_number(year))
    }

    fn month_year(&self, month: YearMonth) -> String {
        with_calendar(|labels| labels.month_year(month))
    }
    fn display_date(&self, date: Date) -> String {
        with_calendar(|labels| labels.display_date(date))
    }
    fn full_date(&self, date: Date) -> String {
        with_calendar(|labels| labels.full_date(date))
    }
    fn date_field_name(&self) -> String {
        with_calendar(|labels| labels.date_field_name())
    }
    fn date_range_field_name(&self) -> String {
        with_calendar(|labels| labels.date_range_field_name())
    }
    fn range_separator(&self) -> String {
        with_calendar(|labels| labels.range_separator())
    }
    fn full_range(&self, range: DateRange) -> String {
        with_calendar(|labels| labels.full_range(range))
    }
    fn pending_range(&self, anchor: Date) -> String {
        with_calendar(|labels| labels.pending_range(anchor))
    }
    fn day_marker(&self, marker: DayMarker) -> Option<String> {
        with_calendar(|labels| labels.day_marker(marker))
    }
}

#[cfg(test)]
mod kept_formatter_tests {
    use super::*;

    fn decimal(tag: &str) -> String {
        let locale: Locale = tag.parse().expect("locale");
        locale
            .format_decimal(&"1234.5".parse().expect("decimal"))
            .expect("formats")
    }

    /// Kept formatters answer for their own locale, and only a few are kept.
    #[test]
    fn kept_formatters_are_per_locale_and_bounded() {
        assert_eq!(decimal("en-US"), "1,234.5");
        assert_eq!(decimal("de-DE"), "1.234,5");
        assert_eq!(
            decimal("en-US"),
            "1,234.5",
            "the English one, not the last one"
        );
        for tag in [
            "fr-FR", "es-ES", "it-IT", "nl-NL", "pl-PL", "sv-SE", "fi-FI", "da-DK", "pt-BR",
        ] {
            let _ = decimal(tag);
        }
        assert_eq!(DECIMALS.with(|kept| kept.borrow().len()), FORMATTERS_KEPT);
        assert_eq!(decimal("de-DE"), "1.234,5", "made again after eviction");
    }
}
