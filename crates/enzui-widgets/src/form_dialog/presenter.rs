//! [`FormPresenter`] — the lifecycle orchestrator — its outcome type
//! [`FormOutcome`], and the stable widget-id / focus helpers
//! ([`field_widget_id`], [`first_invalid_widget_id`], [`focus_first_invalid`]
//! and their scoped siblings).
//!
//! The presenter owns the [`FormSession`] and [`MultilineBridge`] and turns a
//! [`FormDialogEvent`] into a [`FormOutcome`] via [`FormPresenter::update`],
//! keeping the two stores in sync. The id helpers give a form field a stable
//! address so a rejected submit can focus the first invalid control.

use super::bridge::MultilineBridge;
use super::event::FormDialogEvent;
use enzui_core::form::{FieldId, FormModel, FormSession, FormSubmission};
use enzui_core::modal::DismissCause;

// ---------------------------------------------------------------------------
// FormPresenter — the lifecycle orchestrator
// ---------------------------------------------------------------------------

/// What happened when a [`FormPresenter`] processed an event.
///
/// Typed rather than a tuple, because the caller must be able to
/// pattern-match "the user successfully submitted" from "the user
/// cancelled" without interpreting a `DismissCause`.
///
/// There is no `Edited` variant. An edit changes the presenter's
/// internal state and triggers a redraw, but does not produce a
/// domain-level outcome the application needs to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormOutcome {
    /// The form validated and the session was consumed into a submission.
    ///
    /// This happens exactly once per presentation — the presenter
    /// drops the session on success, so a second submit is impossible.
    Submitted(FormSubmission),
    /// The dialog was dismissed without submitting, for this reason.
    ///
    /// Cancel, Escape, scrim — they all land here. The application
    /// receives the cause so it can distinguish "actively cancelled"
    /// from "backed out" if it needs to, but the drafts are gone
    /// regardless: a dismissed form is a declined form.
    Dismissed(DismissCause),
}

/// Owns the lifecycle of one form dialog presentation.
///
/// A caller creates a `FormPresenter` when a form is presented, hands events
/// to [`update`](Self::update), borrows the session for rendering,
/// and drops the presenter when the outcome has been handled.
///
/// # What it owns
///
/// - A [`FormSession`] — the keyed draft store.
/// - A [`MultilineBridge`] — the editor state for multiline fields.
///
/// Keeping these two values together avoids coordinating separate `Option`
/// fields by hand across the application's match arms.
///
/// # What it does not own
///
/// - The modal stack. Presenting and dismissing on the stack is the
///   application's concern; the presenter only reports outcomes.
/// - Message mapping. The presenter speaks [`FormDialogEvent`] and
///   [`FormOutcome`]; the caller wraps them into its own `Message`.
///
/// # Re-invoking closures
///
/// `FormPresenter` stores no closures. The `on_event` mapping lives
/// on [`enz_form_dialog`](crate::form_dialog::enz_form_dialog), which the caller rebuilds each frame.
/// This means it is safe under `Responsive` and any other wrapper
/// that re-invokes its closure — there is no one-shot resource to
/// exhaust.
pub struct FormPresenter {
    session: FormSession,
    bridge: MultilineBridge,
    // Rendering and rejected-submit focus share this namespace.
    field_id_scope: Option<String>,
    /// Set once the presenter has produced a terminal outcome — either a
    /// successful [`FormOutcome::Submitted`] or a
    /// [`FormOutcome::Dismissed`]. Every subsequent event returns `None`
    /// without mutating state.
    ///
    /// This tracks both terminal outcomes. Tracking only successful submission
    /// would leave a presenter live after `Resolved(CancelAction)`, allowing a
    /// later `SubmitRequested` to consume the drafts or a second `Resolved` to
    /// emit another dismissal.
    terminal: bool,
}

impl FormPresenter {
    /// Begin presenting `model`.
    ///
    /// The session and editors are initialised from the model exactly
    /// once. A rerender does not touch them; a reopen builds a new
    /// presenter.
    #[must_use]
    pub fn new(model: FormModel) -> Self {
        let session = FormSession::new(model);
        let bridge = MultilineBridge::new(&session);
        Self {
            session,
            bridge,
            field_id_scope: None,
            terminal: false,
        }
    }

