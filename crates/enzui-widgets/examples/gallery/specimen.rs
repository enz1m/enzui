//! One widget on show: its name, a live preview, controls for some of its
//! attributes, and the code that builds the preview as those controls set it.
//!
//! Each page builds a [`Specimen`] per widget. The controls write to
//! [`Knobs`], a store keyed by name, so a page adds a control without adding a
//! message; the preview and the code read the same values, so the code always
//! shows what is on screen.

use crate::Message;
use enzui_theme::FontRole;
use enzui_widgets::text::{TextSize, enz_text};
use enzui_widgets::text_field::TextFieldSize;
use enzui_widgets::{
    EnzButtonGroupItem, enz_button_group, enz_checkbox, enz_combo_box, enz_slider,
};
use enzui_widgets::{enz_text_field, text::TextTone};
use iced::widget::{column, container, row};
use iced::{Element, Length};
use std::collections::HashMap;

/// A control's value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Knob {
    Flag(bool),
    Pick(usize),
    Number(f32),
    Text(String),
}

/// Every control's value, by key; a key nobody has set reads its default.
#[derive(Debug, Default)]
pub(crate) struct Knobs(HashMap<&'static str, Knob>);

impl Knobs {
    pub(crate) fn set(&mut self, key: &'static str, value: Knob) {
        let _ = self.0.insert(key, value);
    }

    /// Back to the default the control reads.
    pub(crate) fn forget(&mut self, key: &str) {
        let _ = self.0.remove(key);
    }

    pub(crate) fn flag(&self, key: &'static str, default: bool) -> bool {
        match self.0.get(key) {
            Some(Knob::Flag(on)) => *on,
            _ => default,
        }
    }

    pub(crate) fn pick(&self, key: &'static str, default: usize) -> usize {
        match self.0.get(key) {
            Some(Knob::Pick(index)) => *index,
            _ => default,
        }
    }

    pub(crate) fn number(&self, key: &'static str, default: f32) -> f32 {
        match self.0.get(key) {
            Some(Knob::Number(value)) => *value,
            _ => default,
        }
    }

    pub(crate) fn text(&self, key: &'static str, default: &'static str) -> &str {
        match self.0.get(key) {
            Some(Knob::Text(text)) => text,
            _ => default,
        }
    }
}

/// A labelled control for one knob.
fn labelled<'a>(label: &'static str, control: Element<'a, Message>) -> Element<'a, Message> {
    column![
        enz_text(label)
            .size(TextSize::Caption)
            .tone(TextTone::Muted)
            .into_element(),
        control,
    ]
    .spacing(6)
    .into()
}

impl Knobs {
    /// A checkbox for an on/off attribute.
    pub(crate) fn flag_control(
        &self,
        key: &'static str,
        label: &'static str,
        default: bool,
    ) -> Element<'static, Message> {
        enz_checkbox(self.flag(key, default), label, move |on| {
            Message::Knob(key, Knob::Flag(on))
        })
        .into_element()
    }

    /// A segmented control for a few choices, a dropdown for more.
    pub(crate) fn pick_control(
        &self,
        key: &'static str,
        label: &'static str,
        options: &'static [&'static str],
        default: usize,
    ) -> Element<'static, Message> {
        let index = self.pick(key, default).min(options.len().saturating_sub(1));
        if options.len() <= 4 {
            let control = enz_button_group(
                options
                    .iter()
                    .map(|option| EnzButtonGroupItem::new(*option, *option))
                    .collect(),
                move |index| Message::Knob(key, Knob::Pick(index)),
            )
            .size(TextFieldSize::Sm)
            .selected(Some(index))
            .into_element();
            labelled(label, control)
        } else {
            // The dropdown carries its own label.
            enz_combo_box(
                label,
                options.iter().map(|option| (*option).to_owned()).collect(),
                Some(options[index].to_owned()),
                move |chosen| {
                    Message::Knob(
                        key,
                        Knob::Pick(options.iter().position(|o| *o == chosen).unwrap_or(0)),
                    )
                },
            )
            .size(TextFieldSize::Sm)
            .width(180)
            .into_element()
        }
    }

    /// A slider for a number, with its value beside it.
    pub(crate) fn number_control(
        &self,
        key: &'static str,
        label: &'static str,
        range: std::ops::RangeInclusive<f32>,
        step: f32,
        default: f32,
    ) -> Element<'static, Message> {
        let value = self.number(key, default);
        labelled(
            label,
            row![
                container(
                    enz_slider(range, value, move |value| Message::Knob(
                        key,
                        Knob::Number(value)
                    ))
                    .step(step)
                    .into_element()
                )
                .width(160),
                enz_text(if step.fract() == 0.0 {
                    format!("{value:.0}")
                } else {
                    number(value)
                })
                .size(TextSize::Caption)
                .tabular_figures(true)
                .into_element(),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center)
            .into(),
        )
    }

    /// A text field for a string attribute.
    pub(crate) fn text_control<'a>(
        &'a self,
        key: &'static str,
        label: &'static str,
        default: &'static str,
    ) -> Element<'a, Message> {
        container(
            enz_text_field(label, default, self.text(key, default), move |text| {
                Message::Knob(key, Knob::Text(text))
            })
            .size(TextFieldSize::Sm)
            .into_element(),
        )
        .width(200)
        .into()
    }
}

