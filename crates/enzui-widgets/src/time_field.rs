//! A time-of-day field: a shared text field that also steps.
//!
//! The editing is [`crate::text_field`]'s — caret, selection, clipboard, IME
//! and the usual shortcuts all behave as they do in any field, because the same
//! `text_input` is doing the work. This module adds only what a time needs:
//! `ArrowUp`/`ArrowDown`/`PageUp`/`PageDown` step the value, `Enter` and losing
//! focus commit the draft through [`TimeFormat::parse`], and the published node
//! is a `Role::SpinButton` that is both editable and adjustable — one focus
//! target, not a text input shadowed by a spin button.
//!
//! `Escape` over an edit in progress restores the committed text and keeps
//! focus; pressed again on a settled field it does what it does in any field —
//! unfocus, and reach whatever encloses it, so a dialog stays dismissable.
//!
//! The field is *controlled* like the rest of the family: the caller owns the
//! draft string and the committed [`TimeOfDay`], and applies
//! [`TimeFieldEvent`].
//!
//! A time of day is cyclic: stepping past 23:59 wraps to midnight and the field
//! takes no minimum or maximum. "End after start" belongs to the caller,
//! because the pair is date-aware and a cross-midnight event is legitimate.

use enzui_core::focus::{Nav, navigation_from_key};
use enzui_core::semantics::NumericValue;
use enzui_core::time_of_day::{SECONDS_PER_DAY, TimeFormat, TimeOfDay, TimeParseError};
use enzui_theme::Palette;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Id, Operation, Tree, tree};
use iced::advanced::{Shell, Widget, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector, keyboard};
use std::rc::Rc;

use crate::field_chrome::focused_input_in;
use crate::text_field::{TextFieldSize, enz_text_field_owned};
use crate::tokens::FieldVariant;
use crate::value_action::{ValueIntent, ValueRequest};

/// The keyboard step, in minutes, an unconfigured field uses.
pub const DEFAULT_STEP_MINUTES: u32 = 15;

// The step `PageUp` and `PageDown` apply, in seconds.
const PAGE_STEP_SECONDS: u32 = 3600;

// Headroom over the widest time the format can write, so a paste is bounded
// without truncating a legitimate entry mid-edit.
const DRAFT_HEADROOM: usize = 8;

/// What a [`EnzTimeField`] reports to its caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimeFieldEvent {
    /// The text changed. Store it; it is not a time yet, and may never be one.
    Drafted(String),
    /// A time was committed by `Enter`, by losing focus, by an arrow step, or
    /// by a routed assistive-technology action. Store it *and* replace the
    /// draft with the formatted value.
    Committed(TimeOfDay),
    /// A commit was attempted and the text could not be read. The draft is
    /// unchanged so the entry can be corrected; surface it with
    /// [`EnzTimeField::error`].
    Refused(TimeParseError),
}

/// Steps a time of day by `delta` seconds, wrapping at midnight in both
/// directions.
///
/// Total: every `i64` delta is accepted, including [`i64::MIN`] and
/// [`i64::MAX`]. The delta is reduced modulo the day *before* it is added, so
/// no sum can overflow.
#[must_use]
pub fn time_of_day_step(value: TimeOfDay, delta: i64) -> TimeOfDay {
    let day = i64::from(SECONDS_PER_DAY);
    let reduced = delta.rem_euclid(day);
    let wrapped = (i64::from(value.seconds_since_midnight()) + reduced) % day;
    u32::try_from(wrapped)
        .ok()
        .and_then(TimeOfDay::from_seconds_since_midnight)
        .unwrap_or(TimeOfDay::MIDNIGHT)
}

// Only the keys a single-line `text_input` leaves unused. `Home`, `End`,
// `ArrowLeft` and `ArrowRight` move the caret and must keep doing so.
fn step_from_key(event: &keyboard::Event) -> Option<Nav> {
    let keyboard::Event::KeyPressed {
        key: keyboard::Key::Named(named),
        ..
    } = event
    else {
        return None;
    };
    matches!(
        named,
        keyboard::key::Named::ArrowUp
            | keyboard::key::Named::ArrowDown
            | keyboard::key::Named::PageUp
            | keyboard::key::Named::PageDown
    )
    .then(|| navigation_from_key(event))
    .flatten()
}

// Truncates on a character boundary, so a bound never splits a glyph.
fn bound_draft(text: String, cap: usize) -> String {
    match text.char_indices().nth(cap) {
        Some((at, _)) => text.split_at(at).0.to_owned(),
        None => text,
    }
}

