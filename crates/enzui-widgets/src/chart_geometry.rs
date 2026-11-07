//! Shared Cartesian-chart geometry: the drawable plot rect, the data→pixel
//! scales, and the axis-tick machinery.
//!
//! # Why this exists
//!
//! `plot_geometry()` is the single source of truth for every Cartesian chart's
//! mapping: a scatter dot and a line vertex over the same datum must land
//! on the same pixel, and an area's fill must trace the same envelope its line
//! would. Two copies of a gutter rule drift — so the rule lives here once and
//! every Cartesian chart calls it.
//!
//! Mostly pure geometry and string formatting: the drawable rect, the two
//! [`LinearScale`]s, and the tick-label layout. The one theme-aware item is
//! [`chart_label_font`], which resolves `FontRole::Ui` for axis/tick labels.
//! Mark colours and drawing stay in each chart; the identical y-tick
//! `Frame` call is shared so all four chart hosts apply formatter and step with
//! one policy.

use enzui_core::chart::{DomainPolicy, LinearScale, apply_domain_policy, nice_extent, nice_ticks};
use iced::advanced::graphics::text::Paragraph as GraphicsParagraph;
use iced::advanced::text::Paragraph as _;
use iced::widget::canvas::{Frame, Path, Stroke, Text};
use iced::{Color, Point, Size};

// --- gutters ---------------------------------------------------------------

/// Default left gutter reserved for y-axis tick labels, in pixels. Not
/// data-dependent, so the drawable plot rect is a deterministic function of the
/// requested size alone — which keeps the decimation bound `2 * plot_width`
/// exact per size rather than shifting with the data. Wide enough for a
/// caption-size `"-1000"` / `"1.0M"` (~34px measured) plus a 4px gap.
///
/// # This is the default, not the law
///
/// A chart whose y labels are *genuinely* wider than `Y_GUTTER - Y_LABEL_GAP`
/// (36px) had no way to make room, so an app ended up shaping its tick
/// formatter around this constant — `"600 Kbps"` measures 41.6px and was
/// painted as `00 Kbps`, its leading digit clipped at the canvas origin.
/// Widen it per chart with `y_gutter(..)` (see [`resolve_y_gutter`]).
///
/// **Reach for [`y_axis_label`](crate::area_chart::EnzAreaChart::y_axis_label)
/// first.** Naming the unit once on the axis (`"Kbps"`) shortens every tick
/// label (`0 2 4` instead of `0 Kb 2 Kb 4 Kb`), keeps the apps' layout
/// identical, and — unlike extra pixels — is announced to a screen reader.
/// `y_gutter` is the escape hatch for labels that are wide after that: long
/// currency, dates, non-Latin numerals.
pub const Y_GUTTER: f32 = 40.0;

/// The largest fraction of the plot width a caller-requested y gutter may
/// take, so `y_gutter(f32::MAX)` cannot reserve the entire plot away.
///
/// The cap is `max(Y_GUTTER, plot_width * MAX_Y_GUTTER_FRACTION)`, never below
/// the default gutter — so [`Y_GUTTER`] itself is *never* affected by this cap
/// and every existing chart in apps lays out byte-identically.
pub const MAX_Y_GUTTER_FRACTION: f32 = 0.5;

/// Resolve the effective left gutter (the plot rect's `inner_x`) for a
/// requested gutter at a given plot width.
///
/// Three clamps, in order:
///
/// 1. a non-finite request falls back to [`Y_GUTTER`], and a negative one to
///    `0.0` — a caller cannot produce a negative `inner_x`;
/// 2. the request is capped at `max(Y_GUTTER, plot_width *
///    MAX_Y_GUTTER_FRACTION)`, so a large request still leaves half the plot
///    drawable while the default gutter passes through untouched;
/// 3. finally `.min(plot_width)`, the plot-width rule, so a tiny frame degrades
///    to a zero-area plot rather than a negative one.
///
/// Passing [`Y_GUTTER`] returns exactly `Y_GUTTER.min(plot_width)` at every
/// width — the expression this replaced.
#[must_use]
pub fn resolve_y_gutter(requested: f32, plot_width: f32) -> f32 {
    let plot_width = plot_width.max(0.0);
    let requested = if requested.is_finite() {
        requested.max(0.0)
    } else {
        Y_GUTTER
    };
    let cap = Y_GUTTER.max(plot_width * MAX_Y_GUTTER_FRACTION);
    requested.min(cap).min(plot_width)
}

