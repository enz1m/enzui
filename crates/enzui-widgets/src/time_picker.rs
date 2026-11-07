//! A time field with a list of times to pick from, and a date-and-time
//! picker built from it and the date picker.
//!
//! [`enz_time_picker`] is a [`time_field`](crate::time_field) (typed and
//! stepped) with a clock button that opens a list of times at an interval,
//! 30 minutes by default, scrolled to the time nearest the current value.
//! Arrows move through the list, Enter chooses, and Escape or a press
//! outside closes it, returning focus to the clock button. The application
//! owns the draft, the value and whether the list is open, and applies each
//! [`TimePickerEvent`].
//!
//! [`enz_date_time_picker`] places a date picker and a time picker side by
//! side as one labelled group.

use crate::date_picker::{DatePickerEvent, DatePickerState, enz_date_picker};
use crate::focus_button::{FocusStatus, focus_button_maybe};
use crate::menu::{EnzMenuItem, enz_menu};
use crate::time_field::{TimeFieldEvent, enz_time_field};
use enzui_core::civil_date::Date;
use enzui_core::menu_model::{CheckState, EntryId};
use enzui_core::placement::{Placement, Side};
use enzui_core::semantics::{Role, Semantics};
use enzui_core::time_of_day::{TimeFormat, TimeOfDay};
use iced::advanced::widget::Id;
use iced::widget::row;
use iced::{Background, Border, Color, Element, Length};
use std::rc::Rc;

/// The spacing of the listed times unless [`EnzTimePicker::interval_minutes`]
/// says otherwise.
pub const DEFAULT_INTERVAL_MINUTES: u32 = 30;

/// How tall the list of times gets before it scrolls.
const LIST_MAX_HEIGHT: f32 = 260.0;

/// What a time picker asks its application to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimePickerEvent {
    /// The field reported something; apply it as for
    /// [`time_field`](crate::time_field).
    Field(TimeFieldEvent),
    /// The clock button was pressed: open the list, or close it.
    Toggle,
    /// A time was chosen from the list: store it, write it into the draft,
    /// close the list.
    Chosen(TimeOfDay),
    /// The list should close (Escape, a press outside).
    Dismiss,
}

/// Builder for [`enz_time_picker`].
#[must_use]
pub struct EnzTimePicker<'a, Message> {
    draft: &'a str,
    value: TimeOfDay,
    open: bool,
    on_event: Rc<dyn Fn(TimePickerEvent) -> Message + 'a>,
    interval_minutes: u32,
    format: Option<TimeFormat>,
    label: Option<String>,
    id: Option<String>,
    enabled: bool,
}

/// A time picker showing `draft` for the committed `value`, with its list
/// open while `open`; every event goes through `on_event`.
pub fn enz_time_picker<'a, Message: Clone + 'a>(
    draft: &'a str,
    value: TimeOfDay,
    open: bool,
    on_event: impl Fn(TimePickerEvent) -> Message + 'a,
) -> EnzTimePicker<'a, Message> {
    EnzTimePicker {
        draft,
        value,
        open,
        on_event: Rc::new(on_event),
        interval_minutes: DEFAULT_INTERVAL_MINUTES,
        format: None,
        label: None,
        id: None,
        enabled: true,
    }
}

