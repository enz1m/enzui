//! Scrolling keyboard-shortcut references as embedded content or modal surfaces.
//!
//! [`EnzShortcutsDialog::into_content`] builds a title and scrolling list.
//! [`EnzShortcutsDialog::into_host_with`] presents a Close button and modal host
//! over caller-provided background, reporting dismissal causes and focus moves.
//! Applications run focus operations and update their open state from those events.
//!
//! Viewport hints adapt the height, bounded by an explicit cap; layout reserves chrome
//! inside the incoming height. Intrinsic chrome can exceed an infeasible cap.
//! Text-colour overrides affect the title; list ink follows its own live-theme rules.

use crate::button::ButtonKind;
use crate::scroll_view::{enz_scroll_view, key_scroll_forward};
use crate::shortcut_list::{ShortcutEntry, enz_shortcut_list};
use enzui_core::dismissal::DismissPolicy;
use enzui_core::modal::DismissCause;
use iced::widget::{Space, container, text};
use iced::{Color, Element, Length};

/// Namespace of the dialog's own routable control ids.
const SHORTCUTS_ID_PREFIX: &str = "enzui-shortcuts-dialog";

/// Dismissal causes and focus moves reported by a hosted shortcuts dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShortcutsDialogEvent {
    /// Request dismissal with its cause; the application updates presentation state.
    Dismissed(DismissCause),
    /// Request forward focus traversal; the application runs the corresponding operation.
    FocusNext,
    /// Shift+Tab inside the dialog.
    FocusPrevious,
}

impl ShortcutsDialogEvent {
    /// Translate modal-host dismissal and focus events without changing their cause or direction.
    #[must_use]
    pub const fn from_host(event: crate::modal_host::ModalHostEvent) -> Self {
        use crate::modal_host::ModalHostEvent;

        match event {
            ModalHostEvent::Dismissed(cause) => Self::Dismissed(cause),
            ModalHostEvent::FocusNext => Self::FocusNext,
            ModalHostEvent::FocusPrevious => Self::FocusPrevious,
        }
    }

    /// Return unscoped forward or backward traversal, or None for a dismissal.
    /// This operation has no presentation or topmost check. Prefer
    /// [`Self::scoped_focus_operation`] when presentations can be covered or replaced.
    #[must_use]
    pub fn focus_operation<T>(&self) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
    where
        T: Send + 'static,
    {
        use crate::modal_host::ModalHostEvent;

        let host_event = match self {
            Self::FocusNext => ModalHostEvent::FocusNext,
            Self::FocusPrevious => ModalHostEvent::FocusPrevious,
            Self::Dismissed(_) => return None,
        };
        crate::modal_host::focus_wrap_operation(host_event)
    }

    /// Return a focus move restricted to the supplied presentation container id.
    /// The operation resolves the scope in the traversed tree; an absent scope
    /// declines the move. Dismissals return None. Configure a matching
    /// [`EnzShortcutsDialog::scope_key`] on the hosted presentation.
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
            Self::Dismissed(_) => return None,
        };
        crate::modal_host::scoped_focus_operation(host_event, scope)
    }

    /// Return invoker restoration for keyboard or button dismissal.
    /// Focus moves, ambient pointer dismissals such as scrim or drag, and missing
    /// invokers return None. Run the operation over the rebuilt post-dismissal tree.
    #[must_use]
    pub fn restore_focus_operation<T>(
        &self,
        invoker: Option<&iced::advanced::widget::Id>,
    ) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
    where
        T: Send + 'static,
    {
        use crate::modal_host::ModalHostEvent;

        let host_event = match self {
            Self::FocusNext | Self::FocusPrevious => return None,
            Self::Dismissed(cause) => ModalHostEvent::Dismissed(*cause),
        };
        crate::modal_host::restore_focus_operation(host_event, invoker)
    }
}

/// Allow Escape and scrim dismissal through the shared modal policy.
#[must_use]
pub const fn shortcuts_dismiss_policy() -> DismissPolicy {
    DismissPolicy::modal()
}

