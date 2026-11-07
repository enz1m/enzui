//! `EnzBarChart`: a Canvas-based vertical bar chart host.
//!
//! # What this is
//!
//! This widget makes [`enzui_core::chart::BandScale`] reach the screen.
//! The categorical x
//! axis — a bar's pixel position and width both come from the scale, which
//! is precisely what a `LinearScale` cannot express and what `BandScale` exists
//! for.
//!
//! Everything categorical (band placement, band width, inner/outer padding) is
//! [`BandScale`]'s. The value domain applies its [`DomainPolicy`] and widens
//! to whole y ticks ([`enzui_core::chart::nice_extent`]); the finite extent is
//! folded directly from borrowed bar values so cache-key calculation does not
//! allocate a temporary series.
//!
//! # Deliberately narrow
//!
//! Vertical bars only, with labelled values and optional grouped or stacked
//! series. There is no horizontal orientation or animation, and the widget does
//! not expose dead flags for unsupported features. The reusable part is
//! the host structure (token resolution, cache key, semantics, one tab stop),
//! mirroring [`crate::line_chart`].
//!
//! # The reversed y range is the case that matters
//!
//! A vertical bar chart maps *value* to a y pixel that grows downward, so
//! the value scale's range is reversed (`range.0 > range.1`) and the largest
//! value's bar top has the smallest y. Get the reversal wrong and the chart
//! is upside down while every unit test on the scales still passes — so the
//! render tests in `tests/bar_chart_render.rs` assert bar-top ordering, not
//! just that bars exist.
//!
//! # One tab stop, tokens not literals, cached static geometry
//!
//! Same three properties the line chart established: the whole chart is exactly
//! one focusable stop, every colour comes from the [`Palette`],
//! and static geometry is cached by [`BarChartKey`] so a hover rebuilds
//! nothing.

use crate::chart_geometry::{
    AxisLabel, AxisLayout, TickFormatter, chart_description, chart_label_font, draw_y_axis_label,
    draw_y_grid, draw_y_tick_labels, resolve_y_gutter, value_axis_domain, y_axis_label_reserve,
    y_axis_ticks,
};
use crate::chart_palette::ChartPalette;
use enzui_core::chart::{BandScale, DatumKey, DomainPolicy, LinearScale};
use enzui_core::semantics::{Role as SemRole, Semantics};
use enzui_theme::Palette;
use iced::widget::canvas::{self, Cache, Frame, Path, Program, Stroke, Text};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size};
use std::{cell::Cell, rc::Rc};

/// Default plot size when the caller does not choose one.
const DEFAULT_WIDTH: f32 = 480.0;
/// Default plot height when the caller does not choose one.
const DEFAULT_HEIGHT: f32 = 240.0;

/// Default left gutter reserved for y-axis tick labels, in pixels.
///
/// Re-exported, not re-declared. One shared value keeps the bar and line
/// chart gutters aligned. A caller widens it per chart with [`EnzBarChart::y_gutter`].
const Y_GUTTER: f32 = crate::chart_geometry::Y_GUTTER;
/// Bottom gutter reserved for x-axis category labels, in pixels.
const X_GUTTER: f32 = 16.0;
/// Top gutter above the value scale so the domain-maximum y tick keeps its
/// whole centre-aligned label. Shared Cartesian value across chart families;
/// the value scale's top is `TOP_GUTTER`, not `0`.
const TOP_GUTTER: f32 = crate::chart_geometry::TOP_GUTTER;

/// Inner padding (gap between bars) as a fraction of the band step. `0.2` is
/// the conventional d3 default — bars clearly separated but not skeletal. This
/// is handed to [`BandScale::padding_inner`]; a value of `0.0` makes bars
/// touch.
const BAR_PADDING_INNER: f64 = 0.2;
/// Outer padding (padding at each edge) as a fraction of the band step.
const BAR_PADDING_OUTER: f64 = 0.1;

/// Inner padding (gap between the sub-bars *within* one grouped category) as a
/// fraction of the inner band step. Smaller than [`BAR_PADDING_INNER`] because
/// bars in one group read as a set — a wide gap would fragment the group into
/// unrelated bars. `0.08` keeps them visibly distinct but grouped.
const GROUP_INNER_PADDING: f64 = 0.08;

// ---------------------------------------------------------------------------
// Multi-series mode: grouped & stacked vertical bars
// ---------------------------------------------------------------------------

/// How a multi-series bar chart lays its series out within each category.
///
/// A single-series chart (built with [`EnzBarChart::bar`] alone) ignores this —
/// it always renders as one bar per category. The mode only takes effect once
/// the caller adds a [`BarSeries`], and it decides both the geometry *and*
/// the value-domain accumulation, which differ fundamentally:
///
/// * [`Grouped`](BarMode::Grouped) subdivides each category's band with an
///   inner [`BandScale`] over `series.len()`, so the series sit side by
///   side. The value domain is the maximum single value — each bar must
///   fit the plot on its own.
/// * [`Stacked`](BarMode::Stacked) draws each series' segment atop the running
///   sum for its category. The value domain is the **cumulative sum per
///   category** — a stack must fit the plot as a whole. Positive and negative
///   contributions stack in opposite directions from the baseline (see
///   [`stacked_domain`]).
///
/// Using the grouped domain for a stacked chart overflows every stack; using
/// the stacked domain for a grouped chart squashes everything. The mode
/// selects the domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BarMode {
    /// Series sit side by side within each category (an inner band scale).
    #[default]
    Grouped,
    /// Series stack atop one another; positive up, negative down.
    Stacked,
}

/// One named series in a multi-series ([`Grouped`](BarMode::Grouped) or
/// [`Stacked`](BarMode::Stacked)) bar chart.
///
/// The `values` are aligned positionally with the chart's categories (the
/// labels supplied via [`EnzBarChart::bar`] / [`EnzBarChart::category`]): the
/// `i`th value belongs to the `i`th category. A series with fewer values than
/// there are categories contributes nothing to the missing tail categories; a
/// series with more values has its surplus ignored (there is no category to
/// place it in).
#[derive(Debug, Clone, PartialEq)]
pub struct BarSeries {
    /// The series' accessible name (legend / data table). Not drawn on the plot.
    pub name: String,
    /// Per-category values, index-aligned with the chart's categories.
    pub values: Vec<f64>,
}

impl BarSeries {
    /// Build a named series from an iterator of values.
    #[must_use]
    pub fn new(name: impl Into<String>, values: impl IntoIterator<Item = f64>) -> Self {
        Self {
            name: name.into(),
            values: values.into_iter().collect(),
        }
    }

    /// The value for category `i`, or `None` when the series is shorter than the
    /// category list.
    #[must_use]
    pub fn value(&self, i: usize) -> Option<f64> {
        self.values.get(i).copied()
    }
}

/// The stable identity of one datum in a multi-series bar chart:
/// `(category_index, series_index)`.
///
/// A grouped or stacked chart maps a pixel to (category, series), not just
/// a category, so hit testing must carry both. Two series can hold identical
/// values at the same category, so geometry is *not* identity — this key is.
/// Packed into the low 32 bits (series) and next 32 bits (category) of a
/// [`DatumKey`] so it round-trips through the core identity type unchanged.
#[must_use]
pub fn datum_key(category: usize, series: usize) -> DatumKey {
    DatumKey(((category as u64) << 32) | (series as u64 & 0xffff_ffff))
}

/// Inverse of [`datum_key`]: `(category, series)`.
#[must_use]
pub fn datum_key_parts(k: DatumKey) -> (usize, usize) {
    ((k.0 >> 32) as usize, (k.0 & 0xffff_ffff) as usize)
}

/// The value domain for a grouped chart: the extent of every *single*
/// finite value across all series, extended per `policy`.
///
/// Each bar in a grouped chart stands on its own, so the domain must contain
/// the largest (and smallest) individual value — not any sum. Returns `None`
/// when no series holds a finite value.
#[must_use]
pub fn grouped_domain(series: &[BarSeries], policy: DomainPolicy) -> Option<(f64, f64)> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut any = false;
    for s in series {
        for &v in &s.values {
            if v.is_finite() {
                any = true;
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
    }
    any.then(|| value_axis_domain((lo, hi), policy))
}

/// The value domain for a stacked chart: the extent of the per-category
/// cumulative sums, extended per `policy`.
///
/// # Negative stacking policy
///
/// A single running sum is wrong the instant a series goes negative: a `+30`
/// atop a `-10` would report a stack height of `20`, hiding that the stack
/// spans `[-10, +30]` = 40 pixels. So positive and negative contributions are
/// accumulated separately per category:
///
/// * positive values add to a `pos` running total (the top of the upward
///   stack), and
/// * negative values add to a `neg` running total (the bottom of the downward
///   stack).
///
/// The domain is then `(min over categories of neg, max over categories of
/// pos)`, so a category that stacks `[-30, +45]` contributes `-30` to the floor
/// and `+45` to the ceiling. This matches d3's `stackOffsetDiverging`: the
/// baseline stays at zero and each sign grows away from it. Because the extent
/// always spans zero here (a `pos` accumulator starts at 0, a `neg` at 0),
/// [`DomainPolicy::IncludeZero`] is a no-op — but it is applied for consistency
/// with the grouped path.
#[must_use]
pub fn stacked_domain(
    series: &[BarSeries],
    n_categories: usize,
    policy: DomainPolicy,
) -> Option<(f64, f64)> {
    let mut floor = 0.0_f64;
    let mut ceil = 0.0_f64;
    let mut any = false;
    for cat in 0..n_categories {
        let mut pos = 0.0_f64;
        let mut neg = 0.0_f64;
        for s in series {
            if let Some(v) = s.value(cat)
                && v.is_finite()
            {
                any = true;
                if v >= 0.0 {
                    pos += v;
                } else {
                    neg += v;
                }
            }
        }
        ceil = ceil.max(pos);
        floor = floor.min(neg);
    }
    any.then(|| value_axis_domain((floor, ceil), policy))
}

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

/// Resolved theme tokens the chart paints with.
///
/// Every field is read from the [`Palette`]; nothing here is a literal colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarChartTokens {
    /// Grid line colour.
    pub grid: Color,
    /// Axis line colour.
    pub axis: Color,
    /// Tick label colour.
    ///
    /// Tick labels are text, held to WCAG 1.4.3's 4.5:1 rather than the 3:1
    /// non-text floor: `text_muted` measures 7.15 dark / 5.25 light on
    /// `surface`.
    pub tick_label: Color,
    /// The bar fill colour.
    pub bar: Color,
    /// Plot background fill; transparent by default, so the bars sit on the
    /// host surface.
    pub plot_background: Color,
    /// Focus-visible ring colour (painted by the wrapping `FocusButton`).
    pub focus_ring: Color,
}

