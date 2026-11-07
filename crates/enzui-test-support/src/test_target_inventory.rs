//! Source-derived inventory of a crate's Cargo integration test targets.
//!
//! # Purpose
//!
//! A scripted `cargo test --test <target> [filter]` is two claims about the
//! tree that nothing checks: that the target exists, and that the filter
//! selects a test which actually runs in the profile and mode the command
//! uses. Both fail silently — cargo reports "no test target named …" (the
//! runner calls it INCONCLUSIVE), and a filter that matches nothing exits 0
//! with `0 passed`. So the inventory is derived from the source graph of every
//! target, never from a hand-maintained roster that a rename leaves stale.
//!
//! Both Cargo target shapes are covered: `tests/<name>.rs` (standalone) and
//! `tests/<name>/main.rs` (directory). Cargo builds one binary per shape;
//! both are enumerated here.
//!
//! # What is derived
//!
//! Per target: the Cargo target name, its entry path, every reachable source
//! file, and every effective `#[test]` function with the attributes that decide
//! whether it runs — direct or active nested `cfg_attr` forms of
//! `#[ignore = "reason"]` and `#[cfg]` on the function and every module that
//! declared it. From those: the declared
//! count, the ignored count, and separate debug-live and release-live counts,
//! because `#[cfg(not(debug_assertions))]` makes a test exist in one profile
//! only, and an `#[ignore]`d hardware or perf gate is not live by default in
//! either.
//!
//! Reading Rust is delegated to [`crate::sabotage_manifest::scan_source_graph`]
//! (one lexer, one `#[path]`/`include!` resolver, one set of fail-closed
//! rules). This module only interprets what that walker found.
//!
//! # Fail-closed boundary
//!
//! * A `tests/` directory that does not exist, or holds no target, panics: an
//!   empty inventory would report exactly what a clean one reports.
//! * A duplicate Cargo target name panics — two entries cannot both answer
//!   `--test <name>`.
//! * A duplicate logical `mod` registration within one target entry panics
//!   with the module and both declaration sites. The same shared harness may
//!   still be registered by any number of distinct targets.
//! * A target whose graph declares no `#[test]` panics: `--test <name>` would
//!   compile, run zero tests and exit 0.
//! * A `#[cfg]` predicate this module cannot evaluate panics with the file and
//!   line. Liveness is a claim about what runs; guessing is how a hardware or
//!   perf gate gets classified as default-live. New predicates require explicit
//!   evaluator support before they can appear in the inventoried sources.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::sabotage_manifest::{TestRecord, scan_source_graph};

/// Which Cargo shape declares the target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TargetShape {
    /// `tests/<name>.rs`
    Standalone,
    /// `tests/<name>/main.rs`
    Directory,
}

/// The build profile a dispatch runs under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// `cargo test`, with debug assertions.
    Debug,
    /// `cargo test --release`.
    Release,
}

/// A `#[cfg]` atom the inventory knows how to evaluate.
#[derive(Debug, Clone, PartialEq, Eq)]
enum CfgAtom {
    /// True in every `--test` binary, which is the only thing built here.
    Test,
    /// Profile-dependent: the only atom whose value differs debug/release.
    DebugAssertions,
    /// A platform predicate, resolved against the running host rather than
    /// assumed. `target_os = "linux"`, `target_family = "unix"` and `unix` are
    /// true here; `windows` and `target_os = "macos"` are false.
    ///
    /// Treating every `target_*` predicate as "another platform" would be wrong
    /// in the direction that matters: `#[cfg(target_os = "linux")]` gates tests
    /// that *do* run on this host, and calling them uncompiled removes them
    /// from the live counts and makes a real filter look like a zero-match
    /// one.
    Platform { text: String, satisfied: bool },
}

/// Is this platform predicate true for the host running the gate?
///
/// Read from `std::env::consts` and `cfg!`, never from a table: a hard-coded
/// "we are Linux" would be a claim that stops being checked the day it moves.
fn host_satisfies(key: &str, value: &str) -> bool {
    match key {
        "target_os" => value == std::env::consts::OS,
        "target_arch" => value == std::env::consts::ARCH,
        "target_family" => value == std::env::consts::FAMILY,
        // Every other key reaches this function only through `atoms`, which
        // panics on an unsupported one first.
        _ => false,
    }
}

