//! Keyboard focus, activation, traversal, modality, and reveal operations for custom
//! widgets.
//!
//! Input modality is tracked per window, and traversal operations wrap within
//! their operated subtree.

mod nested_reveal;
pub use nested_reveal::{
    AnonymousScrollPolicy, focus_and_reveal_id, reveal_focused_all, reveal_focused_all_with_policy,
    reveal_id_all, reveal_id_all_with_policy,
};

use iced_core::keyboard::{self, key};
use iced_core::widget::operation;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

/// The input modality used to decide whether keyboard focus rings are visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputModality {
    /// Keyboard modality, in which focus rings are visible.
    Keyboard,
    /// Pointer or touch modality, in which focus rings are suppressed.
    Pointer,
}

impl InputModality {
    /// Whether a focus ring should be painted for a keyboard-focused control
    /// under this modality. True only for [`Keyboard`](InputModality::Keyboard).
    #[must_use]
    pub const fn focus_ring_visible(self) -> bool {
        matches!(self, InputModality::Keyboard)
    }
}

static MODALITY: Mutex<Option<WindowModalities>> = Mutex::new(None);

/// Maximum number of keyed native windows whose last input modality is retained.
pub const MAX_TRACKED_WINDOW_MODALITIES: usize = 256;

#[derive(Default)]
struct WindowModalities {
    by_key: HashMap<u64, InputModality>,
    recency: VecDeque<u64>,
}

impl WindowModalities {
    fn get(&self, key: u64) -> Option<InputModality> {
        self.by_key.get(&key).copied()
    }

    fn set(&mut self, key: u64, modality: InputModality) {
        if self.by_key.contains_key(&key) {
            if let Some(at) = self.recency.iter().position(|candidate| *candidate == key) {
                self.recency.remove(at);
            }
        } else if self.by_key.len() >= MAX_TRACKED_WINDOW_MODALITIES
            && let Some(oldest) = self.recency.pop_front()
        {
            self.by_key.remove(&oldest);
        }

        self.by_key.insert(key, modality);
        self.recency.push_back(key);
        debug_assert_eq!(
            self.by_key.len(),
            self.recency.len(),
            "the recency list mirrors the key map"
        );
        debug_assert!(
            self.by_key.len() <= MAX_TRACKED_WINDOW_MODALITIES,
            "tracked windows exceed their bound"
        );
    }
}

thread_local! {
    static MODALITY_UNKEYED: std::cell::Cell<InputModality>
        = const { std::cell::Cell::new(DEFAULT_MODALITY) };
}

/// The bucket for callers with no window handle (headless tests, single-window hosts
/// that cannot report one).
pub const KEY_UNKEYED: u64 = 0;

const DEFAULT_MODALITY: InputModality = InputModality::Keyboard;

fn with_modalities<R>(f: impl FnOnce(&mut WindowModalities) -> R) -> R {
    let mut guard = MODALITY
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    f(guard.get_or_insert_with(WindowModalities::default))
}

/// Returns a nonzero key for an available window handle, or [`KEY_UNKEYED`] otherwise.
#[must_use]
pub fn window_key(window: &dyn iced_core::window::Window) -> u64 {
    let Ok(handle) = window.window_handle() else {
        return KEY_UNKEYED;
    };
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    handle.as_raw().hash(&mut hasher);
    let key = hasher.finish();
    // Reserve zero for the unkeyed bucket even if a real window handle hashes to zero.
    if key == KEY_UNKEYED { 1 } else { key }
}

/// Returns the latest input modality for `key`; unseen windows default to keyboard
/// modality.
#[must_use]
pub fn current_modality_for(key: u64) -> InputModality {
    if key == KEY_UNKEYED {
        MODALITY_UNKEYED.get()
    } else {
        with_modalities(|modalities| modalities.get(key).unwrap_or(DEFAULT_MODALITY))
    }
}

/// The current [`InputModality`] for the unkeyed bucket ([`KEY_UNKEYED`]).
#[must_use]
pub fn current_modality() -> InputModality {
    current_modality_for(KEY_UNKEYED)
}

