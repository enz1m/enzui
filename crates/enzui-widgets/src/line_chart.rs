//! `EnzLineChart`: a Canvas-based Cartesian line chart host.
//!
//! Line marks only. Scales, domain derivation, decimation and the
//! original-data hit index are shared with [`enzui_core::chart`], never
//! reimplemented.
//!
//! One focusable tab stop; arrows move a [`DatumSelection`] cursor between
//! drawn points inside it. The cursor anchors by [`DatumKey`], not pixel
//! index — surviving decimation-changing resizes; a vanished key resolves to
//! the nearest survivor in series order (ties earlier). Host-owned cursor in
//! via [`selection`](EnzLineChart::selection), out via
//! [`on_select`](EnzLineChart::on_select); the widget keeps no second copy.
//!
//! Accessibility: one `Role::Image` node (title, summary, and — with a cursor
//! — the datum as `value` plus 1-based `set_position` among drawn points).
//! No per-datum children (see [`semantics`](EnzLineChart::semantics)).
//!
//! `NaN`/`inf` filter through [`decimate`] and
//! [`data_domain`](enzui_core::chart::data_domain); `0.0` plots literally.

use crate::chart_geometry::{
    AxisLabel, AxisLayout, TickFormatter, Y_GUTTER, chart_description, chart_label_font,
    format_tick, plot_geometry_with, y_axis_label_reserve,
};
use enzui_core::chart::{
    ChartTableRow, DatumKey, DatumStep, DecimatedSeries, DomainPolicy, HitResult, LinearScale,
    Series, SeriesId, data_table, decimate, hit_test, locate_datum, step_datum,
};
use enzui_core::semantics::{Role as SemRole, Semantics};
use enzui_theme::Palette;
use iced::widget::canvas::{self, Cache, Frame, Path, Program, Stroke};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size};
use std::cell::Cell;

// Default plot size when the caller does not choose one.
const DEFAULT_WIDTH: f32 = 480.0;
// Default plot height when the caller does not choose one.
const DEFAULT_HEIGHT: f32 = 240.0;

/// Resolved theme tokens the chart paints with. Every field is
/// resolved from a [`Palette`] via [`Self::from_palette`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineChartTokens {
    /// Grid line colour.
    pub grid: Color,
    /// Axis line colour.
    pub axis: Color,
    /// Tick label colour. Text, so held to WCAG 1.4.3's 4.5:1
    /// contrast floor rather than the 3:1 non-text floor `axis`
    /// uses.
    pub tick_label: Color,
    /// The line stroke colour for series `0`. Later series take
    /// their colour from [`Self::series_mark`], which cycles
    /// through [`Self::series_palette`].
    pub mark: Color,
    /// Plot background fill.
    pub plot_background: Color,
    /// Focus-visible ring colour (painted by the wrapping `FocusButton` on Tab).
    pub focus_ring: Color,
    /// The hover crosshair's rule lines (the vertical/horizontal guides). A
    /// muted line, distinct from both the grid and the axes so it reads as an
    /// overlay rather than as another axis.
    pub crosshair: Color,
    /// The categorical series palette. Each series takes a distinct
    /// entry so a multi-series chart does not collapse into one
    /// tangled line.
    pub series_palette: crate::chart_palette::ChartPalette,
}

// Focus-ring width, matching `ButtonTokens::focus_ring_width`.

impl LineChartTokens {
    /// Resolve from a [`Palette`]. `border` for the grid,
    /// `border_strong` for the axes (so axes read above the grid),
    /// `accent` for the mark, `surface` for the plot field, and
    /// `text_subtle` for the hover crosshair (muted, distinct from
    /// both grid and axes). No literal colour appears in this
    /// function.
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
            series_palette: crate::chart_palette::ChartPalette::from_palette(p),
        }
    }

    /// The stroke colour for series `index`. Index `0` returns
    /// [`Self::mark`]; later indices take entries from
    /// [`Self::series_palette`], cycling after
    /// [`crate::chart_palette::PALETTE_LEN`]. Pure in `index`, so
    /// stable across frames.
    pub fn series_mark(&self, index: usize) -> Color {
        if index == 0 {
            self.mark
        } else {
            self.series_palette.series_color(index)
        }
    }
}

// Snap distance from the cursor to a datum (plot-pixel space).
// Past this the chart draws no crosshair, so an empty-space cursor
// cannot invent a hit.
const CROSSHAIR_SNAP_RADIUS: f32 = 32.0;

// Highlight marker radius on the snapped datum, in pixels.
const CROSSHAIR_MARKER_RADIUS: f32 = 3.5;

// Crosshair rule stroke width, in pixels.
const CROSSHAIR_RULE_WIDTH: f32 = 1.0;

// Keyboard selection halo (outer) and centre (inner) radii, in pixels.
const SELECTION_HALO_RADIUS: f32 = 7.0;
const SELECTION_MARKER_RADIUS: f32 = 3.5;

/// The inputs that change the painted static geometry. Equal keys
/// replay the cache; any change invalidates it. The data is carried
/// as a digest so the key is `Copy + PartialEq` and cheap to
/// compare every frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineChartKey {
    tokens: LineChartTokens,
    plot: Size,
    x_domain: (f64, f64),
    y_domain: (f64, f64),
    data_digest: u64,
    // Every field below is part of the key so a change to it
    // invalidates the cached static layer.
    label_size: f32,
    label_font: iced::Font,
    y_tick_formatter: TickFormatter,
    y_gutter: f32,
    y_axis_label: AxisLabel,
}

impl LineChartKey {
    /// The axis layout this key draws with: the requested gutter
    /// plus the y-axis-label headroom.
    #[must_use]
    pub fn axis_layout(&self) -> AxisLayout {
        AxisLayout {
            y_gutter: self.y_gutter,
            y_axis_label_reserve: y_axis_label_reserve(self.y_axis_label, self.label_size),
        }
    }
}

/// Per-instance canvas geometry cache. Held by the runtime between
/// frames; a matching [`LineChartKey`] replays stored primitives.
#[derive(Debug, Default)]
pub struct LineChartState {
    cache: Cache,
    last: Cell<Option<LineChartKey>>,
    builds: Cell<u32>,
    crosshair_cursor: Cell<Option<Point>>,
}

