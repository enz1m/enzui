//! `KeyboardCapture`: a transparent wrapper that decodes key presses into
//! messages when a focus-holding descendant is inside.
//!
//! # Why this exists
//!
//! Three widgets in the date family — [`crate::calendar`],
//! [`crate::date_picker`] and [`crate::date_range_picker`] — each ship a
//! `event_for_key` decoder that maps a keystroke onto a `CalendarEvent`,
//! `DatePickerEvent` or `DateRangePickerEvent`. Every one of those decoders
//! is well-tested. And every one of them is unreachable from the running
//! widget: `grep -c 'Event::Keyboard' calendar.rs date_picker.rs
//! date_range_picker.rs` returns 0 across all three. The decoders were
//! wired only to their own unit tests, which is why the defect was invisible
//! — the tests called the decoder directly, not the widget.
//!
//! This is a recurring integration hazard: **a green suite means only that
//! the things a test thought to assert
//! hold.** A widget-level test would have caught it because a widget-level
//! test would have driven a real `Event::Keyboard` and read the emitted
//! message, which is exactly what the tests below now do.
//!
//! # What this owns
//!
//! One decision, taken once for every consumer: when to *route* a keystroke
//! to a decoder. Specifically:
//!
//! 1. Only after the child has seen it. A focused control inside the
//!    calendar (a day cell's [`crate::focus_button::FocusButton`]) claims
//!    Enter/Space to activate itself, and only an *unclaimed* keystroke
//!    belongs to the calendar's own key routing. This is the same idiom
//!    `modal_host`'s internal `EscapeCapture` follows — the child goes first,
//!    then the wrapper.
//! 2. Only when the subtree actually holds focus. A calendar embedded in
//!    a form must not eat the application's arrow keys when the user is
//!    typing in a text field somewhere else. See
//!    [`crate::focus_button::any_focused_in`].
//! 3. Both Pressed and Repeat. Holding an arrow key must move the cursor
//!    on every OS-generated repeat, not just the initial press.
//!
//! What it deliberately does not own is *the decoding rule itself*. The
//! decoder is a closure the caller supplies: [`crate::calendar::event_for_key`]
//! for a standalone calendar,
//! [`crate::date_picker::event_for_open_key`] for an open date picker, and
//! [`crate::date_picker::event_for_trigger_key`] for a closed one. A generic
//! wrapper with per-widget decoders is the shape that lets three consumers
//! share transport without any of them owning the others' key vocabulary.

use iced::advanced::{Widget, layout, mouse, overlay, renderer, widget as widget_advanced};
use iced::{Element, Event, Length, Rectangle, Size, Vector, keyboard};

/// A decoder function: given a key and its modifiers, produce the message the
/// caller wants published — or `None` for keys that must bubble unchanged.
///
/// `dyn Fn` rather than a generic parameter for the same reason
/// `crate::calendar::Resolver` is boxed: the wrapper is instantiated by
/// several sites that each supply a different closure, and threading a type
/// parameter through the whole widget chain would infect every caller.
pub type Decoder<'a, Message> =
    Box<dyn Fn(&keyboard::Key, keyboard::Modifiers) -> Option<Message> + 'a>;

/// A [`roving_grid`] decoder: the same key and modifiers, plus the **laid-out
/// bounds of the roving cursor's cell**.
///
/// The extra argument exists because a keyboard context-menu invocation
/// (<kbd>Menu</kbd> / <kbd>Shift+F10</kbd>) has to be *anchored* somewhere, and
/// the convention this library already follows
/// ([`crate::context_target`]'s routed-request arm) is "the centre of the
/// target's own bounds". For a roving grid the target is one cell, whose
/// rectangle is known only after layout — a decoder holding only the focused
/// date could report the day but would have to invent the point, and a menu
/// anchored at the origin is worse than no menu.
///
/// It is the same rectangle the owner paints its cursor ring on, resolved
/// through the caller's own `cursor_bounds` closure, so the ring the user sees
/// and the anchor the menu opens at cannot disagree. `None` when there is no
/// cursor cell to anchor to (a grid whose cursor is not in the visible page).
pub type CursorDecoder<'a, Message> =
    Box<dyn Fn(&keyboard::Key, keyboard::Modifiers, Option<Rectangle>) -> Option<Message> + 'a>;

