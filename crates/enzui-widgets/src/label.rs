//! Compose an optional icon, title and subtitle into one label.
//!
//! [`enz_label`] uses a shrink-wrapped row, a square icon frame, a 13-pixel
//! title, an 11-pixel subtitle and a muted icon. The alias constructor
//! [`enz_icon_label`](crate::icon_label::enz_icon_label) uses fill width, an
//! intrinsic icon frame, a Body title rung, single-line elision and an icon
//! following the row tone. [`LabelPolicy`] exposes the remaining preset
//! differences for empty content and zero icon sizes.
//!
//! Display styles suppress the unwanted branch and its gap while retaining
//! content strings. Catalog names resolve through [`enz_icon`]; use
//! [`EnzLabel::glyph`] for literal text or Unicode glyphs, or
//! [`EnzLabel::icon_source`] for SVG/raster handles that retain their colors.
//!
//! # Ink
//!
//! Colours resolve against the live theme. Catalog/glyph precedence is an explicit icon
//! colour, explicit icon tone, title colour, then row tone. Title and subtitle
//! each prefer their own colour pin over their own tone. The form preset's
//! muted icon tone prevents a title pin or row tone from changing the icon.
//! [`EnzLabel::subtle_on`] adjusts only components whose effective tone is
//! Subtle; explicit colour pins retain precedence.
//!
//! # Icon frame and slot
//!
//! An explicit slot reserves its finite non-negative width, including zero or
//! a width smaller than the icon, and clips overflow. Negative or non-finite
//! slot widths clear the override. Without a slot, the form preset centers the
//! icon in an `icon_size + 4` square; the row preset uses intrinsic sizing.
//! A zero icon-size setter uses the 16-pixel fallback on the form preset.
//! Row catalog icons accept
//! zero; text-glyph icons still fall back to 16 because their font size must
//! be positive. Unpinned icon sizes follow the desktop text scale unless a
//! sizing scale is supplied.
//!
//! # Accessibility
//!
//! Icons are decorative. A named label publishes one aggregate label node,
//! using the retained title even in IconOnly mode and a nonblank subtitle as
//! its description, including when the subtitle is hidden. [`LabelA11y`]
//! overrides or silences that name. Empty Named strings fall back to the title;
//! the form preset treats whitespace-only names as blank, while the row preset
//! preserves them. The form preset suppresses the node when no branch renders;
//! the row preset can retain its named node even with no rendered branches.
//!

use crate::enz_text::{TextShaping, TextSize, TextTokens, TextTone, enz_text};
use crate::icon::{IconTone, enz_icon, tone_ink};
use crate::semantics_container::SemanticsContainer;
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::Palette;
use iced::font::Weight;
use iced::widget::text::LineHeight;
use iced::widget::{container, row};
use iced::{Alignment, Color, Element, Length};

/// Which parts of the label are shown (SwiftUI `labelStyle` parity).
///
/// The SwiftUI-named spelling of [`LabelDisplay`]; both name the same three
/// presentations and [`From`] converts between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LabelStyle {
    /// Show both the icon and the title/subtitle (the default).
    #[default]
    TitleAndIcon,
    /// Show the title/subtitle only; suppress the icon.
    TitleOnly,
    /// Show the icon only; suppress the title and subtitle.
    IconOnly,
}

/// Which parts of the label are shown — the canonical display policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LabelDisplay {
    /// Icon and title/subtitle (the default).
    #[default]
    Both,
    /// Icon only — the title/subtitle strings are retained but not rendered,
    /// and the gap collapses.
    IconOnly,
    /// Title/subtitle only — the icon string is retained but not rendered, and
    /// the gap collapses.
    TitleOnly,
}

impl From<LabelStyle> for LabelDisplay {
    fn from(style: LabelStyle) -> Self {
        match style {
            LabelStyle::TitleAndIcon => Self::Both,
            LabelStyle::TitleOnly => Self::TitleOnly,
            LabelStyle::IconOnly => Self::IconOnly,
        }
    }
}

impl From<LabelDisplay> for LabelStyle {
    fn from(display: LabelDisplay) -> Self {
        match display {
            LabelDisplay::Both => Self::TitleAndIcon,
            LabelDisplay::TitleOnly => Self::TitleOnly,
            LabelDisplay::IconOnly => Self::IconOnly,
        }
    }
}

