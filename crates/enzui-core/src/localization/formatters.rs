//! Money, percentages, relative times and durations in the locale's words
//! (feature `locale-formatters`, CLDR data through ICU4X).
//!
//! Format with the locale of the right category
//! (`Localizer::locale_for`): `Monetary` for money, `Numeric` for
//! percentages, `Time` for relative times and durations.
//!
//! | call | en-US | de |
//! |---|---|---|
//! | `format_currency(1234.5, "EUR")` | €1,234.50 | 1.234,50 € |
//! | `format_percent(42)` | 42% | 42 % |
//! | `format_relative_time(-300)` | 5 minutes ago | vor 5 Minuten |
//! | `format_relative_time(-86_400)` | yesterday | gestern |
//! | `format_duration(3_900 s, Short)` | 1 hr, 5 min | 1 Std., 5 Min. |

use super::{Decimal, Locale, LocalizationError};
use icu_experimental::dimension::currency::CurrencyType;
use icu_experimental::dimension::currency::formatter::CurrencyFormatter;
use icu_experimental::dimension::percent::formatter::PercentFormatter;
use icu_experimental::duration::ValidatedDurationFormatterOptions;
use icu_experimental::duration::options::{BaseStyle, DurationFormatterOptions, FieldDisplay};
use icu_experimental::duration::{Duration, DurationFormatter, DurationSign};
use icu_experimental::relativetime::options::Numeric;
use icu_experimental::relativetime::{RelativeTimeFormatter, RelativeTimeFormatterOptions};
use writeable::Writeable as _;

fn format_error(error: impl std::fmt::Display) -> LocalizationError {
    LocalizationError::Format(error.to_string())
}

/// How wordy a [`Locale::format_duration`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum DurationStyle {
    /// `1h 5m`.
    Narrow,
    /// `1 hr, 5 min`.
    #[default]
    Short,
    /// `1 hour, 5 minutes`.
    Long,
    /// `1:05:00`, or `05:30` under an hour.
    Digital,
}

impl Locale {
    /// `amount` of the currency with ISO 4217 code `currency` (`"EUR"`,
    /// `"USD"`), with its symbol where the locale puts it.
    ///
    /// # Errors
    /// The code is not three ASCII letters, or the locale data is missing.
    pub fn format_currency(
        &self,
        amount: &Decimal,
        currency: &str,
    ) -> Result<String, LocalizationError> {
        let code = CurrencyType::try_from_str(currency)
            .map_err(|_| format_error(format!("{currency:?} is not a currency code")))?;
        let formatter =
            CurrencyFormatter::try_new_symbol((&self.0).into(), code, Default::default())
                .map_err(format_error)?;
        Ok(formatter
            .format_fixed_decimal(amount)
            .write_to_string()
            .into_owned())
    }

    /// `percent` per cent (`42` gives `42%`), with the locale's sign and
    /// spacing.
    ///
    /// # Errors
    /// The locale data is missing.
    pub fn format_percent(&self, percent: &Decimal) -> Result<String, LocalizationError> {
        let formatter = PercentFormatter::try_new((&self.0).into(), Default::default())
            .map_err(format_error)?;
        Ok(formatter.format(percent).write_to_string().into_owned())
    }

    /// A moment `seconds` from now (negative for the past) in words: `in 3
    /// days`, `5 minutes ago`, `yesterday`, `now`. The unit is the largest
    /// that fits: seconds under a minute, minutes under an hour, hours under
    /// a day, then days, weeks, months (30 days) and years (365 days).
    ///
    /// # Errors
    /// The locale data is missing.
    pub fn format_relative_time(&self, seconds: i64) -> Result<String, LocalizationError> {
        const MINUTE: i64 = 60;
        const HOUR: i64 = 60 * MINUTE;
        const DAY: i64 = 24 * HOUR;
        const WEEK: i64 = 7 * DAY;
        const MONTH: i64 = 30 * DAY;
        const YEAR: i64 = 365 * DAY;
        let magnitude = seconds.unsigned_abs();
        let mut options = RelativeTimeFormatterOptions::default();
        options.numeric = Numeric::Auto;
        let prefs = (&self.0).into();
        let (formatter, unit) = match magnitude {
            m if m < MINUTE.unsigned_abs() => (
                RelativeTimeFormatter::try_new_long_second(prefs, options),
                1,
            ),
            m if m < HOUR.unsigned_abs() => (
                RelativeTimeFormatter::try_new_long_minute(prefs, options),
                MINUTE,
            ),
            m if m < DAY.unsigned_abs() => (
                RelativeTimeFormatter::try_new_long_hour(prefs, options),
                HOUR,
            ),
            m if m < WEEK.unsigned_abs() => {
                (RelativeTimeFormatter::try_new_long_day(prefs, options), DAY)
            }
            m if m < MONTH.unsigned_abs() => (
                RelativeTimeFormatter::try_new_long_week(prefs, options),
                WEEK,
            ),
            m if m < YEAR.unsigned_abs() => (
                RelativeTimeFormatter::try_new_long_month(prefs, options),
                MONTH,
            ),
            _ => (
                RelativeTimeFormatter::try_new_long_year(prefs, options),
                YEAR,
            ),
        };
        let formatter = formatter.map_err(format_error)?;
        // Whole units, towards zero: 90 seconds ago is 1 minute ago.
        let count = seconds / unit;
        Ok(formatter
            .format(Decimal::from(count))
            .write_to_string()
            .into_owned())
    }

    /// `duration` in hours, minutes and seconds (whole seconds), as `style`
    /// says; zero fields are left out.
    ///
    /// # Errors
    /// The locale data is missing.
    pub fn format_duration(
        &self,
        duration: std::time::Duration,
        style: DurationStyle,
    ) -> Result<String, LocalizationError> {
        let total = duration.as_secs();
        let value = Duration {
            sign: DurationSign::Positive,
            hours: total / 3600,
            minutes: total / 60 % 60,
            seconds: total % 60,
            ..Duration::default()
        };
        let mut options = DurationFormatterOptions::default();
        options.base = match style {
            DurationStyle::Narrow => BaseStyle::Narrow,
            DurationStyle::Short => BaseStyle::Short,
            DurationStyle::Long => BaseStyle::Long,
            DurationStyle::Digital => BaseStyle::Digital,
        };
        if style == DurationStyle::Digital {
            // `1:05:00`, not `1:05`, which reads as minutes and seconds.
            options.second_visibility = Some(FieldDisplay::Always);
        }
        let options = ValidatedDurationFormatterOptions::validate(options)
            .map_err(|error| format_error(format!("{error:?}")))?;
        let formatter =
            DurationFormatter::try_new((&self.0).into(), options).map_err(format_error)?;
        Ok(formatter.format(&value).write_to_string().into_owned())
    }
}
