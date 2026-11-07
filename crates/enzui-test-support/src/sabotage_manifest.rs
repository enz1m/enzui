//! Source-derived non-vacuity checks for application `sabotage.toml` files.
//!
//! Cargo test names follow the *declared* Rust module tree, not source-file
//! names. This scanner starts at `src/main.rs`/`src/lib.rs` and follows normal
//! out-of-line modules, `#[path = "…"] mod …;`, textual `include!`s, and inline
//! modules. Raw identifiers are normalized to their executable names, and
//! spaced or recursively nested `cfg_attr(..., test)` metadata uses the same
//! lexer. Comments and literal contents are opaque, so fixture text cannot
//! manufacture a live test.
//!
//! # Fail-closed guarantees
//!
//! The scanner exists to catch dead sabotage evidence, so every recognised
//! shape it cannot resolve panics with the offending source path and line:
//!
//! * `#[path = "…"]` whose value is not a single string literal.
//! * `include!(…)` whose argument is not a single string literal (e.g.
//!   `include!(concat!(…))`) — the sabotage runner cannot walk into a
//!   macro-computed fragment.
//! * An out-of-line `mod foo;` that resolves to neither `foo.rs` nor
//!   `foo/mod.rs` (after applying an explicit `#[path]` when present).
//! * Two module declarations that claim the same logical module within one
//!   entry graph. Occurrences are checked before source paths become a set, so
//!   an identical repeated `mod foo;` cannot disappear through deduplication.
//! * An unclosed block comment, string, raw string, Rust attribute, or a
//!   malformed `cfg_attr` meta list that would otherwise truncate a declaration
//!   inventory into a clean-looking prefix.
//!
//! # Supported resolution boundary
//!
//! `#[path]` is only supported at the top level of the containing file. No
//! audited crate places `#[path]` inside an inline `mod {…}` block, so
//! implementing rustc's inline-mod `dir_path` machinery (which pushes the
//! containing file's stem *and* every inline mod name onto the resolution
//! base) is deferred to the day a crate needs it.
//! `combined_inline_and_path_module_is_rejected_as_unsupported` fixtures
//! the boundary; the scanner panics with a clear message if the shape ever
//! appears.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// Location of a source-derived live `#[test]` function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestSite {
    /// The source file.
    pub path: PathBuf,
    /// The 1-based line of the `fn`.
    pub line: usize,
}

/// Result of comparing every manifest filter with the live source tree.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ManifestAudit {
    /// Live tests found in the source tree.
    pub live_test_count: usize,
    /// Records in the manifest.
    pub record_count: usize,
    /// Filters that match no live test.
    pub stale: Vec<String>,
    /// Filters that match more than one test.
    pub ambiguous: Vec<String>,
}

impl ManifestAudit {
    /// Whether every filter names exactly one live test.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.stale.is_empty() && self.ambiguous.is_empty()
    }
}

/// Audit `manifest_dir/sabotage.toml` against tests reachable from
/// `manifest_dir/src/{main,lib}.rs`.
#[must_use]
pub fn audit_sabotage_manifest(manifest_dir: &Path) -> ManifestAudit {
    let manifest = fs::read_to_string(manifest_dir.join("sabotage.toml"))
        .unwrap_or_else(|error| panic!("read {}/sabotage.toml: {error}", manifest_dir.display()));
    let records = parse_manifest_tests(&manifest);
    let live = collect_live_tests(&manifest_dir.join("src"));
    evaluate(&records, &live)
}

/// Assert that every manifest test filter substring-matches exactly one live
/// test. `minimum_live_tests` is a scanner non-vacuity floor chosen by the app.
///
/// Panics when the manifest has no records, when the source scan found fewer
/// than `minimum_live_tests` live tests, or when any record is stale or
/// ambiguous.
pub fn assert_sabotage_manifest(manifest_dir: &Path, minimum_live_tests: usize) {
    let audit = audit_sabotage_manifest(manifest_dir);
    assert!(
        audit.record_count > 0,
        "sabotage.toml appears to have no records at all — parser bug or empty manifest"
    );
    assert!(
        audit.live_test_count >= minimum_live_tests,
        "scanner found only {} #[test] fns under {}/src (expected at least {}) — scanner is broken",
        audit.live_test_count,
        manifest_dir.display(),
        minimum_live_tests
    );
    if !audit.is_clean() {
        let mut message = String::from("sabotage.toml lists dead or ambiguous test paths\n");
        if !audit.stale.is_empty() {
            message.push_str("stale paths (PRESCRIPTION_STALE_PATH):\n");
            message.push_str(&audit.stale.join("\n"));
            message.push('\n');
        }
        if !audit.ambiguous.is_empty() {
            message.push_str("ambiguous test paths (AMBIGUOUS_FILTER):\n");
            message.push_str(&audit.ambiguous.join("\n"));
            message.push('\n');
        }
        panic!("{message}");
    }
}

#[derive(Debug)]
struct Record {
    id: String,
    test_path: String,
    line: usize,
}

fn evaluate(records: &[Record], live: &BTreeMap<String, Vec<TestSite>>) -> ManifestAudit {
    let mut audit = ManifestAudit {
        live_test_count: live.values().map(Vec::len).sum(),
        record_count: records.len(),
        ..ManifestAudit::default()
    };
    for record in records {
        let matches = live
            .iter()
            .flat_map(|(name, sites)| sites.iter().map(move |site| (name, site)))
            .filter(|(name, _)| name.contains(&record.test_path))
            .collect::<Vec<_>>();
        match matches.len() {
            0 => audit.stale.push(format!(
                "  * sabotage.toml:{} id={:?} → `{}` matches no live #[test] (substring)",
                record.line, record.id, record.test_path
            )),
            1 => {}
            count => {
                let locations = matches
                    .iter()
                    .map(|(name, site)| format!("{name} at {}:{}", site.path.display(), site.line))
                    .collect::<Vec<_>>()
                    .join("; ");
                audit.ambiguous.push(format!(
                    "  * sabotage.toml:{} id={:?} → `{}` substring-matches {count} live tests: {locations}",
                    record.line, record.id, record.test_path
                ));
            }
        }
    }
    audit
}

