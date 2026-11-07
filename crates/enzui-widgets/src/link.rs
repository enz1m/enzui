//! Hyperlinks with keyboard activation, live palette colours and underline policies.
//!
//! Activation calls the supplied factory with the owned URL; the caller opens
//! it. External mode adds a glyph and an accessible annotation without changing
//! activation. Defaults are Accent tone, unscaled 13 px text, Regular weight,
//! pointer-hover underlining and enabled interaction. The font uses iced defaults.
//!
//! Supply a unique, stable [`EnzLink::id`] for routed accessibility actions.
//! Anonymous links still publish their name, but the collector strips their
//! advertised actions. Duplicate destinations do not provide unique identities.

use crate::color::darken;
use crate::focus_button::FocusStatus;
use enzui_theme::Palette;
use iced::font::Weight;
use iced::widget::button::Style;
use iced::{Border, Color, Element, Font, Padding};

/// Palette role used for link ink; Accent by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkTone {
    /// The theme's accent color (default).
    #[default]
    Accent,
    /// Muted / secondary text color.
    Muted,
    /// Destructive / red.
    Danger,
    /// Success / green.
    Success,
}

/// Unscaled glyph sizes in logical pixels; Md (13 px) by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkSize {
    /// 11px (shared `text-size-sm` ramp).
    Sm,
    /// 13px (shared `body-size`), default.
    #[default]
    Md,
    /// 17px (shared `text-size-lg`).
    Lg,
    /// 22px (shared `text-size-xl`).
    Xl,
}

impl LinkSize {
    /// Return the unscaled glyph size: 11, 13, 17 or 22 logical pixels.
    pub fn text_size(self) -> f32 {
        match self {
            Self::Sm => 11.0,
            Self::Md => 13.0,
            Self::Lg => 17.0,
            Self::Xl => 22.0,
        }
    }
}

/// Glyph weight; Regular by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkWeight {
    /// 400 (default).
    #[default]
    Regular,
    /// 500.
    Medium,
    /// 600.
    Semibold,
    /// 700.
    Bold,
}

impl LinkWeight {
    fn as_font_weight(self) -> Weight {
        match self {
            Self::Regular => Weight::Normal,
            Self::Medium => Weight::Medium,
            Self::Semibold => Weight::Semibold,
            Self::Bold => Weight::Bold,
        }
    }
}

/// Underline policy for each visual label line; the external glyph is excluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LinkDecoration {
    /// Underline only while the pointer is over an enabled link; the default policy.
    #[default]
    Hover,
    /// Underline in every state, including disabled.
    Always,
    /// Never underline.
    None,
}

impl LinkDecoration {
    // Pressed includes keyboard holds and pointer drag-out; status alone cannot imply hover.
    fn underline_visible(self, status: FocusStatus, pointer_over: bool) -> bool {
        match self {
            LinkDecoration::Always => true,
            LinkDecoration::None => false,
            LinkDecoration::Hover => pointer_over && status != FocusStatus::Disabled,
        }
    }
}

/// Default external-link accessible annotation: `(opens externally)`.
/// Override with [`EnzLink::external_annotation`] or replace the whole name with
/// [`EnzLink::aria_label`].
pub const DEFAULT_EXTERNAL_ANNOTATION: &str = "(opens externally)";
fn accessible_name(
    label: &str,
    external: bool,
    annotation: Option<&str>,
    aria_label: Option<&str>,
) -> String {
    if let Some(name) = aria_label
        && !name.trim().is_empty()
    {
        return name.to_owned();
    }
    if !external {
        return label.to_owned();
    }
    let annotation = annotation.unwrap_or(DEFAULT_EXTERNAL_ANNOTATION);
    if annotation.trim().is_empty() {
        return label.to_owned();
    }
    if label.is_empty() {
        return annotation.to_owned();
    }
    format!("{label} {annotation}")
}

/// Palette-derived link ink and focus-outline overrides.
#[derive(Debug, Clone, Copy)]
pub struct LinkTokens {
    /// Accent color (used for `LinkTone::Accent`).
    pub accent: Color,
    /// Accent ink for hover and press; defaults to the palette hover role.
    pub accent_hover: Color,
    /// Muted text (used for `LinkTone::Muted`).
    pub text_muted: Color,
    /// Danger (used for `LinkTone::Danger`).
    pub danger: Color,
    /// Success (used for `LinkTone::Success`).
    pub success: Color,
    /// Focus ring color.
    pub focus_ring: Color,
}

