//! A compact chart with no frame or axis gutters: bars, or a smooth area.
//!
//! Bars divide the available width and grow from the bottom edge. Zero-height
//! buckets are omitted, so painted ink need not reach both horizontal edges.
//! [`SparkStyle::Area`] draws a smooth line through the readings across the
//! whole width over a fill fading toward the bottom.
//! The accessibility leaf carries a current value and optional trend; empty
//! data announces [`SPARKLINE_EMPTY_ANNOUNCEMENT`] without a numeric reading.
//! Changed data animates the newest sample from the preceding reading through
//! the shared chart-motion scheduler. Dense series use peak-preserving buckets.

use enzui_core::semantics::{NumericValue, Role as SemRole, Semantics};
use enzui_theme::Palette;
use iced::widget::canvas::{self, Cache, Frame, Path, Program};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size};
use std::cell::Cell;

const DEFAULT_WIDTH: f32 = 72.0;
const DEFAULT_HEIGHT: f32 = 24.0;
const MIN_BUCKET_PX: f32 = 2.0;
const DEFAULT_GAP: f32 = 1.0;

/// How a sparkline draws its readings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SparkStyle {
    /// A bar per reading, growing from the bottom (the default).
    #[default]
    Bars,
    /// A smooth line through the readings over a fill fading toward the
    /// bottom, the way a resource history reads at a glance.
    Area,
}

/// Spoken value for a series with no samples.
pub const SPARKLINE_EMPTY_ANNOUNCEMENT: &str = "no data";
/// Direction of the latest reading relative to the preceding reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SparkTrend {
    /// Latest reading exceeds the preceding reading by more than `1e-6`.
    Rising,
    /// Latest reading is below the preceding reading by more than `1e-6`.
    Falling,
    /// Neither directional comparison succeeds.
    Steady,
}

impl SparkTrend {
    /// Spoken word appended to the current value.
    #[must_use]
    pub fn word(self) -> String {
        match self {
            SparkTrend::Rising => enzui_core::localization::text("enzui-sparkline-rising"),
            SparkTrend::Falling => enzui_core::localization::text("enzui-sparkline-falling"),
            SparkTrend::Steady => enzui_core::localization::text("enzui-sparkline-steady"),
        }
    }

    /// Compare the last two readings with an absolute tolerance of `1e-6`.
    /// Returns `None` below two readings. Inputs are not sanitised; a NaN in either
    /// position yields [`SparkTrend::Steady`].
    #[must_use]
    pub fn from_samples(samples: &[f32]) -> Option<SparkTrend> {
        let n = samples.len();
        if n < 2 {
            return None;
        }
        let prev = samples[n - 2];
        let last = samples[n - 1];
        let eps = 1e-6_f32;
        Some(if last > prev + eps {
            SparkTrend::Rising
        } else if last < prev - eps {
            SparkTrend::Falling
        } else {
            SparkTrend::Steady
        })
    }
}

/// Aggregate readings into contiguous buckets containing each range's finite maximum.
/// The cap is at least one, including when `max_buckets` is zero. Series that fit
/// are copied verbatim, including non-finite values. Aggregated ranges with no
/// finite reading produce zero.
#[must_use]
pub fn fit_buckets(samples: &[f32], max_buckets: usize) -> Vec<f32> {
    let max_buckets = max_buckets.max(1);
    if samples.len() <= max_buckets {
        return samples.to_vec();
    }
    let n = samples.len();
    let b = max_buckets;
    let mut out = Vec::with_capacity(b);
    for i in 0..b {
        let start = i * n / b;
        let end = (((i + 1) * n / b).max(start + 1)).min(n);
        let mut peak = f32::NEG_INFINITY;
        for &v in &samples[start..end] {
            if v.is_finite() && v > peak {
                peak = v;
            }
        }
        out.push(if peak.is_finite() { peak } else { 0.0 });
    }
    out
}
#[must_use]
fn max_buckets_for_width(width: f32) -> usize {
    if !width.is_finite() || width <= 0.0 {
        return 1;
    }
    ((width / MIN_BUCKET_PX).floor() as usize).max(1)
}

