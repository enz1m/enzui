use super::*;
use crate::calendar_semantics::{CalendarLabels, DayMarker};
use crate::civil_date::{Date, Weekday};
use crate::placement::{Direction, ambient_direction};
use crate::time_of_day::TimeOfDay;
use std::sync::{Arc, Barrier};

fn localizer(resources: &[(&str, &str)], preferred: &[&str]) -> Localizer {
    let mut builder = CatalogBuilder::new();
    for (locale, source) in resources {
        builder.add_resource(locale, *source).unwrap();
    }
    Localizer::new(
        builder.build(),
        preferred
            .iter()
            .map(|locale| locale.parse().unwrap())
            .collect(),
        Locale::default(),
    )
}

#[test]
fn localization_catalog_admission_fallbacks_and_layers() {
    // localization_resource_admission_is_transactional
    {
        let mut builder = CatalogBuilder::new();
        builder.add_resource("fr", "save = Enregistrer").unwrap();
        assert!(matches!(
            builder.add_resource("fr", "leaked = Fuite\nsave = Écraser"),
            Err(LocalizationError::DuplicateMessage(_))
        ));
        assert!(matches!(
            builder.add_resource("fr", "partial = Invalide\nbroken = {"),
            Err(LocalizationError::InvalidResource(_))
        ));
        assert!(matches!(
            builder.add_resource("not_a_locale", "x = Invalide"),
            Err(LocalizationError::InvalidLocale(_))
        ));
        let localizer = Localizer::new(
            builder.build(),
            vec!["fr".parse().unwrap()],
            Locale::default(),
        );
        assert_eq!(localizer.text("save"), "Enregistrer");
        for key in ["leaked", "partial"] {
            assert!(matches!(
                localizer.try_text(key, None),
                Err(LocalizationError::MissingMessage(_))
            ));
        }
    }

    // localization_negotiates_preferences_and_falls_back_per_message_and_attribute
    {
        let localizer = localizer(
            &[
                (
                    "fr",
                    "save = Enregistrer\nbutton = Bouton\n    .tooltip = Aide",
                ),
                ("de", "save = Speichern\nsecondary = Zweite Wahl"),
                (
                    "en-US",
                    "fallback = Fallback\nbutton = Button\n    .label = Accessible button",
                ),
            ],
            &["fr-CA", "de-DE"],
        );
        assert_eq!(localizer.text("save"), "Enregistrer");
        assert_eq!(localizer.text("secondary"), "Zweite Wahl");
        assert_eq!(localizer.text("fallback"), "Fallback");
        assert_eq!(localizer.text("button.tooltip"), "Aide");
        assert_eq!(localizer.text("button.label"), "Accessible button");
        // The framework ships French, so its own messages follow the preference.
        assert_eq!(localizer.text("enzui-close"), "Fermer");
        assert_eq!(localizer.locale().to_string(), "fr-CA");
        assert_eq!(
            localizer.resolve("fallback", None).locale.as_deref(),
            Some("en-US")
        );
    }

    // localization_broken_patterns_fall_back_and_report_diagnostics
    {
        let localizer = localizer(
            &[
                ("fr", "welcome = Bonjour { $missing }\ncycle = { cycle }"),
                ("en-US", "welcome = Welcome\ncycle = Safe fallback"),
            ],
            &["fr"],
        );
        for (key, expected) in [("welcome", "Welcome"), ("cycle", "Safe fallback")] {
            let result = localizer.resolve(key, None);
            assert_eq!(result.value, expected);
            assert!(matches!(
                result.errors.as_slice(),
                [LocalizationError::Format(_)]
            ));
            assert!(matches!(
                localizer.try_text(key, None),
                Err(LocalizationError::Format(_))
            ));
        }
        let missing = localizer.resolve("absent", None);
        assert_eq!(missing.value, "absent");
        assert!(missing.locale.is_none());
        assert!(matches!(
            missing.errors.as_slice(),
            [LocalizationError::MissingMessage(_)]
        ));
    }

    // localization_framework_translations_sit_between_app_and_fallback
    {
        // No app catalog: the framework's own German applies.
        assert_eq!(localizer(&[], &["de-AT"]).text("enzui-close"), "Schließen");
        // The app's catalog for the preferred language wins.
        let app = localizer(&[("de", "enzui-close = Zumachen")], &["de"]);
        assert_eq!(app.text("enzui-close"), "Zumachen");
        // The framework's German wins over the app's English fallback.
        let fallback = localizer(&[("en-US", "enzui-close = Shut")], &["de"]);
        assert_eq!(fallback.text("enzui-close"), "Schließen");
        // A language the framework does not ship falls back to English.
        assert_eq!(localizer(&[], &["nl"]).text("enzui-close"), "Close");
    }
}