/// Bottom gutter reserved for x-axis tick labels, in pixels. One caption line
/// (~10px) plus a 4px gap above and 2px below.
pub const X_GUTTER: f32 = 16.0;

/// Top gutter reserved above the drawable rect so the **y tick at the domain
/// maximum** — which maps to the top of the value scale — has room for the
/// upper half of its centre-aligned label.
///
/// Without it the top tick mapped to pixel `y = 0` and its label, centred on
/// that row, lost its upper half off the plot's top edge while every lower
/// label stayed whole (the bottom x-gutter reserves space at the bottom but
/// nothing balanced it at the top). A caption label measures ~8 ink rows, so
/// half is ~4px; `8.0` clears that with a pixel of slack, matching the
/// asymmetry the bottom gutter already answers for the x labels.
pub const TOP_GUTTER: f32 = 8.0;

// Compile-time invariant (was a runtime `assert!` in tests, folded by the
// compiler so it could never fail — clippy::assertions_on_constants): a
// non-positive TOP_GUTTER clips the top label. As a module-level const block
// this fails the BUILD, not just a test run, if the constant above is edited
// to violate it.
const _: () = assert!(
    TOP_GUTTER > 0.0,
    "TOP_GUTTER must be positive or the top label clips"
);

/// Horizontal gap, in pixels, between a right-aligned y tick label and the
/// plot's left edge (`inner_x`). The label box therefore occupies
/// `[0, inner_x - Y_LABEL_GAP]`; nothing crosses `inner_x` into the plot.
pub const Y_LABEL_GAP: f32 = 4.0;

/// Vertical gap between the y-axis unit label and the topmost tick label
/// Reserved only when a chart actually sets an axis label, so a chart
/// without one keeps the no-axis-label value range exactly.
pub const Y_AXIS_LABEL_GAP: f32 = 4.0;

/// The vertical space an axis unit label needs above the topmost y tick, in
/// pixels: one caption line plus [`Y_AXIS_LABEL_GAP`], or `0.0` when the chart
/// has no axis label (the default — the apps' geometry is unchanged).
#[must_use]
pub fn y_axis_label_reserve(label: AxisLabel, label_size: f32) -> f32 {
    if label.is_empty() {
        0.0
    } else {
        label_size.max(0.0) + Y_AXIS_LABEL_GAP
    }
}

/// Maximum bytes an [`AxisLabel`] stores inline.
const AXIS_LABEL_CAPACITY: usize = 24;

/// A short y-axis unit label (`"Kbps"`, `"%"`, `"MB/s"`), stored inline so
/// it can live in a `Copy + PartialEq` chart cache key.
///
/// # Why inline and not a `String`
///
/// The axis label is painted, so it must participate in cache identity: a
/// chart that swaps `"Kb"` for `"Mb"` must not replay a cached layer showing
/// the wrong unit. Every chart key is `Copy` (it is compared every frame),
/// which a `String` would break, and a `&'static str` would refuse the
/// formatted units apps actually build. Storing up to
/// 24 bytes inline (`AXIS_LABEL_CAPACITY`) keeps the key `Copy` and still accepts
/// an owned string; a longer label is truncated at a UTF-8 char boundary,
/// because an axis *unit* longer than 24 bytes is a caption, not a unit, and
/// would not fit any gutter anyway.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AxisLabel {
    buf: [u8; AXIS_LABEL_CAPACITY],
    len: u8,
}

impl AxisLabel {
    /// Store `label`, truncated at a char boundary to fit inline.
    #[must_use]
    pub fn new(label: &str) -> Self {
        let mut end = label.len().min(AXIS_LABEL_CAPACITY);
        while end > 0 && !label.is_char_boundary(end) {
            end -= 1;
        }
        let bytes = &label.as_bytes()[..end];
        let mut buf = [0u8; AXIS_LABEL_CAPACITY];
        buf[..end].copy_from_slice(bytes);
        Self {
            buf,
            len: end as u8,
        }
    }

    /// The stored label.
    #[must_use]
    pub fn as_str(&self) -> &str {
        // Constructed only from a char-boundary prefix of a `&str`.
        std::str::from_utf8(&self.buf[..usize::from(self.len)]).unwrap_or("")
    }