/// Analytic bounds of fill calls submitted by [`draw_static`].
/// These bounds do not establish visible pixels: transparent or zero-width fills
/// can be counted, and zero-height buckets are omitted.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SparkDrawReport {
    /// Number of submitted fills.
    pub bars_drawn: usize,
    /// Left edge of submitted fills, or zero when none are submitted.
    pub ink_left: f32,
    /// Right edge of submitted fills, or zero when none are submitted.
    pub ink_right: f32,
    /// Bottom of the non-negative plot height.
    pub baseline_y: f32,
    /// Smallest submitted top coordinate, or the baseline when none are submitted.
    pub min_top: f32,
    /// Largest submitted bottom coordinate, or zero when none are submitted.
    pub ink_bottom: f32,
}

/// Submit bottom-anchored bars for already fitted buckets and return their analytic bounds.
/// A finite, increasing value domain maps its minimum to zero height and maximum to full height.
/// Non-finite readings use `dmin`; non-positive heights are skipped. The final
/// bucket reaches the right edge when submitted. Interior gaps clamp to the
/// bucket width, and no axis gutter is reserved.
pub fn draw_static(
    frame: &mut Frame,
    plot: Size,
    buckets: &[f32],
    dmin: f32,
    dmax: f32,
    gap: f32,
    color: Color,
) -> SparkDrawReport {
    let w = plot.width.max(0.0);
    let h = plot.height.max(0.0);
    let n = buckets.len();
    let empty = SparkDrawReport {
        bars_drawn: 0,
        ink_left: 0.0,
        ink_right: 0.0,
        baseline_y: h,
        min_top: h,
        ink_bottom: 0.0,
    };
    if n == 0 || w <= 0.0 || h <= 0.0 {
        return empty;
    }
    // Finite f32 endpoints can span more than f32::MAX.
    let dmin_f = f64::from(dmin);
    let span = (f64::from(dmax) - dmin_f).max(f64::from(f32::EPSILON));
    let bw = w / n as f32;
    let gap = gap.max(0.0).min(bw);

    let mut drawn = 0usize;
    let mut ink_left = f32::INFINITY;
    let mut ink_right = 0.0_f32;
    let mut min_top = h;
    let mut ink_bottom = 0.0_f32;
    for (i, &v) in buckets.iter().enumerate() {
        let value = if v.is_finite() { v } else { dmin };
        let frac = ((f64::from(value) - dmin_f) / span).clamp(0.0, 1.0) as f32;
        let bar_h = frac * h;
        if bar_h <= 0.0 {
            continue;
        }
        let x0 = i as f32 * bw;
        let x1 = if i == n - 1 {
            w
        } else {
            (x0 + bw - gap).max(x0)
        };
        let top = h - bar_h;
        frame.fill(
            &Path::rectangle(Point::new(x0, top), Size::new((x1 - x0).max(0.0), bar_h)),
            color,
        );
        drawn += 1;
        ink_left = ink_left.min(x0);
        ink_right = ink_right.max(x1);
        min_top = min_top.min(top);
        ink_bottom = ink_bottom.max(top + bar_h);
    }
    SparkDrawReport {
        bars_drawn: drawn,
        ink_left: if drawn > 0 { ink_left } else { 0.0 },
        ink_right,
        baseline_y: h,
        min_top,
        ink_bottom,
    }
}

