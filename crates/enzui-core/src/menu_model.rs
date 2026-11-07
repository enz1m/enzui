//! Renderer-neutral menu entries, navigation state, activation, and open/close lifecycle.
//!
//! Stable [`EntryId`] values preserve highlight identity across model changes, while rendering, placement, and focus restoration remain widget responsibilities.

use crate::context_menu::{MenuEntry, MenuMove, MenuNavigation};
use crate::semantics::{Role, Semantics};
use iced_core::keyboard::{self, key};
use std::collections::HashSet;

/// Stable string identity for a menu entry, including entries created from runtime data.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntryId(pub String);

impl EntryId {
    /// Build an entry id from anything string-like.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The underlying key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for EntryId {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for EntryId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Display for EntryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The optional checked-state affordance of a menu entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CheckState {
    /// No check affordance; a plain command row.
    #[default]
    None,
    /// A checkable row, currently unchecked.
    Unchecked,
    /// A checkable row, currently checked.
    Checked,
}

impl CheckState {
    /// Whether this entry shows a check affordance at all.
    #[must_use]
    pub const fn is_checkable(self) -> bool {
        !matches!(self, Self::None)
    }

    /// Whether the check is currently set.
    #[must_use]
    pub const fn is_checked(self) -> bool {
        matches!(self, Self::Checked)
    }

    /// The state this entry takes when activated.
    ///
    /// A non-checkable entry stays non-checkable — activating a command row
    /// must not silently turn it into a checkbox.
    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::None => Self::None,
            Self::Unchecked => Self::Checked,
            Self::Checked => Self::Unchecked,
        }
    }
}

/// What an entry does when activated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EntryKind {
    /// Resolves activation to one dispatch request.
    #[default]
    Command,
    /// Activating it (or pressing the inline arrow) opens a submenu.
    Submenu,
    /// A non-focusable rule between groups.
    Separator,
}

/// A menu row constructed directly or from a shared [`crate::Action`].
#[derive(Debug, Clone, PartialEq)]
pub struct MenuEntryModel {
    id: EntryId,
    label: String,
    kind: EntryKind,
    enabled: bool,
    check: CheckState,
    icon: Option<String>,
    /// The icon's own colour (a container's colour), over the row's ink.
    icon_tint: Option<iced_core::Color>,
    shortcut: Option<String>,
    description: Option<String>,
    destructive: bool,
    radio: bool,
}

