//! Editable shortcut defaults, overrides, validation, and persistence.
//!
//! One [`EditableShortcutCatalog`] supplies the resolved table used by dispatch, help, and settings.

use std::collections::{BTreeMap, BTreeSet};

use crate::persist::{PersistError, Persistable, SaveOp, Store};
use crate::{Modifier, NamedKey, Shortcut, ShortcutBinding, ShortcutKey};

/// Persisted primary-chord overrides keyed by stable action id.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShortcutOverrides {
    entries: BTreeMap<String, Shortcut>,
}

impl ShortcutOverrides {
    /// Returns the number of persisted override entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether no override entries are stored.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The override for a stable action id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Shortcut> {
        self.entries.get(id)
    }

    /// Iterate the persisted overrides in stable id order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Shortcut)> {
        self.entries.iter().map(|(id, chord)| (id.as_str(), chord))
    }
}

/// The app's validation decision for a captured chord.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShortcutValidity {
    /// The chord may be assigned if it does not conflict.
    Valid,
    /// The chord is structurally or contextually invalid.
    Invalid(String),
    /// The platform or app reserves this chord.
    Reserved(String),
}

/// A refused rebind. Every variant carries user-readable detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RebindRefusal {
    /// No catalog row has the supplied stable id.
    UnknownAction(String),
    /// The candidate cannot be represented or the app rejected it as invalid.
    Invalid {
        /// User-readable explanation.
        reason: String,
    },
    /// The app/platform marked the candidate reserved.
    Reserved {
        /// User-readable explanation.
        reason: String,
    },
    /// Another routed action already owns the chord, including as an alias.
    Conflict {
        /// Stable id of the colliding action.
        action_id: String,
        /// User-facing name of the colliding action.
        action_name: String,
    },
}

impl RebindRefusal {
    /// A complete message suitable for inline settings feedback.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::UnknownAction(id) => {
                crate::localized!("enzui-shortcut-unknown", "id" => id.as_str())
            }
            Self::Invalid { reason } => {
                crate::localized!("enzui-shortcut-invalid", "reason" => reason.as_str())
            }
            Self::Reserved { reason } => {
                crate::localized!("enzui-shortcut-reserved", "reason" => reason.as_str())
            }
            Self::Conflict { action_name, .. } => {
                crate::localized!("enzui-shortcut-conflict", "action" => action_name.as_str())
            }
        }
    }
}

/// A settings row borrowed from the authoritative resolved shortcut table.
#[derive(Clone, Copy, Debug)]
pub struct EditableShortcutRow<'a> {
    /// Stable action id.
    pub id: &'static str,
    /// User-facing action name.
    pub name: &'a str,
    /// Settings/help section.
    pub section: &'a str,
    /// Currently resolved primary chord.
    pub primary: &'a Shortcut,
    /// Aliases retained from the app's catalog.
    pub alternates: &'a [Shortcut],
    /// Whether this action currently has a primary override.
    pub overridden: bool,
}

/// The single owner of defaults, overrides, and the live resolved dispatch table.
pub struct EditableShortcutCatalog<Command, Context = ()> {
    defaults: Vec<ShortcutBinding<Command, Context>>,
    resolved: Vec<ShortcutBinding<Command, Context>>,
    overrides: ShortcutOverrides,
    save_error: Option<String>,
}

impl<Command: Clone, Context> EditableShortcutCatalog<Command, Context> {
    /// Build a catalog with no user overrides.
    #[must_use]
    pub fn new(defaults: Vec<ShortcutBinding<Command, Context>>) -> Self {
        Self::from_overrides(defaults, ShortcutOverrides::default())
    }

