//! [`EnzConfirmDialog`]: the canonical "are you sure?" preset over
//! [`EnzModalHost`](crate::modal_host::EnzModalHost).
//!
//! # Why a preset and not a new host
//!
//! Confirming an action is the single most-copied dialog in any application,
//! and every copy is a chance to get modality, focus order or the meaning of
//! the scrim slightly different. This composes the existing pieces —
//! [`EnzModalHost`](crate::modal_host) for modality,
//! [`EnzModalSurface`](crate::modal_surface) for chrome,
//! [`enz_checkbox`](crate::checkbox) for the option, and
//! [`enzui_core::confirm`] for the lifecycle — rather than adding a
//! fourth definition of what a modal is.
//!
//! # What it decides, and what it refuses to guess
//!
//! Alert status is explicit. A destructive button tone does not make a
//! dialog an alert. Tone is visual weight; alert is an assertion to assistive
//! technology that this interrupts. A confirm dialog for "discard draft?"
//! may be destructive-toned without being an alert, and a non-destructive
//! alert ("your session expired") exists too. Inferring one from the other
//! would make both wrong.
//!
//! The checkbox sits between the message and the actions. That is the
//! canonical position (macOS, GTK, Windows all agree) and it is also the
//! correct *focus* position: reading order and Tab order match, so a user
//! tabbing from the message reaches the option before the buttons that end
//! the dialog.

use enzui_core::confirm::{ConfirmCheckbox, ConfirmState};
use enzui_core::dismissal::DismissPolicy;
use enzui_core::modal::DismissCause;
use enzui_core::semantics::{Role, Semantics, Toggle};
use iced::widget::{column, row};
use iced::{Element, Length};

/// The dismiss cause the trailing action reports.
///
/// The action always sits at the trailing edge and Cancel before it, as
/// GNOME's alert dialogs and Apple's alerts both lay them out: "Cancel, Save"
/// and "Cancel, Delete". A destructive action is told apart by its red
/// tone, not by moving it.
#[must_use]
pub const fn trailing_dismiss_cause(_destructive: bool) -> DismissCause {
    DismissCause::DefaultAction
}

/// The dismiss cause a bare Enter fires on a dialog with this destructive
/// flag: the action on a safe dialog, Cancel on a destructive one, so Return
/// never deletes, the default libadwaita and Apple give a destructive alert.
#[must_use]
pub const fn enter_default_cause(destructive: bool) -> DismissCause {
    if destructive {
        DismissCause::CancelAction
    } else {
        DismissCause::DefaultAction
    }
}

/// What a presented confirm dialog reports to its host.
///
/// # One exit, not two
///
/// A confirm dialog can end by button, by Escape, or by a press on the
/// scrim. Only the first of those originates inside the dialog; the other
/// two are noticed by the [`modal host`](crate::modal_host) wrapped *around*
/// it. The obvious wiring — buttons to the preset, keys and scrim to the
/// host — gives the application two separate places to end one dialog, and
/// the one it forgets is the one that leaks: the presentation closes with
/// its [`ConfirmState`] never snapshotted, so the user's checkbox answer is
/// silently dropped and the next open inherits a stale state.
///
/// So every ending is a [`Resolved`](Self::Resolved), whatever noticed it.
/// [`from_host`](Self::from_host) is the funnel, and
/// [`EnzConfirmDialog::into_host`] is the wiring that cannot be forgotten.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfirmDialogEvent {
    /// The user ticked or unticked the checkbox.
    ///
    /// Reported rather than applied, because the state lives in the
    /// presentation the application owns — the same controlled-widget shape
    /// as every other widget here.
    CheckboxToggled(bool),
    /// The dialog should end, for this reason.
    ///
    /// The reason is a [`DismissCause`], not a bool, because
    /// [`ConfirmDecision::from_cause`](enzui_core::confirm::ConfirmDecision::from_cause)
    /// is what maps cause to meaning — and a scrim press means *cancelled*,
    /// which a bool at this layer would have to guess.
    Resolved(DismissCause),
    /// Tab inside the dialog: move focus onward, wrapping at the end.
    ///
    /// Carried through rather than swallowed, because focus containment is
    /// the host's job and the preset must not quietly drop it — the two
    /// axes are deliberately independent (an insistent alert refuses to
    /// close but must still cycle focus).
    FocusNext,
    /// Shift+Tab inside the dialog.
    FocusPrevious,
}

