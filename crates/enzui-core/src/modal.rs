//! Modal presentation stacks, topmost arbitration, typed dismissal causes, and exactly-once resolution.

use crate::dismissal::{DismissPolicy, EscapePolicy, OutsidePolicy};

/// Identifies one showing of a presentation and is never reused within its [`ModalStack`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PresentationId(u64);

impl PresentationId {
    /// The id of the first presentation a fresh [`ModalStack`] mints.
    ///
    /// `0` is expressible intent — the first showing — not a sentinel for
    /// "no presentation".
    pub const FIRST: PresentationId = PresentationId(0);

    /// The next id, or `None` when the finite namespace is exhausted.
    ///
    /// Exhaustion must fail closed: wrapping to [`FIRST`](Self::FIRST) would
    /// let a delayed event from the first showing authorize a new showing.
    #[must_use]
    pub fn next(self) -> Option<PresentationId> {
        self.0.checked_add(1).map(PresentationId)
    }

    /// The raw counter, for diagnostics and stable-id derivation only. Never
    /// compare ids across stacks — an id is meaningful only within the stack
    /// that mints it.
    #[must_use]
    pub fn raw(self) -> u64 {
        self.0
    }
}

impl Default for PresentationId {
    fn default() -> Self {
        PresentationId::FIRST
    }
}

/// Reports that an event id does not match the live showing for its key.
///
/// Both the submitted and current ids are retained for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaleDismissal {
    /// The id the stale event was stamped with.
    pub event: PresentationId,
    /// The live id for the key, or `None` when that key is not presented.
    pub current: Option<PresentationId>,
}

impl core::fmt::Display for StaleDismissal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.current {
            Some(current) => write!(
                f,
                "stale modal dismissal: event presentation {} != current presentation {}",
                self.event.0, current.0
            ),
            None => write!(
                f,
                "stale modal dismissal: event presentation {} but no such key is presented",
                self.event.0
            ),
        }
    }
}

impl std::error::Error for StaleDismissal {}

/// Describes how a modal presentation ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DismissCause {
    /// The Escape key.
    Escape,
    /// A press landed outside the surface, on the scrim.
    Scrim,
    /// A drag dismissed the surface by crossing its distance or velocity threshold.
    Drag,
    /// The surface's own close affordance (an `x` button).
    CloseButton,
    /// The dialog's default/affirmative action ("Save", "Delete").
    DefaultAction,
    /// The dialog's cancel action ("Cancel", "Not now").
    CancelAction,
    /// An assistive technology asked to dismiss the dialog.
    Assistive,
}

/// Selects the [`DismissPolicy`] field that governs a dismissal cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PolicyAxis {
    /// Governed by [`DismissPolicy::escape`].
    Escape,
    /// Governed by [`DismissPolicy::outside`].
    Outside,
}

impl DismissCause {
    /// Returns the policy axis for an ambient dismissal, or `None` for an explicit surface action.
    #[must_use]
    pub const fn governing_axis(self) -> Option<PolicyAxis> {
        match self {
            Self::Escape | Self::Assistive => Some(PolicyAxis::Escape),
            // Drag and scrim dismissal share the outside-policy axis.
            Self::Scrim | Self::Drag => Some(PolicyAxis::Outside),
            Self::CloseButton | Self::DefaultAction | Self::CancelAction => None,
        }
    }

    /// Returns whether this cause is governed by [`DismissPolicy`].
    #[must_use]
    pub const fn is_policy_governed(self) -> bool {
        self.governing_axis().is_some()
    }

    /// Returns whether the cause originated from a pointer or touch gesture.
    #[must_use]
    pub const fn is_pointer_ambient(self) -> bool {
        matches!(self, Self::Scrim | Self::Drag)
    }

    /// Returns `false` only for the affirmative action, so new causes default to cancellation.
    #[must_use]
    pub const fn is_cancellation(self) -> bool {
        !matches!(self, Self::DefaultAction)
    }
}

/// Returns whether `cause` is permitted, treating assistive dismissal like Escape.
#[must_use]
pub fn permits(policy: DismissPolicy, cause: DismissCause) -> bool {
    match cause.governing_axis() {
        None => true,
        Some(PolicyAxis::Escape) => matches!(policy.escape, EscapePolicy::Dismiss),
        Some(PolicyAxis::Outside) => matches!(policy.outside, OutsidePolicy::Dismiss),
    }
}