    /// Builds a catalog from persisted overrides, discarding unknown ids and values equal to current defaults.
    #[must_use]
    pub fn from_overrides(
        defaults: Vec<ShortcutBinding<Command, Context>>,
        mut overrides: ShortcutOverrides,
    ) -> Self {
        let known: BTreeSet<&str> = defaults.iter().map(ShortcutBinding::id).collect();
        overrides
            .entries
            .retain(|id, _| known.contains(id.as_str()));
        for binding in &defaults {
            if overrides.get(binding.id()) == Some(binding.primary()) {
                overrides.entries.remove(binding.id());
            }
        }
        let resolved = resolve(&defaults, &overrides);
        Self {
            defaults,
            resolved,
            overrides,
            save_error: None,
        }
    }

    /// The authoritative table for dispatch, labels, and read-only help.
    pub fn resolved(&self) -> &[ShortcutBinding<Command, Context>] {
        &self.resolved
    }

    /// The app-supplied defaults.
    pub fn defaults(&self) -> &[ShortcutBinding<Command, Context>] {
        &self.defaults
    }

    /// Returns the catalog-owned persisted override map.
    #[must_use]
    pub fn overrides(&self) -> &ShortcutOverrides {
        &self.overrides
    }

    /// Rows for the editable surface, borrowed from [`Self::resolved`].
    pub fn rows(&self) -> impl Iterator<Item = EditableShortcutRow<'_>> {
        self.resolved.iter().map(|binding| EditableShortcutRow {
            id: binding.id(),
            name: binding.description(),
            section: binding.section(),
            primary: binding.primary(),
            alternates: binding.alternates(),
            overridden: self.overrides.get(binding.id()).is_some(),
        })
    }

    /// Applies a candidate only after persistability, validity, and alias-inclusive conflict checks pass.
    ///
    /// Refusals leave the catalog unchanged and identify any action owning the chord.
    ///
    /// # Errors
    ///
    /// Returns [`RebindRefusal`] for an unknown action, an unpersistable or rejected candidate, a reserved chord, or a conflict.
    pub fn try_rebind(
        &mut self,
        action: &str,
        candidate: Shortcut,
        validate: impl FnOnce(&Shortcut) -> ShortcutValidity,
    ) -> Result<(), RebindRefusal> {
        let default = self
            .defaults
            .iter()
            .find(|b| b.id() == action)
            .ok_or_else(|| RebindRefusal::UnknownAction(action.to_owned()))?;

        if encode_shortcut(&candidate).is_none() {
            return Err(RebindRefusal::Invalid {
                reason: "this logical key cannot be persisted".to_owned(),
            });
        }
        match validate(&candidate) {
            ShortcutValidity::Valid => {}
            ShortcutValidity::Invalid(reason) => return Err(RebindRefusal::Invalid { reason }),
            ShortcutValidity::Reserved(reason) => return Err(RebindRefusal::Reserved { reason }),
        }
        if let Some(owner) = self.resolved.iter().find(|binding| {
            binding.id() != action && binding.shortcuts().any(|chord| chord == &candidate)
        }) {
            return Err(RebindRefusal::Conflict {
                action_id: owner.id().to_owned(),
                action_name: owner.description().to_owned(),
            });
        }

        if &candidate == default.primary() {
            self.overrides.entries.remove(action);
        } else {
            self.overrides.entries.insert(action.to_owned(), candidate);
        }
        self.rebuild();
        Ok(())
    }

    /// Reset one primary chord to the app-supplied default.
    /// Returns whether an override was removed.
    pub fn reset(&mut self, action: &str) -> bool {
        let changed = self.overrides.entries.remove(action).is_some();
        if changed {
            self.rebuild();
        }
        changed
    }

    /// Reset every primary chord to the app-supplied defaults.
    /// Returns whether any override was removed.
    pub fn reset_all(&mut self) -> bool {
        let changed = !self.overrides.is_empty();
        self.overrides.entries.clear();
        if changed {
            self.rebuild();
        }
        changed
    }

    /// Prepares the current overrides for off-thread persistence.
    ///
    /// # Errors
    ///
    /// Returns [`PersistError`] if an override cannot be serialized or the store refuses save admission.
    pub fn prepare_save(&self, store: &Store<ShortcutOverrides>) -> Result<SaveOp, PersistError> {
        store.prepare_save(&self.overrides)
    }

    /// Records the off-thread save result for retrieval through [`Self::save_error`].
    pub fn finish_save(&mut self, result: Result<(), PersistError>) {
        self.save_error = result.err().map(|error| error.user_summary());
    }

    /// Last persistence error, if any, for visible settings feedback.
    #[must_use]
    pub fn save_error(&self) -> Option<&str> {
        self.save_error.as_deref()
    }

    fn rebuild(&mut self) {
        self.resolved = resolve(&self.defaults, &self.overrides);
    }
}

