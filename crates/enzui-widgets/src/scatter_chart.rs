//! `EnzScatterChart`: a Canvas-based Cartesian scatter chart host.
//!
//! Sibling of [`crate::line_chart`] over the same [`enzui_core::chart`]
//! core — same [`Series`], same [`decimate`], same
//! [`crate::chart_geometry::plot_geometry`] — differing only in that each
//! surviving datum is painted as a filled dot rather than joined into a
//! polyline; a scatter dot and a line vertex over the same datum land on
//! the same pixel.
//!
//! Decimation still applies: the min/max-per-column [`decimate`] keeps the
//! extreme points of every pixel column so a scatter of 20,000 points over
//! a 260px-wide plot paints only what a reader can resolve.
//! [`DrawReport::marks`] counts dots at the draw call so the bound is
//! testable against a write-only `Frame`.
//!
//! Every colour is read from the theme via
//! [`ScatterChartTokens::from_palette`]; no colour literal appears in this
//! module. `NaN`/`inf` cannot be expressed as a dot — core's [`decimate`]
//! and [`enzui_core::chart::data_domain`] filter them and a degenerate
//! domain that maps a finite datum to a non-finite pixel is caught by an
//! explicit guard here (matching `line_chart`'s policy). `0.0` is a
//! legitimate value and plots literally.
//!
//! [`Series`]: enzui_core::chart::Series
//! [`decimate`]: enzui_core::chart::decimate

use crate::chart_geometry::{
    AxisLabel, AxisLayout, TickFormatter, Y_GUTTER, chart_description, chart_label_font,
    format_tick, y_axis_label_reserve,
};
use crate::chart_palette::ChartPalette;
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

/// Radius of a scatter dot, in pixels. Geometry, not a palette entry.
const DOT_RADIUS: f32 = 2.5;

/// Height of one decimation grid row, in pixels.
///
/// [`enzui_core::chart::decimate_grid`] keeps one dot per `(column, row)` cell, so this sets how
/// finely density is preserved vertically. `DOT_RADIUS * 2` — one dot diameter
/// — is the natural choice: two dots closer together than their own diameter
/// are not separately resolvable anyway, so a finer grid would emit ink the
/// reader cannot distinguish, and a coarser one would visibly thin a dense
/// region. Geometry, not a palette entry.
pub const GRID_CELL_HEIGHT: f32 = DOT_RADIUS * 2.0;

/// Resolved theme tokens the scatter chart paints with. Every field is read
/// from the [`Palette`]; nothing here is a literal colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScatterChartTokens {
    /// Grid line colour.
    pub grid: Color,
    /// Axis line colour.
    pub axis: Color,
    /// Tick label colour. Tick labels are text and held to the WCAG 1.4.3
    /// 4.5:1 floor, so this resolves separately from [`Self::axis`] (which
    /// answers to the 3:1 non-text floor).
    pub tick_label: Color,
    /// Dot colour for series `0`. Series `n > 0` takes its colour from
    /// [`Self::series_palette`]; see [`Self::series_mark`].
    pub mark: Color,
    /// Plot background fill.
    pub plot_background: Color,
    /// Focus-visible ring colour (painted by the wrapping `FocusButton`).
    pub focus_ring: Color,
    /// Categorical series palette resolved from the same theme, so
    /// multi-series charts render each series in a distinct colour.
    pub series_palette: ChartPalette,
}

impl ScatterChartTokens {
    /// Resolve from a [`Palette`]: `border` for the grid, `border_strong`
    /// for the axes, `text_muted` for tick labels, `accent` for the dots,
    /// `surface` for the plot field. Matches `line_chart`'s mapping.
    #[must_use]
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            grid: p.border,
            axis: p.control_border_strong,
            tick_label: p.text_muted,
            mark: p.accent,
            plot_background: p.surface,
            focus_ring: p.focus_ring,
            series_palette: ChartPalette::from_palette(p),
        }
    }

    /// Dot colour for series `index`. Index `0` returns [`Self::mark`];
    /// every later index takes its entry from [`Self::series_palette`],
    /// cycling after [`crate::chart_palette::PALETTE_LEN`]. The colour is a
    /// pure function of `index` so the cached static layer can replay.
    pub fn series_mark(&self, index: usize) -> Color {
        if index == 0 {
            self.mark
        } else {
            self.series_palette.series_color(index)
        }
    }
}

