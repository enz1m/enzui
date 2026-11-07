//! Keyboard shortcut representation.
//!
//! A [`Shortcut`] pairs a set of modifier keys with a trigger key. This is
//! used by [`Action`](crate::Action) to declare keyboard bindings and by
//! the shortcut dispatch layer to match incoming key events.

use std::fmt::{self, Write as _};
use std::hash::{Hash, Hasher};

use iced_core::keyboard::{self, Key, Modifiers, key::Named, key::Physical};

/// Re-export iced's named logical keys for [`ShortcutKey::Named`].
pub use iced_core::keyboard::key::Named as NamedKey;

/// Re-export the immutable small-string type iced uses for logical character
/// keys, so callers can build a [`ShortcutKey::Character`] from a multi-scalar
/// key without depending on `smol_str` directly.
pub use iced_core::SmolStr;

/// A combinable set of keyboard modifier keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Modifier(u8);

impl Modifier {
    /// No modifiers.
    pub const NONE: Self = Self(0);
    /// The Control modifier.
    pub const CTRL: Self = Self(1 << 0);
    /// Alt.
    pub const ALT: Self = Self(1 << 1);
    /// Shift.
    pub const SHIFT: Self = Self(1 << 2);
    /// Super / Meta / Logo key.
    pub const SUPER: Self = Self(1 << 3);

    /// Returns true if `self` contains all flags in `other`.
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Stable bit representation used by shortcut-override persistence.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Rebuild a modifier set from its stable bit representation.
    /// Returns `None` when unknown future bits are present.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !(Self::CTRL.0 | Self::ALT.0 | Self::SHIFT.0 | Self::SUPER.0) == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    /// Converts iced's modifier state into an enzUI shortcut modifier set.
    pub fn from_iced(modifiers: Modifiers) -> Self {
        let mut result = Self::NONE;
        if modifiers.control() {
            result |= Self::CTRL;
        }
        if modifiers.alt() {
            result |= Self::ALT;
        }
        if modifiers.shift() {
            result |= Self::SHIFT;
        }
        if modifiers.logo() {
            result |= Self::SUPER;
        }
        result
    }

    /// Converts this modifier set into iced's modifier representation.
    pub fn to_iced(self) -> Modifiers {
        let mut result = Modifiers::NONE;
        if self.contains(Self::CTRL) {
            result |= Modifiers::CTRL;
        }
        if self.contains(Self::ALT) {
            result |= Modifiers::ALT;
        }
        if self.contains(Self::SHIFT) {
            result |= Modifiers::SHIFT;
        }
        if self.contains(Self::SUPER) {
            result |= Modifiers::LOGO;
        }
        result
    }
}

impl std::ops::BitOr for Modifier {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Modifier {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl fmt::Display for Modifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if self.contains(Self::CTRL) {
            parts.push("Ctrl");
        }
        if self.contains(Self::ALT) {
            parts.push("Alt");
        }
        if self.contains(Self::SHIFT) {
            parts.push("Shift");
        }
        if self.contains(Self::SUPER) {
            parts.push("Super");
        }
        write!(f, "{}", parts.join("+"))
    }
}

fn eq_ignore_case(expected: &str, actual: &str) -> bool {
    expected
        .chars()
        .flat_map(char::to_lowercase)
        .eq(actual.chars().flat_map(char::to_lowercase))
}

