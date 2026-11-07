//! All open tabs as a grid of cards with a preview, to pick, close or add
//! one (libadwaita's `AdwTabOverview`, a browser's tab grid).
//!
//! The app supplies each card's title and preview (a scaled-down rendering
//! or a placeholder) and handles [`TabOverviewEvent`]s.

use iced::widget::{Space, button, column, container, row};
use iced::{Alignment, Border, Element, Length};

use crate::text::{TextSize, enz_text};

/// One tab's card.
pub struct TabCard<'a, Message> {
    /// The tab's title.
    pub title: String,
    /// What the card shows of the tab.
    pub preview: Element<'a, Message>,
    /// Whether this is the current tab.
    pub selected: bool,
}

/// What the user did in the overview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabOverviewEvent {
    /// A card was chosen: switch to that tab and leave the overview.
    Select(usize),
    /// A card's close button was pressed.
    Close(usize),
    /// The new-tab card was pressed.
    New,
}

/// The width of a card.
pub const CARD_WIDTH: f32 = 220.0;
/// The height of a card's preview.
pub const PREVIEW_HEIGHT: f32 = 140.0;

/// The cards in reading order, wrapping to the width, then a new-tab card.
pub fn enz_tab_overview<'a, Message: Clone + 'a>(
    cards: Vec<TabCard<'a, Message>>,
    on_event: impl Fn(TabOverviewEvent) -> Message + 'a,
) -> Element<'a, Message> {
    let mut grid: Vec<Element<'a, Message>> = cards
        .into_iter()
        .enumerate()
        .map(|(index, card)| {
            let selected = card.selected;
            let preview = container(card.preview)
                .width(Length::Fixed(CARD_WIDTH))
                .height(Length::Fixed(PREVIEW_HEIGHT))
                .clip(true)
                .style(move |theme: &iced::Theme| {
                    let palette = crate::theme_lookup::palette(theme);
                    container::Style {
                        background: Some(palette.surface_alt.into()),
                        border: Border {
                            color: if selected {
                                palette.accent
                            } else {
                                palette.border
                            },
                            width: if selected { 2.0 } else { 1.0 },
                            radius: 10.0.into(),
                        },
                        ..container::Style::default()
                    }
                });
            let close = button(crate::icon::enz_icon("x").size(14.0))
                .padding(4)
                .style(button::text)
                .on_press(on_event(TabOverviewEvent::Close(index)));
            let close = crate::semantics_container::SemanticsContainer::leaf(
                close,
                enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Button)
                    .label_opt(Some(enzui_core::localized!(
                        "enzui-tab-overview-close",
                        "title" => card.title.clone()
                    ))),
            );
            let title = row![
                enz_text(card.title)
                    .size(TextSize::Callout)
                    .width(Length::Fill),
                close,
            ]
            .mirrored(
                enzui_core::placement::ambient_direction()
                    == enzui_core::placement::Direction::RightToLeft,
            )
            .align_y(Alignment::Center)
            .width(Length::Fixed(CARD_WIDTH));
            button(column![preview, title].spacing(6))
                .padding(0)
                .style(button::text)
                .on_press(on_event(TabOverviewEvent::Select(index)))
                .into()
        })
        .collect();
    let new_tab = button(
        container(
            column![
                crate::icon::enz_icon("plus").size(24.0),
                enz_text(enzui_core::localized!("enzui-tab-overview-new")).size(TextSize::Callout),
            ]
            .spacing(6)
            .align_x(Alignment::Center),
        )
        .width(Length::Fixed(CARD_WIDTH))
        .height(Length::Fixed(PREVIEW_HEIGHT))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(|theme: &iced::Theme| {
            let palette = crate::theme_lookup::palette(theme);
            container::Style {
                border: Border {
                    color: palette.border,
                    width: 1.0,
                    radius: 10.0.into(),
                },
                ..container::Style::default()
            }
        }),
    )
    .padding(0)
    .style(button::text)
    .on_press(on_event(TabOverviewEvent::New));
    grid.push(column![new_tab, Space::new().height(24)].into());
    row(grid).spacing(16).wrap().vertical_spacing(16).into()
}