impl ConfirmDialogEvent {
    /// Funnel a [`ModalHostEvent`](crate::modal_host::ModalHostEvent) into
    /// the preset's own vocabulary.
    ///
    /// This is what makes "the dialog ended" a single case for the
    /// application no matter who noticed it, so there is exactly one place
    /// that can snapshot the checkbox and exactly one that can forget to.
    #[must_use]
    pub const fn from_host(event: crate::modal_host::ModalHostEvent) -> Self {
        use crate::modal_host::ModalHostEvent;

        match event {
            ModalHostEvent::Dismissed(cause) => Self::Resolved(cause),
            ModalHostEvent::FocusNext => Self::FocusNext,
            ModalHostEvent::FocusPrevious => Self::FocusPrevious,
        }
    }

    /// The focus operation this event calls for, if this scope is topmost.
    ///
    /// A covered scope — one that another modal is stacked above — must
    /// decline: cycling focus inside a dialog the user cannot see traps
    /// keyboard navigation in an invisible surface. Pass `is_topmost`
    /// from [`enzui_core::modal::ModalStack::is_topmost`] so the
    /// predicate that exists for exactly this purpose is consulted rather
    /// than re-derived.
    ///
    /// Returns `None` when:
    /// - the event is a dismissal or a checkbox toggle (neither is a focus
    ///   move), or
    /// - `is_topmost` is `false` (a covered scope declines).
    ///
    /// Delegates to [`crate::modal_host::focus_wrap_operation`] rather than
    /// re-deriving the mapping: two places deciding the same thing is a
    /// mutation nothing can catch.
    #[must_use]
    pub fn focus_operation<T>(
        &self,
        is_topmost: bool,
    ) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
    where
        T: Send + 'static,
    {
        if !is_topmost {
            return None;
        }
        use crate::modal_host::ModalHostEvent;

        let host_event = match self {
            Self::FocusNext => ModalHostEvent::FocusNext,
            Self::FocusPrevious => ModalHostEvent::FocusPrevious,
            Self::CheckboxToggled(_) | Self::Resolved(_) => return None,
        };
        crate::modal_host::focus_wrap_operation(host_event)
    }

    /// The focus operation this event calls for, **scoped to the presentation
    /// that must receive it**.
    ///
    /// Prefer this over [`focus_operation`](Self::focus_operation). The bool
    /// that method takes is sampled while the application handles the
    /// message, but iced does not run the operation then — it drains the
    /// whole message batch and rebuilds every tree first
    /// (`iced/winit/src/lib.rs`). A scope id is
    /// resolved against the rebuilt tree at traversal time instead, so a
    /// presentation that has since been covered or dismissed declines
    /// structurally rather than by a stale predicate.
    ///
    /// Returns `None` for events that are not focus moves.
    #[must_use]
    pub fn scoped_focus_operation<T>(
        &self,
        scope: iced::advanced::widget::Id,
    ) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
    where
        T: Send + 'static,
    {
        use crate::modal_host::ModalHostEvent;

        let host_event = match self {
            Self::FocusNext => ModalHostEvent::FocusNext,
            Self::FocusPrevious => ModalHostEvent::FocusPrevious,
            Self::CheckboxToggled(_) | Self::Resolved(_) => return None,
        };
        crate::modal_host::scoped_focus_operation(host_event, scope)
    }
}

/// The dismissal policy a confirm dialog uses, given whether it is an alert.
///
/// A plain confirm is dismissible by Escape and the scrim: backing out of
/// "are you sure?" is exactly what those gestures mean. An alert is
/// insistent — it must be answered, so its only exits are its own buttons.
///
/// A free function so the render path and a test ask the same question.
#[must_use]
pub const fn confirm_dismiss_policy(alert: bool) -> DismissPolicy {
    if alert {
        DismissPolicy::insistent()
    } else {
        DismissPolicy::modal()
    }
}

