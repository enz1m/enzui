use super::widget_catalog_page::*;
use std::collections::{BTreeMap, BTreeSet};

use crate::tiers::{declared_pub_mods, live_tier1, src_dir};

// ---------------------------------------------------------------------------
// Keyword-alias validation
// ---------------------------------------------------------------------------

/// True when the module's source carries a real `pub` declaration for
/// `tail_ident`, in the flat file or any sibling file.
///
/// Complement to the module-existence check: a citation whose module exists
/// but whose tail does not is the same confident-pointer-to-nothing class.
/// Shares the JSON generator's scanner, so gate and emitted `"kind"` cannot
/// disagree.
pub(crate) fn module_has_pub_item(module: &str, tail_ident: &str) -> bool {
    module_pub_item_kind(module, tail_ident).is_some()
}

/// Every `mod@X` target must name a real `pub mod`, and every external
/// target must be allowlisted: a renamed-module alias sends readers
/// confidently to nothing.
#[test]
fn keyword_aliases_resolve_to_real_modules() {
    let lib_rs = std::fs::read_to_string(src_dir().join("lib.rs")).expect("read lib.rs");
    let aliases = parse_keyword_aliases(&lib_rs);
    assert!(
        !aliases.is_empty(),
        "parsed zero keyword-alias rows — either the section vanished or the \
         parser's table-row detector is broken. A silently empty parse would \
         let the generator emit a blank section that looks fine."
    );

    // Every row needs keywords and a target; either absence is a stray cell
    // or a pointer to nothing.
    for row in &aliases {
        assert!(
            !row.keywords.is_empty(),
            "alias row has no backticked keywords in its first cell: \
             {row:?}"
        );
        assert!(
            !row.targets.is_empty(),
            "alias row has no `mod@X` or `[`external::path`]` target in its \
             second cell: {row:?}"
        );
    }

    let pub_mods = declared_pub_mods(&lib_rs);

    /// The only external paths an alias may point at. Adding one tells app
    /// authors to look in another crate, so the pointer must resolve — hence
    /// a targeted assertion per entry below.
    const EXTERNAL_ALLOWLIST: &[&str] = &[
        "enzui_core::portal",
        // The window-size answer lives in core but is re-surfaced by this
        // crate, so the pointer names a genuine re-export.
        "enzui_core::window_placement",
        // Core capability, not a widget; the targeted check below keeps the
        // cross-reference honest.
        "enzui_core::shortcut",
        // Core capability; without this row the table denies it exists.
        "enzui_core::file_watch",
        "enzui_core::file_tree_watch",
        // Both rows advise bounding work via a fixed-cap pool; the owner type
        // is named because an author needs the handle, not just the module.
        "enzui_core::blocking_task",
        "enzui_core::blocking_task::BlockingTaskOwner",
    ];

    for row in &aliases {
        for target in &row.targets {
            match target {
                AliasTarget::Module(name) => {
                    assert!(
                        pub_mods.contains(name),
                        "alias {:?} points at `mod@{name}`, but no `pub mod \
                         {name}` exists in enzui-widgets. An alias to a \
                         renamed or deleted module is worse than none — grep \
                         for {name} and rename the target, or drop the alias.",
                        row.keywords,
                    );
                }
                AliasTarget::InternalItem(path) => {
                    let root = path.split("::").next().unwrap_or_default();
                    assert!(
                        pub_mods.contains(root),
                        "alias {:?} contains same-crate item path `{path}`, but \
                         its root `{root}` is not a declared `pub mod` in \
                         enzui-widgets. Internal item classification must \
                         be derived from the crate surface, never naming style.",
                        row.keywords,
                    );
                    // The tail must resolve to a real `pub` item, not just its
                    // owning module.
                    let Some(tail) = path.rsplit("::").next() else {
                        panic!(
                            "alias {:?} internal item path `{path}` has no \
                             `::` tail identifier; classifier accepted a \
                             malformed citation.",
                            row.keywords,
                        );
                    };
                    assert!(
                        module_has_pub_item(root, tail),
                        "alias {:?} cites `{path}`, but no `pub fn/struct/\
                         enum/type/const/static/trait/use \
                         {tail}` is declared in the `{root}` module source. \
                         A function-level citation must resolve to a real \
                         public item; rename the citation to match the API, \
                         or update the API to match the citation.",
                        row.keywords,
                    );
                }
                AliasTarget::External(path) => {
                    assert!(
                        EXTERNAL_ALLOWLIST.contains(&path.as_str()),
                        "alias {:?} points at external path `{path}`, which is \
                         not on the cross-reference allowlist ({EXTERNAL_ALLOWLIST:?}). \
                         Cross-crate aliases are considered surface — add the \
                         path here only after confirming it resolves.",
                        row.keywords,
                    );
                }
            }
        }
    }

    // Each allowlisted path is checked against its crate's own source, root
    // and item: entries cannot outlive their rows, and unreviewed paths fail
    // closed. Calibration first, so an always-`None` resolver cannot pass.
    let core_lib_rs = workspace_root().join("crates/enzui-core/src/lib.rs");
    let core_lib_rs = std::fs::read_to_string(&core_lib_rs)
        .unwrap_or_else(|e| panic!("read {}: {e}", core_lib_rs.display()));
    assert!(
        core_lib_rs.contains("pub mod portal"),
        "the alias table cross-references `enzui_core::portal`, but that \
         module is no longer declared in enzui-core/src/lib.rs. Either \
         update the target or remove the row — pointing app authors at a \
         non-existent module is exactly the failure this test guards against."
    );
    // Same check for the window_placement path.
    assert!(
        core_lib_rs.contains("pub mod window_placement"),
        "the alias table cross-references `enzui_core::window_placement` \
         (the answer to 'remember window size'), but that module is no \
         longer declared in enzui-core/src/lib.rs."
    );
    // Fails in both directions: a renamed module must fail loudly, and so
    // must a table that denies a module that exists.
    assert!(
        core_lib_rs.contains("pub mod file_watch"),
        "the alias table cross-references `enzui_core::file_watch` (the \
         bounded open-document change monitor), but that module is no \
         longer declared in enzui-core/src/lib.rs."
    );
    // A renamed dispatcher module must fail loudly here.
    assert!(
        core_lib_rs.contains("pub mod shortcut"),
        "the alias table cross-references `enzui_core::shortcut` (the \
         shortcut dispatcher apps otherwise hand-roll), but that \
         module is no longer declared in enzui-core/src/lib.rs."
    );
    // Every referenced external path resolves against its own crate; entries
    // cannot outlive their rows. Calibration first, so an always-`None`
    // resolver cannot pass.
    let core_src = workspace_root().join("crates/enzui-core/src");
    assert_eq!(
        pub_item_kind_under(&core_src, "shortcut", "Shortcut"),
        Some("struct"),
        "the cross-crate item resolver cannot find \
         `enzui_core::shortcut::Shortcut`, the shipped `pub struct` the \
         shortcut row names, so every item assertion below is vacuous."
    );
    assert!(
        pub_item_kind_under(&core_src, "shortcut", "definitely_not_a_shortcut_item").is_none(),
        "the cross-crate item resolver accepted a fabricated identifier; a \
         renamed cross-crate citation would not redden this gate."
    );

    for path in aliases
        .iter()
        .flat_map(|row| &row.targets)
        .filter_map(|target| match target {
            AliasTarget::External(path) => Some(path),
            _ => None,
        })
    {
        let mut segments = path.split("::");
        let Some(crate_segment) = segments.next() else {
            panic!("external alias path `{path}` has no crate segment");
        };
        let Some(module) = segments.next() else {
            panic!(
                "external alias path `{path}` names a crate with no module. \
                 A pointer an app author cannot open is the failure this \
                 gate exists for."
            );
        };
        let crate_src = workspace_root()
            .join("crates")
            .join(crate_segment.replace('_', "-"))
            .join("src");
        let lib_rs = crate_src.join("lib.rs");
        let lib_source = std::fs::read_to_string(&lib_rs).unwrap_or_else(|e| {
            panic!(
                "the alias table cross-references `{path}`, but {} is \
                 unreadable: {e}. The crate segment must name a real \
                 workspace crate.",
                lib_rs.display()
            )
        });
        assert!(
            lib_source.contains(&format!("pub mod {module}")),
            "the alias table cross-references `{path}`, but `pub mod \
             {module}` is not declared in {}. Update the target or remove \
             the row — a confident pointer to nothing is worse than none.",
            lib_rs.display()
        );

        // Only in-module paths have a tail; `crate::module` is covered by
        // the module check.
        let inside_module: Vec<&str> = segments.collect();
        if let Some(tail) = inside_module.last() {
            assert!(
                pub_item_kind_under(&crate_src, module, tail).is_some(),
                "the alias table cites `{path}`, but no `pub` declaration of \
                 `{tail}` exists in the `{module}` module of \
                 `{crate_segment}`. A cross-crate item citation must resolve \
                 to a real public item."
            );
        }
    }

    // Still re-exported here, so the author needs no new dependency.
    let wl = std::fs::read_to_string(src_dir().join("window_lifecycle.rs"))
        .expect("read window_lifecycle.rs");
    assert!(
        wl.contains("enzui_core::window_placement"),
        "the keyword table points app authors at \
         `enzui_core::window_placement`, so `window_lifecycle` must keep \
         re-exporting it — otherwise the pointer asks the app to take a \
         dependency the framework is supposed to absorb."
    );
}

