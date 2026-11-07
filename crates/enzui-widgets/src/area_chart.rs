//! `EnzAreaChart`: a Canvas-based Cartesian area chart host.
//!
//! # What this is
//!
//! A sibling of [`crate::line_chart`] on the same [`enzui_core::chart`]
//! core (same [`Series`], decimation, [`crate::chart_geometry::plot_geometry`]),
//! differing in exactly one place: surviving points join into a polyline then
//! close down to a baseline, filling line-to-baseline. Shared scales,
//! gutters, grid, axes and ticks put an area's edge and a line's stroke over
//! the same datum on the same pixel.
//!
//! # The baseline is `0` clamped into the y domain
//!
//! An area needs a reference, and `0` is the natural one (`0` fills nothing;
//! positives fill upward). The baseline pixel is `y = 0` through the draw
//! scale, clamped into the drawable rect, so a zero-excluding domain
//! (e.g. `[10, 40]`) fills to the bottom edge, not off-frame. To include zero
//! in the axis, [`EnzAreaChart::y_domain`] with [`DomainPolicy::IncludeZero`].
//!
//! # Decimation still applies
//!
//! The fill traces the *decimated* upper envelope: a 20,000-point area over
//! 260 px builds a path bounded by pixel columns, not point count. The
//! [`DrawReport::edge_vertices`] observable counts upper-edge vertices at draw
//! time (dodging the stored-field trap and the "visually lossless"
//! measurement problem `line_chart_render.rs` documents).
//!
//! # Tokens, not literals
//!
//! Every colour comes from the theme ([`AreaChartTokens::from_palette`]):
//! accent-at-alpha fill so the grid reads through, full-alpha edge stroke. A
//! literal is the failure mode.
//!
//! # Live-sample motion
//!
//! When the series changes, the newest sample eases from its predecessor to
//! its known value over `motion-normal` — not one-sample-behind
//! interpolation (the current sample is the target immediately: no extra
//! latency). Only the entering edge moves; an unknown future is not
//! predicted.
//!
//! Widget-local transition: no app timer or message rebuilds. Frames run only
//! for the finite transition (180 ms default), stop under reduced motion, and
//! never schedule outside the viewport — so a 1 Hz monitor never becomes a
//! permanent 60 fps workload.
//!
//! # Non-finite data
//!
//! `NaN`/`inf` are not expressible as geometry: core's decimation and
//! [`enzui_core::chart::data_domain`] filter them, and a degenerate domain
//! mapping finite data to non-finite pixels is explicitly guarded (the
//! `line_chart` policy). `0.0` is legitimate and plots literally.

use crate::chart_geometry::{
    AxisLabel, AxisLayout, TickFormatter, Y_GUTTER, chart_description, chart_label_font,
    format_tick, y_axis_label_reserve,
};
use enzui_core::chart::{ChartTableRow, DomainPolicy, Series, data_table};
use enzui_core::semantics::{Role as SemRole, Semantics};
use enzui_theme::Palette;
use iced::widget::canvas::{self, Cache, Frame, Program};
use iced::{Color, Element, Length, Rectangle, Renderer, Size};
use std::cell::Cell;

/// Default plot size when the caller does not choose one.
const DEFAULT_WIDTH: f32 = 480.0;
/// Default plot height when the caller does not choose one.
const DEFAULT_HEIGHT: f32 = 240.0;

/// Alpha applied to the accent for the area fill so the grid reads through it.
/// Geometry/appearance, not a palette entry.
const FILL_ALPHA: f32 = 0.28;

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// Resolved theme tokens the area chart paints with. Every field is read from
/// the [`Palette`]; nothing here is a literal colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AreaChartTokens {
    /// Grid line colour.
    pub grid: Color,
    /// Axis line colour.
    pub axis: Color,
    /// Tick label colour.
    ///
    /// Labels are text, held to WCAG 1.4.3's 4.5:1 rather than the 3:1
    /// non-text floor of the axis lines; `text_muted` reads 7.15 dark / 5.25
    /// light on `surface`.
    pub tick_label: Color,
    /// Upper-edge stroke for the first series (series *n* uses
    /// [`Self::series_edge`]): one colour for all series would read a
    /// two-series chart as a single stack — runs indistinguishable.
    pub edge: Color,
    /// Area fill for the first series (edge at `FILL_ALPHA`); series *n*
    /// uses [`Self::series_fill`] — own hue at `FILL_ALPHA`, reading under
    /// its edge.
    pub fill: Color,
    /// Plot background fill.
    pub plot_background: Color,
    /// Focus-visible ring colour (painted by the wrapping `FocusButton`).
    pub focus_ring: Color,
    /// Categorical series palette from the same theme: each series fills and
    /// strokes in its own hue — one colour for several asserts a
    /// relationship that does not exist (regions unattributable).
    pub series_palette: crate::chart_palette::ChartPalette,
}

