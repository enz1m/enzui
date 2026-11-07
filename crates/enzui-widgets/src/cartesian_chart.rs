//! Shared continuous-Cartesian chart engine.
//!
//! [`EnzCartesianChart`] is the common host for line, area, and point marks.
//! Axes, domains, decimation, cache identity, semantics, and data-table
//! projection belong to this host; [`CartesianMark`] owns only the geometry
//! that differs between marks. Existing `line_chart`, `area_chart`, and
//! `scatter_chart` modules remain source-compatible presets.
//!
//! This is intentionally not a grammar of graphics. Bars use categorical band
//! scales, pie charts are polar, and sparklines deliberately omit this host's
//! chart chrome.
//!
//! # Compatibility presets and sizing
//!
//! [`crate::line_chart::enz_line_chart`], [`crate::area_chart::enz_area_chart`],
//! and [`crate::scatter_chart::enz_scatter_chart`] retain their existing
//! constructors, tokens, reports, defaults, and event behavior while adapting
//! their static paint to this engine. The common host is generic over `Message`
//! and uses fixed `f32` dimensions like the historical line/scatter presets.
//! The area preset intentionally keeps its `Length`-based responsive sizing;
//! use it when `Length::Fill` is required rather than converting a fill
//! request to this host's fixed default without notice.
//!
//! Selection, hit testing, keyboard traversal, and crosshair overlays are
//! explicit on [`EnzCartesianChart`]. They are enabled for any mark policy;
//! compatibility area/scatter presets leave them disabled to preserve shipped
//! event behavior.

use crate::chart_geometry::{
    AxisLabel, AxisLayout, TickFormatter, Y_GUTTER, chart_description, chart_label_font,
    draw_y_axis_label, draw_y_grid, draw_y_tick_labels, fit_x_ticks, format_tick,
    has_labelable_domain, label_width, plot_geometry_with, tick_step, x_label_layout,
    y_axis_label_reserve, y_axis_ticks,
};
use crate::chart_palette::ChartPalette;
use enzui_core::chart::{
    ChartTableRow, DomainPolicy, Series, apply_domain_policy, data_domain, data_table, decimate,
    decimate_grid,
};
use enzui_core::semantics::{Role as SemRole, Semantics};
use enzui_theme::Palette;
use iced::widget::canvas::{self, Cache, Frame, LineCap, LineJoin, Path, Program, Stroke, Text};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size};
use std::cell::Cell;

// Source-scan counter: counts points visited by digest / domain
// derivation.
thread_local! {
    static SOURCE_POINTS_SCANNED: Cell<u64> = const { Cell::new(0) };
}

/// Reset the thread-local source-point scan counter to zero.
pub fn reset_source_scan_counter() {
    SOURCE_POINTS_SCANNED.with(|c| c.set(0));
}

/// Source points scanned since the last [`reset_source_scan_counter`].
pub fn source_scan_count() -> u64 {
    SOURCE_POINTS_SCANNED.with(std::cell::Cell::get)
}

pub(crate) fn record_source_scan(n: u64) {
    SOURCE_POINTS_SCANNED.with(|c| c.set(c.get() + n));
}

const AREA_FILL_ALPHA: f32 = 0.28;

/// The mark painter used by a continuous Cartesian chart.
///
/// Mark-specific dimensions live on the relevant variant, so invalid
/// combinations (for example a point radius on an area) cannot be expressed.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CartesianMark {
    /// A connected polyline.
    Line(LineMark),
    /// A polyline closed to the zero baseline and filled.
    Area(AreaMark),
    /// Independent filled points.
    Points(PointMark),
}

/// Geometry for [`CartesianMark::Line`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineMark {
    /// Stroke width in pixels.
    pub stroke_width: f32,
}
impl Default for LineMark {
    fn default() -> Self {
        Self { stroke_width: 2.0 }
    }
}

/// Geometry for [`CartesianMark::Area`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AreaMark {
    /// Upper-edge stroke width in pixels.
    pub edge_width: f32,
    /// Fill opacity applied to each series colour.
    pub fill_alpha: f32,
}
impl Default for AreaMark {
    fn default() -> Self {
        Self {
            edge_width: 2.0,
            fill_alpha: AREA_FILL_ALPHA,
        }
    }
}

/// Geometry for [`CartesianMark::Points`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointMark {
    /// Dot radius in pixels.
    pub radius: f32,
    /// Scatter decimation cell height in pixels.
    pub grid_cell_height: f32,
}
impl Default for PointMark {
    fn default() -> Self {
        Self {
            radius: 2.5,
            grid_cell_height: 5.0,
        }
    }
}

impl Default for CartesianMark {
    fn default() -> Self {
        Self::Line(LineMark::default())
    }
}

