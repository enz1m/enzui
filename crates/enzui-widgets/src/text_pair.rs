//! Single-line leading text with a capped trailing string or element.
//!
//! String pairs end-elide both labels and omit an exactly empty trailing string
//! and its gap. Trailing width defaults to a 0px floor and 160px cap; the cap wins
//! conflicting floors. Narrow rows leave one requested leading em when possible,
//! unless the trailing floor requires it. Parent limits may compress either cell.
//!
//! Unpinned ink follows the live palette: primary leading and muted trailing.
//! Weights capture ambient Semibold and Regular at construction. Label sizes
//! default to UI-scaled 13px at composition and gaps to stock 8px. Forwarded
//! typography body size and spacing sm override these defaults; explicit setters
//! win. Invalid sizes fall back to stock 13px, gaps to stock 8px.
//!
//! Each cell clips sibling bleed and the whole row clips its outer bounds.
//! Labels announce by default; composing hosts can suppress them and publish
//! their own descriptor. [`EnzTextPairElement`] keeps its supplied child's
//! styling and semantics and does not reserve a leading em.

use crate::enz_text::{TextAlign, TextShaping, TextTokens, TextTone, enz_text};
use crate::max_width::max_width;
use enzui_theme::{Palette, SpacingScale, TypographyScale};
use iced::font::Weight;
use iced::widget::{container, row};
use iced::{Color, Element, Length};
/// Pinned ink for the string labels; the element form uses only leading_color.
#[derive(Debug, Clone, Copy)]
pub struct TextPairTokens {
    /// Leading-label ink.
    pub leading_color: Color,
    /// Trailing-string ink; ignored by the element form.
    pub trailing_color: Color,
}

impl TextPairTokens {
    /// Pin primary leading ink and muted trailing ink from the palette.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            leading_color: p.text,
            trailing_color: p.text_muted,
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
}
/// Stock label-size fallback in logical pixels: 13; unpinned defaults are UI-scaled.
pub const DEFAULT_TEXT_SIZE: f32 = 13.0;
/// Stock gap and invalid-gap fallback in logical pixels: 8.
pub const DEFAULT_SPACING: f32 = 8.0;
/// Default trailing width floor in logical pixels: 0.
pub const DEFAULT_TRAILING_MIN_WIDTH: f32 = 0.0;
/// Default trailing width cap in logical pixels: 160.
pub const DEFAULT_TRAILING_MAX_WIDTH: f32 = 160.0;
#[inline]
fn sane(v: f32, fallback: f32) -> f32 {
    if v.is_finite() && v >= 0.0 {
        v
    } else {
        fallback
    }
}
#[inline]
fn sane_font_size(v: f32, fallback: f32) -> f32 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        fallback
    }
}
/// Single-line leading and trailing strings with responsive trailing reservation.
#[derive(Debug, Clone)]
#[must_use]
pub struct EnzTextPair {
    leading: String,
    trailing: String,
    leading_size: f32,
    trailing_size: f32,
    leading_weight: Weight,
    trailing_weight: Weight,
    shaping: TextShaping,
    leading_tone: TextTone,
    trailing_tone: TextTone,
    spacing: f32,
    trailing_min_width: f32,
    trailing_max_width: f32,
    announce: bool,
    tokens: Option<TextPairTokens>,
    leading_size_explicit: bool,
    trailing_size_explicit: bool,
    spacing_explicit: bool,
    typography: Option<TypographyScale>,
    spacing_scale: Option<SpacingScale>,
}