/// Inputs that change the painted static geometry: theme tokens, plot
/// size, domains, data digest, label metrics, formatter, and y-axis label.
/// A resize or data edit changes at least one field and invalidates the
/// cache; pointer moves do not (the scatter has no hover layer).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScatterChartKey {
    tokens: ScatterChartTokens,
    plot: Size,
    x_domain: (f64, f64),
    y_domain: (f64, f64),
    data_digest: u64,
    label_size: f32,
    /// resolved UI font for axis/tick labels.
    label_font: iced::Font,
    /// Y-axis formatter, included so a formatter change invalidates the cache.
    y_tick_formatter: TickFormatter,
    /// Requested left gutter; part of the key because it moves every
    /// painted pixel.
    y_gutter: f32,
    /// The painted y-axis unit label; part of the key so a unit change
    /// cannot replay a cached layer showing the old unit.
    y_axis_label: AxisLabel,
}

impl ScatterChartKey {
    /// The axis layout this key draws with.
    #[must_use]
    pub fn axis_layout(&self) -> AxisLayout {
        AxisLayout {
            y_gutter: self.y_gutter,
            y_axis_label_reserve: y_axis_label_reserve(self.y_axis_label, self.label_size),
        }
    }
}

/// Per-instance geometry cache plus a rebuild counter exposed through
/// [`Self::builds`] so callers can distinguish cache replay from rebuild.
#[derive(Debug, Default)]
pub struct ScatterChartState {
    cache: Cache,
    last: Cell<Option<ScatterChartKey>>,
    builds: Cell<u32>,
}

impl ScatterChartState {
    /// How many times static geometry has been built for this instance.
    #[must_use]
    pub fn builds(&self) -> u32 {
        self.builds.get()
    }
}

/// The Canvas program painting the scatter's static layer.
#[derive(Debug, Clone)]
pub struct ScatterChartProgram {
    /// Immutable source snapshot with precomputed digest and raw domain.
    snap: crate::cartesian_chart::SeriesSnapshot,
    x_policy: DomainPolicy,
    y_policy: DomainPolicy,
    pinned_tokens: Option<ScatterChartTokens>,
    y_tick_formatter: TickFormatter,
    y_gutter: f32,
    y_axis_label: AxisLabel,
}

impl ScatterChartProgram {
    fn tokens(&self, theme: &iced::Theme) -> ScatterChartTokens {
        crate::theme_lookup::resolve_pinned(
            crate::theme_lookup::PIN_SITE_SCATTER_CHART,
            self.pinned_tokens,
            theme,
            ScatterChartTokens::from_palette,
        )
    }

    fn domains(&self) -> Option<((f64, f64), (f64, f64))> {
        self.snap.domains(self.x_policy, self.y_policy)
    }

    fn label_size(&self, theme: &iced::Theme) -> f32 {
        crate::theme_lookup::typography(theme).caption_size
    }

    /// The cache key for `theme` at `plot` size.
    pub fn key(&self, theme: &iced::Theme, plot: Size) -> ScatterChartKey {
        let (x_domain, y_domain) = self.domains().unwrap_or(((0.0, 0.0), (0.0, 0.0)));
        ScatterChartKey {
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
}

impl<Message> Program<Message> for ScatterChartProgram {
    type State = ScatterChartState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let key = self.key(theme, bounds.size());
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
        vec![static_geometry]
    }
}

/// What one [`draw_static`] call emitted. Every field is incremented at
/// the paint call site from the same value handed to the `Frame`, so a
/// test can assert bounds on decimated draws without inspecting pixels
/// (`Frame` is write-only).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DrawReport {
    /// Dots actually filled, summed over all series. Bounded by the plot's
    /// cell count (`inner_width * rows`), never by input point count.
    pub marks: usize,
    /// Series that contributed at least one dot.
    pub series_drawn: usize,
    /// Distinct colours dots were filled with, in first-drawn order.
    /// Recorded at the fill call from the same `Color` value passed to the
    /// frame, so `mark_colors.len()` cannot drift from what was painted.
    pub mark_colors: Vec<Color>,
    /// Points rejected by the non-finite pixel guard (mapping overflow).
    pub non_finite_skipped: usize,
    /// Horizontal grid lines stroked.
    pub grid_lines: usize,
    /// The `y` pixel of each grid line stroked, in draw order. Equals
    /// [`Self::y_tick_label_baseline_ys`] element-for-element.
    pub grid_line_ys: Vec<f32>,
    /// Axis lines stroked.
    pub axis_lines: usize,
    /// X-axis tick labels actually drawn (after collision reduction).
    pub x_tick_labels: usize,
    /// Y-axis tick labels actually drawn.
    pub y_tick_labels: usize,
    /// The exact Y-tick strings handed to `frame.fill_text`, in draw order.
    pub y_tick_label_texts: Vec<String>,
    /// The baseline `y` pixel of each y tick label, in draw order.
    pub y_tick_label_baseline_ys: Vec<f32>,
    /// The y-axis unit label handed to `frame.fill_text`, or `None` when
    /// the chart has no axis label (the default).
    pub y_axis_label_text: Option<String>,
}

