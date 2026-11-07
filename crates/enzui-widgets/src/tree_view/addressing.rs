//! `tree_view`: addressing.
//!
//! Owns stable row addressing and identity derivation for the tree-view implementation.

use super::*;

/// The default id namespace for tree rows.
pub const ID_PREFIX: &str = "enz-tree-view";

/// Derive one routable `Id` per row, keyed by the
/// node's path of host-supplied logical keys.
///
/// Returns `None` for any row that — or whose ancestor chain — lacks an
/// explicit [`EnzTreeItem::key`]. That row is published anonymous.
///
/// # Identity comes only from keys
///
/// The path is derived only from explicit keys, never from a row's label. A
/// label makes a tempting fallback — deriving the path from
/// `key.unwrap_or(label)` and leaning on
/// [`KeyedIds`](enzui_core::identity::KeyedIds) to disambiguate whatever
/// collides — but occurrence disambiguation fixes uniqueness and cannot fix
/// stability, and a label has neither property.
///
/// * Renaming a row would change its published id. An AT holding the pre-rename
///   id addresses nothing; the row it was on is re-announced as a new node.
/// * Worse, occurrence disambiguation makes a rename *shift its neighbours* —
///   renaming the first of two `mod.rs` rows turns the second from
///   `…/mod.rs#2` into `…/mod.rs`, so a row nobody touched changes identity.
///
/// # Whole ancestor chain must be keyed
///
/// A keyed row under an unkeyed parent is not identified: its parent's
/// segment is the thing that distinguishes `src/mod.rs` from `tests/mod.rs`,
/// and if the parent contributes nothing stable the child's path is not stable
/// either. So the requirement propagates down the chain, and a partially-keyed
/// tree yields ids only for the fully-keyed rows.
///
/// # Index-derived ids are forbidden
///
/// An index-derived id makes focus follow a *slot*: expanding a branch
/// re-points every AT request below it. This is forbidden by the rule in
/// [`enzui_core::identity`]'s module docs.
///
/// # Encoding
///
/// Each path segment is a separate
/// [`OwnerId::child`](enzui_core::identity::OwnerId::child) component
/// rather than a `/`-joined string, so a segment containing `/` cannot forge
/// another row's path (`["a/b", "c"]` and `["a", "b/c"]` are distinct rows and
/// now get distinct ids; joined, they did not).
pub(crate) fn row_ids(
    items: &[EnzTreeItem],
    prefix: &str,
) -> Vec<Option<iced::advanced::widget::Id>> {
    row_owners(items, prefix)
        .into_iter()
        .map(|owner| owner.as_ref().map(|o| o.id(ROW_ROLE)))
        .collect()
}

/// Derive the routable id of each row's context-menu affordance.
///
/// The same owner chain as [`row_ids`] under a different fixed role, so the row
/// and its right-click affordance are two nodes of one identity rather than two
/// independent derivations that can drift apart. Two nodes under one `Id` is
/// what `SemanticsCollector` aborts on, hence the distinct role.
///
/// # Derived from the path, not the label
///
/// That is [`list_row_surface`](crate::list_row_surface)'s rule and it is right
/// *there*: a flat list row's accessible name is its logical key. A tree row's
/// is not. `src/mod.rs` and `tests/mod.rs` share a label and are different
/// nodes, and a rename must not re-point a routed action — the two properties
/// [`row_ids`] deliberately has no label fallback. Deriving the
/// affordance from the label would reintroduce exactly that bug one node to the
/// side, so the affordance follows the row's path
/// instead, and an unkeyed row gets no affordance id at all.
pub(crate) fn row_context_ids(
    items: &[EnzTreeItem],
    prefix: &str,
) -> Vec<Option<iced::advanced::widget::Id>> {
    row_owners(items, prefix)
        .into_iter()
        .map(|owner| owner.as_ref().map(|o| o.id(ROW_CONTEXT_ROLE)))
        .collect()
}