/// `a` moved `amount` of the way to `b`.
fn mix(a: iced::Color, b: iced::Color, amount: f32) -> iced::Color {
    iced::Color {
        r: a.r + (b.r - a.r) * amount,
        g: a.g + (b.g - a.g) * amount,
        b: a.b + (b.b - a.b) * amount,
        a: a.a + (b.a - a.a) * amount,
    }
}

/// A number as code: no trailing zeros, always a float literal.
pub(crate) fn number(value: f32) -> String {
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0');
    if text.ends_with('.') {
        format!("{text}0")
    } else {
        text.to_owned()
    }
}

/// A string as a Rust literal.
pub(crate) fn quoted(text: &str) -> String {
    format!("{text:?}")
}

/// Builds the code shown under a specimen: imports, a constructor call and
/// its builder methods.
#[derive(Debug, Clone)]
pub(crate) struct Code {
    imports: Vec<String>,
    call: String,
    args: Vec<String>,
    methods: Vec<(String, String)>,
    end: Option<&'static str>,
}

/// Starts the code for `function(args..)`, imported from `path`.
pub(crate) fn code(path: &str, function: &str) -> Code {
    Code {
        imports: vec![format!("{path}::{function}")],
        call: function.to_owned(),
        args: Vec::new(),
        methods: Vec::new(),
        end: Some("into_element"),
    }
}

impl Code {
    /// An argument of the constructor, as code.
    pub(crate) fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Another `use` the code needs.
    pub(crate) fn import(mut self, path: impl Into<String>) -> Self {
        self.imports.push(path.into());
        self
    }

    /// Another `use`, when `needed`.
    pub(crate) fn import_if(self, needed: bool, path: impl Into<String>) -> Self {
        if needed { self.import(path) } else { self }
    }

    /// A builder method, always shown.
    pub(crate) fn set(mut self, method: &str, value: impl Into<String>) -> Self {
        self.methods.push((method.to_owned(), value.into()));
        self
    }

    /// A builder method shown only when `shown`: attributes left at their
    /// default stay out of the code, as they would in an app.
    pub(crate) fn set_if(self, shown: bool, method: &str, value: impl Into<String>) -> Self {
        if shown { self.set(method, value) } else { self }
    }

    /// An enum-valued method, shown when not the default, with the enum's
    /// import.
    pub(crate) fn variant(
        self,
        shown: bool,
        method: &str,
        path: &str,
        ty: &str,
        variant: &str,
    ) -> Self {
        if shown {
            self.import(format!("{path}::{ty}"))
                .set(method, format!("{ty}::{variant}"))
        } else {
            self
        }
    }

