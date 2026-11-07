//! Three-action confirmation for saving changes before closing.
//!
//! Actions appear as Discard, Cancel, and Save in a right-aligned row.
//! Unmodified Enter selects Save; modified Enter is consumed. Escape and
//! the scrim report Cancel unless [`EnzSaveChangesDialog::insistent`] is set.
//! [`EnzSaveChangesDialog::into_body`] supports applications with a shared
//! modal host, while [`EnzSaveChangesDialog::into_host`] supplies one.

use enzui_core::dismissal::DismissPolicy;
use enzui_core::semantics::{Role, Semantics};
use iced::widget::{column, row};
use iced::{Element, Length};

/// The default fixed surface width in logical pixels.
pub const DEFAULT_SAVE_CHANGES_WIDTH: f32 = 460.0;

/// An answer from a save-changes dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveChangesAction {
    /// Persist every unsaved document, then close.
    SaveAll,
    /// Abandon the unsaved changes and close.
    Discard,
    /// Keep the window open without changing documents.
    Cancel,
}

impl SaveChangesAction {
    /// Returns the actions in leading-to-trailing visual and focus order.
    #[must_use]
    pub const fn action_order() -> [Self; 3] {
        [Self::Discard, Self::Cancel, Self::SaveAll]
    }

    /// Returns the trailing action selected by unmodified Enter.
    #[must_use]
    pub const fn keyboard_default() -> Self {
        let order = Self::action_order();
        order[order.len() - 1]
    }

    /// Returns whether the action abandons unsaved work.
    #[must_use]
    pub const fn is_destructive(self) -> bool {
        matches!(self, Self::Discard)
    }
}

/// An event emitted by a hosted save-changes dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveChangesEvent {
    /// A button (or the bare-Enter default) was activated.
    Action(SaveChangesAction),
    /// Tab inside the dialog: move focus onward, wrapping at the end.
    FocusNext,
    /// Shift+Tab inside the dialog.
    FocusPrevious,
}

impl SaveChangesEvent {
    /// Maps every host dismissal to Cancel and preserves focus movement.
    #[must_use]
    pub const fn from_host(event: crate::modal_host::ModalHostEvent) -> Self {
        use crate::modal_host::ModalHostEvent;

        match event {
            ModalHostEvent::Dismissed(_) => Self::Action(SaveChangesAction::Cancel),
            ModalHostEvent::FocusNext => Self::FocusNext,
            ModalHostEvent::FocusPrevious => Self::FocusPrevious,
        }
    }
}

/// Returns a modal dismissal policy, or an insistent policy when requested.
#[must_use]
pub const fn save_changes_dismiss_policy(insistent: bool) -> DismissPolicy {
    if insistent {
        DismissPolicy::insistent()
    } else {
        DismissPolicy::modal()
    }
}

/// A three-action save-changes dialog, hosted over an application background.
pub struct EnzSaveChangesDialog<'a, Message> {
    material: Option<enzui_theme::MaterialStyle>,
    grain: enzui_theme::GrainSpec,
    title: String,
    message: String,
    detail: Option<String>,
    save_label: String,
    discard_label: String,
    cancel_label: String,
    saving: bool,
    insistent: bool,
    width: f32,
    id_prefix: Option<String>,
    /// Writing direction of the action row; `None` follows the locale.
    direction: Option<enzui_core::placement::Direction>,
    #[cfg(test)]
    enter_route_probe: bool,
    on_event: Box<dyn Fn(SaveChangesEvent) -> Message + 'a>,
}