    /// Whether no axis label is set (the default).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Default for AxisLabel {
    fn default() -> Self {
        Self {
            buf: [0u8; AXIS_LABEL_CAPACITY],
            len: 0,
        }
    }
}

impl std::fmt::Debug for AxisLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("AxisLabel").field(&self.as_str()).finish()
    }
}

impl std::fmt::Display for AxisLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Compose a chart's accessible description from its caller-set summary and
/// its y-axis unit label.
///
/// An axis unit that exists only as pixels is invisible to a screen reader: the
/// chart publishes one `Role::Image` node, so the unit has exactly one place to
/// go — the description beside the summary. The sentence is appended, never
/// substituted, so a caller's summary is never lost.
#[must_use]
pub fn chart_description(summary: Option<&str>, y_axis_label: AxisLabel) -> Option<String> {
    let axis = (!y_axis_label.is_empty()).then(
        || enzui_core::localized!("enzui-chart-geometry-y-axis-in", "y-axis-label" => y_axis_label.to_string()),
    );
    match (summary, axis) {
        (Some(s), Some(a)) => Some(
            enzui_core::localized!("enzui-chart-axis-description", "summary" => s, "axis" => a),
        ),
        (Some(s), None) => Some(s.to_owned()),
        (None, Some(a)) => Some(a),
        (None, None) => None,
    }
}

/// Target number of ticks per axis handed to
/// [`nice_ticks`]. A *target*, not a
/// guarantee: the nice-number algorithm rounds the step to a human value.
pub const X_TICK_TARGET: usize = 6;
/// Y-axis tick target — see [`X_TICK_TARGET`].
pub const Y_TICK_TARGET: usize = 5;

/// The value-axis domain for a data extent: `policy` applied, then widened to
/// the y tick ladder so the outer grid lines frame the data. A
/// [`DomainPolicy::Fixed`] domain is kept as given.
pub(crate) fn value_axis_domain(extent: (f64, f64), policy: DomainPolicy) -> (f64, f64) {
    let domain = apply_domain_policy(extent, policy);
    match policy {
        DomainPolicy::Fixed(lo, hi) if lo.is_finite() && hi.is_finite() => domain,
        _ => nice_extent(domain, Y_TICK_TARGET),
    }
}

/// Horizontal gap enforced between adjacent x-axis labels before they are
/// considered colliding, in pixels.
pub const X_LABEL_MIN_GAP: f32 = 6.0;

// --- plot geometry ---------------------------------------------------------

/// The drawable plot rect and the data→pixel scales for a given frame `plot`
/// size and pair of data domains.
///
/// # Why this is shared, not recomputed
///
/// Every Cartesian chart (line, area, scatter) and the hover crosshair must map
/// data into exactly the same pixels, or an overlay drawn from an
/// independently-derived scale would sit off the marks it highlights. Deriving
/// the rect and scales once here — and handing the *same* value to every layer —
/// makes that agreement structural rather than a coincidence a later gutter edit
/// could break.
///
/// The `x_scale`/`y_scale` here are the draw scales (range offset by the
/// left gutter). Decimation needs a *different* scale whose range starts at
/// `0` — see [`Self::column_scale`].
#[derive(Debug, Clone, Copy)]
pub struct PlotGeometry {
    /// Left edge of the drawable rect (the y-axis gutter width, clamped).
    pub inner_x: f32,
    /// Drawable width in pixels (never negative).
    pub inner_w: f32,
    /// Drawable height in pixels (never negative).
    pub inner_h: f32,
    /// Data-space x → pixel-space x, range `[inner_x, inner_x + inner_w]`.
    pub x_scale: LinearScale,
    /// Data-space y → pixel-space y, range `[inner_h, 0]` (y grows upward in
    /// data space, downward in pixels).
    pub y_scale: LinearScale,
}

impl PlotGeometry {
    /// The decimation column scale: identical domain to [`Self::x_scale`]
    /// but a range starting at `0`.
    ///
    /// `decimate` buckets points into columns `0..inner_w` via the scale it is
    /// handed and clamps to that range, so it must receive a scale whose range
    /// is `[0, inner_w]` — not the draw scale, whose range is offset by the left
    /// gutter. The caller adds `inner_x` back when placing draw positions.
    #[must_use]
    pub fn column_scale(&self) -> LinearScale {
        LinearScale {
            domain: self.x_scale.domain,
            range: (0.0, f64::from(self.inner_w)),
        }
    }
}

