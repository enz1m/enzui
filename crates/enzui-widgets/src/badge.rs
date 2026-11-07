//! Inline status and count labels with six tones, three sizes and optional icons.
//!
//! Default badges use the medium preset and neutral ink. Unpinned colours
//! follow the live palette; size tokens override presets in either builder order.
//! Small text uses ambient label size, medium text stock 12px and large stock 14px.
//! Labels use ambient Semibold, end elision and a shared line box. Preferred
//! height grows to fit that line box or a visible icon, subject to parent limits.
//! Minimum width allows natural content growth but cannot defeat parent bounds.
//!
//! Non-neutral tones publish a severity group around the label; neutral badges
//! publish just the label. Icons share label ink and zero icon size hides them.

use crate::enz_text::enz_text;
use enzui_core::metrics::{FiniteNonNeg, FinitePositive};
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::{Palette, TypographyScale};
use iced::widget::container;
use iced::widget::text::LineHeight;
use iced::{Border, Color, Element, Length, Padding};

/// Semantic tone for the badge color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BadgeTone {
    /// Neutral (surface-alt background, text foreground).
    #[default]
    Default,
    /// Blue accent (soft tint: `accent_soft` fill, `accent_pressed` ink).
    Accent,
    /// Opaque accent badge with its paired foreground.
    AccentStrong,
    /// Green success.
    Success,
    /// Amber warning.
    Warning,
    /// Red error/danger.
    Error,
}

impl BadgeTone {
    /// Accessible severity vocabulary; neutral returns None and both accent tones use Information.
    pub fn severity_word(self) -> Option<String> {
        match self {
            BadgeTone::Default => None,
            BadgeTone::Accent | BadgeTone::AccentStrong => {
                Some(enzui_core::localization::text("enzui-badge-information"))
            }
            BadgeTone::Success => Some(enzui_core::localization::text("enzui-badge-success")),
            BadgeTone::Warning => Some(enzui_core::localization::text("enzui-badge-warning")),
            BadgeTone::Error => Some(enzui_core::localization::text("enzui-badge-error")),
        }
    }
}

/// Badge size presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BadgeSize {
    /// 18px preferred height; text uses ambient label size.
    Sm,
    /// 24px height, 12px text (default).
    #[default]
    Md,
    /// 30px height, 14px text.
    Lg,
    // Unpinned corners take the theme's `shape.badge` step (a pill by default).
}

impl BadgeSize {
    /// Returns the badge height.
    pub fn height(self) -> f32 {
        match self {
            Self::Sm => 18.0,
            Self::Md => 24.0,
            Self::Lg => 30.0,
        }
    }

    /// Label size in logical pixels: the default label rung for `Sm`, 12 for
    /// `Md`, and 14 for `Lg`. The larger presets have badge-specific metrics.
    pub fn text_size(self) -> f32 {
        match self {
            Self::Sm => TypographyScale::ambient().label_size,
            Self::Md => 12.0,
            Self::Lg => 14.0,
        }
    }

    /// Returns the border radius.
    pub fn radius(self) -> f32 {
        match self {
            Self::Sm => 9.0,
            Self::Md => 12.0,
            Self::Lg => 15.0,
        }
    }

    /// Returns the horizontal padding.
    pub fn padding_x(self) -> f32 {
        match self {
            Self::Sm => 7.0,
            Self::Md => 10.0,
            Self::Lg => 13.0,
        }
    }

    /// Rendered label line-box height in logical pixels, using the shared text metric.
    pub fn line_height_px(self) -> f32 {
        crate::enz_text::line_box_px(self.text_size())
    }

    /// Logical vertical padding around the shared line box, floored at zero.
    /// A line box taller than the preferred height requires a taller badge.
    pub fn padding_y(self) -> f32 {
        ((self.height() - self.line_height_px()) / 2.0).max(0.0)
    }

    /// minimum-width floor: `badge-height + 8px`, so short counts (e.g.
    /// "1") don't collapse narrower than the design minimum.
    pub fn min_width(self) -> f32 {
        self.height() + 8.0
    }
}

/// Badge geometry in logical pixels, independent of palette colours.
///
/// Defaults match [`BadgeSize::Md`]. [`EnzBadge::size_tokens`] pins these values
/// regardless of `.size(...)` order. Invalid values fall back to the medium
/// preset; height and text size must be positive, while other fields allow zero.
/// The effective height is at least the label line box and any visible icon's
/// height, subject to the parent's bounds. Vertical padding centres that content.
#[derive(Debug, Clone, Copy)]
pub struct BadgeSizeTokens {
    /// Preferred pill height; default 24.
    pub height: f32,
    /// Label size; default 12.
    pub text_size: f32,
    /// Pill corner radius; default 12, half the height. Zero produces square corners.
    pub radius: f32,
    /// Padding on each horizontal edge; default 10.
    pub padding_x: f32,
    /// Minimum outer width; default 32. Zero allows the natural content width.
    pub min_width: f32,
    /// Leading glyph size; default 12. Zero hides the glyph.
    pub icon_size: f32,
    /// Gap between a visible glyph and its label; default 4.
    pub icon_gap: f32,
}

