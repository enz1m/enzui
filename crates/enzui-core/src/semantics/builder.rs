//! Builder methods for accessibility semantics.

use super::{
    ActionDisposition, GridPosition, HeadingLevel, Live, NumericValue, Orientation, Role,
    Semantics, SetPosition, SortDirection, Toggle,
};
use iced_core::widget::Id;

fn alloc_join(description: &str, announcement: &str) -> String {
    let mut s = String::with_capacity(description.len() + 2 + announcement.len());
    s.push_str(description);
    s.push_str(", ");
    s.push_str(announcement);
    s
}

impl Semantics {
    /// Start a new semantics descriptor for the given role.
    pub fn new(role: Role) -> Self {
        Self {
            role,
            label: None,
            description: None,
            keyboard_shortcut: None,
            invalid: false,
            expanded: None,
            // Expansion capability defaults on; it matters only when expanded state is present.
            expandable_action: true,
            value: None,
            numeric: None,
            toggled: None,
            selected: None,
            action_disposition: None,
            sort: None,
            disabled: false,
            read_only: false,
            required: false,
            busy: false,
            focused: false,
            supports_focus: false,
            orientation: None,
            supports_default_action: false,
            adjustable: false,
            editable: false,
            supports_context_menu: false,
            grid_position: None,
            level: None,
            set_position: None,
            modal: false,
            active_descendant: None,
            text: None,
            labelled_by: Vec::new(),
            described_by: Vec::new(),
            controls: Vec::new(),
            error_message: None,
            // Seed live-region roles with their standard politeness; other roles inherit from ancestors.
            live: match role {
                Role::Status => Some(Live::Polite),
                Role::Alert => Some(Live::Assertive),
                _ => None,
            },
            heading_level: None,
        }
    }

    /// Sets live-region politeness.
    ///
    /// `None` inherits from ancestors, while `Some(Live::Off)` explicitly suppresses inherited announcements.
    pub fn live(mut self, live: Option<Live>) -> Self {
        self.live = live;
        self
    }