/// Theme fields whose meaning is identical for every Cartesian mark.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CartesianTokens {
    /// Grid-line colour.
    pub grid: Color,
    /// Axis-line colour.
    pub axis: Color,
    /// Tick-label colour.
    pub tick_label: Color,
    /// First-series colour.
    pub mark: Color,
    /// Plot background.
    pub plot_background: Color,
    /// Focus-visible ring colour.
    pub focus_ring: Color,
    /// Hover crosshair rule colour.
    pub crosshair: Color,
    /// Per-series palette.
    pub series_palette: ChartPalette,
}

impl CartesianTokens {
    /// Resolve common chart tokens from a palette.
    #[must_use]
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            grid: p.border,
            axis: p.control_border_strong,
            tick_label: p.text_muted,
            mark: p.accent,
            plot_background: p.surface,
            focus_ring: p.focus_ring,
            crosshair: p.text_subtle,
            series_palette: ChartPalette::from_palette(p),
        }
    }

    /// Stable colour for a series index; series zero preserves `mark`.
    pub fn series_mark(&self, index: usize) -> Color {
        if index == 0 {
            self.mark
        } else {
            self.series_palette.series_color(index)
        }
    }
}

/// Resolved mark paint. Compatibility wrappers use this internal adapter to
/// preserve custom first-series fill colours exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ResolvedMark {
    Line {
        width: f32,
    },
    Area {
        edge_width: f32,
        first_fill: Color,
        fill_alpha: f32,
    },
    Points {
        radius: f32,
        grid_cell_height: f32,
    },
}

impl ResolvedMark {
    fn public(mark: CartesianMark, tokens: CartesianTokens) -> Self {
        match mark {
            CartesianMark::Line(m) => Self::Line {
                width: m.stroke_width,
            },
            CartesianMark::Area(m) => {
                let mut fill = tokens.mark;
                fill.a = m.fill_alpha;
                Self::Area {
                    edge_width: m.edge_width,
                    first_fill: fill,
                    fill_alpha: m.fill_alpha,
                }
            }
            CartesianMark::Points(m) => Self::Points {
                radius: m.radius,
                grid_cell_height: m.grid_cell_height,
            },
        }
    }
}

/// All visible inputs to the shared static draw path.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CartesianKey {
    pub tokens: CartesianTokens,
    pub mark: ResolvedMark,
    pub plot: Size,
    pub x_domain: (f64, f64),
    pub y_domain: (f64, f64),
    pub data_digest: u64,
    pub label_size: f32,
    pub label_font: iced::Font,
    pub x_tick_formatter: fn(f64, f64) -> String,
    pub y_tick_formatter: TickFormatter,
    pub y_gutter: f32,
    pub y_axis_label: AxisLabel,
    /// Whether the chart paints its own rectangular plot fill.
    pub paint_plot_background: bool,
    /// Whether the vertical y-axis rule is painted. Horizontal grid lines and
    /// y labels are independent and remain visible when this is false.
    pub show_y_axis_line: bool,
}

impl PartialEq for CartesianKey {
    fn eq(&self, other: &Self) -> bool {
        self.tokens == other.tokens
            && self.mark == other.mark
            && self.plot == other.plot
            && self.x_domain == other.x_domain
            && self.y_domain == other.y_domain
            && self.data_digest == other.data_digest
            && self.label_size == other.label_size
            && self.label_font == other.label_font
            && std::ptr::fn_addr_eq(self.x_tick_formatter, other.x_tick_formatter)
            && self.y_tick_formatter == other.y_tick_formatter
            && self.y_gutter == other.y_gutter
            && self.y_axis_label == other.y_axis_label
            && self.paint_plot_background == other.paint_plot_background
            && self.show_y_axis_line == other.show_y_axis_line
    }
}

impl CartesianKey {
    pub(crate) fn axis_layout(self) -> AxisLayout {
        AxisLayout {
            y_gutter: self.y_gutter,
            y_axis_label_reserve: y_axis_label_reserve(self.y_axis_label, self.label_size),
        }
    }
}

/// Cheap structural identity shared by every Cartesian preset.
pub(crate) fn digest(series: &[Series]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325_u64;
    let mut mix = |v| {
        h ^= v;
        h = h.wrapping_mul(0x1000_0000_01b3);
    };
    let mut total_points = 0u64;
    mix(series.len() as u64);
    for s in series {
        mix(u64::from(s.id.0));
        mix(s.points.len() as u64);
        total_points += s.points.len() as u64;
        for d in &s.points {
            mix((d.x + 0.0).to_bits());
            mix((d.y + 0.0).to_bits());
        }
    }
    record_source_scan(total_points);
    h
}

// Raw data-domain derivation (before policy). Instrumented.
pub(crate) fn raw_data_domain(series: &[Series]) -> Option<((f64, f64), (f64, f64))> {
    let total_points: u64 = series.iter().map(|s| s.points.len() as u64).sum();
    record_source_scan(total_points);
    data_domain(series)
}

pub(crate) fn apply_policies(
    raw: ((f64, f64), (f64, f64)),
    x: DomainPolicy,
    y: DomainPolicy,
) -> ((f64, f64), (f64, f64)) {
    (
        apply_domain_policy(raw.0, x),
        crate::chart_geometry::value_axis_domain(raw.1, y),
    )
}

