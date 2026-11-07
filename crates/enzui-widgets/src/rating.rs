//! A read-only star score with half-star paint and image accessibility semantics.
//!
//! Finite values clamp to 0..max and round to the nearest half for paint and
//! default spoken text. Numeric semantics retain the clamped, unrounded value.
//! Non-finite values and [`EnzRating::unrated`] paint empty stars, announce
//! [`RATING_UNRATED_ANNOUNCEMENT`] and omit numeric semantics.
//!
//! Defaults are five 16 px stars with 2 px gaps. Filled stars are amber gold,
//! deepened where needed to keep 3:1 against the live theme's page, as a
//! rating reads everywhere from app stores to file managers.
//! Empty stars are faint muted text. [`EnzRating::accent`] and
//! [`EnzRating::dimmed`] pin either. The widget has no rating-input gestures.

use enzui_core::semantics::{NumericValue, Role as SemRole, Semantics};
use enzui_theme::Palette;
use iced::widget::canvas::{self, Cache, Frame, Path, Program};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size};
use std::cell::Cell;
use std::f32::consts::PI;

/// Default number of stars in the row.
pub const DEFAULT_MAX: u8 = 5;
/// Default logical star size (px). Matches a body-text-adjacent glyph.
pub const DEFAULT_STAR_PX: f32 = 16.0;
/// Default gap (px) between adjacent stars.
pub const DEFAULT_GAP_PX: f32 = 2.0;

/// What an unrated rating announces. A widget with no known rating must
/// say so rather than fabricate `0 out of 5`.
pub const RATING_UNRATED_ANNOUNCEMENT: &str = "unrated";

/// Clamp a finite value to 0..max and round to the nearest half, with ties upward.
/// All non-finite inputs return NaN.
#[must_use]
pub fn quantise_half(value: f32, max: u8) -> f32 {
    if !value.is_finite() {
        return f32::NAN;
    }
    let m = f32::from(max);
    let v = value.clamp(0.0, m);
    (v * 2.0).round() / 2.0
}

fn star_path(cx: f32, cy: f32, size: f32) -> Path {
    let r_o = size * 0.5;
    let r_i = r_o * (PI / 10.0).sin() / (PI / 5.0).cos();
    Path::new(|p| {
        for i in 0..10 {
            let angle = -PI / 2.0 + (i as f32) * PI / 5.0;
            let r = if i % 2 == 0 { r_o } else { r_i };
            let x = cx + r * angle.cos();
            let y = cy + r * angle.sin();
            if i == 0 {
                p.move_to(Point::new(x, y));
            } else {
                p.line_to(Point::new(x, y));
            }
        }
        p.close();
    })
}

/// Analytic star-kind counts and nominal horizontal bounds returned by [`draw_static`].
/// These describe requested geometry, not sampled pixels or visible ink.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RatingDrawReport {
    /// Full-star draw requests using filled-star ink.
    pub filled: u8,
    /// Half-star draw requests using left filled-star ink over a dimmed base.
    pub half: u8,
    /// Empty-star draw requests using dimmed ink.
    pub empty: u8,
    /// Left edge of the first nominal star square.
    pub ink_left: f32,
    /// Right edge of the final nominal star square.
    pub ink_right: f32,
}

/// Paint at least one star and return analytic counts and nominal strip bounds.
/// Pass a finite half-star value in 0..max, positive finite star size and finite
/// nonnegative gap. Zero max paints one star. The plot height sets the vertical
/// centre; its width does not resize or clip the strip. Colours are already resolved.
///
/// # Panics
/// Debug builds panic if nominal star bounds exceed the computed strip width by
/// more than 0.5 logical pixels.
#[expect(
    clippy::too_many_arguments,
    reason = "public paint probe keeps each resolved geometry and colour input explicit"
)]
pub fn draw_static(
    frame: &mut Frame,
    plot: Size,
    quantised: f32,
    max: u8,
    star: f32,
    gap: f32,
    accent: Color,
    dimmed: Color,
) -> RatingDrawReport {
    let max_u = max.max(1) as usize;
    let cy = plot.height * 0.5;
    let strip_w = (max_u as f32) * star + ((max_u as f32) - 1.0) * gap;
    let x0_strip = 0.0_f32;
    let full = quantised.floor() as u8;
    let has_half = (quantised - f32::from(full)) >= 0.25 && (quantised - f32::from(full)) < 0.75;
    let (mut filled, mut half, mut empty) = (0u8, 0u8, 0u8);
    let mut ink_left = f32::INFINITY;
    let mut ink_right: f32 = 0.0;

    for i in 0..(max_u as u8) {
        let idx = f32::from(i);
        let cx = x0_strip + idx * (star + gap) + star * 0.5;
        let kind = if i < full {
            filled += 1;
            StarKind::Full
        } else if i == full && has_half {
            half += 1;
            StarKind::Half
        } else {
            empty += 1;
            StarKind::Empty
        };
        match kind {
            StarKind::Full => {
                frame.fill(&star_path(cx, cy, star), accent);
            }
            StarKind::Half => {
                frame.fill(&star_path(cx, cy, star), dimmed);
                frame.fill(&star_left_half_path(cx, cy, star), accent);
            }
            StarKind::Empty => {
                frame.fill(&star_path(cx, cy, star), dimmed);
            }
        }
        let star_left = cx - star * 0.5;
        let star_right = cx + star * 0.5;
        ink_left = ink_left.min(star_left);
        ink_right = ink_right.max(star_right);
    }
    debug_assert!(
        ink_right <= strip_w + 0.5,
        "star ink {ink_right} exceeds strip width {strip_w}"
    );

    RatingDrawReport {
        filled,
        half,
        empty,
        ink_left: if ink_left.is_finite() { ink_left } else { 0.0 },
        ink_right,
    }
}