impl BarChartTokens {
    /// Resolve from a [`Palette`].
    ///
    /// `border` for the grid and `border_strong` for the axes gives the axes
    /// visible priority over the grid in both presets; `accent` is the bar
    /// fill; the plot field is transparent. No other literal colour appears
    /// here, so a theme that changes any of those three moves the chart with
    /// it.
    #[must_use]
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            grid: p.border,
            // Axis lines are non-text UI (3:1 floor);
            // `border_strong` measured 2.29 dark / 1.78 light on the
            // plot background.
            axis: p.control_border_strong,
            tick_label: p.text_muted,
            bar: p.accent,
            plot_background: Color::TRANSPARENT,
            focus_ring: p.focus_ring,
        }
    }
}

// ---------------------------------------------------------------------------
// Cache key
// ---------------------------------------------------------------------------

/// The inputs that change the painted static geometry.
///
/// A pointer move changes none of it, which is what makes the hover-no-rebuild
/// property hold rather than merely be asserted. The data is a digest, so
/// the key stays `Copy + PartialEq` and cheap to compare each frame.
///
/// Both the value series and the label strings are folded into
/// `Self::data_digest`. Labels are painted text; a label-only update leaves
/// every value bit unchanged, so a key that keyed only on values would replay
/// stale cached text. The PieChart overlay has the same cache-key invariant:
/// omitting an input makes a stale cache indistinguishable from one that
/// genuinely re-rendered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarChartKey {
    tokens: BarChartTokens,
    plot: Size,
    /// Number of categories (bars). Part of the key because the band layout is
    /// a function of the count.
    n: usize,
    /// Value-axis domain (after policy).
    y_domain: (f64, f64),
    /// Digest of both the values *and* the labels. Splitting these into two
    /// fields would let a caller pass one and forget the other; folding them
    /// into one digest at the [`digest`] call site makes omitting labels
    /// impossible without editing that call.
    data_digest: u64,
    label_size: f32,
    /// The resolved UI font for axis/tick labels. Carried on the key so
    /// a platform font-preference change invalidates the cache.
    label_font: iced::Font,
    /// The layout mode. Grouped and stacked produce entirely
    /// different pixels from the *same* data, so it must invalidate the cache.
    mode: BarMode,
    /// Number of extra (non-primary) series. Zero for the single-series path.
    /// Part of the key because the inner band layout is a function of the
    /// series count, exactly as `n` is of the category count.
    series_count: usize,
    /// Digest of every extra series' name and values — a series-only edit
    /// (rename, one changed value) must invalidate the cache just as a bar
    /// edit does.
    series_digest: u64,
    /// Per-series colours (multi-series only). `None` for single-series.
    palette: Option<ChartPalette>,
    /// Y-axis formatter, included so a formatter change invalidates the cache.
    y_tick_formatter: TickFormatter,
    /// Requested left gutter; part of the key because it moves every
    /// painted bar.
    y_gutter: f32,
    /// The painted y-axis unit label; part of the key so a unit change
    /// cannot replay a cached layer showing the old unit.
    y_axis_label: AxisLabel,
}

impl BarChartKey {
    /// The axis layout this key draws with.
    #[must_use]
    pub fn axis_layout(&self) -> AxisLayout {
        AxisLayout {
            y_gutter: self.y_gutter,
            y_axis_label_reserve: y_axis_label_reserve(self.y_axis_label, self.label_size),
        }
    }
}

/// A cheap structural digest of the (single) series.
///
/// Folds the category count, every finite value's bit pattern, **and every
/// label's bytes** — labels are drawn text and a label change with unchanged
/// values must invalidate the static-geometry cache. `to_bits` rather than the
/// value, because `f64` is not `Hash` and a `-0.0`/`0.0` swap is a visual
/// no-op that should not invalidate.
///
/// The label length is mixed *before* its bytes so `"ab"+"c"` and `"a"+"bc"`
/// hash differently, and a per-label separator prevents cross-label byte runs
/// from colliding when two labels straddle a boundary.
fn digest(bars: &[Bar]) -> u64 {
    crate::cartesian_chart::record_source_scan(bars.len() as u64);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(0x1000_0000_01b3);
    };
    mix(bars.len() as u64);
    for b in bars {
        // Normalise -0.0 to 0.0 so a sign-of-zero change is not a rebuild.
        mix((b.value + 0.0).to_bits());
        // Length first so `["ab","c"]` and `["a","bc"]` produce distinct
        // digests despite identical concatenated bytes.
        mix(b.label.len() as u64);
        // Fold label bytes in 8-byte chunks. FNV-style diffusion is enough
        // here: this is a cache-invalidation digest, not a security hash.
        let bytes = b.label.as_bytes();
        let mut i = 0;
        while i + 8 <= bytes.len() {
            let chunk = u64::from_le_bytes(
                bytes[i..i + 8]
                    .try_into()
                    .expect("the range is the array's width"),
            );
            mix(chunk);
            i += 8;
        }
        if i < bytes.len() {
            let mut tail = [0u8; 8];
            tail[..bytes.len() - i].copy_from_slice(&bytes[i..]);
            mix(u64::from_le_bytes(tail));
        }
        // Per-label separator so adjacent labels do not merge in the digest
        // stream (`["abc","def"]` vs `["abcdef",""]`).
        mix(0xdead_beef_dead_beef);
    }
    h
}

/// A structural digest of the extra (non-primary) series, folding every
/// series' name bytes and its values.
///
/// A series-only edit — renaming a series, changing one value in it — must
/// invalidate the static-geometry cache exactly as a bar edit does, or the
/// previous frame's segments are replayed (same class as the label-omission
/// bug in [`digest`]). Returns `0` for the single-series path so a chart with
/// no extra series keeps a stable, cheap key.
fn series_digest(series: &[BarSeries]) -> u64 {
    if series.is_empty() {
        return 0;
    }
    let total: u64 = series.iter().map(|s| s.values.len() as u64).sum();
    crate::cartesian_chart::record_source_scan(total);
    let mut h: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut mix = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(0x1000_0000_01b3);
    };
    mix(series.len() as u64);
    for s in series {
        // Name length first so a boundary shift is observable, then bytes.
        mix(s.name.len() as u64);
        for chunk in s.name.as_bytes().chunks(8) {
            let mut tail = [0u8; 8];
            tail[..chunk.len()].copy_from_slice(chunk);
            mix(u64::from_le_bytes(tail));
        }
        mix(0xa5a5_a5a5_a5a5_a5a5);
        mix(s.values.len() as u64);
        for &v in &s.values {
            // Normalise -0.0 so a sign-of-zero change is not a rebuild.
            mix((v + 0.0).to_bits());
        }
        mix(0x5a5a_5a5a_5a5a_5a5a);
    }
    h
}

// ---------------------------------------------------------------------------
// Canvas program
// ---------------------------------------------------------------------------

/// Per-instance geometry cache plus a rebuild counter.
///
/// The counter is the only way a test can distinguish "the cache replayed" from
/// "the cache rebuilt and produced identical pixels".
#[derive(Debug, Default)]
pub struct BarChartState {
    cache: Cache,
    last: Cell<Option<BarChartKey>>,
    builds: Cell<u32>,
}

impl BarChartState {
    /// How many times static geometry has been built for this instance.
    #[must_use]
    pub fn builds(&self) -> u32 {
        self.builds.get()
    }
}

/// One category's value, in domain order (never sorted).
///
/// Public because it appears in [`draw_static`]'s signature, which tests call
/// directly; construct one via [`EnzBarChart::bar`] in normal use.
#[derive(Debug, Clone, PartialEq)]
pub struct Bar {
    /// The category label, drawn centred under the band.
    pub label: String,
    /// The category's value, mapped through the reversed value scale.
    pub value: f64,
}

