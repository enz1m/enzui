//! EnzGradientBackground: configurable linear-gradient surface.
//!
//! Decorative rectangle painted with a gradient between two or three colour
//! stops.
//!
//! # Linear only (radial + conic deferred)
//!
//! iced 0.15 exposes linear gradients only (`iced::gradient::Gradient` has
//! `Linear` only; radial/conic are upstream TBD). Accepting `Radial`/`Conic`
//! enum variants while silently rendering a solid `color_start` would not be a
//! truthful API. This widget is therefore linear-only: there
//! is no kind enum. When iced (or the enzICED fork) grows radial/conic support,
//! the API can add them as real variants.
//!
//! # Theme-driven default stops
//!
//! The defaults are `color-start = accent`, `color-mid = accent.darker(0.2)`,
//! `color-end = accent.darker(0.5)` — driven by the active `EnzTheme.accent`.
//! Unset stops here are resolved from the runtime accent *inside* the style
//! closure, so built-in/runtime/custom themes drive the default gradient. Any
//! stop the caller sets explicitly overrides its theme default.
//!
//! # Geometry notes
//!
//! * `angle` is degrees clockwise from the up vector; iced's
//!   `gradient::Linear::new(Radians)` uses the same convention. The default
//!   `180deg` renders top-to-bottom; the `angle` property overrides it.
//! * Stops at 0 / 50% / 100%; `show_mid: false` uses only start → end.
//! * `darker(f)` HSV algorithm: convert to HSV, divide value
//!   (brightness) by `1 + f`, convert back; alpha unchanged.

use enzui_theme::Palette;
use iced::gradient::Linear;
use iced::widget::container::{self, Style};
use iced::{Background, Border, Color, Element, Length, Radians, Theme};

use crate::theme_lookup::resolve;

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// Default gradient angle (`angle: 180.0` = top-to-bottom).
pub const DEFAULT_GRADIENT_ANGLE_DEGREES: f32 = 180.0;

/// the `color-mid` darkening factor: `accent.darker(0.2)`.
pub const MID_DARKEN_FACTOR: f32 = 0.2;
/// the `color-end` darkening factor: `accent.darker(0.5)`.
pub const END_DARKEN_FACTOR: f32 = 0.5;

/// Normalise a gradient angle in degrees into the canonical range `[0, 360)`.
///
/// * Non-finite (`NaN`, `±inf`) → `0.0`. Unchanged behaviour, pinned by test.
/// * Finite values are wrapped with `rem_euclid(360.0)` so that
///   `450.0` collapses to `90.0`, `-90.0` to `270.0`, and huge finite
///   values (`f32::MAX`) never reach `sin`/`cos` in radian form where
///   the precision is already gone. `0.0` stays `0.0`; `360.0` becomes
///   `0.0` (the same gradient) as zero-as-intent survives the wrap.
///
/// This is a normalisation, not a rejection: a large-but-finite value is
/// expressible intent ("spin the gradient N/360 turns clockwise"), just
/// redundantly expressed, and `rem_euclid` is the right way to fold it.
fn finite_angle(deg: f32) -> f32 {
    if deg.is_finite() {
        // `rem_euclid` returns a value in `[0, 360)` for finite inputs,
        // including negatives — unlike the `%` operator, which would leave
        // `-90.0` as `-90.0`.
        deg.rem_euclid(360.0)
    } else {
        0.0
    }
}

/// Normalise a non-negative dimension (non-finite/negative → 0).
fn finite_nonneg(v: f32) -> f32 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Convenience constructor: `enz_gradient_background(child)`.
///
/// All three stops default to the active theme accent (start = accent,
/// mid = `accent.darker(0.2)`, end = `accent.darker(0.5)`) resolved at render
/// time; override any stop with the builder.
pub fn enz_gradient_background<'a, Message>(
    child: impl Into<Element<'a, Message>>,
) -> EnzGradientBackground<'a, Message>
where
    Message: 'a,
{
    EnzGradientBackground {
        child: child.into(),
        angle_degrees: DEFAULT_GRADIENT_ANGLE_DEGREES,
        color_start: None,
        color_mid: None,
        color_end: None,
        show_mid: true,
        border_radius: 0.0,
        width: Length::Fill,
        height: Length::Fill,
    }
}