/// Wrap `child` so that key presses reaching it are decoded through
/// `decode`, and the resulting message published only when a descendant is
/// focused.
///
/// A free function so callers do not have to spell out the widget type; the
/// three date widgets each build one identical `.into()` from it.
pub fn keyboard_capture<'a, Message: Clone + 'a>(
    child: impl Into<Element<'a, Message>>,
    decode: impl Fn(&keyboard::Key, keyboard::Modifiers) -> Option<Message> + 'a,
) -> Element<'a, Message> {
    Element::new(KeyboardCapture {
        child: child.into(),
        decode: Box::new(decode),
    })
}

/// Wrap `child` so that key presses are decoded through `decode` **whether or
/// not** any descendant is focused.
///
/// The date-picker trigger uses this: the field itself is one focusable
/// [`crate::focus_button::FocusButton`], and Enter/Space on it are already
/// claimed by the button. This variant is for the *closed-field* Up/Down
/// case, where the field is focused (so `any_focused_in` is true) — same
/// behaviour, and named separately only so a future consumer that legitimately
/// wants global routing (say, a keyboard shortcut) has a place to reach.
///
/// A wrapper that *must not* eat keys when nothing in the subtree has focus
/// wants [`keyboard_capture`] instead.
pub fn keyboard_capture_unscoped<'a, Message: Clone + 'a>(
    child: impl Into<Element<'a, Message>>,
    decode: impl Fn(&keyboard::Key, keyboard::Modifiers) -> Option<Message> + 'a,
) -> Element<'a, Message> {
    Element::new(KeyboardCaptureUnscoped {
        child: child.into(),
        decode: Box::new(decode),
    })
}

/// Wrap `child` so key presses are decoded through `decode` when any focus
/// owner in the subtree held focus at the moment of the press: a
/// [`FocusButton`](crate::focus_button::FocusButton), a roving grid, **or a
/// live text input**.
///
/// [`keyboard_capture`] scopes on the widget-level focus owners only, so a
/// composite whose focus sits in a text input — an inline reply editor, an
/// inline rename, a filter box — cannot use it: iced keeps a single-line
/// input's focus in its own `text_input::State`, which
/// [`crate::focus_button::any_focused_in`] cannot see. Widening
/// [`keyboard_capture`] instead would hand a focused caret's arrow keys to the
/// date family's cursor decoders.
///
/// Focus is sampled *before* the child sees the key, because iced's
/// `text_input` unfocuses itself on Escape without capturing it
/// (`iced/widget/src/text_input.rs`), so a scope check taken afterwards
/// reads "nothing is focused" for the key this variant exists to route. The
/// child still decides *capture* first, so an input mid-composition — an IME
/// pre-edit, an in-flight paste, a drag-selection — keeps the Escape that
/// aborts it.
pub fn keyboard_capture_any_focus<'a, Message: Clone + 'a>(
    child: impl Into<Element<'a, Message>>,
    decode: impl Fn(&keyboard::Key, keyboard::Modifiers) -> Option<Message> + 'a,
) -> Element<'a, Message> {
    Element::new(KeyboardCaptureAnyFocus {
        child: child.into(),
        decode: Box::new(decode),
    })
}

/// The scoped variant (see [`keyboard_capture`]).
struct KeyboardCapture<'a, Message> {
    child: Element<'a, Message>,
    decode: Decoder<'a, Message>,
}

/// The any-focus-owner variant (see [`keyboard_capture_any_focus`]).
struct KeyboardCaptureAnyFocus<'a, Message> {
    child: Element<'a, Message>,
    decode: Decoder<'a, Message>,
}

/// The unscoped variant (see [`keyboard_capture_unscoped`]).
struct KeyboardCaptureUnscoped<'a, Message> {
    child: Element<'a, Message>,
    decode: Decoder<'a, Message>,
}