// ---------------------------------------------------------------------------
// The keyword table's COVERAGE FLOOR
// ---------------------------------------------------------------------------

/// Lower-cases and collapses author-typed separators (spaces, hyphens) to
/// `_`, so `split view` meets `split_view` and the self-name check below is
/// mechanical.
fn normalise_keyword(k: &str) -> String {
    k.trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c == ' ' || c == '-' { '_' } else { c })
        .collect()
}

/// Coverage floor: every tier-1 module reachable by a non-self keyword. The
/// table is an index, not an incident log — a self-name alias teaches nothing
/// (the searcher already knew the name), and separator collapsing keeps
/// spaces from dodging the check. Failures name the modules.
#[test]
fn every_tier1_module_has_a_non_self_keyword() {
    let lib_rs = std::fs::read_to_string(src_dir().join("lib.rs")).expect("read lib.rs");
    let tier1 = live_tier1(&lib_rs);
    assert!(
        tier1.len() > 100,
        "derived only {} tier-1 modules — the tier parser is broken, so this \
         coverage floor would be vacuously satisfied",
        tier1.len()
    );

    let aliases = parse_keyword_aliases(&lib_rs);
    assert!(
        !aliases.is_empty(),
        "parsed zero keyword-alias rows — a silently empty parse would make \
         this floor pass with the table gone"
    );

    // module -> NON-SELF keywords only.
    let mut non_self: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for row in &aliases {
        for target in &row.targets {
            let AliasTarget::Module(module) = target else {
                continue; // external cross-refs (portal) do not count toward tier-1 coverage
            };
            for kw in &row.keywords {
                if normalise_keyword(kw) != *module {
                    non_self
                        .entry(module.clone())
                        .or_default()
                        .insert(kw.clone());
                }
            }
        }
    }

    let uncovered: Vec<&str> = tier1
        .iter()
        .filter(|m| {
            non_self
                .get(m.as_str())
                .is_none_or(std::collections::BTreeSet::is_empty)
        })
        .map(String::as_str)
        .collect();

    assert!(
        uncovered.is_empty(),
        "these {} tier-1 module(s) are NOT reachable from the keyword-alias \
         table by any keyword other than their own module name, so an author \
         who does not already know the name cannot find them — which is the \
         failure mode that makes an app hand-roll a widget that already \
         ships:\n\n  {:?}\n\n\
         Add a row under `## Finding a widget by keyword` in \
         crates/enzui-widgets/src/lib.rs mapping the intent term(s) an \
         app author would type (e.g. `segmented control`, `spinbox`, `empty \
         state`) to `[`<module>`](mod@<module>)`. Read the module header \
         first — a wrong alias is worse than a missing one.",
        uncovered.len(),
        uncovered,
    );
}

