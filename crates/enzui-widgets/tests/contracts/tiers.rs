//! Tier lists and module sets the widget catalog is generated from.

pub(crate) use enzui_test_support::hand_roll_scan::declared_pub_mods;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Tier-1 modules that are app-facing API without a widget constructor.
pub(crate) const NON_WIDGET_TIER1: &[&str] = &["file_chooser", "match_ink", "window_lifecycle"];

const TIER2_MARKER: &str = "2. **Internal-composition";
const TIER3_MARKER: &str = "3. **Audit";
const ALIASES_MARKER: &str = "## Aliases";

/// The crate `src/` directory.
pub(crate) fn src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

// `mod@<name>` links between two `lib.rs` doc markers; a missing marker panics.
fn tier_modules(lib_rs: &str, from_marker: &str, to_marker: &str) -> BTreeSet<String> {
    let start = lib_rs
        .find(from_marker)
        .unwrap_or_else(|| panic!("lib.rs tier doc is missing marker {from_marker:?}"));
    let after = &lib_rs[start..];
    let end = after.find(to_marker).unwrap_or_else(|| {
        panic!("lib.rs tier doc: marker {to_marker:?} not found after {from_marker:?}")
    });
    let region = &after[..end];
    let mut set = BTreeSet::new();
    let mut from = 0usize;
    while let Some(rel) = region[from..].find("mod@") {
        let at = from + rel + "mod@".len();
        from = at;
        let name: String = region[at..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            set.insert(name);
        }
    }
    set
}

/// Public modules not listed as tier 2 or tier 3.
pub(crate) fn live_tier1(lib_rs: &str) -> BTreeSet<String> {
    let all = declared_pub_mods(lib_rs);
    let tier2 = tier_modules(lib_rs, TIER2_MARKER, TIER3_MARKER);
    let tier3 = tier_modules(lib_rs, TIER3_MARKER, ALIASES_MARKER);
    let listed: BTreeSet<String> = tier2.union(&tier3).cloned().collect();
    all.difference(&listed).cloned().collect()
}

/// Pull the machine-checked inventory list out of the `lib.rs` deprecation
/// doc (strict-form blocks: `//! <!-- deprecated-inventory:` ...
/// `//! <module>::<item-path>` lines ... `//! -->`). Marker-stripped lines =
/// documented deprecated items (`<module>::<item-path>`).
pub(crate) fn documented_deprecated_inventory(lib_rs: &str) -> BTreeSet<String> {
    let open = "deprecated-inventory:";
    let close = "-->";
    let start = lib_rs.find(open).unwrap_or_else(|| {
        panic!(
            "lib.rs deprecation doc is missing the machine-checked \
             `<!-- deprecated-inventory:` block. Add it so this test can \
             enforce the doc against the source."
        )
    });
    let after = &lib_rs[start + open.len()..];
    let end = after
        .find(close)
        .expect("deprecated-inventory block not closed with `-->`");
    let block = &after[..end];

    let mut set = BTreeSet::new();
    for line in block.lines() {
        let t = line.trim_start().trim_start_matches("//!").trim();
        if t.is_empty() {
            continue;
        }
        set.insert(t.to_owned());
    }
    set
}

/// One `pub use <module>::{...};` statement, flattened to `(module, name)`
/// pairs where `name` is the source-side identifier (the part before any
/// `as`). Reads only real code via the shared code mask, because the `lib.rs` doc
/// block quotes `pub use` lines as examples.
pub(crate) fn root_reexports(lib_rs: &str) -> BTreeSet<(String, String)> {
    let mask = enzui_test_support::hand_roll_scan::rust_code_mask(lib_rs);
    let mut out = BTreeSet::new();
    let needle = "pub use ";
    let mut from = 0usize;
    while let Some(rel) = lib_rs[from..].find(needle) {
        let at = from + rel;
        from = at + needle.len();
        if !mask[at] {
            continue;
        }
        // Must begin a line, like `declared_pub_mods` requires.
        let line_start = lib_rs[..at].rfind('\n').map_or(0, |p| p + 1);
        if !lib_rs[line_start..at].trim().is_empty() {
            continue;
        }
        let rest = &lib_rs[at + needle.len()..];
        let module: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if module.is_empty() {
            continue;
        }
        let after = &rest[module.len()..];
        let Some(after) = after.strip_prefix("::") else {
            continue;
        };
        // Body is either `{ a, b, c };` or `name;`.
        let Some(semi) = after.find(';') else {
            continue;
        };
        let body = after[..semi].trim();
        let body = body
            .strip_prefix('{')
            .and_then(|b| b.strip_suffix('}'))
            .unwrap_or(body);
        for part in body.split(',') {
            let part = part.trim();
            if part.is_empty() || part.starts_with("//") {
                continue;
            }
            // Skip an embedded `// comment` line inside a braced group.
            let Some(first) = part.split_whitespace().next() else {
                continue;
            };
            let name: String = first
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                out.insert((module.clone(), name));
            }
        }
    }
    out
}