impl MenuEntryModel {
    /// A plain command row.
    pub fn command(id: impl Into<EntryId>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind: EntryKind::Command,
            enabled: true,
            check: CheckState::None,
            icon: None,
            icon_tint: None,
            shortcut: None,
            description: None,
            destructive: false,
            radio: false,
        }
    }

    /// A row that opens a submenu.
    pub fn submenu(id: impl Into<EntryId>, label: impl Into<String>) -> Self {
        Self {
            kind: EntryKind::Submenu,
            ..Self::command(id, label)
        }
    }

    /// A non-focusable separator.
    pub fn separator(id: impl Into<EntryId>) -> Self {
        Self {
            kind: EntryKind::Separator,
            enabled: false,
            ..Self::command(id, "")
        }
    }

    /// Constructs an entry from an action's id, label, enabled state, icon name, and rendered shortcut.
    pub fn from_action<Message>(action: &crate::Action<Message>) -> Self {
        let mut entry = Self::command(EntryId::new(action.id()), action.label());
        entry.enabled = action.is_enabled();
        // Forward the requested icon identifier; the row resolver cannot consume the icon's `Debug` representation.
        entry.icon = action.icon().map(|icon| icon.requested_name().to_owned());
        // Use `Shortcut`'s `Display` implementation as the formatting source of truth.
        entry.shortcut = action.shortcut().map(ToString::to_string);
        entry
    }

    /// Set the enabled flag. Separators ignore this — they are never focusable.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        if self.kind != EntryKind::Separator {
            self.enabled = enabled;
        }
        self
    }

    /// Give the row a check affordance.
    #[must_use]
    pub fn check(mut self, check: CheckState) -> Self {
        self.check = check;
        self
    }

    /// Make a checkable row one choice of several, as radio items are: its
    /// check marks the current choice.
    #[must_use]
    pub fn radio(mut self, radio: bool) -> Self {
        self.radio = radio;
        self
    }

    /// Whether the row is one choice of several.
    #[must_use]
    pub fn is_radio(&self) -> bool {
        self.radio
    }

    /// Set the leading icon name.
    #[must_use]
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Paint the row's icon in `color` (a container's or an account's
    /// colour) instead of the row's ink; the label keeps the theme's ink and
    /// a disabled row its disabled ink.
    #[must_use]
    pub fn icon_tint(mut self, color: iced_core::Color) -> Self {
        self.icon_tint = Some(color);
        self
    }

    /// Set the trailing shortcut hint shown in the row and exposed through
    /// the native accessibility shortcut property. Dispatch remains the
    /// application's responsibility.
    #[must_use]
    pub fn shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Set the secondary description line.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Mark the row as destructive (danger tone plus an AT warning).
    #[must_use]
    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    /// Stable identity.
    #[must_use]
    pub fn id(&self) -> &EntryId {
        &self.id
    }

    /// Display label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// What activating this row does.
    #[must_use]
    pub const fn kind(&self) -> EntryKind {
        self.kind
    }

    /// Whether the row accepts activation.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// The check affordance.
    #[must_use]
    pub const fn check_state(&self) -> CheckState {
        self.check
    }

    /// Leading icon name, if any.
    #[must_use]
    pub fn icon_name(&self) -> Option<&str> {
        self.icon.as_deref()
    }

    /// The icon's own colour, if the row has one.
    #[must_use]
    pub fn icon_tint_color(&self) -> Option<iced_core::Color> {
        self.icon_tint
    }

    /// Trailing shortcut hint, if any.
    #[must_use]
    pub fn shortcut_hint(&self) -> Option<&str> {
        self.shortcut.as_deref()
    }

    /// Secondary description, if any.
    #[must_use]
    pub fn description_text(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Whether the row renders in the destructive tone and publishes a
    /// destructive action disposition.
    #[must_use]
    pub const fn is_destructive(&self) -> bool {
        self.destructive
    }

    /// Returns whether keyboard navigation may land on this enabled, non-separator row.
    #[must_use]
    pub const fn is_focusable(&self) -> bool {
        self.enabled && !matches!(self.kind, EntryKind::Separator)
    }

    /// Resolves this row's activation without a model lookup.
    #[must_use]
    pub fn activation(&self) -> Activation {
        if !self.is_focusable() {
            return Activation::None;
        }
        match self.kind {
            EntryKind::Separator => Activation::None,
            EntryKind::Submenu => Activation::OpenSubmenu(self.id.clone()),
            EntryKind::Command => Activation::Dispatch {
                id: self.id.clone(),
                // Choosing a radio item selects it; only a checkbox unchecks.
                check: if self.radio && self.check.is_checkable() {
                    CheckState::Checked
                } else {
                    self.check.toggled()
                },
            },
        }
    }

    /// The navigation entry this row projects to.
    #[must_use]
    pub const fn nav_entry(&self) -> MenuEntry {
        match self.kind {
            EntryKind::Separator => MenuEntry::Separator,
            _ => MenuEntry::Item {
                enabled: self.enabled,
            },
        }
    }
}

/// The split-button segment that received an interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment {
    /// The leading action half of a split button.
    Primary,
    /// The trailing chevron, or the whole surface in menu-only mode.
    Menu,
}

/// How a menu was asked to open, which decides where the highlight starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCause {
    /// Pointer opening does not pre-highlight a row.
    Pointer,
    /// Enter, Space, or Down opens with the first focusable row highlighted.
    KeyboardFirst,
    /// <kbd>Up</kbd>: highlight the last focusable row.
    KeyboardLast,
}

