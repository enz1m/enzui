//! `tree_view`: focusing the tree at a named row.
//!
//! # The gap this closes
//!
//! An [`EnzTreeView`] row is deliberately not its own focus stop: the tree owns
//! the single Tab stop and the rows rove under it, publishing the cursor row as
//! the group's `active_descendant` (WAI-ARIA TreeView; see
//! [`crate::tree_view`]'s module docs). Every generic "return focus to the
//! thing that opened me" primitive available —
//! [`enzui_core::focus::RestoreFocus`], iced's own
//! `operation::focusable::focus` — restores focus to a widget id. Aimed at
//! a tree row id those find no focusable at all: `RestoreFocus` leaves focus
//! wherever the dismissal put it, and raw `focus(row_id)` clears focus from the
//! whole window. So a context menu opened from
//! [`EnzTreeView::row_on_context`](crate::tree_view::EnzTreeView::row_on_context)
//! could name the right row in the AT tree and still lose the focus ring, the
//! roving cursor and the active descendant when it closed.
//!
//! Restoring focus to a tree row is therefore two moves that must happen
//! together: focus the *tree*, and put its roving cursor on the *row*. This
//! module is that one shared move, so no app translates a row id into a private
//! focus scheme of its own.
//!
//! # Addressed by `id_prefix`
//!
//! The row ids an app already computes come from
//! [`tree_row_id`](crate::tree_view::tree_row_id)`(id_prefix, path)`. The
//! `id_prefix` is therefore the address the app already holds for that
//! tree, and it is the namespace that must already be unique per tree for row
//! ids to be unique at all. Minting a second, independent widget id — and
//! defaulting it — would publish a second `Id` per tree that collides between
//! two default-prefixed trees on one screen, which is the duplicate-node abort
//! in [`enzui_core::collect::SemanticsCollector`]. A host that
//! *does* set [`EnzTreeView::id`](crate::tree_view::EnzTreeView::id) can name
//! the tree by that id instead — [`TreeTarget`] carries either.
//!
//! # Two passes
//!
//! Pass one is a probe: it walks the [`TreeRowFocusRequest`] carriers every
//! tree publishes during `operate` and asks whether the addressed tree is on
//! screen at all. Only if it is does pass two run, and only pass two moves
//! focus. That is the same non-destructive shape
//! [`enzui_core::focus::RestoreFocus`] uses and for the same reason: a row
//! (or its whole tree) can be removed by the very command the menu just fired,
//! and a restoration that misses must leave focus alone rather than clear it.
//!
//! Pass two unfocuses every focusable it visits and then hands the addressed
//! tree its focus back through the carrier. Focus stays exclusive (one
//! focused widget in the window, the invariant `tests/focus/exclusivity.rs`
//! pins), which a bare `state.focus()` on the tree would break.
//!
//! # What the tree does with a row that is gone
//!
//! Nothing stale. The widget resolves the requested row id against this
//! frame's published row ids; a miss leaves the roving cursor where
//! reconciliation already put it (the nearest survivor) and still focuses
//! the tree, so the user lands back in the tree they invoked the menu from
//! rather than nowhere. The `active_descendant` published in that same pass is
//! whatever the live cursor names — never the removed row.

use iced::Rectangle;
use iced::advanced::widget::{Id, Operation, operation::Outcome};

/// How a [`FocusTreeRow`] operation names the tree it is aimed at.
///
/// Two ways in, because a tree has two stable addresses and hosts use both:
///
/// * [`TreeTarget::prefix`] — the tree's
///   [`id_prefix`](crate::tree_view::EnzTreeView::id_prefix), the same string
///   the app passes to [`tree_row_id`](crate::tree_view::tree_row_id) to build
///   the row id. Always present (it has a default), so this works for a tree
///   with no widget id at all.
/// * [`TreeTarget::widget`] — an explicit
///   [`EnzTreeView::id`](crate::tree_view::EnzTreeView::id). Use it when two
///   trees share one prefix but have distinct ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeTarget {
    /// Address the tree whose `id_prefix` equals this string.
    Prefix(String),
    /// Address the tree whose widget [`Id`] equals this one.
    Widget(Id),
}

impl TreeTarget {
    /// Address a tree by its [`id_prefix`](crate::tree_view::EnzTreeView::id_prefix).
    #[must_use]
    pub fn prefix(prefix: impl Into<String>) -> Self {
        Self::Prefix(prefix.into())
    }

    /// Address a tree by its explicit widget [`Id`].
    #[must_use]
    pub fn widget(id: impl Into<Id>) -> Self {
        Self::Widget(id.into())
    }

