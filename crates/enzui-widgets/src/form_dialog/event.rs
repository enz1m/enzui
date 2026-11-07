//! [`FormDialogEvent`] — the vocabulary a presented form dialog reports to its
//! host, plus the focus-operation mapping.
//!
//! This is a real behavioural seam, not a bag of variants:
//! [`FormDialogEvent::from_host`] funnels a `ModalHostEvent` into the dialog's
//! own vocabulary, and [`FormDialogEvent::focus_operation`] /
//! [`FormDialogEvent::scoped_focus_operation`] decide which focus operation an
//! event calls for (declining a covered scope).

use enzui_core::form::{FieldId, FieldValue};
use enzui_core::modal::DismissCause;
use iced::widget::text_editor::Action;

/// What a presented form dialog reports to its host.
///
/// # One exit, not two
///
/// Same funnel as [`ConfirmDialogEvent`](crate::confirm_dialog::ConfirmDialogEvent):
/// a dialog can end by button, by Escape or by a scrim press, and only
/// the first originates inside the dialog. Routing all three through one
/// vocabulary keeps the application from ending the same dialog in two
/// places and forgetting the third; a forgotten route closes the
/// presentation with the [`FormSession`](enzui_core::form::FormSession)
/// never submitted, so everything typed is silently dropped. A form has
/// more to lose than a confirm dialog does — a confirm drops one
/// checkbox; a form drops every field.
///
/// # Alpha API evolution
///
/// This public enum is part of the framework's alpha API and is not
/// `#[non_exhaustive]`. Adding a variant such as
/// [`RevealToggled`](Self::RevealToggled) is an accepted source break
/// for downstream exhaustive matches; applications should normally pass
/// events to
/// [`FormPresenter::update`](crate::form_dialog::FormPresenter::update)
/// instead of duplicating its match. The dedicated `RevealToggled`
/// variant is intentional: it carries only a field id, whereas reusing
/// [`Edited`](Self::Edited) would put the secure value into a reveal
/// message. Secure edits use [`FieldValue::Secure`], whose formatting is
/// redacted and whose clones share wipe-on-last-drop custody.
#[derive(Debug, Clone, PartialEq)]
pub enum FormDialogEvent {
    /// The user changed a field. Apply with [`FormSession::edit`](enzui_core::form::FormSession::edit).
    ///
    /// Reported rather than applied, because the session lives in the
    /// presentation the application owns — the same controlled shape as every
    /// other widget here.
    Edited(FieldId, FieldValue),
    /// A multiline field received an editor action.
    ///
    /// Carried separately from [`Edited`](Self::Edited) because it must reach
    /// the `Content`, not just the draft — see [`MultilineBridge`](crate::form_dialog::MultilineBridge). Apply
    /// with [`MultilineBridge::apply`](crate::form_dialog::MultilineBridge::apply), never by hand.
    EditorAction(FieldId, Action),
    /// The user pressed a secure field's reveal toggle.
    ///
    /// Carries the field id only: revealing is a presentation change, so
    /// the masked value never crosses the application boundary in a *reveal*
    /// message. [`Edited`](Self::Edited) necessarily carries the text the user
    /// typed, but secure edits carry it as redacting, shared
    /// [`FieldValue::Secure`] custody. Showing a secret therefore adds no second
    /// value-bearing route. Apply with
    /// [`FormSession::toggle_reveal`](enzui_core::form::FormSession::toggle_reveal),
    /// which the presenter does.
    RevealToggled(FieldId),
    /// The user asked to submit (the affirmative button, or Enter in a field
    /// where Enter submits).
    ///
    /// A *request*, not a result: whether it succeeds is
    /// [`FormSession::submit`](enzui_core::form::FormSession::submit)'s answer, and the dialog stays open on failure
    /// so the errors can be shown.
    SubmitRequested,
    /// The dialog should end, for this reason.
    ///
    /// Every route — cancel button, Escape, scrim — arrives here.
    Resolved(DismissCause),
    /// Tab inside the dialog: move focus onward, wrapping at the end.
    FocusNext,
    /// Shift+Tab inside the dialog.
    FocusPrevious,
}

impl FormDialogEvent {
    /// Funnel a [`ModalHostEvent`](crate::modal_host::ModalHostEvent) into the
    /// dialog's own vocabulary.
    #[must_use]
    pub const fn from_host(event: crate::modal_host::ModalHostEvent) -> Self {
        use crate::modal_host::ModalHostEvent;

        match event {
            ModalHostEvent::Dismissed(cause) => Self::Resolved(cause),
            ModalHostEvent::FocusNext => Self::FocusNext,
            ModalHostEvent::FocusPrevious => Self::FocusPrevious,
        }
    }

    /// The focus operation this event calls for, if this scope is topmost.
    ///
    /// A covered scope — one that another modal is stacked above — must
    /// decline: cycling focus inside a dialog the user cannot see traps
    /// keyboard navigation in an invisible surface. Pass `is_topmost`
    /// from [`enzui_core::modal::ModalStack::is_topmost`] so the
    /// predicate that exists for exactly this purpose is consulted rather
    /// than re-derived.
    ///
    /// Returns `None` when:
    /// - the event is not a focus move (edit, editor action, reveal toggle,
    ///   submit, resolved dismissal), or
    /// - `is_topmost` is `false` (a covered scope declines).
    ///
    /// Delegates to [`crate::modal_host::focus_wrap_operation`] rather than
    /// re-deriving the mapping: two places deciding the same thing is a
    /// mutation nothing can catch.
    #[must_use]
    pub fn focus_operation<T>(
        &self,
        is_topmost: bool,
    ) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
    where
        T: Send + 'static,
    {
        if !is_topmost {
            return None;
        }
        use crate::modal_host::ModalHostEvent;

        let host_event = match self {
            Self::FocusNext => ModalHostEvent::FocusNext,
            Self::FocusPrevious => ModalHostEvent::FocusPrevious,
            Self::Edited(..)
            | Self::EditorAction(..)
            | Self::RevealToggled(_)
            | Self::SubmitRequested
            | Self::Resolved(_) => return None,
        };
        crate::modal_host::focus_wrap_operation(host_event)
    }

    /// The focus operation this event calls for, scoped to the
    /// presentation that must receive it.
    ///
    /// Prefer this over [`focus_operation`](Self::focus_operation). The bool
    /// that method takes is sampled while the application handles the
    /// message, but iced does not run the operation then — it drains the
    /// whole message batch and rebuilds every tree first
    /// (`iced/winit/src/lib.rs`). A scope id is
    /// resolved against the rebuilt tree at traversal time instead, so a
    /// presentation that has since been covered or dismissed declines
    /// structurally rather than by a stale predicate.
    ///
    /// Returns `None` for events that are not focus moves (edit, editor
    /// action, reveal toggle, submit, or resolved dismissal).
    #[must_use]
    pub fn scoped_focus_operation<T>(
        &self,
        scope: iced::advanced::widget::Id,
    ) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
    where
        T: Send + 'static,
    {
        use crate::modal_host::ModalHostEvent;

        let host_event = match self {
            Self::FocusNext => ModalHostEvent::FocusNext,
            Self::FocusPrevious => ModalHostEvent::FocusPrevious,
            Self::Edited(..)
            | Self::EditorAction(..)
            | Self::RevealToggled(_)
            | Self::SubmitRequested
            | Self::Resolved(_) => return None,
        };
        crate::modal_host::scoped_focus_operation(host_event, scope)
    }
}
