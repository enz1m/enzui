//! `docs/widgets.md` is generated from the `# Widget catalog` doc
//! block in `lib.rs`, and a stale copy is a red build.
//!
//! # Why generated
//!
//! Hand-written widget lists are this repo's most reliably-rotting artifacts
//! (precedents: "86 widgets" claimed vs 185+-strong crates; 15-of-19 stale
//! README component rows). Stale catalogs are WORSE than none: nonexistence-
//! claiming pages drive app migrations to hand-roll shipped widgets (six
//! precedents). So pages emit from the already-machine-checked catalog block
//! (DERIVED memberships: tier 1 = all `pub mod` ∖ (tier2 ∪ tier3); declared
//! groupings + prose beside it).
//!
//! # Where the generator lives, and why here
//!
//! `#[test]`s in the `it` binary, beside reused parsers - never `examples/`
//! binaries or `xtask`s: sibling-`export_policy.rs` parsers (`parse_catalog`,
//! `live_tier1`, `declared_pub_mods`, `root_reexports`, `code_mask`) are
//! unreachable outside this test binary (second same-format parsers = the
//! drift this prevents); staleness gates must run as the suite runs
//! (`cargo wt --test contracts widget_catalog_page` -
//! `#[test]`s are that by construction; unrun gates aren't gates); costs are
//! milliseconds of string-building over already-parsed structures inside
//! already-compiled binaries (separate crates = build targets for nothing).
//!
//! # The gate
//!
//! **`docs_widgets_md_is_not_stale`** regenerates and compares. Any drift
//! (new tier-1s, edited summaries/paragraphs, markdown hand-edits) fails with
//! refresh instructions. Generated-once ≠ generated-currently.
//!
//! # Refreshing the page
//!
//! ```sh
//! ENZUI_WRITE_WIDGET_DOC=1 cargo wt --test contracts widget_catalog_page
//! ```
//!
//! The env var makes staleness tests write instead of assert. Deliberately
//! non-default: self-rewriting-expectation generators can never fail.

use enzui_test_support::hand_roll_scan::{
    optional_dir_exists, optional_file_exists, read_dir_sorted, read_source_or_panic, walk_rs_files,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::tiers::{
    CatalogEntry, CatalogGroup, NON_WIDGET_TIER1, declared_pub_mods,
    documented_deprecated_inventory, live_tier1, parse_catalog, root_reexports, src_dir,
};

/// The heading that opens the keyword-alias table in the `# Widget catalog`
/// doc block. A change to this string that is not matched in
/// [`parse_keyword_aliases`] would make the generator silently emit an empty
/// section, which is why the parser panics rather than defaulting when this
/// marker is missing.
pub(crate) const ALIAS_SECTION_MARKER: &str = "## Finding a widget by keyword";

/// The public Naming and imports contract, emitted verbatim into
/// `docs/widgets.md` so an app author or code agent reads one grammar instead
/// of inferring three from whichever common component they saw first. The
/// grammar is stated here and enforced by the gates in `naming_grammar.rs`.
pub(crate) const NAMING_CONTRACT_MD: &str = "\
## Naming and imports\n\n\
Every component in this catalog follows one grammar, so a name tells you what \
it is:\n\n\
- **Canonical component module** - the unprefixed `snake_case` *concept* \
(`text`, `table`, `button`, `image`), never a brand-prefixed or `_widget`-\
suffixed path.\n\
- **Canonical visual constructor** - prefix-first `enz_snake_case` \
(`enz_text`, `enz_disabled_button`). The `enz_` marker always comes first, \
never in the middle.\n\
- **Canonical visual builder / type** - `EnzUpperCamelCase` (`EnzText`, \
`EnzButton`).\n\
- **Models, policies, controllers, measurements and support helpers** use \
ordinary Rust names and do **not** acquire an `Enz`/`enz_` marker merely \
because they support a widget (`TextSize`, `CellCoord`, `ToastHost`, \
`FileRequest`, lifecycle policies).\n\
- Every canonical tier-1 constructor and every `Enz*` builder type is \
reachable **both** from its module and from the crate root \
(`enzui_widgets::enz_button` and `enzui_widgets::button::enz_button` \
both resolve).\n\
- **Compatibility aliases are not canonical catalog entries.** The historical \
brand-prefixed module paths (`enz_text`, `enz_table`, `image_widget`, …) and \
the reversed / bare constructor spellings (`disabled_enz_button`, \
`revision_text`) keep resolving for existing callers, but they do not appear \
in this catalog or in `docs/widgets.json`; reach for the canonical spelling in \
new code.\n\n";

/// One row of the keyword-alias table. `keywords` is the set of terms an app
/// author might type; `targets` records module links, same-crate item links,
/// and exceptional out-of-crate cross-references that answer them;
/// `raw_second_cell` is the markdown cell verbatim, so the rendered page shows
/// the *reason* alongside the pointer.
#[derive(Debug, Clone)]
pub(crate) struct KeywordAlias {
    pub(crate) keywords: Vec<String>,
    pub(crate) targets: Vec<AliasTarget>,
    pub(crate) raw_first_cell: String,
    pub(crate) raw_second_cell: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AliasTarget {
    /// A `mod@<name>` link to a module in this crate.
    Module(String),
    /// A shorthand item link rooted in a declared public module of this crate.
    /// The full path is retained so consumers cannot silently discard the
    /// distinction between an item link and an external alias.
    InternalItem(String),
    /// An intra-doc link whose root is not a declared module of this crate.
    /// The parser records the raw path (e.g. `enzui_core::portal`) so the
    /// resolution test can decide it against an allowlist. Unknown roots stay
    /// external and therefore fail closed.
    External(String),
}

/// Parse the `## Finding a widget by keyword` section into rows.
///
/// Parses the markdown alias tables (from each marker to the next heading of
/// the same or higher level): each `| A | B |` row becomes a [`KeywordAlias`]
/// with the backticked terms of cell A as keywords and the intra-doc links of
/// cell B as targets. A missing marker panics, so a broken parse cannot
/// produce an empty page that gets blessed.
pub(crate) fn parse_keyword_aliases(lib_rs: &str) -> Vec<KeywordAlias> {
    let pub_mods = declared_pub_mods(lib_rs);
    let start = lib_rs.find(ALIAS_SECTION_MARKER).unwrap_or_else(|| {
        panic!(
            "lib.rs module doc is missing the {ALIAS_SECTION_MARKER:?} \
             heading. That heading is what the keyword-alias table lives \
             under; without it the keyword discoverability index is \
             gone silently and the generated docs/widgets.md loses a \
             whole section."
        )
    });
    let tail = &lib_rs[start..];

    // Stop at the next same-or-shallower `//! ##` or `//! # ` heading. The
    // group headings that follow are `//! ###` and must not terminate the
    // section - they live inside the parent `# Widget catalog` heading.
    let mut end = tail.len();
    for (i, line) in tail.lines().enumerate() {
        if i == 0 {
            continue; // this IS the marker line
        }
        let t = line.trim_start().trim_start_matches("//!").trim();
        if t.starts_with("## ") || t.starts_with("# ") {
            // Locate the byte offset of the start of this line within `tail`.
            let mut byte = 0usize;
            for (j, l) in tail.lines().enumerate() {
                if j == i {
                    end = byte;
                    break;
                }
                byte += l.len() + 1; // +1 for the '\n'
            }
            break;
        }
    }
    let section = &tail[..end];

    let mut rows = Vec::new();
    for line in section.lines() {
        let t = line.trim_start().trim_start_matches("//!").trim();
        if !t.starts_with('|') {
            continue;
        }
        // Skip the header row (`| you searched for | reach for |`) and the
        // separator row (`|---|---|`). We identify these by content: the
        // separator has only dashes/pipes/whitespace, and the header names
        // literal words with no backticks or links.
        let cells: Vec<&str> = t
            .trim_start_matches('|')
            .trim_end_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        assert_eq!(
            cells.len(),
            2,
            "keyword table row must contain a keyword cell and a target cell: {line}"
        );
        if cells.iter().all(|c| {
            c.chars()
                .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
        }) {
            continue;
        }
        if cells[0] == "you searched for" && cells[1] == "reach for" {
            continue;
        }
        let keywords = extract_backticked(cells[0]);
        let targets = extract_alias_targets(cells[1], &pub_mods);
        rows.push(KeywordAlias {
            keywords,
            targets,
            raw_first_cell: cells[0].to_owned(),
            raw_second_cell: cells[1].to_owned(),
        });
    }
    rows
}

/// Return each `` `x` `` token in `text` as a bare string (no backticks, no
/// surrounding whitespace).
pub(crate) fn extract_backticked(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(a) = rest.find('`') {
        rest = &rest[a + 1..];
        if let Some(b) = rest.find('`') {
            let token = rest[..b].trim().to_owned();
            if !token.is_empty() {
                out.push(token);
            }
            rest = &rest[b + 1..];
        } else {
            break;
        }
    }
    out
}

/// Return every intra-doc target in a markdown cell. Recognises `mod@<name>`
/// (in-crate module reference) and `[`path::to::item`]` (bracketed intra-doc
/// link with no explicit href). A shorthand path rooted in
/// `declared_pub_modules` is an [`AliasTarget::InternalItem`]; every other root
/// is [`AliasTarget::External`] so an allowlist consumer can fail closed.
pub(crate) fn extract_alias_targets(
    text: &str,
    declared_pub_modules: &BTreeSet<String>,
) -> Vec<AliasTarget> {
    let mut out = Vec::new();

    // `mod@<name>` links.
    let mut rest = text;
    while let Some(a) = rest.find("mod@") {
        let after = &rest[a + "mod@".len()..];
        let name: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            out.push(AliasTarget::Module(name));
        }
        rest = after;
    }

    // Bracketed intra-doc references of the shape [`a::b::c`] with NO
    // trailing `(...)`. `[`x`](mod@x)` is already captured above; we only
    // want the shorthand form here.
    let mut i = 0usize;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'[' && i + 1 < bytes.len() && bytes[i + 1] == b'`' {
            // Find the closing `` ` `` then `]`.
            let start = i + 2;
            let Some(bt_close) = text[start..].find('`') else {
                break;
            };
            let path = &text[start..start + bt_close];
            let after_close = start + bt_close + 1;
            if after_close < text.len() && bytes[after_close] == b']' {
                let past = after_close + 1;
                // Only accept the shorthand form (no explicit `(...)` href).
                let is_shorthand = past >= text.len() || bytes[past] != b'(';
                if is_shorthand && path.contains("::") {
                    // Classification is derived from this crate's actual public
                    // module declarations, never from Rust naming style. This
                    // keeps lowercase item paths internal, keeps external paths
                    // containing uppercase item segments external, and leaves a
                    // newly introduced external crate root visible to the
                    // allowlist gate rather than silently dropping it.
                    let root = path.split("::").next().unwrap_or_default();
                    if declared_pub_modules.contains(root) {
                        out.push(AliasTarget::InternalItem(path.to_owned()));
                    } else {
                        out.push(AliasTarget::External(path.to_owned()));
                    }
                }
                i = past;
                continue;
            }
        }
        i += 1;
    }

    out
}