    /// Whether `prefix`/`id` — the pair one tree publishes on its carrier —
    /// is the tree this target names.
    fn matches(&self, prefix: &str, id: Option<&Id>) -> bool {
        match self {
            Self::Prefix(p) => p == prefix,
            Self::Widget(target) => id == Some(target),
        }
    }
}

/// The carrier an [`EnzTreeView`](crate::tree_view::EnzTreeView) passes to
/// `Operation::custom` so a [`FocusTreeRow`] can find it, and so the tree —
/// which owns the roving cursor — performs the move itself.
///
/// Non-generic and `'static`, which is what lets it travel through
/// `Operation::custom`'s `&mut dyn Any` without imposing `Message: 'static` on
/// the widget: the same carrier shape as
/// [`ContextMenuRequest`](crate::context_menu_action::ContextMenuRequest).
///
/// The requested row id goes in; the resolution happens inside the widget
/// (it owns the row ids and the cursor keys), so no operation ever holds a copy
/// of a tree's row table and the two cannot drift.
#[derive(Debug, Clone)]
pub struct TreeRowFocusRequest {
    /// The publishing tree's row-id namespace.
    prefix: String,
    /// The publishing tree's explicit widget id, if the host set one.
    id: Option<Id>,
    /// The row an operation wants the cursor moved to, once claimed.
    requested_row: Option<Id>,
    /// That row's ancestor key path, when the caller knew it.
    ///
    /// Carried alongside the id, never instead of it: the id is what a present
    /// row is matched by, and the path is what makes a *removed* row degrade
    /// to its deepest surviving ancestor instead of to nothing. An `Id` is
    /// opaque once minted — the path cannot be recovered from it — so a caller
    /// that has the path must hand it over for the tree to reason about.
    requested_path: Option<Vec<String>>,
    /// Whether an operation claimed this tree (`true` even when it named no
    /// row: "focus this tree" is a valid claim on its own).
    claimed: bool,
    /// Whether the claim asks for focus to be taken, as opposed to merely
    /// probing for presence (pass one).
    apply: bool,
}

impl TreeRowFocusRequest {
    /// A carrier announcing a tree at `prefix` (and `id`, when it has one).
    #[must_use]
    pub fn new(prefix: impl Into<String>, id: Option<Id>) -> Self {
        Self {
            prefix: prefix.into(),
            id,
            requested_row: None,
            requested_path: None,
            claimed: false,
            apply: false,
        }
    }

    /// The publishing tree's `id_prefix`.
    #[must_use]
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The publishing tree's widget id, if any.
    #[must_use]
    pub fn id(&self) -> Option<&Id> {
        self.id.as_ref()
    }

    /// Whether an operation claimed this tree this pass.
    #[must_use]
    pub fn claimed(&self) -> bool {
        self.claimed
    }

    /// Whether the claim is the focus-moving pass rather than the probe pass.
    #[must_use]
    pub fn apply(&self) -> bool {
        self.apply
    }

    /// The row the claim asks the cursor to be moved to, if it named one.
    #[must_use]
    pub fn requested_row(&self) -> Option<&Id> {
        self.requested_row.as_ref()
    }

    /// The requested row's ancestor key path, if the caller supplied one.
    #[must_use]
    pub fn requested_path(&self) -> Option<&[String]> {
        self.requested_path.as_deref()
    }

    /// Claim this tree. Called by [`FocusTreeRow`], not by widgets.
    fn claim(&mut self, row: Option<Id>, path: Option<Vec<String>>, apply: bool) {
        self.claimed = true;
        self.apply = apply;
        self.requested_row = row;
        self.requested_path = path;
    }
}

/// The [`Operation`] that focuses a tree at a row: the shared "return focus
/// to this tree row" move.
///
/// Build one with [`focus_tree_row`]. Run it with
/// `iced::advanced::widget::operate`.
///
/// See [`crate::tree_view`]'s module docs for why this is not
/// `focus` (`iced::advanced::widget::operation::focusable::focus`): a row is
/// not its own focus stop. It runs in two passes so the destructive move — the
/// one that takes focus — happens only after a non-destructive probe has
/// proven the tree is present.
#[derive(Debug, Clone)]
pub struct FocusTreeRow {
    /// The tree to focus.
    target: TreeTarget,
    /// The row its roving cursor should land on. `None` focuses the tree and
    /// leaves the cursor alone — the honest fallback when the caller knows the
    /// row is gone.
    row: Option<Id>,
    /// That row's ancestor key path, when known: the tree degrades a removed
    /// row to its deepest surviving ancestor rather than to nothing.
    path: Option<Vec<String>>,
    /// Set in the probe pass when the addressed tree is really on screen.
    found: bool,
    /// `true` in the second (focus-moving) pass.
    applying: bool,
    /// Once-only latch for the applying pass, so two windows or a re-run
    /// cannot double-apply.
    applied: bool,
}

