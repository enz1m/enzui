//! [`EnzFormDialog`]: dynamic forms over
//! [`EnzModalHost`](crate::modal_host::EnzModalHost)s.
//!
//! # Where the thinking lives
//!
//! Most of a form dialog's behaviour is lifecycle: drafts owned per field,
//! values kept across rerenders and reset on reopen, focus after a failed
//! submit. That lives in [`enzui_core::form`]; this module renders it and
//! adds multiline text.
//!
//! # The multiline seam
//!
//! All other kinds round-trip as values (`&str`-in/`String`-out text fields;
//! `bool`-in/`bool`-out toggles). [`FieldValue`] holds all: [`FormSession`]s
//! stay single truth sources; widget layers stay pure session functions.
//! `iced::widget::text_editor` breaks that: opaque-rope `Content`s (caret/
//! selection/undo-owning; [`Action`]-mutated) can't per-frame rebuild without
//! caret destruction. Multiline fields hold two stores: editor `Content`s
//! (caret/selection/undo) + session drafts (everything-else-reasoned values).
//! Uncatchable dual-held single values: explicit seams with single crossing
//! points ([`MultilineBridge`]) enforcing: `Content`s author EDITING (sole
//! caret-knowers interpret [`Action`]s); sessions author VALUES (validation/
//! required/submission/Enter-handling read drafts, never editors); every editing
//! action same-step write-throughs to sessions ([`is_edit`](Action::is_edit)-
//! decided: caret moves churn zero drafts; edits never missed). Stated-aloud
//! last rules: caller-applied-without-write-through actions submit stale texts
//! under new-text screens — type-system-unobjecting. [`apply`](MultilineBridge::apply)s
//! do both-or-neither.

mod bridge;
mod event;
mod presenter;
mod semantics;

pub use bridge::MultilineBridge;
pub use event::FormDialogEvent;
pub use presenter::{
    FormOutcome, FormPresenter, field_widget_id, first_invalid_widget_id, focus_first_invalid,
    scoped_field_widget_id, scoped_first_invalid_widget_id, scoped_focus_first_invalid,
};
pub use semantics::{field_group_semantics, field_semantics, form_semantics};

use enzui_core::form::{FieldKind, FieldValue, FormField, FormSession};
use enzui_core::modal::DismissCause;
use enzui_core::semantics::Semantics;
use iced::widget::text_editor::Action;
use iced::{Element, Length};

const fn form_enter_event(destructive: bool) -> FormDialogEvent {
    if destructive {
        FormDialogEvent::Resolved(DismissCause::CancelAction)
    } else {
        FormDialogEvent::SubmitRequested
    }
}

/// A form dialog over an application background.
pub struct EnzFormDialog<'a, Message> {
    material: Option<enzui_theme::MaterialStyle>,
    grain: enzui_theme::GrainSpec,
    title: String,
    description: Option<String>,
    submit_label: String,
    cancel_label: String,
    submit_kind: crate::button::ButtonKind,
    cancel_kind: crate::button::ButtonKind,
    destructive: bool,
    #[cfg(test)]
    enter_route_probe: bool,
    session: &'a FormSession,
    editors: Option<&'a MultilineBridge>,
    on_event: Box<dyn Fn(FormDialogEvent) -> Message + 'a>,
    /// A per-presentation namespace for field ids
    /// (`enzui-form-field:{scope}:{field}`). Without one, ids are
    /// `enzui-form-field:{field}`, which collide when two forms are on screen
    /// and let one dialog focus the other's field. Set via
    /// [`field_id_scope`](Self::field_id_scope).
    field_id_scope: Option<String>,
    /// Optional cap for the header + scrolling fields + pinned actions inside
    /// the modal surface. The surface's own padding is outside this value.
    max_height: Option<f32>,
    /// Writing direction of the action row; `None` follows the locale.
    direction: Option<enzui_core::placement::Direction>,
}

