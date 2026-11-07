/// One row of a [`ContextMenu`]: a typed command, a separator, or a
/// submenu of more rows.
#[derive(Debug, Clone)]
pub enum ContextMenuItem<Command> {
    /// A command row that hands `command` back when chosen.
    Command {
        /// What choosing the row does.
        command: Command,
        /// The visible label, also its accessible name.
        label: String,
        /// An icon name.
        icon: Option<String>,
        /// The icon's own colour, over the row's ink.
        icon_tint: Option<iced::Color>,
        /// A shortcut hint shown at the trailing edge.
        shortcut: Option<String>,
        /// Whether the row can be chosen.
        enabled: bool,
        /// Whether the row deletes or discards something.
        destructive: bool,
        /// A checkmark state, for a row that toggles something.
        checked: Option<bool>,
        /// The caller's stable key for the row (see [`ContextMenuItem::key`]).
        key: Option<String>,
    },
    /// A divider between groups of rows.
    Separator,
    /// A row that opens more rows.
    Submenu {
        /// The visible label.
        label: String,
        /// An icon name.
        icon: Option<String>,
        /// The icon's own colour, over the row's ink.
        icon_tint: Option<iced::Color>,
        /// The rows it opens.
        items: Vec<ContextMenuItem<Command>>,
        /// The caller's stable key for the row (see [`ContextMenuItem::key`]).
        key: Option<String>,
    },
}

impl<Command> ContextMenuItem<Command> {
    /// A command row.
    pub fn command(command: Command, label: impl Into<String>) -> Self {
        Self::Command {
            command,
            label: label.into(),
            icon: None,
            icon_tint: None,
            shortcut: None,
            enabled: true,
            destructive: false,
            checked: None,
            key: None,
        }
    }

    /// A divider.
    #[must_use]
    pub fn separator() -> Self {
        Self::Separator
    }

    /// A row that opens `items`.
    pub fn submenu(label: impl Into<String>, items: Vec<ContextMenuItem<Command>>) -> Self {
        Self::Submenu {
            label: label.into(),
            icon: None,
            icon_tint: None,
            items,
            key: None,
        }
    }

    /// Name the row with a key that stays the same when rows are inserted,
    /// removed, reordered or relabelled: [`ContextMenu::reconcile`] keeps
    /// the highlight on it, and a queued event for it can only ever choose
    /// it. Unkeyed rows are numbered by position. Keys must be unique within
    /// one menu; a repeated key falls back to the position.
    #[must_use]
    pub fn key(mut self, name: impl Into<String>) -> Self {
        match &mut self {
            Self::Command { key, .. } | Self::Submenu { key, .. } => *key = Some(name.into()),
            Self::Separator => {}
        }
        self
    }

    /// Show an icon.
    #[must_use]
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        match &mut self {
            Self::Command { icon, .. } | Self::Submenu { icon, .. } => *icon = Some(name.into()),
            Self::Separator => {}
        }
        self
    }

    /// Paint the icon in `color` (a container's colour); the label keeps the
    /// theme's ink and a disabled row its disabled ink.
    #[must_use]
    pub fn icon_tint(mut self, color: iced::Color) -> Self {
        match &mut self {
            Self::Command { icon_tint, .. } | Self::Submenu { icon_tint, .. } => {
                *icon_tint = Some(color);
            }
            Self::Separator => {}
        }
        self
    }

    /// Show a shortcut hint.
    #[must_use]
    pub fn shortcut(mut self, hint: impl Into<String>) -> Self {
        if let Self::Command { shortcut, .. } = &mut self {
            *shortcut = Some(hint.into());
        }
        self
    }

    /// Whether the row can be chosen; true by default.
    #[must_use]
    pub fn enabled(mut self, on: bool) -> Self {
        if let Self::Command { enabled, .. } = &mut self {
            *enabled = on;
        }
        self
    }

    /// Mark the row as deleting or discarding something.
    #[must_use]
    pub fn destructive(mut self, on: bool) -> Self {
        if let Self::Command { destructive, .. } = &mut self {
            *destructive = on;
        }
        self
    }

    /// Show a checkmark state.
    #[must_use]
    pub fn checked(mut self, on: bool) -> Self {
        if let Self::Command { checked, .. } = &mut self {
            *checked = Some(on);
        }
        self
    }
}

/// The rows of an open [`ContextMenu`], lowered to the string-keyed model
/// the host renders, with each command filed under its row's key.
struct OpenMenu<Target, Command> {
    target: Target,
    model: MenuModel,
    commands: std::collections::HashMap<EntryId, Command>,
    submenus: std::collections::HashMap<EntryId, MenuModel>,
}

