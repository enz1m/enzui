//! Typed local civil-time acquisition at the system timezone boundary.
//!
//! Conversion samples or accepts one [`SystemTime`],
//! resolves that instant's UTC offset, and returns date-plus-minute data without
//! presentation formatting.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::civil_date::{Date, MAX_YEAR, MIN_YEAR};

const SECONDS_PER_DAY: i64 = 86_400;
const MAX_UTC_OFFSET_SECONDS: i64 = SECONDS_PER_DAY;

/// A local calendar date and zero-based minute within that date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalCivilTime {
    /// The local proleptic-Gregorian date.
    pub date: Date,
    /// The local minute of day, in `0..=1439`.
    pub minute: u16,
}

/// A failure to acquire or represent local civil time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum LocalCivilTimeError {
    /// The target has no supported system-timezone resolver.
    UnsupportedPlatform,
    /// The supplied [`SystemTime`] cannot be represented as a signed Unix second.
    SystemTimeOutOfRange,
    /// The resolver returned an offset outside the supported one-day range.
    UtcOffsetOutOfRange {
        /// Seconds east of UTC returned by the resolver.
        seconds: i64,
    },
    /// Applying the UTC offset overflowed or produced a year outside [`Date`]'s bounds.
    LocalTimeOutOfRange,
    /// The platform timezone conversion failed for the requested instant.
    TimezoneConversionFailed,
    /// A timezone conversion or civil decomposition produced invalid calendar fields.
    InvalidCivilFields,
}

impl fmt::Display for LocalCivilTimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                f.write_str("local civil time is unsupported on this platform")
            }
            Self::SystemTimeOutOfRange => {
                f.write_str("system time is outside the signed Unix-second range")
            }
            Self::UtcOffsetOutOfRange { seconds } => {
                write!(
                    f,
                    "utc offset {seconds} seconds is outside the supported range"
                )
            }
            Self::LocalTimeOutOfRange => {
                f.write_str("offset-adjusted local time is outside the civil-date range")
            }
            Self::TimezoneConversionFailed => f.write_str("system timezone conversion failed"),
            Self::InvalidCivilFields => {
                f.write_str("timezone conversion produced invalid civil fields")
            }
        }
    }
}

impl std::error::Error for LocalCivilTimeError {}

/// Samples the system clock once and returns the corresponding local civil time.
///
/// # Errors
///
/// Returns [`LocalCivilTimeError`] when the platform has no supported timezone
/// resolver, the timezone conversion fails, or the instant, offset, or civil
/// fields cannot be represented.
pub fn current_local_civil_time() -> Result<LocalCivilTime, LocalCivilTimeError> {
    local_civil_time_at(SystemTime::now())
}

/// Converts one instant through the system timezone.
///
/// # Errors
///
/// Returns [`LocalCivilTimeError`] when the platform has no supported timezone
/// resolver, the timezone conversion fails, or the instant, offset, or civil
/// fields cannot be represented.
pub fn local_civil_time_at(instant: SystemTime) -> Result<LocalCivilTime, LocalCivilTimeError> {
    local_civil_time_at_with(instant, system_utc_offset)
}

/// Converts one injected instant through an injected UTC-offset resolver.
///
/// The resolver receives the instant as a mathematically floored signed Unix
/// second and returns seconds east of UTC. It is called once when the instant
/// fits that representation, and is not called otherwise.
///
/// # Errors
///
/// Propagates a resolver error. Returns [`LocalCivilTimeError::SystemTimeOutOfRange`]
/// when `instant` has no signed Unix-second representation,
/// [`LocalCivilTimeError::UtcOffsetOutOfRange`] for offsets beyond one day,
/// [`LocalCivilTimeError::LocalTimeOutOfRange`] when checked offset application
/// or date representation fails, and [`LocalCivilTimeError::InvalidCivilFields`]
/// when decomposition does not form a valid date and minute.
pub fn local_civil_time_at_with(
    instant: SystemTime,
    resolve_offset: impl FnOnce(i64) -> Result<i64, LocalCivilTimeError>,
) -> Result<LocalCivilTime, LocalCivilTimeError> {
    let epoch_seconds = epoch_seconds_floor(instant)?;
    let offset = resolve_offset(epoch_seconds)?;
    if !(-MAX_UTC_OFFSET_SECONDS..=MAX_UTC_OFFSET_SECONDS).contains(&offset) {
        return Err(LocalCivilTimeError::UtcOffsetOutOfRange { seconds: offset });
    }
    let local_seconds = epoch_seconds
        .checked_add(offset)
        .ok_or(LocalCivilTimeError::LocalTimeOutOfRange)?;
    civil_from_unix(local_seconds)
}