// ---------------------------------------------------------------------------
// The `composes_with` relation
// ---------------------------------------------------------------------------

/// The heading that opens the composition table in the `# Widget catalog` doc
/// block. As with the alias marker, a rename that is not
/// matched here would make the generator silently emit an empty section, so
/// [`parse_composes_with`] panics rather than defaulting.
pub(crate) const COMPOSES_SECTION_MARKER: &str = "## Composes with";

/// One row of the composition table: a container module and the modules that
/// go inside it.
///
/// - `module` is the `mod@<name>` target in the first cell.
/// - `targets` are every `mod@<name>` in the second cell, in table order.
/// - `raw_*_cell` preserve the markdown verbatim so the generated page shows
///   the same reasoning the rustdoc block records.
#[derive(Debug, Clone)]
pub(crate) struct ComposesWith {
    pub(crate) module: String,
    pub(crate) targets: Vec<String>,
    pub(crate) raw_module_cell: String,
    pub(crate) raw_targets_cell: String,
    pub(crate) raw_why_cell: String,
}

/// Return the slice of `lib_rs` from `marker` up to the next same-or-shallower
/// `//! ##` / `//! # ` heading (a `###` group heading does not terminate a
/// section, because those live inside the parent `# Widget catalog`).
///
/// Factored out because three sections now need exactly this, and a fourth
/// copy of the scan is a fourth chance for one of them to drift.
pub(crate) fn doc_section<'a>(lib_rs: &'a str, marker: &str) -> Option<&'a str> {
    let start = lib_rs.find(marker)?;
    let tail = &lib_rs[start..];
    let mut end = tail.len();
    let mut byte = 0usize;
    for (i, line) in tail.lines().enumerate() {
        if i > 0 {
            let t = line.trim_start().trim_start_matches("//!").trim();
            if t.starts_with("## ") || t.starts_with("# ") {
                end = byte;
                break;
            }
        }
        byte += line.len() + 1; // +1 for the '\n'
    }
    Some(&tail[..end])
}

/// Split a markdown table line into its trimmed cells, or `None` when the
/// line is not a data row (not a table line, wrong arity, a separator, or the
/// header row identified by its literal first cell).
pub(crate) fn table_row_cells<'a>(
    line: &'a str,
    arity: usize,
    header_first: &str,
) -> Option<Vec<&'a str>> {
    let t = line.trim_start().trim_start_matches("//!").trim();
    if !t.starts_with('|') {
        return None;
    }
    let cells: Vec<&str> = t
        .trim_start_matches('|')
        .trim_end_matches('|')
        .split('|')
        .map(str::trim)
        .collect();
    if cells.len() != arity {
        return None;
    }
    if cells.iter().all(|c| {
        c.chars()
            .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
    }) {
        return None;
    }
    if cells[0] == header_first {
        return None;
    }
    Some(cells)
}

/// Parse the `## Composes with` section into rows.
///
/// A missing marker is a panic, for the same reason
/// [`parse_keyword_aliases`] panics: a silent empty parse would still let the
/// generator produce a page, and the staleness gate would then bless a whole
/// section that vanished.
pub(crate) fn parse_composes_with(lib_rs: &str) -> Vec<ComposesWith> {
    let pub_mods = declared_pub_mods(lib_rs);
    let section = doc_section(lib_rs, COMPOSES_SECTION_MARKER).unwrap_or_else(|| {
        panic!(
            "lib.rs module doc is missing the {COMPOSES_SECTION_MARKER:?} \
             heading. That heading is what the composition table lives under; \
             without it the widget->collaborator edges are gone \
             silently and the generated docs/widgets.md and widgets.json lose \
             a whole section."
        )
    });

    let mut rows = Vec::new();
    for line in section.lines() {
        let Some(cells) = table_row_cells(line, 3, "widget") else {
            continue;
        };
        let Some(AliasTarget::Module(module)) = extract_alias_targets(cells[0], &pub_mods)
            .into_iter()
            .next()
        else {
            panic!(
                "composes-with row's first cell {:?} does not name a \
                 `mod@<name>` widget. Every row must say WHICH container the \
                 collaborators go inside.",
                cells[0]
            )
        };
        let targets: Vec<String> = extract_alias_targets(cells[1], &pub_mods)
            .into_iter()
            .filter_map(|t| match t {
                AliasTarget::Module(m) => Some(m),
                AliasTarget::InternalItem(_) | AliasTarget::External(_) => None,
            })
            .collect();
        rows.push(ComposesWith {
            module,
            targets,
            raw_module_cell: cells[0].to_owned(),
            raw_targets_cell: cells[1].to_owned(),
            raw_why_cell: cells[2].to_owned(),
        });
    }
    rows
}

