//! Circular gauges with normalized numeric semantics and an explicit unsampled state.
//!
//! Rings sweep clockwise from twelve o’clock. A sampled stroke meeting the actual
//! canvas extent uses a track disc and fill wedge instead of a zero-radius ring.
//! [`enz_gauge`] represents a measurement, including measured zero;
//! [`enz_gauge_sampled`] accepts [`Sampled::Unsampled`] for a dashed, muted placeholder
//! with no accessibility numeric value. Its stroke is capped to preserve a hole.
//!
//! [`gauge_domain`] sorts reversed bounds, preserves equal bounds, clamps finite
//! values and substitutes the lower bound for non-finite values. Non-finite bounds
//! select the empty default `0..=1` domain. Sampled paint and numeric semantics use
//! this same domain; equal bounds give an empty sweep.
//!
//! Default center text is a percentage for `0..=1`, otherwise a domain value.
//! Custom text remains visual, while [`value_text_reconciles`] heuristically checks
//! its first ASCII numeric run before announcing it. Contradicting parsed claims
//! use derived domain text instead. Unparseable claims are treated as opaque text.
//! Empty text suppresses formatted output. Unsampled gauges instead request an
//! em dash and announce “not measured yet,” unless explicitly suppressed. Center
//! text renders only with a positive hole-width budget and may be ellipsized.
//!
//! Colors follow the runtime theme unless pinned. Per-instance canvas geometry is
//! cached by fraction, stroke, resolved colors, sample state and motion policy;
//! bounds changes are handled by the canvas cache. A new value sweeps the ring
//! there over the theme's `motion-normal` duration; reduced motion (the theme
//! or [`EnzGauge::reduced_motion`]) lands it at once. For linear indicators,
//! use [`crate::progress_bar`].

use enzui_core::metrics::{FinitePositive, OrderedRangeF32, UnitInterval};
use enzui_core::semantics::{NumericValue, Role as SemRole, Semantics};
use enzui_theme::{Palette, SpacingScale, TypographyScale};
use iced::widget::canvas::{self, Cache, Frame, LineCap, Path, Program, Stroke};
use iced::widget::{column, container};
use iced::{Color, Element, Length, Point, Radians, Rectangle, Renderer};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Process-wide count of ring-cache geometry rebuilds, including empty geometry.
///
/// Cache-hit replay does not increment this relaxed diagnostic tally.
#[doc(hidden)]
pub static RING_TESSELLATIONS: AtomicUsize = AtomicUsize::new(0);

/// Read [`RING_TESSELLATIONS`].
#[doc(hidden)]
pub fn ring_tessellations() -> usize {
    RING_TESSELLATIONS.load(Ordering::Relaxed)
}

/// Reset [`RING_TESSELLATIONS`] to zero and return its previous value.
#[doc(hidden)]
pub fn reset_ring_tessellations() -> usize {
    RING_TESSELLATIONS.swap(0, Ordering::Relaxed)
}

/// Distinguish an absent measurement from a measured value, including zero.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Sampled<T> {
    /// No measurement has been taken yet. Not a value of zero.
    #[default]
    Unsampled,
    /// A real measurement. `Sampled(0.0)` is a measured zero.
    Sampled(T),
}

impl<T> Sampled<T> {
    /// Is this the "no measurement yet" state?
    #[must_use]
    pub fn is_unsampled(&self) -> bool {
        matches!(self, Sampled::Unsampled)
    }

    /// Return the measurement, or `None` when unsampled.
    #[must_use]
    pub fn value(self) -> Option<T> {
        match self {
            Sampled::Unsampled => None,
            Sampled::Sampled(v) => Some(v),
        }
    }

    /// Apply `f` to the measurement, preserving [`Sampled::Unsampled`].
    #[must_use]
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Sampled<U> {
        match self {
            Sampled::Unsampled => Sampled::Unsampled,
            Sampled::Sampled(v) => Sampled::Sampled(f(v)),
        }
    }

    /// Use `fallback` for an absent measurement.
    ///
    /// For visible values or announcements, preserve the unsampled distinction instead.
    #[must_use]
    pub fn unwrap_or(self, fallback: T) -> T {
        match self {
            Sampled::Unsampled => fallback,
            Sampled::Sampled(v) => v,
        }
    }
}

impl<T> From<T> for Sampled<T> {
    fn from(v: T) -> Self {
        Sampled::Sampled(v)
    }
}

impl<T> From<Option<T>> for Sampled<T> {
    fn from(v: Option<T>) -> Self {
        match v {
            None => Sampled::Unsampled,
            Some(v) => Sampled::Sampled(v),
        }
    }
}