fn resolve<Command: Clone, Context>(
    defaults: &[ShortcutBinding<Command, Context>],
    overrides: &ShortcutOverrides,
) -> Vec<ShortcutBinding<Command, Context>> {
    defaults
        .iter()
        .cloned()
        .map(|binding| match overrides.get(binding.id()) {
            Some(chord) => binding.with_primary(chord.clone()),
            None => binding,
        })
        .collect()
}

// A line-oriented, hex-escaped format deliberately ignores malformed and
// future fields one line at a time. One broken override cannot erase another.
impl Persistable for ShortcutOverrides {
    fn persist_serialize(&self) -> Result<String, String> {
        let mut body = String::from("enzui-shortcut-overrides-v1\n");
        for (id, shortcut) in &self.entries {
            let Some(encoded) = encode_shortcut(shortcut) else {
                return Err(format!("shortcut for {id} cannot be persisted"));
            };
            body.push_str("override\t");
            body.push_str(&hex(id.as_bytes()));
            body.push('\t');
            body.push_str(&encoded);
            body.push('\n');
        }
        Ok(body)
    }

    fn persist_deserialize(body: &str) -> Result<Self, String> {
        deserialize_with_limits(body, crate::atomic_state::LineBodyLimits::PRODUCTION)
    }

    fn persist_default() -> Self {
        Self::default()
    }
}

fn deserialize_with_limits(
    body: &str,
    limits: crate::atomic_state::LineBodyLimits,
) -> Result<ShortcutOverrides, String> {
    const FORMAT_LINE: &str = "enzui-shortcut-overrides-v1";
    let records = match body.split_once('\n') {
        Some((first, rest)) if first.strip_suffix('\r').unwrap_or(first) == FORMAT_LINE => rest,
        None if body.strip_suffix('\r').unwrap_or(body) == FORMAT_LINE => "",
        _ => body,
    };
    let mut entries = BTreeMap::new();
    crate::atomic_state::visit_line_body(records, limits, |line| {
        if let Some((id, shortcut)) = decode_override_line(line) {
            entries.insert(id, shortcut);
        }
        Ok::<(), String>(())
    })
    .map_err(|error| error.to_string())?;
    Ok(ShortcutOverrides { entries })
}

fn decode_override_line(line: &str) -> Option<(String, Shortcut)> {
    #[cfg(test)]
    SHORTCUT_LINE_DECODES.with(|count| count.set(count.get() + 1));

    let mut fields = line.split('\t');
    if fields.next() != Some("override") {
        return None;
    }
    let (Some(id), Some(mods), Some(kind), Some(key)) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return None;
    };
    let (Some(id), Ok(mods), Some(key)) = (unhex_text(id), mods.parse::<u8>(), unhex_text(key))
    else {
        return None;
    };
    if fields.next().is_some() {
        return None;
    }
    decode_shortcut(mods, kind, &key).map(|shortcut| (id, shortcut))
}

#[cfg(test)]
thread_local! {
    static SHORTCUT_LINE_DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn encode_shortcut(shortcut: &Shortcut) -> Option<String> {
    let (kind, key) = match shortcut.key() {
        ShortcutKey::Character(key) => ("c", key.as_str().to_owned()),
        ShortcutKey::Named(named) => ("n", named_key_storage(*named)?.to_owned()),
    };
    Some(format!(
        "{}\t{kind}\t{}",
        shortcut.modifiers().bits(),
        hex(key.as_bytes())
    ))
}

fn decode_shortcut(mods: u8, kind: &str, key: &str) -> Option<Shortcut> {
    let modifiers = Modifier::from_bits(mods)?;
    let key = match kind {
        "c" if !key.is_empty() => ShortcutKey::from(key),
        "n" => ShortcutKey::from(named_key_from_storage(key)?),
        _ => return None,
    };
    Some(Shortcut::new(modifiers, key))
}

fn named_key_storage(key: NamedKey) -> Option<&'static str> {
    crate::shortcut::named_key_label(key)
}