#[test]
fn localization_terms_attributes_and_variables_preserve_bidi_isolation() {
    let localizer = localizer(
        &[(
            "ar",
            "-brand = إنزيو\nwelcome = أهلاً { $name } في { -brand }\n    .title = مرحباً",
        )],
        &["ar"],
    );
    let mut args = Arguments::new();
    args.set("name", "Alice");
    assert_eq!(
        localizer.text_with("welcome", &args),
        "أهلاً \u{2068}Alice\u{2069} في إنزيو"
    );
    assert_eq!(localizer.text("welcome.title"), "مرحباً");
}

#[test]
fn localization_exact_numbers_plural_selection_and_validation_1() {
    // localization_plural_rules_cover_arabic_categories_and_exact_matches
    {
        let localizer = localizer(
            &[(
                "ar",
                "items = { $count ->\n    [zero] لا شيء\n    [one] واحد\n    [two] اثنان\n    [few] قليل\n    [many] كثير\n   *[other] آخر\n}\nexact = { $count ->\n    [0] Exact zero\n    [zero] Category zero\n   *[other] Other\n}",
            )],
            &["ar"],
        );
        for (count, expected) in [
            (0, "لا شيء"),
            (1, "واحد"),
            (2, "اثنان"),
            (3, "قليل"),
            (11, "كثير"),
            (100, "آخر"),
        ] {
            let mut args = Arguments::new();
            args.set("count", count);
            assert_eq!(
                localizer.try_text("items", Some(&args)).unwrap(),
                expected,
                "count {count}"
            );
        }
        let mut arguments = Arguments::new();
        arguments.set("count", 0);
        assert_eq!(
            localizer.try_text("exact", Some(&arguments)).unwrap(),
            "Exact zero"
        );
        arguments.set("count", 1);
        assert_eq!(
            localizer.try_text("exact", Some(&arguments)).unwrap(),
            "Other"
        );
    }

    // localization_decimal_formatting_uses_exact_digits_and_locale_numbering
    {
        let amount: Decimal = "1234567.50".parse().unwrap();
        assert_eq!(
            "de-DE"
                .parse::<Locale>()
                .unwrap()
                .format_decimal(&amount)
                .unwrap(),
            "1.234.567,50"
        );
        assert_eq!(
            "fr-FR"
                .parse::<Locale>()
                .unwrap()
                .format_decimal(&amount)
                .unwrap(),
            "1\u{202f}234\u{202f}567,50"
        );
        let arabic: Locale = "en-u-nu-arab".parse().unwrap();
        assert_eq!(
            arabic.format_decimal(&"1234".parse().unwrap()).unwrap(),
            "١,٢٣٤"
        );
        assert_eq!(
            "ar-EG"
                .parse::<Locale>()
                .unwrap()
                .format_decimal(&"1234".parse().unwrap())
                .unwrap(),
            "١٬٢٣٤"
        );
        let integer: Decimal = "9007199254740993".parse().unwrap();
        assert_eq!(
            Locale::default().format_decimal(&integer).unwrap(),
            "9,007,199,254,740,993"
        );
    }

    // localization_fluent_numbers_use_the_message_language_and_number_options
    {
        let localizer = localizer(
            &[(
                "de",
                "amount = Wert: { NUMBER($amount, minimumFractionDigits: 2, maximumFractionDigits: 2) }\nplain = { NUMBER($amount, useGrouping: \"false\") }",
            )],
            &["de"],
        );
        let mut args = Arguments::new();
        args.set("amount", 1234.5);
        assert_eq!(
            localizer.try_text("amount", Some(&args)).unwrap(),
            "Wert: \u{2068}1.234,50\u{2069}"
        );
        assert_eq!(localizer.try_text("plain", Some(&args)).unwrap(), "1234,5");
    }

    // localization_number_functions_reject_unsupported_and_invalid_options
    {
        let localizer = localizer(
            &[(
                "en-US",
                "currency = { NUMBER($amount, style: \"currency\") }\nreversed = { NUMBER($amount, minimumFractionDigits: 3, maximumFractionDigits: 1) }\nunknown = { NUMBER($amount, unsupported: 1) }\ninvalid = { NUMBER($amount, minimumIntegerDigits: 0) }\nvalid = { NUMBER($amount, maximumSignificantDigits: 3, minimumFractionDigits: 4) }",
            )],
            &["en-US"],
        );
        let mut arguments = Arguments::new();
        arguments.set("amount", 1234.5);
        for key in ["currency", "reversed", "unknown", "invalid"] {
            assert!(
                matches!(
                    localizer.try_text(key, Some(&arguments)),
                    Err(LocalizationError::Format(_))
                ),
                "{key}"
            );
        }
        assert_eq!(
            localizer.try_text("valid", Some(&arguments)).unwrap(),
            "1,230"
        );
    }
}

