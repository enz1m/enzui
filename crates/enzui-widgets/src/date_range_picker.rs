//! Controlled date-range field with a two-click calendar gesture.
//!
//! Hosts retain [`DateRangePickerState`] and the committed range. The first day
//! selection establishes an anchor; the second completes a sorted range and
//! closes the popup. Dismissal abandons a pending anchor.
//!
//! The field shows the host’s value except while an anchor is pending. The
//! popup can retain the gesture’s completed range even if the host rejects it.
//! Pending anchors are described separately from the accessible field value.

use enzui_core::calendar_nav::MonthController;
use enzui_core::calendar_semantics::{CalendarLabels, date_range_field_semantics};
use enzui_core::civil_date::{Date, DateBounds, DateRange, Weekday};
use enzui_core::date_selection::{RangeSelection, RangeStep};
use enzui_core::dismissal::Dismissal;
use iced::advanced::widget::Id;
use iced::{Element, Length};
use std::borrow::Cow;

use crate::calendar::{CalendarEvent, enz_calendar};
use crate::date_field::{FieldTrigger, date_field, elevate_popup};
pub use crate::date_picker::DismissOutcome;
use crate::tokens::FieldSizeTokens;

/// Host-owned popup navigation, range gesture, and captured focus target.
#[derive(Debug, Clone)]
pub struct DateRangePickerState {
    controller: MonthController,
    selection: RangeSelection,
    open: bool,
    trigger_id: Id,
    restore_focus: Option<Id>,
}

impl DateRangePickerState {
    /// Creates a closed picker with an empty gesture and navigation initialized from `origin`.
    ///
    /// The caller supplies the origin and bounds; no clock is read. The default
    /// trigger identity is minted once for this state.
    #[must_use]
    pub fn new(origin: Date, first_weekday: Weekday, bounds: DateBounds) -> Self {
        Self {
            controller: MonthController::new(origin, first_weekday, bounds),
            selection: RangeSelection::Empty,
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

    /// Returns the gesture state, which can differ from the host’s committed range.
    #[must_use]
    pub const fn selection(&self) -> RangeSelection {
        self.selection
    }

    /// Returns an anchor awaiting its second endpoint.
    #[must_use]
    pub const fn pending(&self) -> Option<Date> {
        self.selection.pending()
    }

    /// Opens the popup, seeds the gesture from `value`, and replaces the captured focus target.
    ///
    /// An in-bounds range start becomes focused; none or a refused start leaves
    /// navigation unchanged. The seed is retained even if its dates are out of bounds.
    pub fn open(&mut self, value: Option<DateRange>, restore_focus: Option<Id>) {
        self.open = true;
        self.restore_focus = restore_focus;
        self.selection = value.map_or(RangeSelection::Empty, RangeSelection::complete);
        if let Some(range) = value {
            let _ = self.controller.focus_date(range.start());
        }
    }

    /// Closes the popup, cancels a pending anchor, and takes the captured focus target.
    ///
    /// Completed ranges survive. After the simple apply method closes on completion,
    /// this can still return its unconsumed target; later calls return none.
    pub const fn close(&mut self) -> Option<Id> {
        self.open = false;
        self.selection.cancel();
        self.restore_focus.take()
    }

    /// Opens with the supplied value or closes and returns the captured focus target.
    ///
    /// Opening returns no target.
    pub fn toggle(&mut self, value: Option<DateRange>, restore_focus: Option<Id>) -> Option<Id> {
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

    /// Advances the gesture on an accepted day selection and closes on completion.
    ///
    /// Navigation and refused selections return none. The captured focus target is
    /// retained for a later [`Self::close`]; hosts must store completed ranges.
    /// This method does not require the popup to be open.
    pub fn apply_calendar_event(&mut self, event: CalendarEvent) -> Option<RangeStep> {
        let date = crate::calendar::apply_event(&mut self.controller, event)?;
        let step = self.selection.click(date);
        if step.is_complete() {
            self.open = false;
        }
        Some(step)
    }

    /// Advances the gesture and returns the captured focus target only on completion.
    ///
    /// Completion closes the popup and takes the target once. Anchors, navigation,
    /// and refused selections leave it available; hosts must store completed ranges.
    pub fn apply_calendar_event_and_take_focus(
        &mut self,
        event: CalendarEvent,
    ) -> (Option<RangeStep>, Option<Id>) {
        let Some(date) = crate::calendar::apply_event(&mut self.controller, event) else {
            return (None, None);
        };
        let step = self.selection.click(date);
        let restore_focus = if step.is_complete() {
            // Completion preserves the committed gesture while taking only the focus target.
            self.open = false;
            self.restore_focus.take()
        } else {
            None
        };
        (Some(step), restore_focus)
    }
}

/// What a range picker reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateRangePickerEvent {
    /// The field was activated. The host should toggle its state.
    TriggerPressed,
    /// The calendar reported something. The host should feed it to
    /// [`DateRangePickerState::apply_calendar_event`].
    Calendar(CalendarEvent),
    /// The popup should close without completing the gesture.
    Dismissed,
}

/// A date-range field that opens a calendar popover.
pub struct EnzDateRangePicker<'a, Message> {
    value: Option<DateRange>,
    state: &'a DateRangePickerState,
    on_event: Box<dyn Fn(DateRangePickerEvent) -> Message + 'a>,
    label: Option<Cow<'a, str>>,
    placeholder: Option<Cow<'a, str>>,
    today: Option<Date>,
    labels: &'a dyn CalendarLabels,
    metrics: FieldSizeTokens,
    width: Length,
    enabled: bool,
    fill_variant: crate::tokens::FieldVariant,
    direction: Option<enzui_core::placement::Direction>,
    focus_popup_on_mount: bool,
    id: Option<Id>,
}

/// Builds a range picker over a host-owned range and retained gesture state.
///
/// Defaults to an enabled filled field, medium metrics, 240-pixel width, scoped
/// calendar labels, a localized placeholder, and popup focus on mount.
pub fn enz_date_range_picker<'a, Message>(
    value: Option<DateRange>,
    state: &'a DateRangePickerState,
    on_event: impl Fn(DateRangePickerEvent) -> Message + 'a,
) -> EnzDateRangePicker<'a, Message> {
    EnzDateRangePicker {
        value,
        state,
        on_event: Box::new(on_event),
        label: None,
        placeholder: None,
        today: None,
        labels: &enzui_core::localization::LocalizedCalendarLabels,
        metrics: FieldSizeTokens::MD,
        width: Length::Fixed(240.0),
        enabled: true,
        fill_variant: crate::tokens::FieldVariant::default(),
        direction: None,
        focus_popup_on_mount: true,
        id: None,
    }
}

