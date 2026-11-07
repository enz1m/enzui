//! A menu bar's menus and keyboard behaviour: File, Edit, View and so on,
//! each a [`MenuModel`], with submenus.
//!
//! [`MenuBarModel`] owns the menus and which one (and which submenu) is open;
//! the application keeps it and applies every [`MenuBarEvent`] the widget
//! publishes with [`MenuBarModel::apply`], which returns the command a user
//! chose. [`MenuBarModel::event_for_key`] decodes the keys a desktop menu bar
//! answers: F10 opens the first menu (and closes an open one); with a menu
//! open, Up and Down (Home, End) move through it, Left and Right move to the
//! neighbouring menu or into and out of a submenu, Enter and Space choose,
//! and Escape closes the submenu, then the menu.

use crate::context_menu::MenuMove;
use crate::menu_model::{Activation, EntryId, EntryKind, MenuModel};

/// One menu of a [`MenuBarModel`]: its title, its entries, and the entries
/// of each of its submenus.
#[derive(Debug, Clone)]
pub struct MenuBarMenu {
    title: String,
    model: MenuModel,
    submenus: Vec<(EntryId, MenuModel)>,
}

impl MenuBarMenu {
    /// A menu titled `title` (shown in the bar) listing `model`'s entries.
    #[must_use]
    pub fn new(title: impl Into<String>, model: MenuModel) -> Self {
        Self {
            title: title.into(),
            model,
            submenus: Vec::new(),
        }
    }

    /// The entries of the submenu that entry `parent` (a
    /// [`MenuEntryModel::submenu`](crate::menu_model::MenuEntryModel::submenu)) opens.
    #[must_use]
    pub fn submenu(mut self, parent: impl Into<EntryId>, model: MenuModel) -> Self {
        self.submenus.push((parent.into(), model));
        self
    }

    /// The title shown in the bar.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The menu's entries.
    #[must_use]
    pub fn model(&self) -> &MenuModel {
        &self.model
    }

    /// The entries of the submenu `parent` opens, if it has any.
    #[must_use]
    pub fn submenu_model(&self, parent: &EntryId) -> Option<&MenuModel> {
        self.submenus
            .iter()
            .find(|(id, _)| id == parent)
            .map(|(_, model)| model)
    }

    fn submenu_model_mut(&mut self, parent: &EntryId) -> Option<&mut MenuModel> {
        self.submenus
            .iter_mut()
            .find(|(id, _)| id == parent)
            .map(|(_, model)| model)
    }

    fn clear(&mut self) {
        self.model.clear_active();
        for (_, model) in &mut self.submenus {
            model.clear_active();
        }
    }
}

/// What a menu bar asks its model to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuBarEvent {
    /// Open menu `index`, its first entry highlighted.
    Open(usize),
    /// Open menu `index` without highlighting an entry (a pointer press).
    OpenUnhighlighted(usize),
    /// Close the open menu.
    Close,
    /// Open the menu `delta` places from the open one, wrapping at the ends.
    Switch(isize),
    /// Move the highlight in the open menu, or in its open submenu.
    Navigate(MenuMove),
    /// Highlight entry `id` (the pointer is over it).
    Highlight(EntryId),
    /// Choose entry `id`: run a command or open its submenu.
    Activate(EntryId),
    /// Open the submenu entry `id` leads to.
    OpenSubmenu(EntryId),
    /// Close the open submenu.
    CloseSubmenu,
}

/// The menus of a menu bar and which of them is open.
#[derive(Debug, Clone, Default)]
pub struct MenuBarModel {
    menus: Vec<MenuBarMenu>,
    open: Option<usize>,
    submenu: Option<EntryId>,
}

impl MenuBarModel {
    /// A menu bar of `menus`, all closed.
    #[must_use]
    pub fn new(menus: Vec<MenuBarMenu>) -> Self {
        Self {
            menus,
            open: None,
            submenu: None,
        }
    }

    /// The menus, in bar order.
    #[must_use]
    pub fn menus(&self) -> &[MenuBarMenu] {
        &self.menus
    }

    /// The open menu's index.
    #[must_use]
    pub fn open_menu(&self) -> Option<usize> {
        self.open
    }

    /// The entry whose submenu is open.
    #[must_use]
    pub fn open_submenu(&self) -> Option<&EntryId> {
        self.submenu.as_ref()
    }

    /// Applies `event`, returning the command entry the user chose, if any.
    /// Choosing a command closes the menu.
    pub fn apply(&mut self, event: &MenuBarEvent) -> Option<EntryId> {
        match event {
            MenuBarEvent::Open(index) => {
                self.open_at(*index, true);
                None
            }
            MenuBarEvent::OpenUnhighlighted(index) => {
                self.open_at(*index, false);
                None
            }
            MenuBarEvent::Close => {
                self.close();
                None
            }
            MenuBarEvent::Switch(delta) => {
                let (Some(open), Ok(count)) = (self.open, isize::try_from(self.menus.len())) else {
                    return None;
                };
                let index = isize::try_from(open).ok()? + delta;
                self.open_at(usize::try_from(index.rem_euclid(count)).ok()?, true);
                None
            }
            MenuBarEvent::Navigate(movement) => {
                let _ = self.level_mut()?.navigate(*movement);
                None
            }
            MenuBarEvent::Highlight(id) => {
                let _ = self.level_holding_mut(id)?.set_active(id);
                None
            }
            MenuBarEvent::Activate(id) => match self.level_holding_mut(id)?.activate(id) {
                Activation::Dispatch { id, .. } => {
                    self.close();
                    Some(id)
                }
                Activation::OpenSubmenu(id) => {
                    self.open_submenu_of(&id);
                    None
                }
                Activation::None => None,
            },
            MenuBarEvent::OpenSubmenu(id) => {
                self.open_submenu_of(id);
                None
            }
            MenuBarEvent::CloseSubmenu => {
                if let Some(parent) = self.submenu.take()
                    && let Some(model) = self.open_menu_mut()?.submenu_model_mut(&parent)
                {
                    model.clear_active();
                }
                None
            }
        }
    }

