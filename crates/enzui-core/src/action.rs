//! Typed user-facing actions and context-aware keyboard shortcut bindings.
//!
//! `Action` carries one message and optional shortcut. `ShortcutBinding` adds
//! aliases, help metadata, and a live-context predicate for application catalogs.

use crate::Icon;
use crate::shortcut::Shortcut;
use iced_core::keyboard::{self, Key, Modifiers};

/// A named user-facing action with presentation metadata and a typed message.
#[derive(Debug, Clone)]
#[must_use]
pub struct Action<Message> {
    id: &'static str,
    message: Message,
    label: String,
    icon: Option<Icon>,
    shortcut: Option<Shortcut>,
    enabled: bool,
}

impl<Message> Action<Message> {
    /// Creates an enabled action with an ID-derived label and no icon or shortcut.
    pub fn new(id: &'static str, message: Message) -> Self {
        Self {
            id,
            message,
            label: label_fallback(id),
            icon: None,
            shortcut: None,
            enabled: true,
        }
    }

    /// Sets the display label (builder).
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        let label = label.into();
        self.label = if label.trim().is_empty() {
            label_fallback(self.id)
        } else {
            label
        };
        self
    }

    /// Sets the icon (builder).
    pub fn with_icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Sets the keyboard shortcut (builder).
    pub fn with_shortcut(mut self, shortcut: Shortcut) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    /// Sets whether the action is enabled (builder).
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Returns the stable ID.
    pub fn id(&self) -> &'static str {
        self.id
    }

    /// Returns the display label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns the icon, if set.
    pub fn icon(&self) -> Option<&Icon> {
        self.icon.as_ref()
    }

    /// Returns the keyboard shortcut, if set.
    pub fn shortcut(&self) -> Option<&Shortcut> {
        self.shortcut.as_ref()
    }

    /// Returns whether the action is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Returns a reference to the message payload.
    pub fn message(&self) -> &Message {
        &self.message
    }

    /// Consumes the action and returns the message payload.
    pub fn into_message(self) -> Message {
        self.message
    }

    /// Returns whether an iced key press should trigger this action, by
    /// logical key only. Prefer [`matches_event`](Self::matches_event), which
    /// also matches on non-Latin layouts by key position.
    pub fn matches_key(&self, key: &Key, modifiers: Modifiers) -> bool {
        self.enabled
            && self
                .shortcut
                .as_ref()
                .is_some_and(|shortcut| shortcut.matches(key, modifiers))
    }

    /// Returns whether an iced keyboard event should trigger this action.
    pub fn matches_event(&self, event: &keyboard::Event) -> bool {
        self.enabled
            && self
                .shortcut
                .as_ref()
                .is_some_and(|shortcut| shortcut.matches_event(event))
    }

    /// Maps the message type to a different type.
    pub fn map_message<B>(self, f: impl FnOnce(Message) -> B) -> Action<B> {
        Action {
            id: self.id,
            message: f(self.message),
            label: self.label,
            icon: self.icon,
            shortcut: self.shortcut,
            enabled: self.enabled,
        }
    }
}

/// Returns the first enabled action matching a key and modifiers.
///
/// Duplicate shortcuts resolve in iterator order.
pub fn action_for_key<'a, Message: 'a>(
    actions: impl IntoIterator<Item = &'a Action<Message>>,
    key: &Key,
    modifiers: Modifiers,
) -> Option<&'a Action<Message>> {
    actions
        .into_iter()
        .find(|action| action.matches_key(key, modifiers))
}

/// Returns the first enabled action matching a non-repeat key press.
///
/// Other event kinds are rejected before the action iterator is visited.
pub fn action_for_event<'a, Message: 'a>(
    actions: impl IntoIterator<Item = &'a Action<Message>>,
    event: &keyboard::Event,
) -> Option<&'a Action<Message>> {
    let keyboard::Event::KeyPressed { repeat: false, .. } = event else {
        return None;
    };
    actions
        .into_iter()
        .find(|action| action.matches_event(event))
}

/// Clones the message from the first action matching a non-repeat key press.
///
/// Releases, modifier changes, and repeat presses return `None` without scanning
/// the action iterator.
pub fn dispatch_shortcut<'a, Message: Clone + 'a>(
    actions: impl IntoIterator<Item = &'a Action<Message>>,
    event: &keyboard::Event,
) -> Option<Message> {
    let keyboard::Event::KeyPressed { repeat: false, .. } = event else {
        return None;
    };
    actions
        .into_iter()
        .find_map(|action| action.matches_event(event).then(|| action.message.clone()))
}

