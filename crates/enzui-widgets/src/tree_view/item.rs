//! `tree_view`: item.
//!
//! Owns the item model for the tree-view implementation.

use super::*;

/// One row of a pre-flattened tree.
///
/// # Presentation vs identity vs announcement
///
/// This type carries the row's model, never its `Element`.
/// Three separate strings do three separate jobs and are
/// deliberately not collapsed:
///
/// | field | answers | consumed by |
/// |---|---|---|
/// | [`text`](Self::text) | what the *default* row draws | the built-in text row body |
/// | [`aria_label`](Self::aria_label) | what AT announces | the row's `TreeItem` accessible name |
/// | [`key`](Self::key) | which node this is | the routable id and the keyed cursor |
///
/// A row whose body is app-supplied (see
/// [`EnzTreeView::row_content`]) still takes its accessible
/// name from a `String` here — `aria_label` when set, else
/// `text`. The name is not derived from the body because an
/// `Element` is opaque to this widget, so a row named after
/// its content would be named after whatever the app
/// happened to draw first, and a row drawing only a status
/// dot and an avatar would be anonymous to a screen reader.
#[derive(Debug, Clone)]
pub struct EnzTreeItem {
    /// Row label. Drawn by the default row body, and the fallback
    /// accessible name when [`aria_label`](Self::aria_label) is unset.
    ///
    /// A row with an app-supplied body ([`EnzTreeView::row_content`]) does not
    /// draw this — but it is still the row's announced name, so leaving it
    /// empty on a custom row makes that row anonymous unless
    /// [`aria_label`](Self::aria_label) is set.
    pub text: String,
    /// The row's accessible name, overriding [`text`](Self::text).
    ///
    /// Set this when the announced name is not the drawn string:
    ///
    /// * the row's body is app-supplied and draws no single authoritative line
    ///   (a title + metadata stack, a status dot, an avatar);
    /// * the drawn label is an abbreviation (`"~/Dl"` announced as
    ///   `"Downloads"`);
    /// * the drawn label is a glyph.
    ///
    /// Blank (empty or whitespace-only) is treated as unset, not as an
    /// empty name: `Some("")` is worse than no name, because a screen reader
    /// cannot tell it apart from a name that failed to resolve. Same rule
    /// [`text`](Self::text) already follows.
    ///
    /// On the default text row the drawn label is forwarded to the
    /// collector as `content_text`, so overriding the name here does not make
    /// the row announce twice.
    pub aria_label: Option<String>,
    /// Zero-based indentation depth.
    pub level: u8,
    /// If true, this row shows a disclosure chevron.
    pub has_children: bool,
    /// If true, the chevron points down; else right.
    pub expanded: bool,
    /// Per-item selection, consulted only when the view-level
    /// [`EnzTreeView::selected`] index is `None`.
    ///
    /// Restores the source's item-level fallback: a host that tracks selection
    /// on its own model rather than as one index (a multi-select tree, or a
    /// selection owned by a document) has somewhere to put it.
    pub selected: bool,
    /// Stable logical key for this node, used to derive the row's routable
    /// widget id. `None` falls back to [`text`](Self::text).
    ///
    /// Set this whenever the label is not a stable unique name for the node —
    /// a file id, a database key, a path segment. The row's id is derived from
    /// the ancestor chain of keys, not from this segment alone, so two
    /// nodes may share a segment as long as their parents differ.
    pub key: Option<String>,
}

impl EnzTreeItem {
    /// A leaf row.
    pub fn leaf(text: impl Into<String>, level: u8) -> Self {
        Self {
            text: text.into(),
            aria_label: None,
            level,
            has_children: false,
            expanded: false,
            selected: false,
            key: None,
        }
    }

    /// Set this row's stable logical key. See [`Self::key`].
    #[must_use]
    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into());
        self
    }

    /// Set this row's accessible name, overriding the drawn
    /// [`text`](Self::text). See [`Self::aria_label`].
    ///
    /// This is what a row with app-supplied content
    /// ([`EnzTreeView::row_content`]) must use when its body
    /// is not a single authoritative line of prose, so its
    /// `Role::TreeItem` is named.
    #[must_use]
    pub fn aria_label(mut self, label: impl Into<String>) -> Self {
        self.aria_label = Some(label.into());
        self
    }

    /// The row's resolved accessible name, or `None` when the row is
    /// legitimately anonymous.
    ///
    /// Precedence: an explicit [`aria_label`](Self::aria_label) wins, else the
    /// drawn [`text`](Self::text). Blank in either slot is treated as *unset*
    /// rather than as an empty name — `Some("")` is indistinguishable to a
    /// screen reader from a name that failed to resolve, and it defeats the
    /// "is this row named?" question.
    ///
    /// A blank `aria_label` therefore falls through to `text` rather than
    /// blanking the name: `.aria_label("")` is far more likely to be an empty
    /// host string than a deliberate request for anonymity, and silently
    /// dropping a name the row already had would be the worse of the two
    /// mistakes.
    ///
    /// Returned untrimmed, because the default row's drawn label is matched
    /// against this string verbatim by the collector's duplicate-suppression
    /// rule. Trimming here would leave the drawn copy unmatched and the
    /// row would announce twice.
    pub(crate) fn accessible_name(&self) -> Option<&str> {
        self.aria_label
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .or(Some(self.text.as_str()))
            .filter(|s| !s.trim().is_empty())
    }

    /// The row's host-set logical key, or `None` when the
    /// host set none. A row without a key is not routable and
    /// `collect.rs` reports so.
    ///
    /// No label fallback: a label is presentation text and
    /// carries neither property AT identity needs.
    ///
    /// * Not unique — two folders can both contain `mod.rs`,
    ///   and two siblings can legally carry the same display
    ///   name.
    /// * Not stable — a rename shifts the AccessKit id, which
    ///   an AT cache and the keyed cursor cannot follow.
    ///
    /// Occurrence disambiguation could paper over the first
    /// but never the second.
    pub(crate) fn logical_key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// The key segment this row contributes to the
    /// reconciliation path used by the keyed keyboard cursor:
    /// its explicit [`key`](Self::key) when set, else its
    /// label.
    ///
    /// # Why the label fallback is safe here
    ///
    /// This key never leaves the widget. It exists so the
    /// cursor can be re-resolved against the next frame's
    /// row list when the host inserts, removes or collapses
    /// rows, and it is compared only against other keys
    /// derived in the same frame by the same rule. It is not
    /// published to assistive technology, is not an `Id`, and
    /// nothing outside this widget can hold one across a
    /// rebuild.
    ///
    /// Its weakness is bounded and visible: renaming a
    /// keyless row between two frames moves the cursor to
    /// the nearest-survivor row instead of following the
    /// rename. That is a recoverable, one-keystroke
    /// annoyance, which is why the label fallback is confined
    /// to this in-widget segment and does not extend to the
    /// AT-published id [`logical_key`](Self::logical_key)
    /// returns.
    pub(crate) fn reconciliation_segment(&self) -> &str {
        self.key.as_deref().unwrap_or(&self.text)
    }

    /// A branch row.
    pub fn branch(text: impl Into<String>, level: u8, expanded: bool) -> Self {
        Self {
            text: text.into(),
            aria_label: None,
            level,
            has_children: true,
            expanded,
            selected: false,
            key: None,
        }
    }

    /// Mark this row selected (the per-item fallback).
    #[must_use]
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// The navigation view of this row.
    pub(crate) fn as_nav_row(&self) -> TreeRow {
        TreeRow {
            level: self.level,
            has_children: self.has_children,
            expanded: self.expanded,
        }
    }
}
