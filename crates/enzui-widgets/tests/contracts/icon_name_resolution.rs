//! Resolution contracts for literal icon names in framework code.
//!
//! The production scan checks direct `enz_icon` and `Icon::named` literals in
//! `crates/*/src`. Unknown names render the bundled fallback circle.

use enzui_test_support::hand_roll_scan::{
    optional_dir_exists, production_rs_files, read_dir_sorted, read_source_or_panic,
    remove_cfg_test_items_preserving_literals,
};
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("cannot find workspace root")
        .to_path_buf()
}

fn source_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let crates = workspace_root().join("crates");
    for crate_path in read_dir_sorted(&crates) {
        // The shared fail-closed probe keeps `Path::is_dir`'s
        // symlink-following scope (a crate without `src/` is skipped) while a
        // permission or I/O failure reds instead of reading as `false`.
        if !optional_dir_exists(&crate_path) {
            continue;
        }
        let src = crate_path.join("src");
        if !optional_dir_exists(&src) {
            continue;
        }
        out.extend(
            production_rs_files(&src)
                .into_iter()
                .map(|rel| src.join(rel)),
        );
    }
    out.sort();
    assert!(
        out.len() > 100,
        "positive control: expected hundreds of source files, got {} — the \
         walker is looking in the wrong place",
        out.len()
    );
    out
}

const CONSTRUCTORS: [&str; 2] = ["enz_icon(", "Icon::named("];

struct Literal {
    file: PathBuf,
    line: usize,
    name: String,
}

fn literal_arg(rest: &str) -> Option<&str> {
    let inner = rest.strip_prefix('"')?;
    let end = inner.find('"')?;
    Some(&inner[..end])
}

fn production_literals() -> Vec<Literal> {
    let mut out = Vec::new();
    for file in source_files() {
        let text = remove_cfg_test_items_preserving_literals(&read_source_or_panic(&file));
        for (idx, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            for ctor in CONSTRUCTORS {
                let mut from = 0usize;
                while let Some(at) = line[from..].find(ctor) {
                    let start = from + at + ctor.len();
                    if let Some(name) = literal_arg(&line[start..]) {
                        out.push(Literal {
                            file: file.clone(),
                            line: idx + 1,
                            name: name.to_owned(),
                        });
                    }
                    from = start;
                }
            }
        }
    }
    out
}

#[test]
fn every_production_icon_name_resolves_to_a_bundled_glyph() {
    let literals = production_literals();
    // 19 literals since the probe and the doc-example crate left the
    // workspace; a blind walker sees none.
    assert!(
        literals.len() >= 15,
        "population floor: only {} production icon-name literals were scanned; \
         the scanner stopped seeing the tree, so the assertion below proves \
         nothing",
        literals.len()
    );

    assert!(
        enzui_icons::canonical_icon_name("enzui.definitely-not-an-icon").is_none(),
        "positive control failed: an impossible name reported as resolved, so \
         the sweep below cannot fail"
    );

    let unresolved: Vec<String> = literals
        .iter()
        .filter(|l| enzui_icons::canonical_icon_name(&l.name).is_none())
        .map(|l| {
            format!(
                "{}:{} asks for {:?}, which renders the fallback circle",
                l.file
                    .strip_prefix(workspace_root())
                    .unwrap_or_else(|e| {
                        panic!(
                            "icon_name_resolution: strip_prefix failed for {} against {}: {e}",
                            l.file.display(),
                            workspace_root().display()
                        )
                    })
                    .display(),
                l.line,
                l.name
            )
        })
        .collect();

    assert!(
        unresolved.is_empty(),
        "{} production icon name(s) do not resolve and silently paint \
         `assets/fallback.svg` (an outlined circle) instead:\n  {}\n\
         Spell the catalogue name (see crates/enzui-icons/src/aliases.rs \
         and icon-manifest.toml) or add the alias with the glyph.",
        unresolved.len(),
        unresolved.join("\n  ")
    );
}