impl LinkTokens {
    /// Derive link tokens from a palette.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            accent: p.accent,
            accent_hover: p.accent_hover,
            text_muted: p.text_muted,
            danger: p.destructive,
            success: p.success,
            focus_ring: p.focus_ring,
        }
    }

    /// Dark-mode preset.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Light-mode preset.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

/// Builder for a hyperlink whose URL is delivered to the activation callback.
#[must_use]
pub struct EnzLink<'a, Message> {
    label: String,
    url: String,
    on_activate: Box<dyn Fn(String) -> Message + 'a>,
    tone: LinkTone,
    size: LinkSize,
    weight: LinkWeight,
    decoration: LinkDecoration,
    enabled: bool,
    external: bool,
    external_glyph_size: Option<f32>,
    external_annotation: Option<String>,
    aria_label: Option<String>,
    description: Option<String>,
    tokens: Option<LinkTokens>,
    id: Option<iced::advanced::widget::Id>,
}

/// Create an enabled, anonymous link with lazy activation.
/// The callback receives the URL on pointer, keyboard or routed activation;
/// it is not called during view construction.
pub fn enz_link<'a, Message: 'a>(
    label: impl Into<String>,
    url: impl Into<String>,
    on_activate: impl Fn(String) -> Message + 'a,
) -> EnzLink<'a, Message> {
    EnzLink {
        label: label.into(),
        url: url.into(),
        on_activate: Box::new(on_activate),
        tone: LinkTone::default(),
        size: LinkSize::default(),
        weight: LinkWeight::default(),
        decoration: LinkDecoration::default(),
        enabled: true,
        external: false,
        external_glyph_size: None,
        external_annotation: None,
        aria_label: None,
        description: None,
        tokens: None,
        id: None,
    }
}