impl<Target, Command> OpenMenu<Target, Command> {
    fn new(target: Target, items: Vec<ContextMenuItem<Command>>) -> Self {
        let mut open = Self {
            target,
            model: MenuModel::new(Vec::new()),
            commands: std::collections::HashMap::new(),
            submenus: std::collections::HashMap::new(),
        };
        let mut next = 0usize;
        let mut keys = std::collections::HashSet::new();
        open.model = open.lower(items, &mut next, &mut keys);
        open
    }

    /// Lowers `items` into a model: a keyed row is filed under its key, an
    /// unkeyed one (or a repeated key) under its position.
    fn lower(
        &mut self,
        items: Vec<ContextMenuItem<Command>>,
        next: &mut usize,
        keys: &mut std::collections::HashSet<String>,
    ) -> MenuModel {
        let mut entries = Vec::with_capacity(items.len());
        for item in items {
            let position = EntryId::new(format!("row-{next}"));
            *next += 1;
            let keyed = match &item {
                ContextMenuItem::Command { key, .. } | ContextMenuItem::Submenu { key, .. } => {
                    key.clone()
                }
                ContextMenuItem::Separator => None,
            };
            let id = match keyed {
                Some(key) if keys.insert(key.clone()) => EntryId::new(format!("key:{key}")),
                _ => position,
            };
            let entry = match item {
                ContextMenuItem::Command {
                    command,
                    label,
                    icon,
                    icon_tint,
                    shortcut,
                    enabled,
                    destructive,
                    checked,
                    ..
                } => {
                    self.commands.insert(id.clone(), command);
                    let mut entry = enzui_core::menu_model::MenuEntryModel::command(id, label)
                        .enabled(enabled)
                        .destructive(destructive);
                    if let Some(icon) = icon {
                        entry = entry.icon(icon);
                    }
                    if let Some(color) = icon_tint {
                        entry = entry.icon_tint(color);
                    }
                    if let Some(shortcut) = shortcut {
                        entry = entry.shortcut(shortcut);
                    }
                    if let Some(on) = checked {
                        entry = entry.check(if on {
                            CheckState::Checked
                        } else {
                            CheckState::Unchecked
                        });
                    }
                    entry
                }
                ContextMenuItem::Separator => enzui_core::menu_model::MenuEntryModel::separator(id),
                ContextMenuItem::Submenu {
                    label,
                    icon,
                    icon_tint,
                    items,
                    ..
                } => {
                    let child = self.lower(items, next, keys);
                    self.submenus.insert(id.clone(), child);
                    let mut entry = enzui_core::menu_model::MenuEntryModel::submenu(id, label);
                    if let Some(icon) = icon {
                        entry = entry.icon(icon);
                    }
                    if let Some(color) = icon_tint {
                        entry = entry.icon_tint(color);
                    }
                    entry
                }
            };
            entries.push(entry);
        }
        MenuModel::new(entries)
    }
}

/// A context menu that remembers what it was opened on and hands back typed
/// commands: no string ids to map back, and focus returns to the row it was
/// opened from.
///
/// Open it from a row's right click ([`crate::context_target::enz_context_target`]
/// reports the point), mount it once around the view with [`Self::host`], and
/// pass its events to [`Self::update`]:
///
/// ```ignore
/// // update
/// Message::RowMenu(index, at) => self.menu.open_restoring(
///     index,
///     InvocationKind::Pointer(at),
///     vec![
///         ContextMenuItem::command(Cmd::Rename, "Rename").icon("pencil"),
///         ContextMenuItem::separator(),
///         ContextMenuItem::command(Cmd::Delete, "Delete").destructive(true),
///     ],
///     FocusRestoreTarget::Widget(row_id(index)),
/// ),
/// Message::Menu(event) => {
///     let (chosen, focus) = self.menu.update(&event);
///     if let Some((index, command)) = chosen { /* run it */ }
///     return focus;
/// }
/// // view
/// self.menu.host(page, Message::Menu).into()
/// ```
pub struct ContextMenu<Target, Command> {
    open: Option<OpenMenu<Target, Command>>,
    state: ContextMenuState,
}

impl<Target, Command> std::fmt::Debug for ContextMenu<Target, Command> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextMenu")
            .field("open", &self.is_open())
            .finish_non_exhaustive()
    }
}

impl<Target, Command> Default for ContextMenu<Target, Command> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Target, Command> ContextMenu<Target, Command> {
    /// A closed menu.
    #[must_use]
    pub fn new() -> Self {
        Self {
            open: None,
            state: ContextMenuState::closed(),
        }
    }

    /// Open `items` for `target` at `at`, replacing any open menu.
    pub fn open(
        &mut self,
        target: Target,
        at: InvocationKind,
        items: Vec<ContextMenuItem<Command>>,
    ) {
        self.open = Some(OpenMenu::new(target, items));
        self.state.open(at);
    }