/// Derive the drawable rect and draw scales for `plot` at the given `x_domain`
/// / `y_domain`. See [`PlotGeometry`] for why this is a single source of truth.
#[must_use]
pub fn plot_geometry(plot: Size, x_domain: (f64, f64), y_domain: (f64, f64)) -> PlotGeometry {
    plot_geometry_with(plot, x_domain, y_domain, AxisLayout::default())
}

/// The caller-tunable part of the chart's axis layout.
///
/// Defaults reproduce the default geometry exactly: [`Y_GUTTER`] wide, no
/// axis-label headroom. A chart threads this from its cache key so a gutter or
/// unit change invalidates the cached layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisLayout {
    /// Requested left gutter, before clamping — see [`resolve_y_gutter`].
    pub y_gutter: f32,
    /// Extra headroom above the topmost tick for the y-axis unit label — see
    /// [`y_axis_label_reserve`]. `0.0` when there is no axis label.
    pub y_axis_label_reserve: f32,
}

impl Default for AxisLayout {
    fn default() -> Self {
        Self {
            y_gutter: Y_GUTTER,
            y_axis_label_reserve: 0.0,
        }
    }
}

/// [`plot_geometry`] with a caller-chosen [`AxisLayout`].
#[must_use]
pub fn plot_geometry_with(
    plot: Size,
    x_domain: (f64, f64),
    y_domain: (f64, f64),
    axis: AxisLayout,
) -> PlotGeometry {
    // The gutter is a deterministic function of `plot` size and the caller's
    // request — never of the data — so the decimation bound stays exact per
    // size. Clamp so a tiny frame degrades to a zero-area plot rather than a
    // negative one, and so no request can reserve the plot away.
    let inner_x = resolve_y_gutter(axis.y_gutter, plot.width);
    let inner_bottom = (plot.height - X_GUTTER).max(0.0);
    let inner_w = (plot.width - inner_x).max(0.0);
    let inner_h = inner_bottom.max(0.0);
    // Reserve `TOP_GUTTER` above the value scale so the domain-max y tick sits
    // low enough for its whole label to fit. Clamp so a short frame
    // (where TOP_GUTTER would exceed inner_h) still yields a non-inverted
    // range rather than a top below the bottom. An axis unit label adds its
    // own headroom on top so it never sits on the topmost tick label.
    let value_top = (TOP_GUTTER + axis.y_axis_label_reserve.max(0.0)).min(inner_h);

    let x_scale = LinearScale {
        domain: x_domain,
        range: (f64::from(inner_x), f64::from(inner_x + inner_w)),
    };
    let y_scale = LinearScale {
        domain: y_domain,
        range: (f64::from(inner_h), f64::from(value_top)),
    };

    PlotGeometry {
        inner_x,
        inner_w,
        inner_h,
        x_scale,
        y_scale,
    }
}

// --- tick labels -----------------------------------------------------------

/// A chart-axis formatter stored in a cache key.
///
/// The public builders accept a plain function pointer so non-capturing
/// closures coerce naturally. This wrapper gives all chart keys one explicit
/// address-based equality rule: swapping a formatter invalidates cached text,
/// while comparing function pointers never relies on Rust's unreliable direct
/// function-pointer equality.
#[derive(Clone, Copy)]
pub(crate) struct TickFormatter(fn(f64, f64) -> String);

impl TickFormatter {
    pub(crate) fn new(formatter: fn(f64, f64) -> String) -> Self {
        Self(formatter)
    }

    fn format(self, value: f64, step: f64) -> String {
        (self.0)(value, step)
    }
}

impl Default for TickFormatter {
    fn default() -> Self {
        Self(format_tick)
    }
}

impl std::fmt::Debug for TickFormatter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TickFormatter(..)")
    }
}

impl PartialEq for TickFormatter {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::fn_addr_eq(self.0, other.0)
    }
}

