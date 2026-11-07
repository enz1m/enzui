//! Single-line caption bands for form groups and sidebar sections.
//!
//! Titles retain their supplied case. Size captures the ambient label typography
//! at construction; weight uses the ambient Semibold rung when composed. The
//! band adds 8 logical pixels to the title size and fills its slot by default.
//! Unpinned ink follows the live palette, defaulting to Muted.
//!
//! Titles publish one level-2 heading by default and participate in elision
//! auditing. [`EnzSectionHeader::sticky_on`] paints a backdrop while keeping
//! the caption in its section's scroll viewport.

use crate::enz_text::{TextTokens, TextTone, enz_text};
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::{Palette, SpacingScale, TypographyScale};
use iced::widget::container;
use iced::widget::text::LineHeight;
use iced::{Color, Element, Length};

/// Default accessibility heading level: 2.
pub const DEFAULT_HEADING_LEVEL: usize = 2;

/// Return the ambient label size in logical pixels.
pub fn default_title_size() -> f32 {
    TypographyScale::ambient().label_size
}

/// Stock label size in logical pixels: 11; ambient typography may differ.
pub const DEFAULT_TITLE_SIZE: f32 = 11.0;

/// Return the stock vertical band allowance in logical pixels: 8.
/// This does not read ambient or forwarded spacing.
pub fn band_padding() -> f32 {
    SpacingScale::default().sm()
}

/// Stock vertical band allowance in logical pixels: 8.
pub const BAND_PADDING: f32 = 8.0;

/// Logical pixels added to title size for the requested line box: 5.
/// The composed text applies its supported line-box limits.
pub const LINE_HEIGHT_EXTRA: f32 = 5.0;

fn sane(v: f32, fallback: f32) -> f32 {
    if v.is_finite() && v > 0.0 {
        v
    } else {
        fallback
    }
}

/// Explicit caption ink, overriding tone and backdrop-based resolution.
#[derive(Debug, Clone, Copy)]
pub struct SectionHeaderTokens {
    /// Pinned caption ink.
    pub text_color: Color,
}

impl SectionHeaderTokens {
    /// Pin the supplied palette's muted ink.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            text_color: p.text_muted,
        }
    }

    /// Pin the stock dark palette's muted ink.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Pin the stock light palette's muted ink.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

/// Caption builder with optional heading semantics and sticky presentation.
#[must_use]
pub struct EnzSectionHeader<'a, Message> {
    title: String,
    title_size: f32,
    tokens: Option<SectionHeaderTokens>,
    tone: TextTone,
    subtle_backdrop: Option<Color>,
    width: Length,
    announce: bool,
    audit_elision: bool,
    heading_level: usize,
    sticky_backdrop: Option<Color>,
    _phantom: std::marker::PhantomData<&'a Message>,
}

/// Own the supplied title with ambient label size, Muted ink and level-2 heading semantics.
pub fn enz_section_header<'a, Message>(title: impl Into<String>) -> EnzSectionHeader<'a, Message> {
    EnzSectionHeader {
        title: title.into(),
        title_size: default_title_size(),
        tokens: None,
        tone: TextTone::Muted,
        subtle_backdrop: None,
        width: Length::Fill,
        announce: true,
        audit_elision: false,
        heading_level: DEFAULT_HEADING_LEVEL,
        sticky_backdrop: None,
        _phantom: std::marker::PhantomData,
    }
}

