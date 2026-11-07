use crate::*;
use enzui_core::placement::{Alignment, Side};
use enzui_widgets::MenuButtonKind;
use iced::widget::{column, row};

const MENU_KINDS: [MenuButtonKind; 3] = [
    MenuButtonKind::Default,
    MenuButtonKind::Flat,
    MenuButtonKind::Toolbar,
];
const MENU_KIND_NAMES: &[&str] = &["Default", "Flat", "Toolbar"];
const SIDES: [Side; 4] = [Side::Bottom, Side::Top, Side::Left, Side::Right];
const SIDE_NAMES: &[&str] = &["Bottom", "Top", "Left", "Right"];
const ALIGNMENTS: [Alignment; 3] = [Alignment::Start, Alignment::Center, Alignment::End];
const ALIGNMENT_NAMES: &[&str] = &["Start", "Center", "End"];

impl Gallery {
    pub(crate) fn dialogs_page(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let menu_kind = k.pick("menu.kind", 0);
        let side = k.pick("popover.side", 0);
        let alignment = k.pick("popover.alignment", 0);
        let destructive = k.flag("confirm.destructive", true);
        let confirm = k.text("confirm.label", "Delete");
        column![
            self.specimen(
                "enz_menu_button",
                "A button that opens a menu of commands. The core MenuModel lists them; apply_event() tracks the open state.",
                column![
                    enzui_widgets::enz_menu_button("File", &self.menu, &self.menu_state, Message::Menu)
                        .kind(MENU_KINDS[menu_kind])
                        .into_element(),
                    enz_text(self.last_action.clone()).into_element(),
                ]
                .spacing(12),
            )
            .controls([k.pick_control("menu.kind", "Kind", MENU_KIND_NAMES, 0)])
            .code(
                &code("enzui_widgets", "enz_menu_button")
                    .arg(quoted("File"))
                    .arg("&self.menu")
                    .arg("&self.menu_state")
                    .arg("Message::Menu")
                    .variant(
                        menu_kind != 0,
                        "kind",
                        "enzui_widgets",
                        "MenuButtonKind",
                        MENU_KIND_NAMES[menu_kind],
                    ),
            )
            .into_element(),
            self.specimen(
                "enz_popover",
                "Floating content anchored to a trigger, closed by Escape or a click outside. The app says whether it is open.",
                row![
                    enzui_widgets::popover::enz_popover(
                        enz_button("Share", Message::Share).into_element(),
                        self.share_open.then(|| {
                            column![
                                enz_text("Share “Quarterly report”").into_element(),
                                enz_text_field("Name or email", "", &self.name, Message::Name)
                                    .into_element(),
                                enz_checkbox(self.agree, "Can edit", Message::Agree).into_element(),
                            ]
                            .spacing(10)
                            .width(260)
                            .into()
                        }),
                        || Message::ShareClosed,
                    )
                    .side(SIDES[side])
                    .alignment(ALIGNMENTS[alignment])
                    .label("Share")
                    .into_element()
                ]
                .padding(iced::Padding::ZERO.left(if side == 2 { 280 } else { 0 })),
            )
            .controls([
                k.pick_control("popover.side", "Side", SIDE_NAMES, 0),
                k.pick_control("popover.alignment", "Alignment", ALIGNMENT_NAMES, 0),
            ])
            .code(
                &code("enzui_widgets::popover", "enz_popover")
                    .arg("enz_button(\"Share\", Message::Share).into_element()")
                    .arg("self.share_open.then(|| share_form())")
                    .arg("|| Message::ShareClosed")
                    .variant(side != 0, "side", "enzui_core::placement", "Side", SIDE_NAMES[side])
                    .variant(
                        alignment != 0,
                        "alignment",
                        "enzui_core::placement",
                        "Alignment",
                        ALIGNMENT_NAMES[alignment],
                    )
                    .set("label", quoted("Share")),
            )
            .into_element(),
            self.specimen(
                "enz_about_dialog",
                "The standard About window: name, version, credits, licence and links. It hosts itself over the page while open.",
                enz_button("About the gallery", Message::OpenAbout).into_element(),
            )
            .code_text(
                "use enzui_widgets::about_dialog::enz_about_dialog;\n\n\
                 // Wraps the whole window body; shows while self.about is true.\n\
                 enz_about_dialog(self.about, \"enzUI gallery\")\n    \
                     .icon(\"layout-grid\")\n    \
                     .version(\"0.1.0\")\n    \
                     .developer(\"The enzUI developers\")\n    \
                     .website(\"https://example.org/enzui\")\n    \
                     .license(\"MIT OR Apache-2.0\")\n    \
                     .into_host_with(body, Message::About)",
            )
            .into_element(),
            self.specimen(
                "enz_confirm_dialog",
                "Asks before an action that cannot be undone. A destructive one colours the confirm button.",
                column![
                    enz_button("Delete file…", Message::OpenDialog)
                        .kind(if destructive { ButtonKind::Destructive } else { ButtonKind::Primary })
                        .into_element(),
                    enz_text(self.last_action.clone()).into_element(),
                ]
                .spacing(12),
            )
            .controls([
                k.text_control("confirm.label", "Confirm label", "Delete"),
                k.flag_control("confirm.destructive", "Destructive", true),
            ])
            .code(
                &code("enzui_widgets::confirm_dialog", "enz_confirm_dialog")
                    .arg(quoted("Delete “report.pdf”?"))
                    .arg(quoted("The file is moved to the trash."))
                    .arg("&state")
                    .arg("Message::Dialog")
                    .set("confirm_label", quoted(confirm))
                    .set_if(destructive, "destructive", "true")
                    .set("into_host", "body")
                    .end(None),
            )
            .into_element(),
        ]
        .spacing(32)
        .into()
    }
}