// ---------------------------------------------------------------------------
// Same-crate item links vs cross-crate alias targets
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Wrong-first-hit ordering and negative-control gates
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// `docs/widgets.md` must cover every `pub mod`, or the module
// must be on the machine-checked exclusion inventory
// ---------------------------------------------------------------------------
//
// The concrete miss: shipped modules absent from the page. Rule: every
// `pub mod` is a catalog entry, a `mod@` target, or an inventory row
// (`module_name — reason`); anything else fails the build.

/// Marker constant, so a block rename surfaces as a compile fix rather than
/// a silent parser miss.
const EXCLUSION_INVENTORY_MARKER: &str = "<!-- widgets-md-exclusion-inventory:";

/// One row of the exclusion inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExclusionEntry {
    pub(crate) module: String,
    pub(crate) reason: String,
}

/// Silent-empty guard: a missing marker or a zero-entry block is a panic —
/// otherwise unchecked modules slip through by not being checked at all.
pub(crate) fn parse_exclusion_inventory(lib_rs: &str) -> Vec<ExclusionEntry> {
    let start = lib_rs.find(EXCLUSION_INVENTORY_MARKER).unwrap_or_else(|| {
        panic!(
            "lib.rs is missing the {EXCLUSION_INVENTORY_MARKER:?} HTML \
             comment block. That block is the machine-checked list of \
             `pub mod`s that are deliberately absent from docs/widgets.md; \
             without it every module would either have to reach the page or \
             fail this check, and there is no third state."
        )
    });
    // Skip the marker line itself.
    let after = &lib_rs[start + EXCLUSION_INVENTORY_MARKER.len()..];
    let close = after.find("-->").unwrap_or_else(|| {
        panic!(
            "lib.rs exclusion-inventory block starts with \
             {EXCLUSION_INVENTORY_MARKER:?} but is never closed with `-->`. \
             An unterminated inventory would swallow the rest of the file."
        )
    });
    let block = &after[..close];

    // Spacing distinguishes the separator from hyphens within names or reasons.
    const SEPARATOR: &str = " - ";
    let mut out = Vec::new();
    for line in block.lines() {
        let mut t = line.trim().to_owned();
        if let Some(rest) = t.strip_prefix("//!") {
            t = rest.trim().to_owned();
        }
        if t.is_empty() {
            continue;
        }
        let Some((name, reason)) = t.split_once(SEPARATOR) else {
            panic!(
                "exclusion-inventory line does not use the required \
                 spaced-hyphen delimiter ({SEPARATOR:?}): {t:?}. Each entry must be \
                 `<module_name>{SEPARATOR}<reason>` so the parser can \
                 unambiguously separate the two."
            )
        };
        let name = name.trim();
        let reason = reason.trim();
        assert!(
            !name.is_empty(),
            "exclusion-inventory entry has an empty module name: {t:?}"
        );
        assert!(
            reason.len() >= 20,
            "exclusion-inventory entry for `{name}` has a suspiciously short \
             reason ({reason:?}). The whole point of a per-module inventory \
             is that a reviewer can see why *this specific* module is off \
             the catalog; \"internal\" is not a reason, it is the tier."
        );
        assert!(
            name.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "exclusion-inventory entry has a non-module-name in the first \
             cell: {name:?}"
        );
        out.push(ExclusionEntry {
            module: name.to_owned(),
            reason: reason.to_owned(),
        });
    }

    assert!(
        !out.is_empty(),
        "exclusion-inventory block parsed to zero entries. That is the \
         silent-empty failure mode this parser exists to reject — either \
         the block emptied out (nobody excludes anything, so every `pub mod` \
         should be on the page; verify with the completeness test) or the \
         spaced-hyphen delimiter changed and none of the lines matched."
    );

    out
}