/// What an unsampled gauge / progress bar paints where a value
/// would go: an em dash, never `0` or `0%`.
pub const UNSAMPLED_VALUE_TEXT: &str = "—";

/// Accessible value text used when no measurement exists.
pub const UNSAMPLED_ANNOUNCEMENT: &str = "not measured yet";

/// Equal on/off dash lengths in logical pixels for the placeholder ring.
///
/// Each length is `max(stroke * 1.5, 2)`; non-finite strokes use two pixels.
#[must_use]
pub fn unsampled_dash(stroke: f32) -> [f32; 2] {
    let on = if stroke.is_finite() {
        (stroke * 1.5).max(2.0)
    } else {
        2.0
    };
    [on, on]
}

/// Cap placeholder stroke at one third of the extent to preserve its ring hole.
///
/// Finite strokes clamp to `0..=cap`; non-finite strokes use the cap. This helper
/// does not require a positive stroke, unlike the gauge builder’s normalization.
#[must_use]
pub fn unsampled_stroke(extent: f32, stroke: f32) -> f32 {
    let cap = (extent / 3.0).max(0.0);
    if stroke.is_finite() {
        stroke.clamp(0.0, cap)
    } else {
        cap
    }
}

/// Select the accent, success, warning or danger fill token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GaugeTone {
    /// `accent` (default).
    #[default]
    Accent,
    /// `tone-success-fg`.
    Success,
    /// `tone-warning-fg`.
    Warning,
    /// `tone-error-fg`.
    Danger,
}

/// Colour tokens for the gauge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GaugeTokens {
    /// `accent` — the default fill-arc colour.
    pub accent_color: Color,
    /// `tone-success-fg`.
    pub success_color: Color,
    /// `tone-warning-fg`.
    pub warning_color: Color,
    /// `tone-error-fg`.
    pub danger_color: Color,
    /// Track color derived from [`Palette::meter_track`].
    pub track_color: Color,
    /// Reserved track-outline color; the current gauge drawing does not use it.
    pub track_border_color: Color,
    /// Centred value text.
    pub text_color: Color,
    /// Caption label under the ring.
    pub muted_color: Color,
    /// Muted placeholder color, separate from the measured track color.
    pub unsampled_color: Color,
}

impl GaugeTokens {
    /// Derive arc, track and text colors from a palette.
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            accent_color: p.accent,
            success_color: p.tone_success_fg,
            warning_color: p.tone_warning_fg,
            danger_color: p.tone_error_fg,
            track_color: p.meter_track(),
            track_border_color: p.meter_track_border(),
            text_color: p.text,
            muted_color: p.text_muted,
            unsampled_color: p.text_muted,
        }
    }
    /// Preset built from the built-in dark palette. Prefer
    /// [`GaugeTokens::from_palette`] with the live palette when one is in scope.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Preset built from the built-in light palette. Prefer
    /// [`GaugeTokens::from_palette`] with the live palette when one is in scope.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
    /// Resolve the fill colour for a semantic tone.
    pub fn color_for(&self, tone: GaugeTone) -> Color {
        match tone {
            GaugeTone::Accent => self.accent_color,
            GaugeTone::Success => self.success_color,
            GaugeTone::Warning => self.warning_color,
            GaugeTone::Danger => self.danger_color,
        }
    }
}

/// Default ring diameter.
pub const DEFAULT_DIAMETER: f32 = 80.0;
/// Ring stroke width used for a non-finite or non-positive width.
pub const DEFAULT_TRACK_WIDTH: f32 = 8.0;

/// The default stroke as a share of the diameter: a ring keeps its weight
/// as it grows, as a system gauge does, instead of thinning out.
pub const TRACK_RATIO: f32 = 0.12;

/// The thinnest default stroke, in logical pixels.
pub const MIN_TRACK_WIDTH: f32 = 4.0;

/// The stroke a gauge `diameter` across draws unless
/// [`EnzGauge::track_width`] sets one: [`TRACK_RATIO`] of the diameter,
/// at least [`MIN_TRACK_WIDTH`].
#[must_use]
pub fn default_track_width(diameter: f32) -> f32 {
    (diameter * TRACK_RATIO).round().max(MIN_TRACK_WIDTH)
}

/// The centre value's text size for a ring `diameter` across: a fifth of
/// it, never smaller than [`value_text_size`].
#[must_use]
pub fn value_text_size_for(diameter: f32) -> f32 {
    (diameter * 0.2).round().max(value_text_size())
}