    /// Present again a submission the application refused: the values come
    /// back as drafts and `id` shows `message` as its error. See
    /// [`FormSession::refused`]. Run [`scoped_focus_first_invalid`] (or the
    /// unscoped [`focus_first_invalid`]) after it so the keyboard lands in the
    /// field to correct.
    #[must_use]
    pub fn refused(
        model: FormModel,
        submission: enzui_core::form::FormSubmission,
        id: &enzui_core::form::FieldId,
        message: impl Into<String>,
    ) -> Self {
        let session = FormSession::refused(model, submission, id, message);
        let bridge = MultilineBridge::new(&session);
        Self {
            session,
            bridge,
            field_id_scope: None,
            terminal: false,
        }
    }

    /// Namespace this presentation's field widget ids by `scope`, giving them
    /// the form `enzui-form-field:{scope}:{field}`.
    ///
    /// Two forms on screen that share a field id otherwise mint the same
    /// widget id for it, and `focus(id)` lands on whichever the tree walks
    /// first. The presenter uses this namespace when a rejected submit focuses
    /// the first invalid field, and
    /// [`EnzFormDialog::presenter`](crate::form_dialog::EnzFormDialog::presenter)
    /// renders the fields under the same value.
    ///
    /// ```
    /// use enzui_core::form::{FieldId, FormField, FormModel};
    /// use enzui_widgets::form_dialog::{FormPresenter, scoped_field_widget_id};
    ///
    /// let model = FormModel::new(vec![FormField::text("name", "Name")]).expect("model");
    /// let presenter = FormPresenter::new(model).with_field_id_scope("host-editor");
    ///
    /// assert_eq!(
    ///     scoped_field_widget_id(presenter.field_id_scope(), &FieldId::new("name")),
    ///     iced::advanced::widget::Id::from("enzui-form-field:host-editor:name".to_owned()),
    /// );
    /// ```
    #[must_use]
    pub fn with_field_id_scope(mut self, scope: impl Into<String>) -> Self {
        self.field_id_scope = Some(scope.into());
        self
    }

    /// The field-id namespace this presentation renders and focuses under, if
    /// any. See [`Self::with_field_id_scope`].
    #[must_use]
    pub fn field_id_scope(&self) -> Option<&str> {
        self.field_id_scope.as_deref()
    }

    /// The live session, for rendering.
    ///
    /// Borrow, not move: the session survives across rerenders
    /// because nobody takes it.
    #[must_use]
    pub const fn session(&self) -> &FormSession {
        &self.session
    }

    /// The multiline editors, for rendering.
    #[must_use]
    pub const fn bridge(&self) -> &MultilineBridge {
        &self.bridge
    }

    /// Swap in a new model, preserving drafts for fields that survive.
    ///
    /// Delegates to [`FormSession::retarget`] and
    /// [`MultilineBridge::sync`] in one call, so the two stores
    /// cannot end up out of step.
    pub fn retarget(&mut self, model: FormModel) {
        self.session.retarget(model);
        self.bridge.sync(&self.session);
    }

