//! Buttons with token styling, keyboard activation, focus rings and optional icons.
//!
//! [`ButtonKind::Primary`] is a neutral filled button; [`ButtonKind::Accent`]
//! marks the suggested or default action, such as a dialog's confirm. [`ButtonKind::Quiet`] uses neutral ink and pointer washes,
//! while [`ButtonKind::Ghost`] uses accent ink outside glass surfaces.
//!
//! Unpinned colours and corners follow the runtime theme. Size presets scale
//! height, text and icons with the desktop text size; [`EnzButton::size_tokens`]
//! pins geometry. Disabled builders retain their callbacks for re-enabling.
//!
//! [`EnzButton::selected`] publishes selection without changing ordinary button
//! paint. [`enz_filter_chip`] also changes selected paint. Use the view-switcher
//! widgets for grouped selection with a shared Tab stop and arrow navigation.

use crate::focus_button::FocusStatus;
/// Button colour tokens and geometry overrides.
pub use crate::tokens::{ButtonSizeTokens, ButtonTokens};
use iced::widget::button::{Status, Style};
use iced::widget::text;
use iced::{Border, Color, Element, Font, Length, Padding};
const ICON_ONLY_PADDING: f32 = 4.0;

/// The glyph of an icon-only preset button, for a button `height` tall: a
/// little over half of it, never smaller than the icon a label carries. A
/// glyph sized to sit beside text (one pixel under the label) reads as a
/// speck alone in a 26 px button.
#[must_use]
pub fn icon_only_glyph(height: f32, text_icon: f32) -> f32 {
    (height * 0.57).round().max(text_icon)
}
/// Button paint variants; destructive variants also publish an accessibility disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonKind {
    /// Neutral filled default with a subtle edge.
    #[default]
    Primary,
    /// Neutral filled surface with an outline.
    Secondary,
    /// Neutral text with transparent fill and border in every enabled state.
    /// Use [`crate::link::enz_link`] for URI navigation.
    Plain,
    /// Neutral text, transparent at rest, with borderless hover and pressed washes.
    Quiet,
    /// Accent text with pointer washes and a transparent default outline.
    /// Glass palettes use neutral text; custom tokens can override each state.
    Ghost,
    /// Neutral text and a persistent outline, transparent at rest.
    Outlined,
    /// Filled danger treatment and destructive accessibility disposition.
    Destructive,
    /// Accent fill and per-state foreground for a suggested action.
    Accent,
    /// Filled success treatment for positive confirmation.
    Success,
    /// Danger text and outline, transparent at rest, with destructive disposition.
    DestructiveOutlined,
    /// Borderless danger text with pointer washes and destructive disposition.
    QuietDestructive,
}
/// Geometry presets with nominal heights at 100% desktop text size.
/// Height, text and icons scale with desktop text size. Unpinned corners take
/// the theme's `shape.button` step (a pill by default); an invalid radius uses
/// the preset fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonSize {
    /// Nominal 22 px height.
    Xs,
    /// Nominal 26 px height.
    Sm,
    /// Nominal 28 px height; the default.
    #[default]
    Md,
    /// Nominal 40 px height.
    Lg,
    /// Nominal 52 px height.
    Xl,
}