/// The shape a gauge draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GaugeStyle {
    /// A full ring filling clockwise from twelve o'clock (default).
    #[default]
    Ring,
    /// A 270° arc open at the bottom, filling clockwise from its lower-left
    /// end: a dial, for a level within a range (a speedometer, a meter).
    Arc,
}

/// Where a [`GaugeStyle::Arc`] starts: lower left, at 135°.
pub const ARC_START_ANGLE: f32 = std::f32::consts::FRAC_PI_4 * 3.0;

/// How far a [`GaugeStyle::Arc`] sweeps: three quarters of a turn.
pub const ARC_SWEEP: f32 = std::f32::consts::FRAC_PI_2 * 3.0;

/// Return the default spacing scale’s `space3` gap in logical pixels.
pub fn label_spacing() -> f32 {
    SpacingScale::default().space3
}

/// Return the ambient typography scale’s caption size in logical pixels.
pub fn label_size() -> f32 {
    TypographyScale::ambient().label_size
}

/// Return the ambient typography scale’s center-text size in logical pixels.
pub fn value_text_size() -> f32 {
    TypographyScale::ambient().body_size
}

/// The domain the gauge falls back to when a bound is not expressible at all
/// (NaN / ±∞): the constructor's own `0.0..=1.0`, drawn and announced empty.
pub const FALLBACK_MIN: f32 = 0.0;
/// Upper end of [`FALLBACK_MIN`]'s domain.
pub const FALLBACK_MAX: f32 = 1.0;

/// Finite, ordered bounds and an in-range value shared by sampled paint and semantics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GaugeDomain {
    min: f32,
    max: f32,
    value: f32,
}

impl GaugeDomain {
    /// Inclusive lower bound (finite, `<= max`).
    #[must_use]
    pub fn min(self) -> f32 {
        self.min
    }

    /// Inclusive upper bound (finite, `>= min`).
    #[must_use]
    pub fn max(self) -> f32 {
        self.max
    }

    /// Current value (finite, inside `[min, max]`).
    #[must_use]
    pub fn value(self) -> f32 {
        self.value
    }

    /// Compute the clamped sweep fraction with widened arithmetic.
    ///
    /// Widening prevents finite `f32` spans from overflowing during subtraction. Equal
    /// bounds or a non-finite quotient return zero.
    #[must_use]
    pub fn fraction_f64(self) -> f64 {
        let (value, min, max) = (
            f64::from(self.value),
            f64::from(self.min),
            f64::from(self.max),
        );
        #[allow(
            clippy::neg_cmp_op_on_partial_ord,
            reason = "NaN must take the fallback branch; `max <= min` is false for NaN"
        )]
        if !(max > min) {
            return 0.0;
        }
        let t = (value - min) / (max - min);
        if !t.is_finite() {
            return 0.0;
        }
        t.clamp(0.0, 1.0)
    }

    /// Narrow [`Self::fraction_f64`] to the finite `[0, 1]` canvas fraction.
    #[must_use]
    pub fn fraction(self) -> f32 {
        UnitInterval::clamped(self.fraction_f64() as f32).get()
    }
}

/// Normalize a caller's `(value, min, max)` triple into the single
/// [`GaugeDomain`] used for both drawing and the AT announcement.
///
/// Normalization policy:
///
/// | caller input | normalized domain |
/// |---|---|
/// | valid (`min <= max`, all finite, value in range) | unchanged |
/// | reversed bounds | bounds sorted (`new_sorted`) |
/// | equal bounds | honoured literally; `value` clamped onto the point |
/// | out-of-range finite `value` | clamped into `[min, max]` |
/// | non-finite `value` | `min` (the end the empty ring draws) |
/// | non-finite bound | [`FALLBACK_MIN`]`..=`[`FALLBACK_MAX`], `value = min` |
#[must_use]
pub fn gauge_domain(value: f32, min: f32, max: f32) -> GaugeDomain {
    let Some(range) = OrderedRangeF32::new_sorted(min, max) else {
        return GaugeDomain {
            min: FALLBACK_MIN,
            max: FALLBACK_MAX,
            value: FALLBACK_MIN,
        };
    };
    let value = if value.is_finite() {
        range.clamp(value)
    } else {
        range.lo()
    };
    GaugeDomain {
        min: range.lo(),
        max: range.hi(),
        value,
    }
}

/// The numeric claim a caller's formatted `value_text` makes, if it
/// makes one.
///
/// Formatted strings are PRESENTATION — yet screen readers speak them AS the
/// value, so a numbered string asserts about [`GaugeDomain`]'s quantity. This
/// type is that assertion, extracted for checking against normalized numerics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ValueTextClaim {
    /// The number the string leads with, under the interpretation that parsed.
    pub number: f32,
    /// How many fractional digits the string displayed. The reconciliation
    /// tolerance is half a unit in that last place, so `"72%"` legitimately
    /// stands for any fraction in `[0.715, 0.725]` while `"72.00%"` does not.
    pub decimals: u32,
    /// Whether the string carried a `%`, i.e. whether the number is a
    /// *percentage of the domain* rather than a value *in* the domain.
    pub percent: bool,
}