/// One `#[test]` function with its runnability classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoryTest {
    /// Full cargo test name (`module::path::function`), what a filter matches.
    pub name: String,
    /// The source file.
    pub path: PathBuf,
    /// The 1-based line of the `fn`.
    pub line: usize,
    /// Verbatim `#[ignore]` reason, `Some("")` when written without one.
    pub ignore_reason: Option<String>,
    /// Verbatim `#[cfg(…)]` predicates gating the test or its modules.
    pub cfg_predicates: Vec<String>,
    /// Platform atoms the test names (`windows`, `target_os = "macos"`, …).
    pub platform_constraints: BTreeSet<String>,
    debug_compiled: bool,
    release_compiled: bool,
}

impl InventoryTest {
    /// Does this test run in `profile` without `--ignored`?
    #[must_use]
    pub fn is_live(&self, profile: Profile) -> bool {
        self.ignore_reason.is_none() && self.is_compiled(profile)
    }

    /// Does this test exist in `profile` at all (ignored or not)?
    #[must_use]
    pub fn is_compiled(&self, profile: Profile) -> bool {
        match profile {
            Profile::Debug => self.debug_compiled,
            Profile::Release => self.release_compiled,
        }
    }
}

/// One Cargo integration test target, derived from the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetInventory {
    /// The Cargo target name.
    pub name: String,
    /// The target's root source file.
    pub entry: PathBuf,
    /// Whether the target is one file or a directory.
    pub shape: TargetShape,
    /// Every source file compiled into the target, entry included.
    pub sources: BTreeSet<PathBuf>,
    /// Every `#[test]` function, classified.
    pub tests: Vec<InventoryTest>,
    /// `#[test]` declarations inside `macro_rules!` bodies: real tests exist
    /// only as expansions, under names no filter can be written against.
    pub macro_template_tests: usize,
    /// The target declares a `#[global_allocator]`: it counts allocations, so
    /// it must stay its own process and cannot be merged into another target.
    pub owns_process_allocator: bool,
}

impl TargetInventory {
    /// How many tests the target declares.
    #[must_use]
    pub fn declared(&self) -> usize {
        self.tests.len()
    }

    /// How many of them are `#[ignore]`d.
    #[must_use]
    pub fn ignored(&self) -> usize {
        self.tests
            .iter()
            .filter(|test| test.ignore_reason.is_some())
            .count()
    }

    /// Tests that run under a plain `cargo test --test <name>` in `profile`.
    #[must_use]
    pub fn live(&self, profile: Profile) -> usize {
        self.tests
            .iter()
            .filter(|test| test.is_live(profile))
            .count()
    }

    /// Tests a cargo filter selects. Cargo matches a filter as a substring of
    /// the full test name, so this is the same relation the harness applies.
    #[must_use]
    pub fn matching(&self, filter: &str) -> Vec<&InventoryTest> {
        self.tests
            .iter()
            .filter(|test| test.name.contains(filter))
            .collect()
    }
}

/// Names of the `[[test]]` targets the crate manifest beside `tests_dir`
/// declares with `harness = false`.
fn custom_harness_targets(tests_dir: &Path) -> std::collections::BTreeSet<String> {
    let manifest = tests_dir
        .parent()
        .map(|crate_dir| crate_dir.join("Cargo.toml"));
    let Some(text) = manifest.and_then(|path| std::fs::read_to_string(path).ok()) else {
        return std::collections::BTreeSet::new();
    };
    let mut out = std::collections::BTreeSet::new();
    for block in text.split("[[test]]").skip(1) {
        let block = block.split("\n[").next().unwrap_or(block);
        let mut name = None;
        let mut harness_free = false;
        for line in block.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if let Some(value) = line.strip_prefix("name") {
                name = value
                    .trim_start()
                    .strip_prefix('=')
                    .map(|v| v.trim().trim_matches('"').to_owned());
            } else if line.replace(' ', "") == "harness=false" {
                harness_free = true;
            }
        }
        if let (Some(name), true) = (name, harness_free) {
            out.insert(name);
        }
    }
    out
}