/// Semantic colour family for the title and unoverridden icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LabelTone {
    /// Full-strength text tone (default).
    #[default]
    Default,
    /// Muted / secondary (`text_muted`).
    Muted,
    /// Subtle / faintest tier (`text_subtle`).
    Subtle,
    /// Accent (informational / interactive).
    Accent,
    /// Success (`success` palette entry).
    Success,
    /// Warning (`warning` palette entry).
    Warning,
    /// Danger / destructive (`destructive` palette entry).
    Danger,
}

impl LabelTone {
    pub(crate) fn text_tone(self) -> TextTone {
        match self {
            Self::Default => TextTone::Default,
            Self::Muted => TextTone::Muted,
            Self::Subtle => TextTone::Subtle,
            Self::Accent => TextTone::Accent,
            Self::Success => TextTone::Success,
            Self::Warning => TextTone::Warning,
            Self::Danger => TextTone::Danger,
        }
    }

    pub(crate) fn icon_tone(self) -> IconTone {
        match self {
            Self::Default => IconTone::Text,
            Self::Muted => IconTone::Muted,
            Self::Subtle => IconTone::Subtle,
            Self::Accent => IconTone::Accent,
            Self::Success => IconTone::Success,
            Self::Warning => IconTone::Warning,
            Self::Danger => IconTone::Danger,
        }
    }
}

/// Choose the aggregate accessible name or suppress the label.
///
/// An available name yields one passive label node with a decorative icon.
/// Blank-name and nothing-rendered behavior follows [`LabelPolicy`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LabelA11y {
    /// Name the label by its retained title (default). An empty title
    /// publishes nothing — there is no name to give, and a node with no name is
    /// noise.
    #[default]
    Auto,
    /// Name the label explicitly, overriding the visible title (an
    /// icon-only row whose glyph means more than its title says).
    ///
    /// An empty string falls back to [`Self::Auto`]. Use
    /// [`Self::Decorative`] to mean silent.
    Named(String),
    /// Publish nothing at all. For a label that merely restates an adjacent
    /// control already announced — the same intent as
    /// [`crate::decorative::decorative`], expressed at the label level.
    Decorative,
}

impl LabelA11y {
    fn name_for(&self, title: &str, trim: bool) -> Option<String> {
        let pick = match self {
            Self::Auto => title,
            Self::Named(n) if !n.is_empty() => n.as_str(),
            Self::Named(_) => title,
            Self::Decorative => return None,
        };
        let blank = if trim {
            pick.trim().is_empty()
        } else {
            pick.is_empty()
        };
        (!blank).then(|| pick.to_owned())
    }
}

#[derive(Debug, Clone)]
pub(crate) enum LabelIconSource {
    Name(String),

    Glyph(String),
    Artwork(crate::icon::IconSource),
}

impl LabelIconSource {
    fn is_empty(&self) -> bool {
        match self {
            Self::Name(s) | Self::Glyph(s) => s.is_empty(),
            Self::Artwork(_) => false,
        }
    }
}

/// Default title text size in px (`body-size`), used by the [`enz_label`]
/// preset. Fallback for a non-finite / non-positive
/// [`EnzLabel::title_size`].
pub const DEFAULT_TITLE_SIZE: f32 = 13.0;
/// Default subtitle text size in px (`label-size`).
pub const DEFAULT_SUBTITLE_SIZE: f32 = 11.0;
/// Default title rung (`body-size` = 13 px, [`TextSize::Body`]), used by the
/// [`enz_icon_label`](crate::icon_label::enz_icon_label) preset.
///
/// This is the stock fallback: when the view owner calls
/// [`EnzLabel::typography`], the size resolves through the `TextSize::Body`
/// rung of the active scale instead.
pub const DEFAULT_TEXT_SIZE: TextSize = TextSize::Body;
/// Baseline icon size and invalid-size fallback, in logical pixels.
///
/// Unpinned sizes use a supplied sizing scale or the desktop text scale.
pub const DEFAULT_ICON_SIZE: f32 = 16.0;
/// Default gap between the icon and the text column in px (`spacing-sm`).
/// Stock fallback; the active [`SpacingScale::sm`] rung wins when
/// [`EnzLabel::spacing_scale`] is forwarded.
///
/// [`SpacingScale::sm`]: enzui_theme::SpacingScale::sm
pub const DEFAULT_GAP: f32 = 8.0;
/// Compiled default title weight.
///
/// Rendering uses an explicit weight, a supplied typography scale's Medium rung,
/// or the application's published Medium rung. See [`EnzLabel::effective_weight`].
pub const DEFAULT_WEIGHT: Weight = Weight::Medium;
const SUBTITLE_GAP: f32 = 2.0;
const ICON_FRAME_PAD: f32 = 4.0;