impl AreaChartTokens {
    /// Resolve from a [`Palette`] — the same base mapping `line_chart` uses,
    /// so the charts read as one family (`border` grid, `border_strong` axes,
    /// `accent` edge, `accent` at `FILL_ALPHA` fill, `surface` field). No
    /// literal appears, so theme changes move the chart.
    #[must_use]
    pub fn from_palette(p: &Palette) -> Self {
        let mut fill = p.accent;
        fill.a = FILL_ALPHA;
        Self {
            grid: p.border,
            // Axis lines are non-text UI held to 3:1; `border_strong` measures
            // only 2.29 dark / 1.78 light on the plot.
            axis: p.control_border_strong,
            tick_label: p.text_muted,
            edge: p.accent,
            fill,
            plot_background: p.surface,
            focus_ring: p.focus_ring,
            series_palette: crate::chart_palette::ChartPalette::from_palette(p),
        }
    }

    /// Upper-edge stroke for series `index`: `0` keeps [`Self::edge`] (the
    /// accent — single-series stays byte-identical); later series take their
    /// own [`Self::series_palette`] entry.
    pub fn series_edge(&self, index: usize) -> Color {
        if index == 0 {
            self.edge
        } else {
            self.series_palette.series_color(index)
        }
    }

    /// Fill for series `index`: the series edge at `FILL_ALPHA` — translucent
    /// fill under an opaque edge of the same hue.
    pub fn series_fill(&self, index: usize) -> Color {
        if index == 0 {
            self.fill
        } else {
            let mut c = self.series_palette.series_color(index);
            c.a = FILL_ALPHA;
            c
        }
    }
}

// ---------------------------------------------------------------------------
// Cache key
// ---------------------------------------------------------------------------

/// The inputs that change the painted static geometry. A pointer move changes
/// none of it; a resize or data edit invalidates the cache.
#[derive(Debug, Clone, Copy)]
pub struct AreaChartKey {
    tokens: AreaChartTokens,
    plot: Size,
    x_domain: (f64, f64),
    y_domain: (f64, f64),
    data_digest: u64,
    label_size: f32,
    /// resolved UI font for axis/tick labels.
    label_font: iced::Font,
    /// X-tick label formatter. A `fn` pointer keeps the key `Copy`; the
    /// hand-written `PartialEq` compares by ADDRESS so a swap invalidates
    /// without tripping `unpredictable_function_pointer_comparisons`.
    x_tick_formatter: fn(f64, f64) -> String,
    /// Y-tick formatter: address in cache identity via [`TickFormatter`], so
    /// a unit change cannot replay stale labels.
    y_tick_formatter: TickFormatter,
    /// Requested left gutter, pre-clamp: in the key because it moves every
    /// painted pixel.
    y_gutter: f32,
    /// Y-axis unit label: in the key because it is PAINTED — a cached layer
    /// replayed after a unit change would show the old unit.
    y_axis_label: AxisLabel,
    paint_plot_background: bool,
    show_y_axis_line: bool,
    /// How the edge joins the samples: in the key because it moves pixels.
    curve: ChartCurve,
}

/// How an area chart's edge joins its samples.
///
/// The curve is presentation only: the data table, the accessible summary and
/// every measured value stay the samples themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ChartCurve {
    /// Straight segments between samples (the default).
    #[default]
    Linear,
    /// A monotone cubic through every sample that never overshoots a
    /// segment's own range, so peaks and zeros stay true
    /// ([`enzui_core::chart::monotone_cubic`]).
    Monotone,
    /// A smooth curve near the samples that rounds a lone spike into a hump
    /// a little below its value and never leaves the data's range, for
    /// live readings where the trend matters more than each sample
    /// ([`enzui_core::chart::smooth_spline`]).
    Smooth,
}

/// Samples per segment a curved edge is drawn with.
const CURVE_STEPS: usize = 8;

/// `points` resampled for `curve`.
pub(crate) fn curve_points(points: &[(f64, f64)], curve: ChartCurve) -> Vec<(f64, f64)> {
    match curve {
        ChartCurve::Linear => points.to_vec(),
        ChartCurve::Monotone => enzui_core::chart::monotone_cubic(points, CURVE_STEPS),
        ChartCurve::Smooth => enzui_core::chart::smooth_spline(points, CURVE_STEPS),
    }
}

/// `series` with each edge resampled for `curve`.
fn curved(series: &[Series], curve: ChartCurve) -> Option<Vec<Series>> {
    (curve != ChartCurve::Linear).then(|| {
        series
            .iter()
            .map(|run| {
                let points: Vec<(f64, f64)> =
                    run.points.iter().map(|point| (point.x, point.y)).collect();
                Series::from_points(run.id, curve_points(&points, curve))
            })
            .collect()
    })
}

