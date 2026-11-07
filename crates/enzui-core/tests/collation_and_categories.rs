//! Locale-aware sorting, search folding, and the locale of each `LC_*`
//! category.

#![allow(
    missing_docs,
    clippy::unwrap_used,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking"
)]
use enzui_core::file_browser::{FileBrowserModel, FileEntry};
use enzui_core::localization::{
    Catalog, Collation, CollationStrength, Locale, LocaleCategory, Localizer,
    category_locale_from_env,
};
use std::cmp::Ordering;
use std::collections::HashMap;

fn locale(tag: &str) -> Locale {
    Locale::from_posix(tag).unwrap()
}

fn sorted(tag: &str, collation: Collation, words: &[&str]) -> Vec<String> {
    let collator = locale(tag).collator(collation).unwrap();
    let mut words: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
    collator.sort_by_key(&mut words, String::as_str);
    words
}

#[test]
fn locale_categories_control_sort_filter_and_formatting() {
    // each_language_sorts_its_own_way
    {
        let words = ["zebra", "Äpfel", "apple", "Zoo", "eclair", "éclair"];
        assert_eq!(
            sorted("en_US", Collation::default(), &words),
            ["Äpfel", "apple", "eclair", "éclair", "zebra", "Zoo"],
            "accents beside their letter (Äpfel by its f), not after z"
        );
        assert_eq!(
            sorted("sv_SE", Collation::default(), &["ö", "z", "ä", "a"]),
            ["a", "z", "ä", "ö"],
            "Swedish puts ä and ö after z"
        );
        assert_eq!(
            sorted("en_US", Collation::NATURAL, &["file10", "file2", "file1"]),
            ["file1", "file2", "file10"]
        );
        assert_eq!(
            sorted("en_US", Collation::default(), &["file10", "file2"]),
            ["file10", "file2"],
            "digits are characters without numeric ordering"
        );
        let base = locale("en_US")
            .collator(Collation {
                numeric: false,
                strength: CollationStrength::Base,
            })
            .unwrap();
        assert_eq!(base.compare("resume", "Résumé"), Ordering::Equal);
        let accents = locale("en_US")
            .collator(Collation {
                numeric: false,
                strength: CollationStrength::Accents,
            })
            .unwrap();
        assert_eq!(accents.compare("resume", "RESUME"), Ordering::Equal);
        assert_ne!(accents.compare("resume", "résumé"), Ordering::Equal);
    }

    // the_file_browser_sorts_and_filters_by_language
    {
        let mut model = FileBrowserModel::new();
        model.set_entries(
            [
                "Zebra.txt",
                "Ärende.txt",
                "Apple.txt",
                "Café.txt",
                "file10.txt",
                "file2.txt",
            ]
            .into_iter()
            .map(entry)
            .collect(),
        );
        model.set_collation_locale(&locale("en_US"));
        let names = |model: &FileBrowserModel| -> Vec<String> {
            model
                .visible()
                .iter()
                .map(|entry| entry.name.clone())
                .collect()
        };
        assert_eq!(
            names(&model),
            [
                "Apple.txt",
                "Ärende.txt",
                "Café.txt",
                "file2.txt",
                "file10.txt",
                "Zebra.txt"
            ]
        );
        model.set_collation_locale(&locale("sv_SE"));
        assert_eq!(names(&model).last().map(String::as_str), Some("Ärende.txt"));
        model.set_query("CAFE");
        assert_eq!(names(&model), ["Café.txt"], "without case or accents");
    }

    // categories_follow_lc_all_then_their_variable_then_lang
    {
        let read = env(&[("LANG", "en_US.UTF-8"), ("LC_TIME", "de_DE.UTF-8")]);
        assert_eq!(
            category_locale_from_env(LocaleCategory::Time, &read),
            Some(locale("de_DE"))
        );
        assert_eq!(
            category_locale_from_env(LocaleCategory::Numeric, &read),
            Some(locale("en_US"))
        );
        let all = env(&[("LC_ALL", "fr_FR.UTF-8"), ("LC_TIME", "de_DE.UTF-8")]);
        assert_eq!(
            category_locale_from_env(LocaleCategory::Time, &all),
            Some(locale("fr_FR"))
        );
        assert_eq!(
            category_locale_from_env(LocaleCategory::Collate, env(&[])),
            None
        );
    }

    // a_localizer_formats_with_the_category_locale
    {
        let localizer =
            Localizer::new(Catalog::default(), vec![locale("en_US")], Locale::default());
        assert_eq!(
            localizer.locale_for(LocaleCategory::Numeric),
            locale("en_US"),
            "follows the UI"
        );
        let before = localizer.generation();
        assert!(localizer.set_category_locale(LocaleCategory::Numeric, Some(locale("de_DE"))));
        assert!(localizer.generation() > before, "views rebuild");
        assert!(!localizer.set_category_locale(LocaleCategory::Numeric, Some(locale("de_DE"))));
        let numeric = localizer.locale_for(LocaleCategory::Numeric);
        assert_eq!(
            numeric.format_decimal(&"1234.5".parse().unwrap()).unwrap(),
            "1.234,5"
        );
        assert_eq!(localizer.locale_for(LocaleCategory::Time), locale("en_US"));
        assert!(localizer.set_category_locale(LocaleCategory::Numeric, None));
        assert_eq!(
            localizer.locale_for(LocaleCategory::Numeric),
            locale("en_US")
        );
    }
}

fn entry(name: &str) -> FileEntry {
    FileEntry::file(format!("/tmp/{name}"), name, 1)
}

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    move |key| map.get(key).cloned()
}
