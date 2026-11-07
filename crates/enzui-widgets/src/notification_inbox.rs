//! The in-app notification list: past notifications, newest first, unread
//! ones marked, each dismissible, with Clear All (a notification centre
//! inside the app). The model is [`enzui_core::notification_inbox::Inbox`].

use enzui_core::notification_inbox::Inbox;
use iced::widget::{Space, button, column, container, row};
use iced::{Alignment, Element, Length};

use crate::text::{TextSize, TextTone, enz_text};

/// What the user did in the inbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxEvent {
    /// An entry was chosen: act on it, and mark it read.
    Open(u64),
    /// An entry's dismiss button was pressed.
    Dismiss(u64),
    /// Clear All was pressed.
    ClearAll,
}

/// Applies the inbox's own part of `event`: reading, dismissing, clearing.
pub fn apply(inbox: &mut Inbox, event: InboxEvent) {
    match event {
        InboxEvent::Open(id) => inbox.mark_read(id),
        InboxEvent::Dismiss(id) => inbox.dismiss(id),
        InboxEvent::ClearAll => inbox.clear(),
    }
}

/// The inbox's entries with a header and Clear All, or the empty state.
pub fn enz_notification_inbox<'a, Message: Clone + 'a>(
    inbox: &Inbox,
    on_event: impl Fn(InboxEvent) -> Message + 'a,
) -> Element<'a, Message> {
    // Rows run from the start edge: the right one in right-to-left text.
    let rtl =
        enzui_core::placement::ambient_direction() == enzui_core::placement::Direction::RightToLeft;
    let header = row![
        enz_text(enzui_core::localized!("enzui-inbox-title"))
            .size(TextSize::Headline)
            .width(Length::Fill),
        crate::button::enz_button(
            enzui_core::localized!("enzui-inbox-clear"),
            on_event(InboxEvent::ClearAll)
        )
        .kind(crate::button::ButtonKind::Quiet)
        .enabled(!inbox.entries().is_empty()),
    ]
    .mirrored(rtl)
    .align_y(Alignment::Center);
    if inbox.entries().is_empty() {
        return column![
            header,
            container(enz_text(enzui_core::localized!("enzui-inbox-empty")).tone(TextTone::Muted))
                .center_x(Length::Fill)
                .padding(24),
        ]
        .spacing(8)
        .into();
    }
    let entries = inbox.entries().iter().map(|entry| {
        let unread_dot: Element<'a, Message> = if entry.read {
            Space::new().width(8).height(8).into()
        } else {
            container(Space::new().width(8).height(8))
                .style(|theme: &iced::Theme| {
                    let palette = crate::theme_lookup::palette(theme);
                    container::Style {
                        background: Some(palette.accent.into()),
                        border: iced::Border::default().rounded(4),
                        ..container::Style::default()
                    }
                })
                .into()
        };
        let when = enzui_core::local_time::local_civil_time_at(entry.at)
            .map(|time| format!("{:02}:{:02}", time.minute / 60, time.minute % 60))
            .unwrap_or_default();
        let mut text = column![
            row![
                enz_text(entry.title.clone())
                    .size(TextSize::Headline)
                    .width(Length::Fill),
                enz_text(when)
                    .size(TextSize::Caption)
                    .tone(TextTone::Muted)
                    .tabular_figures(true),
            ]
            .mirrored(rtl)
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(2)
        .width(Length::Fill);
        if let Some(body) = &entry.body {
            text = text.push(
                enz_text(body.clone())
                    .tone(TextTone::Muted)
                    .width(Length::Fill),
            );
        }
        let dismiss = crate::semantics_container::SemanticsContainer::leaf(
            button(crate::icon::enz_icon("x").size(14.0))
                .padding(4)
                .style(button::text)
                .on_press(on_event(InboxEvent::Dismiss(entry.id))),
            enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Button)
                .label_opt(Some(enzui_core::localized!("enzui-inbox-dismiss"))),
        );
        crate::list_row_surface::enz_list_row_surface(
            container(
                row![unread_dot, text]
                    .mirrored(rtl)
                    .spacing(10)
                    .align_y(Alignment::Center),
            )
            .padding([8, 0]),
            on_event(InboxEvent::Open(entry.id)),
        )
        .content_height(true)
        .label(entry.title.clone())
        .trailing(dismiss)
        .trailing_visibility(crate::list_row_surface::TrailingVisibility::OnHoverOrFocus)
        .into_element()
    });
    column![header, column(entries).spacing(2)]
        .spacing(8)
        .into()
}