pub(crate) const CATALOG_MARKER: &str = "# Widget catalog";
pub(crate) const CATALOG_END_MARKER: &str = "# Export / alias / deprecation boundary";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CatalogEntry {
    /// The `mod@<name>` target -- a tier-1 module name.
    pub(crate) module: String,
    pub(crate) summary: String,
}

/// One `### ` group in the § Widget catalog: its heading, the prose paragraph
/// between the heading and the first bullet (may be empty), and its entries.
///
/// The prose is part of the *parsed* structure rather than something a
/// generator re-derives, because `docs/widgets.md` carries it through and
/// there must be exactly one source for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CatalogGroup {
    pub(crate) heading: String,
    /// Doc-comment lines between the heading and the first bullet, `//!`
    /// stripped, blank lines preserved as empty strings, leading/trailing
    /// blanks removed.
    pub(crate) prose: Vec<String>,
    pub(crate) entries: Vec<CatalogEntry>,
}

/// Parse the § Widget catalog region into groups.
///
/// Groups = `//! ### <heading>` lines; entries = `mod@<name>` intra-doc links
/// on `- ` bullets beneath (tier-list-identical link forms = machine-
/// checkability). Three `mod@` kinds are DELIBERATELY non-entries: above-first-
/// heading section prose (catalog-explaining [`media_controls`](mod@media_controls)
/// references — hence first-heading-starting collection); group-prose-
/// paragraph member namings ("[`enz_text`] is the primitive..." would double-
/// list members); second-`mod@`s in bullet summaries (first-bullet-links =
/// entries; rests = cross-references). SOLE catalog-format parsers:
/// `docs/widgets.md` generates from here (see `widget_catalog_page.rs`) —
/// second parsers cannot drift.
pub(crate) fn parse_catalog(lib_rs: &str) -> Vec<CatalogGroup> {
    let start = lib_rs.find(CATALOG_MARKER).unwrap_or_else(|| {
        panic!(
            "lib.rs module doc is missing the {CATALOG_MARKER:?} section. The \
             widget catalog is the positive enumeration of tier 1; without it \
             tier 1 is defined only by subtraction."
        )
    });
    let region_end = lib_rs[start..]
        .find(CATALOG_END_MARKER)
        .unwrap_or_else(|| panic!("catalog region: {CATALOG_END_MARKER:?} not found after it"));
    let region = &lib_rs[start..start + region_end];

    /// The first `mod@<name>` target in `t`, if any.
    fn first_mod_link(t: &str) -> Option<String> {
        let at = t.find("mod@")? + "mod@".len();
        let name: String = t[at..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        (!name.is_empty()).then_some(name)
    }

    let mut groups: Vec<CatalogGroup> = Vec::new();
    for line in region.lines() {
        let t = line.trim_start().trim_start_matches("//!").trim();
        if let Some(heading) = t.strip_prefix("### ") {
            groups.push(CatalogGroup {
                heading: heading.trim().to_owned(),
                prose: Vec::new(),
                entries: Vec::new(),
            });
            continue;
        }
        let Some(current) = groups.last_mut() else {
            continue; // prose above the first group heading
        };
        if let Some(bullet) = t.strip_prefix("- ")
            && let Some(module) = first_mod_link(bullet)
        {
            // Later spaced hyphens belong to the summary.
            let (_, summary) = bullet.split_once(" - ").unwrap_or_else(|| {
                panic!("catalog bullet requires a spaced-hyphen separator: {bullet}")
            });
            let summary = summary.trim().to_owned();
            current.entries.push(CatalogEntry { module, summary });
            continue;
        }
        // Prose only counts before the group's first bullet; a line after the
        // bullets is a continuation the generated page has no place for.
        if current.entries.is_empty() {
            current.prose.push(t.to_owned());
        }
    }

    for g in &mut groups {
        while g.prose.first().is_some_and(std::string::String::is_empty) {
            g.prose.remove(0);
        }
        while g.prose.last().is_some_and(std::string::String::is_empty) {
            g.prose.pop();
        }
    }
    groups
}