    /// [`Self::open`], returning focus to `restore` when the menu closes.
    pub fn open_restoring(
        &mut self,
        target: Target,
        at: InvocationKind,
        items: Vec<ContextMenuItem<Command>>,
        restore: FocusRestoreTarget,
    ) {
        self.open = Some(OpenMenu::new(target, items));
        self.state.open_with_restore(at, restore);
    }

    /// Replace the rows of the showing menu without reopening it: its
    /// target, anchor, focus-restore target and showing stay, a surviving
    /// highlighted row keeps the highlight (by [key](ContextMenuItem::key)),
    /// and a highlight on a removed or disabled row moves to the nearest
    /// row that can take it. Events still queued for a removed or disabled
    /// row are refused; one for a keyed row can only choose that row. An
    /// open submenu follows its new rows, and closes if its parent row is
    /// gone. Returns whether a menu was showing.
    pub fn reconcile(&mut self, items: Vec<ContextMenuItem<Command>>) -> bool {
        let Some(old) = self.open.take() else {
            return false;
        };
        let mut fresh = OpenMenu::new(old.target, items);
        let mut model = old.model;
        let _ = model.reconcile(fresh.model.entries().to_vec());
        fresh.model = model;
        // Each open submenu level follows the rows its parent now opens.
        let mut keep = 0;
        for level in self.state.stack().levels() {
            let parent_model = match keep {
                0 => &fresh.model,
                depth => self.state.stack().levels()[depth - 1].model(),
            };
            let still_opens = parent_model.entry(level.parent()).is_some()
                && fresh.submenus.contains_key(level.parent());
            if !still_opens {
                break;
            }
            keep += 1;
        }
        while self.state.stack().depth() > keep {
            let _ = self.state.stack_mut().close_submenu();
        }
        for level in self.state.stack_mut().levels_mut() {
            if let Some(rows) = fresh.submenus.get(level.parent()) {
                let _ = level.model_mut().reconcile(rows.entries().to_vec());
            }
        }
        self.open = Some(fresh);
        true
    }

    /// Whether the menu is showing.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open.is_some() && self.state.is_open()
    }

    /// What the menu was opened on, while it shows.
    #[must_use]
    pub fn target(&self) -> Option<&Target> {
        self.open
            .as_ref()
            .filter(|_| self.state.is_open())
            .map(|open| &open.target)
    }

    /// The command on the highlighted row of the deepest showing surface.
    #[must_use]
    pub fn highlighted(&self) -> Option<&Command> {
        let open = self.open.as_ref().filter(|_| self.state.is_open())?;
        let model = self
            .state
            .stack()
            .levels()
            .last()
            .map_or(&open.model, |level| level.model());
        open.commands.get(model.active()?)
    }

    /// Close the menu without choosing anything. The showing stamp is kept,
    /// so the next opening outdates every event from this one.
    pub fn close(&mut self) {
        self.open = None;
        let _ = self.state.close();
    }

    /// Apply an event from [`Self::host`]. A chosen command comes back with
    /// its target, and the menu closes; the task returns focus to the row the
    /// menu was opened from when it closes.
    pub fn update<Message>(
        &mut self,
        event: &ContextMenuEvent,
    ) -> (Option<(Target, Command)>, iced::Task<Message>)
    where
        Message: Send + 'static,
    {
        let Some(open) = self.open.as_mut() else {
            return (None, iced::Task::none());
        };
        let submenus = &open.submenus;
        let resolve = |_depth: usize, parent: &EntryId| submenus.get(parent).cloned();
        let update: ContextMenuUpdate<Message> =
            apply_event_restoring_with(&mut open.model, &mut self.state, event, &resolve);
        let (activated, restore) = update.into_parts();
        let focus = match restore {
            Some(operation) => iced::advanced::widget::operate(operation),
            None => iced::Task::none(),
        };
        let chosen = activated.and_then(|id| open.commands.remove(&id));
        if chosen.is_some() || !self.state.is_open() {
            let closed = self.open.take();
            if let (Some(command), Some(closed)) = (chosen, closed) {
                return (Some((closed.target, command)), focus);
            }
        }
        (None, focus)
    }

    /// Mount the menu around `page`; keep it mounted while closed, so the
    /// page's state survives opening and closing. `page` keeps its own
    /// events while the menu is closed.
    pub fn host<'a, Message: Clone + 'a>(
        &'a self,
        page: impl Into<Element<'a, Message>>,
        on_event: impl Fn(ContextMenuEvent) -> Message + 'a,
    ) -> EnzContextMenuHost<'a, Message> {
        enz_optional_point_context_menu_host(
            page,
            self.open.as_ref().map(|open| (&open.model, &self.state)),
            on_event,
        )
    }
}
