use crate::*;
use enzui_widgets::button::ButtonIconPosition;
use enzui_widgets::text_field::TextFieldSize;
use enzui_widgets::{LinkTone, ToolbarIconVariant};
use iced::widget::{column, row};

const KINDS: [ButtonKind; 11] = [
    ButtonKind::Primary,
    ButtonKind::Accent,
    ButtonKind::Secondary,
    ButtonKind::Outlined,
    ButtonKind::Plain,
    ButtonKind::Quiet,
    ButtonKind::Ghost,
    ButtonKind::Success,
    ButtonKind::Destructive,
    ButtonKind::DestructiveOutlined,
    ButtonKind::QuietDestructive,
];
const KIND_NAMES: &[&str] = &[
    "Primary",
    "Accent",
    "Secondary",
    "Outlined",
    "Plain",
    "Quiet",
    "Ghost",
    "Success",
    "Destructive",
    "DestructiveOutlined",
    "QuietDestructive",
];
const SIZES: [ButtonSize; 5] = [
    ButtonSize::Xs,
    ButtonSize::Sm,
    ButtonSize::Md,
    ButtonSize::Lg,
    ButtonSize::Xl,
];
const SIZE_NAMES: &[&str] = &["Xs", "Sm", "Md", "Lg", "Xl"];
const ICONS: &[&str] = &["None", "save", "download", "trash-2"];
const ICON_SIDES: [ButtonIconPosition; 3] = [
    ButtonIconPosition::Leading,
    ButtonIconPosition::Trailing,
    ButtonIconPosition::Only,
];
const ICON_SIDE_NAMES: &[&str] = &["Leading", "Trailing", "Only"];
const FIELD_SIZES: [TextFieldSize; 3] = [TextFieldSize::Sm, TextFieldSize::Md, TextFieldSize::Lg];
const FIELD_SIZE_NAMES: &[&str] = &["Sm", "Md", "Lg"];
const TOOLBAR_ICONS: &[&str] = &["undo", "redo", "trash-2", "search"];
const TOOLBAR_LABELS: [&str; 4] = ["Undo", "Redo", "Delete", "Search"];
const LINK_TONES: [LinkTone; 4] = [
    LinkTone::Accent,
    LinkTone::Muted,
    LinkTone::Danger,
    LinkTone::Success,
];
const LINK_TONE_NAMES: &[&str] = &["Accent", "Muted", "Danger", "Success"];
const TIP_SIDES: [iced::widget::tooltip::Position; 4] = [
    iced::widget::tooltip::Position::Bottom,
    iced::widget::tooltip::Position::Top,
    iced::widget::tooltip::Position::Left,
    iced::widget::tooltip::Position::Right,
];
const TIP_SIDE_NAMES: &[&str] = &["Bottom", "Top", "Left", "Right"];

