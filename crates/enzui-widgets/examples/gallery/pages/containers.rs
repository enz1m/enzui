use crate::*;
use enzui_widgets::card::CardTone;
use enzui_widgets::text::TextTone;
use iced::widget::column;

const CARD_TONES: [CardTone; 3] = [CardTone::Default, CardTone::Elevated, CardTone::Flat];
const CARD_TONE_NAMES: &[&str] = &["Default", "Elevated", "Flat"];
const HEADER_TONES: [TextTone; 3] = [TextTone::Default, TextTone::Muted, TextTone::Accent];
const HEADER_TONE_NAMES: &[&str] = &["Default", "Muted", "Accent"];

impl Gallery {
    pub(crate) fn containers_page(&self) -> Element<'_, Message> {
        column![
            self.card_specimen(),
            self.group_box_specimen(),
            self.section_header_specimen(),
            self.header_bar_specimen(),
            self.status_bar_specimen(),
        ]
        .spacing(32)
        .width(Length::Fill)
        .into()
    }

    fn card_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let title = k.text("card.title", "Card");
        let described = k.flag("card.description", false);
        let footer = k.flag("card.footer", false);
        let tone = k.pick("card.tone", 0);
        let action = k.flag("card.action", true);
        let content = if action {
            column![
                enz_text("Cards group related content.").into_element(),
                enz_button("Action", Message::Clicked("card action")).into_element(),
            ]
            .spacing(8)
        } else {
            column![enz_text("Cards group related content.").into_element()]
        };
        let mut card = enz_card(content).title(title).tone(CARD_TONES[tone]);
        if described {
            card = card.description("A line under the title");
        }
        if footer {
            card = card.footer("Updated just now");
        }
        self.specimen(
            "enz_card",
            "A surface that groups related content under an optional title.",
            container(card.into_element()).width(Length::Fill.max(420.0)),
        )
        .controls([
            k.text_control("card.title", "Title", "Card"),
            k.pick_control("card.tone", "Tone", CARD_TONE_NAMES, 0),
            k.flag_control("card.description", "Description", false),
            k.flag_control("card.footer", "Footer", false),
            k.flag_control("card.action", "Button inside", true),
        ])
        .code(
            &code("enzui_widgets", "enz_card")
                .arg(if action {
                    "column![\n    enz_text(\"Cards group related content.\").into_element(),\n    enz_button(\"Action\", Message::Act).into_element(),\n]\n.spacing(8)"
                } else {
                    "enz_text(\"Cards group related content.\").into_element()"
                })
                .set("title", quoted(title))
                .variant(tone != 0, "tone", "enzui_widgets::card", "CardTone", CARD_TONE_NAMES[tone])
                .set_if(described, "description", quoted("A line under the title"))
                .set_if(footer, "footer", quoted("Updated just now")),
        )
        .into_element()
    }

    fn group_box_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let title = k.text("group.title", "Sign in");
        let bordered = k.flag("groupbox.bordered", false);
        let icon = k.flag("groupbox.icon", false);
        let mut group = enzui_widgets::group_box::enz_group_box(
            title,
            column![
                enz_toggle(self.notify, "Send notifications", Message::Notify).into_element(),
                enz_checkbox(self.agree, "Remember me", Message::Agree).into_element(),
            ]
            .spacing(8),
        )
        .bordered(bordered);
        if icon {
            group = group.title_icon("user");
        }
        self.specimen(
            "enz_group_box",
            "A titled frame around a few related controls.",
            container(group.into_element()).width(Length::Fill.max(420.0)),
        )
        .controls([
            k.text_control("group.title", "Title", "Sign in"),
            k.flag_control("groupbox.bordered", "Bordered", false),
            k.flag_control("groupbox.icon", "Title icon", false),
        ])
        .code(
            &code("enzui_widgets::group_box", "enz_group_box")
                .arg(quoted(title))
                .arg("column![toggle, checkbox].spacing(8)")
                .set_if(bordered, "bordered", "true")
                .set_if(icon, "title_icon", quoted("user")),
        )
        .into_element()
    }

    fn section_header_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let title = k.text("section.title", "Recent files");
        let tone = k.pick("section.tone", 0);
        self.specimen(
            "enz_section_header",
            "A heading that starts a section of a list or page.",
            enzui_widgets::section_header::enz_section_header(title)
                .tone(HEADER_TONES[tone])
                .into_element(),
        )
        .controls([
            k.text_control("section.title", "Title", "Recent files"),
            k.pick_control("section.tone", "Tone", HEADER_TONE_NAMES, 0),
        ])
        .code(
            &code("enzui_widgets::section_header", "enz_section_header")
                .arg(quoted(title))
                .variant(
                    tone != 0,
                    "tone",
                    "enzui_widgets::text",
                    "TextTone",
                    HEADER_TONE_NAMES[tone],
                ),
        )
        .into_element()
    }

    fn header_bar_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let title = k.text("header.title", "Header bar");
        let subtitle = k.flag("header.subtitle", true);
        let back = k.flag("header.back", false);
        let mut bar = enzui_widgets::header_bar::enz_header_bar(title)
            .show_back_button(back)
            .on_back(Message::Clicked("back"));
        if subtitle {
            bar = bar.subtitle("A pane's own title row");
        }
        self.specimen(
            "enz_header_bar",
            "The title row at the top of a pane, with optional back button and actions.",
            container(bar.into_element()).style(container::bordered_box),
        )
        .controls([
            k.text_control("header.title", "Title", "Header bar"),
            k.flag_control("header.subtitle", "Subtitle", true),
            k.flag_control("header.back", "Back button", false),
        ])
        .code(
            &code("enzui_widgets::header_bar", "enz_header_bar")
                .arg(quoted(title))
                .set_if(subtitle, "subtitle", quoted("A pane's own title row"))
                .set_if(back, "show_back_button", "true")
                .set_if(back, "on_back", "Message::Back"),
        )
        .into_element()
    }

    fn status_bar_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let encoding = k.flag("status.encoding", true);
        let mut bar = enzui_widgets::status_bar::enz_status_bar_strip()
            .push_leading(enzui_widgets::status_bar::StatusBarItem::text("Ready"))
            .push_trailing(enzui_widgets::status_bar::StatusBarItem::text(
                "Ln 12, Col 4",
            ));
        if encoding {
            bar = bar.push_trailing(enzui_widgets::status_bar::StatusBarItem::text("UTF-8"));
        }
        self.specimen(
            "enz_status_bar_strip",
            "A strip along the bottom of a window or pane, with items at both ends.",
            container(bar.into_element()).style(container::bordered_box),
        )
        .controls([k.flag_control("status.encoding", "Encoding item", true)])
        .code_text(format!(
            "use enzui_widgets::status_bar::{{StatusBarItem, enz_status_bar_strip}};\n\n\
             enz_status_bar_strip()\n    \
                 .push_leading(StatusBarItem::text(\"Ready\"))\n    \
                 .push_trailing(StatusBarItem::text(\"Ln 12, Col 4\")){}\n    \
                 .into_element()",
            if encoding {
                "\n    .push_trailing(StatusBarItem::text(\"UTF-8\"))"
            } else {
                ""
            }
        ))
        .into_element()
    }
}