    /// Process one [`FormDialogEvent`].
    ///
    /// Returns the lifecycle outcome (if any) and a `Task` for
    /// focus operations. The caller should run the task and, if an
    /// outcome was produced, act on it (dismiss the modal, consume
    /// the submission, drop the presenter).
    ///
    /// `scope` is the presentation's focus scope id — the same id the host's
    /// surface carries. It is consulted only by the arms that move focus
    /// (Tab/Shift-Tab and a rejected submit), and it is an
    /// id rather than a topmost bool because of when the operation runs:
    /// iced drains the entire message batch and rebuilds every tree before
    /// executing widget operations
    /// (`iced/winit/src/lib.rs`), so a bool sampled
    /// here describes a tree that may no longer exist. A scoped operation is
    /// resolved against the rebuilt tree instead, and only rendered
    /// presentations contribute their scope id — so a covered form declines
    /// structurally.
    ///
    /// `None` means "unscoped": correct for a caller with a single
    /// presentation and nothing to arbitrate against.
    ///
    /// Every other arm (edits, submission, dismissal) is unaffected by scope:
    /// a covered form still accepts programmatic edits from its owner, and
    /// every dismissal path must resolve regardless of what is stacked above
    /// it.
    ///
    /// # Submit is event-time, not view-time
    ///
    /// `SubmitRequested` validates and, on success, consumes the
    /// session into a [`FormSubmission`] here — at event time —
    /// not during view construction. The submit factory is therefore the
    /// application's own match arm on [`FormOutcome::Submitted`], which receives
    /// the submission by value.
    pub fn update<Message>(
        &mut self,
        event: FormDialogEvent,
        scope: Option<iced::advanced::widget::Id>,
    ) -> (Option<FormOutcome>, iced::Task<Message>)
    where
        Message: Send + 'static,
    {
        if self.terminal {
            // Terminal is terminal. Once the presenter has emitted a
            // Submitted or Dismissed, every subsequent event — an edit, a
            // stray focus move, a second SubmitRequested, a second
            // Resolved — is a no-op. The host owns dropping the presenter;
            // until it does, the presenter refuses to speak twice.
            return (None, iced::Task::none());
        }
        match event {
            FormDialogEvent::Edited(id, value) => {
                self.session.edit(&id, value);
                (None, iced::Task::none())
            }
            FormDialogEvent::EditorAction(id, action) => {
                self.bridge.apply(&mut self.session, &id, action);
                (None, iced::Task::none())
            }
            FormDialogEvent::RevealToggled(id) => {
                // The reveal lives in the session, so it is reset by
                // the same things that reset a draft -- a new presenter
                // (reopen) and `retarget` -- rather than by an application
                // remembering to clear a `bool`. No outcome: showing a
                // password is not a lifecycle event the host acts on, and
                // publishing one would tempt a host into reading the value.
                self.session.toggle_reveal(&id);
                (None, iced::Task::none())
            }
            FormDialogEvent::FocusNext | FormDialogEvent::FocusPrevious => {
                let operation = match scope {
                    Some(scope) => event.scoped_focus_operation::<Message>(scope),
                    // Unscoped: a lone presentation has nothing to arbitrate
                    // against, so it cycles focus wherever it is rendered.
                    None => event.focus_operation::<Message>(true),
                };
                let task = operation.map_or_else(iced::Task::none, iced::advanced::widget::operate);
                (None, task)
            }
            FormDialogEvent::SubmitRequested => {
                // `submit()` takes `self` and hands the session back
                // on failure, so a failed submit cannot discard drafts
                // and a success cannot be snapshotted twice. A failed
                // submit is not terminal: it leaves drafts and errors
                // intact so the user can fix and retry.
                let session =
                    std::mem::replace(&mut self.session, FormSession::new(FormModel::default()));
                match session.submit() {
                    Ok(submission) => {
                        self.terminal = true;
                        (Some(FormOutcome::Submitted(submission)), iced::Task::none())
                    }
                    Err(session) => {
                        // Handed back with drafts and errors intact.
                        // Focus the first problem.
                        let task = scoped_focus_first_invalid::<Message>(
                            self.field_id_scope.as_deref(),
                            &session,
                        )
                        .map(|operation| confine(operation, scope))
                        .map_or_else(iced::Task::none, iced::advanced::widget::operate);
                        self.session = session;
                        (None, task)
                    }
                }
            }
            FormDialogEvent::Resolved(cause) => {
                // Every exit route — cancel, Escape, scrim — collapses
                // into one outcome. The drafts die with the presenter,
                // and the presenter itself goes terminal so a stray
                // later SubmitRequested or second Resolved cannot
                // produce another outcome.
                self.terminal = true;
                (Some(FormOutcome::Dismissed(cause)), iced::Task::none())
            }
        }
    }
}

// A covered presentation is not rendered, so its scope id is absent from the
// rebuilt tree and the operation matches nothing instead of reaching into what
// covers it; `modal_host::scoped_focus_operation` scopes Tab the same way.
fn confine<Message: 'static>(
    operation: Box<dyn iced::advanced::widget::Operation<Message>>,
    scope: Option<iced::advanced::widget::Id>,
) -> Box<dyn iced::advanced::widget::Operation<Message>> {
    match scope {
        Some(scope) => Box::new(iced::advanced::widget::operation::scope(scope, operation)),
        None => operation,
    }
}

impl std::fmt::Debug for FormPresenter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FormPresenter")
            .field("session", &self.session)
            .field("bridge", &self.bridge)
            .field("field_id_scope", &self.field_id_scope)
            .field("terminal", &self.terminal)
            .finish()
    }
}