// Transparent keyboard wrappers share the child’s tree node.
macro_rules! kc_delegate {
    () => {
        fn tag(&self) -> widget_advanced::tree::Tag {
            self.child.as_widget().tag()
        }

        fn state(&self) -> widget_advanced::tree::State {
            self.child.as_widget().state()
        }

        fn diff(&mut self, tree: &mut widget_advanced::Tree) {
            self.child.as_widget_mut().diff(tree);
        }

        fn size(&self) -> Size<Length> {
            self.child.as_widget().size()
        }

        fn layout(
            &mut self,
            tree: &mut widget_advanced::Tree,
            renderer: &iced::Renderer,
            limits: &layout::Limits,
        ) {
            self.child.as_widget_mut().layout(tree, renderer, limits);
        }

        fn draw(
            &self,
            tree: &widget_advanced::Tree,
            renderer: &mut iced::Renderer,
            theme: &iced::Theme,
            style: &renderer::Style,
            layout: layout::Layout,
            cursor: mouse::Cursor,
            viewport: &Rectangle,
        ) {
            self.child
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
        }

        fn mouse_interaction(
            &self,
            tree: &widget_advanced::Tree,
            layout: layout::Layout,
            cursor: mouse::Cursor,
            viewport: &Rectangle,
            renderer: &iced::Renderer,
        ) -> mouse::Interaction {
            self.child
                .as_widget()
                .mouse_interaction(tree, layout, cursor, viewport, renderer)
        }

        fn operate(
            &mut self,
            tree: &mut widget_advanced::Tree,
            layout: layout::Layout,
            viewport: &Rectangle,
            renderer: &iced::Renderer,
            operation: &mut dyn widget_advanced::Operation,
        ) {
            self.child
                .as_widget_mut()
                .operate(tree, layout, viewport, renderer, operation);
        }

        fn overlay<'b>(
            &'b mut self,
            tree: &'b mut widget_advanced::Tree,
            layout: layout::Layout,
            renderer: &iced::Renderer,
            viewport: &Rectangle,
            translation: Vector,
            window: Size,
        ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
            self.child.as_widget_mut().overlay(
                tree,
                layout,
                renderer,
                viewport,
                translation,
                window,
            )
        }

        fn frame_interest(&self, tree: &widget_advanced::Tree) -> bool {
            self.child.as_widget().frame_interest(tree)
        }

        // Frame events bypass input decoding to avoid redundant descendant updates.
        fn update_frame(
            &mut self,
            tree: &mut widget_advanced::Tree,
            event: &Event,
            layout: layout::Layout,
            cursor: mouse::Cursor,
            renderer: &iced::Renderer,
            shell: &mut iced::advanced::Shell<'_, Message>,
            viewport: &Rectangle,
        ) {
            self.child
                .as_widget_mut()
                .update_frame(tree, event, layout, cursor, renderer, shell, viewport);
        }
    };
}

macro_rules! transparent_passthrough {
    ($ty:ident, $decode_call:expr) => {
        impl<Message> Widget<Message, iced::Theme, iced::Renderer> for $ty<'_, Message>
        where
            Message: Clone,
        {
            kc_delegate!();

            fn update(
                &mut self,
                tree: &mut widget_advanced::Tree,
                event: &Event,
                layout: layout::Layout,
                cursor: mouse::Cursor,
                renderer: &iced::Renderer,
                shell: &mut iced::advanced::Shell<'_, Message>,
                viewport: &Rectangle,
            ) {
                // Child first, exactly like `EscapeCapture`: a focused day
                // cell owns Enter/Space to activate itself, and only what the
                // child leaves alone belongs to our routing.
                self.child
                    .as_widget_mut()
                    .update(tree, event, layout, cursor, renderer, shell, viewport);

                if shell.is_event_captured() {
                    return;
                }

                let Event::Keyboard(keyboard::Event::KeyPressed {
                    key,
                    modifiers,
                    physical_key,
                    ..
                }) = event
                else {
                    return;
                };

                // The scoping check is inlined per-variant here — the
                // unscoped wrapper skips it, the scoped one runs it.
                if !$decode_call(self, tree) {
                    return;
                }

                if let Some(message) = decode_by_layout_or_position(key, *physical_key, |key| {
                    (self.decode)(key, *modifiers)
                }) {
                    shell.publish(message);
                    shell.capture_event();
                }
            }
        }
    };
}

/// Decodes `key` as typed; on a non-Latin layout, a key the decoder does not
/// recognise is decoded again as the Latin letter at its position, so Ctrl+С on
/// a Russian keyboard reaches a Ctrl+C binding.
fn decode_by_layout_or_position<Message>(
    key: &keyboard::Key,
    physical_key: keyboard::key::Physical,
    decode: impl Fn(&keyboard::Key) -> Option<Message>,
) -> Option<Message> {
    decode(key).or_else(|| {
        let latin = enzui_core::shortcut::latin_fallback(key, physical_key)?;
        decode(&keyboard::Key::Character(latin.to_string().into()))
    })
}

