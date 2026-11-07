//! Accessible formatting helpers for schedule times and durations.

/// Formats a minute offset as 24-hour `HH:MM`, wrapping across midnight.
#[must_use]
pub fn format_hh_mm(minute: i32) -> String {
    let m = minute.rem_euclid(1440);
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// Formats a minute offset with the current localizer's clock preferences.
///
/// Values wrap within a day. Unsupported CLDR display cycles use the locale's editable grammar.
#[must_use]
pub fn format_localized_clock(minute: i32) -> String {
    let minute = minute.rem_euclid(1440) as u32;
    let localizer = crate::localization::current();
    if let Some(time) = crate::time_of_day::TimeOfDay::hm(minute / 60, minute % 60) {
        return localizer
            .locale_for(crate::localization::LocaleCategory::Time)
            .format_time(time, false)
            .or_else(|_| localizer.time_format().map(|format| format.format(time)))
            .unwrap_or_else(|_| format_hh_mm(minute as i32));
    }
    format_hh_mm(minute as i32)
}

/// Formats a duration with catalog vocabulary; English falls back to hours and minutes.
#[must_use]
pub fn format_duration(minutes: i32) -> String {
    let mins = minutes.max(0);
    let h = mins / 60;
    let m = mins % 60;
    match (h, m) {
        (0, m) => crate::localized!("enzui-schedule-minutes", "minutes" => m),
        (h, 0) => crate::localized!("enzui-schedule-hours", "hours" => h),
        (h, m) => crate::localized!("enzui-schedule-hours-minutes", "hours" => h, "minutes" => m),
    }
}
