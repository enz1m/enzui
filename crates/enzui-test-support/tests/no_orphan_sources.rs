//! Every Rust file in a framework crate is compiled from some target.
//!
//! rustc ignores a `.rs` file that no `mod`, `#[path]` or `include!` reaches,
//! so a test file split out of a larger one and never wired in simply stops
//! running, with every suite green. This gate walks each crate from its Cargo
//! targets, follows those three kinds of reference, and names any `.rs` file
//! under `src/`, `tests/`, `examples/` or `benches/` that nothing reaches.
//!
//! It replaces the per-split "reconstruction" gates that each checked one
//! past file split for the same failure.

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

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn workspace_crates() -> Vec<PathBuf> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut dirs: Vec<PathBuf> = fs::read_dir(&crates)
        .expect("crates directory")
        .map(|entry| entry.expect("crate entry").path())
        .filter(|dir| dir.join("Cargo.toml").is_file())
        .collect();
    dirs.sort();
    assert!(dirs.len() >= 16, "the crate scan lost the tree: {dirs:?}");
    dirs
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// The crate's target roots: the conventional ones plus every `path = ".."`
/// Cargo.toml declares for a lib, bin, test, example or bench.
fn roots(krate: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for conventional in ["src/lib.rs", "src/main.rs", "build.rs"] {
        roots.push(krate.join(conventional));
    }
    for dir in ["src/bin", "tests", "examples", "benches"] {
        if let Ok(entries) = fs::read_dir(krate.join(dir)) {
            for entry in entries {
                let path = entry.expect("dir entry").path();
                if path.extension().is_some_and(|ext| ext == "rs") {
                    roots.push(path);
                } else if path.join("main.rs").is_file() {
                    roots.push(path.join("main.rs"));
                }
            }
        }
    }
    let manifest = fs::read_to_string(krate.join("Cargo.toml")).expect("Cargo.toml");
    for line in manifest.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("path")
            && let Some(value) = rest.trim_start().strip_prefix('=')
        {
            let value = value.trim().trim_matches('"');
            if value.ends_with(".rs") {
                roots.push(krate.join(value));
            }
        }
    }
    roots.into_iter().filter(|root| root.is_file()).collect()
}

/// Whether `file` resolves its `mod name;` declarations beside itself (a
/// crate root or a `mod.rs`) rather than in a directory named after itself.
fn owns_its_directory(file: &Path, roots: &[PathBuf]) -> bool {
    file.file_name().is_some_and(|name| name == "mod.rs") || roots.iter().any(|root| root == file)
}

/// The string literal starting at the first `"` of `text`, if any.
fn first_literal(text: &str) -> Option<&str> {
    let start = text.find('"')? + 1;
    let end = start + text[start..].find('"')?;
    Some(&text[start..end])
}

/// Every file `file` compiles in through `mod`, `#[path]` or `include!`.
/// `include_str!` only reads a file's text, so it does not count. Inline module blocks (`mod a { mod b; }`) nest the lookup
/// directory, which is tracked by brace depth.
fn references(file: &Path, roots: &[PathBuf]) -> Vec<PathBuf> {
    let source = fs::read_to_string(file).unwrap_or_default();
    let parent = file.parent().expect("file has a parent").to_path_buf();
    let base = if owns_its_directory(file, roots) {
        parent.clone()
    } else {
        parent.join(file.file_stem().expect("file stem"))
    };
    let mut found = Vec::new();
    let mut pending_path: Option<String> = None;
    // Inline `mod name {` blocks open a nested directory until their brace
    // closes.
    let mut inline: Vec<(String, usize)> = Vec::new();
    let mut depth = 0usize;
    for raw in source.lines() {
        let line = raw.trim();
        if line.starts_with("//") {
            continue;
        }
        if let Some(at) = line.find("include!(")
            && let Some(literal) = first_literal(&line[at..])
        {
            let target = if line[at..].contains("CARGO_MANIFEST_DIR") {
                let krate = roots[0]
                    .ancestors()
                    .find(|a| a.join("Cargo.toml").is_file());
                krate.map(|k| k.join(literal.trim_start_matches('/')))
            } else {
                Some(parent.join(literal))
            };
            if let Some(target) = target {
                found.push(target);
            }
        }
        if line.starts_with("#[path") {
            pending_path = first_literal(line).map(str::to_owned);
            continue;
        }
        let declaration = line
            .trim_start_matches("pub(crate) ")
            .trim_start_matches("pub(super) ")
            .trim_start_matches("pub ");
        if let Some(rest) = declaration.strip_prefix("mod ") {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            let tail = rest[name.len()..].trim();
            let dir = inline
                .iter()
                .fold(base.clone(), |dir, (module, _)| dir.join(module));
            if tail.starts_with(';') {
                match pending_path.take() {
                    Some(path) => found.push(if inline.is_empty() {
                        parent.join(path)
                    } else {
                        dir.join(path)
                    }),
                    None => {
                        found.push(dir.join(format!("{name}.rs")));
                        found.push(dir.join(&name).join("mod.rs"));
                    }
                }
            } else if tail.starts_with('{') {
                inline.push((name, depth));
            }
        } else if !line.starts_with("#[") {
            pending_path = None;
        }
        for c in code_only(line).chars() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth = depth.saturating_sub(1);
                    if inline.last().is_some_and(|(_, opened)| *opened == depth) {
                        inline.pop();
                    }
                }
                _ => {}
            }
        }
    }
    found
}

/// `line` without string and char literals or a trailing comment, so braces
/// inside `"{}"` or `'{'` do not count.
fn code_only(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                while let Some(inner) = chars.next() {
                    match inner {
                        '\\' => {
                            chars.next();
                        }
                        '"' => break,
                        _ => {}
                    }
                }
            }
            '\'' => {
                // A char literal is at most a few chars; a lifetime has no
                // closing quote and is left alone.
                let rest: String = chars.clone().take(10).collect();
                if let Some(end) = rest.find('\'')
                    && end <= 8
                {
                    for _ in 0..=end {
                        chars.next();
                    }
                }
            }
            '/' if chars.peek() == Some(&'/') => break,
            _ => out.push(c),
        }
    }
    out
}

fn normalise(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[test]
fn every_rust_file_in_every_crate_is_reached_by_a_target() {
    let mut orphans = Vec::new();
    let mut scanned = 0usize;
    for krate in workspace_crates() {
        let roots: Vec<PathBuf> = roots(&krate).iter().map(|r| normalise(r)).collect();
        let mut reached: BTreeSet<PathBuf> = BTreeSet::new();
        let mut queue: Vec<PathBuf> = roots.clone();
        while let Some(file) = queue.pop() {
            if !reached.insert(file.clone()) {
                continue;
            }
            for target in references(&file, &roots) {
                let target = normalise(&target);
                if target.is_file() && !reached.contains(&target) {
                    queue.push(target);
                }
            }
        }
        let mut files = Vec::new();
        for dir in ["src", "tests", "examples", "benches"] {
            rs_files(&krate.join(dir), &mut files);
        }
        for file in files {
            scanned += 1;
            let file = normalise(&file);
            if !reached.contains(&file) {
                orphans.push(file.display().to_string());
            }
        }
    }
    assert!(scanned > 1000, "the scan lost the tree: {scanned} files");
    assert!(
        orphans.is_empty(),
        "{} Rust file(s) are reached by no Cargo target, `mod`, `#[path]` or \
         `include!`, so rustc never compiles them:\n{}",
        orphans.len(),
        orphans.join("\n")
    );
}