impl ButtonSize {
    /// Resolve preset geometry at the effective desktop text scale.
    /// Height, text and icons scale; padding, radius and gap retain preset values.
    pub fn tokens(self) -> ButtonSizeTokens {
        match self {
            Self::Xs => ButtonSizeTokens::XS,
            Self::Sm => ButtonSizeTokens::SM,
            Self::Md => ButtonSizeTokens::MD,
            Self::Lg => ButtonSizeTokens::LG,
            Self::Xl => ButtonSizeTokens::XL,
        }
        .at_text_scale()
    }
    pub(crate) fn resolve_radius(self, theme: &iced::Theme) -> f32 {
        let radius = crate::theme_lookup::shape_radius(theme, |shape| shape.button);
        enzui_core::metrics::FiniteNonNeg::new_or(radius, self.tokens().radius).get()
    }
}
/// Builder for a styled [`crate::focus_button::FocusButton`].
/// Pointer release, Enter, Space and routed activation use the enabled callback.
#[must_use]
pub struct EnzButton<'a, Message> {
    label: String,
    message: Option<Message>,
    message_lazy: Option<Box<dyn Fn() -> Message + 'a>>,
    kind: ButtonKind,
    size: ButtonSize,
    enabled: bool,
    tokens: Option<ButtonTokens>,
    icon: String,
    icon_position: ButtonIconPosition,
    aria_label: Option<String>,
    tooltip: Option<String>,
    tooltip_delay: Option<std::time::Duration>,
    aria_description: Option<String>,
    selected: Option<bool>,
    selection_visual: bool,
    set_position: Option<enzui_core::semantics::SetPosition>,
    role: enzui_core::semantics::Role,
    size_tokens_override: Option<ButtonSizeTokens>,
    min_width: Option<f32>,
    width: Option<Length>,
    id: Option<iced::advanced::widget::Id>,
    audit_contrast: bool,
    audit_elision: bool,
    ellipsis: text::Ellipsis,
    reclaim_focus_on_return: Option<iced::advanced::widget::Id>,
    relinquish_focus_while_open: bool,
    material: Option<enzui_theme::MaterialStyle>,
    elevation: Option<u8>,
    mnemonic: bool,
    _phantom: std::marker::PhantomData<&'a ()>,
}
/// Icon placement relative to the visible label; ignored when no icon is supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonIconPosition {
    /// Icon before the label; the default.
    #[default]
    Leading,
    /// Icon after the label.
    Trailing,
    /// Hide the visible label and show the icon; the label still names the button.
    Only,
}
/// Create an enabled, content-sized Primary button with Md geometry and a bound message.
pub fn enz_button<'a, Message>(
    label: impl Into<String>,
    on_press: Message,
) -> EnzButton<'a, Message> {
    EnzButton {
        label: label.into(),
        message: Some(on_press),
        message_lazy: None,
        kind: ButtonKind::Primary,
        size: ButtonSize::Md,
        enabled: true,
        tokens: None,
        icon: String::new(),
        icon_position: ButtonIconPosition::Leading,
        aria_label: None,
        tooltip: None,
        tooltip_delay: None,
        aria_description: None,
        selected: None,
        selection_visual: false,
        set_position: None,
        role: enzui_core::semantics::Role::Button,
        size_tokens_override: None,
        min_width: None,
        width: None,
        id: None,
        audit_contrast: false,
        audit_elision: false,
        ellipsis: text::Ellipsis::End,
        reclaim_focus_on_return: None,
        relinquish_focus_while_open: false,
        material: None,
        elevation: None,
        mnemonic: false,
        _phantom: std::marker::PhantomData,
    }
}
/// Create a compact outlined toggle chip with visible and accessible selection.
/// Selection uses accent paint; both states publish selection on the button node.
pub fn enz_filter_chip<'a, Message: Clone + 'a>(
    label: impl Into<String>,
    selected: bool,
    on_toggle: Message,
) -> EnzButton<'a, Message> {
    let mut chip = enz_button(label, on_toggle)
        .kind(ButtonKind::Outlined)
        .size(ButtonSize::Sm)
        .selected(selected);
    chip.selection_visual = true;
    chip
}
/// Create a button whose factory runs on accepted activation rather than view construction.
/// Disabled buttons do not call the factory.
pub fn enz_button_with<'a, Message>(
    label: impl Into<String>,
    on_press: impl Fn() -> Message + 'a,
) -> EnzButton<'a, Message> {
    EnzButton {
        label: label.into(),
        message: None,
        message_lazy: Some(Box::new(on_press)),
        kind: ButtonKind::Primary,
        size: ButtonSize::Md,
        enabled: true,
        tokens: None,
        icon: String::new(),
        icon_position: ButtonIconPosition::Leading,
        aria_label: None,
        tooltip: None,
        tooltip_delay: None,
        aria_description: None,
        selected: None,
        selection_visual: false,
        set_position: None,
        role: enzui_core::semantics::Role::Button,
        size_tokens_override: None,
        min_width: None,
        width: None,
        id: None,
        audit_contrast: false,
        audit_elision: false,
        ellipsis: text::Ellipsis::End,
        reclaim_focus_on_return: None,
        relinquish_focus_while_open: false,
        material: None,
        elevation: None,
        mnemonic: false,
        _phantom: std::marker::PhantomData,
    }
}
/// Build a disabled, message-less element with the supplied kind and size.
pub fn enz_disabled_button<'a, Message: Clone + 'a>(
    label: impl Into<String>,
    kind: ButtonKind,
    size: ButtonSize,
) -> Element<'a, Message> {
    EnzButton {
        label: label.into(),
        message: None,
        message_lazy: None,
        kind,
        size,
        enabled: false,
        tokens: None,
        icon: String::new(),
        icon_position: ButtonIconPosition::Leading,
        aria_label: None,
        tooltip: None,
        tooltip_delay: None,
        aria_description: None,
        selected: None,
        selection_visual: false,
        set_position: None,
        role: enzui_core::semantics::Role::Button,
        size_tokens_override: None,
        min_width: None,
        width: None,
        id: None,
        audit_contrast: false,
        audit_elision: false,
        ellipsis: text::Ellipsis::End,
        reclaim_focus_on_return: None,
        relinquish_focus_while_open: false,
        material: None,
        elevation: None,
        mnemonic: false,
        _phantom: std::marker::PhantomData,
    }
    .into_element()
}
/// Create a disabled, message-less builder with Md geometry.
/// Further setters may add icons, selection and accessibility metadata; enabling
/// it does not supply a callback.
pub fn enz_inert_button<'a, Message: Clone + 'a>(
    label: impl Into<String>,
    kind: ButtonKind,
) -> EnzButton<'a, Message> {
    EnzButton {
        label: label.into(),
        message: None,
        message_lazy: None,
        kind,
        size: ButtonSize::Md,
        enabled: false,
        tokens: None,
        icon: String::new(),
        icon_position: ButtonIconPosition::Leading,
        aria_label: None,
        tooltip: None,
        tooltip_delay: None,
        aria_description: None,
        selected: None,
        selection_visual: false,
        set_position: None,
        role: enzui_core::semantics::Role::Button,
        size_tokens_override: None,
        min_width: None,
        width: None,
        id: None,
        audit_contrast: false,
        audit_elision: false,
        ellipsis: text::Ellipsis::End,
        reclaim_focus_on_return: None,
        relinquish_focus_while_open: false,
        material: None,
        elevation: None,
        mnemonic: false,
        _phantom: std::marker::PhantomData,
    }
}
#[deprecated(
    since = "0.2.0",
    note = "prefix-first naming — use `enz_disabled_button`"
)]
/// Deprecated alias for [`enz_disabled_button`].
pub fn disabled_enz_button<'a, Message: Clone + 'a>(
    label: impl Into<String>,
    kind: ButtonKind,
    size: ButtonSize,
) -> Element<'a, Message> {
    enz_disabled_button(label, kind, size)
}
/// Deprecated alias for [`enz_inert_button`].
#[deprecated(since = "0.2.0", note = "prefix-first naming — use `enz_inert_button`")]
pub fn inert_enz_button<'a, Message: Clone + 'a>(
    label: impl Into<String>,
    kind: ButtonKind,
) -> EnzButton<'a, Message> {
    enz_inert_button(label, kind)
}