/// One row of a bar chart's companion data table.
///
/// A bar's identity is its category, so a row is `(index, label, value)` — the
/// analogue of [`enzui_core::chart::ChartTableRow`] for the categorical
/// (non-point-series) bar wrapper. See [`EnzBarChart::data_table`].
#[derive(Debug, Clone, PartialEq)]
pub struct BarTableRow {
    /// Category index in draw order (`0`-based).
    pub index: usize,
    /// The category label.
    pub label: String,
    /// The category's value, stated verbatim (may be non-finite).
    pub value: f64,
    /// Series index this value belongs to. `0` for single-series and for the
    /// primary series of a multi-series chart.
    pub series: usize,
    /// The series' name (empty for single-series and for the primary series,
    /// which has no separate name).
    pub series_name: String,
}

/// The result of a multi-series bar-chart hit test: the original
/// `(category, series)` datum whose painted rectangle was clicked.
///
/// The `key` is [`datum_key(category, series)`](datum_key) — stable identity,
/// not geometry, so two series with identical values remain distinguishable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarHit {
    /// Category index (outer band), in domain order.
    pub category: usize,
    /// Series index. `0` is the primary series.
    pub series: usize,
    /// Stable identity of the clicked datum.
    pub key: DatumKey,
}

/// The Canvas program painting the static layer.
#[derive(Debug, Clone)]
pub struct BarChartProgram {
    bars: Rc<BarSnapshotInner>,
    mode: BarMode,
    pinned_tokens: Option<BarChartTokens>,
    palette: Option<ChartPalette>,
    y_tick_formatter: TickFormatter,
    y_gutter: f32,
    y_axis_label: AxisLabel,
    cached_bar_digest: u64,
    cached_series_digest: u64,
    /// Precomputed value-axis domain; avoids rescanning every draw.
    cached_y_domain: Option<(f64, f64)>,
    /// The keyboard cursor, outlined over the cached bars.
    selection: Option<BarSelection>,
}

impl BarChartProgram {
    /// Resolve tokens for the draw-time theme (or the pinned override).
    fn tokens(&self, theme: &iced::Theme) -> BarChartTokens {
        crate::theme_lookup::resolve_pinned(
            crate::theme_lookup::PIN_SITE_BAR_CHART,
            self.pinned_tokens,
            theme,
            BarChartTokens::from_palette,
        )
    }

    /// The primary series (the `bar()` values, series 0) followed by the extra
    /// series, aligned to the category count. The primary series is series
    /// index `0`; a caller who used only [`EnzBarChart::category`] with no
    /// primary value leaves the leading series at zero for each category.
    fn all_series(&self) -> Vec<BarSeries> {
        let mut out = Vec::with_capacity(self.bars.series.len() + 1);
        out.push(BarSeries {
            name: String::new(),
            values: self.bars.bars.iter().map(|b| b.value).collect(),
        });
        out.extend(self.bars.series.iter().cloned());
        out
    }

    /// The painted rectangle of the bar `selection` names.
    fn selected_rect(
        &self,
        renderer: &Renderer,
        plot: Size,
        key: BarChartKey,
        selection: BarSelection,
    ) -> Option<Rectangle> {
        let mut scratch = Frame::new(renderer, plot);
        let geom = if self.bars.series.is_empty() {
            let report = draw_static(&mut scratch, plot, &self.bars.bars, key);
            if selection.series != 0 {
                return None;
            }
            *report.bars.get(selection.category)?
        } else {
            let labels: Vec<String> = self.bars.bars.iter().map(|b| b.label.clone()).collect();
            let report = draw_multi_static(&mut scratch, plot, &labels, &self.all_series(), key);
            report
                .segments
                .iter()
                .find(|segment| {
                    segment.category == selection.category && segment.series == selection.series
                })?
                .geom
        };
        Some(Rectangle::new(
            Point::new(geom.x_start, geom.top),
            Size::new(geom.width, geom.bottom - geom.top),
        ))
    }

    fn label_size(&self, theme: &iced::Theme) -> f32 {
        crate::theme_lookup::typography(theme).caption_size
    }

    /// The cache key for `theme` at `plot` size.
    ///
    /// Uses precomputed digest, series digest and y-domain from construction
    /// time rather than rescanning every source point.
    fn key(&self, theme: &iced::Theme, plot: Size) -> BarChartKey {
        let y_domain = self.cached_y_domain.unwrap_or((0.0, 0.0));
        BarChartKey {
            tokens: self.tokens(theme),
            plot,
            n: self.bars.bars.len(),
            y_domain,
            data_digest: self.cached_bar_digest,
            label_size: self.label_size(theme),
            label_font: chart_label_font(theme),
            mode: self.mode,
            series_count: self.bars.series.len(),
            series_digest: self.cached_series_digest,
            palette: if self.bars.series.is_empty() {
                None
            } else {
                Some(self.palette.unwrap_or_else(|| {
                    crate::theme_lookup::resolve(theme, ChartPalette::from_palette)
                }))
            },
            y_tick_formatter: self.y_tick_formatter,
            y_gutter: self.y_gutter,
            y_axis_label: self.y_axis_label,
        }
    }
}

impl<Message> Program<Message> for BarChartProgram {
    type State = BarChartState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let key = self.key(theme, bounds.size());

        // Only the *visible* inputs invalidate. A pointer move reaches this
        // function but changes no field of `key`.
        if state.last.get() != Some(key) {
            state.cache.clear();
            state.last.set(Some(key));
        }

        // `draw` runs on every frame, including cache hits where the closure
        // below is never called. Clone only the shared handle here (a refcount
        // bump); the owned working copies the multi-series path needs
        // (`all_series`, the per-category `labels`) are derived inside the
        // closure so a cache-hit frame allocates O(1), not O(bars).
        let inner = Rc::clone(&self.bars);
        let geometry = state.cache.draw(renderer, bounds.size(), |frame| {
            state.builds.set(state.builds.get() + 1);
            if inner.series.is_empty() {
                let _ = draw_static(frame, bounds.size(), &inner.bars, key);
            } else {
                let all_series = self.all_series();
                let labels: Vec<String> = inner.bars.iter().map(|b| b.label.clone()).collect();
                let _ = draw_multi_static(frame, bounds.size(), &labels, &all_series, key);
            }
        });
        let mut layers = vec![geometry];
        // The keyboard cursor is drawn uncached over the bars: the bar's
        // rectangle comes from the same geometry pass, run on a scratch frame.
        if let Some(rect) = self
            .selection
            .and_then(|selection| self.selected_rect(renderer, bounds.size(), key, selection))
        {
            let mut overlay = Frame::new(renderer, bounds.size());
            let ring = 2.0;
            overlay.stroke(
                &Path::rounded_rectangle(
                    Point::new(rect.x - ring, rect.y - ring),
                    Size::new(rect.width + 2.0 * ring, rect.height + 2.0 * ring),
                    iced::border::radius(BAR_CORNER_RADIUS.min(rect.width / 4.0) + ring),
                ),
                Stroke::default()
                    .with_color(key.tokens.focus_ring)
                    .with_width(ring),
            );
            layers.push(overlay.into_geometry());
        }
        layers
    }
}

/// The pixel geometry of one painted bar, reported back for testing.
///
/// A [`Frame`] is write-only, so the only way to pin bar geometry is to count
/// it at the point of drawing. Every field below is captured from the same
/// rectangle handed to `frame.fill`, so it cannot drift from what was painted
/// (the stored-field trap).
///
/// # Negative and mixed bars
///
/// A bar for a negative value grows downward from the baseline (`bottom >
/// baseline`), and its geometry has `top == baseline`. A positive bar grows
/// upward (`top < baseline`, `bottom == baseline`). [`Self::height`] is the
/// literal drawn-rect height `bottom - top`, correct for both directions — a
/// hit-test expecting `height > 0` must not drop negative bars as invisible.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarGeometry {
    /// Leading pixel edge of the bar in domain order (the band's `start`,
    /// mapped to the inner rect). For an increasing x range this is the bar's
    /// left edge.
    pub x_start: f32,
    /// Bar width in pixels — comes straight from [`BandScale::bandwidth`].
    pub width: f32,
    /// Pixel y of the bar's top (its smallest y). For a positive bar this
    /// is above the baseline; for a negative bar it equals the baseline.
    pub top: f32,
    /// Pixel y of the bar's bottom (its largest y). For a positive bar
    /// this equals the baseline; for a negative bar it is below.
    pub bottom: f32,
    /// Pixel y of the bar's baseline (the value axis's zero / floor).
    pub baseline: f32,
}

impl BarGeometry {
    /// The bar's drawn height in pixels, `bottom - top`.
    ///
    /// Non-negative for well-formed geometry; `bottom - top` stays correct
    /// for negative bars (`top == baseline`) hung below the baseline.
    #[must_use]
    pub fn height(&self) -> f32 {
        (self.bottom - self.top).max(0.0)
    }

    /// Whether the bar hangs below the baseline (a negative-value bar).
    #[must_use]
    pub fn is_negative(&self) -> bool {
        // A negative bar has `top == baseline` and `bottom > baseline`. Use a
        // tiny epsilon for the equality so a numerical dust pixel does not
        // flip the sign.
        self.bottom > self.baseline + 0.001
    }
}