impl OpenCause {
    /// The initial navigation movement this cause implies, if any.
    #[must_use]
    pub const fn initial_move(self) -> Option<MenuMove> {
        match self {
            Self::Pointer => None,
            Self::KeyboardFirst => Some(MenuMove::First),
            Self::KeyboardLast => Some(MenuMove::Last),
        }
    }
}

/// The reason a menu closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseCause {
    /// A row was activated.
    Activated,
    /// <kbd>Escape</kbd>.
    Escape,
    /// A click landed outside the popup.
    OutsideClick,
    /// The trigger was clicked again while open.
    TriggerToggle,
    /// The host closed it programmatically (window lost focus, model emptied).
    Host,
}

impl CloseCause {
    /// Returns whether dismissal should restore focus to the trigger; host-driven closure does not.
    #[must_use]
    pub const fn restores_focus(self) -> bool {
        !matches!(self, Self::Host)
    }
}

/// What an activation attempt resolved to.
#[derive(Debug, Clone, PartialEq)]
pub enum Activation {
    /// Requests dispatch and menu closure, carrying the entry's post-toggle check state.
    Dispatch {
        /// The entry that fired.
        id: EntryId,
        /// The check state the entry takes as a result.
        check: CheckState,
    },
    /// Open this entry's submenu; the menu stays open.
    OpenSubmenu(EntryId),
    /// Nothing happened — no active row, or the row is disabled or a separator.
    None,
}

impl Activation {
    /// Whether this activation closes the menu.
    #[must_use]
    pub const fn closes(&self) -> bool {
        matches!(self, Self::Dispatch { .. })
    }
}

/// One menu level whose active row is stored by stable id and resolved against current entries.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MenuModel {
    entries: Vec<MenuEntryModel>,
    active: Option<EntryId>,
    /// The entries each submenu row opens, keyed by that row's id.
    submenus: Vec<(EntryId, MenuModel)>,
}

impl MenuModel {
    /// Build a model from entries, with no active row.
    #[must_use]
    pub fn new(entries: Vec<MenuEntryModel>) -> Self {
        Self {
            entries,
            active: None,
            submenus: Vec::new(),
        }
    }

    /// The entries the submenu row `parent` (a [`MenuEntryModel::submenu`])
    /// opens. A submenu's own submenus are not opened.
    #[must_use]
    pub fn with_submenu(mut self, parent: impl Into<EntryId>, model: MenuModel) -> Self {
        let parent = parent.into();
        self.submenus.retain(|(id, _)| *id != parent);
        self.submenus.push((parent, model));
        self
    }

    /// The submenu row `parent` opens, if it has entries.
    #[must_use]
    pub fn submenu(&self, parent: &EntryId) -> Option<&MenuModel> {
        self.submenus
            .iter()
            .find(|(id, _)| id == parent)
            .map(|(_, model)| model)
    }

    /// The submenu row `parent` opens, to move its highlight.
    pub fn submenu_mut(&mut self, parent: &EntryId) -> Option<&mut MenuModel> {
        self.submenus
            .iter_mut()
            .find(|(id, _)| id == parent)
            .map(|(_, model)| model)
    }

    /// The entries, in order.
    #[must_use]
    pub fn entries(&self) -> &[MenuEntryModel] {
        &self.entries
    }

    /// How many entries there are, separators included.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no entries at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many entries keyboard navigation can land on.
    #[must_use]
    pub fn focusable_count(&self) -> usize {
        self.entries.iter().filter(|e| e.is_focusable()).count()
    }

    /// Returns whether at least one focusable entry makes the menu useful to open.
    #[must_use]
    pub fn has_focusable(&self) -> bool {
        // Stop at the first focusable entry; this predicate runs on every closed-trigger render.
        self.entries.iter().any(MenuEntryModel::is_focusable)
    }

    /// The active row's id, if any.
    #[must_use]
    pub fn active(&self) -> Option<&EntryId> {
        self.active.as_ref()
    }

    /// Returns the active row's index in the current entries, if any.
    #[must_use]
    pub fn active_index(&self) -> Option<usize> {
        let active = self.active.as_ref()?;
        self.index_of(active)
    }

