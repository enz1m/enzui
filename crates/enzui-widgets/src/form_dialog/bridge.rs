//! [`MultilineBridge`] — the one crossing point between a multiline field's
//! [`Content`] (caret, selection, undo) and the [`FormSession`] draft.
//!
//! The whole reason this type exists is behavioural: every editing
//! [`Action`] must write through to the session in the same step it is
//! applied ([`MultilineBridge::apply`]), so the two stores can never disagree.
//! [`MultilineBridge::sync`] and [`MultilineBridge::content`] keep the editor
//! set aligned with the session's multiline fields.

use enzui_core::form::{FieldId, FieldKind, FieldValue, FormField, FormSession};
use iced::widget::text_editor::{Action, Content};
use std::collections::{BTreeMap, BTreeSet};

/// The editor state for a form's multiline fields.
///
/// Held by the presentation alongside its [`FormSession`], for exactly the
/// same reason: initialised once, preserved across rerenders, reset on
/// reopen, isolated between nested modals.
///
/// # Why this type exists at all
///
/// It would be simpler to let the application keep a bare `Content` per
/// multiline field. It would also be wrong: nothing would then oblige the
/// write-through to the session, allowing displayed and submitted strings to
/// diverge without either store exposing it alone. Making the
/// bridge the *only* way to apply an action means the two stores cannot
/// disagree, because there is no code path that updates one without the other.
#[derive(Default)]
pub struct MultilineBridge {
    editors: BTreeMap<FieldId, Content>,
}

impl std::fmt::Debug for MultilineBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `Content` is not Debug. Report shape, which is what a failing test
        // actually wants to know.
        f.debug_struct("MultilineBridge")
            .field("fields", &self.editors.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl MultilineBridge {
    /// Build the editors a session's multiline fields need, seeded from its
    /// current drafts.
    ///
    /// Seeded from the *session*, not from the model's initial values, so a
    /// bridge rebuilt after a [`retarget`](FormSession::retarget) shows what
    /// the user has actually typed rather than reverting to the defaults.
    #[must_use]
    pub fn new(session: &FormSession) -> Self {
        let mut bridge = Self::default();
        bridge.sync(session);
        bridge
    }

    /// Create editors for multiline fields that gained one, and drop editors
    /// for fields that are gone.
    ///
    /// The mirror of [`FormSession::retarget`]'s draft pruning: an editor for
    /// a field no longer in the model would hold text nothing can submit and
    /// nobody can see, and would grow without bound across retargets.
    ///
    /// Existing editors are left alone, because recreating one would destroy
    /// the caret and undo history of a field the user may be mid-edit in.
    pub fn sync(&mut self, session: &FormSession) {
        // Collected into a set, not a list: the `retain` below asks "is this
        // editor still wanted?" once per editor, and over a list that is
        // O(editors x multiline fields). Same shape as the quadratic scaling
        // defect that occurs in the render path when a membership question is
        // answered by scanning.
        let wanted: Vec<&FormField> = session
            .model()
            .fields()
            .iter()
            .filter(|f| matches!(f.kind, FieldKind::Multiline { .. }))
            .collect();
        let wanted_ids: BTreeSet<&FieldId> = wanted.iter().map(|f| &f.id).collect();

        self.editors.retain(|id, _| wanted_ids.contains(id));

        for field in wanted {
            if !self.editors.contains_key(&field.id) {
                let seed = session
                    .value(&field.id)
                    .and_then(FieldValue::as_text)
                    .unwrap_or_default();
                self.editors
                    .insert(field.id.clone(), Content::with_text(seed));
            }
        }
    }

    /// The editor for `id`, if it has one.
    #[must_use]
    pub fn content(&self, id: &FieldId) -> Option<&Content> {
        self.editors.get(id)
    }

    /// How many editors are held. Exposed so "bounded to active fields" is
    /// assertable rather than merely stated.
    #[must_use]
    pub fn len(&self) -> usize {
        self.editors.len()
    }

    /// Whether no editors are held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.editors.is_empty()
    }

    /// Apply an editor action, writing edits through to the session.
    ///
    /// This is the single crossing of the seam. Returns whether the session's
    /// draft changed.
    ///
    /// A non-editing action (a caret move, a selection, a scroll) is applied
    /// to the editor and not written through: the value did not change,
    /// and writing anyway would clear the field's error every time the user
    /// pressed an arrow key.
    ///
    /// An edit to a field the session refuses — read-only, disabled, absent —
    /// is not applied to the editor either. Letting the editor accept text the
    /// session rejects is precisely the divergence this type exists to
    /// prevent: the user would watch themselves type into a read-only field
    /// and then watch the form submit without it.
    pub fn apply(&mut self, session: &mut FormSession, id: &FieldId, action: Action) -> bool {
        let Some(content) = self.editors.get_mut(id) else {
            return false;
        };

        if !action.is_edit() {
            content.perform(action);
            return false;
        }

        // Ask the session first. An edit it will not accept must not reach
        // the editor, or the two stores diverge on screen.
        if !session
            .model()
            .field(id)
            .is_some_and(enzui_core::form::FormField::is_editable)
        {
            return false;
        }

        content.perform(action);
        session.edit(id, FieldValue::Text(content.text()))
    }
}