impl EnzTextPair {
    /// Pin leading size in logical pixels, overriding forwarded typography.
    /// Nonpositive or non-finite input pins stock 13px.
    pub fn leading_size(mut self, size: f32) -> Self {
        self.leading_size = sane_font_size(size, DEFAULT_TEXT_SIZE);
        self.leading_size_explicit = true;
        self
    }
    /// Pin trailing size in logical pixels, overriding forwarded typography.
    /// Nonpositive or non-finite input pins stock 13px.
    pub fn trailing_size(mut self, size: f32) -> Self {
        self.trailing_size = sane_font_size(size, DEFAULT_TEXT_SIZE);
        self.trailing_size_explicit = true;
        self
    }
    /// Pin leading weight; the default captures ambient Semibold at construction.
    pub fn leading_weight(mut self, weight: Weight) -> Self {
        self.leading_weight = weight;
        self
    }
    /// Pin trailing weight; the default captures ambient Regular at construction.
    pub fn trailing_weight(mut self, weight: Weight) -> Self {
        self.trailing_weight = weight;
        self
    }
    /// Set shaping for both text labels.
    pub fn shaping(mut self, shaping: impl Into<TextShaping>) -> Self {
        self.shaping = shaping.into();
        self
    }
    /// Pin the logical gap, overriding forwarded spacing; invalid input uses stock 8px.
    /// String pairs omit the gap when trailing text is exactly empty.
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self.spacing_explicit = true;
        self
    }
    /// Forward body size for unpinned generated labels; explicit size setters win.
    /// Without forwarding, label sizes use UI-scaled stock 13px.
    pub fn typography(mut self, typography: TypographyScale) -> Self {
        self.typography = Some(typography);
        self
    }
    /// Forward sm spacing for an unpinned gap; explicit spacing wins.
    /// Without forwarding, the gap is stock 8px.
    pub fn spacing_scale(mut self, spacing: SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }
    /// Select live leading ink when colour tokens are unpinned.
    pub fn leading_tone(mut self, tone: TextTone) -> Self {
        self.leading_tone = tone;
        self
    }
    /// Select live trailing-string ink when colour tokens are unpinned.
    pub fn trailing_tone(mut self, tone: TextTone) -> Self {
        self.trailing_tone = tone;
        self
    }
    /// Set a logical trailing-width floor, default 0, bounded by the cap and parent.
    /// Negative or non-finite input uses stock 0px.
    pub fn trailing_min_width(mut self, w: f32) -> Self {
        self.trailing_min_width = sane(w, DEFAULT_TRAILING_MIN_WIDTH);
        self
    }
    /// Set a logical trailing-width cap, default 160; zero is a real cap.
    /// The cap wins conflicting floors. Negative or non-finite input uses stock 160px.
    pub fn trailing_max_width(mut self, w: f32) -> Self {
        self.trailing_max_width = sane(w, DEFAULT_TRAILING_MAX_WIDTH);
        self
    }
    /// Pin both label colours, overriding live tones.
    pub fn tokens(mut self, tokens: TextPairTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }
    /// Publish both text labels by default; false suppresses them without changing layout.
    /// A composing host must publish its own descriptor when these labels are suppressed.
    pub fn announce(mut self, announce: bool) -> Self {
        self.announce = announce;
        self
    }
    /// Compose a responsive row that recomputes reservations for its allocated width.
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        crate::responsive::enz_responsive(move |allocation| {
            self.clone().into_element_at_width(allocation.width)
        })
        .width(Length::Fill)
        .height(Length::Shrink)
        .into()
    }

    fn into_element_at_width<'a, Message: 'a>(self, width: f32) -> Element<'a, Message> {
        let leading_px = if self.leading_size_explicit {
            self.leading_size
        } else {
            self.typography.map_or_else(
                || enzui_theme::text_scale::ui_px(DEFAULT_TEXT_SIZE),
                |t| t.body_size,
            )
        };
        let trailing_px = if self.trailing_size_explicit {
            self.trailing_size
        } else {
            self.typography.map_or_else(
                || enzui_theme::text_scale::ui_px(DEFAULT_TEXT_SIZE),
                |t| t.body_size,
            )
        };
        let spacing_px = if self.spacing_explicit {
            self.spacing
        } else {
            self.spacing_scale.map_or(DEFAULT_SPACING, |s| s.sm())
        };
        let leading_size = sane_font_size(leading_px, DEFAULT_TEXT_SIZE);
        let trailing_size = sane_font_size(trailing_px, DEFAULT_TEXT_SIZE);
        let spacing = sane(spacing_px, DEFAULT_SPACING);
        let max_w = sane(self.trailing_max_width, DEFAULT_TRAILING_MAX_WIDTH);
        // Clamp the floor first so it cannot defeat the cap.
        let min_w = sane(self.trailing_min_width, DEFAULT_TRAILING_MIN_WIDTH).min(max_w);
        // Reserve a leading em unless the explicit metadata floor requires that space.
        let max_w = max_w.min((width - spacing - leading_size).max(min_w));
        let has_trailing = !self.trailing.is_empty();
        let pinned = self.tokens;

        let announce = self.announce;
        let mut leading = enz_text(self.leading)
            .size_px(leading_size)
            .weight(self.leading_weight)
            .shaping(self.shaping)
            .align(TextAlign::Left)
            .single_line_elided()
            .announce(announce)
            .full_width();
        leading = match pinned {
            None => leading.tone(self.leading_tone),
            Some(t) => leading.tone(TextTone::Default).tokens(TextTokens {
                default_color: t.leading_color,
                ..TextTokens::dark()
            }),
        };
        // Cell clips prevent sibling bleed; the outer clip bounds the whole row.
        let leading_cell = container(leading).width(Length::Fill).clip(true);
        if !has_trailing {
            return container(row![leading_cell].width(Length::Fill))
                .clip(true)
                .into();
        }
        let mut trailing = enz_text(self.trailing)
            .size_px(trailing_size)
            .weight(self.trailing_weight)
            .shaping(self.shaping)
            .align(TextAlign::Right)
            .single_line_elided()
            .announce(announce)
            .width(Length::Shrink.min(min_w));
        trailing = match pinned {
            None => trailing.tone(self.trailing_tone),
            Some(t) => trailing.tone(TextTone::Default).tokens(TextTokens {
                default_color: t.trailing_color,
                ..TextTokens::dark()
            }),
        };
        let trailing_cell: Element<'a, Message> =
            container(max_width(Element::from(trailing), max_w))
                .clip(true)
                .into();

        let inner = row![leading_cell, trailing_cell]
            .spacing(spacing)
            .align_y(iced::Alignment::Center)
            .width(Length::Fill);
        container(inner).clip(true).into()
    }
}