/// Widget-capture and modal-ownership context for shortcut predicates.
///
/// Global rows are disabled when either owner is present. Capture alone is
/// insufficient: a focused dialog button can leave a bare key uncaptured.
/// Derive modal ownership from the active presentation, such as
/// [`ModalStack::blocks_background`](crate::modal::ModalStack::blocks_background).
/// Rows without a scope predicate keep their own dispatch policy.
///
/// ```
/// use enzui_core::action::KeyRouteScope;
///
/// assert!(KeyRouteScope::is_global(&KeyRouteScope::shell()));
/// assert!(!KeyRouteScope::is_global(&KeyRouteScope::new(true, false)));
/// assert!(!KeyRouteScope::is_global(&KeyRouteScope::new(false, true)));
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyRouteScope {
    captured: bool,
    modal_owns_keyboard: bool,
}

impl KeyRouteScope {
    /// A scope from the two ownership facts: whether a focused widget consumed
    /// the press, and whether a live presentation makes the background inert.
    #[must_use]
    pub const fn new(captured: bool, modal_owns_keyboard: bool) -> Self {
        Self {
            captured,
            modal_owns_keyboard,
        }
    }

    /// The scope of a press nothing else owns: no capture, no modal.
    #[must_use]
    pub const fn shell() -> Self {
        Self::new(false, false)
    }

    /// Whether a focused widget already consumed the press.
    #[must_use]
    pub const fn captured(self) -> bool {
        self.captured
    }

    /// Whether a live presentation owns the keyboard.
    #[must_use]
    pub const fn modal_owns_keyboard(self) -> bool {
        self.modal_owns_keyboard
    }

    /// Whether application-global rows may run: the predicate to hand
    /// [`ShortcutBinding::when`].
    // Taken by reference because `ShortcutBinding::when` accepts
    // `fn(&Context) -> bool`, not because the scope is expensive to copy.
    #[must_use]
    pub fn is_global(scope: &Self) -> bool {
        !scope.captured && !scope.modal_owns_keyboard
    }

    /// Whether a row that belongs to a presentation rather than to the shell
    /// may run: everything [`Self::is_global`] admits, plus a press arriving
    /// while a dialog is up.
    ///
    /// The predicate for a bare key that opens or closes a presentation, which
    /// would otherwise be unable to close the presentation it opened. It still
    /// yields to a focused text widget, so the key stays typable.
    #[must_use]
    pub fn is_uncaptured(scope: &Self) -> bool {
        !scope.captured
    }
}

/// A shortcut-catalog entry with a primary chord, aliases, help metadata, and a live-context predicate.
#[derive(Debug)]
#[must_use]
pub struct ShortcutBinding<Command, Context = ()> {
    id: &'static str,
    command: Command,
    primary: Shortcut,
    alternates: Vec<Shortcut>,
    section: String,
    description: String,
    enabled: bool,
    predicate: fn(&Context) -> bool,
}

fn always_active<Context>(_: &Context) -> bool {
    true
}

// Manual so cloning a catalog requires only its payload to be cloneable;
// `Context` is mentioned only behind a function pointer and is not owned.
impl<Command: Clone, Context> Clone for ShortcutBinding<Command, Context> {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            command: self.command.clone(),
            primary: self.primary.clone(),
            alternates: self.alternates.clone(),
            section: self.section.clone(),
            description: self.description.clone(),
            enabled: self.enabled,
            predicate: self.predicate,
        }
    }
}

impl<Command> ShortcutBinding<Command> {
    /// Creates an unconditional binding with an ID-derived description and empty section.
    pub fn new(id: &'static str, command: Command, primary: Shortcut) -> Self {
        Self {
            id,
            command,
            primary,
            alternates: Vec::new(),
            section: String::new(),
            description: label_fallback(id),
            enabled: true,
            predicate: always_active,
        }
    }
}

impl<Command, Context> ShortcutBinding<Command, Context> {
    /// Creates an unconditional binding for an explicitly selected context type.
    pub fn new_in_context(id: &'static str, command: Command, primary: Shortcut) -> Self {
        Self {
            id,
            command,
            primary,
            alternates: Vec::new(),
            section: String::new(),
            description: label_fallback(id),
            enabled: true,
            predicate: always_active,
        }
    }