/// The pixel geometry of one painted segment in a grouped or stacked chart,
/// tagged with its `(category, series)` identity.
///
/// Its `geom` is the same [`BarGeometry`] a single-series bar reports, so every
/// height/baseline invariant carries over. Its `key` is the *identity* the hit-test returns — crucially not derived from geometry, so two
/// series holding identical values at the same category remain distinguishable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarSegment {
    /// Category index (outer band), in domain order.
    pub category: usize,
    /// Series index. `0` is the primary (`bar()`) series.
    pub series: usize,
    /// Stable identity — [`datum_key(category, series)`](datum_key).
    pub key: DatumKey,
    /// The painted rectangle's geometry.
    pub geom: BarGeometry,
}

/// What one [`draw_static`] call emitted.
///
/// The `bars` vector is the observable that proves `BandScale` is wired: N
/// categories produce N bars whose `x_start`s follow domain order and whose
/// widths equal the band width. There is no other way to see this off pixels —
/// two bars of the same colour are indistinguishable in a pixel count.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BarDrawReport {
    /// One entry per painted bar, in domain order.
    pub bars: Vec<BarGeometry>,
    /// One entry per painted segment in grouped/stacked mode, carrying its
    /// `(category, series)` identity. Empty in single-series mode, where the
    /// observable is [`Self::bars`]. Each segment's `key` is [`datum_key`], so
    /// a hit-test observed here maps a pixel back to the original datum.
    pub segments: Vec<BarSegment>,
    /// Horizontal grid lines stroked.
    pub grid_lines: usize,
    /// The `y` pixel of each grid line stroked, in draw order. Equals
    /// [`Self::y_tick_label_baseline_ys`] element-for-element — a grid line is
    /// a value tick, from the one shared `YAxisTicks`. This holds for the
    /// single-series path *and* the grouped/stacked second draw path.
    pub grid_line_ys: Vec<f32>,
    /// Axis lines stroked.
    pub axis_lines: usize,
    /// Category (x-axis) labels drawn.
    pub category_labels: usize,
    /// Value (y-axis) numeric tick labels drawn in the left gutter.
    pub y_tick_labels: usize,
    /// The exact Y-tick strings handed to `frame.fill_text`, in draw order.
    pub y_tick_label_texts: Vec<String>,
    /// The baseline `y` pixel of each y tick label, in draw order.
    pub y_tick_label_baseline_ys: Vec<f32>,
    /// Bars skipped because their value or mapped pixel was non-finite.
    pub non_finite_skipped: usize,
    /// Small "missing" ticks drawn at the baseline for non-finite bands
    /// A skipped band that would otherwise be anonymous
    /// gets a short horizontal stroke centred on the band, plus its category
    /// label; this counter is the observable so a test can pin the choice.
    pub non_finite_ticks: usize,
    /// The exact y-axis unit label handed to `frame.fill_text`, or `None` when
    /// the chart has no axis label (the default) — recorded at the draw call
    ///
    pub y_axis_label_text: Option<String>,
}

/// Paint grid, axes, category labels and the bars, reporting what was emitted.
///
/// # Gutters
///
/// The drawable plot is inset from `plot` by `Y_GUTTER` on the left and
/// `X_GUTTER` on the bottom, so labels never overdraw the bars. A bar's pixel
/// position is stated against the inset rect.
///
/// # The band scale and the reversed value scale
///
/// The x (category) axis is a [`BandScale`] over `0..n` across the inner rect's
/// horizontal pixel range `[inner_x, inner_x + inner_w]`. Category names are
/// elided within each band, or omitted when even an ellipsis cannot fit;
/// accessible data retains their full names. The y (value) axis is
/// a [`LinearScale`] with a reversed range `(inner_h, 0.0)` — pixels grow
/// downward, so the domain max maps to the smallest pixel. That single
/// reversal is what stands the bars upright; dropping it flips the chart.
///
/// **A `Frame` cannot be constructed without a renderer**, so this is pinned by
/// `tests/bar_chart_render.rs`, which builds a real `Frame`, calls this, and
/// asserts the returned [`BarDrawReport`].
pub fn draw_static(frame: &mut Frame, plot: Size, bars: &[Bar], key: BarChartKey) -> BarDrawReport {
    let mut report = BarDrawReport::default();
    if plot.width <= 0.0 || plot.height <= 0.0 {
        return report;
    }

    frame.fill(
        &Path::rectangle(Point::ORIGIN, plot),
        key.plot_background_color(),
    );

    // --- inner plot rect (gutters reserved for labels) ----------------
    let axis = key.axis_layout();
    let inner_x = resolve_y_gutter(axis.y_gutter, plot.width);
    let inner_bottom = (plot.height - X_GUTTER).max(0.0);
    let inner_w = (plot.width - inner_x).max(0.0);
    let inner_h = inner_bottom.max(0.0);
    let label_size = key.label_size;

    // --- shared value scale + y ticks: One nice-tick set feeds grid and
    // labels. The value scale's reversed range with a TOP_GUTTER top
    // (not 0); it must match the bar scale below so bars, grid lines
    // and value ticks all agree.
    let value_scale = LinearScale {
        domain: key.y_domain,
        range: (
            f64::from(inner_h),
            // Unit labels reserve headroom above the topmost tick; 0 without
            // one (hence the no-axis-label `TOP_GUTTER`).
            f64::from((TOP_GUTTER + axis.y_axis_label_reserve).min(inner_h)),
        ),
    };
    let y_ticks = y_axis_ticks(key.y_domain, value_scale);
    let draw_chrome = inner_w > 0.0 && inner_h > 0.0 && !bars.is_empty();

    // --- grid: one line per value tick, at each tick label's baseline -----
    if draw_chrome {
        let grid_stroke = Stroke::default()
            .with_color(key.grid_color())
            .with_width(1.0);
        report.grid_line_ys = draw_y_grid(frame, &y_ticks, inner_x, inner_w, grid_stroke);
        report.grid_lines = report.grid_line_ys.len();
    }

    // --- axes ---------------------------------------------------------
    const AXIS_WIDTH: f32 = 1.0;
    let axis_inset = AXIS_WIDTH / 2.0;
    let axis_stroke = Stroke::default()
        .with_color(key.axis_color())
        .with_width(AXIS_WIDTH);
    let x_axis_y = inner_h - axis_inset;
    frame.stroke(
        &Path::line(
            Point::new(inner_x, x_axis_y),
            Point::new(inner_x + inner_w, x_axis_y),
        ),
        axis_stroke,
    );
    report.axis_lines += 1;

    if !draw_chrome {
        return report;
    }

    // --- y-axis tick labels --------------------------
    // The `Y_GUTTER` inset is populated with numeric tick labels (mirroring
    // the line chart's y-axis) so callers read the values bars reach.
    {
        // Tick labels are text (4.5:1); axis lines are not.
        let (texts, baselines) = draw_y_tick_labels(
            frame,
            &y_ticks,
            inner_x,
            label_size,
            key.label_font,
            key.tick_label_color(),
            key.y_tick_formatter,
        );
        report.y_tick_label_texts = texts;
        report.y_tick_label_baseline_ys = baselines;
        report.y_tick_labels = report.y_tick_label_texts.len();

        // the y-axis UNIT, named once at the top of the gutter.
        report.y_axis_label_text = draw_y_axis_label(
            frame,
            key.y_axis_label,
            inner_x,
            label_size,
            key.label_font,
            key.tick_label_color(),
        );
    }

    // --- scales -------------------------------------------------------
    // Category axis: a BAND scale — position and width both come from here.
    // `padding_inner` separates the bars; `padding_outer` insets the
    // first/last band from the axis edges.
    let x_band = BandScale::new(
        bars.len(),
        (f64::from(inner_x), f64::from(inner_x + inner_w)),
    )
    .padding_inner(BAR_PADDING_INNER)
    .padding_outer(BAR_PADDING_OUTER);

    // Value axis: the same `value_scale` the grid and y ticks used above — a
    // REVERSED range so value grows *up* the screen, with a TOP_GUTTER top
    // (not 0) keeping the domain-max value tick label whole. Reusing the
    // one scale is what keeps bars, grid lines and value ticks in lock-step.
    let y_scale = value_scale;

    // The baseline is where value == 0 lands, clamped into the plot so a
    // domain that does not include zero still draws from an in-rect floor.
    let baseline_px = {
        let raw = y_scale.map(0.0);
        raw.clamp(0.0, f64::from(inner_h)) as f32
    };

    // Zero-baseline stroke when zero sits strictly inside the plot interior:
    // without it a negative bar hangs from an invisible origin. Baselines
    // glued to an edge are covered by the axis stroke — no double-draw.
    if baseline_px > 0.5 && baseline_px < inner_h - 0.5 {
        frame.stroke(
            &Path::line(
                Point::new(inner_x, baseline_px),
                Point::new(inner_x + inner_w, baseline_px),
            ),
            axis_stroke,
        );
        report.axis_lines += 1;
    }

    // --- bars ---------------------------------------------------------
    let bar_color = key.bar_color();
    for (i, b) in bars.iter().enumerate() {
        // Position and width from the band scale — not evenly-spaced literals.
        let Some(band) = x_band.band(i) else {
            continue;
        };

        // Missing-value ticks remain visible even when a narrow band omits its label.
        let value_px = y_scale.map(b.value);
        if !b.value.is_finite() || !value_px.is_finite() {
            report.non_finite_skipped += 1;
            if draw_category_label(
                frame,
                &x_band,
                i,
                &b.label,
                inner_h,
                label_size,
                key.label_font,
                // A category label is text.
                key.tick_label_color(),
            ) {
                report.category_labels += 1;
            }
            // Small horizontal tick centred on the band at the baseline, in
            // the axis colour. Signals "a datum exists here, its value is
            // missing" — different from an empty band (no datum).
            if let Some(center) = x_band.center(i) {
                let cx = center as f32;
                let tick_half = (band.width as f32 * 0.4).max(2.0);
                frame.stroke(
                    &Path::line(
                        Point::new(cx - tick_half, baseline_px),
                        Point::new(cx + tick_half, baseline_px),
                    ),
                    axis_stroke,
                );
                report.non_finite_ticks += 1;
            }
            continue;
        }

        // `band.start` leads in domain order (left edge for increasing
        // ranges); the rect spans [start, start+width].
        let x_left = band.start as f32;
        let width = band.width as f32;

        let value_px = value_px as f32;
        let top = value_px.min(baseline_px);
        let bottom = value_px.max(baseline_px);
        let height = bottom - top;

        if width > 0.0 && height > 0.0 {
            frame.fill(
                &bar_path(x_left, top, width, height, value_px <= baseline_px),
                bar_color,
            );
        }

        // Report every dimension captured from the drawn rect. `bottom` is
        // stored so `BarGeometry::height` is `bottom - top` and stays correct
        // for negative bars (top == baseline, bottom > baseline).
        report.bars.push(BarGeometry {
            x_start: x_left,
            width,
            top,
            bottom,
            baseline: baseline_px,
        });

        if draw_category_label(
            frame,
            &x_band,
            i,
            &b.label,
            inner_h,
            label_size,
            key.label_font,
            // A category label is text.
            key.tick_label_color(),
        ) {
            report.category_labels += 1;
        }
    }

    report
}