/// Builder for a modal keyboard-shortcuts dialog. See [`enz_shortcuts_dialog`].
#[must_use]
pub struct EnzShortcutsDialog<'a, Message: Clone> {
    material: Option<enzui_theme::MaterialStyle>,
    grain: enzui_theme::GrainSpec,
    open: bool,
    title: String,
    entries: Vec<ShortcutEntry>,
    close_label: String,
    dialog_width: f32,
    dialog_max_height: f32,
    explicit_height_cap: bool,
    viewport_height: Option<f32>,
    content_padding: f32,
    content_spacing: f32,
    surface_radius: f32,
    surface_border_width: f32,
    scrim: Option<Color>,
    surface_background: Option<Color>,
    surface_border: Option<Color>,
    text_color: Option<Color>,
    on_close: Option<Message>,
    invoker: Option<iced::advanced::widget::Id>,
    scope_key: Option<String>,
    _phantom: std::marker::PhantomData<&'a ()>,
}

/// Build a shortcut reference with a 520 px width and 480 px height cap.
/// Defaults include 12 px padding, 16 px content spacing, 12 px radius and the
/// "Keyboard Shortcuts" title. Content, host and convenience conversions honor
/// open; [`EnzShortcutsDialog::into_surface`] builds a bare surface independently.
pub fn enz_shortcuts_dialog<Message: Clone>(
    open: bool,
    entries: Vec<ShortcutEntry>,
) -> EnzShortcutsDialog<'static, Message> {
    EnzShortcutsDialog {
        material: None,
        grain: enzui_theme::GrainSpec::None,
        open,
        title: enzui_core::localization::text("enzui-shortcuts-dialog-keyboard-shortcuts"),
        entries,
        close_label: enzui_core::localization::text("enzui-shortcuts-dialog-close"),
        dialog_width: 520.0,
        dialog_max_height: 480.0,
        explicit_height_cap: false,
        viewport_height: None,
        content_padding: crate::modal_surface::DEFAULT_DIALOG_PADDING.px(),
        content_spacing: 16.0,
        surface_radius: 12.0,
        surface_border_width: 1.0,
        scrim: None,
        surface_background: None,
        surface_border: None,
        text_color: None,
        on_close: None,
        invoker: None,
        scope_key: None,
        _phantom: std::marker::PhantomData,
    }
}