impl PartialEq for AreaChartKey {
    fn eq(&self, other: &Self) -> bool {
        self.tokens == other.tokens
            && self.plot == other.plot
            && self.x_domain == other.x_domain
            && self.y_domain == other.y_domain
            && self.data_digest == other.data_digest
            && self.label_size == other.label_size
            && self.label_font == other.label_font
            // Address comparison: a false "unequal" (identical functions,
            // different addresses) only forces a harmless redraw — never
            // stale labels.
            && (self.x_tick_formatter as usize) == (other.x_tick_formatter as usize)
            && self.y_tick_formatter == other.y_tick_formatter
            && self.y_gutter == other.y_gutter
            && self.curve == other.curve
            && self.y_axis_label == other.y_axis_label
            && self.paint_plot_background == other.paint_plot_background
            && self.show_y_axis_line == other.show_y_axis_line
    }
}

// ---------------------------------------------------------------------------
// Canvas program
// ---------------------------------------------------------------------------

/// Per-instance geometry cache plus a rebuild counter.
#[derive(Debug, Default)]
pub struct AreaChartState {
    cache: Cache,
    last: Cell<Option<AreaChartKey>>,
    builds: Cell<u32>,
}

impl AreaChartState {
    /// How many times static geometry has been built for this instance.
    #[must_use]
    pub fn builds(&self) -> u32 {
        self.builds.get()
    }
}

/// The Canvas program painting the area's static layer.
#[derive(Debug, Clone)]
pub struct AreaChartProgram {
    /// Immutable source snapshot with precomputed digest and raw domain.
    snap: crate::cartesian_chart::SeriesSnapshot,
    x_policy: DomainPolicy,
    y_policy: DomainPolicy,
    pinned_tokens: Option<AreaChartTokens>,
    x_tick_formatter: fn(f64, f64) -> String,
    y_tick_formatter: TickFormatter,
    y_gutter: f32,
    y_axis_label: AxisLabel,
    paint_plot_background: bool,
    show_y_axis_line: bool,
    curve: ChartCurve,
    transition_phase: crate::chart_motion::ChartMotionPhase,
}

impl AreaChartProgram {
    /// The series this program draws.
    pub(crate) fn series(&self) -> &[Series] {
        self.snap.as_slice()
    }

    /// The edge curve this program draws.
    pub(crate) fn edge_curve(&self) -> ChartCurve {
        self.curve
    }

    fn tokens(&self, theme: &iced::Theme) -> AreaChartTokens {
        crate::theme_lookup::resolve_pinned(
            crate::theme_lookup::PIN_SITE_AREA_CHART,
            self.pinned_tokens,
            theme,
            AreaChartTokens::from_palette,
        )
    }

    fn domains(&self) -> Option<((f64, f64), (f64, f64))> {
        self.snap.domains(self.x_policy, self.y_policy)
    }

    fn label_size(&self, theme: &iced::Theme) -> f32 {
        crate::theme_lookup::typography(theme).caption_size
    }

    /// Cache key for `theme` at `plot` size.
    pub fn key(&self, theme: &iced::Theme, plot: Size) -> AreaChartKey {
        let (x_domain, y_domain) = self.domains().unwrap_or(((0.0, 0.0), (0.0, 0.0)));
        AreaChartKey {
            tokens: self.tokens(theme),
            plot,
            x_domain,
            y_domain,
            data_digest: self.snap.digest(),
            label_size: self.label_size(theme),
            label_font: chart_label_font(theme),
            x_tick_formatter: self.x_tick_formatter,
            y_tick_formatter: self.y_tick_formatter,
            y_gutter: self.y_gutter,
            y_axis_label: self.y_axis_label,
            paint_plot_background: self.paint_plot_background,
            show_y_axis_line: self.show_y_axis_line,
            curve: self.curve,
        }
    }
}

impl<Message> Program<Message> for AreaChartProgram {
    type State = AreaChartState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let phase = self.transition_phase.get().clamp(0.0, 1.0);
        let mut key = self.key(theme, bounds.size());
        // In-flight: the interpolated newest point changes the data — digest
        // the working copy. Settled: snapshot digest.
        let working: Option<Vec<Series>> = if phase < 1.0 {
            let mut v = self.snap.as_slice().to_vec();
            let t = f64::from(phase);
            for run in &mut v {
                let n = run.points.len();
                if n >= 2 {
                    let from = run.points[n - 2].y;
                    let to = run.points[n - 1].y;
                    if from.is_finite() && to.is_finite() {
                        run.points[n - 1].y = from + (to - from) * t;
                    }
                }
            }
            key.data_digest = crate::cartesian_chart::digest(&v);
            Some(v)
        } else {
            None
        };
        let draw_slice: &[Series] = working.as_deref().unwrap_or(self.snap.as_slice());
        let curve = curved(draw_slice, self.curve);
        let draw_slice: &[Series] = curve.as_deref().unwrap_or(draw_slice);
        if state.last.get() != Some(key) {
            state.cache.clear();
            state.last.set(Some(key));
        }
        let plot = bounds.size();
        let static_geometry = state.cache.draw(renderer, plot, |frame| {
            state.builds.set(state.builds.get() + 1);
            let _ = draw_static(frame, plot, draw_slice, key);
        });
        vec![static_geometry]
    }
}