/// Corner radius at a bar's value end, before clamping to its size.
const BAR_CORNER_RADIUS: f32 = 4.0;

/// A bar's path: the value end is rounded and the baseline end stays square,
/// so the bar sits flush on its axis. `grows_up` is true for a bar above its
/// baseline.
fn bar_path(x: f32, top: f32, width: f32, height: f32, grows_up: bool) -> Path {
    let r = BAR_CORNER_RADIUS.min(width / 4.0).min(height);
    let radius = if grows_up {
        iced::border::Radius {
            top_left: r,
            top_right: r,
            bottom_right: 0.0,
            bottom_left: 0.0,
        }
    } else {
        iced::border::Radius {
            top_left: 0.0,
            top_right: 0.0,
            bottom_right: r,
            bottom_left: r,
        }
    };
    Path::rounded_rectangle(Point::new(x, top), Size::new(width, height), radius)
}

fn fit_category_label(label: &str, width: f32, size: f32, font: iced::Font) -> Option<String> {
    use crate::chart_geometry::label_width;
    use unicode_segmentation::UnicodeSegmentation;
    if label.is_empty() {
        return None;
    }
    if label_width(label, size, font) <= width {
        return Some(label.to_owned());
    }
    if label_width("…", size, font) > width {
        return None;
    }
    let ends: Vec<_> = std::iter::once(0)
        .chain(label.grapheme_indices(true).skip(1).map(|(i, _)| i))
        .collect();
    let mut low = 0;
    let mut high = ends.len();
    while low + 1 < high {
        let mid = (low + high) / 2;
        if label_width(
            &format!("{}…", label.get(..ends[mid]).unwrap_or(label)),
            size,
            font,
        ) <= width
        {
            low = mid;
        } else {
            high = mid;
        }
    }
    Some(format!("{}…", label.get(..ends[low]).unwrap_or(label)))
}

// Full category names remain in semantics when their painted names are elided.
#[allow(
    clippy::too_many_arguments,
    reason = "one category label's geometry, scale and style"
)]
fn draw_category_label(
    frame: &mut Frame,
    x_band: &BandScale,
    i: usize,
    label: &str,
    inner_h: f32,
    label_size: f32,
    label_font: iced::Font,
    axis_color: Color,
) -> bool {
    let Some(center) = x_band.center(i) else {
        return false;
    };
    let band = x_band.band(i).expect("existing band");
    let Some(content) = fit_category_label(
        label,
        (band.width as f32 - 2.0).max(0.0),
        label_size,
        label_font,
    ) else {
        return false;
    };
    frame.fill_text(Text {
        content,
        position: Point::new(center as f32, inner_h + 2.0),
        color: axis_color,
        size: label_size.into(),
        font: label_font,
        align_x: iced::advanced::text::Alignment::Center,
        align_y: iced::alignment::Vertical::Top,
        ..Text::default()
    });
    true
}

/// Paint a multi-series (grouped or stacked) bar chart, reporting the
/// per-segment geometry and identity.
///
/// The chrome (background, grid, axes, y-tick labels, zero baseline) is drawn
/// identically to [`draw_static`]; only the bars differ. `labels` are the
/// category labels (index-aligned with each series' values); `series[0]` is
/// the primary `bar()` series.
///
/// # Grouped vs. stacked geometry
///
/// * Grouped: the outer [`BandScale`] gives each category's slot; an inner
///   [`BandScale`] over `series.len()` subdivides that slot so the series sit
///   side by side. Each bar is drawn from the baseline like a single-series
///   bar. This is a band nested inside a band — no new
///   geometry maths.
/// * Stacked: series share the outer band's full width. Each category keeps
///   a running `pos` top and `neg` bottom; a positive value's segment spans
///   `[running_pos, running_pos + value]` in value space (mapped through
///   the reversed y scale), a negative value's spans `[running_neg + value,
///   running_neg]`. Positive and negative grow in opposite directions from
///   the baseline, matching [`stacked_domain`].
pub fn draw_multi_static(
    frame: &mut Frame,
    plot: Size,
    labels: &[String],
    series: &[BarSeries],
    key: BarChartKey,
) -> BarDrawReport {
    let mut report = BarDrawReport::default();
    if plot.width <= 0.0 || plot.height <= 0.0 {
        return report;
    }

    frame.fill(
        &Path::rectangle(Point::ORIGIN, plot),
        key.plot_background_color(),
    );

    // --- inner plot rect (identical to draw_static) -------------------
    let axis = key.axis_layout();
    let inner_x = resolve_y_gutter(axis.y_gutter, plot.width);
    let inner_bottom = (plot.height - X_GUTTER).max(0.0);
    let inner_w = (plot.width - inner_x).max(0.0);
    let inner_h = inner_bottom.max(0.0);
    let label_size = key.label_size;
    let n = labels.len();

    // --- shared value scale + y ticks feed grid and labels, exactly
    // as the single-series `draw_static` path does — the same computation,
    // not a copy.
    let value_scale = LinearScale {
        domain: key.y_domain,
        // top of the value range is TOP_GUTTER, not 0.
        range: (
            f64::from(inner_h),
            // Unit labels reserve headroom above the topmost tick; 0 without
            // one (hence the no-axis-label `TOP_GUTTER`).
            f64::from((TOP_GUTTER + axis.y_axis_label_reserve).min(inner_h)),
        ),
    };
    let y_ticks = y_axis_ticks(key.y_domain, value_scale);
    let draw_chrome = inner_w > 0.0 && inner_h > 0.0 && n != 0 && !series.is_empty();

    // --- grid: one line per value tick, at each tick label's baseline -----
    if draw_chrome {
        let grid_stroke = Stroke::default()
            .with_color(key.grid_color())
            .with_width(1.0);
        report.grid_line_ys = draw_y_grid(frame, &y_ticks, inner_x, inner_w, grid_stroke);
        report.grid_lines = report.grid_line_ys.len();
    }

    // --- axes ---------------------------------------------------------
    const AXIS_WIDTH: f32 = 1.0;
    let axis_inset = AXIS_WIDTH / 2.0;
    let axis_stroke = Stroke::default()
        .with_color(key.axis_color())
        .with_width(AXIS_WIDTH);
    let x_axis_y = inner_h - axis_inset;
    frame.stroke(
        &Path::line(
            Point::new(inner_x, x_axis_y),
            Point::new(inner_x + inner_w, x_axis_y),
        ),
        axis_stroke,
    );
    report.axis_lines += 1;

    if !draw_chrome {
        return report;
    }

    // --- y-axis tick labels -------------------------------------------
    {
        // Tick labels are text (4.5:1); axis lines are not.
        let (texts, baselines) = draw_y_tick_labels(
            frame,
            &y_ticks,
            inner_x,
            label_size,
            key.label_font,
            key.tick_label_color(),
            key.y_tick_formatter,
        );
        report.y_tick_label_texts = texts;
        report.y_tick_label_baseline_ys = baselines;
        report.y_tick_labels = report.y_tick_label_texts.len();

        // the y-axis UNIT, named once at the top of the gutter.
        report.y_axis_label_text = draw_y_axis_label(
            frame,
            key.y_axis_label,
            inner_x,
            label_size,
            key.label_font,
            key.tick_label_color(),
        );
    }

    // --- scales -------------------------------------------------------
    // Outer category band — same as single-series.
    let x_band = BandScale::new(n, (f64::from(inner_x), f64::from(inner_x + inner_w)))
        .padding_inner(BAR_PADDING_INNER)
        .padding_outer(BAR_PADDING_OUTER);

    // The same `value_scale` the grid and y ticks used above; TOP_GUTTER
    // top keeps the domain-max value tick label whole and must match so bars
    // align with their value ticks.
    let y_scale = value_scale;

    let baseline_px = {
        let raw = y_scale.map(0.0);
        raw.clamp(0.0, f64::from(inner_h)) as f32
    };

    // Zero-baseline stroke when zero sits strictly inside the plot interior
    // (identical rule to draw_static).
    if baseline_px > 0.5 && baseline_px < inner_h - 0.5 {
        frame.stroke(
            &Path::line(
                Point::new(inner_x, baseline_px),
                Point::new(inner_x + inner_w, baseline_px),
            ),
            axis_stroke,
        );
        report.axis_lines += 1;
    }

    match key.mode() {
        BarMode::Grouped => draw_grouped_bars(
            frame,
            &x_band,
            &y_scale,
            series,
            baseline_px,
            &key,
            &mut report,
        ),
        BarMode::Stacked => draw_stacked_bars(
            frame,
            &x_band,
            &y_scale,
            series,
            baseline_px,
            &key,
            &mut report,
        ),
    }

    for (i, label) in labels.iter().enumerate() {
        if draw_category_label(
            frame,
            &x_band,
            i,
            label,
            inner_h,
            label_size,
            key.label_font,
            key.tick_label_color(),
        ) {
            report.category_labels += 1;
        }
    }

    report
}