    /// The event a key press asks for, or `None` for a key the menu bar
    /// does not answer (it then reaches the rest of the window).
    #[must_use]
    pub fn event_for_key(
        &self,
        key: &iced_core::keyboard::Key,
        modifiers: iced_core::keyboard::Modifiers,
    ) -> Option<MenuBarEvent> {
        use crate::mnemonic::Mnemonic;
        use iced_core::keyboard::Key;
        use iced_core::keyboard::key::Named;
        if matches!(key, Key::Character(_)) && !modifiers.control() && !modifiers.logo() {
            // Alt and a title's access key open its menu.
            if modifiers.alt()
                && let Some(index) = self
                    .menus
                    .iter()
                    .position(|menu| Mnemonic::parse(menu.title()).matches(key))
            {
                return Some(MenuBarEvent::Open(index));
            }
            // In an open menu, an entry's access key chooses it.
            if let Some(level) = self.level()
                && let Some(entry) = level.entries().iter().find(|entry| {
                    entry.is_focusable() && Mnemonic::parse(entry.label()).matches(key)
                })
            {
                return Some(MenuBarEvent::Activate(entry.id().clone()));
            }
        }
        if modifiers.control() || modifiers.alt() || modifiers.logo() {
            return None;
        }
        let Key::Named(named) = key else {
            return None;
        };
        let Some(open) = self.open else {
            return (*named == Named::F10 && !self.menus.is_empty())
                .then_some(MenuBarEvent::Open(0));
        };
        let in_submenu = self.submenu.is_some();
        Some(match named {
            Named::F10 => MenuBarEvent::Close,
            Named::Escape if in_submenu => MenuBarEvent::CloseSubmenu,
            Named::Escape => MenuBarEvent::Close,
            Named::ArrowDown => MenuBarEvent::Navigate(MenuMove::Next),
            Named::ArrowUp => MenuBarEvent::Navigate(MenuMove::Previous),
            Named::Home => MenuBarEvent::Navigate(MenuMove::First),
            Named::End => MenuBarEvent::Navigate(MenuMove::Last),
            Named::Enter | Named::Space => MenuBarEvent::Activate(self.level()?.active()?.clone()),
            Named::ArrowLeft if in_submenu => MenuBarEvent::CloseSubmenu,
            Named::ArrowLeft => MenuBarEvent::Switch(-1),
            Named::ArrowRight if !in_submenu => {
                let menu = &self.menus[open];
                match menu.model.active().and_then(|id| menu.model.entry(id)) {
                    Some(entry)
                        if entry.kind() == EntryKind::Submenu
                            && menu.submenu_model(entry.id()).is_some() =>
                    {
                        MenuBarEvent::OpenSubmenu(entry.id().clone())
                    }
                    _ => MenuBarEvent::Switch(1),
                }
            }
            Named::ArrowRight => MenuBarEvent::Switch(1),
            _ => return None,
        })
    }

    fn open_at(&mut self, index: usize, highlight: bool) {
        if index >= self.menus.len() {
            return;
        }
        self.close();
        self.open = Some(index);
        if highlight {
            let _ = self.menus[index].model.navigate(MenuMove::First);
        }
    }

    fn close(&mut self) {
        if let Some(open) = self.open.take() {
            self.menus[open].clear();
        }
        self.submenu = None;
    }

    fn open_submenu_of(&mut self, parent: &EntryId) {
        let Some(menu) = self.open_menu_mut() else {
            return;
        };
        let _ = menu.model.set_active(parent);
        let Some(model) = menu.submenu_model_mut(parent) else {
            return;
        };
        let _ = model.navigate(MenuMove::First);
        self.submenu = Some(parent.clone());
    }

    fn open_menu_mut(&mut self) -> Option<&mut MenuBarMenu> {
        self.menus.get_mut(self.open?)
    }

    /// The menu level keys act on: the open submenu, else the open menu.
    fn level(&self) -> Option<&MenuModel> {
        let menu = self.menus.get(self.open?)?;
        match &self.submenu {
            Some(parent) => menu.submenu_model(parent),
            None => Some(&menu.model),
        }
    }

    fn level_mut(&mut self) -> Option<&mut MenuModel> {
        let submenu = self.submenu.clone();
        let menu = self.open_menu_mut()?;
        match submenu {
            Some(parent) => menu.submenu_model_mut(&parent),
            None => Some(&mut menu.model),
        }
    }

    /// The open level that holds entry `id`: the submenu, else the menu.
    fn level_holding_mut(&mut self, id: &EntryId) -> Option<&mut MenuModel> {
        let submenu = self.submenu.clone();
        let menu = self.open_menu_mut()?;
        if let Some(parent) = submenu
            && menu
                .submenu_model(&parent)
                .is_some_and(|model| model.entry(id).is_some())
        {
            return menu.submenu_model_mut(&parent);
        }
        menu.model.entry(id).is_some().then_some(&mut menu.model)
    }
}

#[cfg(test)]
mod tests;