/// Parse finite interpretations of the first ASCII numeric run in formatted text.
///
/// Try verbatim, comma grouping, and comma decimal/period grouping forms. A `%`
/// anywhere in the string marks each interpretation as a percentage. Later runs
/// are ignored. If no interpretation parses as finite `f32`, return no claims;
/// [`value_text_reconciles`] then treats the text as opaque.
#[must_use]
pub fn value_text_claims(text: &str) -> Vec<ValueTextClaim> {
    let chars: Vec<char> = text.chars().collect();
    let Some(first_digit) = chars.iter().position(char::is_ascii_digit) else {
        return Vec::new();
    };
    let mut start = first_digit;
    while start > 0 && matches!(chars[start - 1], '0'..='9' | '.' | ',') {
        start -= 1;
    }
    if start > 0 && matches!(chars[start - 1], '-' | '+') {
        start -= 1;
    }
    let mut end = first_digit + 1;
    while end < chars.len() && matches!(chars[end], '0'..='9' | '.' | ',') {
        end += 1;
    }
    while end > start && !chars[end - 1].is_ascii_digit() {
        end -= 1;
    }
    let run: String = chars[start..end].iter().collect();
    let percent = text.contains('%');

    let mut out: Vec<ValueTextClaim> = Vec::new();
    let mut push = |candidate: String| {
        if let Ok(number) = candidate.parse::<f32>() {
            if !number.is_finite() {
                return;
            }
            let decimals = candidate
                .split_once('.')
                .map_or(0, |(_, frac)| frac.len() as u32);
            let claim = ValueTextClaim {
                number,
                decimals,
                percent,
            };
            if !out.contains(&claim) {
                out.push(claim);
            }
        }
    };
    push(run.clone());
    push(run.replace(',', ""));
    push(run.replace('.', "").replace(',', "."));
    out
}

/// Format the normalized domain value for accessibility fallback.
#[must_use]
pub fn derived_value_text(domain: GaugeDomain) -> String {
    format!("{}", domain.value())
}

/// Heuristically reconcile formatted text with a normalized domain.
///
/// No parsed claims means acceptance. A percent claim compares to sweep fraction
/// times 100; other claims compare to domain value or sweep fraction. Any parsed
/// interpretation may match, with half a unit in its last displayed decimal place
/// plus relative float tolerance. This is a formatting heuristic, not a full
/// localized-number parser or validation of every numeric statement in the text.
#[must_use]
pub fn value_text_reconciles(text: &str, domain: GaugeDomain) -> bool {
    let claims = value_text_claims(text);
    if claims.is_empty() {
        return true;
    }
    let fraction = domain.fraction();
    claims.iter().any(|claim| {
        let targets: [f32; 2] = if claim.percent {
            [fraction * 100.0, fraction * 100.0]
        } else {
            [domain.value(), fraction]
        };
        let place = 0.5 * 10f32.powi(-(claim.decimals as i32));
        targets.iter().any(|target| {
            let tol = place + 1e-6 * target.abs().max(1.0);
            (claim.number - target).abs() <= tol
        })
    })
}

/// Normalize a caller domain and return its canvas sweep fraction.
#[must_use]
pub fn gauge_fraction(value: f32, min: f32, max: f32) -> f32 {
    gauge_domain(value, min, max).fraction()
}

/// Return `(diameter - stroke) / 2`, clamped at zero.
///
/// The drawing uses [`is_compact_geometry`] to replace a zero-radius sampled ring
/// with a disc. This helper does not sanitize caller diameter or stroke inputs.
#[must_use]
pub fn ring_radius_for(diameter: f32, stroke: f32) -> f32 {
    ((diameter - stroke) / 2.0).max(0.0)
}

/// Select compact-disc geometry when [`ring_radius_for`] is non-positive.
#[must_use]
pub fn is_compact_geometry(diameter: f32, stroke: f32) -> bool {
    ring_radius_for(diameter, stroke) <= 0.0
}

fn sane_diameter(v: f32) -> f32 {
    FinitePositive::new_or(v, DEFAULT_DIAMETER).get()
}

fn sane_stroke(v: f32, diameter: f32) -> f32 {
    FinitePositive::new_or(v, DEFAULT_TRACK_WIDTH)
        .get()
        .min(diameter)
}

