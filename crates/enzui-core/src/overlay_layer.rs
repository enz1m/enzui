//! Named z-order classes for overlays.
//!
//! Tooltip, popup, menu, modal, and system indices increase by `2.0`; the tooltip index remains below iced’s default `1.0`.

/// A named overlay z-order class; higher classes paint above lower classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OverlayLayer {
    /// Tooltip bubbles at the lowest named layer.
    Tooltip,
    /// Dropdowns and attached popovers, above tooltips and below menus.
    Popup,
    /// Context menus, menu bars, and submenu stacks, above popups and below modals.
    Menu,
    /// Modal dialogs and blocking sheets, above the named tooltip, popup, and menu classes.
    Modal,
    /// Full-screen system and diagnostic surfaces at the highest named layer.
    System,
}

impl OverlayLayer {
    /// Returns the z-index, spaced by `2.0` to allow within-class offsets smaller than `2.0`.
    #[must_use]
    pub const fn z_index(self) -> f32 {
        match self {
            Self::Tooltip => 0.0,
            Self::Popup => 2.0,
            Self::Menu => 4.0,
            Self::Modal => 6.0,
            Self::System => 8.0,
        }
    }
}
