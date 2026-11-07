//! Retained focus, popup and keyed highlight state.

use super::*;

#[derive(Debug, Clone, Default)]
pub(crate) struct ComboState {
    pub(crate) focus: enzui_core::focus::FocusState,
    pub(crate) open: bool,
    pub(crate) active: Option<usize>,
    pub(crate) active_key: Option<String>,
    pub(crate) pressed_row: Option<usize>,
    pub(crate) hovered_row: Option<usize>,
    pub(crate) keyboard_navigation: bool,
    pub(crate) window_key: u64,
    /// The search query the list was last filtered by.
    pub(crate) query: String,
    /// The list unrolling as it opens.
    pub(crate) reveal: crate::anchored_overlay::PopupReveal,
}

impl iced::advanced::widget::operation::Focusable for ComboState {
    fn is_focused(&self) -> bool {
        self.focus.is_focused()
    }
    fn focus(&mut self) {
        self.focus.focus();
    }
    fn unfocus(&mut self) {
        self.focus.unfocus();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ComboKey {
    Open { last: bool },
    Move(MenuMove),
    Commit,
    Dismiss,
    Swallow,
}

pub(crate) fn key_for_combo(open: bool, event: &iced::keyboard::Event) -> Option<ComboKey> {
    use enzui_core::menu_model::{MenuKey, key_for_open_menu, open_cause_for_key};

    if open {
        return match key_for_open_menu(event)? {
            MenuKey::Move(movement) => Some(ComboKey::Move(movement)),
            MenuKey::Activate => Some(ComboKey::Commit),
            MenuKey::Dismiss => Some(ComboKey::Dismiss),
            // Swallow submenu keys so an open selection popup does not navigate the background.
            MenuKey::OpenSubmenu | MenuKey::CloseSubmenu => Some(ComboKey::Swallow),
        };
    }

    use enzui_core::menu_model::OpenCause;
    match open_cause_for_key(event)? {
        OpenCause::KeyboardLast => Some(ComboKey::Open { last: true }),
        OpenCause::KeyboardFirst => Some(ComboKey::Open { last: false }),
        OpenCause::Pointer => None,
    }
}