impl Gallery {
    pub(crate) fn buttons_page(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        column![
            self.button_specimen(),
            self.button_kinds_specimen(),
            self.button_group_specimen(),
            {
                let label = k.text("chip.label", "Unread");
                specimen(
                    "enz_filter_chip",
                    "A toggle that filters a list; it shows whether it is on.",
                    row(["Unread", "Starred", "Attachments"]
                        .into_iter()
                        .enumerate()
                        .map(|(index, name)| {
                            let name = if index == 0 { label } else { name };
                            enzui_widgets::enz_filter_chip(
                                name,
                                self.chips[index],
                                Message::Chip(index),
                            )
                            .into_element()
                        }))
                    .spacing(8),
                )
                .controls([k.text_control("chip.label", "Label", "Unread")])
                .code(
                    &code("enzui_widgets", "enz_filter_chip")
                        .arg(quoted(label))
                        .arg("self.unread_only")
                        .arg("Message::ToggleUnread"),
                )
                .copied(self.copied)
                .into_element()
            },
            {
                let icon = k.pick("toolbar.icon", 0);
                let variant = k.pick("toolbar.variant", 0);
                let active = k.flag("toolbar.active", false);
                let enabled = k.flag("toolbar.enabled", true);
                let mut tool = enzui_widgets::enz_toolbar_icon(
                    TOOLBAR_ICONS[icon],
                    Message::Clicked(TOOLBAR_ICONS[icon]),
                )
                .variant(if variant == 0 {
                    ToolbarIconVariant::Standard
                } else {
                    ToolbarIconVariant::InkOnly
                })
                .enabled(enabled);
                if active {
                    tool = tool.active(true);
                }
                let tool = tool.label(TOOLBAR_LABELS[icon]);
                specimen(
                    "enz_toolbar_icon",
                    "An icon button for a toolbar or titlebar. Its label is required: it names the button for screen readers and shows as its tooltip.",
                    tool.into_element(),
                )
                .controls([
                    k.pick_control("toolbar.icon", "Icon", TOOLBAR_ICONS, 0),
                    k.pick_control("toolbar.variant", "Variant", &["Standard", "InkOnly"], 0),
                    k.flag_control("toolbar.active", "Active", false),
                    k.flag_control("toolbar.enabled", "Enabled", true),
                ])
                .code(
                    &code("enzui_widgets", "enz_toolbar_icon")
                        .arg(quoted(TOOLBAR_ICONS[icon]))
                        .arg(format!("Message::{}", TOOLBAR_LABELS[icon]))
                        .set("label", quoted(TOOLBAR_LABELS[icon]))
                        .variant(variant == 1, "variant", "enzui_widgets", "ToolbarIconVariant", "InkOnly")
                        .set_if(active, "active", "true")
                        .set_if(!enabled, "enabled", "false"),
                )
                .copied(self.copied)
                .into_element()
            },
            {
                let keys = k.text("keys.text", "Ctrl+Shift+Enter");
                let symbols = k.flag("keys.symbols", true);
                let parts: Vec<&str> = keys
                    .split('+')
                    .map(str::trim)
                    .filter(|key| !key.is_empty())
                    .collect();
                specimen(
                    "enz_key_badge",
                    "A keyboard key, for shortcut hints, one badge per key. Keys with a well-known symbol (Shift, Enter, Tab, Backspace, Space, arrows, Command, Option) draw it; screen readers still hear the name.",
                    row(parts.iter().map(|key| {
                        enzui_widgets::enz_key_badge(*key)
                            .symbol(symbols)
                            .into_element()
                    }))
                    .spacing(4),
                )
                .controls([
                    k.text_control("keys.text", "Shortcut", "Ctrl+Shift+Enter"),
                    k.flag_control("keys.symbols", "Symbols", true),
                ])
                .code_text(format!(
                    "use enzui_widgets::enz_key_badge;\n\nrow([{}].map(|key| enz_key_badge(key){}.into_element()))\n    .spacing(4)",
                    parts.iter().map(|key| quoted(key)).collect::<Vec<_>>().join(", "),
                    if symbols { "" } else { ".symbol(false)" }
                ))
                .copied(self.copied)
                .into_element()
            },
            {
                let tip = k.text("tooltip.text", "Saves a copy");
                let side = k.pick("tooltip.side", 0);
                specimen(
                    "enz_tooltip",
                    "Wraps any widget with a hint that fades in on hover. Hover the button.",
                    enzui_widgets::enz_tooltip(
                        enz_button("Hover me", Message::Clicked("tooltip")).into_element(),
                        tip,
                        TIP_SIDES[side],
                    ),
                )
                .controls([
                    k.text_control("tooltip.text", "Tip", "Saves a copy"),
                    k.pick_control("tooltip.side", "Position", TIP_SIDE_NAMES, 0),
                ])
                .code(
                    &code("enzui_widgets", "enz_tooltip")
                        .import("iced::widget::tooltip::Position")
                        .arg("enz_button(\"Hover me\", Message::Save).into_element()")
                        .arg(quoted(tip))
                        .arg(format!("Position::{}", TIP_SIDE_NAMES[side]))
                        .end(None),
                )
                .copied(self.copied)
                .into_element()
            },
            {
                let text = k.text("link.text", "iced on the web");
                let tone = k.pick("link.tone", 0);
                let external = k.flag("link.external", false);
                specimen(
                    "enz_link",
                    "Inline text that opens a URL or runs an action.",
                    enzui_widgets::enz_link(text, "https://iced.rs", |_| Message::Clicked("link"))
                        .tone(LINK_TONES[tone])
                        .external(external)
                        .into_element(),
                )
                .controls([
                    k.text_control("link.text", "Text", "iced on the web"),
                    k.pick_control("link.tone", "Tone", LINK_TONE_NAMES, 0),
                    k.flag_control("link.external", "External mark", false),
                ])
                .code(
                    &code("enzui_widgets", "enz_link")
                        .arg(quoted(text))
                        .arg(quoted("https://iced.rs"))
                        .arg("Message::OpenUrl")
                        .variant(tone != 0, "tone", "enzui_widgets", "LinkTone", LINK_TONE_NAMES[tone])
                        .set_if(external, "external", "true"),
                )
                .copied(self.copied)
                .into_element()
            },
            enz_text(self.last_action.clone()).into_element(),
        ]
        .spacing(32)
        .into()
    }

