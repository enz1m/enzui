//! Editable shortcut settings backed by a host-owned resolved catalog.
//!
//! Capture and reset messages request host updates; the pane does not commit
//! changes itself. Use the same catalog’s `resolved()` table for dispatch and
//! read-only help. Feedback marks a matching row invalid and publishes an Alert;
//! catalog save errors publish a separate Alert.
//!
//! Add [`editable_shortcuts_reset_all_action`] through the settings dialog’s
//! action slot to keep it reachable while rows scroll. Catalog action IDs form
//! capture-control namespaces; panes sharing action IDs reuse those identities.

use std::rc::Rc;

use enzui_core::EditableShortcutCatalog;
use enzui_core::live_region::Announcement;
use iced::widget::{column, container, row};
use iced::{Alignment, Element, Length};

use crate::button::{ButtonKind, ButtonSize, enz_button_with};
use crate::enz_text::{TextTone, enz_text};
use crate::key_capture_chip::{KeyCapture, enz_key_capture_chip};
use crate::preferences_group::enz_preferences_group;
use crate::preferences_row::enz_preferences_row;
use crate::semantics_container::SemanticsContainer;
use crate::settings_dialog::{SettingsAction, SettingsPage, settings_action};
fn alert_notice<'a, Message: 'a>(message: String, padding: iced::Padding) -> Element<'a, Message> {
    // The alert region owns the spoken name; suppress its text child’s semantics.
    let visible = container(
        enz_text(message.clone())
            .tone(TextTone::Danger)
            .announce(false),
    )
    .padding(padding);
    SemanticsContainer::leaf(visible, Announcement::Alert.region(message)).into()
}

/// Messages produced by the shared editable-shortcuts pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditableShortcutsEvent {
    /// A report from one row's shared key-capture chip.
    Capture {
        /// Stable catalog action id.
        action: String,
        /// Capture report; a candidate is not yet a commit.
        event: KeyCapture,
    },
    /// Restore one action's app-supplied default.
    Reset {
        /// Stable catalog action id.
        action: String,
    },
    /// Restore every app-supplied default.
    ResetAll,
}

/// Builder for the SettingsPage-compatible shortcut pane.
pub struct EnzEditableShortcuts<'a, Message, Command, Context> {
    catalog: &'a EditableShortcutCatalog<Command, Context>,
    armed_action: Option<&'a str>,
    feedback: Option<(&'a str, &'a str)>,
    on_event: Rc<dyn Fn(EditableShortcutsEvent) -> Message + 'a>,
    heading: bool,
}

/// Build an editable shortcut pane from the application's authoritative model.
///
/// `armed_action` is the stable id currently recording in this window. Every
/// capture control is named `"<action name> shortcut"`, has a stable id
/// namespace derived from the action id, and remains an ordinary keyboard/AT
/// reachable [`key_capture_chip`](crate::key_capture_chip).
pub fn enz_editable_shortcuts<'a, Message, Command, Context>(
    catalog: &'a EditableShortcutCatalog<Command, Context>,
    armed_action: Option<&'a str>,
    on_event: impl Fn(EditableShortcutsEvent) -> Message + 'a,
) -> EnzEditableShortcuts<'a, Message, Command, Context> {
    EnzEditableShortcuts {
        catalog,
        armed_action,
        feedback: None,
        on_event: Rc::new(on_event),
        heading: true,
    }
}

