//! Cooperative unsaved-changes close interception.
//!
//! Applications must disable iced’s automatic close handling, route every cooperative close request through [`CloseGuard`], and issue a window-close task only for [`CloseGuardAction::Close`]. Save All proceeds sequentially; non-vetoable shutdown should rely on work the application already saved as it changed ([`crate::autosave`], with [`crate::recovery`] copies for unsaved documents), or call [`CloseGuard::force_close`]. [`crate::undo::UndoStack::is_clean`] says whether a document has unsaved changes.

use std::{collections::VecDeque, sync::Arc};

use iced_core::{Event, window};

/// Value for iced’s `exit_on_close_request` setting.
///
/// `false` keeps close requests interceptable; the application must close only after receiving [`CloseGuardAction::Close`].
pub const EXIT_ON_CLOSE_REQUEST: bool = false;

/// A cooperative request to close one live window.
///
/// Use [`Self::from_event`] for compositor requests and [`Self::for_window`] for application-owned close controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloseRequest {
    window: window::Id,
}

impl CloseRequest {
    /// Convert a real iced close event into a guarded request.
    #[must_use]
    pub fn from_event(event: &Event, window: window::Id) -> Option<Self> {
        matches!(event, Event::Window(window::Event::CloseRequested)).then_some(Self { window })
    }

    /// Creates a guarded request for an application-owned titlebar or Quit action without closing the window.
    #[must_use]
    pub const fn for_window(window: window::Id) -> Self {
        Self { window }
    }

    /// The window which is still alive while this request is reviewed.
    #[must_use]
    pub const fn window(self) -> window::Id {
        self.window
    }
}

/// A dirty item identified by a stable save key and visible review name.
///
/// Names may repeat, but keys must be unique within one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsavedItem<Key> {
    /// Stable application identity for the save path.
    pub key: Key,
    /// Visible document or item name shown in the close-decision surface.
    pub name: String,
}

impl<Key> UnsavedItem<Key> {
    /// Pair a stable key with its visible name.
    pub fn new(key: Key, name: impl Into<String>) -> Self {
        Self {
            key,
            name: name.into(),
        }
    }
}

/// The user's one decision for all items in a pending close request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseDecision {
    /// Save every pending item, one at a time, and close only after all succeed.
    SaveAll,
    /// Close without saving any pending item.
    Discard,
    /// Veto this close. The application and every dirty item remain live.
    Cancel,
}

/// Result of the app's save path for the item requested by
/// [`CloseGuardAction::Save`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveOutcome {
    /// The item reached durable application storage and is no longer dirty.
    Saved,
    /// Saving failed. Keep the window open and show the app's error.
    Failed,
    /// The user cancelled an asynchronous Save As operation.
    Cancelled,
}

/// Opaque identity for one save attempt.
///
/// Return the token unchanged to [`CloseGuard::save_finished`]; cancellation, retry, and advancing each invalidate the previous token.
#[derive(Debug, Clone)]
pub struct SaveAttemptToken(Arc<()>);

impl SaveAttemptToken {
    fn fresh() -> Self {
        Self(Arc::new(()))
    }
}

impl PartialEq for SaveAttemptToken {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for SaveAttemptToken {}

/// An action the host must execute after a guard transition.
///
/// Only [`Self::Close`] may produce a window-close task; [`Self::Hold`] leaves the window alive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseGuardAction<Key> {
    /// Keeps the window alive; when a request is pending, its items remain available for one decision surface.
    Hold,
    /// Runs the save path for one key and carries its attempt token to completion.
    Save {
        /// Stable application identity for the item being saved.
        key: Key,
        /// Identity binding the eventual completion to this exact request.
        attempt: SaveAttemptToken,
    },
    /// It is now safe (or explicitly forced) to destroy this window.
    Close(window::Id),
}

/// Why a save completion report was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveReportError {
    /// No guarded Save All operation is waiting for a result.
    NoSavePending,
    /// The report names a different item than the one the guard requested.
    WrongItem,
    /// The item matches, but the completion belongs to an invalidated request.
    StaleAttempt,
}

