//! Controlled single-date field with a calendar popover.
//!
//! Hosts retain [`DatePickerState`] and the selected value, then apply
//! [`DatePickerEvent`] proposals. [`DatePickerState::apply_calendar_event_and_close`]
//! combines selection with closing and returning the captured focus target.
//!
//! Opening with an in-bounds value focuses it; opening without a value retains
//! the existing cursor and browsed month. [`crate::date_range_picker`] supplies
//! the two-click range gesture over the same field chrome and calendar.

use enzui_core::calendar_nav::MonthController;
use enzui_core::calendar_semantics::{CalendarLabels, date_field_semantics};
use enzui_core::civil_date::{Date, DateBounds, Weekday};
use enzui_core::dismissal::{DismissPolicy, Dismissal};
use iced::advanced::widget::Id;
use iced::{Element, Length};
use std::borrow::Cow;

use crate::calendar::{CalendarEvent, enz_calendar};
use crate::date_field::{FieldTrigger, date_field, elevate_popup};
use crate::tokens::FieldSizeTokens;

pub use crate::date_field::{
    DateFieldTokens, FIELD_ICON, FIELD_ICON_SIZE, POPUP_GAP, POPUP_RADIUS, date_popup_placement,
};

/// Returns a nonmodal popover policy: outside presses pass through and Escape closes.
#[must_use]
pub const fn date_dismiss_policy() -> DismissPolicy {
    DismissPolicy::popover()
}

pub(crate) fn calendar_owner_id(trigger: &Id) -> Id {
    enzui_core::identity::derive_id(&format!("{trigger:?}"), "calendar-grid")
}

/// A closing transition’s change flag and captured focus-restoration target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DismissOutcome {
    /// Whether the popup transitioned from open to closed.
    pub changed: bool,
    /// The focus target to restore, captured at open time. Taken exactly once:
    /// a second dismissal of an already-closed popup returns `None` here.
    pub restore_focus: Option<Id>,
}

/// Host-owned popup state, calendar cursor, and persistent trigger identity.
#[derive(Debug, Clone)]
pub struct DatePickerState {
    controller: MonthController,
    open: bool,
    trigger_id: Id,
    restore_focus: Option<Id>,
}

impl DatePickerState {
    /// Creates a closed picker with navigation initialized from `origin` and bounds.
    ///
    /// The caller supplies the origin; no clock is read. A stable default trigger id
    /// is minted once for this state.
    #[must_use]
    pub fn new(origin: Date, first_weekday: Weekday, bounds: DateBounds) -> Self {
        Self {
            controller: MonthController::new(origin, first_weekday, bounds),
            open: false,
            trigger_id: Id::unique(),
            restore_focus: None,
        }
    }

    /// Reports whether picker state is open, independently of widget availability.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Returns the host-owned calendar navigation state.
    #[must_use]
    pub const fn controller(&self) -> &MonthController {
        &self.controller
    }

    /// Returns this state’s persistent default trigger identity.
    ///
    /// Rebuilds using the same state reuse it; an explicit widget id overrides it.
    #[must_use]
    pub fn trigger_id(&self) -> Id {
        self.trigger_id.clone()
    }

    /// Opens the popup and replaces its captured focus-restoration target.
    ///
    /// An in-bounds value becomes focused and visible. None or an out-of-bounds
    /// value leaves navigation unchanged; opening still succeeds.
    pub fn open(&mut self, value: Option<Date>, restore_focus: Option<Id>) {
        self.open = true;
        self.restore_focus = restore_focus;
        if let Some(date) = value {
            let _ = self.controller.focus_date(date);
        }
    }

    /// Closes the popup and takes the captured focus target, leaving navigation unchanged.
    ///
    /// Repeated calls return no target after it has been taken.
    pub const fn close(&mut self) -> Option<Id> {
        self.open = false;
        self.restore_focus.take()
    }

    /// Opens onto the supplied value or closes and returns the captured focus target.
    ///
    /// Opening returns no target.
    pub fn toggle(&mut self, value: Option<Date>, restore_focus: Option<Id>) -> Option<Id> {
        if self.open {
            self.close()
        } else {
            self.open(value, restore_focus);
            None
        }
    }