/// Draws `buckets` as a smooth line over a fading fill across the whole of
/// `plot`, mapped like [`draw_static`]'s bars.
pub fn draw_area(
    frame: &mut Frame,
    plot: Size,
    buckets: &[f32],
    dmin: f32,
    dmax: f32,
    color: Color,
) {
    let (w, h) = (plot.width.max(0.0), plot.height.max(0.0));
    if buckets.is_empty() || w <= 0.0 || h <= 0.0 {
        return;
    }
    let span = (f64::from(dmax) - f64::from(dmin)).max(f64::from(f32::EPSILON));
    // Half the stroke stays inside at the top and bottom.
    let stroke = 1.5_f32;
    let inset = stroke / 2.0;
    let last = (buckets.len() - 1).max(1) as f64;
    let points: Vec<(f64, f64)> = buckets
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let value = if v.is_finite() { *v } else { dmin };
            let frac = ((f64::from(value) - f64::from(dmin)) / span).clamp(0.0, 1.0);
            (
                i as f64 / last * f64::from(w),
                f64::from(inset) + (1.0 - frac) * f64::from(h - stroke),
            )
        })
        .collect();
    let points = if points.len() == 1 {
        vec![(0.0, points[0].1), (f64::from(w), points[0].1)]
    } else {
        enzui_core::chart::smooth_spline(&points, 6)
    };
    let edge: Vec<Point> = points
        .iter()
        .map(|(x, y)| Point::new(*x as f32, *y as f32))
        .collect();
    let fill = Path::new(|b| {
        b.move_to(Point::new(edge[0].x, h));
        for p in &edge {
            b.line_to(*p);
        }
        b.line_to(Point::new(edge[edge.len() - 1].x, h));
        b.close();
    });
    frame.fill(
        &fill,
        crate::cartesian_chart::area_fill(Color { a: 0.28, ..color }, &edge, h),
    );
    let line = Path::new(|b| {
        b.move_to(edge[0]);
        for p in &edge[1..] {
            b.line_to(*p);
        }
    });
    frame.stroke(
        &line,
        canvas::Stroke::default()
            .with_color(color)
            .with_width(stroke)
            .with_line_cap(canvas::LineCap::Round)
            .with_line_join(canvas::LineJoin::Round),
    );
}

/// Per-instance geometry cache with a rebuild counter.
#[derive(Default)]
pub struct SparkState {
    cache: Cache,
    last: Cell<Option<u64>>,
    builds: Cell<u32>,
}

impl SparkState {
    /// Number of geometry rebuilds, excluding cached redraws.
    #[must_use]
    pub fn builds(&self) -> u32 {
        self.builds.get()
    }
}

/// Canvas program with cached bar geometry and newest-sample interpolation.
pub struct SparkProgram {
    samples: Vec<f32>,
    dmin: f32,
    dmax: f32,
    gap: f32,
    color: Color,
    style: SparkStyle,
    transition_phase: crate::chart_motion::ChartMotionPhase,
}

impl SparkProgram {
    /// Set the shared draw-time transition phase for deterministic rendering probes.
    #[doc(hidden)]
    pub fn set_transition_phase_for_test(&self, phase: f32) {
        self.transition_phase.set(phase);
    }
}

impl<Message> Program<Message> for SparkProgram {
    type State = SparkState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let dmin = self.dmin;
        let dmax = self.dmax;
        let gap = self.gap;
        let color = self.color;
        let phase = self.transition_phase.get().clamp(0.0, 1.0);
        let mut animated_samples;
        let samples = if phase < 1.0 && self.samples.len() >= 2 {
            animated_samples = self.samples.clone();
            let n = animated_samples.len();
            let from = animated_samples[n - 2];
            let to = animated_samples[n - 1];
            animated_samples[n - 1] = from + (to - from) * phase;
            animated_samples.as_slice()
        } else {
            self.samples.as_slice()
        };
        let style = self.style;
        let painted_key = spark_key(samples, dmin, dmax, gap, color, style);
        if state.last.get() != Some(painted_key) {
            state.cache.clear();
            state.last.set(Some(painted_key));
        }
        let geometry = state.cache.draw(renderer, bounds.size(), |frame| {
            state.builds.set(state.builds.get() + 1);
            let max_buckets = max_buckets_for_width(bounds.width);
            let buckets = fit_buckets(samples, max_buckets);
            match style {
                SparkStyle::Bars => {
                    let _ = draw_static(frame, bounds.size(), &buckets, dmin, dmax, gap, color);
                }
                SparkStyle::Area => draw_area(frame, bounds.size(), &buckets, dmin, dmax, color),
            }
        });
        vec![geometry]
    }
}
fn spark_key(
    samples: &[f32],
    dmin: f32,
    dmax: f32,
    gap: f32,
    color: Color,
    style: SparkStyle,
) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    let mut mix = |bytes: [u8; 4]| {
        for b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    mix((samples.len() as u32).to_le_bytes());
    for v in samples {
        mix(v.to_bits().to_le_bytes());
    }
    for f in [dmin, dmax, gap, color.r, color.g, color.b, color.a] {
        mix(f.to_bits().to_le_bytes());
    }
    mix(u32::from(style == SparkStyle::Area).to_le_bytes());
    h
}

