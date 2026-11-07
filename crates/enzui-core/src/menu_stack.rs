//! Generation-scoped stack of nested menu child levels.
//!
//! The host owns the root model, so depth zero means no child submenu. Checked operations reject a different showing generation, and each child retains its own [`MenuModel`] and highlight.

use crate::context_menu::MenuMove;
use crate::menu_model::{Activation, EntryId, MenuModel};
use crate::table::Generation;

/// One child menu level, including its parent row and independent model state.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuLevel {
    parent: EntryId,
    model: MenuModel,
}

impl MenuLevel {
    /// Build a level for `parent`'s submenu.
    #[must_use]
    pub fn new(parent: EntryId, model: MenuModel) -> Self {
        Self { parent, model }
    }

    /// The row, in the level above, that opened this one.
    #[must_use]
    pub fn parent(&self) -> &EntryId {
        &self.parent
    }

    /// This level's rows and highlight.
    #[must_use]
    pub fn model(&self) -> &MenuModel {
        &self.model
    }

    /// Mutable access, for navigation and highlighting within this level.
    pub fn model_mut(&mut self) -> &mut MenuModel {
        &mut self.model
    }
}

/// A submenu event whose generation does not match the current showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaleMenu {
    /// The generation the event was stamped with.
    pub event: Generation,
    /// The generation the stack currently holds.
    pub current: Generation,
}

impl core::fmt::Display for StaleMenu {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "stale submenu event: generation {} != current generation {}",
            self.event.raw(),
            self.current.raw()
        )
    }
}

impl std::error::Error for StaleMenu {}

/// Child submenu levels and generation for one showing of a root menu.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MenuStack {
    levels: Vec<MenuLevel>,
    generation: Generation,
}

impl MenuStack {
    /// Creates an empty stack at [`Generation::FIRST`], which is a valid initial showing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The generation of the showing currently represented.
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// Clears child levels, advances the showing generation, and returns the value callers must stamp on new events.
    pub fn reset(&mut self) -> Generation {
        self.levels.clear();
        self.generation = self.generation.next();
        self.generation
    }

    /// Whether `generation` names the showing this stack currently represents.
    #[must_use]
    pub fn is_current(&self, generation: Generation) -> bool {
        self.generation == generation
    }

    /// Returns the number of open child levels; zero means only the host-owned root remains.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.levels.len()
    }

    /// Every open child level, root-most first.
    #[must_use]
    pub fn levels(&self) -> &[MenuLevel] {
        &self.levels
    }

    /// Mutable access to every child level, including parent levels while a deeper child is open.
    pub fn levels_mut(&mut self) -> &mut [MenuLevel] {
        &mut self.levels
    }

    /// The deepest open child level, if any.
    #[must_use]
    pub fn deepest(&self) -> Option<&MenuLevel> {
        self.levels.last()
    }

    /// The deepest open child level, mutably — the level keyboard navigation
    /// applies to while a submenu is open.
    pub fn deepest_mut(&mut self) -> Option<&mut MenuLevel> {
        self.levels.last_mut()
    }

    /// Returns whether `parent` owns the open submenu at `parent_depth`; ids may repeat at different depths.
    #[must_use]
    pub fn is_open_for(&self, parent_depth: usize, parent: &EntryId) -> bool {
        self.levels
            .get(parent_depth)
            .is_some_and(|level| &level.parent == parent)
    }

    /// Opens `model` below `parent` at `parent_depth`, where zero is the root.
    ///
    /// Deeper levels are removed first. Returns `false` for an unavailable depth or an already-open identical submenu.
    pub fn open_submenu(&mut self, parent_depth: usize, parent: EntryId, model: MenuModel) -> bool {
        if parent_depth > self.levels.len() {
            return false;
        }
        if self.is_open_for(parent_depth, &parent) {
            return false;
        }
        self.levels.truncate(parent_depth);
        self.levels.push(MenuLevel::new(parent, model));
        true
    }

    /// Opens a submenu only when `generation` matches the current showing.
    ///
    /// # Errors
    ///
    /// Returns [`StaleMenu`] when `generation` is not current.
    pub fn open_submenu_checked(
        &mut self,
        generation: Generation,
        parent_depth: usize,
        parent: EntryId,
        model: MenuModel,
    ) -> Result<bool, StaleMenu> {
        if !self.is_current(generation) {
            return Err(StaleMenu {
                event: generation,
                current: self.generation,
            });
        }
        Ok(self.open_submenu(parent_depth, parent, model))
    }

    /// Closes the deepest child level and returns its parent id, or `None` when only the root remains.
    pub fn close_submenu(&mut self) -> Option<EntryId> {
        self.levels.pop().map(|level| level.parent)
    }

    /// Closes the deepest submenu only when `generation` matches the current showing.
    ///
    /// # Errors
    ///
    /// Returns [`StaleMenu`] when `generation` is not current.
    pub fn close_submenu_checked(
        &mut self,
        generation: Generation,
    ) -> Result<Option<EntryId>, StaleMenu> {
        if !self.is_current(generation) {
            return Err(StaleMenu {
                event: generation,
                current: self.generation,
            });
        }
        Ok(self.close_submenu())
    }

    /// Closes every child level without advancing the showing generation, returning whether anything changed.
    pub fn close_all(&mut self) -> bool {
        let had = !self.levels.is_empty();
        self.levels.clear();
        had
    }

    /// Applies movement to the deepest child, returning `None` when the host-owned root must handle it.
    pub fn navigate_deepest(&mut self, movement: MenuMove) -> Option<Option<EntryId>> {
        self.deepest_mut()
            .map(|level| level.model_mut().navigate(movement))
    }

    /// Activates the deepest child's active row.
    ///
    /// `None` means no child is open; `Some(Activation::None)` means the open child produced no activation.
    #[must_use]
    pub fn activate_deepest(&self) -> Option<Activation> {
        self.deepest().map(|level| level.model().activate_active())
    }
}
