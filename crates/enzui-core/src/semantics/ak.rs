//! Converts renderer-neutral [`Semantics`] values into AccessKit nodes.
use super::{Live, NumericValue, Orientation, Role, Semantics, SetPosition, SortDirection, Toggle};
use accesskit::{
    Action, Live as AkLive, Node, Orientation as AkOrientation, Role as AkRole,
    SortDirection as AkSortDirection, Toggled,
};

impl From<Role> for AkRole {
    fn from(role: Role) -> Self {
        match role {
            Role::Button => AkRole::Button,
            Role::Link => AkRole::Link,
            Role::CheckBox => AkRole::CheckBox,
            Role::Switch => AkRole::Switch,
            Role::RadioButton => AkRole::RadioButton,
            Role::RadioGroup => AkRole::RadioGroup,
            Role::Slider => AkRole::Slider,
            Role::SpinButton => AkRole::SpinButton,
            Role::TextInput => AkRole::TextInput,
            Role::PasswordInput => AkRole::PasswordInput,
            Role::ProgressIndicator => AkRole::ProgressIndicator,
            Role::ListItem => AkRole::ListItem,
            Role::Tab => AkRole::Tab,
            Role::TabList => AkRole::TabList,
            Role::Label => AkRole::Label,
            Role::Document => AkRole::Document,
            Role::Heading => AkRole::Heading,
            Role::Image => AkRole::Image,
            Role::Disclosure => AkRole::DisclosureTriangle,
            Role::Splitter => AkRole::Splitter,
            Role::Menu => AkRole::Menu,
            Role::MenuItem => AkRole::MenuItem,
            Role::MenuItemCheckBox => AkRole::MenuItemCheckBox,
            Role::Grid => AkRole::Grid,
            Role::GridRow => AkRole::Row,
            Role::GridCell => AkRole::Cell,
            Role::ColumnHeader => AkRole::ColumnHeader,
            Role::Tree => AkRole::Tree,
            Role::TreeItem => AkRole::TreeItem,
            Role::DateInput => AkRole::DateInput,
            Role::Dialog => AkRole::Dialog,
            Role::AlertDialog => AkRole::AlertDialog,
            Role::ComboBox => AkRole::ComboBox,
            Role::EditableComboBox => AkRole::EditableComboBox,
            Role::ListBox => AkRole::ListBox,
            Role::ListBoxOption => AkRole::ListBoxOption,
            Role::Group => AkRole::Group,
            Role::Status => AkRole::Status,
            Role::Alert => AkRole::Alert,
            Role::MenuBar => AkRole::MenuBar,
            Role::MenuItemRadio => AkRole::MenuItemRadio,
            Role::TabPanel => AkRole::TabPanel,
            Role::Toolbar => AkRole::Toolbar,
            Role::Tooltip => AkRole::Tooltip,
            // AccessKit's splitter is AT-SPI's separator.
            Role::Separator => AkRole::Splitter,
            Role::Navigation => AkRole::Navigation,
            Role::Main => AkRole::Main,
            Role::Search => AkRole::Search,
            Role::Banner => AkRole::Banner,
            Role::ContentInfo => AkRole::ContentInfo,
            Role::Complementary => AkRole::Complementary,
            Role::Region => AkRole::Region,
            Role::Form => AkRole::Form,
            Role::Terminal => AkRole::Terminal,
        }
    }
}

impl From<Live> for AkLive {
    fn from(l: Live) -> Self {
        match l {
            Live::Off => AkLive::Off,
            Live::Polite => AkLive::Polite,
            Live::Assertive => AkLive::Assertive,
        }
    }
}

impl From<Toggle> for Toggled {
    fn from(t: Toggle) -> Self {
        match t {
            Toggle::False => Toggled::False,
            Toggle::True => Toggled::True,
            Toggle::Mixed => Toggled::Mixed,
        }
    }
}

impl From<Orientation> for AkOrientation {
    fn from(o: Orientation) -> Self {
        match o {
            Orientation::Horizontal => AkOrientation::Horizontal,
            Orientation::Vertical => AkOrientation::Vertical,
        }
    }
}

/// Converts only genuinely sorted states to AccessKit sort directions.
///
/// [`SortDirection::Unsorted`] returns an error so the caller can clear the native property.
impl TryFrom<SortDirection> for AkSortDirection {
    type Error = ();

    fn try_from(d: SortDirection) -> Result<Self, Self::Error> {
        match d {
            SortDirection::Unsorted => Err(()),
            SortDirection::Ascending => Ok(AkSortDirection::Ascending),
            SortDirection::Descending => Ok(AkSortDirection::Descending),
            SortDirection::Other => Ok(AkSortDirection::Other),
        }
    }
}