/// Returns whether a focused control should paint a ring using the unkeyed modality
/// bucket.
#[must_use]
pub fn focus_ring_visible(focused: bool) -> bool {
    focus_ring_visible_for(focused, KEY_UNKEYED)
}

/// Returns whether a focused control should paint a ring using the modality for `key`.
#[must_use]
pub fn focus_ring_visible_for(focused: bool, key: u64) -> bool {
    focused && current_modality_for(key).focus_ring_visible()
}

/// Sets the input modality of the unkeyed bucket.
pub fn set_modality(modality: InputModality) {
    set_modality_for(KEY_UNKEYED, modality);
}

/// Sets the input modality of the bucket identified by `key`.
pub fn set_modality_for(key: u64, modality: InputModality) {
    if key == KEY_UNKEYED {
        MODALITY_UNKEYED.set(modality);
    } else {
        with_modalities(|modalities| modalities.set(key, modality));
    }
}

/// Whether a keyboard event means the user is navigating by keyboard.
///
/// A press or release carrying Ctrl, Alt or Logo is a shortcut chord, not
/// navigation: copying a pointer selection with Ctrl+C must not make the focus
/// ring appear. Neither is a modifier key on its own, which arrives before its
/// modifier state does. This is the `:focus-visible` heuristic browsers apply.
#[must_use]
pub fn signals_keyboard_modality(event: &iced_core::keyboard::Event) -> bool {
    use iced_core::keyboard::{Event, Key, key::Named};
    match event {
        Event::KeyPressed { key, modifiers, .. } | Event::KeyReleased { key, modifiers, .. } => {
            let modifier_key = matches!(
                key,
                Key::Named(
                    Named::Control
                        | Named::Shift
                        | Named::Alt
                        | Named::AltGraph
                        | Named::Super
                        | Named::Meta
                        | Named::Hyper
                        | Named::CapsLock
                )
            );
            !(modifier_key || modifiers.control() || modifiers.alt() || modifiers.logo())
        }
        _ => false,
    }
}

/// Update the unkeyed ([`KEY_UNKEYED`]) [`InputModality`] from a raw input `event`.
/// Multi-window widgets call [`note_input_event_for`] with their window key instead.
pub fn note_input_event(event: &iced_core::Event) {
    note_input_event_for(event, KEY_UNKEYED);
}

/// Update the [`InputModality`] of the window identified by `key` from a raw input
/// `event`. The per-window counterpart of [`note_input_event`]: a widget passes
/// `window_key(shell.window())` so its window's modality is independent of every other
/// window's.
pub fn note_input_event_for(event: &iced_core::Event, key: u64) {
    use iced_core::{Event, mouse, touch};
    let modality = match event {
        Event::Keyboard(keyboard_event) if signals_keyboard_modality(keyboard_event) => {
            InputModality::Keyboard
        }
        Event::Keyboard(_) => return,
        Event::Mouse(
            mouse::Event::ButtonPressed(_)
            | mouse::Event::ButtonReleased(_)
            | mouse::Event::WheelScrolled { .. },
        )
        | Event::Touch(touch::Event::FingerPressed { .. } | touch::Event::FingerLifted { .. }) => {
            InputModality::Pointer
        }
        _ => return,
    };
    set_modality_for(key, modality);
}

/// Per-widget keyboard-focus state. Embed one in a widget's tree state and implement
/// [`operation::Focusable`] by delegating to it, so that iced's `focus`, `focus_next`,
/// `focus_previous`, and `find_focused` operations include the widget in the Tab ring.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FocusState {
    focused: bool,
}

impl FocusState {
    /// A new, unfocused state.
    pub fn new() -> Self {
        Self { focused: false }
    }

    /// Whether the widget currently holds keyboard focus.
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Give the widget keyboard focus.
    pub fn focus(&mut self) {
        self.focused = true;
    }

    /// Remove keyboard focus.
    pub fn unfocus(&mut self) {
        self.focused = false;
    }

    /// Report this widget to a focus
    /// [`Operation`](iced_core::widget::operation::Operation), so standard iced focus
    /// tasks include it. Call from a `Widget::operate` implementation.
    pub fn operate<T>(
        &mut self,
        operation: &mut dyn operation::Operation<T>,
        id: Option<&iced_core::widget::Id>,
        bounds: iced_core::Rectangle,
    ) {
        operation.focusable(id, bounds, self);
    }
}

