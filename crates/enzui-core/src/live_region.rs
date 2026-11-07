//! Named live-region policies for advisory, urgent, and silent accessibility updates.

use crate::semantics::{Live, Role, Semantics};

/// A named role and politeness policy for a live-region announcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Announcement {
    /// Announces advisory status politely at the next convenient pause.
    Status,
    /// Announces an urgent, non-modal alert assertively.
    Alert,
    /// Publishes a navigable status region that does not announce automatically.
    Silent,
}

impl Announcement {
    /// The [`Role`] this term publishes.
    #[must_use]
    pub fn role(self) -> Role {
        match self {
            Announcement::Status | Announcement::Silent => Role::Status,
            Announcement::Alert => Role::Alert,
        }
    }

    /// Returns the live-region politeness for this announcement policy.
    #[must_use]
    pub fn politeness(self) -> Live {
        match self {
            Announcement::Status => Live::Polite,
            Announcement::Alert => Live::Assertive,
            Announcement::Silent => Live::Off,
        }
    }

    /// Builds live-region semantics using a non-blank message as the accessible name.
    ///
    /// Blank messages leave the accessible name unset.
    pub fn region(self, message: impl Into<String>) -> Semantics {
        let message: String = message.into();
        let semantics = Semantics::new(self.role()).live(Some(self.politeness()));
        if message.trim().is_empty() {
            semantics
        } else {
            semantics.label(message)
        }
    }
}
