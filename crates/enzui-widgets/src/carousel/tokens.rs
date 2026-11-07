//! Colour and geometry tokens, indicator styles, and layout constants for
//! [`EnzCarousel`](super::EnzCarousel).
//!
//! These carry real behaviour: [`CarouselTokens::from_palette`] derives every
//! token from a runtime [`Palette`], [`CarouselTokens::sanitized`] guards the
//! `radius` against non-finite input, [`CarouselTokens::threshold`]
//! validates the swipe distance, and [`Indicator::width`] maps a style to a
//! pixel extent used by the view code.

use enzui_core::metrics::FinitePositive;
use enzui_core::paging::DEFAULT_SWIPE_THRESHOLD;
use enzui_theme::Palette;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Diameter of a `Dots` indicator.
pub const DOT_SIZE: f32 = 8.0;
/// Structural cap on the number of *individually realised* indicator
/// [`focus_button`](crate::focus_button::FocusButton)s the strip will
/// materialise.
///
/// Above this the strip switches to a compact hybrid (prev / current / next
/// dots plus a "Page X of N" numeric readout) so a 10,000-page carousel does
/// not build 10,000 focusables, tree slots and per-item motion wrappers. The
/// parent [`carousel_semantics`](enzui_core::paging::carousel_semantics) container still announces the live "Page X
/// of N" from the cursor, so unrealised pages remain *describable* — the
/// perf cap is not an a11y regression.
///
/// Chosen structurally, not by taste: an 8-px dot with a 6-px gap uses
/// 14 px each — 12 dots is 168 px end-to-end, which is close to the
/// widest visually parseable strip on the smallest carousel this crate is
/// used from (320-px viewports). Above
/// that, a human cannot count the dots to know which one is selected, so a
/// numeric readout is what a sighted user would *want* anyway. This is not a
/// tuned threshold hiding a perf bug — a strip with 20 realised dots would
/// be unreadable, so the cap encodes what the visual medium supports.
pub const MAX_VISIBLE_DOTS: usize = 12;
/// Width of the *active* `Lines` indicator.
pub const LINE_ACTIVE_WIDTH: f32 = 24.0;
/// Width of an inactive `Lines` indicator.
pub const LINE_WIDTH: f32 = 16.0;
/// Height of a bar indicator.
pub const LINE_HEIGHT: f32 = 4.0;
/// Gap between indicators.
pub const INDICATOR_SPACING: f32 = 6.0;
/// Body padding.
pub const BODY_PADDING: f32 = 16.0;
/// Gap between the page body and the indicator row.
pub const BODY_SPACING: f32 = 12.0;
/// Travel (logical px) a contested touch drag must clear before its dominant
/// axis is judged.
///
/// Below this, a press over a scrollable page body has not moved decisively
/// enough to tell a horizontal page swipe from a vertical scroll, so neither
/// claims. Matched to the reorder/drag deadband family so the whole toolkit
/// arms gestures at one distance.
pub const SWIPE_ARBITRATION_DEADBAND: f32 = 8.0;
/// Prev/next chrome inset from the edge.
pub const CHROME_INSET: f32 = 8.0;
/// Prev/next button size.
pub const CHROME_SIZE: f32 = 36.0;
/// How far inside the page an [`ArrowPlacement::Inside`](super::ArrowPlacement::Inside)
/// arrow sits from its side.
pub const INSIDE_ARROW_INSET: f32 = 12.0;
/// The prev/next chevron's size.
pub const ARROW_GLYPH: f32 = 20.0;

/// Extra horizontal inset the page body needs on each side so its content
/// clears the prev/next chrome in the default
/// [`ArrowPlacement::Inside`](super::ArrowPlacement::Inside): see
/// [`chrome_clearance_for`].
#[must_use]
pub const fn chrome_clearance() -> f32 {
    chrome_clearance_for(super::ArrowPlacement::Inside)
}