/// Sweep start angle: 12 o'clock. iced's y axis points down, so `-PI/2` is up
/// and *increasing* angle sweeps clockwise on screen.
pub const START_ANGLE: f32 = -std::f32::consts::FRAC_PI_2;

/// Arc end angle for a `[0, 1]` fraction, sweeping clockwise from 12 o'clock.
#[must_use]
pub fn arc_end_angle(fraction: f32) -> f32 {
    START_ANGLE + UnitInterval::clamped(fraction).get() * std::f32::consts::TAU
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct GaugeKey {
    fraction: f32,
    stroke: f32,
    style: GaugeStyle,
    fill: Color,
    track: Color,
    unsampled: bool,
    unsampled_color: Color,
    reduced_motion: bool,
}

/// Per-instance geometry cache kept by the runtime between frames.
#[derive(Debug, Default)]
pub struct GaugeState {
    cache: Cache,
    last: Cell<Option<GaugeKey>>,
    /// The painted fraction, sweeping to a new value.
    sweep: crate::motion::StateFade<f32>,
}

#[derive(Debug, Clone, Copy)]
struct GaugeProgram {
    fraction: f32,
    stroke: f32,
    style: GaugeStyle,
    unsampled: bool,
    tone: GaugeTone,
    pinned_tokens: Option<GaugeTokens>,
    fill_override: Option<Color>,
    reduced_motion: Option<bool>,
}

impl GaugeProgram {
    fn colors(&self, theme: &iced::Theme) -> (Color, Color) {
        let tokens = self.tokens(theme);
        let fill = self
            .fill_override
            .unwrap_or_else(|| tokens.color_for(self.tone));
        (fill, tokens.track_color)
    }

    fn tokens(&self, theme: &iced::Theme) -> GaugeTokens {
        crate::theme_lookup::resolve_pinned(
            crate::theme_lookup::PIN_SITE_GAUGE,
            self.pinned_tokens,
            theme,
            GaugeTokens::from_palette,
        )
    }

    fn motion_reduced(&self, theme: &iced::Theme) -> bool {
        self.reduced_motion
            .unwrap_or_else(|| crate::theme_lookup::reduced_motion(theme))
    }
}

impl<Message> Program<Message> for GaugeProgram {
    type State = GaugeState;

    fn frame_interest(&self, state: &Self::State) -> bool {
        state.sweep.is_moving() || state.sweep.needs_frame(&self.fraction)
    }

    fn update(
        &self,
        state: &mut Self::State,
        event: &iced::Event,
        _bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let iced::Event::Window(iced::window::Event::RedrawRequested(now)) = event else {
            return None;
        };
        let fraction = self.fraction;
        state
            .sweep
            .frame_over(
                *now,
                self.reduced_motion,
                self.unsampled,
                enzui_theme::MotionToken::Normal,
                |_: &iced::Theme| fraction,
            )
            .then(canvas::Action::request_redraw)
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let (fill_color, track_color) = self.colors(theme);
        let key = GaugeKey {
            fraction: state.sweep.paint(theme, self.fraction),
            stroke: self.stroke,
            style: self.style,
            fill: fill_color,
            track: track_color,
            unsampled: self.unsampled,
            unsampled_color: self.tokens(theme).unsampled_color,
            reduced_motion: self.motion_reduced(theme),
        };
        // Canvas handles bounds changes; retained key changes must clear its geometry.
        if state.last.get() != Some(key) {
            state.cache.clear();
            state.last.set(Some(key));
        }

        let geometry = state.cache.draw(renderer, bounds.size(), |frame| {
            draw_ring(frame, bounds.width, bounds.height, key);
        });
        vec![geometry]
    }
}

fn draw_ring(frame: &mut Frame, width: f32, height: f32, key: GaugeKey) {
    // Count cache misses even when empty bounds produce no visible paths.
    RING_TESSELLATIONS.fetch_add(1, Ordering::Relaxed);
    let extent = width.min(height).max(0.0);
    let stroke_w = key.stroke.min(extent);
    if extent <= 0.0 {
        return;
    }
    let center = Point::new(width / 2.0, height / 2.0);
    // Keep placeholders separate from compact discs that read visually as measured values.
    if key.unsampled {
        draw_unsampled_placeholder(frame, center, extent, key);
        return;
    }
    if is_compact_geometry(extent, stroke_w) {
        draw_compact_disc(frame, center, extent, key);
        return;
    }
    let radius = ring_radius_for(extent, stroke_w);
    if stroke_w <= 0.0 {
        return;
    }

    let base = Stroke::default()
        .with_width(stroke_w)
        .with_line_cap(LineCap::Round);

    let arc = |start: f32, end: f32| {
        Path::new(|b| {
            b.arc(canvas::path::Arc {
                center,
                radius,
                start_angle: Radians(start),
                end_angle: Radians(end),
            });
        })
    };
    let (track, start, end) = match key.style {
        GaugeStyle::Ring => (
            Path::circle(center, radius),
            START_ANGLE,
            arc_end_angle(key.fraction),
        ),
        GaugeStyle::Arc => (
            arc(ARC_START_ANGLE, ARC_START_ANGLE + ARC_SWEEP),
            ARC_START_ANGLE,
            ARC_START_ANGLE + UnitInterval::clamped(key.fraction).get() * ARC_SWEEP,
        ),
    };
    frame.stroke(&track, base.with_color(key.track));

    if key.fraction > 0.0 {
        frame.stroke(&arc(start, end), base.with_color(key.fill));
    }
}

fn draw_unsampled_placeholder(frame: &mut Frame, center: Point, extent: f32, key: GaugeKey) {
    let stroke_w = unsampled_stroke(extent, key.stroke);
    let radius = ring_radius_for(extent, stroke_w);
    if radius <= 0.0 || stroke_w <= 0.0 {
        return;
    }
    let dash = unsampled_dash(stroke_w);
    let ring = Path::circle(center, radius);
    frame.stroke(
        &ring,
        Stroke {
            style: canvas::Style::Solid(key.unsampled_color),
            width: stroke_w,
            // Round caps can close the gaps of thick placeholder dashes.
            line_cap: LineCap::Butt,
            line_join: canvas::LineJoin::default(),
            line_dash: canvas::LineDash {
                segments: &dash,
                offset: 0,
            },
        },
    );
}

fn draw_compact_disc(frame: &mut Frame, center: Point, extent: f32, key: GaugeKey) {
    let radius = extent / 2.0;
    if radius <= 0.0 {
        return;
    }
    frame.fill(&Path::circle(center, radius), key.track);
    let fraction = UnitInterval::clamped(key.fraction).get();
    if fraction > 0.0 {
        let wedge = crate::pie_chart::slice_path(center, radius, 0.0, 0.0, fraction);
        frame.fill(&wedge, key.fill);
    }
}

/// A circular, token-driven gauge.
#[derive(Debug, Clone)]
#[must_use]
pub struct EnzGauge {
    sample: Sampled<f32>,
    min: f32,
    max: f32,
    diameter: f32,
    track_width: Option<f32>,
    style: GaugeStyle,
    tone: GaugeTone,
    label: Option<String>,
    value_text: Option<String>,
    tokens: Option<GaugeTokens>,
    fill_override: Option<Color>,
    reduced_motion: Option<bool>,
}

impl EnzGauge {
    /// Set caller bounds, normalized by [`gauge_domain`] when consumed.
    pub fn range(mut self, min: f32, max: f32) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    /// Set a measured domain value, including measured zero.
    ///
    /// Use [`Self::sampled`] to represent no measurement.
    pub fn value(mut self, value: f32) -> Self {
        self.sample = Sampled::Sampled(value);
        self
    }

    /// Set measured or unsampled state.
    ///
    /// Unsampled gauges use a dashed placeholder and publish no numeric value.
    pub fn sampled(mut self, sample: Sampled<f32>) -> Self {
        self.sample = sample;
        self
    }

    /// Diameter in logical pixels; defaults to 80 for non-finite or non-positive input.
    pub fn diameter(mut self, d: f32) -> Self {
        self.diameter = sane_diameter(d);
        self
    }

    /// Stroke width in logical pixels, normalized during element construction.
    /// Unset, it follows the diameter ([`default_track_width`]).
    ///
    /// Non-finite or non-positive values use eight pixels; widths cap at the diameter.
    pub fn track_width(mut self, w: f32) -> Self {
        self.track_width = Some(w);
        self
    }

    /// The shape: a full ring (default) or an open dial arc.
    pub fn style(mut self, style: GaugeStyle) -> Self {
        self.style = style;
        self
    }

    /// Semantic fill tone (default `Accent`).
    pub fn tone(mut self, tone: GaugeTone) -> Self {
        self.tone = tone;
        self
    }

    /// Caption under the ring, and the accessible name.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Override desired center text and reconciled accessibility value text.
    ///
    /// Sampled gauges preserve visual text even when its parsed numeric claim is
    /// rejected for announcement. Empty text suppresses formatted output. Unsampled
    /// gauges replace nonempty overrides with an em dash and “not measured yet.”
    /// Center text is omitted without a positive hole-width budget and may be elided.
    pub fn value_text(mut self, text: impl Into<String>) -> Self {
        self.value_text = Some(text.into());
        self
    }

    /// Override the resolved fill colour directly.
    pub fn fill(mut self, color: Color) -> Self {
        self.fill_override = Some(color);
        self
    }

    /// Pin the colour tokens (skips runtime-theme resolution).
    pub fn tokens(mut self, tokens: GaugeTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Override runtime-theme motion policy for this static gauge.
    ///
    /// The resolved flag reaches draw/cache state but does not animate geometry.
    pub fn reduced_motion(mut self, reduced: bool) -> Self {
        self.reduced_motion = Some(reduced);
        self
    }

    /// Resolve an explicit motion override or the supplied theme’s policy.
    #[must_use]
    pub fn resolved_reduced_motion(&self, theme: &iced::Theme) -> bool {
        self.reduced_motion
            .unwrap_or_else(|| crate::theme_lookup::reduced_motion(theme))
    }

    /// The stored reading, including whether a measurement exists.
    #[must_use]
    pub fn sample(&self) -> Sampled<f32> {
        self.sample
    }

    /// Has this gauge no measurement yet?
    #[must_use]
    pub fn is_unsampled(&self) -> bool {
        self.sample.is_unsampled()
    }

    /// Normalize the stored bounds and reading.
    ///
    /// For unsampled gauges, substitute the raw lower-bound field before normalizing;
    /// this is a synthetic value and can become the upper end after reversed-bound
    /// repair. Use [`Self::sampled_domain`] when absence must remain distinguishable.
    #[must_use]
    pub fn domain(&self) -> GaugeDomain {
        gauge_domain(self.sample.unwrap_or(self.min), self.min, self.max)
    }

    /// Normalize a measured reading while preserving [`Sampled::Unsampled`].
    #[must_use]
    pub fn sampled_domain(&self) -> Sampled<GaugeDomain> {
        let (min, max) = (self.min, self.max);
        self.sample.map(|v| gauge_domain(v, min, max))
    }

    /// Return the fraction of [`Self::domain`], including its synthetic unsampled value.
    ///
    /// Unsampled drawing ignores this fraction. It is not a measurement or a reliable
    /// zero indicator for that state; inspect [`Self::sampled_domain`] instead.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        self.domain().fraction()
    }

    /// Resolve the accessibility value text.
    ///
    /// Measured values derive domain text when unset or when parsed numeric claims
    /// contradict the domain; accepted overrides remain unchanged. Empty overrides
    /// return `None`. Unsampled values return [`UNSAMPLED_ANNOUNCEMENT`] unless empty
    /// text was explicitly requested. This also supplies the name when no label exists.
    #[must_use]
    pub fn announced_value_text(&self) -> Option<String> {
        let domain = self.domain();
        if self.sample.is_unsampled() {
            return match self.value_text.as_deref() {
                Some("") => None,
                _ => Some(UNSAMPLED_ANNOUNCEMENT.to_owned()),
            };
        }
        match self.value_text.as_deref() {
            None => Some(derived_value_text(domain)),
            Some("") => None,
            Some(text) if value_text_reconciles(text, domain) => Some(text.to_owned()),
            Some(_) => Some(derived_value_text(domain)),
        }
    }

    /// Resolve desired center text before layout budgeting.
    ///
    /// Measured values preserve overrides or derive percentages for `0..=1` and
    /// domain values otherwise. Unsampled values use [`UNSAMPLED_VALUE_TEXT`]. Empty
    /// overrides remain empty in either state. Rendering may omit or elide this text.
    #[must_use]
    pub fn painted_value_text(&self) -> Option<String> {
        if self.sample.is_unsampled() {
            return match self.value_text.as_deref() {
                Some("") => Some(String::new()),
                _ => Some(UNSAMPLED_VALUE_TEXT.to_owned()),
            };
        }
        self.value_text.clone().or_else(|| {
            Some(
                if self.domain().min() == 0.0 && self.domain().max() == 1.0 {
                    format!("{:.0}%", self.domain().fraction() * 100.0)
                } else {
                    derived_value_text(self.domain())
                },
            )
        })
    }

    /// Build progress-indicator semantics with reconciled value text.
    ///
    /// Measured values publish the normalized numeric triple in caller units.
    /// Unsampled values omit numeric data; an empty text override also omits formatted
    /// value text. An explicit label takes precedence over the announced value as name.
    pub fn semantics(&self) -> Semantics {
        let announced = self.announced_value_text();
        let a11y_label = self.label.clone().or_else(|| announced.clone());
        let base = Semantics::new(SemRole::ProgressIndicator)
            .label_opt(a11y_label)
            .value_opt(announced);
        match self.sampled_domain() {
            Sampled::Unsampled => base,
            Sampled::Sampled(domain) => base.numeric(NumericValue {
                value: f64::from(domain.value()),
                min: f64::from(domain.min()),
                max: f64::from(domain.max()),
                step: None,
            }),
        }
    }

    /// Convert into an iced [`Element`].
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        let diameter = sane_diameter(self.diameter);
        let stroke = sane_stroke(
            self.track_width
                .unwrap_or_else(|| default_track_width(diameter)),
            diameter,
        );
        let domain = self.domain();
        let fraction = domain.fraction();
        let unsampled = self.sample.is_unsampled();
        let semantics = self.semantics();
        let pinned = self.tokens;

        let program = GaugeProgram {
            fraction,
            stroke,
            style: self.style,
            unsampled,
            tone: self.tone,
            pinned_tokens: pinned,
            fill_override: self.fill_override,
            reduced_motion: self.reduced_motion,
        };
        let ring: Element<'a, Message> = canvas::Canvas::new(program)
            .width(Length::Fixed(diameter))
            .height(Length::Fixed(diameter))
            .into();

        // Center text must fit inside the requested ring hole.
        let value_width = (diameter - 2.0 * stroke - 8.0).max(0.0);
        let ring = match self.painted_value_text() {
            Some(vt) if !vt.is_empty() && value_width > 0.0 => {
                let ink = move |theme: &iced::Theme| {
                    let t = pinned.unwrap_or_else(|| {
                        crate::theme_lookup::resolve(theme, GaugeTokens::from_palette)
                    });
                    iced::widget::text::Style {
                        color: Some(if unsampled {
                            t.unsampled_color
                        } else {
                            t.text_color
                        }),
                    }
                };
                let centered = container(
                    iced::widget::text(vt)
                        .size(value_text_size_for(diameter))
                        .font(
                            enzui_theme::text_defaults::font(
                                enzui_theme::FontRole::Ui,
                                enzui_theme::WeightRung::Bold,
                            )
                            .tabular(),
                        )
                        .width(Length::Fixed(value_width))
                        .align_x(iced::alignment::Horizontal::Center)
                        .wrapping(iced::widget::text::Wrapping::None)
                        .ellipsis(iced::widget::text::Ellipsis::End)
                        .style(ink),
                )
                .width(Length::Fixed(diameter))
                .height(Length::Fixed(diameter))
                .center_x(Length::Fixed(diameter))
                .center_y(Length::Fixed(diameter));
                iced::widget::stack![ring, centered].into()
            }
            _ => ring,
        };

        let body: Element<'a, Message> = match self.label {
            Some(l) if !l.is_empty() => {
                let ink = move |theme: &iced::Theme| {
                    let t = pinned.unwrap_or_else(|| {
                        crate::theme_lookup::resolve(theme, GaugeTokens::from_palette)
                    });
                    iced::widget::text::Style {
                        color: Some(t.muted_color),
                    }
                };
                column![
                    ring,
                    container(iced::widget::text(l).size(label_size()).style(ink))
                        .width(Length::Fixed(diameter))
                        .center_x(Length::Fixed(diameter)),
                ]
                .spacing(label_spacing())
                .width(Length::Fixed(diameter))
                .into()
            }
            _ => ring,
        };

        crate::semantics_container::SemanticsContainer::leaf(body, semantics).into()
    }
}

impl<'a, Message: 'a> From<EnzGauge> for Element<'a, Message> {
    fn from(g: EnzGauge) -> Self {
        g.into_element()
    }
}

/// Construct a measured gauge with default `0..=1` bounds.
///
/// Zero is a real reading; use [`enz_gauge_sampled`] for an absent measurement.
pub fn enz_gauge(value: f32) -> EnzGauge {
    enz_gauge_sampled(Sampled::Sampled(value))
}

/// Construct a measured or unsampled gauge with default `0..=1` bounds.
///
/// `Option<f32>` converts into [`Sampled`]. No measurement selects the dashed
/// placeholder and numeric-free semantics; zero remains a measured value.
pub fn enz_gauge_sampled(sample: Sampled<f32>) -> EnzGauge {
    EnzGauge {
        sample,
        min: 0.0,
        max: 1.0,
        diameter: DEFAULT_DIAMETER,
        track_width: None,
        style: GaugeStyle::default(),
        tone: GaugeTone::default(),
        label: None,
        value_text: None,
        tokens: None,
        fill_override: None,
        reduced_motion: None,
    }
}