/// Build a form dialog reporting through `on_event`.
///
/// `session` is borrowed from the presentation rather than owned, which is
/// what makes "initialised once, preserved across rerenders, reset on reopen"
/// structural — see [`FormSession`].
pub fn enz_form_dialog<'a, Message>(
    title: impl Into<String>,
    session: &'a FormSession,
    on_event: impl Fn(FormDialogEvent) -> Message + 'a,
) -> EnzFormDialog<'a, Message> {
    EnzFormDialog {
        material: None,
        grain: enzui_theme::GrainSpec::None,
        title: title.into(),
        description: None,
        submit_label: enzui_core::localization::text("enzui-form-dialog-save"),
        cancel_label: enzui_core::localization::text("enzui-form-dialog-cancel"),
        submit_kind: crate::button::ButtonKind::Accent,
        cancel_kind: crate::button::ButtonKind::Quiet,
        destructive: false,
        #[cfg(test)]
        enter_route_probe: false,
        session,
        editors: None,
        on_event: Box::new(on_event),
        field_id_scope: None,
        max_height: None,
        direction: None,
    }
}

/// `background` with `dialog` over it when there is one, mounted the same
/// way whether or not it is shown, so the page keeps its widget state (a
/// scroll offset) as the dialog comes and goes; see
/// [`host_when_shown`](crate::modal_host::host_when_shown). `on_event` is the
/// callback the dialog itself takes.
pub fn form_dialog_host<'a, Message: Clone + 'a>(
    background: impl Into<Element<'a, Message>>,
    dialog: Option<EnzFormDialog<'a, Message>>,
    on_event: impl Fn(FormDialogEvent) -> Message + 'a,
) -> Element<'a, Message> {
    crate::modal_host::host_when_shown(background, dialog, EnzFormDialog::into_host, move |event| {
        on_event(FormDialogEvent::from_host(event))
    })
}

impl<'a, Message> EnzFormDialog<'a, Message> {
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

