use crate::specimen::SIZE_NAMES;
use crate::*;
use enzui_widgets::TextFieldSize;
use iced::widget::column;

const FRUITS: [&str; 11] = [
    "Apple",
    "Apricot",
    "Banana",
    "Blackberry",
    "Cherry",
    "Damson",
    "Grape",
    "Mango",
    "Orange",
    "Peach",
    "Pear",
];
const FIELD_SIZES: [TextFieldSize; 3] = [TextFieldSize::Sm, TextFieldSize::Md, TextFieldSize::Lg];
const INTERVALS: &[&str] = &["15", "30", "60"];

impl Gallery {
    pub(crate) fn selection_page(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        column![
            {
                let label = k.text("combo.label", "Favourite fruit");
                let placeholder = k.text("combo.placeholder", "Pick one");
                let searchable = k.flag("combo.searchable", true);
                let size = k.pick("combo.size", 1);
                let error = k.flag("combo.error", false);
                let enabled = k.flag("combo.enabled", true);
                let mut combo = enz_combo_box(
                    label,
                    FRUITS.map(String::from).to_vec(),
                    self.fruit.clone(),
                    Message::Fruit,
                )
                .placeholder(placeholder)
                .size(FIELD_SIZES[size])
                .enabled(enabled);
                if searchable {
                    combo = combo.searchable(self.fruit_query.clone(), Message::FruitQuery);
                }
                if error {
                    combo = combo.error("Pick a fruit");
                }
                self.specimen(
                    "enz_combo_box",
                    "A dropdown: one choice out of a long list. Searchable filters the list as you type.",
                    container(combo.into_element()).width(Length::Fill.max(360.0)),
                )
                .controls([
                    k.text_control("combo.label", "Label", "Favourite fruit"),
                    k.text_control("combo.placeholder", "Placeholder", "Pick one"),
                    k.pick_control("combo.size", "Size", SIZE_NAMES, 1),
                    k.flag_control("combo.searchable", "Searchable", true),
                    k.flag_control("combo.error", "Error", false),
                    k.flag_control("combo.enabled", "Enabled", true),
                ])
                .code(
                    &code("enzui_widgets", "enz_combo_box")
                        .arg(quoted(label))
                        .arg("fruits.clone()")
                        .arg("self.fruit.clone()")
                        .arg("Message::Fruit")
                        .set("placeholder", quoted(placeholder))
                        .variant(size != 1, "size", "enzui_widgets", "TextFieldSize", SIZE_NAMES[size])
                        .set_if(
                            searchable,
                            "searchable",
                            "self.query.clone(), Message::Query",
                        )
                        .set_if(error, "error", quoted("Pick a fruit"))
                        .set_if(!enabled, "enabled", "false"),
                )
                .into_element()
            },
            {
                let enabled = k.flag("radio_button.enabled", true);
                self.specimen(
                    "enz_radio_button",
                    "A single radio option, for laying out choices yourself; all options share one selected value.",
                    column(["Free", "Personal", "Team"].into_iter().zip(0u8..).map(
                        |(label, value)| {
                            enzui_widgets::radio_button::enz_radio_button(
                                label,
                                value,
                                self.plan,
                                Message::Plan,
                            )
                            .enabled(enabled)
                            .into_element()
                        },
                    ))
                    .spacing(6),
                )
                .controls([k.flag_control("radio_button.enabled", "Enabled", true)])
                .code(
                    &code("enzui_widgets::radio_button", "enz_radio_button")
                        .arg(quoted("Personal"))
                        .arg("Plan::Personal")
                        .arg("self.plan")
                        .arg("Message::Plan")
                        .set_if(!enabled, "enabled", "false"),
                )
                .into_element()
            },
            {
                let label = k.text("time.label", "Meeting starts");
                let interval = k.pick("time.interval", 1);
                let minutes: u32 = INTERVALS[interval].parse().unwrap_or(30);
                let enabled = k.flag("time.enabled", true);
                self.specimen(
                    "enz_time_picker",
                    "A time of day: type it, or pick from a list at a fixed interval.",
                    container(
                        enzui_widgets::enz_time_picker(
                            &self.time_draft,
                            self.time,
                            self.time_open,
                            Message::Time,
                        )
                        .format(time_format())
                        .label(label)
                        .interval_minutes(minutes)
                        .enabled(enabled)
                        .into_element(),
                    )
                    .width(Length::Fill.max(360.0)),
                )
                .controls([
                    k.text_control("time.label", "Label", "Meeting starts"),
                    k.pick_control("time.interval", "Interval (minutes)", INTERVALS, 1),
                    k.flag_control("time.enabled", "Enabled", true),
                ])
                .code(
                    &code("enzui_widgets", "enz_time_picker")
                        .arg("&self.time_draft")
                        .arg("self.time")
                        .arg("self.time_open")
                        .arg("Message::Time")
                        .import("enzui_core::time_of_day::{HourCycle, TimeFormat}")
                        .set("format", "TimeFormat::new(HourCycle::H23)")
                        .set("label", quoted(label))
                        .set_if(minutes != 30, "interval_minutes", minutes.to_string())
                        .set_if(!enabled, "enabled", "false"),
                )
                .into_element()
            },
            {
                let title = k.text("prefs.title", "Printing");
                let bordered = k.flag("prefs.bordered", true);
                let described = k.flag("prefs.description", false);
                let subtitles = k.flag("prefs.subtitles", true);
                let row_of = |title: &'static str, subtitle: &'static str| {
                    let row = enz_preferences_row(title);
                    if subtitles { row.subtitle(subtitle) } else { row }
                };
                let mut group = enz_preferences_group(
                    column![
                        row_of("Notifications", "Show a banner for new messages")
                            .control(enz_toggle(self.notify, "", Message::Notify).into_element())
                            .into_element(),
                        row_of("Copies", "How many to print")
                            .control(enz_stepper(self.copies, Message::Copies).into_element())
                            .into_element(),
                    ]
                    .spacing(0),
                )
                .bordered(bordered)
                .title(title);
                if described {
                    group = group.description("Applies to every printer.");
                }
                self.specimen(
                    "enz_preferences_group + enz_preferences_row",
                    "A settings list: each row has a title, an optional subtitle and a control on the right.",
                    group.into_element(),
                )
                .controls([
                    k.text_control("prefs.title", "Group title", "Printing"),
                    k.flag_control("prefs.bordered", "Bordered", true),
                    k.flag_control("prefs.description", "Description", false),
                    k.flag_control("prefs.subtitles", "Row subtitles", true),
                ])
                .code_text(format!(
                    "use enzui_widgets::{{enz_preferences_group, enz_preferences_row, enz_stepper, enz_toggle}};\n\n\
                     enz_preferences_group(column![\n    \
                         enz_preferences_row(\"Notifications\"){}\n        \
                             .control(enz_toggle(self.notify, \"\", Message::Notify).into_element())\n        \
                             .into_element(),\n    \
                         enz_preferences_row(\"Copies\"){}\n        \
                             .control(enz_stepper(self.copies, Message::Copies).into_element())\n        \
                             .into_element(),\n\
                     ])\n\
                     .bordered({bordered})\n\
                     .title({}){}\n\
                     .into_element()",
                    if subtitles { "\n        .subtitle(\"Show a banner for new messages\")" } else { "" },
                    if subtitles { "\n        .subtitle(\"How many to print\")" } else { "" },
                    quoted(title),
                    if described { "\n.description(\"Applies to every printer.\")" } else { "" },
                ))
                .into_element()
            },
        ]
        .spacing(32)
        .width(Length::Fill)
        .into()
    }
}