// Scoped: only fire when a descendant focus owner is focused. This includes
// both ordinary FocusButtons and the calendar's roving-grid owner; otherwise
// Escape would stop routing the moment focus correctly left the trigger.
fn roving_grid_focused_in(tree: &widget_advanced::Tree) -> bool {
    if tree.tag == widget_advanced::tree::Tag::of::<RovingGridState>() {
        return tree
            .state
            .downcast_ref::<RovingGridState>()
            .focus
            .is_focused();
    }
    tree.children.iter().any(roving_grid_focused_in)
}

transparent_passthrough!(KeyboardCapture, |_this: &Self,
                                           tree: &widget_advanced::Tree|
 -> bool {
    crate::focus_button::any_focused_in(tree) || roving_grid_focused_in(tree)
});

// Unscoped: always fire. Reserved for a caller that explicitly wants global
// routing; the date family does not use this today but the shape needs to be
// available to reach into shortcut land later without a second widget.
transparent_passthrough!(KeyboardCaptureUnscoped, |_this: &Self,
                                                   _tree: &widget_advanced::Tree|
 -> bool { true });

/// Whether a live single-line input inside `tree` currently holds the caret.
///
/// iced keeps a `text_input`'s focus in its own retained `text_input::State`,
/// which the `FocusButton` walk in [`crate::focus_button::any_focused_in`]
/// cannot reach. [`crate::field_chrome::focused_input_in`] answers the
/// narrower question of the first input state node in a subtree; this
/// scope is any focused input, so a composite holding a second field keeps
/// routing when the later one holds the caret.
fn text_input_focused_in(tree: &widget_advanced::Tree) -> bool {
    type InputState = iced::widget::text_input::State<
        <iced::Renderer as iced::advanced::text::Renderer>::Paragraph,
    >;

    if tree.tag == widget_advanced::tree::Tag::of::<InputState>() {
        return tree.state.downcast_ref::<InputState>().is_focused();
    }
    tree.children.iter().any(text_input_focused_in)
}

/// Whether any focus owner this crate can observe holds focus inside `tree`.
fn any_focus_owner_in(tree: &widget_advanced::Tree) -> bool {
    crate::focus_button::any_focused_in(tree)
        || roving_grid_focused_in(tree)
        || text_input_focused_in(tree)
}

// Not a `transparent_passthrough!` arm: focus is sampled before the child
// update, which that macro's shape cannot express. See
// `keyboard_capture_any_focus` for why the ordering is load-bearing.
impl<Message> Widget<Message, iced::Theme, iced::Renderer> for KeyboardCaptureAnyFocus<'_, Message>
where
    Message: Clone,
{
    kc_delegate!();

    fn update(
        &mut self,
        tree: &mut widget_advanced::Tree,
        event: &Event,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let was_focused = any_focus_owner_in(tree);

        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);

        if shell.is_event_captured() {
            return;
        }

        let Event::Keyboard(keyboard::Event::KeyPressed {
            key,
            modifiers,
            physical_key,
            ..
        }) = event
        else {
            return;
        };

        if !was_focused {
            return;
        }

        if let Some(message) =
            decode_by_layout_or_position(key, *physical_key, |key| (self.decode)(key, *modifiers))
        {
            shell.publish(message);
            shell.capture_event();
        }
    }
}

impl<'a, Message: 'a> std::fmt::Debug for KeyboardCaptureAnyFocus<'a, Message> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyboardCaptureAnyFocus")
            .finish_non_exhaustive()
    }
}

impl<'a, Message: 'a> std::fmt::Debug for KeyboardCapture<'a, Message> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyboardCapture").finish_non_exhaustive()
    }
}

impl<'a, Message: 'a> std::fmt::Debug for KeyboardCaptureUnscoped<'a, Message> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyboardCaptureUnscoped")
            .finish_non_exhaustive()
    }
}

/// Consumes key presses matching `guard` before `child` receives them.
///
/// Matched events are not republished. Other events pass through unchanged.
pub fn keyboard_precapture<'a, Message: Clone + 'a>(
    child: impl Into<Element<'a, Message>>,
    guard: impl Fn(&keyboard::Key, keyboard::Modifiers) -> bool + 'a,
) -> Element<'a, Message> {
    Element::new(KeyboardPreGuard {
        child: child.into(),
        guard: Box::new(guard),
    })
}

type KeyGuard<'a> = dyn Fn(&keyboard::Key, keyboard::Modifiers) -> bool + 'a;