    /// Find an entry's index by id.
    #[must_use]
    pub fn index_of(&self, id: &EntryId) -> Option<usize> {
        self.entries.iter().position(|e| &e.id == id)
    }

    /// Find an entry by id.
    #[must_use]
    pub fn entry(&self, id: &EntryId) -> Option<&MenuEntryModel> {
        self.entries.iter().find(|e| &e.id == id)
    }

    /// Returns entry ids that occur more than once.
    #[must_use]
    pub fn duplicate_ids(&self) -> Vec<EntryId> {
        // Hash sets keep duplicate detection linear; collect duplicates in encounter order for stable diagnostics.
        let mut seen: HashSet<&EntryId> = HashSet::with_capacity(self.entries.len());
        let mut reported: HashSet<&EntryId> = HashSet::new();
        let mut dupes: Vec<EntryId> = Vec::new();
        for entry in &self.entries {
            if !seen.insert(&entry.id) && reported.insert(&entry.id) {
                dupes.push(entry.id.clone());
            }
        }
        dupes
    }

    /// Returns entries after keeping the first non-separator occurrence of each id; separators are not deduplicated.
    #[must_use]
    pub fn dedup_entries(&self) -> Vec<&MenuEntryModel> {
        let mut seen: HashSet<&EntryId> = HashSet::with_capacity(self.entries.len());
        self.entries
            .iter()
            .filter(|e| e.kind == EntryKind::Separator || seen.insert(&e.id))
            .collect()
    }

    fn navigation(&self) -> MenuNavigation {
        let mut nav =
            MenuNavigation::new(self.entries.iter().map(MenuEntryModel::nav_entry).collect());
        if let Some(index) = self.active_index() {
            nav.set_active(index);
        }
        nav
    }

    /// Applies a keyboard movement through [`MenuNavigation`] and returns the new active id only when it changes.
    pub fn navigate(&mut self, movement: MenuMove) -> Option<EntryId> {
        let before = self.active.clone();
        let mut nav = self.navigation();
        let index = nav.apply(movement)?;
        let id = self.entries.get(index)?.id.clone();
        self.active = Some(id.clone());
        if before.as_ref() == Some(&id) {
            None
        } else {
            Some(id)
        }
    }

    /// Point the highlight at a specific row (hover, or an open-with-highlight).
    ///
    /// Rejects separators, disabled rows and unknown ids, leaving the previous
    /// highlight untouched — same contract as [`MenuNavigation::set_active`].
    pub fn set_active(&mut self, id: &EntryId) -> bool {
        let Some(index) = self.index_of(id) else {
            return false;
        };
        if !self.entries[index].is_focusable() {
            return false;
        }
        self.active = Some(id.clone());
        true
    }

    /// Drop the highlight (pointer left the menu), in the submenus too.
    pub fn clear_active(&mut self) {
        self.active = None;
        for (_, submenu) in &mut self.submenus {
            submenu.clear_active();
        }
    }

    /// Replaces entries while preserving a surviving focusable active id.
    ///
    /// If the active row disappears or becomes unfocusable, selection moves to the nearest focusable row at or after its former position, then wraps backward.
    pub fn reconcile(&mut self, entries: Vec<MenuEntryModel>) -> Option<EntryId> {
        let previous_index = self.active_index();
        let previous = self.active.clone();
        self.entries = entries;

        if let Some(id) = previous.as_ref()
            && let Some(index) = self.index_of(id)
            && self.entries[index].is_focusable()
        {
            return self.active.clone();
        }

        self.active = previous_index.and_then(|start| self.nearest_focusable(start));
        self.active.clone()
    }

    fn nearest_focusable(&self, start: usize) -> Option<EntryId> {
        self.entries
            .iter()
            .skip(start)
            .find(|e| e.is_focusable())
            .or_else(|| {
                self.entries
                    .iter()
                    .take(start)
                    .rev()
                    .find(|e| e.is_focusable())
            })
            .map(|e| e.id.clone())
    }

