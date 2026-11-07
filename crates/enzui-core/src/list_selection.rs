//! Renderer-neutral list selection and activation policy.
//!
//! Selection is keyed by stable ids; ranges resolve against current list order.
//! Filtering preserves absent ids; [`ListSelection::reconcile`] reports deletion.
//!
//! # Example
//! ```
//! use enzui_core::list_selection::{ClickModifiers, IndexedClickModel, ListSelection};
//! let order = IndexedClickModel::new(["first", "second", "last"]);
//! let mut selection = ListSelection::<&str>::default();
//! selection.click(&order, &"second", ClickModifiers::PLAIN);
//! selection.click(&order, &"last", ClickModifiers::RANGE);
//! assert_eq!(selection.len(), 2);
//! ```

use crate::drag::ItemId;
use std::collections::{BTreeSet, HashMap};
use std::hash::Hash;
use std::sync::Arc;

/// Pointer modifiers for toggle and range selection gestures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClickModifiers {
    /// The toggle modifier (Ctrl on Linux/Windows, Cmd on macOS).
    pub toggle: bool,
    /// The range-extend modifier (Shift).
    pub range: bool,
}

impl ClickModifiers {
    /// No modifiers — a plain click.
    pub const PLAIN: Self = Self {
        toggle: false,
        range: false,
    };
    /// Ctrl/Cmd only — toggle one item.
    pub const TOGGLE: Self = Self {
        toggle: true,
        range: false,
    };
    /// Shift only — select a range, replacing the rest.
    pub const RANGE: Self = Self {
        toggle: false,
        range: true,
    };
    /// Ctrl/Cmd+Shift — add a range to the existing selection.
    pub const TOGGLE_RANGE: Self = Self {
        toggle: true,
        range: true,
    };
}

/// Controls whether an unmodified pointer click activates or selects a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMode {
    /// A plain pointer click activates the row without changing selection.
    #[default]
    Browse,
    /// A plain pointer click applies selection; keyboard activation still activates the row.
    Selection,
}

/// Reports whether a routed gesture was ignored, activated, or applied selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClickOutcome<Id = ItemId> {
    /// The id was absent from the current model, so no state changed.
    Ignored,
    /// Activates the row without changing selection.
    Activated(Id),
    /// Applies selection and reports whether durable state changed.
    Selected {
        /// Whether the selection state actually changed.
        changed: bool,
    },
}

/// Stores selected ids, range endpoints, and pointer-click mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListSelection<Id = ItemId> {
    selected: BTreeSet<Id>,
    anchor: Option<Id>,
    active: Option<Id>,
    mode: SelectionMode,
}

impl<Id> Default for ListSelection<Id> {
    fn default() -> Self {
        Self {
            selected: BTreeSet::new(),
            anchor: None,
            active: None,
            mode: SelectionMode::default(),
        }
    }
}

impl ListSelection<ItemId> {
    /// An empty selection with no anchor using the framework's standard item id.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl<Id> ListSelection<Id>
where
    Id: Clone + Ord,
{
    /// Whether `id` is currently selected.
    #[must_use]
    pub fn contains<Q>(&self, id: &Q) -> bool
    where
        Id: std::borrow::Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.selected.contains(id)
    }

    /// How many items are selected.
    #[must_use]
    pub fn len(&self) -> usize {
        self.selected.len()
    }

    /// Whether nothing is selected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    /// The current anchor id, if any.
    #[must_use]
    pub fn anchor(&self) -> Option<&Id> {
        self.anchor.as_ref()
    }

    /// The moving endpoint used by keyboard navigation.
    #[must_use]
    pub fn active(&self) -> Option<&Id> {
        self.active.as_ref()
    }

    /// Iterate selected ids in stable key order.
    pub fn iter(&self) -> impl Iterator<Item = &Id> {
        self.selected.iter()
    }

    /// Add an id without changing the gesture endpoints.
    pub fn insert(&mut self, id: Id) -> bool {
        self.selected.insert(id)
    }

    /// Remove an id without changing the gesture endpoints.
    pub fn remove(&mut self, id: &Id) -> bool {
        self.selected.remove(id)
    }

    /// Retains matching selected ids and clears endpoints absent from the retained selection.
    pub fn retain(&mut self, mut keep: impl FnMut(&Id) -> bool) {
        self.selected.retain(|id| keep(id));
        if self
            .anchor
            .as_ref()
            .is_some_and(|id| !self.selected.contains(id))
        {
            self.anchor = None;
        }
        if self
            .active
            .as_ref()
            .is_some_and(|id| !self.selected.contains(id))
        {
            self.active = None;
        }
    }

    /// Clear range/cursor endpoints while preserving the selected set.
    pub fn clear_endpoints(&mut self) {
        self.anchor = None;
        self.active = None;
    }

