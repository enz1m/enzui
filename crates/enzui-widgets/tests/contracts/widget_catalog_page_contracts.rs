use super::widget_catalog_page::*;
use std::collections::BTreeSet;

use crate::tiers::{live_tier1, src_dir};

// ---------------------------------------------------------------------------
// Focus-contract coverage
// ---------------------------------------------------------------------------

/// One caller-wiring contract a catalog entry is required to name.
///
/// See [`CALLER_WIRING_CONTRACTS`] for the criterion that puts a module here.
struct WiringContract {
    /// The tier-1 module whose catalog entry must name the contract.
    module: &'static str,
    /// Substrings (matched case-insensitively) the catalog summary must
    /// contain. These are the *names of the wiring* — the builder, the
    /// operation, the subscription — not prose, so a reworded entry that
    /// still names the same call keeps passing while one that drops the
    /// call reddens.
    summary_must_name: &'static [&'static str],
    /// A symbol that must still exist in the module's own code (doc
    /// comments are stripped before the search). The summary tells a caller
    /// to use this; if it is renamed or deleted the entry is now instructing
    /// people to write code that does not compile, which is a worse failure
    /// than saying nothing.
    ///
    /// It names the seam this crate owns, never an upstream one: iced's
    /// `text_input::focus` appears in `search_field.rs` only inside comments,
    /// because the *caller* is who calls it. What `search_field` must keep
    /// offering is the `.id(..)` builder that gives that call a target, so
    /// that is the anchor.
    source_anchor: &'static str,
    /// What a caller who reads only the catalog entry, then writes the
    /// obvious code, actually ships. This is the justification for the row
    /// and is quoted in the failure message.
    defect: &'static str,
}

/// The tier-1 entries whose caller wiring the catalog must name.
///
/// # The criterion, in one sentence
///
/// > An entry must name its wiring when a caller who reads only that entry
/// > and then writes the obvious call gets a widget that renders correctly
/// > and is functionally wrong — not merely unstyled or incomplete.
///
/// The load-bearing half is "renders correctly". `enz_search_field(..)` is
/// the shape: it looks finished on screen and is unusable by
/// keyboard, so nothing in the author's own review loop can catch it. A
/// widget that merely looks unfinished is self-correcting and does not
/// belong here — which is why the 143 audited entries mostly stay
/// as they are. `enz_divider` has no wiring contract; neither does
/// `enz_badge`. Adding a wiring sentence to entries with no wiring is how a
/// catalog becomes noise.
///
/// Each row below was confirmed by measuring the unwired call, not by
/// reading the module header — the header is where these contracts were
/// already hiding, so trusting it would re-derive the same blind spot.
///
/// The list is hand-curated because the distinction is semantic: no scan can
/// tell an *optional* `.id(..)` convenience from a *required* one. What the
/// test below supplies is the ratchet — a curated list with no gate is a
/// document that rots in a week.
/// The tier-1 modules that carry a named caller-wiring contract, as the
/// generator sees them. Exposed to [`render_json`](super::widget_catalog_page::render_json)
/// so `docs/widgets.json` can emit `wiring_contract: true` for exactly these
/// modules from the same curated list this suite gates — a second
/// hand-maintained copy is forbidden. A module is `wiring_contract: true` iff it appears
/// here; every other tier-1 module is emitted `false`, which is a real audited
/// verdict (see `docs/caller-wiring-audit.md`: most modules correctly name no
/// wiring), not an "unknown".
pub(crate) fn wiring_contract_modules() -> BTreeSet<&'static str> {
    CALLER_WIRING_CONTRACTS.iter().map(|c| c.module).collect()
}