/// Builder returned by [`enz_gradient_background`]. Unset stops (`None`) are
/// resolved from the runtime accent in the style closure.
#[must_use]
pub struct EnzGradientBackground<'a, Message> {
    child: Element<'a, Message>,
    angle_degrees: f32,
    color_start: Option<Color>,
    color_mid: Option<Color>,
    color_end: Option<Color>,
    show_mid: bool,
    border_radius: f32,
    width: Length,
    height: Length,
}

impl<'a, Message> EnzGradientBackground<'a, Message>
where
    Message: 'a,
{
    /// Set the gradient angle in degrees clockwise from the up vector.
    /// Non-finite → 0.
    pub fn angle_degrees(mut self, deg: f32) -> Self {
        self.angle_degrees = finite_angle(deg);
        self
    }

    /// Set the first colour stop . Overrides the accent
    /// default.
    pub fn color_start(mut self, c: Color) -> Self {
        self.color_start = Some(c);
        self
    }

    /// Set the middle colour stop (`color-mid`, at 50 %). Only used when
    /// [`Self::show_mid`] is `true` (default). Overrides `accent.darker(0.2)`.
    pub fn color_mid(mut self, c: Color) -> Self {
        self.color_mid = Some(c);
        self
    }

    /// Set the final colour stop . Overrides
    /// `accent.darker(0.5)`.
    pub fn color_end(mut self, c: Color) -> Self {
        self.color_end = Some(c);
        self
    }

    /// Toggle the middle colour stop.
    pub fn show_mid(mut self, show: bool) -> Self {
        self.show_mid = show;
        self
    }

    /// Set the outer border radius. Non-finite/negative → 0.
    pub fn border_radius(mut self, r: f32) -> Self {
        self.border_radius = finite_nonneg(r);
        self
    }

    /// Outer width (default `Length::Fill`).
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Outer height (default `Length::Fill`).
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Emit the [`container::Style`] this builder would hand to iced for the
    /// given runtime `theme`.
    ///
    /// Test-only hook (hidden from rustdoc) so integration tests can inspect
    /// the emitted [`Linear::angle`] and stop array — the values iced
    /// actually renders with, *after* the builder's `angle_degrees` setter
    /// has applied [`finite_angle`]. Reading `self.angle_degrees` directly
    /// would be indistinguishable from a broken passthrough that emits the
    /// stored value into `to_radians()` verbatim.
    #[doc(hidden)]
    pub fn emit_style_for_test(&self, theme: &Theme) -> Style {
        gradient_style(
            theme,
            self.angle_degrees,
            self.color_start,
            self.color_mid,
            self.color_end,
            self.show_mid,
            self.border_radius,
        )
    }
}

impl<'a, Message> EnzGradientBackground<'a, Message>
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

impl<'a, Message> From<EnzGradientBackground<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(w: EnzGradientBackground<'a, Message>) -> Self {
        let angle_deg = w.angle_degrees;
        let start_override = w.color_start;
        let mid_override = w.color_mid;
        let end_override = w.color_end;
        let show_mid = w.show_mid;
        let radius = w.border_radius;

        container::Container::new(w.child)
            .width(w.width)
            .height(w.height)
            .style(move |theme: &Theme| {
                gradient_style(
                    theme,
                    angle_deg,
                    start_override,
                    mid_override,
                    end_override,
                    show_mid,
                    radius,
                )
            })
            .into()
    }
}