/// Builds a save-changes dialog reporting every action through `on_event`.
///
/// The default labels are “Save”, “Discard”, and “Cancel”. A prompt covering
/// several documents says so with [`EnzSaveChangesDialog::save_label`] and
/// [`EnzSaveChangesDialog::discard_label`] (“Save All”, “Discard All”); the
/// affirmative action is [`SaveChangesAction::SaveAll`] either way. When presented
/// with [`EnzSaveChangesDialog::into_host`], host dismissal and focus events
/// use the same callback.
pub fn enz_save_changes_dialog<'a, Message>(
    title: impl Into<String>,
    message: impl Into<String>,
    on_event: impl Fn(SaveChangesEvent) -> Message + 'a,
) -> EnzSaveChangesDialog<'a, Message> {
    EnzSaveChangesDialog {
        material: None,
        grain: enzui_theme::GrainSpec::None,
        title: title.into(),
        message: message.into(),
        detail: None,
        save_label: enzui_core::localization::text("enzui-save-changes-dialog-save"),
        discard_label: enzui_core::localization::text("enzui-save-changes-dialog-discard"),
        cancel_label: enzui_core::localization::text("enzui-save-changes-dialog-cancel"),
        saving: false,
        insistent: false,
        width: DEFAULT_SAVE_CHANGES_WIDTH,
        id_prefix: None,
        direction: None,
        #[cfg(test)]
        enter_route_probe: false,
        on_event: Box::new(on_event),
    }
}

/// `background` with `dialog` over it when there is one, mounted the same
/// way whether or not it is shown, so the page keeps its widget state (a
/// scroll offset) as the dialog comes and goes; see
/// [`host_when_shown`](crate::modal_host::host_when_shown). `on_event` is the
/// callback the dialog itself takes.
pub fn save_changes_dialog_host<'a, Message: Clone + 'a>(
    background: impl Into<Element<'a, Message>>,
    dialog: Option<EnzSaveChangesDialog<'a, Message>>,
    on_event: impl Fn(SaveChangesEvent) -> Message + 'a,
) -> Element<'a, Message> {
    crate::modal_host::host_when_shown(
        background,
        dialog,
        EnzSaveChangesDialog::into_host,
        move |event| on_event(SaveChangesEvent::from_host(event)),
    )
}

impl<'a, Message> EnzSaveChangesDialog<'a, Message> {
    /// Lay the action row out for `direction` instead of the locale's: the
    /// action row runs from the start edge, so in right-to-left text the
    /// trailing action sits at the left. `None` (the default) follows the
    /// ambient locale.
    #[must_use]
    pub const fn direction(mut self, direction: enzui_core::placement::Direction) -> Self {
        self.direction = Some(direction);
        self
    }
    /// Put the dialog on liquid glass of `style` (see [`crate::glass`]). Its
    /// buttons and fields default to glass.
    #[must_use]
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// Lay grain on the dialog surface; see [`crate::grain`].
    #[must_use]
    pub fn grain(mut self, grain: impl Into<enzui_theme::GrainSpec>) -> Self {
        self.grain = grain.into();
        self
    }

    /// Sets the detail shown between the message and actions.
    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Rename the affirmative action (default “Save”; “Save All” when the
    /// prompt covers several documents).
    #[must_use]
    pub fn save_label(mut self, label: impl Into<String>) -> Self {
        self.save_label = label.into();
        self
    }

    /// Rename the destructive (Discard) action.
    #[must_use]
    pub fn discard_label(mut self, label: impl Into<String>) -> Self {
        self.discard_label = label.into();
        self
    }

    /// Rename the safe (Cancel) action.
    #[must_use]
    pub fn cancel_label(mut self, label: impl Into<String>) -> Self {
        self.cancel_label = label.into();
        self
    }

    /// Disables the save action and changes its label to “Saving…” while true.
    ///
    /// Cancel and Discard remain enabled.
    #[must_use]
    pub const fn saving(mut self, saving: bool) -> Self {
        self.saving = saving;
        self
    }

    /// Sets the surface width in logical pixels.
    #[must_use]
    pub const fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Requires an explicit button answer when true.
    ///
    /// The default is false; Escape and the scrim then report Cancel.
    #[must_use]
    pub const fn insistent(mut self, insistent: bool) -> Self {
        self.insistent = insistent;
        self
    }

    /// Sets the prefix used for routed button ids.
    ///
    /// When unset, the prefix is derived from the title.
    #[must_use]
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = Some(prefix.into());
        self
    }

    /// Returns the explicit id prefix, or one derived from the title.
    #[must_use]
    pub fn effective_id_prefix(&self) -> String {
        self.id_prefix
            .clone()
            .unwrap_or_else(|| format!("enz-save-changes-{}", self.title))
    }