impl<'a, Message> EnzDateRangePicker<'a, Message> {
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

    /// Sets text shown when no value is displayed.
    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<Cow<'a, str>>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Sets the date highlighted as today in the popup; defaults to none and never reads a clock.
    #[must_use]
    pub const fn today(mut self, today: Option<Date>) -> Self {
        self.today = today;
        self
    }

    /// Overrides the scoped locale's date formatting and accessibility vocabulary.
    #[must_use]
    pub const fn labels(mut self, labels: &'a dyn CalendarLabels) -> Self {
        self.labels = labels;
        self
    }

    /// Replace medium field dimensions; invalid values use the medium defaults.
    #[must_use]
    pub const fn metrics(mut self, metrics: FieldSizeTokens) -> Self {
        self.metrics = metrics;
        self
    }

    /// Sets field width; popup calendar width is independent.
    #[must_use]
    pub const fn width(mut self, width: Length) -> Self {
        self.width = width;
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

    /// Returns a pending anchor as a one-day range, otherwise the host’s value.
    ///
    /// A completed gesture never overrides a range the host rejected.
    #[must_use]
    pub fn displayed_range(&self) -> Option<DateRange> {
        match self.state.pending() {
            Some(anchor) => Some(DateRange::single(anchor)),
            None => self.value,
        }
    }

    #[must_use]
    fn display_text(&self) -> String {
        self.displayed_range().map_or_else(
            || {
                self.placeholder.as_deref().map_or_else(
                    || enzui_core::localization::text("enzui-date-range-picker-select-dates"),
                    str::to_owned,
                )
            },
            |range| self.labels.display_range(range),
        )
    }

    #[must_use]
    const fn shows_value(&self) -> bool {
        self.state.pending().is_some() || self.value.is_some()
    }

    /// Returns the gesture’s displayed range, falling back to the host’s value.
    ///
    /// A completed gesture can therefore remain highlighted after host rejection.
    #[must_use]
    pub fn popup_range(&self) -> Option<DateRange> {
        self.state.selection.displayed().or(self.value)
    }

    /// Resolves trigger semantics from the host value, pending anchor, and live-popup state.
    pub fn semantics(&self) -> enzui_core::Semantics {
        date_range_field_semantics(
            self.label.as_deref(),
            self.value,
            self.state.pending(),
            self.popup_is_live(),
            self.enabled,
            self.labels,
        )
    }

    /// Reports whether an enabled widget should show the host’s open popup.
    #[must_use]
    pub const fn popup_is_live(&self) -> bool {
        self.state.open && self.enabled
    }
}

impl<'a, Message: Clone + 'a> EnzDateRangePicker<'a, Message> {
    /// Builds the field and, when enabled and open, its calendar popup.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        let resolve = crate::date_field::field_resolver_variant(self.enabled, self.fill_variant);

