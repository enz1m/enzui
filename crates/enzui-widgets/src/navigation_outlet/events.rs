//! Back eligibility and Escape decoding for navigation outlets.

use super::*;

/// What a key press means to an open outlet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NavigationEvent {
    /// Go back one level.
    Pop,
}

/// Decode Escape when both policy flags are true and the press is not a repeat.
/// All other keys return `None`. This helper does not inspect modifiers or child
/// event capture; the outlet adapter gives children first refusal.
#[must_use]
pub fn event_for_key_repeat(
    key: &iced::keyboard::Key,
    pop_on_escape: bool,
    back_enabled: bool,
    repeat: bool,
) -> Option<NavigationEvent> {
    if repeat {
        return None;
    }
    if !pop_on_escape || !back_enabled {
        return None;
    }
    match key {
        iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape) => {
            Some(NavigationEvent::Pop)
        }
        _ => None,
    }
}

/// [`event_for_key_repeat`] for an initial (non-repeat) press.
#[must_use]
pub fn event_for_key(
    key: &iced::keyboard::Key,
    pop_on_escape: bool,
    back_enabled: bool,
) -> Option<NavigationEvent> {
    event_for_key_repeat(key, pop_on_escape, back_enabled, false)
}

/// Whether the path has a parent and the caller allows back navigation.
/// This helper does not inspect header visibility or a pop callback.
#[must_use]
pub fn back_enabled(path: &RoutePath, can_pop: bool) -> bool {
    can_pop && path.can_pop()
}