impl LineChartState {
    /// Number of times static geometry has actually been rebuilt on
    /// this instance. A cache-hit draw does not increment it.
    #[must_use]
    pub fn builds(&self) -> u32 {
        self.builds.get()
    }
}

/// The Canvas program painting the static layer.
#[derive(Debug, Clone)]
pub struct LineChartProgram {
    snap: crate::cartesian_chart::SeriesSnapshot,
    x_policy: DomainPolicy,
    y_policy: DomainPolicy,
    pinned_tokens: Option<LineChartTokens>,
    crosshair: bool,
    selection: Option<DatumSelection>,
    y_tick_formatter: TickFormatter,
    y_gutter: f32,
    y_axis_label: AxisLabel,
}

/// The inner plot rectangle inset by the fixed gutters, in local
/// widget coordinates (`(0, 0)` at the canvas' top-left). Same
/// definition the marks were drawn against, so hit-testing and
/// tooltip anchoring cannot drift off the datum.
#[must_use]
pub fn plot_rect(bounds: Size) -> Rectangle {
    plot_rect_with(bounds, AxisLayout::default())
}

/// [`plot_rect`] for a chart with a caller-chosen [`AxisLayout`].
/// Hit-testing and tooltip placement must pass the same layout the
/// chart was drawn with; callers holding a [`LineChartKey`] pass
/// `key.axis_layout()`.
#[must_use]
pub fn plot_rect_with(bounds: Size, axis: AxisLayout) -> Rectangle {
    // Borrow `plot_geometry`'s rect and discard the scales so the
    // gutter arithmetic lives in exactly one place.
    let geom = plot_geometry_with(bounds, (0.0, 1.0), (0.0, 1.0), axis);
    let (inner_x, inner_w, inner_h) = (geom.inner_x, geom.inner_w, geom.inner_h);
    Rectangle {
        x: inner_x,
        y: 0.0,
        width: inner_w,
        height: inner_h,
    }
}

impl LineChartProgram {
    // Resolve tokens for the draw-time theme, honouring a pinned override.
    fn tokens(&self, theme: &iced::Theme) -> LineChartTokens {
        crate::theme_lookup::resolve_pinned(
            crate::theme_lookup::PIN_SITE_LINE_CHART,
            self.pinned_tokens,
            theme,
            LineChartTokens::from_palette,
        )
    }

    // Domains with policies applied to the snapshot's raw domain.
    fn domains(&self) -> Option<((f64, f64), (f64, f64))> {
        self.snap.domains(self.x_policy, self.y_policy)
    }

    // Tick-label font size: `caption_size` (10 px in the built-in
    // scale), read from the theme rather than hard-coded.
    fn label_size(&self, theme: &iced::Theme) -> f32 {
        crate::theme_lookup::typography(theme).caption_size
    }

    /// Hit-test the chart at `local_cursor` (widget-local coordinates,
    /// `(0, 0)` at the top-left of `bounds`).
    ///
    /// Nearest original datum + widget-local pixel, or `None` when: outside
    /// the plot rect (gutters don't hit-test); no finite series; degenerate
    /// queried-axis domain. Delegates to
    /// [`hit_test`] over UN-decimated points
    /// (decimated-away data stays hittable), on [`draw_static`]'s scales (the
    /// pixel is where the mark drew). Hand the point to
    /// [`position_chart_tooltip`](crate::chart_tooltip::position_chart_tooltip)
    /// with [`plot_rect`] to anchor/flip/clamp a tooltip.
    #[must_use]
    pub fn hit_at(&self, local_cursor: Point, bounds: Size) -> Option<(HitResult, Point)> {
        let plot = plot_rect_with(bounds, self.axis_layout());
        if plot.width <= 0.0 || plot.height <= 0.0 {
            return None;
        }
        // Guard against non-finite cursors and cursors outside the plot rect —
        // gutters must not report a hit.
        if !local_cursor.x.is_finite() || !local_cursor.y.is_finite() {
            return None;
        }
        if local_cursor.x < plot.x
            || local_cursor.x > plot.x + plot.width
            || local_cursor.y < plot.y
            || local_cursor.y > plot.y + plot.height
        {
            return None;
        }

        let (x_domain, y_domain) = self.domains()?;
        // A degenerate domain along either axis has no meaningful nearest
        // datum — `LinearScale::map` collapses to a single pixel and every
        // point sits on top of every other point.
        if x_domain.0 == x_domain.1 || y_domain.0 == y_domain.1 {
            return None;
        }

        let x_scale = LinearScale {
            domain: x_domain,
            range: (f64::from(plot.x), f64::from(plot.x + plot.width)),
        };
        let y_scale = LinearScale {
            domain: y_domain,
            range: (f64::from(plot.height), 0.0),
        };

        // The decimator is fed the same [0, inner_w] range `draw_static` uses.
        // `hit_test` borrows `s`'s original points explicitly, so decimation
        // choices do not influence the hit set and the result retains no full
        // source clone.
        let dec_scale = LinearScale {
            domain: x_domain,
            range: (0.0, f64::from(plot.width)),
        };

        let px = f64::from(local_cursor.x);
        let py = f64::from(local_cursor.y);

        let mut best: Option<(HitResult, Point)> = None;
        for s in self.snap.as_slice() {
            let dec = decimate(s, plot.width.max(0.0) as u32, &dec_scale);
            let Some(h) = hit_test(&dec, s, px, py, &x_scale, &y_scale) else {
                continue;
            };
            match best {
                Some((b, _)) if b.distance_sq <= h.distance_sq => {}
                _ => {
                    let x_pix = x_scale.map(h.datum.x) as f32;
                    let y_pix = y_scale.map(h.datum.y) as f32;
                    if !x_pix.is_finite() || !y_pix.is_finite() {
                        continue;
                    }
                    best = Some((h, Point::new(x_pix, y_pix)));
                }
            }
        }
        best
    }

    // Cache key for `theme` at `plot` size.
    fn key(&self, theme: &iced::Theme, plot: Size) -> LineChartKey {
        let (x_domain, y_domain) = self.domains().unwrap_or(((0.0, 0.0), (0.0, 0.0)));
        LineChartKey {
            tokens: self.tokens(theme),
            plot,
            x_domain,
            y_domain,
            data_digest: self.snap.digest(),
            label_size: self.label_size(theme),
            label_font: chart_label_font(theme),
            y_tick_formatter: self.y_tick_formatter,
            y_gutter: self.y_gutter,
            y_axis_label: self.y_axis_label,
        }
    }