/// The number of decimal places needed to render `step` exactly, capped at 6.
///
/// A step of `0.25` needs 2 (not the 1 that `ceil(-log10)` gives), because
/// the tick values `0.25`, `0.75` carry a hundredths digit.
#[must_use]
pub fn decimals_for_step(step: f64) -> usize {
    let s = step.abs();
    if s >= 1.0 {
        return 0;
    }
    for d in 1..=6usize {
        let scaled = s * 10f64.powi(d as i32);
        if (scaled - scaled.round()).abs() < 1e-6 {
            return d;
        }
    }
    6
}

/// Format a tick value for an axis, at a precision derived from the tick
/// step so every label on one axis shares a format.
///
/// Exact zero is always the bare `"0"`. Large magnitudes get a compact
/// `k`/`M`/`B` suffix so a value like `1000000` does not overflow a narrow
/// gutter; the extremes the ladder does not cover fall back to scientific.
#[must_use]
pub fn format_tick(value: f64, step: f64) -> String {
    if value == 0.0 {
        // Exact zero is always "0", never "0.00" or "-0".
        return "0".to_owned();
    }
    let mag = value.abs();

    // Scientific fallback for the extremes the suffix ladder does not cover.
    if mag >= 1e12 || (mag > 0.0 && mag < 1e-4) {
        return format!("{value:.0e}");
    }

    // Suffix ladder for large magnitudes; the catalog spells each suffix.
    let (scaled, suffix_key) = if mag >= 1e9 {
        (value / 1e9, Some("enzui-chart-billions"))
    } else if mag >= 1e6 {
        (value / 1e6, Some("enzui-chart-millions"))
    } else if mag >= 1e5 {
        (value / 1e3, Some("enzui-chart-thousands"))
    } else {
        (value, None)
    };

    match suffix_key {
        None => {
            let decimals = decimals_for_step(step);
            localized_decimal(format!("{value:.decimals$}"))
        }
        Some(key) => {
            let s = format!("{scaled:.1}");
            let s = s.strip_suffix(".0").unwrap_or(&s).to_owned();
            enzui_core::localized!(key, "value" => localized_decimal(s))
        }
    }
}

/// `digits` (an ASCII decimal) in the current locale's numerals and separators.
fn localized_decimal(digits: String) -> String {
    digits
        .parse()
        .ok()
        .and_then(|number| {
            enzui_core::localization::current()
                .locale_for(enzui_core::localization::LocaleCategory::Numeric)
                .format_decimal(&number)
                .ok()
        })
        .unwrap_or(digits)
}

/// Exact rendered width of `label` at `size`px in `font`, shaped against the
/// process-wide font system. No live renderer is required — `min_bounds()`
/// shapes synchronously.
///
/// The `font` argument is load-bearing: the chart *renders* its axis labels in
/// the catalog's `Ui` family (via [`chart_label_font`]), so it must *measure*
/// them in that same font or the gutter/collision maths disagrees with what is
/// drawn — labels clip, overlap, or leave a wrongly-sized gutter the moment a
/// platform font preference moves the UI family off `Font::DEFAULT`.
/// This function stays theme-free; the caller resolves the `Font` once from
/// [`chart_label_font`] and threads the *same* value here and into `fill_text`.
#[must_use]
pub fn label_width(label: &str, size: f32, font: iced::Font) -> f32 {
    GraphicsParagraph::with_text(iced::advanced::text::Text {
        content: label,
        bounds: Size::new(f32::INFINITY, size.max(1.0)),
        size: size.into(),
        line_height: iced::widget::text::LineHeight::default(),
        font,
        align_x: iced::advanced::text::Alignment::Center,
        align_y: iced::alignment::Vertical::Center,
        shaping: iced::widget::text::Shaping::Advanced,
        wrapping: iced::widget::text::Wrapping::None,
        ellipsis: iced::widget::text::Ellipsis::End,
        hint_factor: None,
    })
    .min_bounds()
    .width
}

/// The spacing between the first two ticks, used to pick label precision.
/// Falls back to the whole span for a degenerate single-tick set.
#[must_use]
pub fn tick_step(ticks: &[f64]) -> f64 {
    match ticks {
        [a, b, ..] => (b - a).abs(),
        [only] => only.abs().max(1.0),
        [] => 1.0,
    }
}