#[derive(Debug, Default)]
struct TimeFieldState {
    // Whether the inner input held focus on the previous pass, so losing it can
    // be noticed without this wrapper owning a second focus target.
    was_focused: bool,
    // A routed value action armed during `operate`, applied on the next event
    // pass so the caller's closure runs once per action rather than per frame.
    pending: Option<ValueIntent>,
}

/// A time-of-day field that is typed into like any text field and stepped like
/// a spin button.
///
/// Build one with [`enz_time_field`].
pub struct EnzTimeField<'a, Message> {
    draft: &'a str,
    value: TimeOfDay,
    format: TimeFormat,
    step_seconds: u32,
    label: Option<String>,
    aria_label: Option<String>,
    error: Option<String>,
    description: Option<String>,
    on_event: Rc<dyn Fn(TimeFieldEvent) -> Message + 'a>,
    enabled: bool,
    id: Option<Id>,
    palette: Option<Palette>,
    size: TextFieldSize,
    variant: FieldVariant,
    trailing: Option<Element<'a, Message>>,
}

/// A time field showing `draft`, standing for the committed `value`, reporting
/// through `on_event`.
///
/// `draft` is the text on screen and `value` the time it was last read as; a
/// caller that keeps them in step through [`TimeFieldEvent`] gets a field that
/// displays what was typed and announces what was committed. Defaults to
/// the current locale's hour cycle and catalog day periods (H12 if that fails),
/// a [`DEFAULT_STEP_MINUTES`] step and [`TextFieldSize::Md`].
pub fn enz_time_field<'a, Message>(
    draft: &'a str,
    value: TimeOfDay,
    on_event: impl Fn(TimeFieldEvent) -> Message + 'a,
) -> EnzTimeField<'a, Message> {
    EnzTimeField {
        draft,
        value,
        format: enzui_core::localization::current()
            .time_format()
            .unwrap_or_else(|_| TimeFormat::new(enzui_core::time_of_day::HourCycle::H12)),
        step_seconds: DEFAULT_STEP_MINUTES * 60,
        label: None,
        aria_label: None,
        error: None,
        description: None,
        on_event: Rc::from(on_event),
        enabled: true,
        id: None,
        palette: None,
        size: TextFieldSize::default(),
        variant: FieldVariant::default(),
        trailing: None,
    }
}

impl<'a, Message> EnzTimeField<'a, Message> {
    /// Sets the formatter that both writes and reads this field's text.
    #[must_use]
    pub fn format(mut self, format: TimeFormat) -> Self {
        self.format = format;
        self
    }

    /// Sets the keyboard step in minutes; `0` is ignored.
    ///
    /// Typed times keep their own precision — a 15-minute step does not round
    /// `9:07` on commit.
    #[must_use]
    pub fn step_minutes(mut self, minutes: u32) -> Self {
        self.set_step(minutes.saturating_mul(60));
        self
    }

    /// Sets the keyboard step in seconds; `0` is ignored.
    #[must_use]
    pub fn step_seconds(mut self, seconds: u32) -> Self {
        self.set_step(seconds);
        self
    }

    // A zero step would freeze the arrows while still advertising a numeric
    // step of zero, so it is refused rather than stored.
    fn set_step(&mut self, seconds: u32) {
        if seconds > 0 {
            self.step_seconds = seconds;
        }
    }

    /// Sets the visible and accessible label.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Sets an accessibility-only name, for a row that already paints the
    /// caption beside the field. Consulted only when no visible label is set.
    #[must_use]
    pub fn aria_label(mut self, name: impl Into<String>) -> Self {
        self.aria_label = Some(name.into());
        self
    }

    /// Marks the field invalid and shows `error` under it.
    #[must_use]
    pub fn error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// Sets the helper caption shown when there is no error.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Disables the field: no focus stop, no typing, no stepping.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Sets the widget id routed `Increment`, `Decrement` and `SetValue` reach.
    ///
    /// Required for the spin-button contract: those actions are carried back to
    /// this id, so a field without one publishes an ordinary text input rather
    /// than advertising stepping it cannot deliver.
    #[must_use]
    pub fn id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Pins the palette instead of resolving it from the runtime theme.
    #[must_use]
    pub fn palette(mut self, palette: Palette) -> Self {
        self.palette = Some(palette);
        self
    }

    /// Selects the shared inline-control height tier.
    #[must_use]
    pub fn size(mut self, size: TextFieldSize) -> Self {
        self.size = size;
        self
    }

