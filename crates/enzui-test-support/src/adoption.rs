//! Shared check that an app uses the framework's everyday plumbing instead
//! of its own copy.
//!
//! Each [`AdoptionRule`] names a piece of plumbing apps used to write for
//! themselves and the framework API that replaces it. [`scan_adoption`]
//! walks an app's production sources and reports where the old shape still
//! appears; an app's test asserts the report is empty apart from explained
//! exceptions with [`assert_adoption`]:
//!
//! ```no_run
//! use enzui_test_support::adoption::{AdoptionRule, assert_adoption};
//!
//! let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
//! assert_adoption(
//!     &src,
//!     &[(
//!         "editor/drag.rs",
//!         AdoptionRule::NativeDropRuntime,
//!         "drags its own `application/x-notes-card` type between windows",
//!     )],
//! );
//! ```
//!
//! The scan reads code with comments and string contents masked, and skips
//! `#[cfg(test)]` modules, like [`crate::titlebar_adoption`].

use std::path::Path;

use crate::hand_roll_scan::{inline_test_module_spans, mask_rust_non_code, production_rs_files};

/// A piece of plumbing the framework provides, found written by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AdoptionRule {
    /// A file chooser driven through `PendingRequest` / `DbusBackend`
    /// instead of `file_chooser::ChooserSession`.
    ChooserPlumbing,
    /// An app's own message for each `PortalOutcome` of a file chooser
    /// instead of `file_chooser::failure_text`.
    ChooserFailureText,
    /// `NativeDragRuntime::connect` in app code: for dropped files use
    /// `file_drop::enz_file_drop_target`, which keeps text drag working.
    NativeDropRuntime,
    /// An app-global `Localizer` (a `static`, `LazyLock` or `OnceLock`)
    /// instead of `AppServices::localizer`.
    LocalizerGlobal,
    /// `ModalFocusLifecycle` bookkeeping in app code instead of
    /// `modal_host::DialogStack`.
    ModalFocusBookkeeping,
    /// `ApplicationService::connect_or_forward*` in app code instead of
    /// `single_instance::SingleInstance`.
    SingleInstanceGlue,
    /// A context menu's string entry ids applied by hand
    /// (`context_menu_host::apply_event*`) instead of `ContextMenu<Target,
    /// Command>`.
    ContextMenuIds,
}

impl AdoptionRule {
    /// Every rule, with the code shape that finds it.
    const PATTERNS: [(Self, &'static str); 10] = [
        (Self::ChooserPlumbing, "PendingRequest"),
        (Self::ChooserPlumbing, "DbusBackend::"),
        (Self::ChooserFailureText, "PortalOutcome::Unavailable =>"),
        (Self::NativeDropRuntime, "NativeDragRuntime::connect"),
        (Self::LocalizerGlobal, "LazyLock<Localizer>"),
        (Self::LocalizerGlobal, "OnceLock<Localizer>"),
        (Self::ModalFocusBookkeeping, "ModalFocusLifecycle::open"),
        (Self::SingleInstanceGlue, "connect_or_forward"),
        (Self::ContextMenuIds, "apply_event_restoring("),
        (Self::ContextMenuIds, "context_menu_host::apply_event("),
    ];
}

/// One place an app wrote its own plumbing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptionFinding {
    /// The file, relative to the scanned `src` root, `/`-separated.
    pub file: String,
    /// The 1-based line.
    pub line: usize,
    /// What was found.
    pub rule: AdoptionRule,
}

/// The findings of one scan and how many files it read.
#[derive(Debug, Clone, Default)]
pub struct AdoptionReport {
    /// Production source files read.
    pub files: usize,
    /// Everything found, in file and line order.
    pub findings: Vec<AdoptionFinding>,
}

/// Scan every production `.rs` file under `src_root`.
pub fn scan_adoption(src_root: &Path) -> AdoptionReport {
    let mut report = AdoptionReport::default();
    for rel in production_rs_files(src_root) {
        let Ok(source) = std::fs::read_to_string(src_root.join(&rel)) else {
            continue;
        };
        report.files += 1;
        report.findings.extend(scan_source(&rel, &source));
    }
    report
        .findings
        .sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    report
}

/// Scan one file's source.
pub fn scan_source(file: &str, source: &str) -> Vec<AdoptionFinding> {
    let mut code = mask_rust_non_code(source).into_bytes();
    for (start, end) in inline_test_module_spans(source) {
        for byte in &mut code[start..end.min(source.len())] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }
    let code = String::from_utf8(code).expect("masking keeps UTF-8");
    let mut findings = Vec::new();
    for (rule, pattern) in AdoptionRule::PATTERNS {
        for (at, _) in code.match_indices(pattern) {
            let line = source[..at].matches('\n').count() + 1;
            let duplicate = findings
                .iter()
                .any(|f: &AdoptionFinding| f.rule == rule && f.line == line);
            if !duplicate {
                findings.push(AdoptionFinding {
                    file: file.to_owned(),
                    line,
                    rule,
                });
            }
        }
    }
    // A `static` localizer is a global too.
    for (at, _) in code.match_indices("static ") {
        let line_end = code[at..].find('\n').map_or(code.len(), |end| at + end);
        if code[at..line_end].contains("Localizer") {
            let line = source[..at].matches('\n').count() + 1;
            if !findings
                .iter()
                .any(|f| f.rule == AdoptionRule::LocalizerGlobal && f.line == line)
            {
                findings.push(AdoptionFinding {
                    file: file.to_owned(),
                    line,
                    rule: AdoptionRule::LocalizerGlobal,
                });
            }
        }
    }
    findings.sort_by_key(|f| f.line);
    findings
}

/// Assert `src_root` uses the framework's plumbing, apart from `allowed`
/// exceptions: `(file, rule, reason)`, the file relative to `src_root`.
/// Every exception must still be found and must say why it exists.
pub fn assert_adoption(src_root: &Path, allowed: &[(&str, AdoptionRule, &str)]) {
    let report = scan_adoption(src_root);
    assert!(
        report.files > 0,
        "the adoption scan read no files under {}: is the source root right?",
        src_root.display()
    );
    for (file, rule, reason) in allowed {
        assert!(
            !reason.trim().is_empty(),
            "the exception for {rule:?} in {file} needs a reason"
        );
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.file == *file && f.rule == *rule),
            "the exception for {rule:?} in {file} matches nothing; remove it"
        );
    }
    let unexplained: Vec<_> = report
        .findings
        .iter()
        .filter(|f| {
            !allowed
                .iter()
                .any(|(file, rule, _)| f.file == *file && f.rule == *rule)
        })
        .collect();
    assert!(
        unexplained.is_empty(),
        "plumbing the framework provides, written by hand (see \
         enzui_test_support::adoption::AdoptionRule):\n{}",
        unexplained
            .iter()
            .map(|f| format!("  {}:{} {:?}", f.file, f.line, f.rule))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