    // Axis layout used for hit-testing. Only the gutter is threaded
    // (no theme is resolved here); the vertical headroom lives in the
    // key that `draw_static` receives.
    fn axis_layout(&self) -> AxisLayout {
        AxisLayout {
            y_gutter: self.y_gutter,
            ..AxisLayout::default()
        }
    }
}

pub(crate) fn crosshair_redraw<Message>(
    previous: &Cell<Option<Point>>,
    enabled: bool,
    event: &iced::Event,
    bounds: Rectangle,
    cursor: iced::mouse::Cursor,
) -> Option<iced::widget::Action<Message>> {
    if !enabled {
        return None;
    }
    let position = match event {
        iced::Event::Mouse(iced::mouse::Event::CursorMoved { .. }) => cursor.position_in(bounds),
        iced::Event::Mouse(iced::mouse::Event::CursorLeft) => None,
        _ => return None,
    };
    (previous.replace(position) != position).then(iced::widget::Action::request_redraw)
}

impl<Message> Program<Message> for LineChartProgram {
    type State = LineChartState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: iced::mouse::Cursor,
    ) -> Option<iced::widget::Action<Message>> {
        crosshair_redraw(
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
        cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        state.crosshair_cursor.set(if self.crosshair {
            cursor.position_in(bounds)
        } else {
            None
        });
        let key = self.key(theme, bounds.size());

        // The cursor is deliberately absent from `key`; the crosshair
        // is a separate uncached layer below, so a hover cannot cost
        // a rebuild.
        if state.last.get() != Some(key) {
            state.cache.clear();
            state.last.set(Some(key));
        }

        let inner = std::rc::Rc::clone(&self.snap.inner);
        let plot = bounds.size();
        let static_geometry = state.cache.draw(renderer, plot, |frame| {
            state.builds.set(state.builds.get() + 1);
            let _ = draw_static(frame, plot, inner.series.as_slice(), key);
        });

        let mut geometries = vec![static_geometry];

        // Crosshair layer: uncached, cursor-dependent. `draw_crosshair`
        // itself rejects a cursor outside the plot rect, so this can
        // be a no-op frame.
        if let (true, Some(pos)) = (self.crosshair, cursor.position_in(bounds)) {
            let mut frame = Frame::new(renderer, plot);
            let _ = draw_crosshair(&mut frame, plot, inner.series.as_slice(), key, pos);
            geometries.push(frame.into_geometry());
        }

        // Keyboard selection layer: uncached, independent of the mouse
        // cursor. Drawn last so it stays visible over the line and
        // any pointer crosshair.
        if let Some(selection) = self.selection {
            let mut frame = Frame::new(renderer, plot);
            let _ = draw_selection(&mut frame, plot, inner.series.as_slice(), key, selection);
            geometries.push(frame.into_geometry());
        }

        geometries
    }
}

/// What one [`draw_static`] call actually emitted. Every field is
/// incremented from the values handed to the path builder, so a
/// report describes what was drawn rather than what was intended.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DrawReport {
    /// Vertices pushed into a mark path, summed over all series.
    /// Bounded by `2 * plot.width`, never by the input point count.
    pub mark_vertices: usize,
    /// Points rejected by the non-finite pixel guard. Non-zero only
    /// for a domain so wide that mapping overflows to `NaN`.
    pub non_finite_skipped: usize,
    /// Horizontal grid lines stroked.
    pub grid_lines: usize,
    /// `y` pixel of each grid line stroked, in draw order.
    /// Bit-identical to [`Self::y_tick_label_baseline_ys`]: grid
    /// lines and tick labels share one `YAxisTicks` source.
    pub grid_line_ys: Vec<f32>,
    /// Axis lines stroked.
    pub axis_lines: usize,
    /// X-axis tick labels drawn, after collision reduction.
    /// Counted at the `fill_text` call, so painted labels, not
    /// intended ones.
    pub x_tick_labels: usize,
    /// Y-axis tick labels actually drawn.
    pub y_tick_labels: usize,
    /// The exact Y-tick strings handed to `frame.fill_text`, in draw order.
    pub y_tick_label_texts: Vec<String>,
    /// The baseline `y` pixel of each y tick label, in draw order.
    /// Equals [`Self::grid_line_ys`] element-for-element.
    pub y_tick_label_baseline_ys: Vec<f32>,
    /// Series that contributed at least one stroked vertex.
    pub series_drawn: usize,
    /// Distinct colours line paths were stroked with, in first-drawn
    /// order. Recorded at the `frame.stroke` call, so
    /// `mark_colors.len()` vs `series_drawn` distinguishes two
    /// series stroked in one colour from one series with more
    /// segments.
    pub mark_colors: Vec<Color>,
    /// Exact y-axis unit label handed to `frame.fill_text`, or
    /// `None` when the chart has no axis label. Recorded at the
    /// draw call.
    pub y_axis_label_text: Option<String>,
}

// The drawn set for `series` at `plot` size: renderer and keyboard
// traversal call this — drawn and reachable are the same points. The
// decimator buckets columns `0..inner_w` on its handed scale, so hand a
// `[0, inner_w]` range — not the draw scale (`[inner_x, inner_x + inner_w]`
// gutter-offset). Callers add `inner_x` back for marks.
fn drawn_points(
    series: &Series,
    plot: Size,
    x_domain: (f64, f64),
    axis: AxisLayout,
) -> DecimatedSeries {
    let geom = plot_geometry_with(plot, x_domain, (0.0, 1.0), axis);
    decimate(series, geom.inner_w.max(0.0) as u32, &geom.column_scale())
}

// Resolve a host-controlled key to the datum the current decimation
// can show. Exact keys win; if a resize dropped the key, pick the
// surviving drawn datum nearest in original series order (ties
// earlier). A key absent from the source data resolves to `None`
// rather than an endpoint.
fn resolve_drawn_datum(
    series: &Series,
    decimated: &DecimatedSeries,
    key: DatumKey,
) -> Option<(enzui_core::chart::DecimatedPoint, usize, usize)> {
    if let Some(exact) = locate_datum(decimated, key) {
        return Some(exact);
    }

    let original_index = series.points.iter().position(|datum| datum.key == key)?;
    let nearest = decimated.points.iter().min_by_key(|point| {
        (
            point.original_index.abs_diff(original_index),
            point.original_index,
        )
    })?;
    locate_datum(decimated, nearest.key)
}

