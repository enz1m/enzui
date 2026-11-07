use crate::*;
use iced::widget::column;

const MARKDOWN: &str = "## Release notes\n\nThis build adds **receipts** for file drops and \
                        `dismiss` for dialogs.\n\n- Faster titlebars\n- Routable commands\n\n\
                        > Quotes keep their own rhythm.";

impl Gallery {
    pub(crate) fn content_page(&self) -> Element<'_, Message> {
        column![
            self.markdown_specimen(),
            self.code_block_specimen(),
            self.chat_bubble_specimen(),
            self.media_controls_specimen(),
            self.labeled_content_specimen(),
            self.divider_specimen(),
            self.preview_row_specimen(),
        ]
        .spacing(32)
        .width(Length::Fill)
        .into()
    }

    fn markdown_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let body = k.number("md.body", 14.0);
        self.specimen(
            "enz_markdown",
            "Renders Markdown: headings, emphasis, code, lists, quotes and links.",
            enzui_widgets::enz_markdown(MARKDOWN)
                .body_size(body)
                .into_element(),
        )
        .controls([k.number_control("md.body", "Body size", 11.0..=20.0, 1.0, 14.0)])
        .code(
            &code("enzui_widgets", "enz_markdown")
                .arg("&self.notes")
                .set_if(body != 14.0, "body_size", number(body)),
        )
        .into_element()
    }

    fn code_block_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let header = k.flag("cb.header", true);
        let copy = k.flag("cb.copy", true);
        let framed = k.flag("cb.framed", false);
        self.specimen(
            "enz_code_block",
            "Source code with highlighting and a copy button, like the code under every specimen here.",
            enzui_widgets::code_block::enz_code_block(
                "fn main() {\n    println!(\"hello, enzUI\");\n}",
            )
            .language("rust")
            .show_header(header)
            .show_copy(copy)
            .framed(framed)
            .into_element(),
        )
        .controls([
            k.flag_control("cb.header", "Header", true),
            k.flag_control("cb.copy", "Copy button", true),
            k.flag_control("cb.framed", "Framed", false),
        ])
        .code(
            &code("enzui_widgets::code_block", "enz_code_block")
                .arg("source")
                .set("language", quoted("rust"))
                .set_if(!header, "show_header", "false")
                .set_if(!copy, "show_copy", "false")
                .set_if(framed, "framed", "true")
                .set_if(copy, "on_copy_requested", "Message::Copy"),
        )
        .into_element()
    }

    fn chat_bubble_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let author = k.flag("chat.author", false);
        let timestamp = k.flag("chat.timestamp", false);
        let bubble = |text: &'static str, role: enzui_widgets::ChatRole, name: &'static str| {
            let mut bubble = enzui_widgets::chat_bubble::enz_chat_bubble(text, role);
            if author {
                bubble = bubble.author(name);
            }
            if timestamp {
                bubble = bubble.timestamp("09:41");
            }
            bubble.into_element()
        };
        self.specimen(
            "enz_chat_bubble",
            "One message in a conversation. The role puts it on the right side and colours it.",
            column![
                bubble(
                    "Can you summarise the meeting?",
                    enzui_widgets::ChatRole::User,
                    "You"
                ),
                bubble(
                    "Three decisions: ship Friday, freeze the API, and write the docs.",
                    enzui_widgets::ChatRole::Assistant,
                    "Assistant"
                ),
            ]
            .spacing(8),
        )
        .controls([
            k.flag_control("chat.author", "Author", false),
            k.flag_control("chat.timestamp", "Timestamp", false),
        ])
        .code(
            &code("enzui_widgets::chat_bubble", "enz_chat_bubble")
                .import("enzui_widgets::ChatRole")
                .arg("message.text.clone()")
                .arg("ChatRole::User")
                .set_if(author, "author", quoted("You"))
                .set_if(timestamp, "timestamp", quoted("09:41")),
        )
        .into_element()
    }

    fn media_controls_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let playlist = k.flag("media.playlist", false);
        let size = k.pick("media.size", 0);
        let mut controls = enzui_widgets::media_controls::enz_media_controls(
            "gallery-player",
            self.media_snapshot(),
        );
        if playlist {
            controls =
                controls.playlist_controls(enzui_widgets::media_controls::PlaylistTransport {
                    shuffle: self.shuffle,
                    repeat_mode: self.repeat,
                });
        }
        if size == 1 {
            controls = controls.transport_button_size(ButtonSize::Md);
        }
        self.specimen(
            "enz_media_controls",
            "Play, seek and volume for audio or video. It shows a media Snapshot and sends a message per action.",
            Element::from(controls).map(Message::Media),
        )
        .controls([
            k.flag_control("media.playlist", "Shuffle and repeat", false),
            k.pick_control("media.size", "Transport size", &["Default", "Md"], 0),
        ])
        .code(
            &code("enzui_widgets::media_controls", "enz_media_controls")
                .arg(quoted("player"))
                .arg("self.player.snapshot()")
                .import_if(playlist, "enzui_widgets::media_controls::PlaylistTransport")
                .set_if(
                    playlist,
                    "playlist_controls",
                    "PlaylistTransport {\n    shuffle: self.shuffle,\n    repeat_mode: self.repeat,\n}",
                )
                .import_if(size == 1, "enzui_widgets::ButtonSize")
                .set_if(size == 1, "transport_button_size", "ButtonSize::Md")
                .end(None),
        )
        .into_element()
    }

    fn labeled_content_specimen(&self) -> Element<'_, Message> {
        let value = self.knobs.text("labeled.value", "0.1.0");
        self.specimen(
            "enz_labeled_content",
            "A label and its value on one row, for read-only details.",
            enz_preferences_group(column![
                enzui_widgets::labeled_content::enz_labeled_content("Version")
                    .value(value)
                    .into_element(),
                enzui_widgets::labeled_content::enz_labeled_content("License")
                    .value("MIT OR Apache-2.0")
                    .into_element(),
            ])
            .bordered(true)
            .into_element(),
        )
        .controls([self.knobs.text_control("labeled.value", "Value", "0.1.0")])
        .code(
            &code("enzui_widgets::labeled_content", "enz_labeled_content")
                .arg(quoted("Version"))
                .set("value", quoted(value)),
        )
        .into_element()
    }

    fn divider_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let label = k.flag("divider.label", true);
        let fade = k.flag("divider.fade", false);
        let thickness = k.number("divider.thickness", 1.0);
        let mut divider = enzui_widgets::divider::enz_divider()
            .fade_ends(fade)
            .thickness(thickness);
        if label {
            divider = divider.label("or");
        }
        self.specimen(
            "enz_divider",
            "A line between groups, optionally with a word in the middle.",
            Element::from(divider),
        )
        .controls([
            k.flag_control("divider.label", "Label", true),
            k.flag_control("divider.fade", "Fade ends", false),
            k.number_control("divider.thickness", "Thickness", 1.0..=4.0, 1.0, 1.0),
        ])
        .code(
            &code("enzui_widgets::divider", "enz_divider")
                .set_if(label, "label", quoted("or"))
                .set_if(fade, "fade_ends", "true")
                .set_if(thickness != 1.0, "thickness", number(thickness))
                .end(None),
        )
        .into_element()
    }

    fn preview_row_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let lines = k.number("preview.lines", 2.0) as u32;
        let selected = k.flag("preview.selected", false);
        let time = k.flag("preview.time", true);
        let avatar = k.flag("preview.avatar", true);
        let unread = k.flag("preview.unread", true);
        let mut row = enzui_widgets::preview_row::enz_preview_row(
            "Weekly report",
            Message::Clicked("report"),
        )
        .excerpt(
            "Numbers are up across every region; see the attached sheet. The \
                 summary for the board follows on Monday, with the regional detail \
                 in the appendix.",
        )
        .excerpt_lines(lines)
        .unread(unread)
        .selected(selected);
        if time {
            row = row.trailing("9:41");
        }
        if avatar {
            row = row.leading(enzui_widgets::enz_avatar("Ada Lovelace").into_element());
        }
        self.specimen(
            "enz_preview_row",
            "A list row with a title and a clipped excerpt, as in a mail or notes list: an avatar or icon before it, a time after the title, and an unread dot.",
            enz_preferences_group(row.into_element())
                .bordered(true)
                .into_element(),
        )
        .controls([
            k.number_control("preview.lines", "Excerpt lines", 1.0..=4.0, 1.0, 2.0),
            k.flag_control("preview.time", "Time", true),
            k.flag_control("preview.avatar", "Avatar", true),
            k.flag_control("preview.unread", "Unread", true),
            k.flag_control("preview.selected", "Selected", false),
        ])
        .code(
            &code("enzui_widgets::preview_row", "enz_preview_row")
                .arg(quoted("Weekly report"))
                .arg("Message::Open(id)")
                .set("excerpt", "&message.excerpt")
                .set_if(lines != 2, "excerpt_lines", lines.to_string())
                .set_if(time, "trailing", "&message.time")
                .set_if(avatar, "leading", "enz_avatar(&message.from).into_element()")
                .set_if(unread, "unread", "!message.read")
                .set_if(selected, "selected", "true"),
        )
        .into_element()
    }
}