impl<'a, Message: Clone + 'a> EnzButton<'a, Message> {
    /// Reads an underscore in the label as the access key marker
    /// (`"_Save"`, see [`crate::mnemonic`]): the letter is underlined while
    /// Alt is held, and Alt with the letter presses the button. Off by
    /// default, so labels show underscores as written.
    pub fn mnemonic(mut self, enabled: bool) -> Self {
        self.mnemonic = enabled;
        self
    }

    /// Select button paint; defaults to [`ButtonKind::Primary`].
    pub fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = kind;
        self
    }
    /// Select preset geometry; defaults to [`ButtonSize::Md`].
    /// [`Self::size_tokens`] takes precedence regardless of setter order.
    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }
    /// Enable or disable activation; defaults to enabled.
    /// Messages and factories survive toggles and are gated by the final value.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
    /// Raise this button to `level` (0 to 5) instead of the theme's
    /// `elevation.button`. At 0 a filled button is flat and edged by a
    /// border; above it, borderless with that level's shadow.
    pub fn elevation(mut self, level: u8) -> Self {
        self.elevation = Some(level.min(5));
        self
    }
    /// Wrap the button in glass with the supplied material style and matching live corners.
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }
    /// Pin colour and chrome tokens instead of resolving them from the runtime theme.
    /// This does not pin size geometry or corners; use [`Self::size_tokens`] for that.
    pub fn tokens(mut self, tokens: ButtonTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }
    /// Set a Lucide name or semantic alias resolved by the bundled icon library.
    /// An empty string omits the icon.
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = name.into();
        self
    }
    /// Choose icon placement; defaults to [`ButtonIconPosition::Leading`].
    /// Ignored without an icon. [`ButtonIconPosition::Only`] hides the visible label.
    pub fn icon_position(mut self, pos: ButtonIconPosition) -> Self {
        self.icon_position = pos;
        self
    }
    /// Override the app's hover delay for this control; zero starts revealing immediately.
    pub fn tooltip_delay(mut self, delay: std::time::Duration) -> Self {
        self.tooltip_delay = Some(delay);
        self
    }

    /// Add supplementary hover text. Icon-only buttons otherwise use their accessible name.
    /// Empty text disables the hint without removing that name.
    pub fn tooltip(mut self, tip: impl Into<String>) -> Self {
        self.tooltip = Some(tip.into());
        self
    }

    /// Override the accessible name; otherwise the original label supplies it, including icon-only buttons.
    pub fn aria_label(mut self, name: impl Into<String>) -> Self {
        self.aria_label = Some(name.into());
        self
    }
    /// Publish an accessible description independently of the name.
    /// Tooltip text is not copied automatically; supply the same hint here when needed.
    pub fn aria_description(mut self, description: impl Into<String>) -> Self {
        self.aria_description = Some(description.into());
        self
    }
    /// Publish selected or unselected state without changing ordinary button paint.
    /// [`enz_filter_chip`] also changes its selected paint. Grouped selection should
    /// use [`crate::view_switcher`] or [`crate::inline_view_switcher`] for arrow navigation.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = Some(selected);
        self
    }
    /// Publish a one-based domain position and set size for accessibility.
    /// Pagination cells use the page number and page count, rather than the visible slot.
    pub fn set_position(mut self, position: enzui_core::semantics::SetPosition) -> Self {
        self.set_position = Some(position);
        self
    }
    /// Override the accessibility role; defaults to Button.
    /// Focus and Enter/Space activation remain unchanged.
    pub fn role(mut self, role: enzui_core::semantics::Role) -> Self {
        self.role = role;
        self
    }
    /// Pin geometry, including corners, instead of presets and live theme radii.
    /// Takes precedence over [`Self::size`] regardless of setter order. Icon-only
    /// buttons also retain the supplied horizontal padding.
    pub fn size_tokens(mut self, tokens: ButtonSizeTokens) -> Self {
        self.size_tokens_override = Some(tokens);
        self
    }
    /// Set a width floor in logical pixels, clamped to the parent.
    /// Long labels can grow beyond the floor. Explicit [`Self::width`] wins.
    pub fn min_width(mut self, px: f32) -> Self {
        self.min_width = Some(px);
        self
    }
    /// Request the full width allocated by the parent when true.
    /// False clears the explicit width and restores content sizing or the
    /// [`Self::min_width`] floor. Shares setter state with [`Self::width`].
    pub fn fill_width(mut self, fill: bool) -> Self {
        self.width = if fill { Some(Length::Fill) } else { None };
        self
    }
    /// Set the button width; defaults to content-sized and overrides [`Self::min_width`].
    /// Non-Shrink widths centre content; Fill covers the full allocated hit region.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = Some(width.into());
        self
    }
    /// Set a stable widget ID for focus and routed actions.
    /// Collections should derive IDs from logical item keys across reorder and removal.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }
    /// Reclaim focus using the popup owner’s matching return token.
    /// Set it only while the popup is closed; None disables reclaim. The token
    /// is independent of [`Self::id`]. See [`crate::menu::EnzMenu::focus_return_to`].
    pub fn reclaim_focus_on_return(mut self, token: Option<iced::advanced::widget::Id>) -> Self {
        self.reclaim_focus_on_return = token;
        self
    }
    /// Release focus while the opened popup is active.
    /// Pair true while open with [`Self::reclaim_focus_on_return`] while closed;
    /// false is the default.
    pub fn relinquish_focus_while(mut self, open: bool) -> Self {
        self.relinquish_focus_while_open = open;
        self
    }
    /// Opt into accessibility contrast observations for resting, hover and pressed styles.
    /// Disabled is exempt. The audit shares the paint resolver, but transparent
    /// fills require the caller to account for the underlying surface. A stable
    /// [`Self::id`] identifies findings.
    pub fn audit_contrast(mut self, audit: bool) -> Self {
        self.audit_contrast = audit;
        self
    }
    /// Opt into label elision observations using the label’s font, size and allocated width.
    /// Defaults to false; a stable [`Self::id`] identifies findings. Icon-only
    /// buttons have no visible label to audit.
    pub fn audit_elision(mut self, audit: bool) -> Self {
        self.audit_elision = audit;
        self
    }
    /// Set single-line label elision; defaults to [`text::Ellipsis::End`].
    /// Middle preserves both ends of paths or other suffix-sensitive values.
    /// Icon-only buttons have no visible label to elide.
    pub fn ellipsis(mut self, ellipsis: text::Ellipsis) -> Self {
        self.ellipsis = ellipsis;
        self
    }
    /// Build the button and its optional glass and accessibility audit wrappers.
    pub fn into_element(mut self) -> Element<'a, Message> {
        let mnemonic = self.mnemonic.then(|| {
            let mnemonic = enzui_core::mnemonic::Mnemonic::parse(&self.label);
            self.label.clone_from(&mnemonic.text);
            mnemonic
        });
        let size = self.size;
        let size_tokens_pinned = self.size_tokens_override.is_some();
        let size_tokens = self
            .size_tokens_override
            .unwrap_or_else(|| self.size.tokens());
        let kind = self.kind;
        let material = self.material;
        let elevation = self.elevation;
        let paint_kind = if self.selection_visual && self.selected == Some(true) {
            ButtonKind::Accent
        } else {
            kind
        };
        let enabled = self.enabled;
        let pinned_tokens = self.tokens.clone();
        // The style closure writes before icon tint reads, keeping per-state foregrounds aligned.
        let status_cell = std::rc::Rc::new(std::cell::Cell::new(
            crate::focus_button::FocusStatus::Active,
        ));
        let content: Element<'a, Message> = build_button_content(
            &self.label,
            &self.icon,
            ButtonContentSpec {
                position: self.icon_position,
                text_size: size_tokens.text_size,
                icon_size: if self.icon_position == ButtonIconPosition::Only && !size_tokens_pinned
                {
                    icon_only_glyph(size_tokens.height, size_tokens.icon_size)
                } else {
                    size_tokens.icon_size
                },
                gap: size_tokens.gap,
                fg_role: button_fg_role(paint_kind),
                enabled,
                pinned_tokens: self.tokens.clone(),
                status_cell: std::rc::Rc::clone(&status_cell),
                audit_elision: self.audit_elision,
                audit_id: self.id.clone(),
                ellipsis: self.ellipsis,
                mnemonic_at: mnemonic.as_ref().and_then(|mnemonic| mnemonic.at.clone()),
            },
        );
        let tooltip = self.tooltip.clone().or_else(|| {
            icon_only(&self.label, &self.icon, self.icon_position).then(|| {
                self.aria_label
                    .clone()
                    .unwrap_or_else(|| self.label.clone())
            })
        });
        let a11y_name = self
            .aria_label
            .clone()
            .unwrap_or_else(|| self.label.clone());
        let mut semantics = enzui_core::semantics::Semantics::new(self.role)
            .label(a11y_name)
            .destructive_action(matches!(
                kind,
                ButtonKind::Destructive
                    | ButtonKind::DestructiveOutlined
                    | ButtonKind::QuietDestructive
            ))
            .disabled(!enabled)
            .default_action();
        if let Some(description) = self.aria_description.clone() {
            semantics = semantics.description(description);
        }
        if let Some(sel) = self.selected {
            semantics = semantics.selected(sel);
        }
        if let Some(pos) = self.set_position {
            semantics = semantics.set_position(pos);
        }
        let mut effective_lazy: Option<Box<dyn Fn() -> Message + 'a>> =
            if enabled { self.message_lazy } else { None };
        let effective_message = if enabled && effective_lazy.is_none() {
            self.message
        } else {
            None
        };
        // The access key presses the button with the same message.
        let on_key: Option<std::rc::Rc<dyn Fn() -> Message + 'a>> = match &mnemonic {
            Some(mnemonic) if mnemonic.key.is_some() => match effective_lazy.take() {
                Some(lazy) => {
                    let shared: std::rc::Rc<dyn Fn() -> Message + 'a> = std::rc::Rc::from(lazy);
                    let pressed = std::rc::Rc::clone(&shared);
                    effective_lazy = Some(Box::new(move || pressed()));
                    Some(shared)
                }
                None => effective_message.clone().map(|message| {
                    std::rc::Rc::new(move || message.clone())
                        as std::rc::Rc<dyn Fn() -> Message + 'a>
                }),
            },
            _ => None,
        };
        // Explicit geometry owns padding; only presets receive the compact icon-only inset.
        let padding_x = if self.icon_position == ButtonIconPosition::Only
            && !self.icon.is_empty()
            && !size_tokens_pinned
        {
            ICON_ONLY_PADDING
        } else {
            size_tokens.padding_x
        };

        let min_width = self.min_width;
        let mut fb = if let Some(lazy) = effective_lazy {
            crate::focus_button::focus_button_maybe_with(content, Some(lazy))
        } else {
            crate::focus_button::focus_button_maybe(content, effective_message)
        }
        .padding(label_only_padding(
            &self.label,
            &self.icon,
            &size_tokens,
            padding_x,
        ))
        .height(Length::Fixed(size_tokens.height))
        .relinquish_focus_while(self.relinquish_focus_while_open)
        .reclaim_focus_on_return(self.reclaim_focus_on_return.clone());
        if icon_only(&self.label, &self.icon, self.icon_position) {
            fb = fb.centered_loose_content();
        }
        if let Some(w) = self.width {
            fb = fb.width(w);
        } else if let Some(w) = min_width {
            fb = fb.width(Length::Fit.min(w));
        } else if icon_only(&self.label, &self.icon, self.icon_position) {
            // As wide as it is tall: with the theme's pill radius, a circle.
            fb = fb.width(Length::Fixed(size_tokens.height));
        }
        let audit_id = self.id.clone();
        if let Some(id) = self.id {
            fb = fb.id(id);
        }
        // Suppress the drawn label as a separate semantic child when the button has another name.
        if self.aria_label.as_ref().is_some_and(|al| *al != self.label) {
            fb = fb.content_text(self.label.clone());
        }
        let audit_contrast = self.audit_contrast;
        let audit_pinned = pinned_tokens.clone();
        let audit_size_tokens = size_tokens;
        let element: Element<'a, Message> = fb
            .clip(true)
            .semantics(semantics)
            // Outset placement keeps the ring distinct from a filled button’s border and background.
            .outset_focus_ring(true)
            .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
                status_cell.set(status);
                let tokens = pinned_tokens.clone().unwrap_or_else(|| {
                    crate::theme_lookup::resolve(theme, ButtonTokens::from_palette)
                });
                let fill_status = match status {
                    FocusStatus::Active | FocusStatus::Focused => Status::Active,
                    FocusStatus::Hovered => Status::Hovered,
                    FocusStatus::Pressed => Status::Pressed,
                    FocusStatus::Disabled => Status::Disabled,
                };
                let mut size_tokens = size_tokens;
                if !size_tokens_pinned {
                    size_tokens.radius = size.resolve_radius(theme);
                }
                let style = style_for(paint_kind, fill_status, enabled, &tokens, &size_tokens);
                // Pinned tokens keep exactly what the caller pinned.
                if pinned_tokens.is_some() || !enabled {
                    return style;
                }
                raised(style, paint_kind, fill_status, theme, elevation)
            })
            .into();
        let element = match material {
            Some(style) => {
                let pinned_radius = size_tokens.radius;
                crate::glass::enz_glass(element)
                    .style(style)
                    .radius_resolved(move |theme| {
                        if size_tokens_pinned {
                            pinned_radius
                        } else {
                            size.resolve_radius(theme)
                        }
                    })
                    .into()
            }
            None => element,
        };
        let element = match (mnemonic, on_key) {
            (Some(mnemonic), Some(on_key)) => {
                crate::mnemonic::with_mnemonic_rc(element, mnemonic, on_key)
            }
            _ => element,
        };

        let element = match tooltip {
            Some(tip) => crate::tooltip::enz_tooltip_with_delay(
                element,
                tip,
                iced::widget::tooltip::Position::Top,
                self.tooltip_delay
                    .unwrap_or_else(crate::tooltip::default_tooltip_delay),
            ),
            None => element,
        };
        if !audit_contrast {
            return element;
        }
        let mut probe = crate::ContrastScanProbe::all_statuses(
            element,
            crate::accessibility_metrics::ContrastMode::Small,
            move |theme: &iced::Theme,
                  status: enzui_core::accessibility_scan::InteractionStatus| {
                use enzui_core::accessibility_scan::InteractionStatus;
                let tokens = audit_pinned.clone().unwrap_or_else(|| {
                    crate::theme_lookup::resolve(theme, ButtonTokens::from_palette)
                });
                let fill_status = match status {
                    InteractionStatus::Resting | InteractionStatus::Focused => Status::Active,
                    InteractionStatus::Hovered => Status::Hovered,
                    InteractionStatus::Pressed => Status::Pressed,
                    InteractionStatus::Disabled => Status::Disabled,
                };
                let style = style_for(
                    paint_kind,
                    fill_status,
                    enabled,
                    &tokens,
                    &audit_size_tokens,
                );
                // Transparent fills need surface-aware interpretation by the audit caller.
                let bg = match style.background {
                    Some(iced::Background::Color(c)) => c,
                    _ => tokens.disabled_bg,
                };
                (style.text_color, bg)
            },
        )
        .allow_status(enzui_core::accessibility_scan::InteractionStatus::Disabled);
        if let Some(id) = audit_id {
            probe = probe.id(id);
        }
        probe.into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzButton<'a, Message>> for Element<'a, Message> {
    fn from(btn: EnzButton<'a, Message>) -> Self {
        btn.into_element()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KindInteraction {
    Active,
    Hovered,
    Pressed,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct KindVisual {
    pub bg: Color,
    pub fg: Color,
    pub border_color: Color,
    pub border_width: f32,
}
pub(crate) fn kind_visual(
    kind: ButtonKind,
    interaction: KindInteraction,
    tokens: &ButtonTokens,
) -> KindVisual {
    use KindInteraction::{Active, Hovered, Pressed};
    match kind {
        ButtonKind::Primary => {
            let bg = match interaction {
                Active => tokens.primary_bg,
                Hovered => tokens.primary_hover_bg,
                Pressed => tokens.primary_pressed_bg,
            };
            KindVisual {
                bg,
                fg: tokens.primary_fg,
                border_color: tokens.primary_border,
                border_width: tokens.primary_border_width,
            }
        }
        ButtonKind::Secondary => {
            let bg = match interaction {
                Active => tokens.secondary_bg,
                Hovered => tokens.secondary_hover_bg,
                Pressed => tokens.secondary_pressed_bg,
            };
            KindVisual {
                bg,
                fg: tokens.secondary_fg,
                border_color: tokens.secondary_border,
                border_width: tokens.border_width,
            }
        }
        ButtonKind::Plain => KindVisual {
            bg: Color::TRANSPARENT,
            fg: tokens.secondary_fg,
            border_color: Color::TRANSPARENT,
            border_width: 0.0,
        },
        ButtonKind::Quiet => {
            let bg = match interaction {
                Active => Color::TRANSPARENT,
                Hovered => tokens.ghost_hover_bg,
                Pressed => tokens.ghost_pressed_bg,
            };
            KindVisual {
                bg,
                fg: tokens.secondary_fg,
                border_color: Color::TRANSPARENT,
                border_width: 0.0,
            }
        }
        ButtonKind::Ghost => {
            let bg = match interaction {
                Active => tokens.ghost_bg,
                Hovered => tokens.ghost_hover_bg,
                Pressed => tokens.ghost_pressed_bg,
            };
            let fg = match interaction {
                Active => tokens.ghost_fg,
                Hovered => tokens.ghost_hover_fg,
                Pressed => tokens.ghost_pressed_fg,
            };
            KindVisual {
                bg,
                fg,
                border_color: tokens.ghost_border,
                border_width: tokens.ghost_border_width,
            }
        }
        ButtonKind::Outlined => {
            let bg = match interaction {
                Active => Color::TRANSPARENT,
                Hovered => tokens.secondary_hover_bg,
                Pressed => tokens.secondary_pressed_bg,
            };
            KindVisual {
                bg,
                fg: tokens.secondary_fg,
                border_color: tokens.outlined_border,
                border_width: tokens.border_width,
            }
        }
        ButtonKind::Destructive => {
            let bg = match interaction {
                Active => tokens.destructive_bg,
                Hovered => tokens.destructive_hover_bg,
                Pressed => tokens.destructive_pressed_bg,
            };
            KindVisual {
                bg,
                fg: tokens.destructive_fg,
                border_color: Color::TRANSPARENT,
                border_width: 0.0,
            }
        }
        ButtonKind::Accent => {
            let (bg, fg) = match interaction {
                Active => (tokens.accent_bg, tokens.accent_fg),
                Hovered => (tokens.accent_hover_bg, tokens.accent_hover_fg),
                Pressed => (tokens.accent_pressed_bg, tokens.accent_pressed_fg),
            };
            KindVisual {
                bg,
                fg,
                border_color: Color::TRANSPARENT,
                border_width: 0.0,
            }
        }
        ButtonKind::Success => {
            let bg = match interaction {
                Active => tokens.success_bg,
                Hovered => tokens.success_hover_bg,
                Pressed => tokens.success_pressed_bg,
            };
            KindVisual {
                bg,
                fg: tokens.success_fg,
                border_color: Color::TRANSPARENT,
                border_width: 0.0,
            }
        }
        ButtonKind::DestructiveOutlined => {
            let bg = match interaction {
                Active => Color::TRANSPARENT,
                Hovered => tokens.secondary_hover_bg,
                Pressed => tokens.secondary_pressed_bg,
            };
            KindVisual {
                bg,
                fg: tokens.quiet_destructive_fg,
                border_color: tokens.quiet_destructive_fg,
                border_width: tokens.border_width,
            }
        }
        ButtonKind::QuietDestructive => {
            let bg = match interaction {
                Active => Color::TRANSPARENT,
                Hovered => tokens.ghost_hover_bg,
                Pressed => tokens.ghost_pressed_bg,
            };
            KindVisual {
                bg,
                fg: tokens.quiet_destructive_fg,
                border_color: Color::TRANSPARENT,
                border_width: 0.0,
            }
        }
    }
}
pub(crate) fn kind_is_framed(kind: ButtonKind) -> bool {
    matches!(kind, ButtonKind::Secondary | ButtonKind::Outlined)
}
fn style_for(
    kind: ButtonKind,
    status: Status,
    enabled: bool,
    tokens: &ButtonTokens,
    size_tokens: &ButtonSizeTokens,
) -> Style {
    if !enabled || matches!(status, Status::Disabled) {
        // Kinds that are transparent at rest stay transparent when disabled
        // and only dim their label: a fill appearing on a disabled quiet
        // button made it look more like a button than its enabled
        // neighbours (Calculator's MS / MR / MC).
        let transparent_at_rest = matches!(
            kind,
            ButtonKind::Plain
                | ButtonKind::Quiet
                | ButtonKind::Ghost
                | ButtonKind::QuietDestructive
        );
        return Style {
            background: Some(
                if transparent_at_rest {
                    Color::TRANSPARENT
                } else {
                    tokens.disabled_bg
                }
                .into(),
            ),
            text_color: tokens.disabled_fg,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: size_tokens.radius.into(),
            },
            ..Style::default()
        };
    }

    let interaction = match status {
        Status::Active => KindInteraction::Active,
        Status::Hovered => KindInteraction::Hovered,
        Status::Pressed => KindInteraction::Pressed,
        Status::Disabled => unreachable!(),
    };
    let KindVisual {
        bg,
        fg,
        border_color,
        border_width,
    } = kind_visual(kind, interaction, tokens);

    Style {
        background: Some(bg.into()),
        text_color: fg,
        border: Border {
            color: border_color,
            // An invisible edge takes no width: the renderer does not fill
            // under a border, so a transparent one left the fill a pixel
            // short of the button's bounds.
            width: if border_color.a > 0.0 {
                border_width
            } else {
                0.0
            },
            radius: size_tokens.radius.into(),
        },
        ..Style::default()
    }
}
/// `style` lifted to the theme's button elevation (`elevation.button`), as
/// macOS lifts its push buttons: every filled kind (Primary, Secondary,
/// Accent, Destructive, Success) keeps its hairline edge and casts the same
/// small contact shadow under it, which drops while pressed so the button
/// sinks under the pointer. Outlined and flat kinds cast none. Level 0 is
/// flat; glass and high contrast stay flat.
fn raised(
    mut style: Style,
    kind: ButtonKind,
    status: Status,
    theme: &iced::Theme,
    level: Option<u8>,
) -> Style {
    let filled = matches!(
        kind,
        ButtonKind::Primary
            | ButtonKind::Secondary
            | ButtonKind::Accent
            | ButtonKind::Destructive
            | ButtonKind::Success
    );
    let palette = crate::theme_lookup::palette(theme);
    let level = level.unwrap_or_else(|| crate::theme_lookup::elevation(theme).button());
    if !filled
        || level == 0
        || matches!(status, Status::Pressed)
        || palette.is_on_glass()
        || palette.is_high_contrast()
    {
        return style;
    }
    style.shadow = contact_shadow(
        level,
        crate::theme_lookup::shadow_color(theme),
        palette.is_dark(),
    );
    style
}

/// The shadow a filled button casts at `level`: one pixel down and a pixel
/// and a half soft at level 1, a little more per level above, and faint,
/// reading as the button resting on the surface rather than floating over
/// it. A dark theme needs a deeper shade for the same edge.
pub(crate) fn contact_shadow(level: u8, color: Color, dark: bool) -> iced::Shadow {
    let step = f32::from(level.max(1) - 1);
    let alpha = if dark { 0.32 } else { 0.12 } + step * 0.04;
    iced::Shadow {
        color: Color {
            a: alpha.min(1.0),
            ..color
        },
        offset: iced::Vector::new(0.0, 1.0 + step * 0.5),
        blur_radius: 1.5 + step * 1.5,
    }
}

fn button_fg_role(kind: ButtonKind) -> crate::icon::ButtonFgRole {
    crate::icon::ButtonFgRole::from(kind)
}

fn label_font() -> Font {
    enzui_theme::text_defaults::font(enzui_theme::FontRole::Ui, enzui_theme::WeightRung::Semibold)
}
fn label_optical_offset(text_size: f32) -> f32 {
    crate::optical_centre::cap_centre_offset(label_font(), text_size, text::LineHeight::default())
}
fn label_only_padding(
    label: &str,
    icon: &str,
    size_tokens: &ButtonSizeTokens,
    padding_x: f32,
) -> Padding {
    let shift = if !label.is_empty() && icon.is_empty() {
        label_optical_offset(size_tokens.text_size)
    } else {
        0.0
    };
    let vertical = shift_padding(shift);
    Padding {
        left: padding_x,
        right: padding_x,
        ..vertical
    }
}
fn shift_padding(shift: f32) -> Padding {
    let span = shift + shift;
    Padding::ZERO.top(span.max(0.0)).bottom((-span).max(0.0))
}
fn button_label<'a>(
    label: &str,
    text_size: f32,
    ellipsis: text::Ellipsis,
) -> text::Text<'a, iced::Theme> {
    text(label.to_owned())
        .size(text_size)
        .font(label_font())
        .wrapping(text::Wrapping::None)
        .ellipsis(ellipsis)
        .center()
}
struct ButtonContentSpec {
    position: ButtonIconPosition,
    text_size: f32,
    icon_size: f32,
    gap: f32,
    fg_role: crate::icon::ButtonFgRole,
    enabled: bool,
    pinned_tokens: Option<ButtonTokens>,
    status_cell: std::rc::Rc<std::cell::Cell<crate::focus_button::FocusStatus>>,
    audit_elision: bool,
    audit_id: Option<iced::advanced::widget::Id>,
    ellipsis: text::Ellipsis,
    /// The access key's letter in the label, underlined while Alt is held.
    mnemonic_at: Option<std::ops::Range<usize>>,
}
fn icon_only(label: &str, icon: &str, position: ButtonIconPosition) -> bool {
    !icon.is_empty() && (position == ButtonIconPosition::Only || label.is_empty())
}
fn build_button_content<'a, Message: 'a>(
    label: &str,
    icon: &str,
    spec: ButtonContentSpec,
) -> Element<'a, Message> {
    let ButtonContentSpec {
        position,
        text_size,
        icon_size,
        gap,
        fg_role,
        enabled,
        pinned_tokens,
        status_cell,
        audit_elision,
        audit_id,
        ellipsis,
        mnemonic_at,
    } = spec;
    // The label, and the same label with the access key underlined, when
    // there is one; `shape` sets their height and vertical alignment.
    let label_with =
        |shape: &dyn Fn(text::Text<'a, iced::Theme>) -> text::Text<'a, iced::Theme>,
         rich_shape: &dyn Fn(
            iced::widget::text::Rich<'a, (), Message, iced::Theme>,
        ) -> iced::widget::text::Rich<'a, (), Message, iced::Theme>|
         -> Element<'a, Message> {
            let plain: Element<'a, Message> =
                shape(button_label(label, text_size, ellipsis)).into();
            let Some(at) = mnemonic_at
                .clone()
                .filter(|at| label.get(at.clone()).is_some())
            else {
                return plain;
            };
            let spans = [
                iced::widget::span(label.get(..at.start).unwrap_or_default().to_owned()),
                iced::widget::span(label.get(at.clone()).unwrap_or_default().to_owned())
                    .underline(true),
                iced::widget::span(label.get(at.end..).unwrap_or_default().to_owned()),
            ];
            let marked = rich_shape(
                iced::widget::rich_text(spans)
                    .size(text_size)
                    .font(label_font())
                    .wrapping(text::Wrapping::None)
                    .ellipsis(ellipsis)
                    .center(),
            );
            crate::mnemonic::mnemonic_label(plain, marked.into())
        };
    let maybe_wrap_label = |lbl_el: Element<'a, Message>| -> Element<'a, Message> {
        if !audit_elision || label.is_empty() {
            return lbl_el;
        }
        let probe_font = label_font();
        let mut probe =
            crate::ElisionScanProbe::new(lbl_el, label.to_owned(), text_size, probe_font);
        if let Some(id) = audit_id.clone() {
            probe = probe.id(id);
        }
        probe.into()
    };
    if icon.is_empty() {
        return maybe_wrap_label(label_with(&|text| text, &|rich| rich));
    }
    let icon_el = crate::icon::enz_icon(icon)
        .size(icon_size)
        .tint_with_status(
            status_cell,
            move |theme: &iced::Theme, status: crate::focus_button::FocusStatus| {
                let tokens = pinned_tokens.clone().unwrap_or_else(|| {
                    crate::theme_lookup::resolve(theme, ButtonTokens::from_palette)
                });
                crate::icon::button_fg_ink_from_tokens_for_status(fg_role, enabled, &tokens, status)
            },
        );

    if icon_only(label, icon, position) {
        return icon_el.into();
    }
    // Shift the label’s cap centre while the icon remains centred in its own box.
    let span = 2.0 * label_optical_offset(text_size);
    let line = text::LineHeight::default()
        .to_absolute(iced::Pixels(text_size))
        .0;
    let align = if span > 0.0 {
        iced::alignment::Vertical::Bottom
    } else {
        iced::alignment::Vertical::Top
    };
    let lbl_el: Element<'a, Message> = if span == 0.0 {
        maybe_wrap_label(label_with(&|text| text, &|rich| rich))
    } else {
        maybe_wrap_label(label_with(
            &|text| text.height(Length::Fixed(line + span.abs())).align_y(align),
            &|rich| rich.height(Length::Fixed(line + span.abs())).align_y(align),
        ))
    };
    let row = match position {
        ButtonIconPosition::Leading => iced::widget::row![icon_el, lbl_el],
        ButtonIconPosition::Trailing => iced::widget::row![lbl_el, icon_el],
        ButtonIconPosition::Only => unreachable!(),
    };
    row.spacing(gap).align_y(iced::Alignment::Center).into()
}

#[cfg(test)]
mod quiet_kind;
