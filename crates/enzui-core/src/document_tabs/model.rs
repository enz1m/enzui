//! Keyed document-tab reconciliation, selection, focus, and reveal state.

use super::*;
use iced_core::keyboard;
use std::collections::HashMap;

/// A keyed document-tab collection with selection, pinned partitioning, focus, and
/// transient state.
#[derive(Debug, Clone)]
pub struct DocumentTabs {
    items: Vec<DocumentItem>,
    selected: Option<DocumentKey>,
    runtime: HashMap<DocumentKey, TabRuntime>,
    seq: u64,
    orientation: Orientation,
    focused: Option<DocumentKey>,
    reveal_pending: Option<DocumentKey>,
}

impl Default for DocumentTabs {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            selected: None,
            runtime: HashMap::new(),
            seq: 0,
            orientation: Orientation::Horizontal,
            focused: None,
            reveal_pending: None,
        }
    }
}

impl DocumentTabs {
    /// Creates an empty collection with no selection or focus.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build from an initial item list.
    ///
    /// # Errors
    /// [`DocumentTabsError::DuplicateKey`] if two items share a key.
    pub fn from_items(items: Vec<DocumentItem>) -> Result<Self, DocumentTabsError> {
        let mut tabs = Self::new();
        tabs.reconcile(items)?;
        Ok(tabs)
    }

    /// Returns all tabs in model order.
    #[must_use]
    pub fn items(&self) -> &[DocumentItem] {
        &self.items
    }

    /// How many tabs there are in total.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether there are no tabs at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Iterates all keys in model order.
    pub fn keys(&self) -> impl Iterator<Item = &DocumentKey> {
        self.items.iter().map(|i| &i.key)
    }

    /// Iterates pinned tabs in relative model order.
    pub fn pinned(&self) -> impl Iterator<Item = &DocumentItem> {
        self.items.iter().filter(|i| i.pinned)
    }

    /// Iterates unpinned tabs in relative model order.
    pub fn session(&self) -> impl Iterator<Item = &DocumentItem> {
        self.items.iter().filter(|i| !i.pinned)
    }

    /// The item for a key, if present.
    #[must_use]
    pub fn item(&self, key: &DocumentKey) -> Option<&DocumentItem> {
        self.items.iter().find(|i| &i.key == key)
    }

    /// Whether a key is present.
    #[must_use]
    pub fn contains(&self, key: &DocumentKey) -> bool {
        self.item(key).is_some()
    }

    /// Returns a key’s index in model order.
    #[must_use]
    pub fn position_of(&self, key: &DocumentKey) -> Option<usize> {
        self.items.iter().position(|i| &i.key == key)
    }

    /// Horizontal strip or vertical sidebar.
    #[must_use]
    pub fn orientation(&self) -> Orientation {
        self.orientation
    }

    /// Changes orientation without changing items, selection, focus, runtime state, or pending
    /// reveal.
    pub fn set_orientation(&mut self, orientation: Orientation) {
        self.orientation = orientation;
    }