const CALLER_WIRING_CONTRACTS: &[WiringContract] = &[
    WiringContract {
        module: "search_field",
        summary_must_name: &["focus"],
        source_anchor: "pub fn id",
        defect: "apps open a search surface without \
                 focusing the field. It renders perfectly and cannot be \
                 typed into without a mouse.",
    },
    WiringContract {
        module: "modal_host",
        summary_must_name: &["initial_focus_operation", "restore_focus_operation"],
        source_anchor: "initial_focus_operation",
        defect: "the host moves NO focus by itself. Measured: building \
                 enz_modal_host with a dialog present leaves ZERO focused \
                 nodes. 11 of 11 apps use the host; 2 call \
                 initial_focus_operation and 9 call restore_focus_operation, \
                 so the dialog opens with focus nowhere.",
    },
    WiringContract {
        module: "virtual_list",
        summary_must_name: &["scroll_offset"],
        source_anchor: "self_scrolling",
        defect: "the realised window follows the plumbed offset, not the \
                 inner scrollbar. Measured over 10,000 items: unwired it \
                 realises rows 0..=14 at every offset; with \
                 .scroll_offset(9000) it realises 295..=314. The pixels \
                 scroll and the content never changes.",
    },
    WiringContract {
        module: "virtual_grid",
        summary_must_name: &["scroll_offset"],
        source_anchor: "self_scrolling",
        defect: "same shape as virtual_list. Measured over 10,000 tiles: \
                 unwired realises 0..=48 at every offset; with \
                 .scroll_offset(9000) it realises 574..=629.",
    },
    WiringContract {
        module: "chat_list",
        summary_must_name: &["scroll_offset"],
        source_anchor: "scroll_offset",
        defect: "same shape, and this widget has NO self-scrolling mode, so \
                 the on_scroll -> update -> scroll_offset round trip is the \
                 only route. Measured over 10,000 messages: unwired realises \
                 0..=9 at every offset; with .scroll_offset(9000), 120..=134.",
    },
    WiringContract {
        module: "action_bar",
        summary_must_name: &["overflow_open"],
        source_anchor: "overflow_open",
        defect: "the bar measures the fold but does not own the menu's open \
                 state. Measured with 8 actions at 160 px: 1 focusable with \
                 the state unwired, 9 with .overflow_open(true). The folded \
                 actions have no keyboard OR pointer route at all.",
    },
    WiringContract {
        module: "titlebar_actions",
        summary_must_name: &["overflow_open"],
        source_anchor: "overflow_open",
        defect: "same shape as action_bar. Measured with 8 actions and \
                 force_compact: 1 focusable unwired, 9 with \
                 .overflow_open(true).",
    },
    WiringContract {
        module: "key_capture_chip",
        summary_must_name: &["armed"],
        source_anchor: "armed",
        defect: "`armed` is an INPUT the host owns, not state the chip \
                 latches. Activating the chip only publishes \
                 KeyCapture::Started; until the caller feeds .armed(true) \
                 back the chip records nothing, so a shortcut recorder built \
                 the obvious way is a button that does nothing.",
    },
    WiringContract {
        module: "streaming_text",
        summary_must_name: &["caret_visible"],
        source_anchor: "caret_visible",
        defect: "the widget owns no clock and has no `update` and no \
                 `Subscription` (grep: zero `fn update` in the module). The \
                 caret only blinks if the caller flips .caret_visible from an \
                 iced::time::every tick.",
    },
    WiringContract {
        module: "file_chooser",
        summary_must_name: &["pump"],
        source_anchor: "pub fn pump",
        defect: "FileRequest::start only OPENS the request. Without the \
                 caller also calling pump() from its own tick subscription, \
                 the native dialog appears and its outcome never arrives — \
                 the app silently never learns which file was picked.",
    },
    WiringContract {
        module: "sheet",
        summary_must_name: &["SheetPresenter::update"],
        source_anchor: "pub fn update",
        defect: "the resizable surface publishes typed gesture, grabber, and scroll events; without routing them through SheetPresenter::update it renders at one height but never changes detents",
    },
    WiringContract {
        module: "text_area",
        summary_must_name: &["auto_grow", "row_count", "line_count"],
        source_anchor: "pub fn auto_grow",
        defect: "a caller building an auto-growing composer reaches for \
                 the obvious `Content::line_count`, which counts LOGICAL `\\n` \
                 lines, not rendered rows. A pasted paragraph that wraps to \
                 five rows is one logical line, so the box is sized to one row \
                 and the other four are clipped — it renders and is silently \
                 wrong for every wrapped line. The entry must name `auto_grow` \
                 (the widget sizes itself) and `row_count` (the correct manual \
                 measurement), and warn off `line_count`.",
    },
];