/// What one [`draw_static`] call emitted, and why it exists: a [`Frame`] is
/// write-only, so the decimation property ("20,000 points build an edge
/// bounded by pixel width, not point count") has no pixel signature —
/// decimation is visually lossless. Every field increments **inside the loop
/// feeding the path builder**, from the values handed to `move_to`/`line_to`,
/// so it cannot drift from what was drawn (the stored-field trap).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DrawReport {
    /// Number of full-canvas plot-background fills emitted.
    pub plot_background_fills: usize,
    /// Upper-edge vertices pushed into a path, all series summed — the
    /// decimation observable: bounded by `2 * inner_width`, never by point
    /// count. The baseline closure (+2 points per non-empty series) is not
    /// counted: this is the *data* edge, so a bypass reads as a jump to the
    /// point count.
    pub edge_vertices: usize,
    /// Points rejected by the non-finite pixel guard (mapping overflow).
    pub non_finite_skipped: usize,
    /// Series whose fill region was actually closed to the baseline and filled.
    pub filled_regions: usize,
    /// Horizontal grid lines stroked.
    pub grid_lines: usize,
    /// `y` pixel per grid line stroked, in draw order — each entry equals the
    /// same-index [`Self::y_tick_label_baseline_ys`]: a grid line *is* a tick.
    pub grid_line_ys: Vec<f32>,
    /// Axis lines stroked.
    pub axis_lines: usize,
    /// X-axis tick labels actually drawn (after collision reduction).
    pub x_tick_labels: usize,
    /// Exact X-tick STRINGS drawn, in order, captured from the value handed
    /// to `frame.fill_text`: asserts the per-chart formatter (e.g.
    /// `-30s`/`now`) reaches pixels, and the default is byte-for-byte the
    /// shared `format_tick`.
    pub x_tick_label_texts: Vec<String>,
    /// Y-axis tick labels actually drawn.
    pub y_tick_labels: usize,
    /// The exact Y-tick strings handed to `frame.fill_text`, in draw order.
    pub y_tick_label_texts: Vec<String>,
    /// Baseline `y` per y-tick label, in order — bit-identical to
    /// [`Self::grid_line_ys`]: one shared `YAxisTicks::pys` feeds both.
    pub y_tick_label_baseline_ys: Vec<f32>,
    /// The y the fill actually closed to (plot-local), or `None` unfilled —
    /// recorded at draw time so a test observes *where* the baseline landed,
    /// not just that a region closed. Without it `filled_regions == 1` holds
    /// with the baseline on the bottom edge or hundreds of pixels off-frame:
    /// removing the clamp would change no assertion — the check that cannot
    /// fail.
    pub baseline_y: Option<f32>,
    /// Distinct upper-edge strokes painted, first-drawn order — the
    /// multi-series observable. Two series in one edge colour look exactly
    /// like one series with more segments (appearance cannot distinguish),
    /// so record at the `frame.stroke` call from the handed value.
    pub edge_colors: Vec<Color>,
    /// Distinct fills painted, first-drawn order — recorded at `frame.fill`
    /// from the handed value.
    pub fill_colors: Vec<Color>,
    /// Exact y-axis unit string handed to `frame.fill_text` (`None` with
    /// no axis label, the default) — recorded at the draw call, so the test
    /// observes painted text, not the setter.
    pub y_axis_label_text: Option<String>,
}

/// The cartesian chart an area chart's `key` draws.
fn cartesian_key(key: AreaChartKey) -> crate::cartesian_chart::CartesianKey {
    crate::cartesian_chart::CartesianKey {
        tokens: crate::cartesian_chart::CartesianTokens {
            grid: key.tokens.grid,
            axis: key.tokens.axis,
            tick_label: key.tokens.tick_label,
            mark: key.tokens.edge,
            plot_background: key.tokens.plot_background,
            focus_ring: key.tokens.focus_ring,
            crosshair: key.tokens.focus_ring,
            series_palette: key.tokens.series_palette,
        },
        mark: crate::cartesian_chart::ResolvedMark::Area {
            edge_width: 2.0,
            first_fill: key.tokens.fill,
            fill_alpha: FILL_ALPHA,
        },
        plot: key.plot,
        x_domain: key.x_domain,
        y_domain: key.y_domain,
        data_digest: key.data_digest,
        label_size: key.label_size,
        label_font: key.label_font,
        x_tick_formatter: key.x_tick_formatter,
        y_tick_formatter: key.y_tick_formatter,
        y_gutter: key.y_gutter,
        y_axis_label: key.y_axis_label,
        paint_plot_background: key.paint_plot_background,
        show_y_axis_line: key.show_y_axis_line,
    }
}

/// Paints only the areas of `series`, without the chart around them: the
/// part of a live chart that scrolls.
pub(crate) fn draw_marks(frame: &mut Frame, plot: Size, series: &[Series], key: AreaChartKey) {
    let _ = crate::cartesian_chart::draw_marks(frame, plot, series, cartesian_key(key));
}