/// Choose an x-tick set whose labels do not overlap at `plot_width`.
///
/// When labels collide the count is *reduced* (fewer, wider-spaced ticks)
/// rather than drawing every tick and hiding every other label. If even two
/// labels cannot fit, the x tick labels are omitted. Because
/// [`nice_ticks`] keeps ticks on the human
/// ladder, a lower target yields a *coarser* rung (10 → 20 → 50), not the same
/// ticks minus some — so every drawn tick keeps a label.
///
/// `label_font` must be the *same* [`chart_label_font`] value the chart hands to
/// `fill_text`: the collision test measures labels in the font they are painted
/// in, so a platform UI-font preference cannot make measurement and rendering
/// disagree.
#[must_use]
pub fn fit_x_ticks(
    domain: (f64, f64),
    plot_width: f32,
    label_size: f32,
    label_font: iced::Font,
) -> Vec<f64> {
    use enzui_core::chart::nice_ticks;
    let mut target = X_TICK_TARGET;
    loop {
        let ticks = nice_ticks(domain.0, domain.1, target);
        if ticks.len() < 2 {
            return ticks;
        }
        let step = tick_step(&ticks);
        let widest = ticks
            .iter()
            .map(|&t| label_width(&format_tick(t, step), label_size, label_font))
            .fold(0.0f32, f32::max);
        let budget = plot_width / (ticks.len() as f32 - 1.0);
        if widest + X_LABEL_MIN_GAP <= budget {
            return ticks;
        }
        if target <= 2 || ticks.len() == 2 {
            return Vec::new();
        }
        target -= 1;
    }
}

/// Whether a chart with these domains has a real (non-degenerate) extent to
/// label. A flat or empty chart draws chrome but no numbers.
#[must_use]
pub fn has_labelable_domain(x_domain: (f64, f64), y_domain: (f64, f64)) -> bool {
    x_domain.0 != x_domain.1 || y_domain.0 != y_domain.1
}

/// The position at which an x-axis tick label is centred, just below the plot.
///
/// Do not use for edge ticks. A centre-aligned label at `px` with width `w`
/// spans `[px - w/2, px + w/2]`. If a `nice_ticks` value lands on the domain
/// max the tick sits at `inner_x + inner_w` — the plot's right edge — and the
/// centred label spills half its width past the canvas, where it is clipped
/// Callers that know the label width must use [`x_label_layout`]
/// instead, which shifts to `Left`/`Right` alignment near the edges.
#[must_use]
pub fn x_label_position(px: f32, inner_h: f32) -> Point {
    Point::new(px, inner_h + 2.0)
}

/// The (position, alignment) pair for an x-axis tick label at pixel `px`,
/// knowing the label's rendered width and the plot's horizontal extent.
///
/// Centre-aligned by default, but shifts to `Left` alignment when the tick
/// sits near the plot's left edge and to `Right` alignment when it sits
/// near the right edge, so the label box stays inside `[0, plot_width]`
/// and no glyph is clipped by the canvas bound.
///
/// # Why alignment, not a clamped centre
///
/// Clamping the *centre* toward the plot rect would slide the label away from
/// its tick, making it read as belonging to the wrong tick. Switching to a
/// side alignment anchors the label's near edge to the plot edge and lets its
/// far edge grow inward — the label stays visually anchored to its tick and
/// no ink lands past the plot bound.
///
/// The threshold is `label_w/2 + 1.0` (half a label width, plus a pixel of
/// slack so a label that only *just* fits still centres): anything closer to
/// an edge than that would clip under `Center` alignment, so it switches to a
/// side alignment. `plot_width` is the whole canvas width, not `inner_w` —
/// the plot bounds run from `0` (not `inner_x`) on the right of a label,
/// because a label's left half may extend into the y-gutter (that is where
/// the leftmost x-tick's label lives).
#[must_use]
pub fn x_label_layout(
    px: f32,
    inner_h: f32,
    label_w: f32,
    plot_width: f32,
) -> (Point, iced::advanced::text::Alignment) {
    use iced::advanced::text::Alignment;
    let half = label_w * 0.5;
    let slack = 1.0;
    let y = inner_h + 2.0;
    // Right edge: a centred label at `px` would spill past `plot_width` when
    // `px + half > plot_width`. Anchor the label's right edge at the plot's
    // right edge instead.
    if px + half + slack > plot_width {
        (Point::new(plot_width, y), Alignment::Right)
    } else if px - half - slack < 0.0 {
        // Left edge: symmetric. The leftmost tick lives at `inner_x` which is
        // typically Y_GUTTER, so a centred label's left half sits in the
        // gutter — outside the plot rect but inside the canvas. It only
        // clips against x=0.
        (Point::new(0.0, y), Alignment::Left)
    } else {
        (Point::new(px, y), Alignment::Center)
    }
}

