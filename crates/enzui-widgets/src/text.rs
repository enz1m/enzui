//! EnzText: token-driven typography primitive — the SwiftUI-style
//! typography leaf other components compose over (size ramp, weight, tone,
//! italic, alignment). A subset port: per-token fill-height, valign,
//! intrinsic-width, selectable, and inline-span decorations are out of scope.
//!
//! # Shaping
//!
//! Plain and measured text default to [`TextShaping::Auto`] for compatibility;
//! prose can opt into [`Advanced`](TextShaping::Advanced), controlled code or
//! chrome into [`Basic`](TextShaping::Basic).
//!
//! # Tones and backdrops
//!
//! A subtle caption on a non-default fill must use [`EnzText::subtle_on`]
//! (with [`muted_on`](EnzText::muted_on) / [`tone_on`](EnzText::tone_on) for
//! the sibling rungs): the weak ink resolves against the actual backdrop,
//! because a leaf has no access to its parent's fill during the style pass.
//!
//! # This run owns its ink
//!
//! `enz_text` resolves a colour on every path and installs it, so a
//! container's inherited `text_color` never reaches it — pin the run with
//! `.color(..)`, or opt into the inherited channel with
//! [`EnzText::inherit_color(true)`](EnzText::inherit_color). Rationale,
//! ancestor/leaf lists, and a pixel-measured probe live in
//! [`crate::inherited_ink`].

use enzui_core::semantics::{HeadingLevel, Role, Semantics};
use enzui_theme::{ContrastFloor, EnzTheme, FontFamily, FontRole, Palette, SemanticTone};
use iced::font::{Stretch, Style, Weight};
use iced::widget::text;
use iced::{Color, Element, Font};

/// Text shaping policy shared by [`EnzText`] and exact text measurement.
///
/// [`Auto`](Self::Auto) is the compatibility default and follows iced's
/// content-sensitive heuristic: ASCII selects Basic while any non-ASCII
/// character selects Advanced. [`Advanced`](Self::Advanced) is the stable
/// prose policy, with kerning, ligatures, fallback, and complex-script shaping
/// independent of content. [`Basic`](Self::Basic) is the explicit low-cost,
/// no-ligature mode for text whose font and repertoire are controlled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TextShaping {
    /// Iced compatibility mode: Basic for ASCII, Advanced otherwise.
    #[default]
    Auto,
    /// Stable full shaping and font fallback for prose and user content.
    Advanced,
    /// Per-codepoint shaping without kerning, ligatures, or font fallback.
    Basic,
}

impl From<TextShaping> for iced::widget::text::Shaping {
    fn from(policy: TextShaping) -> Self {
        match policy {
            TextShaping::Advanced => Self::Advanced,
            TextShaping::Basic => Self::Basic,
            TextShaping::Auto => Self::Auto,
        }
    }
}

impl From<iced::widget::text::Shaping> for TextShaping {
    fn from(shaping: iced::widget::text::Shaping) -> Self {
        match shaping {
            iced::widget::text::Shaping::Advanced => Self::Advanced,
            iced::widget::text::Shaping::Basic => Self::Basic,
            iced::widget::text::Shaping::Auto => Self::Auto,
        }
    }
}

/// Proportional factor used by [`line_box_px`] before rounding and clamping.
pub const LINE_BOX_FACTOR: f32 = 1.35;

/// Round `size_px * LINE_BOX_FACTOR` and clamp to the logical pixel bounds
/// [`MIN_LINE_BOX_PX`]`..=`[`MAX_LINE_BOX_PX`]. NaN and negative infinity map
/// to the minimum; positive infinity maps to the maximum.
///
/// Explicit size pins and typography scale rungs use this proportional box;
/// unscaled semantic sizes use [`TextSize::line_height_px`].
pub fn line_box_px(size_px: f32) -> f32 {
    clamp_line_box((size_px * LINE_BOX_FACTOR).round())
}

/// Minimum logical line-box height, preventing zero or negative shaping steps.
pub const MIN_LINE_BOX_PX: f32 = 1.0;

/// Maximum logical line-box height, also used for positive infinity.
pub const MAX_LINE_BOX_PX: f32 = 4096.0;

#[allow(
    clippy::manual_clamp,
    reason = "non-finite input is handled first, which `clamp` would not do"
)]
pub(crate) fn clamp_line_box(px: f32) -> f32 {
    if !px.is_finite() {
        return if px == f32::INFINITY {
            MAX_LINE_BOX_PX
        } else {
            MIN_LINE_BOX_PX
        };
    }
    px.max(MIN_LINE_BOX_PX).min(MAX_LINE_BOX_PX)
}

// ---------------------------------------------------------------------------
// Size enum (SwiftUI-style ramp)
// ---------------------------------------------------------------------------

/// SwiftUI-style text-size ramp. Each variant maps to a fixed px size and
/// a size-implied default weight. Use with `.weight()` to override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextSize {
    /// 10 px, Regular. Very small captions.
    Caption2,
    /// 11 px, Regular. Captions.
    Caption,
    /// 12 px, Regular. Footnotes.
    Footnote,
    /// 12 px, Regular. Subheadlines.
    Subheadline,
    /// 13 px, Regular. Callouts.
    Callout,
    /// 13 px, Regular. Body text (the default).
    Body,
    /// 13 px, Semibold. Emphasised body.
    Headline,
    /// 17 px, Semibold. Section title.
    Title3,
    /// 22 px, Bold. Page/pane title.
    Title2,
    /// 28 px, Bold. Screen title.
    Title1,
    /// 34 px, Bold. Hero.
    LargeTitle,
    // --- Legacy ramp (Xs/Sm/Md/Lg/Xl) ---
    /// Legacy: 10 px, Regular.
    Xs,
    /// Legacy: 11 px, Regular.
    Sm,
    /// Legacy: 13 px, Regular.
    Md,
    /// Legacy: 17 px, Semibold.
    Lg,
    /// Legacy: 22 px, Bold.
    Xl,
}

impl TextSize {
    /// Resolve the px size.
    pub fn px(self) -> f32 {
        match self {
            Self::Caption2 | Self::Xs => 10.0,
            Self::Caption | Self::Sm => 11.0,
            Self::Footnote | Self::Subheadline => 12.0,
            Self::Callout | Self::Body | Self::Headline | Self::Md => 13.0,
            Self::Title3 | Self::Lg => 17.0,
            Self::Title2 | Self::Xl => 22.0,
            Self::Title1 => 28.0,
            Self::LargeTitle => 34.0,
        }
    }

    /// Read the variant's logical pixel size from a typography scale.
    /// Every variant returns Some; several variants share one scale rung.
    /// Values are returned as supplied, without size validation or scaling.
    ///
    /// | Variants | Scale field |
    /// |---|---|
    /// | Caption2, Xs | caption_size |
    /// | Caption, Sm | label_size |
    /// | Footnote, Subheadline | footnote_size |
    /// | Callout, Body, Headline, Md | body_size |
    /// | Title3, Lg | title3_size |
    /// | Title2, Xl | large_title_size |
    /// | Title1 | title1_size |
    /// | LargeTitle | hero_size |
    pub fn scale_size(self, scale: &enzui_theme::TypographyScale) -> Option<f32> {
        match self {
            Self::Caption2 | Self::Xs => Some(scale.caption_size),
            Self::Caption | Self::Sm => Some(scale.label_size),
            Self::Footnote | Self::Subheadline => Some(scale.footnote_size),
            Self::Callout | Self::Body | Self::Headline | Self::Md => Some(scale.body_size),
            Self::Title3 | Self::Lg => Some(scale.title3_size),
            Self::Title2 | Self::Xl => Some(scale.large_title_size),
            Self::Title1 => Some(scale.title1_size),
            Self::LargeTitle => Some(scale.hero_size),
        }
    }

    /// Resolve the size-implied default weight.
    ///
    /// These are the compiled baseline faces. Unpinned text resolves the
    /// application's current rung through [`Self::resolve_weight`] instead.
    pub fn default_weight(self) -> Weight {
        match self {
            Self::Headline | Self::Title3 | Self::Lg => Weight::Semibold,
            Self::LargeTitle => Weight::ExtraBold,
            Self::Title2 | Self::Title1 | Self::Xl => Weight::Bold,
            _ => Weight::Normal,
        }
    }