// SeriesSnapshot: shared source data + lazy metadata.
#[derive(Debug)]
pub(crate) struct SnapshotInner {
    pub(crate) series: Vec<Series>,
    meta: Cell<Option<SnapshotMeta>>,
}

impl Clone for SnapshotInner {
    fn clone(&self) -> Self {
        Self {
            series: self.series.clone(),
            meta: Cell::new(self.meta.get()),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SnapshotMeta {
    digest: u64,
    raw_domain: Option<((f64, f64), (f64, f64))>,
}

// Metadata belongs to the shared source; a COW mutation invalidates only its fork.
#[derive(Debug, Clone)]
pub(crate) struct SeriesSnapshot {
    pub(crate) inner: std::rc::Rc<SnapshotInner>,
}

impl SeriesSnapshot {
    pub(crate) fn empty() -> Self {
        Self {
            inner: std::rc::Rc::new(SnapshotInner {
                series: Vec::new(),
                meta: Cell::new(None),
            }),
        }
    }

    pub(crate) fn push(&mut self, s: Series) {
        std::rc::Rc::make_mut(&mut self.inner).series.push(s);
        self.inner.meta.set(None);
    }

    pub(crate) fn ensure(&self) {
        if self.inner.meta.get().is_some() {
            return;
        }
        let d = digest(&self.inner.series);
        let raw = raw_data_domain(&self.inner.series);
        self.inner.meta.set(Some(SnapshotMeta {
            digest: d,
            raw_domain: raw,
        }));
    }

    fn meta(&self) -> SnapshotMeta {
        self.ensure();
        self.inner.meta.get().expect("ensure fills the metadata")
    }

    pub(crate) fn digest(&self) -> u64 {
        self.meta().digest
    }

    pub(crate) fn domains(
        &self,
        x: DomainPolicy,
        y: DomainPolicy,
    ) -> Option<((f64, f64), (f64, f64))> {
        self.meta().raw_domain.map(|raw| apply_policies(raw, x, y))
    }

    pub(crate) fn as_slice(&self) -> &[Series] {
        &self.inner.series
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct CartesianDrawReport {
    pub plot_background_fills: usize,
    pub mark_vertices: usize,
    pub non_finite_skipped: usize,
    pub series_drawn: usize,
    pub mark_colors: Vec<Color>,
    pub filled_regions: usize,
    pub fill_colors: Vec<Color>,
    pub baseline_y: Option<f32>,
    pub grid_lines: usize,
    pub grid_line_ys: Vec<f32>,
    pub axis_lines: usize,
    pub x_tick_labels: usize,
    pub x_tick_label_texts: Vec<String>,
    pub y_tick_labels: usize,
    pub y_tick_label_texts: Vec<String>,
    pub y_tick_label_baseline_ys: Vec<f32>,
    pub y_axis_label_text: Option<String>,
}

/// An area's fill: `color` deepening toward the edge's highest point and
/// fading toward the baseline, so the edge reads as the surface and the
/// ground stays light.
pub(crate) fn area_fill(color: Color, edge: &[Point], baseline: f32) -> canvas::Fill {
    let top = edge.iter().map(|p| p.y).fold(baseline, f32::min);
    if baseline - top < 1.0 {
        return color.into();
    }
    let gradient = canvas::gradient::Linear::new(Point::new(0.0, top), Point::new(0.0, baseline))
        .add_stop(
            0.0,
            Color {
                a: (color.a * 1.6).min(1.0),
                ..color
            },
        )
        .add_stop(
            1.0,
            Color {
                a: color.a * 0.15,
                ..color
            },
        );
    canvas::Fill {
        style: canvas::Style::Gradient(canvas::Gradient::Linear(gradient)),
        ..canvas::Fill::default()
    }
}

pub(crate) fn draw_static(
    frame: &mut Frame,
    plot: Size,
    series: &[Series],
    key: CartesianKey,
) -> CartesianDrawReport {
    let mut report = CartesianDrawReport::default();
    if plot.width <= 0.0 || plot.height <= 0.0 {
        return report;
    }
    if key.paint_plot_background {
        frame.fill(
            &Path::rectangle(Point::ORIGIN, plot),
            key.tokens.plot_background,
        );
        report.plot_background_fills += 1;
    }
    let geom = plot_geometry_with(plot, key.x_domain, key.y_domain, key.axis_layout());
    let (inner_x, inner_w, inner_h) = (geom.inner_x, geom.inner_w, geom.inner_h);
    let y_ticks = y_axis_ticks(key.y_domain, geom.y_scale);
    let draw_chrome =
        has_labelable_domain(key.x_domain, key.y_domain) && inner_w > 0.0 && inner_h > 0.0;
    if draw_chrome {
        report.grid_line_ys = draw_y_grid(
            frame,
            &y_ticks,
            inner_x,
            inner_w,
            Stroke::default()
                .with_color(key.tokens.grid)
                .with_width(1.0),
        );
        report.grid_lines = report.grid_line_ys.len();
    }
    let axis_stroke = Stroke::default()
        .with_color(key.tokens.axis)
        .with_width(1.0);
    frame.stroke(
        &Path::line(
            Point::new(inner_x, inner_h - 0.5),
            Point::new(inner_x + inner_w, inner_h - 0.5),
        ),
        axis_stroke,
    );
    report.axis_lines += 1;
    if key.show_y_axis_line {
        frame.stroke(
            &Path::line(
                Point::new(inner_x + 0.5, 0.0),
                Point::new(inner_x + 0.5, inner_h),
            ),
            axis_stroke,
        );
        report.axis_lines += 1;
    }

    if draw_chrome {
        let x_ticks = fit_x_ticks(key.x_domain, inner_w, key.label_size, key.label_font);
        let step = tick_step(&x_ticks);
        for t in x_ticks {
            let px = geom.x_scale.map(t);
            if !px.is_finite() {
                continue;
            }
            let content = (key.x_tick_formatter)(t, step);
            report.x_tick_label_texts.push(content.clone());
            let width = label_width(&content, key.label_size, key.label_font);
            let (position, align_x) = x_label_layout(px as f32, inner_h, width, plot.width);
            frame.fill_text(Text {
                content,
                position,
                color: key.tokens.tick_label,
                size: key.label_size.into(),
                font: key.label_font,
                align_x,
                align_y: iced::alignment::Vertical::Top,
                ..Text::default()
            });
            report.x_tick_labels += 1;
        }
        let (texts, baselines) = draw_y_tick_labels(
            frame,
            &y_ticks,
            inner_x,
            key.label_size,
            key.label_font,
            key.tokens.tick_label,
            key.y_tick_formatter,
        );
        report.y_tick_label_texts = texts;
        report.y_tick_label_baseline_ys = baselines;
        report.y_tick_labels = report.y_tick_label_texts.len();
        report.y_axis_label_text = draw_y_axis_label(
            frame,
            key.y_axis_label,
            inner_x,
            key.label_size,
            key.label_font,
            key.tokens.tick_label,
        );
    }

    draw_marks_into(frame, &geom, series, &key, &mut report);
    report
}

/// Paints only the marks of `series` (no background, grid, axes or labels):
/// the moving part of a live chart, over chrome painted once.
pub(crate) fn draw_marks(
    frame: &mut Frame,
    plot: Size,
    series: &[Series],
    key: CartesianKey,
) -> CartesianDrawReport {
    let mut report = CartesianDrawReport::default();
    if plot.width <= 0.0 || plot.height <= 0.0 {
        return report;
    }
    let geom = plot_geometry_with(plot, key.x_domain, key.y_domain, key.axis_layout());
    draw_marks_into(frame, &geom, series, &key, &mut report);
    report
}

fn draw_marks_into(
    frame: &mut Frame,
    geom: &crate::chart_geometry::PlotGeometry,
    series: &[Series],
    key: &CartesianKey,
    report: &mut CartesianDrawReport,
) {
    let (inner_w, inner_h) = (geom.inner_w, geom.inner_h);
    let dec_scale = geom.column_scale();
    match key.mark {
        ResolvedMark::Line { width } => {
            for (index, s) in series.iter().enumerate() {
                let dec = decimate(s, inner_w.max(0.0) as u32, &dec_scale);
                let color = key.tokens.series_mark(index);
                let mut emitted = 0;
                let mut skipped = 0;
                let path = Path::new(|b| {
                    let mut started = false;
                    for p in &dec.points {
                        let (x, y) = (geom.x_scale.map(p.x), geom.y_scale.map(p.y));
                        if !x.is_finite() || !y.is_finite() {
                            skipped += 1;
                            continue;
                        }
                        let point = Point::new(x as f32, y as f32);
                        if started {
                            b.line_to(point);
                        } else {
                            b.move_to(point);
                            started = true;
                        }
                        emitted += 1;
                    }
                });
                report.mark_vertices += emitted;
                report.non_finite_skipped += skipped;
                if emitted > 0 {
                    frame.stroke(
                        &path,
                        Stroke::default()
                            .with_color(color)
                            .with_width(width)
                            .with_line_cap(LineCap::Round)
                            .with_line_join(LineJoin::Round),
                    );
                    report.series_drawn += 1;
                    if !report.mark_colors.contains(&color) {
                        report.mark_colors.push(color);
                    }
                }
            }
        }
        ResolvedMark::Area {
            edge_width,
            first_fill,
            fill_alpha,
        } => {
            let raw = geom.y_scale.map(0.0);
            let baseline = if raw.is_finite() {
                (raw as f32).clamp(0.0, inner_h)
            } else {
                inner_h
            };
            report.baseline_y = Some(baseline);
            for (index, s) in series.iter().enumerate() {
                let dec = decimate(s, inner_w.max(0.0) as u32, &dec_scale);
                let mut edge = Vec::with_capacity(dec.points.len());
                for p in &dec.points {
                    let (x, y) = (geom.x_scale.map(p.x), geom.y_scale.map(p.y));
                    if !x.is_finite() || !y.is_finite() {
                        report.non_finite_skipped += 1;
                        continue;
                    }
                    edge.push(Point::new(x as f32, y as f32));
                    report.mark_vertices += 1;
                }
                if edge.is_empty() {
                    continue;
                }
                let edge_color = key.tokens.series_mark(index);
                let fill_color = if index == 0 {
                    first_fill
                } else {
                    let mut c = edge_color;
                    c.a = fill_alpha;
                    c
                };
                let first_x = edge[0].x;
                let last_x = edge[edge.len() - 1].x;
                let fill = Path::new(|b| {
                    b.move_to(Point::new(first_x, baseline));
                    for p in &edge {
                        b.line_to(*p);
                    }
                    b.line_to(Point::new(last_x, baseline));
                    b.close();
                });
                frame.fill(&fill, area_fill(fill_color, &edge, baseline));
                report.filled_regions += 1;
                if !report.fill_colors.contains(&fill_color) {
                    report.fill_colors.push(fill_color);
                }
                let outline = Path::new(|b| {
                    let mut first = true;
                    for p in &edge {
                        if first {
                            b.move_to(*p);
                            first = false;
                        } else {
                            b.line_to(*p);
                        }
                    }
                });
                frame.stroke(
                    &outline,
                    Stroke::default()
                        .with_color(edge_color)
                        .with_width(edge_width)
                        .with_line_cap(LineCap::Round)
                        .with_line_join(LineJoin::Round),
                );
                report.series_drawn += 1;
                if !report.mark_colors.contains(&edge_color) {
                    report.mark_colors.push(edge_color);
                }
            }
        }
        ResolvedMark::Points {
            radius,
            grid_cell_height,
        } => {
            let rows = if grid_cell_height > 0.0 {
                (inner_h / grid_cell_height).ceil().max(1.0) as u32
            } else {
                1
            };
            let row_scale = enzui_core::chart::LinearScale {
                domain: key.y_domain,
                range: (f64::from(rows), 0.0),
            };
            // Dots at the domain's extremes are drawn whole: the point scales
            // stop a dot's radius short of the plot's edges, as Swift Charts
            // and matplotlib pad a scatter.
            let inset_scale = |scale: &enzui_core::chart::LinearScale| {
                let inset = f64::from(radius + 1.0);
                let (a, b) = scale.range;
                let toward = if b >= a { inset } else { -inset };
                if (b - a).abs() > 2.0 * inset {
                    enzui_core::chart::LinearScale {
                        domain: scale.domain,
                        range: (a + toward, b - toward),
                    }
                } else {
                    *scale
                }
            };
            let (x_scale, y_scale) = (inset_scale(&geom.x_scale), inset_scale(&geom.y_scale));
            for (index, s) in series.iter().enumerate() {
                let color = key.tokens.series_mark(index);
                let dec = decimate_grid(s, inner_w.max(0.0) as u32, rows, &dec_scale, &row_scale);
                let mut emitted = 0;
                for p in &dec.points {
                    let (x, y) = (x_scale.map(p.x), y_scale.map(p.y));
                    if !x.is_finite() || !y.is_finite() {
                        report.non_finite_skipped += 1;
                        continue;
                    }
                    frame.fill(&Path::circle(Point::new(x as f32, y as f32), radius), color);
                    emitted += 1;
                }
                report.mark_vertices += emitted;
                if emitted > 0 {
                    report.series_drawn += 1;
                    if !report.mark_colors.contains(&color) {
                        report.mark_colors.push(color);
                    }
                }
            }
        }
    }
}

/// Shared non-visual model used by compatibility wrappers and the common host.
#[derive(Debug, Clone)]
pub struct CartesianModel {
    snap: SeriesSnapshot,
    title: String,
    summary: Option<String>,
    x_policy: DomainPolicy,
    y_policy: DomainPolicy,
    y_axis_label: AxisLabel,
}

impl CartesianModel {
    /// Create an empty titled model.
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            snap: SeriesSnapshot::empty(),
            title: title.into(),
            summary: None,
            x_policy: DomainPolicy::default(),
            y_policy: DomainPolicy::default(),
            y_axis_label: AxisLabel::default(),
        }
    }
    /// Add a series. O(1) amortized; metadata recomputed on next read.
    #[must_use]
    pub fn push(mut self, series: Series) -> Self {
        self.snap.push(series);
        self
    }
    /// Set the accessible summary.
    #[must_use]
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }
    /// Set X-domain policy.
    #[must_use]
    pub fn x_domain(mut self, policy: DomainPolicy) -> Self {
        self.x_policy = policy;
        self
    }
    /// Set Y-domain policy.
    #[must_use]
    pub fn y_domain(mut self, policy: DomainPolicy) -> Self {
        self.y_policy = policy;
        self
    }
    /// Name the Y-axis unit.
    #[must_use]
    pub fn y_axis_label(mut self, label: impl AsRef<str>) -> Self {
        self.y_axis_label = AxisLabel::new(label.as_ref());
        self
    }
    /// Original series, never the decimated projection.
    #[must_use]
    pub fn series(&self) -> &[Series] {
        self.snap.as_slice()
    }
    /// Domain pair with policies applied, from cached raw domain.
    #[must_use]
    pub fn domains(&self) -> Option<((f64, f64), (f64, f64))> {
        self.snap.domains(self.x_policy, self.y_policy)
    }
    /// One bounded chart semantics node.
    pub fn semantics(&self) -> Semantics {
        let node = Semantics::new(SemRole::Image).label(self.title.clone());
        match chart_description(self.summary.as_deref(), self.y_axis_label) {
            Some(d) => node.description(d),
            None => node,
        }
    }
    /// Full companion table from original data.
    #[must_use]
    pub fn data_table(&self) -> Vec<ChartTableRow> {
        data_table(self.snap.as_slice())
    }
}

/// Common Cartesian builder for new code.
///
/// Compatibility wrappers retain their historical sizing and interaction
/// defaults; this builder makes mark selection explicit and uses fixed pixel
/// sizing. It can be converted mechanically to a preset by choosing the same
/// [`CartesianMark`].
#[derive(Clone)]
pub struct EnzCartesianChart<Message> {
    model: CartesianModel,
    mark: CartesianMark,
    width: f32,
    height: f32,
    x_tick_formatter: fn(f64, f64) -> String,
    y_tick_formatter: TickFormatter,
    y_gutter: f32,
    crosshair: bool,
    selection: Option<crate::line_chart::DatumSelection>,
    on_select: Option<std::rc::Rc<dyn Fn(crate::line_chart::DatumSelection) -> Message>>,
    _message: std::marker::PhantomData<Message>,
}

impl<Message> EnzCartesianChart<Message> {
    /// Choose the mark painter.
    #[must_use]
    pub fn mark(mut self, mark: CartesianMark) -> Self {
        self.mark = mark;
        self
    }
    /// Fixed plot width; non-positive/non-finite values are refused.
    #[must_use]
    pub fn width(mut self, width: f32) -> Self {
        if width.is_finite() && width > 0.0 {
            self.width = width;
        }
        self
    }
    /// Fixed plot height; non-positive/non-finite values are refused.
    #[must_use]
    pub fn height(mut self, height: f32) -> Self {
        if height.is_finite() && height > 0.0 {
            self.height = height;
        }
        self
    }
    /// Set accessible summary.
    #[must_use]
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.model = self.model.summary(summary);
        self
    }
    /// Set X domain policy.
    #[must_use]
    pub fn x_domain(mut self, policy: DomainPolicy) -> Self {
        self.model = self.model.x_domain(policy);
        self
    }
    /// Set Y domain policy.
    #[must_use]
    pub fn y_domain(mut self, policy: DomainPolicy) -> Self {
        self.model = self.model.y_domain(policy);
        self
    }
    /// Set X tick formatter.
    #[must_use]
    pub fn x_tick_formatter(mut self, formatter: fn(f64, f64) -> String) -> Self {
        self.x_tick_formatter = formatter;
        self
    }
    /// Set Y tick formatter.
    #[must_use]
    pub fn y_tick_formatter(mut self, formatter: fn(f64, f64) -> String) -> Self {
        self.y_tick_formatter = TickFormatter::new(formatter);
        self
    }
    /// Set Y unit label.
    #[must_use]
    pub fn y_axis_label(mut self, label: impl AsRef<str>) -> Self {
        self.model = self.model.y_axis_label(label);
        self
    }
    /// Set Y label gutter.
    #[must_use]
    pub fn y_gutter(mut self, gutter: f32) -> Self {
        if gutter.is_finite() && gutter >= 0.0 {
            self.y_gutter = gutter;
        }
        self
    }
    /// Enable or disable the nearest-datum hover crosshair.
    #[must_use]
    pub fn crosshair(mut self, enabled: bool) -> Self {
        self.crosshair = enabled;
        self
    }
    /// Set the host-owned keyboard selection cursor.
    #[must_use]
    pub fn selection(mut self, selection: Option<crate::line_chart::DatumSelection>) -> Self {
        self.selection = selection;
        self
    }
    /// Publish a message when an arrow key traverses to another drawn datum.
    #[must_use]
    pub fn on_select(
        mut self,
        f: impl Fn(crate::line_chart::DatumSelection) -> Message + 'static,
    ) -> Self {
        self.on_select = Some(std::rc::Rc::new(f));
        self
    }
    /// Add a series.
    #[must_use]
    pub fn push(mut self, series: Series) -> Self {
        self.model = self.model.push(series);
        self
    }
    /// Original series.
    #[must_use]
    pub fn series(&self) -> &[Series] {
        self.model.series()
    }
    /// One chart semantics node, including the selected datum when present.
    pub fn semantics(&self) -> Semantics {
        if self.selection.is_some() {
            self.line_adapter().semantics()
        } else {
            self.model.semantics()
        }
    }
    /// Resolve a traversal request against the points painted at this size.
    #[must_use]
    pub fn traverse(
        &self,
        key: crate::line_chart::TraversalKey,
    ) -> Option<crate::line_chart::DatumSelection> {
        self.line_adapter().traverse(key)
    }
    /// Hit-test original data and return its widget-local pixel anchor.
    #[must_use]
    pub fn hit_at(
        &self,
        cursor: Point,
        bounds: Size,
    ) -> Option<(enzui_core::chart::HitResult, Point)> {
        self.line_adapter().program().hit_at(cursor, bounds)
    }
    fn line_adapter(&self) -> crate::line_chart::EnzLineChart<Message> {
        let mut chart = crate::line_chart::enz_line_chart(self.model.title.clone())
            .width(self.width)
            .height(self.height)
            .x_domain(self.model.x_policy)
            .y_domain(self.model.y_policy)
            .y_gutter(self.y_gutter)
            .y_axis_label(self.model.y_axis_label.as_str())
            .selection(self.selection)
            .crosshair(self.crosshair);
        if let Some(summary) = &self.model.summary {
            chart = chart.summary(summary.clone());
        }
        for series in self.model.series() {
            chart = chart.push(series.clone());
        }
        chart
    }
    /// Full companion table.
    #[must_use]
    pub fn data_table(&self) -> Vec<ChartTableRow> {
        self.model.data_table()
    }
    /// Selected mark policy.
    #[must_use]
    pub fn mark_policy(&self) -> CartesianMark {
        self.mark
    }
    /// Canvas program used by the real draw path.
    #[must_use]
    pub fn program(&self) -> CartesianProgram {
        self.model.snap.ensure();
        CartesianProgram {
            model: self.model.clone(),
            mark: self.mark,
            x_tick_formatter: self.x_tick_formatter,
            y_tick_formatter: self.y_tick_formatter,
            y_gutter: self.y_gutter,
            crosshair: self.crosshair,
            selection: self.selection,
        }
    }
    /// Cache identity at the configured fixed size.
    #[must_use]
    pub fn key_for(&self, theme: &iced::Theme) -> CartesianCacheKey {
        self.program()
            .key(theme, Size::new(self.width, self.height))
    }
    /// Build one focusable chart element.
    pub fn into_element<'a>(self) -> Element<'a, Message>
    where
        Message: 'a + Clone,
    {
        let semantics = self.semantics();
        let width = self.width;
        let height = self.height;
        let canvas = iced::widget::canvas(self.program())
            .width(Length::Fixed(width))
            .height(Length::Fixed(height));
        const RING: f32 = 2.0;
        let stop: Element<'a, Message> =
            crate::focus_button::focus_button_maybe::<Message, iced::Theme, Renderer>(canvas, None)
                .focusable_without_activation(true)
                .padding(RING)
                .width(Length::Fixed(width + 2.0 * RING))
                .height(Length::Fixed(height + 2.0 * RING))
                .semantics(semantics)
                .style_focus(move |theme, status| {
                    let tokens = crate::theme_lookup::resolve(theme, CartesianTokens::from_palette);
                    let mut style = iced::widget::button::Style {
                        background: None,
                        text_color: iced::widget::button::Style::default().text_color,
                        border: iced::Border::default(),
                        ..Default::default()
                    };
                    if status == crate::focus_button::FocusStatus::Focused {
                        style.border = iced::Border {
                            color: tokens.focus_ring,
                            width: RING,
                            radius: 0.0.into(),
                        };
                    }
                    style
                })
                .into();
        let Some(on_select) = self.on_select.clone() else {
            return stop;
        };
        crate::keyboard_capture::keyboard_capture(stop, move |key, modifiers| {
            // Ctrl/Alt/Logo-modified traversal keys are application chords; the
            // shared decoder rejects them so they keep bubbling to the app
            // instead of being captured by a focused chart. See
            // `crate::line_chart::event_for_key`.
            let traversal = crate::line_chart::event_for_key(key, modifiers)?;
            let next = self.traverse(traversal)?;
            Some(on_select(next))
        })
    }
}

impl<'a, Message: 'a + Clone> From<EnzCartesianChart<Message>> for Element<'a, Message> {
    fn from(chart: EnzCartesianChart<Message>) -> Self {
        chart.into_element()
    }
}

