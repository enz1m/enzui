//! Confirmation decisions and optional checkbox state.
//!
//! A result captures the decision, dismissal cause, and checkbox value together. `None` means no checkbox was offered; an unchecked checkbox yields `Some(false)`.

use crate::modal::DismissCause;

/// The user’s decision, independent of the surface’s dismissal cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfirmDecision {
    /// The user took the default action — the affirmative one.
    Confirmed,
    /// The user backed out, by any route.
    Cancelled,
}

impl ConfirmDecision {
    /// Maps [`DismissCause::DefaultAction`] to [`Self::Confirmed`] and every other cause to [`Self::Cancelled`].
    #[must_use]
    pub const fn from_cause(cause: DismissCause) -> Self {
        match cause {
            DismissCause::DefaultAction => Self::Confirmed,
            DismissCause::Escape
            | DismissCause::Scrim
            | DismissCause::Drag
            | DismissCause::CloseButton
            | DismissCause::CancelAction
            | DismissCause::Assistive => Self::Cancelled,
        }
    }

    /// Whether this is the affirmative decision.
    #[must_use]
    pub const fn is_confirmed(self) -> bool {
        matches!(self, Self::Confirmed)
    }
}

/// Configuration for an optional confirmation-dialog checkbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmCheckbox {
    /// Label shown beside the checkbox.
    pub label: String,
    /// What it starts as, each time the dialog is presented.
    pub initially_checked: bool,
    /// Whether the user may change the checkbox; disabled values are still displayed and reported.
    pub enabled: bool,
}

impl ConfirmCheckbox {
    /// A checkbox that starts unchecked and may be changed.
    #[must_use]
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            initially_checked: false,
            enabled: true,
        }
    }

    /// Sets whether the checkbox starts checked.
    #[must_use]
    pub fn initially_checked(mut self, checked: bool) -> Self {
        self.initially_checked = checked;
        self
    }

    /// Sets whether the user may change the checkbox.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// Live checkbox state for one dialog presentation.
///
/// It survives rerenders of that presentation and resets when a new presentation is created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmState {
    checkbox: Option<ConfirmCheckbox>,
    checked: bool,
}

impl ConfirmState {
    /// Begin a presentation of a dialog with `checkbox`.
    #[must_use]
    pub fn new(checkbox: Option<ConfirmCheckbox>) -> Self {
        let checked = checkbox.as_ref().is_some_and(|c| c.initially_checked);
        Self { checkbox, checked }
    }

    /// The checkbox specification, if this dialog has one.
    #[must_use]
    pub const fn checkbox(&self) -> Option<&ConfirmCheckbox> {
        self.checkbox.as_ref()
    }

    /// Returns whether the box is checked, or `false` when no checkbox is present.
    #[must_use]
    pub const fn is_checked(&self) -> bool {
        self.checked
    }

    /// Returns whether the presented checkbox exists and is enabled.
    #[must_use]
    pub fn is_interactive(&self) -> bool {
        self.checkbox.as_ref().is_some_and(|c| c.enabled)
    }

    /// Applies a user toggle and returns whether the value changed.
    ///
    /// Absent or disabled checkboxes reject the change and preserve their value.
    pub fn toggle(&mut self, checked: bool) -> bool {
        if !self.is_interactive() || self.checked == checked {
            return false;
        }
        self.checked = checked;
        true
    }

    /// Consumes this presentation and captures its decision, cause, and checkbox value.
    #[must_use]
    pub fn snapshot(self, cause: DismissCause) -> ConfirmDialogResult {
        ConfirmDialogResult {
            decision: ConfirmDecision::from_cause(cause),
            cause,
            checkbox_checked: self.checkbox.map(|_| self.checked),
        }
    }
}

/// Everything a confirm dialog reports, captured atomically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConfirmDialogResult {
    /// What the user chose.
    pub decision: ConfirmDecision,
    /// The dismissal route, retained even when several routes imply the same decision.
    pub cause: DismissCause,
    /// The checkbox's final value, or `None` if the dialog had no checkbox.
    pub checkbox_checked: Option<bool>,
}

impl ConfirmDialogResult {
    /// Whether the user took the affirmative action.
    #[must_use]
    pub const fn is_confirmed(self) -> bool {
        self.decision.is_confirmed()
    }

    /// Returns `true` only when the user confirmed and the offered checkbox ended checked.
    #[must_use]
    pub fn confirmed_and_checked(self) -> bool {
        self.is_confirmed() && self.checkbox_checked == Some(true)
    }
}