impl<'a, Message: 'a> EnzSectionHeader<'a, Message> {
    /// Set the title size in logical pixels. Non-finite or nonpositive values retain the prior size.
    pub fn title_size(mut self, size: f32) -> Self {
        self.title_size = sane(size, self.title_size);
        self
    }

    /// Pin caption ink instead of resolving the live tone or backdrop.
    pub fn tokens(mut self, tokens: SectionHeaderTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Select live palette ink; defaults to Muted. Explicit tokens take precedence.
    pub fn tone(mut self, tone: TextTone) -> Self {
        self.tone = tone;
        self
    }

    /// Resolve Subtle ink against the supplied backdrop using the palette contrast policy.
    /// This does not paint the backdrop. Explicit tokens take precedence; selecting
    /// another tone leaves the backdrop dormant until Subtle is selected again.
    pub fn subtle_on(mut self, backdrop: Color) -> Self {
        self.tone = TextTone::Subtle;
        self.subtle_backdrop = Some(backdrop);
        self
    }

    /// Keep the caption within its section's viewport and paint the supplied backdrop.
    /// Place it first in a separate column containing its section's rows: the parent
    /// bounds limit sticking. Headers in one flat column share those bounds.
    /// The heading retains its position before the rows in the accessibility tree.
    pub fn sticky_on(mut self, backdrop: Color) -> Self {
        self.sticky_backdrop = Some(backdrop);
        self
    }

    /// Set the band width policy; defaults to Fill. Shrink hugs the constrained label.
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Publish one heading for the title; defaults to true.
    /// False suppresses all title semantics while retaining rendered text.
    pub fn announce(mut self, announce: bool) -> Self {
        self.announce = announce;
        self
    }

    /// Request title elision auditing.
    /// The composed single-line text already enables auditing, so false does not
    /// disable it and the default header is audited without this call.
    pub fn audit_elision(mut self, on: bool) -> Self {
        self.audit_elision = on;
        self
    }

    /// Set the accessibility heading level; defaults to 2. Zero is coerced to 1.
    pub fn heading_level(mut self, level: usize) -> Self {
        self.heading_level = level.max(1);
        self
    }

    /// Return the requested line-box height: title size plus 5 logical pixels.
    /// The composed text may clamp this to its supported range.
    pub fn reserved_line_height(&self) -> f32 {
        self.title_size + LINE_HEIGHT_EXTRA
    }

    /// Return the fixed band height: title size plus 8 logical pixels.
    pub fn band_height(&self) -> f32 {
        self.title_size + band_padding()
    }

    /// Resolve caption ink using pin, backdrop-aware Subtle, then palette tone precedence.
    pub fn resolved_text_color(&self, palette: &Palette) -> Color {
        match (self.tokens, self.tone, self.subtle_backdrop) {
            (Some(t), _, _) => t.text_color,
            (None, TextTone::Subtle, Some(backdrop)) => palette.text_subtle_on(backdrop),
            (None, _, _) => TextTokens::from_palette(palette).color_for(self.tone),
        }
    }
    fn build_label(&self) -> crate::enz_text::EnzText {
        let title_size = self.title_size;
        let line_height = title_size + LINE_HEIGHT_EXTRA;
        // The outer band owns width policy; its limits constrain the full-width label.
        let mut label = enz_text(self.title.clone())
            .size_px(title_size)
            .weight_rung(enzui_theme::WeightRung::Semibold)
            .single_line_elided()
            .full_width()
            .line_height(LineHeight::Absolute(line_height.into()));
        label = match (self.tokens, self.tone, self.subtle_backdrop) {
            (Some(t), _, _) => label.tokens(TextTokens {
                default_color: t.text_color,
                muted_color: t.text_color,
                subtle_color: t.text_color,
                accent_color: t.text_color,
                success_color: t.text_color,
                warning_color: t.text_color,
                danger_color: t.text_color,
                disabled_color: t.text_color,
            }),
            (None, TextTone::Subtle, Some(backdrop)) => label.subtle_on(backdrop),
            (None, tone, _) => label.tone(tone),
        };
        // Suppress inner label semantics so heading opt-out also suppresses the title.
        let mut label = label.announce(false);
        if self.audit_elision {
            label = label.audit_elision(true);
        }
        label
    }

    /// Return the title heading and level, or None when announcements are disabled.
    pub fn heading_semantics(&self) -> Option<Semantics> {
        if !self.announce {
            return None;
        }
        Some(
            Semantics::new(Role::Heading)
                .label(self.title.clone())
                .level(self.heading_level),
        )
    }

    /// Compose the fixed-height caption band, with optional sticky presentation.
    pub fn into_element(self) -> Element<'a, Message> {
        let height = self.title_size + band_padding();
        let width = self.width;
        let heading = self.heading_semantics();
        let label = self.build_label();

        let sticky_backdrop = self.sticky_backdrop;
        let mut band = container(label)
            .width(width)
            .height(Length::Fixed(height))
            .align_x(iced::alignment::Horizontal::Left)
            .center_y(Length::Fixed(height));
        if let Some(backdrop) = sticky_backdrop {
            band = band.style(move |_| container::Style::default().background(backdrop));
        }
        let band: Element<'a, Message> = band.into();
        // Descend into the band so elision scans reach the probe despite suppressed label semantics.
        let header = match heading {
            Some(semantics) => {
                crate::semantics_container::SemanticsContainer::new(band, semantics).into()
            }
            None => band,
        };
        match sticky_backdrop {
            Some(_) => iced::widget::sticky(header).into(),
            None => header,
        }
    }
}

impl<'a, Message: 'a> From<EnzSectionHeader<'a, Message>> for Element<'a, Message> {
    fn from(h: EnzSectionHeader<'a, Message>) -> Self {
        h.into_element()
    }
}