impl FocusTreeRow {
    /// Focus the tree named by `target`, roving its cursor onto `row`.
    #[must_use]
    pub fn new(target: TreeTarget, row: Option<Id>) -> Self {
        Self {
            target,
            row,
            path: None,
            found: false,
            applying: false,
            applied: false,
        }
    }

    /// Tell the tree the requested row's ancestor key path — the same
    /// `path` [`tree_row_id`](crate::tree_view::tree_row_id) derived the id
    /// from.
    ///
    /// With it, a row that has been removed by the command the menu just fired
    /// degrades to its deepest surviving ancestor (the collapsed branch,
    /// the parent folder) rather than leaving the cursor wherever it happened
    /// to be. Without it the fallback is still safe — focus the tree, keep the
    /// live cursor — just less specific.
    #[must_use]
    pub fn with_path(mut self, path: Vec<String>) -> Self {
        self.path = Some(path);
        self
    }

    /// The requested row's ancestor key path, if one was supplied.
    #[must_use]
    pub fn path(&self) -> Option<&[String]> {
        self.path.as_deref()
    }

    /// The tree this operation is aimed at.
    #[must_use]
    pub fn target(&self) -> &TreeTarget {
        &self.target
    }

    /// The row the cursor is asked to land on, if any.
    #[must_use]
    pub fn row(&self) -> Option<&Id> {
        self.row.as_ref()
    }

    /// Whether the addressed tree was found on screen during the probe pass.
    ///
    /// `false` means nothing moved at all — the tree was removed along with
    /// the row, and focus was deliberately left where it was.
    #[must_use]
    pub fn found(&self) -> bool {
        self.found
    }

    /// The second pass, built from the first. Private: the chain builds it.
    fn applying_pass(&self) -> Self {
        Self {
            target: self.target.clone(),
            row: self.row.clone(),
            path: self.path.clone(),
            found: true,
            applying: true,
            applied: false,
        }
    }
}

impl<T> Operation<T> for FocusTreeRow {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        // Keep descending: a tree can be nested arbitrarily deep behind
        // containers, scrollables and split panes.
        operate(self);
    }

    fn focusable(
        &mut self,
        _id: Option<&Id>,
        _bounds: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        // Probe pass: touch nothing. Focus is only ever moved once the tree is
        // known to be present, which is what keeps a missed restoration
        // harmless rather than window-clearing.
        if !self.applying {
            return;
        }
        // Applying pass: focus is exclusive, so every other focusable gives it
        // up. The tree gets its own focus back through the carrier below —
        // `TreeViewWidget::operate` publishes the carrier *after* its
        // `operation.focusable` call, so the tree's unfocus here is followed,
        // in the same visit, by the claim that re-focuses it.
        state.unfocus();
    }

    fn custom(&mut self, _id: Option<&Id>, _bounds: Rectangle, state: &mut dyn std::any::Any) {
        // Only a tree's row-focus carrier concerns us; every other `custom`
        // payload (semantics nodes, activation requests, context-menu
        // requests, scan probes) fails the downcast.
        let Some(request) = state.downcast_mut::<TreeRowFocusRequest>() else {
            return;
        };
        if !self.target.matches(request.prefix(), request.id()) {
            return;
        }
        if self.applying {
            // Exactly once, even though an operation is re-run per window and
            // per chained pass.
            if self.applied {
                return;
            }
            self.applied = true;
            request.claim(self.row.clone(), self.path.clone(), true);
        } else {
            self.found = true;
            // Probe claims carry no `apply`, so the widget records presence
            // without moving anything.
            request.claim(None, None, false);
        }
    }

    fn finish(&self) -> Outcome<T> {
        if self.found && !self.applying {
            // The tree is really there, so the destructive half is safe.
            Outcome::Chain(Box::new(self.applying_pass()))
        } else {
            Outcome::None
        }
    }
}

/// Build the operation that focuses the tree named by `target` and roves its
/// cursor onto `row`.
///
/// `row` is an [`Option`] because "the row is gone, put me back in the tree" is
/// a first-class outcome, not an error: pass `None` and the tree takes focus
/// with its cursor left on whatever survived.
#[must_use]
pub fn focus_tree_row(target: TreeTarget, row: Option<Id>) -> FocusTreeRow {
    FocusTreeRow::new(target, row)
}
