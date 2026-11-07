//! EnzClamp: cap child width to a readable-width max, centre in the parent.
//!
//! A container that caps its child width to a readable-width maximum
//! (narrow / default / wide / xwide = 480 / 720 / 960 / 1280 px) and
//! horizontally aligns the capped column inside a wider parent
//! (start / center / end).
//!
//! Use for reading panes, article bodies, chat transcripts — anywhere text
//! shouldn't fill a full-screen viewport width and become unreadable.
//!
//! ## Plain Container composition (no custom Widget)
//!
//! This is canonical `container` composition; no custom `Widget` and no
//! second layout pass are needed. iced's `container` passes finite limits
//! inward, and `Limits::width(Length::Fixed(cap))` clamps the requested width
//! to the parent's available maximum — so an inner `Length::Fixed(cap)` column
//! already resolves to `min(parent_width, cap)`. When the parent is wider
//! than the cap the column sits at `cap`; when the parent is narrower it
//! shrinks to the parent width rather than overflowing.
//!
//! The outer `Length::Fill` container horizontally aligns the capped
//! column via [`ClampAlign`] (start / center / end).
//!
//! ### Fill height
//!
//! By default the wrapper is natural-height (`Shrink`), suitable for a
//! scroll column. Set [`EnzClamp::fill`] for panes that own their
//! height (detail panes): both the outer and inner containers take
//! `Length::Fill` height.
//!
//! ### Cap tiers are shared theme tokens
//!
//! The four readable-width tiers (narrow / default / wide / xwide =
//! 480 / 720 / 960 / 1280) are not hard-coded here — they resolve through
//! [`enzui_theme::SizingTokens`] (`container_narrow` … `container_xwide`),
//! so a custom theme can change the width limits.
//!
//! ## Theme reactivity
//!
//! The tier resolves to a `Length::Fixed(cap)` on an inner container, consumed
//! in `Container::layout()` where iced supplies no `&Theme`. So the ladder
//! cannot be resolved *inside* layout; instead a live view forwards its
//! [`SizingTokens`] at build time via [`EnzClamp::sizing_tokens`], following
//! the same three-tier precedence [`crate::status_bar::EnzStatusBar`] uses:
//!
//! 1. an explicit [`EnzClamp::max_width`] pixel pin wins,
//! 2. otherwise a forwarded [`SizingTokens`] resolves the [`ClampSize`] tier,
//! 3. otherwise the stock [`SizingTokens::default`] tiers apply.
//!
//! Without step (2), a rescaled theme's `container-*` values silently do not
//! reach the laid-out cap.

use enzui_theme::SizingTokens;
use iced::widget::container;
use iced::{Element, Length, alignment::Horizontal};

// ---------------------------------------------------------------------------
// Presets
// ---------------------------------------------------------------------------

/// Readable-width presets. Each maps to a shared
/// [`SizingTokens`] container tier .
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClampSize {
    /// Narrow tier — dense text columns, mobile-first reading (`container_narrow`, 480).
    Narrow,
    /// Default tier — the everyday reading width (`container_default`, 720).
    #[default]
    Default,
    /// Wide tier — wider prose, blog posts, docs pages (`container_wide`, 960).
    Wide,
    /// Extra-wide tier — dashboards, chart grids (`container_xwide`, 1280).
    Xwide,
}

impl ClampSize {
    /// Resolve this tier against a theme's [`SizingTokens`]. Prefer this over
    /// [`ClampSize::width`] when the app carries a customised sizing scale;
    /// [`EnzClamp::sizing_tokens`] threads the whole scale through and applies
    /// this at build time.
    pub fn width_from(self, tokens: &SizingTokens) -> f32 {
        match self {
            ClampSize::Narrow => tokens.container_narrow,
            ClampSize::Default => tokens.container_default,
            ClampSize::Wide => tokens.container_wide,
            ClampSize::Xwide => tokens.container_xwide,
        }
    }

    /// Resolve this tier against the default [`SizingTokens`] (480/720/960/1280).
    ///
    /// The px becomes an inner container's `Length::Fixed(cap)`, read in
    /// `Container::layout` where iced supplies no `&Theme`. So the ladder
    /// cannot be resolved *inside* layout; a rescaled theme's `container-*`
    /// values reach the cap through [`EnzClamp::sizing_tokens`], which sets an
    /// internal scale used at construction. This method is the stock fallback
    /// when no scale is forwarded.
    pub fn width(self) -> f32 {
        self.width_from(&SizingTokens::default())
    }
}

/// Horizontal alignment of the clamped column when the parent is wider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClampAlign {
    /// Pin the clamped column to the parent's left edge.
    Start,
    /// Centre the clamped column in the parent (the default).
    #[default]
    Center,
    /// Pin the clamped column to the parent's right edge.
    End,
}