fn parse_manifest_tests(manifest: &str) -> Vec<Record> {
    let mut records = Vec::new();
    let mut in_record = false;
    let mut id = None;
    let mut record_line = 0;
    for (index, raw) in manifest.lines().enumerate() {
        let line = raw.trim_start();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if line == "[[sabotage]]" {
            in_record = true;
            id = None;
            record_line = index + 1;
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_record = false;
            continue;
        }
        if !in_record {
            continue;
        }
        if let Some(value) = line.strip_prefix("id = ") {
            id = Some(toml_string(value));
        } else if let Some(value) = line.strip_prefix("test = ") {
            records.push(Record {
                id: id
                    .clone()
                    .unwrap_or_else(|| format!("<no id, line {record_line}>")),
                test_path: toml_string(value),
                line: index + 1,
            });
        }
    }
    records
}

fn toml_string(value: &str) -> String {
    let value = value.trim();
    if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        return inner
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
            .replace("\\n", "\n")
            .replace("\\t", "\t");
    }
    value
        .strip_prefix('\'')
        .and_then(|v| v.strip_suffix('\''))
        .unwrap_or(value)
        .to_owned()
}

/// Collect tests reachable through the crate's real source module graph.
///
/// Panics with a source path and line number on any recognised but
/// unresolvable shape — see the module docs for the fail-closed list.
#[must_use]
pub fn collect_live_tests(src_root: &Path) -> BTreeMap<String, Vec<TestSite>> {
    let mut scanner = Scanner::default();
    for root in ["lib.rs", "main.rs"] {
        let path = src_root.join(root);
        if path.is_file() {
            scanner.scan_entry(path, Vec::new(), src_root.to_path_buf(), Vec::new());
        }
    }
    scanner.tests
}

/// Collect executable `#[test]` function declarations from one source file.
///
/// This is the file-local projection of the same declaration recognizer used
/// by [`collect_live_tests`] and [`scan_source_graph`]. It deliberately does
/// not follow out-of-line `mod` declarations: callers that already own a
/// source-derived file inventory can count and name declarations in each exact
/// file without counting a child graph more than once.
///
/// Each site includes its exact source path and line. Comments and every Rust
/// string form are opaque. Missing, unreadable, or malformed source fails
/// closed with the path rather than returning a clean-looking empty list.
#[must_use]
pub fn collect_declared_tests(path: &Path) -> Vec<TestSite> {
    collect_declared_test_records(path)
        .into_iter()
        .map(|record| record.site)
        .collect()
}

/// Collect named executable `#[test]` records from one source file.
///
/// This is the rich arm of [`collect_declared_tests`]: each record additionally
/// carries its file-local inline-module path and stacked `#[cfg]` / `#[ignore]`
/// attributes for validators that need more than a declaration count.
#[must_use]
pub fn collect_declared_test_records(path: &Path) -> Vec<TestRecord> {
    let source =
        fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    declared_tests_in_source(&source, path)
}

/// Parse executable `#[test]` declarations from in-memory Rust source.
///
/// This is the fixture-facing arm of [`collect_declared_tests`]. `path` is the
/// source identity used in every returned site and malformed-input diagnostic;
/// callers reading real files should use [`collect_declared_tests`] so missing
/// and unreadable inputs also fail closed.
#[must_use]
pub fn declared_tests_in_source(source: &str, path: &Path) -> Vec<TestRecord> {
    recognize_declared_tests(source, path).records
}

#[derive(Debug, Default)]
struct DeclaredTestScan {
    records: Vec<TestRecord>,
    macro_template_tests: usize,
}

/// Canonical file-local executable-test recognizer.
///
/// Graph walking and module resolution are separate concerns: the graph
/// scanner calls this once per reached file and only prefixes the returned
/// names/attributes with the owning out-of-line module path. Keeping the
/// recognition here means file-local census gates and graph consumers cannot
/// drift on attribute stacking, comments, or literals.
fn recognize_declared_tests(source: &str, path: &Path) -> DeclaredTestScan {
    let tokens = lex(source, path);
    let mut scan = DeclaredTestScan::default();
    let mut stack: Vec<(String, i32)> = Vec::new();
    let mut module_attributes: Vec<(i32, Vec<String>)> = Vec::new();
    let mut pending_attributes = Vec::new();
    let mut pending_test = false;
    let mut depth = 0i32;
    let mut index = 0;

    while index < tokens.len() {
        if is_other_attribute(&tokens, index) {
            let end = attribute_end(&tokens, index, path, source);
            let inner = matches!(
                tokens.get(index + 1).map(|token| &token.kind),
                Some(TokenKind::Punct(b'!'))
            );
            if !inner {
                let is_test = attribute_has_active_meta(&tokens, index, end, "test", path, source);
                pending_test |= is_test;
                // A plain `#[test]` is the declaration marker, not a
                // runnability modifier. Keep cfg_attr verbatim because one
                // wrapper can also emit `ignore` or `cfg` metadata.
                if !is_test || !test_attribute(&tokens, index, end) {
                    pending_attributes.push(attribute_text(source, &tokens, index, end));
                }
            }
            index = end;
            continue;
        }

        match &tokens[index].kind {
            TokenKind::Punct(b'{') => {
                depth += 1;
                pending_test = false;
                pending_attributes.clear();
            }
            TokenKind::Punct(b'}') => {
                depth -= 1;
                while stack.last().is_some_and(|(_, open)| depth < *open) {
                    stack.pop();
                }
                while module_attributes
                    .last()
                    .is_some_and(|(open, _)| depth < *open)
                {
                    module_attributes.pop();
                }
                pending_test = false;
                pending_attributes.clear();
            }
            TokenKind::Ident(word) if word == "fn" && pending_test => {
                if let Some(name) = ident_at(&tokens, index + 1) {
                    let mut name_path: Vec<String> =
                        stack.iter().map(|(name, _)| name.clone()).collect();
                    name_path.push(name.to_owned());
                    let mut attributes = Vec::new();
                    for (_, module) in &module_attributes {
                        attributes.extend(module.iter().cloned());
                    }
                    attributes.extend(pending_attributes.iter().cloned());
                    scan.records.push(TestRecord {
                        name: name_path.join("::"),
                        site: TestSite {
                            path: path.to_path_buf(),
                            line: line_for(source, tokens[index].offset),
                        },
                        attributes,
                        source_span: function_span(&tokens, index, path, source),
                    });
                } else {
                    // `fn $name()` inside a `macro_rules!` body.
                    scan.macro_template_tests += 1;
                }
                pending_test = false;
                pending_attributes.clear();
            }
            TokenKind::Ident(word) if word == "mod" => {
                if let (Some(name), Some(TokenKind::Punct(b'{'))) = (
                    ident_at(&tokens, index + 1),
                    tokens.get(index + 2).map(|token| &token.kind),
                ) {
                    stack.push((name.to_owned(), depth + 1));
                    module_attributes.push((depth + 1, std::mem::take(&mut pending_attributes)));
                }
                pending_test = false;
                pending_attributes.clear();
            }
            _ if is_item_qualifier(&tokens[index]) => {}
            _ => {
                pending_test = false;
                pending_attributes.clear();
            }
        }
        index += 1;
    }
    scan
}