/// Returns the human-readable and speakable label for a named key, or `None` when it is not mapped.
pub fn named_key_label(key: NamedKey) -> Option<&'static str> {
    Some(match key {
        Named::Enter => "Enter",
        Named::Tab => "Tab",
        Named::Space => "Space",
        Named::Escape => "Esc",
        Named::ArrowUp => "Up",
        Named::ArrowDown => "Down",
        Named::ArrowLeft => "Left",
        Named::ArrowRight => "Right",
        Named::Home => "Home",
        Named::End => "End",
        Named::PageUp => "Page Up",
        Named::PageDown => "Page Down",
        Named::Backspace => "Backspace",
        Named::Delete => "Delete",
        Named::Insert => "Insert",
        Named::Clear => "Clear",
        Named::Copy => "Copy",
        Named::Cut => "Cut",
        Named::Paste => "Paste",
        Named::Undo => "Undo",
        Named::Redo => "Redo",
        Named::Select => "Select",
        Named::ContextMenu => "Context Menu",
        Named::Help => "Help",
        Named::Pause => "Pause",
        Named::Cancel => "Cancel",
        Named::Find => "Find",
        Named::Again => "Again",
        Named::Print => "Print",
        Named::PrintScreen => "Print Screen",
        Named::ZoomIn => "Zoom In",
        Named::ZoomOut => "Zoom Out",
        Named::Open => "Open",
        Named::Save => "Save",
        Named::New => "New",
        Named::Close => "Close",
        Named::Exit => "Exit",
        Named::SpellCheck => "Spell Check",
        Named::Shift => "Shift",
        Named::Control => "Ctrl",
        Named::Alt => "Alt",
        Named::AltGraph => "Alt Gr",
        Named::Meta => "Meta",
        Named::Super => "Super",
        Named::Hyper => "Hyper",
        Named::Fn => "Fn",
        Named::FnLock => "Fn Lock",
        Named::CapsLock => "Caps Lock",
        Named::NumLock => "Num Lock",
        Named::ScrollLock => "Scroll Lock",
        Named::AudioVolumeUp => "Volume Up",
        Named::AudioVolumeDown => "Volume Down",
        Named::AudioVolumeMute => "Mute",
        Named::MediaPlayPause => "Play/Pause",
        Named::MediaStop => "Stop",
        Named::MediaTrackNext => "Next Track",
        Named::MediaTrackPrevious => "Previous Track",
        Named::BrowserBack => "Browser Back",
        Named::BrowserForward => "Browser Forward",
        Named::BrowserRefresh => "Browser Refresh",
        Named::BrowserHome => "Browser Home",
        Named::BrowserSearch => "Browser Search",
        // Keep function-key labels explicit so display does not depend on upstream `Debug` formatting.
        Named::F1 => "F1",
        Named::F2 => "F2",
        Named::F3 => "F3",
        Named::F4 => "F4",
        Named::F5 => "F5",
        Named::F6 => "F6",
        Named::F7 => "F7",
        Named::F8 => "F8",
        Named::F9 => "F9",
        Named::F10 => "F10",
        Named::F11 => "F11",
        Named::F12 => "F12",
        Named::F13 => "F13",
        Named::F14 => "F14",
        Named::F15 => "F15",
        Named::F16 => "F16",
        Named::F17 => "F17",
        Named::F18 => "F18",
        Named::F19 => "F19",
        Named::F20 => "F20",
        Named::F21 => "F21",
        Named::F22 => "F22",
        Named::F23 => "F23",
        Named::F24 => "F24",
        _ => return None,
    })
}

/// A logical character string or named key that can trigger a shortcut.
///
/// Character keys may contain multiple Unicode scalars. Equality, hashing, and matching compare their per-scalar Unicode lowercase mappings.
#[derive(Debug, Clone)]
pub enum ShortcutKey {
    /// A locale-aware character key, as iced reports it.
    Character(SmolStr),
    /// A non-character iced logical key.
    Named(Named),
}

impl PartialEq for ShortcutKey {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            // Apply the same per-scalar Unicode lowercase mapping used by dispatch.
            (Self::Character(a), Self::Character(b)) => eq_ignore_case(a.as_str(), b.as_str()),
            (Self::Named(a), Self::Named(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for ShortcutKey {}

impl Hash for ShortcutKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::Character(key) => {
                // Hash a variant discriminant before the payload.
                0u8.hash(state);
                // Hash the per-scalar Unicode lowercase sequence used by equality.
                for scalar in key.chars().flat_map(char::to_lowercase) {
                    scalar.hash(state);
                }
                // Terminate the variable-length character payload to keep its hash encoding prefix-free.
                0xFFFF_FFFFu32.hash(state);
            }
            Self::Named(key) => {
                1u8.hash(state);
                key.hash(state);
            }
        }
    }
}