    /// Sets the human description shown in shortcut help.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        let description = description.into();
        self.description = if description.trim().is_empty() {
            label_fallback(self.id)
        } else {
            description
        };
        self
    }

    /// Sets the shortcut-help section. An empty section produces a flat list.
    pub fn with_section(mut self, section: impl Into<String>) -> Self {
        self.section = section.into();
        self
    }

    /// Replaces the primary chord while preserving aliases, metadata, context, and enabled state.
    pub fn with_primary(mut self, shortcut: Shortcut) -> Self {
        self.primary = shortcut;
        self
    }

    /// Adds another chord that dispatches this binding's same command.
    pub fn with_alternate(mut self, shortcut: Shortcut) -> Self {
        self.alternates.push(shortcut);
        self
    }

    /// Adds alternate chords that dispatch this binding's same command.
    pub fn with_alternates(mut self, shortcuts: impl IntoIterator<Item = Shortcut>) -> Self {
        self.alternates.extend(shortcuts);
        self
    }

    /// Sets whether this binding participates in dispatch.
    pub fn with_enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Changes the context type and installs a non-capturing predicate evaluated during dispatch.
    pub fn when<NewContext>(
        self,
        predicate: fn(&NewContext) -> bool,
    ) -> ShortcutBinding<Command, NewContext> {
        ShortcutBinding {
            id: self.id,
            command: self.command,
            primary: self.primary,
            alternates: self.alternates,
            section: self.section,
            description: self.description,
            enabled: self.enabled,
            predicate,
        }
    }

    /// Returns the stable binding ID.
    pub fn id(&self) -> &'static str {
        self.id
    }

    /// Returns the typed command resolved by the application reducer.
    pub fn command(&self) -> &Command {
        &self.command
    }

    /// Returns the primary, user-presented chord.
    pub fn primary(&self) -> &Shortcut {
        &self.primary
    }

    /// Returns the alternate chords for the same command.
    pub fn alternates(&self) -> &[Shortcut] {
        &self.alternates
    }

    /// Iterates the primary chord followed by every alternate.
    pub fn shortcuts(&self) -> impl Iterator<Item = &Shortcut> {
        std::iter::once(&self.primary).chain(self.alternates.iter())
    }

    /// Returns the shortcut-help section.
    pub fn section(&self) -> &str {
        &self.section
    }

    /// Returns the human shortcut-help description.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns whether the binding is enabled and allowed by its live context.
    pub fn is_active(&self, context: &Context) -> bool {
        self.enabled && (self.predicate)(context)
    }

    /// Returns whether any chord matches a non-repeat key press in this context.
    pub fn matches_event(&self, event: &keyboard::Event, context: &Context) -> bool {
        self.is_active(context)
            && self
                .shortcuts()
                .any(|shortcut| shortcut.matches_event(event))
    }
}

/// Borrowed shortcut-help data with one row per binding.
///
/// Primary and alternate chords remain grouped in the same row.
#[derive(Debug, Clone, Copy)]
pub struct ShortcutHelpEntry<'a> {
    id: &'static str,
    section: &'a str,
    description: &'a str,
    primary: &'a Shortcut,
    alternates: &'a [Shortcut],
}

impl<'a> ShortcutHelpEntry<'a> {
    /// Returns the binding's stable ID, suitable for a row key.
    pub fn id(&self) -> &'static str {
        self.id
    }

    /// Returns the presentation section.
    pub fn section(&self) -> &'a str {
        self.section
    }

    /// Returns the human description.
    pub fn description(&self) -> &'a str {
        self.description
    }

    /// Returns the primary chord shown by compact help surfaces.
    pub fn primary(&self) -> &'a Shortcut {
        self.primary
    }

    /// Returns alternate chords for this same row.
    pub fn alternates(&self) -> &'a [Shortcut] {
        self.alternates
    }

    /// Iterates all chords represented by this one help row.
    pub fn shortcuts(&self) -> impl Iterator<Item = &'a Shortcut> {
        std::iter::once(self.primary).chain(self.alternates.iter())
    }
}

/// Derives one help row per binding while keeping alternate chords on that row.
pub fn shortcut_help<'a, Command: 'a, Context: 'a>(
    bindings: impl IntoIterator<Item = &'a ShortcutBinding<Command, Context>>,
) -> Vec<ShortcutHelpEntry<'a>> {
    bindings
        .into_iter()
        .map(|binding| ShortcutHelpEntry {
            id: binding.id,
            section: &binding.section,
            description: &binding.description,
            primary: &binding.primary,
            alternates: &binding.alternates,
        })
        .collect()
}

/// Returns the first active binding matching a non-repeat key press.
///
/// Non-press and repeat events are rejected before scanning, and duplicates
/// resolve in iterator order.
pub fn shortcut_binding_for_event<'a, Command: 'a, Context: 'a>(
    bindings: impl IntoIterator<Item = &'a ShortcutBinding<Command, Context>>,
    event: &keyboard::Event,
    context: &Context,
) -> Option<&'a ShortcutBinding<Command, Context>> {
    let keyboard::Event::KeyPressed { repeat: false, .. } = event else {
        return None;
    };
    bindings
        .into_iter()
        .find(|binding| binding.matches_event(event, context))
}

/// Clones the command from the first binding active in the supplied context and matching the event.
pub fn dispatch_shortcut_command<'a, Command: Clone + 'a, Context: 'a>(
    bindings: impl IntoIterator<Item = &'a ShortcutBinding<Command, Context>>,
    event: &keyboard::Event,
    context: &Context,
) -> Option<Command> {
    shortcut_binding_for_event(bindings, event, context).map(|binding| binding.command.clone())
}

fn label_fallback(id: &str) -> String {
    let segment = id.rsplit(['.', '/', ':']).next().unwrap_or(id);
    let words = segment
        .split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            let Some(first) = chars.next() else {
                return String::new();
            };
            first.to_uppercase().collect::<String>() + chars.as_str()
        })
        .collect::<Vec<_>>();

    if words.is_empty() {
        crate::localization::text("enzui-action-unnamed-action")
    } else {
        words.join(" ")
    }
}
