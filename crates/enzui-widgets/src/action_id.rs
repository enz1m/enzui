//! Stable widget identifiers for controls that render canonical actions.
//!
//! Omit the prefix to preserve a canonical command ID, or supply a stable
//! instance prefix when multiple controls render the same command. Prefixes
//! and keys are concatenated with `-`, without escaping; choose them so their
//! combinations do not collide. Repeated actions can pass occurrence keys from
//! [`enzui_core::identity::KeyedIds`] to [`derive_action_id_keyed`].

use enzui_core::identity::derive_id;
use iced::advanced::widget::Id;

/// Derive an action control ID from a stable optional prefix.
/// `None` borrows the canonical static ID without allocation. `Some(prefix)`
/// allocates `prefix-action_id`. Equal inputs produce equal IDs across rebuilds.
#[must_use]
pub fn derive_action_id(id_prefix: Option<&str>, action_id: &'static str) -> Id {
    match id_prefix {
        Some(prefix) => derive_id(prefix, action_id),
        None => Id::new(action_id),
    }
}

/// Derive an ID from a caller-supplied occurrence key.
/// `None` owns a copy of the key; `Some(prefix)` allocates `prefix-key`.
/// This helper does not disambiguate or escape the key itself. Keys from
/// [`enzui_core::identity::KeyedIds`] already escape `#` and append occurrence
/// suffixes, including escaping on the first occurrence.
#[must_use]
pub fn derive_action_id_keyed(id_prefix: Option<&str>, disambiguated_key: &str) -> Id {
    match id_prefix {
        Some(prefix) => derive_id(prefix, disambiguated_key),
        None => Id::from(disambiguated_key.to_owned()),
    }
}