    /// The semantic weight rung this variant sits on.
    ///
    /// The theme-facing half of the weight ramp: the active scale says which
    /// CSS number the rung resolves to, so editing one token restyles every
    /// variant on that rung and nothing else.
    ///
    /// | rung | variants |
    /// |---|---|
    /// | `Regular` | `Caption2`, `Caption`, `Footnote`, `Subheadline`, `Callout`, `Body`, `Xs`, `Sm`, `Md` |
    /// | `Semibold` | `Headline`, `Title3`, `Lg` |
    /// | `Bold` | `Title2`, `Title1`, `Xl` |
    /// | `ExtraBold` | `LargeTitle` |
    ///
    /// No variant sits on `Medium`; that rung belongs to
    /// [`EnzLabel::effective_weight`](crate::label::EnzLabel::effective_weight).
    pub fn weight_rung(self) -> enzui_theme::WeightRung {
        use enzui_theme::WeightRung;
        match self {
            Self::Headline | Self::Title3 | Self::Lg => WeightRung::Semibold,
            Self::LargeTitle => WeightRung::ExtraBold,
            Self::Title2 | Self::Title1 | Self::Xl => WeightRung::Bold,
            _ => WeightRung::Regular,
        }
    }

    /// Resolve the effective font weight for this variant under an optional
    /// active [`TypographyScale`](enzui_theme::TypographyScale).
    ///
    /// A forwarded scale takes precedence over the application's published
    /// weight rung. An explicit [`EnzText::weight`] pin is applied by the owner.
    pub fn resolve_weight(self, scale: Option<&enzui_theme::TypographyScale>) -> Weight {
        match scale {
            Some(scale) => scale.iced_weight(self.weight_rung()),
            None => enzui_theme::text_defaults::weight(self.weight_rung()),
        }
    }

    /// Baseline semantic line-box height in logical pixels. [`Self::resolve_line_height`]
    /// applies ambient scaling when no typography scale is supplied.
    pub fn line_height_px(self) -> f32 {
        match self {
            Self::Caption2 | Self::Xs => 14.0,
            Self::Caption | Self::Sm => 15.0,
            Self::Footnote | Self::Subheadline => 16.0,
            Self::Callout | Self::Body | Self::Headline | Self::Md => 18.0,
            Self::Title3 | Self::Lg => 22.0,
            Self::Title2 | Self::Xl => 28.0,
            Self::Title1 => 34.0,
            Self::LargeTitle => 41.0,
        }
    }

    /// Read a supplied typography scale rung verbatim, or apply the ambient
    /// text factor to the baseline size. A caller supplying a scale must apply
    /// its desired factor to that scale; this method adds none.
    /// [`EnzText::size_px`] pins are handled by the owning text run.
    pub fn resolve_size(self, scale: Option<&enzui_theme::TypographyScale>) -> f32 {
        if let Some(scale) = scale
            && let Some(px) = self.scale_size(scale)
        {
            return px;
        }
        enzui_theme::text_scale::ui_px(self.px())
    }

    /// Resolve a logical line-box height: [`line_box_px`] of a supplied
    /// scale rung, or the baseline semantic box multiplied by the ambient factor.
    /// The bare-ramp branch does not clamp; [`EnzText::effective_line_height_px`]
    /// clamps its final result after explicit overrides.
    ///
    /// A default scale can change line boxes even when sizes match: Title3 has
    /// a 22-pixel baseline box and a 23-pixel proportional box.
    pub fn resolve_line_height(self, scale: Option<&enzui_theme::TypographyScale>) -> f32 {
        if let Some(scale) = scale
            && let Some(px) = self.scale_size(scale)
        {
            return line_box_px(px);
        }
        enzui_theme::text_scale::ui_px(self.line_height_px())
    }
}

// ---------------------------------------------------------------------------
// Tone enum
// ---------------------------------------------------------------------------

/// Semantic text tone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextTone {
    /// `text` — full-strength (default).
    Default,
    /// `text_muted`.
    Muted,
    /// `text_subtle`.
    Subtle,
    /// `accent`.
    Accent,
    /// `success`.
    Success,
    /// `warning`.
    Warning,
    /// `destructive`.
    Danger,
    /// `disabled` — the muted grey used for disabled control text. Distinct
    /// from `Muted`, which is for secondary but
    /// still-active copy.
    Disabled,
}

/// Colour tokens for text tones. Wired from `Palette`.
#[derive(Debug, Clone, Copy)]
pub struct TextTokens {
    /// Ink for [`TextTone::Default`] (default `text`).
    pub default_color: Color,
    /// Ink for [`TextTone::Muted`] secondary copy (default `text_muted`).
    pub muted_color: Color,
    /// Ink for [`TextTone::Subtle`] tertiary copy (default `text_subtle`).
    pub subtle_color: Color,
    /// Ink for [`TextTone::Accent`] text (default `accent`).
    pub accent_color: Color,
    /// Ink for [`TextTone::Success`] text — the audited `tone_success_fg`, not
    /// the raw success fill.
    pub success_color: Color,
    /// Ink for [`TextTone::Warning`] text — the audited `tone_warning_fg`.
    pub warning_color: Color,
    /// Ink for [`TextTone::Danger`] text — the audited `tone_error_fg`.
    pub danger_color: Color,
    /// Ink for [`TextTone::Disabled`] text (default `disabled`).
    pub disabled_color: Color,
}

impl TextTokens {
    /// Derive from a palette.
    ///
    /// Semantic status tones use the audited text foregrounds
    /// (`tone_success_fg`/`tone_warning_fg`/`tone_error_fg`) rather than the
    /// raw `success`/`warning`/`destructive` fills: the fills target
    /// chips/icons/borders, these are the contrast-tuned values for
    /// tone-coloured *text* on the app surface.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            default_color: p.text,
            muted_color: p.text_muted,
            subtle_color: p.text_subtle,
            accent_color: p.accent,
            success_color: p.tone_success_fg,
            warning_color: p.tone_warning_fg,
            danger_color: p.tone_error_fg,
            disabled_color: p.disabled,
        }
    }
    /// Derive from a palette with the **weak rungs resolved against the fill
    /// they will actually be painted on**.
    ///
    /// [`Self::from_palette`] hands back the raw `text_muted` / `text_subtle`
    /// tokens, correct only while the run lands on a backdrop those already
    /// clear 4.5:1 against. A widget that *chooses its own fill* knows that
    /// backdrop at resolve time and no caller can supply it: in the shipped
    /// light preset both weak tokens measure below the SC 1.4.3 body-text
    /// floor on `surface_alt`.
    ///
    /// [`TextTone::Muted`] resolves through [`Palette::text_muted_on`] and
    /// [`TextTone::Subtle`] through [`Palette::subtle_on`] at
    /// [`ContrastFloor::Text`]; both keep the raw token byte-for-byte wherever
    /// it already clears the floor. The three audited status rungs resolve the
    /// same way through [`Palette::tone_on`]; an unreachable hue degrades to
    /// the honest best-available colour.
    ///
    /// Every other rung is left as [`Self::from_palette`] produces it. In
    /// particular [`TextTone::Disabled`] is not resolved here: an inactive
    /// control is governed by the palette's disabled *band*, and promoting it
    /// to body ink would erase the disabled state.
    ///
    /// `backdrop` must be the opaque fill the widget paints; pass the base
    /// surface a translucent wash floats over, not the wash.
    pub fn from_palette_on(p: &Palette, backdrop: Color) -> Self {
        Self {
            muted_color: p.text_muted_on(backdrop),
            subtle_color: p.subtle_on(backdrop, ContrastFloor::Text),
            success_color: p.tone_on(SemanticTone::Success, backdrop).color(),
            warning_color: p.tone_on(SemanticTone::Warning, backdrop).color(),
            danger_color: p.tone_on(SemanticTone::Error, backdrop).color(),
            ..Self::from_palette(p)
        }
    }
    /// Dark preset.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Light preset.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
    /// Look up the colour for a tone.
    pub fn color_for(&self, tone: TextTone) -> Color {
        match tone {
            TextTone::Default => self.default_color,
            TextTone::Muted => self.muted_color,
            TextTone::Subtle => self.subtle_color,
            TextTone::Accent => self.accent_color,
            TextTone::Success => self.success_color,
            TextTone::Warning => self.warning_color,
            TextTone::Danger => self.danger_color,
            TextTone::Disabled => self.disabled_color,
        }
    }
}

// ---------------------------------------------------------------------------
// Alignment enum
// ---------------------------------------------------------------------------

/// Horizontal alignment. Maps to iced's `iced::alignment::Horizontal`.
///
/// `Start` and `End` follow the writing direction: in a right-to-left locale
/// `Start` is the right edge. `Left` and `Right` are physical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlign {
    /// The start edge of the writing direction (the default).
    #[default]
    Start,
    /// The end edge of the writing direction.
    End,
    /// Left, in every direction.
    Left,
    /// Centre.
    Center,
    /// Right, in every direction.
    Right,
}

