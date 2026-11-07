//! Shared listbox group, option and status descriptors.
//!
//! [`crate::suggestion_list`], [`crate::autocomplete_field`] and
//! [`crate::command_palette`] publish these descriptors through their widget
//! trees. These helpers do not create focus stops or enforce a single selected
//! row. Positions and totals belong to the caller's result set, which can be
//! larger than its rendered window.

use enzui_core::semantics::{Role, Semantics, SetPosition};

/// Create a named ListBox descriptor; the caller publishes it on the group.
pub fn listbox(label: impl Into<String>) -> Semantics {
    Semantics::new(Role::ListBox).label(label.into())
}

/// Create a named option with explicit selected state and set position.
/// `index_1_based` and `total` describe the complete caller result set, including
/// rows outside a rendered window. Values are forwarded without validation;
/// the caller maintains valid positions and selection policy.
pub fn listbox_option(
    label: impl Into<String>,
    is_active: bool,
    index_1_based: usize,
    total: usize,
) -> Semantics {
    Semantics::new(Role::ListBoxOption)
        .label(label.into())
        .selected(is_active)
        .set_position(SetPosition {
            index: index_1_based,
            count: total,
        })
}

/// Create a named Status descriptor with polite live-region behavior.
/// Empty, loading and error notices remain separate from selectable options.
pub fn listbox_notice(label: impl Into<String>) -> Semantics {
    Semantics::new(Role::Status).label(label.into())
}