/// One live modal presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presentation<Key, Focus> {
    key: Key,
    id: PresentationId,
    policy: DismissPolicy,
    restore_focus: Option<Focus>,
}

impl<Key, Focus> Presentation<Key, Focus> {
    /// Returns the caller-defined stable identity of the dialog.
    pub const fn key(&self) -> &Key {
        &self.key
    }

    /// Returns the showing id, which changes when the same key is reopened.
    #[must_use]
    pub const fn id(&self) -> PresentationId {
        self.id
    }

    /// The dismissal policy this presentation was opened with.
    #[must_use]
    pub const fn policy(&self) -> DismissPolicy {
        self.policy
    }

    /// The focus target to restore when this presentation ends, if the
    /// invoker declared one.
    pub const fn restore_focus(&self) -> Option<&Focus> {
        self.restore_focus.as_ref()
    }
}

/// Contains the identity, cause, and optional focus target of an ended presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution<Key, Focus> {
    /// Which dialog ended.
    pub key: Key,
    /// Identifies the specific showing that ended.
    pub id: PresentationId,
    /// What ended it.
    pub cause: DismissCause,
    /// Where focus should return, if the invoker declared a target.
    pub restore_focus: Option<Focus>,
}

impl<Key, Focus> Resolution<Key, Focus> {
    /// Whether the user backed out rather than confirming.
    #[must_use]
    pub const fn is_cancellation(&self) -> bool {
        self.cause.is_cancellation()
    }
}

/// Contains the requested resolution and any presentations unwound above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unwound<Key, Focus> {
    /// The presentation the caller named.
    pub resolved: Resolution<Key, Focus>,
    /// Presentations that were stacked above it and cannot outlive it,
    /// returned top-down so a caller can unwind them in order.
    pub orphaned: Vec<Resolution<Key, Focus>>,
}

/// Stores live modal presentations in bottom-to-top order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModalStack<Key, Focus> {
    entries: Vec<Presentation<Key, Focus>>,
    // Next showing id; `None` permanently refuses new presentations after exhaustion.
    next_id: Option<PresentationId>,
}

impl<Key, Focus> Default for ModalStack<Key, Focus> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Key, Focus> ModalStack<Key, Focus> {
    /// Creates an empty stack whose first showing uses [`PresentationId::FIRST`].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_id: Some(PresentationId::FIRST),
        }
    }

    /// How many presentations are live.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is presented.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The topmost presentation — the one that owns Escape, the scrim, and
    /// focus containment.
    #[must_use]
    pub fn topmost(&self) -> Option<&Presentation<Key, Focus>> {
        self.entries.last()
    }

    /// Returns whether any live presentation makes the application background inert.
    #[must_use]
    pub fn blocks_background(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.policy.modality.consumes_outside_input())
    }

    /// Every live presentation, bottom to top.
    pub fn iter(&self) -> impl Iterator<Item = &Presentation<Key, Focus>> {
        self.entries.iter()
    }
}

impl<Key: PartialEq, Focus> ModalStack<Key, Focus> {
    /// Returns whether `key` owns the topmost modal scope.
    pub fn is_topmost(&self, key: &Key) -> bool {
        self.topmost().is_some_and(|entry| &entry.key == key)
    }

    /// Whether `key` is currently presented at any depth.
    pub fn contains(&self, key: &Key) -> bool {
        self.entries.iter().any(|entry| &entry.key == key)
    }

    /// Returns the live showing id for `key`, if present.
    #[must_use]
    pub fn current_id(&self, key: &Key) -> Option<PresentationId> {
        self.entries
            .iter()
            .find(|entry| &entry.key == key)
            .map(|entry| entry.id)
    }

    /// Returns whether `id` identifies the currently live showing for `key`.
    #[must_use]
    pub fn is_current_presentation(&self, key: &Key, id: PresentationId) -> bool {
        self.current_id(key) == Some(id)
    }

    /// Changes the dismissal policy of `key`'s live presentation, as a
    /// dialog does while an operation it started must finish first. Returns
    /// whether `key` is presented.
    pub fn set_policy(&mut self, key: &Key, policy: DismissPolicy) -> bool {
        match self.entries.iter_mut().find(|entry| &entry.key == key) {
            Some(entry) => {
                entry.policy = policy;
                true
            }
            None => false,
        }
    }