impl TextAlign {
    /// Maps to iced's `iced::alignment::Horizontal` for the ambient writing
    /// direction.
    pub fn into_horizontal(self) -> iced::alignment::Horizontal {
        let rtl = enzui_core::placement::ambient_direction()
            == enzui_core::placement::Direction::RightToLeft;
        match self {
            TextAlign::Start if rtl => iced::alignment::Horizontal::Right,
            TextAlign::End if rtl => iced::alignment::Horizontal::Left,
            TextAlign::Start | TextAlign::Left => iced::alignment::Horizontal::Left,
            TextAlign::Center => iced::alignment::Horizontal::Center,
            TextAlign::End | TextAlign::Right => iced::alignment::Horizontal::Right,
        }
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// A runtime colour resolver: given the current `iced::Theme`, returns the
/// ink to paint this run with this frame. Used by widgets that own a token set
/// `enz_text.rs` does not know about, so the run re-tints on a theme switch in
/// lock-step with that widget's own colours.
///
/// The text-side twin of [`crate::icon::EnzIcon::tint_resolved`]:
/// [`EnzText::color`] pins a *fixed* colour (stops tracking the theme) and
/// [`EnzText::tone`] names only the eight shared [`TextTone`]s; a widget-owned
/// colour token is neither.
type ColorResolver = std::rc::Rc<dyn Fn(&iced::Theme) -> Color + 'static>;

/// The typography primitive. Stores content + resolved styling; converts
/// to an iced `Element` on demand.
#[derive(Clone)]
#[must_use]
pub struct EnzText {
    content: String,
    size: TextSize,
    /// Arbitrary pixel-size override; when set, wins over the `size` ramp.
    size_px: Option<f32>,
    tone: TextTone,
    /// Active runtime [`TypographyScale`], forwarded by the view owner at build
    /// time. `None` = the local [`TextSize`] ramp. When set, a
    /// variant with a semantic rung ([`TextSize::scale_size`]) resolves its px
    /// (and a proportional line height) from the scale, so a theme with a
    /// custom scale actually resizes/reflows text. An explicit
    /// [`EnzText::size_px`] / [`EnzText::line_height`] override still wins.
    typography: Option<enzui_theme::TypographyScale>,
    /// Explicit colour override. When set, wins over both the [`TextTone`] and
    /// the runtime-theme lookup — for text drawn on a coloured fill (e.g. white
    /// on an accent chat bubble) where no palette tone applies.
    color_override: Option<Color>,
    /// Backdrop and WCAG clause supplied by [`Self::subtle_on`],
    /// [`Self::subtle_on_with_floor`] or [`Self::muted_on`]. `None` preserves
    /// the raw `text_subtle` / `text_muted` token for every existing caller;
    /// `Some` resolves the selected tone against that backdrop at paint time.
    /// The clause is carried for the subtle rung; the muted rung has only the
    /// body-text floor today.
    tone_backdrop: Option<(Color, ContrastFloor)>,
    /// Per-render colour resolver. When set, wins over *everything* else —
    /// the widget that installed it owns this run's ink.
    color_resolver: Option<ColorResolver>,
    /// Inherit the enclosing renderer style's `text_color` instead of pinning
    /// one. When `true`, [`EnzText::into_element`] installs no text
    /// style, so the run draws with whatever `text_color` its parent hands
    /// down — e.g. a `button`'s status-dependent ink, which is how a
    /// breadcrumb crumb-link tints without knowing focus status. Overridden by
    /// an explicit [`EnzText::color`] / [`EnzText::color_resolved`] /
    /// [`EnzText::tokens`]; those pin ink and leave nothing to inherit.
    inherit_color: bool,
    weight: Option<Weight>,
    weight_rung: Option<enzui_theme::WeightRung>,
    italic: bool,
    numeric: iced::font::Numeric,
    shaping: TextShaping,
    align: TextAlign,
    tokens: Option<TextTokens>,
    /// Line wrapping. **Default `iced::widget::text::Wrapping::Word`**, matching iced's
    /// own `#[default]` — a run given a bound reflows rather than deleting
    /// its overflow. Opt out with [`EnzText::no_wrap`] for the *label* case
    /// (a table cell, a chip, a status pill) where a stable single-line row
    /// height matters more than the tail of the string.
    wrapping: iced::widget::text::Wrapping,
    /// Ellipsis strategy. **Default `End`**, under either wrapping mode: a
    /// run that is *bounded in height* (by [`EnzText::lines`] or a fixed
    /// parent) ellipsizes its last visible line rather than hard-clipping
    /// mid-glyph. Unbounded, there is no line to elide and this costs
    /// nothing.
    ellipsis: iced::widget::text::Ellipsis,
    /// Explicit line height; `None` = the size-implied semantic ramp
    /// ([`TextSize::line_height_px`]).
    line_height: Option<iced::widget::text::LineHeight>,
    /// Outer width. **Default `None` = Shrink** (shrink-wrapped; opt into
    /// fill via [`EnzText::full_width`]).
    width: Option<iced::Length>,
    /// Line clamp : `0` = unlimited (the default), `1` =
    /// single-line, `n` = at most `n` rendered lines. Implemented as a
    /// height cap of `n × line_height`, since iced's `Text` has no native
    /// max-line-count knob. See [`EnzText::lines`] for the exact contract.
    lines: u16,
    /// Text max-width cap : `None` = uncapped. When set
    /// the run is wrapped in [`crate::max_width::MaxWidth`], which shrinks the
    /// layout limits so the paragraph never exceeds `cap` px (and wraps past
    /// it). iced's `Text` has no `max_width` field, so this is layout-level.
    text_max_width: Option<f32>,
    /// Whether this text should be announced to assistive tech. `false`
    /// suppresses duplicate parent/child announcement.
    /// The platform adapter consumes this; stored + tested
    /// here as the contract.
    announce: bool,
    font_family: Option<FontFamily>,
    font_role: FontRole,
    /// Elision audit: tracks whether the caller explicitly
    /// asked for single-line elision via [`Self::single_line_elided`].
    ///
    /// Separate from [`Self::wrapping`] / [`Self::ellipsis`] because those
    /// cannot distinguish "an *audited* single-line label" from "single-line
    /// geometry" via [`Self::no_wrap`] — both write the identical pair. Only
    /// an explicit opt-in marks the run as one whose overflow is a defect the
    /// audit should observe. Pure audit metadata: neither paint nor layout
    /// consults it.
    single_line_elided: bool,
    /// Elision audit: controls publishing an `AccessibilityCheck::Elision`
    /// node for this run.
    ///
    /// **Default `false`, auto-enabled to `true`** by
    /// [`Self::single_line_elided`]; explicitly disable for deliberate
    /// elision. [`Self::into_element`] inserts the probe only when both this
    /// and `single_line_elided` hold. Setting this without opting into
    /// single-line elision is a no-op.
    audit_elision: bool,
    /// Optional stable id copied onto the elision-scan probe so a finding
    /// names the control. Read only when [`Self::audit_elision`] is `true`.
    audit_elision_id: Option<iced::advanced::widget::Id>,
    /// the outline rung this run claims, or `None` (the default) for a
    /// plain non-structural label.
    ///
    /// **Default `None`**, so an unmigrated `enz_text` publishes exactly the
    /// anonymous `Role::Label` it always has. Read at [`Self::into_element`],
    /// which publishes `Role::Heading` + `aria-level`; nothing about paint or
    /// layout consults it — a heading looks exactly like the same text
    /// without the claim. Size and structure are independent.
    heading: Option<HeadingLevel>,
    /// A byte range of `content` drawn one weight rung heavier, such as the
    /// matched word in a search result. `None` (the default) draws the run as
    /// one plain paragraph.
    emphasis: Option<std::ops::Range<usize>>,
}

/// Hand-written because `ColorResolver` is a `dyn Fn` and cannot derive
/// `Debug`. Every other field is forwarded verbatim; the resolver is reported
/// as a bool so `{:?}` still says whether one is installed.
impl std::fmt::Debug for EnzText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnzText")
            .field("content", &self.content)
            .field("size", &self.size)
            .field("size_px", &self.size_px)
            .field("typography", &self.typography)
            .field("tone", &self.tone)
            .field("color_override", &self.color_override)
            .field("tone_backdrop", &self.tone_backdrop)
            .field("color_resolver", &self.color_resolver.is_some())
            .field("weight", &self.weight)
            .field("italic", &self.italic)
            .field("shaping", &self.shaping)
            .field("align", &self.align)
            .field("tokens", &self.tokens)
            .field("wrapping", &self.wrapping)
            .field("ellipsis", &self.ellipsis)
            .field("line_height", &self.line_height)
            .field("width", &self.width)
            .field("lines", &self.lines)
            .field("text_max_width", &self.text_max_width)
            .field("announce", &self.announce)
            .field("font_family", &self.font_family)
            .field("font_role", &self.font_role)
            .field("single_line_elided", &self.single_line_elided)
            .field("audit_elision", &self.audit_elision)
            .field("audit_elision_id", &self.audit_elision_id.is_some())
            .field("heading", &self.heading)
            .field("emphasis", &self.emphasis)
            .finish()
    }
}

