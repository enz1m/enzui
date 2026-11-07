//! Centred title and optional subtitle, with live palette ink and optional heading semantics.
//!
//! Title and subtitle use ambient Bold and Medium weights. Unpinned geometry
//! resolves when composed: stock spacing gives 28px single-line and 36px
//! two-line bands, while ambient typography gives body size plus 1 for the title
//! and label size for the subtitle. Inter-line spacing defaults to 1px.
//! Custom sizes may need corresponding height overrides to fit both line boxes.
//!
//! Text is single-line, end-elided and audited for elision by default. A heading
//! annotation promotes only the title's existing label node; ordinary titles
//! remain labels. [`crate::header_bar`] composes this widget for its title.

use crate::enz_text::{TextAlign, TextTokens, TextTone, enz_text};
use enzui_core::semantics::HeadingLevel;
use enzui_theme::{ContrastFloor, Palette, SpacingScale, TypographyScale};
use iced::widget::{column, container};
use iced::{Alignment, Color, Length};
/// Title sizes and fixed band geometry; colours resolve through the composed text.
#[derive(Debug, Clone, Copy)]
pub struct WindowTitleTokens {
    /// Title size in logical pixels; from_scales uses body size plus 1.
    pub title_size: f32,
    /// Subtitle size in logical pixels; from_scales uses label size.
    pub subtitle_size: f32,
    /// Fixed band height without a subtitle in logical pixels.
    pub height_single: f32,
    /// Fixed band height with a subtitle in logical pixels.
    pub height_double: f32,
    /// Inter-line gap in logical pixels; defaults to 1.
    pub spacing: f32,
}

impl WindowTitleTokens {
    /// Use the supplied spacing ladder and ambient typography.
    /// Heights are space11 minus or plus space2; dimensions are not validated here.
    pub fn from_spacing(scale: &SpacingScale) -> Self {
        Self::from_scales(scale, &TypographyScale::ambient())
    }
    /// Derive sizes from body size plus 1 and label size, with heights space11 minus or plus space2.
    /// The inter-line gap is 1px. This helper does not validate the supplied scales.
    pub fn from_scales(spacing: &SpacingScale, typography: &TypographyScale) -> Self {
        Self {
            title_size: typography.body_size + 1.0,
            subtitle_size: typography.label_size,
            height_single: spacing.space11 - spacing.space2,
            height_double: spacing.space11 + spacing.space2,
            spacing: 1.0,
        }
    }
    /// Use stock spacing and ambient typography: stock heights are 28 and 36px.
    pub fn defaults() -> Self {
        Self::from_scales(&SpacingScale::default(), &TypographyScale::ambient())
    }
}

impl Default for WindowTitleTokens {
    fn default() -> Self {
        Self::defaults()
    }
}
/// Title-stack builder with tone, geometry and heading options.
#[must_use]
pub struct EnzWindowTitle {
    title: String,
    subtitle: String,
    centered: bool,
    tokens: Option<WindowTitleTokens>,
    title_tone: Option<TextTone>,
    subtitle_tone: Option<TextTone>,
    subtitle_backdrop: Option<(Color, ContrastFloor)>,
    heading: Option<HeadingLevel>,
    audit_elision: bool,
    pane_header: bool,
}

