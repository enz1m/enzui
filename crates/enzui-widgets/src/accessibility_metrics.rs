//! [`hit_target_probe`] and [`contrast_probe`] — accessibility audit math.
//!
//! Pure functions that warn on small tap targets and low colour contrast.
//!
//! ## Standards scope (read before trusting the result)
//!
//! - Hit target: a conservative size warning, not a full WCAG
//!   determination. The 24 px baseline is WCAG 2.2 SC 2.5.8 Target
//!   Size (Minimum) — not 2.5.5 Target Size (Enhanced), which is
//!   44 px. 2.5.8 also permits spacing, inline,
//!   user-agent-controlled, essential, and equivalent exceptions that
//!   a single width/height function cannot evaluate — so a flagged
//!   target is a candidate to review, and a passing target only
//!   clears the raw-size branch. The `HIT_TARGET_EPSILON` below is
//!   sub-pixel measurement-noise tolerance, not standards grace: a
//!   target is treated as undersized the moment its deficit exceeds
//!   float noise, so 23.5 px is flagged.
//! - Collapsed targets are the worst case, not an absent measurement.
//!   A `0x0` (or negative, or `NaN`) rendered size is reported as a
//!   maximal deficit (`min_size` on that axis), never as clean. "This
//!   control has not been laid out yet / is not presented" is a
//!   genuinely different fact from "this control measured zero", and
//!   it is carried out of band by
//!   [`AccessibilityCheck::HitTarget::hidden`], not inferred from the
//!   measurement.
//!
//! [`AccessibilityCheck::HitTarget::hidden`]: enzui_core::accessibility_scan::AccessibilityCheck::HitTarget
//! - Contrast: WCAG 1.4.3 (AA), 4.5:1 small text / 3:1 large. The
//!   ratio is compared unrounded and strictly against the threshold
//!   (WCAG requires that computed values are not rounded before
//!   comparison).
//!
//! Same pattern as [`mod@crate::bounds_probe`]: the math is here, and native
//! discovery lives in
//! [`crate::accessibility_scan_widgets`] via the shared audit collector.
//!
//! ## Example
//!
//! ```
//! use enzui_widgets::accessibility_metrics::{hit_target_probe, contrast_probe, ContrastMode};
//! use iced::Color;
//!
//! let ht = hit_target_probe(20.0, 16.0, 24.0, false);
//! assert!(ht.undersized);
//! assert_eq!(ht.deficit_x, 4.0);
//!
//! let c = contrast_probe(
//!     Color::from_rgb(0.9, 0.9, 0.9),
//!     Color::from_rgb(1.0, 1.0, 1.0),
//!     ContrastMode::Small,
//!     false,
//! );
//! assert!(c.low_contrast);
//! ```

use iced::Color;

/// `EnzLayoutAudit.min-hit-target` — default 24 px (WCAG 2.2 SC 2.5.8
/// Target Size (Minimum); a conservative size warning, not a full 2.5.8
/// determination — see the module docs).
pub const DEFAULT_MIN_HIT_TARGET: f32 = 24.0;

pub(crate) fn hit_floor(length: iced::Length, floor: f32) -> iced::Length {
    use iced::Length;
    use iced::advanced::graphics::core::length::{Bounds, Constraint};
    if floor > 0.0 {
        match length {
            Length::Bounded {
                bounds: Bounds::Min(min) | Bounds::Both { min, .. },
                ..
            } => length.min(min.max(floor)),
            Length::Fluid(Constraint::Min(min)) => Length::Fluid(Constraint::Min(min.max(floor))),
            _ => length.min(floor),
        }
    } else {
        length
    }
}