impl<'a, Message: Clone + 'a> EnzLink<'a, Message> {
    /// Sets the tone.
    pub fn tone(mut self, tone: LinkTone) -> Self {
        self.tone = tone;
        self
    }

    /// Sets the size.
    pub fn size(mut self, size: LinkSize) -> Self {
        self.size = size;
        self
    }

    /// Sets the font weight.
    pub fn weight(mut self, weight: LinkWeight) -> Self {
        self.weight = weight;
        self
    }

    /// Set the underline policy; Hover by default.
    /// Always includes disabled links. Hover requires the live pointer to be over
    /// an enabled link; a keyboard press or drag outside does not create an underline.
    pub fn decoration(mut self, decoration: LinkDecoration) -> Self {
        self.decoration = decoration;
        self
    }

    /// Sets whether the link is enabled.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Add an external-link glyph and accessible-name annotation when true.
    /// The glyph defaults to text size minus 2 px, with an 8 px floor.
    /// This changes presentation only; the caller still owns URL opening.
    pub fn external(mut self, external: bool) -> Self {
        self.external = external;
        self
    }

    /// Pin the external glyph size in logical pixels, with an 8 px floor.
    /// Non-finite values retain any prior pin. Without a pin, size derives from the
    /// label size. This applies only in external mode.
    pub fn external_glyph_size(mut self, size: f32) -> Self {
        if size.is_finite() {
            self.external_glyph_size = Some(size.max(8.0));
        }
        self
    }

    /// Set the accessible annotation appended only in external mode.
    /// Defaults to [`DEFAULT_EXTERNAL_ANNOTATION`]. Empty or whitespace-only values
    /// remove the annotation; a nonblank [`Self::aria_label`] overrides the whole name.
    pub fn external_annotation(mut self, annotation: impl Into<String>) -> Self {
        self.external_annotation = Some(annotation.into());
        self
    }

    /// Replace the complete accessible name verbatim, without an external annotation.
    /// Blank or whitespace-only names fall back to label-and-annotation composition.
    pub fn aria_label(mut self, name: impl Into<String>) -> Self {
        self.aria_label = Some(name.into());
        self
    }

    /// Set a caller-localized accessible description separate from the name.
    /// Blank or whitespace-only descriptions are omitted.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Pin tokens explicitly, bypassing runtime-theme derivation.
    pub fn tokens(mut self, tokens: LinkTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Assign a stable, unique identity for routed accessibility actions.
    /// Unset by default: anonymous links publish their name without advertised
    /// actions. URLs are not used as default identities because destinations can repeat.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Converts this builder into an iced `Element`.
    pub fn into_element(self) -> Element<'a, Message> {
        let tone = self.tone;
        let enabled = self.enabled;
        let pinned_tokens = self.tokens;
        let size = self.size;
        let weight = self.weight;
        let external = self.external;
        let glyph_size_override = self.external_glyph_size;
        let a11y_label = accessible_name(
            &self.label,
            external,
            self.external_annotation.as_deref(),
            self.aria_label.as_deref(),
        );
        let a11y_description = self.description.clone();
        let drawn_text = self.label.clone();
        let decoration = self.decoration;
        let label_font = Font {
            weight: weight.as_font_weight(),
            ..Font::DEFAULT
        };
        let label_widget = iced::widget::text(self.label)
            .font(label_font)
            .size(size.text_size());
        // The button writes status before its child glyph draws, keeping interaction ink aligned.
        let status_cell = std::rc::Rc::new(std::cell::Cell::new(FocusStatus::Active));
        let content: Element<'a, Message> = if external {
            let glyph_size =
                glyph_size_override.unwrap_or_else(|| (size.text_size() - 2.0).max(8.0));
            let icon_el = crate::icon::enz_icon("external-link")
                .size(glyph_size)
                .tint_with_status(
                    std::rc::Rc::clone(&status_cell),
                    move |theme: &iced::Theme, status| {
                        let tokens = pinned_tokens.unwrap_or_else(|| {
                            crate::theme_lookup::resolve(theme, LinkTokens::from_palette)
                        });
                        // The glyph applies disabled opacity below; avoid
                        // attenuating opaque ink twice.
                        link_fg(tone, status, true, &tokens)
                    },
                )
                .opacity(if enabled { 1.0 } else { 0.5 });
            iced::widget::row![label_widget, icon_el]
                .spacing(enzui_theme::SpacingScale::default().xs())
                .align_y(iced::Alignment::Center)
                .into()
        } else {
            label_widget.into()
        };
        let link_id = self.id;

        let url = self.url;
        let on_activate = self.on_activate;
        let activation: Option<Box<dyn Fn() -> Message + 'a>> = if enabled {
            Some(Box::new(move || (on_activate)(url.clone())))
        } else {
            None
        };

        let mut semantics =
            enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Link)
                .label(a11y_label)
                .disabled(!enabled)
                .default_action();
        if let Some(desc) = a11y_description
            && !desc.trim().is_empty()
        {
            semantics = semantics.description(desc);
        }

        let mut fb = crate::focus_button::focus_button_maybe(content, None::<Message>)
            .on_press_with_maybe(activation)
            .padding(Padding::ZERO)
            // Suppress drawn text even when localization changes the accessible name.
            .content_text(drawn_text)
            .semantics(semantics);
        if let Some(id) = link_id {
            fb = fb.id(id);
        }
        fb.underline(move |status: FocusStatus, pointer_over: bool| {
            decoration.underline_visible(status, pointer_over)
        })
        .underline_line_height(size.text_size() * 1.3)
        .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
            status_cell.set(status);
            let tokens = pinned_tokens
                .unwrap_or_else(|| crate::theme_lookup::resolve(theme, LinkTokens::from_palette));
            style_for(theme, tone, status, enabled, &tokens)
        })
        .into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzLink<'a, Message>> for Element<'a, Message> {
    fn from(link: EnzLink<'a, Message>) -> Self {
        link.into_element()
    }
}

fn tone_color(tone: LinkTone, tokens: &LinkTokens) -> Color {
    match tone {
        LinkTone::Accent => tokens.accent,
        LinkTone::Muted => tokens.text_muted,
        LinkTone::Danger => tokens.danger,
        LinkTone::Success => tokens.success,
    }
}
fn tone_hover_color(tone: LinkTone, tokens: &LinkTokens) -> Color {
    match tone {
        LinkTone::Accent => tokens.accent_hover,
        other => darken(tone_color(other, tokens), 0.12),
    }
}
fn link_fg(tone: LinkTone, status: FocusStatus, enabled: bool, tokens: &LinkTokens) -> Color {
    let base = tone_color(tone, tokens);
    if !enabled {
        return Color { a: 0.5, ..base };
    }
    match status {
        FocusStatus::Hovered | FocusStatus::Pressed => tone_hover_color(tone, tokens),
        _ => base,
    }
}

fn style_for(
    theme: &iced::Theme,
    tone: LinkTone,
    status: FocusStatus,
    enabled: bool,
    tokens: &LinkTokens,
) -> Style {
    use crate::focus_button::FocusRingTheme;

    let color = link_fg(tone, status, enabled, tokens);
    let border = if matches!(status, FocusStatus::Focused) && enabled {
        Border {
            color: tokens.focus_ring,
            width: theme.enz_focus_ring_width(),
            radius: 2.0.into(),
        }
    } else {
        Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 0.0.into(),
        }
    };

    Style {
        background: None,
        text_color: color,
        border,
        ..Style::default()
    }
}
