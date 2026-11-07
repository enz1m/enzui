//! Lists the messages each translation is missing or keeps after English
//! dropped them.
//!
//! ```sh
//! # The framework's own translations:
//! cargo run --bin enzui-l10n-coverage
//! # An app's catalogs: the reference first, then each translation.
//! cargo run --bin enzui-l10n-coverage -- locales/en-US.ftl locales/de.ftl locales/fr.ftl
//! ```
//!
//! Exits with status 1 when any translation is incomplete.

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a command-line report prints"
)]

use enzui_core::localization::{Coverage, coverage, framework_coverage};

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    let reports: Vec<(String, Coverage)> = match paths.split_first() {
        None => framework_coverage()
            .into_iter()
            .map(|(locale, report)| (locale.to_owned(), report))
            .collect(),
        Some((reference, translations)) => {
            let read = |path: &str| {
                std::fs::read_to_string(path).unwrap_or_else(|error| {
                    eprintln!("{path}: {error}");
                    std::process::exit(2)
                })
            };
            let reference = read(reference);
            translations
                .iter()
                .map(|path| {
                    let report = coverage(&reference, &read(path)).unwrap_or_else(|error| {
                        eprintln!("{path}: {error}");
                        std::process::exit(2)
                    });
                    (path.clone(), report)
                })
                .collect()
        }
    };
    let mut complete = true;
    for (name, report) in &reports {
        println!(
            "{name}: {:.1}% of {} messages",
            report.percent(),
            report.total
        );
        for key in &report.missing {
            println!("  missing  {key}");
        }
        for key in &report.obsolete {
            println!("  obsolete {key}");
        }
        complete &= report.is_complete();
    }
    if !complete {
        std::process::exit(1);
    }
}