/// The per-row owner namespace every published row id is minted from.
///
/// One walk, one pathing rule: [`row_ids`] and [`row_context_ids`] differ only
/// in the fixed role they ask this owner for. A second copy of the ancestry
/// walk is the defect (two derivations that agree until they do not, and
/// then a context menu opens on the wrong row).
fn row_owners(items: &[EnzTreeItem], prefix: &str) -> Vec<Option<enzui_core::identity::OwnerId>> {
    let root = enzui_core::identity::OwnerId::of_kind(ID_PREFIX, prefix);
    // The owner of each open ancestor, indexed by level. `None` marks an
    // ancestor with no logical key, which poisons the chain below it.
    let mut ancestry: Vec<Option<enzui_core::identity::OwnerId>> = Vec::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let level = usize::from(item.level);
        // Truncate to this row's depth. `min` guards a malformed list that
        // jumps a level (0 -> 2) without panicking.
        ancestry.truncate(level.min(ancestry.len()));
        // The parent's owner: the deepest surviving entry, or the root.
        let parent = match ancestry.last() {
            // Parent exists but is unkeyed -> this row cannot be identified.
            Some(None) => None,
            Some(Some(owner)) => Some(owner.clone()),
            None => Some(root.clone()),
        };
        let own = match (parent, item.logical_key()) {
            (Some(parent), Some(key)) => Some(parent.child(key)),
            _ => None,
        };
        out.push(own.clone());
        ancestry.push(own);
    }
    out
}

/// The fixed logical role a tree row's node id carries. Fixed, never the label:
/// see [`row_ids`].
const ROW_ROLE: &str = "row";

/// The fixed logical role of a row's context-menu affordance. Distinct
/// from [`ROW_ROLE`] because the two are separate published nodes.
const ROW_CONTEXT_ROLE: &str = "row-ctx";

/// Compute the routable widget `Id` (`iced::advanced::widget::Id`) an
/// [`EnzTreeView`] derives for the row whose ancestor-key `path` is given,
/// under `id_prefix`.
///
/// Mirrors the internal `row_ids` derivation exactly: same [`ID_PREFIX`]
/// namespace, same `OwnerId` child chain, same `ROW_ROLE`. Public so an app
/// migrating an existing custom row set to a shared tree can recompute the
/// row id from stable model state — for focus restoration, or for a
/// context-menu host that names its invoker by session id — without
/// duplicating the pathing rule and drifting out of sync (the defect).
///
/// For a flat tree, `path = &[key]` — one segment per level, top-down.
#[must_use]
pub fn tree_row_id(id_prefix: &str, path: &[&str]) -> iced::advanced::widget::Id {
    let mut owner = enzui_core::identity::OwnerId::of_kind(ID_PREFIX, id_prefix);
    for segment in path {
        owner = owner.child(segment);
    }
    owner.id(ROW_ROLE)
}

/// Compute the routable widget `Id` (`iced::advanced::widget::Id`) an
/// [`EnzTreeView`] derives for the *context-menu affordance* of the row at
/// `path`.
///
/// Same owner chain as [`tree_row_id`] under the internal `ROW_CONTEXT_ROLE`
/// segment, mirroring the internal `row_context_ids` derivation. A
/// `context_menu_host` whose `at_id` names this id publishes its `ShowContextMenu` node at the exact
/// slot the tree's own row surface advertises, so an AT-routed
/// `ShowContextMenu` reaches the menu without going through the row's
/// pointer path.
#[must_use]
pub fn tree_row_context_id(id_prefix: &str, path: &[&str]) -> iced::advanced::widget::Id {
    let mut owner = enzui_core::identity::OwnerId::of_kind(ID_PREFIX, id_prefix);
    for segment in path {
        owner = owner.child(segment);
    }
    owner.id(ROW_CONTEXT_ROLE)
}

