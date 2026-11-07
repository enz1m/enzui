use crate::specimen::SIZE_NAMES;
use crate::*;
use enzui_widgets::{SwitcherStyle, TabListWidth, TabPillItem, TabSelectionStyle, TextFieldSize};
use iced::widget::column;

const FIELD_SIZES: [TextFieldSize; 3] = [TextFieldSize::Sm, TextFieldSize::Md, TextFieldSize::Lg];
const SELECTIONS: [TabSelectionStyle; 3] = [
    TabSelectionStyle::FilledAccent,
    TabSelectionStyle::TranslucentAccent,
    TabSelectionStyle::NeutralMaterial,
];
const SELECTION_NAMES: &[&str] = &["FilledAccent", "TranslucentAccent", "NeutralMaterial"];

impl Gallery {
    pub(crate) fn navigation_page(&self) -> Element<'_, Message> {
        column![
            self.view_switcher_specimen(),
            self.tab_list_specimen(),
            self.breadcrumb_specimen(),
            self.pagination_specimen(),
            self.disclosure_specimen(),
            self.expander_specimen(),
            self.accordion_specimen(),
            self.wizard_specimen(),
            self.sidebar_row_specimen(),
        ]
        .spacing(32)
        .into()
    }

    fn view_switcher_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let inline = k.pick("switcher.style", 0) == 1;
        let icons = k.flag("switcher.icons", false);
        let switcher_collapse = k.flag("switcher.collapse", false);
        let badge = k.flag("switcher.badge", false);
        let size = k.pick("switcher.size", 1);
        let page = |id: &'static str, label: &'static str, icon: &'static str| {
            let page = SwitcherPage::new(id, label);
            let page = if icons { page.with_icon(icon) } else { page };
            if badge && id == "inbox" {
                page.with_badge("3")
            } else {
                page
            }
        };
        let page_code = |id: &str, label: &str, icon: &str| {
            format!(
                "SwitcherPage::new({}, {}){}{}",
                quoted(id),
                quoted(label),
                if icons {
                    format!(".with_icon({})", quoted(icon))
                } else {
                    String::new()
                },
                if badge && id == "inbox" {
                    ".with_badge(\"3\")"
                } else {
                    ""
                },
            )
        };
        self.specimen(
            "enz_view_switcher",
            "Switches between the main views of a window, usually in its titlebar. It is the tab list below on a track, with the same cells, keys and selection styles.",
            column![
                enz_view_switcher(
                    vec![
                        page("inbox", "Inbox", "inbox"),
                        page("sent", "Sent", "send"),
                        page("archive", "Archive", "archive"),
                    ],
                    self.tab,
                    Message::Tab,
                )
                .style(if inline {
                    SwitcherStyle::InlineSegmented
                } else {
                    SwitcherStyle::PillStrip
                })
                .size(FIELD_SIZES[size])
                .collapse_inactive_labels(icons && switcher_collapse)
                .into_element(),
                enz_text(format!("Showing tab {}", self.tab + 1)).into_element(),
            ]
            .spacing(12),
        )
        .controls([
            self.knobs.pick_control(
                "switcher.style",
                "Style",
                &["PillStrip", "InlineSegmented"],
                0,
            ),
            self.knobs
                .pick_control("switcher.size", "Size", SIZE_NAMES, 1),
            self.knobs.flag_control("switcher.icons", "Icons", false),
            self.knobs.flag_control("switcher.badge", "Badge", false),
            self.knobs
                .flag_control("switcher.collapse", "Label on selected only", false),
        ])
        .code(
            &code("enzui_widgets", "enz_view_switcher")
                .import("enzui_widgets::SwitcherPage")
                .arg(format!(
                    "vec![\n    {},\n    {},\n    {},\n]",
                    page_code("inbox", "Inbox", "inbox"),
                    page_code("sent", "Sent", "send"),
                    page_code("archive", "Archive", "archive"),
                ))
                .arg("self.view")
                .arg("Message::ShowView")
                .variant(
                    inline,
                    "style",
                    "enzui_widgets",
                    "SwitcherStyle",
                    "InlineSegmented",
                )
                .set_if(icons && switcher_collapse, "collapse_inactive_labels", "true")
                .variant(
                    size != 1,
                    "size",
                    "enzui_widgets",
                    "TextFieldSize",
                    SIZE_NAMES[size],
                ),
        )
        .into_element()
    }

    fn tab_list_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let fill = k.pick("tabs.width", 0) == 1;
        let icons = k.flag("tabs.icons", false);
        let selection = k.pick("tabs.selection", 0);
        let item = |label: &'static str, icon: &'static str| {
            let item = TabPillItem::new(label);
            if icons { item.icon(icon) } else { item }
        };
        self.specimen(
            "enz_tab_list",
            "Tabs that switch the content below them, inside a page. On a track, the same strip is the view switcher.",
            enzui_widgets::enz_tab_list(
                [
                    item("Overview", "layout-dashboard"),
                    item("Activity", "activity"),
                    item("Settings", "settings"),
                ],
                self.section,
                Message::Section,
            )
            .width_policy(if fill { TabListWidth::Fill } else { TabListWidth::Hug })
            .selection_indicator(SELECTIONS[selection])
            .collapse_inactive_labels(icons && k.flag("tabs.collapse", false))
            .into_element(),
        )
        .controls([
            k.pick_control("tabs.width", "Width", &["Hug", "Fill"], 0),
            k.pick_control("tabs.selection", "Selection", SELECTION_NAMES, 0),
            k.flag_control("tabs.icons", "Icons", false),
            k.flag_control("tabs.collapse", "Label on selected only", false),
        ])
        .code(
            &code("enzui_widgets", "enz_tab_list")
                .arg(if icons {
                    "[\n    TabPillItem::new(\"Overview\").icon(\"layout-dashboard\"),\n    TabPillItem::new(\"Activity\").icon(\"activity\"),\n    TabPillItem::new(\"Settings\").icon(\"settings\"),\n]"
                } else {
                    "[\"Overview\", \"Activity\", \"Settings\"]"
                })
                .arg("self.section")
                .arg("Message::Section")
                .import_if(icons, "enzui_widgets::TabPillItem")
                .variant(fill, "width_policy", "enzui_widgets", "TabListWidth", "Fill")
                .set_if(
                    icons && k.flag("tabs.collapse", false),
                    "collapse_inactive_labels",
                    "true",
                )
                .variant(
                    selection != 0,
                    "selection_indicator",
                    "enzui_widgets",
                    "TabSelectionStyle",
                    SELECTION_NAMES[selection],
                ),
        )
        .into_element()
    }

    fn breadcrumb_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let separator = k.pick("crumb.separator", 0);
        let max_visible = k.number("crumb.max", 4.0) as usize;
        let mut crumbs = enzui_widgets::enz_breadcrumb(
            vec![
                "Home".to_owned(),
                "Projects".to_owned(),
                "enzUI".to_owned(),
                "docs".to_owned(),
            ],
            Message::Crumb,
        )
        .max_visible(max_visible);
        if separator == 1 {
            crumbs = crumbs.separator("/");
        }
        self.specimen(
            "enz_breadcrumb",
            "Where the user is in a hierarchy; each crumb goes back to that level. Hidden middle crumbs fold into a menu.",
            column![
                crumbs.into_element(),
                enz_text(format!("Crumb {} selected", self.crumb + 1)).into_element(),
            ]
            .spacing(12),
        )
        .controls([
            k.pick_control("crumb.separator", "Separator", &["Chevron", "Slash"], 0),
            k.number_control("crumb.max", "Max visible", 2.0..=4.0, 1.0, 4.0),
        ])
        .code(
            &code("enzui_widgets", "enz_breadcrumb")
                .arg("vec![\"Home\".into(), \"Projects\".into(), \"enzUI\".into(), \"docs\".into()]")
                .arg("Message::Crumb")
                .set_if(separator == 1, "separator", quoted("/"))
                .set_if(max_visible != 4, "max_visible", max_visible.to_string()),
        )
        .into_element()
    }

    fn pagination_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let pages = k.number("pages.count", 12.0) as i32;
        let siblings = k.number("pages.siblings", 1.0) as i32;
        let compact = k.flag("pages.compact", false);
        self.specimen(
            "enz_pagination",
            "Moves through numbered pages of results.",
            enzui_widgets::enz_pagination(pages, self.page_no.min(pages), Message::PageNo)
                .sibling_count(siblings)
                .compact(compact)
                .into_element(),
        )
        .controls([
            k.number_control("pages.count", "Pages", 2.0..=40.0, 1.0, 12.0),
            k.number_control("pages.siblings", "Siblings", 0.0..=3.0, 1.0, 1.0),
            k.flag_control("pages.compact", "Compact", false),
        ])
        .code(
            &code("enzui_widgets", "enz_pagination")
                .arg(pages.to_string())
                .arg("self.page")
                .arg("Message::Page")
                .set_if(siblings != 1, "sibling_count", siblings.to_string())
                .set_if(compact, "compact", "true"),
        )
        .into_element()
    }

    fn disclosure_specimen(&self) -> Element<'_, Message> {
        let label = self.knobs.text("disclosure.label", "Show details");
        self.specimen(
            "enz_disclosure",
            "A chevron and label that show or hide what follows. The app decides what to show.",
            column![
                enzui_widgets::enz_disclosure(label, self.details, Message::Details).into_element(),
                Element::from(if self.details {
                    container(
                        enz_text("Disclosed content sits under its toggle, indented to the label.")
                            .into_element(),
                    )
                    .padding(iced::Padding::ZERO.left(24))
                    .into()
                } else {
                    Element::from(Space::new())
                }),
            ]
            .spacing(8),
        )
        .controls([self
            .knobs
            .text_control("disclosure.label", "Label", "Show details")])
        .code_text(format!(
            "use enzui_widgets::enz_disclosure;\n\n\
             column![\n    \
                 enz_disclosure({}, self.details, Message::Details).into_element(),\n    \
                 // Shown only while open:\n    \
                 if self.details {{ details() }} else {{ Space::new().into() }},\n\
             ]",
            quoted(label)
        ))
        .into_element()
    }

    fn expander_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let subtitle = k.flag("expander.subtitle", true);
        let icon = k.flag("expander.icon", false);
        let switch = k.flag("expander.switch", false);
        let mut expander =
            enzui_widgets::enz_expander("Advanced options", self.advanced, Message::Advanced)
                .content(enz_text("Expanded content goes here.").into_element());
        if subtitle {
            expander = expander.subtitle("Proxy, cache and logging");
        }
        if icon {
            expander = expander.icon("settings");
        }
        if switch {
            expander = expander
                .show_enable_switch(true)
                .enable_value(self.notify)
                .on_enable_toggled(Message::Notify);
        }
        self.specimen(
            "enz_expander",
            "A row that opens to show more settings under it. It can carry its own on/off switch.",
            expander.into_element(),
        )
        .controls([
            k.flag_control("expander.subtitle", "Subtitle", true),
            k.flag_control("expander.icon", "Icon", false),
            k.flag_control("expander.switch", "Enable switch", false),
        ])
        .code(
            &code("enzui_widgets", "enz_expander")
                .arg(quoted("Advanced options"))
                .arg("self.advanced")
                .arg("Message::Advanced")
                .set(
                    "content",
                    "enz_text(\"Expanded content goes here.\").into_element()",
                )
                .set_if(subtitle, "subtitle", quoted("Proxy, cache and logging"))
                .set_if(icon, "icon", quoted("settings"))
                .set_if(switch, "show_enable_switch", "true")
                .set_if(switch, "enable_value", "self.proxy_on")
                .set_if(switch, "on_enable_toggled", "Message::Proxy"),
        )
        .into_element()
    }

    fn accordion_specimen(&self) -> Element<'_, Message> {
        self.specimen(
            "enz_accordion",
            "Sections of which at most one is open; opening one closes the others.",
            enzui_widgets::accordion::enz_accordion(
                [
                    (
                        "What is enzUI?".to_owned(),
                        enz_text("A Linux GUI framework built on iced.").into_element(),
                    ),
                    (
                        "Does it follow my theme?".to_owned(),
                        enz_text("Yes: it reads the shared theme.toml.").into_element(),
                    ),
                ],
                self.faq,
                Message::Faq,
            ),
        )
        .code(
            &code("enzui_widgets::accordion", "enz_accordion")
                .arg("[\n    (\"What is enzUI?\".to_owned(), answer_one()),\n    (\"Does it follow my theme?\".to_owned(), answer_two()),\n]")
                .arg("self.open_question")
                .arg("Message::OpenQuestion")
                .end(None),
        )
        .into_element()
    }

    fn wizard_specimen(&self) -> Element<'_, Message> {
        self.specimen(
            "enz_wizard",
            "Steps through a task one page at a time. Next stays off until the page can go on.",
            container(enzui_widgets::wizard::enz_wizard(
                &self.wizard,
                &["Account", "Profile", "Done"].map(str::to_owned),
                match self.wizard.current() {
                    0 => enz_text_field("Your name", "Ada Lovelace", &self.name, Message::Name)
                        .into_element(),
                    1 => enz_text("Pick a picture later in Settings.").into_element(),
                    _ => enz_text(format!("All set, {}.", self.name)).into_element(),
                },
                !self.name.trim().is_empty(),
                Message::Wizard,
            ))
            .height(Length::Shrink),
        )
        .code_text(
            "use enzui_widgets::wizard::{apply, enz_wizard};\n\n\
             // In the state: wizard: enzui_core::wizard::Wizard::new(3)\n\
             // In update:    Message::Wizard(event) => { apply(&mut self.wizard, event); }\n\
             enz_wizard(\n    \
                 &self.wizard,\n    \
                 &[\"Account\", \"Profile\", \"Done\"].map(str::to_owned),\n    \
                 self.step_page(),\n    \
                 !self.name.trim().is_empty(),\n    \
                 Message::Wizard,\n\
             )",
        )
        .into_element()
    }

    fn sidebar_row_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let icon = k.flag("row.icon", true);
        let count = k.flag("row.count", false);
        let selected = k.flag("row.selected", true);
        let row_of = |label: &'static str, icon_name: &'static str, chosen: bool| {
            let mut row = enz_sidebar_row(label, Message::Clicked(label)).selected(chosen);
            if icon {
                row = row.icon(icon_name);
            }
            if count && label == "Downloads" {
                row = row.count("4");
            }
            row.into_element()
        };
        self.specimen(
            "enz_sidebar_row",
            "A row in a sidebar: an icon, a label and an optional count. The selected row is highlighted.",
            column![
                row_of("Documents", "folder", selected),
                row_of("Downloads", "download", false),
            ]
            .width(260),
        )
        .controls([
            k.flag_control("row.icon", "Icon", true),
            k.flag_control("row.count", "Count", false),
            k.flag_control("row.selected", "Selected", true),
        ])
        .code(
            &code("enzui_widgets", "enz_sidebar_row")
                .arg(quoted("Downloads"))
                .arg("Message::Show(Place::Downloads)")
                .set_if(icon, "icon", quoted("download"))
                .set_if(count, "count", quoted("4"))
                .set("selected", "self.place == Place::Downloads"),
        )
        .into_element()
    }
}