/// `EnzLayoutAudit.min-contrast-ratio` — 4.5:1 (WCAG 1.4.3 AA small text).
pub const DEFAULT_MIN_CONTRAST_RATIO: f32 = 4.5;
/// `EnzLayoutAudit.min-contrast-ratio-large` — 3.0:1 (WCAG 1.4.3 AA large text).
pub const DEFAULT_MIN_CONTRAST_RATIO_LARGE: f32 = 3.0;
/// Sub-pixel measurement-noise tolerance (px). Not standards grace:
/// this only absorbs float/rounding noise in a measured dimension so
/// a target at exactly `min_size` isn't flagged by a `1e-6` deficit.
/// Any real deficit (e.g. 0.5 px → 23.5 px target) is flagged.
///
/// # Scale awareness
///
/// Unlike [`SPILL_EPSILON`](crate::bounds_probe::SPILL_EPSILON), this
/// epsilon is pure float-noise tolerance (1e-3 px). At any display
/// scale factor, 1e-3 logical pixels is still float noise — it does
/// not correspond to a visible pixel boundary. Therefore this
/// constant is scale-independent by design:
/// `hit_target_epsilon_for_scale` is provided for symmetry but
/// returns the same value regardless of scale.
pub const HIT_TARGET_EPSILON: f32 = 1.0e-3;

/// Compute the audit epsilon for a given display scale factor.
///
/// Returns [`HIT_TARGET_EPSILON`] unchanged — this epsilon is float-noise tolerance,
/// not a visual-significance threshold, so it does not scale with DPI.
/// See [`HIT_TARGET_EPSILON`]'s doc for the rationale.
///
/// Provided for API symmetry with
/// [`spill_epsilon_for_scale`](crate::bounds_probe::spill_epsilon_for_scale)
/// and to document the design decision rather than leaving it implicit.
pub fn hit_target_epsilon_for_scale(_scale: f32) -> f32 {
    HIT_TARGET_EPSILON
}

/// Result of a hit-target probe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitTargetProbe {
    /// Per-axis width deficit (px below `min_size`); zero when at or above.
    pub deficit_x: f32,
    /// Per-axis height deficit.
    pub deficit_y: f32,
    /// Worst of the two axes.
    pub deficit_amount: f32,
    /// True when width is undersized past [`HIT_TARGET_EPSILON`].
    pub undersized_x: bool,
    /// True when height is undersized past [`HIT_TARGET_EPSILON`].
    pub undersized_y: bool,
    /// True when either axis is undersized.
    pub undersized: bool,
}

/// Probe a rendered target against a minimum comfortable size.
///
/// - `target_width` / `target_height` — the target's rendered size in px.
/// - `min_size` — the minimum comfortable size on each axis
///   (default [`DEFAULT_MIN_HIT_TARGET`] = 24 px).
/// - `allow_small` — when `true`, no deficit is flagged (
///   intentionally-tiny affordances like table row handles).
///
/// # Collapsed axes
///
/// A non-positive or `NaN` `target_width`/`target_height` is a
/// maximal deficit (`min_size` on that axis), so a `0x0` control —
/// the worst possible hit target, and the usual symptom of a layout
/// collapse — is flagged rather than reported clean.
///
/// This deliberately does not try to recognise "not yet laid out"
/// from the measurement: a width of zero is indistinguishable from
/// a collapsed width, and resolving that ambiguity in favour of
/// "clean" is what let the worst case audit as compliant. A control
/// that genuinely must not be audited (hidden, not presented,
/// pre-layout) says so explicitly — `allow_small` for an
/// intentionally tiny affordance, or
/// `AccessibilityCheck::HitTarget { hidden: true, .. }` for a
/// control that is not on screen at all.
pub fn hit_target_probe(
    target_width: f32,
    target_height: f32,
    min_size: f32,
    allow_small: bool,
) -> HitTargetProbe {
    // A collapsed (or unrepresentable) axis is the maximal deficit: the target
    // offers `min_size` px less than it must on that axis.
    let axis_deficit = |measured: f32| -> f32 {
        if allow_small {
            0.0
        } else if measured.is_nan() || measured <= 0.0 {
            min_size.max(0.0)
        } else {
            (min_size - measured).max(0.0)
        }
    };
    let deficit_x = axis_deficit(target_width);
    let deficit_y = axis_deficit(target_height);
    let deficit_amount = deficit_x.max(deficit_y);
    let undersized_x = deficit_x > HIT_TARGET_EPSILON;
    let undersized_y = deficit_y > HIT_TARGET_EPSILON;
    HitTargetProbe {
        deficit_x,
        deficit_y,
        deficit_amount,
        undersized_x,
        undersized_y,
        undersized: undersized_x || undersized_y,
    }
}