struct KeyboardPreGuard<'a, Message> {
    child: Element<'a, Message>,
    guard: Box<KeyGuard<'a>>,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for KeyboardPreGuard<'_, Message>
where
    Message: Clone,
{
    kc_delegate!();

    fn update(
        &mut self,
        tree: &mut widget_advanced::Tree,
        event: &Event,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event
            && (self.guard)(key, *modifiers)
        {
            shell.capture_event();
            return;
        }
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }
}

impl<'a, Message: 'a> std::fmt::Debug for KeyboardPreGuard<'a, Message> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyboardPreGuard").finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// The roving-tabindex owner
// ---------------------------------------------------------------------------

/// Per-grid state: a single focus stop for the whole grid.
///
/// The *roving cursor* itself is deliberately not here — a calendar's
/// cursor lives in the host's [`enzui_core::calendar_nav::MonthController`],
/// and duplicating it in widget state would let focus and selection drift. This
/// widget owns only "is the grid the current Tab stop", which is genuinely widget
/// state because it survives a re-render and iced's focus operations write it.
#[derive(Debug, Clone, Copy, Default)]
struct RovingGridState {
    focus: enzui_core::focus::FocusState,
    /// Per-window modality key: captured in `update`, read in
    /// `draw`, so a commit in another window does not hide the cursor ring.
    window_key: u64,
}

impl RovingGridState {
    fn new(focused: bool) -> Self {
        let mut focus = enzui_core::focus::FocusState::new();
        if focused {
            focus.focus();
        }
        Self {
            focus,
            window_key: enzui_core::focus::KEY_UNKEYED,
        }
    }
}

impl iced::advanced::widget::operation::Focusable for RovingGridState {
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

/// Wrap a grid `child` so the whole grid is one Tab stop with arrow-key
/// roving focus inside it, instead of one Tab stop per cell.
///
/// A 42-cell calendar grid built from 42 focusable
/// [`crate::focus_button::FocusButton`]s makes a keyboard user press Tab 42
/// times to leave the widget; the WAI-ARIA grid pattern is a single tab stop
/// where arrows move a roving cursor. The cells become
/// [`embedded`](crate::focus_button::FocusButton::embedded) (withdrawn from
/// the focus ring) and this owner becomes the single focusable.
///
/// `enabled` gates focusability: a disabled grid is not a Tab stop at all,
/// matching [`crate::focus_button`]'s rule for a button with no `on_press`
/// and [`crate::radio_group`]'s rule for a group with no reachable option.
///
/// The decoder is the same [`crate::calendar::event_for_key`] the scoped
/// [`keyboard_capture`] used; the only difference is *what holds the focus*.
/// Because this owner is the focusable, a `KeyPressed` reaches the decoder
/// exactly when the grid is the focused control — which is the roving-tabindex
/// contract.
///
/// # The roving-cursor ring is painted by the owner, not the cell
///
/// A day cell must not set its own `focus_visible`, because that also
/// drives the cell's AT `focused` flag, and two calendars sharing a
/// controller (the probe's disabled twin; two live calendars on the same
/// month) would then each report a focused cell — and `collect.rs` panics on
/// two nodes claiming focus in one tree. The WAI-ARIA answer, and the one
/// [`crate::radio_group`] already uses, is for the owner to paint the
/// cursor ring, and only when the owner itself holds focus. `cursor_bounds`
/// locates the cursor cell from the child's laid-out tree (the calendar knows
/// its own structure), and `ring` is the ring colour resolved from the live
/// theme. Both are `None` for a grid with no cursor to draw.
pub fn roving_grid<'a, Message: Clone + 'a>(
    child: impl Into<Element<'a, Message>>,
    enabled: bool,
    owner: RovingGridOwner,
    cursor_bounds: impl Fn(layout::Layout, &iced::advanced::widget::Tree) -> Option<Rectangle> + 'a,
    ring: impl Fn(&iced::Theme) -> RovingRing + 'a,
    decode: impl Fn(&keyboard::Key, keyboard::Modifiers, Option<Rectangle>) -> Option<Message> + 'a,
) -> Element<'a, Message> {
    Element::new(RovingGrid {
        child: child.into(),
        enabled,
        id: owner.id,
        semantics: owner.semantics,
        focus_on_mount: owner.focus_on_mount,
        cursor_bounds: Box::new(cursor_bounds),
        ring: Box::new(ring),
        decode: Box::new(decode),
    })
}