impl<'a, Message: Clone + 'a> EnzTimePicker<'a, Message> {
    /// List times this many minutes apart (1 to 720); the field still takes
    /// any typed time.
    pub fn interval_minutes(mut self, minutes: u32) -> Self {
        self.interval_minutes = minutes.clamp(1, 720);
        self
    }

    /// The format that writes and reads the times; by default the locale's.
    pub fn format(mut self, format: TimeFormat) -> Self {
        self.format = Some(format);
        self
    }

    /// The caption above the field and its accessible name.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// A stable identity: the field is `<id>`, the clock button
    /// `<id>-list`. By default it derives from the label.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set availability.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Build the picker.
    pub fn into_element(self) -> Element<'a, Message> {
        let format = self.format.clone().unwrap_or_else(|| {
            enzui_core::localization::current()
                .time_format()
                .unwrap_or_else(|_| TimeFormat::new(enzui_core::time_of_day::HourCycle::H12))
        });
        let base = self
            .id
            .clone()
            .or_else(|| {
                self.label
                    .as_ref()
                    .map(|label| format!("enz-time-picker-{label}"))
            })
            .unwrap_or_else(|| "enz-time-picker".to_owned());
        let button_id = Id::from(format!("{base}-list"));
        let open = self.open && self.enabled;
        let name = enzui_core::localization::text("enzui-time-picker-choose");
        let button = focus_button_maybe(
            crate::icon::enz_icon("clock").size(16.0),
            self.enabled
                .then(|| (self.on_event)(TimePickerEvent::Toggle)),
        )
        .id(button_id.clone())
        .padding(4)
        .relinquish_focus_while(open)
        .reclaim_focus_on_return((!open).then(|| button_id.clone()))
        .semantics(
            Semantics::new(Role::Button)
                .label(name)
                .expanded(open)
                .default_action(),
        )
        .style_focus(|theme: &iced::Theme, status: FocusStatus| {
            let palette = crate::theme_lookup::palette(theme);
            iced::widget::button::Style {
                background: matches!(status, FocusStatus::Hovered | FocusStatus::Pressed)
                    .then(|| Background::Color(palette.overlay_hover())),
                text_color: palette.text_muted,
                border: Border {
                    color: if status == FocusStatus::Focused {
                        palette.focus_ring
                    } else {
                        Color::TRANSPARENT
                    },
                    width: crate::theme_lookup::focus_ring_width(theme),
                    radius: crate::theme_lookup::shape_radius(theme, |shape| shape.button).into(),
                },
                ..Default::default()
            }
        });
        let list = open.then(|| self.list(&format, &base, button_id.clone()));
        let on_dismiss = Rc::clone(&self.on_event);
        // The clock sits inside the field's trailing edge, as a date
        // picker's calendar does; its list opens under the field's end.
        let button = crate::anchored_overlay::dismissible_anchored_menu(
            button,
            list,
            Placement::new(Side::Bottom)
                .gap(8.0)
                .alignment(enzui_core::placement::Alignment::End),
            move || on_dismiss(TimePickerEvent::Dismiss),
        );
        let on_field = Rc::clone(&self.on_event);
        let mut field = enz_time_field(self.draft, self.value, move |event| {
            on_field(TimePickerEvent::Field(event))
        })
        .format(format.clone())
        .step_minutes(self.interval_minutes.min(60))
        .id(Id::from(base.clone()))
        .enabled(self.enabled)
        .trailing(button);
        if let Some(label) = &self.label {
            field = field.label(label.clone());
        }
        field.into_element()
    }

    /// The list of times, the one nearest the value highlighted and the
    /// value itself checked.
    fn list(&self, format: &TimeFormat, base: &str, button: Id) -> Element<'a, Message> {
        let step = self.interval_minutes * 60;
        let value = self.value.seconds_since_midnight();
        let nearest = (value + step / 2) / step * step % enzui_core::time_of_day::SECONDS_PER_DAY;
        let mut menu = enz_menu()
            .label(enzui_core::localization::text("enzui-time-picker-times"))
            .id_prefix(format!("{base}-times"))
            .hug_content(true)
            .hug_min_width(120.0)
            .roving_self(true)
            .focus_return_to(button)
            .active(EntryId::new(nearest.to_string()));
        let mut seconds = 0;
        while seconds < enzui_core::time_of_day::SECONDS_PER_DAY {
            if let Some(time) = TimeOfDay::from_seconds_since_midnight(seconds) {
                let mut item = EnzMenuItem::command(
                    seconds.to_string(),
                    format.format(time),
                    (self.on_event)(TimePickerEvent::Chosen(time)),
                );
                if seconds == value {
                    item = item.check(CheckState::Checked).radio(true);
                }
                menu = menu.push_item(item);
            }
            seconds += step;
        }
        // A day of times scrolls inside a list a little over seven rows tall.
        crate::max_height::MaxHeight::new(menu.into_element(), LIST_MAX_HEIGHT).into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzTimePicker<'a, Message>> for Element<'a, Message> {
    fn from(picker: EnzTimePicker<'a, Message>) -> Self {
        picker.into_element()
    }
}

/// What a date-and-time picker asks its application to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DateTimePickerEvent {
    /// The date half reported something; apply it as for
    /// [`date_picker`](crate::date_picker).
    Date(DatePickerEvent),
    /// The time half reported something; apply it as a [`TimePickerEvent`].
    Time(TimePickerEvent),
}

/// A date picker and a time picker side by side, published as one group
/// named `label`. The application owns both halves as it would separately.
#[allow(
    clippy::too_many_arguments,
    reason = "the two halves' controlled state is the picker's whole input"
)]
pub fn enz_date_time_picker<'a, Message: Clone + 'a>(
    label: &str,
    date: Option<Date>,
    date_state: &'a DatePickerState,
    time_draft: &'a str,
    time: TimeOfDay,
    time_open: bool,
    on_event: impl Fn(DateTimePickerEvent) -> Message + 'a,
) -> Element<'a, Message> {
    let on_event = Rc::new(on_event);
    let on_date = Rc::clone(&on_event);
    let on_time = Rc::clone(&on_event);
    let date = enz_date_picker(date, date_state, move |event| {
        on_date(DateTimePickerEvent::Date(event))
    })
    .label(enzui_core::localization::text(
        "enzui-date-time-picker-date",
    ))
    .into_element();
    let time = enz_time_picker(time_draft, time, time_open, move |event| {
        on_time(DateTimePickerEvent::Time(event))
    })
    .label(enzui_core::localization::text(
        "enzui-date-time-picker-time",
    ))
    .id(format!("enz-date-time-{label}"))
    .into_element();
    crate::semantics_container::SemanticsContainer::new(
        row![date, time]
            .spacing(12)
            .align_y(iced::alignment::Vertical::Bottom)
            .width(Length::Shrink),
        Semantics::new(Role::Group).label(label.to_owned()),
    )
    .into()
}