// ---------------------------------------------------------------------------
// The `## Choose between siblings` relation
// ---------------------------------------------------------------------------

/// The heading that opens the sibling-choice table in the `# Widget catalog`
/// doc block. As with the alias and composes markers, a rename that is
/// not matched here would make the generator silently emit an empty section,
/// so [`parse_sibling_choices`] panics rather than defaulting.
pub(crate) const SIBLING_CHOICE_SECTION_MARKER: &str = "## Choose between siblings";

/// One row of the sibling-choice table: an intent family, the shipped widgets
/// that could answer it, the question a caller must answer to pick between
/// them, and the per-sibling decision.
///
/// - `family` = first-cell stable slugs (`shell`, `titlebar-band`, …):
///   intent-naming plain text (no `mod@` links - not widgets).
/// - `siblings` = second-cell `mod@<name>` targets.
/// - `raw_*_cell` = verbatim markdown (generated pages show rustdoc-recorded
///   reasoning identically).
#[derive(Debug, Clone)]
pub(crate) struct SiblingChoice {
    pub(crate) family: String,
    pub(crate) siblings: Vec<String>,
    pub(crate) raw_family_cell: String,
    pub(crate) raw_siblings_cell: String,
    pub(crate) raw_question_cell: String,
    pub(crate) raw_pick_cell: String,
}

/// Parse the `## Choose between siblings` section into rows.
///
/// A missing marker is a panic, for the same reason [`parse_keyword_aliases`]
/// panics: a silent empty parse would still let the generator produce a page,
/// and the staleness gate would then bless a whole section that vanished.
pub(crate) fn parse_sibling_choices(lib_rs: &str) -> Vec<SiblingChoice> {
    let section = doc_section(lib_rs, SIBLING_CHOICE_SECTION_MARKER).unwrap_or_else(|| {
        panic!(
            "lib.rs module doc is missing the {SIBLING_CHOICE_SECTION_MARKER:?} \
             heading. That heading is what the sibling-choice table lives \
             under; without it the intent->family disambiguation is \
             gone silently and the generated docs/widgets.md loses a whole \
             section."
        )
    });

    let declared_pub_modules = declared_pub_mods(lib_rs);
    let mut rows = Vec::new();
    for line in section.lines() {
        let Some(cells) = table_row_cells(line, 4, "family") else {
            continue;
        };
        // The family slug is plain text in the first cell: no backticks, no
        // `mod@` links. Trim only - the raw cell is preserved separately.
        let family = cells[0].trim().to_owned();
        assert!(
            !family.is_empty(),
            "sibling-choice row's `family` cell is empty; every row must \
             name an intent family (e.g. `shell`, `segmented-control`)."
        );
        let siblings: Vec<String> = extract_alias_targets(cells[1], &declared_pub_modules)
            .into_iter()
            .filter_map(|t| match t {
                AliasTarget::Module(m) => Some(m),
                AliasTarget::InternalItem(_) | AliasTarget::External(_) => None,
            })
            .collect();
        rows.push(SiblingChoice {
            family,
            siblings,
            raw_family_cell: cells[0].to_owned(),
            raw_siblings_cell: cells[1].to_owned(),
            raw_question_cell: cells[2].to_owned(),
            raw_pick_cell: cells[3].to_owned(),
        });
    }
    rows
}

/// The prose lines between the `## Choose between siblings` marker and its
/// first table row, preserved verbatim by [`render`] so the generated page
/// carries the same reason the rustdoc block records.
pub(crate) fn parse_sibling_choice_prose(lib_rs: &str) -> Vec<String> {
    let Some(section) = doc_section(lib_rs, SIBLING_CHOICE_SECTION_MARKER) else {
        return Vec::new();
    };
    let mut before: Vec<String> = Vec::new();
    for (i, line) in section.lines().enumerate() {
        if i == 0 {
            continue; // the marker line itself
        }
        let t = line.trim_start().trim_start_matches("//!").trim();
        if t.starts_with('|') {
            break;
        }
        before.push(t.to_owned());
    }
    while before.first().is_some_and(std::string::String::is_empty) {
        before.remove(0);
    }
    while before.last().is_some_and(std::string::String::is_empty) {
        before.pop();
    }
    before
}

/// The prose lines between the `## Composes with` marker and its first table
/// row, preserved verbatim by [`render`] so the generated page carries the
/// same reason the rustdoc block records.
pub(crate) fn parse_composes_prose(lib_rs: &str) -> Vec<String> {
    let Some(section) = doc_section(lib_rs, COMPOSES_SECTION_MARKER) else {
        return Vec::new();
    };
    let mut before: Vec<String> = Vec::new();
    for (i, line) in section.lines().enumerate() {
        if i == 0 {
            continue; // the marker line itself
        }
        let t = line.trim_start().trim_start_matches("//!").trim();
        if t.starts_with('|') {
            break;
        }
        before.push(t.to_owned());
    }
    while before.first().is_some_and(std::string::String::is_empty) {
        before.remove(0);
    }
    while before.last().is_some_and(std::string::String::is_empty) {
        before.pop();
    }
    before
}

/// The workspace root, from `CARGO_MANIFEST_DIR` (`crates/enzui-widgets`).
pub(crate) fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two levels above the crate manifest")
        .to_path_buf()
}

pub(crate) fn widgets_md_path() -> PathBuf {
    workspace_root().join("docs/widgets.md")
}

/// The machine-readable sibling of `docs/widgets.md`, emitted
/// from the same [`read_model`] so it cannot drift.
pub(crate) fn widgets_json_path() -> PathBuf {
    workspace_root().join("docs/widgets.json")
}

pub(crate) fn example_rs_path() -> PathBuf {
    workspace_root().join("crates/enzui-widgets/tests/fixtures/widget_catalog.rs")
}

// Only explicitly marked, compiled example regions are published in the catalogue.
pub(crate) const EXAMPLE_REGIONS: &[&str] = &["shape", "compose"];

/// Pull the lines between `// catalog-example:<name>:start` and
/// `// catalog-example:<name>:end` out of the compiled documentation fixture,
/// with the common leading indentation removed.
pub(crate) fn example_region(example_rs: &str, name: &str) -> Vec<String> {
    let start = format!("// catalog-example:{name}:start");
    let end = format!("// catalog-example:{name}:end");
    let mut out = Vec::new();
    let mut inside = false;
    let mut found_start = false;
    let mut found_end = false;
    for line in example_rs.lines() {
        if line.trim() == start {
            inside = true;
            found_start = true;
            continue;
        }
        if line.trim() == end {
            inside = false;
            found_end = true;
            continue;
        }
        if inside {
            out.push(line.to_owned());
        }
    }
    assert!(
        found_start && found_end,
        "tests/fixtures/widget_catalog.rs is missing the `{name}` region markers \
         ({start:?} / {end:?}). The generated page includes that region, so a \
         renamed or deleted marker must be a hard failure, not an empty block."
    );
    assert!(
        !out.is_empty(),
        "the `{name}` region in tests/fixtures/widget_catalog.rs is empty"
    );

    let indent = out
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    out.iter()
        .map(|l| {
            if l.len() >= indent {
                l[indent..].to_string()
            } else {
                l.trim_start().to_owned()
            }
        })
        .collect()
}

// Names absent from root re-exports retain their module path in generated imports.
pub(crate) fn module_api(reexports: &BTreeSet<(String, String)>, module: &str) -> Vec<String> {
    let at_root: BTreeSet<&str> = reexports
        .iter()
        .filter(|(m, _)| m == module)
        .map(|(_, n)| n.as_str())
        .collect();

    let mut ctors: Vec<String> = Vec::new();
    let mut types: Vec<String> = Vec::new();
    for name in source_public_items(module) {
        let path = if at_root.contains(name.as_str()) {
            name.clone()
        } else {
            format!("{module}::{name}")
        };
        if name.starts_with("enz_") {
            ctors.push(path);
        } else {
            types.push(path);
        }
    }

    // The `enz_<module>` form is the primary constructor where it exists
    // (`enz_button` for `button`); the rest are variants (`enz_hcenter`) and
    // follow it. Otherwise the BTreeSet's alphabetical order stands.
    let primary = format!("enz_{module}");
    if let Some(pos) = ctors.iter().position(|c| *c == primary) {
        ctors.swap(0, pos);
    }
    ctors.extend(types);
    ctors
}