/// Stable identity and live-owner policy for a [`roving_grid`].
#[derive(Debug, Clone)]
pub struct RovingGridOwner {
    /// Stable focus and routing id.
    pub id: iced::advanced::widget::Id,
    /// Base semantics; live focus is applied by the widget state.
    pub semantics: enzui_core::Semantics,
    /// Whether newly-mounted state starts focused.
    pub focus_on_mount: bool,
}

impl RovingGridOwner {
    /// Create an identified owner.
    #[must_use]
    pub fn new(
        id: iced::advanced::widget::Id,
        semantics: enzui_core::Semantics,
        focus_on_mount: bool,
    ) -> Self {
        Self {
            id,
            semantics,
            focus_on_mount,
        }
    }
}

/// Whether the owner should paint the roving-cursor ring this frame: only when
/// it holds keyboard focus and the last input was the keyboard.
///
/// A free function so the gate is reachable from a unit test — a style/draw
/// closure is not (the lesson `calendar::resolve_tokens` was extracted for).
/// A click focuses the grid but must
/// not leave a ring, because the ring is a keyboard affordance.
#[must_use]
pub fn cursor_ring_visible(focused: bool) -> bool {
    cursor_ring_visible_for(focused, enzui_core::focus::KEY_UNKEYED)
}

/// Per-window counterpart of [`cursor_ring_visible`]: gate the
/// roving-cursor ring on the modality of the window identified by `key` (from
/// `enzui_core::focus::window_key`), so a commit gesture in another window
/// does not hide this grid's cursor ring.
#[must_use]
pub fn cursor_ring_visible_for(focused: bool, key: u64) -> bool {
    enzui_core::focus::focus_ring_visible_for(focused, key)
}

/// The colour and geometry of the roving-cursor ring the owner paints.
#[derive(Debug, Clone, Copy)]
pub struct RovingRing {
    /// Ring colour.
    pub color: iced::Color,
    /// Ring stroke width.
    pub width: f32,
    /// Corner radius.
    pub radius: f32,
}

/// Resolves the cursor cell from the child's layout and tree.
type CursorBounds<'a> =
    Box<dyn Fn(layout::Layout, &iced::advanced::widget::Tree) -> Option<Rectangle> + 'a>;

/// The roving-tabindex owner widget (see [`roving_grid`]).
struct RovingGrid<'a, Message> {
    child: Element<'a, Message>,
    enabled: bool,
    id: iced::advanced::widget::Id,
    semantics: enzui_core::Semantics,
    focus_on_mount: bool,
    /// Resolves the cursor cell from the child's layout and tree.
    cursor_bounds: CursorBounds<'a>,
    ring: Box<dyn Fn(&iced::Theme) -> RovingRing + 'a>,
    decode: CursorDecoder<'a, Message>,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for RovingGrid<'_, Message>
