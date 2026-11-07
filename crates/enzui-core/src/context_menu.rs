//! Renderer-neutral context-menu invocation, viewport placement, and keyboard navigation.
//!
//! Point placement flips before overflowing a far edge and then clamps to the requested edge margin. Navigation wraps while skipping separators and disabled rows.

use crate::metrics::FiniteNonNeg;
use iced_core::keyboard::{self, key};

/// Returns whether a key press requests keyboard context-menu invocation.
///
/// The Menu key accepts any modifiers; F10 requires Shift, and plain F10 is not claimed.
#[must_use]
pub fn opens_context_menu(event: &keyboard::Event) -> bool {
    let keyboard::Event::KeyPressed {
        key: pressed,
        modifiers,
        ..
    } = event
    else {
        return false;
    };
    key_opens_context_menu(pressed, *modifiers)
}

/// Applies [`opens_context_menu`] binding rules to an already-decoded key press.
#[must_use]
pub fn key_opens_context_menu(pressed: &keyboard::Key, modifiers: keyboard::Modifiers) -> bool {
    match pressed {
        keyboard::Key::Named(key::Named::ContextMenu) => true,
        keyboard::Key::Named(key::Named::F10) => modifiers.shift(),
        _ => false,
    }
}

/// A renderer-independent rectangle in logical viewport coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
    /// Width.
    pub width: f32,
    /// Height.
    pub height: f32,
}

impl Rect {
    /// Construct a rectangle.
    #[must_use]
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// A point in logical viewport coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    /// Horizontal position.
    pub x: f32,
    /// Vertical position.
    pub y: f32,
}

impl Point {
    /// Construct a point.
    #[must_use]
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// Describes an invocation and the geometry used to choose its anchor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InvocationKind {
    /// Secondary click: anchor at the exact pointer position.
    Pointer(Point),
    /// Keyboard invocation anchored at the focused target’s bottom-left corner.
    Keyboard(Rect),
}

impl InvocationKind {
    /// The anchor point this invocation places the menu at.
    #[must_use]
    pub fn anchor(self) -> Point {
        match self {
            Self::Pointer(p) => p,
            Self::Keyboard(bounds) => Point::new(bounds.x, bounds.y + bounds.height),
        }
    }
}

/// Places a menu at `anchor`, flipping each axis on far-edge overflow and then clamping to `edge_margin`.
///
/// Non-finite menu or viewport extents, anchor coordinates, and margins are treated as zero.
#[must_use]
pub fn place_menu(
    anchor: Point,
    menu: (f32, f32),
    viewport: (f32, f32),
    edge_margin: f32,
) -> Point {
    let margin = FiniteNonNeg::new_or(edge_margin, 0.0).get();
    Point::new(
        place_axis(anchor.x, menu.0, viewport.0, margin),
        place_axis(anchor.y, menu.1, viewport.1, margin),
    )
}

/// Places a submenu beside `parent`, flipping horizontally across the parent when needed and clamping both axes.
///
/// Vertical placement begins at the parent’s top and never flips; oversized surfaces pin to the near margin.
#[must_use]
pub fn place_submenu(
    parent: Rect,
    menu: (f32, f32),
    viewport: (f32, f32),
    edge_margin: f32,
) -> Point {
    let margin = FiniteNonNeg::new_or(edge_margin, 0.0).get();
    let width = FiniteNonNeg::new_or(menu.0, 0.0).get();
    let height = FiniteNonNeg::new_or(menu.1, 0.0).get();
    let viewport_w = FiniteNonNeg::new_or(viewport.0, 0.0).get();
    let viewport_h = FiniteNonNeg::new_or(viewport.1, 0.0).get();
    let parent_x = if parent.x.is_finite() { parent.x } else { 0.0 };
    let parent_y = if parent.y.is_finite() { parent.y } else { 0.0 };
    let parent_w = FiniteNonNeg::new_or(parent.width, 0.0).get();

    let right = parent_x + parent_w;
    let x = if right + width <= viewport_w - margin {
        right
    } else {
        parent_x - width
    };

    Point::new(
        clamp_axis(x, width, viewport_w, margin),
        clamp_axis(parent_y, height, viewport_h, margin),
    )
}

fn clamp_axis(placed: f32, size: f32, viewport: f32, margin: f32) -> f32 {
    (viewport - size - margin).min(placed).max(margin)
}