impl EnzText {
    /// Override the size ramp variant (default `Body`).
    pub fn size(mut self, size: TextSize) -> Self {
        self.size = size;
        self
    }

    /// Override with an arbitrary pixel size (wins over the [`TypographyScale`]
    /// and the [`TextSize`] ramp). Non-finite/`<= 0` values are ignored.
    ///
    /// [`TypographyScale`]: enzui_theme::TypographyScale
    pub fn size_px(mut self, px: f32) -> Self {
        if px.is_finite() && px > 0.0 {
            self.size_px = Some(px);
        }
        self
    }

    /// Forward the active runtime [`TypographyScale`].
    ///
    /// The production seam that makes a custom scale actually **resize and
    /// reflow** text: iced computes layout before the runtime theme exists,
    /// so a leaf cannot follow a typography token from inside its own layout
    /// pass — the view owner holding the live `EnzTheme` forwards it.
    ///
    /// Precedence (highest first): an explicit [`Self::size_px`] pin → the
    /// forwarded scale rung ([`TextSize::scale_size`]) → the local
    /// [`TextSize`] ramp. A variant with no rung keeps its local px. Line
    /// height follows: when the scale supplies the size and no explicit
    /// [`Self::line_height`] is set, the line box is `scale_size * 1.35` so it
    /// reflows with the size.
    ///
    /// [`TypographyScale`]: enzui_theme::TypographyScale
    pub fn typography(mut self, scale: enzui_theme::TypographyScale) -> Self {
        self.typography = Some(scale);
        self
    }

    /// Pin the selected role's family from this theme.
    ///
    /// Set [`Self::font_role`] first to select `Monospace` or `Document`.
    /// Weight, italic and stretch remain independent. Without this pin the
    /// selected role follows the published application font catalog.
    pub fn font_from_theme(mut self, theme: &EnzTheme) -> Self {
        self.font_family = Some(theme.font_catalog.family(self.font_role).clone());
        self
    }

    /// Select the font role, defaulting to [`FontRole::Ui`].
    ///
    /// Unpinned families follow the published application catalog. A preceding
    /// [`Self::font_from_theme`] remains pinned; select the role first when
    /// resolving a family from an explicit theme.
    pub fn font_role(mut self, role: FontRole) -> Self {
        self.font_role = role;
        self
    }

    /// Set the shaping policy for this run.
    ///
    /// The compatibility default is [`TextShaping::Auto`]. Prose that must
    /// keep the same kerning and ligatures across plain, rich, and translated
    /// forms should request [`TextShaping::Advanced`]. Use
    /// [`TextShaping::Basic`] only when the font and repertoire are controlled
    /// and complex-script shaping and fallback are intentionally unwanted.
    pub fn shaping(mut self, shaping: impl Into<TextShaping>) -> Self {
        self.shaping = shaping.into();
        self
    }

    /// Return the shaping policy committed to iced at layout and paint time.
    pub fn shaping_policy(&self) -> TextShaping {
        self.shaping
    }

    /// Set line wrapping directly. The default is
    /// `iced::widget::text::Wrapping::Word`; pass
    /// `Wrapping::None` (or call [`Self::no_wrap`]) for a single-line label.
    pub fn wrapping(mut self, wrapping: iced::widget::text::Wrapping) -> Self {
        self.wrapping = wrapping;
        self
    }

    /// Opt into word-wrapping (multi-line).
    ///
    /// `wrap(true)` sets `Wrapping::Word` and **keeps `Ellipsis::End`**, so a
    /// *bounded* run ellipsizes its last visual line rather than
    /// hard-clipping mid-glyph; an *unbounded* run simply flows, so the
    /// default costs nothing without a cap.
    ///
    /// Setter order is deterministic. `wrap` writes only `wrapping` and
    /// `ellipsis`; [`Self::lines`] writes only `lines`; neither reads the
    /// other, so `.wrap(true).lines(3)` and `.lines(3).wrap(true)` agree.
    /// Pass an explicit [`Self::ellipsis`] *after* `wrap` to opt out.
    ///
    /// `Wrapping::Word` is already the default, so `.wrap(true)` is a
    /// no-op on a fresh run — kept for migrating callers and for
    /// `.wrap(cond)` with a runtime `cond`. The interesting direction is
    /// [`Self::no_wrap`].
    pub fn wrap(mut self, wrap: bool) -> Self {
        if wrap {
            self.wrapping = iced::widget::text::Wrapping::Word;
            // Keep end-elision so a *bounded* wrapped run signals overflow on
            // its last visible line (cosmic-text ellipsizes the last visual
            // line against max height). Was `Ellipsis::None` — that hard-clipped
            // bounded multiline with no `…`, the bounded-overflow failure.
            self.ellipsis = iced::widget::text::Ellipsis::End;
        } else {
            self.wrapping = iced::widget::text::Wrapping::None;
            self.ellipsis = iced::widget::text::Ellipsis::End;
        }
        self
    }

    /// Opt out of wrapping: single line, end-elided.
    ///
    /// The label case — a table cell, a chip, a status pill — where a stable
    /// row height is worth more than the tail of the string. Exactly
    /// `.wrap(false)`, named for the thing it is for.
    ///
    /// The two failure modes are not symmetric: a label that wrongly wraps
    /// gets taller — visible immediately, to the author, on the first run. A
    /// sentence that wrongly elides has information deleted, only at
    /// narrow widths the author never resizes to. A default should fail in
    /// the direction visible to the person who can fix it.
    ///
    /// This is not [`Self::single_line_elided`]: the two produce the same
    /// `wrapping`/`ellipsis` pair, but `single_line_elided` additionally
    /// auto-enables the elision audit probe. Use that when a truncated
    /// label would be a bug worth surfacing; use `no_wrap` when single-line
    /// is simply the intended geometry.
    pub fn no_wrap(self) -> Self {
        self.wrap(false)
    }

    /// Set whether this text is announced to assistive tech (default true).
    /// `announce(false)` `announce:false` to prevent duplicate
    /// parent/child announcement (consumed by the platform adapter).
    pub fn announce(mut self, announce: bool) -> Self {
        self.announce = announce;
        self
    }

    /// Whether this text is announced to assistive tech.
    pub fn is_announced(&self) -> bool {
        self.announce
    }

    /// Claim that this run is a heading at the given outline rung.
    ///
    /// The run publishes a `Role::Heading` node carrying the text as its
    /// accessible name and `level.level()` as its `aria-level`.
    /// Nothing visual changes — same size, weight, ink, and layout box,
    /// byte for byte.
    ///
    /// The claim rides the primitive the text already goes through because a
    /// separate wrapper would have to re-expose `enz_text`'s whole surface to
    /// be adoptable: one line at the call site, no restyle.
    ///
    /// What stops every label claiming to be one, in the order they bite:
    ///
    /// 1. The rung is named, not numbered.
    ///    [`HeadingLevel::Section`] states where the text sits in the page's
    ///    outline; a bare number invites copying off the size ramp — a large
    ///    *value* is not a heading however big it renders.
    /// 2. The frame is audited.
    ///    [`enzui_core::heading_outline::HeadingOutline`] reconstructs
    ///    the outline from a collected frame and fails on a second Page rung,
    ///    a level skip, an unnamed or unleveled heading, and on density.
    /// 3. The apps wire that audit per screen, so over-claiming reddens a
    ///    test rather than shipping.
    ///
    /// The claim implies announcement: `announce(false)` suppresses the
    /// heading node too (a silent heading stop would be worse than none).
    ///
    /// [`HeadingLevel::Section`]: enzui_core::semantics::HeadingLevel::Section
    pub fn heading(mut self, level: HeadingLevel) -> Self {
        self.heading = Some(level);
        self
    }

    /// The outline rung this run claims, or `None` for a plain label.
    pub fn heading_level(&self) -> Option<HeadingLevel> {
        self.heading
    }