    /// Returns the configured dismissal policy.
    #[must_use]
    pub const fn policy(&self) -> DismissPolicy {
        save_changes_dismiss_policy(self.insistent)
    }

    /// Returns modal semantics named and described by the prompt.
    ///
    /// Dismissible prompts use `Dialog`; insistent prompts use `AlertDialog`.
    pub fn semantics(&self) -> Semantics {
        let role = if self.insistent {
            Role::AlertDialog
        } else {
            Role::Dialog
        };
        Semantics::new(role)
            .label(self.title.as_str())
            .description(self.message.as_str())
            .modal(true)
    }

    /// Returns the visual kind for an action.
    #[must_use]
    pub const fn button_kind(action: SaveChangesAction) -> crate::button::ButtonKind {
        match action {
            SaveChangesAction::SaveAll => crate::button::ButtonKind::Primary,
            SaveChangesAction::Discard => crate::button::ButtonKind::Destructive,
            // The way out is neutral text, not accent-inked like a link.
            SaveChangesAction::Cancel => crate::button::ButtonKind::Quiet,
        }
    }
}

impl<'a, Message: Clone + 'a> EnzSaveChangesDialog<'a, Message> {
    /// Builds a standalone surface without a modal host.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        self.into_surface()
    }

    /// Builds a surface with its Dialog or AlertDialog semantics node.
    #[must_use]
    pub fn into_surface(self) -> Element<'a, Message> {
        let semantics = self.semantics();
        Element::from(crate::semantics_container::SemanticsContainer::new(
            self.into_body(),
            semantics,
        ))
    }

    /// Builds the body without a dialog semantics node.
    ///
    /// Use this inside an [`EnzModalHost`](crate::modal_host::EnzModalHost),
    /// which publishes the dialog node.
    #[must_use]
    pub fn into_body(self) -> Element<'a, Message> {
        let saving = self.saving;
        #[cfg(test)]
        let enter_route_probe = self.enter_route_probe;
        let id_prefix = self.effective_id_prefix();
        let on_event: std::rc::Rc<dyn Fn(SaveChangesEvent) -> Message + 'a> =
            std::rc::Rc::from(self.on_event);

        let mut body = column![
            iced::widget::text(self.title).size(enzui_theme::text_scale::ui_px(18.0)),
            iced::widget::text(self.message).size(enzui_theme::text_scale::ui_px(13.0)),
        ]
        .spacing(10);
        if let Some(detail) = self.detail {
            body = body.push(iced::widget::text(detail).size(enzui_theme::text_scale::ui_px(13.0)));
        }

        #[cfg(test)]
        if enter_route_probe {
            body = body.push(
                crate::focus_button::focus_button_maybe::<Message, iced::Theme, iced::Renderer>(
                    iced::widget::text("focus target"),
                    None,
                )
                .focusable_without_activation(true)
                .id(iced::advanced::widget::Id::new(
                    "save-changes-enter-route-probe",
                )),
            );
        }

        let mut actions = row![iced::widget::Space::new().width(Length::Fill)].spacing(10);
        for action in SaveChangesAction::action_order() {
            let (label, enabled) = match action {
                SaveChangesAction::SaveAll if saving => (
                    enzui_core::localization::text("enzui-save-changes-dialog-saving"),
                    false,
                ),
                SaveChangesAction::SaveAll => (self.save_label.clone(), true),
                SaveChangesAction::Discard => (self.discard_label.clone(), true),
                SaveChangesAction::Cancel => (self.cancel_label.clone(), true),
            };
            let on_event = std::rc::Rc::clone(&on_event);
            let id = enzui_core::identity::derive_id(&id_prefix, action_id_suffix(action));
            actions = actions.push(
                crate::button::enz_button_with(label, move || {
                    on_event(SaveChangesEvent::Action(action))
                })
                .kind(Self::button_kind(action))
                .enabled(enabled)
                .id(id)
                .into_element(),
            );
        }
        body = body.push(actions.mirrored(
            enzui_core::placement::resolve_direction(self.direction)
                == enzui_core::placement::Direction::RightToLeft,
        ));

        // Shift+Enter activates a focused button unless intercepted before its child.
        let guarded = crate::keyboard_capture::keyboard_precapture(body, |key, modifiers| {
            is_enter(key) && has_modifier(modifiers)
        });

        // Unscoped: the dialog owns the window, so Enter resolves even when a
        // click on its text left nothing focused.
        let enter_event = std::rc::Rc::clone(&on_event);
        let width = self.width;
        let captured =
            crate::keyboard_capture::keyboard_capture_unscoped(guarded, move |key, modifiers| {
                if saving || !is_enter(key) || has_modifier(modifiers) {
                    return None;
                }
                Some(enter_event(SaveChangesEvent::Action(
                    SaveChangesAction::keyboard_default(),
                )))
            });

        crate::modal_surface::enz_modal_surface(captured)
            .material_opt(self.material)
            .grain(self.grain)
            .width(Length::Fixed(width))
            .into_element()
    }

    /// Hosts the dialog over `background`.
    ///
    /// Buttons, Enter, dismissal, and focus movement report through the
    /// builder’s callback. The host inherits the prompt’s name, role, and
    /// dismissal policy.
    #[must_use]
    pub fn into_host(self, background: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
        self.into_modal_host(background).into_element()
    }

    /// Builds the configured modal host before conversion into an element.
    #[must_use]
    pub fn into_modal_host(
        self,
        background: impl Into<Element<'a, Message>>,
    ) -> crate::modal_host::EnzModalHost<'a, Message> {
        let policy = self.policy();
        let insistent = self.insistent;
        let label = self.title.clone();

        // The surface consumes the builder, so the callback is shared with the host.
        let mut this = self;
        let placeholder: Box<dyn Fn(SaveChangesEvent) -> Message + 'a> =
            Box::new(|_| unreachable!());
        let on_event: std::rc::Rc<dyn Fn(SaveChangesEvent) -> Message + 'a> =
            std::rc::Rc::from(std::mem::replace(&mut this.on_event, placeholder));
        let surface_event = {
            let on_event = std::rc::Rc::clone(&on_event);
            move |event: SaveChangesEvent| on_event(event)
        };
        this.on_event = Box::new(surface_event);
        let surface = this.into_body();

        crate::modal_host::enz_modal_host(background, Some(surface), move |event| {
            on_event(SaveChangesEvent::from_host(event))
        })
        .policy(policy)
        .alert(insistent)
        .label(label)
    }
}