    /// Builder form of [`set_orientation`](Self::set_orientation).
    #[must_use]
    pub fn with_orientation(mut self, orientation: Orientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// Iterates pinned tabs followed by session tabs, independent of orientation.
    pub fn visual_order(&self) -> impl Iterator<Item = &DocumentItem> {
        self.pinned().chain(self.session())
    }

    /// Iterates keys in visual order.
    pub fn visual_keys(&self) -> impl Iterator<Item = &DocumentKey> {
        self.visual_order().map(|i| &i.key)
    }

    /// Returns a key’s index in pinned-first visual order.
    #[must_use]
    pub fn visual_position_of(&self, key: &DocumentKey) -> Option<usize> {
        self.visual_keys().position(|k| k == key)
    }

    /// The selected key, or `None`.
    #[must_use]
    pub fn selected(&self) -> Option<&DocumentKey> {
        self.selected.as_ref()
    }

    /// The selected item, or `None`.
    #[must_use]
    pub fn selected_item(&self) -> Option<&DocumentItem> {
        self.selected.as_ref().and_then(|k| self.item(k))
    }

    /// The per-key transient state for a key, if the key is present.
    #[must_use]
    pub fn runtime(&self, key: &DocumentKey) -> Option<&TabRuntime> {
        if self.contains(key) {
            self.runtime.get(key)
        } else {
            None
        }
    }

    /// Returns the number of retained per-key runtime records.
    #[must_use]
    pub fn retained_state_len(&self) -> usize {
        self.runtime.len()
    }

    /// Selects an enabled key, returning a distinct no-op outcome for absent or disabled keys.
    pub fn select(&mut self, key: &DocumentKey) -> SelectOutcome {
        match self.item(key) {
            None => SelectOutcome::NotFound,
            Some(item) if !item.enabled => SelectOutcome::Disabled,
            Some(_) => {
                if self.selected.as_ref() == Some(key) {
                    return SelectOutcome::AlreadySelected;
                }
                self.selected = Some(key.clone());
                let rt = self.runtime.entry(key.clone()).or_default();
                rt.activations = rt.activations.saturating_add(1);
                self.reveal_pending = Some(key.clone());
                SelectOutcome::Changed
            }
        }
    }

    /// Clears the selection and any pending reveal.
    pub fn clear_selection(&mut self) {
        self.selected = None;
        self.reveal_pending = None;
    }

    /// Takes and clears the one pending reveal key, if any.
    pub fn take_pending_reveal(&mut self) -> Option<DocumentKey> {
        // Selection and reconciliation only leave live enabled keys pending, so no liveness check is
        // needed here.
        self.reveal_pending.take()
    }

    /// Returns the collection’s single roving focus key, if it has been established.
    #[must_use]
    pub fn focused(&self) -> Option<&DocumentKey> {
        self.focused.as_ref()
    }

    /// Returns whether `key` is the roving focus target.
    #[must_use]
    pub fn is_focused(&self, key: &DocumentKey) -> bool {
        self.focused.as_ref() == Some(key)
    }

    /// Focuses an enabled key without changing selection.
    ///
    /// Absent and disabled keys are no-ops with distinct outcomes.
    pub fn focus(&mut self, key: &DocumentKey) -> FocusOutcome {
        match self.item(key) {
            None => FocusOutcome::NotFound,
            Some(item) if !item.enabled => FocusOutcome::Disabled,
            Some(_) => {
                if self.focused.as_ref() == Some(key) {
                    return FocusOutcome::Unchanged(key.clone());
                }
                self.focused = Some(key.clone());
                FocusOutcome::Moved(key.clone())
            }
        }
    }

    /// Clears the roving focus key.
    pub fn clear_focus(&mut self) {
        self.focused = None;
    }

    /// Moves the roving focus key in visual order, skipping disabled tabs and clamping at the ends.
    ///
    /// With no current focus, `Last` enters at the last focusable tab and other moves enter at
    /// the first. Selection and reveal state are unchanged.
    pub fn move_focus(&mut self, movement: FocusMove) -> FocusOutcome {
        let order: Vec<DocumentKey> = self
            .visual_order()
            .filter(|i| i.enabled)
            .map(|i| i.key.clone())
            .collect();
        let (Some(first), Some(last)) = (order.first().cloned(), order.last().cloned()) else {
            self.focused = None;
            return FocusOutcome::Empty;
        };

        let target = match movement {
            FocusMove::First => first,
            FocusMove::Last => last,
            FocusMove::Previous | FocusMove::Next => {
                let at = self
                    .focused
                    .as_ref()
                    .and_then(|k| order.iter().position(|o| o == k));
                match at {
                    None => first,
                    Some(i) if movement == FocusMove::Previous => {
                        order[i.saturating_sub(1)].clone()
                    }
                    Some(i) => order[(i + 1).min(order.len() - 1)].clone(),
                }
            }
        };

        if self.focused.as_ref() == Some(&target) {
            return FocusOutcome::Unchanged(target);
        }
        self.focused = Some(target.clone());
        FocusOutcome::Moved(target)
    }

    /// Maps and applies a key press for the current orientation, returning `None` for keys the
    /// strip does not own.
    pub fn focus_for_key(
        &mut self,
        key: &keyboard::Key,
        modifiers: keyboard::Modifiers,
        rtl: bool,
    ) -> Option<FocusOutcome> {
        let movement = FocusMove::for_key(key, modifiers, self.orientation, rtl)?;
        Some(self.move_focus(movement))
    }

    /// Replaces all items while preserving runtime state by key and repairing selection and focus.
    ///
    /// Removed or disabled targets move to the next enabled neighbor in their prior order, then
    /// the previous neighbor, then the first newly enabled tab. Selection uses model order;
    /// focus uses visual order.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentTabsError::DuplicateKey`] without changing the collection.
    ///
    /// # Complexity
    ///
    /// Runs in linear expected time over the old and new collections.
    pub fn reconcile(&mut self, next: Vec<DocumentItem>) -> Result<(), DocumentTabsError> {
        // Validate all keys before mutation so duplicate input leaves the collection unchanged.
        let mut seen: HashMap<&DocumentKey, ()> = HashMap::with_capacity(next.len());
        for item in &next {
            if seen.insert(&item.key, ()).is_some() {
                return Err(DocumentTabsError::DuplicateKey(item.key.clone()));
            }
        }

        let previous_order: Vec<DocumentKey> = self.items.iter().map(|i| i.key.clone()).collect();
        let previous_selected = self.selected.clone();
        let previous_visual: Vec<DocumentKey> = self.visual_keys().cloned().collect();
        let previous_focus = self.focused.clone();

        let mut runtime = HashMap::with_capacity(next.len());
        for item in &next {
            let existing = self.runtime.get(&item.key).copied();
            let rt = existing.unwrap_or_else(|| {
                self.seq += 1;
                TabRuntime {
                    first_seen: self.seq,
                    activations: 0,
                }
            });
            runtime.insert(item.key.clone(), rt);
        }
        self.runtime = runtime;
        self.items = next;

        // Build one borrowed enabled-key lookup for both repairs; do not retain it as ordering state.
        let enabled_lookup: HashMap<&DocumentKey, bool> = self
            .items
            .iter()
            .map(|item| (&item.key, item.enabled))
            .collect();

        let new_model_keys: Vec<&DocumentKey> = self.items.iter().map(|i| &i.key).collect();
        let new_visual_keys: Vec<&DocumentKey> = Self::visual_key_order(&self.items);

        let repaired_selected = Self::repair_target(
            &enabled_lookup,
            &previous_order,
            &new_model_keys,
            previous_selected.as_ref(),
        );
        let repaired_focus = Self::repair_target(
            &enabled_lookup,
            &previous_visual,
            &new_visual_keys,
            previous_focus.as_ref(),
        );

        self.apply_repaired_selection(repaired_selected);
        // Apply only the repaired focus key; focus repair never activates or reveals a tab.
        self.focused = repaired_focus.key();
        Ok(())
    }

    fn visual_key_order(items: &[DocumentItem]) -> Vec<&DocumentKey> {
        items
            .iter()
            .filter(|i| i.pinned)
            .chain(items.iter().filter(|i| !i.pinned))
            .map(|i| &i.key)
            .collect()
    }

    fn repair_target(
        enabled: &HashMap<&DocumentKey, bool>,
        previous_order: &[DocumentKey],
        new_order: &[&DocumentKey],
        previous: Option<&DocumentKey>,
    ) -> RepairOutcome {
        let Some(previous) = previous else {
            return RepairOutcome::None;
        };

        if Self::present_and_enabled(enabled, previous) {
            return RepairOutcome::Survived(previous.clone());
        }

        let selectable = |k: &DocumentKey| -> Option<DocumentKey> {
            Self::present_and_enabled(enabled, k).then(|| k.clone())
        };
        let old_index = previous_order
            .iter()
            .position(|k| k == previous)
            .unwrap_or(previous_order.len());

        let forward = previous_order.iter().skip(old_index).find_map(&selectable);
        let backward = || {
            previous_order
                .iter()
                .take(old_index)
                .rev()
                .find_map(&selectable)
        };
        let fallback = || {
            new_order
                .iter()
                .copied()
                .find(|k| Self::present_and_enabled(enabled, k))
                .cloned()
        };

        match forward.or_else(backward).or_else(fallback) {
            Some(key) => RepairOutcome::Promoted(key),
            None => RepairOutcome::None,
        }
    }

    #[inline]
    fn present_and_enabled(enabled: &HashMap<&DocumentKey, bool>, key: &DocumentKey) -> bool {
        reconcile_probes::bump();
        enabled.get(key).copied().unwrap_or(false)
    }

    fn apply_repaired_selection(&mut self, repaired: RepairOutcome) {
        if let RepairOutcome::Survived(key) = &repaired {
            self.selected = Some(key.clone());
            return;
        }
        self.selected = repaired.key();
        if let Some(key) = self.selected.clone() {
            let rt = self.runtime.entry(key.clone()).or_default();
            rt.activations = rt.activations.saturating_add(1);
            self.reveal_pending = Some(key);
        } else {
            self.reveal_pending = None;
        }
    }
}
