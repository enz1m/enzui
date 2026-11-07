//! Logical text-size and semantic weight tokens.
//!
//! [`TypographyScale`] stores an authored ramp with a baked text-size factor.
//! [`TypographyScale::with_text_scale`] replaces that factor rather than
//! compounding it. Explicit weight overrides remain caller-controlled;
//! [`css_weight_to_iced`] quantizes CSS weights to renderer weight categories.
//! Letter-spacing values are retained as data but are not rendered by this
//! build; see [`LETTER_SPACING_UNSUPPORTED`].

use crate::text_scale::TextScale;
use iced_core::font::Weight;

/// Diagnostic for tracking edits unsupported by this build's text descriptor.
/// The shared theme-file layer rejects those edits; changing the struct fields
/// directly has no effect on shaping, wrapping or measured glyph advances.
pub const LETTER_SPACING_UNSUPPORTED: &str = "letter spacing is not rendered by this build: iced's text descriptor \
     carries no tracking field, so the value would change no glyph advance, \
     no wrap point and no measured width";

/// A semantic font-weight rung of a [`TypographyScale`].
///
/// Rungs select CSS weights through [`TypographyScale::iced_weight`].
/// Ordering follows declaration order, independently of a custom scale's values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WeightRung {
    /// Ordinary running text.
    Regular,
    /// Slightly emphasised UI text (a control label).
    Medium,
    /// Section headers and headline rungs.
    Semibold,
    /// Titles.
    Bold,
    /// The hero/display rung.
    ExtraBold,
}

impl WeightRung {
    /// Every rung, in ascending compiled-default order.
    pub const ALL: [WeightRung; 5] = [
        WeightRung::Regular,
        WeightRung::Medium,
        WeightRung::Semibold,
        WeightRung::Bold,
        WeightRung::ExtraBold,
    ];

    /// The dotted theme-file token path that carries this rung.
    #[must_use]
    pub const fn token_path(self) -> &'static str {
        match self {
            WeightRung::Regular => "typography.weight_regular",
            WeightRung::Medium => "typography.weight_medium",
            WeightRung::Semibold => "typography.weight_semibold",
            WeightRung::Bold => "typography.weight_bold",
            WeightRung::ExtraBold => "typography.weight_extrabold",
        }
    }
}

/// Quantize a CSS numeric weight to one of iced's nine weight categories.
///
/// Round to the nearest hundred with ties upward, bounded to the end
/// categories. The renderer chooses the available font face; this does not
/// guarantee that a host font provides the requested weight.
///
/// | numeric | face |
/// |---|---|
/// | ≤ 149 | [`Weight::Thin`] (100) |
/// | 150…249 | [`Weight::ExtraLight`] (200) |
/// | 250…349 | [`Weight::Light`] (300) |
/// | 350…449 | [`Weight::Normal`] (400) |
/// | 450…549 | [`Weight::Medium`] (500) |
/// | 550…649 | [`Weight::Semibold`] (600) |
/// | 650…749 | [`Weight::Bold`] (700) |
/// | 750…849 | [`Weight::ExtraBold`] (800) |
/// | ≥ 850 | [`Weight::Black`] (900) |
///
/// Values outside the theme-file range (`1..=1000`) select an end category
/// without wrapping. Variable numeric weights are quantized by the same rule.
///
/// ```
/// use enzui_theme::typography::css_weight_to_iced;
/// use iced_core::font::Weight;
///
/// assert_eq!(css_weight_to_iced(400), Weight::Normal);
/// assert_eq!(css_weight_to_iced(449), Weight::Normal);
/// assert_eq!(css_weight_to_iced(450), Weight::Medium);
/// assert_eq!(css_weight_to_iced(900), Weight::Black);
/// ```
#[must_use]
pub const fn css_weight_to_iced(css: u16) -> Weight {
    let clamped = if css > 1000 { 1000 } else { css };
    match (clamped + 50) / 100 {
        0 | 1 => Weight::Thin,
        2 => Weight::ExtraLight,
        3 => Weight::Light,
        4 => Weight::Normal,
        5 => Weight::Medium,
        6 => Weight::Semibold,
        7 => Weight::Bold,
        8 => Weight::ExtraBold,
        _ => Weight::Black,
    }
}