pub(crate) fn sane(value: f32, default: f32) -> f32 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        default
    }
}

pub(crate) fn sane_font_size(value: f32, default: f32) -> f32 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        default
    }
}

/// How the icon occupies the leading column when no explicit
/// [`EnzLabel::slot_width`] is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconFrame {
    /// The icon takes its own intrinsic (`icon_size`) box — the
    /// [`enz_icon_label`](crate::icon_label::enz_icon_label) preset.
    Intrinsic,
    /// The icon is centred in a square of `icon_size + 4` px — the
    /// [`enz_label`] preset, matching the form primitive's icon box.
    Square,
}

/// Preset rules for icon framing, empty content and zero icon sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelPolicy {
    /// Icon column shape when no explicit slot width is set.
    pub icon_frame: IconFrame,
    /// Whether an empty title still occupies a text column and its icon gap.
    pub render_empty_title: bool,
    /// Whether a whitespace-only title counts as no accessible name.
    /// The form preset trims; the row preset compares the raw string.
    pub trim_blank_title: bool,
    /// Whether a label whose every visual branch is suppressed (`IconOnly`
    /// with no icon) publishes no node.
    pub silent_when_nothing_renders: bool,
    /// Whether a zero icon-size setter uses the default instead of zero.
    /// Text-glyph font sizes require a positive fallback regardless.
    pub zero_icon_size_is_default: bool,
}

impl LabelPolicy {
    /// The [`enz_label`] (form/preferences) preset.
    pub const FORM: Self = Self {
        icon_frame: IconFrame::Square,
        render_empty_title: true,
        trim_blank_title: true,
        silent_when_nothing_renders: true,
        zero_icon_size_is_default: true,
    };
    /// The [`enz_icon_label`](crate::icon_label::enz_icon_label) (SwiftUI-Label
    /// row) preset.
    pub const ROW: Self = Self {
        icon_frame: IconFrame::Intrinsic,
        render_empty_title: false,
        trim_blank_title: false,
        silent_when_nothing_renders: false,
        zero_icon_size_is_default: false,
    };
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct InkRule {
    tone: LabelTone,
    icon_tone: Option<LabelTone>,
    icon_color: Option<Color>,
    text_color: Option<Color>,
    subtle_backdrop: Option<Color>,
}

impl InkRule {
    fn icon(self, palette: &Palette) -> Color {
        match (self.icon_color, self.icon_tone, self.text_color) {
            (Some(c), _, _) => c,
            (None, Some(t), _) => self.tone_ink(t, palette),
            (None, None, Some(c)) => c,
            (None, None, None) => self.tone_ink(self.tone, palette),
        }
    }

    fn label(self, palette: &Palette) -> Color {
        self.text_color.unwrap_or_else(|| {
            if self.tone == LabelTone::Subtle
                && let Some(backdrop) = self.subtle_backdrop
            {
                palette.text_subtle_on(backdrop)
            } else {
                TextTokens::from_palette(palette).color_for(self.tone.text_tone())
            }
        })
    }

    fn tone_ink(self, tone: LabelTone, palette: &Palette) -> Color {
        if tone == LabelTone::Subtle
            && let Some(backdrop) = self.subtle_backdrop
        {
            palette.text_subtle_on(backdrop)
        } else {
            tone_ink(tone.icon_tone(), palette)
        }
    }
}

/// The canonical label builder: an optional icon, a title, an optional
/// subtitle. See [`enz_label`] and
/// [`enz_icon_label`](crate::icon_label::enz_icon_label) for the two presets.
#[derive(Debug, Clone)]
#[must_use]
pub struct EnzLabel<'a> {
    pub(crate) icon: LabelIconSource,
    pub(crate) title: String,
    pub(crate) subtitle: String,
    pub(crate) display: LabelDisplay,
    pub(crate) tone: LabelTone,
    pub(crate) icon_tone: Option<LabelTone>,
    pub(crate) icon_color: Option<Color>,
    pub(crate) text_color: Option<Color>,
    pub(crate) subtitle_tone: LabelTone,
    pub(crate) subtitle_color: Option<Color>,

    pub(crate) subtle_backdrop: Option<Color>,
    pub(crate) size: TextSize,

    pub(crate) title_px: Option<f32>,

    pub(crate) subtitle_px: Option<f32>,

    pub(crate) subtitle_rung: Option<TextSize>,
    pub(crate) icon_size: f32,

