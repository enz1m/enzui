//! Keyed document-tab requests.

use super::*;

/// Identify the gesture that requested a tab close.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CloseSource {
    /// The inline close × affordance was activated — by pointer, or by a
    /// routed AccessKit `Click` on the tab's
    /// [`close_id`](crate::document_tab::EnzDocumentTab::close_id).
    CloseButton,
    /// Middle-click on the tab body (the universal browser gesture), anchored
    /// to its press.
    MiddleClick,
    /// A keyboard request — <kbd>Delete</kbd> on the cursored tab. Named
    /// separately because a keyboard close has no pointer position to anchor a
    /// confirmation popover to.
    Keyboard,
}

/// Request a document-tab action using stable document identity.
///
/// The application owns model updates. [`NewRequested`](Self::NewRequested) has
/// no key because the document has not been created yet.
#[derive(Debug, Clone, PartialEq)]
pub enum TabEvent {
    /// Activate (select) this tab — left click, or <kbd>Enter</kbd>/<kbd>Space</kbd>
    /// on the cursored tab.
    Activated(DocumentKey),
    /// Close this tab, and how it was asked for.
    ///
    /// A request, not a command: the application owns the item list, so it
    /// decides whether to prompt, veto or reconcile. The widget never removes a
    /// tab by itself.
    CloseRequested(DocumentKey, CloseSource),
    /// Open this tab's context menu, with the typed invocation that
    /// decides the anchor — [`InvocationKind::Pointer`] carries the event
    /// position, [`InvocationKind::Keyboard`] the focused tab's bounds.
    ContextRequested(DocumentKey, InvocationKind),
    /// The roving cursor moved to this key (an arrow, <kbd>Home</kbd> or
    /// <kbd>End</kbd>). The application stores it back on the model.
    ///
    /// Separate from [`Activated`](Self::Activated) because automatic and
    /// manual activation are explicit: under manual
    /// activation an arrow moves the cursor without opening a document.
    FocusMoved(DocumentKey),
    /// The new-tab affordance was activated. Carries no key — the document it
    /// creates does not exist yet, so there is nothing to name.
    NewRequested,
}

impl TabEvent {
    /// Return the addressed key, or `None` for [`NewRequested`](Self::NewRequested).
    #[must_use]
    pub fn key(&self) -> Option<&DocumentKey> {
        match self {
            Self::Activated(k)
            | Self::CloseRequested(k, _)
            | Self::ContextRequested(k, _)
            | Self::FocusMoved(k) => Some(k),
            Self::NewRequested => None,
        }
    }
}