/// Paint grid, axes, tick labels and the line marks into `frame`,
/// returning what was actually emitted.
///
/// The drawable plot insets from `plot` by left `Y_GUTTER` / bottom
/// `X_GUTTER` (labels never overdraw marks): datum pixels offset by the
/// gutter from pre-label layout — mark/axis assertions state against the
/// inset rect.
pub fn draw_static(
    frame: &mut Frame,
    plot: Size,
    series: &[Series],
    key: LineChartKey,
) -> DrawReport {
    let common = crate::cartesian_chart::draw_static(
        frame,
        plot,
        series,
        crate::cartesian_chart::CartesianKey {
            tokens: crate::cartesian_chart::CartesianTokens {
                grid: key.tokens.grid,
                axis: key.tokens.axis,
                tick_label: key.tokens.tick_label,
                mark: key.tokens.mark,
                plot_background: key.tokens.plot_background,
                focus_ring: key.tokens.focus_ring,
                crosshair: key.tokens.crosshair,
                series_palette: key.tokens.series_palette,
            },
            mark: crate::cartesian_chart::ResolvedMark::Line { width: 2.0 },
            plot: key.plot,
            x_domain: key.x_domain,
            y_domain: key.y_domain,
            data_digest: key.data_digest,
            label_size: key.label_size,
            label_font: key.label_font,
            x_tick_formatter: format_tick,
            y_tick_formatter: key.y_tick_formatter,
            y_gutter: key.y_gutter,
            y_axis_label: key.y_axis_label,
            paint_plot_background: false,
            show_y_axis_line: false,
        },
    );
    DrawReport {
        mark_vertices: common.mark_vertices,
        non_finite_skipped: common.non_finite_skipped,
        grid_lines: common.grid_lines,
        grid_line_ys: common.grid_line_ys,
        axis_lines: common.axis_lines,
        x_tick_labels: common.x_tick_labels,
        y_tick_labels: common.y_tick_labels,
        y_tick_label_texts: common.y_tick_label_texts,
        y_tick_label_baseline_ys: common.y_tick_label_baseline_ys,
        series_drawn: common.series_drawn,
        mark_colors: common.mark_colors,
        y_axis_label_text: common.y_axis_label_text,
    }
}

/// What one [`draw_selection`] call painted. Fields are set from
/// the exact point handed to `Path::circle`, so a report describes
/// what was drawn.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SelectionReport {
    /// Whether the halo and marker were painted.
    pub drawn: bool,
    /// The key actually shown. This can differ from the host key after a resize
    /// drops that key; the nearest surviving drawn datum is used instead.
    pub resolved_key: Option<DatumKey>,
    /// Widget-local pixel centre of the indicator.
    pub marker: Option<Point>,
}

/// Draw the controlled keyboard datum cursor as an uncached
/// overlay: focus-ring halo and mark-coloured centre on the
/// resolved datum. If a resize dropped `selection.datum`, the
/// nearest survivor in original series order is drawn instead;
/// missing series and non-finite mapped pixels draw nothing.
pub fn draw_selection(
    frame: &mut Frame,
    plot: Size,
    series: &[Series],
    key: LineChartKey,
    selection: DatumSelection,
) -> SelectionReport {
    let mut report = SelectionReport::default();
    let Some(series) = series.iter().find(|series| series.id == selection.series) else {
        return report;
    };
    let decimated = drawn_points(series, plot, key.x_domain, key.axis_layout());
    let Some((point, _, _)) = resolve_drawn_datum(series, &decimated, selection.datum) else {
        return report;
    };

    let geometry = plot_geometry_with(plot, key.x_domain, key.y_domain, key.axis_layout());
    let x = geometry.x_scale.map(point.x);
    let y = geometry.y_scale.map(point.y);
    if !x.is_finite() || !y.is_finite() {
        return report;
    }
    let marker = Point::new(x as f32, y as f32);

    frame.fill(
        &Path::circle(marker, SELECTION_HALO_RADIUS),
        key.focus_ring_color(),
    );
    frame.fill(
        &Path::circle(marker, SELECTION_MARKER_RADIUS),
        key.mark_color(),
    );

    report.drawn = true;
    report.resolved_key = Some(point.key);
    report.marker = Some(marker);
    report
}

/// What one [`draw_crosshair`] call decided. Every field is set at
/// the point of drawing (or of the early return), so a report
/// describes what was painted. The `Default` (all-`false`/`None`)
/// is the "nothing drawn" report.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CrosshairReport {
    /// Whether a crosshair was drawn. `false` when the cursor is outside the
    /// plot area, outside the widget, or no datum is within the snap radius.
    pub drawn: bool,
    /// Pixel position of the snapped datum's marker, when `drawn`.
    /// `None` otherwise. The marker sits on the datum, not on the
    /// cursor.
    pub marker: Option<Point>,
    /// The nearest datum's original index when a hit was found,
    /// regardless of whether it was inside the snap radius.
    pub hit_index: Option<usize>,
}