/// Semantics for a confirm dialog's surface.
///
/// Split out so the role/modality/description mapping is testable without
/// building an element, and so the preset cannot describe a dialog
/// differently from how [`crate::modal_host::dialog_semantics`] would.
pub fn confirm_semantics(title: &str, message: &str, alert: bool) -> Semantics {
    let role = if alert {
        Role::AlertDialog
    } else {
        Role::Dialog
    };
    Semantics::new(role)
        .label(title)
        // The message is the accessible *description*, not part of the name:
        // a screen reader announces the name on focus and the description
        // after it, so folding the message into the label would make every
        // announcement of this dialog recite the whole paragraph.
        .description(message)
        .modal(true)
}

/// The accessible name for the checkbox row.
///
/// The label is the name; there is no separate aria-label, because a
/// checkbox whose spoken name differs from its visible text is a checkbox
/// two users cannot talk about.
pub fn checkbox_semantics(spec: &ConfirmCheckbox, checked: bool) -> Semantics {
    Semantics::new(Role::CheckBox)
        .label(spec.label.as_str())
        .toggled(if checked { Toggle::True } else { Toggle::False })
        // `disabled`, not "not enabled": a forced choice is still displayed
        // and still announced, it just cannot be changed.
        .disabled(!spec.enabled)
}

/// A confirm dialog, hosted over an application background.
pub struct EnzConfirmDialog<'a, Message> {
    material: Option<enzui_theme::MaterialStyle>,
    grain: enzui_theme::GrainSpec,
    title: String,
    message: String,
    confirm_label: String,
    cancel_label: String,
    confirm_kind: crate::button::ButtonKind,
    cancel_kind: crate::button::ButtonKind,
    destructive: bool,
    alert: bool,
    #[cfg(test)]
    enter_route_probe: bool,
    /// Namespace for the routed control ids. `None` falls back to
    /// a prefix derived from the title in [`Self::effective_id_prefix`], so a
    /// preset built the ordinary way still routes its confirm / cancel /
    /// checkbox controls rather than leaving them anonymous.
    id_prefix: Option<String>,
    /// Writing direction of the action row; `None` follows the locale.
    direction: Option<enzui_core::placement::Direction>,
    state: &'a ConfirmState,
    on_event: Box<dyn Fn(ConfirmDialogEvent) -> Message + 'a>,
}

/// `background` with `dialog` over it when there is one, mounted the same
/// way whether or not it is shown.
///
/// Wrapping the page in a host only while a dialog is open moves the page to
/// another place in the widget tree on every open and close, so all of its
/// widget state starts over: a scrolled page jumps back to the top as the
/// dialog appears. Host every frame through this instead; `on_event` is the
/// callback the dialog itself takes.
pub fn confirm_dialog_host<'a, Message: Clone + 'a>(
    background: impl Into<Element<'a, Message>>,
    dialog: Option<EnzConfirmDialog<'a, Message>>,
    on_event: impl Fn(ConfirmDialogEvent) -> Message + 'a,
) -> Element<'a, Message> {
    crate::modal_host::host_when_shown(
        background,
        dialog,
        EnzConfirmDialog::into_host,
        move |event| on_event(ConfirmDialogEvent::from_host(event)),
    )
}

/// Build a confirm dialog reporting through `on_event`.
///
/// `state` is borrowed from the presentation rather than owned, which is what
/// makes "initialised once, preserved across rerenders, reset on reopen"
/// structural — see [`ConfirmState`].
pub fn enz_confirm_dialog<'a, Message>(
    title: impl Into<String>,
    message: impl Into<String>,
    state: &'a ConfirmState,
    on_event: impl Fn(ConfirmDialogEvent) -> Message + 'a,
) -> EnzConfirmDialog<'a, Message> {
    EnzConfirmDialog {
        material: None,
        grain: enzui_theme::GrainSpec::None,
        title: title.into(),
        message: message.into(),
        confirm_label: enzui_core::localization::text("enzui-confirm-dialog-ok"),
        cancel_label: enzui_core::localization::text("enzui-confirm-dialog-cancel"),
        confirm_kind: crate::button::ButtonKind::Accent,
        cancel_kind: crate::button::ButtonKind::Quiet,
        destructive: false,
        alert: false,
        #[cfg(test)]
        enter_route_probe: false,
        id_prefix: None,
        direction: None,
        state,
        on_event: Box::new(on_event),
    }
}