impl EnzWindowTitle {
    /// Set the subtitle; empty text removes its line and selects the single-line height.
    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        self.subtitle = s.into();
        self
    }
    /// Centre the text and stack; defaults to true. False selects left alignment.
    pub fn centered(mut self, on: bool) -> Self {
        self.centered = on;
        self
    }
    /// Select live title ink; defaults to Default.
    pub fn title_tone(mut self, tone: TextTone) -> Self {
        self.title_tone = Some(tone);
        self
    }
    /// Select live subtitle ink; defaults to Muted.
    /// A stored backdrop resolves only Subtle or Muted tones.
    pub fn subtitle_tone(mut self, tone: TextTone) -> Self {
        self.subtitle_tone = Some(tone);
        self
    }
    /// Resolve subtitle Subtle ink against the supplied backdrop at the text contrast floor.
    /// This does not paint the backdrop or alter title ink.
    pub fn subtle_on(self, backdrop: Color) -> Self {
        self.subtle_on_with_floor(backdrop, ContrastFloor::Text)
    }
    /// Resolve subtitle Subtle ink against a supplied backdrop and contrast floor.
    /// Use LargeText only when the subtitle qualifies for that clause.
    pub fn subtle_on_with_floor(mut self, backdrop: Color, floor: ContrastFloor) -> Self {
        self.subtitle_tone = Some(TextTone::Subtle);
        self.subtitle_backdrop = Some((backdrop, floor));
        self
    }
    /// Resolve subtitle Muted ink against the supplied backdrop at the text contrast floor.
    /// This does not paint the backdrop or alter title ink.
    pub fn muted_on(mut self, backdrop: Color) -> Self {
        self.subtitle_tone = Some(TextTone::Muted);
        self.subtitle_backdrop = Some((backdrop, ContrastFloor::Text));
        self
    }
    /// Resolve subtitle ink, applying the stored backdrop to weak tones when present.
    pub fn resolved_subtitle_color(&self, palette: &Palette) -> Color {
        let tone = self.subtitle_tone.unwrap_or(TextTone::Muted);
        match (tone, self.subtitle_backdrop) {
            (TextTone::Subtle, Some((backdrop, floor))) => palette.subtle_on(backdrop, floor),
            (TextTone::Muted, Some((backdrop, _))) => palette.text_muted_on(backdrop),
            _ => TextTokens::from_palette(palette).color_for(tone),
        }
    }
    /// Annotate the title label with a heading level; defaults to no heading.
    /// The subtitle remains an ordinary label, and visual styling is unaffected.
    pub fn heading(mut self, level: HeadingLevel) -> Self {
        self.heading = Some(level);
        self
    }
    /// Set the title as a pane heading (the header above a sidebar) rather
    /// than a window title: Semibold instead of Bold, and at body size unless
    /// [`Self::tokens`] pins one. A titlebar inside the shell's sidebar header
    /// sets this itself.
    pub fn pane_header(mut self, on: bool) -> Self {
        self.pane_header = on;
        self
    }
    /// Return the optional title heading level.
    pub fn heading_level(&self) -> Option<HeadingLevel> {
        self.heading
    }
    /// Request auditing for title and subtitle elision.
    /// Both composed text runs already enable auditing, so false does not disable it.
    pub fn audit_elision(mut self, on: bool) -> Self {
        self.audit_elision = on;
        self
    }
    /// Pin sizes and band heights; nonpositive or non-finite values use current defaults.
    /// Spacing permits zero; negative or non-finite gaps use the default 1px.
    pub fn tokens(mut self, t: WindowTitleTokens) -> Self {
        let d = WindowTitleTokens::defaults();
        let fix = |v: f32, fallback: f32| {
            if v.is_finite() && v > 0.0 {
                v
            } else {
                fallback
            }
        };
        self.tokens = Some(WindowTitleTokens {
            title_size: fix(t.title_size, d.title_size),
            subtitle_size: fix(t.subtitle_size, d.subtitle_size),
            height_single: fix(t.height_single, d.height_single),
            height_double: fix(t.height_double, d.height_double),
            spacing: if t.spacing.is_finite() && t.spacing >= 0.0 {
                t.spacing
            } else {
                d.spacing
            },
        });
        self
    }
    /// Return whether the subtitle text is nonempty.
    pub fn has_subtitle(&self) -> bool {
        !self.subtitle.is_empty()
    }
    /// Return the selected single-line or two-line band height in logical pixels.
    pub fn resolved_height(&self) -> f32 {
        let t = self.tokens.unwrap_or_default();
        if self.has_subtitle() {
            t.height_double
        } else {
            t.height_single
        }
    }
    /// Compose the fixed-height title stack with live ink and optional heading semantics.
    pub fn into_element<'a, Message: 'a>(self) -> iced::Element<'a, Message> {
        let t = self.tokens.unwrap_or_default();
        let has_sub = !self.subtitle.is_empty();
        let height = if has_sub {
            t.height_double
        } else {
            t.height_single
        };

        let text_align = if self.centered {
            TextAlign::Center
        } else {
            TextAlign::Start
        };
        let col_align = if self.centered {
            Alignment::Center
        } else {
            Alignment::Start
        };
        let (title_size, title_weight) = if self.pane_header {
            let size = if self.tokens.is_some() {
                t.title_size
            } else {
                t.title_size - 1.0
            };
            (size, enzui_theme::WeightRung::Semibold)
        } else {
            (t.title_size, enzui_theme::WeightRung::Bold)
        };
        let mut title_run = enz_text(self.title)
            .size_px(title_size)
            .weight_rung(title_weight)
            .tone(self.title_tone.unwrap_or(TextTone::Default))
            .align(text_align)
            .single_line_elided()
            .full_width();
        if let Some(level) = self.heading {
            title_run = title_run.heading(level);
        }
        if self.audit_elision {
            title_run = title_run.audit_elision(true);
        }
        let title_el: iced::Element<'a, Message> = title_run.into();

        let stack: iced::Element<'a, Message> = if has_sub {
            let sub_tone = self.subtitle_tone.unwrap_or(TextTone::Muted);
            let sub_ink = match (sub_tone, self.subtitle_backdrop) {
                (TextTone::Subtle, Some((backdrop, floor))) => {
                    enz_text(self.subtitle).subtle_on_with_floor(backdrop, floor)
                }
                (TextTone::Muted, Some((backdrop, _))) => {
                    enz_text(self.subtitle).muted_on(backdrop)
                }
                _ => enz_text(self.subtitle).tone(sub_tone),
            };
            let mut sub_run = sub_ink
                .size_px(t.subtitle_size)
                .weight_rung(enzui_theme::WeightRung::Medium)
                .align(text_align)
                .single_line_elided()
                .full_width();
            if self.audit_elision {
                sub_run = sub_run.audit_elision(true);
            }
            let sub_el: iced::Element<'a, Message> = sub_run.into();
            column![title_el, sub_el]
                .spacing(t.spacing)
                .align_x(col_align)
                .into()
        } else {
            column![title_el].align_x(col_align).into()
        };

        container(stack)
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .align_x(if self.centered {
                iced::alignment::Horizontal::Center
            } else {
                iced::alignment::Horizontal::Left
            })
            .align_y(iced::alignment::Vertical::Center)
            .into()
    }
}

impl<'a, Message: 'a> From<EnzWindowTitle> for iced::Element<'a, Message> {
    fn from(w: EnzWindowTitle) -> Self {
        w.into_element()
    }
}
/// Own a centred title without subtitle, colour overrides or heading annotation.
pub fn enz_window_title(title: impl Into<String>) -> EnzWindowTitle {
    EnzWindowTitle {
        title: title.into(),
        subtitle: String::new(),
        centered: true,
        tokens: None,
        title_tone: None,
        subtitle_tone: None,
        subtitle_backdrop: None,
        heading: None,
        audit_elision: false,
        pane_header: false,
    }
}