    /// Supporting text under the title, and the dialog's accessible
    /// description.
    #[must_use]
    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = Some(text.into());
        self
    }

    /// Namespace this form's field widget ids by `scope`. DISTINCT per
    /// presentation when two forms co-screen (unscoped-id-collision field docs
    /// explain why). Scoped-form first-invalid focusing uses
    /// [`scoped_focus_first_invalid`]/[`scoped_first_invalid_widget_id`] with
    /// same scopes. [`FormPresenter`]-driven forms instead namespace presenters
    /// ([`with_field_id_scope`](FormPresenter::with_field_id_scope)) + adopt via
    /// [`presenter`](Self::presenter): rendered-id/presenter-failed-submit-focus-target
    /// undrifted.
    #[must_use]
    pub fn field_id_scope(mut self, scope: impl Into<String>) -> Self {
        self.field_id_scope = Some(scope.into());
        self
    }

    /// Cap the dialog content while keeping the title and actions pinned.
    ///
    /// Only the field list scrolls. The cap applies inside the modal surface,
    /// so callers deriving it from a viewport should subtract the surface's
    /// vertical padding. A finite positive value enables that scrolling path;
    /// `NaN`, either infinity, zero, and negative values are treated exactly
    /// like the uncapped default.
    #[must_use]
    pub fn max_height(mut self, height: f32) -> Self {
        self.max_height = (height.is_finite() && height > 0.0).then_some(height);
        self
    }

    /// Name the submit button. A button that says what it does is readable
    /// without the title, which is what a screen-reader user hears when
    /// tabbing straight to the actions.
    #[must_use]
    pub fn submit_label(mut self, label: impl Into<String>) -> Self {
        self.submit_label = label.into();
        self
    }

    /// Name the cancelling button.
    #[must_use]
    pub fn cancel_label(mut self, label: impl Into<String>) -> Self {
        self.cancel_label = label.into();
        self
    }

    /// Override the submit button's kind. Defaults to
    /// [`Accent`](crate::button::ButtonKind::Accent): the submit is the
    /// form's default action, so it is its most prominent button. See
    /// [`EnzConfirmDialog::confirm_kind`](crate::confirm_dialog::EnzConfirmDialog::confirm_kind).
    #[must_use]
    pub const fn submit_kind(mut self, kind: crate::button::ButtonKind) -> Self {
        self.submit_kind = kind;
        self
    }

    /// Override the cancel button's kind. Defaults to
    /// [`ButtonKind::Quiet`](crate::button::ButtonKind::Quiet): neutral text
    /// with no fill, so the affirmative action stays the only filled button
    /// and the way out does not read as a link, as in the confirm dialogs.
    #[must_use]
    pub const fn cancel_kind(mut self, kind: crate::button::ButtonKind) -> Self {
        self.cancel_kind = kind;
        self
    }

    /// The kind the submit button is actually built with.
    ///
    /// The view calls exactly this, so a test of this function is a test of
    /// the rendered button rather than of a parallel reimplementation.
    #[must_use]
    pub const fn effective_submit_kind(&self) -> crate::button::ButtonKind {
        if self.destructive {
            crate::button::ButtonKind::Destructive
        } else {
            self.submit_kind
        }
    }

    /// The kind the cancel button is actually built with. `destructive` does
    /// not reach it: only the affirmative action carries the danger tone.
    #[must_use]
    pub const fn effective_cancel_kind(&self) -> crate::button::ButtonKind {
        self.cancel_kind
    }

    /// Give the submit button destructive tone and destructive action
    /// semantics.
    ///
    /// Takes precedence over [`Self::submit_kind`]: a destructive form is
    /// destructive regardless of what kind was requested.
    #[must_use]
    pub const fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    /// Supply the editors for this form's multiline fields.
    ///
    /// Required if the model has any: a multiline field without a bridge
    /// cannot be rendered, because there is nothing holding its caret. See
    /// [`Self::needs_editors`].
    #[must_use]
    pub const fn editors(mut self, editors: &'a MultilineBridge) -> Self {
        self.editors = Some(editors);
        self
    }

    /// Render this form under the presenter that owns it. Single-call
    /// multiline-editor + field-id-namespace adoption from `presenter`s:
    /// rendered field ids == presenter-focused rejected-submit ids. Dialog-
    /// alone [`field_id_scope`](Self::field_id_scope)s leave presenters aiming at
    /// control-carried unscoped ids.
    ///
    /// # Panics
    ///
    /// Debug-build session-unowning `presenter`s: namespaces would belong to
    /// different forms.
    #[must_use]
    pub fn presenter(mut self, presenter: &'a FormPresenter) -> Self {
        debug_assert!(
            std::ptr::eq(self.session, presenter.session()),
            "the presenter must own the session this dialog renders, or its \
             field-id namespace describes another form"
        );
        self.editors = Some(presenter.bridge());
        self.field_id_scope = presenter.field_id_scope().map(str::to_owned);
        self
    }

    /// The submit button's widget id: `enzui-form-action:<scope>:submit`,
    /// under the [`field_id_scope`](Self::field_id_scope) (or the presenter's)
    /// and otherwise under the title, so an accessibility `Activate` routes to
    /// it and two differently scoped forms stay distinct.
    #[must_use]
    pub fn submit_widget_id(&self) -> iced::advanced::widget::Id {
        self.action_widget_id("submit")
    }

    /// The cancel button's widget id; see [`Self::submit_widget_id`].
    #[must_use]
    pub fn cancel_widget_id(&self) -> iced::advanced::widget::Id {
        self.action_widget_id("cancel")
    }

    fn action_widget_id(&self, action: &str) -> iced::advanced::widget::Id {
        let scope = self.field_id_scope.as_deref().unwrap_or(&self.title);
        iced::advanced::widget::Id::from(format!("enzui-form-action:{scope}:{action}"))
    }

    /// Whether this form has a multiline field, and therefore needs
    /// [`editors`](Self::editors).
    ///
    /// Queryable so the requirement is assertable rather than a runtime
    /// surprise.
    #[must_use]
    pub fn needs_editors(&self) -> bool {
        self.session
            .model()
            .fields()
            .iter()
            .any(|f| matches!(f.kind, FieldKind::Multiline { .. }))
    }

    /// The dialog surface's semantics.
    pub fn semantics(&self) -> Semantics {
        form_semantics(&self.title, self.description.as_deref())
    }

    /// The dismissal policy for form dialogs:
    /// [`modal`](enzui_core::dismissal::DismissPolicy::modal), so Escape
    /// and a scrim press back out of unsaved changes. A discard confirmation,
    /// if wanted, belongs in the application's `Resolved` handling.
    #[must_use]
    pub const fn policy(&self) -> enzui_core::dismissal::DismissPolicy {
        enzui_core::dismissal::DismissPolicy::modal()
    }
}