fn place_axis(anchor: f32, size: f32, viewport: f32, margin: f32) -> f32 {
    let size = FiniteNonNeg::new_or(size, 0.0).get();
    let viewport = FiniteNonNeg::new_or(viewport, 0.0).get();
    let anchor = if anchor.is_finite() { anchor } else { 0.0 };

    let far_limit = viewport - size - margin;
    let placed = if anchor + size <= viewport - margin {
        anchor
    } else {
        anchor - size
    };

    placed.min(far_limit).max(margin)
}

/// The focusability shape of one context-menu entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuEntry {
    /// A command row that may be enabled or disabled.
    Item {
        /// Whether the row can currently be activated. Disabled rows are
        /// skipped by every navigation movement.
        enabled: bool,
    },
    /// A non-interactive divider. Always skipped.
    Separator,
}

impl MenuEntry {
    /// A selectable, enabled item.
    #[must_use]
    pub fn item() -> Self {
        Self::Item { enabled: true }
    }

    /// A present-but-unselectable item.
    #[must_use]
    pub fn disabled() -> Self {
        Self::Item { enabled: false }
    }

    /// Whether navigation may land on this entry.
    #[must_use]
    pub fn is_focusable(self) -> bool {
        matches!(self, Self::Item { enabled: true })
    }
}

/// A navigation movement requested by the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuMove {
    /// Moves to the next focusable entry.
    Next,
    /// Moves to the previous focusable entry.
    Previous,
    /// <kbd>Home</kbd>.
    First,
    /// <kbd>End</kbd>.
    Last,
}

/// Test-only deterministic tally of entries examined during navigation scans.
///
/// Incremented once per entry inspected in [`MenuNavigation::has_focusable`] and
/// [`MenuNavigation::scan_from`]. The store is thread-local so parallel tests
/// measure only their own scan work, and the whole module is `#[cfg(test)]`, so
/// no counter code or state exists in a non-test build.
#[cfg(test)]
pub(crate) mod scan_probe {
    use std::cell::Cell;

    thread_local! {
        static VISITED: Cell<usize> = const { Cell::new(0) };
    }

    /// Records that one entry was examined by a navigation scan.
    pub(crate) fn record() {
        VISITED.with(|v| v.set(v.get() + 1));
    }
}

/// Tracks focusable entries and the active row for one open menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuNavigation {
    entries: Vec<MenuEntry>,
    active: Option<usize>,
}

impl MenuNavigation {
    /// Creates navigation with no active row.
    ///
    /// The first `Next` selects the first focusable row, and the first `Previous` selects the last.
    #[must_use]
    pub fn new(entries: Vec<MenuEntry>) -> Self {
        Self {
            entries,
            active: None,
        }
    }

    /// The currently active row index, if any.
    #[must_use]
    pub fn active(&self) -> Option<usize> {
        self.active
    }

    /// Returns whether any entry can receive navigation focus.
    #[must_use]
    pub fn has_focusable(&self) -> bool {
        self.entries.iter().any(|e| {
            #[cfg(test)]
            scan_probe::record();
            e.is_focusable()
        })
    }

    /// Applies a movement with wrap-around, skipping disabled rows and separators.
    ///
    /// Returns `None` when no entry is focusable.
    pub fn apply(&mut self, movement: MenuMove) -> Option<usize> {
        if !self.has_focusable() {
            self.active = None;
            return None;
        }
        let n = self.entries.len();
        self.active = match movement {
            MenuMove::First => self.scan_from(0, 1),
            MenuMove::Last => self.scan_from(n - 1, -1),
            MenuMove::Next => match self.active {
                None => self.scan_from(0, 1),
                Some(i) => self.scan_from((i + 1) % n, 1),
            },
            MenuMove::Previous => match self.active {
                None => self.scan_from(n - 1, -1),
                Some(i) => self.scan_from((i + n - 1) % n, -1),
            },
        };
        self.active
    }

    /// Activates `index` if it is focusable and returns whether it was accepted.
    pub fn set_active(&mut self, index: usize) -> bool {
        if self.entries.get(index).is_some_and(|e| e.is_focusable()) {
            self.active = Some(index);
            true
        } else {
            false
        }
    }

    /// Clears the active row.
    pub fn clear(&mut self) {
        self.active = None;
    }

    fn scan_from(&self, start: usize, step: isize) -> Option<usize> {
        let n = self.entries.len();
        if n == 0 {
            return None;
        }
        let mut i = start;
        for _ in 0..n {
            #[cfg(test)]
            scan_probe::record();
            if self.entries[i].is_focusable() {
                return Some(i);
            }
            i = if step > 0 {
                (i + 1) % n
            } else {
                (i + n - 1) % n
            };
        }
        None
    }
}