    /// Ends with a different call than `.into_element()`, or none.
    pub(crate) fn end(mut self, end: Option<&'static str>) -> Self {
        self.end = end;
        self
    }

    pub(crate) fn render(&self) -> String {
        let mut out = String::new();
        for line in grouped_imports(&self.imports) {
            out.push_str(&line);
            out.push('\n');
        }
        out.push('\n');
        let one_line = format!("{}({})", self.call, self.args.join(", "));
        if one_line.len() <= 72 && !one_line.contains('\n') {
            out.push_str(&one_line);
        } else {
            out.push_str(&self.call);
            out.push_str("(\n");
            for arg in &self.args {
                out.push_str(&indent(arg, 1));
                out.push_str(",\n");
            }
            out.push(')');
        }
        for (method, value) in &self.methods {
            out.push('\n');
            out.push_str(&indent(&format!(".{method}({value})"), 1));
        }
        if let Some(end) = self.end {
            out.push('\n');
            out.push_str(&indent(&format!(".{end}()"), 1));
        }
        out
    }
}

/// `use` lines, one per module, names sorted.
fn grouped_imports(paths: &[String]) -> Vec<String> {
    let mut modules: Vec<(String, Vec<String>)> = Vec::new();
    for path in paths {
        let (module, name) = path.rsplit_once("::").unwrap_or(("", path));
        match modules.iter_mut().find(|(m, _)| m == module) {
            Some((_, names)) => {
                if !names.iter().any(|n| n == name) {
                    names.push(name.to_owned());
                }
            }
            None => modules.push((module.to_owned(), vec![name.to_owned()])),
        }
    }
    modules.sort();
    modules
        .into_iter()
        .map(|(module, mut names)| {
            names.sort();
            if names.len() == 1 {
                format!("use {module}::{};", names[0])
            } else {
                format!("use {module}::{{{}}};", names.join(", "))
            }
        })
        .collect()
}

fn indent(text: &str, depth: usize) -> String {
    let pad = "    ".repeat(depth);
    text.lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{pad}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The card's corner radius.
const CORNER: f32 = 12.0;

/// One widget on show.
pub(crate) struct Specimen<'a> {
    name: &'static str,
    about: &'static str,
    preview: Element<'a, Message>,
    controls: Vec<Element<'a, Message>>,
    code: String,
    copied: bool,
}

/// A specimen of the widget built by `name`, described by `about`.
pub(crate) fn specimen<'a>(
    name: &'static str,
    about: &'static str,
    preview: impl Into<Element<'a, Message>>,
) -> Specimen<'a> {
    Specimen {
        name,
        about,
        preview: preview.into(),
        controls: Vec::new(),
        code: String::new(),
        copied: false,
    }
}

impl crate::Gallery {
    /// A specimen that knows whether its code was the last copied.
    pub(crate) fn specimen<'a>(
        &self,
        name: &'static str,
        about: &'static str,
        preview: impl Into<Element<'a, Message>>,
    ) -> Specimen<'a> {
        specimen(name, about, preview).copied(self.copied)
    }
}

/// The three sizes most controls take.
pub(crate) const SIZE_NAMES: &[&str] = &["Sm", "Md", "Lg"];

impl<'a> Specimen<'a> {
    /// The controls for its attributes.
    pub(crate) fn controls(
        mut self,
        controls: impl IntoIterator<Item = Element<'a, Message>>,
    ) -> Self {
        self.controls.extend(controls);
        self
    }

    /// The code that builds the preview.
    pub(crate) fn code(mut self, code: &Code) -> Self {
        self.code = code.render();
        self
    }

    /// Code written out by hand, for a widget whose call does not fit
    /// [`Code`].
    pub(crate) fn code_text(mut self, code: impl Into<String>) -> Self {
        self.code = code.into();
        self
    }

    pub(crate) fn into_element(self) -> Element<'a, Message> {
        self.into()
    }

    /// Whether this specimen's code was the last copied.
    pub(crate) fn copied(mut self, last_copied: Option<&str>) -> Self {
        self.copied = last_copied == Some(self.name);
        self
    }
}