impl<'a, Message: Clone + 'a, Command: Clone, Context>
    EnzEditableShortcuts<'a, Message, Command, Context>
{
    /// Mark the matching action’s chip invalid and show its reason as an Alert.
    /// Unknown action IDs show no candidate feedback. None clears that feedback;
    /// catalog save errors are independent and remain visible.
    #[must_use]
    pub fn feedback(mut self, feedback: Option<(&'a str, &'a str)>) -> Self {
        self.feedback = feedback;
        self
    }

    /// Draw the "Keyboard shortcuts" title and description above the rows;
    /// defaults to true. Pass false when the page already has its own heading
    /// (with a reset-all control, say), so the title does not appear twice.
    /// The rows keep their accessible group name either way.
    #[must_use]
    pub fn heading(mut self, show: bool) -> Self {
        self.heading = show;
        self
    }

    /// Build the pane body accepted directly by `EnzSettingsDialog::page`.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        let mut rows = column![].spacing(8).width(Length::Fill);

        for item in self.catalog.rows() {
            let action = item.id.to_owned();
            let capture_action = action.clone();
            let reset_action = action.clone();
            let on_capture = Rc::clone(&self.on_event);
            let on_reset = Rc::clone(&self.on_event);
            let capture_label =
                enzui_core::localized!("enzui-editable-shortcuts-shortcut", "action" => item.name);
            let is_invalid = self.feedback.is_some_and(|(id, _)| id == item.id);

            let capture: Element<'a, Message> = enz_key_capture_chip(capture_label, move |event| {
                on_capture(EditableShortcutsEvent::Capture {
                    action: capture_action.clone(),
                    event,
                })
            })
            .binding(Some(item.primary.clone()))
            .armed(self.armed_action == Some(item.id))
            .invalid(is_invalid)
            .id_prefix(format!("editable-shortcut-{}", item.id))
            .into();

            // An icon-only reset beside a changed shortcut, as GNOME's
            // shortcut editor does: the text button did not fit the control
            // slot next to the key chip and showed as "Res…", and a disabled
            // button on every unchanged row was noise. Unchanged rows keep
            // the same width free so the key chips stay in one column.
            let reset_slot = ButtonSize::Sm.tokens().height;
            let reset: Element<'a, Message> = if item.overridden {
                enz_button_with(String::new(), move || {
                    on_reset(EditableShortcutsEvent::Reset {
                        action: reset_action.clone(),
                    })
                })
                .icon("rotate-ccw")
                .kind(ButtonKind::Ghost)
                .size(ButtonSize::Sm)
                .tooltip(enzui_core::localization::text("enzui-editable-shortcuts-reset"))
                .aria_label(enzui_core::localized!("enzui-editable-shortcuts-reset-shortcut", "action" => item.name))
                .into()
            } else {
                iced::widget::Space::new()
                    .width(Length::Fixed(reset_slot))
                    .height(Length::Fixed(reset_slot))
                    .into()
            };

            let controls: Element<'a, Message> = row![capture, reset]
                .spacing(8)
                .align_y(Alignment::Center)
                .into();
            let row: Element<'a, Message> = enz_preferences_row(item.name)
                .subtitle(item.section)
                .control(controls)
                .into();
            let mut block = column![row].width(Length::Fill);
            if let Some((id, message)) = self.feedback
                && id == item.id
            {
                block = block.push(alert_notice(
                    message.to_owned(),
                    iced::Padding {
                        top: 0.0,
                        right: 10.0,
                        bottom: 4.0,
                        left: 10.0,
                    },
                ));
            }
            rows = rows.push(block);
        }

        if let Some(error) = self.catalog.save_error() {
            rows = rows.push(alert_notice(
                enzui_core::localized!("enzui-editable-shortcuts-could-not-save-shortcut-changes", "error" => error),
                iced::Padding::new(10.0),
            ));
        }

        let group = enz_preferences_group(rows).aria_label(enzui_core::localization::text(
            "enzui-editable-shortcuts-keyboard-shortcuts",
        ));
        if !self.heading {
            return group.into();
        }
        group
            .title(&enzui_core::localization::text(
                "enzui-editable-shortcuts-keyboard-shortcuts",
            ))
            .description(&enzui_core::localization::text(
                "enzui-editable-shortcuts-choose-a-shortcut-then-press-the-new-key-combination",
            ))
            .into()
    }
}

impl<'a, Message: Clone + 'a, Command: Clone, Context>
    From<EnzEditableShortcuts<'a, Message, Command, Context>> for Element<'a, Message>
{
    fn from(value: EnzEditableShortcuts<'a, Message, Command, Context>) -> Self {
        value.into_element()
    }
}

/// Create the Shortcuts settings page with its keyboard icon.
/// The settings dialog’s automatic navigation adapts this page to available width.
#[must_use]
pub fn shortcuts_settings_page() -> SettingsPage {
    SettingsPage::new(
        "shortcuts",
        enzui_core::localization::text("enzui-editable-shortcuts-shortcuts"),
    )
    .icon("keyboard")
}

/// Pinned Settings footer command for resetting every shortcut.
///
/// Add this with `EnzSettingsDialog::action`; unlike a button appended to the
/// pane it remains reachable while the shortcut rows scroll.
pub fn editable_shortcuts_reset_all_action<'a, Message>(
    on_reset: impl Fn() -> Message + 'a,
) -> SettingsAction<'a, Message> {
    settings_action(
        "reset-all-shortcuts",
        enzui_core::localization::text("enzui-editable-shortcuts-reset-all-shortcuts"),
        on_reset,
    )
}