/// Extra horizontal inset the page body needs on each side so its content
/// clears arrows at `placement`, with a [`CHROME_INSET`] gap past them.
///
/// The chrome is stacked over the body, so by default a full-bleed page
/// paints underneath the arrows, which is what an image gallery wants and
/// what a row of pressable cards does not. A caller that wants the second
/// behaviour asks for it with
/// [`EnzCarousel::chrome_inset`](super::EnzCarousel::chrome_inset); this is the
/// distance that opt-in applies. Outside arrows already sit in a gutter
/// beside the page and need none.
///
/// The widget's own layout calls it (`carousel/view.rs`), so there is no
/// second copy of the arithmetic to drift: resize the chrome and the opt-in
/// inset moves with it.
#[must_use]
pub const fn chrome_clearance_for(placement: super::ArrowPlacement) -> f32 {
    match placement {
        super::ArrowPlacement::Inside => INSIDE_ARROW_INSET + CHROME_SIZE + CHROME_INSET,
        super::ArrowPlacement::Edge => CHROME_SIZE / 2.0 + CHROME_INSET,
        super::ArrowPlacement::Outside => 0.0,
    }
}
/// Height available for page content inside a carousel of `stage_height`,
/// accounting for the padding, spacing, and indicator row.
///
/// This is the vertical counterpart of [`chrome_clearance`]: the widget owns
/// the arithmetic over its own layout constants so a caller never
/// hand-computes the space the widget's chrome occupies.
///
/// # Why a free function with an `indicator` parameter
///
/// The vertical chrome depends on the configured [`Indicator`] style:
/// [`Indicator::None`] omits the indicator row, returning more body space.
/// A bare `stage_height` argument (the shape
/// [`chrome_clearance`] uses) would therefore be wrong, because callers with
/// different indicator styles would get the same answer.
///
/// It is a free function rather than a method on [`EnzCarousel`](super::EnzCarousel)
/// because callers need the slot height *before* building page closures —
/// a method on the builder would require constructing the carousel with pages
/// first, creating a chicken-and-egg with [`CarouselPage`](super::CarouselPage)
/// builders that capture the result.
///
/// Like [`chrome_clearance`], it is defined over the widget's own constants
/// ([`BODY_PADDING`], [`BODY_SPACING`], [`Indicator::height`]) and the view
/// code uses the same values, so a change to any of them moves both the
/// rendered layout and the published answer.
#[must_use]
pub fn body_slot_height(stage_height: f32, indicator: Indicator) -> f32 {
    (stage_height - 2.0 * BODY_PADDING - BODY_SPACING - indicator.height()).max(0.0)
}

/// Indicator transition duration (ms). Recorded, not applied.
pub const INDICATOR_FADE_MS: u16 = 120;
/// Default namespace for per-indicator widget ids.
///
/// Each indicator publishes a stable id `"{id_prefix}-{page id}"` so an
/// assistive technology can route an activation to a specific page by id rather
/// than only to whatever is focused. Override with [`EnzCarousel::id_prefix`](super::EnzCarousel::id_prefix)
/// when two carousels share a screen, so their per-dot ids never collide.
pub const DEFAULT_ID_PREFIX: &str = "enz-carousel";

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// Colour and geometry tokens for [`EnzCarousel`](super::EnzCarousel).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarouselTokens {
    /// Optional stage fill, transparent by default. Custom tokens can supply a fill.
    pub background: iced::Color,
    /// Carousel outline (`border`).
    pub border: iced::Color,
    /// Inactive indicator fill (`border-strong`).
    pub indicator: iced::Color,
    /// Hovered indicator fill (`text-muted`).
    pub indicator_hover: iced::Color,
    /// Active indicator fill (`accent`).
    pub indicator_selected: iced::Color,
    /// Chrome glyph tint (`text`).
    pub chrome_foreground: iced::Color,
    /// Chrome background (`surface`).
    pub chrome_background: iced::Color,
    /// Stage and page corner radius (the theme's `media` radius by default); ignored by transparent stages.
    pub radius: f32,
    /// Distance a drag must travel before it pages.
    pub swipe_threshold: f32,
    /// Intended indicator transition duration (ms). Recorded, not applied.
    pub indicator_fade_ms: u16,
}

impl CarouselTokens {
    /// Derive tokens from `theme`: its palette, and its `media` shape for the
    /// corners.
    #[must_use]
    pub fn from_theme(theme: &iced::Theme) -> Self {
        Self {
            radius: crate::theme_lookup::shape_radius(theme, |s| s.media),
            ..crate::theme_lookup::resolve(theme, Self::from_palette)
        }
    }