        let display_text = self.display_text();
        let shows_value = self.shows_value();
        let semantics = self.semantics();
        let trigger_id = self.id.clone().unwrap_or_else(|| self.state.trigger_id());
        let calendar_id = crate::date_picker::calendar_owner_id(&trigger_id);
        let painted = self.popup_range();
        let popup_live = self.popup_is_live();
        let open = self.state.open;
        let focused_date = self.state.controller().focused();
        let enabled = self.enabled;
        let focus_popup_on_mount = self.focus_popup_on_mount;

        let on_event: std::rc::Rc<dyn Fn(DateRangePickerEvent) -> Message + 'a> =
            std::rc::Rc::from(self.on_event);

        let trigger = FieldTrigger {
            label: self.label.as_deref(),
            display: display_text,
            shows_value,
            on_press: self.enabled.then({
                let on_event = std::rc::Rc::clone(&on_event);
                move || on_event(DateRangePickerEvent::TriggerPressed)
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
                popup_on_event(DateRangePickerEvent::Calendar(ev))
            })
            .range(painted)
            .today(self.today);
            if let Some(dir) = direction_for_cal {
                calendar = calendar.direction(dir);
            }
            calendar = calendar
                .id(calendar_id)
                .focus_on_mount(focus_popup_on_mount);
            crate::date_picker::surface_press_shield(elevate_popup(
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
            if open {
                event_for_open_key(key, modifiers, rtl, focused_date).map(&*on_event)
            } else {
                use iced::keyboard::key;
                if matches!(
                    key,
                    iced::keyboard::Key::Named(key::Named::ArrowDown | key::Named::ArrowUp)
                ) && !(modifiers.control() || modifiers.alt() || modifiers.logo())
                {
                    Some(on_event(DateRangePickerEvent::TriggerPressed))
                } else {
                    None
                }
            }
        });

        crate::date_picker::dismiss_on_outside_press(
            keyed,
            open,
            crate::date_picker::date_dismiss_policy(),
            move || dismiss_event(DateRangePickerEvent::Dismissed),
        )
    }
}

impl<'a, Message: Clone + 'a> From<EnzDateRangePicker<'a, Message>> for Element<'a, Message> {
    fn from(picker: EnzDateRangePicker<'a, Message>) -> Self {
        picker.into_element()
    }
}

/// Decodes Enter, Space, ArrowDown, and ArrowUp as trigger activation.
///
/// This helper does not inspect modifiers. The widget routes Enter/Space through
/// the trigger and rejects Ctrl, Alt, or Logo on closed-field arrow activation.
#[must_use]
pub fn event_for_trigger_key(key: &iced::keyboard::Key) -> Option<DateRangePickerEvent> {
    crate::date_picker::event_for_trigger_key(key).map(|_| DateRangePickerEvent::TriggerPressed)
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
) -> Option<DateRangePickerEvent> {
    use iced::keyboard::key;
    if matches!(key, iced::keyboard::Key::Named(key::Named::Escape)) {
        return Some(DateRangePickerEvent::Dismissed);
    }
    crate::calendar::event_for_key(key, modifiers, rtl, focused).map(DateRangePickerEvent::Calendar)
}