impl operation::Focusable for FocusState {
    fn is_focused(&self) -> bool {
        FocusState::is_focused(self)
    }

    fn focus(&mut self) {
        FocusState::focus(self);
    }

    fn unfocus(&mut self) {
        FocusState::unfocus(self);
    }
}

/// Releases focus when `can_take_input` is false and returns whether focus changed.
pub fn release_focus_if_inert(state: &mut dyn operation::Focusable, can_take_input: bool) -> bool {
    if can_take_input || !state.is_focused() {
        return false;
    }
    state.unfocus();
    true
}

/// What a pointer press means for one focusable control's focus flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressFocus {
    /// The press landed on this control: it takes keyboard focus.
    Take,
    /// The press landed somewhere else: this control gives up keyboard focus.
    Drop,
    /// Nothing can be concluded — leave the focus flag exactly as it is.
    Leave,
}

/// Returns how a press position changes one control’s focus.
///
/// An unknown position returns [`PressFocus::Leave`]; overlapping bounds may each
/// return [`PressFocus::Take`].
#[must_use]
pub fn press_focus(
    press_position: Option<iced_core::Point>,
    bounds: iced_core::Rectangle,
) -> PressFocus {
    let Some(position) = press_position else {
        return PressFocus::Leave;
    };
    if bounds.contains(position) {
        PressFocus::Take
    } else {
        PressFocus::Drop
    }
}

/// How a pointer press changes one control's focus, including presses the
/// control's own layer never sees.
///
/// [`press_focus`] can only answer from a position, and a position is exactly
/// what a covered or clipped subtree loses: an ancestor that hides its
/// children from the pointer hands them
/// [`Cursor::Levitating`](iced_core::mouse::Cursor::Levitating) (a scrollable
/// whose viewport the pointer left, a rounded clip whose corner it is outside)
/// or [`Cursor::Unavailable`](iced_core::mouse::Cursor::Unavailable), and both
/// report `position() == None`. Every focusable under such an ancestor then
/// answers [`PressFocus::Leave`] and keeps whatever focus it held, which
/// is how a titlebar button stayed focused — and kept answering Space — while
/// the user clicked in the content pane below it.
///
/// The three cursor states are three different facts, and this is the mapping
/// that distinguishes them:
///
/// * `Available(p)` — the press may be on this control: `Take` when `bounds`
///   contains `p`, `Drop` otherwise.
/// * `Levitating(p)` and `Obstructed(p)` — the press has a known position but
///   belongs to a layer *above* this control (an overlay, or the rest of the
///   window outside a scrollable's viewport), so it is never this control's:
///   always `Drop`. It must not be `Take` even when `bounds` contains `p`; a
///   covered control is not the one the user hit.
/// * `Unavailable` — nothing is known about where the press was: `Leave`.
///
/// Returns `None` for events that are not primary presses.
///
/// # Example
///
/// ```
/// use enzui_core::focus::{PressFocus, press_focus_from_cursor};
/// use iced_core::{Event, Point, Rectangle, mouse};
///
/// let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
/// let button = Rectangle::new(Point::new(0.0, 0.0), [50.0, 20.0].into());
/// let elsewhere = mouse::Cursor::Available(Point::new(400.0, 300.0));
///
/// assert_eq!(
///     press_focus_from_cursor(&press, elsewhere, button),
///     Some(PressFocus::Drop),
/// );
/// // The same press, seen by a subtree an ancestor hid from the pointer.
/// assert_eq!(
///     press_focus_from_cursor(&press, elsewhere.levitate(), button),
///     Some(PressFocus::Drop),
/// );
/// // A scrollable obstructs its content for a press outside its viewport,
/// // even one over where a scrolled-away row would be.
/// let over_row = mouse::Cursor::Available(Point::new(10.0, 10.0));
/// assert_eq!(
///     press_focus_from_cursor(&press, over_row.obstruct(), button),
///     Some(PressFocus::Drop),
/// );
/// ```
#[must_use]
pub fn press_focus_from_cursor(
    event: &iced_core::Event,
    cursor: iced_core::mouse::Cursor,
    bounds: iced_core::Rectangle,
) -> Option<PressFocus> {
    let position = pointer_press_position(event, cursor)?;
    // A finger carries its own position, so the mouse cursor's layer says
    // nothing about it; only a mouse press is judged by levitation.
    let above = matches!(event, iced_core::Event::Mouse(_))
        && matches!(
            cursor,
            iced_core::mouse::Cursor::Levitating(_) | iced_core::mouse::Cursor::Obstructed(_)
        );
    Some(if above {
        PressFocus::Drop
    } else {
        press_focus(position, bounds)
    })
}