impl Semantics {
    /// Build an AccessKit [`Node`] for this control. Geometry (bounds) and
    /// child relationships are the caller's responsibility — this fills in
    /// the semantic properties only.
    pub fn to_accesskit_node(&self) -> Node {
        // `is_heading` also recognizes promoted labels, which retain `Role::Label` before AccessKit conversion.
        let ak_role: AkRole = if self.is_heading() {
            AkRole::Heading
        } else {
            self.role.into()
        };
        let mut node = Node::new(ak_role);

        if let Some(label) = &self.label {
            node.set_label(label.clone());
            if ak_role == AkRole::Label && self.value.is_none() {
                node.set_value(label.clone());
            }
        }
        if let Some(shortcut) = &self.keyboard_shortcut {
            node.set_keyboard_shortcut(shortcut.clone());
        }
        // Compose states without audible Linux properties into the caller’s accessible description.
        if let Some(description) = self.accessible_description() {
            node.set_description(description);
        }
        if self.invalid {
            node.set_invalid(accesskit::Invalid::True);
        }
        if let Some(expanded) = self.expanded {
            node.set_expanded(expanded);
        }
        if let Some(value) = &self.value {
            node.set_value(value.clone());
        }
        if let Some(NumericValue {
            value,
            min,
            max,
            step,
        }) = self.numeric
        {
            node.set_numeric_value(value);
            node.set_min_numeric_value(min);
            node.set_max_numeric_value(max);
            if let Some(step) = step {
                node.set_numeric_value_step(step);
            }
        }
        if let Some(toggled) = self.toggled {
            node.set_toggled(toggled.into());
        }
        // AT-SPI announces inserted live regions only when politeness is present; `None` preserves ancestor inheritance.
        if let Some(live) = self.live {
            node.set_live(live.into());
        }
        // Publish `Some(false)` for unselected set members; only `None` means non-selectable.
        if let Some(selected) = self.selected {
            node.set_selected(selected);
        }
        // Clear the native direction for absent or unsorted state; `Other` means genuinely sorted, while the description preserves “sortable, not sorted”.
        match self.sort.and_then(|d| AkSortDirection::try_from(d).ok()) {
            Some(dir) => node.set_sort_direction(dir),
            None => node.clear_sort_direction(),
        }
        if let Some(orientation) = self.orientation {
            node.set_orientation(orientation.into());
        }
        if self.disabled {
            node.set_disabled();
        }
        if self.read_only {
            node.set_read_only();
        }
        if self.required {
            node.set_required();
        }
        if self.busy {
            node.set_busy();
        }
        if self.modal {
            node.set_modal();
        }
        // Derive the target node ID with the tree assembler’s hash; assembly drops unresolved targets.
        if let Some(target) = &self.active_descendant {
            node.set_active_descendant(crate::collect::node_id_of_widget(target));
        }
        let targets = |ids: &[iced_core::widget::Id]| {
            ids.iter()
                .map(crate::collect::node_id_of_widget)
                .collect::<Vec<_>>()
        };
        if !self.labelled_by.is_empty() {
            node.set_labelled_by(targets(&self.labelled_by));
        }
        if !self.described_by.is_empty() {
            node.set_described_by(targets(&self.described_by));
        }
        if !self.controls.is_empty() {
            node.set_controls(targets(&self.controls));
        }
        if let Some(target) = &self.error_message {
            node.set_error_message(crate::collect::node_id_of_widget(target));
        }
        // Publish structural position independently of disabled action gating.
        if let Some(pos) = self.grid_position {
            node.set_row_index(pos.row);
            node.set_column_index(pos.column);
            node.set_row_count(pos.row_count);
            node.set_column_count(pos.column_count);
        }
        if let Some(level) = self.level {
            node.set_level(level);
        }
        if let Some(SetPosition { index, count }) = self.set_position {
            // Convert the caller’s 1-based set index to AccessKit’s 0-based `position_in_set`; adapters add one when announcing it.
            node.set_position_in_set(index.saturating_sub(1));

            // Publish `size_of_set` only on container roles because AccessKit consumers resolve it from the item’s parent.
            if self.is_set_container() {
                node.set_size_of_set(count);
            }
        }

        // Disabled nodes expose no actions.
        if !self.disabled {
            if self.is_interactive() {
                node.add_action(Action::Focus);
            }
            // Advertise the matching expand or collapse action only when toggle capability is declared.
            if self.expandable_action {
                match self.expanded {
                    Some(true) => node.add_action(Action::Collapse),
                    Some(false) => node.add_action(Action::Expand),
                    None => {}
                }
            }
            if self.supports_default_action {
                node.add_action(Action::Click);
            }
            // Advertise context-menu access independently of the default click action.
            if self.supports_context_menu {
                node.add_action(Action::ShowContextMenu);
            }
            // Advertise stepping only for writable controls that declare adjustability and provide a numeric contract.
            if self.adjustable && !self.read_only && self.numeric.is_some() {
                node.add_action(Action::Increment);
                node.add_action(Action::Decrement);
            }
            // Advertise text `SetValue` from declared editability, not from whether a value string is exposed.
            if self.editable
                && !self.read_only
                && matches!(
                    self.role,
                    Role::TextInput | Role::PasswordInput | Role::EditableComboBox
                )
            {
                node.add_action(Action::SetValue);
            }
            // Editable spin buttons and adjustable sliders advertise numeric `SetValue`; value disclosure is not required.
            if self.numeric_settable() {
                node.add_action(Action::SetValue);
            }
        }
        node
    }

    /// Whether the control takes a numeric `SetValue`, before disabled-state
    /// gating: an editable spin button, or a slider that declares adjustability
    /// and a numeric contract. Writable either way.
    pub(crate) fn numeric_settable(&self) -> bool {
        !self.read_only
            && match self.role {
                Role::SpinButton => self.editable,
                Role::Slider => self.adjustable && self.numeric.is_some(),
                _ => false,
            }
    }
}