/// The per-row reconciliation keys: `/`-joined ancestor paths, as
/// plain `String`s, so the keyboard cursor can be stored and re-resolved by row
/// identity across model changes.
///
/// # Differences from [`row_ids`]
///
/// The two functions answer different questions and only one result is
/// published:
///
/// | | [`row_ids`] | `row_keys` |
/// |---|---|---|
/// | consumed by | assistive technology, across frames | this widget, between two adjacent frames |
/// | keyless row | no id (anonymous) | falls back to the label |
/// | cost of being wrong | an AT action operates the wrong row | the cursor jumps to a neighbour |
///
/// A label fallback is unacceptable in a published id (a rename silently
/// re-points a routed action) and acceptable here (a rename moves the cursor to
/// the nearest survivor, which the user sees and can correct with one
/// keystroke). Keeping the label fallback *only* on the internal side is what
/// lets an unkeyed tree still have a usable keyboard cursor while refusing to
/// tell an AT something untrue.
///
/// Occurrence disambiguation via
/// [`KeyedIds::key_for`](enzui_core::identity::KeyedIds::key_for) is
/// retained here, because a cursor key must be unique within the frame to
/// resolve to one row at all.
pub(crate) fn row_keys(items: &[EnzTreeItem], prefix: &str) -> Vec<String> {
    let mut keyed = enzui_core::identity::KeyedIds::new(prefix);
    let mut ancestry: Vec<&str> = Vec::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let level = usize::from(item.level);
        ancestry.truncate(level.min(ancestry.len()));
        ancestry.push(item.reconciliation_segment());
        out.push(keyed.key_for(&ancestry.join("/")));
    }
    out
}

/// The stable selection key of row `index` in `items`, for
/// [`EnzTreeView::selected_key`].
///
/// Returns the same path-derived identity the widget reconciles selection
/// against — so a host that captures
/// `selection_key_for(&items, i)` when a row is clicked, and feeds it back via
/// `.selected_key(Some(key))`, keeps the selection on that node across any
/// insert/remove/reorder or ancestor expand/collapse. `None` if `index` is out
/// of range (an empty tree, a stale index) — `0` is a valid index.
///
/// The key depends only on the rows' `level`/`key`/`text`, so the host can
/// build it from the same `Vec<EnzTreeItem>` it hands the widget without
/// touching the widget's internals or reimplementing the pathing rule (which
/// would be a second copy that drifts — the defect).
#[must_use]
pub fn selection_key_for(items: &[EnzTreeItem], index: usize) -> Option<SequenceKey> {
    // Reuse the exact reconciliation-key derivation the widget uses, so a key
    // built here resolves against the widget's `row_keys` with no translation.
    let keys = row_keys(items, ID_PREFIX);
    keys.get(index).map(|k| SequenceKey::new(k.clone()))
}

/// The deepest ancestor of `key` that is present in `keys`.
///
/// A row key is a `/`-joined path (`src/widgets/button.rs`). Collapsing an
/// ancestor removes the descendant rows but keeps the ancestor, so when the
/// cursor's own key is gone the walk follows its parent chain — dropping one trailing
/// `/segment` at a time — and return the first ancestor that still has a row.
/// That is the collapsed branch the user can see and just acted on.
///
/// Returns `None` if no ancestor survives (the whole subtree was removed rather
/// than collapsed), so the caller can fall back to the generic
/// nearest-survivor policy.
///
/// The occurrence suffix (``) only ever appears on the *final* segment of a
/// derived key (it disambiguates the full path), so truncating at `/` never
/// splits inside a suffix.
pub(crate) fn nearest_present_ancestor(key: &str, keys: &[&str]) -> Option<String> {
    let mut cur = key;
    while let Some(slash) = cur.rfind('/') {
        cur = cur.split_at(slash).0;
        if keys.contains(&cur) {
            return Some(cur.to_owned());
        }
    }
    None
}