/// Whether `name` is shown: `ENZUI_GALLERY_ONLY=enz_button,enz_slider`
/// shows only those specimens, for reviewing a few widgets at once. A
/// specimen named after several widgets is matched by its first.
fn shown(name: &str) -> bool {
    let first = name.split_whitespace().next().unwrap_or(name);
    std::env::var("ENZUI_GALLERY_ONLY").map_or(true, |only| {
        only.split(',')
            .any(|wanted| wanted.trim() == name || wanted.trim() == first)
    })
}

impl<'a> From<Specimen<'a>> for Element<'a, Message> {
    fn from(specimen: Specimen<'a>) -> Self {
        let name = specimen.name;
        if !shown(name) {
            return iced::widget::Space::new().into();
        }
        let heading = column![
            enz_text(name)
                .size(TextSize::Headline)
                .font_role(FontRole::Monospace)
                .into_element(),
            enz_text(specimen.about)
                .tone(TextTone::Muted)
                .into_element(),
        ]
        .spacing(2);
        // The stage rounds the card's top corners, or all four when no
        // controls sit under it.
        let radius = if specimen.controls.is_empty() {
            iced::border::Radius::new(CORNER)
        } else {
            iced::border::Radius::new(0.0).top(CORNER)
        };
        let stage = container(specimen.preview)
            .padding(24)
            .width(Length::Fill)
            .style(move |theme: &iced::Theme| {
                let palette = enzui_theme::Palette::from_iced(theme);
                container::Style {
                    // Close to the window's own background, so widgets look
                    // as they do in an app, lifted just enough to set the
                    // card apart from the page.
                    background: Some(mix(palette.background, palette.surface, 0.45).into()),
                    border: iced::Border {
                        radius,
                        ..Default::default()
                    },
                    ..Default::default()
                }
            });
        let mut card = column![stage];
        if !specimen.controls.is_empty() {
            card = card.push(
                container(
                    row(specimen.controls)
                        .spacing(20)
                        .align_y(iced::Alignment::End)
                        .wrap()
                        .vertical_spacing(14),
                )
                .padding([14, 16])
                .width(Length::Fill),
            );
        }
        let card = container(card)
            .clip(true)
            .width(Length::Fill)
            .style(|theme: &iced::Theme| {
                let palette = enzui_theme::Palette::from_iced(theme);
                container::Style {
                    background: Some(palette.surface.into()),
                    border: iced::Border {
                        radius: CORNER.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            });
        let mut body = column![heading, card].spacing(10);
        if !specimen.code.is_empty() {
            body = body.push(
                enzui_widgets::code_block::enz_code_block(specimen.code)
                    .language("rust")
                    .copied(specimen.copied)
                    .on_copy_requested(move |code| Message::CopyCode(name, code))
                    .into_element(),
            );
        }
        body.into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_shows_only_changed_attributes_with_their_imports() {
        let plain = code("enzui_widgets", "enz_button")
            .arg(quoted("Save"))
            .arg("Message::Save")
            .variant(
                false,
                "kind",
                "enzui_widgets::button",
                "ButtonKind",
                "Primary",
            )
            .set_if(false, "enabled", "true")
            .import_if(false, "enzui_widgets::ButtonSize")
            .render();
        assert_eq!(
            plain,
            "use enzui_widgets::enz_button;\n\nenz_button(\"Save\", Message::Save)\n    .into_element()"
        );
        let accent = code("enzui_widgets", "enz_button")
            .arg(quoted("Save"))
            .arg("Message::Save")
            .variant(
                true,
                "kind",
                "enzui_widgets::button",
                "ButtonKind",
                "Accent",
            )
            .set_if(true, "enabled", "false")
            .render();
        assert_eq!(
            accent,
            "use enzui_widgets::enz_button;\nuse enzui_widgets::button::ButtonKind;\n\n\
             enz_button(\"Save\", Message::Save)\n    .kind(ButtonKind::Accent)\n    \
             .enabled(false)\n    .into_element()"
        );
    }
}