    /// Resolve an activation of the currently active row.
    #[must_use]
    pub fn activate_active(&self) -> Activation {
        match self.active.as_ref() {
            Some(id) => self.activate(id),
            None => Activation::None,
        }
    }

    /// Resolves activation for an id; unknown, disabled, and separator rows return [`Activation::None`].
    #[must_use]
    pub fn activate(&self, id: &EntryId) -> Activation {
        self.entry(id)
            .map_or(Activation::None, MenuEntryModel::activation)
    }
}

/// Application-owned open state for a menu and its currently open submenu.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MenuState {
    open: bool,
    submenu: Option<EntryId>,
}

impl MenuState {
    /// A closed menu.
    #[must_use]
    pub fn closed() -> Self {
        Self::default()
    }

    /// Whether the popup is showing.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// The open submenu's parent entry, if a submenu is showing.
    #[must_use]
    pub fn open_submenu(&self) -> Option<&EntryId> {
        self.submenu.as_ref()
    }

    /// Opens the menu, returning `false` if it was already open.
    pub fn open(&mut self) -> bool {
        if self.open {
            return false;
        }
        self.open = true;
        true
    }

    /// Closes the menu and any submenu, returning `false` if it was already closed.
    pub fn close(&mut self, _cause: CloseCause) -> bool {
        if !self.open {
            return false;
        }
        self.open = false;
        self.submenu = None;
        true
    }

    /// Open a submenu. Only meaningful while the menu itself is open.
    pub fn open_submenu_for(&mut self, id: EntryId) -> bool {
        if !self.open || self.submenu.as_ref() == Some(&id) {
            return false;
        }
        self.submenu = Some(id);
        true
    }

    /// Close only the submenu, leaving the parent menu open (<kbd>Left</kbd>).
    pub fn close_submenu(&mut self) -> bool {
        self.submenu.take().is_some()
    }
}

/// An action decoded from a keypress while a menu is open, with Left and Right reserved for submenu navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKey {
    /// Move the highlight.
    Move(MenuMove),
    /// Activate the highlighted row.
    Activate,
    /// Dismiss the menu.
    Dismiss,
    /// Open the highlighted row's submenu (<kbd>Right</kbd>).
    OpenSubmenu,
    /// Close the current submenu, staying in the parent (<kbd>Left</kbd>).
    CloseSubmenu,
}

/// Decodes a keypress for an open menu.
///
/// Unclaimed keys and keys modified by Ctrl, Alt, or Logo return `None` so application chords can bubble. Shift remains menu-owned, and Escape dismisses regardless of modifiers.
#[must_use]
pub fn key_for_open_menu(event: &keyboard::Event) -> Option<MenuKey> {
    let keyboard::Event::KeyPressed {
        key: pressed,
        modifiers,
        ..
    } = event
    else {
        return None;
    };
    // Check Escape first so modifiers cannot prevent menu dismissal.
    if matches!(pressed, keyboard::Key::Named(key::Named::Escape)) {
        return Some(MenuKey::Dismiss);
    }
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    match pressed {
        keyboard::Key::Named(key::Named::ArrowDown) => Some(MenuKey::Move(MenuMove::Next)),
        keyboard::Key::Named(key::Named::ArrowUp) => Some(MenuKey::Move(MenuMove::Previous)),
        keyboard::Key::Named(key::Named::Home) => Some(MenuKey::Move(MenuMove::First)),
        keyboard::Key::Named(key::Named::End) => Some(MenuKey::Move(MenuMove::Last)),
        keyboard::Key::Named(key::Named::Enter | key::Named::Space) => Some(MenuKey::Activate),
        keyboard::Key::Named(key::Named::ArrowRight) => Some(MenuKey::OpenSubmenu),
        keyboard::Key::Named(key::Named::ArrowLeft) => Some(MenuKey::CloseSubmenu),
        _ => None,
    }
}