#[test]
fn localization_locale_selection_is_shared_idempotent_and_window_local() {
    let localizer = localizer(
        &[("en-US", "save = Save"), ("fr", "save = Enregistrer")],
        &["en-US"],
    );
    let shared = localizer.clone();
    let independent = localizer.snapshot();
    assert_eq!(shared.generation(), 0);
    assert!(localizer.select(vec!["fr".parse().unwrap()]));
    assert_eq!(shared.generation(), 1);
    assert_eq!(shared.text("save"), "Enregistrer");
    assert_eq!(independent.text("save"), "Save");
    assert!(!shared.select(vec!["fr".parse().unwrap()]));
    assert_eq!(localizer.generation(), 1);
    assert!(shared.select(Vec::new()));
    assert_eq!(localizer.text("save"), "Save");
}

#[test]
fn localization_scopes_restore_during_unwinding() {
    let outer = localizer(&[("fr", "save = Enregistrer")], &["fr"]);
    let inner = localizer(&[("de", "save = Speichern")], &["de"]);
    outer.scope(|| {
        assert_eq!(text("save"), "Enregistrer");
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            inner.scope(|| {
                assert_eq!(text("save"), "Speichern");
                panic!("scope restoration control");
            });
        }));
        assert!(result.is_err());
        assert_eq!(text("save"), "Enregistrer");
    });
    assert!(super::scoped_locale().is_none());
}