impl From<char> for ShortcutKey {
    fn from(value: char) -> Self {
        let mut buffer = [0u8; 4];
        Self::Character(SmolStr::new(value.encode_utf8(&mut buffer)))
    }
}

impl From<&str> for ShortcutKey {
    fn from(value: &str) -> Self {
        Self::Character(SmolStr::from(value))
    }
}

impl From<String> for ShortcutKey {
    fn from(value: String) -> Self {
        Self::Character(SmolStr::from(value))
    }
}

impl From<SmolStr> for ShortcutKey {
    fn from(value: SmolStr) -> Self {
        Self::Character(value)
    }
}

impl From<Named> for ShortcutKey {
    fn from(value: Named) -> Self {
        Self::Named(value)
    }
}

impl ShortcutKey {
    /// Returns the complete logical character string, or `None` for a named key.
    pub fn as_character(&self) -> Option<&str> {
        match self {
            Self::Character(key) => Some(key.as_str()),
            Self::Named(_) => None,
        }
    }

    /// Returns the named key, if this is a named key.
    pub fn as_named(&self) -> Option<Named> {
        match self {
            Self::Named(key) => Some(*key),
            Self::Character(_) => None,
        }
    }

    fn matches(&self, key: &Key) -> bool {
        match (self, key.as_ref()) {
            (Self::Named(expected), Key::Named(actual)) => *expected == actual,
            // Compare the complete logical string as one key.
            (Self::Character(expected), Key::Character(actual)) => {
                eq_ignore_case(expected.as_str(), actual)
            }
            _ => false,
        }
    }
}

impl fmt::Display for ShortcutKey {
    // Format labels for both display and speech; unmapped named keys fall back to their `Debug` spelling.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Uppercase every scalar directly into the formatter.
            Self::Character(key) => {
                for upper in key.chars().flat_map(char::to_uppercase) {
                    f.write_char(upper)?;
                }
                Ok(())
            }
            Self::Named(key) => match named_key_label(*key) {
                Some(label) => f.write_str(label),
                None => write!(f, "{key:?}"),
            },
        }
    }
}

/// A keyboard shortcut consisting of modifiers and a logical trigger key.
///
/// Character-key equality and hashing are case-insensitive, matching dispatch behavior.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Shortcut {
    modifiers: Modifier,
    key: ShortcutKey,
}

impl Shortcut {
    /// Creates a new shortcut.
    pub fn new(modifiers: Modifier, key: impl Into<ShortcutKey>) -> Self {
        Self {
            modifiers,
            key: key.into(),
        }
    }

    /// Formats the chord with localized key names while keeping `Display` canonical for persistence and parsing.
    #[must_use]
    pub fn localized_label(&self) -> String {
        let mut parts = Vec::new();
        for (modifier, key) in [
            (Modifier::CTRL, "enzui-key-control"),
            (Modifier::ALT, "enzui-key-alt"),
            (Modifier::SHIFT, "enzui-key-shift"),
            (Modifier::SUPER, "enzui-key-super"),
        ] {
            if self.modifiers.contains(modifier) {
                parts.push(crate::localization::text(key));
            }
        }
        let key = match self.key.as_named() {
            Some(key) if named_key_label(key).is_some() => {
                crate::localization::text(&format!("enzui-key-{key:?}").to_ascii_lowercase())
            }
            _ => self.key.to_string(),
        };
        parts.push(key);
        parts.join("+")
    }

    /// Returns the modifier keys.
    pub fn modifiers(&self) -> Modifier {
        self.modifiers
    }

    /// Borrows the logical trigger key.
    pub fn key(&self) -> &ShortcutKey {
        &self.key
    }

    /// Returns whether the logical key matches case-insensitively and the modifiers match exactly.
    pub fn matches(&self, key: &Key, modifiers: Modifiers) -> bool {
        self.modifiers == Modifier::from_iced(modifiers) && self.key.matches(key)
    }