where
    Message: Clone,
{
    fn tag(&self) -> widget_advanced::tree::Tag {
        widget_advanced::tree::Tag::of::<RovingGridState>()
    }

    fn state(&self) -> widget_advanced::tree::State {
        widget_advanced::tree::State::new(RovingGridState::new(self.enabled && self.focus_on_mount))
    }

    fn diff(&mut self, tree: &mut widget_advanced::Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut widget_advanced::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn draw(
        &self,
        tree: &widget_advanced::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );

        // The roving-cursor ring is the owner's to paint, and only while the
        // owner holds keyboard focus and the modality is keyboard-last (so a
        // pointer click does not leave a ring behind). Painting
        // it here rather than on the cell is what keeps two calendars sharing a
        // controller from both claiming AT focus (the `collect.rs` panic).
        let (focused, window_key) = {
            let state = tree.state.downcast_ref::<RovingGridState>();
            (state.focus.is_focused(), state.window_key)
        };
        if !cursor_ring_visible_for(focused, window_key) {
            return;
        }
        let Some(bounds) = (self.cursor_bounds)(layout, &tree.children[0]) else {
            return;
        };
        let ring = (self.ring)(theme);
        use iced::advanced::Renderer as _;
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: iced::Border {
                    color: ring.color,
                    width: ring.width,
                    radius: ring.radius.into(),
                },
                shadow: iced::Shadow::default(),
                snap: true,
            },
            iced::Background::Color(iced::Color::TRANSPARENT),
        );
    }

    fn mouse_interaction(
        &self,
        tree: &widget_advanced::Tree,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut widget_advanced::Tree,
        layout: layout::Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn widget_advanced::Operation,
    ) {
        let state = tree.state.downcast_mut::<RovingGridState>();
        // Publish the grid owner itself from the same live focus state used by
        // keyboard routing and ring painting. A builder-time `focused` bit
        // would drift as soon as Tab or a pointer moved focus.
        let mut owner = enzui_core::collect::SemanticsNode::group(
            self.semantics.clone().focused(state.focus.is_focused()),
        );
        operation.custom(Some(&self.id), layout.bounds(), &mut owner);

        // One identified focus stop for the whole grid. A disabled grid is not
        // a Tab stop and drops any focus it still held so Tab can move past it.
        if self.enabled {
            operation.focusable(Some(&self.id), layout.bounds(), state);
        } else if state.focus.is_focused() {
            state.focus.unfocus();
        }
        // Descend so rows and embedded cells publish their AT nodes.
        operation.container(Some(&self.id), layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                viewport,
                renderer,
                operation,
            );
        });
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget_advanced::Tree,
        layout: layout::Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }

    fn update(
        &mut self,
        tree: &mut widget_advanced::Tree,
        event: &Event,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // Keep this window's input modality current, so the focus ring is
        // suppressed on a pointer-last frame — matching `radio_group`. Keyed
        // per window and stashed for `draw`.
        let window_key = enzui_core::focus::window_key(shell.window());
        tree.state.downcast_mut::<RovingGridState>().window_key = window_key;
        enzui_core::focus::note_input_event_for(event, window_key);

        // Child first: an embedded cell still handles its own pointer click
        // (it publishes `DaySelected`), and only what the child leaves alone
        // is ours. This is the `EscapeCapture` idiom.
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);

        let bounds = layout.bounds();

        // Exclusive pointer focus, against the *grid's* bounds — the grid is
        // the focusable, not the cell. A press inside focuses the grid; a
        // press outside drops it. Runs even when disabled so a disabled grid
        // sheds any stale focus and never takes new focus.
        //
        // The calendar's header carries NON-embedded focus_buttons of its
        // own (the paging arrows and the month/year chooser title). The child
        // updates first, so if one of those just took the press, it now holds
        // focus. The grid owner must not also take focus in that case — two
        // focus claims (Grid owner + header Button) in one tree is precisely
        // what `collect/ak.rs` rejects. So the owner defers to a
        // descendant that claimed the press: if any descendant focusable is now
        // focused, the owner drops rather than doubling the claim.
        if let Some(decision) = enzui_core::focus::press_focus_from_cursor(event, cursor, bounds) {
            use enzui_core::focus::PressFocus;
            let child_took_focus = crate::focus_button::any_focused_in(&tree.children[0]);
            let live = self.enabled;
            let state = tree.state.downcast_mut::<RovingGridState>();
            let was_focused = state.focus.is_focused();
            match decision {
                // A descendant focusable (a header arrow / the chooser title)
                // already claimed this press; the owner must not double-claim.
                PressFocus::Take if child_took_focus => state.focus.unfocus(),
                PressFocus::Take if live => state.focus.focus(),
                PressFocus::Take | PressFocus::Drop => state.focus.unfocus(),
                PressFocus::Leave => {}
            }
            if state.focus.is_focused() != was_focused {
                shell.request_redraw();
            }
        }

        if shell.is_event_captured() || !self.enabled {
            return;
        }

        let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
            return;
        };

        // Keys only reach the decoder when the grid is the focused control.
        // An unfocused calendar must not eat the application's ArrowDown.
        if !tree
            .state
            .downcast_ref::<RovingGridState>()
            .focus
            .is_focused()
        {
            return;
        }

        // The cursor cell's rectangle is resolved from the live layout, at the
        // moment the key is handled, so a keyboard context invocation anchors
        // where the ring is actually painted this frame.
        let cursor_rect = (self.cursor_bounds)(layout, &tree.children[0]);
        if let Some(message) = (self.decode)(key, *modifiers, cursor_rect) {
            shell.publish(message);
            shell.capture_event();
        }
    }

    fn frame_interest(&self, tree: &widget_advanced::Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut widget_advanced::Tree,
        event: &Event,
        layout: layout::Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_opaque(
            &mut self.child,
            tree,
            0,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }
}

impl<'a, Message: 'a> std::fmt::Debug for RovingGrid<'a, Message> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RovingGrid")
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}