    /// Replace the selected set after a non-pointer bulk gesture.
    pub fn replace<I>(&mut self, ids: I, anchor: Option<Id>, active: Option<Id>)
    where
        I: IntoIterator<Item = Id>,
    {
        self.selected = ids.into_iter().collect();
        self.anchor = anchor;
        self.active = active;
    }

    /// Returns selected ids in current list order, omitting ids absent from `order`.
    #[must_use]
    pub fn selected_in_order(&self, order: &[Id]) -> Vec<Id> {
        order
            .iter()
            .filter(|id| self.selected.contains(*id))
            .cloned()
            .collect()
    }

    /// Applies a selection click against the current ordered model.
    ///
    /// Unknown ids are ignored and return `false`; otherwise the result reports whether state changed.
    pub fn click(
        &mut self,
        model: &impl SelectionOrder<Id>,
        id: &Id,
        mods: ClickModifiers,
    ) -> bool {
        let Some(clicked_idx) = model.index_of(id) else {
            return false;
        };

        let before = self.snapshot();

        match (mods.range, mods.toggle) {
            (false, false) => {
                self.selected.clear();
                self.selected.insert(id.clone());
                self.anchor = Some(id.clone());
            }
            (false, true) => {
                if !self.selected.remove(id) {
                    self.selected.insert(id.clone());
                }
                // Toggle gestures preserve the range anchor.
            }
            (true, false) => match self.resolved_anchor_index(model) {
                Some(anchor_idx) => {
                    let range = span(anchor_idx, clicked_idx, model);
                    self.selected = range.into_iter().collect();
                    // Range gestures preserve their original anchor across successive extensions.
                }
                None => {
                    self.selected.clear();
                    self.selected.insert(id.clone());
                    self.anchor = Some(id.clone());
                }
            },
            (true, true) => match self.resolved_anchor_index(model) {
                Some(anchor_idx) => {
                    let range = span(anchor_idx, clicked_idx, model);
                    for rid in range {
                        self.selected.insert(rid);
                    }
                }
                None => {
                    if !self.selected.remove(id) {
                        self.selected.insert(id.clone());
                    }
                    // Clear an absent anchor so reinserting its id cannot revive a range origin the user no longer has.
                    self.clear_stale_anchor(model);
                }
            },
        }

        self.active = Some(id.clone());
        self.snapshot() != before
    }

    /// The current selection mode: whether a plain click activates or selects.
    ///  See [`SelectionMode`].
    #[must_use]
    pub fn mode(&self) -> SelectionMode {
        self.mode
    }

    /// Enters selection mode without clearing the current selection.
    ///
    /// Returns whether the mode changed.
    pub fn enter_selection_mode(&mut self) -> bool {
        if self.mode == SelectionMode::Selection {
            return false;
        }
        self.mode = SelectionMode::Selection;
        true
    }

    /// Returns to browse mode and clears selection and range endpoints.
    ///
    /// Returns whether any state changed.
    pub fn leave_selection_mode(&mut self) -> bool {
        let mode_changed = self.mode != SelectionMode::Browse;
        let selection_changed = !self.selected.is_empty();
        let anchor_changed = self.anchor.is_some();
        let active_changed = self.active.is_some();
        self.mode = SelectionMode::Browse;
        self.selected.clear();
        self.anchor = None;
        self.active = None;
        mode_changed || selection_changed || anchor_changed || active_changed
    }

    /// Clears selection and range endpoints without changing the mode.
    ///
    /// Returns whether any state changed.
    pub fn clear(&mut self) -> bool {
        let changed = !self.selected.is_empty() || self.anchor.is_some() || self.active.is_some();
        self.selected.clear();
        self.anchor = None;
        self.active = None;
        changed
    }

    /// Routes a pointer click according to the current mode and modifiers.
    ///
    /// Unknown ids return [`ClickOutcome::Ignored`].
    pub fn on_pointer_click(
        &mut self,
        model: &impl SelectionOrder<Id>,
        id: &Id,
        mods: ClickModifiers,
    ) -> ClickOutcome<Id> {
        if model.index_of(id).is_none() {
            return ClickOutcome::Ignored;
        }
        let is_plain = !mods.toggle && !mods.range;
        if is_plain && self.mode == SelectionMode::Browse {
            return ClickOutcome::Activated(id.clone());
        }
        let changed = self.click(model, id, mods);
        ClickOutcome::Selected { changed }
    }

    /// Activates a known id without changing selection, in either mode.
    ///
    /// Unknown ids return [`ClickOutcome::Ignored`].
    pub fn on_activate(&mut self, model: &impl SelectionOrder<Id>, id: &Id) -> ClickOutcome<Id> {
        if model.index_of(id).is_none() {
            return ClickOutcome::Ignored;
        }
        ClickOutcome::Activated(id.clone())
    }