impl<'a, Message> EnzConfirmDialog<'a, Message> {
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

    /// Name the affirmative button.
    ///
    /// Named actions rather than "OK": a button that says what it does is
    /// readable without the message, which is what a screen-reader user
    /// hears when tabbing straight to the actions.
    #[must_use]
    pub fn confirm_label(mut self, label: impl Into<String>) -> Self {
        self.confirm_label = label.into();
        self
    }

    /// Name the cancelling button.
    #[must_use]
    pub fn cancel_label(mut self, label: impl Into<String>) -> Self {
        self.cancel_label = label.into();
        self
    }

    /// Override the affirmative button's kind.
    ///
    /// The default is [`ButtonKind::Accent`]: a dialog's default action is
    /// its most prominent button, as macOS draws the default button and
    /// libadwaita styles the default response. [`ButtonKind::Primary`] is a
    /// neutral fill that sits close to the dialog surface in the dark theme,
    /// so it does not mark the default. A destructive dialog uses
    /// [`ButtonKind::Destructive`] whatever this says.
    ///
    /// [`ButtonKind::Accent`]: crate::button::ButtonKind::Accent
    /// [`ButtonKind::Primary`]: crate::button::ButtonKind::Primary
    /// [`ButtonKind::Destructive`]: crate::button::ButtonKind::Destructive
    #[must_use]
    pub const fn confirm_kind(mut self, kind: crate::button::ButtonKind) -> Self {
        self.confirm_kind = kind;
        self
    }

    /// Override the cancel button's kind. Defaults to
    /// [`ButtonKind::Quiet`](crate::button::ButtonKind::Quiet) — neutral text
    /// with a hover wash, so the affirmative action stays the only filled
    /// button and the way out does not read as a link or a selection.
    #[must_use]
    pub const fn cancel_kind(mut self, kind: crate::button::ButtonKind) -> Self {
        self.cancel_kind = kind;
        self
    }

    /// The kind the affirmative button is actually built with.
    ///
    /// The view calls exactly this, so a test of this function is a test of
    /// the rendered button rather than of a parallel reimplementation.
    #[must_use]
    pub const fn effective_confirm_kind(&self) -> crate::button::ButtonKind {
        if self.destructive {
            crate::button::ButtonKind::Destructive
        } else {
            self.confirm_kind
        }
    }

    /// The kind the cancel button is actually built with. `destructive` does
    /// not reach it: only the affirmative action carries the danger tone.
    #[must_use]
    pub const fn effective_cancel_kind(&self) -> crate::button::ButtonKind {
        self.cancel_kind
    }

    /// Give the affirmative button destructive tone and destructive action
    /// semantics.
    ///
    /// This deliberately does not make the dialog an alert — disposition
    /// belongs to the action; alert status belongs to the surface. See the
    /// module docs.
    #[must_use]
    pub const fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    /// Announce this dialog as an alert that must be answered.
    ///
    /// Explicit, never inferred from [`destructive`](Self::destructive).
    /// Also switches the dismissal policy to insistent, because "must be
    /// answered" and "Escape does not close it" are the same statement.
    #[must_use]
    pub const fn alert(mut self, alert: bool) -> Self {
        self.alert = alert;
        self
    }