/// What "on the page" means: a catalog entry, an alias target, or a `mod@`
/// link — always a resolving source path, never a built-doc URL (dead on a
/// fresh clone).
fn modules_referenced_in_widgets_md(md: &str) -> BTreeSet<String> {
    let needle = "crates/enzui-widgets/src/";
    let mut out = BTreeSet::new();
    let mut from = 0usize;
    while let Some(rel) = md[from..].find(needle) {
        let at = from + rel + needle.len();
        from = at;
        let tail = &md[at..];
        // Flat `<name>.rs` or `<name>/mod.rs` only, so substrings cannot
        // register phantom modules.
        let name: String = tail
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_')
            .collect();
        if name.is_empty() {
            continue;
        }
        let after = &tail[name.len()..];
        // Only a link whose text is the module's own name is a catalog
        // entry; item links (`[`Y_GUTTER`](...chart_geometry.rs)`) reach the
        // same file without listing the module.
        let text_is_module = md[..at - needle.len()]
            .strip_suffix("../")
            .and_then(|head| head.strip_suffix("]("))
            .is_some_and(|head| head.ends_with(&format!("[`{name}`")));
        if text_is_module && (after.starts_with(".rs") || after.starts_with("/mod.rs")) {
            out.insert(name);
        }
    }
    out
}