/// Typography scale for text sizes, weights, and letter spacing.
///
/// Sizes are logical pixels; weights are CSS numeric values. Public fields
/// are not validated by construction. Letter spacing remains unrendered data.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypographyScale {
    /// Caption and smallest-text size.
    pub caption_size: f32,
    /// Label text.
    pub label_size: f32,
    /// Monospace text.
    pub monospace_size: f32,
    /// Status-bar text, independently configurable from the footnote and
    /// monospace rungs that also default to 12 logical pixels.
    pub statusbar_text_size: f32,
    /// Footnote / subheadline prose text.
    pub footnote_size: f32,
    /// Body text (default).
    pub body_size: f32,
    /// Section header.
    pub section_size: f32,
    /// Title3 rung, defaulting to 17 logical pixels between the default
    /// `section_size` (15) and `title_size` (18). It independently controls
    /// `TextSize::Title3` and `TextSize::Lg`.
    pub title3_size: f32,
    /// Title.
    pub title_size: f32,
    /// Title1 screen-title rung, defaulting to 28 logical pixels and used by
    /// `TextSize::Title1`.
    pub title1_size: f32,
    /// Mid-range display rung, defaulting to 22 logical pixels for
    /// `TextSize::Title2` and `TextSize::Xl`; `TextSize::LargeTitle` uses
    /// [`Self::hero_size`].
    pub large_title_size: f32,
    /// Hero/display rung, 34 logical pixels by default, used by
    /// `TextSize::LargeTitle`; [`Self::large_title_size`] is the separate
    /// 22-pixel default used by `TextSize::Title2` and `TextSize::Xl`.
    pub hero_size: f32,

    /// The desktop text-size factor already multiplied into the twelve size
    /// rungs above (`1.0` for the authored ramp).
    ///
    /// Read it to learn what a scale carries; change it through
    /// [`Self::with_text_scale`], which rescales the rungs with it. Assigning
    /// this field alone records a factor the rungs do not have.
    pub text_scale: f32,

    /// Regular font weight ([`WeightRung::Regular`], 400 by default).
    pub weight_regular: u16,
    /// Medium weight ([`WeightRung::Medium`], 500 by default).
    pub weight_medium: u16,
    /// Semibold weight ([`WeightRung::Semibold`], 600 by default).
    pub weight_semibold: u16,
    /// Bold weight ([`WeightRung::Bold`], 700 by default).
    pub weight_bold: u16,
    /// Extra-bold / hero weight ([`WeightRung::ExtraBold`], 800 by default).
    pub weight_extrabold: u16,

    /// Tight letter spacing, in logical pixels.
    ///
    /// Not rendered; see [`LETTER_SPACING_UNSUPPORTED`].
    pub letter_spacing_tight: f32,
    /// Normal letter spacing, in logical pixels. Not rendered; see
    /// [`LETTER_SPACING_UNSUPPORTED`].
    pub letter_spacing_normal: f32,
    /// Wide letter spacing, in logical pixels. Not rendered; see
    /// [`LETTER_SPACING_UNSUPPORTED`].
    pub letter_spacing_wide: f32,
}

impl Default for TypographyScale {
    fn default() -> Self {
        Self {
            caption_size: 10.0,
            label_size: 11.0,
            monospace_size: 12.0,
            statusbar_text_size: 12.0,
            footnote_size: 12.0,
            body_size: 13.0,
            section_size: 15.0,
            title3_size: 17.0,
            title_size: 18.0,
            large_title_size: 22.0,
            title1_size: 28.0,
            hero_size: 34.0,

            text_scale: 1.0,

            weight_regular: 400,
            weight_medium: 500,
            weight_semibold: 600,
            weight_bold: 700,
            weight_extrabold: 800,

            letter_spacing_tight: -0.4,
            letter_spacing_normal: 0.0,
            letter_spacing_wide: 0.4,
        }
    }
}