    pub(crate) icon_size_explicit: bool,
    pub(crate) slot_width: Option<f32>,
    pub(crate) weight: Option<Weight>,
    pub(crate) shaping: TextShaping,

    pub(crate) line_height: Option<f32>,
    pub(crate) spacing: f32,

    pub(crate) spacing_explicit: bool,
    pub(crate) fill: bool,

    pub(crate) wrap: Option<bool>,

    pub(crate) typography: Option<enzui_theme::TypographyScale>,
    pub(crate) sizing: Option<enzui_theme::SizingTokens>,
    pub(crate) spacing_scale: Option<enzui_theme::SpacingScale>,
    pub(crate) a11y: LabelA11y,
    pub(crate) policy: LabelPolicy,
    pub(crate) _lifetime: std::marker::PhantomData<&'a ()>,
}

/// Construct a label with the given title, preset for the form/preferences
/// shape (shrink-wrapped row, centred `icon_size + 4` icon square, muted icon
/// tone, 13 px medium title, 11 px muted subtitle;
/// [`TitleAndIcon`](LabelStyle::TitleAndIcon), theme-resolved, no subtitle or
/// icon until set). See [`enz_icon_label`](crate::icon_label::enz_icon_label)
/// for the row preset of the same builder.
pub fn enz_label<'a>(title: impl Into<String>) -> EnzLabel<'a> {
    EnzLabel {
        icon: LabelIconSource::Name(String::new()),
        title: title.into(),
        subtitle: String::new(),
        display: LabelDisplay::Both,
        tone: LabelTone::Default,

        // The form preset intentionally prevents title ink from propagating to its icon.
        icon_tone: Some(LabelTone::Muted),
        icon_color: None,
        text_color: None,
        subtitle_tone: LabelTone::Muted,
        subtitle_color: None,
        subtle_backdrop: None,
        size: DEFAULT_TEXT_SIZE,
        title_px: Some(DEFAULT_TITLE_SIZE),
        subtitle_px: Some(DEFAULT_SUBTITLE_SIZE),
        subtitle_rung: None,
        icon_size: DEFAULT_ICON_SIZE,
        icon_size_explicit: false,
        slot_width: None,
        weight: None,
        shaping: TextShaping::default(),
        line_height: None,
        spacing: DEFAULT_GAP,
        spacing_explicit: false,
        fill: false,
        wrap: None,
        typography: None,
        sizing: None,
        spacing_scale: None,
        a11y: LabelA11y::Auto,
        policy: LabelPolicy::FORM,
        _lifetime: std::marker::PhantomData,
    }
}

pub(crate) fn icon_label_preset(icon: String, label: String) -> EnzLabel<'static> {
    EnzLabel {
        icon: LabelIconSource::Name(icon),
        title: label,
        fill: true,
        wrap: Some(false),
        icon_tone: None,
        title_px: None,
        policy: LabelPolicy::ROW,
        ..enz_label(String::new())
    }
}

impl<'a> EnzLabel<'a> {
    /// Secondary line below the title. Empty by default (no subtitle line).
    /// A nonblank subtitle also becomes the accessible description, even when
    /// hidden by IconOnly display.
    pub fn subtitle(mut self, s: impl Into<String>) -> Self {
        self.subtitle = s.into();
        self
    }
    /// Leading icon by bundled catalog name (canonical Lucide or semantic
    /// alias). Empty by default (no icon); also suppressed when the display is
    /// [`LabelDisplay::TitleOnly`].
    pub fn icon(mut self, i: impl Into<String>) -> Self {
        self.icon = LabelIconSource::Name(i.into());
        self
    }
    /// Use SVG or raster artwork without recoloring it; bundled names and
    /// [`SymbolicSvg`](crate::icon::IconSource::SymbolicSvg) glyphs take label ink.
    pub fn icon_source(mut self, source: impl Into<crate::icon::IconSource>) -> Self {
        self.icon = match source.into() {
            crate::icon::IconSource::Named(name) => LabelIconSource::Name(name),
            artwork => LabelIconSource::Artwork(artwork),
        };
        self
    }

    /// Interpret the icon string as an arbitrary text/Unicode glyph shaped as
    /// text rather than a catalog name (escape hatch, e.g. `"⌘"`).
    pub fn glyph(mut self, glyph: impl Into<String>) -> Self {
        self.icon = LabelIconSource::Glyph(glyph.into());
        self
    }
    /// Which parts to show, in the SwiftUI spelling. Defaults to
    /// [`LabelStyle::TitleAndIcon`].
    pub fn label_style(mut self, s: LabelStyle) -> Self {
        self.display = s.into();
        self
    }
    /// Which parts to show. Defaults to [`LabelDisplay::Both`].
    pub fn display(mut self, display: LabelDisplay) -> Self {
        self.display = display;
        self
    }