/// Opaque cache identity for a common Cartesian program.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CartesianCacheKey(CartesianKey);

/// Shared Canvas cache state. `builds` observes real cache reuse.
#[derive(Debug, Default)]
pub struct CartesianState {
    cache: Cache,
    last: Cell<Option<CartesianKey>>,
    builds: Cell<u32>,
    crosshair_cursor: Cell<Option<Point>>,
}

impl CartesianState {
    /// Number of static geometry builds performed by this state.
    #[must_use]
    pub fn builds(&self) -> u32 {
        self.builds.get()
    }
}

/// Canvas program for all three continuous Cartesian mark policies.
#[derive(Debug, Clone)]
pub struct CartesianProgram {
    model: CartesianModel,
    mark: CartesianMark,
    x_tick_formatter: fn(f64, f64) -> String,
    y_tick_formatter: TickFormatter,
    y_gutter: f32,
    crosshair: bool,
    selection: Option<crate::line_chart::DatumSelection>,
}

impl CartesianProgram {
    fn interaction_chart(
        &self,
        tokens: CartesianTokens,
        plot: Size,
    ) -> crate::line_chart::EnzLineChart<()> {
        let mut chart = crate::line_chart::enz_line_chart("")
            .width(plot.width)
            .height(plot.height)
            .x_domain(self.model.x_policy)
            .y_domain(self.model.y_policy)
            .y_gutter(self.y_gutter)
            .y_axis_label(self.model.y_axis_label.as_str())
            .selection(self.selection)
            .crosshair(self.crosshair)
            .tokens(crate::line_chart::LineChartTokens {
                grid: tokens.grid,
                axis: tokens.axis,
                tick_label: tokens.tick_label,
                mark: tokens.mark,
                plot_background: tokens.plot_background,
                focus_ring: tokens.focus_ring,
                crosshair: tokens.crosshair,
                series_palette: tokens.series_palette,
            });
        for series in self.model.series() {
            chart = chart.push(series.clone());
        }
        chart
    }