    fn button_kinds_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let size = k.pick("kinds.size", 2);
        let icons = k.flag("kinds.icons", false);
        let buttons = KINDS.iter().zip(KIND_NAMES).map(|(kind, name)| {
            let mut button = enz_button(*name, Message::Clicked(name))
                .kind(*kind)
                .size(SIZES[size]);
            if icons {
                button = button.icon("save");
            }
            button.into_element()
        });
        specimen(
            "ButtonKind",
            "Every kind side by side: the filled kinds lead, the outlined and flat ones step back, and the danger ones say what they do.",
            row(buttons).spacing(10).wrap().vertical_spacing(10),
        )
        .controls([
            k.pick_control("kinds.size", "Size", SIZE_NAMES, 2),
            k.flag_control("kinds.icons", "Icons", false),
        ])
        .code_text(
            "use enzui_widgets::{ButtonKind, enz_button};\n\n\
             enz_button(\"Save\", Message::Save).kind(ButtonKind::Accent).into_element()",
        )
        .into_element()
    }

    fn button_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let label = k.text("button.label", "Save");
        let kind = k.pick("button.kind", 0);
        let size = k.pick("button.size", 2);
        let icon = k.pick("button.icon", 0);
        let side = k.pick("button.icon_side", 0);
        let enabled = k.flag("button.enabled", true);
        let fill = k.flag("button.fill", false);
        let elevation = k.pick("button.elevation", 0);
        let mut button = enz_button(label, Message::Clicked("button"))
            .kind(KINDS[kind])
            .size(SIZES[size])
            .enabled(enabled)
            .fill_width(fill);
        if elevation > 0 {
            button = button.elevation(elevation as u8 - 1);
        }
        if icon != 0 {
            button = button.icon(ICONS[icon]).icon_position(ICON_SIDES[side]);
        }
        let mut controls = vec![
            k.text_control("button.label", "Label", "Save"),
            k.pick_control("button.kind", "Kind", KIND_NAMES, 0),
            k.pick_control("button.size", "Size", SIZE_NAMES, 2),
            k.pick_control("button.icon", "Icon", ICONS, 0),
        ];
        if icon != 0 {
            controls.push(k.pick_control("button.icon_side", "Icon position", ICON_SIDE_NAMES, 0));
        }
        controls.push(k.pick_control(
            "button.elevation",
            "Elevation",
            &["Theme", "0", "1", "2"],
            0,
        ));
        controls.push(k.flag_control("button.enabled", "Enabled", true));
        controls.push(k.flag_control("button.fill", "Fill width", false));
        specimen(
            "enz_button",
            "Runs an action. The kind says how much it matters; the size follows the surrounding controls. Buttons are flat, set apart by their fill; the theme's elevation.button level (0 by default) lifts the filled kinds with a faint shadow.",
            button.into_element(),
        )
        .controls(controls)
        .code(
            &code("enzui_widgets", "enz_button")
                .arg(quoted(label))
                .arg("Message::Save")
                .variant(kind != 0, "kind", "enzui_widgets", "ButtonKind", KIND_NAMES[kind])
                .variant(size != 2, "size", "enzui_widgets", "ButtonSize", SIZE_NAMES[size])
                .set_if(icon != 0, "icon", quoted(ICONS[icon]))
                .variant(
                    icon != 0 && side != 0,
                    "icon_position",
                    "enzui_widgets::button",
                    "ButtonIconPosition",
                    ICON_SIDE_NAMES[side],
                )
                .set_if(elevation > 0, "elevation", (elevation.saturating_sub(1)).to_string())
                .set_if(!enabled, "enabled", "false")
                .set_if(fill, "fill_width", "true"),
        )
        .copied(self.copied)
        .into_element()
    }

    fn button_group_specimen(&self) -> Element<'_, Message> {
        let k = &self.knobs;
        let icons = k.flag("group.icons", true);
        let icon_only = k.flag("group.icon_only", false);
        let size = k.pick("group.size", 1);
        let group_style = k.pick("group.style", 0);
        let collapse = k.flag("group.collapse", false);
        let dividers = k.flag("group.dividers", true);
        let item = |id: &'static str, label: &'static str, icon: &'static str| {
            let item = enzui_widgets::EnzButtonGroupItem::new(id, label);
            if icons { item.icon(icon) } else { item }
        };
        let preview = enzui_widgets::enz_button_group(
            vec![
                item("left", "Left", "align-left"),
                item("center", "Center", "align-center"),
                item("right", "Right", "align-right"),
            ],
            Message::Align,
        )
        .icon_only(icons && icon_only)
        .size(FIELD_SIZES[size])
        .selected(Some(self.align))
        .collapse_inactive_labels(icons && collapse)
        .dividers(dividers);
        let preview = match group_style {
            1 => preview.style(enzui_widgets::ButtonGroupStyle::Tonal),
            2 => preview.style(enzui_widgets::ButtonGroupStyle::Accent),
            _ => preview,
        }
        .into_element();
        let items = if icons {
            "vec![\n    EnzButtonGroupItem::new(\"left\", \"Left\").icon(\"align-left\"),\n    EnzButtonGroupItem::new(\"center\", \"Center\").icon(\"align-center\"),\n    EnzButtonGroupItem::new(\"right\", \"Right\").icon(\"align-right\"),\n]"
        } else {
            "vec![\n    EnzButtonGroupItem::new(\"left\", \"Left\"),\n    EnzButtonGroupItem::new(\"center\", \"Center\"),\n    EnzButtonGroupItem::new(\"right\", \"Right\"),\n]"
        };
        specimen(
            "enz_button_group",
            "A segmented control: one choice out of a few, side by side. A single choice is a thumb inside a track; a set of toggles is tinted.",
            preview,
        )
        .controls([
            k.flag_control("group.icons", "Icons", true),
            k.flag_control("group.icon_only", "Icon only", false),
            k.pick_control("group.size", "Size", FIELD_SIZE_NAMES, 1),
            k.pick_control("group.style", "Style", &["Thumb", "Tonal", "Accent"], 0),
            k.flag_control("group.collapse", "Label on selected only", false),
            k.flag_control("group.dividers", "Dividers", true),
        ])
        .code(
            &code("enzui_widgets", "enz_button_group")
                .import("enzui_widgets::EnzButtonGroupItem")
                .arg(items)
                .arg("Message::Align")
                .set_if(icons && icon_only, "icon_only", "true")
                .variant(
                    size != 1,
                    "size",
                    "enzui_widgets",
                    "TextFieldSize",
                    FIELD_SIZE_NAMES[size],
                )
                .set("selected", "Some(self.align)")
                .set_if(icons && collapse, "collapse_inactive_labels", "true")
                .set_if(!dividers, "dividers", "false")
                .variant(
                    group_style != 0,
                    "style",
                    "enzui_widgets",
                    "ButtonGroupStyle",
                    ["Thumb", "Tonal", "Accent"][group_style],
                ),
        )
        .copied(self.copied)
        .into_element()
    }
}