/// Classifies primary-button and touch presses.
///
/// The outer `Option` distinguishes other events; the inner value is absent when a
/// mouse press has no cursor position.
#[must_use]
pub fn pointer_press_position(
    event: &iced_core::Event,
    cursor: iced_core::mouse::Cursor,
) -> Option<Option<iced_core::Point>> {
    use iced_core::{Event, mouse, touch};
    match event {
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => Some(cursor.position()),
        Event::Touch(touch::Event::FingerPressed { position, .. }) => Some(Some(*position)),
        _ => None,
    }
}

/// Classifies primary-button releases and touch lifts.
///
/// The outer `Option` distinguishes other events; the inner value is absent when a
/// mouse release has no cursor position.
#[must_use]
pub fn pointer_release_position(
    event: &iced_core::Event,
    cursor: iced_core::mouse::Cursor,
) -> Option<Option<iced_core::Point>> {
    use iced_core::{Event, mouse, touch};
    match event {
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => Some(cursor.position()),
        Event::Touch(touch::Event::FingerLifted { position, .. }) => Some(Some(*position)),
        _ => None,
    }
}

/// Returns `true` for non-repeating Enter or Space presses without Ctrl, Alt, or Logo
/// modifiers.
#[must_use]
pub fn activation_from_key(event: &keyboard::Event) -> bool {
    let keyboard::Event::KeyPressed {
        key: keyboard::Key::Named(key::Named::Enter | key::Named::Space),
        repeat: false,
        modifiers,
        ..
    } = event
    else {
        return false;
    };
    !(modifiers.control() || modifiers.alt() || modifiers.logo())
}

/// Which way an app-level Tab traversal moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Traversal {
    /// Tab — move to the next focusable control.
    Next,
    /// Shift+Tab — move to the previous focusable control.
    Previous,
}

impl Traversal {
    /// Whether this direction moves forward.
    #[must_use]
    pub const fn is_forward(self) -> bool {
        matches!(self, Self::Next)
    }
}

/// Decodes bare Tab and Shift+Tab as forward and backward traversal, leaving Ctrl, Alt,
/// and Logo chords unhandled.
#[must_use]
pub fn traversal_from_key(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
) -> Option<Traversal> {
    if !matches!(key, keyboard::Key::Named(key::Named::Tab)) {
        return None;
    }
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    Some(if modifiers.shift() {
        Traversal::Previous
    } else {
        Traversal::Next
    })
}

/// Decodes an ignored runtime Tab press; captured events never trigger application
/// traversal.
#[must_use]
pub fn traversal_from_event(
    event: &iced_core::Event,
    status: iced_core::event::Status,
) -> Option<Traversal> {
    if status != iced_core::event::Status::Ignored {
        return None;
    }
    let iced_core::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event
    else {
        return None;
    };
    traversal_from_key(key, *modifiers)
}

/// Directional intent decoded from arrow, Home, End, PageUp, and PageDown keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    /// `ArrowUp` / `ArrowRight` — increment / next.
    Increment,
    /// `ArrowDown` / `ArrowLeft` — decrement / previous.
    Decrement,
    /// `Home` — jump to minimum / first.
    Home,
    /// `End` — jump to maximum / last.
    End,
    /// `PageUp` — large increment.
    PageUp,
    /// `PageDown` — large decrement.
    PageDown,
}