    /// Derive tokens from a runtime palette.
    #[must_use]
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            background: iced::Color::TRANSPARENT,
            // A shade of the stage's own fill.
            border: crate::surface_edge::edge_color(
                p.surface,
                crate::surface_edge::DEFAULT_EDGE_DEPTH,
            ),
            // An inactive page indicator is a control, so it takes the 3:1 border.
            indicator: p.control_border_strong,
            indicator_hover: p.text_muted,
            indicator_selected: p.accent,
            chrome_foreground: p.text,
            chrome_background: p.surface,
            radius: enzui_theme::text_defaults::shape_radius(|s| s.media),
            swipe_threshold: DEFAULT_SWIPE_THRESHOLD,
            indicator_fade_ms: INDICATOR_FADE_MS,
        }
    }

    /// Dark preset.
    #[must_use]
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Light preset.
    #[must_use]
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }

    /// Validated swipe threshold, substituting the default for invalid input.
    #[must_use]
    pub fn threshold(&self) -> FinitePositive {
        FinitePositive::new_or(self.swipe_threshold, DEFAULT_SWIPE_THRESHOLD)
    }

    /// Normalize the raw `f32` geometry so no non-finite (NaN / ±inf) or
    /// negative value from an external caller can reach renderer coordinates.
    ///
    /// Only `radius` needs guarding here: it becomes the carousel surface
    /// `Border` radius (`t.radius.into()` at the draw site). `swipe_threshold`
    /// is already sanitized at its read site via [`threshold`](Self::threshold)
    /// (`FinitePositive::new_or`) and is a gesture distance, not a renderer
    /// coordinate, so it is left untouched here — adding a second guard would be
    /// redundant. `indicator_fade_ms` is a `u16` and cannot be non-finite.
    ///
    /// `radius` is [`FiniteNonNeg`](enzui_core::metrics::FiniteNonNeg): a
    /// px extent that may legitimately be zero (a square-cornered
    /// carousel), so zero is preserved, not floored. The fallback is the
    /// [`from_palette`](Self::from_palette) value (the theme's `media` shape). Colours pass
    /// through.
    #[must_use]
    pub fn sanitized(&self) -> Self {
        use enzui_core::metrics::FiniteNonNeg;
        Self {
            radius: FiniteNonNeg::new_or(
                self.radius,
                enzui_theme::text_defaults::shape_radius(|s| s.media),
            )
            .get(),
            ..*self
        }
    }
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// How the carousel's own stage — the background fill and outline — is
/// painted.
///
/// The default is [`Filled`](Self::Filled), the shape the widget has always
/// shipped: the stage paints a fill and (with
/// [`EnzCarousel::bordered`](super::EnzCarousel::bordered)) a one-pixel
/// outline, both resolved from the runtime [`CarouselTokens`] so a theme
/// switch flows through. Choose [`Transparent`](Self::Transparent) to let
/// whatever surface sits *behind*
/// the carousel — a card, a wash, an app root — reach every pixel of the
/// stage instead. This is the same shape
/// [`EnzStatusBarStrip::embedded`](crate::status_bar::EnzStatusBarStrip::embedded)
/// has on the status bar: the widget's chrome moves out of the way so
/// a container that already paints does not have to fight it, and the app
/// never has to pin a resolved palette colour to get there.
///
/// The distinction that matters (why this exists, not just a
/// [`CarouselTokens`] pin): the choice is resolved at draw time, inside
/// the same `iced::widget::container` style closure that resolves the
/// filled shape's colours, so a re-themed palette moves the host pixel
/// straight through the carousel unchanged. A [`CarouselTokens`] value
/// captured at build time cannot track a theme switch, which is the drift
/// this closes.
///
/// # Composition
///
/// On a [`Transparent`](Self::Transparent) stage there is no band to paint,
/// so [`EnzCarousel::bordered`](super::EnzCarousel::bordered) and
/// [`CarouselTokens::radius`] are both documented no-ops — the same rule
/// [`EnzStatusBarStrip::embedded`](crate::status_bar::EnzStatusBarStrip::embedded)
/// applies to [`EnzStatusBarStrip::radius`](crate::status_bar::EnzStatusBarStrip::radius)
/// on an embedded strip. Indicators, prev/next chrome and page
/// layout are untouched: only the outer stage fill and border move.
///
/// # Interaction with [`chrome_clearance`] / [`body_slot_height`]
///
/// Neither answer changes on a transparent stage: the widget's
/// [`BODY_PADDING`], [`BODY_SPACING`] and indicator height are all inside
/// the padded `iced::widget::column` the stage container wraps, not part
/// of the stage's fill closure. A caller computing a page slot gets the
/// same number on either stage style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CarouselStageStyle {
    /// Paint the stage fill (and, with
    /// [`EnzCarousel::bordered`](super::EnzCarousel::bordered), the one-pixel
    /// outline) resolved from the runtime [`CarouselTokens`] —
    /// including transparent default fill.
    #[default]
    Filled,
    /// Paint no fill and no border. The stage is empty pixels; whatever
    /// surface sits behind the carousel — a detail card, an accent wash,
    /// the app root — reaches every pixel of the stage unchanged. See the
    /// [type-level docs](CarouselStageStyle) for the composition rules.
    Transparent,
}

/// How page position is indicated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Indicator {
    /// Equal-size round dots.
    Dots,
    /// Slim bars, with the active one wider.
    #[default]
    Lines,
    /// No indicator row at all.
    ///
    /// A carousel driven entirely by external chrome uses this to show no
    /// indicator.
    None,
}

impl Indicator {
    /// Width of one indicator in this style.
    #[must_use]
    pub fn width(self, selected: bool) -> f32 {
        match self {
            Indicator::Dots => DOT_SIZE,
            Indicator::Lines if selected => LINE_ACTIVE_WIDTH,
            Indicator::Lines => LINE_WIDTH,
            Indicator::None => 0.0,
        }
    }

    /// Height of one indicator in this style (vertical counterpart of
    /// [`width`](Self::width)).
    ///
    /// Dots use [`DOT_SIZE`], bars use [`LINE_HEIGHT`], and `None` contributes zero.
    /// This is the single source of truth used by
    /// [`body_slot_height`](super::body_slot_height) and the view layout, so
    /// changing the indicator geometry moves both.
    #[must_use]
    pub fn height(self) -> f32 {
        match self {
            Indicator::Dots => DOT_SIZE,
            Indicator::Lines => LINE_HEIGHT,
            Indicator::None => 0.0,
        }
    }
}