/// Draw the hover crosshair as an uncached overlay: two rules and
/// a marker on the nearest original datum, only within
/// `CROSSHAIR_SNAP_RADIUS` px of it.
///
/// `drawn: false` when: outside the drawable plot; no finite datum; nearest
/// beyond snap.
pub fn draw_crosshair(
    frame: &mut Frame,
    plot: Size,
    series: &[Series],
    key: LineChartKey,
    cursor: Point,
) -> CrosshairReport {
    let mut report = CrosshairReport::default();
    if plot.width <= 0.0 || plot.height <= 0.0 {
        return report;
    }

    let geom = plot_geometry_with(plot, key.x_domain, key.y_domain, key.axis_layout());
    if geom.inner_w <= 0.0 || geom.inner_h <= 0.0 {
        return report;
    }

    // Cursor must be inside the drawable plot rect; a cursor in a
    // gutter or off the widget is not over data.
    let inside = cursor.x >= geom.inner_x
        && cursor.x <= geom.inner_x + geom.inner_w
        && cursor.y >= 0.0
        && cursor.y <= geom.inner_h;
    if !inside {
        return report;
    }

    let px = f64::from(cursor.x);
    let py = f64::from(cursor.y);

    // Nearest *original* datum across every series, in pixel space.
    // Decimation uses a column scale whose range starts at 0; the
    // gutter-offset draw scales go to `hit_test`, so the returned
    // pixel is where the mark was drawn.
    let dec_scale = geom.column_scale();
    let mut best: Option<(f64, Point, usize)> = None;
    for s in series {
        let dec = decimate(s, geom.inner_w.max(0.0) as u32, &dec_scale);
        if let Some(hit) = hit_test(&dec, s, px, py, &geom.x_scale, &geom.y_scale) {
            let mx = geom.x_scale.map(hit.datum.x);
            let my = geom.y_scale.map(hit.datum.y);
            if !mx.is_finite() || !my.is_finite() {
                continue;
            }
            let better = match &best {
                Some((d, _, _)) => hit.distance_sq < *d,
                None => true,
            };
            if better {
                best = Some((
                    hit.distance_sq,
                    Point::new(mx as f32, my as f32),
                    hit.original_index,
                ));
            }
        }
    }

    let Some((distance_sq, marker, hit_index)) = best else {
        return report;
    };
    report.hit_index = Some(hit_index);

    // Nothing within the snap radius means no crosshair. Compared
    // in squared pixels to match `hit_test`'s `distance_sq`.
    let radius_sq = f64::from(CROSSHAIR_SNAP_RADIUS) * f64::from(CROSSHAIR_SNAP_RADIUS);
    if distance_sq > radius_sq {
        return report;
    }

    // Draw the crosshair at the datum, never at the cursor.
    let rule = Stroke::default()
        .with_color(key.crosshair_color())
        .with_width(CROSSHAIR_RULE_WIDTH);

    // Vertical + horizontal rules through the datum, plus a marker
    // in the mark colour so it reads as "this point".
    frame.stroke(
        &Path::line(
            Point::new(marker.x, 0.0),
            Point::new(marker.x, geom.inner_h),
        ),
        rule,
    );
    frame.stroke(
        &Path::line(
            Point::new(geom.inner_x, marker.y),
            Point::new(geom.inner_x + geom.inner_w, marker.y),
        ),
        rule,
    );
    frame.fill(
        &Path::circle(marker, CROSSHAIR_MARKER_RADIUS),
        key.mark_color(),
    );

    report.drawn = true;
    report.marker = Some(marker);
    report
}

impl LineChartKey {
    /// Grid colour this key paints with.
    pub fn grid_color(&self) -> Color {
        self.tokens.grid
    }
    /// Axis colour this key paints with.
    pub fn axis_color(&self) -> Color {
        self.tokens.axis
    }
    /// Tick-label ink this key paints with. Text, so held to WCAG
    /// 4.5:1 — distinct from [`Self::axis_color`].
    pub fn tick_label_color(&self) -> Color {
        self.tokens.tick_label
    }
    /// Mark colour this key paints with (series 0).
    pub fn mark_color(&self) -> Color {
        self.tokens.mark
    }
    /// Stroke colour for series `index`, distinct per series and
    /// stable across frames.
    pub fn series_mark_color(&self, index: usize) -> Color {
        self.tokens.series_mark(index)
    }
    /// Plot background colour this key paints with.
    pub fn plot_background_color(&self) -> Color {
        self.tokens.plot_background
    }
    /// Crosshair rule-line colour this key paints with.
    pub fn crosshair_color(&self) -> Color {
        self.tokens.crosshair
    }
    /// Keyboard selection halo colour this key paints with.
    pub fn focus_ring_color(&self) -> Color {
        self.tokens.focus_ring
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
}

/// Where the keyboard selection cursor is: a series id and a
/// [`DatumKey`] within it. Anchored by key rather than pixel index
/// so it survives a resize that re-decimates the series. Held by
/// the host (as with a calendar's `MonthController`) and handed
/// back to the builder each frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatumSelection {
    /// Which series the cursor is in.
    pub series: SeriesId,
    /// Which datum within that series.
    pub datum: DatumKey,
}

impl DatumSelection {
    /// A cursor on `datum` of `series`.
    #[must_use]
    pub fn new(series: SeriesId, datum: DatumKey) -> Self {
        Self { series, datum }
    }
}

/// A keyboard traversal request decoded from a key press.
///
/// Two axes, because a multi-series chart is two-dimensional to a keyboard
/// user: [`DatumStep`] walks along x within one series, [`Self::SeriesStep`]
/// crosses between series at (approximately) the same x.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraversalKey {
    /// Move along x within the current series (Left/Right/Home/End).
    Datum(DatumStep),
    /// Move to the previous (`-1`, ArrowUp) or next (`+1`,
    /// ArrowDown) series, in caller push order (the legend order).
    /// Screen-position mapping is deliberately not used: two
    /// crossing series would make the same key move in opposite
    /// directions depending on where the cursor is.
    SeriesStep(i32),
}

/// The chart's key vocabulary. [`event_for_key`] derives from this table;
/// unlisted keys return `None` and keep bubbling — Tab still leaves, app
/// shortcuts still arrive.
///
/// | Key | Meaning |
/// |---|---|
/// | ArrowLeft / ArrowRight | previous / next drawn point in the current series |
/// | Home / End | first / last drawn point in the current series |
/// | ArrowUp / ArrowDown | previous / next series (multi-series charts) |
pub const TRAVERSAL_CLAIMS: &[(iced::keyboard::key::Named, TraversalKey)] = &[
    (
        iced::keyboard::key::Named::ArrowLeft,
        TraversalKey::Datum(DatumStep::Previous),
    ),
    (
        iced::keyboard::key::Named::ArrowRight,
        TraversalKey::Datum(DatumStep::Next),
    ),
    (
        iced::keyboard::key::Named::Home,
        TraversalKey::Datum(DatumStep::First),
    ),
    (
        iced::keyboard::key::Named::End,
        TraversalKey::Datum(DatumStep::Last),
    ),
    (
        iced::keyboard::key::Named::ArrowUp,
        TraversalKey::SeriesStep(-1),
    ),
    (
        iced::keyboard::key::Named::ArrowDown,
        TraversalKey::SeriesStep(1),
    ),
];