/// Where `key`'s plot (inside the gutters) sits in a chart of size `plot`.
pub(crate) fn plot_area(plot: Size, key: AreaChartKey) -> iced::Rectangle {
    let key = cartesian_key(key);
    let geom = crate::chart_geometry::plot_geometry_with(
        plot,
        key.x_domain,
        key.y_domain,
        key.axis_layout(),
    );
    iced::Rectangle::new(
        iced::Point::new(geom.inner_x, 0.0),
        Size::new(geom.inner_w, geom.inner_h),
    )
}

/// Paint grid, axes, tick labels and the filled area(s), reporting what was
/// emitted. The drawable plot insets from `plot` by the shared gutters (datum
/// pixels offset by the y-gutter); a `Frame` needs a renderer, so gutter
/// behaviour is exercised by `tests/area_chart_render.rs`, not unit tests.
pub fn draw_static(
    frame: &mut Frame,
    plot: Size,
    series: &[Series],
    key: AreaChartKey,
) -> DrawReport {
    let common = crate::cartesian_chart::draw_static(frame, plot, series, cartesian_key(key));
    DrawReport {
        plot_background_fills: common.plot_background_fills,
        edge_vertices: common.mark_vertices,
        non_finite_skipped: common.non_finite_skipped,
        filled_regions: common.filled_regions,
        grid_lines: common.grid_lines,
        grid_line_ys: common.grid_line_ys,
        axis_lines: common.axis_lines,
        x_tick_labels: common.x_tick_labels,
        x_tick_label_texts: common.x_tick_label_texts,
        y_tick_labels: common.y_tick_labels,
        y_tick_label_texts: common.y_tick_label_texts,
        y_tick_label_baseline_ys: common.y_tick_label_baseline_ys,
        baseline_y: common.baseline_y,
        edge_colors: common.mark_colors,
        fill_colors: common.fill_colors,
        y_axis_label_text: common.y_axis_label_text,
    }
}

impl AreaChartKey {
    /// Grid colour this key paints with.
    pub fn grid_color(&self) -> Color {
        self.tokens.grid
    }
    /// Axis colour this key paints with.
    pub fn axis_color(&self) -> Color {
        self.tokens.axis
    }
    /// Tick-label ink this key paints with (text, held to 4.5:1, so distinct
    /// from [`Self::axis_color`]).
    pub fn tick_label_color(&self) -> Color {
        self.tokens.tick_label
    }
    /// Upper-edge colour this key paints with (series 0).
    pub fn edge_color(&self) -> Color {
        self.tokens.edge
    }
    /// Fill colour this key paints with (series 0).
    pub fn fill_color(&self) -> Color {
        self.tokens.fill
    }
    /// Upper-edge stroke for series `index` — distinct per series, stable
    /// across frames (pure function of the index).
    pub fn series_edge_color(&self, index: usize) -> Color {
        self.tokens.series_edge(index)
    }
    /// Fill for series `index` — the series edge at `FILL_ALPHA`.
    pub fn series_fill_color(&self, index: usize) -> Color {
        self.tokens.series_fill(index)
    }
    /// Plot background colour this key would paint with when enabled.
    pub fn plot_background_color(&self) -> Color {
        self.tokens.plot_background
    }
    /// Whether this key paints the rectangular plot background.
    #[must_use]
    pub fn paints_plot_background(&self) -> bool {
        self.paint_plot_background
    }
    /// Whether this key paints the vertical y-axis rule.
    #[must_use]
    pub fn paints_y_axis_line(&self) -> bool {
        self.show_y_axis_line
    }
    /// The x domain this key draws with.
    #[must_use]
    pub fn x_domain(&self) -> (f64, f64) {
        self.x_domain
    }
    /// The y domain this key draws with.
    #[must_use]
    pub fn y_domain(&self) -> (f64, f64) {
        self.y_domain
    }

    /// Axis layout this key draws with (requested gutter + unit-label
    /// headroom), derived here so `draw_static` and plot-rect reasoners read
    /// one rule.
    #[must_use]
    pub fn axis_layout(&self) -> AxisLayout {
        AxisLayout {
            y_gutter: self.y_gutter,
            y_axis_label_reserve: y_axis_label_reserve(self.y_axis_label, self.label_size),
        }
    }

    /// The y-axis unit label this key paints, empty by default.
    #[must_use]
    pub fn y_axis_label(&self) -> AxisLabel {
        self.y_axis_label
    }
}

// ---------------------------------------------------------------------------
// Widget builder
// ---------------------------------------------------------------------------