impl<'a, Message: 'a> From<EnzTextPair> for Element<'a, Message> {
    fn from(pair: EnzTextPair) -> Self {
        pair.into_element()
    }
}
/// Leading label with a capped trailing element and separate cell clips.
///
/// The trailing child retains its typography, ink and semantics. It may wrap or
/// grow vertically; the pair clips it without installing text elision. The child
/// is always present with a gap, even when empty. Unlike the string form, this
/// form does not reserve a leading em. Floors remain subject to cap and parent.
#[must_use]
pub struct EnzTextPairElement<'a, Message: 'a> {
    leading: String,
    trailing: Element<'a, Message>,
    leading_size: f32,
    leading_weight: Weight,
    shaping: TextShaping,
    leading_tone: TextTone,
    spacing: f32,
    trailing_min_width: f32,
    trailing_max_width: f32,
    announce: bool,
    tokens: Option<TextPairTokens>,
    leading_size_explicit: bool,
    spacing_explicit: bool,
    typography: Option<TypographyScale>,
    spacing_scale: Option<SpacingScale>,
}

impl<'a, Message: 'a> EnzTextPairElement<'a, Message> {
    /// Pin leading size in logical pixels, overriding forwarded typography.
    /// Nonpositive or non-finite input pins stock 13px.
    pub fn leading_size(mut self, size: f32) -> Self {
        self.leading_size = sane_font_size(size, DEFAULT_TEXT_SIZE);
        self.leading_size_explicit = true;
        self
    }
    /// Pin leading weight; the default captures ambient Semibold at construction.
    pub fn leading_weight(mut self, weight: Weight) -> Self {
        self.leading_weight = weight;
        self
    }
    /// Set shaping for the generated text runs; supplied trailing elements are unaffected.
    pub fn shaping(mut self, shaping: impl Into<TextShaping>) -> Self {
        self.shaping = shaping.into();
        self
    }
    /// Select live leading ink when colour tokens are unpinned.
    pub fn leading_tone(mut self, tone: TextTone) -> Self {
        self.leading_tone = tone;
        self
    }
    /// Pin the logical gap, overriding forwarded spacing; invalid input uses stock 8px.
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = spacing;
        self.spacing_explicit = true;
        self
    }
    /// Forward body size for unpinned generated labels; explicit size setters win.
    /// Without forwarding, label sizes use UI-scaled stock 13px.
    pub fn typography(mut self, typography: TypographyScale) -> Self {
        self.typography = Some(typography);
        self
    }
    /// Forward sm spacing for an unpinned gap; explicit spacing wins.
    /// Without forwarding, the gap is stock 8px.
    pub fn spacing_scale(mut self, spacing: SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }
    /// Set a logical trailing-width floor, default 0, bounded by the cap and parent.
    /// Negative or non-finite input uses stock 0px.
    pub fn trailing_min_width(mut self, w: f32) -> Self {
        self.trailing_min_width = sane(w, DEFAULT_TRAILING_MIN_WIDTH);
        self
    }
    /// Set a logical trailing-width cap, default 160; zero is a real cap.
    /// The cap wins conflicting floors. Negative or non-finite input uses stock 160px.
    pub fn trailing_max_width(mut self, w: f32) -> Self {
        self.trailing_max_width = sane(w, DEFAULT_TRAILING_MAX_WIDTH);
        self
    }
    /// Pin leading ink only; the trailing element keeps its own styling.
    pub fn tokens(mut self, tokens: TextPairTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }
    /// Publish the leading label by default; false leaves trailing semantics unaffected.
    pub fn announce(mut self, announce: bool) -> Self {
        self.announce = announce;
        self
    }
    /// Compose the row, resolving generated-label size and gap from forwarded scales.
    pub fn into_element(self) -> Element<'a, Message> {
        let leading_px = if self.leading_size_explicit {
            self.leading_size
        } else {
            self.typography.map_or_else(
                || enzui_theme::text_scale::ui_px(DEFAULT_TEXT_SIZE),
                |t| t.body_size,
            )
        };
        let spacing_px = if self.spacing_explicit {
            self.spacing
        } else {
            self.spacing_scale.map_or(DEFAULT_SPACING, |s| s.sm())
        };
        let leading_size = sane_font_size(leading_px, DEFAULT_TEXT_SIZE);
        let spacing = sane(spacing_px, DEFAULT_SPACING);
        let max_w = sane(self.trailing_max_width, DEFAULT_TRAILING_MAX_WIDTH);
        // Clamp the floor first so it cannot defeat the cap.
        let min_w = sane(self.trailing_min_width, DEFAULT_TRAILING_MIN_WIDTH).min(max_w);
        let pinned = self.tokens;
        let announce = self.announce;
        let mut leading = enz_text(self.leading)
            .size_px(leading_size)
            .weight(self.leading_weight)
            .shaping(self.shaping)
            .align(TextAlign::Left)
            .single_line_elided()
            .announce(announce)
            .full_width();
        leading = match pinned {
            None => leading.tone(self.leading_tone),
            Some(t) => leading.tone(TextTone::Default).tokens(TextTokens {
                default_color: t.leading_color,
                ..TextTokens::dark()
            }),
        };
        // Cell clips prevent sibling bleed; the outer clip bounds the whole row.
        let leading_cell = container(leading).width(Length::Fill).clip(true);
        let trailing_cell: Element<'a, Message> = container(max_width(self.trailing, max_w))
            .width(Length::Shrink.min(min_w))
            .clip(true)
            .into();

        let inner = row![leading_cell, trailing_cell]
            .spacing(spacing)
            .align_y(iced::Alignment::Center)
            .width(Length::Fill);
        container(inner).clip(true).into()
    }
}