/// Decodes arrow, Home, End, PageUp, and PageDown presses without Ctrl, Alt, or Logo modifiers.
#[must_use]
pub fn navigation_from_key(event: &keyboard::Event) -> Option<Nav> {
    let keyboard::Event::KeyPressed {
        key: keyboard::Key::Named(named),
        modifiers,
        ..
    } = event
    else {
        return None;
    };
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    Some(match named {
        key::Named::ArrowUp | key::Named::ArrowRight => Nav::Increment,
        key::Named::ArrowDown | key::Named::ArrowLeft => Nav::Decrement,
        key::Named::Home => Nav::Home,
        key::Named::End => Nav::End,
        key::Named::PageUp => Nav::PageUp,
        key::Named::PageDown => Nav::PageDown,
        _ => return None,
    })
}

/// [`navigation_from_key`] for a horizontal control laid out in `direction`:
/// in a right-to-left one, Left increments and Right decrements, so the arrow
/// points the way the value or selection moves on screen.
#[must_use]
pub fn navigation_from_key_in(
    event: &keyboard::Event,
    direction: crate::placement::Direction,
) -> Option<Nav> {
    let nav = navigation_from_key(event)?;
    let horizontal = matches!(
        event,
        keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(key::Named::ArrowLeft | key::Named::ArrowRight),
            ..
        }
    );
    Some(match nav {
        Nav::Increment if horizontal && direction == crate::placement::Direction::RightToLeft => {
            Nav::Decrement
        }
        Nav::Decrement if horizontal && direction == crate::placement::Direction::RightToLeft => {
            Nav::Increment
        }
        nav => nav,
    })
}

/// Returns the wrapping target index, or `None` when `total` is zero.
pub fn wrap_target(focused: Option<usize>, total: usize, forward: bool) -> Option<usize> {
    if total == 0 {
        return None;
    }
    Some(match (focused, forward) {
        (None, true) => 0,
        (None, false) => total - 1,
        (Some(i), true) => (i + 1) % total,
        (Some(i), false) => (i + total - 1) % total,
    })
}

/// Moves focus to the next focusable widget, wrapping to the first and preserving focus
/// in a one-control scope.
pub fn focus_next_wrapping<T>() -> impl operation::Operation<T>
where
    T: Send + 'static,
{
    use iced_core::widget::operation::focusable::{Count, count};

    struct FocusNextWrapping {
        count: Count,
        current: usize,
    }

    impl<T> operation::Operation<T> for FocusNextWrapping {
        fn focusable(
            &mut self,
            _id: Option<&iced_core::widget::Id>,
            _bounds: iced_core::Rectangle,
            state: &mut dyn operation::Focusable,
        ) {
            let Some(target) = wrap_target(self.count.focused, self.count.total, true) else {
                return;
            };
            if let Some(focused) = self.count.focused
                && focused == self.current
                && focused != target
            {
                state.unfocus();
            }
            // Avoid refocusing the current target because some widgets reset caret and blink state
            // on `focus()`.
            if self.current == target && !state.is_focused() {
                state.focus();
            }
            self.current += 1;
        }

        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn operation::Operation<T>)) {
            operate(self);
        }
    }

    operation::then(count(), |count| FocusNextWrapping { count, current: 0 })
}

/// Moves focus to the previous focusable widget, wrapping to the last and preserving
/// focus in a one-control scope.
pub fn focus_previous_wrapping<T>() -> impl operation::Operation<T>
where
    T: Send + 'static,
{
    use iced_core::widget::operation::focusable::{Count, count};

    struct FocusPreviousWrapping {
        count: Count,
        current: usize,
    }

    impl<T> operation::Operation<T> for FocusPreviousWrapping {
        fn focusable(
            &mut self,
            _id: Option<&iced_core::widget::Id>,
            _bounds: iced_core::Rectangle,
            state: &mut dyn operation::Focusable,
        ) {
            let Some(target) = wrap_target(self.count.focused, self.count.total, false) else {
                return;
            };
            if let Some(focused) = self.count.focused
                && focused == self.current
                && focused != target
            {
                state.unfocus();
            }
            // Avoid refocusing the current target because some widgets reset caret and blink state
            // on `focus()`.
            if self.current == target && !state.is_focused() {
                state.focus();
            }
            self.current += 1;
        }

        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn operation::Operation<T>)) {
            operate(self);
        }
    }

    operation::then(count(), |count| FocusPreviousWrapping { count, current: 0 })
}