/// Text size classification for WCAG contrast thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContrastMode {
    /// Small text — minimum ratio 4.5:1 (WCAG AA).
    #[default]
    Small,
    /// Large text (≥ 24 px or ≥ 18.66 px bold) — minimum 3:1 (relaxed AA).
    Large,
}

/// Result of a contrast probe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContrastProbe {
    /// WCAG relative-luminance-based contrast ratio (1.0..21.0).
    pub contrast_ratio: f32,
    /// The minimum ratio for the caller's [`ContrastMode`] (or the explicit
    /// threshold when [`contrast_probe_with`] is used).
    pub min_ratio: f32,
    /// True when the (unrounded) contrast ratio is strictly below `min_ratio`.
    /// WCAG requires computed values are compared without rounding, so this is
    /// a direct `ratio < min_ratio` — 4.499 fails Small, 3.0 passes Large.
    pub low_contrast: bool,
}

/// The sRGB→linear breakpoint. WCAG 2.x uses 0.04045; the alternative
/// 0.03928 value differs by less than 1e-4.
pub const SRGB_LINEAR_BREAKPOINT: f32 = 0.04045;

/// WCAG channel-linearisation: sRGB 0..1 → linear 0..1.
///
/// `c/12.92` when `c <= `[`SRGB_LINEAR_BREAKPOINT`]`, else
/// `((c + 0.055) / 1.055).powf(2.4)`. The input is clamped to `[0, 1]`
/// (non-finite → 0) so an out-of-gamut or `NaN` channel can never produce a
/// bogus luminance.
pub fn channel_linear(c: f32) -> f32 {
    // `NaN.clamp` returns `NaN`, so guard NaN explicitly; `clamp` already maps
    // `+INF → 1.0` and `-INF → 0.0`.
    let c = if c.is_nan() { 0.0 } else { c.clamp(0.0, 1.0) };
    if c <= SRGB_LINEAR_BREAKPOINT {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// WCAG relative luminance of an sRGB `Color`
/// (`0.2126 R + 0.7152 G + 0.0722 B`, each channel linearised).
///
/// The colour's alpha is ignored here — luminance is defined on opaque RGB.
/// Callers with translucent paint should composite first (see
/// [`composite_over`] / [`contrast_ratio`], which composites `fg` over `bg`).
pub fn luminance(color: Color) -> f32 {
    0.2126 * channel_linear(color.r)
        + 0.7152 * channel_linear(color.g)
        + 0.0722 * channel_linear(color.b)
}

/// Alpha-composite `fg` over an opaque `bg` (`bg.a` is treated as
/// 1.0), giving the colour painted on screen. WCAG contrast is
/// defined on the rendered colours, so a translucent text/scrim
/// must be flattened first.
pub fn composite_over(fg: Color, bg: Color) -> Color {
    let a = if fg.a.is_finite() {
        fg.a.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mix = |f: f32, b: f32| f * a + b * (1.0 - a);
    Color::from_rgb(mix(fg.r, bg.r), mix(fg.g, bg.g), mix(fg.b, bg.b))
}

/// Compute the WCAG contrast ratio between a foreground and an
/// opaque background (`1.0..21.0`).
///
/// `fg` is alpha-composited over `bg` first (see [`composite_over`]),
/// and `bg` itself is treated as opaque (`bg.a` is ignored). This is
/// the correct entry point only when the background is a solid fill
/// — a toast surface, an avatar disc, a filled button. When the
/// background can be translucent (a scrim, a hover wash, an
/// accent-soft tint), its alpha must not be discarded without
/// notice: use [`contrast_ratio_over`] and pass the surface the
/// background sits on, or the ratio is measured against a colour
/// that is never painted (a false 21:1 for transparent-white over
/// black, whose visible colours have 1:1 contrast).
pub fn contrast_ratio(fg: Color, bg: Color) -> f32 {
    // An opaque background is its own painted colour, so composite
    // over itself (identity for `a == 1.0`, and honouring `a` for
    // the translucent case in terms of itself, which is the
    // alpha-discarding behaviour — see `contrast_ratio_over` for
    // the correct translucent handling).
    contrast_ratio_over(fg, bg, bg)
}

/// Compute the WCAG contrast ratio between a foreground and a
/// possibly translucent background, resolving the background against
/// `backdrop` (`1.0..21.0`).
///
/// WCAG contrast is defined on the colours painted on screen. When
/// the background layer is translucent (a scrim, a hover overlay,
/// an accent-soft tint), the pixel a user sees is that layer
/// composited over whatever is behind it. Discarding the
/// background's alpha — as a plain [`contrast_ratio`] does — scores
/// against a colour that is never on screen: a transparent-white
/// background over a black `backdrop` would be read as white (a
/// false 21:1) when the visible colours have 1:1 contrast.
///
/// This composites `bg` over `backdrop` to obtain the painted
/// surface, then composites `fg` over that surface, and scores the
/// two painted colours.
///
/// - `backdrop` is the colour behind `bg` — the surface `bg` is
///   painted on (the window/page background for a top-level scrim,
///   the card surface for a hover wash on a list row). It is
///   treated as opaque; if it too can be translucent, resolve it
///   first.
/// - `bg.a == 0.0` is expressible intent: a fully transparent
///   background shows the backdrop through entirely, so the
///   surface is `backdrop`.
/// - `bg.a == 1.0` makes `backdrop` irrelevant (compositing an
///   opaque colour is the identity), so an opaque-background
///   caller is unaffected.
pub fn contrast_ratio_over(fg: Color, bg: Color, backdrop: Color) -> f32 {
    // Resolve the translucent background over its backdrop to get
    // the colour painted as the surface, then paint fg over that
    // surface.
    let painted_bg = composite_over(bg, backdrop);
    let painted_fg = composite_over(fg, painted_bg);
    let l1 = luminance(painted_fg);
    let l2 = luminance(painted_bg);
    let (hi, lo) = if l1 > l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

/// `tone` itself when it reaches `min_ratio` against the opaque `bg`, else
/// `tone` mixed toward white (on a dark `bg`) or black (on a light one) only
/// as far as `min_ratio` needs, so a tone glyph keeps its hue instead of
/// turning neutral. Returns the pure endpoint when even that falls short.
pub fn legible_tone(tone: Color, bg: Color, min_ratio: f32) -> Color {
    if contrast_ratio(tone, bg) >= min_ratio {
        return tone;
    }
    let end = if luminance(bg) < 0.18 {
        Color::WHITE
    } else {
        Color::BLACK
    };
    let mix = |t: f32| {
        Color::from_rgba(
            tone.r + (end.r - tone.r) * t,
            tone.g + (end.g - tone.g) * t,
            tone.b + (end.b - tone.b) * t,
            tone.a,
        )
    };
    if contrast_ratio(mix(1.0), bg) < min_ratio {
        return end;
    }
    // Contrast grows monotonically along the mix toward the far endpoint.
    let (mut lo, mut hi) = (0.0_f32, 1.0_f32);
    for _ in 0..16 {
        let mid = (lo + hi) / 2.0;
        if contrast_ratio(mix(mid), bg) >= min_ratio {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    mix(hi)
}

/// Probe a foreground/background pair for WCAG 1.4.3 contrast compliance using
/// the standard [`ContrastMode`] threshold (4.5 small / 3.0 large).
pub fn contrast_probe(fg: Color, bg: Color, mode: ContrastMode, allow_low: bool) -> ContrastProbe {
    let min_ratio = match mode {
        ContrastMode::Small => DEFAULT_MIN_CONTRAST_RATIO,
        ContrastMode::Large => DEFAULT_MIN_CONTRAST_RATIO_LARGE,
    };
    contrast_probe_with(fg, bg, min_ratio, allow_low)
}

/// Probe with an explicit `min_ratio` threshold, so a host can tighten or
/// loosen the policy. Compares the unrounded ratio strictly
/// (`ratio < min_ratio`).
pub fn contrast_probe_with(fg: Color, bg: Color, min_ratio: f32, allow_low: bool) -> ContrastProbe {
    let ratio = contrast_ratio(fg, bg);
    let low_contrast = !allow_low && ratio < min_ratio;
    ContrastProbe {
        contrast_ratio: ratio,
        min_ratio,
        low_contrast,
    }
}