    /// Namespace the dialog's routed control ids.
    ///
    /// Each interactive control — confirm, cancel, and the optional checkbox —
    /// derives a stable `iced::advanced::widget::Id` under this prefix, so
    /// an AT `Click`/`Focus`/`SetValue` request against any of them routes.
    /// When unset, [`Self::effective_id_prefix`] derives a prefix from the
    /// title, so the controls are routable without the caller having to
    /// remember this — the same label-fallback rule `radio_group` uses.
    /// Set it when two confirm dialogs can be live at once (their titles would
    /// otherwise collide).
    #[must_use]
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = Some(prefix.into());
        self
    }

    /// The prefix the dialog's control ids are actually derived from: an
    /// explicit [`Self::id_prefix`], else one derived from the title.
    ///
    /// Deriving from the title guarantees the confirm / cancel / checkbox
    /// controls are never anonymous (and so never unroutable),
    /// even for a preset built the ordinary way. An empty title would produce
    /// an empty prefix, which `derive_id` still turns into a stable key, so the
    /// controls stay distinct from each other via their action suffixes.
    #[must_use]
    pub fn effective_id_prefix(&self) -> String {
        self.id_prefix
            .clone()
            .unwrap_or_else(|| format!("enz-confirm-{}", self.title))
    }

    /// The dismissal policy this dialog needs.
    #[must_use]
    pub const fn policy(&self) -> DismissPolicy {
        confirm_dismiss_policy(self.alert)
    }

    /// The dialog surface's semantics.
    pub fn semantics(&self) -> Semantics {
        confirm_semantics(&self.title, &self.message, self.alert)
    }

    /// The checkbox row's semantics, if this dialog has a checkbox.
    #[must_use]
    pub fn checkbox_semantics(&self) -> Option<Semantics> {
        self.state
            .checkbox()
            .map(|spec| checkbox_semantics(spec, self.state.is_checked()))
    }

    /// Whether the checkbox is currently changeable.
    #[must_use]
    pub fn checkbox_is_interactive(&self) -> bool {
        self.state.is_interactive()
    }
}