/// One `#[test] fn` with the attributes that decide whether it runs.
///
/// `attributes` holds the verbatim attribute text (`#[ignore = "…"]`,
/// `#[cfg(not(debug_assertions))]`, …) attached to the function *and* to every
/// module on the path that declared it, so a caller can decide liveness
/// without re-lexing the tree. Interpretation is deliberately left to the
/// caller: this module knows how to read Rust, not what a profile means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRecord {
    /// The function name.
    pub name: String,
    /// Where it is declared.
    pub site: TestSite,
    /// Its attributes and those of the modules declaring it, verbatim.
    pub attributes: Vec<String>,
    /// Byte range of the function declaration and body in `site.path`.
    ///
    /// File-local consumers can use this to inspect the actual test body
    /// without searching from one textual `#[test]` spelling to the next.
    pub source_span: Range<usize>,
}

impl TestRecord {
    /// The effective `ignore` reason, including an empty string for a bare
    /// `#[ignore]`. Active, recursively nested `cfg_attr` metadata is handled
    /// by the same lexer that recognized this test declaration.
    #[must_use]
    pub fn ignore_reason(&self) -> Option<String> {
        self.effective_attribute_bodies("ignore")
            .into_iter()
            .next()
            .map(|body| {
                let value = body.trim().trim_start_matches('=').trim();
                let tokens = lex(value, &self.site.path);
                match tokens.first().map(|token| &token.kind) {
                    Some(TokenKind::String(reason)) => reason.clone(),
                    _ => String::new(),
                }
            })
    }

    /// Bodies of effective attributes named `name` (`"(test)"` for
    /// `#[cfg(test)]`, `""` for a bare attribute).
    #[must_use]
    pub fn effective_attribute_bodies(&self, name: &str) -> Vec<String> {
        self.attributes
            .iter()
            .flat_map(|attribute| effective_attribute_bodies(attribute, name, &self.site.path))
            .collect()
    }
}

/// Everything one entry file reaches: the source files that compile into it
/// and the `#[test]` functions they declare.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SourceGraph {
    /// Every source file compiled into the entry, entry included.
    pub files: BTreeSet<PathBuf>,
    /// The `#[test]` functions those files declare.
    pub tests: Vec<TestRecord>,
    /// `#[test]` declarations inside a `macro_rules!` body, whose function
    /// name is a macro fragment (`fn $kb()`) rather than an identifier.
    ///
    /// They are declarations no filter can name: only the expansions exist in
    /// the harness, under names this walker cannot compute. Counting them
    /// separately is what lets a textual `#[test]` census be reconciled with
    /// the graph instead of quietly disagreeing with it.
    pub macro_template_tests: usize,
    /// Every outer item attribute in the graph, verbatim and in source order.
    ///
    /// Lexed, so a `#[global_allocator]` written inside a comment or a string
    /// literal cannot appear here — the process-isolation classification of a
    /// target must not be forgeable by prose.
    pub attributes: Vec<String>,
}

/// Walk the module graph rooted at one entry file (`tests/<name>.rs`,
/// `tests/<name>/main.rs`, `src/lib.rs`, …).
///
/// This is the same walker, lexer and `#[path]`/`include!` resolution the
/// sabotage-manifest audit uses; a second scanner would be a second set of
/// blind spots. Fail-closed behaviour is identical (see the module docs).
///
/// # Panics
///
/// If `entry` is not a readable file, or any recognised module shape cannot be
/// resolved.
#[must_use]
pub fn scan_source_graph(entry: &Path) -> SourceGraph {
    assert!(
        entry.is_file(),
        "scan_source_graph({}): entry file does not exist; a graph derived \
         from a missing root would report an empty, clean-looking tree",
        entry.display()
    );
    let mut scanner = Scanner::default();
    let module_dir = entry
        .parent()
        .expect("entry file has a parent directory")
        .to_path_buf();
    scanner.scan_entry(entry.to_path_buf(), Vec::new(), module_dir, Vec::new());
    SourceGraph {
        files: scanner.files,
        tests: scanner.records,
        attributes: scanner.attributes,
        macro_template_tests: scanner.macro_template_tests,
    }
}

#[derive(Default)]
struct Scanner {
    tests: BTreeMap<String, Vec<TestSite>>,
    records: Vec<TestRecord>,
    files: BTreeSet<PathBuf>,
    attributes: Vec<String>,
    macro_template_tests: usize,
    active: BTreeSet<PathBuf>,
    /// Module ownership is per entry (one Cargo target), not per physical
    /// source file. Keep parsed occurrences until they have been checked:
    /// converting them into a set first would make `mod leaf; mod leaf;` look
    /// exactly like one registration.
    module_registrations: BTreeMap<String, TestSite>,
    current_entry: Option<PathBuf>,
}