    /// Closes an open popup when the dismissal requests it, returning change and focus target.
    ///
    /// Ignored decisions and already-closed popups return no change or focus target.
    pub const fn apply_dismissal(&mut self, dismissal: Dismissal) -> DismissOutcome {
        if dismissal.closes() && self.open {
            DismissOutcome {
                changed: true,
                restore_focus: self.close(),
            }
        } else {
            DismissOutcome {
                changed: false,
                restore_focus: None,
            }
        }
    }

    /// Replaces controller bounds without changing the host’s selected value.
    pub fn set_bounds(&mut self, bounds: DateBounds) {
        self.controller.set_bounds(bounds);
    }

    /// Applies a calendar proposal and returns a date to select without closing the popup.
    ///
    /// Hosts store the returned selection; this method does not require the popup to be open.
    pub fn apply_calendar_event(&mut self, event: CalendarEvent) -> Option<Date> {
        crate::calendar::apply_event(&mut self.controller, event)
    }

    /// Applies a calendar proposal and closes on successful day selection.
    ///
    /// Returns the selected date and captured focus target. Navigation and refused
    /// selections return neither and leave the popup open state unchanged.
    pub fn apply_calendar_event_and_close(
        &mut self,
        event: CalendarEvent,
    ) -> (Option<Date>, Option<Id>) {
        let selected = self.apply_calendar_event(event);
        let restore_focus = if selected.is_some() {
            self.close()
        } else {
            None
        };
        (selected, restore_focus)
    }
}

/// Proposed trigger, calendar, and dismissal actions for the host to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatePickerEvent {
    /// The field was activated; the popup should toggle.
    TriggerPressed,
    /// The calendar reported something.
    Calendar(CalendarEvent),
    /// The popup should close (Escape, or a press outside).
    Dismissed,
}

/// A date field with a calendar popover.
pub struct EnzDatePicker<'a, Message> {
    label: Option<Cow<'a, str>>,
    label_visible: bool,
    placeholder: std::borrow::Cow<'a, str>,
    value: Option<Date>,
    state: &'a DatePickerState,
    labels: &'a dyn CalendarLabels,
    on_event: Box<dyn Fn(DatePickerEvent) -> Message + 'a>,
    value_text: Option<String>,
    today: Option<Date>,
    metrics: FieldSizeTokens,
    enabled: bool,
    fill_variant: crate::tokens::FieldVariant,
    width: Length,
    direction: Option<enzui_core::placement::Direction>,
    focus_popup_on_mount: bool,
    id: Option<Id>,
}

/// Builds a single-date picker over a host-owned value and retained state.
///
/// Defaults to an enabled filled field, medium sizing, shrink width, labels in
/// the current locale, a localized placeholder, and popup focus on mount.
pub fn enz_date_picker<'a, Message>(
    value: Option<Date>,
    state: &'a DatePickerState,
    on_event: impl Fn(DatePickerEvent) -> Message + 'a,
) -> EnzDatePicker<'a, Message> {
    EnzDatePicker {
        label: None,
        label_visible: true,
        placeholder: enzui_core::localization::text("enzui-date-picker-pick-a-date").into(),
        value,
        state,
        labels: &enzui_core::localization::LocalizedCalendarLabels,
        on_event: Box::new(on_event),
        value_text: None,
        today: None,
        metrics: FieldSizeTokens::MD,
        enabled: true,
        fill_variant: crate::tokens::FieldVariant::default(),
        width: Length::Shrink,
        direction: None,
        focus_popup_on_mount: true,
        id: None,
    }
}