    /// Like [`matches`](Self::matches), and on a non-Latin layout also by
    /// position: Ctrl+С on a Russian keyboard is the key where Ctrl+C sits,
    /// so it triggers a Ctrl+C shortcut. See [`latin_fallback`].
    pub fn matches_physical(
        &self,
        key: &Key,
        physical_key: Physical,
        modifiers: Modifiers,
    ) -> bool {
        self.matches(key, modifiers)
            || (self.modifiers == Modifier::from_iced(modifiers)
                && latin_fallback(key, physical_key).is_some_and(|latin| {
                    self.key.matches(&Key::Character(latin.to_string().into()))
                }))
    }

    /// Returns whether a non-repeated iced key-press event matches this
    /// shortcut, by logical key or, on a non-Latin layout, by position.
    pub fn matches_event(&self, event: &keyboard::Event) -> bool {
        match event {
            keyboard::Event::KeyPressed {
                key,
                modifiers,
                physical_key,
                repeat,
                ..
            } => !repeat && self.matches_physical(key, *physical_key, *modifiers),
            _ => false,
        }
    }
}

impl fmt::Display for Shortcut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.modifiers != Modifier::NONE {
            write!(f, "{}+{}", self.modifiers, self.key)
        } else {
            write!(f, "{}", self.key)
        }
    }
}

/// The Latin letter at a key's position when the layout produced a
/// non-Latin character there (Cyrillic, Greek, Hebrew, Arabic...), as GTK and
/// Qt do for shortcuts. `None` for Latin characters, so a Latin layout such
/// as AZERTY keeps its own letters, and for named keys.
#[must_use]
pub fn latin_fallback(key: &Key, physical_key: Physical) -> Option<char> {
    match key {
        Key::Character(text) if text.chars().next().is_some_and(|c| c >= '\u{370}') => {
            key.to_latin(physical_key)
        }
        _ => None,
    }
}

/// Returns whether `got` begins with the exact debug variant name `want`, followed only by a non-identifier boundary.
pub fn debug_name_is(got: &str, want: &str) -> bool {
    got.strip_prefix(want)
        .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
}

#[cfg(test)]
mod layout_tests {
    use super::*;
    use iced_core::keyboard::key::Code;

    fn press(text: &str, code: Code, modifiers: Modifiers) -> keyboard::Event {
        keyboard::Event::KeyPressed {
            key: Key::Character(text.into()),
            modified_key: Key::Character(text.into()),
            physical_key: Physical::Code(code),
            location: keyboard::Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        }
    }

    #[test]
    fn non_latin_layouts_match_by_position() {
        let copy = Shortcut::new(Modifier::CTRL, 'c');
        // Russian: the C key types "с" (Cyrillic es).
        assert!(copy.matches_event(&press("с", Code::KeyC, Modifiers::COMMAND)));
        // Greek: the C key types "ψ".
        assert!(copy.matches_event(&press("ψ", Code::KeyC, Modifiers::COMMAND)));
        // The position must be the C key, and the modifiers must match.
        assert!(!copy.matches_event(&press("с", Code::KeyV, Modifiers::COMMAND)));
        assert!(!copy.matches_event(&press("с", Code::KeyC, Modifiers::empty())));
    }

    #[test]
    fn latin_layouts_keep_their_own_letters() {
        // AZERTY: the key at the Q position types "a". Ctrl+A is the letter
        // the user pressed, not the position.
        let select_all = Shortcut::new(Modifier::CTRL, 'a');
        let quit = Shortcut::new(Modifier::CTRL, 'q');
        let azerty_a = press("a", Code::KeyQ, Modifiers::COMMAND);
        assert!(select_all.matches_event(&azerty_a));
        assert!(!quit.matches_event(&azerty_a));
        assert_eq!(
            latin_fallback(&Key::Character("é".into()), Physical::Code(Code::Digit2)),
            None
        );
        assert_eq!(
            latin_fallback(&Key::Named(Named::Enter), Physical::Code(Code::Enter)),
            None
        );
    }
}