impl Scanner {
    fn scan_entry(
        &mut self,
        path: PathBuf,
        prefix: Vec<String>,
        module_dir: PathBuf,
        inherited_attributes: Vec<String>,
    ) {
        self.module_registrations.clear();
        self.current_entry = Some(path.clone());
        self.scan(path, prefix, module_dir, inherited_attributes);
    }

    fn register_module(&mut self, name: String, site: TestSite) {
        if let Some(first) = self.module_registrations.insert(name.clone(), site.clone()) {
            let entry = self
                .current_entry
                .as_ref()
                .expect("module scan always has a target entry");
            panic!(
                "duplicate local module registration `{name}` within target entry {}: \
                 first declared at {}:{}, repeated at {}:{}; keep exactly one \
                 `mod` declaration for this module in this Cargo target",
                entry.display(),
                first.path.display(),
                first.line,
                site.path.display(),
                site.line,
            );
        }
    }

    fn scan(
        &mut self,
        path: PathBuf,
        prefix: Vec<String>,
        module_dir: PathBuf,
        inherited_attributes: Vec<String>,
    ) {
        if !self.active.insert(path.clone()) {
            return;
        }
        self.files.insert(path.clone());
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
        let local_tests = recognize_declared_tests(&source, &path);
        self.macro_template_tests += local_tests.macro_template_tests;
        for local in local_tests.records {
            let mut name_path = prefix.clone();
            name_path.extend(local.name.split("::").map(str::to_owned));
            let mut attributes = inherited_attributes.clone();
            attributes.extend(local.attributes);
            let record = TestRecord {
                name: name_path.join("::"),
                site: local.site,
                attributes,
                source_span: local.source_span,
            };
            self.tests
                .entry(record.name.clone())
                .or_default()
                .push(record.site.clone());
            self.records.push(record);
        }

        let tokens = lex(&source, &path);
        let mut stack: Vec<(String, i32)> = Vec::new();
        let mut depth = 0i32;
        let mut pending_path: Option<(i32, String)> = None;
        // Verbatim attribute text seen since the last item boundary, plus the
        // attributes of every module that declared this file. An `#[ignore]`
        // or `#[cfg]` on an enclosing module gates the tests inside it just
        // as much as one written on the function.
        let mut pending_attributes: Vec<String> = Vec::new();
        let mut module_attributes: Vec<(i32, Vec<String>)> = Vec::new();
        let mut index = 0;

        while index < tokens.len() {
            // `#[path = "…"]` — fail-closed on any other shape at the same
            // token position (this catches `#[path]`, `#[path(x)]`, and
            // `#[path = expr]`, all of which would silently disable the
            // gate today).
            if is_path_attribute_head(&tokens, index) {
                let value = require_path_attribute(&tokens, index, &path, &source);
                if !stack.is_empty() {
                    panic!(
                        "{}:{}: `#[path]` inside an inline mod block is not \
                         supported by the sabotage manifest scanner. See \
                         `enzui_test_support::sabotage_manifest` module \
                         docs for the documented resolution boundary.",
                        path.display(),
                        line_for(&source, tokens[index].offset),
                    );
                }
                pending_path = Some((depth, value));
                index = attribute_end(&tokens, index, &path, &source);
                continue;
            }
            // Skip any other attribute without disturbing pending markers,
            // so an attribute chain between `#[path]` and its module still
            // lands correctly. Validate cfg_attr through the canonical meta
            // parser and retain wrappers because they may emit cfg/ignore.
            if is_other_attribute(&tokens, index) {
                let end = attribute_end(&tokens, index, &path, &source);
                let inner = matches!(
                    tokens.get(index + 1).map(|t| &t.kind),
                    Some(TokenKind::Punct(b'!'))
                );
                if !inner {
                    let is_plain_test = test_attribute(&tokens, index, end);
                    let _ = attribute_has_active_meta(&tokens, index, end, "test", &path, &source);
                    if !is_plain_test {
                        let text = attribute_text(&source, &tokens, index, end);
                        self.attributes.push(text.clone());
                        pending_attributes.push(text);
                    }
                }
                index = end;
                continue;
            }

            match &tokens[index].kind {
                TokenKind::Punct(b'{') => {
                    depth += 1;
                    pending_attributes.clear();
                }
                TokenKind::Punct(b'}') => {
                    depth -= 1;
                    while stack.last().is_some_and(|(_, open)| depth < *open) {
                        stack.pop();
                    }
                    while module_attributes
                        .last()
                        .is_some_and(|(open, _)| depth < *open)
                    {
                        module_attributes.pop();
                    }
                    pending_path = None;
                    pending_attributes.clear();
                }
                TokenKind::Ident(word) if word == "mod" => {
                    if let Some(name) = ident_at(&tokens, index + 1) {
                        match tokens.get(index + 2).map(|token| &token.kind) {
                            Some(TokenKind::Punct(b'{')) => {
                                let mut module_path = prefix.clone();
                                module_path.extend(stack.iter().map(|(name, _)| name.clone()));
                                module_path.push(name.to_owned());
                                self.register_module(
                                    module_path.join("::"),
                                    TestSite {
                                        path: path.clone(),
                                        line: line_for(&source, tokens[index].offset),
                                    },
                                );
                                stack.push((name.to_owned(), depth + 1));
                                module_attributes
                                    .push((depth + 1, std::mem::take(&mut pending_attributes)));
                                pending_path = None;
                            }
                            Some(TokenKind::Punct(b';')) => {
                                let mut child_prefix = prefix.clone();
                                child_prefix.extend(stack.iter().map(|(name, _)| name.clone()));
                                child_prefix.push(name.to_owned());
                                let mod_line = line_for(&source, tokens[index].offset);
                                self.register_module(
                                    child_prefix.join("::"),
                                    TestSite {
                                        path: path.clone(),
                                        line: mod_line,
                                    },
                                );
                                let current_dir = inline_module_dir(&module_dir, &stack);
                                let explicit = pending_path
                                    .take()
                                    .filter(|(attribute_depth, _)| *attribute_depth == depth)
                                    .map(|(_, value)| value);
                                let (child, child_dir) = resolve_module(
                                    &path,
                                    &current_dir,
                                    name,
                                    explicit.as_deref(),
                                    mod_line,
                                );
                                let mut child_attributes = inherited_attributes.clone();
                                for (_, module) in &module_attributes {
                                    child_attributes.extend(module.iter().cloned());
                                }
                                child_attributes.extend(pending_attributes.iter().cloned());
                                self.scan(child, child_prefix, child_dir, child_attributes);
                            }
                            _ => {}
                        }
                    }
                    pending_attributes.clear();
                }
                TokenKind::Ident(word) if word == "include" => {
                    // Only the standard `include!(…)` macro walks source
                    // into another file; `include_str!` / `include_bytes!`
                    // hit a different ident and are ignored by name.
                    if punct_at(&tokens, index + 1, b'!') && punct_at(&tokens, index + 2, b'(') {
                        let value = require_include_literal(&tokens, index, &path, &source);
                        let child = path.parent().unwrap_or_else(|| Path::new("")).join(value);
                        if !child.is_file() {
                            panic!(
                                "{}:{}: `include!(…)` fragment {} not found",
                                path.display(),
                                line_for(&source, tokens[index].offset),
                                child.display(),
                            );
                        }
                        let mut child_prefix = prefix.clone();
                        child_prefix.extend(stack.iter().map(|(name, _)| name.clone()));
                        let mut child_attributes = inherited_attributes.clone();
                        for (_, module) in &module_attributes {
                            child_attributes.extend(module.iter().cloned());
                        }
                        self.scan(
                            child.clone(),
                            child_prefix,
                            child
                                .parent()
                                .expect("a joined child has a parent")
                                .to_path_buf(),
                            child_attributes,
                        );
                    }
                    pending_path = None;
                    pending_attributes.clear();
                }
                // A visibility or item qualifier sits *between* an attribute
                // and the item it applies to (`#[cfg(unix)] pub mod host;`),
                // so it must not drop the remembered attributes. Every other
                // token ends the attribute run.
                _ if is_item_qualifier(&tokens[index]) => {}
                _ => {
                    pending_attributes.clear();
                }
            }
            index += 1;
        }
        self.active.remove(&path);
    }
}