impl<'a, Message: Clone + 'a> EnzDatePicker<'a, Message> {
    /// Overrides the persistent default trigger identity held by picker state.
    ///
    /// Capture this same id at open time when restoring focus after dismissal.
    #[must_use]
    pub fn id(mut self, id: Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Sets the painted label and accessible name.
    ///
    /// Defaults to no painted label; unnamed triggers use the locale’s field name.
    #[must_use]
    pub fn label(mut self, label: impl Into<Cow<'a, str>>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Choose whether the field label is painted above the trigger.
    ///
    /// The default is `true`. Set this to `false` when a form lays the same
    /// label beside the field; the string supplied through [`Self::label`]
    /// remains the trigger's accessible name.
    #[must_use]
    pub const fn label_visible(mut self, visible: bool) -> Self {
        self.label_visible = visible;
        self
    }

    /// Sets text shown when no value is displayed.
    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<std::borrow::Cow<'a, str>>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Overrides the scoped locale's date formatting and accessibility vocabulary.
    #[must_use]
    pub const fn labels(mut self, labels: &'a dyn CalendarLabels) -> Self {
        self.labels = labels;
        self
    }

    /// Overrides visible text and gives it value ink even when the selected date is none.
    ///
    /// Accessible date value still comes from the selected date and locale labels.
    #[must_use]
    pub fn value_text(mut self, text: impl Into<String>) -> Self {
        self.value_text = Some(text.into());
        self
    }

    /// Sets the date highlighted as today in the popup; defaults to none and never reads a clock.
    #[must_use]
    pub const fn today(mut self, today: Option<Date>) -> Self {
        self.today = today;
        self
    }

    /// Sets the field sizing preset; defaults to medium.
    #[must_use]
    pub fn size(mut self, size: crate::text_field::TextFieldSize) -> Self {
        self.metrics = size.tokens();
        self
    }

    /// Sets trigger fill treatment; defaults to [`crate::tokens::FieldVariant::Filled`].
    #[must_use]
    pub const fn variant(mut self, variant: crate::tokens::FieldVariant) -> Self {
        self.fill_variant = variant;
        self
    }

    /// Enables interaction; defaults to true.
    ///
    /// A disabled widget hides an open popup without changing the host-owned open state.
    #[must_use]
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Sets field width; popup calendar width is independent.
    #[must_use]
    pub const fn width(mut self, width: Length) -> Self {
        self.width = width;
        self
    }

    /// Pins trigger layout, popup alignment, calendar columns, and horizontal navigation direction.
    ///
    /// Without an override, all follow the ambient direction.
    #[must_use]
    pub const fn direction(mut self, direction: enzui_core::placement::Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Controls calendar focus on popup mount; defaults to true.
    ///
    /// Retained calendar state avoids taking focus again on each rebuild.
    #[must_use]
    pub const fn focus_popup_on_mount(mut self, focus: bool) -> Self {
        self.focus_popup_on_mount = focus;
        self
    }

    fn display_text(&self) -> String {
        self.value_text.clone().unwrap_or_else(|| match self.value {
            Some(date) => self.labels.display_date(date),
            None => self.placeholder.to_string(),
        })
    }

    const fn shows_value(&self) -> bool {
        self.value.is_some() || self.value_text.is_some()
    }

    const fn popup_is_live(&self) -> bool {
        self.enabled && self.state.open
    }

    /// Builds the field and, when enabled and open, its calendar popup.
    pub fn into_element(self) -> Element<'a, Message> {
        let resolve = crate::date_field::field_resolver_variant(self.enabled, self.fill_variant);

        let display_text = self.display_text();
        let shows_value = self.shows_value();
        let popup_live = self.popup_is_live();
        let semantics = date_field_semantics(
            self.label.as_deref(),
            self.value,
            popup_live,
            self.enabled,
            self.labels,
        );
        let open = self.state.open;
        let focused_date = self.state.controller().focused();
        let enabled = self.enabled;
        let focus_popup_on_mount = self.focus_popup_on_mount;

        let on_event: std::rc::Rc<dyn Fn(DatePickerEvent) -> Message + 'a> =
            std::rc::Rc::from(self.on_event);

        let trigger_id = self.id.clone().unwrap_or_else(|| self.state.trigger_id());
        let calendar_id = calendar_owner_id(&trigger_id);

        let trigger = FieldTrigger {
            label: self.label.as_deref().filter(|_| self.label_visible),
            display: display_text,
            shows_value,
            on_press: self.enabled.then({
                let on_event = std::rc::Rc::clone(&on_event);
                move || on_event(DatePickerEvent::TriggerPressed)
            }),
            id: Some(trigger_id),
            embedded: open,
            semantics,
            metrics: self.metrics,
            width: self.width,
            direction: self.direction,
        };

        let resolved_direction = enzui_core::placement::resolve_direction(self.direction);
        let rtl = matches!(
            resolved_direction,
            enzui_core::placement::Direction::RightToLeft
        );

        let popup_on_event = std::rc::Rc::clone(&on_event);
        let direction_for_cal = self.direction;
        let popup = popup_live.then(|| {
            let mut calendar = enz_calendar(self.state.controller(), move |ev| {
                popup_on_event(DatePickerEvent::Calendar(ev))
            })
            .selected(self.value)
            .today(self.today);
            if let Some(dir) = direction_for_cal {
                calendar = calendar.direction(dir);
            }
            calendar = calendar
                .id(calendar_id)
                .focus_on_mount(focus_popup_on_mount);
            surface_press_shield(elevate_popup(
                calendar,
                std::rc::Rc::clone(&resolve),
                self.labels,
            ))
        });

        let field = date_field(trigger, popup, resolve);

        if !enabled {
            return field;
        }

        let dismiss_event = std::rc::Rc::clone(&on_event);
        let keyed = crate::keyboard_capture::keyboard_capture(field, move |key, modifiers| {
            // Closed-trigger Enter/Space are already handled by the focus button.
            if open {
                event_for_open_key(key, modifiers, rtl, focused_date).map(&*on_event)
            } else {
                use iced::keyboard::key;
                if matches!(
                    key,
                    iced::keyboard::Key::Named(key::Named::ArrowDown | key::Named::ArrowUp)
                ) && !(modifiers.control() || modifiers.alt() || modifiers.logo())
                {
                    Some(on_event(DatePickerEvent::TriggerPressed))
                } else {
                    None
                }
            }
        });

        dismiss_on_outside_press(keyed, open, date_dismiss_policy(), move || {
            dismiss_event(DatePickerEvent::Dismissed)
        })
    }
}

impl<'a, Message: Clone + 'a> From<EnzDatePicker<'a, Message>> for Element<'a, Message> {
    fn from(picker: EnzDatePicker<'a, Message>) -> Self {
        picker.into_element()
    }
}

/// Decodes Enter, Space, ArrowDown, and ArrowUp as trigger activation.
///
/// This helper does not inspect modifiers. The widget routes Enter/Space through
/// the trigger and rejects Ctrl, Alt, or Logo on closed-field arrow activation.
#[must_use]
pub fn event_for_trigger_key(key: &iced::keyboard::Key) -> Option<DatePickerEvent> {
    use iced::keyboard::key;
    matches!(
        key,
        iced::keyboard::Key::Named(
            key::Named::Enter | key::Named::Space | key::Named::ArrowDown | key::Named::ArrowUp
        )
    )
    .then_some(DatePickerEvent::TriggerPressed)
}

/// Decodes Escape as dismissal and delegates other keys to calendar navigation and activation.
///
/// Escape dismisses even with modifiers; unclaimed calendar keys return none.
#[must_use]
pub fn event_for_open_key(
    key: &iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
    rtl: bool,
    focused: Date,
) -> Option<DatePickerEvent> {
    use iced::keyboard::key;
    if matches!(key, iced::keyboard::Key::Named(key::Named::Escape)) {
        return Some(DatePickerEvent::Dismissed);
    }
    crate::calendar::event_for_key(key, modifiers, rtl, focused).map(DatePickerEvent::Calendar)
}

use iced::advanced::{Widget, layout, mouse, overlay, renderer, widget as widget_advanced};
use iced::{Event, Rectangle, Size, Vector};

use enzui_core::dismissal::{DismissRegions, on_press};

/// Captures unclaimed mouse-button presses inside the popup surface.
///
/// Child widgets handle input first. This prevents padding/header-gap presses
/// from reaching outside dismissal; touch input is not shielded here.
#[must_use]
pub fn surface_press_shield<'a, Message: Clone + 'a>(
    popup: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    Element::new(SurfacePressShield {
        popup: popup.into(),
    })
}

struct SurfacePressShield<'a, Message> {
    popup: Element<'a, Message>,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for SurfacePressShield<'_, Message>
where
    Message: Clone,
{
    fn tag(&self) -> widget_advanced::tree::Tag {
        self.popup.as_widget().tag()
    }

    fn state(&self) -> widget_advanced::tree::State {
        self.popup.as_widget().state()
    }

    fn diff(&mut self, tree: &mut widget_advanced::Tree) {
        self.popup.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.popup.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut widget_advanced::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) {
        self.popup.as_widget_mut().layout(tree, renderer, limits);
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
        self.popup
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
        self.popup
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
        self.popup
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
        self.popup
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
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
        // Let day controls claim presses before protecting the remaining popup surface.
        self.popup
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);

        let inside = match event {
            Event::Mouse(mouse::Event::ButtonPressed(_)) => cursor.is_over(layout.bounds()),
            Event::Touch(iced::touch::Event::FingerPressed { position, .. }) => {
                layout.bounds().contains(*position)
            }
            _ => false,
        };
        if !shell.is_event_captured() && inside {
            shell.capture_event();
        }
    }
}