/// The widget id a form field's control is rendered with.
///
/// Deterministic and derived from the field id, so a caller can focus a field
/// it knows the id of without the dialog having to hand out ids at render
/// time. Namespaced so it cannot collide with an application's own ids.
///
/// This is what makes [`focus_first_invalid`] possible: focusing "the first
/// problem" needs a *stable* address for that field, and a form is rebuilt on
/// every rerender, so the address has to be a function of the model rather
/// than something captured during a render.
#[must_use]
pub fn field_widget_id(field: &FieldId) -> iced::advanced::widget::Id {
    scoped_field_widget_id(None, field)
}

/// The widget id a form field's control is rendered with, namespaced by
/// a per-presentation scope.
///
/// `scope == None` reproduces [`field_widget_id`] exactly
/// (`enzui-form-field:{field}`), which is correct for a single form on screen.
/// `scope == Some(s)` produces `enzui-form-field:{s}:{field}`, so two forms
/// presented at once cannot mint the same widget id for a field they happen to
/// share by id. Without this, `focus(id)` on a rejected submit lands on the
/// first matching widget in the tree — which may be the other dialog's field.
///
/// Each control is rendered with this id, and
/// [`scoped_first_invalid_widget_id`] computes the focus target with the same
/// scope, so the id the dialog draws and the id it focuses always agree.
#[must_use]
pub fn scoped_field_widget_id(scope: Option<&str>, field: &FieldId) -> iced::advanced::widget::Id {
    match scope {
        Some(scope) => {
            iced::advanced::widget::Id::from(format!("enzui-form-field:{scope}:{}", field.as_str()))
        }
        None => iced::advanced::widget::Id::from(format!("enzui-form-field:{}", field.as_str())),
    }
}

/// Focus the first field that failed validation.
///
/// Returns `None` when the session is clean — there is no problem to land on.
/// Pair with a rejected submit: a form that reports errors but leaves focus on
/// the button the user just pressed makes them hunt for the field.
///
/// Only *enabled* invalid fields are candidates, and they are considered in
/// model order, because [`FormSession::first_invalid`] decides that and this
/// defers to it rather than re-deriving the rule.
/// The widget id a rejected submit should move focus to.
///
/// Split out from [`focus_first_invalid`] so the *target* is observable: an
/// `Operation` is opaque, so a test that only had the operation could not
/// check it against the id the dialog actually rendered, and the two are
/// written in different places. Both functions derive the id here, so they
/// cannot disagree.
#[must_use]
pub fn first_invalid_widget_id(session: &FormSession) -> Option<iced::advanced::widget::Id> {
    scoped_first_invalid_widget_id(None, session)
}

/// The widget id a rejected submit should move focus to, namespaced by
/// `scope`.
///
/// `scope == None` reproduces [`first_invalid_widget_id`]. Pass the same scope
/// the dialog rendered its fields with (see [`scoped_field_widget_id`]) so the
/// focus target matches a field in this presentation, not an identically-named
/// field in another one.
#[must_use]
pub fn scoped_first_invalid_widget_id(
    scope: Option<&str>,
    session: &FormSession,
) -> Option<iced::advanced::widget::Id> {
    session
        .first_invalid()
        .map(|field| scoped_field_widget_id(scope, field))
}

/// Boxed, and an `Operation` rather than a `Task`, to match
/// [`crate::modal_host::focus_wrap_operation`]: the widget layer says *what*
/// should be focused and leaves running it to the application, which is what
/// makes it assertable without a runtime.
#[must_use]
pub fn focus_first_invalid<T>(
    session: &FormSession,
) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
where
    T: Send + 'static,
{
    scoped_first_invalid_widget_id(None, session).map(|id| {
        Box::new(iced::advanced::widget::operation::focusable::focus(id))
            as Box<dyn iced::advanced::widget::Operation<T>>
    })
}

/// Focus the first invalid field, namespaced by `scope`.
///
/// The scoped sibling of [`focus_first_invalid`]. Pass the same scope the
/// dialog rendered with (`field_id_scope`), so a rejected submit in a stacked
/// presentation focuses this form's first problem, not a same-named field in a
/// form beneath it.
#[must_use]
pub fn scoped_focus_first_invalid<T>(
    scope: Option<&str>,
    session: &FormSession,
) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
where
    T: Send + 'static,
{
    scoped_first_invalid_widget_id(scope, session).map(|id| {
        Box::new(iced::advanced::widget::operation::focusable::focus(id))
            as Box<dyn iced::advanced::widget::Operation<T>>
    })
}