/// Paint grid, axes, tick labels, and scatter dots, returning what was
/// emitted.
///
/// The drawable plot is inset from `plot` by the shared gutters, so a
/// datum's pixel position is offset by the y-gutter relative to the frame;
/// mark coordinates are stated against the inset rect.
pub fn draw_static(
    frame: &mut Frame,
    plot: Size,
    series: &[Series],
    key: ScatterChartKey,
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
                crosshair: key.tokens.focus_ring,
                series_palette: key.tokens.series_palette,
            },
            mark: crate::cartesian_chart::ResolvedMark::Points {
                radius: DOT_RADIUS,
                grid_cell_height: GRID_CELL_HEIGHT,
            },
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
        marks: common.mark_vertices,
        series_drawn: common.series_drawn,
        mark_colors: common.mark_colors,
        non_finite_skipped: common.non_finite_skipped,
        grid_lines: common.grid_lines,
        grid_line_ys: common.grid_line_ys,
        axis_lines: common.axis_lines,
        x_tick_labels: common.x_tick_labels,
        y_tick_labels: common.y_tick_labels,
        y_tick_label_texts: common.y_tick_label_texts,
        y_tick_label_baseline_ys: common.y_tick_label_baseline_ys,
        y_axis_label_text: common.y_axis_label_text,
    }
}