fn is_enter(key: &iced::keyboard::Key) -> bool {
    matches!(
        key,
        iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter)
    )
}

fn has_modifier(modifiers: iced::keyboard::Modifiers) -> bool {
    modifiers.shift() || modifiers.control() || modifiers.alt() || modifiers.logo()
}

const fn action_id_suffix(action: SaveChangesAction) -> &'static str {
    match action {
        SaveChangesAction::SaveAll => "save-all",
        SaveChangesAction::Discard => "discard",
        SaveChangesAction::Cancel => "cancel",
    }
}

/// The routed id of an action's button under `id_prefix`.
///
/// Matches the id [`EnzSaveChangesDialog`] renders, so a host can address a
/// specific button (to focus the safe default, or route an AT request) without
/// the dialog in hand.
#[must_use]
pub fn save_changes_action_id(
    id_prefix: &str,
    action: SaveChangesAction,
) -> iced::advanced::widget::Id {
    enzui_core::identity::derive_id(id_prefix, action_id_suffix(action))
}

/// Builds a scoped focus operation targeting the save button.
///
/// Use it when composing [`EnzSaveChangesDialog::into_body`] in a shared host.
/// The operation declines when `scope` is not the active presentation.
#[must_use]
pub fn focus_default_action_operation<T>(
    id_prefix: &str,
    scope: iced::advanced::widget::Id,
) -> Box<dyn iced::advanced::widget::Operation<T>>
where
    T: Send + 'static,
{
    let id = save_changes_action_id(id_prefix, SaveChangesAction::keyboard_default());
    Box::new(iced::advanced::widget::operation::scope(
        scope,
        iced::advanced::widget::operation::focusable::focus(id),
    ))
}