fn epoch_seconds_floor(instant: SystemTime) -> Result<i64, LocalCivilTimeError> {
    match instant.duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            i64::try_from(duration.as_secs()).map_err(|_| LocalCivilTimeError::SystemTimeOutOfRange)
        }
        Err(error) => {
            let duration = error.duration();
            let seconds = -i128::from(duration.as_secs());
            let floored = if duration.subsec_nanos() == 0 {
                seconds
            } else {
                seconds - 1
            };
            i64::try_from(floored).map_err(|_| LocalCivilTimeError::SystemTimeOutOfRange)
        }
    }
}

fn civil_from_unix(seconds: i64) -> Result<LocalCivilTime, LocalCivilTimeError> {
    let days = seconds.div_euclid(SECONDS_PER_DAY);
    let second_of_day = seconds.rem_euclid(SECONDS_PER_DAY);
    let z = days
        .checked_add(719_468)
        .ok_or(LocalCivilTimeError::LocalTimeOutOfRange)?;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    let minute = second_of_day / 60;

    validated_local_civil(year, month, day, minute)
}

fn validated_local_civil(
    year: i64,
    month: i64,
    day: i64,
    minute: i64,
) -> Result<LocalCivilTime, LocalCivilTimeError> {
    if year < i64::from(MIN_YEAR) || year > i64::from(MAX_YEAR) {
        return Err(LocalCivilTimeError::LocalTimeOutOfRange);
    }
    let year = i32::try_from(year).map_err(|_| LocalCivilTimeError::LocalTimeOutOfRange)?;
    let month = u32::try_from(month).map_err(|_| LocalCivilTimeError::InvalidCivilFields)?;
    let day = u32::try_from(day).map_err(|_| LocalCivilTimeError::InvalidCivilFields)?;
    let minute = u16::try_from(minute).map_err(|_| LocalCivilTimeError::InvalidCivilFields)?;
    if minute >= 24 * 60 {
        return Err(LocalCivilTimeError::InvalidCivilFields);
    }
    let date = Date::new(year, month, day).ok_or(LocalCivilTimeError::InvalidCivilFields)?;
    Ok(LocalCivilTime { date, minute })
}

#[allow(
    unsafe_code,
    reason = "the C library is the only source of the system time zone offset"
)]
fn system_utc_offset(epoch_seconds: i64) -> Result<i64, LocalCivilTimeError> {
    #[cfg(target_os = "linux")]
    {
        use std::mem::MaybeUninit;

        let native_time = libc::time_t::try_from(epoch_seconds)
            .map_err(|_| LocalCivilTimeError::SystemTimeOutOfRange)?;
        let mut native_civil = MaybeUninit::<libc::tm>::uninit();
        // SAFETY: `native_time` and `native_civil` are live, aligned values;
        // `localtime_r` writes one `tm` through the exclusive result pointer.
        let result = unsafe { libc::localtime_r(&native_time, native_civil.as_mut_ptr()) };
        if result.is_null() {
            return Err(LocalCivilTimeError::TimezoneConversionFailed);
        }
        // SAFETY: a non-null `localtime_r` result points to the supplied output
        // and guarantees that the complete `tm` value was initialized.
        let native_civil = unsafe { native_civil.assume_init() };
        validate_native_civil(&native_civil)?;
        // `tm_gmtoff` is a C `long`: 32 bits on 32-bit targets.
        #[allow(
            clippy::useless_conversion,
            reason = "`tm_gmtoff` is a C `long`, 32 bits on 32-bit targets"
        )]
        let offset = i64::from(native_civil.tm_gmtoff);
        Ok(offset)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = epoch_seconds;
        Err(LocalCivilTimeError::UnsupportedPlatform)
    }
}

#[cfg(target_os = "linux")]
fn validate_native_civil(native: &libc::tm) -> Result<(), LocalCivilTimeError> {
    let year = native
        .tm_year
        .checked_add(1900)
        .ok_or(LocalCivilTimeError::InvalidCivilFields)?;
    if !(MIN_YEAR..=MAX_YEAR).contains(&year) {
        return Err(LocalCivilTimeError::LocalTimeOutOfRange);
    }
    let month = native
        .tm_mon
        .checked_add(1)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(LocalCivilTimeError::InvalidCivilFields)?;
    let day = u32::try_from(native.tm_mday).map_err(|_| LocalCivilTimeError::InvalidCivilFields)?;
    if Date::new(year, month, day).is_none()
        || !(0..24).contains(&native.tm_hour)
        || !(0..60).contains(&native.tm_min)
        || !(0..=60).contains(&native.tm_sec)
    {
        return Err(LocalCivilTimeError::InvalidCivilFields);
    }
    Ok(())
}