impl ScatterChartKey {
    /// Grid colour this key paints with.
    pub fn grid_color(&self) -> Color {
        self.tokens.grid
    }
    /// Axis colour this key paints with.
    pub fn axis_color(&self) -> Color {
        self.tokens.axis
    }
    /// Tick-label ink this key paints with; text, so distinct from
    /// [`Self::axis_color`] which answers to the non-text 3:1 floor.
    pub fn tick_label_color(&self) -> Color {
        self.tokens.tick_label
    }
    /// Dot colour this key paints with (series 0).
    pub fn mark_color(&self) -> Color {
        self.tokens.mark
    }
    /// The dot colour for series `index` — distinct per series, and stable
    /// across frames because it is a pure function of the index.
    pub fn series_mark_color(&self, index: usize) -> Color {
        self.tokens.series_mark(index)
    }
    /// Plot background colour this key paints with.
    pub fn plot_background_color(&self) -> Color {
        self.tokens.plot_background
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

/// A token-driven Cartesian scatter chart with one tab stop and a cached static
/// layer.
#[derive(Debug, Clone)]
pub struct EnzScatterChart {
    // Immutable source snapshot shared with the Canvas program by
    // `Rc::clone` and rebuilt on `push`.
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
    tokens: Option<ScatterChartTokens>,
    y_tick_formatter: TickFormatter,
    y_gutter: f32,
    y_axis_label: AxisLabel,
}

impl EnzScatterChart {
    /// Plot width: pixels, or a growing [`Length`] such as `Length::Fill` to
    /// fill the parent's width. A non-finite or non-positive pixel width is
    /// refused, leaving the previous value; otherwise every mapped pixel is NaN.
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

    /// Pin resolved tokens, bypassing the draw-time theme. For tests and for
    /// callers that already resolved a palette.
    #[must_use]
    pub fn tokens(mut self, tokens: ScatterChartTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Add a series.
    #[must_use]
    pub fn push(mut self, series: Series) -> Self {
        self.snap.push(series);
        self
    }

    /// Semantics node this chart publishes: one `Role::Image` carrying the
    /// title as its label. When a summary or [`Self::y_axis_label`] is set,
    /// they are combined into the node's description.
    pub fn semantics(&self) -> Semantics {
        let s = Semantics::new(SemRole::Image).label(self.title.clone());
        match chart_description(self.summary.as_deref(), self.y_axis_label) {
            Some(d) => s.description(d),
            None => s,
        }
    }

    /// Name the y-axis unit once instead of repeating it on every tick.
    /// Prefer this to [`Self::y_gutter`]: it shortens the labels rather than
    /// shrinking the plot, and the unit is announced to assistive tech.
    #[must_use]
    pub fn y_axis_label(mut self, label: impl AsRef<str>) -> Self {
        self.y_axis_label = AxisLabel::new(label.as_ref());
        self
    }

    /// Widen the left gutter reserved for y tick labels, in pixels (default
    /// 40). Clamped by
    /// [`crate::chart_geometry::resolve_y_gutter`] so a
    /// caller cannot reserve the plot away.
    #[must_use]
    pub fn y_gutter(mut self, gutter: f32) -> Self {
        if gutter.is_finite() && gutter >= 0.0 {
            self.y_gutter = gutter;
        }
        self
    }

    /// The series this chart will draw.
    #[must_use]
    pub fn series(&self) -> &[Series] {
        self.snap.as_slice()
    }

    /// Companion data table listing every original datum as a
    /// [`ChartTableRow`]. Built from the original series, so it still lists
    /// every point after
    /// [`enzui_core::chart::decimate_grid`] drops per-cell duplicates.
    #[must_use]
    pub fn data_table(&self) -> Vec<ChartTableRow> {
        data_table(self.series())
    }

    /// The Canvas program this chart draws with.
    #[must_use]
    pub fn program(&self) -> ScatterChartProgram {
        self.snap.ensure();
        ScatterChartProgram {
            snap: self.snap.clone(),
            x_policy: self.x_policy,
            y_policy: self.y_policy,
            pinned_tokens: self.tokens,
            y_tick_formatter: self.y_tick_formatter,
            y_gutter: self.y_gutter,
            y_axis_label: self.y_axis_label,
        }
    }

    /// The cache key this chart would use for `theme` at its configured size.
    #[must_use]
    pub fn key_for(&self, theme: &iced::Theme) -> ScatterChartKey {
        self.program()
            .key(theme, Size::new(self.width, self.height))
    }

    /// Build the element. The Canvas is wrapped in a
    /// [`crate::focus_button::FocusButton`] with no activation message so
    /// the chart is one tab stop total (not one per datum) and paints a
    /// focus ring when focused.
    pub fn into_element<'a, Message: 'a + Clone>(self) -> Element<'a, Message> {
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
        crate::focus_button::focus_button_maybe::<Message, iced::Theme, Renderer>(canvas, None)
            .focusable_without_activation(true)
            .padding(ring)
            .width(Length::Fixed(self.width + 2.0 * ring))
            .height(Length::Fixed(self.height + 2.0 * ring))
            .semantics(semantics)
            .style_focus(move |theme, status| {
                use crate::focus_button::FocusStatus;
                let tokens = pinned.unwrap_or_else(|| {
                    crate::theme_lookup::resolve(theme, ScatterChartTokens::from_palette)
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

impl<'a, Message: 'a + Clone> From<EnzScatterChart> for Element<'a, Message> {
    fn from(c: EnzScatterChart) -> Self {
        c.into_element()
    }
}

/// Construct a scatter chart with the accessible `title` as its
/// [`Role::Image`](enzui_core::semantics::Role::Image) label. The title is required so a chart cannot ship
/// as an unnamed graphic.
#[must_use]
pub fn enz_scatter_chart(title: impl Into<String>) -> EnzScatterChart {
    EnzScatterChart {
        snap: crate::cartesian_chart::SeriesSnapshot::empty(),
        width: DEFAULT_WIDTH,
        fill_width: false,
        height: DEFAULT_HEIGHT,
        title: title.into(),
        summary: None,
        x_policy: DomainPolicy::default(),
        y_policy: DomainPolicy::default(),
        tokens: None,
        y_tick_formatter: TickFormatter::default(),
        y_gutter: Y_GUTTER,
        y_axis_label: AxisLabel::default(),
    }
}