    /// Set the accessible name.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the native accessible keyboard shortcut.
    pub fn keyboard_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.keyboard_shortcut = Some(shortcut.into());
        self
    }

    /// Sets or clears the accessible name.
    pub fn label_opt(mut self, label: Option<impl Into<String>>) -> Self {
        self.label = label.map(Into::into);
        self
    }

    /// Set the longer description.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Marks whether the control’s current value is invalid.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// Sets expanded state and declares that assistive technology may toggle it.
    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = Some(expanded);
        self.expandable_action = true;
        self
    }

    /// Sets expanded state without advertising an expand or collapse action.
    pub fn expanded_state_only(mut self, expanded: bool) -> Self {
        self.expanded = Some(expanded);
        self.expandable_action = false;
        self
    }

    /// Set the textual value.
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    /// Sets or clears the textual value.
    pub fn value_opt(mut self, value: Option<impl Into<String>>) -> Self {
        self.value = value.map(Into::into);
        self
    }

    /// Set the numeric value contract.
    pub fn numeric(mut self, numeric: NumericValue) -> Self {
        self.numeric = Some(numeric);
        self
    }

    /// Set the checked / toggled state.
    pub fn toggled(mut self, toggled: Toggle) -> Self {
        self.toggled = Some(toggled);
        self
    }

    /// Set the selected state.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = Some(selected);
        self
    }

    /// Marks the default action as destructive, or clears the disposition when `false`.
    pub fn destructive_action(mut self, destructive: bool) -> Self {
        self.action_disposition = destructive.then_some(ActionDisposition::Destructive);
        self
    }

    /// Sets a sortable column’s current sort state.
    pub fn sort(mut self, sort: SortDirection) -> Self {
        self.sort = Some(sort);
        self
    }

    /// Returns the caller description with the sort announcement appended.
    ///
    /// Either component may stand alone; returns `None` when both are absent.
    pub fn sort_announcement(&self) -> Option<String> {
        match (&self.description, self.sort) {
            (Some(d), Some(sort)) => Some(alloc_join(d, &sort.announcement())),
            (None, Some(sort)) => Some(sort.announcement()),
            (Some(d), None) => Some(d.clone()),
            (None, None) => None,
        }
    }

    /// Returns the platform description, prefixing destructive disposition and preserving caller and sort descriptions.
    ///
    /// Returns `None` when no component is present.
    pub fn accessible_description(&self) -> Option<String> {
        let description = self.sort_announcement();
        match (self.action_disposition, description) {
            (Some(disposition), Some(description)) => {
                Some(alloc_join(&disposition.announcement(), &description))
            }
            (Some(disposition), None) => Some(disposition.announcement()),
            (None, description) => description,
        }
    }

    /// Mark the control disabled/enabled.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Mark the control focused/unfocused.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Adds focus routing without adding activation or value-changing actions.
    pub fn focusable(mut self) -> Self {
        self.supports_focus = true;
        self
    }

    /// Set the orientation.
    pub fn orientation(mut self, orientation: Orientation) -> Self {
        self.orientation = Some(orientation);
        self
    }

    /// Declare that the control supports the default (activate) action.
    pub fn default_action(mut self) -> Self {
        self.supports_default_action = true;
        self
    }

    /// Declares whether assistive technology may increment or decrement this control’s numeric value.
    pub fn adjustable(mut self, adjustable: bool) -> Self {
        self.adjustable = adjustable;
        self
    }

    /// Declares whether assistive technology may set this control’s value directly.
    pub fn editable(mut self, editable: bool) -> Self {
        self.editable = editable;
        self
    }

    /// Declares support for the assistive-technology context-menu action.
    pub fn context_menu(mut self) -> Self {
        self.supports_context_menu = true;
        self
    }

    /// Publish a cell's position within an enclosing grid.
    pub fn grid_position(mut self, position: GridPosition) -> Self {
        self.grid_position = Some(position);
        self
    }

    /// Sets the item’s 1-based hierarchical depth.
    pub fn level(mut self, level: usize) -> Self {
        self.level = Some(level);
        self
    }

    /// Changes the role to [`Role::Heading`] and sets its outline level.
    pub fn heading(mut self, level: HeadingLevel) -> Self {
        self.role = Role::Heading;
        self.level = Some(level.level());
        self.heading_level = Some(level);
        self
    }

    /// Sets a heading level without replacing the existing role.
    ///
    /// This supports label-based collection while the AccessKit adapter exposes the node as a heading.
    pub fn heading_label(mut self, level: HeadingLevel) -> Self {
        self.heading_level = Some(level);
        self.level = Some(level.level());
        self
    }

    /// Publish the item's 1-based position within its set ("4 of 9").
    pub fn set_position(mut self, position: SetPosition) -> Self {
        self.set_position = Some(position);
        self
    }

    /// Sets whether this surface makes content outside it inert to assistive technology.
    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    /// Sets the active descendant while keyboard focus remains on this node.
    pub fn active_descendant(mut self, target: Id) -> Self {
        self.active_descendant = Some(target);
        self
    }

    /// Adds a widget whose text names this node, such as a form row's label.
    pub fn labelled_by(mut self, target: Id) -> Self {
        self.labelled_by.push(target);
        self
    }

    /// Adds a widget whose text describes this node, such as a hint.
    pub fn described_by(mut self, target: Id) -> Self {
        self.described_by.push(target);
        self
    }

    /// Adds a widget this node controls, such as a tab's panel.
    pub fn controls(mut self, target: Id) -> Self {
        self.controls.push(target);
        self
    }

    /// Sets the widget that shows this node's error message.
    pub fn error_message(mut self, target: Id) -> Self {
        self.error_message = Some(target);
        self
    }

    /// Marks an editable control as read-only without disabling it.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Marks whether a value is required independently of its current validity.
    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }

    /// Marks the element busy while it works on something; assistive
    /// technology announces it.
    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }
}