    /// The accessibility node this run publishes when it claims a heading:
    /// [`Role::Heading`] named by the text, at the claimed `aria-level`.
    ///
    /// `None` when no rung was claimed, or when [`Self::announce`] is off.
    /// Split out so a test can assert the published node without a renderer.
    ///
    /// [`Role::Heading`]: enzui_core::semantics::Role::Heading
    pub fn heading_semantics(&self) -> Option<Semantics> {
        let level = self.heading?;
        if !self.announce {
            return None;
        }
        // use `heading_label()` instead of `heading()`. This
        // keeps `role = Label` in the collected list so downstream
        // `role_count(nodes, Role::Label, text)` calls still find the
        // node, while setting `heading_level` so the AT tree emits
        // `Role::Heading` and `HeadingOutline` counts the entry.
        Some(
            Semantics::new(Role::Label)
                .label(self.content.clone())
                .heading_label(level),
        )
    }

    /// Convenience: single-line, no wrap, end-elided — sets `Wrapping::None`
    /// and `Ellipsis::End`.
    ///
    /// Does not set width. A shrink-wrapped run always fits its own single
    /// line, so elision only engages once a *bound* is imposed — pair this
    /// with [`Self::full_width`] or [`Self::width`]/[`Self::text_max_width`].
    pub fn single_line_elided(mut self) -> Self {
        self.wrapping = iced::widget::text::Wrapping::None;
        self.ellipsis = iced::widget::text::Ellipsis::End;
        // Track that the *caller* opted in, distinct from the identical pair
        // written by the plain `no_wrap()` geometry opt-out. Consulted at
        // `into_element` to decide whether to insert an elision-scan probe.
        self.single_line_elided = true;
        // Auto-enable the elision audit for every single-line-elided run, so
        // no widget has to thread `.audit_elision(true)`. Since the probe wraps only
        // the text element (not the parent widget), `space_reserved = 0.0`
        // is correct in every case — the parent layout already assigns the
        // text its share of width after icons/padding.
        self.audit_elision = true;
        self
    }

    /// Whether the caller explicitly opted into single-line elision via
    /// [`Self::single_line_elided`]. Read by tests and by `into_element` to
    /// decide whether an elision-scan probe applies to this run.
    pub fn is_single_line_elided(&self) -> bool {
        self.single_line_elided
    }

    /// Elision audit: opt this run into the a11y elision audit.
    ///
    /// When `on` is `true` and [`Self::single_line_elided`] has been
    /// called, [`Self::into_element`] wraps the run in a probe publishing an
    /// `AccessibilityCheck::Elision` node. Without both conditions this is a
    /// no-op. [`Self::single_line_elided`] auto-enables this; call
    /// `.audit_elision(false)` after it to suppress the probe for
    /// deliberately-capped labels.
    pub fn audit_elision(mut self, on: bool) -> Self {
        self.audit_elision = on;
        self
    }

    /// Whether this run has [`Self::audit_elision`] enabled.
    pub fn is_audit_elision_enabled(&self) -> bool {
        self.audit_elision
    }