// The half path shares the full star's axis vertices to avoid a seam.
fn star_left_half_path(cx: f32, cy: f32, size: f32) -> Path {
    let r_o = size * 0.5;
    let r_i = r_o * (PI / 10.0).sin() / (PI / 5.0).cos();
    Path::new(|p| {
        for (n, &i) in [0usize, 9, 8, 7, 6, 5].iter().enumerate() {
            let angle = -PI / 2.0 + (i as f32) * PI / 5.0;
            let r = if i % 2 == 0 { r_o } else { r_i };
            let x = cx + r * angle.cos();
            let y = cy + r * angle.sin();
            if n == 0 {
                p.move_to(Point::new(x, y));
            } else {
                p.line_to(Point::new(x, y));
            }
        }
        p.close();
    })
}

#[derive(Clone, Copy)]
enum StarKind {
    Full,
    Half,
    Empty,
}

#[derive(Default)]
struct RatingState {
    cache: Cache,
    last: Cell<Option<u64>>,
}

struct RatingProgram {
    quantised: f32,
    max: u8,
    star: f32,
    gap: f32,
    accent: Option<Color>,
    dimmed: Option<Color>,
}

/// The default filled-star gold on `palette`: amber, deepened only as far as
/// the 3:1 a meaningful graphic needs against the page.
pub fn star_gold(palette: &Palette) -> Color {
    Palette::with_contrast_on(palette.background, palette.rating, 3.0)
}

/// The default empty-star ink on `palette`.
fn star_dimmed(palette: &Palette) -> Color {
    Color {
        a: 0.35,
        ..palette.text_muted
    }
}

impl<Message> Program<Message> for RatingProgram {
    type State = RatingState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let quantised = self.quantised;
        let max = self.max;
        let star = self.star;
        let gap = self.gap;
        let palette = crate::theme_lookup::palette(theme);
        let accent = self.accent.unwrap_or_else(|| star_gold(&palette));
        let dimmed = self.dimmed.unwrap_or_else(|| star_dimmed(&palette));
        let key = rating_key(quantised, max, star, gap, accent, dimmed);
        if state.last.get() != Some(key) {
            state.cache.clear();
            state.last.set(Some(key));
        }
        let geo = state.cache.draw(renderer, bounds.size(), |frame| {
            draw_static(
                frame,
                bounds.size(),
                quantised,
                max,
                star,
                gap,
                accent,
                dimmed,
            );
        });
        vec![geo]
    }
}

fn rating_key(q: f32, max: u8, star: f32, gap: f32, a: Color, d: Color) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    let mut mix = |bytes: [u8; 4]| {
        for b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    mix(q.to_bits().to_le_bytes());
    mix(u32::from(max).to_le_bytes());
    for f in [star, gap, a.r, a.g, a.b, a.a, d.r, d.g, d.b, d.a] {
        mix(f.to_bits().to_le_bytes());
    }
    h
}

/// A fractional star-rating display widget. See the module docs.
pub struct EnzRating {
    value: f32,
    max: u8,
    label: Option<String>,
    value_text: Option<String>,
    star: f32,
    gap: f32,
    accent: Option<Color>,
    dimmed: Option<Color>,
    unrated: bool,
}

/// Build a five-star display with clamped value and half-star paint.
/// Non-finite input starts unrated, with no numeric accessibility value.
pub fn enz_rating(value: f32) -> EnzRating {
    EnzRating {
        value,
        max: DEFAULT_MAX,
        label: None,
        value_text: None,
        star: DEFAULT_STAR_PX,
        gap: DEFAULT_GAP_PX,
        accent: None,
        dimmed: None,
        unrated: !value.is_finite(),
    }
}

