use iced::Color;

use super::ContrastFloor;
use super::color_math::{quantise_srgb, wcag_contrast, wcag_luminance};

/// Result of searching for contrast along the input's fixed OkLab `a`/`b` path.
///
/// Returned colours are converted, clamped, and quantized in sRGB, so their
/// measured hue and chroma can differ slightly from the search path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RaisedInk {
    /// The input already cleared the floor and is byte-identical.
    AlreadyLegible(Color),
    /// The quantized result produced from a pre-conversion candidate on the
    /// fixed OkLab `a`/`b` search path cleared the floor.
    Raised(Color),
    /// No pre-conversion candidate on the fixed OkLab `a`/`b` search path
    /// produces a quantized result that clears the floor.
    Impossible {
        /// The highest-contrast quantized result produced from the search path.
        best: Color,
        /// The best colour's ratio, which is below the requested floor.
        achieved: f32,
    },
}

impl RaisedInk {
    /// Returns the paintable colour, including the best result when impossible.
    pub fn color(self) -> Color {
        match self {
            Self::AlreadyLegible(c) | Self::Raised(c) => c,
            Self::Impossible { best, .. } => best,
        }
    }

    /// Returns `false` only for [`Self::Impossible`].
    pub fn meets_floor(self) -> bool {
        !matches!(self, Self::Impossible { .. })
    }
}

/// Returns an already-legible input unchanged; otherwise searches fixed-`a`/`b`
/// OkLab candidates and tests their quantized sRGB results at the named floor.
pub fn raise_to_floor(color: Color, on: Color, floor: ContrastFloor) -> RaisedInk {
    raise_to_target(color, on, floor.minimum_ratio())
}

/// Returns an input already meeting `target` unchanged; otherwise searches
/// fixed-`a`/`b` OkLab candidates and tests their quantized sRGB results.
///
/// Callers should supply a positive finite target. A target already met by the
/// input—including non-positive values and negative infinity—returns
/// [`RaisedInk::AlreadyLegible`]. Positive infinity returns
/// [`RaisedInk::Impossible`]. `NaN` returns [`RaisedInk::Raised`] with the
/// higher-contrast reachable endpoint, without a meaningful floor guarantee.
pub fn raise_to_target(color: Color, on: Color, target: f32) -> RaisedInk {
    use super::color_math::memo;
    memo::lookup(&memo::RAISE, memo::key([color, on], target), || {
        raise_to_target_uncached(color, on, target)
    })
}

fn raise_to_target_uncached(color: Color, on: Color, target: f32) -> RaisedInk {
    if wcag_contrast(color, on) >= target {
        return RaisedInk::AlreadyLegible(color);
    }

    let (l0, a0, b0) = srgb_to_oklab(color);
    let alpha = color.a;
    // Search candidates keep OkLab a/b fixed, then convert and quantize before
    // contrast checks.
    let render = |l: f32| -> Color {
        let mut c = oklab_to_srgb(l, a0, b0);
        c.a = alpha;
        quantise_srgb(c)
    };

    // The reachable band is the OkLab-L interval for which (L, a0, b0) stays in
    // the sRGB gamut. Holding a0/b0 fixed, a nonzero chroma cannot reach L=0 or
    // L=1, so this interval bounds the luminance — and therefore the contrast —
    // available along this fixed-a/b path. `color` itself is in gamut, so l0
    // lies inside.
    let (l_lo, l_hi) = in_gamut_l_range(l0, a0, b0);

    let contrast_at = |l: f32| wcag_contrast(render(l), on);
    let c_lo = contrast_at(l_lo);
    let c_hi = contrast_at(l_hi);

    // Contrast is V-shaped in L (minimum where the ink's luminance equals the
    // backdrop's), so the maximum over the band is at one of the two extremes.
    // If neither extreme clears the floor, nothing in between does either.
    if c_lo < target && c_hi < target {
        let (best_l, achieved) = if c_lo >= c_hi {
            (l_lo, c_lo)
        } else {
            (l_hi, c_hi)
        };
        return RaisedInk::Impossible {
            best: render(best_l),
            achieved,
        };
    }

    let l_star = l_of_matching_luminance(on, l_lo, l_hi, &render);

    let dark = if c_lo >= target {
        let hi = l0.min(l_star);
        Some(bisect_toward_darker(l_lo, hi, target, &contrast_at))
    } else {
        None
    };
    let light = if c_hi >= target {
        let lo = l0.max(l_star);
        Some(bisect_toward_lighter(lo, l_hi, target, &contrast_at))
    } else {
        None
    };

    let chosen = match (dark, light) {
        (Some(d), Some(l)) => {
            if (l0 - d) <= (l - l0) {
                d
            } else {
                l
            }
        }
        (Some(d), None) => d,
        (None, Some(l)) => l,
        (None, None) => {
            if c_lo >= c_hi {
                l_lo
            } else {
                l_hi
            }
        }
    };
    RaisedInk::Raised(render(chosen))
}

