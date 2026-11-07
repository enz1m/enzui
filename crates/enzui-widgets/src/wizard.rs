//! A step-by-step assistant: the steps listed across the top, the current
//! step's page, and Cancel, Back and Next (Finish on the last step) below
//! (GTK's `GtkAssistant`, a Qt `QWizard`).
//!
//! The app owns the pages, their validation and an
//! [`enzui_core::wizard::Wizard`]; [`enz_wizard`] draws them and reports
//! [`WizardEvent`]s to apply with [`apply`].

use enzui_core::wizard::Wizard;
use iced::widget::{Space, button, column, container, row};
use iced::{Alignment, Border, Color, Element, Length};

use crate::button::{ButtonKind, enz_button};
use crate::text::{TextSize, TextTone, enz_text};

/// What the user did in a wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardEvent {
    /// Back was pressed.
    Back,
    /// Next was pressed on a step that allows it.
    Next,
    /// Finish was pressed on the last step.
    Finish,
    /// Cancel was pressed.
    Cancel,
    /// A reached step in the step list was chosen.
    GoTo(usize),
}

/// Moves `wizard` for `event`; Finish and Cancel are the app's to handle and
/// return `false`, as does a move that cannot happen.
pub fn apply(wizard: &mut Wizard, event: WizardEvent) -> bool {
    match event {
        WizardEvent::Back => wizard.back(),
        WizardEvent::Next => wizard.advance(),
        WizardEvent::GoTo(step) => wizard.go_to(step),
        WizardEvent::Finish | WizardEvent::Cancel => false,
    }
}

/// The wizard: `titles` names each step (as many as the wizard has),
/// `page` is the current step's content, and `can_advance` says whether its
/// answers allow Next or Finish.
pub fn enz_wizard<'a, Message: Clone + 'a>(
    wizard: &Wizard,
    titles: &[String],
    page: impl Into<Element<'a, Message>>,
    can_advance: bool,
    on_event: impl Fn(WizardEvent) -> Message + 'a,
) -> Element<'a, Message> {
    let current = wizard.current();
    let mut steps = row![].spacing(8).align_y(Alignment::Center);
    for (index, title) in titles.iter().enumerate() {
        if index > 0 {
            steps = steps.push(connector(index <= current));
        }
        let state = if index == current {
            StepState::Current
        } else if index < current {
            StepState::Done
        } else if wizard.is_reached(index) {
            StepState::Reached
        } else {
            StepState::Ahead
        };
        let label = enz_text(title.clone())
            .size(if state == StepState::Current {
                TextSize::Headline
            } else {
                TextSize::Body
            })
            .tone(if state == StepState::Ahead {
                TextTone::Subtle
            } else {
                TextTone::Default
            })
            .no_wrap();
        let step = row![marker(index, state), label]
            .spacing(8)
            .align_y(Alignment::Center);
        if state != StepState::Current && wizard.is_reached(index) {
            steps = steps.push(
                button(step)
                    .padding([2, 4])
                    .style(button::text)
                    .on_press(on_event(WizardEvent::GoTo(index))),
            );
        } else {
            steps = steps.push(container(step).padding([2, 4]));
        }
    }
    let progress = enz_text(enzui_core::localized!(
        "enzui-wizard-step",
        "current" => (current + 1).to_string(),
        "total" => wizard.steps().to_string()
    ))
    .size(TextSize::Caption)
    .tone(TextTone::Muted);
    let advance = if wizard.is_last() {
        enz_button(
            enzui_core::localized!("enzui-wizard-finish"),
            on_event(WizardEvent::Finish),
        )
    } else {
        enz_button(
            enzui_core::localized!("enzui-wizard-next"),
            on_event(WizardEvent::Next),
        )
    }
    .kind(ButtonKind::Accent)
    .enabled(can_advance);
    let footer = row![
        enz_button(
            enzui_core::localized!("enzui-wizard-cancel"),
            on_event(WizardEvent::Cancel)
        )
        .kind(ButtonKind::Quiet),
        Space::new().width(Length::Fill),
        enz_button(
            enzui_core::localized!("enzui-wizard-back"),
            on_event(WizardEvent::Back)
        )
        .kind(ButtonKind::Secondary)
        .enabled(wizard.can_go_back()),
        advance,
    ]
    .spacing(8)
    .align_y(Alignment::Center);
    column![
        column![steps, progress].spacing(6),
        // Another step's page fades in.
        crate::motion::fade_on_change(
            current,
            container(page.into())
                .height(Length::Fill)
                .width(Length::Fill),
        ),
        footer,
    ]
    .spacing(16)
    .into()
}

/// Diameter of a step's numbered circle.
const MARKER_SIZE: f32 = 24.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepState {
    /// Before the current step: a tinted circle with a check.
    Done,
    /// The current step: a filled accent circle with its number.
    Current,
    /// After the current step but already visited: an accent ring.
    Reached,
    /// Not reached yet: a quiet ring.
    Ahead,
}

/// A step's circle: its number, or a check once it is done.
fn marker<'a, Message: 'a>(index: usize, state: StepState) -> Element<'a, Message> {
    let content: Element<'a, Message> = if state == StepState::Done {
        crate::icon::enz_icon("check")
            .size(enzui_theme::text_scale::ui_px(14.0))
            .tint_resolved(|theme| crate::theme_lookup::palette(theme).accent)
            .into()
    } else {
        enz_text((index + 1).to_string())
            .size(TextSize::Footnote)
            .weight(iced::font::Weight::Semibold)
            .tabular_figures(true)
            .color_resolved(move |theme| {
                let p = crate::theme_lookup::palette(theme);
                match state {
                    StepState::Current => p.filled_color(p.accent).text,
                    StepState::Reached => p.accent,
                    StepState::Done | StepState::Ahead => p.text_muted,
                }
            })
            .into()
    };
    let size = enzui_theme::text_scale::ui_px(MARKER_SIZE);
    container(content)
        .center(Length::Fixed(size))
        .style(move |theme: &iced::Theme| {
            let p = crate::theme_lookup::palette(theme);
            let (background, border) = match state {
                StepState::Current => (Some(p.filled_color(p.accent).color), Color::TRANSPARENT),
                StepState::Done => (Some(p.accent_soft), Color::TRANSPARENT),
                StepState::Reached => (None, p.accent),
                StepState::Ahead => (None, p.control_border_strong),
            };
            container::Style {
                background: background.map(Into::into),
                border: Border {
                    color: border,
                    width: if border.a > 0.0 { 1.5 } else { 0.0 },
                    radius: (size / 2.0).into(),
                },
                ..container::Style::default()
            }
        })
        .into()
}

/// The rule joining two steps; accent once the step after it is reached.
fn connector<'a, Message: 'a>(passed: bool) -> Element<'a, Message> {
    container(Space::new())
        .width(Length::Fill)
        .height(2)
        .style(move |theme: &iced::Theme| {
            let p = crate::theme_lookup::palette(theme);
            container::Style {
                background: Some(if passed { p.accent } else { p.border }.into()),
                border: Border {
                    radius: 1.0.into(),
                    ..Border::default()
                },
                ..container::Style::default()
            }
        })
        .into()
}
