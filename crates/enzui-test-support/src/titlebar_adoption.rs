//! Shared check that an app builds its titlebar the framework way.
//!
//! The framework owns the titlebar's height, its side padding and the window
//! controls, and gives commands a folding `.actions(..)` slot. An app that
//! restates those by hand drifts from every other app the day the framework
//! changes them. [`scan_titlebar_adoption`] walks an app's production sources
//! and reports each place that does; an app's test asserts the report is empty
//! apart from deliberate, explained exceptions:
//!
//! ```no_run
//! use enzui_test_support::titlebar_adoption::{TitlebarRule, assert_titlebar_adoption};
//!
//! let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
//! assert_titlebar_adoption(
//!     &src,
//!     &[(
//!         "view/player.rs",
//!         TitlebarRule::PinnedHeight,
//!         "the full-screen player's band is taller to hold the 44 px transport",
//!     )],
//! );
//! ```
//!
//! The scan is lexical: it reads code with comments and string contents
//! masked, so commented-out code never counts and a rule's name inside a
//! string never matches.

use std::path::Path;

use crate::hand_roll_scan::{inline_test_module_spans, mask_rust_non_code, production_rs_files};

/// What a finding says the app restated by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TitlebarRule {
    /// A titlebar height chosen by the app: `titlebar_height` on the shell's
    /// tokens, or `.height(..)` on an `enz_title_bar()`. The shell's default
    /// is the one height every app shares.
    PinnedHeight,
    /// `.padding_x(..)` on an `enz_title_bar()`: the band's side padding is
    /// the framework's.
    PinnedPadding,
    /// `.show_close(false)` on a bar: inside a shell that draws the window
    /// controls a bar draws none, so this is no longer needed.
    ShowCloseFalse,
    /// `.aria_label(x).tooltip(x)` with the same text on a toolbar icon:
    /// `.label(x)` sets both.
    LabelTwice,
    /// Three or more buttons pushed into one `enz_title_bar()`: commands
    /// belong in `.actions(..)`, which folds them into a menu when the window
    /// is narrow instead of squeezing the title.
    PushedCommands,
}

/// One place an app restated what the framework owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitlebarFinding {
    /// The file, relative to the scanned `src` root, `/`-separated.
    pub file: String,
    /// The 1-based line of the call.
    pub line: usize,
    /// What was restated.
    pub rule: TitlebarRule,
}

/// The findings of one scan and how much it read.
#[derive(Debug, Clone, Default)]
pub struct TitlebarReport {
    /// Production source files read.
    pub files: usize,
    /// `enz_title_bar()` calls found, the population the bar rules apply to.
    pub bars: usize,
    /// Everything found, in file and line order.
    pub findings: Vec<TitlebarFinding>,
}

/// Scan every production `.rs` file under `src_root`.
pub fn scan_titlebar_adoption(src_root: &Path) -> TitlebarReport {
    let mut report = TitlebarReport::default();
    for rel in production_rs_files(src_root) {
        let Ok(source) = std::fs::read_to_string(src_root.join(&rel)) else {
            continue;
        };
        report.files += 1;
        let (bars, findings) = scan_source(&rel, &source);
        report.bars += bars;
        report.findings.extend(findings);
    }
    report
        .findings
        .sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    report
}

/// Scan one file's source, returning how many `enz_title_bar()` calls it has
/// and its findings.
pub fn scan_source(file: &str, source: &str) -> (usize, Vec<TitlebarFinding>) {
    let mut code = mask_rust_non_code(source).into_bytes();
    for (start, end) in inline_test_module_spans(source) {
        for byte in &mut code[start..end.min(source.len())] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }
    let code = String::from_utf8(code).expect("masking keeps UTF-8");
    let line_of = |at: usize| source[..at].matches('\n').count() + 1;
    let mut findings = Vec::new();
    let mut found = |at: usize, rule| {
        findings.push(TitlebarFinding {
            file: file.to_owned(),
            line: line_of(at),
            rule,
        });
    };

    for at in matches(&code, "titlebar_height") {
        let rest = code[at + "titlebar_height".len()..].trim_start();
        if rest.starts_with(':') && !rest.starts_with("::") || rest.starts_with('(') {
            found(at, TitlebarRule::PinnedHeight);
        }
    }
    for at in matches(&code, ".show_close(") {
        let arg = call_argument(&code, source, at + ".show_close".len());
        if arg.as_deref().map(str::trim) == Some("false") {
            found(at, TitlebarRule::ShowCloseFalse);
        }
    }

    let mut bars = 0;
    for at in matches(&code, "enz_title_bar()") {
        bars += 1;
        let chain = method_chain(&code, source, at + "enz_title_bar()".len());
        if chain.iter().any(|(name, _, _)| name == "height") {
            found(at, TitlebarRule::PinnedHeight);
        }
        if chain.iter().any(|(name, _, _)| name == "padding_x") {
            found(at, TitlebarRule::PinnedPadding);
        }
        let pushed = chain
            .iter()
            .filter(|(name, _, arg)| name == "push" && is_button(arg))
            .count();
        if pushed >= 3 {
            found(at, TitlebarRule::PushedCommands);
        }
    }

    for at in matches(&code, "enz_toolbar_icon(") {
        let open = at + "enz_toolbar_icon".len();
        let Some(close) = matching_paren(&code, open) else {
            continue;
        };
        let chain = method_chain(&code, source, close + 1);
        let arg = |wanted: &str| {
            chain
                .iter()
                .find(|(name, _, _)| name == wanted)
                .map(|(_, _, arg)| normalized(arg))
        };
        if let (Some(name), Some(tip)) = (arg("aria_label"), arg("tooltip"))
            && name == tip
        {
            let at_label = chain
                .iter()
                .find(|(name, _, _)| name == "aria_label")
                .map_or(at, |(_, at, _)| *at);
            found(at_label, TitlebarRule::LabelTwice);
        }
    }

    (bars, findings)
}