    /// Attach a stable id to the elision-audit finding this run produces.
    /// Only read when [`Self::audit_elision`] is `true`.
    pub fn audit_elision_id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.audit_elision_id = Some(id);
        self
    }

    /// Draw the bytes `range` of the content one weight rung heavier — the
    /// matched word in a search result, as Okular and Papers show it.
    ///
    /// A range that is empty, out of bounds or not on character boundaries
    /// is ignored and the run draws plain. Size, wrapping, line clamp,
    /// ellipsis and colour policy are unchanged.
    pub fn emphasize(mut self, range: std::ops::Range<usize>) -> Self {
        self.emphasis = Some(range);
        self
    }

    /// The emphasised range, when it is usable for this content.
    fn usable_emphasis(&self) -> Option<std::ops::Range<usize>> {
        self.emphasis.clone().filter(|range| {
            range.start < range.end
                && range.end <= self.content.len()
                && self.content.is_char_boundary(range.start)
                && self.content.is_char_boundary(range.end)
        })
    }

    /// Clamp the rendered line count.
    ///
    /// - `0` — unlimited (the default): height follows content.
    /// - `1` — single line.
    /// - `n` — at most `n` lines.
    ///
    /// iced's `Text` has no explicit max-line-count knob, so `lines(n)` is a
    /// height cap of `n × effective_line_height` via
    /// [`crate::max_height::MaxHeight`].
    ///
    /// With [`Self::wrap`]`(true)` (which keeps `Ellipsis::End`), an
    /// overflowing bounded wrapped run ends its last visible line with `…`
    /// rather than hard-clipping. For `n >= 2` use [`Self::wrap`]`(true)`; for
    /// `n == 1` the single-line default already elides. The cap is applied at
    /// [`Self::into_element`].
    pub fn lines(mut self, lines: u16) -> Self {
        self.lines = lines;
        self
    }

    /// The line clamp this run will render with (`0` = unlimited).
    pub fn lines_clamp(&self) -> u16 {
        self.lines
    }

    /// Cap the laid-out text width at `cap` px.
    ///
    /// What happens past the cap depends on the wrapping mode: with the
    /// default `Wrapping::Word` the text *wraps* at the cap; with
    /// [`Self::no_wrap`] it stays one line and *elides* at the cap. iced's
    /// `Text` has no `max_width`, so this wraps the run in
    /// [`crate::max_width::MaxWidth`] (`min(available, cap)`). Non-finite or
    /// `<= 0` caps are ignored. Applied at [`Self::into_element`].
    pub fn text_max_width(mut self, cap: f32) -> Self {
        if cap.is_finite() && cap > 0.0 {
            self.text_max_width = Some(cap);
        } else {
            self.text_max_width = None;
        }
        self
    }

    /// The text max-width cap this run will render with (`None` = uncapped).
    pub fn text_max_width_cap(&self) -> Option<f32> {
        self.text_max_width
    }

    /// Set the ellipsis strategy.
    pub fn ellipsis(mut self, ellipsis: iced::widget::text::Ellipsis) -> Self {
        self.ellipsis = ellipsis;
        self
    }

    /// Set an explicit line height (else iced's 1.3× default).
    pub fn line_height(mut self, lh: impl Into<iced::widget::text::LineHeight>) -> Self {
        self.line_height = Some(lh.into());
        self
    }

    /// Set the outer width (e.g. `Length::Fill` for a full-width label).
    pub fn width(mut self, width: impl Into<iced::Length>) -> Self {
        self.width = Some(width.into());
        self
    }

    /// Convenience: fill the available width (leading alignment by default).
    pub fn full_width(mut self) -> Self {
        self.width = Some(iced::Length::Fill);
        self
    }

    /// The outer width policy this run will render with (`None` = default
    /// `Shrink`). Read by callers that need to prove they installed a width
    /// bound for end-elision to engage against (e.g. `tab_pill`'s Fill cells).
    pub fn width_policy(&self) -> Option<iced::Length> {
        self.width
    }

    /// Override the semantic tone (default `Default`).
    /// Pin an explicit foreground colour, overriding the tone / runtime theme.
    /// Use for text on a coloured fill (e.g. white body text on an `accent`
    /// user chat bubble). Prefer [`EnzText::tone`] whenever a palette tone fits.
    pub fn color(mut self, color: Color) -> Self {
        self.color_override = Some(color);
        self
    }

    /// Paint this run with a colour resolved from the runtime `iced::Theme`
    /// on every render via `resolver`.
    ///
    /// For a widget-owned colour token `enz_text.rs` does not know about that
    /// must still re-tint on a theme switch in lock-step with that widget's
    /// chrome. The text-side twin of
    /// [`crate::icon::EnzIcon::tint_resolved`]. Overrides [`Self::color`],
    /// [`Self::tone`] and [`Self::tokens`]: the installing widget owns this
    /// run's ink outright.
    pub fn color_resolved(mut self, resolver: impl Fn(&iced::Theme) -> Color + 'static) -> Self {
        self.color_resolver = Some(std::rc::Rc::new(resolver));
        self
    }

    /// Set the semantic colour tone. Defaults to [`TextTone::Default`]. A
    /// [`Self::color`] override, when set, wins over the tone.
    pub fn tone(mut self, tone: TextTone) -> Self {
        self.tone = tone;
        self
    }

    /// Paint subtle / tertiary copy against the surface it actually lands on.
    ///
    /// The backdrop-aware counterpart of `.tone(TextTone::Subtle)`: promotes
    /// to `text_muted` or `text` only when the raw subtle token would miss
    /// the 4.5:1 body-text floor. Existing `.tone(Subtle)` callers keep the
    /// raw token byte-for-byte; resolution is explicit because iced does not
    /// propagate a parent's fill to children. An explicit [`Self::color`],
    /// [`Self::color_resolved`] or [`Self::tokens`] still wins.
    pub fn subtle_on(self, backdrop: Color) -> Self {
        self.subtle_on_with_floor(backdrop, ContrastFloor::Text)
    }

    /// Paint subtle copy against `backdrop` under an explicit WCAG clause.
    ///
    /// Qualifying large text may use [`ContrastFloor::LargeText`] so a heading
    /// stops promotion at SC 1.4.3's 3:1 floor instead of hard-coding a colour
    /// or unnecessarily becoming primary ink. The exception applies only at
    /// WCAG's large-text size/weight threshold; a heading role alone does not
    /// qualify. Normal-size text should keep [`Self::subtle_on`].
    ///
    /// [`ContrastFloor::NonText`] exists for graphics and is normally consumed
    /// by [`crate::icon::EnzIcon::tint_theme_subtle_on_with_floor`], not by a
    /// text run.
    pub fn subtle_on_with_floor(mut self, backdrop: Color, floor: ContrastFloor) -> Self {
        self.tone = TextTone::Subtle;
        self.tone_backdrop = Some((backdrop, floor));
        self
    }

    /// Paint muted / secondary copy against the surface it actually lands on.
    ///
    /// The backdrop-aware counterpart of `.tone(TextTone::Muted)`, retaining
    /// muted ink when it clears 4.5:1 and promoting to body text only when
    /// required. Existing `.tone(Muted)` callers remain byte-identical. An
    /// explicit [`Self::color`], [`Self::color_resolved`] or [`Self::tokens`]
    /// still wins.
    pub fn muted_on(mut self, backdrop: Color) -> Self {
        self.tone = TextTone::Muted;
        self.tone_backdrop = Some((backdrop, ContrastFloor::Text));
        self
    }

    /// Paint a semantic status tone against the surface it actually lands on.
    ///
    /// `tone` must be one of [`TextTone::Success`], [`TextTone::Warning`] or
    /// [`TextTone::Danger`] — any other value is a caller error and leaves
    /// the tone unchanged. The audited `tone_*_fg` role is retained
    /// byte-identical when it already clears the 4.5:1 floor, otherwise raised
    /// on its hue; an unreachable hue degrades to the best available colour.
    /// Existing `.tone(..)` callers keep the raw role byte-for-byte. An
    /// explicit [`Self::color`], [`Self::color_resolved`] or [`Self::tokens`]
    /// still wins.
    pub fn tone_on(mut self, tone: TextTone, backdrop: Color) -> Self {
        // Only the three audited status rungs have a palette-level tone_on
        // resolver; any other rung silently keeps its raw tone (no-op).
        if matches!(
            tone,
            TextTone::Success | TextTone::Warning | TextTone::Danger
        ) {
            self.tone = tone;
            self.tone_backdrop = Some((backdrop, ContrastFloor::Text));
        }
        self
    }

    /// Paint disabled control text against its actual opaque `backdrop`.
    ///
    /// Resolves through [`Palette::disabled_on_with_preferred`]: byte-identical
    /// wherever the raw rung already satisfies the readable-but-inactive band,
    /// promoted only where it falls outside it. The result is the complete
    /// disabled treatment — apply no further opacity multiplier.
    pub fn disabled_on(mut self, backdrop: Color) -> Self {
        self.tone = TextTone::Disabled;
        self.tone_backdrop = Some((backdrop, ContrastFloor::NonText));
        self
    }

    /// Inherit the enclosing renderer style's `text_color` rather than pinning
    /// one.
    ///
    /// A default `enz_text` always resolves and installs a colour, so a run
    /// nested inside a `button` ignores the button's status-dependent ink.
    /// `inherit_color(true)` installs no style, letting
    /// `renderer::Style.text_color` reach the glyphs untouched. A pinned
    /// [`Self::color`] / [`Self::color_resolved`] / [`Self::tokens`] still
    /// wins. The only way an ancestor's declared ink reaches this run —
    /// see [`crate::inherited_ink`] for why a container style paints nothing.
    pub fn inherit_color(mut self, inherit: bool) -> Self {
        self.inherit_color = inherit;
        self
    }

    /// Whether this run inherits its `text_color` from the enclosing renderer
    /// style ([`Self::inherit_color`]).
    pub fn is_color_inherited(&self) -> bool {
        self.inherit_color
    }

    /// Override the font weight (default is the size-implied weight).
    ///
    /// This is level 1 and wins outright: a run pinned to a face keeps it
    /// under every theme, including one whose
    /// [`TypographyScale`](enzui_theme::TypographyScale) moves the rung
    /// this run's [`TextSize`] would otherwise resolve.
    pub fn weight(mut self, weight: Weight) -> Self {
        self.weight = Some(weight);
        self
    }

    /// Enable italic (oblique). Default false.
    pub fn italic(mut self, italic: bool) -> Self {
        self.italic = italic;
        self
    }

    /// Draw digits with tabular figures (OpenType `tnum`), all one width, so
    /// a number that changes in place keeps its width and numbers in a
    /// column line up. Default off.
    pub fn tabular_figures(mut self, tabular: bool) -> Self {
        self.numeric = if tabular {
            iced::font::Numeric::Tabular
        } else {
            iced::font::Numeric::Proportional
        };
        self
    }

    /// Horizontal alignment (default `Left`).
    pub fn align(mut self, align: TextAlign) -> Self {
        self.align = align;
        self
    }

    /// Select a semantic weight independently of the size rung.
    ///
    /// A forwarded typography scale takes precedence over application defaults;
    /// an explicit [`Self::weight`] pin takes precedence over both.
    pub fn weight_rung(mut self, rung: enzui_theme::WeightRung) -> Self {
        self.weight_rung = Some(rung);
        self
    }

    /// Override the colour tokens.
    pub fn tokens(mut self, tokens: TextTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Resolve the effective font weight, in precedence order:
    ///
    /// 1. an explicit [`EnzText::weight`] pin;
    /// 2. the active [`EnzText::typography`] scale's number for the selected
    ///    semantic rung, mapped to a discrete face;
    /// 3. the application's published semantic weight rung.
    ///
    /// [`Self::weight_rung`] selects a different rung without pinning its face.
    /// The default application rungs reproduce [`TextSize::default_weight`].
    pub fn effective_weight(&self) -> Weight {
        self.weight.unwrap_or_else(|| {
            let rung = self.weight_rung.unwrap_or_else(|| self.size.weight_rung());
            self.typography.as_ref().map_or_else(
                || enzui_theme::text_defaults::weight(rung),
                |scale| scale.iced_weight(rung),
            )
        })
    }

    /// Resolve the effective pixel size, in precedence order:
    ///
    /// 1. an explicit [`EnzText::size_px`] pin;
    /// 2. the active [`EnzText::typography`] scale rung, when set and the
    ///    variant has a rung;
    /// 3. the local [`TextSize`] ramp.
    ///
    /// Levels 2 and 3 delegate to the shared [`TextSize::resolve_size`],
    /// which measurement also calls; only the level-1 pin is layered here.
    /// This is the value handed to the iced `text`.
    pub fn effective_size_px(&self) -> f32 {
        if let Some(px) = self.size_px {
            return px;
        }
        self.size.resolve_size(self.typography.as_ref())
    }

    /// Resolve the run's logical line-box height, clamped to the supported range.
    /// An explicit absolute height wins; a relative height multiplies effective
    /// size. Invalid explicit values fall back to the size-implied box. Without
    /// an override, a size pin uses [`line_box_px`], otherwise the shared
    /// [`TextSize::resolve_line_height`] selects the scale or ambient ramp.
    pub fn effective_line_height_px(&self) -> f32 {
        use iced::widget::text::LineHeight;

        let ramp_lh = if let Some(p) = self.size_px {
            line_box_px(p)
        } else {
            self.size.resolve_line_height(self.typography.as_ref())
        };

        let resolved = match self.line_height {
            Some(LineHeight::Absolute(px)) => {
                enzui_core::metrics::FinitePositive::new_or(px.0, ramp_lh).get()
            }
            Some(LineHeight::Relative(factor)) => {
                let raw = self.effective_size_px() * factor;
                enzui_core::metrics::FinitePositive::new_or(raw, ramp_lh).get()
            }
            None => ramp_lh,
        };

        // Finite positive overrides can still fall outside the supported line-box range.
        clamp_line_box(resolved)
    }

    /// The line-wrapping mode this run will render with (default
    /// `iced::widget::text::Wrapping::Word`, matching iced).
    pub fn wrapping_mode(&self) -> iced::widget::text::Wrapping {
        self.wrapping
    }

    /// The ellipsis strategy this run will render with (default
    /// `iced::widget::text::Ellipsis::End`).
    pub fn ellipsis_mode(&self) -> iced::widget::text::Ellipsis {
        self.ellipsis
    }

    /// The ink this run resolves to under `theme`, applying the same
    /// precedence [`Self::into_element`] installs at the render edge:
    /// `color_resolved` resolver, else pinned `color`, else `None` when
    /// `inherit_color` (the enclosing style paints it), else the [`Self::tone`]
    /// ink from the theme palette (or pinned [`TextTokens`]).
    ///
    /// The testable twin of the render-time resolver: a test reading this
    /// observes what the widget actually paints, not an inert stored field.
    /// `None` means "inherits".
    pub fn resolved_ink(&self, theme: &iced::Theme) -> Option<Color> {
        if let Some(resolver) = &self.color_resolver {
            Some(resolver(theme))
        } else if let Some(c) = self.color_override {
            Some(c)
        } else if self.inherit_color {
            None
        } else {
            match (self.tokens, self.tone, self.tone_backdrop) {
                (Some(tokens), TextTone::Disabled, Some((backdrop, _))) => {
                    let p = crate::theme_lookup::palette(theme);
                    Some(p.disabled_on_with_preferred(tokens.disabled_color, backdrop))
                }
                (Some(tokens), _, _) => Some(tokens.color_for(self.tone)),
                (None, TextTone::Subtle, Some((backdrop, floor))) => {
                    Some(crate::theme_lookup::palette(theme).subtle_on(backdrop, floor))
                }
                (None, TextTone::Muted, Some((backdrop, _))) => {
                    Some(crate::theme_lookup::palette(theme).text_muted_on(backdrop))
                }
                (None, TextTone::Success, Some((backdrop, _))) => Some(
                    crate::theme_lookup::palette(theme)
                        .tone_on(SemanticTone::Success, backdrop)
                        .color(),
                ),
                (None, TextTone::Warning, Some((backdrop, _))) => Some(
                    crate::theme_lookup::palette(theme)
                        .tone_on(SemanticTone::Warning, backdrop)
                        .color(),
                ),
                (None, TextTone::Danger, Some((backdrop, _))) => Some(
                    crate::theme_lookup::palette(theme)
                        .tone_on(SemanticTone::Error, backdrop)
                        .color(),
                ),
                (None, TextTone::Disabled, Some((backdrop, _))) => {
                    let p = crate::theme_lookup::palette(theme);
                    Some(p.disabled_on_with_preferred(p.disabled, backdrop))
                }
                (None, _, _) => Some(
                    TextTokens::from_palette(&crate::theme_lookup::palette(theme))
                        .color_for(self.tone),
                ),
            }
        }
    }

    /// The exact `iced::Font` this run commits on its `text` widget.
    ///
    /// **`into_element` builds its font through this method**, so observing
    /// this observes the exact descriptor handed to the renderer — not an
    /// inert stored field. `family` is the resolved role family (interned
    /// here, once, at the render edge); `weight` is
    /// [`Self::effective_weight`]; `style` is `Italic` iff [`Self::italic`];
    /// `stretch` is always `Normal` (`EnzText` has no stretch knob and the
    /// role selects only the family); `numeric` is tabular iff
    /// [`Self::tabular_figures`].
    ///
    /// `pub` so a composing widget or route-level test can observe the exact
    /// descriptor this run will paint.
    pub fn committed_font(&self) -> Font {
        let weight = self.effective_weight();
        let style = if self.italic {
            Style::Italic
        } else {
            Style::Normal
        };
        Font {
            family: self
                .font_family
                .clone()
                .unwrap_or_else(|| enzui_theme::text_defaults::family(self.font_role))
                .to_iced(),
            weight,
            style,
            stretch: Stretch::Normal,
            numeric: self.numeric,
        }
    }

    /// Convert into an iced [`Element`].
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: the backdrop exists whenever its tone does"
    )]
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        // make sure `Family::SansSerif` names a family that is actually
        // installed before this run's glyphs shape. Without it, on a host
        // lacking cosmic-text's default the generic resolves to nothing and
        // fallback picks a face with no Latin letters, so labels painted zero
        // glyphs while laying out normally. The framework installs the
        // fallback itself; idempotent behind a `Once`.
        crate::sans_serif_fallback::ensure_sans_serif_fallback();

        // The same accessor the tests observe, so a resolution defect breaks
        // both the widget and the test.
        let font = self.committed_font();

        let horizontal = self.align.into_horizontal();

        // Effective per-line height in px, resolved *before* building the
        // widget so the `lines` clamp can multiply by it (kept in lock-step
        // with `effective_line_height_px`).
        let line_height_px = self.effective_line_height_px();

        let px = self.effective_size_px();
        // resolve the heading claim before `self.content` moves into
        // iced's `text` widget below — the node is named by that string.
        let heading = self.heading_semantics();
        // Snapshot the label text before it moves into iced's `text` widget —
        // the elision-scan probe below needs it to shape a natural-width
        // reference under infinite bounds. When the audit is off we still
        // do the clone (a `String::clone` on a short label) rather than
        // borrow-splitting the struct; correctness beats saving a memcpy on
        // the audit-off path.
        let content_for_probe = self.content.clone();
        let emphasis = self.usable_emphasis();
        let element: Element<'a, Message> = if let Some(range) = emphasis {
            let mut strong = font;
            strong.weight = heavier(font.weight);
            let content = self.content;
            let spans = vec![
                iced::widget::span(content.get(..range.start).unwrap_or_default().to_owned())
                    .font(font),
                iced::widget::span(content.get(range.clone()).unwrap_or_default().to_owned())
                    .font(strong),
                iced::widget::span(content.get(range.end..).unwrap_or_default().to_owned())
                    .font(font),
            ];
            let mut rich = iced::widget::rich_text::<(), Message, iced::Theme>(spans)
                .size(px)
                .font(font)
                .align_x(horizontal)
                .wrapping(self.wrapping)
                .ellipsis(self.ellipsis)
                .line_height(iced::widget::text::LineHeight::Absolute(
                    line_height_px.into(),
                ));
            if let Some(w) = self.width {
                rich = rich.width(w);
            }
            let tone = self.tone;
            let tone_backdrop = self.tone_backdrop;
            if let Some(resolver) = self.color_resolver {
                rich.style(move |theme: &iced::Theme| iced::widget::text::Style {
                    color: Some(resolver(theme)),
                })
                .into()
            } else if let Some(c) = self.color_override {
                rich.color(c).into()
            } else if self.inherit_color {
                rich.into()
            } else {
                match self.tokens {
                    Some(tokens)
                        if matches!(tone, TextTone::Disabled) && tone_backdrop.is_some() =>
                    {
                        let (backdrop, _) = tone_backdrop.expect("guarded above");
                        rich.style(move |theme: &iced::Theme| iced::widget::text::Style {
                            color: Some(
                                crate::theme_lookup::palette(theme)
                                    .disabled_on_with_preferred(tokens.disabled_color, backdrop),
                            ),
                        })
                        .into()
                    }
                    Some(tokens) => rich.color(tokens.color_for(tone)).into(),
                    None => rich
                        .style(move |theme: &iced::Theme| iced::widget::text::Style {
                            color: Some(themed_ink(theme, tone, tone_backdrop)),
                        })
                        .into(),
                }
            }
        } else {
            let mut base = text(self.content)
                .size(px)
                .font(font)
                .align_x(horizontal)
                .shaping(self.shaping.into())
                .wrapping(self.wrapping)
                .ellipsis(self.ellipsis);
            // Line height: always pass the sanitised absolute value.
            // `effective_line_height_px()` already resolved every arm, so this is
            // the single point where the validated metric enters iced's widget —
            // closing the second path a negative/NaN value could take.
            base = base.line_height(iced::widget::text::LineHeight::Absolute(
                line_height_px.into(),
            ));
            if let Some(w) = self.width {
                base = base.width(w);
            }

            // Colour follows the runtime theme: when tokens are not
            // pinned, resolve them per-render from the runtime `iced::Theme` so a
            // light/dark/custom-theme swap re-tints the text. A pinned token set
            // keeps its fixed colour.
            let tone = self.tone;
            let tone_backdrop = self.tone_backdrop;
            // A per-render resolver wins over everything: the widget that installed
            // it owns this run's ink, and it still re-reads the theme every frame.
            if let Some(resolver) = self.color_resolver {
                base.style(move |theme: &iced::Theme| iced::widget::text::Style {
                    color: Some(resolver(theme)),
                })
                .into()
            } else if let Some(c) = self.color_override {
                // An explicit colour override wins over tone and runtime theme.
                base.color(c).into()
            } else if self.inherit_color {
                // pin no colour — the run draws with the enclosing
                // renderer style's `text_color` (e.g. a button's status ink).
                base.into()
            } else {
                match self.tokens {
                    Some(tokens)
                        if matches!(tone, TextTone::Disabled) && tone_backdrop.is_some() =>
                    {
                        let (backdrop, _) = tone_backdrop.expect("guarded above");
                        base.style(move |theme: &iced::Theme| iced::widget::text::Style {
                            color: Some(
                                crate::theme_lookup::palette(theme)
                                    .disabled_on_with_preferred(tokens.disabled_color, backdrop),
                            ),
                        })
                        .into()
                    }
                    Some(tokens) => base.color(tokens.color_for(tone)).into(),
                    None => base
                        .style(move |theme: &iced::Theme| iced::widget::text::Style {
                            color: Some(themed_ink(theme, tone, tone_backdrop)),
                        })
                        .into(),
                }
            }
        };

        // Layout-level wrappers, applied inside-out. iced's `Text` exposes
        // neither a line-count clamp nor a max-width, so each is realised as a
        // limits-shrinking single-child wrapper:
        //
        //  * `text_max_width` -> `MaxWidth` (cap layout width, wrap past it)
        //  * `lines(n>=1)`    -> `MaxHeight` cap of `n * line_height` px
        //  * `announce(false)`-> `SuppressAnnounce` (drop the child's
        //                        `Operation::text`, so the collector never
        //                        publishes a duplicate `Role::Label`)
        let element = match self.text_max_width {
            Some(cap) => crate::max_width::max_width(element, cap).into(),
            None => element,
        };
        let element = if self.lines >= 1 {
            let cap = f32::from(self.lines) * line_height_px;
            crate::max_height::MaxHeight::new(element, cap).into()
        } else {
            element
        };
        let element = if self.announce {
            element
        } else {
            SuppressAnnounce { child: element }.into()
        };

        // Elision scan probe. Fires only when the caller opted into
        // single-line elision (overflow = defect, not a wrapped paragraph)
        // and `audit_elision` is on. The probe wraps the label element
        // itself, so `space_reserved` is `0.0` — parent-assigned width flows
        // down as the label's. It shapes with the same resolved font and size
        // the paint path uses, so the measured natural width agrees with
        // what iced paints.
        let element = if self.audit_elision && self.single_line_elided {
            let mut probe = crate::accessibility_scan_widgets::ElisionScanProbe::new(
                element,
                content_for_probe,
                px,
                font,
            )
            .shaping(self.shaping);
            if let Some(id) = self.audit_elision_id {
                probe = probe.id(id);
            }
            probe.into()
        } else {
            element
        };

        // A claimed rung publishes its `Role::Heading` node outermost,
        // via a descending wrapper, never `leaf`: a terminal `leaf` would
        // refuse to traverse and silently drop every `TextElision` finding for
        // headings, and the run's own `Operation::text` must keep flowing so
        // the string stays discoverable. The duplicate `Role::Label` is
        // dropped by the collector's exact-match rule, so AT sees one node.
        // Suppressing the label widget-side instead would blind
        // `Simulator::find` to the heading string — rejected.
        match heading {
            Some(semantics) => {
                crate::semantics_container::SemanticsContainer::new(element, semantics).into()
            }
            None => element,
        }
    }
}