impl<'a, Message: Clone + 'a> EnzFormDialog<'a, Message> {
    /// Finish the builder and hand back the [`Element`], which is step 3 of
    /// the build pattern every widget in this crate follows
    /// (`docs/widgets.md`). Dialog elements are surfaces ([`into_surface`](Self::into_surface)-
    /// identical: sole argument-free `Element`-becoming spellings — hosting
    /// needs hosted-over backgrounds). Thin forwarders (single implementations;
    /// undriftable dual spellings). User-facing presentations prefer
    /// [`into_host`](Self::into_host): field-shared-`on_event`-wired Escape/scrim
    /// (bare surfaces caller-leave).
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        self.into_surface()
    }

    /// Build the dialog surface, for handing to a
    /// [`modal host`](crate::modal_host::enz_modal_host).
    ///
    /// Prefer [`into_host`](Self::into_host), which wires every exit.
    #[must_use]
    pub fn into_surface(self) -> Element<'a, Message> {
        // Every decision is taken before the fields move, so the render path
        // and the queryable methods above cannot drift.
        let semantics = self.semantics();
        let submit_id = self.submit_widget_id();
        let cancel_id = self.cancel_widget_id();
        let submit_kind = self.effective_submit_kind();
        let cancel_kind = self.effective_cancel_kind();
        #[cfg(test)]
        let enter_route_probe = self.enter_route_probe;
        // Captured before `self.on_event` moves, so the render loop below can
        // scope every field id by it.
        let field_id_scope = self.field_id_scope.clone();
        let max_height = self.max_height;
        let on_event: std::rc::Rc<dyn Fn(FormDialogEvent) -> Message + 'a> =
            std::rc::Rc::from(self.on_event);

        // Themed text, not bare `iced::widget::text`: bare text takes the
        // window's default ink, so a dialog drawn under a different theme
        // (a dark playback surface in a light app, say) showed dark text on
        // its dark surface.
        let mut header = iced::widget::column![
            crate::enz_text::enz_text(self.title)
                .size_px(enzui_theme::text_scale::ui_px(18.0))
                .weight_rung(enzui_theme::WeightRung::Semibold)
        ]
        .spacing(6);
        if let Some(description) = self.description {
            header = header.push(
                crate::enz_text::enz_text(description)
                    .size_px(enzui_theme::text_scale::ui_px(13.0))
                    .tone(crate::enz_text::TextTone::Muted),
            );
        }

        let rows: Vec<Element<'a, Message>> = self
            .session
            .model()
            .fields()
            .iter()
            .map(|field| {
                field_element(
                    field,
                    self.session,
                    self.editors,
                    field_id_scope.as_deref(),
                    &on_event,
                )
            })
            .collect();
        #[cfg(test)]
        let rows = {
            let mut rows = rows;
            if enter_route_probe {
                rows.push(
                    crate::focus_button::focus_button_maybe::<
                        Message,
                        iced::Theme,
                        iced::Renderer,
                    >(iced::widget::text("focus target"), None)
                    .focusable_without_activation(true)
                    .id(iced::advanced::widget::Id::new("form-enter-route-probe"))
                    .into(),
                );
            }
            rows
        };

        let cancel_btn = crate::button::enz_button_with(self.cancel_label, {
            let on_event = std::rc::Rc::clone(&on_event);
            move || on_event(FormDialogEvent::Resolved(DismissCause::CancelAction))
        })
        .id(cancel_id)
        .kind(cancel_kind);
        let submit_btn = crate::button::enz_button_with(self.submit_label, {
            let on_event = std::rc::Rc::clone(&on_event);
            move || on_event(FormDialogEvent::SubmitRequested)
        })
        .id(submit_id)
        .kind(submit_kind);

        // Trailing-action order: the safe action stays at the trailing
        // edge. A non-destructive form draws [Cancel] [Submit]; a destructive
        // form inverts to [Submit(destructive)] [Cancel] so a keyboard user
        // reaching for Enter (with focus outside a text field) does not fire
        // the destructive path by muscle memory.
        let rtl = enzui_core::placement::resolve_direction(self.direction)
            == enzui_core::placement::Direction::RightToLeft;
        let actions: iced::Element<'a, Message> = if self.destructive {
            iced::widget::row![submit_btn, cancel_btn]
                .spacing(10)
                .mirrored(rtl)
                .into()
        } else {
            iced::widget::row![cancel_btn, submit_btn]
                .spacing(10)
                .mirrored(rtl)
                .into()
        };

        let fields: Element<'a, Message> = crate::form_layout::enz_form_layout(rows)
            .form_padding(0.0)
            .into();
        let footer: Element<'a, Message> =
            iced::widget::row![iced::widget::Space::new().width(Length::Fill), actions]
                .mirrored(rtl)
                .into();
        let body: Element<'a, Message> = if let Some(max_height) = max_height {
            // The title and action row are measured chrome. Only the fields
            // scroll, and their cap is whatever measured chrome leaves — no
            // guessed footer height.
            let fields: Element<'a, Message> = crate::scroll_view::contain_wheel(
                crate::scroll_view::enz_scrollable(fields).height(Length::Shrink),
                crate::scroll_view::Axis::Vertical,
            );
            crate::capped_dialog_body::enz_capped_dialog_body(
                vec![header.into()],
                fields,
                vec![footer],
                14.0,
                max_height,
            )
            .into_element()
        } else {
            iced::widget::column![header, fields, footer]
                .spacing(14)
                .into()
        };

        // Uncaptured Enter follows the safe trailing action, including Cancel
        // after destructive layout reverses the buttons. Scoped to focus
        // inside the form: a multiline field does not capture the Enter that
        // inserts its newline, so an unscoped default would also submit.
        let destructive = self.destructive;
        let enter_event = std::rc::Rc::clone(&on_event);
        let body: Element<'a, Message> =
            crate::keyboard_capture::keyboard_capture(body, move |key, modifiers| {
                if !matches!(
                    key,
                    iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter)
                ) {
                    return None;
                }
                if modifiers.control() || modifiers.alt() || modifiers.logo() || modifiers.shift() {
                    return None;
                }
                Some(enter_event(form_enter_event(destructive)))
            });

        // Up to 460 px wide, narrower in a smaller viewport, with no minimum:
        // `Length::Fill.max(460.0)`. `Fixed(460)` would also impose a minimum,
        // and `Fill` would go full-bleed on a desktop.
        let surface = crate::modal_surface::enz_modal_surface(body)
            .material_opt(self.material)
            .grain(self.grain)
            .width(Length::Fill.max(460.0))
            .into_element();

        Element::from(crate::semantics_container::SemanticsContainer::new(
            surface, semantics,
        ))
    }

    /// Host this dialog over `background`, wiring every exit to one event.
    /// Intended form-dialog presentations. [`into_surface`](Self::into_surface)s fit
    /// unusual-host-composing callers, but surface-only returns leave host
    /// Escape/scrim self-wired: second [`FormDialogEvent`]-external exit paths.
    /// *This*-dialog-taken host policies + accessible names (never re-passed):
    /// dialog/host undisagreeable.
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

    /// Split this dialog into a shared callback and a dialog that
    /// reports through a clone of it. [`into_modal_host`](Self::into_modal_host)-used
    /// seams: surface-own-reporting-closure handing + callback keeping for
    /// hosts. `..self`-forwarded (never enumerated re-constructions): newly-
    /// added builder fields constructionally reach surfaces. Public-seam-
    /// separately-inspectable host/surface pairs (over fully-composed-element-
    /// only observations).
    fn split_callback(mut self) -> (std::rc::Rc<dyn Fn(FormDialogEvent) -> Message + 'a>, Self) {
        let placeholder: Box<dyn Fn(FormDialogEvent) -> Message + 'a> =
            Box::new(|_| unreachable!("placeholder callback is never invoked"));
        let on_event: std::rc::Rc<dyn Fn(FormDialogEvent) -> Message + 'a> =
            std::rc::Rc::from(std::mem::replace(&mut self.on_event, placeholder));
        let surface_event = {
            let on_event = std::rc::Rc::clone(&on_event);
            move |event: FormDialogEvent| on_event(event)
        };
        let forwarded = Self {
            on_event: Box::new(surface_event),
            ..self
        };
        (on_event, forwarded)
    }

    /// The configured [`EnzModalHost`](crate::modal_host::EnzModalHost),
    /// before it is built into an element. [`into_host`](Self::into_host) = this +
    /// `.into_element()`. Split for caller/gate inspection of actually-
    /// configured hosts (policies, accessible names, scopes) over
    /// wrapped-unaddressable final composed elements.
    #[must_use]
    pub fn into_modal_host(
        self,
        background: impl Into<Element<'a, Message>>,
    ) -> crate::modal_host::EnzModalHost<'a, Message> {
        let policy = self.policy();
        let label = self.title.clone();
        let (on_event, forwarded) = self.split_callback();
        let surface = forwarded.into_surface();

        crate::modal_host::enz_modal_host(background, Some(surface), move |event| {
            on_event(FormDialogEvent::from_host(event))
        })
        .policy(policy)
        .label(label)
    }
}