impl EnzRating {
    /// Set the accessible name (e.g. `"Review score"`).
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Override spoken value text for rated scores; unrated scores always say "unrated".
    /// The default is "{quantised} out of {max} stars".
    #[must_use]
    pub fn value_text(mut self, s: impl Into<String>) -> Self {
        self.value_text = Some(s.into());
        self
    }

    /// The total number of stars (default `5`). Clamped to at least `1`.
    #[must_use]
    pub fn max(mut self, max: u8) -> Self {
        self.max = max.max(1);
        self
    }

    /// Set star size in logical pixels; defaults to 16.
    /// Non-finite or nonpositive values retain the previous size.
    #[must_use]
    pub fn star(mut self, px: f32) -> Self {
        if px.is_finite() && px > 0.0 {
            self.star = px;
        }
        self
    }

    /// Set the gap in logical pixels; defaults to 2.
    /// Non-finite or negative values retain the previous gap; zero is allowed.
    #[must_use]
    pub fn gap(mut self, px: f32) -> Self {
        if px.is_finite() && px >= 0.0 {
            self.gap = px;
        }
        self
    }

    /// Pin filled-star ink; defaults to the theme's star gold ([`star_gold`]).
    #[must_use]
    pub fn accent(mut self, c: Color) -> Self {
        self.accent = Some(c);
        self
    }

    /// Pin empty-star ink; defaults to the theme's muted text at 0.35 alpha.
    #[must_use]
    pub fn dimmed(mut self, c: Color) -> Self {
        self.dimmed = Some(c);
        self
    }

    /// Mark the score unknown, painting empty stars and announcing "unrated".
    /// Non-finite constructor input sets this implicitly; numeric semantics are omitted.
    #[must_use]
    pub fn unrated(mut self) -> Self {
        self.unrated = true;
        self
    }

    fn quantised(&self) -> f32 {
        if self.unrated {
            f32::NAN
        } else {
            quantise_half(self.value, self.max)
        }
    }

    fn announced_value(&self) -> String {
        if self.unrated {
            return RATING_UNRATED_ANNOUNCEMENT.to_owned();
        }
        if let Some(s) = &self.value_text {
            return s.clone();
        }
        let q = self.quantised();
        enzui_core::localized!("enzui-rating-out-of-stars", "rating" => format_value(q), "maximum" => self.max)
    }

    fn numeric(&self) -> Option<NumericValue> {
        if self.unrated {
            return None;
        }
        Some(NumericValue {
            value: f64::from(self.value.clamp(0.0, f32::from(self.max))),
            min: 0.0,
            max: f64::from(self.max),
            step: Some(0.5),
        })
    }

    /// Describe an image leaf with label and spoken value.
    /// Rated numeric values clamp to 0..max without half-star rounding, with step 0.5;
    /// unrated values omit the numeric domain.
    pub fn semantics(&self) -> Semantics {
        let announced = self.announced_value();
        let label = self.label.clone().or_else(|| Some(announced.clone()));
        let mut s = Semantics::new(SemRole::Image)
            .label_opt(label)
            .value_opt(Some(announced));
        if let Some(n) = self.numeric() {
            s = s.numeric(n);
        }
        s
    }

    /// The widget's intrinsic pixel width — enough for `max` stars plus gaps.
    #[must_use]
    pub fn intrinsic_width(&self) -> f32 {
        f32::from(self.max) * self.star + (f32::from(self.max) - 1.0) * self.gap
    }

    /// Consume the builder and produce the element tree.
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        let semantics = self.semantics();
        let quantised = self.quantised();
        let max = self.max;
        let star = self.star;
        let gap = self.gap;
        let paint_q = if quantised.is_finite() {
            quantised
        } else {
            0.0
        };
        let program = RatingProgram {
            quantised: paint_q,
            max,
            star,
            gap,
            accent: self.accent,
            dimmed: self.dimmed,
        };
        let width = self.intrinsic_width();
        let canvas: Element<'a, Message> = canvas::Canvas::new(program)
            .width(Length::Fixed(width))
            .height(Length::Fixed(star))
            .into();
        crate::semantics_container::SemanticsContainer::leaf(canvas, semantics).into()
    }
}

impl<'a, Message: 'a> From<EnzRating> for Element<'a, Message> {
    fn from(r: EnzRating) -> Self {
        r.into_element()
    }
}

fn format_value(v: f32) -> String {
    if (v - v.round()).abs() < 1e-4 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}