/// The position at which a y-axis tick label is right-aligned, in the gutter.
///
/// Right-aligned at `inner_x - Y_LABEL_GAP`, so a label of width `w` occupies
/// `[inner_x - Y_LABEL_GAP - w, inner_x - Y_LABEL_GAP]`: it grows *leftward*
/// into the gutter and never crosses `inner_x` into the plot.
#[must_use]
pub fn y_label_position(inner_x: f32, py: f32) -> Point {
    Point::new(inner_x - Y_LABEL_GAP, py)
}

/// Whether a y tick label of rendered width `label_w` fits inside the gutter
/// `[0, inner_x]` when right-aligned by [`y_label_position`], with no clipping
/// at the canvas's left edge and no ink crossing `inner_x` into the plot.
///
/// The right-aligned label spans `[inner_x - Y_LABEL_GAP - label_w,
/// inner_x - Y_LABEL_GAP]`. The right edge is `inner_x - Y_LABEL_GAP < inner_x`
/// for any positive gap, so the plot side is safe by construction; the binding
/// constraint is the left edge, `inner_x - Y_LABEL_GAP - label_w >= 0`.
#[must_use]
pub fn y_label_fits_gutter(label_w: f32, inner_x: f32) -> bool {
    label_w <= inner_x - Y_LABEL_GAP
}

/// The shared y-axis tick layout: one nice-tick computation, mapped through
/// the value scale, that drives both the horizontal grid lines and the y
/// tick labels.
///
/// # Why one struct feeds both
///
/// Grid lines at fixed fractions of the plot height would agree with labels
/// at [`nice_ticks`] values only when the ticks land on quarters; on every
/// other domain a label would name a line that means a different number.
/// Deriving the grid and the labels from the *same*
/// `pys` here makes "a line is a tick" structural: [`draw_y_grid`] strokes at
/// `pys[i]` and [`draw_y_tick_labels`] baselines the label at the *same*
/// `pys[i]`, so the two coordinate vectors are bit-identical, not merely close.
pub(crate) struct YAxisTicks {
    /// The nice tick values actually drawable (finite pixel), ascending.
    pub values: Vec<f64>,
    /// The tick step, used to pick the label formatter's precision.
    pub step: f64,
    /// The pixel `y` of each entry in [`Self::values`] (same index/length),
    /// filtered to the finite ones. This is the single coordinate list the
    /// grid *and* the labels both read.
    pub pys: Vec<f32>,
}

/// Compute the shared y-axis tick layout for `domain` mapped through `scale`.
///
/// The nice ticks are derived once, here; both the grid and the labels take
/// their coordinates from the returned [`YAxisTicks::pys`]. Non-finite mapped
/// pixels (a degenerate domain can produce them) are dropped from *both*
/// `values` and `pys` together, so the two stay index-aligned and every drawn
/// grid line keeps a label and vice versa.
pub(crate) fn y_axis_ticks(domain: (f64, f64), scale: LinearScale) -> YAxisTicks {
    let raw = nice_ticks(domain.0, domain.1, Y_TICK_TARGET);
    let step = tick_step(&raw);
    let mut values = Vec::with_capacity(raw.len());
    let mut pys = Vec::with_capacity(raw.len());
    for t in raw {
        let py = scale.map(t);
        if !py.is_finite() {
            continue;
        }
        values.push(t);
        pys.push(py as f32);
    }
    YAxisTicks { values, step, pys }
}

/// Stroke the shared horizontal grid — one line per y tick, at exactly the
/// pixel each tick's label is baselined on. Returns the grid-line `y`
/// coordinates in draw order, recorded at the `frame.stroke` call so the
/// [`DrawReport`](crate::line_chart::DrawReport) observable cannot drift from
/// what was painted.
///
/// Keeping the stroke here — fed the same [`YAxisTicks`] as
/// [`draw_y_tick_labels`] — is what prevents the next chart (or bar chart's
/// second multi-series draw path) from silently re-introducing a fixed-fraction
/// grid that disagrees with the labels.
pub(crate) fn draw_y_grid(
    frame: &mut Frame,
    ticks: &YAxisTicks,
    inner_x: f32,
    inner_w: f32,
    stroke: Stroke,
) -> Vec<f32> {
    let mut ys = Vec::with_capacity(ticks.pys.len());
    for &py in &ticks.pys {
        let line = Path::line(Point::new(inner_x, py), Point::new(inner_x + inner_w, py));
        frame.stroke(&line, stroke);
        ys.push(py);
    }
    ys
}