/// Draw grouped bars: an inner band scale subdivides each outer band across
/// `series.len()`, so the series sit side by side. Each bar draws from the
/// baseline exactly like a single-series bar; only its x-slot differs.
fn draw_grouped_bars(
    frame: &mut Frame,
    x_band: &BandScale,
    y_scale: &LinearScale,
    series: &[BarSeries],
    baseline_px: f32,
    key: &BarChartKey,
    report: &mut BarDrawReport,
) {
    let n_series = series.len();
    for cat in 0..key.category_count() {
        let Some(outer) = x_band.band(cat) else {
            continue;
        };
        // Inner band over the series, laid out across the outer band's own
        // pixel span. `outer.start .. outer.start + outer.width` in domain
        // order — the inner band inherits the outer's direction.
        let inner_band = BandScale::new(n_series, (outer.start, outer.start + outer.width))
            .padding_inner(GROUP_INNER_PADDING)
            .padding_outer(0.0);

        for (si, s) in series.iter().enumerate() {
            let Some(v) = s.value(cat) else {
                continue;
            };
            let value_px = y_scale.map(v);
            if !v.is_finite() || !value_px.is_finite() {
                report.non_finite_skipped += 1;
                continue;
            }
            let Some(slot) = inner_band.band(si) else {
                continue;
            };
            let x_left = slot.start as f32;
            let width = slot.width as f32;
            let value_px = value_px as f32;
            let top = value_px.min(baseline_px);
            let bottom = value_px.max(baseline_px);
            let height = bottom - top;
            if width > 0.0 && height > 0.0 {
                frame.fill(
                    &bar_path(x_left, top, width, height, value_px <= baseline_px),
                    key.series_color(si),
                );
            }
            report.segments.push(BarSegment {
                category: cat,
                series: si,
                key: datum_key(cat, si),
                geom: BarGeometry {
                    x_start: x_left,
                    width,
                    top,
                    bottom,
                    baseline: baseline_px,
                },
            });
        }
    }
}

/// Draw stacked bars: each series' segment sits atop the running sum for its
/// category. Positive contributions grow up from the running positive top;
/// negative contributions grow down from the running negative bottom — the
/// two never overlap, matching [`stacked_domain`].
fn draw_stacked_bars(
    frame: &mut Frame,
    x_band: &BandScale,
    y_scale: &LinearScale,
    series: &[BarSeries],
    baseline_px: f32,
    key: &BarChartKey,
    report: &mut BarDrawReport,
) {
    for cat in 0..key.category_count() {
        let Some(band) = x_band.band(cat) else {
            continue;
        };
        let x_left = band.start as f32;
        let width = band.width as f32;

        // Running value totals in *data* space, per sign. A positive value
        // spans [pos, pos + v]; a negative spans [neg + v, neg]. Both are then
        // mapped through the reversed y scale to pixels.
        let mut pos = 0.0_f64;
        let mut neg = 0.0_f64;
        for (si, s) in series.iter().enumerate() {
            let Some(v) = s.value(cat) else {
                continue;
            };
            if !v.is_finite() {
                report.non_finite_skipped += 1;
                continue;
            }
            let (v0, v1) = if v >= 0.0 {
                let lo = pos;
                pos += v;
                (lo, pos)
            } else {
                let hi = neg;
                neg += v;
                (neg, hi)
            };
            let p0 = y_scale.map(v0);
            let p1 = y_scale.map(v1);
            if !p0.is_finite() || !p1.is_finite() {
                report.non_finite_skipped += 1;
                continue;
            }
            let (p0, p1) = (p0 as f32, p1 as f32);
            let top = p0.min(p1);
            let bottom = p0.max(p1);
            let height = bottom - top;
            if width > 0.0 && height > 0.0 {
                frame.fill(
                    &Path::rectangle(Point::new(x_left, top), Size::new(width, height)),
                    key.series_color(si),
                );
            }
            report.segments.push(BarSegment {
                category: cat,
                series: si,
                key: datum_key(cat, si),
                geom: BarGeometry {
                    x_start: x_left,
                    width,
                    top,
                    bottom,
                    baseline: baseline_px,
                },
            });
        }
    }
}

impl BarChartKey {
    /// Grid colour this key paints with.
    pub fn grid_color(&self) -> Color {
        self.tokens.grid
    }
    /// Axis colour this key paints with.
    pub fn axis_color(&self) -> Color {
        self.tokens.axis
    }
    /// Tick-label ink this key paints with (text, held to
    /// 4.5:1, so distinct from [`Self::axis_color`]).
    pub fn tick_label_color(&self) -> Color {
        self.tokens.tick_label
    }
    /// Bar fill colour this key paints with.
    pub fn bar_color(&self) -> Color {
        self.tokens.bar
    }
    /// Plot background colour this key paints with.
    pub fn plot_background_color(&self) -> Color {
        self.tokens.plot_background
    }
    /// The value-axis domain this key draws with.
    #[must_use]
    pub fn y_domain(&self) -> (f64, f64) {
        self.y_domain
    }
    /// Number of categories.
    #[must_use]
    pub fn category_count(&self) -> usize {
        self.n
    }
    /// The layout mode this key draws with.
    #[must_use]
    pub fn mode(&self) -> BarMode {
        self.mode
    }
    /// Number of series (primary + extra) this key draws with. `0` for the
    /// single-series path.
    #[must_use]
    pub fn series_count(&self) -> usize {
        self.series_count
    }
    /// The per-series colour for `series` in multi-series mode, cycling every
    /// 8 hues. Falls back to the bar-fill token when no palette is set.
    pub fn series_color(&self, series: usize) -> Color {
        match self.palette {
            Some(p) => p.series_color(series),
            None => self.tokens.bar,
        }
    }
}

// ---------------------------------------------------------------------------
// Bar snapshot (shared source data + lazy metadata)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct BarSourceMeta {
    bar_digest: u64,
    ser_digest: u64,
    raw_grouped: Option<(f64, f64)>,
    raw_stacked: Option<(f64, f64)>,
}

#[derive(Debug)]
struct BarSnapshotInner {
    bars: Vec<Bar>,
    series: Vec<BarSeries>,
    meta: Cell<Option<BarSourceMeta>>,
}

impl Clone for BarSnapshotInner {
    fn clone(&self) -> Self {
        Self {
            bars: self.bars.clone(),
            series: self.series.clone(),
            meta: Cell::new(self.meta.get()),
        }
    }
}

#[derive(Debug, Clone)]
struct BarSnapshot {
    inner: Rc<BarSnapshotInner>,
}

impl BarSnapshot {
    fn empty() -> Self {
        Self {
            inner: Rc::new(BarSnapshotInner {
                bars: Vec::new(),
                series: Vec::new(),
                meta: Cell::new(None),
            }),
        }
    }

    fn push_bar(&mut self, bar: Bar) {
        Rc::make_mut(&mut self.inner).bars.push(bar);
        self.inner.meta.set(None);
    }

    fn push_series(&mut self, s: BarSeries) {
        Rc::make_mut(&mut self.inner).series.push(s);
        self.inner.meta.set(None);
    }