impl<'a, Message: 'a> std::fmt::Debug for SurfacePressShield<'a, Message> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SurfacePressShield").finish_non_exhaustive()
    }
}

/// Publishes dismissal for unclaimed mouse-button presses and touches
/// according to `policy`.
///
/// Only runs while open and when the cursor position is known. Field bounds
/// are treated as the trigger region. Wrap popup content in [`surface_press_shield`]
/// so unclaimed inside presses do not look outside. Capture follows the policy’s
/// consumption decision; [`date_dismiss_policy`] permits press-through.
#[must_use]
pub fn dismiss_on_outside_press<'a, Message: Clone + 'a>(
    field: impl Into<Element<'a, Message>>,
    open: bool,
    policy: DismissPolicy,
    on_dismiss: impl Fn() -> Message + 'a,
) -> Element<'a, Message> {
    Element::new(DismissOnOutsidePress {
        field: field.into(),
        open,
        policy,
        on_dismiss: Box::new(on_dismiss),
    })
}

struct DismissOnOutsidePress<'a, Message> {
    field: Element<'a, Message>,
    open: bool,
    policy: DismissPolicy,
    on_dismiss: Box<dyn Fn() -> Message + 'a>,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for DismissOnOutsidePress<'_, Message>
where
    Message: Clone,
{
    fn tag(&self) -> widget_advanced::tree::Tag {
        self.field.as_widget().tag()
    }

    fn state(&self) -> widget_advanced::tree::State {
        self.field.as_widget().state()
    }

    fn diff(&mut self, tree: &mut widget_advanced::Tree) {
        self.field.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.field.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut widget_advanced::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) {
        self.field.as_widget_mut().layout(tree, renderer, limits);
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
        self.field
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
        self.field
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
        self.field
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
        self.field
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
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
        self.field
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);

        if !self.open || shell.is_event_captured() {
            return;
        }

        // A touch outside dismisses as a click does; it carries its own place.
        let position = match event {
            Event::Mouse(mouse::Event::ButtonPressed(_)) => cursor.position(),
            Event::Touch(iced::touch::Event::FingerPressed { position, .. }) => Some(*position),
            _ => None,
        };
        let Some(position) = position else {
            return;
        };

        // Popup surface presses must have been captured before reaching this guard.
        let regions = DismissRegions::empty().trigger(layout.bounds());
        let outcome = on_press(self.policy, &regions, position);

        if outcome.closes() {
            shell.publish((self.on_dismiss)());
        }
        if outcome.consumes() {
            shell.capture_event();
        }
    }
}

impl<'a, Message: 'a> std::fmt::Debug for DismissOnOutsidePress<'a, Message> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DismissOnOutsidePress")
            .field("open", &self.open)
            .finish_non_exhaustive()
    }
}