    fn key(&self, theme: &iced::Theme, plot: Size) -> CartesianCacheKey {
        let tokens = crate::theme_lookup::resolve(theme, CartesianTokens::from_palette);
        let (x_domain, y_domain) = self.model.domains().unwrap_or(((0.0, 0.0), (0.0, 0.0)));
        CartesianCacheKey(CartesianKey {
            tokens,
            mark: ResolvedMark::public(self.mark, tokens),
            plot,
            x_domain,
            y_domain,
            data_digest: self.model.snap.digest(),
            label_size: crate::theme_lookup::typography(theme).caption_size,
            label_font: chart_label_font(theme),
            x_tick_formatter: self.x_tick_formatter,
            y_tick_formatter: self.y_tick_formatter,
            y_gutter: self.y_gutter,
            y_axis_label: self.model.y_axis_label,
            paint_plot_background: false,
            show_y_axis_line: false,
        })
    }
}

impl<Message> Program<Message> for CartesianProgram {
    type State = CartesianState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> Option<iced::widget::Action<Message>> {
        crate::line_chart::crosshair_redraw(
            &state.crosshair_cursor,
            self.crosshair,
            event,
            bounds,
            cursor,
        )
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        state.crosshair_cursor.set(if self.crosshair {
            _cursor.position_in(bounds)
        } else {
            None
        });
        let key = self.key(theme, bounds.size()).0;
        if state.last.get() != Some(key) {
            state.cache.clear();
            state.last.set(Some(key));
        }
        let plot = bounds.size();
        let geometry = state.cache.draw(renderer, plot, |frame| {
            state.builds.set(state.builds.get() + 1);
            let _ = draw_static(frame, plot, self.model.series(), key);
        });
        let mut geometries = vec![geometry];
        let needs_crosshair = self.crosshair && _cursor.position_in(bounds).is_some();
        let needs_selection = self.selection.is_some();
        if needs_crosshair || needs_selection {
            let interaction = self.interaction_chart(key.tokens, plot);
            let interaction_key = interaction.key_for(theme);
            if let (true, Some(position)) = (self.crosshair, _cursor.position_in(bounds)) {
                let mut frame = Frame::new(renderer, plot);
                let _ = crate::line_chart::draw_crosshair(
                    &mut frame,
                    plot,
                    self.model.series(),
                    interaction_key,
                    position,
                );
                geometries.push(frame.into_geometry());
            }
            if let Some(selection) = self.selection {
                let mut frame = Frame::new(renderer, plot);
                let _ = crate::line_chart::draw_selection(
                    &mut frame,
                    plot,
                    self.model.series(),
                    interaction_key,
                    selection,
                );
                geometries.push(frame.into_geometry());
            }
        }
        geometries
    }
}

/// Construct the common Cartesian host with line marks by default.
#[must_use]
pub fn enz_cartesian_chart<Message>(title: impl Into<String>) -> EnzCartesianChart<Message> {
    EnzCartesianChart {
        model: CartesianModel::new(title),
        mark: CartesianMark::default(),
        width: 480.0,
        height: 240.0,
        x_tick_formatter: format_tick,
        y_tick_formatter: TickFormatter::default(),
        y_gutter: Y_GUTTER,
        crosshair: true,
        selection: None,
        on_select: None,
        _message: std::marker::PhantomData,
    }
}