/// A module's own `pub fn enz_*` constructors and `pub struct Enz*` builder
/// types, read from its source files (flat `<name>.rs` plus every `.rs` under
/// a `<name>/` directory, the way `has_enz_signature` does - several
/// constructors live in `<name>/builder.rs`). SOURCE (not crate-root
/// re-export lists) is the right input: pages must name every offered
/// constructor; [`module_api`] then decides per name which path resolves.
pub(crate) fn source_public_items(module: &str) -> Vec<String> {
    fn scan(content: &str, out: &mut BTreeSet<String>) {
        for line in content.lines() {
            let t = line.trim_start();
            if let Some(rest) = t
                .strip_prefix("pub fn enz_")
                .or_else(|| t.strip_prefix("pub const fn enz_"))
            {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    out.insert(format!("enz_{name}"));
                }
            }
            // `pub struct Enz*` and `pub type Enz*`: canonical builder aliases
            // (e.g. `EnzIconLabel`, `EnzInlineViewSwitcher`) that struct-only
            // scans missed, incompleting API cells. `#[deprecated] pub type
            // Enz*`s (e.g. `EnzInputOTP`) caught here but stripped below:
            // readers never told to write migrated-off names.
            if let Some(rest) = t
                .strip_prefix("pub struct Enz")
                .or_else(|| t.strip_prefix("pub type Enz"))
            {
                let name: String = rest
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    out.insert(format!("Enz{name}"));
                }
            }
        }
    }
    fn walk(dir: &std::path::Path, out: &mut BTreeSet<String>) {
        for rel in walk_rs_files(dir) {
            scan(&read_source_or_panic(&dir.join(rel)), out);
        }
    }

    let src = src_dir();
    let mut out = BTreeSet::new();
    let flat = src.join(format!("{module}.rs"));
    // Tolerate only NotFound for these optional module halves.
    if optional_file_exists(&flat) {
        scan(&read_source_or_panic(&flat), &mut out);
    }
    let dir = src.join(module);
    if optional_dir_exists(&dir) {
        walk(&dir, &mut out);
    }

    // `#[deprecated]`-named drops: API cells tell readers what to WRITE;
    // lib.rs already routes off old spellings (`EnzInputOTP` ->
    // `EnzInputOtp`); advertising deprecated aliases undoes that. DERIVED
    // exclusions from lib.rs `<!-- deprecated-inventory: -->` blocks (machine-
    // checked vs real `#[deprecated]`s by
    // `export_policy::lib_rs_deprecated_inventory_matches_source`): future
    // deprecated `pub type Enz*`s exclude themselves on inventory-joining -
    // zero hard-coded names here.
    let deprecated = deprecated_item_names(module);
    out.into_iter()
        .filter(|n| !deprecated.contains(n))
        .collect()
}

/// Item names `module` has `#[deprecated]`, read from the machine-checked
/// `<!-- deprecated-inventory: -->` block in lib.rs (entries are
/// `<module>::<item>`). Feeds [`source_public_items`] deprecated-alias
/// exclusion. Inventory-DERIVED (never `EnzInputOTP`-hard-coded): filters track
/// crates automatically via the same
/// `export_policy::lib_rs_deprecated_inventory_matches_source`-proved blocks.
fn deprecated_item_names(module: &str) -> BTreeSet<String> {
    let lib_rs = read_source_or_panic(&src_dir().join("lib.rs"));
    let prefix = format!("{module}::");
    documented_deprecated_inventory(&lib_rs)
        .into_iter()
        .filter_map(|entry| entry.strip_prefix(&prefix).map(str::to_owned))
        .collect()
}

/// Rewrite rustdoc `mod@` intra-doc links into markdown links a plain renderer
/// can follow. Catalog prose lives in rustdoc comments ([`enz_text`](mod@enz_text)s
/// work there); verbatim-copied into `.md`s they are DEAD links (literal-href
/// renders on GitHub/every viewer). Rewritten to module-file source-relative
/// links (see [`module_source_rel`]): fresh-clone-resolving (unlike old
/// `../target/doc/...` links, dead until `cargo doc` runs). Module headers are
/// the agent-worth-reading things anyway, and the generator's own inputs.
pub(crate) fn rewrite_mod_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("(mod@") {
        let (before, tail) = rest.split_at(at);
        out.push_str(before);
        let after = &tail["(mod@".len()..];
        let name: String = after
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if name.is_empty() {
            // Not a link target after all; copy the marker through unchanged
            // rather than dropping text.
            out.push_str("(mod@");
            rest = after;
            continue;
        }
        let _ = write!(out, "({}", module_source_rel(&name));
        rest = &after[name.len()..];
    }
    out.push_str(rest);
    out
}

/// The link target for a module, **relative to `docs/`** (where the generated
/// `docs/widgets.md` lives), pointing at the module's own source file. Flat
/// `src/<name>.rs` or directory `src/<name>/mod.rs` (six tier-1 directories):
/// filesystems CHECKED so emitted links exist (dead catalog links = dead
/// discovery entries; generators never guess). Cross-crate names
/// (`enzui_core::portal`) never arrive (`[`path`]` shorthands, not
/// `mod@`s): every `name` is a widgets-crate module. `docs/widgets_links_resolve`
/// walks every emitted link, failing unmoved-by-renames - renames redden
/// instead of silently rotting catalogs.
pub(crate) fn module_source_rel(name: &str) -> String {
    let src = src_dir();
    // A metadata failure here would silently emit the flat fallback
    // link for a module that really is a directory, so only NotFound may
    // fall through to the next layout.
    if optional_file_exists(&src.join(format!("{name}.rs"))) {
        format!("../crates/enzui-widgets/src/{name}.rs")
    } else if optional_file_exists(&src.join(name).join("mod.rs")) {
        format!("../crates/enzui-widgets/src/{name}/mod.rs")
    } else {
        // Neither layout exists: emit the flat form so the resolve test below
        // reddens with the exact path, rather than the generator inventing a
        // plausible-but-wrong link.
        format!("../crates/enzui-widgets/src/{name}.rs")
    }
}

/// A GitHub-style anchor for a markdown heading, so the table of contents
/// links resolve in every renderer we care about.
pub(crate) fn anchor(heading: &str) -> String {
    heading
        .chars()
        .filter_map(|c| {
            if c.is_alphanumeric() {
                Some(c.to_ascii_lowercase())
            } else if c == ' ' || c == '-' {
                Some('-')
            } else {
                None
            }
        })
        .collect()
}

/// Everything the generator needs, gathered from source in one place so the
/// positive control can assert floors on it before any markdown exists.
pub(crate) struct CatalogModel {
    pub(crate) groups: Vec<CatalogGroup>,
    pub(crate) tier1: BTreeSet<String>,
    pub(crate) all_pub_mods: BTreeSet<String>,
    /// `module -> importable paths`, constructors first. See [`module_api`].
    pub(crate) api: BTreeMap<String, Vec<String>>,
    pub(crate) examples: BTreeMap<String, Vec<String>>,
    pub(crate) aliases: Vec<KeywordAlias>,
    /// The prose lines between `## Finding a widget by keyword` and the alias
    /// table itself (i.e. the paragraphs that explain the section). Preserved
    /// verbatim by [`render`] so the generated page carries the same reason
    /// the rustdoc block does.
    pub(crate) alias_prose: Vec<String>,
    /// The trailing paragraphs *after* the alias table, up to the next
    /// same-or-shallower heading. Same reason as [`alias_prose`].
    pub(crate) alias_epilogue: Vec<String>,
    /// The container→collaborator composition rows.
    pub(crate) composes_with: Vec<ComposesWith>,
    /// The prose lines between `## Composes with` and its table, preserved
    /// verbatim by [`render`].
    pub(crate) composes_prose: Vec<String>,
    /// The intent-family→sibling-choice rows.
    pub(crate) sibling_choices: Vec<SiblingChoice>,
    /// The prose lines between `## Choose between siblings` and its table,
    /// preserved verbatim by [`render`].
    pub(crate) sibling_choice_prose: Vec<String>,
}