/// Decodes a keypress for a closed trigger.
///
/// Up opens on the last focusable row; other supported keys open on the first. Ctrl, Alt, and Logo chords bubble, while Shift remains trigger-owned.
#[must_use]
pub fn open_cause_for_key(event: &keyboard::Event) -> Option<OpenCause> {
    let keyboard::Event::KeyPressed {
        key: pressed,
        modifiers,
        ..
    } = event
    else {
        return None;
    };
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    match pressed {
        keyboard::Key::Named(key::Named::ArrowUp) => Some(OpenCause::KeyboardLast),
        keyboard::Key::Named(key::Named::Enter | key::Named::Space | key::Named::ArrowDown) => {
            Some(OpenCause::KeyboardFirst)
        }
        _ => None,
    }
}

/// Builds accessibility semantics for a menu trigger.
///
/// With a nonblank `aria_label`, the node is a combo box named by that label and the nonblank trigger label becomes its value. Otherwise it is a button named `More {label} actions`, or `Menu` when the trigger label is blank. Expanded state is always published.
pub fn trigger_semantics(
    label: Option<&str>,
    aria_label: Option<&str>,
    open: bool,
    enabled: bool,
) -> Semantics {
    let value = label.filter(|l| !l.trim().is_empty());
    match aria_label.filter(|a| !a.trim().is_empty()) {
        Some(name) => Semantics::new(Role::ComboBox)
            .label(name.to_owned())
            .value_opt(value.map(str::to_owned))
            .expanded(open)
            .default_action()
            .disabled(!enabled),
        None => {
            let name = match value {
                Some(label) => {
                    crate::localized!("enzui-menu-model-more-actions", "label" => label)
                }
                None => crate::localization::text("enzui-menu-model-menu"),
            };
            Semantics::new(Role::Button)
                .label(name)
                .expanded(open)
                .default_action()
                .disabled(!enabled)
        }
    }
}

/// Semantics for the popup surface itself.
pub fn menu_semantics(label: Option<&str>, model: &MenuModel) -> Semantics {
    let name = match label {
        Some(label) if !label.trim().is_empty() => {
            crate::localized!("enzui-menu-model-named-menu", "label" => label)
        }
        _ => crate::localization::text("enzui-menu-model-menu"),
    };
    let semantics = Semantics::new(Role::Menu).label(name);
    if model.has_focusable() {
        semantics
    } else {
        semantics.description(crate::localization::text(
            "enzui-menu-model-no-available-actions",
        ))
    }
}

/// Builds semantics for one row, with a one-based position among focusable rows.
#[must_use]
pub fn entry_semantics(model: &MenuModel, id: &EntryId) -> Option<Semantics> {
    let entry = model.entry(id)?;
    if entry.kind == EntryKind::Separator {
        return None;
    }

    let total = model.focusable_count();
    let position = model
        .entries()
        .iter()
        .filter(|e| e.is_focusable())
        .position(|e| &e.id == id)
        .map(|index| index + 1);

    // Checkable rows use `MenuItemCheckBox` so assistive technology receives checked-state semantics and the matching action role.
    let role = if entry.check_state().is_checkable() && entry.is_radio() {
        Role::MenuItemRadio
    } else if entry.check_state().is_checkable() {
        Role::MenuItemCheckBox
    } else {
        Role::MenuItem
    };
    let mut semantics = Semantics::new(role)
        .label(entry.label().to_owned())
        .default_action()
        .destructive_action(entry.is_destructive())
        .disabled(!entry.is_enabled())
        .focused(model.active() == Some(id));

    if let Some(position) = position {
        semantics = semantics.value(
            crate::localized!("enzui-menu-position", "position" => position, "total" => total),
        );
    }
    if entry.check_state().is_checkable() {
        semantics = semantics.selected(entry.check_state().is_checked());
    }
    if entry.kind == EntryKind::Submenu {
        semantics = semantics.expanded(model.active() == Some(id));
    }
    if let Some(description) = entry.description_text() {
        semantics = semantics.description(description.to_owned());
    }
    if let Some(shortcut) = entry
        .shortcut_hint()
        .filter(|shortcut| !shortcut.is_empty())
    {
        semantics = semantics.keyboard_shortcut(shortcut.to_owned());
    }

    Some(semantics)
}
