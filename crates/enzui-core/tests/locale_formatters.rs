//! Money, percentages, relative times and durations in four languages,
//! from CLDR.

#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::print_stdout,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing"
)]
use enzui_core::localization::{Decimal, DurationStyle, Locale};
use std::time::Duration;

fn locale(tag: &str) -> Locale {
    Locale::from_posix(tag).unwrap()
}

#[test]
fn money_percent_relative_time_and_durations_read_as_the_language_writes_them() {
    let amount: Decimal = "1234.5".parse().unwrap();
    let cases = [
        (
            "en_US",
            "€1,234.50",
            "42%",
            "5 minutes ago",
            "yesterday",
            "in 2 hours",
            "1 hr, 5 min",
        ),
        (
            "de_DE",
            "1.234,50\u{a0}€",
            "42\u{a0}%",
            "vor 5 Minuten",
            "gestern",
            "in 2 Stunden",
            "1 Std., 5 Min.",
        ),
        (
            "fr_FR",
            "1\u{202f}234,50\u{a0}€",
            "42\u{a0}%",
            "il y a 5 minutes",
            "hier",
            "dans 2 heures",
            "1\u{202f}h et 5\u{a0}min",
        ),
        (
            "hu_HU",
            "1234,50\u{a0}€",
            "42%",
            "5 perccel ezelőtt",
            "tegnap",
            "2 óra múlva",
            "1 ó és 5 p",
        ),
    ];
    for (tag, money, percent, minutes_ago, yesterday, in_hours, duration) in cases {
        let l = locale(tag);
        assert_eq!(l.format_currency(&amount, "eur").unwrap(), money, "{tag}");
        assert_eq!(
            l.format_percent(&Decimal::from(42)).unwrap(),
            percent,
            "{tag}"
        );
        assert_eq!(l.format_relative_time(-300).unwrap(), minutes_ago, "{tag}");
        assert_eq!(l.format_relative_time(-86_400).unwrap(), yesterday, "{tag}");
        assert_eq!(l.format_relative_time(7_200).unwrap(), in_hours, "{tag}");
        assert_eq!(
            l.format_duration(Duration::from_secs(3_900), DurationStyle::Short)
                .unwrap(),
            duration,
            "{tag}"
        );
    }
    let en = locale("en_US");
    assert_eq!(
        en.format_relative_time(-90).unwrap(),
        "1 minute ago",
        "whole units"
    );
    assert_eq!(en.format_relative_time(0).unwrap(), "now");
    assert_eq!(en.format_relative_time(-10 * 86_400).unwrap(), "last week");
    assert_eq!(en.format_relative_time(400 * 86_400).unwrap(), "next year");
    assert_eq!(
        en.format_duration(Duration::from_secs(3_900), DurationStyle::Digital)
            .unwrap(),
        "1:05:00"
    );
    assert_eq!(
        en.format_duration(Duration::from_secs(330), DurationStyle::Digital)
            .unwrap(),
        "05:30"
    );
    assert!(en.format_currency(&amount, "EURO").is_err());
}