#[derive(Debug, Clone)]
struct Pending<Key> {
    window: window::Id,
    items: VecDeque<UnsavedItem<Key>>,
    awaiting_save: Option<SaveAttemptToken>,
}

/// Holds one window’s cooperative close request until it is safe to close.
///
/// Use a separate guard for each independently closeable window.
#[derive(Debug, Clone)]
pub struct CloseGuard<Key> {
    pending: Option<Pending<Key>>,
}

impl<Key> Default for CloseGuard<Key> {
    fn default() -> Self {
        Self { pending: None }
    }
}

impl<Key> CloseGuard<Key>
where
    Key: Clone + Eq,
{
    /// Create an idle guard.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reviews a close request against a complete dirty-item snapshot.
    ///
    /// Empty input closes immediately, non-empty input is retained and held, and duplicate requests are held while a review or save is pending.
    pub fn request(
        &mut self,
        request: CloseRequest,
        dirty_items: impl IntoIterator<Item = UnsavedItem<Key>>,
    ) -> CloseGuardAction<Key> {
        if self.pending.is_some() {
            return CloseGuardAction::Hold;
        }

        let items: VecDeque<_> = dirty_items.into_iter().collect();
        if items.is_empty() {
            return CloseGuardAction::Close(request.window);
        }

        self.pending = Some(Pending {
            window: request.window,
            items,
            awaiting_save: None,
        });
        CloseGuardAction::Hold
    }

    /// Applies a decision to the pending request.
    ///
    /// With no pending request this returns [`CloseGuardAction::Hold`]. While a save is running, another `SaveAll` also returns `Hold`; `Cancel` and `Discard` invalidate that attempt.
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: a pending close always holds a dirty item"
    )]
    pub fn decide(&mut self, decision: CloseDecision) -> CloseGuardAction<Key> {
        let Some(pending) = self.pending.as_mut() else {
            return CloseGuardAction::Hold;
        };

        match decision {
            CloseDecision::Cancel => {
                self.pending = None;
                CloseGuardAction::Hold
            }
            CloseDecision::Discard => {
                let window = pending.window;
                self.pending = None;
                CloseGuardAction::Close(window)
            }
            CloseDecision::SaveAll => {
                if pending.awaiting_save.is_some() {
                    CloseGuardAction::Hold
                } else {
                    let attempt = SaveAttemptToken::fresh();
                    pending.awaiting_save = Some(attempt.clone());
                    CloseGuardAction::Save {
                        key: pending
                            .items
                            .front()
                            .expect("a pending close always has a dirty item")
                            .key
                            .clone(),
                        attempt,
                    }
                }
            }
        }
    }

    /// Reports completion of the exact key and attempt requested by [`CloseGuardAction::Save`].
    ///
    /// Success advances sequentially and closes only after the final item. Failure or cancellation retains that item.
    ///
    /// # Errors
    ///
    /// Returns [`SaveReportError::NoSavePending`] when no save is awaiting completion, [`SaveReportError::WrongItem`] for another key, and [`SaveReportError::StaleAttempt`] for an invalidated token.
    pub fn save_finished(
        &mut self,
        key: &Key,
        attempt: &SaveAttemptToken,
        outcome: SaveOutcome,
    ) -> Result<CloseGuardAction<Key>, SaveReportError> {
        let Some(pending) = self.pending.as_mut() else {
            return Err(SaveReportError::NoSavePending);
        };
        let Some(expected_attempt) = pending.awaiting_save.as_ref() else {
            return Err(SaveReportError::NoSavePending);
        };
        if pending.items.front().map(|item| &item.key) != Some(key) {
            return Err(SaveReportError::WrongItem);
        }
        if expected_attempt != attempt {
            return Err(SaveReportError::StaleAttempt);
        }

        pending.awaiting_save = None;
        if outcome != SaveOutcome::Saved {
            return Ok(CloseGuardAction::Hold);
        }

        pending.items.pop_front();
        if let Some(next) = pending.items.front() {
            let attempt = SaveAttemptToken::fresh();
            pending.awaiting_save = Some(attempt.clone());
            return Ok(CloseGuardAction::Save {
                key: next.key.clone(),
                attempt,
            });
        }

        let window = pending.window;
        self.pending = None;
        Ok(CloseGuardAction::Close(window))
    }

    /// Clears pending review and returns an unconditional close action for a non-vetoable shutdown.
    pub fn force_close(&mut self, window: window::Id) -> CloseGuardAction<Key> {
        self.pending = None;
        CloseGuardAction::Close(window)
    }

    /// Whether a request is currently held.
    #[must_use]
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Whether the save path is currently running for the first pending item.
    #[must_use]
    pub fn is_saving(&self) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.awaiting_save.is_some())
    }

    /// Returns the held window while a close decision or save is pending.
    #[must_use]
    pub fn pending_window(&self) -> Option<window::Id> {
        self.pending.as_ref().map(|pending| pending.window)
    }

    /// Iterates items still awaiting a decision or successful save in application order.
    pub fn pending_items(&self) -> impl Iterator<Item = &UnsavedItem<Key>> {
        self.pending.iter().flat_map(|pending| pending.items.iter())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    #[derive(Debug)]
    struct Harness {
        windows: BTreeSet<window::Id>,
        dirty: BTreeMap<u8, (&'static str, bool)>,
        guard: CloseGuard<u8>,
        saves: Vec<u8>,
    }

    impl Harness {
        fn with_window(entries: &[(u8, &'static str, bool)]) -> (Self, window::Id) {
            let id = window::Id::unique();
            (
                Self {
                    windows: BTreeSet::from([id]),
                    dirty: entries
                        .iter()
                        .copied()
                        .map(|(k, n, d)| (k, (n, d)))
                        .collect(),
                    guard: CloseGuard::new(),
                    saves: Vec::new(),
                },
                id,
            )
        }

        fn real_close(&mut self, id: window::Id) -> CloseGuardAction<u8> {
            let event = Event::Window(window::Event::CloseRequested);
            let request = CloseRequest::from_event(&event, id)
                .expect("the real CloseRequested event must enter the guard");
            let dirty = self
                .dirty
                .iter()
                .filter(|(_, (_, is_dirty))| *is_dirty)
                .map(|(key, (name, _))| UnsavedItem::new(*key, *name))
                .collect::<Vec<_>>();
            self.guard.request(request, dirty)
        }

        fn apply(&mut self, action: CloseGuardAction<u8>) {
            match action {
                CloseGuardAction::Hold => {}
                CloseGuardAction::Close(id) => {
                    self.windows.remove(&id);
                }
                CloseGuardAction::Save { key, attempt } => {
                    self.saves.push(key);
                    self.dirty.get_mut(&key).expect("save key exists").1 = false;
                    let next = self
                        .guard
                        .save_finished(&key, &attempt, SaveOutcome::Saved)
                        .expect("save report matches requested key and attempt");
                    self.apply(next);
                }
            }
        }
    }

    #[test]
    fn cancel_keeps_dirty_items_intact_and_app_running() {
        let (mut app, id) = Harness::with_window(&[(1, "alpha.txt", true), (2, "beta.txt", true)]);
        let held = app.real_close(id);
        app.apply(held);
        let before = app.dirty.clone();

        let cancel = app.guard.decide(CloseDecision::Cancel);
        assert_eq!(cancel, CloseGuardAction::Hold);
        app.apply(cancel);

        assert_eq!(app.dirty, before, "Cancel must not mutate any dirty item");
        assert!(
            app.windows.contains(&id),
            "Cancel must leave the window and app running"
        );
        assert!(
            !app.guard.is_pending(),
            "Cancel dismisses this close request"
        );
        assert!(app.saves.is_empty(), "Cancel must not enter the save path");
    }
}