/// Token-driven Cartesian area chart with one tab stop and a cached static
/// layer. `width`/`height` are [`Length`], so `Fill` grows to the parent;
/// `plot_geometry` already follows the resolved `bounds.size()` from `draw` —
/// gridlines, labels, decimation columns and focus ring track the painted
/// extent, never a cached width. Bare `f32` still works (`Into<Length>` →
/// `Fixed`).
#[derive(Debug, Clone)]
pub struct EnzAreaChart {
    /// Immutable source snapshot, rebuilt on `push` and shared with the Canvas
    /// program so `program()`/`key_for()` never rescan unchanged data.
    snap: crate::cartesian_chart::SeriesSnapshot,
    width: Length,
    height: Length,
    title: String,
    summary: Option<String>,
    x_policy: DomainPolicy,
    y_policy: DomainPolicy,
    tokens: Option<AreaChartTokens>,
    /// Formats an X tick VALUE (given the step) into its label. Defaults to
    /// the shared [`format_tick`] (existing charts keep numeric labels); a
    /// time-series override reads the axis as a unit (`-30s`/`now`). A `fn`
    /// pointer (not a boxed closure) keeps [`AreaChartKey`] `Copy` +
    /// cacheable.
    x_tick_formatter: fn(f64, f64) -> String,
    /// Formats a Y-axis tick value and receives the axis tick step.
    y_tick_formatter: TickFormatter,
    /// Requested left gutter in pixels, default
    /// [`crate::chart_geometry::Y_GUTTER`].
    y_gutter: f32,
    /// The y-axis unit label, empty by default.
    y_axis_label: AxisLabel,
    /// Paint the chart's own rectangular surface. Disable this when the chart
    /// is drawn directly on its host page and should inherit that backdrop.
    paint_plot_background: bool,
    /// Paint the vertical rule at the left edge of the data plot. Tick labels
    /// and horizontal grid lines remain independent.
    show_y_axis_line: bool,
    curve: ChartCurve,
    live: Option<crate::live_area_chart::LiveWindow>,
}

impl EnzAreaChart {
    /// Joins the samples with `curve` (default [`ChartCurve::Linear`]).
    #[must_use]
    pub fn curve(mut self, curve: ChartCurve) -> Self {
        self.curve = curve;
        self
    }

    /// Scrolls the chart continuously through a fixed `window` of time, the
    /// newest sample taken at `sampled_at`.
    ///
    /// Each point's `x` is its age in seconds relative to `sampled_at`
    /// (the newest at `0`, older ones negative); the x axis is fixed to
    /// `-window..0`. The timeline keeps moving between samples, showing the
    /// window that ended one sample interval ago: each new sample slides in
    /// from the right edge, and the edge holds the latest observation rather
    /// than predicting one if the next is late. The render lane carries the
    /// slide, so the UI thread draws the chart once per sample. Motion stops
    /// under reduced motion, while the chart is off-screen, and once the data
    /// is stale (three sample intervals without a new one); no application
    /// message or timer is involved.
    #[must_use]
    pub fn live(mut self, window: std::time::Duration, sampled_at: iced::time::Instant) -> Self {
        let seconds = window.as_secs_f64();
        if seconds.is_finite() && seconds > 0.0 {
            self.x_policy = DomainPolicy::Fixed(-seconds, 0.0);
            self.live = Some(crate::live_area_chart::LiveWindow {
                seconds,
                sampled_at,
            });
        }
        self
    }

