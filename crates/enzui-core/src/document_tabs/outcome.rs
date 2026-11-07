//! Outcomes for document-tab selection, focus, and reconciliation.

use super::*;
use iced_core::keyboard::{self, key};

/// Outcome of selecting a document key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectOutcome {
    /// The selection moved to the requested key.
    Changed,
    /// The key was already selected; no activation was recorded.
    AlreadySelected,
    /// The key is absent and selection is unchanged.
    NotFound,
    /// The key is disabled and selection is unchanged.
    Disabled,
}

impl SelectOutcome {
    /// Whether the selection actually moved.
    #[must_use]
    pub fn changed(self) -> bool {
        matches!(self, Self::Changed)
    }
}

/// A movement of the roving focus key in visual order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FocusMove {
    /// One tab earlier in visual order. Clamps at the first tab; never wraps.
    Previous,
    /// One tab later in visual order. Clamps at the last tab; never wraps.
    Next,
    /// Moves to the first focusable tab in visual order.
    First,
    /// Moves to the last focusable tab in visual order.
    Last,
}

impl FocusMove {
    /// Maps an unmodified or Shift-modified key to a movement, returning `None` for keys
    /// outside the strip’s axis.
    ///
    /// Horizontal arrows mirror under RTL; vertical arrows do not. Control, Alt, and Super
    /// modifiers are declined.
    #[must_use]
    pub fn for_key(
        key: &keyboard::Key,
        modifiers: keyboard::Modifiers,
        orientation: Orientation,
        rtl: bool,
    ) -> Option<Self> {
        if modifiers.control() || modifiers.alt() || modifiers.logo() {
            return None;
        }
        let keyboard::Key::Named(named) = key else {
            return None;
        };
        let (back, forward) = if rtl {
            (Self::Next, Self::Previous)
        } else {
            (Self::Previous, Self::Next)
        };
        Some(match (named, orientation) {
            (key::Named::ArrowLeft, Orientation::Horizontal) => back,
            (key::Named::ArrowRight, Orientation::Horizontal) => forward,
            (key::Named::ArrowUp, Orientation::Vertical) => Self::Previous,
            (key::Named::ArrowDown, Orientation::Vertical) => Self::Next,
            (key::Named::Home, _) => Self::First,
            (key::Named::End, _) => Self::Last,
            _ => return None,
        })
    }
}

/// Outcome of a focus request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusOutcome {
    /// The cursor moved, and now sits on this key.
    Moved(DocumentKey),
    /// Focus remains on the carried key, including when movement clamps at an end.
    Unchanged(DocumentKey),
    /// No tab is focusable, and the cursor is cleared.
    Empty,
    /// The requested key is absent and focus is unchanged.
    NotFound,
    /// The requested key is disabled and focus is unchanged.
    Disabled,
}

impl FocusOutcome {
    /// Whether the cursor actually moved.
    #[must_use]
    pub fn moved(&self) -> bool {
        matches!(self, Self::Moved(_))
    }

    /// The key the cursor ended up on, for the two outcomes that have one.
    #[must_use]
    pub fn key(&self) -> Option<&DocumentKey> {
        match self {
            Self::Moved(k) | Self::Unchanged(k) => Some(k),
            Self::Empty | Self::NotFound | Self::Disabled => None,
        }
    }
}

// Distinguishes a surviving target from a promoted neighbor so callers can gate side effects.
pub(crate) enum RepairOutcome {
    Survived(DocumentKey),
    Promoted(DocumentKey),
    None,
}

impl RepairOutcome {
    pub(crate) fn key(self) -> Option<DocumentKey> {
        match self {
            Self::Survived(k) | Self::Promoted(k) => Some(k),
            Self::None => None,
        }
    }
}