#[test]
fn caller_wiring_contract_is_named_in_catalog_entry() {
    let model = read_model();
    let entries: Vec<_> = model.groups.iter().flat_map(|g| &g.entries).collect();

    let mut failures: Vec<String> = Vec::new();
    for contract in CALLER_WIRING_CONTRACTS {
        let Some(entry) = entries.iter().find(|e| e.module == contract.module) else {
            panic!(
                "CALLER_WIRING_CONTRACTS names `{}`, which is not a catalog \
                 entry. Remove the row or add the module to the catalog.",
                contract.module
            );
        };
        let lower = entry.summary.to_lowercase();
        for term in contract.summary_must_name {
            if !lower.contains(&term.to_lowercase()) {
                failures.push(format!(
                    "  {} — summary does not name `{}`.\n      why it matters: {}",
                    contract.module, term, contract.defect
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "the following catalog entries describe what the widget looks \
         like without naming the caller wiring that makes it work. A caller \
         who reads only the entry and writes the obvious call ships a widget \
         that RENDERS CORRECTLY and is functionally wrong — the shape \
         several apps hit independently.\n\n{}\n\n\
         Fix by editing the group-table bullet in \
         crates/enzui-widgets/src/lib.rs, then regenerate:\n\
         ENZUI_WRITE_WIDGET_DOC=1 cargo wt --test contracts widget_catalog_page",
        failures.join("\n")
    );
}

/// The call each contract's summary tells a caller to make must still exist
/// in that module's source.
///
/// Without this the previous test is only half a gate: it holds the *entry*
/// to naming a call, but nothing holds the *call* to existing. A rename
/// would leave the catalog instructing every reader to write code that does
/// not compile — a worse outcome than silence, because it is confidently
/// wrong.
#[test]
fn caller_wiring_anchors_still_exist_in_module_source() {
    let mut missing: Vec<String> = Vec::new();
    for contract in CALLER_WIRING_CONTRACTS {
        let dir = src_dir();
        let flat = dir.join(format!("{}.rs", contract.module));
        let nested = dir.join(contract.module).join("mod.rs");
        let mut paths = vec![if flat.exists() { flat } else { nested }];
        let submodule_dir = dir.join(contract.module);
        if submodule_dir.is_dir() {
            let mut submodules: Vec<_> = std::fs::read_dir(&submodule_dir)
                .unwrap_or_else(|e| panic!("read {}: {e}", submodule_dir.display()))
                .map(|entry| entry.expect("read module source entry").path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
                .collect();
            submodules.sort();
            paths.extend(submodules);
        }
        let code = paths
            .iter()
            .map(|path| {
                std::fs::read_to_string(path)
                    .unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
                    .lines()
                    .filter(|line| !line.trim_start().starts_with("//"))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !code.contains(contract.source_anchor) {
            missing.push(format!(
                "  {} — catalog entry tells callers to use `{}`, which no \
                 longer appears in {}",
                contract.module,
                contract.source_anchor,
                paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    assert!(
        missing.is_empty(),
        "a catalog entry names caller wiring that no longer exists in \
         the widget's source. Either restore the API or update BOTH the \
         CALLER_WIRING_CONTRACTS row and the catalog bullet in lib.rs:\n\n{}",
        missing.join("\n")
    );
}

/// Every `siblings` cell must name at least two `mod@<name>` links, and every
/// name must be a real `pub mod` in the crate.
///
/// This is the base sanity check the composes-with table has for its own
/// targets: a cell claiming disambiguation with a phantom module is a check
/// that reads convincingly and does nothing.
#[test]
fn sibling_choice_families_resolve_to_real_modules() {
    let lib_rs = std::fs::read_to_string(src_dir().join("lib.rs")).expect("read lib.rs");
    let rows = parse_sibling_choices(&lib_rs);
    assert!(
        !rows.is_empty(),
        "parsed zero sibling-choice rows -- the section marker moved or the \
         table was gutted; a silently empty parse is what the parser is meant \
         to panic on, so seeing this assertion instead means the section \
         renders but is empty"
    );
    let live = live_tier1(&lib_rs);

    for row in &rows {
        assert!(
            row.siblings.len() >= 2,
            "sibling-choice row `{}` lists {} sibling(s); the table \
             exists to disambiguate BETWEEN candidates, so a row with fewer \
             than two is either not a sibling family or should live in the \
             composes-with table instead. Raw cell: {:?}",
            row.family,
            row.siblings.len(),
            row.raw_siblings_cell,
        );
        for module in &row.siblings {
            assert!(
                live.contains(module),
                "sibling-choice row `{}` names `mod@{module}` in its \
                 `siblings` cell, but `{module}` is not a tier-1 `pub mod` in \
                 the crate. A row must point at real widgets; a phantom \
                 target is the same rot the composes-with table guards \
                 against.",
                row.family,
            );
        }
    }
}