impl<'a, Message: 'a> From<EnzText> for Element<'a, Message> {
    fn from(t: EnzText) -> Self {
        t.into_element()
    }
}

/// A run's ink under the runtime theme when no tokens are pinned: its tone,
/// resolved against the backdrop the caller named, if any.
fn themed_ink(
    theme: &iced::Theme,
    tone: TextTone,
    tone_backdrop: Option<(Color, ContrastFloor)>,
) -> Color {
    match (tone, tone_backdrop) {
        (TextTone::Subtle, Some((backdrop, floor))) => {
            crate::theme_lookup::palette(theme).subtle_on(backdrop, floor)
        }
        (TextTone::Muted, Some((backdrop, _))) => {
            crate::theme_lookup::palette(theme).text_muted_on(backdrop)
        }
        (TextTone::Success, Some((backdrop, _))) => crate::theme_lookup::palette(theme)
            .tone_on(SemanticTone::Success, backdrop)
            .color(),
        (TextTone::Warning, Some((backdrop, _))) => crate::theme_lookup::palette(theme)
            .tone_on(SemanticTone::Warning, backdrop)
            .color(),
        (TextTone::Danger, Some((backdrop, _))) => crate::theme_lookup::palette(theme)
            .tone_on(SemanticTone::Error, backdrop)
            .color(),
        (TextTone::Disabled, Some((backdrop, _))) => {
            let p = crate::theme_lookup::palette(theme);
            p.disabled_on_with_preferred(p.disabled, backdrop)
        }
        _ => TextTokens::from_palette(&crate::theme_lookup::palette(theme)).color_for(tone),
    }
}

