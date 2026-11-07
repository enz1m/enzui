//! Shared role and capability classifications for semantics collection, mapping, and routing.

use super::{Role, Semantics};

impl Semantics {
    /// Returns whether the role or an explicit capability makes this a focus target.
    pub fn is_interactive(&self) -> bool {
        self.supports_focus
            || matches!(
                self.role,
                Role::Button
                | Role::Link
                | Role::CheckBox
                | Role::Switch
                | Role::RadioButton
                | Role::Slider
                | Role::SpinButton
                | Role::TextInput
                | Role::PasswordInput
                | Role::ListItem
                | Role::Tab
                | Role::TreeItem
                | Role::Disclosure
                | Role::MenuItem
                | Role::MenuItemCheckBox
                | Role::MenuItemRadio
                | Role::ComboBox
                | Role::EditableComboBox
                | Role::ListBoxOption
                // Splitters are keyboard focus stops with routed resize actions.
                | Role::Splitter
            )
    }

    /// Returns whether the node is a native heading or carries a promoted heading level.
    pub fn is_heading(&self) -> bool {
        self.role == Role::Heading || self.heading_level.is_some()
    }

    /// Returns whether the enabled node would advertise at least one assistive-technology action.
    ///
    /// This capability set must match AccessKit action emission so anonymous actionable nodes can be diagnosed accurately.
    #[must_use]
    pub fn advertises_action(&self) -> bool {
        if self.disabled {
            return false;
        }
        self.is_interactive()
            || self.supports_default_action
            || self.supports_context_menu
            || (self.expandable_action && self.expanded.is_some())
            || (self.adjustable && !self.read_only && self.numeric.is_some())
            || (self.editable
                && !self.read_only
                && matches!(
                    self.role,
                    Role::TextInput
                        | Role::PasswordInput
                        | Role::EditableComboBox
                        | Role::SpinButton
                ))
    }

    /// Returns whether the role is a container from which AccessKit resolves `size_of_set`.
    pub fn is_set_container(&self) -> bool {
        matches!(
            self.role,
            Role::Group
                | Role::ListBox
                | Role::Tree
                | Role::Menu
                | Role::MenuBar
                | Role::TabList
                | Role::RadioGroup
                | Role::Grid
        )
    }

    /// Returns whether this node may suppress a descendant label that exactly duplicates its own accessible name or description.
    ///
    /// This includes interactive controls and structural roles that render their own name inline; grouping containers and separately captioned content are excluded.
    pub fn owns_duplicable_name(&self) -> bool {
        self.is_interactive()
            || matches!(
                self.role,
                Role::ColumnHeader
                    | Role::GridCell
                    | Role::DateInput
                    | Role::Splitter
            )
            // Include both native headings and labels promoted through `heading_label`.
            || self.is_heading()
    }

    /// Returns whether missing accessible names may be synthesized from visible child text.
    ///
    /// Text-entry, range, combo-box, container, live-region, image, and passive roles are excluded because their visible text is a value, descendant content, or separate caption rather than the control name.
    #[must_use]
    pub fn takes_name_from_content(&self) -> bool {
        matches!(
            self.role,
            Role::Button
                | Role::Link
                | Role::CheckBox
                | Role::Switch
                | Role::RadioButton
                | Role::ListItem
                | Role::Tab
                | Role::TreeItem
                | Role::Disclosure
                | Role::MenuItem
                | Role::MenuItemCheckBox
                | Role::MenuItemRadio
                | Role::ListBoxOption
                | Role::ColumnHeader
                | Role::GridCell
        )
    }
}