/// Every integration test target under `tests_dir`, keyed by Cargo name.
///
/// # Panics
///
/// See the module's fail-closed boundary: missing directory, empty
/// population, duplicate name, empty target, unevaluable `#[cfg]`.
#[must_use]
pub fn integration_targets(tests_dir: &Path) -> BTreeMap<String, TargetInventory> {
    assert!(
        tests_dir.is_dir(),
        "integration_targets({}): no tests directory. Every subject below is \
         derived from it, so a wrong path would report an empty, clean tree",
        tests_dir.display()
    );
    let mut entries: Vec<(String, PathBuf, TargetShape)> = Vec::new();
    let read = std::fs::read_dir(tests_dir)
        .unwrap_or_else(|error| panic!("read_dir {}: {error}", tests_dir.display()));
    for entry in read {
        let path = entry.expect("tests/ entry").path();
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if path.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
            entries.push((stem.to_owned(), path.clone(), TargetShape::Standalone));
        } else if path.is_dir() && path.join("main.rs").is_file() {
            entries.push((
                stem.to_owned(),
                path.join("main.rs"),
                TargetShape::Directory,
            ));
        }
    }
    entries.sort();
    let custom_harness = custom_harness_targets(tests_dir);

    let mut targets: BTreeMap<String, TargetInventory> = BTreeMap::new();
    for (name, entry, shape) in entries {
        let graph = scan_source_graph(&entry);
        let tests = graph
            .tests
            .iter()
            .map(|record| classify(&entry, record))
            .collect::<Vec<_>>();
        if custom_harness.contains(&name) {
            // A `harness = false` target runs its own `main`; it has no
            // `#[test]` by construction, but it must still have a main.
            let source = std::fs::read_to_string(&entry)
                .unwrap_or_else(|error| panic!("read {}: {error}", entry.display()));
            assert!(
                source.contains("fn main("),
                "cargo target `--test {name}` is `harness = false` but {} has no `fn main`",
                entry.display()
            );
        }
        assert!(
            !tests.is_empty() || custom_harness.contains(&name),
            "cargo target `--test {name}` ({}) declares no #[test]: it would \
             compile, run zero tests and exit 0, which is indistinguishable \
             from success for every caller that names it",
            entry.display()
        );
        let inventory = TargetInventory {
            name: name.clone(),
            entry: entry.clone(),
            shape,
            sources: graph.files,
            tests,
            macro_template_tests: graph.macro_template_tests,
            owns_process_allocator: graph
                .attributes
                .iter()
                .any(|attribute| attribute.starts_with("#[global_allocator]")),
        };
        if let Some(previous) = targets.insert(name.clone(), inventory) {
            panic!(
                "two entries claim cargo target `--test {name}`: {} and {}. \
                 Cargo cannot dispatch an ambiguous target name",
                previous.entry.display(),
                entry.display()
            );
        }
    }
    assert!(
        !targets.is_empty(),
        "no integration test target was derived from {}; an empty inventory \
         cannot fail, so it is treated as a broken scan",
        tests_dir.display()
    );
    targets
}

fn classify(entry: &Path, record: &TestRecord) -> InventoryTest {
    let ignore_reason = record.ignore_reason();
    let cfg_predicates = record.effective_attribute_bodies("cfg");

    let mut platform_constraints = BTreeSet::new();
    let mut debug_compiled = true;
    let mut release_compiled = true;
    for predicate in &cfg_predicates {
        let inner = predicate
            .strip_prefix('(')
            .and_then(|body| body.strip_suffix(')'))
            .unwrap_or_else(|| {
                panic!(
                    "{}:{} ({}): `#[cfg{predicate}]` is not `#[cfg(…)]`; the \
                     inventory refuses to guess whether the test runs",
                    record.site.path.display(),
                    record.site.line,
                    entry.display(),
                )
            });
        for atom in atoms(inner, &record.site.path, record.site.line) {
            if let CfgAtom::Platform { text, .. } = atom {
                platform_constraints.insert(text);
            }
        }
        debug_compiled &= evaluate(inner, &record.site.path, record.site.line, true);
        release_compiled &= evaluate(inner, &record.site.path, record.site.line, false);
    }

    InventoryTest {
        name: record.name.clone(),
        path: record.site.path.clone(),
        line: record.site.line,
        ignore_reason,
        cfg_predicates,
        platform_constraints,
        debug_compiled,
        release_compiled,
    }
}