/// Assert `src_root`'s titlebars follow the framework, apart from `allowed`
/// exceptions: `(file, rule, reason)`, the file relative to `src_root`. Every
/// exception must still be found, so a fixed one is removed from the list,
/// and must say why it exists.
pub fn assert_titlebar_adoption(src_root: &Path, allowed: &[(&str, TitlebarRule, &str)]) {
    let report = scan_titlebar_adoption(src_root);
    assert!(
        report.files > 0 && report.bars + report.findings.len() > 0,
        "the titlebar scan read {} files under {} and found no titlebar: \
         is the source root right?",
        report.files,
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
        "titlebar code that restates what the framework owns (see \
         enzui_test_support::titlebar_adoption::TitlebarRule):\n{}",
        unexplained
            .iter()
            .map(|f| format!("  {}:{} {:?}", f.file, f.line, f.rule))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Byte offsets of `needle` in `code`, at identifier boundaries.
fn matches<'c>(code: &'c str, needle: &'c str) -> impl Iterator<Item = usize> + 'c {
    code.match_indices(needle)
        .map(|(at, _)| at)
        .filter(move |&at| {
            let before = code[..at].chars().next_back();
            let word = |c: char| c.is_alphanumeric() || c == '_';
            !needle.starts_with(|c: char| word(c)) || !before.is_some_and(word)
        })
}

/// The index of the `)` closing the `(` at `open`, in masked code.
fn matching_paren(code: &str, open: usize) -> Option<usize> {
    let bytes = code.as_bytes();
    if bytes.get(open) != Some(&b'(') {
        return None;
    }
    let mut depth = 0usize;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// The source text of the call argument whose `(` follows `at` in `code`.
fn call_argument(code: &str, source: &str, at: usize) -> Option<String> {
    let open = at + code[at..].len() - code[at..].trim_start().len();
    let close = matching_paren(code, open)?;
    Some(source[open + 1..close].to_owned())
}

/// The `.name(arg)` calls chained from `at`: name, offset and the argument's
/// source text.
fn method_chain(code: &str, source: &str, mut at: usize) -> Vec<(String, usize, String)> {
    let mut chain = Vec::new();
    loop {
        let rest = &code[at..];
        let trimmed = rest.trim_start();
        let Some(after_dot) = trimmed.strip_prefix('.') else {
            break;
        };
        let dot = at + (rest.len() - trimmed.len());
        let name: String = after_dot
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() {
            break;
        }
        let open = dot + 1 + name.len();
        if code.as_bytes().get(open) != Some(&b'(') {
            break;
        }
        let Some(close) = matching_paren(code, open) else {
            break;
        };
        chain.push((name, dot, source[open + 1..close].to_owned()));
        at = close + 1;
    }
    chain
}

/// Whether a pushed argument is a button.
fn is_button(arg: &str) -> bool {
    [
        "enz_toolbar_icon(",
        "icon_button(",
        "enz_button(",
        "button(",
    ]
    .iter()
    .any(|call| arg.contains(call))
}

/// An argument with whitespace and a trailing `.clone()` dropped, so
/// `label.clone()` and `label` compare equal.
fn normalized(arg: &str) -> String {
    let squeezed: String = arg.split_whitespace().collect();
    squeezed
        .strip_suffix(".clone()")
        .unwrap_or(&squeezed)
        .to_owned()
}
