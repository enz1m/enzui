use crate::specimen::SIZE_NAMES;
use crate::*;
use enzui_widgets::{
    CheckboxSize, FieldVariant, Orientation, RadioGroupSize, StepperSize, TextFieldSize, ToggleSize,
};
use iced::widget::{column, row};

const FIELD_SIZES: [TextFieldSize; 3] = [TextFieldSize::Sm, TextFieldSize::Md, TextFieldSize::Lg];
const TOGGLE_SIZES: [ToggleSize; 3] = [ToggleSize::Sm, ToggleSize::Md, ToggleSize::Lg];
const CHECK_SIZES: [CheckboxSize; 3] = [CheckboxSize::Sm, CheckboxSize::Md, CheckboxSize::Lg];
const RADIO_SIZES: [RadioGroupSize; 3] =
    [RadioGroupSize::Sm, RadioGroupSize::Md, RadioGroupSize::Lg];
const STEPPER_SIZES: [StepperSize; 3] = [StepperSize::Sm, StepperSize::Md, StepperSize::Lg];

impl Gallery {
    pub(crate) fn input_page(&self) -> Element<'_, Message> {
        column![
            self.text_field_specimen(),
            self.secure_field_specimen(),
            self.search_field_specimen(),
            self.toggle_specimen(),
            self.checkbox_specimen(),
            self.radio_group_specimen(),
            self.slider_specimen(),
            self.range_slider_specimen(),
            self.stepper_specimen(),
            self.chip_input_specimen(),
            self.otp_specimen(),
            self.rating_input_specimen(),
            self.specimen(
                "RichEditor::view",
                "Formatted text: bold, italic, lists and links, with its own toolbar. The editor owns its document; the view renders it.",
                self.notes
                    .view(Message::Notes)
                    .editor_height(enzui_widgets::text_area::AreaHeight::Fixed(90.0))
                    .into_element(),
            )
            .code_text(
                "use enzui_widgets::rich_edit::RichEditor;\n\
                 use enzui_widgets::text_area::AreaHeight;\n\n\
                 // In the state: RichEditor::from_html(\"<strong>Agenda</strong>\")?\n\
                 // In update:    let _ = self.notes.update(action);\n\
                 self.notes\n    \
                     .view(Message::Notes)\n    \
                     .editor_height(AreaHeight::Fixed(90.0))\n    \
                     .into_element()",
            )
            .into_element(),
        ]
        .spacing(32)
        .width(Length::Fill)
        .into()
    }

    fn text_field_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let label = k.text("field.label", "Name");
        let placeholder = k.text("field.placeholder", "Ada Lovelace");
        let size = k.pick("field.size", 1);
        let outlined = k.pick("field.variant", 0) == 1;
        let described = k.flag("field.description", false);
        let error = k.flag("field.error", false);
        let enabled = k.flag("field.enabled", true);
        let mut field = enz_text_field(label, placeholder, &self.name, Message::Name)
            .size(FIELD_SIZES[size])
            .variant(if outlined {
                FieldVariant::Outlined
            } else {
                FieldVariant::Filled
            })
            .enabled(enabled);
        if described {
            field = field.description("As it appears on invoices");
        }
        if error {
            field = field.error("A name is needed");
        }
        self.specimen(
            "enz_text_field",
            "One line of text with a label above it. A description explains it; an error replaces the description and colours the border.",
            container(field.into_element()).width(Length::Fill.max(360.0)),
        )
        .controls([
            k.text_control("field.label", "Label", "Name"),
            k.text_control("field.placeholder", "Placeholder", "Ada Lovelace"),
            k.pick_control("field.size", "Size", SIZE_NAMES, 1),
            k.pick_control("field.variant", "Variant", &["Filled", "Outlined"], 0),
            k.flag_control("field.description", "Description", false),
            k.flag_control("field.error", "Error", false),
            k.flag_control("field.enabled", "Enabled", true),
        ])
        .code(
            &code("enzui_widgets", "enz_text_field")
                .arg(quoted(label))
                .arg(quoted(placeholder))
                .arg("&self.name")
                .arg("Message::Name")
                .variant(size != 1, "size", "enzui_widgets", "TextFieldSize", SIZE_NAMES[size])
                .variant(outlined, "variant", "enzui_widgets", "FieldVariant", "Outlined")
                .set_if(described, "description", quoted("As it appears on invoices"))
                .set_if(error, "error", quoted("A name is needed"))
                .set_if(!enabled, "enabled", "false"),
        )
        .into_element()
    }

    fn secure_field_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let label = k.text("secure.label", "Password");
        let size = k.pick("secure.size", 1);
        let error = k.flag("secure.error", false);
        let mut field = enz_secure_field(
            &self.password,
            Message::Password,
            self.revealed,
            Message::Reveal,
        )
        .label(label)
        .size(FIELD_SIZES[size]);
        if error {
            field = field.error("At least 8 characters");
        }
        self.specimen(
            "enz_secure_field",
            "A password field. The eye button shows the text; copying out of it is refused.",
            container(field.into_element()).width(Length::Fill.max(360.0)),
        )
        .controls([
            k.text_control("secure.label", "Label", "Password"),
            k.pick_control("secure.size", "Size", SIZE_NAMES, 1),
            k.flag_control("secure.error", "Error", false),
        ])
        .code(
            &code("enzui_widgets", "enz_secure_field")
                .arg("&self.password")
                .arg("Message::Password")
                .arg("self.revealed")
                .arg("Message::ToggleReveal")
                .set("label", quoted(label))
                .variant(
                    size != 1,
                    "size",
                    "enzui_widgets",
                    "TextFieldSize",
                    SIZE_NAMES[size],
                )
                .set_if(error, "error", quoted("At least 8 characters")),
        )
        .into_element()
    }

    fn search_field_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let placeholder = k.text("search.placeholder", "Search");
        let size = k.pick("search.size", 1);
        let enabled = k.flag("search.enabled", true);
        self.specimen(
            "enz_search_field",
            "A search box with a magnifier and a clear button.",
            container(
                enz_search_field(&self.search, Message::Search)
                    .placeholder(placeholder)
                    .size(FIELD_SIZES[size])
                    .enabled(enabled)
                    .into_element(),
            )
            .width(Length::Fill.max(360.0)),
        )
        .controls([
            k.text_control("search.placeholder", "Placeholder", "Search"),
            k.pick_control("search.size", "Size", SIZE_NAMES, 1),
            k.flag_control("search.enabled", "Enabled", true),
        ])
        .code(
            &code("enzui_widgets", "enz_search_field")
                .arg("&self.query")
                .arg("Message::Search")
                .set_if(placeholder != "Search", "placeholder", quoted(placeholder))
                .variant(
                    size != 1,
                    "size",
                    "enzui_widgets",
                    "TextFieldSize",
                    SIZE_NAMES[size],
                )
                .set_if(!enabled, "enabled", "false"),
        )
        .into_element()
    }

    fn toggle_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let label = k.text("toggle.label", "Send notifications");
        let size = k.pick("toggle.size", 1);
        let described = k.flag("toggle.description", false);
        let enabled = k.flag("toggle.enabled", true);
        let mut toggle = enz_toggle(self.notify, label, Message::Notify)
            .size(TOGGLE_SIZES[size])
            .enabled(enabled);
        if described {
            toggle = toggle.description("A banner for each new message");
        }
        self.specimen(
            "enz_toggle",
            "An on/off switch for a setting that applies at once.",
            toggle.into_element(),
        )
        .controls([
            k.text_control("toggle.label", "Label", "Send notifications"),
            k.pick_control("toggle.size", "Size", SIZE_NAMES, 1),
            k.flag_control("toggle.description", "Description", false),
            k.flag_control("toggle.enabled", "Enabled", true),
        ])
        .code(
            &code("enzui_widgets", "enz_toggle")
                .arg("self.notify")
                .arg(quoted(label))
                .arg("Message::Notify")
                .variant(
                    size != 1,
                    "size",
                    "enzui_widgets",
                    "ToggleSize",
                    SIZE_NAMES[size],
                )
                .set_if(
                    described,
                    "description",
                    quoted("A banner for each new message"),
                )
                .set_if(!enabled, "enabled", "false"),
        )
        .into_element()
    }

    fn checkbox_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let size = k.pick("check.size", 1);
        let enabled = k.flag("check.enabled", true);
        let size_of = CHECK_SIZES[size];
        let preview = column![
            enz_checkbox(
                self.extras[0] && self.extras[1],
                "All extras",
                Message::AllExtras
            )
            .mixed(self.extras[0] != self.extras[1])
            .size(size_of)
            .enabled(enabled)
            .into_element(),
            column![
                enz_checkbox(self.extras[0], "Spell checking", |on| Message::Extra(0, on))
                    .size(size_of)
                    .enabled(enabled)
                    .into_element(),
                enz_checkbox(self.extras[1], "Grammar hints", |on| Message::Extra(1, on))
                    .size(size_of)
                    .enabled(enabled)
                    .into_element(),
            ]
            .spacing(12)
            .padding(iced::Padding::ZERO.left(24)),
        ]
        .spacing(12);
        self.specimen(
            "enz_checkbox",
            "A yes/no choice. A parent shows the mixed state while only some of its children are on.",
            preview,
        )
        .controls([
            k.pick_control("check.size", "Size", SIZE_NAMES, 1),
            k.flag_control("check.enabled", "Enabled", true),
        ])
        .code(
            &code("enzui_widgets", "enz_checkbox")
                .arg("all")
                .arg(quoted("All extras"))
                .arg("Message::AllExtras")
                .set("mixed", "some && !all")
                .variant(size != 1, "size", "enzui_widgets", "CheckboxSize", SIZE_NAMES[size])
                .set_if(!enabled, "enabled", "false"),
        )
        .into_element()
    }

    fn radio_group_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let label = k.text("radio.label", "Size");
        let horizontal = k.pick("radio.orientation", 0) == 1;
        let size = k.pick("radio.size", 1);
        let enabled = k.flag("radio.enabled", true);
        self.specimen(
            "enz_radio_group",
            "One choice out of a short list, all visible at once.",
            enz_radio_group(label, &SIZES, self.size, Message::Size)
                .orientation(if horizontal {
                    Orientation::Horizontal
                } else {
                    Orientation::Vertical
                })
                .size(RADIO_SIZES[size])
                .enabled(enabled)
                .into_element(),
        )
        .controls([
            k.text_control("radio.label", "Label", "Size"),
            k.pick_control(
                "radio.orientation",
                "Orientation",
                &["Vertical", "Horizontal"],
                0,
            ),
            k.pick_control("radio.size", "Size", SIZE_NAMES, 1),
            k.flag_control("radio.enabled", "Enabled", true),
        ])
        .code(
            &code("enzui_widgets", "enz_radio_group")
                .arg(quoted(label))
                .arg("&[\"Small\", \"Medium\", \"Large\"]")
                .arg("self.size")
                .arg("Message::Size")
                .variant(
                    horizontal,
                    "orientation",
                    "enzui_widgets",
                    "Orientation",
                    "Horizontal",
                )
                .variant(
                    size != 1,
                    "size",
                    "enzui_widgets",
                    "RadioGroupSize",
                    SIZE_NAMES[size],
                )
                .set_if(!enabled, "enabled", "false"),
        )
        .into_element()
    }

    fn slider_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let step = k.number("slider.step", 0.0);
        let labelled = k.flag("slider.label", false);
        let show_value = k.flag("slider.show_value", false);
        let marks = k.flag("slider.marks", true);
        let enabled = k.flag("slider.enabled", true);
        let grow = k.flag("slider.grow", false);
        let mut slider = enz_slider(0.0..=1.0, self.volume, Message::Volume)
            .thumb_grow(grow)
            .show_value(show_value)
            .step_marks(marks)
            .enabled(enabled);
        if step > 0.0 {
            slider = slider.step(step);
        }
        if labelled {
            slider = slider.label("Volume");
        }
        self.specimen(
            "enz_slider",
            "A value in a range, set by dragging or with the arrow keys. A step snaps it; marks show the steps.",
            container(slider.into_element()).width(Length::Fill.max(360.0)),
        )
        .controls([
            k.number_control("slider.step", "Step (0 = none)", 0.0..=0.5, 0.05, 0.0),
            k.flag_control("slider.label", "Label", false),
            k.flag_control("slider.show_value", "Show value", false),
            k.flag_control("slider.marks", "Step marks", true),
            k.flag_control("slider.grow", "Thumb grows on hover", false),
            k.flag_control("slider.enabled", "Enabled", true),
        ])
        .code(
            &code("enzui_widgets", "enz_slider")
                .arg("0.0..=1.0")
                .arg("self.volume")
                .arg("Message::Volume")
                .set_if(step > 0.0, "step", number(step))
                .set_if(labelled, "label", quoted("Volume"))
                .set_if(show_value, "show_value", "true")
                .set_if(!marks, "step_marks", "false")
                .set_if(grow, "thumb_grow", "true")
                .set_if(!enabled, "enabled", "false"),
        )
        .into_element()
    }

    fn range_slider_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let step = k.number("range.step", 5.0);
        let enabled = k.flag("range.enabled", true);
        self.specimen(
            "enz_range_slider",
            "A low and a high value in one track, for a price or date range.",
            row![
                container(
                    enzui_widgets::enz_range_slider(0.0..=100.0, self.price, Message::Price)
                        .step(step.max(1.0))
                        .label("Price")
                        .enabled(enabled)
                        .into_element()
                )
                .width(Length::Fill.max(360.0)),
                enz_text(format!("€{:.0} – €{:.0}", self.price.0, self.price.1)).into_element(),
            ]
            .spacing(12)
            .align_y(iced::Alignment::Center),
        )
        .controls([
            k.number_control("range.step", "Step", 1.0..=25.0, 1.0, 5.0),
            k.flag_control("range.enabled", "Enabled", true),
        ])
        .code(
            &code("enzui_widgets", "enz_range_slider")
                .arg("0.0..=100.0")
                .arg("self.price")
                .arg("Message::Price")
                .set("step", number(step.max(1.0)))
                .set("label", quoted("Price"))
                .set_if(!enabled, "enabled", "false"),
        )
        .into_element()
    }

    fn stepper_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let decimal = k.pick("stepper.kind", 0) == 1;
        let wrap = k.flag("stepper.wrap", false);
        let size = k.pick("stepper.size", 1);
        let labelled = k.flag("stepper.label", true);
        let editable = k.flag("stepper.editable", true);
        let range_error = k.flag("stepper.range_error", true);
        let places = k.pick("stepper.places", 2);
        let decimal_step = [1.0, 0.1, 0.05][places.min(2)];
        let max = k.number("stepper.max", 99.0) as i32;
        let step = k.number("stepper.step", 1.0) as i32;
        let label = if decimal { "Line height" } else { "Copies" };
        let stepper: Element<'_, Message> = if decimal {
            let mut stepper = enz_stepper(self.line_height, Message::LineHeight)
                .min(0.8)
                .max(3.0)
                .step(decimal_step)
                .wrap(wrap)
                .editable(editable)
                .range_error(range_error)
                .size(STEPPER_SIZES[size]);
            if labelled {
                stepper = stepper.label(label);
            }
            stepper.into_element()
        } else {
            let mut stepper = enz_stepper(self.copies, Message::Copies)
                .min(1)
                .max(max)
                .step(step)
                .wrap(wrap)
                .editable(editable)
                .range_error(range_error)
                .size(STEPPER_SIZES[size]);
            if labelled {
                stepper = stepper.label(label);
            }
            stepper.into_element()
        };
        let mut controls = vec![k.pick_control("stepper.kind", "Value", &["Whole", "Decimal"], 0)];
        if decimal {
            controls.push(k.pick_control("stepper.places", "Step", &["1", "0.1", "0.05"], 2));
        } else {
            controls.push(k.number_control("stepper.max", "Max", 5.0..=99.0, 1.0, 99.0));
            controls.push(k.number_control("stepper.step", "Step", 1.0..=10.0, 1.0, 1.0));
        }
        controls.extend([
            k.pick_control("stepper.size", "Size", SIZE_NAMES, 1),
            k.flag_control("stepper.wrap", "Wrap", false),
            k.flag_control("stepper.label", "Label", true),
            k.flag_control("stepper.editable", "Editable", true),
            k.flag_control("stepper.range_error", "Range error", true),
        ]);
        let base = if decimal {
            code("enzui_widgets", "enz_stepper")
                .arg("self.line_height")
                .arg("Message::LineHeight")
                .set("min", "0.8")
                .set("max", "3.0")
                .set("step", number(decimal_step as f32))
        } else {
            code("enzui_widgets", "enz_stepper")
                .arg("self.copies")
                .arg("Message::Copies")
                .set("min", "1")
                .set("max", max.to_string())
                .set_if(step != 1, "step", step.to_string())
        };
        self.specimen(
            "enz_stepper",
            "A number with minus and plus buttons: whole (i32, u32) or decimal (f32, f64), which keeps the places its step needs. Click the value to type one; Enter or clicking away commits, Escape cancels.",
            stepper,
        )
        .controls(controls)
        .code(
            &base
                .set_if(wrap, "wrap", "true")
                .set_if(!editable, "editable", "false")
                .set_if(range_error, "range_error", "true")
                .variant(
                    size != 1,
                    "size",
                    "enzui_widgets",
                    "StepperSize",
                    SIZE_NAMES[size],
                )
                .set_if(labelled, "label", quoted(label)),
        )
        .into_element()
    }

    fn chip_input_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let placeholder = k.text("chips.placeholder", "Add a tag");
        let enabled = k.flag("chips.enabled", true);
        self.specimen(
            "enz_chip_input",
            "A list of short values typed into one field: Enter or a comma makes a chip, Backspace removes the last.",
            container(
                enzui_widgets::enz_chip_input(&self.tags, &self.tag_draft, Message::Tags)
                    .label("Tags")
                    .placeholder(placeholder)
                    .enabled(enabled)
                    .into_element(),
            )
            .width(Length::Fill.max(420.0)),
        )
        .controls([
            k.text_control("chips.placeholder", "Placeholder", "Add a tag"),
            k.flag_control("chips.enabled", "Enabled", true),
        ])
        .code_text(format!(
            "use enzui_widgets::chip_input::{{DEFAULT_SEPARATORS, apply_chip_event}};\n\
             use enzui_widgets::enz_chip_input;\n\n\
             // In update:\n\
             // Message::Tags(event) => apply_chip_event(\n\
             //     &mut self.tags, &mut self.draft, &event, DEFAULT_SEPARATORS,\n\
             // ),\n\
             enz_chip_input(&self.tags, &self.draft, Message::Tags)\n    \
                 .label(\"Tags\")\n    \
                 .placeholder({}){}\n    \
                 .into_element()",
            quoted(placeholder),
            if enabled { "" } else { "\n    .enabled(false)" },
        ))
        .into_element()
    }

    fn otp_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let length = k.number("otp.length", 6.0) as usize;
        let group = k.number("otp.group", 3.0) as usize;
        let invalid = k.flag("otp.invalid", false);
        self.specimen(
            "enz_input_otp",
            "A one-time code, one cell per character, grouped for reading.",
            enzui_widgets::enz_input_otp(&self.code, Message::Code)
                .label("Verification code")
                .length(length)
                .group_size(group)
                .invalid(invalid)
                .into_element(),
        )
        .controls([
            k.number_control("otp.length", "Length", 4.0..=8.0, 1.0, 6.0),
            k.number_control("otp.group", "Group size", 0.0..=4.0, 1.0, 3.0),
            k.flag_control("otp.invalid", "Invalid", false),
        ])
        .code(
            &code("enzui_widgets", "enz_input_otp")
                .arg("&self.code")
                .arg("Message::Code")
                .set("label", quoted("Verification code"))
                .set_if(length != 6, "length", length.to_string())
                .set_if(group != 3, "group_size", group.to_string())
                .set_if(invalid, "invalid", "true"),
        )
        .into_element()
    }

    fn rating_input_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let max = k.number("rating_input.max", 5.0) as u8;
        self.specimen(
            "enz_rating_input",
            "A star rating the user sets by clicking or with the arrow keys.",
            enzui_widgets::rating_input::enz_rating_input(self.stars.min(max), max, Message::Stars),
        )
        .controls([k.number_control("rating_input.max", "Stars", 3.0..=10.0, 1.0, 5.0)])
        .code(
            &code("enzui_widgets::rating_input", "enz_rating_input")
                .arg("self.stars")
                .arg(max.to_string())
                .arg("Message::Stars")
                .end(None),
        )
        .into_element()
    }
}