/// Emit the [`container::Style`] the widget hands iced, given the runtime
/// theme and the builder's resolved config.
///
/// This is the single place where the builder's `angle_degrees: f32` is
/// finalised into a [`Radians`] on the emitted [`Linear`] gradient, so a
/// behavioural test can inspect what iced actually renders rather than the
/// pre-normalisation angle stored on the builder. Both the `Container::style`
/// closure and integration tests (see
/// `tests/gradient_angle_normalisation.rs`) call this function — there is
/// no second code path.
#[doc(hidden)]
pub fn gradient_style(
    theme: &Theme,
    angle_deg: f32,
    start_override: Option<Color>,
    mid_override: Option<Color>,
    end_override: Option<Color>,
    show_mid: bool,
    radius: f32,
) -> Style {
    // Resolve unset stops from the runtime accent (so themes drive the
    // default gradient); explicit overrides win.
    let accent = resolve(theme, |p: &Palette| p.accent);
    let start = start_override.unwrap_or(accent);
    let mid = mid_override.unwrap_or_else(|| darker(accent, MID_DARKEN_FACTOR));
    let end = end_override.unwrap_or_else(|| darker(accent, END_DARKEN_FACTOR));

    let mut linear = Linear::new(Radians(angle_deg.to_radians()));
    linear = linear.add_stop(0.0, start);
    if show_mid {
        linear = linear.add_stop(0.5, mid);
    }
    linear = linear.add_stop(1.0, end);

    Style {
        background: Some(Background::Gradient(linear.into())),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: radius.into(),
        },
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Darken a colour the way the `Color.darker(factor)` does: convert to HSV,
/// divide the value (brightness) by `1 + factor`, convert back. Alpha is
/// unchanged. A non-finite or negative factor is treated as `0` (identity).
///
/// # Distinct from [`crate::color::darken`]
///
/// The crate has two colour-darkening authorities, and they use
/// different maths. They must not be swapped:
///
/// | function | maths | when to use |
/// |---|---|---|
/// | [`crate::color::darken`] | per-channel `c * (1 - amount)` (linear ramp) | shared hover/press darken step (~8 % of channel) used by toggle / slider / link and the shared tokens — the small-factor regime where the two agree to within ~1 % |
/// | `gradient_background::darker` (this) | HSV, `value /= 1 + factor`  | the `EnzGradientBackground` default mid/end stops — needs the HSV value curve so themed defaults look right |
///
/// At the `MID_DARKEN_FACTOR = 0.2` / `END_DARKEN_FACTOR = 0.5` used here,
/// the two diverge measurably; at `factor = 1.0` the HSV form halves brightness
/// while `darken` produces pure black. Aliasing one to the other silently
/// changes rendered output. See the `darken_and_darker_diverge_at_large_factor`
/// test in `crates/enzui-widgets/src/color.rs` (guards against a future
/// "cleanup" merging them).
pub(crate) fn darker(c: Color, factor: f32) -> Color {
    let factor = if factor.is_finite() {
        factor.max(0.0)
    } else {
        0.0
    };
    let (h, s, v) = rgb_to_hsv(c.r, c.g, c.b);
    let v = v / (1.0 + factor);
    let (r, g, b) = hsv_to_rgb(h, s, v);
    Color { r, g, b, a: c.a }
}

/// RGB (0..=1) → HSV (h in degrees 0..360, s/v 0..=1).
fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let h = if delta <= f32::EPSILON {
        0.0
    } else if (max - r).abs() < f32::EPSILON {
        60.0 * (((g - b) / delta) % 6.0)
    } else if (max - g).abs() < f32::EPSILON {
        60.0 * (((b - r) / delta) + 2.0)
    } else {
        60.0 * (((r - g) / delta) + 4.0)
    };
    let h = if h < 0.0 { h + 360.0 } else { h };
    let s = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };
    (h, s, max)
}

/// HSV (h in degrees, s/v 0..=1) → RGB (0..=1).
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let v = v.clamp(0.0, 1.0);
    let c = v * s;
    let h = h.rem_euclid(360.0);
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h {
        _ if h < 60.0 => (c, x, 0.0),
        _ if h < 120.0 => (x, c, 0.0),
        _ if h < 180.0 => (0.0, c, x),
        _ if h < 240.0 => (0.0, x, c),
        _ if h < 300.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        (r + m).clamp(0.0, 1.0),
        (g + m).clamp(0.0, 1.0),
        (b + m).clamp(0.0, 1.0),
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