    /// Selects the field fill variant.
    #[must_use]
    pub fn variant(mut self, variant: FieldVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Places a control inside the field's trailing edge, as
    /// [`EnzTextField::trailing`](crate::text_field::EnzTextField::trailing)
    /// does: a time picker's clock button.
    #[must_use]
    pub fn trailing(mut self, element: impl Into<Element<'a, Message>>) -> Self {
        self.trailing = Some(element.into());
        self
    }

    /// Converts into an [`Element`].
    pub fn into_element(self) -> Element<'a, Message>
    where
        Message: Clone + 'a,
    {
        Element::new(TimeFieldAdapter::new(self))
    }

    // The longest string this format can write, over both hour widths and both
    // day periods.
    fn widest_text(&self) -> usize {
        [0_u32, 8, 10, 18, 23]
            .into_iter()
            .filter_map(|hour| TimeOfDay::new(hour, 58, 58))
            .map(|time| self.format.format(time).chars().count())
            .max()
            .unwrap_or(0)
    }

    // A paste is bounded rather than left to grow: this field's value has a
    // fixed written width, so anything far past it is not an entry in progress.
    fn draft_cap(&self) -> usize {
        self.widest_text() + DRAFT_HEADROOM
    }

    fn stepped(&self, nav: Nav) -> TimeOfDay {
        let step = i64::from(self.step_seconds);
        let page = i64::from(PAGE_STEP_SECONDS);
        match nav {
            Nav::Increment => time_of_day_step(self.value, step),
            Nav::Decrement => time_of_day_step(self.value, -step),
            Nav::PageUp => time_of_day_step(self.value, page),
            Nav::PageDown => time_of_day_step(self.value, -page),
            // Unreachable through `step_from_key`, which never yields them.
            Nav::Home | Nav::End => self.value,
        }
    }

    // A bare non-negative integer is the numeric contract the node published —
    // seconds since midnight, which is what AT-SPI's spin-button path sends
    // back. Anything else is a written time, read by the grammar typing uses.
    fn routed_value(&self, intent: &ValueIntent) -> Result<TimeOfDay, TimeParseError> {
        match intent {
            ValueIntent::Increment => Ok(self.stepped(Nav::Increment)),
            ValueIntent::Decrement => Ok(self.stepped(Nav::Decrement)),
            ValueIntent::Set(text) => match text.trim().parse::<u32>() {
                Ok(seconds) => TimeOfDay::from_seconds_since_midnight(seconds)
                    .ok_or(TimeParseError::OutOfRange),
                Err(_) => self.format.parse(text),
            },
        }
    }

    fn numeric(&self) -> NumericValue {
        NumericValue {
            value: f64::from(self.value.seconds_since_midnight()),
            min: 0.0,
            max: f64::from(SECONDS_PER_DAY - 1),
            step: Some(f64::from(self.step_seconds)),
        }
    }

    fn commit(&self, shell: &mut Shell<'_, Message>) {
        match self.format.parse(self.draft) {
            Ok(time) => shell.publish((self.on_event)(TimeFieldEvent::Committed(time))),
            Err(reason) => shell.publish((self.on_event)(TimeFieldEvent::Refused(reason))),
        }
    }
}

// The narrow wrapper that turns a shared text field into a spin button: it
// intercepts the four stepping keys plus `Enter` and `Escape`, offers the
// routed-value carrier, and notices focus loss. Everything else — every
// character, caret move, selection, paste and input-method commit — is handed
// straight to the field underneath.
struct TimeFieldAdapter<'a, Message> {
    field: EnzTimeField<'a, Message>,
    input: Element<'a, Message>,
}