    /// Plot width as anything convertible to [`Length`] (`Fill` grows with
    /// the parent; bare `300.0` fixes pixels). Non-finite/non-positive
    /// `Fixed` is refused (previous value kept); every other `Length` is
    /// accepted verbatim.
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        let w = width.into();
        if let Length::Fixed(v) = w {
            if v.is_finite() && v > 0.0 {
                self.width = w;
            }
        } else {
            self.width = w;
        }
        self
    }

    /// Plot height as [`Length`] — the [`Self::width`] sizing contract.
    #[must_use]
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        let h = height.into();
        if let Length::Fixed(v) = h {
            if v.is_finite() && v > 0.0 {
                self.height = h;
            }
        } else {
            self.height = h;
        }
        self
    }

    /// Configured width as a `Length`, so a test asserts what iced receives.
    #[must_use]
    pub fn width_length(&self) -> Length {
        self.width
    }

    /// The configured height as a `Length`.
    #[must_use]
    pub fn height_length(&self) -> Length {
        self.height
    }

    /// Resolve `Length` sizing to fixed pixels when both axes are `Fixed`,
    /// else the default size. Only for the renderer-free `key_for` API; the
    /// draw uses layout-resolved `bounds.size()`.
    fn key_size(&self) -> Size {
        let w = match self.width {
            Length::Fixed(v) => v,
            _ => DEFAULT_WIDTH,
        };
        let h = match self.height {
            Length::Fixed(v) => v,
            _ => DEFAULT_HEIGHT,
        };
        Size::new(w, h)
    }

    /// The accessible summary announced alongside the title.
    #[must_use]
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// How the y domain is derived (default [`DomainPolicy::Automatic`]).
    #[must_use]
    pub fn y_domain(mut self, policy: DomainPolicy) -> Self {
        self.y_policy = policy;
        self
    }

    /// How the x domain is derived (default [`DomainPolicy::Automatic`]).
    ///
    /// Time-series pin a full window (e.g. `-(HISTORY_LEN-1) .. 0`) with
    /// [`DomainPolicy::Fixed`]: the axis is stable from the first paint with
    /// few samples — samples sit true near the right edge, the line starts
    /// partway across. This changes the AXIS, not the DATA: no fabricated
    /// points.
    #[must_use]
    pub fn x_domain(mut self, policy: DomainPolicy) -> Self {
        self.x_policy = policy;
        self
    }

    /// Override the X tick formatter (default [`format_tick`]): receives tick
    /// VALUE and STEP, returns the label. The seam for unit axes
    /// (`-30s`/`now`) without touching the shared `format_tick` that
    /// percentage/byte charts rely on.
    #[must_use]
    pub fn x_tick_formatter(mut self, formatter: fn(f64, f64) -> String) -> Self {
        self.x_tick_formatter = formatter;
        self
    }

    /// Override the Y tick formatter (default [`format_tick`]): receives
    /// value and step; the step picks precision/unit without re-deriving
    /// ticks (e.g. `|value, _| format!("{value:.0}%")`).
    #[must_use]
    pub fn y_tick_formatter(mut self, formatter: fn(f64, f64) -> String) -> Self {
        self.y_tick_formatter = TickFormatter::new(formatter);
        self
    }

    /// Name the y axis's unit once, not on every tick — the primary fix
    /// for a clipped y axis. Per-tick units (`"0 Kb"`, `"2 Kb"`…) pay the
    /// unit `n` times in a widest-label gutter (apps then gamed magnitudes
    /// to fit 36px); naming it here costs one headroom line, shortens labels
    /// instead of shrinking the plot, and publishes to AT via
    /// [`Self::semantics`]. Prefer over [`Self::y_gutter`].
    ///
    /// Painted right-aligned at the gutter top, on the tick-label edge, in
    /// headroom above the topmost tick. Truncated to 24 bytes
    /// ([`AxisLabel`]): a *unit*, not a caption.
    #[must_use]
    pub fn y_axis_label(mut self, label: impl AsRef<str>) -> Self {
        self.y_axis_label = AxisLabel::new(label.as_ref());
        self
    }

    /// Widen (or narrow) the left y-tick gutter (default 40px ≈ 36px caption
    /// label). Reach for [`Self::y_axis_label`] first — it shortens labels
    /// instead of shrinking the plot; use this for genuinely wide labels
    /// (currency, dates, non-Latin numerals) after the unit is factored out.
    /// Clamped by [`crate::chart_geometry::resolve_y_gutter`]: never
    /// negative, never more than half the plot width (or the 40px default,
    /// whichever larger), never more than the plot — the plot cannot be
    /// reserved away.
    #[must_use]
    pub fn y_gutter(mut self, gutter: f32) -> Self {
        if gutter.is_finite() && gutter >= 0.0 {
            self.y_gutter = gutter;
        }
        self
    }

    /// Whether the chart paints its own rectangular plot background (default
    /// `false`, so the chart sits on its host surface).
    #[must_use]
    pub fn plot_background(mut self, paint: bool) -> Self {
        self.paint_plot_background = paint;
        self
    }

    /// Whether the vertical y-axis rule paints (default `false`; tick labels
    /// and grid lines stay visible regardless).
    #[must_use]
    pub fn y_axis_line(mut self, show: bool) -> Self {
        self.show_y_axis_line = show;
        self
    }

    /// Pin resolved tokens, bypassing the draw-time theme.
    #[must_use]
    pub fn tokens(mut self, tokens: AreaChartTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Add a series.
    #[must_use]
    pub fn push(mut self, series: Series) -> Self {
        self.snap.push(series);
        self
    }

    /// Semantics: one `Role::Image` named by the title, described by the
    /// summary — plus the unit when [`Self::y_axis_label`] is set (a
    /// pixel-only unit tells a screen reader nothing, and this is the
    /// chart's only node).
    pub fn semantics(&self) -> Semantics {
        let s = Semantics::new(SemRole::Image).label(self.title.clone());
        match chart_description(self.summary.as_deref(), self.y_axis_label) {
            Some(d) => s.description(d),
            None => s,
        }
    }

    /// The drawn series, exposed so a test hands [`draw_static`] the *same*
    /// slice the widget does.
    #[must_use]
    pub fn series(&self) -> &[Series] {
        self.snap.as_slice()
    }

    /// Companion data table: every original datum as [`ChartTableRow`] —
    /// the accessible table holds all data, not the decimated on-pixel
    /// envelope.
    #[must_use]
    pub fn data_table(&self) -> Vec<ChartTableRow> {
        data_table(self.series())
    }

    /// The Canvas program this chart draws with.
    #[must_use]
    pub fn program(&self) -> AreaChartProgram {
        self.program_with_phase(crate::chart_motion::phase())
    }

    fn program_with_phase(
        &self,
        transition_phase: crate::chart_motion::ChartMotionPhase,
    ) -> AreaChartProgram {
        self.snap.ensure();
        AreaChartProgram {
            snap: self.snap.clone(),
            x_policy: self.x_policy,
            y_policy: self.y_policy,
            pinned_tokens: self.tokens,
            x_tick_formatter: self.x_tick_formatter,
            y_tick_formatter: self.y_tick_formatter,
            y_gutter: self.y_gutter,
            y_axis_label: self.y_axis_label,
            paint_plot_background: self.paint_plot_background,
            show_y_axis_line: self.show_y_axis_line,
            curve: self.curve,
            transition_phase,
        }
    }

    /// Cache key at the configured size — pixel size unknown until layout
    /// for non-`Fixed` extents, so this keys at the default size; tests
    /// needing a painted extent use [`Self::key_for_size`] with the drawn
    /// `Size`.
    #[must_use]
    pub fn key_for(&self, theme: &iced::Theme) -> AreaChartKey {
        self.program().key(theme, self.key_size())
    }

    /// Cache key at a specific `plot` pixel size — the laid-out draw size.
    #[must_use]
    pub fn key_for_size(&self, theme: &iced::Theme, plot: Size) -> AreaChartKey {
        self.program().key(theme, plot)
    }

    /// Build the element: the Canvas in a `FocusButton` with no activation —
    /// the whole chart is one tab stop, not one per datum.
    /// `.focusable_without_activation(true)` and `.style_focus(…)` are
    /// load-bearing: without the first zero tab stops, without the second
    /// focus with no ring.
    pub fn into_element<'a, Message: 'a + Clone>(self) -> Element<'a, Message> {
        let semantics = self.semantics();
        let pinned = self.tokens;
        let width = self.width;
        let height = self.height;
        let data_key = self.snap.digest();
        let transition_phase = crate::chart_motion::phase();
        let canvas: Element<'a, Message> = match self.live {
            Some(window) => crate::viewport_gate::ViewportGate::new(Element::from(
                crate::live_area_chart::LiveAreaChart::new(
                    self.program_with_phase(transition_phase),
                    window,
                ),
            ))
            .into(),
            None => {
                let canvas: Element<'a, Message> = iced::widget::canvas(
                    self.program_with_phase(std::rc::Rc::clone(&transition_phase)),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
                crate::chart_motion::ChartMotion::new(canvas, data_key, transition_phase).into()
            }
        };

        let ring = enzui_theme::text_defaults::focus_ring_width();
        // The focus button owns outer sizing: `Fixed(v)` grows by `2 * ring`
        // so the padded canvas resolves back to `v` (shipped fixed layouts
        // exact). Non-`Fixed` takes the caller's `Length` verbatim; the
        // `Fill`-asking canvas resolves to parent-minus-ring — so `Fill`
        // reaches canvas, ring, and (via `bounds.size()` in `draw`) plot
        // geometry, gridlines and labels.
        let outer_width = match width {
            Length::Fixed(v) => Length::Fixed(v + 2.0 * ring),
            other => other,
        };
        let outer_height = match height {
            Length::Fixed(v) => Length::Fixed(v + 2.0 * ring),
            other => other,
        };
        crate::focus_button::focus_button_maybe::<Message, iced::Theme, Renderer>(canvas, None)
            .focusable_without_activation(true)
            .padding(ring)
            .width(outer_width)
            .height(outer_height)
            .semantics(semantics)
            .style_focus(move |theme, status| {
                use crate::focus_button::FocusStatus;
                let tokens = pinned.unwrap_or_else(|| {
                    crate::theme_lookup::resolve(theme, AreaChartTokens::from_palette)
                });
                let mut style = iced::widget::button::Style {
                    background: None,
                    text_color: iced::widget::button::Style::default().text_color,
                    border: iced::Border::default(),
                    ..Default::default()
                };
                if status == FocusStatus::Focused {
                    style.border = iced::Border {
                        color: tokens.focus_ring,
                        width: ring,
                        radius: 0.0.into(),
                    };
                }
                style
            })
            .into()
    }
}