impl From<ClampAlign> for Horizontal {
    fn from(a: ClampAlign) -> Self {
        match a {
            ClampAlign::Start => Horizontal::Left,
            ClampAlign::Center => Horizontal::Center,
            ClampAlign::End => Horizontal::Right,
        }
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Wrap `child` in a `size`-capped readable column horizontally aligned
/// inside a full-width outer.
pub fn enz_clamp<'a, Message>(
    child: impl Into<Element<'a, Message>>,
    size: ClampSize,
) -> EnzClamp<'a, Message>
where
    Message: 'a,
{
    EnzClamp {
        child: child.into(),
        size,
        // Explicit pixel override; wins over both the forwarded scale and
        // `size.width()` when set. `None` at construction means "resolve from
        // the scale (or stock)".
        max_width_explicit: None,
        sizing_tokens: None,
        align: ClampAlign::default(),
        fill: false,
    }
}

/// Builder returned by [`enz_clamp`].
#[must_use]
pub struct EnzClamp<'a, Message> {
    child: Element<'a, Message>,
    /// The requested tier (retained so a later `.sizing_tokens()` can resolve
    /// it, and so `.max_width(px)` overrides it without erasing the intent).
    size: ClampSize,
    /// Explicit pixel override (highest precedence). Set by
    /// [`EnzClamp::max_width`]; wins over both the forwarded [`SizingTokens`]
    /// and the stock `ClampSize::width()`.
    max_width_explicit: Option<f32>,
    /// Active runtime sizing tokens forwarded by the view owner. `None`
    /// falls back to the stock [`SizingTokens::default`].
    sizing_tokens: Option<SizingTokens>,
    align: ClampAlign,
    fill: bool,
}

impl<'a, Message> EnzClamp<'a, Message>
where
    Message: 'a,
{
    /// Horizontal alignment of the clamped column (default `Center`).
    pub fn align(mut self, align: ClampAlign) -> Self {
        self.align = align;
        self
    }

    /// Override the readable-width cap (default from `ClampSize`, or from a
    /// forwarded [`Self::sizing_tokens`] when one is set).
    ///
    /// Non-finite or negative values are treated as "no meaningful cap" and
    /// clamped to `0.0` (an empty column) rather than being passed to layout
    /// as `Length::Fixed(NaN/∞)`.
    ///
    /// Precedence: this explicit pin wins over the forwarded scale, which
    /// wins over the stock [`ClampSize::width()`] fallback.
    pub fn max_width(mut self, max_width: f32) -> Self {
        let clamped = if max_width.is_finite() {
            max_width.max(0.0)
        } else {
            0.0
        };
        self.max_width_explicit = Some(clamped);
        self
    }

    /// Forward the active runtime [`SizingTokens`] so a rescaled theme's
    /// `container-*` values resolve the [`ClampSize`] tier, matching the
    /// pattern [`crate::status_bar::EnzStatusBar::spacing_scale`] uses.
    ///
    /// The tier resolves to a `Length::Fixed(cap)` at build time (the point
    /// this builder converts into an [`Element`]); iced runs `Container::layout`
    /// with no `&Theme`, so this build-time thread is the only route by which
    /// a rescaled theme's ladder can reach the laid-out cap.
    ///
    /// Precedence: an explicit [`Self::max_width`] pin wins over this
    /// forwarded scale, which wins over the stock [`SizingTokens::default`].
    pub fn sizing_tokens(mut self, tokens: SizingTokens) -> Self {
        self.sizing_tokens = Some(tokens);
        self
    }

    /// Resolve the effective pixel cap under the three-tier precedence.
    fn resolved_max_width(&self) -> f32 {
        if let Some(px) = self.max_width_explicit {
            return px;
        }
        match &self.sizing_tokens {
            Some(tokens) => self.size.width_from(tokens),
            None => self.size.width(),
        }
    }

    /// Take the parent's full height (detail panes that own their height)
    /// rather than hugging the natural content height. Default `false`.
    pub fn fill(mut self, fill: bool) -> Self {
        self.fill = fill;
        self
    }
}

impl<'a, Message> EnzClamp<'a, Message>
where
    Message: 'a,
{
    /// Finish the builder and hand back the [`Element`], which is step 3 of
    /// the build pattern every widget in this crate follows (`docs/widgets.md`).
    ///
    /// This forwards to the `From<Self> for Element` impl below, so the two
    /// spellings are one implementation and cannot drift apart. Prefer this
    /// one inside `row![]` / `column![]`, where `.into()` has no target type
    /// to infer.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzClamp<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(c: EnzClamp<'a, Message>) -> Self {
        // Resolve the tier against the forwarded scale when
        // present, before the value is baked into `Length::Fixed` and consumed
        // in `Container::layout` (pinned by
        // `forwarded_sizing_tokens_move_max_width`).
        let cap = c.resolved_max_width();
        // Height policy: fill the parent (detail pane) or hug content.
        let height = if c.fill { Length::Fill } else { Length::Shrink };
        // Inner: the readable column. `Fixed(cap)` resolves to
        // min(parent_width, cap) because Container passes finite limits inward
        // and Limits::width clamps to the parent max — no overflow, no custom
        // Widget. Height follows the fill policy.
        let inner = container(c.child).width(Length::Fixed(cap)).height(height);
        // Outer: full-width slot that aligns the inner column left / centre /
        // right. Height follows the fill policy (Fill for panes, Shrink so the
        // wrapper doesn't inflate its row when hugging content).
        container(inner)
            .width(Length::Fill)
            .height(height)
            .align_x(Horizontal::from(c.align))
            .into()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