    fn ensure(&self) {
        if self.inner.meta.get().is_some() {
            return;
        }
        let bd = digest(&self.inner.bars);
        let sd = series_digest(&self.inner.series);
        let raw_grouped = compute_raw_grouped(&self.inner.bars, &self.inner.series);
        let raw_stacked = compute_raw_stacked(&self.inner.bars, &self.inner.series);
        self.inner.meta.set(Some(BarSourceMeta {
            bar_digest: bd,
            ser_digest: sd,
            raw_grouped,
            raw_stacked,
        }));
    }

    fn meta(&self) -> BarSourceMeta {
        self.ensure();
        self.inner.meta.get().expect("ensure fills the metadata")
    }

    fn bars(&self) -> &[Bar] {
        &self.inner.bars
    }

    fn extra_series(&self) -> &[BarSeries] {
        &self.inner.series
    }

    fn y_domain(&self, mode: BarMode, policy: DomainPolicy) -> Option<(f64, f64)> {
        let m = self.meta();
        // A single series keeps its tight extent regardless of stacking mode.
        let raw = if self.inner.series.is_empty() {
            m.raw_grouped
        } else {
            match mode {
                BarMode::Grouped => m.raw_grouped,
                BarMode::Stacked => m.raw_stacked,
            }
        };
        raw.map(|d| value_axis_domain(d, policy))
    }
}

// Grouped-mode raw domain fold. Instrumented.
fn compute_raw_grouped(bars: &[Bar], series: &[BarSeries]) -> Option<(f64, f64)> {
    let mut scan: u64 = bars.len() as u64;
    for s in series {
        scan += s.values.len() as u64;
    }
    crate::cartesian_chart::record_source_scan(scan);
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut any = false;
    for b in bars {
        if b.value.is_finite() {
            any = true;
            lo = lo.min(b.value);
            hi = hi.max(b.value);
        }
    }
    for s in series {
        for &v in &s.values {
            if v.is_finite() {
                any = true;
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
    }
    any.then_some((lo, hi))
}

// Stacked-mode raw domain fold. Instrumented.
fn compute_raw_stacked(bars: &[Bar], series: &[BarSeries]) -> Option<(f64, f64)> {
    let n = bars.len();
    let mut scan: u64 = n as u64;
    for s in series {
        scan += s.values.len() as u64;
    }
    crate::cartesian_chart::record_source_scan(scan);
    let mut floor = 0.0_f64;
    let mut ceil = 0.0_f64;
    let mut any = false;
    for (cat, bar) in bars.iter().enumerate() {
        let mut pos = 0.0_f64;
        let mut neg = 0.0_f64;
        let v = bar.value;
        if v.is_finite() {
            any = true;
            if v >= 0.0 {
                pos += v;
            } else {
                neg += v;
            }
        }
        for s in series {
            if let Some(v) = s.value(cat)
                && v.is_finite()
            {
                any = true;
                if v >= 0.0 {
                    pos += v;
                } else {
                    neg += v;
                }
            }
        }
        ceil = ceil.max(pos);
        floor = floor.min(neg);
    }
    any.then_some((floor, ceil))
}

/// A token-driven vertical bar chart with one tab stop and a cached
/// static layer.
///
/// A single series of labelled values; category on x (a [`BandScale`]),
/// value on y (a reversed [`LinearScale`]).  See the module docs for the
/// deliberate scope limits.
///
/// Bars are placed in insertion order; the [`BandScale`] never sorts
/// them.  In single-series use each [`bar`](Self::bar) is the whole datum.
/// In multi-series use the `bar` value becomes the **primary series'
/// (index 0)** value for that category; extra [`series`](Self::series)
/// supply the rest positionally.
#[derive(Clone)]
pub struct EnzBarChart<Message = ()> {
    snap: BarSnapshot,
    mode: BarMode,
    width: f32,
    /// Whether the plot fills its parent's width (built at the width the
    /// layout gives it) instead of a fixed pixel width.
    fill_width: bool,
    height: f32,
    title: String,
    summary: Option<String>,
    y_policy: DomainPolicy,
    tokens: Option<BarChartTokens>,
    palette: Option<ChartPalette>,
    y_tick_formatter: TickFormatter,
    y_gutter: f32,
    y_axis_label: AxisLabel,
    selection: Option<BarSelection>,
    on_select: Option<Rc<dyn Fn(BarSelection) -> Message>>,
}

/// Which bar the keyboard cursor is on: a category and a series (`0` is the
/// primary, [`EnzBarChart::bar`], series).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BarSelection {
    /// The category (band), in domain order.
    pub category: usize,
    /// The series inside the category.
    pub series: usize,
}

impl<Message> std::fmt::Debug for EnzBarChart<Message> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnzBarChart")
            .field("title", &self.title)
            .field("categories", &self.snap.bars().len())
            .field("mode", &self.mode)
            .field("selection", &self.selection)
            .field("on_select", &self.on_select.is_some())
            .finish_non_exhaustive()
    }
}

impl<Message> EnzBarChart<Message> {
    /// Name the value axis's unit once, instead of repeating it on every
    /// tick. Prefer this to [`Self::y_gutter`]: it shortens every tick
    /// label rather than shrinking the plot, and it is published to assistive
    /// tech as part of the chart's description.
    #[must_use]
    pub fn y_axis_label(mut self, label: impl AsRef<str>) -> Self {
        self.y_axis_label = AxisLabel::new(label.as_ref());
        self
    }

    /// Widen the left gutter reserved for value tick labels, in pixels
    /// (default 40). Clamped by
    /// [`crate::chart_geometry::resolve_y_gutter`] so a
    /// caller cannot reserve the plot away.
    #[must_use]
    pub fn y_gutter(mut self, gutter: f32) -> Self {
        if gutter.is_finite() && gutter >= 0.0 {
            self.y_gutter = gutter;
        }
        self
    }
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

    /// How the value (y) domain is derived. Default [`DomainPolicy::IncludeZero`]
    /// — bars are drawn from a baseline, so the axis floor must reach zero or
    /// heights misrepresent magnitude.
    #[must_use]
    pub fn y_domain(mut self, policy: DomainPolicy) -> Self {
        self.y_policy = policy;
        self
    }