impl Default for BadgeSizeTokens {
    fn default() -> Self {
        Self::from_size(BadgeSize::Md)
    }
}

impl BadgeSizeTokens {
    /// Seed an override from the existing badge size ladder.
    pub fn from_size(size: BadgeSize) -> Self {
        Self {
            height: size.height(),
            text_size: size.text_size(),
            radius: size.radius(),
            padding_x: size.padding_x(),
            min_width: size.min_width(),
            icon_size: size.text_size(),
            icon_gap: enzui_theme::SpacingScale::default().xs(),
        }
    }

    fn sanitized(self) -> Self {
        let defaults = Self::default();
        Self {
            height: FinitePositive::new_or(self.height, defaults.height).get(),
            text_size: FinitePositive::new_or(self.text_size, defaults.text_size).get(),
            radius: FiniteNonNeg::new_or(self.radius, defaults.radius).get(),
            padding_x: FiniteNonNeg::new_or(self.padding_x, defaults.padding_x).get(),
            min_width: FiniteNonNeg::new_or(self.min_width, defaults.min_width).get(),
            icon_size: FiniteNonNeg::new_or(self.icon_size, defaults.icon_size).get(),
            icon_gap: FiniteNonNeg::new_or(self.icon_gap, defaults.icon_gap).get(),
        }
    }
}

/// Color tokens for badges, derived from the palette.
#[derive(Debug, Clone, Copy)]
pub struct BadgeTokens {
    /// Default tone background.
    pub default_bg: Color,
    /// Default tone text.
    pub default_fg: Color,
    /// Accent tone background (soft).
    pub accent_bg: Color,
    /// Accent tone text.
    pub accent_fg: Color,
    /// Opaque accent background resolved for its paired foreground.
    pub accent_strong_bg: Color,
    /// Foreground paired with the opaque accent background.
    pub accent_strong_fg: Color,
    /// Success tone background (soft).
    pub success_bg: Color,
    /// Success tone text.
    pub success_fg: Color,
    /// Warning tone background (soft).
    pub warning_bg: Color,
    /// Warning tone text.
    pub warning_fg: Color,
    /// Error tone background (soft).
    pub error_bg: Color,
    /// Error tone text.
    pub error_fg: Color,
}

impl BadgeTokens {
    /// Resolve badge fills and ink from the live semantic palette.
    pub fn from_palette(p: &Palette) -> Self {
        let strong = p.filled_color(p.accent);
        Self {
            default_bg: p.surface_alt,
            default_fg: p.text,
            accent_bg: p.accent_soft,
            accent_fg: Palette::with_contrast_on(p.accent_soft, p.accent_pressed, 4.5),
            accent_strong_bg: strong.color,
            accent_strong_fg: strong.text,
            success_bg: p.success_soft,
            success_fg: p
                .tone_on(enzui_theme::SemanticTone::Success, p.success_soft)
                .color(),
            warning_bg: p.warning_soft,
            warning_fg: p
                .tone_on(enzui_theme::SemanticTone::Warning, p.warning_soft)
                .color(),
            error_bg: p.danger_soft,
            error_fg: p
                .tone_on(enzui_theme::SemanticTone::Error, p.danger_soft)
                .color(),
        }
    }

    /// Pin the stock dark palette.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Pin the stock light palette.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Return fill and label ink for the selected tone.
    pub fn colors_for_tone(&self, tone: BadgeTone) -> (Color, Color) {
        match tone {
            BadgeTone::Default => (self.default_bg, self.default_fg),
            BadgeTone::Accent => (self.accent_bg, self.accent_fg),
            BadgeTone::AccentStrong => (self.accent_strong_bg, self.accent_strong_fg),
            BadgeTone::Success => (self.success_bg, self.success_fg),
            BadgeTone::Warning => (self.warning_bg, self.warning_fg),
            BadgeTone::Error => (self.error_bg, self.error_fg),
        }
    }
}

/// The badge's accessibility node: a group labelled with its tone ("error
/// badge"), since colour alone carries severity. The badge text already
/// reaches the tree through its drawn text and is not repeated. `Default`
/// returns `None`.
#[must_use]
pub fn badge_semantics(tone: BadgeTone) -> Option<Semantics> {
    let word = tone.severity_word()?;
    Some(
        Semantics::new(Role::Group)
            .label(enzui_core::localized!("enzui-badge-badge", "label" => word.to_lowercase())),
    )
}

/// Builder for an EnzBadge element.
#[must_use]
pub struct EnzBadge {
    material: Option<enzui_theme::MaterialStyle>,
    label: String,
    tone: BadgeTone,
    size: BadgeSize,
    size_tokens: Option<BadgeSizeTokens>,
    tokens: Option<BadgeTokens>,
    icon: String,
}