#[test]
fn localization_scopes_are_isolated_across_overlapping_threads() {
    let mut builder = CatalogBuilder::new();
    builder.add_resource("fr", "save = Enregistrer").unwrap();
    builder.add_resource("de", "save = Speichern").unwrap();
    builder.add_resource("en-US", "shared-number = { NUMBER($value, minimumFractionDigits: 2, maximumFractionDigits: 2) }").unwrap();
    let catalog = builder.build();
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = [("fr", "Enregistrer"), ("de", "Speichern")]
        .into_iter()
        .map(|(language, expected)| {
            let barrier = Arc::clone(&barrier);
            let catalog = catalog.clone();
            std::thread::spawn(move || {
                let localizer =
                    Localizer::new(catalog, vec![language.parse().unwrap()], Locale::default());
                localizer.scope(|| {
                    barrier.wait();
                    let mut arguments = Arguments::new();
                    arguments.set("value", 1234.5);
                    for _ in 0..16 {
                        assert_eq!(text("save"), expected);
                        assert_eq!(text_with("shared-number", &arguments), "1,234.50");
                    }
                    barrier.wait();
                });
                assert!(super::scoped_locale().is_none());
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
}

#[test]
fn localization_locale_negotiation_and_calendar_defaults_1() {
    // localization_posix_and_unicode_locales_respect_scripts
    {
        assert_eq!(
            Locale::from_posix("sr_RS.UTF-8@latin").unwrap().to_string(),
            "sr-Latn-RS"
        );
        assert_eq!(Locale::from_posix("C.UTF-8").unwrap(), Locale::default());
        assert!(Locale::from_posix("sr_RS@unknown").is_err());
        for (tag, direction) in [
            ("ar", Direction::RightToLeft),
            ("az-Arab", Direction::RightToLeft),
            ("ar-Latn", Direction::LeftToRight),
            ("he", Direction::RightToLeft),
            ("en", Direction::LeftToRight),
        ] {
            let locale: Locale = tag.parse().unwrap();
            assert_eq!(locale.direction(), direction, "{tag}");
            let localizer = Localizer::new(Catalog::default(), vec![locale], Locale::default());
            localizer.scope(|| assert_eq!(ambient_direction(), direction));
        }
    }

    // localization_environment_preferences_obey_posix_and_gnu_precedence
    {
        let read = |key: &str| match key {
            "LC_ALL" => Some("fr_CA.UTF-8".to_owned()),
            "LC_MESSAGES" => Some("de_DE.UTF-8".to_owned()),
            "LANG" => Some("en_US.UTF-8".to_owned()),
            "LANGUAGE" => Some("ar:fr_CA:invalid_tag_bad:ar".to_owned()),
            _ => None,
        };
        let locales = preferred_locales_from_env(read);
        assert_eq!(
            locales.iter().map(ToString::to_string).collect::<Vec<_>>(),
            ["ar", "fr-CA"]
        );
        let locales = preferred_locales_from_env(|key| {
            if key == "LC_ALL" {
                Some("C.UTF-8".to_owned())
            } else {
                read(key)
            }
        });
        assert_eq!(locales, vec![Locale::default()]);
        assert_eq!(
            preferred_locales_from_env(|_| None),
            vec![Locale::default()]
        );
    }

    // localization_cldr_dates_time_and_week_preferences_share_the_locale
    {
        let french: Locale = "fr-FR".parse().unwrap();
        let date = Date::new(2026, 10, 19).unwrap();
        assert_eq!(
            french.format_date(date, DateStyle::Long).unwrap(),
            "19 octobre 2026"
        );
        assert_eq!(french.first_weekday().unwrap(), Weekday::Monday);
        assert_eq!(Locale::default().first_weekday().unwrap(), Weekday::Sunday);
        assert_eq!(
            "en-US-u-fw-wed"
                .parse::<Locale>()
                .unwrap()
                .first_weekday()
                .unwrap(),
            Weekday::Wednesday
        );
        let time = TimeOfDay::new(13, 5, 7).unwrap();
        assert_eq!(french.format_time(time, false).unwrap(), "13:05");
        assert_eq!(french.format_time(time, true).unwrap(), "13:05:07");
        assert_eq!(
            "en-US-u-hc-h23"
                .parse::<Locale>()
                .unwrap()
                .format_time(time, false)
                .unwrap(),
            "13:05"
        );
    }

    // localization_calendar_defaults_switch_and_explicit_snapshots_stay_stable
    {
        let localizer = localizer(
            &[(
                "fr",
                "enzui-date = Date française\nenzui-date-events = { $count ->\n    [one] Un événement\n   *[other] { $count } événements\n}",
            )],
            &["fr"],
        );
        let snapshot = LocaleCalendarLabels::new(&localizer).unwrap();
        localizer.scope(|| {
            assert_eq!(LocalizedCalendarLabels.month_name(10), "octobre");
            assert_eq!(LocalizedCalendarLabels.date_field_name(), "Date française");
            assert_eq!(
                LocalizedCalendarLabels
                    .day_marker(DayMarker::Count(1))
                    .as_deref(),
                Some("Un événement")
            );
            localizer.select(vec!["de".parse().unwrap()]);
            assert_eq!(LocalizedCalendarLabels.month_name(10), "Oktober");
            assert_eq!(snapshot.month_name(10), "octobre");
            assert_eq!(snapshot.date_field_name(), "Date française");
        });
        assert_eq!(LocalizedCalendarLabels.month_name(10), "October");
    }
}

#[test]
fn localization_catalog_coverage_and_pseudo_locale() {
    // localization_bundled_english_is_valid_and_has_no_unresolved_plain_messages
    {
        let resource = fluent_bundle::FluentResource::try_new(ENGLISH.to_owned()).unwrap();
        assert!(resource.entries().count() >= 150);
        let localizer = localizer(&[], &["en-US"]);
        for line in ENGLISH.lines().filter(|line| line.starts_with("enzui-")) {
            let (key, value) = line.split_once(" = ").unwrap();
            if !value.contains('{') {
                assert_eq!(localizer.try_text(key, None).unwrap(), value, "{key}");
            }
        }
        assert_eq!(localizer.try_text("enzui-close", None).unwrap(), "Close");
    }

    // localization_pseudo_locale_accents_and_lengthens_but_keeps_values
    {
        let pseudo = localizer(&[], &[PSEUDO_LOCALE]);
        assert_eq!(pseudo.text("enzui-close"), "[Çļöšé ~~]");
        let mut arguments = Arguments::new();
        arguments.set("title", "Report");
        let closed = pseudo.text_with("enzui-document-tab-close", &arguments);
        assert!(closed.contains("Report"), "{closed:?}");
        assert!(closed.contains("Çļöšé"), "{closed:?}");
        assert_eq!(pseudolocalize(""), "[ ~]");
    }

    // localization_coverage_lists_missing_and_obsolete_keys
    {
        let report = coverage(
            "a = A\nb = B\n    .title = T\nc = C\n-term = T",
            "a = A\nb = B\nold = O\n-other = X",
        )
        .unwrap();
        assert_eq!(report.missing, ["b.title", "c"]);
        assert_eq!(report.obsolete, ["old"]);
        assert_eq!(report.total, 4);
        assert!(!report.is_complete());
        assert!((report.percent() - 50.0).abs() < 1e-9);
        assert!(coverage("a = A", "a = {").is_err());
    }

    // localization_shipped_translations_are_complete
    {
        for (locale, report) in framework_coverage() {
            assert!(
                report.is_complete(),
                "{locale}: missing {:?}, obsolete {:?}; translate new keys in every \
             crates/enzui-core/src/localization/locales/*.ftl",
                report.missing,
                report.obsolete
            );
            // Each translation parses into the bundle the localizer uses.
            let translated = localizer(&[], &[locale]).resolve("enzui-close", None);
            assert_eq!(translated.locale.as_deref(), Some(locale), "{translated:?}");
        }
    }
}

#[test]
fn localization_time_grammar_and_translated_periods() {
    // localization_time_grammar_uses_the_locale_cycle_and_catalog_periods
    {
        use crate::time_of_day::{HourCycle, TimeOfDay};
        let localizer = localizer(
            &[("fr", "enzui-time-am = matin\nenzui-time-pm = soir")],
            &["fr-FR"],
        );
        assert_eq!(localizer.locale().hour_cycle().unwrap(), HourCycle::H23);
        let time = TimeOfDay::hm(13, 5).unwrap();
        let format = localizer.time_format().unwrap();
        assert_eq!(format.format(time), "13:05");
        assert_eq!(format.parse("13:05").unwrap(), time);
        localizer.select(vec!["fr-FR-u-hc-h12".parse().unwrap()]);
        let format = localizer.time_format().unwrap();
        assert_eq!(format.format(time), "1:05 soir");
        assert_eq!(format.parse("1:05 soir").unwrap(), time);
        localizer.select(vec!["fr-u-hc-h24".parse().unwrap()]);
        let format = localizer.time_format().unwrap();
        assert_eq!(format.format(TimeOfDay::MIDNIGHT), "24:00");
        assert_eq!(format.parse("24:00").unwrap(), TimeOfDay::MIDNIGHT);
        assert!(localizer.locale().format_time(time, false).is_err());
    }

    // localization_time_grammar_rejects_ambiguous_translated_day_periods
    {
        for labels in [
            "enzui-time-am = period\nenzui-time-pm = period",
            "enzui-time-am = M\nenzui-time-pm = m",
            "enzui-time-am = 1\nenzui-time-pm = 12",
            "enzui-time-am = m\nenzui-time-pm = 5 m",
        ] {
            let localizer = localizer(&[("en-US", labels)], &["en-US"]);
            assert!(localizer.time_format().is_err(), "{labels}");
        }
    }

    // localization_compact_hours_and_schedule_clocks_use_native_digits_and_locale_cycles
    {
        let localizer = localizer(&[("ar", "enzui-time-pm = م")], &["ar-EG"]);
        let time = TimeOfDay::hm(13, 5).unwrap();
        assert_eq!(
            localizer.format_compact_hour(time).unwrap(),
            // The catalogue pattern is `{ $hour } { $period }`, as CLDR's "h a".
            "\u{2068}١\u{2069} \u{2068}م\u{2069}"
        );
        // Outside a scope the clock follows the environment's locale.
        let unscoped = crate::localization::current()
            .locale()
            .format_time(time, false)
            .unwrap();
        assert_eq!(crate::schedule::format_localized_clock(785), unscoped);
        localizer.scope(|| {
            let clock = crate::schedule::format_localized_clock(785);
            assert!(clock.contains('١'), "{clock}");
            assert!(clock.contains('م'), "{clock}");
            assert!(!clock.contains("13:05"), "{clock}");
        });
        localizer.select(vec!["fr-FR".parse().unwrap()]);
        assert_eq!(localizer.format_compact_hour(time).unwrap(), "13");
        localizer.select(vec!["en-US-u-hc-h24".parse().unwrap()]);
        assert_eq!(
            localizer.format_compact_hour(TimeOfDay::MIDNIGHT).unwrap(),
            "24"
        );
    }
}

#[test]
fn localization_safe_errors_never_disclose_diagnostics() {
    // localization_safe_error_summaries_translate_without_disclosing_diagnostics
    {
        use crate::paged_text::{PagedTextSourceError, PagedTextSourceErrorKind};
        let localizer = localizer(
            &[(
                "fr",
                "enzui-source-permission-denied = Accès refusé\nenzui-persist-failed = Échec de l’enregistrement",
            )],
            &["fr"],
        );
        let source = PagedTextSourceError::new(
            PagedTextSourceErrorKind::Permission,
            "secret/path\nbackend error",
        );
        let persistence = crate::persist::PersistError::new("private errno payload");
        localizer.scope(|| {
            assert_eq!(source.user_summary(), "Accès refusé");
            assert_eq!(persistence.user_summary(), "Échec de l’enregistrement");
        });
        assert_eq!(source.diagnostic(), "secret/path\nbackend error");
        assert_eq!(persistence.diagnostic(), "private errno payload");
    }

    // localization_safe_error_summaries_bound_translated_characters_and_remove_controls
    {
        let resource = format!(
            "enzui-persist-failed = {{ \"\\u000A\\u0009\" }}{}",
            "é".repeat(300)
        );
        let localizer = localizer(&[("fr", &resource)], &["fr"]);
        assert!(
            localizer
                .try_text("enzui-persist-failed", None)
                .unwrap()
                .contains('\n')
        );
        localizer.scope(|| {
            let error = crate::persist::PersistError::new("private backend error");
            assert_eq!(error.user_summary(), "é".repeat(256));
            assert_eq!(error.diagnostic(), "private backend error");
        });
    }
}

#[test]
fn localization_macro_borrows_owned_and_temporary_arguments_and_keeps_plural_types() {
    let localizer = localizer(
        &[(
            "fr",
            "greeting = { $count ->\n    [one] Bonjour { $name } { $extra }\n   *[other] Autres\n}",
        )],
        &["fr"],
    );
    localizer.scope(|| {
        let name = String::from("Ada");
        let greeting = crate::localized!("greeting", "name" => name, "extra" => String::from("x"), "count" => 1);
        assert_eq!(greeting, "Bonjour \u{2068}Ada\u{2069} \u{2068}x\u{2069}");
        assert_eq!(name, "Ada");
    });
}

#[test]
fn localization_shortcuts_translate_display_without_changing_bindings_or_canonical_text() {
    use crate::shortcut::{Modifier, NamedKey, Shortcut};
    let localizer = localizer(
        &[("fr", "enzui-key-shift = Maj\nenzui-key-enter = Entrée")],
        &["fr"],
    );
    let shortcut = Shortcut::new(Modifier::CTRL | Modifier::SHIFT, NamedKey::Enter);
    localizer.scope(|| {
        assert_eq!(shortcut.localized_label(), "Ctrl+Maj+Entrée");
        assert_eq!(shortcut.to_string(), "Ctrl+Shift+Enter");
        assert!(shortcut.matches(
            &iced_core::keyboard::Key::Named(NamedKey::Enter),
            iced_core::keyboard::Modifiers::CTRL | iced_core::keyboard::Modifiers::SHIFT
        ));
    });
}

#[test]
fn localization_locale_negotiation_and_calendar_defaults_2() {
    // localization_calendar_narrow_headings_and_numerals_follow_the_locale
    {
        let localizer = localizer(&[], &["ar-EG"]);
        localizer.scope(|| {
            assert_eq!(LocalizedCalendarLabels.day_number(19), "١٩");
            assert_eq!(LocalizedCalendarLabels.year_number(2026), "٢٠٢٦");
            assert_eq!(LocalizedCalendarLabels.weekday_narrow(Weekday::Monday), "ن");
            assert!(
                LocalizedCalendarLabels
                    .weekday_short(Weekday::Monday)
                    .chars()
                    .count()
                    > 1
            );
        });
        assert_eq!(LocalizedCalendarLabels.day_number(19), "19");
    }

    // localization_desktop_clock_format_sits_between_locale_keyword_and_cldr
    {
        use crate::time_of_day::HourCycle;
        let english: Locale = "en-US".parse().unwrap();
        assert_eq!(english.hour_cycle_with(None).unwrap(), HourCycle::H12);
        assert_eq!(english.hour_cycle_with(Some(true)).unwrap(), HourCycle::H23);
        let german: Locale = "de-DE".parse().unwrap();
        assert_eq!(german.hour_cycle_with(Some(false)).unwrap(), HourCycle::H12);
        let explicit: Locale = "en-US-u-hc-h12".parse().unwrap();
        assert_eq!(
            explicit.hour_cycle_with(Some(true)).unwrap(),
            HourCycle::H12,
            "an explicit u-hc outranks the desktop"
        );
    }
}

#[test]
fn localization_invalid_argument_values_fail_without_partial_text_and_do_not_poison_later_calls() {
    let localizer = localizer(&[("fr", "value = { $value }")], &["fr"]);
    let mut arguments = Arguments::new();
    for value in [
        Value::from(f64::NAN),
        Value::from(f64::INFINITY),
        Value::Error,
    ] {
        arguments.set("value", value);
        assert!(matches!(
            localizer.try_text("value", Some(&arguments)),
            Err(LocalizationError::Format(_))
        ));
    }
    arguments.set("value", 42);
    assert_eq!(localizer.try_text("value", Some(&arguments)).unwrap(), "42");
}

#[test]
fn localization_exact_numbers_plural_selection_and_validation_2() {
    // localization_number_rounding_resolves_ties_away_from_zero
    {
        let localizer = localizer(
            &[(
                "en-US",
                "fraction = { NUMBER($number, maximumFractionDigits: 0) }\nsignificant = { NUMBER($number, maximumSignificantDigits: 2) }",
            )],
            &["en-US"],
        );
        let mut arguments = Arguments::new();
        for (number, expected) in [(2.5, "3"), (-2.5, "-3")] {
            arguments.set("number", number);
            assert_eq!(
                localizer.try_text("fraction", Some(&arguments)).unwrap(),
                expected
            );
        }
        for (number, expected) in [(2.25, "2.3"), (-2.25, "-2.3")] {
            arguments.set("number", number);
            assert_eq!(
                localizer.try_text("significant", Some(&arguments)).unwrap(),
                expected
            );
        }
    }

    // localization_ordinal_plural_rules_keep_number_selector_types
    {
        let localizer = localizer(
            &[(
                "en-US",
                "ordinal = { NUMBER($position, type: \"ordinal\") ->\n    [one] st\n    [two] nd\n    [few] rd\n   *[other] th\n}",
            )],
            &["en-US"],
        );
        let mut arguments = Arguments::new();
        for (position, expected) in [
            (1, "st"),
            (2, "nd"),
            (3, "rd"),
            (4, "th"),
            (11, "th"),
            (12, "th"),
            (13, "th"),
            (21, "st"),
            (22, "nd"),
            (23, "rd"),
        ] {
            arguments.set("position", position);
            assert_eq!(
                localizer.try_text("ordinal", Some(&arguments)).unwrap(),
                expected,
                "{position}"
            );
        }
    }
}