pub(crate) fn read_model() -> CatalogModel {
    let lib_rs = read_source_or_panic(&src_dir().join("lib.rs"));
    let example_rs = read_source_or_panic(&example_rs_path());

    let groups = parse_catalog(&lib_rs);
    let tier1 = live_tier1(&lib_rs);
    let all_pub_mods = declared_pub_mods(&lib_rs);
    let reexports = root_reexports(&lib_rs);
    let aliases = parse_keyword_aliases(&lib_rs);
    let (alias_prose, alias_epilogue) = parse_alias_prose(&lib_rs);
    let composes_with = parse_composes_with(&lib_rs);
    let composes_prose = parse_composes_prose(&lib_rs);
    let sibling_choices = parse_sibling_choices(&lib_rs);
    let sibling_choice_prose = parse_sibling_choice_prose(&lib_rs);

    let mut api = BTreeMap::new();
    for group in &groups {
        for entry in &group.entries {
            api.insert(entry.module.clone(), module_api(&reexports, &entry.module));
        }
    }

    let examples = EXAMPLE_REGIONS
        .iter()
        .map(|n| (n.to_string(), example_region(&example_rs, n)))
        .collect();

    CatalogModel {
        groups,
        tier1,
        all_pub_mods,
        api,
        examples,
        aliases,
        alias_prose,
        alias_epilogue,
        composes_with,
        composes_prose,
        sibling_choices,
        sibling_choice_prose,
    }
}

/// Split the alias section into (prose before the first table row, prose
/// after the last table row). Both are `//!`-stripped and trimmed.
pub(crate) fn parse_alias_prose(lib_rs: &str) -> (Vec<String>, Vec<String>) {
    let Some(start) = lib_rs.find(ALIAS_SECTION_MARKER) else {
        return (Vec::new(), Vec::new());
    };
    let tail = &lib_rs[start..];

    // Same terminator logic as parse_keyword_aliases: stop at the next `##`
    // or `#` heading (but not `###`, which is a group inside `# Widget
    // catalog`).
    let mut end = tail.len();
    for (i, line) in tail.lines().enumerate() {
        if i == 0 {
            continue;
        }
        let t = line.trim_start().trim_start_matches("//!").trim();
        if t.starts_with("## ") || t.starts_with("# ") {
            let mut byte = 0usize;
            for (j, l) in tail.lines().enumerate() {
                if j == i {
                    end = byte;
                    break;
                }
                byte += l.len() + 1;
            }
            break;
        }
    }
    let section = &tail[..end];

    let mut before: Vec<String> = Vec::new();
    let mut after: Vec<String> = Vec::new();
    let mut seen_table = false;
    let mut past_table = false;

    for (i, line) in section.lines().enumerate() {
        if i == 0 {
            continue; // the marker line itself
        }
        let t = line.trim_start().trim_start_matches("//!").trim();
        if t.starts_with('|') {
            seen_table = true;
            continue;
        }
        if seen_table && !t.starts_with('|') && !t.is_empty() {
            past_table = true;
        }
        if past_table {
            after.push(t.to_owned());
        } else if !seen_table {
            before.push(t.to_owned());
        }
    }
    // Also collect blank lines that fall inside the epilogue after the first
    // non-blank has been observed.
    // (Both `before` and `after` may have trailing/leading blanks; trim them.)
    while before.first().is_some_and(std::string::String::is_empty) {
        before.remove(0);
    }
    while before.last().is_some_and(std::string::String::is_empty) {
        before.pop();
    }
    while after.first().is_some_and(std::string::String::is_empty) {
        after.remove(0);
    }
    while after.last().is_some_and(std::string::String::is_empty) {
        after.pop();
    }
    (before, after)
}