/// Decode a key press into a traversal request, or `None` for
/// keys that must bubble unchanged. Derived from
/// [`TRAVERSAL_CLAIMS`].
///
/// Bare and Shift-modified traversal keys only: Ctrl/Alt/Logo-modified
/// navigation stays app-owned (Ctrl+Home, Alt+Left, Logo+Arrow). Shift stays
/// widget-owned for future range extension.
#[must_use]
pub fn event_for_key(
    key: &iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
) -> Option<TraversalKey> {
    use iced::keyboard::Key;
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    let Key::Named(named) = key else {
        return None;
    };
    TRAVERSAL_CLAIMS
        .iter()
        .find_map(|(claim_key, action)| (*claim_key == *named).then_some(*action))
}

/// A token-driven Cartesian line chart with one tab stop and a cached static
/// layer.
pub struct EnzLineChart<Message> {
    // Immutable source snapshot, shared with the Canvas program so
    // `program()`/`key_for()` do not rescan unchanged data.
    snap: crate::cartesian_chart::SeriesSnapshot,
    width: f32,
    /// Whether the plot fills its parent's width (built at the width the
    /// layout gives it) instead of a fixed pixel width.
    fill_width: bool,
    height: f32,
    title: String,
    summary: Option<String>,
    x_policy: DomainPolicy,
    y_policy: DomainPolicy,
    tokens: Option<LineChartTokens>,
    y_tick_formatter: TickFormatter,
    y_gutter: f32,
    y_axis_label: AxisLabel,
    crosshair: bool,
    selection: Option<DatumSelection>,
    // `on_select` is a factory rather than a value: the published
    // message depends on where the cursor lands, which is only known
    // after the key is decoded against the current data.
    on_select: Option<std::rc::Rc<dyn Fn(DatumSelection) -> Message>>,
}

// Hand-written because `on_select` is a `dyn Fn`, so no `Debug`
// derive is possible.
impl<Message> std::fmt::Debug for EnzLineChart<Message> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnzLineChart")
            .field("series", &self.snap.as_slice().len())
            .field("width", &self.width)
            .field("height", &self.height)
            .field("title", &self.title)
            .field("crosshair", &self.crosshair)
            .field("selection", &self.selection)
            .field("on_select", &self.on_select.is_some())
            .finish_non_exhaustive()
    }
}

impl<Message> Clone for EnzLineChart<Message> {
    fn clone(&self) -> Self {
        Self {
            snap: self.snap.clone(),
            width: self.width,
            fill_width: self.fill_width,
            height: self.height,
            title: self.title.clone(),
            summary: self.summary.clone(),
            x_policy: self.x_policy,
            y_policy: self.y_policy,
            tokens: self.tokens,
            y_tick_formatter: self.y_tick_formatter,
            y_gutter: self.y_gutter,
            y_axis_label: self.y_axis_label,
            crosshair: self.crosshair,
            selection: self.selection,
            on_select: self.on_select.clone(),
        }
    }
}