/// Move focus with wrapping, then reveal it through all enclosing scroll viewports.
#[must_use]
pub fn traversal_operation<T>(direction: Traversal) -> Box<dyn operation::Operation<T>>
where
    T: Send + 'static,
{
    match direction {
        Traversal::Next => nested_reveal::after_focus(focus_next_wrapping::<()>()),
        Traversal::Previous => nested_reveal::after_focus(focus_previous_wrapping::<()>()),
    }
}

/// Focuses `target` only if a preliminary tree pass finds it, avoiding destructive
/// focus loss when it is absent.
#[derive(Debug)]
pub struct RestoreFocus {
    target: iced_core::widget::Id,
    found: bool,
}

impl RestoreFocus {
    /// Construct an operation that will focus `target` iff it is present.
    #[must_use]
    pub const fn new(target: iced_core::widget::Id) -> Self {
        Self {
            target,
            found: false,
        }
    }
}

impl<T: 'static> operation::Operation<T> for RestoreFocus {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn operation::Operation<T>)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        id: Option<&iced_core::widget::Id>,
        _bounds: iced_core::Rectangle,
        _state: &mut dyn operation::Focusable,
    ) {
        if id == Some(&self.target) {
            self.found = true;
        }
    }

    fn finish(&self) -> operation::Outcome<T> {
        if self.found {
            operation::Outcome::Chain(Box::new(operation::focusable::focus(self.target.clone())))
        } else {
            operation::Outcome::None
        }
    }
}

/// Returns the minimum offset that reveals an item on one axis.
/// Oversized items maximize visible coverage without moving an already covered viewport.
///
/// Non-finite input leaves `current` unchanged.
#[must_use]
pub fn reveal_axis_offset(current: f32, view_len: f32, item_start: f32, item_len: f32) -> f32 {
    if ![current, view_len, item_start, item_len]
        .iter()
        .all(|v| v.is_finite())
    {
        return current;
    }
    let view_end = current + view_len;
    let item_end = item_start + item_len;
    if item_len > view_len {
        if item_start > current {
            item_start
        } else if item_end < view_end {
            item_end - view_len
        } else {
            current
        }
    } else if item_start < current {
        item_start
    } else if item_end > view_end {
        item_end - view_len
    } else {
        current
    }
}

/// Scrolls a named view by the minimum distance needed to reveal its focused
/// descendant; missing or already-visible targets do nothing.
#[must_use]
pub fn reveal_focused<T>(scroll_view: iced_core::widget::Id) -> Box<dyn operation::Operation<T>>
where
    T: Send + 'static,
{
    use iced_core::widget::operation::scrollable::{AbsoluteOffset, scroll_to};

    struct FindReveal {
        target: iced_core::widget::Id,
        scroll_bounds: Option<iced_core::Rectangle>,
        translation: iced_core::Vector,
        focused_bounds: Option<iced_core::Rectangle>,
        inside_depth: Option<u32>,
    }

    impl<T: Send + 'static> operation::Operation<T> for FindReveal {
        fn scrollable(
            &mut self,
            id: Option<&iced_core::widget::Id>,
            bounds: iced_core::Rectangle,
            _content_bounds: iced_core::Size,
            translation: iced_core::Vector,
            _state: &mut dyn operation::Scrollable,
        ) {
            if id == Some(&self.target) {
                self.scroll_bounds = Some(bounds);
                self.translation = translation;
                self.inside_depth = Some(0);
            }
        }

        fn focusable(
            &mut self,
            _id: Option<&iced_core::widget::Id>,
            bounds: iced_core::Rectangle,
            state: &mut dyn operation::Focusable,
        ) {
            if state.is_focused() && self.inside_depth.is_some() {
                self.focused_bounds = Some(bounds);
            }
        }

        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn operation::Operation<T>)) {
            if let Some(ref mut depth) = self.inside_depth {
                *depth += 1;
            }
            operate(self);
            if let Some(ref mut depth) = self.inside_depth {
                *depth -= 1;
                if *depth == 0 {
                    self.inside_depth = None;
                }
            }
        }

        fn finish(&self) -> operation::Outcome<T> {
            let (Some(view), Some(item)) = (self.scroll_bounds, self.focused_bounds) else {
                return operation::Outcome::None;
            };
            // Operation bounds are window-relative, so subtract the viewport origin before applying
            // its scroll translation.
            let x = reveal_axis_offset(self.translation.x, view.width, item.x - view.x, item.width);
            let y = reveal_axis_offset(
                self.translation.y,
                view.height,
                item.y - view.y,
                item.height,
            );
            // Skip an unchanged offset because `scroll_to` still invokes the scroll callback.
            if x == self.translation.x && y == self.translation.y {
                return operation::Outcome::None;
            }
            operation::Outcome::Chain(Box::new(scroll_to(
                self.target.clone(),
                AbsoluteOffset { x, y }.into(),
                iced_core::widget::operation::Animation::Instant,
            )))
        }
    }

    Box::new(FindReveal {
        target: scroll_view,
        scroll_bounds: None,
        translation: iced_core::Vector::new(0.0, 0.0),
        focused_bounds: None,
        inside_depth: None,
    })
}