/// Split a `cfg` predicate into the atoms it names, ignoring `all`/`any`/`not`
/// structure. Used for reporting platform constraints; the yes/no answer comes
/// from [`evaluate`].
fn atoms(predicate: &str, path: &Path, line: usize) -> Vec<CfgAtom> {
    let mut out = Vec::new();
    let mut rest = predicate.trim();
    while !rest.is_empty() {
        let head: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();
        if head.is_empty() {
            rest = &rest[1..];
            continue;
        }
        rest = &rest[head.len()..];
        let tail = rest.trim_start();
        match head.as_str() {
            "all" | "any" | "not" => {}
            "test" => out.push(CfgAtom::Test),
            "debug_assertions" => out.push(CfgAtom::DebugAssertions),
            "unix" => out.push(CfgAtom::Platform {
                text: "unix".to_owned(),
                satisfied: cfg!(unix),
            }),
            "windows" => out.push(CfgAtom::Platform {
                text: "windows".to_owned(),
                satisfied: cfg!(windows),
            }),
            "target_os" | "target_family" | "target_arch" => {
                let value = tail
                    .strip_prefix('=')
                    .map(str::trim)
                    .and_then(|value| value.strip_prefix('"'))
                    .and_then(|value| value.split('"').next())
                    .unwrap_or_else(|| {
                        panic!(
                            "{}:{line}: `cfg({predicate})` names {head} without a \
                             string literal value; the inventory cannot decide \
                             whether the test runs on this host",
                            path.display()
                        )
                    });
                out.push(CfgAtom::Platform {
                    text: format!("{head} = {value:?}"),
                    satisfied: host_satisfies(&head, value),
                });
                // Step past the consumed `= "value"` so its letters are not
                // re-read as another atom.
                let consumed = tail.find(value).unwrap_or(0) + value.len() + 1;
                let offset = rest.len() - tail.len() + consumed;
                rest = &rest[offset.min(rest.len())..];
            }
            other => panic!(
                "{}:{line}: `cfg({predicate})` uses the unsupported atom \
                 `{other}`. Liveness classification is fail-closed: extend \
                 `test_target_inventory::CfgAtom` in the change that \
                 introduces the predicate, so a hardware, feature or platform \
                 gate is never silently counted as default-live",
                path.display()
            ),
        }
    }
    out
}

/// Evaluate a `cfg` predicate for this host, with `debug_assertions` set by
/// `debug`. Fails closed through [`atoms`] on anything unsupported.
fn evaluate(predicate: &str, path: &Path, line: usize, debug: bool) -> bool {
    let predicate = predicate.trim();
    for (name, combine) in [("all", true), ("any", false)] {
        if let Some(body) = wrapped(predicate, name) {
            let mut result = combine;
            for part in split_top_level(body) {
                let value = evaluate(part, path, line, debug);
                if combine {
                    result &= value;
                } else {
                    result |= value;
                }
            }
            return result;
        }
    }
    if let Some(body) = wrapped(predicate, "not") {
        return !evaluate(body, path, line, debug);
    }
    match atoms(predicate, path, line).as_slice() {
        [CfgAtom::Test] => true,
        [CfgAtom::DebugAssertions] => debug,
        [CfgAtom::Platform { satisfied, .. }] => *satisfied,
        other => panic!(
            "{}:{line}: `cfg({predicate})` resolved to {other:?}, which the \
             inventory cannot evaluate as a single predicate",
            path.display()
        ),
    }
}

fn wrapped<'a>(predicate: &'a str, name: &str) -> Option<&'a str> {
    predicate
        .strip_prefix(name)?
        .trim_start()
        .strip_prefix('(')?
        .strip_suffix(')')
}

fn split_top_level(body: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (index, byte) in body.bytes().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b',' if depth == 0 => {
                parts.push(body[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    let tail = body[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
}