fn named_key_from_storage(label: &str) -> Option<NamedKey> {
    use NamedKey::*;
    Some(match label {
        "Enter" => Enter,
        "Tab" => Tab,
        "Space" => Space,
        "Esc" => Escape,
        "Up" => ArrowUp,
        "Down" => ArrowDown,
        "Left" => ArrowLeft,
        "Right" => ArrowRight,
        "Home" => Home,
        "End" => End,
        "Page Up" => PageUp,
        "Page Down" => PageDown,
        "Backspace" => Backspace,
        "Delete" => Delete,
        "Insert" => Insert,
        "Clear" => Clear,
        "Copy" => Copy,
        "Cut" => Cut,
        "Paste" => Paste,
        "Undo" => Undo,
        "Redo" => Redo,
        "Select" => Select,
        "Context Menu" => ContextMenu,
        "Help" => Help,
        "Pause" => Pause,
        "Cancel" => Cancel,
        "Find" => Find,
        "Again" => Again,
        "Print" => Print,
        "Print Screen" => PrintScreen,
        "Zoom In" => ZoomIn,
        "Zoom Out" => ZoomOut,
        "Open" => Open,
        "Save" => Save,
        "New" => New,
        "Close" => Close,
        "Exit" => Exit,
        "Spell Check" => SpellCheck,
        "Shift" => Shift,
        "Ctrl" => Control,
        "Alt" => Alt,
        "Alt Gr" => AltGraph,
        "Meta" => Meta,
        "Super" => Super,
        "Hyper" => Hyper,
        "Fn" => Fn,
        "Fn Lock" => FnLock,
        "Caps Lock" => CapsLock,
        "Num Lock" => NumLock,
        "Scroll Lock" => ScrollLock,
        "Volume Up" => AudioVolumeUp,
        "Volume Down" => AudioVolumeDown,
        "Mute" => AudioVolumeMute,
        "Play/Pause" => MediaPlayPause,
        "Stop" => MediaStop,
        "Next Track" => MediaTrackNext,
        "Previous Track" => MediaTrackPrevious,
        "Browser Back" => BrowserBack,
        "Browser Forward" => BrowserForward,
        "Browser Refresh" => BrowserRefresh,
        "Browser Home" => BrowserHome,
        "Browser Search" => BrowserSearch,
        "F1" => F1,
        "F2" => F2,
        "F3" => F3,
        "F4" => F4,
        "F5" => F5,
        "F6" => F6,
        "F7" => F7,
        "F8" => F8,
        "F9" => F9,
        "F10" => F10,
        "F11" => F11,
        "F12" => F12,
        "F13" => F13,
        "F14" => F14,
        "F15" => F15,
        "F16" => F16,
        "F17" => F17,
        "F18" => F18,
        "F19" => F19,
        "F20" => F20,
        "F21" => F21,
        "F22" => F22,
        "F23" => F23,
        "F24" => F24,
        _ => return None,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex_text(value: &str) -> Option<String> {
    // Decode bytes rather than character slices so malformed multibyte fields are rejected without panicking.
    let raw = value.as_bytes();
    if !raw.len().is_multiple_of(2) {
        return None;
    }
    let bytes: Option<Vec<u8>> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let hi = (pair[0] as char).to_digit(16)?;
            let lo = (pair[1] as char).to_digit(16)?;
            Some((hi * 16 + lo) as u8)
        })
        .collect();
    String::from_utf8(bytes?).ok()
}

#[cfg(test)]
mod tests;