impl<'a, Message: Clone + 'a> EnzShortcutsDialog<'a, Message> {
    /// Use material chrome for the dialog surface.
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// Lay grain on the dialog surface; see [`crate::grain`].
    pub fn grain(mut self, grain: impl Into<enzui_theme::GrainSpec>) -> Self {
        self.grain = grain.into();
        self
    }

    /// Dialog title. Defaults to `"Keyboard Shortcuts"`.
    pub fn title(mut self, s: impl Into<String>) -> Self {
        self.title = s.into();
        self
    }
    /// Label on the close button. Defaults to `"Close"`.
    pub fn close_label(mut self, s: impl Into<String>) -> Self {
        self.close_label = s.into();
        self
    }
    /// Dialog width in px. Non-finite or non-positive input falls back to 520.
    pub fn dialog_width(mut self, w: f32) -> Self {
        self.dialog_width = enzui_core::metrics::FinitePositive::new_or(w, 520.0).get();
        self
    }
    /// Pin the surface cap in logical pixels; invalid values use 480.
    /// Viewport hints can reduce this cap. Without a pin, a hint adapts the
    /// default 480 px height to the viewport. Intrinsic chrome can exceed the cap.
    pub fn dialog_max_height(mut self, h: f32) -> Self {
        self.dialog_max_height = enzui_core::metrics::FinitePositive::new_or(h, 480.0).get();
        self.explicit_height_cap = true;
        self
    }
    /// Adapt the height to the window height minus 48 px.
    /// Smaller explicit caps remain in effect. Invalid input clears the hint;
    /// incoming layout limits constrain the surface without a hint.
    pub fn viewport_height(mut self, h: f32) -> Self {
        self.viewport_height = if h.is_finite() && h > 0.0 {
            Some(h)
        } else {
            None
        };
        self
    }
    const fn viewport_margin(&self) -> f32 {
        24.0
    }
    /// Resolve the viewport-adapted height, bounded by an explicitly pinned cap.
    /// This is a requested cap, not measured geometry; parent limits and intrinsic
    /// chrome affect the final size.
    #[must_use]
    pub fn resolved_max_height(&self) -> f32 {
        match self.viewport_height {
            Some(vh) => {
                let avail = (vh - 2.0 * self.viewport_margin()).max(0.0);
                if self.explicit_height_cap {
                    self.dialog_max_height.min(avail)
                } else {
                    avail
                }
            }
            None => self.dialog_max_height,
        }
    }
    /// Inner padding in px. Non-finite or negative input falls back to 12 (`Md`).
    pub fn content_padding(mut self, p: f32) -> Self {
        self.content_padding = enzui_core::metrics::FiniteNonNeg::new_or(
            p,
            crate::modal_surface::DEFAULT_DIALOG_PADDING.px(),
        )
        .get();
        self
    }
    /// Vertical spacing between content rows in px. Non-finite or negative
    /// input falls back to 16.
    pub fn content_spacing(mut self, s: f32) -> Self {
        self.content_spacing = enzui_core::metrics::FiniteNonNeg::new_or(s, 16.0).get();
        self
    }
    /// Surface corner radius in px. Non-finite or negative input falls back to 12.
    pub fn surface_radius(mut self, r: f32) -> Self {
        self.surface_radius = enzui_core::metrics::FiniteNonNeg::new_or(r, 12.0).get();
        self
    }
    /// Surface border width in px. Non-finite or negative input falls back to 1.
    pub fn surface_border_width(mut self, w: f32) -> Self {
        self.surface_border_width = enzui_core::metrics::FiniteNonNeg::new_or(w, 1.0).get();
        self
    }
    /// Override the backdrop scrim colour. Unset by default (from the theme).
    pub fn scrim(mut self, c: Color) -> Self {
        self.scrim = Some(c);
        self
    }
    /// Override the dialog surface fill. Unset by default (from the theme).
    pub fn surface_background(mut self, c: Color) -> Self {
        self.surface_background = Some(c);
        self
    }
    /// Override the surface border colour. Unset by default (from the theme).
    pub fn surface_border(mut self, c: Color) -> Self {
        self.surface_border = Some(c);
        self
    }
    /// Pin title ink; shortcut-list headers, descriptions and badges keep their own theme rules.
    pub fn text_color(mut self, c: Color) -> Self {
        self.text_color = Some(c);
        self
    }
    /// Configure the dismissal message for [`Self::into_element`] and From conversion.
    /// Without it those paths build a passive preview. The rich host conversions
    /// use their supplied event callback instead of this message.
    pub fn on_close(mut self, msg: Message) -> Self {
        self.on_close = Some(msg);
        self
    }

    /// Store the opening control id for [`Self::restore_focus_operation_for`].
    /// Storing it alone does not run a focus operation.
    pub fn invoker(mut self, id: iced::advanced::widget::Id) -> Self {
        self.invoker = Some(id);
        self
    }

    /// Name the hosted presentation for scoped focus operations.
    /// Use [`crate::modal_host::presentation_scope_id`] to derive the matching operation id.
    pub fn scope_key(mut self, key: impl Into<String>) -> Self {
        self.scope_key = Some(key.into());
        self
    }

    /// The id set by [`invoker`](Self::invoker), if any.
    #[must_use]
    pub fn invoker_id(&self) -> Option<&iced::advanced::widget::Id> {
        self.invoker.as_ref()
    }

    /// Build dismissal restoration using this builder's stored invoker.
    /// Focus moves, ambient pointer dismissals and missing invokers return None.
    /// The application runs the operation over its rebuilt view after dismissal.
    #[must_use]
    pub fn restore_focus_operation_for<T>(
        &self,
        event: &ShortcutsDialogEvent,
    ) -> Option<Box<dyn iced::advanced::widget::Operation<T>>>
    where
        T: Send + 'static,
    {
        event.restore_focus_operation(self.invoker.as_ref())
    }

    /// Whether the dialog is currently open.
    pub fn is_open(&self) -> bool {
        self.open
    }
    /// The number of shortcut entries in the list.
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// The dismissal policy this dialog uses.
    #[must_use]
    pub const fn policy(&self) -> DismissPolicy {
        shortcuts_dismiss_policy()
    }

    fn content_parts(
        title: String,
        entries: Vec<ShortcutEntry>,
        text_pin: Option<Color>,
    ) -> (Element<'a, Message>, Element<'a, Message>) {
        let title_row: Element<'a, Message> = text(title)
            .size(crate::enz_text::TextSize::Title2.px())
            .font(enzui_theme::text_defaults::font(
                enzui_theme::FontRole::Ui,
                enzui_theme::WeightRung::ExtraBold,
            ))
            .style(move |t: &iced::Theme| iced::widget::text::Style {
                color: Some(text_pin.unwrap_or_else(|| crate::theme_lookup::palette(t).text)),
            })
            .into();
        let list: Element<'a, Message> = enz_shortcut_list(entries).into();
        let scroll_body: Element<'a, Message> = enz_scroll_view(list)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        (title_row, scroll_body)
    }

    /// Build a fill-sized title and scrolling list without modal chrome or a Close button.
    /// The caller supplies outer padding and height limits. Content spacing and title
    /// ink apply; dialog geometry, surface settings and dismissal callbacks do not.
    /// Closed content returns an empty fill placeholder. Page keys forward to the list.
    #[must_use]
    pub fn into_content(self) -> Element<'a, Message> {
        if !self.open {
            return Space::new().width(Length::Fill).height(Length::Fill).into();
        }

        let (title_row, scroll_body) =
            Self::content_parts(self.title, self.entries, self.text_color);
        let content: Element<'a, Message> = iced::widget::column![title_row, scroll_body]
            .spacing(self.content_spacing)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        key_scroll_forward(content)
    }

    /// Build the scrolling card and Close control without a modal host, regardless of open.
    /// The optional factory runs when Close is activated; this bare surface does not
    /// handle Escape or scrim dismissal. The requested height cap includes measured
    /// chrome and padding; when chrome cannot fit, the list body receives zero room
    /// and chrome may exceed the cap. Scrim settings apply only on hosted paths.
    #[must_use]
    pub fn into_surface(
        self,
        on_close: Option<Box<dyn Fn() -> Message + 'a>>,
    ) -> Element<'a, Message> {
        let scrim_pin = self.scrim;
        let surface_bg_pin = self.surface_background;
        let surface_border_pin = self.surface_border;
        let _ = scrim_pin;
        let text_pin = self.text_color;
        let dialog_width = self.dialog_width;
        let dialog_max_height = self.resolved_max_height();
        let content_padding = self.content_padding;
        let content_spacing = self.content_spacing;
        let surface_radius = self.surface_radius;
        let surface_border_width = self.surface_border_width;
        let (title_row, scroll_body) =
            Self::content_parts(self.title.clone(), self.entries.clone(), text_pin);
        let close_button: Element<'a, Message> = match on_close {
            // A stable id routes the button's invoke action to assistive
            // technology; one shortcuts dialog per window keeps it unique.
            Some(f) => crate::button::enz_button_with(self.close_label.clone(), f)
                .aria_label(self.close_label)
                .id(enzui_core::identity::derive_id(
                    SHORTCUTS_ID_PREFIX,
                    "close",
                ))
                .into(),
            None => crate::button::enz_inert_button(self.close_label.clone(), ButtonKind::Primary)
                .aria_label(self.close_label)
                .into(),
        };

        let close_row: Element<'a, Message> = container(close_button)
            .width(Length::Fill)
            .align_x(iced::alignment::Horizontal::Right)
            .into();
        // Intrinsic title and Close chrome survive even when the remaining body cap is zero.
        // A zero constructor cap means unbounded; a positive cap preserves chrome-only layout.
        let content_max_h = (dialog_max_height - content_padding * 2.0).max(f32::MIN_POSITIVE);
        let content: Element<'a, Message> = crate::capped_dialog_body::CappedDialogBody::new(
            vec![title_row],
            scroll_body,
            vec![close_row],
            content_spacing,
            content_max_h,
        )
        .into();
        // Close owns focus outside the scroll subtree, so page keys need surface-level forwarding.
        let content: Element<'a, Message> = key_scroll_forward(content);
        let mut surface = crate::modal_surface::enz_modal_surface(content)
            .material_opt(self.material)
            .grain(self.grain)
            .width(Length::Fixed(dialog_width))
            .padding(content_padding)
            .radius(surface_radius)
            .border_width(surface_border_width);
        if let Some(bg) = surface_bg_pin {
            surface = surface.background(bg);
        }
        if let Some(bc) = surface_border_pin {
            surface = surface.border_color(bc);
        }

        surface.into_element()
    }

    /// Host over the supplied background and report dismissal causes and focus moves.
    /// The host captures input while open; the application updates state and runs
    /// focus operations. Closed hosts leave the background live.
    #[must_use]
    pub fn into_host_with(
        self,
        background: impl Into<Element<'a, Message>>,
        on_event: impl Fn(ShortcutsDialogEvent) -> Message + 'a,
    ) -> Element<'a, Message> {
        self.into_modal_host(background, on_event).into_element()
    }

    /// Configure the modal host with the supplied background and event callback.
    /// Open controls surface presence; title names the dialog, and an optional scope
    /// key enables matching scoped focus operations. The on_close message is unused.
    #[must_use]
    pub fn into_modal_host(
        self,
        background: impl Into<Element<'a, Message>>,
        on_event: impl Fn(ShortcutsDialogEvent) -> Message + 'a,
    ) -> crate::modal_host::EnzModalHost<'a, Message> {
        let policy = self.policy();
        let label = self.title.clone();
        let scrim = self.scrim;
        let open = self.open;
        let scope_key = self.scope_key.clone();
        let on_event = std::rc::Rc::new(on_event);
        let close_msg: Box<dyn Fn() -> Message + 'a> = {
            let on_event = std::rc::Rc::clone(&on_event);
            Box::new(move || on_event(ShortcutsDialogEvent::Dismissed(DismissCause::CloseButton)))
        };
        let surface = open.then(|| self.into_surface(Some(close_msg)));

        let mut host = crate::modal_host::enz_modal_host(background, surface, move |event| {
            on_event(ShortcutsDialogEvent::from_host(event))
        })
        .policy(policy)
        .label(label);
        if let Some(scrim) = scrim {
            host = host.scrim(scrim);
        }
        if let Some(ref key) = scope_key {
            host = host.scope(crate::modal_host::presentation_scope_id(key));
        }
        host
    }

    /// Host over an empty background using the configured on_close message.
    /// With a message, Close, Escape and scrim request dismissal; Tab is captured
    /// without publishing focus moves. Without a message, the surface and scrim
    /// are a passive preview with no modal input handling. Closed conversion returns
    /// an empty fill placeholder. Use [`Self::into_host_with`] for a live background
    /// and application-controlled focus traversal.
    pub fn into_element(self) -> Element<'a, Message> {
        if !self.open {
            return Space::new().width(Length::Fill).height(Length::Fill).into();
        }
        let background: Element<'a, Message> =
            Space::new().width(Length::Fill).height(Length::Fill).into();
        match self.on_close.clone() {
            // Suppress focus events here: mapping them to on_close would dismiss on Tab.
            Some(on_close) => self
                .into_modal_host(background, move |_event| on_close.clone())
                .no_focus_events()
                .into_element(),
            None => {
                let _ = background;
                let scrim_pin = self.scrim;
                let surface = self.into_surface(None);
                let centred = container(surface)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(Length::Fill);
                let scrim_layer = container(Space::new().width(Length::Fill).height(Length::Fill))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .style(move |t: &iced::Theme| {
                        let color = crate::modal_host::resolve_scrim(
                            scrim_pin,
                            &crate::theme_lookup::palette(t),
                        );
                        iced::widget::container::Style {
                            background: Some(color.into()),
                            ..Default::default()
                        }
                    });
                iced::widget::stack![scrim_layer, centred].into()
            }
        }
    }
}

impl<'a, Message: Clone + 'a> From<EnzShortcutsDialog<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzShortcutsDialog<'a, Message>) -> Self {
        w.into_element()
    }
}