impl TypographyScale {
    /// The text-size factor baked into this scale's size rungs.
    ///
    /// Admits the raw factor through [`TextScale::from_backend`]. This bounds
    /// the reported factor without validating or repairing size rungs.
    #[must_use]
    pub fn scale_factor(&self) -> TextScale {
        TextScale::from_backend(self.text_scale)
    }

    /// The authored ramp with any text scale removed.
    ///
    /// Baseline leaves sizes unchanged; otherwise each size is divided by the
    /// admitted factor. Finite values round-trip subject to rounding; invalid
    /// or overflowed sizes are not repaired.
    #[must_use]
    pub fn base_ramp(&self) -> Self {
        let factor = self.scale_factor();
        let mut base = *self;
        base.map_sizes(|px| factor.unapply_px(px));
        base.text_scale = TextScale::BASELINE.factor();
        base
    }

    /// The default ramp at the effective desktop text scale, for a widget that
    /// sizes its text before it can see the theme.
    #[must_use]
    pub fn ambient() -> Self {
        crate::text_defaults::typography().with_text_scale(crate::text_scale::effective())
    }

    /// This ramp with `scale` as its text-size factor.
    ///
    /// Divide out the admitted prior factor, then multiply all twelve size
    /// rungs by the new one, subject to floating-point rounding. Weights and
    /// unrendered tracking values stay unchanged. Repeated scaling is not a
    /// guarantee of bit-identical recovery for arbitrary authored values.
    #[must_use]
    pub fn with_text_scale(&self, scale: TextScale) -> Self {
        let mut scaled = self.base_ramp();
        scaled.map_sizes(|px| scale.apply_px(px));
        scaled.text_scale = scale.factor();
        scaled
    }

    /// Set the unscaled body size while preserving the ramp's proportions.
    ///
    /// Invalid sizes leave the ramp unchanged. Desktop font sizes are logical
    /// pixels here; point conversion belongs to the platform adapter.
    #[must_use]
    pub fn with_base_body_size(&self, size: f32) -> Self {
        let mut base = self.base_ramp();
        if size.is_finite() && size > 0.0 && base.body_size > 0.0 {
            let factor = size / base.body_size;
            base.map_sizes(|px| px * factor);
        }
        base.with_text_scale(self.scale_factor())
    }

    fn map_sizes(&mut self, f: impl Fn(f32) -> f32) {
        for rung in [
            &mut self.caption_size,
            &mut self.label_size,
            &mut self.monospace_size,
            &mut self.statusbar_text_size,
            &mut self.footnote_size,
            &mut self.body_size,
            &mut self.section_size,
            &mut self.title3_size,
            &mut self.title_size,
            &mut self.title1_size,
            &mut self.large_title_size,
            &mut self.hero_size,
        ] {
            *rung = f(*rung);
        }
    }

    /// The CSS numeric weight this scale assigns to `rung`.
    #[must_use]
    pub const fn weight(&self, rung: WeightRung) -> u16 {
        match rung {
            WeightRung::Regular => self.weight_regular,
            WeightRung::Medium => self.weight_medium,
            WeightRung::Semibold => self.weight_semibold,
            WeightRung::Bold => self.weight_bold,
            WeightRung::ExtraBold => self.weight_extrabold,
        }
    }

    /// The discrete face `rung` resolves to — [`Self::weight`] through
    /// [`css_weight_to_iced`].
    #[must_use]
    pub const fn iced_weight(&self, rung: WeightRung) -> Weight {
        css_weight_to_iced(self.weight(rung))
    }

    /// Whether the three unrendered `letter_spacing_*` rungs still hold their
    /// compiled defaults.
    #[must_use]
    pub fn letter_spacing_is_default(&self) -> bool {
        let d = Self::default();
        self.letter_spacing_tight == d.letter_spacing_tight
            && self.letter_spacing_normal == d.letter_spacing_normal
            && self.letter_spacing_wide == d.letter_spacing_wide
    }
}