/// Scrolls the innermost named ancestor needed to reveal `target`; missing targets and
/// anonymous ancestors do nothing.
#[must_use]
pub fn reveal_id<T>(target: iced_core::widget::Id) -> Box<dyn operation::Operation<T>>
where
    T: Send + 'static,
{
    use iced_core::widget::operation::scrollable::{AbsoluteOffset, scroll_to};
    use std::any::Any;

    struct ScrollFrame {
        id: Option<iced_core::widget::Id>,
        bounds: iced_core::Rectangle,
        translation: iced_core::Vector,
        pushed_at_depth: u32,
    }

    struct FindTarget {
        target: iced_core::widget::Id,
        stack: Vec<ScrollFrame>,
        target_bounds: Option<iced_core::Rectangle>,
        enclosing: Option<(
            iced_core::widget::Id,
            iced_core::Rectangle,
            iced_core::Vector,
        )>,
        depth: u32,
        pending: Option<ScrollFrame>,
    }

    impl FindTarget {
        fn record_if_target(
            &mut self,
            id: Option<&iced_core::widget::Id>,
            bounds: iced_core::Rectangle,
        ) {
            if self.target_bounds.is_some() {
                return;
            }
            if id != Some(&self.target) {
                return;
            }
            self.target_bounds = Some(bounds);
            for frame in self.stack.iter().rev() {
                if let Some(id) = frame.id.clone() {
                    self.enclosing = Some((id, frame.bounds, frame.translation));
                    return;
                }
            }
        }
    }

    impl<T: Send + 'static> operation::Operation<T> for FindTarget {
        fn scrollable(
            &mut self,
            id: Option<&iced_core::widget::Id>,
            bounds: iced_core::Rectangle,
            _content_bounds: iced_core::Size,
            translation: iced_core::Vector,
            _state: &mut dyn operation::Scrollable,
        ) {
            self.record_if_target(id, bounds);
            self.pending = Some(ScrollFrame {
                id: id.cloned(),
                bounds,
                translation,
                pushed_at_depth: self.depth,
            });
        }

        fn focusable(
            &mut self,
            id: Option<&iced_core::widget::Id>,
            bounds: iced_core::Rectangle,
            _state: &mut dyn operation::Focusable,
        ) {
            self.record_if_target(id, bounds);
        }

        fn text_input(
            &mut self,
            id: Option<&iced_core::widget::Id>,
            bounds: iced_core::Rectangle,
            _state: &mut dyn operation::TextInput,
        ) {
            self.record_if_target(id, bounds);
        }

        fn text(
            &mut self,
            id: Option<&iced_core::widget::Id>,
            bounds: iced_core::Rectangle,
            _text: &str,
        ) {
            self.record_if_target(id, bounds);
        }

        fn custom(
            &mut self,
            id: Option<&iced_core::widget::Id>,
            bounds: iced_core::Rectangle,
            _state: &mut dyn Any,
        ) {
            self.record_if_target(id, bounds);
        }

        fn container(
            &mut self,
            id: Option<&iced_core::widget::Id>,
            bounds: iced_core::Rectangle,
            _viewport: &iced_core::Rectangle,
        ) {
            self.record_if_target(id, bounds);
        }

        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn operation::Operation<T>)) {
            self.depth += 1;
            let pushed = self.pending.take().map(|mut frame| {
                frame.pushed_at_depth = self.depth;
                self.stack.push(frame);
                self.depth
            });
            operate(self);
            if let Some(pushed_at) = pushed {
                while let Some(top) = self.stack.last() {
                    if top.pushed_at_depth >= pushed_at {
                        self.stack.pop();
                    } else {
                        break;
                    }
                }
            }
            self.depth -= 1;
        }

        fn finish(&self) -> operation::Outcome<T> {
            let (Some(item), Some((scroller_id, view, translation))) =
                (self.target_bounds, self.enclosing.clone())
            else {
                return operation::Outcome::None;
            };
            // Operation bounds are window-relative, so subtract the viewport origin before applying
            // its scroll translation.
            let x = reveal_axis_offset(translation.x, view.width, item.x - view.x, item.width);
            let y = reveal_axis_offset(translation.y, view.height, item.y - view.y, item.height);
            if x == translation.x && y == translation.y {
                return operation::Outcome::None;
            }
            operation::Outcome::Chain(Box::new(scroll_to(
                scroller_id,
                AbsoluteOffset { x, y }.into(),
                iced_core::widget::operation::Animation::Instant,
            )))
        }
    }

    Box::new(FindTarget {
        target,
        stack: Vec::new(),
        target_bounds: None,
        enclosing: None,
        depth: 0,
        pending: None,
    })
}

