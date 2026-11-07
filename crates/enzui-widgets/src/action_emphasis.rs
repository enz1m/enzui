//! Shared recommended-action and quiet-action button emphasis.
//!
//! Suggested maps to Accent, while Neutral maps to Secondary, a neutral
//! filled button that reads as a button on a tinted banner or a page. The enum defaults to
//! Neutral; individual widgets choose their own defaults. [`crate::banner`] and
//! [`crate::status_page`] use this vocabulary for their action buttons.

use crate::button::ButtonKind;
/// Choose recommended Accent treatment or neutral Secondary treatment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActionEmphasis {
    /// Use Secondary treatment; this is the enum default.
    #[default]
    Neutral,
    /// Use Accent treatment for a recommended next action.
    Suggested,
}

impl ActionEmphasis {
    /// Return Accent for Suggested or Secondary for Neutral.
    #[must_use]
    pub fn button_kind(self) -> ButtonKind {
        match self {
            ActionEmphasis::Suggested => ButtonKind::Accent,
            ActionEmphasis::Neutral => ButtonKind::Secondary,
        }
    }
}