fn is_item_qualifier(token: &Token) -> bool {
    match &token.kind {
        TokenKind::Ident(word) => matches!(
            word.as_str(),
            "pub" | "crate" | "self" | "super" | "in" | "async" | "unsafe" | "extern" | "const"
        ),
        TokenKind::Punct(byte) => matches!(byte, b'(' | b')'),
        TokenKind::String(_) => false,
    }
}

/// Verbatim source text of the attribute whose `#` sits at `start` and whose
/// closing `]` is the token before `end`. Reading the source range keeps the
/// text exactly as written (`#[ignore = "reason"]`), so a caller classifying
/// an ignore reason sees what a human reviewer sees.
fn attribute_text(source: &str, tokens: &[Token], start: usize, end: usize) -> String {
    let from = tokens[start].offset;
    let to = tokens
        .get(end - 1)
        .map_or(source.len(), |token| token.offset + 1);
    source[from..to.min(source.len())].to_owned()
}

fn inline_module_dir(base: &Path, stack: &[(String, i32)]) -> PathBuf {
    stack
        .iter()
        .fold(base.to_path_buf(), |dir, (name, _)| dir.join(name))
}

fn resolve_module(
    declaring_file: &Path,
    module_dir: &Path,
    name: &str,
    explicit: Option<&str>,
    mod_line: usize,
) -> (PathBuf, PathBuf) {
    let parent = declaring_file
        .parent()
        .expect("declaring file must live under a directory");
    let child = if let Some(relative) = explicit {
        parent.join(relative)
    } else {
        let flat = module_dir.join(format!("{name}.rs"));
        let nested = module_dir.join(name).join("mod.rs");
        if flat.is_file() {
            flat
        } else if nested.is_file() {
            nested
        } else {
            panic!(
                "{}:{}: `mod {name};` resolves to neither {} nor {} — the \
                 sabotage manifest scanner cannot audit a missing module",
                declaring_file.display(),
                mod_line,
                flat.display(),
                nested.display(),
            );
        }
    };
    if !child.is_file() {
        panic!(
            "{}:{}: `#[path = \"…\"] mod {name};` points at {} which is not \
             a readable file",
            declaring_file.display(),
            mod_line,
            child.display(),
        );
    }
    let child_dir = if child.file_name().and_then(|v| v.to_str()) == Some("mod.rs") {
        child
            .parent()
            .expect("mod.rs always has a parent")
            .to_path_buf()
    } else {
        let stem = child
            .file_stem()
            .expect("child module file has a stem")
            .to_owned();
        child
            .parent()
            .expect("child module file has a parent")
            .join(stem)
    };
    (child, child_dir)
}

fn line_for(source: &str, offset: usize) -> usize {
    1 + source[..offset.min(source.len())]
        .bytes()
        .filter(|b| *b == b'\n')
        .count()
}

#[derive(Debug)]
struct Token {
    kind: TokenKind,
    offset: usize,
}

#[derive(Debug)]
enum TokenKind {
    Ident(String),
    String(String),
    Punct(u8),
}

fn ident_at(tokens: &[Token], index: usize) -> Option<&str> {
    match tokens.get(index)?.kind {
        TokenKind::Ident(ref value) => Some(value),
        _ => None,
    }
}

fn punct_at(tokens: &[Token], index: usize, value: u8) -> bool {
    matches!(tokens.get(index).map(|t| &t.kind), Some(TokenKind::Punct(found)) if *found == value)
}