/// Builder for a compact bar chart with a single accessibility leaf.
pub struct EnzSparkline {
    samples: Vec<f32>,
    label: Option<String>,
    value_text: Option<String>,
    range: Option<(f32, f32)>,
    color: Option<Color>,
    announce_trend: bool,
    width: Length,
    height: Length,
    gap: f32,
    style: SparkStyle,
}

/// Construct a chart from time-ordered readings, replacing non-finite values with zero.
/// Defaults to a fixed 72 × 24 logical-pixel slot, a 1 px interior gap, and spoken
/// trend announcements. Empty data submits no bars and announces no data.
pub fn enz_sparkline(samples: impl Into<Vec<f32>>) -> EnzSparkline {
    EnzSparkline {
        samples: samples
            .into()
            .into_iter()
            .map(|v| if v.is_finite() { v } else { 0.0 })
            .collect(),
        label: None,
        value_text: None,
        range: None,
        color: None,
        announce_trend: true,
        width: Length::Fixed(DEFAULT_WIDTH),
        height: Length::Fixed(DEFAULT_HEIGHT),
        gap: DEFAULT_GAP,
        style: SparkStyle::Bars,
    }
}

impl EnzSparkline {
    /// Draw the readings as bars (the default) or as a smooth area.
    #[must_use]
    pub fn style(mut self, style: SparkStyle) -> Self {
        self.style = style;
        self
    }

    /// Set the accessible name.
    /// Without an override the spoken value supplies the name; empty overrides are accepted.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Override the spoken current value.
    /// Otherwise near-integral readings use an `i64` integer (saturating outside its
    /// range), and other readings use one decimal place.
    /// Trend words are appended separately; empty series always announce no data.
    #[must_use]
    pub fn value_text(mut self, value: impl Into<String>) -> Self {
        self.value_text = Some(value.into());
        self
    }

    /// Set the value domain used for bars and numeric accessibility.
    /// Finite endpoints are sorted; non-finite endpoints fall back to the data domain.
    /// The derived minimum includes zero and negative readings, and the maximum is
    /// the data peak, expanded above the minimum by `f32::EPSILON` where representable.
    /// Empty data uses `0..1`.
    #[must_use]
    pub fn range(mut self, min: f32, max: f32) -> Self {
        self.range = Some((min, max));
        self
    }

    /// Set bar colour; the default is [`Palette::dark`]'s accent, independent of runtime theme.
    #[must_use]
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Append the trend word to the spoken value; enabled by default.
    #[must_use]
    pub fn announce_trend(mut self, on: bool) -> Self {
        self.announce_trend = on;
        self
    }

