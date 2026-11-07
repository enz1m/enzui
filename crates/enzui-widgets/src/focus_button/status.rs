//! `focus_button`: status.

use super::*;

/// Live button state for a child's style or draw callback.
///
/// The owning button sets this immediately before drawing its content. Read
/// with [`Self::get`] inside the child's callback; reading while building a
/// view does not describe the upcoming frame. Before the first paint it returns
/// [`FocusStatus::Active`]. Use a separate handle per button.
#[derive(Clone, Debug)]
pub struct ContentStatus(std::rc::Rc<std::cell::Cell<FocusStatus>>);

impl Default for ContentStatus {
    fn default() -> Self {
        Self(std::rc::Rc::new(std::cell::Cell::new(FocusStatus::Active)))
    }
}

impl ContentStatus {
    /// The owning button's current painted interaction state.
    #[must_use]
    pub fn get(&self) -> FocusStatus {
        self.0.get()
    }

    pub(crate) fn set(&self, status: FocusStatus) {
        self.0.set(status);
    }
}

/// Interaction status handed to a [`FocusButton`] style closure. Mirrors the
/// stock `button::Status` but adds `Focused` so callers can render a
/// focus-visible ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusStatus {
    /// Idle, not hovered/pressed/focused.
    Active,
    /// Focus-visible without pointer hover or an active press.
    /// Standalone controls use window keyboard modality; embedded controls use
    /// the host's focus-visible flag. Hovered controls overlay the ring separately.
    Focused,
    /// Pointer hovering.
    Hovered,
    /// A pointer press or keyboard Space is held, including a pointer dragged away.
    Pressed,
    /// No primary activation is installed; read-only controls may still hold focus.
    Disabled,
}

pub(crate) enum Activation<'a, Message> {
    Eager(Message),
    Clipboard(String),
    Lazy(Box<dyn Fn() -> Message + 'a>),
    Modified(Box<dyn Fn(keyboard::Modifiers) -> Message + 'a>),
    Positioned(Box<dyn Fn(Option<iced::Point>) -> Message + 'a>),
}

impl<Message: Clone> Activation<'_, Message> {
    pub(crate) fn dispatch(
        &self,
        shell: &mut Shell<'_, Message>,
        modifiers: keyboard::Modifiers,
        at: Option<iced::Point>,
    ) {
        if let Self::Clipboard(text) = self {
            shell.write_clipboard(iced::advanced::graphics::core::clipboard::Content::Text(
                text.clone(),
            ));
        } else {
            shell.publish(self.message_with_point(modifiers, at));
        }
    }
    pub(crate) fn message_with_point(
        &self,
        modifiers: keyboard::Modifiers,
        at: Option<iced::Point>,
    ) -> Message {
        match self {
            Activation::Eager(m) => m.clone(),
            Activation::Lazy(f) => f(),
            Activation::Modified(f) => f(modifiers),
            Activation::Positioned(f) => f(at),
            Activation::Clipboard(_) => unreachable!("clipboard activation uses dispatch"),
        }
    }
}
#[derive(Debug, Default)]
pub(crate) struct State {
    pub(crate) identity: Option<iced::advanced::widget::Id>,
    pub(crate) frame_owed: bool,
    pub(crate) focus: FocusState,
    pub(crate) is_pressed: bool,
    pub(crate) pressed_finger: Option<touch::Finger>,
    pub(crate) space_held: bool,
    // Preserve the chord at press time even if modifiers are released before the pointer.
    pub(crate) press_modifiers: keyboard::Modifiers,
    // Pointer activation reports its press anchor; keyboard and routed activation have none.
    pub(crate) press_point: Option<iced::Point>,
    pub(crate) live_modifiers: keyboard::Modifiers,
    // Persist the frame cache across view rebuilds so input can detect paint changes.
    pub(crate) status: Option<FocusStatus>,
    // Hover status can stay unchanged while keyboard focus adds or removes a ring.
    pub(crate) focus_visible_painted: Option<bool>,
    pub(crate) window_key: u64,
    // Measurement-only trees must not consume retained focus-return state.
    pub(crate) has_updated: bool,
    // Consume routed activation before dispatch so later events cannot replay it.
    pub(crate) activate_pending: bool,
    /// The fill, ink, border and shadow fading between states (hover,
    /// press release, selection), read at presented frames.
    pub(crate) fade: crate::motion::StateFade<Style>,
}

impl iced::advanced::widget::operation::Focusable for State {
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

/// Return whether this tree contains a keyboard-focused [`FocusButton`].
/// Descends through unfocused outer buttons to include nested close controls.
#[must_use]
pub fn any_focused_in(tree: &Tree) -> bool {
    if tree.tag == tree::Tag::of::<State>() && tree.state.downcast_ref::<State>().focus.is_focused()
    {
        return true;
    }
    tree.children.iter().any(any_focused_in)
}
