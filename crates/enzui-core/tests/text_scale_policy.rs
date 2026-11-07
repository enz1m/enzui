//! The shared text-scale policy: which layer wins, what a file delivers, and
//! what an unusable or absent backend leaves the application with.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use enzui_core::theme_file::ThemeDocument;
use enzui_core::theme_file::document::DiagnosticKind;
use enzui_theme::EnzThemeMode;

fn parse(text: &str) -> ThemeDocument {
    ThemeDocument::parse(text).expect("valid TOML")
}

#[test]
fn an_unusable_file_text_scale_is_refused_and_leaves_the_ramp_at_its_pins() {
    for (value, kind) in [
        ("9.0", DiagnosticKind::OutOfRange),
        ("0", DiagnosticKind::OutOfRange),
        ("nan", DiagnosticKind::NotFinite),
        ("\"large\"", DiagnosticKind::WrongType),
    ] {
        let doc = parse(&format!(
            "version = 1\n[typography]\nbody_size = 16\ntext_scale = {value}\n"
        ));
        let kinds: Vec<DiagnosticKind> = doc.diagnostics().iter().map(|d| d.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![kind],
            "`text_scale = {value}` reported {kinds:?}"
        );

        let theme = doc.resolve(EnzThemeMode::Dark);
        assert!(
            theme.text_scale().is_baseline(),
            "`text_scale = {value}` moved the ramp to {}",
            theme.text_scale().factor()
        );
        assert_eq!(theme.typography.body_size, 16.0);
    }
}