    /// Set slot width; the default is 72 fixed logical pixels.
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Set slot height; the default is 24 fixed logical pixels.
    #[must_use]
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }
    fn domain(&self) -> (f32, f32) {
        if let Some((a, b)) = self.range
            && let Some(r) = enzui_core::metrics::OrderedRangeF32::new_sorted(a, b)
        {
            return (r.lo(), r.hi().max(r.lo() + f32::EPSILON));
        }
        let mut dmax = f32::NEG_INFINITY;
        let mut dmin = 0.0_f32;
        for &v in &self.samples {
            if v.is_finite() {
                dmax = dmax.max(v);
                dmin = dmin.min(v);
            }
        }
        if !dmax.is_finite() {
            dmax = 1.0;
        }
        (dmin, dmax.max(dmin + f32::EPSILON))
    }

    /// Direction of the last two sanitised readings, or `None` below two readings.
    #[must_use]
    pub fn trend(&self) -> Option<SparkTrend> {
        SparkTrend::from_samples(&self.samples)
    }
    fn announced_value(&self) -> Option<String> {
        let Some(&latest) = self.samples.last() else {
            return Some(SPARKLINE_EMPTY_ANNOUNCEMENT.to_owned());
        };
        let base = self
            .value_text
            .clone()
            .unwrap_or_else(|| format_reading(latest));
        match self.trend() {
            Some(t) if self.announce_trend => Some(
                enzui_core::localized!("enzui-sparkline-trend-description", "base" => base, "trend" => t.word()),
            ),
            _ => Some(base),
        }
    }
    fn numeric(&self) -> Option<NumericValue> {
        let latest = *self.samples.last()?;
        let (dmin, dmax) = self.domain();
        Some(NumericValue {
            value: f64::from(latest),
            min: f64::from(dmin),
            max: f64::from(dmax),
            step: None,
        })
    }

    /// Build image semantics with the name, spoken value and latest numeric reading.
    /// Empty series have no numeric value. Without an explicit name the spoken value
    /// also supplies the name.
    pub fn semantics(&self) -> Semantics {
        let announced = self.announced_value();
        let label = self.label.clone().or_else(|| announced.clone());
        let mut s = Semantics::new(SemRole::Image)
            .label_opt(label)
            .value_opt(announced);
        if let Some(n) = self.numeric() {
            s = s.numeric(n);
        }
        s
    }

    /// Build a canvas program with the current samples, domain, colour and settled motion phase.
    /// Standalone programs retain that phase until explicitly changed; [`Self::into_element`]
    /// wraps a program in the chart-motion scheduler.
    #[must_use]
    pub fn program(&self) -> SparkProgram {
        self.program_with_phase(crate::chart_motion::phase())
    }

    fn program_with_phase(
        &self,
        transition_phase: crate::chart_motion::ChartMotionPhase,
    ) -> SparkProgram {
        let (dmin, dmax) = self.domain();
        SparkProgram {
            samples: self.samples.clone(),
            dmin,
            dmax,
            gap: self.gap,
            color: self.color.unwrap_or_else(|| Palette::dark().accent),
            style: self.style,
            transition_phase,
        }
    }

    /// Build the canvas with chart-motion scheduling and a terminal accessibility leaf.
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        let semantics = self.semantics();
        let transition_phase = crate::chart_motion::phase();
        let program = self.program_with_phase(std::rc::Rc::clone(&transition_phase));
        let key = spark_key(
            &program.samples,
            program.dmin,
            program.dmax,
            program.gap,
            program.color,
            program.style,
        );
        let canvas: Element<'a, Message> = canvas::Canvas::new(program)
            .width(self.width)
            .height(self.height)
            .into();
        let canvas: Element<'a, Message> =
            crate::chart_motion::ChartMotion::new(canvas, key, transition_phase).into();
        // Bars are decorative; traversal stops at the chart's single accessibility node.
        crate::semantics_container::SemanticsContainer::leaf(canvas, semantics).into()
    }
}

impl<'a, Message: 'a> From<EnzSparkline> for Element<'a, Message> {
    fn from(spark: EnzSparkline) -> Self {
        spark.into_element()
    }
}
fn format_reading(v: f32) -> String {
    if (v - v.round()).abs() < 1e-4 {
        format!("{}", v.round() as i64)
    } else {
        format!("{v:.1}")
    }
}