impl<'a, Message: 'a + Clone> From<EnzAreaChart> for Element<'a, Message> {
    fn from(c: EnzAreaChart) -> Self {
        c.into_element()
    }
}

/// Area chart with the required accessible `title` — not optional, because
/// the chart requires a name.
#[must_use]
pub fn enz_area_chart(title: impl Into<String>) -> EnzAreaChart {
    EnzAreaChart {
        snap: crate::cartesian_chart::SeriesSnapshot::empty(),
        width: Length::Fixed(DEFAULT_WIDTH),
        height: Length::Fixed(DEFAULT_HEIGHT),
        title: title.into(),
        summary: None,
        x_policy: DomainPolicy::default(),
        y_policy: DomainPolicy::default(),
        tokens: None,
        // Shared numeric formatter by default: untouched charts byte-for-byte
        // unchanged.
        x_tick_formatter: format_tick,
        y_tick_formatter: TickFormatter::default(),
        y_gutter: Y_GUTTER,
        y_axis_label: AxisLabel::default(),
        paint_plot_background: false,
        show_y_axis_line: false,
        curve: ChartCurve::Linear,
        live: None,
    }
}

// ---------------------------------------------------------------------------
// Tests (renderer-free algebra; the Frame-driven tests live in
// tests/area_chart_render.rs)
// ---------------------------------------------------------------------------