/// Render the whole page. Pure over [`CatalogModel`], so the positive control
/// can drive it with a fixture and prove it emits *something* per entry.
pub(crate) fn render(model: &CatalogModel) -> String {
    let total: usize = model.groups.iter().map(|g| g.entries.len()).sum();
    let mut out = String::new();

    out.push_str("# Widget catalog\n\n");
    out.push_str(
        "**Do not edit this file.** It is generated from the `# Widget catalog`\n\
         doc block at the top of `crates/enzui-widgets/src/lib.rs` by\n\
         `crates/enzui-widgets/tests/contracts/widget_catalog_page.rs`, and the\n\
         `docs_widgets_md_is_not_stale` test\n\
         (`crates/enzui-widgets/tests/contracts/widget_catalog_page_staleness.rs`)\n\
         fails the build if this file and that\n\
         block disagree. To change an entry, edit the doc block and regenerate:\n\n",
    );
    out.push_str(
        "```sh\n\
         ENZUI_WRITE_WIDGET_DOC=1 cargo wt --test contracts widget_catalog_page\n\
         ```\n\n",
    );
    let _ = write!(
        out,
        "This page lists all **{total}** tier-1 (supported public API) widget\n\
         modules, grouped by what you are trying to do. Tier 1 is derived, not\n\
         typed: it is every `pub mod` in the crate minus the internal-composition\n\
         and audit/probe lists, and `widget_catalog_partitions_tier1` makes a\n\
         missing, doubled or phantom entry a hard error.\n\n"
    );
    out.push_str(
        "New to the framework? Read [building-an-app.md](building-an-app.md)\n\
         first - it covers the `Message` round-trip, the app shell and\n\
         responsiveness. This page answers the other question: *what already\n\
         exists*.\n\n",
    );
    out.push_str("Focusable widgets register with the framework focus ring. Enter the event loop\nthrough `enzui_widgets::app_setup::run` to get automatic Tab and Shift+Tab\ntraversal. Hosts with their own event loop must install traversal explicitly;\n`run_with_traversal` selects a custom chord or disables the default binding.\nSee [building-an-app.md](building-an-app.md) for the application setup route.\n\n");

    // --- How to read an entry ------------------------------------------------
    out.push_str("## How to read an entry\n\n");
    out.push_str(
        "Each entry names its module, what it is for, and the crate-root imports\n\
         it provides:\n\n\
         - **`enz_*`** - the constructor function(s). Where a module offers\n\
           several, the one named after the module comes first and the rest are\n\
           variants.\n\
         - **`Enz*`** - the builder type(s) the constructor returns. You need the\n\
           name when a struct field or a function signature has to mention it;\n\
           every tier-1 builder type is re-exported at the crate root, so\n\
           `enzui_widgets::EnzButton` resolves.\n\
         - **source** - the module's own file, linked from its name in each\n\
           table below. It resolves on a fresh clone (no `cargo doc` step), and\n\
           the module header is the option list and rationale worth reading. To\n\
           render the API as HTML instead, run\n\
           `cargo doc --workspace --all-features --no-deps` and open\n\
           `target/doc/enzui_widgets/<module>/index.html`.\n\n\
         The shape is the same for every widget:\n\n",
    );
    out.push_str("```rust\n");
    for line in &model.examples["shape"] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("```\n\n");
    out.push_str(
        "Prefer a group's container over hand-rolling one: the containers carry\n\
         the token wiring and the accessibility semantics that a bare\n\
         `iced::widget::container` cannot.\n\n",
    );
    out.push_str("```rust\n");
    for line in &model.examples["compose"] {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("```\n\n");
    out.push_str(
        "Both blocks above are compiled by the `contracts` integration-test target.\n\
         They are marked regions of `crates/enzui-widgets/tests/fixtures/widget_catalog.rs`,\n\
         spliced in verbatim, never retyped.\n\n",
    );

    // --- Naming and imports contract ----------------------------------------
    out.push_str(NAMING_CONTRACT_MD);

    // --- Keyword aliases -----------------------------------------------------
    //
    // Placed here, between the shape example and the per-task groups,
    // because it answers the other search direction: an app author who came
    // with a name in mind rather than a task. Rendered verbatim from the
    // rustdoc block so a single source is checked and displayed.
    out.push_str("## Finding a widget by keyword\n\n");
    for line in &model.alias_prose {
        out.push_str(&rewrite_mod_links(line));
        out.push('\n');
    }
    if !model.alias_prose.is_empty() {
        out.push('\n');
    }
    out.push_str("| you searched for | reach for |\n");
    out.push_str("|---|---|\n");
    for row in &model.aliases {
        let _ = writeln!(
            out,
            "| {} | {} |",
            rewrite_mod_links(&row.raw_first_cell),
            rewrite_mod_links(&row.raw_second_cell)
        );
    }
    out.push('\n');
    for line in &model.alias_epilogue {
        out.push_str(&rewrite_mod_links(line));
        out.push('\n');
    }
    if !model.alias_epilogue.is_empty() {
        out.push('\n');
    }

    // --- Composes with -------------------------------------------------------
    //
    // Composition edges. Rendered verbatim from the rustdoc block so one source is
    // checked and displayed. It answers a second direction: not "where did it
    // ship" but "what else do I need in order to use it correctly".
    out.push_str("## Composes with\n\n");
    for line in &model.composes_prose {
        out.push_str(&rewrite_mod_links(line));
        out.push('\n');
    }
    if !model.composes_prose.is_empty() {
        out.push('\n');
    }
    out.push_str("| widget | composes with | why |\n");
    out.push_str("|---|---|---|\n");
    for row in &model.composes_with {
        let _ = writeln!(
            out,
            "| {} | {} | {} |",
            rewrite_mod_links(&row.raw_module_cell),
            rewrite_mod_links(&row.raw_targets_cell),
            rewrite_mod_links(&row.raw_why_cell),
        );
    }
    out.push('\n');

    // --- Choose between siblings -------------------------------------------
    //
    // The alias table answers "where did X ship" and the composes-with table
    // "what else do I need inside it". This table answers the third question the catalog
    // structurally cannot: "when several shipped widgets answer one intent,
    // which one do I want". Rendered verbatim from the rustdoc block so one
    // source is checked and displayed, placed here so a caller who has found
    // a candidate through the alias table sees the disambiguation before
    // paging into the per-group tables.
    out.push_str("## Choose between siblings\n\n");
    for line in &model.sibling_choice_prose {
        out.push_str(&rewrite_mod_links(line));
        out.push('\n');
    }
    if !model.sibling_choice_prose.is_empty() {
        out.push('\n');
    }
    out.push_str("| family | siblings | the question that decides | pick |\n");
    out.push_str("|---|---|---|---|\n");
    for row in &model.sibling_choices {
        let _ = writeln!(
            out,
            "| {} | {} | {} | {} |",
            rewrite_mod_links(&row.raw_family_cell),
            rewrite_mod_links(&row.raw_siblings_cell),
            rewrite_mod_links(&row.raw_question_cell),
            rewrite_mod_links(&row.raw_pick_cell),
        );
    }
    out.push('\n');

    // --- Table of contents ---------------------------------------------------
    out.push_str("## Groups\n\n");
    for group in &model.groups {
        let _ = writeln!(
            out,
            "- [{}](#{}) - {} widgets",
            group.heading,
            anchor(&group.heading),
            group.entries.len()
        );
    }
    out.push('\n');

    // --- The groups ----------------------------------------------------------
    for group in &model.groups {
        let _ = write!(out, "## {}\n\n", group.heading);
        if !group.prose.is_empty() {
            for line in &group.prose {
                out.push_str(&rewrite_mod_links(line));
                out.push('\n');
            }
            out.push('\n');
        }
        out.push_str("| widget | what it is for | crate-root API |\n");
        out.push_str("|---|---|---|\n");
        for entry in &group.entries {
            let api = model
                .api
                .get(&entry.module)
                .unwrap_or_else(|| panic!("no API recorded for {}", entry.module))
                .iter()
                .map(|p| format!("`{p}`"))
                .collect::<Vec<_>>();
            let summary = rewrite_mod_links(&entry.summary).replace('|', "\\|");
            // A blank API cell reads as "we failed to find one". For the
            // NON_WIDGET_TIER1 modules there is genuinely nothing to
            // construct, so say that rather than leaving a hole a reader has
            // to interpret.
            let api_cell = if api.is_empty() && NON_WIDGET_TIER1.contains(&entry.module.as_str()) {
                "*not a widget - see module docs*".to_owned()
            } else {
                api.join(", ")
            };
            let _ = writeln!(
                out,
                "| [`{}`]({}) | {} | {} |",
                entry.module,
                module_source_rel(&entry.module),
                summary,
                api_cell,
            );
        }
        out.push('\n');
    }

    // --- Footer --------------------------------------------------------------
    out.push_str("## What is not on this page\n\n");
    let _ = write!(
        out,
        "The crate declares {} `pub mod`s; {total} of them are tier 1 and listed\n\
         above. The remainder are **internal-composition** modules (public only so\n\
         widgets in this crate and the probe can compose them) and **audit/probe**\n\
         modules (they power the test and A/B harness). Both lists, and the reason\n\
         each exists, are in the § Export / alias / deprecation boundary section of\n\
         the crate rustdoc. Their surface is unstable - do not depend on it.\n",
        model.all_pub_mods.len()
    );

    // The catalog prose is rustdoc; turn its item links into source links so
    // the page works on a forge as well as in `cargo doc`.
    let rewritten = enzui_test_support::doc_links::rewrite_rustdoc_links(
        &out,
        &workspace_root(),
        "enzui-widgets",
        "docs",
    );
    assert!(
        rewritten.unresolved.is_empty(),
        "the widget catalog links to items that no source file defines: {:?}",
        rewritten.unresolved
    );
    rewritten.markdown
}

// ---------------------------------------------------------------------------
// docs/widgets.json - the machine-readable sibling index
// ---------------------------------------------------------------------------

/// Escape a string for inclusion in a JSON double-quoted literal. Hand-rolled
/// because the crate has no serde in its dev-deps and this gate must not move
/// `Cargo.lock` (a new dep would force a whole-graph rebuild). Only the escapes
/// JSON requires are emitted; every other char passes through, which is why the
/// round-trip test parses the output back with a real JSON reader rather than
/// trusting this function.
pub(crate) fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// Strip rustdoc/markdown link syntax out of a summary so the JSON `summary`
/// field is plain prose. `[`x`](mod@x)` and `[`x`](target)` both collapse to
/// link-text `x`; stray backticks removed. JSON indexes filter
/// programmatically, never render: link chrome is noise there. `char`-wise
/// iteration (never byte-indexed): multi-byte UTF-8 (en/em-dashes, accented)
/// survives intact instead of Latin-1-mojibake-splitting.
pub(crate) fn plain_summary(text: &str) -> String {
    // First, collapse `[label](href)` to `label`.
    // Walk by char indices so multi-byte UTF-8 is never split.
    let mut out = String::with_capacity(text.len());
    let mut i = 0usize;
    while i < text.len() {
        if text[i..].starts_with('[')
            && let Some(close) = text[i + 1..].find(']')
        {
            let label = &text[i + 1..i + 1 + close];
            let after = i + 1 + close + 1;
            if after < text.len()
                && text[after..].starts_with('(')
                && let Some(paren) = text[after..].find(')')
            {
                out.push_str(label);
                i = after + paren + 1;
                continue;
            }
            // `[label]` with no `(href)`: keep the label, drop the brackets.
            out.push_str(label);
            i = i + 1 + close + 1;
            continue;
        }
        // Advance by one CHAR, preserving multi-byte codepoints.
        let ch = text[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    // Then drop backticks and any leftover `mod@` markers, and collapse runs of
    // whitespace so the field is one clean line.
    let dechromed = out.replace('`', "").replace("mod@", "");
    dechromed.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The keyword-alias terms that reach each module, gathered from the parsed
/// alias table. Populates JSON `keywords` arrays for one-cheap-call
/// `jq '.[] | select(.keywords[] | test("segment"))'`s (prose-only-catalog
/// full-read costs avoided). Function-level citations (`[\`m::x\`]` second
/// cells) also contribute to OWNING module `m`s: intra-doc-shorthand function
/// rows would otherwise floor-drop keywords for `docs/widgets.json` consumers
/// while markdown rows still name functions.
pub(crate) fn keywords_by_module(aliases: &[KeywordAlias]) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in aliases {
        for target in &row.targets {
            let owner = match target {
                AliasTarget::Module(m) => Some(m.as_str()),
                AliasTarget::InternalItem(path) => path.split("::").next(),
                AliasTarget::External(_) => None,
            };
            if let Some(m) = owner {
                let e = out.entry(m.to_owned()).or_default();
                for k in &row.keywords {
                    e.insert(k.clone());
                }
            }
        }
    }
    out
}

/// The same-crate item citations that must appear as their own JSON
/// records. Entries are `[\`m::x\]` shorthands whose owning `m`s sit off
/// tier-1 widget surfaces (no widget JSON row absorbs keywords), yet `x`s are
/// shipped public capabilities app authors search. Consumers reach intents via
/// real-`"kind"` (`fn`/`struct`/…), full `"path"`, owning `"module"`, leaf
/// `"item"` rows.
///
/// # Which rows contribute (generic, no per-alias exception)
///
/// Alias rows whose every target lies outside the tier-1 widget surfaces.
/// A row pointing at a widget is a widget row, resolved through the widget
/// JSON, and must not spawn a second capability row.
pub(crate) fn item_capability_rows(
    aliases: &[KeywordAlias],
    tier1: &BTreeSet<String>,
) -> Vec<ItemCapabilityRow> {
    let mut per_path: BTreeMap<String, ItemCapabilityRow> = BTreeMap::new();
    for row in aliases {
        // Generic predicate: skip any row that touches the tier-1 widget
        // surface. Such a row is a widget row whose keywords resolve through
        // the widget's own JSON object; promoting an item it merely mentions
        // (often contrastively) to a capability row is a wrong-hit.
        let touches_tier1 = row.targets.iter().any(|t| match t {
            AliasTarget::Module(m) => tier1.contains(m),
            AliasTarget::InternalItem(p) => p
                .split("::")
                .next()
                .is_some_and(|root| tier1.contains(root)),
            AliasTarget::External(_) => false,
        });
        if touches_tier1 {
            continue;
        }
        for target in &row.targets {
            let AliasTarget::InternalItem(path) = target else {
                continue;
            };
            let Some(root) = path.split("::").next() else {
                continue;
            };
            let Some(tail) = path.rsplit("::").next() else {
                continue;
            };
            // Item-kind truth: read the real declaration kind from the module
            // source. A citation the discovery gate already proved resolvable
            // must have a kind; a `None` here is a generator invariant break,
            // not a data condition, so it panics rather than mislabels.
            let kind = module_pub_item_kind(root, tail).unwrap_or_else(|| {
                panic!(
                    "item-capability citation `{path}` names no `pub` item in \
                     module `{root}`; keyword_aliases_resolve_to_real_modules \
                     should have reddened first. Refusing to emit a JSON row \
                     with an unknown item kind."
                )
            });
            let entry = per_path
                .entry(path.clone())
                .or_insert_with(|| ItemCapabilityRow {
                    module: root.to_owned(),
                    path: path.clone(),
                    api: tail.to_owned(),
                    kind,
                    keywords: BTreeSet::new(),
                });
            for k in &row.keywords {
                entry.keywords.insert(k.clone());
            }
        }
    }
    per_path.into_values().collect()
}

/// One same-crate item capability record; owning module, full path,
/// leaf item name, its real declaration `kind` and the keywords that reach it.
#[derive(Debug, Clone)]
pub(crate) struct ItemCapabilityRow {
    pub(crate) module: String,
    pub(crate) path: String,
    pub(crate) api: String,
    /// The real declaration kind (`fn`/`struct`/`enum`/…), read from source by
    /// [`module_pub_item_kind`]. This is the single source of item-kind truth
    /// the JSON row prints, so a non-function subject cannot ship as a
    /// "function".
    pub(crate) kind: &'static str,
    pub(crate) keywords: BTreeSet<String>,
}

/// The real declaration kind of a same-crate `pub` item named `tail` in
/// `module`, as stable lowercase keywords (`fn`, `struct`, `enum`, `trait`,
/// `type`, `const`, `static`, `union`, `mod`, `reexport` for `pub use`s), or
/// `None` on no-matching-`pub`-declaration. Flat `<module>.rs` + every
/// `<module>/`-directory file reads (sibling-scattered APIs). Call-syntax-
/// shaped scans (`"pub <kind> <ident>"` trimmed lines): comment/string
/// identifier mentions never spoof real items. Single item scanner:
/// [`module_has_pub_item`] delegates; [`item_capability_rows`] JSON-labels
/// whatever returns.
pub(crate) fn module_pub_item_kind(module: &str, tail: &str) -> Option<&'static str> {
    pub_item_kind_under(&src_dir(), module, tail)
}

/// [`module_pub_item_kind`] against an arbitrary crate's `src` directory.
///
/// Same scanner, so a cross-crate citation in the keyword table is resolved by
/// exactly the rules that resolve a same-crate one; only the root moves.
pub(crate) fn pub_item_kind_under(src: &Path, module: &str, tail: &str) -> Option<&'static str> {
    fn scan(source: &str, tail: &str) -> Option<&'static str> {
        // (`pub <kind> ` prefix, canonical JSON kind). fn variants (including
        // `pub const fn`) come before `pub const ` so a `const fn` is labelled
        // `fn`, not `const`.
        const KINDS: &[(&str, &str)] = &[
            ("pub fn ", "fn"),
            ("pub async fn ", "fn"),
            ("pub unsafe fn ", "fn"),
            ("pub extern fn ", "fn"),
            ("pub const fn ", "fn"),
            ("pub const ", "const"),
            ("pub static ", "static"),
            ("pub struct ", "struct"),
            ("pub enum ", "enum"),
            ("pub trait ", "trait"),
            ("pub type ", "type"),
            ("pub union ", "union"),
            ("pub mod ", "mod"),
        ];
        for line in source.lines() {
            let trimmed = line.trim_start();
            for (needle, kind) in KINDS {
                if let Some(rest) = trimmed.strip_prefix(needle) {
                    let name: String = rest
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect();
                    if name == tail {
                        return Some(kind);
                    }
                }
            }
            // `pub use <path>::<tail>` / `pub use <path>::{a, tail, b}` is how
            // an item declared in a submodule reaches the module's public API.
            if let Some(rest) = trimmed.strip_prefix("pub use ") {
                let cleaned: String = rest
                    .chars()
                    .map(|c| {
                        if c == '{' || c == '}' || c == ',' || c == ';' {
                            ' '
                        } else {
                            c
                        }
                    })
                    .collect();
                for token in cleaned.split_whitespace() {
                    let leaf = token.rsplit("::").next().unwrap_or(token);
                    let leaf = leaf.trim_end_matches('*');
                    if leaf == tail {
                        return Some("reexport");
                    }
                }
            }
        }
        None
    }
    let flat = src.join(format!("{module}.rs"));
    let dir = src.join(module);

    // Read the complete declared subject set before looking for a match. A
    // declaration in the flat file or an early child must not hide an
    // unreadable later child.
    let mut sources = Vec::new();
    if optional_file_exists(&flat) {
        sources.push(read_source_or_panic(&flat));
    }
    if optional_dir_exists(&dir) {
        // Scan immediate children only: nested test/support
        // modules are not part of this public-item lookup.
        sources.extend(
            read_dir_sorted(&dir)
                .into_iter()
                .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
                .map(|path| read_source_or_panic(&path)),
        );
    }
    sources.iter().find_map(|source| scan(source, tail))
}