/// Completeness gate: every `pub mod` reaches the page or carries an
/// inventory reason. A missing module fails hard — a shipped widget nobody
/// can find is silent and permanent.
#[test]
fn widgets_md_covers_every_pub_mod() {
    let lib_rs = std::fs::read_to_string(src_dir().join("lib.rs")).expect("read lib.rs");
    let md = std::fs::read_to_string(widgets_md_path())
        .unwrap_or_else(|e| panic!("read docs/widgets.md: {e}"));

    let pub_mods = declared_pub_mods(&lib_rs);
    assert!(
        pub_mods.len() > 100,
        "parsed only {} `pub mod` declarations — the parser is broken (the \
         crate has well over 100)",
        pub_mods.len()
    );

    let on_page = modules_referenced_in_widgets_md(&md);
    assert!(
        on_page.len() > 100,
        "only {} modules are referenced in docs/widgets.md — either the file \
         is missing/empty or the link-parser is broken. A silent empty parse \
         would make every `pub mod` look excluded and the whole test \
         vacuous.",
        on_page.len()
    );

    let inventory = parse_exclusion_inventory(&lib_rs);
    let excluded: BTreeSet<String> = inventory.iter().map(|e| e.module.clone()).collect();

    // (a) Entries must be real `pub mod`s; a phantom entry looks like
    // coverage for a module that does not exist.
    let phantom: Vec<&str> = excluded
        .iter()
        .filter(|m| !pub_mods.contains(m.as_str()))
        .map(String::as_str)
        .collect();
    assert!(
        phantom.is_empty(),
        "the docs/widgets.md exclusion inventory lists names that are NOT \
         declared `pub mod` in lib.rs (renamed? removed?): {phantom:?}. \
         Remove or rename the entries — a phantom exclusion is a hole in \
         the completeness check."
    );

    // (b) Page and inventory must be disjoint; a module in both means its
    // exclusion reason went stale and must be deleted.
    let double: Vec<&str> = excluded
        .iter()
        .filter(|m| on_page.contains(m.as_str()))
        .map(String::as_str)
        .collect();
    assert!(
        double.is_empty(),
        "these modules are BOTH on docs/widgets.md AND on the exclusion \
         inventory: {double:?}. A row on the inventory says \"this is \
         deliberately off the page\"; if the module has since reached the \
         page, delete the inventory row so the check can catch the next \
         time a shipped widget silently drops off."
    );

    // (c) The gate: on the page or on the inventory.
    let uncovered: Vec<&str> = pub_mods
        .iter()
        .filter(|m| !on_page.contains(m.as_str()) && !excluded.contains(m.as_str()))
        .map(String::as_str)
        .collect();
    assert!(
        uncovered.is_empty(),
        "these `pub mod`s are neither documented in docs/widgets.md nor on \
         the exclusion inventory in lib.rs: {uncovered:?}. `rich_edit` and \
         `rich_text` have fallen into exactly this hole — \
         shipped but undiscoverable. Either:\n\n  \
         (1) add the module to a catalog group in the `# Widget catalog` \
             block, or add it to the keyword-alias table, so \
             docs/widgets.md references it, or\n  \
         (2) add a row to the `<!-- widgets-md-exclusion-inventory: -->` \
             HTML comment in lib.rs with a *per-module* reason explaining \
             why this module is `pub` for framework-internal composition \
             and never named in app code.\n\n\
         There is no third state: a `pub mod` neither on the page nor on \
         the inventory is a build failure, because that failure mode is \
         silent and permanent."
    );
}

/// Plausible floor on the inventory itself: an emptied inventory stops
/// enforcing anything. Shape-checked the way the catalog is.
#[test]
fn exclusion_inventory_is_populated_and_covers_real_modules() {
    let lib_rs = std::fs::read_to_string(src_dir().join("lib.rs")).expect("read lib.rs");
    let inventory = parse_exclusion_inventory(&lib_rs);

    // Below ~30 of ~60 internal modules, the inventory was almost certainly
    // mass-edited into lightness.
    assert!(
        inventory.len() >= 30,
        "the exclusion inventory has only {} entries — the crate has ~60 \
         internal-composition/audit modules that legitimately do not appear \
         on the catalog page, so an inventory this short means most of them \
         have been dropped and would silently fail the completeness gate. \
         If several genuinely reached the catalog, keep this floor honest by \
         lowering the number here rather than removing this test.",
        inventory.len()
    );

    // Reasons must not be boilerplate: one phrase stamped everywhere goes
    // unread.
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for row in &inventory {
        *counts.entry(row.reason.clone()).or_default() += 1;
    }
    let repeated: Vec<(&String, &usize)> = counts.iter().filter(|(_, n)| **n >= 5).collect();
    assert!(
        repeated.is_empty(),
        "these reasons appear ≥5 times across the exclusion inventory — the \
         inventory has drifted toward boilerplate, which is one step from an \
         inventory nobody reads: {repeated:?}. A reason should describe *why \
         this specific module* is not on the page; a stock \"internal\" \
         string just says \"tier 2\", which is not a reason."
    );
}