/// Construct a neutral medium badge with live ink and no icon.
pub fn enz_badge(label: &str) -> EnzBadge {
    EnzBadge {
        material: None,
        label: label.to_owned(),
        tone: BadgeTone::default(),
        size: BadgeSize::default(),
        size_tokens: None,
        tokens: None,
        icon: String::new(),
    }
}

impl EnzBadge {
    /// Set the semantic tone.
    pub fn tone(mut self, tone: BadgeTone) -> Self {
        self.tone = tone;
        self
    }

    /// Set the size variant.
    pub fn size(mut self, size: BadgeSize) -> Self {
        self.size = size;
        self
    }

    /// Pin badge geometry, taking precedence over `.size(...)` in either order.
    ///
    /// Colours continue to follow the runtime theme unless `.tokens(...)` is set.
    /// See [`BadgeSizeTokens`] for units, defaults and invalid-value handling.
    pub fn size_tokens(mut self, tokens: BadgeSizeTokens) -> Self {
        self.size_tokens = Some(tokens);
        self
    }

    /// Pin all tone colours while leaving geometry independent.
    pub fn tokens(mut self, tokens: BadgeTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Set a leading catalog icon tinted with label ink; empty names or zero size omit it.
    pub fn icon(mut self, name: impl Into<String>) -> Self {
        self.icon = name.into();
        self
    }

    /// Wrap the badge in the supplied liquid glass style.
    /// The wrapper radius follows the size preset; size_tokens radius changes only the pill.
    pub fn material(mut self, style: enzui_theme::MaterialStyle) -> Self {
        self.material = Some(style);
        self
    }

    /// Build the element. Tokens track the runtime `iced::Theme` unless
    /// pinned via `.tokens()`.
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        let material = self.material;
        let radius = self.size.radius();
        crate::glass::wrap(self.build_element(), material, radius)
    }

    fn build_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        let pinned_tokens = self.tokens;
        let tone = self.tone;
        let pinned_radius = self.size_tokens.is_some();
        let sz = self
            .size_tokens
            .unwrap_or_else(|| BadgeSizeTokens::from_size(self.size))
            .sanitized();
        let icon_name = self.icon;
        let has_icon = !icon_name.is_empty() && sz.icon_size > 0.0;
        let line_height = crate::enz_text::line_box_px(sz.text_size);
        let content_height = line_height.max(if has_icon { sz.icon_size } else { 0.0 });
        let height = sz.height.max(content_height);
        let padding_y = (height - content_height) / 2.0;

        let resolve = move |theme: &iced::Theme| -> BadgeTokens {
            crate::theme_lookup::resolve_pinned(
                crate::theme_lookup::PIN_SITE_BADGE,
                pinned_tokens,
                theme,
                BadgeTokens::from_palette,
            )
        };

        let label = enz_text(self.label)
            .size_px(sz.text_size)
            .line_height(LineHeight::Absolute(line_height.into()))
            .weight_rung(enzui_theme::WeightRung::Semibold)
            .single_line_elided()
            .color_resolved(move |theme: &iced::Theme| resolve(theme).colors_for_tone(tone).1);

        let content: Element<'a, Message> = if !has_icon {
            label.into()
        } else {
            let icon_el = crate::icon::enz_icon(&icon_name)
                .size(sz.icon_size)
                .tint_resolved(move |theme: &iced::Theme| resolve(theme).colors_for_tone(tone).1);
            iced::widget::row![icon_el, label]
                .spacing(sz.icon_gap)
                .align_y(iced::Alignment::Center)
                .into()
        };

        let min_content_width = (sz.min_width - 2.0 * sz.padding_x).max(0.0);
        let padded_content = crate::min_width::min_width(
            iced::widget::container(content).align_x(iced::alignment::Horizontal::Center),
            min_content_width,
        );

        let pill: Element<'a, Message> = container(padded_content)
            .padding(
                Padding::new(padding_y)
                    .left(sz.padding_x)
                    .right(sz.padding_x),
            )
            .height(Length::Fixed(height))
            .align_y(iced::alignment::Vertical::Center)
            .align_x(iced::alignment::Horizontal::Center)
            .style(move |theme: &iced::Theme| container::Style {
                // The baseline is already hinted; fill-only snapping shifts fractional rows away from their ink.
                snap: false,
                background: Some(resolve(theme).colors_for_tone(tone).0.into()),
                border: Border {
                    radius: if pinned_radius {
                        sz.radius
                    } else {
                        crate::theme_lookup::shape_radius(theme, |shape| shape.badge)
                    }
                    .into(),
                    width: 0.0,
                    color: Color::TRANSPARENT,
                },
                ..Default::default()
            })
            .into();
        // The severity group must retain the label as a separate text node.
        match badge_semantics(tone) {
            Some(semantics) => {
                crate::semantics_container::SemanticsContainer::new(pill, semantics).into()
            }
            None => pill,
        }
    }
}

impl<'a, Message: 'a> From<EnzBadge> for Element<'a, Message> {
    fn from(badge: EnzBadge) -> Self {
        badge.into_element()
    }
}
