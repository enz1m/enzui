//! `sheet`: event.
//!
//! Owns the host-facing sheet event vocabulary:
//! [`SheetEvent`] (the enum a presented sheet reports back,
//! plus the helpers that translate a `ModalHostEvent` into
//! it) and [`drag_dismiss_cause`] (which drag resolutions
//! end the sheet, and with which cause).

use super::*;

/// What a presented sheet reports to its host.
///
/// Every way a sheet can end funnels to one
/// [`Dismissed`](SheetEvent::Dismissed) case carrying the
/// reason, so the application has exactly one result path
/// to make exactly-once — the same shape the confirm/form
/// dialogs use
/// ([`ConfirmDialogEvent`](crate::confirm_dialog::ConfirmDialogEvent)).
/// Focus-cycle requests keep their own variants so
/// dismissal and focus stay on separate axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SheetEvent {
    /// The sheet should end, for this reason.
    ///
    /// A [`DismissCause`], not a bool: a drag-away means *cancelled* while a
    /// button means *confirmed*, and a caller reconstructing that from a
    /// bare `closed: bool` will eventually read it out of order.
    Dismissed(DismissCause),
    /// Tab inside the sheet: move focus onward, wrapping
    /// at the end.
    ///
    /// Carried through rather than swallowed. Focus
    /// containment is the host's job and is a different
    /// axis from dismissal, so the caller processes this
    /// as a focus request rather than a close.
    FocusNext,
    /// Shift+Tab inside the sheet.
    FocusPrevious,
}

impl SheetEvent {
    /// Funnel a [`ModalHostEvent`](crate::modal_host::ModalHostEvent) into the
    /// sheet's own vocabulary.
    ///
    /// This is what makes a focus-cycle request do focus
    /// cycling rather than dismissal: a
    /// `FocusNext`/`FocusPrevious` maps to a focus event,
    /// and only an actual `Dismissed(cause)` maps to
    /// `Dismissed`.
    #[must_use]
    pub const fn from_host(event: crate::modal_host::ModalHostEvent) -> Self {
        use crate::modal_host::ModalHostEvent;

        match event {
            ModalHostEvent::Dismissed(cause) => Self::Dismissed(cause),
            ModalHostEvent::FocusNext => Self::FocusNext,
            ModalHostEvent::FocusPrevious => Self::FocusPrevious,
        }
    }

    /// The focus operation this event calls for, if this scope is topmost.
    ///
    /// A covered scope — one that another modal is stacked
    /// above — must decline: cycling focus inside a dialog
    /// the user cannot see traps keyboard navigation in an
    /// invisible surface. Callers pass `is_topmost` from
    /// [`enzui_core::modal::ModalStack::is_topmost`]
    /// (the shared coverage predicate) rather than
    /// computing coverage locally.
    ///
    /// Returns `None` when:
    /// - the event is a dismissal (ending a sheet is not
    ///   a focus move), or
    /// - `is_topmost` is `false` (a covered scope
    ///   declines).
    ///
    /// The `FocusNext`/`FocusPrevious` → iced-operation
    /// mapping lives in
    /// [`crate::modal_host::focus_wrap_operation`] and is
    /// called from here so this entry point and the direct
    /// modal-host callers agree by construction.
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
            Self::Dismissed(_) => return None,
        };
        crate::modal_host::focus_wrap_operation(host_event)
    }

    /// The focus operation this event calls for, scoped to
    /// the presentation that must receive it.
    ///
    /// Prefer this over
    /// [`focus_operation`](Self::focus_operation). The bool
    /// that method takes is sampled while the application
    /// handles the message, but iced does not run the
    /// operation then — it drains the whole message batch
    /// and rebuilds every tree first
    /// (`iced/winit/src/lib.rs`). A
    /// scope id is resolved against the rebuilt tree at
    /// traversal time instead, so a presentation that has
    /// since been covered or dismissed declines structurally
    /// rather than by a stale predicate.
    ///
    /// Returns `None` for a dismissal, which is not a
    /// focus move.
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
            Self::Dismissed(_) => return None,
        };
        crate::modal_host::scoped_focus_operation(host_event, scope)
    }
}

/// The [`DismissCause`] a resolved drag implies, if it means the sheet ends.
///
/// A drag resolves to [`Settle`](DragResolution::Settle) (step to another
/// detent), [`SpringBack`](DragResolution::SpringBack) (stay), or
/// [`Dismiss`](DragResolution::Dismiss). Only the last ends the sheet, and it
/// ends it for [`DismissCause::Drag`] — a spatial ambient exit governed by
/// the outside axis, so an insistent sheet that refuses the scrim also
/// refuses the swipe.
///
/// Returning `Option` rather than a bare cause is what
/// routes the drag through the same
/// [`ModalStack`](enzui_core::modal::ModalStack)
/// dismissal call as Escape, the scrim, the close button
/// and an AT request — one exactly-once path for every
/// dismissal cause.
#[must_use]
pub const fn drag_dismiss_cause(resolution: DragResolution) -> Option<DismissCause> {
    match resolution {
        DragResolution::Dismiss => Some(DismissCause::Drag),
        DragResolution::Settle(_) | DragResolution::SpringBack => None,
    }
}