    /// Presents a unique key and returns a fresh showing id.
    ///
    /// Returns `None` when the key is already live or the id namespace is exhausted; refused presentations do not consume an id.
    pub fn present(
        &mut self,
        key: Key,
        policy: DismissPolicy,
        restore_focus: Option<Focus>,
    ) -> Option<PresentationId> {
        if self.contains(&key) {
            return None;
        }
        let id = self.next_id?;
        self.next_id = id.next();
        self.entries.push(Presentation {
            key,
            id,
            policy,
            restore_focus,
        });
        Some(id)
    }

    /// Dismisses the topmost presentation when its policy permits `cause`.
    ///
    /// Returns `None` when the stack is empty or the policy refuses dismissal.
    pub fn dismiss_topmost(&mut self, cause: DismissCause) -> Option<Resolution<Key, Focus>> {
        let policy = self.topmost()?.policy;
        if !permits(policy, cause) {
            return None;
        }
        let entry = self.entries.pop()?;
        Some(Resolution {
            key: entry.key,
            id: entry.id,
            cause,
            restore_focus: entry.restore_focus,
        })
    }

    /// Dismisses the topmost presentation only when `id` matches its live showing.
    ///
    /// A missing or mismatched topmost id returns [`StaleDismissal`] before policy evaluation; a matched cause refused by policy returns `Ok(None)`.
    ///
    /// # Errors
    ///
    /// [`StaleDismissal`] when `id` is not the live topmost presentation.
    pub fn dismiss_topmost_checked(
        &mut self,
        id: PresentationId,
        cause: DismissCause,
    ) -> Result<Option<Resolution<Key, Focus>>, StaleDismissal> {
        let current = self.topmost().map(|entry| entry.id);
        if current != Some(id) {
            return Err(StaleDismissal { event: id, current });
        }
        Ok(self.dismiss_topmost(cause))
    }

    /// Dismisses `key` at any depth and returns presentations above it in top-down order.
    pub fn dismiss(&mut self, key: &Key, cause: DismissCause) -> Option<Unwound<Key, Focus>> {
        let index = self.entries.iter().position(|entry| &entry.key == key)?;
        if !permits(self.entries[index].policy, cause) {
            return None;
        }
        let mut orphaned = Vec::new();
        while self.entries.len() > index + 1 {
            let entry = self.entries.pop()?;
            orphaned.push(Resolution {
                key: entry.key,
                id: entry.id,
                cause,
                restore_focus: entry.restore_focus,
            });
        }
        let entry = self.entries.pop()?;
        Some(Unwound {
            resolved: Resolution {
                key: entry.key,
                id: entry.id,
                cause,
                restore_focus: entry.restore_focus,
            },
            orphaned,
        })
    }

    /// Dismisses `key` only when `id` matches its live showing.
    ///
    /// A mismatch returns [`StaleDismissal`] before policy evaluation; success also unwinds presentations above the key.
    ///
    /// # Errors
    ///
    /// [`StaleDismissal`] when `id` is not a live presentation.
    pub fn dismiss_checked(
        &mut self,
        key: &Key,
        id: PresentationId,
        cause: DismissCause,
    ) -> Result<Option<Unwound<Key, Focus>>, StaleDismissal> {
        let current = self.current_id(key);
        if current != Some(id) {
            return Err(StaleDismissal { event: id, current });
        }
        Ok(self.dismiss(key, cause))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Dlg {
        Form,
        Confirm,
    }

    type Stack = ModalStack<Dlg, &'static str>;

    #[test]
    fn dismissing_the_topmost_by_key_orphans_nothing() {
        let mut stack = Stack::new();
        stack.present(Dlg::Form, DismissPolicy::modal(), None);
        stack.present(Dlg::Confirm, DismissPolicy::modal(), None);

        let unwound = stack
            .dismiss(&Dlg::Confirm, DismissCause::DefaultAction)
            .expect("present and permitted");
        assert_eq!(unwound.resolved.key, Dlg::Confirm);
        assert!(unwound.orphaned.is_empty());
        assert!(stack.is_topmost(&Dlg::Form));
    }
}