    /// Override the semantic tone (default [`LabelTone::Default`]). Resolved
    /// from the runtime theme every render.
    pub fn tone(mut self, tone: LabelTone) -> Self {
        self.tone = tone;
        self
    }
    /// Set the row tone to Subtle and adjust Subtle ink against `backdrop`.
    ///
    /// Explicit colour pins and non-Subtle icon/subtitle tone overrides retain
    /// precedence. The form preset's default Muted icon therefore stays muted.
    pub fn subtle_on(mut self, backdrop: Color) -> Self {
        self.tone = LabelTone::Subtle;
        self.subtle_backdrop = Some(backdrop);
        self
    }
    /// Give a catalog icon or glyph its own tone (runtime-resolved). When unset it
    /// follows the text tone (the [`enz_label`] preset presets it to
    /// [`LabelTone::Muted`]).
    pub fn icon_tone(mut self, tone: LabelTone) -> Self {
        self.icon_tone = Some(tone);
        self
    }
    /// Pin catalog-icon or glyph ink. Beats [`Self::icon_tone`],
    /// [`Self::text_color`] and [`Self::tone`].
    pub fn icon_color(mut self, color: Color) -> Self {
        self.icon_color = Some(color);
        self
    }
    /// Pin the title to an explicit colour, independently of the icon.
    /// Beats [`Self::tone`] for the title.
    ///
    /// Catalog icons and glyphs inherit title ink unless they carry an [`Self::icon_color`]
    /// pin or [`Self::icon_tone`] (which the [`enz_label`] preset sets).
    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = Some(color);
        self
    }
    /// Pin the subtitle (muted) ink colour. Unset by default (resolved from
    /// [`Self::subtitle_tone`] and the runtime theme).
    pub fn muted_color(mut self, c: Color) -> Self {
        self.subtitle_color = Some(c);
        self
    }
    /// Give the subtitle its own semantic tone (default [`LabelTone::Muted`]).
    pub fn subtitle_tone(mut self, tone: LabelTone) -> Self {
        self.subtitle_tone = tone;
        self
    }
    /// Size the subtitle from a typography rung, clearing its pixel-size pin.
    ///
    /// A supplied typography scale then applies to that rung. Both presets
    /// start with an 11-pixel subtitle pin.
    pub fn subtitle_rung(mut self, rung: TextSize) -> Self {
        self.subtitle_rung = Some(rung);
        self.subtitle_px = None;
        self
    }

    pub(crate) fn ink(&self) -> InkRule {
        InkRule {
            tone: self.tone,
            icon_tone: self.icon_tone,
            icon_color: self.icon_color,
            text_color: self.text_color,
            subtle_backdrop: self.subtle_backdrop,
        }
    }

    fn subtitle_ink(&self) -> InkRule {
        InkRule {
            tone: self.subtitle_tone,
            icon_tone: None,
            icon_color: None,
            text_color: self.subtitle_color,
            subtle_backdrop: self.subtle_backdrop,
        }
    }

    /// Resolve icon ink from the supplied palette.
    ///
    /// Precedence is icon colour, icon tone, title colour, then row tone.
    pub fn resolved_icon_color(&self, palette: &Palette) -> Color {
        self.ink().icon(palette)
    }

    /// Resolve title ink from its colour pin or semantic tone.
    pub fn resolved_label_color(&self, palette: &Palette) -> Color {
        self.ink().label(palette)
    }

    /// Resolve subtitle ink from its colour pin or subtitle tone.
    pub fn resolved_subtitle_color(&self, palette: &Palette) -> Color {
        self.subtitle_ink().label(palette)
    }

    /// Override the title size rung (default [`TextSize::Body`] = 13 px).
    ///
    /// Also clears any [`Self::title_size`] pixel pin, so the rung (and a
    /// forwarded [`Self::typography`] scale) takes effect on either preset.
    pub fn size(mut self, size: TextSize) -> Self {
        self.size = size;
        self.title_px = None;
        self
    }
    /// Pin the title size in logical pixels (form default 13).
    ///
    /// Non-positive or non-finite sizes use 13. The pin overrides the size rung
    /// and supplied typography; [`Self::size`] clears it.
    pub fn title_size(mut self, s: f32) -> Self {
        self.title_px =
            Some(enzui_core::metrics::FinitePositive::new_or(s, DEFAULT_TITLE_SIZE).get());
        self
    }
    /// Subtitle text size in px (default 11, the label size). Finite-positive
    /// normalized at the boundary.
    pub fn subtitle_size(mut self, s: f32) -> Self {
        self.subtitle_px =
            Some(enzui_core::metrics::FinitePositive::new_or(s, DEFAULT_SUBTITLE_SIZE).get());
        self
    }
    /// Pin icon size in logical pixels (baseline 16).
    ///
    /// Negative or non-finite values use 16. The form preset also replaces zero;
    /// the row preset retains zero for catalog icons. Text glyphs use a 16-pixel
    /// font fallback at zero. This overrides a supplied sizing scale.
    pub fn icon_size(mut self, s: f32) -> Self {
        self.icon_size = if self.policy.zero_icon_size_is_default {
            enzui_core::metrics::FinitePositive::new_or(s, DEFAULT_ICON_SIZE).get()
        } else {
            sane(s, DEFAULT_ICON_SIZE)
        };
        self.icon_size_explicit = true;
        self
    }
    /// Set the icon-to-text gap in logical pixels (default 8).
    ///
    /// Negative or non-finite values use 8; zero is preserved. Explicit gap or
    /// spacing setters override the supplied spacing scale.
    pub fn gap(mut self, g: f32) -> Self {
        self.spacing = enzui_core::metrics::FiniteNonNeg::new_or(g, DEFAULT_GAP).get();
        self.spacing_explicit = true;
        self
    }
    /// Gap between the icon and the text column in px (default 8) — the
    /// [`enz_icon_label`](crate::icon_label::enz_icon_label) spelling of
    /// [`Self::gap`]. Sanitized identically.
    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = sane(spacing, DEFAULT_GAP);
        self.spacing_explicit = true;
        self
    }
    /// Reserve and clip an explicit icon column in logical pixels.
    ///
    /// Finite non-negative widths are kept, including zero and widths smaller
    /// than the icon. Invalid widths clear the override independently of setter
    /// order; the preset icon frame then applies.
    pub fn slot_width(mut self, width: f32) -> Self {
        self.slot_width = if width.is_finite() && width >= 0.0 {
            Some(width)
        } else {
            None
        };
        self
    }
    /// Pin the title face independently of the application's medium rung.
    pub fn weight(mut self, weight: Weight) -> Self {
        self.weight = Some(weight);
        self
    }
    /// Resolve the title face: explicit pin, forwarded typography, then the
    /// application's published medium rung (compiled baseline: 500).
    pub fn effective_weight(&self) -> Weight {
        self.weight.unwrap_or_else(|| match &self.typography {
            Some(scale) => scale.iced_weight(enzui_theme::WeightRung::Medium),
            None => enzui_theme::text_defaults::weight(enzui_theme::WeightRung::Medium),
        })
    }
    /// Set the shaping policy of the title, subtitle, and text-glyph icon.
    ///
    /// It defaults to [`TextShaping::Auto`], matching [`enz_text`]. Use
    /// [`TextShaping::Advanced`] for content-stable prose or
    /// [`TextShaping::Basic`] for a controlled ASCII label whose no-ligature
    /// geometry is intentional.
    pub fn shaping(mut self, shaping: impl Into<TextShaping>) -> Self {
        self.shaping = shaping.into();
        self
    }
    /// Pin the title line-box height in logical pixels.
    ///
    /// Non-positive or non-finite values clear the pin and restore the text
    /// primitive's natural line box. Subtitle and glyph line boxes are unaffected.
    pub fn line_height(mut self, px: f32) -> Self {
        self.line_height = if px.is_finite() && px > 0.0 {
            Some(px)
        } else {
            None
        };
        self
    }
    /// Pin the title line box to the current effective icon size.
    ///
    /// This snapshots the size at this call; later icon-size or sizing-scale
    /// changes do not update it. A zero effective size clears the line-height pin.
    pub fn co_centre_to_icon(self) -> Self {
        let px = self.effective_icon_size();
        self.line_height(px)
    }
    /// Read the title line-box pin, or `None` for the natural text line box.
    pub fn effective_line_height(&self) -> Option<f32> {
        self.line_height
    }
    /// Supply the typography scale used to build title and subtitle runs.
    ///
    /// Sizes resolve through their rungs unless pixel-pinned. The view owner
    /// must supply updates; layout has no runtime theme. The form preset's title
    /// and both presets' subtitles start pixel-pinned.
    pub fn typography(mut self, scale: enzui_theme::TypographyScale) -> Self {
        self.typography = Some(scale);
        self
    }
    /// Supply the icon sizing scale.
    ///
    /// Its `icon_sm` field applies unless [`Self::icon_size`] was explicitly set.
    pub fn sizing(mut self, sizing: enzui_theme::SizingTokens) -> Self {
        self.sizing = Some(sizing);
        self
    }
    /// Supply the spacing scale; its small rung applies unless gap or spacing was set.
    pub fn spacing_scale(mut self, spacing: enzui_theme::SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }
    /// Stretch the label to fill available width (default on the
    /// [`enz_icon_label`](crate::icon_label::enz_icon_label) preset). When
    /// `false` it shrink-wraps its content (the [`enz_label`] default).
    pub fn fill(mut self, fill: bool) -> Self {
        self.fill = fill;
        self
    }
    /// Choose wrapping or single-line end elision.
    ///
    /// The form preset keeps the text primitive's wrapping default; the row
    /// preset defaults to single-line elision.
    pub fn wrap(mut self, wrap: bool) -> Self {
        self.wrap = Some(wrap);
        self
    }

    /// Override the accessible-name policy (see [`LabelA11y`]).
    pub fn a11y(mut self, a11y: LabelA11y) -> Self {
        self.a11y = a11y;
        self
    }

    /// Resolve icon size from an explicit setter, supplied scale, or desktop text scale.
    ///
    /// Invalid supplied `icon_sm` values use 16; zero is preserved. A zero text
    /// glyph still uses the positive font-size fallback during construction.
    pub fn effective_icon_size(&self) -> f32 {
        if self.icon_size_explicit {
            return self.icon_size;
        }
        match self.sizing {
            Some(s) => sane(s.icon_sm, DEFAULT_ICON_SIZE),

            None => enzui_theme::text_scale::ui_px(DEFAULT_ICON_SIZE),
        }
    }

    /// The icon↔text gap this label will actually build with: the explicit
    /// [`Self::spacing`] / [`Self::gap`] override, else the forwarded
    /// [`Self::spacing_scale`]'s `sm()` rung, else [`DEFAULT_GAP`].
    pub fn effective_spacing(&self) -> f32 {
        if self.spacing_explicit {
            return self.spacing;
        }
        match self.spacing_scale {
            Some(s) => sane(s.sm(), DEFAULT_GAP),
            None => DEFAULT_GAP,
        }
    }

    /// Whether the leading icon will render: an icon is set and the display is
    /// not [`LabelDisplay::TitleOnly`].
    pub fn show_icon(&self) -> bool {
        !self.icon.is_empty() && self.display != LabelDisplay::TitleOnly
    }
    /// Whether the text column will render: the display is not
    /// [`LabelDisplay::IconOnly`], and either the title is non-empty or the
    /// preset lays out an empty text column
    /// ([`LabelPolicy::render_empty_title`]).
    pub fn show_title(&self) -> bool {
        self.display != LabelDisplay::IconOnly
            && (self.policy.render_empty_title || !self.title.is_empty())
    }
    /// Whether the subtitle line will render: the text column renders and the
    /// subtitle is non-empty.
    pub fn show_subtitle(&self) -> bool {
        self.show_title() && !self.subtitle.is_empty()
    }
    /// The preset policy this label was built with.
    pub fn policy(&self) -> LabelPolicy {
        self.policy
    }

    pub(crate) fn build_label(&self) -> crate::enz_text::EnzText {
        let ink = self.ink();

        let mut label = enz_text(self.title.clone())
            .color_resolved(move |theme| ink.label(&crate::theme_lookup::palette(theme)))
            .weight(self.effective_weight())
            .shaping(self.shaping)
            // Raw runs must stay silent when the aggregate name policy publishes no node.
            .announce(false);
        label = match self.title_px {
            Some(px) => label.size_px(px),
            None => label.size(self.size),
        };
        if let Some(px) = self.line_height {
            label = label.line_height(LineHeight::Absolute(px.into()));
        }
        if let Some(t) = self.typography {
            label = label.typography(t);
        }
        self.apply_flow(label)
    }

    fn build_subtitle(&self) -> crate::enz_text::EnzText {
        let ink = self.subtitle_ink();
        let mut sub = enz_text(self.subtitle.clone())
            .color_resolved(move |theme| ink.label(&crate::theme_lookup::palette(theme)))
            .shaping(self.shaping)
            .announce(false);
        sub = match (self.subtitle_px, self.subtitle_rung) {
            (Some(px), _) => sub.size_px(px),
            (None, Some(rung)) => sub.size(rung),
            (None, None) => sub.size(self.size),
        };
        if let Some(t) = self.typography {
            sub = sub.typography(t);
        }
        self.apply_flow(sub)
    }

    fn apply_flow(&self, run: crate::enz_text::EnzText) -> crate::enz_text::EnzText {
        let mut run = match self.wrap {
            None => return run,
            Some(true) => run.wrap(true),

            Some(false) => run.single_line_elided(),
        };
        if self.fill {
            run = run.full_width();
        }
        run
    }

    fn frame_icon<'b, Message: 'b>(
        &self,
        element: Element<'b, Message>,
        icon_size: f32,
    ) -> Element<'b, Message> {
        if let Some(w) = self.slot_width {
            return container(element)
                .width(Length::Fixed(w))
                .align_x(Alignment::Center)
                .clip(true)
                .into();
        }
        match self.policy.icon_frame {
            IconFrame::Intrinsic => element,
            IconFrame::Square => {
                let side = icon_size + ICON_FRAME_PAD;
                container(element)
                    .width(Length::Fixed(side))
                    .height(Length::Fixed(side))
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center)
                    .into()
            }
        }
    }

    /// Consume the builder and produce the widget tree.
    ///
    /// Colours resolve from the runtime `iced::Theme` per render: explicit
    /// caller pins win, unpinned defaults follow the theme. The label owns its
    /// content outright (the `'a` parameter is phantom, API-compat only), so
    /// the element carries any caller-needed lifetime — including shorter than
    /// the builder's.
    pub fn into_element<'b, Message: 'b>(self) -> Element<'b, Message> {
        let show_icon = self.show_icon();
        let show_title = self.show_title();

        let icon_size = self.effective_icon_size();
        let gap = self.effective_spacing();

        let mut children: Vec<Element<'b, Message>> = Vec::with_capacity(2);

        if show_icon {
            let ink = self.ink();
            let icon_el: Element<'b, Message> = match &self.icon {
                LabelIconSource::Name(name) => enz_icon(name)
                    .size(icon_size)
                    .tint_resolved(move |theme| ink.icon(&crate::theme_lookup::palette(theme)))
                    .into(),
                LabelIconSource::Artwork(source) => {
                    source.clone().into_slot_element(icon_size, |icon| {
                        icon.tint_resolved(move |theme| {
                            ink.icon(&crate::theme_lookup::palette(theme))
                        })
                    })
                }
                LabelIconSource::Glyph(glyph) => {
                    enz_text(glyph.clone())
                        // Text fonts need positive size; catalog icons can preserve a zero extent.
                        .size_px(sane_font_size(icon_size, DEFAULT_ICON_SIZE))
                        .shaping(self.shaping)
                        .color_resolved(move |theme| ink.icon(&crate::theme_lookup::palette(theme)))
                        .into()
                }
            };

            children
                .push(crate::decorative::decorative(self.frame_icon(icon_el, icon_size)).into());
        }

        if show_title {
            let title = self.build_label();
            if self.show_subtitle() {
                let mut col = iced::widget::Column::new()
                    .spacing(SUBTITLE_GAP)
                    .push(title)
                    .push(self.build_subtitle());
                if self.fill {
                    col = col.width(Length::Fill);
                }
                children.push(col.into());
            } else {
                children.push(title.into());
            }
        }

        let spacing = if show_icon && show_title { gap } else { 0.0 };

        let mut r = row(children).spacing(spacing).align_y(Alignment::Center);
        r = if self.fill {
            r.width(Length::Fill)
        } else {
            r.width(Length::Shrink)
        };

        if self.a11y == LabelA11y::Decorative {
            return crate::decorative::decorative(r).into();
        }

        if self.policy.silent_when_nothing_renders && !(show_icon || show_title) {
            return r.into();
        }
        match self
            .a11y
            .name_for(&self.title, self.policy.trim_blank_title)
        {
            Some(name) => {
                let mut semantics = Semantics::new(Role::Label).label(name);
                if !self.subtitle.trim().is_empty() {
                    semantics = semantics.description(self.subtitle.clone());
                }
                SemanticsContainer::leaf(r, semantics).into()
            }

            None => r.into(),
        }
    }
}

// The builder lifetime is phantom; conversion must not force the message lifetime to match it.
impl<'a, 'b, Message: 'b> From<EnzLabel<'a>> for Element<'b, Message> {
    fn from(w: EnzLabel<'a>) -> Self {
        w.into_element()
    }
}