/// Render one field. Control-needed everything session-borrowed straight:
/// `&'a FormField` labels/placeholders/helpers + `&'a FormSession` values/
/// errors. Zero view-cloned strings: staleable session-screen second copies
/// none.
fn field_element<'a, Message: Clone + 'a>(
    field: &'a FormField,
    session: &'a FormSession,
    editors: Option<&'a MultilineBridge>,
    scope: Option<&str>,
    on_event: &std::rc::Rc<dyn Fn(FormDialogEvent) -> Message + 'a>,
) -> Element<'a, Message> {
    // A session always holds a draft for every field in its model, so the
    // fallback is unreachable via the public API. It falls back to the
    // field's own initial rather than panicking, because a dialog that
    // vanishes is worse than one showing a default.
    let value = session.value(&field.id).unwrap_or(&field.initial);
    let error = session.error_for(&field.id);
    // Only for the read-only stand-in (`labelled_fallback`), which has no inner
    // control to publish a node: there the field must describe itself fully.
    // The real controls below publish their own interactive node, so their
    // wrapper is a *grouping* node (`field_group_semantics`) — not this one
    // — or every field would emit two interactive nodes.
    let fallback_semantics = field_semantics(field, value, error);
    let error_text = error.unwrap_or_default();
    let helper = field.helper.as_deref().unwrap_or_default();
    let id = &field.id;

    let control: Element<'a, Message> = match &field.kind {
        FieldKind::Secure { placeholder } => {
            // Reveal state lives on the session (`FormSession::is_revealed` /
            // `toggle_reveal`), so a form's password field uses the shipped
            // `EnzSecureField` -- labelled Show/Hide eye, pressed state,
            // always-redacted AT value -- instead of a masked text field with
            // no way to check what was typed.
            let text = value.as_text().unwrap_or_default();
            let revealed = session.is_revealed(&field.id);
            let on_input = {
                let on_event = std::rc::Rc::clone(on_event);
                move |next: String| {
                    on_event(FormDialogEvent::Edited(
                        id.clone(),
                        FieldValue::secure(next),
                    ))
                }
            };
            let on_toggle = {
                // The event carries the id, never the value: pressing the eye
                // adds no new route for the secret to reach an application's
                // `Message`. Typing sends `Edited` with a redacting
                // `FieldValue::Secure` that shares wipe-on-last-drop custody.
                // Event-time, so the host's mapping does not run once
                // per view pass.
                let on_event = std::rc::Rc::clone(on_event);
                move || on_event(FormDialogEvent::RevealToggled(id.clone()))
            };
            let mut widget =
                crate::secure_field::enz_secure_field_with(text, on_input, revealed, on_toggle)
                    .label(field.label.as_str())
                    .placeholder(placeholder.as_deref().unwrap_or_default())
                    .id(scoped_field_widget_id(scope, &field.id))
                    .enabled(field.enabled)
                    .read_only(field.read_only)
                    .error(error_text)
                    .description(helper);
            if field.enter_submits() {
                // Event-time, exactly as the text path below.
                let on_event = std::rc::Rc::clone(on_event);
                widget = widget.on_submit_with(move || on_event(FormDialogEvent::SubmitRequested));
            }
            widget.into_element()
        }
        FieldKind::Text { placeholder } => {
            let text = value.as_text().unwrap_or_default();
            // Ask the field, not the session-by-id: the session's by-id form
            // has to scan the model to find the field this function is
            // already holding, which made rendering a form quadratic in its
            // own field count. Measured by the scaling gate.
            let submits = field.enter_submits();
            let on_input = {
                let on_event = std::rc::Rc::clone(on_event);
                move |next: String| {
                    on_event(FormDialogEvent::Edited(id.clone(), FieldValue::Text(next)))
                }
            };
            let mut widget = crate::text_field::enz_text_field(
                field.label.as_str(),
                placeholder.as_deref().unwrap_or_default(),
                text,
                on_input,
            )
            .id(scoped_field_widget_id(scope, &field.id))
            .enabled(field.enabled)
            .read_only(field.read_only)
            .error(error_text)
            .description(helper);
            if submits {
                let on_event = std::rc::Rc::clone(on_event);
                // The submit factory runs only on an actual Enter press.
                // An eager `on_submit(on_event(SubmitRequested))` would run
                // the caller's mapping, and its side effects, on every view
                // pass. `enter_submits()` already excludes disabled, read-only
                // and multiline fields.
                widget = widget.on_submit_with(move || on_event(FormDialogEvent::SubmitRequested));
            }
            widget.into_element()
        }
        FieldKind::Multiline { placeholder, rows } => {
            let Some(content) = editors.and_then(|e| e.content(&field.id)) else {
                // No bridge for a multiline field. Render the value read-only
                // rather than dropping the field: a form silently missing a
                // field is far harder to notice than one that will not accept
                // typing. `needs_editors()` exists so this is assertable
                // before it happens.
                return labelled_fallback(
                    field.label.as_str(),
                    value.as_text().unwrap_or_default(),
                    fallback_semantics,
                );
            };
            let on_action = {
                let on_event = std::rc::Rc::clone(on_event);
                move |action: Action| on_event(FormDialogEvent::EditorAction(id.clone(), action))
            };
            crate::text_area::enz_text_area(content, on_action)
                .id(scoped_field_widget_id(scope, &field.id))
                .label(field.label.as_str())
                .placeholder(placeholder.as_deref().unwrap_or_default())
                .enabled(field.enabled)
                .read_only(field.read_only)
                .error(error_text)
                .description(helper)
                .area_height(f32::from((*rows).max(1)) * MULTILINE_ROW_HEIGHT)
                .into_element()
        }
        FieldKind::Toggle => {
            let on = matches!(value, FieldValue::Toggle(true));
            let toggle = {
                let on_event = std::rc::Rc::clone(on_event);
                move |next: bool| {
                    on_event(FormDialogEvent::Edited(
                        id.clone(),
                        FieldValue::Toggle(next),
                    ))
                }
            };
            // `EnzToggle`s, never bare `Switch`es. Bare switches render
            // controls only (AT-labelled but sighted-unidentifiable unlabelled
            // sliders on forms). `EnzToggle` labelled compositions with label
            // activation targets (every-other-row-here behaviours). SEPARATELY
            // forwarded `enabled` + `read_only` (above Text/Multiline-arm +
            // `text_field.rs`-documented read-only-vs-disabled-contract matched):
            // collapsed `enabled(field.is_editable())` fully disables (unfocused,
            // unannounced, dimmed). Reachable+announced edit-refusing read-only
            // fields: [`read_only`](EnzToggle::read_only)-supplied contracts.
            crate::toggle::enz_toggle(on, field.label.as_str(), toggle)
                .id(scoped_field_widget_id(scope, &field.id))
                .description(helper)
                .enabled(field.enabled)
                .read_only(field.read_only)
                .into_element()
        }
        FieldKind::Choice { options } => {
            // The value is the option's identity and a label marks only its
            // first occurrence, so the draft is resolved to a position here. A
            // value the options no longer offer resolves to neither a position
            // nor a label, so it selects nothing instead of a same-labelled
            // stranger.
            let selected_index = match value {
                FieldValue::Choice(Some(chosen)) => options.iter().position(|o| &o.value == chosen),
                _ => None,
            };
            let selected_label =
                selected_index.and_then(|index| options.get(index).map(|o| o.label.clone()));
            let labels: Vec<String> = options.iter().map(|o| o.label.clone()).collect();
            // The combo box needs some label callback (the builder requires it),
            // but it is never used: `on_select_index` below is installed and is
            // preferred at every selection site. Kept minimal.
            let on_select_label = {
                let on_event = std::rc::Rc::clone(on_event);
                let options = options.clone();
                let id = id.clone();
                move |chosen: String| {
                    // Fallback path only (unreachable while
                    // `on_select_index` is installed): first label match.
                    let value = options
                        .iter()
                        .find(|o| o.label == chosen)
                        .map(|o| o.value.clone());
                    on_event(FormDialogEvent::Edited(
                        id.clone(),
                        FieldValue::Choice(value),
                    ))
                }
            };
            let on_select_index = {
                let on_event = std::rc::Rc::clone(on_event);
                let options = options.clone();
                move |index: usize| {
                    // Map the selected option's INDEX back to its value. The
                    // index is the option's positional identity, so two options
                    // that share a display label do not
                    // collapse to the first match — the value stored is exactly
                    // the picked option's, not the first with that label. The
                    // combo box guarantees `index` is in range.
                    let value = options.get(index).map(|o| o.value.clone());
                    on_event(FormDialogEvent::Edited(
                        id.clone(),
                        FieldValue::Choice(value),
                    ))
                }
            };
            crate::combo_box::enz_combo_box(
                field.label.as_str(),
                labels,
                selected_label,
                on_select_label,
            )
            .selected_index(selected_index)
            .on_select_index(on_select_index)
            .id(scoped_field_widget_id(scope, &field.id))
            // `enabled` and `read_only` forwarded SEPARATELY, like
            // the Text/Multiline/Toggle arms. Collapsing a read-only Choice into
            // `enabled(field.is_editable())` would make it fully disabled,
            // unfocusable, and unannounced. A read-only field remains reachable
            // and announced while selection is refused; `EnzComboBox::read_only`
            // supplies that contract.
            .enabled(field.enabled)
            .read_only(field.read_only)
            .error(error_text)
            .into_element()
        }
    };

    // The wrapper is a NON-interactive grouping node. The `control`
    // above already publishes its own interactive node (TextInput / Switch /
    // ComboBox); wrapping it in a second interactive node made every field
    // emit two, with contradictory contracts for a Choice (RadioGroup around a
    // ComboBox). The group carries the field-level label/required/error the
    // control does not know about, and the control keeps its own role.
    Element::from(crate::semantics_container::SemanticsContainer::new(
        control,
        field_group_semantics(field, error),
    ))
}

/// Pixels per requested multiline row.
///
/// A constant rather than a literal at the call site, so the "rows" the model
/// asks for and the height the view produces are related in exactly one
/// place.
const MULTILINE_ROW_HEIGHT: f32 = 20.0;

/// A non-editable stand-in for a multiline field with no editor.
fn labelled_fallback<'a, Message: 'a>(
    label: &str,
    text: &str,
    semantics: Semantics,
) -> Element<'a, Message> {
    let column: Element<'a, Message> = iced::widget::column![
        iced::widget::text(label.to_owned()).size(enzui_theme::text_scale::ui_px(13.0)),
        iced::widget::text(text.to_owned()).size(enzui_theme::text_scale::ui_px(14.0)),
    ]
    .spacing(4)
    .into();

    Element::from(crate::semantics_container::SemanticsContainer::new(
        column,
        semantics.read_only(true),
    ))
}