    /// Removes selected ids and endpoints absent from `model`.
    ///
    /// Call this for deletion, not temporary filtering. Returns whether state changed.
    pub fn reconcile(&mut self, model: &impl SelectionOrder<Id>) -> bool {
        let before_len = self.selected.len();
        self.selected.retain(|id| model.index_of(id).is_some());
        let selection_changed = self.selected.len() != before_len;
        let anchor_changed = self.clear_stale_anchor(model);
        let active_changed = if self
            .active
            .as_ref()
            .is_some_and(|id| model.index_of(id).is_none())
        {
            self.active = None;
            true
        } else {
            false
        };
        selection_changed || anchor_changed || active_changed
    }

    fn clear_stale_anchor(&mut self, model: &impl SelectionOrder<Id>) -> bool {
        if self
            .anchor
            .as_ref()
            .is_some_and(|a| model.index_of(a).is_none())
        {
            self.anchor = None;
            true
        } else {
            false
        }
    }

    fn resolved_anchor_index(&self, model: &impl SelectionOrder<Id>) -> Option<usize> {
        self.anchor.as_ref().and_then(|a| model.index_of(a))
    }

    fn snapshot(&self) -> (Vec<Id>, Option<Id>, Option<Id>) {
        let ids: Vec<Id> = self.selected.iter().cloned().collect();
        (ids, self.anchor.clone(), self.active.clone())
    }
}

/// Immutable item order used by pointer gestures and deletion reconciliation.
///
/// The slice contains unique ids; `index_of` must resolve positions in that slice.
pub trait SelectionOrder<Id> {
    /// The complete order, including items outside the visible viewport.
    fn order(&self) -> &[Id];

    /// Resolves a stable id to its current position, or returns `None` if absent.
    fn index_of(&self, id: &Id) -> Option<usize>;
}

impl<Id, T: SelectionOrder<Id> + ?Sized> SelectionOrder<Id> for Arc<T> {
    fn order(&self) -> &[Id] {
        (**self).order()
    }

    fn index_of(&self, id: &Id) -> Option<usize> {
        (**self).index_of(id)
    }
}

/// The ordered, deduplicated ids used to resolve positional gestures.
#[derive(Debug, Clone)]
pub struct ClickModel<Id = ItemId> {
    order: Arc<[Id]>,
}

impl<Id> ClickModel<Id>
where
    Id: Clone + Ord,
{
    /// Builds a model in input order, keeping the first occurrence of each id.
    #[must_use]
    pub fn new(order: impl IntoIterator<Item = Id>) -> Self {
        let mut seen = BTreeSet::new();
        let mut order_out = Vec::new();
        for id in order {
            if seen.insert(id.clone()) {
                order_out.push(id);
            }
        }
        Self {
            order: order_out.into(),
        }
    }

    /// The ids, in order.
    #[must_use]
    pub fn order(&self) -> &[Id] {
        &self.order
    }

    /// The index of `id` in the list, or `None` if absent.
    #[must_use]
    pub fn index_of(&self, id: &Id) -> Option<usize> {
        self.order.iter().position(|x| x == id)
    }
}

impl<Id: Clone + Ord> SelectionOrder<Id> for ClickModel<Id> {
    fn order(&self) -> &[Id] {
        self.order()
    }

    fn index_of(&self, id: &Id) -> Option<usize> {
        self.index_of(id)
    }
}

/// Shared immutable selection order with expected constant-time id lookup.
///
/// Build once when the directory or projection changes, preferably off the UI
/// thread. Clones share both the order and index; ranges clone only their ids.
#[derive(Debug, Clone)]
pub struct IndexedClickModel<Id = ItemId> {
    data: Arc<IndexedOrder<Id>>,
}

#[derive(Debug)]
struct IndexedOrder<Id> {
    order: Vec<Id>,
    positions: HashMap<Id, usize>,
}

impl<Id: Clone + Eq + Hash> IndexedClickModel<Id> {
    /// Keeps the first occurrence of each id in input order.
    #[must_use]
    pub fn new(order: impl IntoIterator<Item = Id>) -> Self {
        let mut positions = HashMap::new();
        let mut unique = Vec::new();
        for id in order {
            if let std::collections::hash_map::Entry::Vacant(entry) = positions.entry(id.clone()) {
                entry.insert(unique.len());
                unique.push(id);
            }
        }
        Self {
            data: Arc::new(IndexedOrder {
                order: unique,
                positions,
            }),
        }
    }

    /// Borrows the shared order without cloning directory entries.
    #[must_use]
    pub fn order(&self) -> &[Id] {
        &self.data.order
    }

    /// Resolves an id in expected constant time, including missing ids.
    #[must_use]
    pub fn index_of(&self, id: &Id) -> Option<usize> {
        self.data.positions.get(id).copied()
    }
}

impl<Id: Clone + Eq + Hash> SelectionOrder<Id> for IndexedClickModel<Id> {
    fn order(&self) -> &[Id] {
        self.order()
    }

    fn index_of(&self, id: &Id) -> Option<usize> {
        self.index_of(id)
    }
}

fn span<Id: Clone + Ord>(a: usize, b: usize, model: &impl SelectionOrder<Id>) -> Vec<Id> {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    model.order()[lo..=hi].to_vec()
}
