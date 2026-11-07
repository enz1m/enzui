//! Whether Caps Lock is on, as the last keyboard event said.
//!
//! The window reports Caps Lock with
//! `iced::keyboard::Event::CapsLockChanged` on Wayland and X11, only when it
//! changes; a widget mounted later still needs the current state, so
//! [`observe`] keeps it process-wide and [`caps_lock`] reads it. Where the
//! platform does not report it, a typed letter whose case disagrees with
//! Shift reveals it. Secure text fields use this for their Caps Lock warning.

use iced_core::keyboard::{self, Key};
use std::sync::atomic::{AtomicBool, Ordering};

static CAPS_LOCK: AtomicBool = AtomicBool::new(false);

/// Whether Caps Lock is on, as far as the events seen so far tell.
#[must_use]
pub fn caps_lock() -> bool {
    CAPS_LOCK.load(Ordering::Relaxed)
}

/// What `event` says about Caps Lock: the platform's report, or a typed
/// letter whose case disagrees (or agrees) with Shift. `None` when it says
/// nothing.
#[must_use]
pub fn caps_lock_from(event: &keyboard::Event) -> Option<bool> {
    match event {
        keyboard::Event::CapsLockChanged(on) => Some(*on),
        keyboard::Event::KeyPressed {
            text: Some(text),
            modifiers,
            key: Key::Character(_),
            ..
        } => {
            let mut letters = text.chars().filter(|c| c.is_alphabetic());
            let (Some(letter), None) = (letters.next(), letters.next()) else {
                return None;
            };
            if letter.is_uppercase() == letter.is_lowercase() {
                return None;
            }
            // Upper case without Shift, or lower case with it.
            Some(letter.is_uppercase() != modifiers.shift())
        }
        _ => None,
    }
}

/// Updates the state from `event`; returns what the event says.
pub fn observe(event: &keyboard::Event) -> Option<bool> {
    let on = caps_lock_from(event)?;
    CAPS_LOCK.store(on, Ordering::Relaxed);
    Some(on)
}