// The OkLab-L interval `[lo, hi]` containing `l0` for which `(L, a, b)` stays
// inside the sRGB gamut. Found by bisection outward from `l0` (which is in
// gamut by construction). Each side exits the gamut monotonically as a channel
// crosses 0 (going down) or 1 (going up).
pub(super) fn in_gamut_l_range(l0: f32, a: f32, b: f32) -> (f32, f32) {
    let in_gamut = |l: f32| {
        let c = oklab_to_srgb(l, a, b);
        const E: f32 = 1.0 / 512.0;
        c.r >= -E && c.r <= 1.0 + E && c.g >= -E && c.g <= 1.0 + E && c.b >= -E && c.b <= 1.0 + E
    };
    let lo = if in_gamut(0.0) {
        0.0
    } else {
        let (mut lo, mut hi) = (0.0_f32, l0); // lo out, hi in
        for _ in 0..24 {
            let mid = 0.5 * (lo + hi);
            if in_gamut(mid) {
                hi = mid;
            } else {
                lo = mid;
            }
        }
        hi
    };
    let hi = if in_gamut(1.0) {
        1.0
    } else {
        let (mut lo, mut hi) = (l0, 1.0_f32); // lo in, hi out
        for _ in 0..24 {
            let mid = 0.5 * (lo + hi);
            if in_gamut(mid) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        lo
    };
    (lo, hi)
}

// The lightness in `[lo, hi]` whose rendered luminance matches `on`'s (the
// contrast V-minimum). Luminance rises monotonically with L at fixed chroma,
// so a bisection is sound; clamps to an endpoint when the backdrop is outside
// the band's luminance range.
pub(super) fn l_of_matching_luminance(
    on: Color,
    lo: f32,
    hi: f32,
    render: &impl Fn(f32) -> Color,
) -> f32 {
    let on_lum = wcag_luminance(on);
    if wcag_luminance(render(lo)) >= on_lum {
        return lo;
    }
    if wcag_luminance(render(hi)) <= on_lum {
        return hi;
    }
    let (mut lo, mut hi) = (lo, hi);
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if wcag_luminance(render(mid)) < on_lum {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

// Largest L in `[lo, hi]` whose contrast clears `target`, where `render(lo)`
// passes and contrast falls as L rises (the darker arm). Returns the passing
// endpoint on any monotonicity surprise, so the result always clears `target`.
pub(super) fn bisect_toward_darker(
    lo: f32,
    hi: f32,
    target: f32,
    contrast_at: &impl Fn(f32) -> f32,
) -> f32 {
    if contrast_at(hi) >= target {
        return hi;
    }
    let (mut lo, mut hi) = (lo, hi); // lo passes, hi fails
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if contrast_at(mid) >= target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

// Smallest L in `[lo, hi]` whose contrast clears `target`, where `render(hi)`
// passes and contrast rises as L climbs (the lighter arm). Returns the passing
// endpoint on any monotonicity surprise, so the result always clears `target`.
pub(super) fn bisect_toward_lighter(
    lo: f32,
    hi: f32,
    target: f32,
    contrast_at: &impl Fn(f32) -> f32,
) -> f32 {
    if contrast_at(lo) >= target {
        return lo;
    }
    let (mut lo, mut hi) = (lo, hi); // lo fails, hi passes
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if contrast_at(mid) >= target {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

pub(super) fn srgb_to_linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

// Linear channel → sRGB gamma (not clamped, so callers can detect out-of-gamut).
pub(super) fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Returns Euclidean OkLab distance between two opaque colours.
pub fn perceptual_distance(a: Color, b: Color) -> f32 {
    let (l1, a1, b1) = srgb_to_oklab(a);
    let (l2, a2, b2) = srgb_to_oklab(b);
    ((l1 - l2).powi(2) + (a1 - a2).powi(2) + (b1 - b2).powi(2)).sqrt()
}

pub(crate) fn srgb_to_oklab(c: Color) -> (f32, f32, f32) {
    let r = srgb_to_linear(c.r);
    let g = srgb_to_linear(c.g);
    let b = srgb_to_linear(c.b);

    let l = 0.412_221_47 * r + 0.536_332_54 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;

    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();

    (
        0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_,
        1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_,
        0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_,
    )
}

// OkLab `(L, a, b)` → sRGB [`Color`] (alpha 1.0), not gamut-clamped so an
// out-of-gamut lightness is detectable by [`in_gamut_l_range`]. Downstream
// [`quantise_srgb`] clamps for display.
pub(crate) fn oklab_to_srgb(l: f32, a: f32, b: f32) -> Color {
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;

    let l3 = l_ * l_ * l_;
    let m3 = m_ * m_ * m_;
    let s3 = s_ * s_ * s_;

    let r = 4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3;
    let g = -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_38 * s3;
    let b = -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3;

    Color {
        r: linear_to_srgb(r),
        g: linear_to_srgb(g),
        b: linear_to_srgb(b),
        a: 1.0,
    }
}