/// Releases every focused control in a subtree and returns the number changed.
#[must_use]
pub fn release_all_focus() -> impl operation::Operation<usize> {
    struct ReleaseAll {
        released: usize,
    }

    impl operation::Operation<usize> for ReleaseAll {
        fn focusable(
            &mut self,
            _id: Option<&iced_core::widget::Id>,
            _bounds: iced_core::Rectangle,
            state: &mut dyn operation::Focusable,
        ) {
            if state.is_focused() {
                state.unfocus();
                self.released += 1;
            }
        }

        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn operation::Operation<usize>)) {
            operate(self);
        }

        fn finish(&self) -> operation::Outcome<usize> {
            operation::Outcome::Some(self.released)
        }
    }

    ReleaseAll { released: 0 }
}

/// Unfocuses every focused control except `keep`; finishes with how many it
/// released.
///
/// For a control that took focus on its own: the others let go, and the
/// claimed one is not touched, so its caret and selection stay where they are.
#[must_use]
pub fn release_focus_except(keep: iced_core::widget::Id) -> impl operation::Operation<usize> {
    struct ReleaseOthers {
        keep: iced_core::widget::Id,
        released: usize,
    }

    impl operation::Operation<usize> for ReleaseOthers {
        fn focusable(
            &mut self,
            id: Option<&iced_core::widget::Id>,
            _bounds: iced_core::Rectangle,
            state: &mut dyn operation::Focusable,
        ) {
            if id != Some(&self.keep) && state.is_focused() {
                state.unfocus();
                self.released += 1;
            }
        }

        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn operation::Operation<usize>)) {
            operate(self);
        }

        fn finish(&self) -> operation::Outcome<usize> {
            operation::Outcome::Some(self.released)
        }
    }

    ReleaseOthers { keep, released: 0 }
}

/// Counts focused controls in a subtree, including anonymous controls.
#[must_use]
pub fn count_focused() -> impl operation::Operation<usize> {
    struct CountFocused {
        focused: usize,
    }

    impl operation::Operation<usize> for CountFocused {
        fn focusable(
            &mut self,
            _id: Option<&iced_core::widget::Id>,
            _bounds: iced_core::Rectangle,
            state: &mut dyn operation::Focusable,
        ) {
            if state.is_focused() {
                self.focused += 1;
            }
        }

        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn operation::Operation<usize>)) {
            operate(self);
        }

        fn finish(&self) -> operation::Outcome<usize> {
            operation::Outcome::Some(self.focused)
        }
    }

    CountFocused { focused: 0 }
}