impl<Message> EnzLineChart<Message> {
    /// Plot width: pixels, or a growing [`Length`] such as `Length::Fill` to
    /// fill the parent's width.
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        match width.into() {
            Length::Fixed(width) => {
                if width.is_finite() && width > 0.0 {
                    self.width = width;
                    self.fill_width = false;
                }
            }
            // Any growing length fills: the chart is built at the width the
            // layout hands it.
            _ => self.fill_width = true,
        }
        self
    }

    /// Plot height in pixels.
    #[must_use]
    pub fn height(mut self, height: f32) -> Self {
        if height.is_finite() && height > 0.0 {
            self.height = height;
        }
        self
    }

    /// The accessible summary announced alongside the title.
    #[must_use]
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// How the x domain is derived (default [`DomainPolicy::Automatic`]).
    #[must_use]
    pub fn x_domain(mut self, policy: DomainPolicy) -> Self {
        self.x_policy = policy;
        self
    }

    /// How the y domain is derived (default [`DomainPolicy::Automatic`]).
    #[must_use]
    pub fn y_domain(mut self, policy: DomainPolicy) -> Self {
        self.y_policy = policy;
        self
    }

    /// Override the Y-axis tick label formatter (default [`format_tick`]).
    ///
    /// The function receives both tick value and tick step, allowing unit and
    /// precision choices such as `|value, _| format!("{value:.0}%")`.
    #[must_use]
    pub fn y_tick_formatter(mut self, formatter: fn(f64, f64) -> String) -> Self {
        self.y_tick_formatter = TickFormatter::new(formatter);
        self
    }

    /// Name the y-axis unit once, instead of repeating it on every
    /// tick. Shortens every tick label and is published to
    /// assistive tech as part of the chart's description (see
    /// [`Self::semantics`]). Prefer this to widening
    /// [`Self::y_gutter`].
    #[must_use]
    pub fn y_axis_label(mut self, label: impl AsRef<str>) -> Self {
        self.y_axis_label = AxisLabel::new(label.as_ref());
        self
    }

    /// Widen or narrow the left gutter reserved for y tick labels,
    /// in pixels. Default [`crate::chart_geometry::Y_GUTTER`]
    /// (40 px). Prefer [`Self::y_axis_label`]; use this only when
    /// labels are wide after the unit is factored out. Clamped by
    /// [`crate::chart_geometry::resolve_y_gutter`] so a caller
    /// cannot reserve the plot away.
    #[must_use]
    pub fn y_gutter(mut self, gutter: f32) -> Self {
        if gutter.is_finite() && gutter >= 0.0 {
            self.y_gutter = gutter;
        }
        self
    }

    // Axis layout this chart lays out with.
    fn axis_layout(&self) -> AxisLayout {
        AxisLayout {
            y_gutter: self.y_gutter,
            ..AxisLayout::default()
        }
    }

    /// Where the keyboard selection cursor currently is. Host
    /// state (see [`DatumSelection`]); `None` (the default) means
    /// no cursor yet, and the first arrow press enters the data.
    #[must_use]
    pub fn selection(mut self, selection: Option<DatumSelection>) -> Self {
        self.selection = selection;
        self
    }

    /// Publish `f(next)` when an arrow key moves the selection
    /// cursor. Without this the arrow keys bubble untouched. The
    /// factory runs only on a real move — never during view
    /// construction and never for a key that clamped at an end.
    #[must_use]
    pub fn on_select(mut self, f: impl Fn(DatumSelection) -> Message + 'static) -> Self {
        self.on_select = Some(std::rc::Rc::new(f));
        self
    }

    /// Pin resolved tokens, bypassing draw-time theme resolution.
    #[must_use]
    pub fn tokens(mut self, tokens: LineChartTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Enable or disable the hover crosshair layer (on by default).
    /// The layer is only built while the cursor is over the widget
    /// and draws nothing outside `CROSSHAIR_SNAP_RADIUS`, so it
    /// costs nothing when the pointer is elsewhere.
    #[must_use]
    pub fn crosshair(mut self, on: bool) -> Self {
        self.crosshair = on;
        self
    }

    /// Add a series.
    #[must_use]
    pub fn push(mut self, series: Series) -> Self {
        self.snap.push(series);
        self
    }

    /// One `Role::Image` node with the required title as name and
    /// the summary as description. `Image`, not `Group`: no published
    /// children. With a cursor, the node also carries `value` (selected datum
    /// `"x, y"`), 1-based `set_position` among drawn points (ARIA), and
    /// `selected: true`. Positions resolve against the actually-drawn points
    /// at configured size: post-resize key drops follow the pixels'
    /// nearest-survivor rule.
    pub fn semantics(&self) -> Semantics {
        let s = Semantics::new(SemRole::Image).label(self.title.clone());
        // The y-axis unit rides the description beside the summary,
        // so an axis unit that exists only as pixels is still read.
        let s = match chart_description(self.summary.as_deref(), self.y_axis_label) {
            Some(d) => s.description(d),
            None => s,
        };
        let Some(cursor) = self.selection else {
            return s;
        };
        let Some(series) = self.snap.as_slice().iter().find(|x| x.id == cursor.series) else {
            return s;
        };
        let ((x_domain, _), ()) = match self.domains() {
            Some(d) => (d, ()),
            None => return s,
        };
        let dec = drawn_points(
            series,
            Size::new(self.width, self.height),
            x_domain,
            self.axis_layout(),
        );
        let Some((point, position, count)) = resolve_drawn_datum(series, &dec, cursor.datum) else {
            return s;
        };
        s.value(format!("{}, {}", point.x, point.y))
            .selected(true)
            .set_position(enzui_core::semantics::SetPosition {
                index: position,
                count,
            })
    }

    // `(x, y)` domains this chart draws with, resolved through the
    // program (and thence through core).
    fn domains(&self) -> Option<((f64, f64), (f64, f64))> {
        self.program().domains()
    }

    /// The [`DatumKey`]s this chart draws for `series` at its
    /// configured size, in x order — the same set [`draw_static`]
    /// and [`Self::traverse`] see.
    #[must_use]
    pub fn drawn_keys(&self, series: SeriesId) -> Vec<DatumKey> {
        let Some(((x_domain, _), ())) = self.domains().map(|d| (d, ())) else {
            return Vec::new();
        };
        let Some(s) = self.snap.as_slice().iter().find(|s| s.id == series) else {
            return Vec::new();
        };
        drawn_points(
            s,
            Size::new(self.width, self.height),
            x_domain,
            self.axis_layout(),
        )
        .points
        .iter()
        .map(|p| p.key)
        .collect()
    }

    /// The cursor value the host handed to [`Self::selection`],
    /// read back unchanged (a resize never rewrites it).
    #[must_use]
    pub fn selected(&self) -> Option<DatumSelection> {
        self.selection
    }

    /// Where a traversal key moves the cursor, or `None` when it
    /// cannot move (Right at the last point, an empty series,
    /// Up/Down on a one-series chart). `None` keeps the lazy
    /// `on_select` factory from running and stops a clamped key
    /// requesting a redraw of an unchanged frame.
    #[must_use]
    pub fn traverse(&self, key: TraversalKey) -> Option<DatumSelection> {
        let (x_domain, _) = self.domains()?;
        let plot = Size::new(self.width, self.height);

        match key {
            TraversalKey::Datum(step) => {
                // With no cursor, an along-x key enters the first
                // non-empty series so a chart whose first series is
                // empty is still traversable.
                let (series, current) = match self.selection {
                    Some(c) => (
                        self.snap.as_slice().iter().find(|s| s.id == c.series)?,
                        Some(c.datum),
                    ),
                    None => (
                        self.snap.as_slice().iter().find(|s| {
                            !drawn_points(s, plot, x_domain, self.axis_layout())
                                .points
                                .is_empty()
                        })?,
                        None,
                    ),
                };
                let dec = drawn_points(series, plot, x_domain, self.axis_layout());
                // A resize can drop the host key; resolve it to the
                // nearest survivor before stepping, or the key would
                // re-enter at an endpoint.
                let current = match current {
                    Some(key) => Some(resolve_drawn_datum(series, &dec, key)?.0.key),
                    None => None,
                };
                let next = step_datum(&dec, current, step)?;
                Some(DatumSelection::new(series.id, next))
            }
            TraversalKey::SeriesStep(delta) => {
                // Series order is caller push order, so the index in
                // `self.series` is the axis; ends clamp as x does.
                let from = match self.selection {
                    Some(c) => self.snap.as_slice().iter().position(|s| s.id == c.series)?,
                    // No cursor: Down enters the first series, Up the
                    // last.
                    None => {
                        let target = if delta >= 0 {
                            0
                        } else {
                            self.snap.as_slice().len().checked_sub(1)?
                        };
                        let series = self.snap.as_slice().get(target)?;
                        let dec = drawn_points(series, plot, x_domain, self.axis_layout());
                        let key = step_datum(&dec, None, DatumStep::First)?;
                        return Some(DatumSelection::new(series.id, key));
                    }
                };
                // Clamp rather than wrap.
                let to = usize::try_from(i64::try_from(from).ok()? + i64::from(delta)).ok()?;
                if to == from {
                    return None;
                }
                let series = self.snap.as_slice().get(to)?;

                // Land on the nearest drawn point to the current x in
                // the new series, so crossing series preserves the
                // reading position and never names an undrawn point.
                let dec = drawn_points(series, plot, x_domain, self.axis_layout());
                if dec.points.is_empty() {
                    return None;
                }
                let current_x = self.selection.and_then(|c| {
                    let s = self.snap.as_slice().iter().find(|s| s.id == c.series)?;
                    let drawn = drawn_points(s, plot, x_domain, self.axis_layout());
                    resolve_drawn_datum(s, &drawn, c.datum).map(|(p, _, _)| p.x)
                });
                let key = match current_x {
                    Some(x) => {
                        dec.points
                            .iter()
                            .min_by(|a, b| {
                                (a.x - x)
                                    .abs()
                                    .partial_cmp(&(b.x - x).abs())
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            })?
                            .key
                    }
                    None => dec.points[0].key,
                };
                Some(DatumSelection::new(series.id, key))
            }
        }
    }

    /// The series this chart will draw.
    #[must_use]
    pub fn series(&self) -> &[Series] {
        self.snap.as_slice()
    }

    /// The companion data table: every original datum as a
    /// [`ChartTableRow`], in series-then-datum order. Built from
    /// the original series, not the decimated subset, so no datum
    /// is missing from the accessible text representation.
    #[must_use]
    pub fn data_table(&self) -> Vec<ChartTableRow> {
        data_table(self.series())
    }

    /// The Canvas program this chart draws with.
    #[must_use]
    pub fn program(&self) -> LineChartProgram {
        // Compute metadata on the builder's snapshot so the cloned
        // snapshot in the program carries cached values.
        self.snap.ensure();
        LineChartProgram {
            snap: self.snap.clone(),
            x_policy: self.x_policy,
            y_policy: self.y_policy,
            pinned_tokens: self.tokens,
            crosshair: self.crosshair,
            y_tick_formatter: self.y_tick_formatter,
            selection: self.selection,
            y_gutter: self.y_gutter,
            y_axis_label: self.y_axis_label,
        }
    }

    /// The cache key this chart would use for `theme` at its configured size.
    #[must_use]
    pub fn key_for(&self, theme: &iced::Theme) -> LineChartKey {
        self.program()
            .key(theme, Size::new(self.width, self.height))
    }

    /// Build the element. The Canvas is wrapped in a
    /// [`FocusButton`](crate::focus_button::FocusButton) with no activation
    /// message: exactly one tab stop for the whole chart.
    ///
    /// `.focusable_without_activation(true)` + `.style_focus(…)` are both
    /// required: the former registers the message-less focusable (else
    /// keyboard-unreachable), the latter paints the ring (else the stock
    /// catalog paints none). The ring rides the wrapper's outer edge over an
    /// opaque canvas fill: the wrapper pads `FOCUS_RING_WIDTH` and grows `2
    /// *` per axis — the plot keeps requested size, the ring lands in the
    /// gap. Traversal routes via
    /// [`keyboard_capture`](crate::keyboard_capture::keyboard_capture),
    /// focused-subtree-scoped (a form chart never eats foreign typing arrows).
    /// No [`on_select`](Self::on_select), no wrap: nothing to publish, and
    /// wrapping would only capture app-owned keys.
    pub fn into_element<'a>(self) -> Element<'a, Message>
    where
        Message: 'a + Clone,
    {
        if self.fill_width {
            let height = self.height + 2.0 * enzui_theme::text_defaults::focus_ring_width();
            return iced::widget::container(iced::widget::responsive(move |size| {
                let mut chart = self.clone();
                chart.fill_width = false;
                chart.width =
                    (size.width - 2.0 * enzui_theme::text_defaults::focus_ring_width()).max(1.0);
                chart.into_element()
            }))
            .width(Length::Fill)
            .height(Length::Fixed(height))
            .into();
        }
        let semantics = self.semantics();
        let pinned = self.tokens;
        let canvas = iced::widget::canvas(self.program())
            .width(Length::Fixed(self.width))
            .height(Length::Fixed(self.height));

        let ring = enzui_theme::text_defaults::focus_ring_width();
        let stop: Element<'a, Message> =
            crate::focus_button::focus_button_maybe::<Message, iced::Theme, Renderer>(canvas, None)
                .focusable_without_activation(true)
                .padding(ring)
                .width(Length::Fixed(self.width + 2.0 * ring))
                .height(Length::Fixed(self.height + 2.0 * ring))
                .semantics(semantics)
                .style_focus(move |theme, status| {
                    use crate::focus_button::FocusStatus;
                    // Ring colour from the live theme unless the
                    // caller pinned tokens.
                    let tokens = pinned.unwrap_or_else(|| {
                        crate::theme_lookup::resolve(theme, LineChartTokens::from_palette)
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
                .into();

        let Some(on_select) = self.on_select.clone() else {
            return stop;
        };

        crate::keyboard_capture::keyboard_capture(stop, move |key, modifiers| {
            // A key that cannot move yields `None`, so no message
            // is published and the key keeps bubbling; Ctrl/Alt/Logo
            // modified keys are refused by the decoder so
            // application chords still reach the app.
            let next = self.traverse(event_for_key(key, modifiers)?)?;
            Some(on_select(next))
        })
    }
}

impl<'a, Message: 'a + Clone> From<EnzLineChart<Message>> for Element<'a, Message> {
    fn from(c: EnzLineChart<Message>) -> Self {
        c.into_element()
    }
}

/// Construct a line chart with the required accessible `title`;
/// there is no `Option`-typed alternative because every chart
/// requires a title.
#[must_use]
pub fn enz_line_chart<Message>(title: impl Into<String>) -> EnzLineChart<Message> {
    EnzLineChart {
        snap: crate::cartesian_chart::SeriesSnapshot::empty(),
        width: DEFAULT_WIDTH,
        fill_width: false,
        height: DEFAULT_HEIGHT,
        title: title.into(),
        summary: None,
        x_policy: DomainPolicy::default(),
        y_policy: DomainPolicy::default(),
        tokens: None,
        y_gutter: Y_GUTTER,
        y_axis_label: AxisLabel::default(),
        y_tick_formatter: TickFormatter::default(),
        crosshair: true,
        selection: None,
        on_select: None,
    }
}