fn is_path_attribute_head(tokens: &[Token], index: usize) -> bool {
    punct_at(tokens, index, b'#')
        && punct_at(tokens, index + 1, b'[')
        && ident_at(tokens, index + 2) == Some("path")
}

fn is_other_attribute(tokens: &[Token], index: usize) -> bool {
    punct_at(tokens, index, b'#')
        && (punct_at(tokens, index + 1, b'[')
            || (punct_at(tokens, index + 1, b'!') && punct_at(tokens, index + 2, b'[')))
}

fn require_path_attribute(
    tokens: &[Token],
    index: usize,
    source_path: &Path,
    source: &str,
) -> String {
    let ok = punct_at(tokens, index + 3, b'=')
        && matches!(
            tokens.get(index + 4).map(|t| &t.kind),
            Some(TokenKind::String(_))
        )
        && punct_at(tokens, index + 5, b']');
    if !ok {
        panic!(
            "{}:{}: `#[path]` attribute must be exactly `#[path = \"…\"]` — \
             computed paths (macros, concat, expressions) cannot be resolved \
             by the sabotage manifest scanner",
            source_path.display(),
            line_for(source, tokens[index].offset),
        );
    }
    match &tokens[index + 4].kind {
        TokenKind::String(value) => value.clone(),
        _ => unreachable!("guarded by matches! above"),
    }
}

fn test_attribute(tokens: &[Token], index: usize, end: usize) -> bool {
    end == index + 4
        && punct_at(tokens, index, b'#')
        && punct_at(tokens, index + 1, b'[')
        && ident_at(tokens, index + 2) == Some("test")
        && punct_at(tokens, index + 3, b']')
}

/// Whether an outer attribute emits `target` in a test build.
///
/// `cfg_attr` is recursive. Unknown predicates are conservatively inactive:
/// only predicates whose truth follows from test-harness compilation (`test`)
/// or Rust's identity predicates (`all()` / `any(...)`) can manufacture an
/// executable identity. Malformed `cfg_attr` is loud rather than looking like
/// an ordinary unrelated attribute.
fn attribute_has_active_meta(
    tokens: &[Token],
    start: usize,
    end: usize,
    target: &str,
    path: &Path,
    source: &str,
) -> bool {
    let body_start = start
        + if punct_at(tokens, start + 1, b'!') {
            3
        } else {
            2
        };
    meta_has_active(
        tokens,
        body_start,
        end.saturating_sub(1),
        target,
        path,
        source,
    )
}

fn meta_has_active(
    tokens: &[Token],
    start: usize,
    end: usize,
    target: &str,
    path: &Path,
    source: &str,
) -> bool {
    let Some(head) = ident_at(tokens, start) else {
        return false;
    };
    if head == target {
        if target == "test" && start + 1 != end {
            panic!(
                "{}:{}: malformed test attribute; test declaration inventory is inconclusive",
                path.display(),
                line_for(source, tokens[start].offset),
            );
        }
        return true;
    }
    if head != "cfg_attr" {
        return false;
    }
    let parts = cfg_attr_parts(tokens, start, end, path, source);
    let active = cfg_predicate_in_test(tokens, parts[0].0, parts[0].1) == Some(true);
    let mut nested_match = false;
    for &(from, to) in &parts[1..] {
        // Parse every nested cfg_attr even under an inactive/unknown branch:
        // malformed source must never truncate the inventory quietly.
        nested_match |= meta_has_active(tokens, from, to, target, path, source);
    }
    active && nested_match
}