/// Render `docs/widgets.json`: one object per tier-1 catalog entry, keyed by
/// module, carrying exactly these fields - `module`, `tier`,
/// `group`, `summary`, `api`, `keywords`, and the optional `prefer_over`
/// and `hazards`. `hazards` stays ABSENT until conventions wire (empty arrays
/// would claim "audited, none" over "not yet audited" truths); `prefer_over`
/// likewise sourceless-until-checked (JSON carries zero unverified claims).
/// `# Hazards`-wirers fill fields here.
///
/// Same-[`CatalogModel`] emissions as
/// `render`: `docs_widgets_json_is_not_stale` fails disagreeing builds;
/// round-trip tests prove JSON-parseable bytes.
pub(crate) fn render_json(model: &CatalogModel) -> String {
    let kw = keywords_by_module(&model.aliases);
    // The same curated caller-wiring list this crate's contract suite
    // gates, exposed so the JSON's `wiring_contract` bool has one source.
    let wiring = crate::widget_catalog_page_contracts::wiring_contract_modules();
    // `module -> the modules it composes with`, in table order
    // (not sorted - the row lists layers outermost-first and that order is
    // information a consumer should keep).
    let composes: BTreeMap<&str, &[String]> = model
        .composes_with
        .iter()
        .map(|r| (r.module.as_str(), r.targets.as_slice()))
        .collect();
    let mut out = String::new();
    out.push_str("[\n");

    // Flatten groups in page order so the JSON row order matches the markdown.
    let entries: Vec<(&str, &CatalogEntry)> = model
        .groups
        .iter()
        .flat_map(|g| g.entries.iter().map(move |e| (g.heading.as_str(), e)))
        .collect();

    for (idx, (group, entry)) in entries.iter().enumerate() {
        let module = &entry.module;
        let api = model
            .api
            .get(module)
            .map_or(&[][..], std::vec::Vec::as_slice);
        let summary = plain_summary(&entry.summary);
        let keywords: Vec<&String> = kw
            .get(module)
            .map(|s| s.iter().collect())
            .unwrap_or_default();

        out.push_str("  {\n");
        let _ = writeln!(out, "    \"module\": \"{}\",", json_escape(module));
        out.push_str("    \"tier\": 1,\n");
        let _ = writeln!(out, "    \"group\": \"{}\",", json_escape(group));
        let _ = writeln!(out, "    \"summary\": \"{}\",", json_escape(&summary));

        out.push_str("    \"api\": [");
        for (i, a) in api.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            let _ = write!(out, "\"{}\"", json_escape(a));
        }
        out.push_str("],\n");

        // `keywords` is the last REQUIRED field, so it has no trailing comma.
        out.push_str("    \"keywords\": [");
        for (i, k) in keywords.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            let _ = write!(out, "\"{}\"", json_escape(k));
        }
        out.push(']');
        // `composes_with` is present only for modules that have a row
        // in the `## Composes with` table. Same absent-vs-empty discipline as
        // `hazards` below: a missing key means "no checked composition edge
        // recorded", never "audited, composes with nothing".
        if let Some(targets) = composes.get(module.as_str()) {
            out.push_str(",\n    \"composes_with\": [");
            for (i, t) in targets.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                let _ = write!(out, "\"{}\"", json_escape(t));
            }
            out.push(']');
        }

        // A TOTAL bool (every row carries it). `true` for the modules on
        // the curated caller-wiring list, `false` for the rest - and `false`
        // is a real verdict here (`docs/caller-wiring-audit.md`: most modules
        // correctly name no wiring contract), so unlike `hazards`/`prefer_over`
        // this field is never omitted.
        let _ = write!(
            out,
            ",\n    \"wiring_contract\": {}",
            wiring.contains(module.as_str())
        );
        out.push('\n');
        // `prefer_over` and `hazards` are intentionally OMITTED, not empty:
        // absent means "no checked source yet" (the hazards field is filled separately),
        // whereas `[]` would assert "audited, none". A consumer treats a
        // missing key and an empty array identically for filtering, so this
        // costs nothing and avoids a false claim.

        out.push_str("  }");
        if idx + 1 < entries.len() {
            out.push(',');
        }
        out.push('\n');
    }

    // Same-crate item capability records. Emitted after the tier-1
    // widget entries so an ordering-sensitive consumer that stops at the
    // widget rows still sees the same array shape as before, while a
    // consumer that scans for a `"kind"` field reaches the intent through
    // the same array. These rows carry the same required base fields as the
    // widget rows (`module`, `tier`, `group`, `summary`, `api`, `keywords`,
    // `wiring_contract`) so a plain-schema consumer is not surprised, plus
    // the item's real `"kind"` (`fn`/`struct`/…, read from source), the
    // leaf `"item"` name and the full `"path"` a rustdoc consumer resolves.
    let item_rows = item_capability_rows(&model.aliases, &model.tier1);
    // The last widget row was emitted without a trailing comma. When we are
    // about to attach item-capability rows, patch that closing `}\n` to
    // `},\n` so the JSON array remains strict. Subsequent item rows carry
    // their own trailing comma from the per-iteration tail below.
    if !item_rows.is_empty() && !entries.is_empty() {
        assert!(
            out.ends_with("  }\n"),
            "render_json widget-row tail must end with `  }}\\n` before \
             item-capability rows attach; got: {:?}",
            &out[out.len().saturating_sub(16)..]
        );
        out.truncate(out.len() - 1);
        out.push_str(",\n");
    }
    for (fi, row) in item_rows.iter().enumerate() {
        out.push_str("  {\n");
        let _ = writeln!(out, "    \"module\": \"{}\",", json_escape(&row.module));
        out.push_str("    \"tier\": 1,\n");
        let _ = writeln!(
            out,
            "    \"group\": \"{}\",",
            json_escape("Item-level capabilities")
        );
        let _ = writeln!(
            out,
            "    \"summary\": \"{}\",",
            json_escape(&format!(
                "item-level capability `{}` (a `{}`) in module `{}`; the \
                 keyword row lands here for programmatic resolution.",
                row.path, row.kind, row.module
            ))
        );
        // An item-level capability is not a widget constructor, so
        // `api` stays empty here (the naming-grammar gate enforces the
        // `enz_*`/`Enz*` prefix on catalog constructors and would otherwise
        // read a raw item name as a broken canonical constructor). The item
        // identifier is carried in `item` and `path` alongside `kind`.
        out.push_str("    \"api\": [],\n");
        let _ = writeln!(out, "    \"path\": \"{}\",", json_escape(&row.path));
        let _ = writeln!(out, "    \"item\": \"{}\",", json_escape(&row.api));
        // Item-kind truth: the real declaration kind, read from source. A
        // future non-function subject prints its own kind (`struct`, `enum`,
        // …) here rather than being mislabelled `function`.
        let _ = writeln!(out, "    \"kind\": \"{}\",", json_escape(row.kind));
        out.push_str("    \"keywords\": [");
        for (i, k) in row.keywords.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            let _ = write!(out, "\"{}\"", json_escape(k));
        }
        out.push(']');
        out.push_str(",\n    \"wiring_contract\": false\n");
        out.push_str("  }");
        if fi + 1 < item_rows.len() {
            out.push(',');
        }
        out.push('\n');
    }

    out.push_str("]\n");
    out
}