/// One weight rung heavier than `weight`, for an emphasised span.
fn heavier(weight: Weight) -> Weight {
    match weight {
        Weight::Thin | Weight::ExtraLight | Weight::Light | Weight::Normal => Weight::Semibold,
        Weight::Medium | Weight::Semibold => Weight::Bold,
        Weight::Bold | Weight::ExtraBold | Weight::Black => Weight::Black,
    }
}

/// Construct a themed text run with default styling.
///
/// Uses `TextSize::Body`, `TextTone::Default`, `TextAlign::Start`, and
/// [`TextShaping::Auto`].
pub fn enz_text<S: Into<String>>(content: S) -> EnzText {
    EnzText {
        content: content.into(),
        size: TextSize::Body,
        size_px: None,
        typography: None,
        tone: TextTone::Default,
        color_override: None,
        tone_backdrop: None,
        inherit_color: false,
        color_resolver: None,
        weight: None,
        weight_rung: None,
        italic: false,
        numeric: iced::font::Numeric::Proportional,
        shaping: TextShaping::default(),
        align: TextAlign::Start,
        tokens: None,
        font_family: None,
        font_role: FontRole::Ui,
        // word-wrap by default, matching iced's own `#[default]`.
        // A sentence given a bound reflows instead of silently losing its
        // tail; a *label* opts out with `.no_wrap()`. See the `wrapping`
        // field docs.
        wrapping: iced::widget::text::Wrapping::Word,
        ellipsis: iced::widget::text::Ellipsis::End,
        line_height: None,
        width: None,
        lines: 0,
        text_max_width: None,
        announce: true,
        //  Elision audit: audit metadata defaults, all off.
        // A plain `enz_text` matches the baseline behavior
        // byte-for-byte: no probe wrapping, no extra finding.
        single_line_elided: false,
        audit_elision: false,
        audit_elision_id: None,
        // not a heading unless the caller says so. A structural claim
        // is never inferred from size, weight or content.
        heading: None,
        emphasis: None,
    }
}

// ---------------------------------------------------------------------------
// SuppressAnnounce: a transparent single-child wrapper whose `operate` does
// not descend into the child, so the child's stock `Operation::text` never
// reaches the collector. This realises `announce:false`: `announce(true)`
// adds nothing (the collector already turns stock `Text` into an anonymous
// `Role::Label`), while `false` must *suppress* that label — done by refusing
// to traverse (draw, layout, events, overlays delegate straight through).
//
// NOTE: this also hides the text from `Simulator::find`, which consumes the
// same operation. Acceptable here — `announce(false)` explicitly asks for AT
// invisibility — but not for visible text (see `key_badge.rs`, where the
// opposite trade was made).

use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, layout, mouse, overlay, renderer};
use iced::{Event, Length, Rectangle, Size, Vector};

struct SuppressAnnounce<'a, Message> {
    child: Element<'a, Message>,
}

impl<Message> iced::advanced::Widget<Message, iced::Theme, iced::Renderer>
    for SuppressAnnounce<'_, Message>
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }
    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }
    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }
    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }
    // Shares the child's tree node, so its frame interest is the child's.
    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.child, tree)
    }
    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &iced::Rectangle,
    ) {
        crate::frame_routing::forward_frame_transparent(
            &mut self.child,
            tree,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }
    fn operate(
        &mut self,
        _tree: &mut Tree,
        _layout: Layout,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
        _operation: &mut dyn Operation,
    ) {
        // Deliberately do not descend into the child. The child's stock
        // `Operation::text` would otherwise publish a `Role::Label`; refusing
        // to traverse suppresses it.
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message: 'a> From<SuppressAnnounce<'a, Message>> for Element<'a, Message> {
    fn from(w: SuppressAnnounce<'a, Message>) -> Self {
        Element::new(w)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