impl<'a, Message: 'a> From<EnzTextPairElement<'a, Message>> for Element<'a, Message> {
    fn from(pair: EnzTextPairElement<'a, Message>) -> Self {
        pair.into_element()
    }
}
/// Construct a leading label with a capped, clipped trailing element.
/// The supplied element retains its styling and semantics; see [`EnzTextPairElement`].
pub fn enz_text_pair_element<'a, L: Into<String>, Message: 'a>(
    leading: L,
    trailing: Element<'a, Message>,
) -> EnzTextPairElement<'a, Message> {
    EnzTextPairElement {
        leading: leading.into(),
        trailing,
        leading_size: DEFAULT_TEXT_SIZE,
        leading_weight: enzui_theme::text_defaults::weight(enzui_theme::WeightRung::Semibold),
        shaping: TextShaping::default(),
        leading_tone: TextTone::Default,
        spacing: DEFAULT_SPACING,
        trailing_min_width: DEFAULT_TRAILING_MIN_WIDTH,
        trailing_max_width: DEFAULT_TRAILING_MAX_WIDTH,
        announce: true,
        tokens: None,
        leading_size_explicit: false,
        spacing_explicit: false,
        typography: None,
        spacing_scale: None,
    }
}
/// Construct two single-line labels with live ink and default geometry.
pub fn enz_text_pair<L: Into<String>, T: Into<String>>(leading: L, trailing: T) -> EnzTextPair {
    EnzTextPair {
        leading: leading.into(),
        trailing: trailing.into(),
        leading_size: DEFAULT_TEXT_SIZE,
        trailing_size: DEFAULT_TEXT_SIZE,
        leading_weight: enzui_theme::text_defaults::weight(enzui_theme::WeightRung::Semibold),
        trailing_weight: enzui_theme::text_defaults::weight(enzui_theme::WeightRung::Regular),
        shaping: TextShaping::default(),
        leading_tone: TextTone::Default,
        trailing_tone: TextTone::Muted,
        spacing: DEFAULT_SPACING,
        trailing_min_width: DEFAULT_TRAILING_MIN_WIDTH,
        trailing_max_width: DEFAULT_TRAILING_MAX_WIDTH,
        announce: true,
        tokens: None,
        leading_size_explicit: false,
        trailing_size_explicit: false,
        spacing_explicit: false,
        typography: None,
        spacing_scale: None,
    }
}