    /// Override the Y-axis tick label formatter (default
    /// [`format_tick`](crate::chart_geometry::format_tick)).
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
    pub fn tokens(mut self, tokens: BarChartTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Add one labelled bar (insertion order preserved by [`BandScale`]).
    #[must_use]
    pub fn bar(mut self, label: impl Into<String>, value: f64) -> Self {
        self.snap.push_bar(Bar {
            label: label.into(),
            value,
        });
        self
    }

    /// Add a category label with no primary-series value (primary contributes
    /// `0`). For multi-series charts where every value lives in an added
    /// [`series`](Self::series).
    #[must_use]
    pub fn category(mut self, label: impl Into<String>) -> Self {
        self.snap.push_bar(Bar {
            label: label.into(),
            value: 0.0,
        });
        self
    }

    /// Add an extra data series (grouped or stacked per [`mode`](Self::mode)).
    /// Values are index-aligned with categories from [`bar`](Self::bar)/
    /// [`category`](Self::category); the primary `bar()` values are series 0.
    #[must_use]
    pub fn series(mut self, series: BarSeries) -> Self {
        self.snap.push_series(series);
        self
    }

    /// Choose the multi-series layout mode. Ignored when no extra series is
    /// added (a single-series chart is always one bar per category).
    #[must_use]
    pub fn mode(mut self, mode: BarMode) -> Self {
        self.mode = mode;
        self
    }

    /// Pin the per-series colour palette, bypassing the draw-time theme.
    #[must_use]
    pub fn palette(mut self, palette: ChartPalette) -> Self {
        self.palette = Some(palette);
        self
    }

    /// The semantics node: one `Role::Image` carrying the title and summary.
    /// When [`Self::y_axis_label`] is set the unit is appended to the
    /// description so it is visible to assistive tech.
    pub fn semantics(&self) -> Semantics {
        let s = Semantics::new(SemRole::Image).label(self.title.clone());
        let s = match chart_description(self.summary.as_deref(), self.y_axis_label) {
            Some(d) => s.description(d),
            None => s,
        };
        // With a cursor, the bar it is on is the value, and its place among
        // all bars the position ("bar 4 of 12").
        match self.selected_bar() {
            Some((selection, text)) => {
                let series = self.series_count();
                s.value(text)
                    .set_position(enzui_core::semantics::SetPosition {
                        index: selection.category * series + selection.series + 1,
                        count: self.snap.bars().len() * series,
                    })
            }
            None => s,
        }
    }

    /// How many series each category holds (`1` without extra series).
    fn series_count(&self) -> usize {
        self.snap.extra_series().len() + 1
    }

    /// The selected bar, resolved against the data, and how it reads:
    /// "Category: value" or "Series, Category: value".
    fn selected_bar(&self) -> Option<(BarSelection, String)> {
        let selection = self.selection?;
        let bar = self.snap.bars().get(selection.category)?;
        let value = if selection.series == 0 {
            bar.value
        } else {
            self.snap
                .extra_series()
                .get(selection.series - 1)?
                .value(selection.category)?
        };
        let text = if selection.series == 0 {
            format!("{}: {value}", bar.label)
        } else {
            let series = &self.snap.extra_series()[selection.series - 1].name;
            format!("{series}, {}: {value}", bar.label)
        };
        Some((selection, text))
    }

    /// Where the keyboard cursor is ([`Self::on_select`] reports moves).
    /// The host owns it; the chart keeps no copy.
    #[must_use]
    pub fn selection(mut self, selection: Option<BarSelection>) -> Self {
        self.selection = selection;
        self
    }

    /// Publish a cursor move: Left and Right (Home, End) move between
    /// categories, Up and Down between the series of a category.
    #[must_use]
    pub fn on_select(mut self, f: impl Fn(BarSelection) -> Message + 'static) -> Self {
        self.on_select = Some(Rc::new(f));
        self
    }

    /// Where a traversal key moves the cursor, or `None` when it cannot move
    /// (past an end, no bars). Without a cursor, any key enters the first bar.
    #[must_use]
    pub fn traverse(&self, key: crate::line_chart::TraversalKey) -> Option<BarSelection> {
        use crate::line_chart::TraversalKey;
        use enzui_core::chart::DatumStep;
        let categories = self.snap.bars().len();
        let series = self.series_count();
        if categories == 0 {
            return None;
        }
        let Some(current) = self.selection else {
            return Some(BarSelection {
                category: 0,
                series: 0,
            });
        };
        let last = categories - 1;
        let next = match key {
            TraversalKey::Datum(DatumStep::Previous) => BarSelection {
                category: current.category.checked_sub(1)?,
                ..current
            },
            TraversalKey::Datum(DatumStep::Next) => BarSelection {
                category: (current.category < last).then_some(current.category + 1)?,
                ..current
            },
            TraversalKey::Datum(DatumStep::First) => BarSelection {
                category: 0,
                ..current
            },
            TraversalKey::Datum(DatumStep::Last) => BarSelection {
                category: last,
                ..current
            },
            TraversalKey::SeriesStep(delta) => {
                let to = i64::try_from(current.series).ok()? + i64::from(delta);
                let to = usize::try_from(to).ok().filter(|to| *to < series)?;
                BarSelection {
                    series: to,
                    ..current
                }
            }
        };
        (next != current).then_some(next)
    }

    /// The companion data table: every category and its
    /// value, in draw order.
    ///
    /// A bar chart's datum identity is its category, so a row is
    /// `(index, label, value)` rather than the `(series, x, y)` shape the
    /// point-series charts use. A host renders this as an accessible table so a
    /// screen-reader user reads the same categories and values the bars show.
    /// Non-finite values are stated verbatim (a table cell can hold `NaN`; the
    /// bar cannot), so the table's row count always equals the caller's bar
    /// count.
    ///
    /// # Multi-series
    ///
    /// In single-series use each row is one category. In multi-series use each
    /// row is one (category, series) datum — the row's `index` is the
    /// category, and the extra [`BarTableRow::series`] / [`BarTableRow::series_name`]
    /// fields carry the series identity so a screen-reader user reads every
    /// segment the bars show, from the original data.
    #[must_use]
    pub fn data_table(&self) -> Vec<BarTableRow> {
        if self.snap.extra_series().is_empty() {
            return self
                .snap
                .bars()
                .iter()
                .enumerate()
                .map(|(index, b)| BarTableRow {
                    index,
                    label: b.label.clone(),
                    value: b.value,
                    series: 0,
                    series_name: String::new(),
                })
                .collect();
        }
        // Multi-series: one row per (category, series), category-major so a
        // reader walks each category's segments together. Series 0 is primary.
        let all = self.program().all_series();
        let mut rows = Vec::with_capacity(self.snap.bars().len() * all.len());
        for (index, b) in self.snap.bars().iter().enumerate() {
            for (si, s) in all.iter().enumerate() {
                rows.push(BarTableRow {
                    index,
                    label: b.label.clone(),
                    value: s.value(index).unwrap_or(0.0),
                    series: si,
                    series_name: s.name.clone(),
                });
            }
        }
        rows
    }

    /// Hit test a pointer position against the drawn segments, returning the
    /// original `(category, series)` datum whose painted rectangle contains
    /// `point` — or `None` for a click in whitespace.
    ///
    /// The report must come from a real [`draw_multi_static`] pass so the rects
    /// are the ones actually painted. Identity is [`datum_key`], not derived
    /// from geometry: two series holding identical values at the same category
    /// stay distinguishable because the returned key encodes both indices.
    #[must_use]
    pub fn hit_test(report: &BarDrawReport, point: Point) -> Option<BarHit> {
        // Reverse order so a later-drawn (visually on-top) segment wins an
        // overlap; grouped segments never overlap, but stacked segments abut.
        report.segments.iter().rev().find_map(|seg| {
            let g = &seg.geom;
            let inside_x = point.x >= g.x_start && point.x < g.x_start + g.width;
            let inside_y = point.y >= g.top && point.y < g.bottom;
            (inside_x && inside_y).then_some(BarHit {
                category: seg.category,
                series: seg.series,
                key: seg.key,
            })
        })
    }

    /// The Canvas program this chart draws with. Exposed so tests can drive
    /// `draw` without a window.
    #[must_use]
    pub fn program(&self) -> BarChartProgram {
        self.snap.ensure();
        let m = self.snap.meta();
        BarChartProgram {
            bars: Rc::clone(&self.snap.inner),
            mode: self.mode,
            pinned_tokens: self.tokens,
            palette: self.palette,
            y_tick_formatter: self.y_tick_formatter,
            y_gutter: self.y_gutter,
            y_axis_label: self.y_axis_label,
            cached_bar_digest: m.bar_digest,
            cached_series_digest: m.ser_digest,
            cached_y_domain: self.snap.y_domain(self.mode, self.y_policy),
            selection: self.selection,
        }
    }

    /// The cache key this chart would use for `theme` at its configured size.
    #[must_use]
    pub fn key_for(&self, theme: &iced::Theme) -> BarChartKey {
        self.program()
            .key(theme, Size::new(self.width, self.height))
    }

    /// Run `draw_static` for real against `plot`, returning what was emitted.
    ///
    /// Exposed so `tests/bar_chart_render.rs` hands the *same* bars to
    /// `draw_static` that the widget does, rather than rebuilding them
    /// alongside.
    #[must_use]
    pub fn draw_report(&self, frame: &mut Frame, plot: Size, theme: &iced::Theme) -> BarDrawReport {
        let program = self.program();
        let key = program.key(theme, plot);
        if self.snap.extra_series().is_empty() {
            draw_static(frame, plot, self.snap.bars(), key)
        } else {
            let labels: Vec<String> = self.snap.bars().iter().map(|b| b.label.clone()).collect();
            draw_multi_static(frame, plot, &labels, &program.all_series(), key)
        }
    }

    /// Build the element.
    ///
    /// The Canvas is wrapped in a [`crate::focus_button::FocusButton`] with no
    /// activation message, making the whole chart exactly one tab stop
    /// It uses the same `focusable_without_activation` and `style_focus`
    /// wiring as the line chart.
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
                // A message-less focus button defaults to `disabled == true` (it
                // has no `on_press`), which would announce the chart as an
                // unavailable control. But a chart is not a *control* — it is a
                // read-only graphic (`Role::Image`), semantically *enabled but
                // non-actionable*, the same category `carousel`'s selected page
                // indicator is in. Override the derived flag so
                // AT does not read "disabled" over the chart's title/summary.
                .disabled_semantics(false)
                .padding(ring)
                .width(Length::Fixed(self.width + 2.0 * ring))
                .height(Length::Fixed(self.height + 2.0 * ring))
                .semantics(semantics)
                .style_focus(move |theme, status| {
                    use crate::focus_button::FocusStatus;
                    let tokens = pinned.unwrap_or_else(|| {
                        crate::theme_lookup::resolve(theme, BarChartTokens::from_palette)
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
        // Arrows move the cursor while the chart has focus; a key that
        // cannot move publishes nothing and keeps bubbling.
        crate::keyboard_capture::keyboard_capture(stop, move |key, modifiers| {
            let next = self.traverse(crate::line_chart::event_for_key(key, modifiers)?)?;
            Some(on_select(next))
        })
    }
}

impl<'a, Message: 'a + Clone> From<EnzBarChart<Message>> for Element<'a, Message> {
    fn from(c: EnzBarChart<Message>) -> Self {
        c.into_element()
    }
}

/// Construct a vertical bar chart with the required accessible `title`.
///
/// The title is not optional: the chart is a graphic and needs an
/// accessible name, as with `enz_line_chart`.
#[must_use]
pub fn enz_bar_chart<Message>(title: impl Into<String>) -> EnzBarChart<Message> {
    EnzBarChart {
        snap: BarSnapshot::empty(),
        mode: BarMode::Grouped,
        width: DEFAULT_WIDTH,
        fill_width: false,
        height: DEFAULT_HEIGHT,
        title: title.into(),
        summary: None,
        y_policy: DomainPolicy::IncludeZero,
        tokens: None,
        palette: None,
        y_tick_formatter: TickFormatter::default(),
        y_gutter: Y_GUTTER,
        y_axis_label: AxisLabel::default(),
        selection: None,
        on_select: None,
    }
}

// ---------------------------------------------------------------------------
// Tests (builder + key algebra; render/geometry lives in tests/bar_chart_render.rs)
// ---------------------------------------------------------------------------