impl<'a, Message: Clone + 'a> EnzConfirmDialog<'a, Message> {
    /// Finish the builder and hand back the [`Element`], which is step 3 of
    /// the build pattern every widget in this crate follows
    /// (`docs/widgets.md`).
    ///
    /// For a dialog the element is the surface — exactly what
    /// [`into_surface`](Self::into_surface) returns, which is the only
    /// argument-free way this builder can become an `Element` (hosting needs
    /// a background to host *over*). This method is a thin forwarder to it,
    /// so there is one implementation and the two spellings cannot drift
    /// apart.
    ///
    /// **Prefer [`into_host`](Self::into_host) for anything user-facing**: it
    /// wires Escape and the scrim to the same `on_event` the buttons use,
    /// which a bare surface leaves to the caller.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        self.into_surface()
    }

    /// Build the dialog surface, for handing to a
    /// [`modal host`](crate::modal_host::enz_modal_host).
    #[must_use]
    pub fn into_surface(self) -> Element<'a, Message> {
        // Standalone surfaces publish Dialog; hosted surfaces leave it to the host.
        let semantics = self.semantics();
        Element::from(crate::semantics_container::SemanticsContainer::new(
            self.into_surface_body(),
            semantics,
        ))
    }

    /// The dialog's body (title, message, checkbox, actions) with routable
    /// control ids but no `Role::Dialog` semantics node of its own.
    ///
    /// Used by [`Self::into_modal_host`], where the host publishes the Dialog
    /// node. [`Self::into_surface`] wraps this with the Dialog semantics for
    /// the standalone case. Splitting them is what stops the hosted preset from
    /// publishing two nested `Role::Dialog` nodes for one dialog.
    #[must_use]
    fn into_surface_body(self) -> Element<'a, Message> {
        // Every decision is taken before the fields move, so the render path
        // and the queryable methods above cannot drift.
        let confirm_kind = self.effective_confirm_kind();
        let cancel_kind = self.effective_cancel_kind();
        let rtl = enzui_core::placement::resolve_direction(self.direction)
            == enzui_core::placement::Direction::RightToLeft;
        #[cfg(test)]
        let enter_route_probe = self.enter_route_probe;
        // Stable, routable ids for every actionable control.
        // Without these the confirm / cancel buttons and the checkbox published
        // `Role::Button`/`Role::CheckBox` with `default_action` and NO id, so
        // the collector stripped their advertised `Click`/`Focus` and an AT
        // user could read the dialog but never answer it. Derived from one
        // prefix so the three are distinct and stable across frames.
        let id_prefix = self.effective_id_prefix();
        let confirm_id = enzui_core::identity::derive_id(&id_prefix, "confirm");
        let cancel_id = enzui_core::identity::derive_id(&id_prefix, "cancel");
        let checkbox_id = enzui_core::identity::derive_id(&id_prefix, "checkbox");
        // Shared because the checkbox's toggle closure must outlive this
        // function while the buttons' messages are produced here and now.
        let on_event: std::rc::Rc<dyn Fn(ConfirmDialogEvent) -> Message + 'a> =
            std::rc::Rc::from(self.on_event);

        let mut body = column![
            iced::widget::text(self.title)
                .size(enzui_theme::text_scale::ui_px(17.0))
                .font(enzui_theme::text_defaults::font(
                    enzui_theme::FontRole::Ui,
                    enzui_theme::WeightRung::Bold,
                )),
            iced::widget::text(self.message).size(enzui_theme::text_scale::ui_px(13.0)),
        ]
        .spacing(8);

        // Between message and actions: the canonical position, and the one
        // that makes reading order and Tab order agree.
        if let Some(spec) = self.state.checkbox() {
            let checked = self.state.is_checked();
            let toggle = {
                let on_event = std::rc::Rc::clone(&on_event);
                move |next: bool| on_event(ConfirmDialogEvent::CheckboxToggled(next))
            };
            body = body.push(
                crate::checkbox::enz_checkbox(checked, spec.label.as_str(), toggle)
                    .enabled(spec.enabled)
                    .id(checkbox_id)
                    .into_element(),
            );
        }
        #[cfg(test)]
        if enter_route_probe {
            body = body.push(
                crate::focus_button::focus_button_maybe::<Message, iced::Theme, iced::Renderer>(
                    iced::widget::text("focus target"),
                    None,
                )
                .focusable_without_activation(true)
                .id(iced::advanced::widget::Id::new("confirm-enter-route-probe")),
            );
        }

        let cancel_btn = crate::button::enz_button_with(self.cancel_label, {
            let on_event = std::rc::Rc::clone(&on_event);
            move || on_event(ConfirmDialogEvent::Resolved(DismissCause::CancelAction))
        })
        .kind(cancel_kind)
        .id(cancel_id);
        let confirm_btn = crate::button::enz_button_with(self.confirm_label, {
            let on_event = std::rc::Rc::clone(&on_event);
            move || on_event(ConfirmDialogEvent::Resolved(DismissCause::DefaultAction))
        })
        .kind(confirm_kind)
        .id(confirm_id);

        // [Cancel] [Action] in reading order, the action at the trailing
        // edge in either tone and either direction; Enter's default is
        // decided separately by `enter_default_cause`.
        let actions: iced::Element<'a, Message> = row![cancel_btn, confirm_btn]
            .spacing(8)
            .mirrored(rtl)
            .into();

        body = body.push(
            iced::widget::container(
                row![iced::widget::Space::new().width(Length::Fill), actions].mirrored(rtl),
            )
            .padding(iced::Padding::ZERO.top(12.0)),
        );

        // Enter resolves the default unless a focused descendant consumed it.
        // Unscoped: the dialog owns the window while it is up.
        let destructive = self.destructive;
        let enter_event: std::rc::Rc<dyn Fn(ConfirmDialogEvent) -> Message + 'a> =
            std::rc::Rc::clone(&on_event);
        let captured: iced::Element<'a, Message> =
            crate::keyboard_capture::keyboard_capture_unscoped(body, move |key, modifiers| {
                if !matches!(
                    key,
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter)
                ) {
                    return None;
                }
                // Enter with modifiers is an app-level chord (Ctrl+Enter
                // "send", Shift+Enter "newline"); the dialog default must not
                // steal them.
                if modifiers.control() || modifiers.alt() || modifiers.logo() || modifiers.shift() {
                    return None;
                }
                Some(enter_event(ConfirmDialogEvent::Resolved(
                    enter_default_cause(destructive),
                )))
            });

        crate::modal_surface::enz_modal_surface(captured)
            .material_opt(self.material)
            .grain(self.grain)
            // 420 px, or the window less its margins on a phone.
            .width(Length::Fill.max(420.0))
            .into_element()
    }

    /// Host this dialog over `background`, wiring every exit to one event.
    ///
    /// This is the intended way to present a confirm dialog.
    /// [`into_surface`](Self::into_surface) exists for a caller composing an
    /// unusual host, but it hands back only the surface — leaving the
    /// caller to wire the host's Escape and scrim themselves, which is
    /// precisely the two-exit bug described on [`ConfirmDialogEvent`].
    ///
    /// The host's policy, alert flag and accessible name are taken from
    /// *this* dialog rather than passed again, so a preset that says
    /// `.alert(true)` cannot end up inside a host that closes on Escape.
    #[must_use]
    pub fn into_host(self, background: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
        self.into_modal_host(background).into_element()
    }

    /// Host this dialog with its surface tagged by `scope`.
    ///
    /// The scope id is what a focus operation aims at, so that a covered
    /// presentation declines at traversal time rather than by a boolean
    /// sampled earlier — see
    /// [`scoped_focus_operation`](crate::modal_host::scoped_focus_operation)
    /// for why the timing matters.
    #[must_use]
    pub fn into_scoped_host(
        self,
        background: impl Into<Element<'a, Message>>,
        scope: iced::advanced::widget::Id,
    ) -> Element<'a, Message> {
        self.into_modal_host(background).scope(scope).into_element()
    }

    /// The configured [`EnzModalHost`](crate::modal_host::EnzModalHost),
    /// before it is built into an element.
    ///
    /// [`into_host`](Self::into_host) is this plus `.into_element()`. It is
    /// split out because returning the *element* leaves nothing to assert
    /// against: a test could only re-ask the preset what policy it wanted,
    /// never what the host was actually handed. That gap let a sabotage that
    /// hardcoded a modal policy here — silently making every alert
    /// Escape-closable — pass the whole suite.
    ///
    /// The closure handed to [`enz_modal_host`](crate::modal_host::enz_modal_host)
    /// is the only runtime join between
    /// [`ConfirmDialogEvent::from_host`] and the application's `on_event`.
    /// It is exercised by
    /// `into_modal_host_closure_joins_from_host_to_on_event` via
    /// [`EnzModalHost`](crate::modal_host::EnzModalHost)'s internal `dispatch`,
    /// which asserts exactly one correctly-typed event per cause.
    #[must_use]
    pub fn into_modal_host(
        self,
        background: impl Into<Element<'a, Message>>,
    ) -> crate::modal_host::EnzModalHost<'a, Message> {
        // Read before the fields move, and read through the same queryable
        // methods a test calls, so host and preset cannot disagree.
        let policy = self.policy();
        let alert = self.alert;
        let label = self.title.clone();
        // Cloned out of the box because `into_surface` consumes `self`; the
        // host needs the same callback the surface got.
        let mut this = self;
        // Lift the callback out so the rest of `this` can be forwarded whole.
        let placeholder: Box<dyn Fn(ConfirmDialogEvent) -> Message + 'a> =
            Box::new(|_| unreachable!());
        let on_event: std::rc::Rc<dyn Fn(ConfirmDialogEvent) -> Message + 'a> =
            std::rc::Rc::from(std::mem::replace(&mut this.on_event, placeholder));
        let surface_event = {
            let on_event = std::rc::Rc::clone(&on_event);
            move |event: ConfirmDialogEvent| on_event(event)
        };

        // `..this` rather than an enumerated re-construction: this forwards
        // every field by construction, so adding a field to the struct can
        // never silently fail to reach the surface (only `on_event` differs).
        // Body-only, not `into_surface()`. The enclosing host publishes the
        // single `Role::Dialog` node from its own
        // `dialog_semantics(label, alert, policy)`; a Dialog node on the
        // surface too would be a duplicate nested dialog.
        let surface = EnzConfirmDialog {
            on_event: Box::new(surface_event),
            ..this
        }
        .into_surface_body();

        crate::modal_host::enz_modal_host(background, Some(surface), move |event| {
            // The funnel: Escape and the scrim arrive here as
            // `Resolved(cause)`, indistinguishable from a button press, so
            // the application's single `Resolved` arm snapshots the
            // checkbox for all of them.
            on_event(ConfirmDialogEvent::from_host(event))
        })
        .policy(policy)
        .alert(alert)
        .label(label)
    }
}