/// Paint the shared Cartesian y-axis tick labels and return the exact strings
/// handed to [`Frame::fill_text`].
///
/// Area, bar, line, and scatter charts have different mark geometry, but a y
/// tick has one policy: take the shared [`YAxisTicks`], pass both value and
/// step to the caller's formatter, and paint right-aligned text in the left
/// gutter. Keeping that path here prevents one chart (or bar chart's second
/// multi-series draw path) from silently bypassing a formatter added to the
/// others.
///
/// Returns `(texts, baseline_ys)`: the exact strings handed to
/// [`Frame::fill_text`] and the pixel `y` each label was baselined on — the
/// *same* `ticks.pys` values [`draw_y_grid`] strokes at, so a test can assert
/// line/label agreement from the report instead of guessing at pixels.
pub(crate) fn draw_y_tick_labels(
    frame: &mut Frame,
    ticks: &YAxisTicks,
    inner_x: f32,
    label_size: f32,
    label_font: iced::Font,
    color: Color,
    formatter: TickFormatter,
) -> (Vec<String>, Vec<f32>) {
    let mut painted = Vec::with_capacity(ticks.values.len());
    let mut baselines = Vec::with_capacity(ticks.values.len());
    for (t, &py) in ticks.values.iter().zip(&ticks.pys) {
        let content = formatter.format(*t, ticks.step);
        frame.fill_text(Text {
            content: content.clone(),
            position: y_label_position(inner_x, py),
            color,
            size: label_size.into(),
            font: label_font,
            align_x: iced::advanced::text::Alignment::Right,
            align_y: iced::alignment::Vertical::Center,
            ..Text::default()
        });
        painted.push(content);
        baselines.push(py);
    }
    (painted, baselines)
}

/// Paint the y-axis unit label at the top of the gutter, right-aligned on
/// the same `inner_x - Y_LABEL_GAP` edge every tick label is aligned to, and
/// return the exact string handed to [`Frame::fill_text`].
///
/// Returns `None` — painting nothing — when the chart has no axis label, which
/// is the default, preserving the no-label output of existing charts.
/// The label sits in the headroom [`y_axis_label_reserve`] added above the
/// topmost tick, so it cannot overprint a tick label.
pub(crate) fn draw_y_axis_label(
    frame: &mut Frame,
    label: AxisLabel,
    inner_x: f32,
    label_size: f32,
    label_font: iced::Font,
    color: Color,
) -> Option<String> {
    if label.is_empty() {
        return None;
    }
    let content = label.as_str().to_owned();
    frame.fill_text(Text {
        content: content.clone(),
        // Centred on the middle of its own reserved line, which starts at the
        // canvas top: the tick labels below start at TOP_GUTTER + reserve.
        position: Point::new(inner_x - Y_LABEL_GAP, label_size * 0.5),
        color,
        size: label_size.into(),
        font: label_font,
        align_x: iced::advanced::text::Alignment::Right,
        align_y: iced::alignment::Vertical::Center,
        ..Text::default()
    });
    Some(content)
}

/// The resolved `iced::Font` for chart axis / tick labels.
///
/// Chart labels use the UI family and regular weight rung, with upright style
/// and normal stretch. Compiled defaults reproduce `Font::DEFAULT`.
///
/// A theme extension takes precedence over the published application defaults.
#[must_use]
pub fn chart_label_font(theme: &iced::Theme) -> iced::Font {
    match crate::theme_lookup::enz_theme(theme) {
        Some(enz) => enz.resolve_font(
            enzui_theme::FontRole::Ui,
            enz.typography.iced_weight(enzui_theme::WeightRung::Regular),
            iced::font::Style::Normal,
            iced::font::Stretch::Normal,
        ),
        None => enzui_theme::text_defaults::font(
            enzui_theme::FontRole::Ui,
            enzui_theme::WeightRung::Regular,
        ),
    }
}