impl<'a, Message: Clone + 'a> TimeFieldAdapter<'a, Message> {
    fn new(mut field: EnzTimeField<'a, Message>) -> Self {
        let numeric = field.numeric();
        let placeholder = field.format.format(field.value);
        let cap = field.draft_cap();
        let on_event = Rc::clone(&field.on_event);
        let mut input = enz_text_field_owned(
            field.label.clone().unwrap_or_default(),
            placeholder,
            field.draft,
            // The field's only message producer, so a message here is a real
            // edit: a keystroke, a delete, a paste or an input-method commit.
            move |text: String| on_event(TimeFieldEvent::Drafted(bound_draft(text, cap))),
        )
        .size(field.size)
        .variant(field.variant)
        .enabled(field.enabled)
        .input_purpose(iced::advanced::input_method::Purpose::Time);
        // Adjustability is advertised only when this wrapper can actually route
        // the request back: the carrier is keyed on the caller's id, so without
        // one the node would offer `Increment` to nothing.
        if let Some(id) = field.id.clone() {
            input = input.id(id).adjustable_value(numeric);
        }
        if let Some(palette) = field.palette.clone() {
            input = input.palette(palette);
        }
        if let Some(trailing) = field.trailing.take() {
            input = input.trailing(trailing);
        }
        if let Some(error) = field.error.clone() {
            input = input.error(error);
        } else if let Some(description) = field.description.clone() {
            input = input.description(description);
        }
        if field.label.is_none()
            && let Some(aria) = field.aria_label.clone()
        {
            input = input.aria_label(aria);
        }
        Self {
            input: input.into_element(),
            field,
        }
    }

    // Returns whether this wrapper consumed the key. Only the four stepping
    // keys plus a meaningful `Enter` or `Escape`; everything else, including
    // every caret, selection and clipboard chord, falls through untouched.
    fn intercept(&self, event: &keyboard::Event, shell: &mut Shell<'_, Message>) -> bool {
        if let Some(nav) = step_from_key(event) {
            let stepped = self.field.stepped(nav);
            shell.publish((self.field.on_event)(TimeFieldEvent::Committed(stepped)));
            return true;
        }
        let keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(named),
            modifiers,
            ..
        } = event
        else {
            return false;
        };
        if modifiers.control() || modifiers.alt() || modifiers.logo() {
            return false;
        }
        match named {
            keyboard::key::Named::Enter => {
                self.field.commit(shell);
                true
            }
            // A settled field lets Escape reach whatever encloses it; only an
            // edit in progress is worth cancelling here.
            keyboard::key::Named::Escape => {
                let settled = self.field.format.format(self.field.value);
                if settled == self.field.draft {
                    return false;
                }
                shell.publish((self.field.on_event)(TimeFieldEvent::Drafted(settled)));
                true
            }
            _ => false,
        }
    }
}

impl<Message: Clone> Widget<Message, iced::Theme, iced::Renderer>
    for TimeFieldAdapter<'_, Message>
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<TimeFieldState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(TimeFieldState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        // `diff_children` creates the child's state node on first call; this
        // iced's `Widget` has no `children()` hook.
        tree.diff_children(std::slice::from_mut(&mut self.input));
    }

    fn size(&self) -> Size<Length> {
        self.input.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.input
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.input.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );

        if self.field.enabled
            && let Some(id) = &self.field.id
        {
            let mut request = ValueRequest::new(id.clone());
            operation.custom(Some(id), layout.bounds(), &mut request);
            if let Some(intent) = request.take() {
                tree.state.downcast_mut::<TimeFieldState>().pending = Some(intent);
            }
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // Taken unconditionally so a field rebuilt disabled discards an armed
        // action rather than applying it on a later frame.
        let pending = tree.state.downcast_mut::<TimeFieldState>().pending.take();
        if let Some(intent) = pending
            && self.field.enabled
        {
            match self.field.routed_value(&intent) {
                Ok(time) => {
                    shell.publish((self.field.on_event)(TimeFieldEvent::Committed(time)));
                }
                Err(reason) => {
                    shell.publish((self.field.on_event)(TimeFieldEvent::Refused(reason)));
                }
            }
            shell.request_redraw();
        }

        let focused = focused_input_in::<iced::Renderer>(&tree.children[0]).unwrap_or(false);
        if self.field.enabled
            && focused
            && let Event::Keyboard(key_event) = event
            && self.intercept(key_event, shell)
        {
            shell.capture_event();
            shell.request_redraw();
            return;
        }

        self.input.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );

        // Focus can be lost to a click elsewhere or to a focus operation, and
        // neither leaves an event this wrapper could match on. Comparing the
        // input's own flag across passes is what makes blur observable without
        // a second focus target.
        let now_focused = focused_input_in::<iced::Renderer>(&tree.children[0]).unwrap_or(false);
        let state = tree.state.downcast_mut::<TimeFieldState>();
        if state.was_focused && !now_focused && self.field.enabled {
            state.was_focused = false;
            self.field.commit(shell);
        } else {
            state.was_focused = now_focused;
        }
        crate::frame_routing::refresh_opaque_child(&self.input, tree, 0);
    }

    // The adapter owns the input's tree node, so frame interest and frame
    // dispatch come from the input; the default interest would keep every
    // time field on the frame route.
    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_opaque(
            &mut self.input,
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

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.input.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.input.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.input.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: Clone + 'a> From<EnzTimeField<'a, Message>> for Element<'a, Message> {
    fn from(field: EnzTimeField<'a, Message>) -> Self {
        field.into_element()
    }
}
