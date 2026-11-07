use crate::*;
use iced::widget::{column, row};

const SWATCHES: [(u8, u8, u8); 5] = [
    (0x35, 0x84, 0xe4),
    (0x2e, 0xc2, 0x7e),
    (0xf6, 0xd3, 0x2d),
    (0xe0, 0x1b, 0x24),
    (0x91, 0x41, 0xac),
];

impl Gallery {
    pub(crate) fn pickers_page(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        column![
            {
                let label = k.text("date.label", "Due date");
                self.specimen(
                    "enz_date_picker",
                    "A date field that opens a month calendar. The state keeps the open month and the bounds.",
                    container(
                        enzui_widgets::date_picker::enz_date_picker(
                            self.date,
                            &self.date_state,
                            Message::Date,
                        )
                        .label(label)
                        .into_element(),
                    )
                    .width(Length::Fill.max(360.0)),
                )
                .controls([k.text_control("date.label", "Label", "Due date")])
                .code(
                    &code("enzui_widgets::date_picker", "enz_date_picker")
                        .arg("self.date")
                        .arg("&self.date_state")
                        .arg("Message::Date")
                        .set("label", quoted(label)),
                )
                .into_element()
            },
            {
                let label = k.text("color.label", "Accent colour");
                self.specimen(
                    "enz_color_picker",
                    "Picks a colour on a saturation plane and a hue strip, or as hex.",
                    enzui_widgets::color_picker::enz_color_picker(self.color, Message::Color)
                        .label(label)
                        .into_element(),
                )
                .controls([k.text_control("color.label", "Label", "Accent colour")])
                .code(
                    &code("enzui_widgets::color_picker", "enz_color_picker")
                        .arg("self.color")
                        .arg("Message::Color")
                        .set("label", quoted(label)),
                )
                .into_element()
            },
            {
                let size = k.number("swatch.size", 28.0);
                let current = self.color.to_color();
                self.specimen(
                    "enz_color_swatch",
                    "One preset colour to click. The selected swatch shows a ring.",
                    row(SWATCHES.map(|(r, g, b)| {
                        let color = iced::Color::from_rgb8(r, g, b);
                        let picked = enzui_widgets::color_picker::Hsva::from_color(color);
                        Element::from(
                            enzui_widgets::color_swatch::enz_color_swatch(color, Message::Color(picked))
                                .size(size)
                                .selected(same_color(color, current)),
                        )
                    }))
                    .spacing(8),
                )
                .controls([k.number_control("swatch.size", "Size", 20.0..=48.0, 2.0, 28.0)])
                .code(
                    &code("enzui_widgets::color_swatch", "enz_color_swatch")
                        .arg("Color::from_rgb8(0x35, 0x84, 0xe4)")
                        .arg("Message::PickBlue")
                        .set_if(size != 28.0, "size", number(size))
                        .set("selected", "self.color == blue")
                        .end(None),
                )
                .into_element()
            },
            {
                let height = k.number("emoji.height", 228.0);
                self.specimen(
                    "enz_emoji_picker",
                    "Searchable emoji in categories, with the recently used first.",
                    column![
                        enzui_widgets::emoji_picker::enz_emoji_picker(
                            &self.emoji_query,
                            self.emoji_category,
                            &self.emoji_recents,
                            Message::Emoji,
                        )
                        .grid_height(height)
                        .into_element(),
                        enz_text(self.last_action.clone()).into_element(),
                    ]
                    .spacing(12),
                )
                .controls([k.number_control("emoji.height", "Grid height", 120.0..=408.0, 12.0, 228.0)])
                .code(
                    &code("enzui_widgets::emoji_picker", "enz_emoji_picker")
                        .arg("&self.emoji_query")
                        .arg("self.emoji_category")
                        .arg("&self.emoji_recents")
                        .arg("Message::Emoji")
                        .set_if(height != 228.0, "grid_height", number(height)),
                )
                .into_element()
            },
        ]
        .spacing(32)
        .width(Length::Fill)
        .into()
    }
}

/// Whether two colours match to 8-bit precision.
fn same_color(a: iced::Color, b: iced::Color) -> bool {
    a.into_rgba8() == b.into_rgba8()
}