fn cfg_attr_parts(
    tokens: &[Token],
    start: usize,
    end: usize,
    path: &Path,
    source: &str,
) -> Vec<(usize, usize)> {
    let malformed = || -> ! {
        panic!(
            "{}:{}: malformed cfg_attr; test declaration inventory is inconclusive",
            path.display(),
            line_for(source, tokens[start].offset),
        )
    };
    if !punct_at(tokens, start + 1, b'(') || !punct_at(tokens, end.saturating_sub(1), b')') {
        malformed();
    }
    let mut parts = Vec::new();
    let mut part = start + 2;
    let mut depth = 0i32;
    for (relative, token) in tokens[start + 2..end - 1].iter().enumerate() {
        let cursor = start + 2 + relative;
        match token.kind {
            TokenKind::Punct(b'(' | b'[' | b'{') => depth += 1,
            TokenKind::Punct(b')' | b']' | b'}') => {
                depth -= 1;
                if depth < 0 {
                    malformed();
                }
            }
            TokenKind::Punct(b',') if depth == 0 => {
                if part == cursor {
                    malformed();
                }
                parts.push((part, cursor));
                part = cursor + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        malformed();
    }
    if part < end - 1 {
        parts.push((part, end - 1));
    } else if parts.len() < 2 {
        // No metadata followed the predicate. A trailing comma is accepted
        // only after at least one complete metadata item.
        malformed();
    }
    if parts.len() < 2 {
        malformed();
    }
    parts
}

fn cfg_predicate_in_test(tokens: &[Token], start: usize, end: usize) -> Option<bool> {
    if start >= end {
        return None;
    }
    if ident_at(tokens, start) == Some("test") && start + 1 == end {
        return Some(true);
    }
    let operator = ident_at(tokens, start)?;
    if !matches!(operator, "all" | "any" | "not")
        || !punct_at(tokens, start + 1, b'(')
        || !punct_at(tokens, end - 1, b')')
    {
        return None;
    }
    let args = split_meta_arguments(tokens, start + 2, end - 1);
    match operator {
        "all" => {
            let values = args
                .iter()
                .map(|&(from, to)| cfg_predicate_in_test(tokens, from, to))
                .collect::<Vec<_>>();
            if values.contains(&Some(false)) {
                Some(false)
            } else if values.iter().all(Option::is_some) {
                Some(true)
            } else {
                None
            }
        }
        "any" => {
            let values = args
                .iter()
                .map(|&(from, to)| cfg_predicate_in_test(tokens, from, to))
                .collect::<Vec<_>>();
            if values.contains(&Some(true)) {
                Some(true)
            } else if values.iter().all(Option::is_some) {
                Some(false)
            } else {
                None
            }
        }
        "not" if args.len() == 1 => cfg_predicate_in_test(tokens, args[0].0, args[0].1).map(|v| !v),
        _ => None,
    }
}

fn split_meta_arguments(tokens: &[Token], start: usize, end: usize) -> Vec<(usize, usize)> {
    if start == end {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut part = start;
    let mut depth = 0i32;
    for (relative, token) in tokens[start..end].iter().enumerate() {
        let cursor = start + relative;
        match token.kind {
            TokenKind::Punct(b'(' | b'[' | b'{') => depth += 1,
            TokenKind::Punct(b')' | b']' | b'}') => depth -= 1,
            TokenKind::Punct(b',') if depth == 0 => {
                if part < cursor {
                    out.push((part, cursor));
                }
                part = cursor + 1;
            }
            _ => {}
        }
    }
    if part < end {
        out.push((part, end));
    }
    out
}

fn effective_attribute_bodies(attribute: &str, target: &str, path: &Path) -> Vec<String> {
    let tokens = lex(attribute, path);
    if !is_other_attribute(&tokens, 0) {
        return Vec::new();
    }
    let end = attribute_end(&tokens, 0, path, attribute);
    fn collect(
        tokens: &[Token],
        start: usize,
        end: usize,
        target: &str,
        path: &Path,
        source: &str,
        out: &mut Vec<String>,
    ) {
        let Some(head) = ident_at(tokens, start) else {
            return;
        };
        if head == target {
            let valid_shape = match target {
                "test" => start + 1 == end,
                "ignore" => start + 1 == end || punct_at(tokens, start + 1, b'='),
                "cfg" => punct_at(tokens, start + 1, b'('),
                _ => true,
            };
            if !valid_shape {
                panic!(
                    "{}:{}: malformed {target} attribute; test declaration inventory is \
                     inconclusive",
                    path.display(),
                    line_for(source, tokens[start].offset),
                );
            }
            let from = tokens
                .get(start + 1)
                .map_or(tokens[start].offset + head.len(), |token| token.offset);
            let to = tokens.get(end).map_or(source.len(), |token| token.offset);
            out.push(source[from..to.max(from)].trim().to_owned());
            return;
        }
        if head != "cfg_attr" {
            return;
        }
        let parts = cfg_attr_parts(tokens, start, end, path, source);
        match cfg_predicate_in_test(tokens, parts[0].0, parts[0].1) {
            Some(true) => {
                for &(from, to) in &parts[1..] {
                    collect(tokens, from, to, target, path, source, out);
                }
            }
            Some(false) => {}
            // A profile, feature or target predicate decides whether the
            // attribute applies, and the inventory cannot know which build
            // runs: dropping it would overstate (or hide) runnable tests.
            None if parts[1..]
                .iter()
                .any(|&(from, to)| wraps(tokens, from, to, target)) =>
            {
                panic!(
                    "{}:{}: `cfg_attr` makes `{target}` depend on a configuration the test \
                     inventory cannot evaluate; its runnability is indeterminate",
                    path.display(),
                    line_for(source, tokens[start].offset),
                );
            }
            None => {}
        }
    }
    /// Whether the attribute `from..to` is `target`, directly or inside a
    /// nested `cfg_attr`.
    fn wraps(tokens: &[Token], from: usize, to: usize, target: &str) -> bool {
        match ident_at(tokens, from) {
            Some(head) if head == target => true,
            Some("cfg_attr") => (from..to).any(|at| ident_at(tokens, at) == Some(target)),
            _ => false,
        }
    }
    let mut out = Vec::new();
    let body_start = if punct_at(&tokens, 1, b'!') { 3 } else { 2 };
    collect(
        &tokens,
        body_start,
        end - 1,
        target,
        path,
        attribute,
        &mut out,
    );
    out
}

fn function_span(tokens: &[Token], index: usize, path: &Path, source: &str) -> Range<usize> {
    let start = tokens[index].offset;
    let Some(open) = (index + 2..tokens.len()).find(|&at| punct_at(tokens, at, b'{')) else {
        panic!(
            "{}:{}: attributed test function has no body",
            path.display(),
            line_for(source, start)
        );
    };
    let mut depth = 1i32;
    for token in &tokens[open + 1..] {
        match token.kind {
            TokenKind::Punct(b'{') => depth += 1,
            TokenKind::Punct(b'}') => {
                depth -= 1;
                if depth == 0 {
                    return start..token.offset + 1;
                }
            }
            _ => {}
        }
    }
    panic!(
        "{}:{}: unclosed attributed test function body",
        path.display(),
        line_for(source, start)
    );
}

fn attribute_end(tokens: &[Token], index: usize, source_path: &Path, source: &str) -> usize {
    // Advance past `#[` (or `#![`) and match brackets to their close.
    let mut cursor = index + 2;
    if matches!(
        tokens.get(index + 1).map(|t| &t.kind),
        Some(TokenKind::Punct(b'!'))
    ) {
        cursor += 1;
    }
    let mut depth = 1i32;
    while cursor < tokens.len() && depth > 0 {
        match tokens[cursor].kind {
            TokenKind::Punct(b'[') => depth += 1,
            TokenKind::Punct(b']') => depth -= 1,
            _ => {}
        }
        cursor += 1;
    }
    assert!(
        depth == 0,
        "{}:{}: unclosed Rust attribute; test declaration inventory is inconclusive",
        source_path.display(),
        line_for(source, tokens[index].offset),
    );
    cursor
}

fn require_include_literal(
    tokens: &[Token],
    index: usize,
    source_path: &Path,
    source: &str,
) -> String {
    // Expect `include ! ( "…" )`. Anything else (concat!, path variables, an
    // empty argument list) is fail-closed.
    match (
        tokens.get(index + 3).map(|t| &t.kind),
        tokens.get(index + 4).map(|t| &t.kind),
    ) {
        (Some(TokenKind::String(value)), Some(TokenKind::Punct(b')'))) => value.clone(),
        _ => panic!(
            "{}:{}: `include!(…)` argument must be a single string literal — \
             computed arguments (concat!, env!, path variables) cannot be \
             followed by the sabotage manifest scanner",
            source_path.display(),
            line_for(source, tokens[index].offset),
        ),
    }
}

fn lex(source: &str, source_path: &Path) -> Vec<Token> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_whitespace() {
            index += 1;
        } else if bytes[index..].starts_with(b"//") {
            index += bytes[index..]
                .iter()
                .position(|b| *b == b'\n')
                .unwrap_or(bytes.len() - index);
        } else if bytes[index..].starts_with(b"/*") {
            index = skip_block_comment(bytes, index).unwrap_or_else(|| {
                panic!(
                    "{}:{}: unclosed block comment; test declaration inventory is inconclusive",
                    source_path.display(),
                    line_for(source, index),
                )
            });
        } else if raw_string_start(source, index) {
            let (value, end) = raw_string(source, index).unwrap_or_else(|| {
                panic!(
                    "{}:{}: unclosed raw string; test declaration inventory is inconclusive",
                    source_path.display(),
                    line_for(source, index),
                )
            });
            tokens.push(Token {
                kind: TokenKind::String(value),
                offset: index,
            });
            index = end;
        } else if bytes[index] == b'"'
            || (bytes[index] == b'b' && bytes.get(index + 1) == Some(&b'"'))
        {
            let (value, end) = normal_string(source, index).unwrap_or_else(|| {
                panic!(
                    "{}:{}: unclosed string; test declaration inventory is inconclusive",
                    source_path.display(),
                    line_for(source, index),
                )
            });
            tokens.push(Token {
                kind: TokenKind::String(value),
                offset: index,
            });
            index = end;
        } else if bytes[index] == b'\''
            && let Some(end) = char_literal_end(bytes, index)
        {
            index = end + 1;
        } else if bytes[index..].starts_with(b"r#")
            && bytes
                .get(index + 2)
                .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        {
            // Rust raw identifiers have the same semantic name as their
            // unprefixed spelling. In particular, the test harness reports
            // `fn r#match` as `match`, and `mod r#type` contributes `type` to
            // its executable path.
            let offset = index;
            let start = index + 2;
            index = start + 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            tokens.push(Token {
                kind: TokenKind::Ident(source[start..index].to_owned()),
                offset,
            });
        } else if bytes[index].is_ascii_alphabetic() || bytes[index] == b'_' {
            let start = index;
            index += 1;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            tokens.push(Token {
                kind: TokenKind::Ident(source[start..index].to_owned()),
                offset: start,
            });
        } else {
            tokens.push(Token {
                kind: TokenKind::Punct(bytes[index]),
                offset: index,
            });
            index += 1;
        }
    }
    tokens
}

fn skip_block_comment(bytes: &[u8], mut index: usize) -> Option<usize> {
    index += 2;
    let mut depth = 1;
    while index < bytes.len() && depth > 0 {
        if bytes[index..].starts_with(b"/*") {
            depth += 1;
            index += 2;
        } else if bytes[index..].starts_with(b"*/") {
            depth -= 1;
            index += 2;
        } else {
            index += 1;
        }
    }
    (depth == 0).then_some(index)
}

fn raw_string_start(source: &str, start: usize) -> bool {
    let bytes = source.as_bytes();
    let r = if bytes.get(start) == Some(&b'r') {
        start
    } else if bytes.get(start..start + 2) == Some(b"br") {
        start + 1
    } else {
        return false;
    };
    let mut quote = r + 1;
    while bytes.get(quote) == Some(&b'#') {
        quote += 1;
    }
    bytes.get(quote) == Some(&b'"')
}

fn raw_string(source: &str, start: usize) -> Option<(String, usize)> {
    let bytes = source.as_bytes();
    let r = if bytes.get(start) == Some(&b'r') {
        start
    } else if bytes.get(start..start + 2) == Some(b"br") {
        start + 1
    } else {
        return None;
    };
    let mut quote = r + 1;
    while bytes.get(quote) == Some(&b'#') {
        quote += 1;
    }
    if bytes.get(quote) != Some(&b'"') {
        return None;
    }
    let hashes = quote - r - 1;
    let closer = format!("\"{}", "#".repeat(hashes));
    let body = quote + 1;
    let relative = source[body..].find(&closer)?;
    Some((
        source[body..body + relative].to_owned(),
        body + relative + closer.len(),
    ))
}

fn normal_string(source: &str, start: usize) -> Option<(String, usize)> {
    let bytes = source.as_bytes();
    let quote = if bytes[start] == b'b' {
        start + 1
    } else {
        start
    };
    let mut index = quote + 1;
    let mut value = String::new();
    while index < bytes.len() {
        match bytes[index] {
            b'"' => return Some((value, index + 1)),
            b'\\' if index + 1 < bytes.len() => {
                index += 1;
                value.push(match bytes[index] {
                    b'n' => '\n',
                    b'r' => '\r',
                    b't' => '\t',
                    other => other as char,
                });
                index += 1;
            }
            byte if byte.is_ascii() => {
                value.push(byte as char);
                index += 1;
            }
            _ => {
                let ch = source[index..]
                    .chars()
                    .next()
                    .expect("index is inside the source");
                value.push(ch);
                index += ch.len_utf8();
            }
        }
    }
    None
}

fn char_literal_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut index = start + 1;
    if index >= bytes.len() {
        return None;
    }
    if bytes[index] == b'\\' {
        index += 1;
        while index < bytes.len() && index <= start + 12 {
            if bytes[index] == b'\'' {
                return Some(index);
            }
            index += 1;
        }
        return None;
    }
    let mut end = index + 1;
    while end < bytes.len() && (bytes[end] & 0xc0) == 0x80 {
        end += 1;
    }
    (bytes.get(end) == Some(&b'\'')).then_some(end)
}
