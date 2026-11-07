//! Accessible pie and donut charts with a bounded number of slices.
//!
//! # Slices
//!
//! Each [`PieSlice`] has a stable id, a label, a value in any unit, and an
//! optional colour; the widget derives every angle and percentage. Ids must
//! be unique and non-empty: later duplicates and the reserved
//! [`RESERVED_OTHER_ID`] are dropped (see [`resolve_slices`]). Non-finite and
//! negative values are excluded, and a zero total renders the empty state.
//! At most [`MAX_SLICES`] slices are drawn; the rest are aggregated into one
//! "Other" slice, so drawing and description cost stay bounded.
//!
//! Selection is driven by the host through [`selected`](EnzPieChart::selected)
//! and keyed by id; the chart itself handles no pointer or keyboard input.
//!
//! # Geometry
//!
//! The plot is the largest square that fits, centred. Slices start at
//! 12 o'clock and run clockwise, like [`gauge`](crate::gauge).
//! [`donut`](EnzPieChart::donut) carves a hole whose ratio is clamped to
//! `[0, MAX_INNER_RATIO]`.
//!
//! # Accessibility
//!
//! The chart publishes one `Role::Group` node named by its title, with a
//! summary of up to [`MAX_DESCRIBED_SLICES`] labels and percentages.
//! [`data_table`](EnzPieChart::data_table) returns the same rows for an
//! equivalent accessible table.
//!
//! # Performance
//!
//! Wedges are cached separately from the selection highlight, so changing the
//! selection redraws only the highlight overlay.

use crate::chart_legend::ChartLegendEntry;
use crate::chart_palette::ChartPalette;
use enzui_core::semantics::{Role as SemRole, Semantics};
use enzui_theme::Palette;
use iced::widget::canvas::{self, Cache, Path, Program, Stroke};
use iced::widget::{center, container, text};
use iced::{Color, Element, Length, Point, Rectangle, Renderer};
use std::cell::RefCell;
use std::collections::HashSet;

// Thread-local counters of wedge and highlight tessellations, read by tests
// to prove a selection change re-tessellates only the highlight. They are
// thread-local so parallel tests drawing other charts do not disturb them.
mod tessellation_counters {
    use std::cell::Cell;

    thread_local! {
        pub(super) static WEDGE: Cell<usize> = const { Cell::new(0) };
        pub(super) static HIGHLIGHT: Cell<usize> = const { Cell::new(0) };
    }
}

// Bump this thread's wedge-tessellation count. Called from inside
// the base cache closure, so a replayed (cache-hit) draw does not
// bump it.
#[inline]
pub(crate) fn note_wedge_tessellation() {
    tessellation_counters::WEDGE.with(|c| c.set(c.get().wrapping_add(1)));
}

// Bump this thread's highlight-tessellation count.
#[inline]
pub(crate) fn note_highlight_tessellation() {
    tessellation_counters::HIGHLIGHT.with(|c| c.set(c.get().wrapping_add(1)));
}

/// Reset this thread's wedge-tessellation count and return its
/// previous value.
#[doc(hidden)]
pub fn reset_wedge_tessellations() -> usize {
    tessellation_counters::WEDGE.with(|c| c.replace(0))
}

/// Reset this thread's highlight-tessellation count and return its
/// previous value.
#[doc(hidden)]
pub fn reset_highlight_tessellations() -> usize {
    tessellation_counters::HIGHLIGHT.with(|c| c.replace(0))
}

/// Maximum number of rendered slices. Beyond this the tail is aggregated into
/// one synthetic "other" slice.
///
/// A pie with more than a couple of dozen wedges is unreadable long before it
/// is slow, so the bound is a legibility decision that also happens to cap
/// geometry cost. This is the documented slice and semantic bound.
pub const MAX_SLICES: usize = 24;

/// Maximum number of slices named individually in the accessible summary.
/// The remainder is aggregated so the spoken description stays bounded.
pub const MAX_DESCRIBED_SLICES: usize = 8;

/// Maximum donut `inner_ratio`. Above this the ring is too thin to read and
/// degenerates visually.
pub const MAX_INNER_RATIO: f32 = 0.95;

/// Label used for the aggregated tail slice.
pub const OTHER_LABEL: &str = "Other";

/// Reserved slice ID used internally for the aggregated tail slice.
/// Callers must not use this ID — slices carrying it are dropped
/// without notice because they would collide with the synthetic
/// "Other" aggregation entry.
pub const RESERVED_OTHER_ID: &str = "__enz_other__";

/// A stable slice identifier. Selection is expressed in terms of this, never
/// an index, so reordering or re-binding the data cannot move the selection.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SliceId(pub String);

impl SliceId {
    /// Wrap a string as a stable slice id.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
    /// Borrow the id as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SliceId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<String> for SliceId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// One typed slice: a stable id, a human label, a finite non-negative original
/// value, and an optional colour override.
#[derive(Debug, Clone, PartialEq)]
#[must_use]
pub struct PieSlice {
    /// Stable identity, used as the key for caller-driven selection and for the
    /// accessible description. Not an interaction handle: the widget does no
    /// pointer or keyboard hit testing.
    pub id: SliceId,
    /// Human-readable label (legend, tooltip, accessible summary).
    pub label: String,
    /// The caller's original magnitude, in any unit. Non-finite or negative
    /// values are invalid and are excluded from the total.
    pub value: f32,
    /// Optional semantic colour. `None` (or fully transparent) falls back to
    /// the categorical palette entry for the slice's position.
    pub color: Option<Color>,
}

impl PieSlice {
    /// Build a slice with a stable id, label and magnitude. No colour override
    /// (falls back to the categorical palette).
    pub fn new(id: impl Into<SliceId>, label: impl Into<String>, value: f32) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            value,
            color: None,
        }
    }

    /// Override this slice's fill colour.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }

    /// Whether this slice contributes to the chart. Invalid (non-finite,
    /// negative) and zero values do not.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.value.is_finite() && self.value > 0.0
    }
}

/// A slice after validation, aggregation, and share computation. This is what
/// the geometry, the accessible summary, and the data-table hook all read, so
/// they cannot disagree.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedSlice {
    /// Stable identity carried through from the source [`PieSlice`].
    pub id: SliceId,
    /// Human-readable label carried through from the source slice.
    pub label: String,
    /// The original (or aggregated) magnitude.
    pub value: f32,
    /// Share of the valid total, in `[0, 1]`.
    pub fraction: f32,
    /// Sweep start as a fraction of the full turn, measured clockwise from 12
    /// o'clock.
    pub start: f32,
    /// Palette index this slice draws from when it has no colour override.
    pub palette_index: usize,
    /// Colour override, if any.
    pub color: Option<Color>,
    /// True for the synthetic aggregated tail slice.
    pub is_other: bool,
}

impl ResolvedSlice {
    /// Share as a percentage in `[0, 100]`.
    #[must_use]
    pub fn percent(&self) -> f32 {
        self.fraction * 100.0
    }
}

/// Validate, deduplicate, bound, aggregate, and compute shares for a slice
/// list. Returns an empty vec when nothing is valid (empty, all zero, all
/// invalid), so callers render the empty state. One O(n) pass over the input
/// and one output allocation bounded by [`MAX_SLICES`].
///
/// # Duplicate and reserved IDs
///
/// Selection relies on a stable [`SliceId`], so IDs must be unique. The first
/// slice with an ID wins and later duplicates are dropped, resolving the
/// ambiguity rather than propagating it. Slices with the reserved
/// [`RESERVED_OTHER_ID`] or an empty ID are dropped too. Debug builds fire a
/// `debug_assert!` on a rejected slice so developers see it early; release
/// builds drop silently.
#[must_use]
pub fn resolve_slices(slices: &[PieSlice]) -> Vec<ResolvedSlice> {
    // Phase 1: validate and deduplicate.
    //
    // Only valid slices contribute: `v > 0.0`, with non-finite values
    // rejected explicitly rather than relying on NaN comparisons
    // happening to be false.
    //
    // Deduplicate by id: duplicate IDs make selection ambiguous
    // because `find` returns the first match. First occurrence
    // wins; subsequent duplicates, reserved IDs and empty IDs are
    // dropped. `debug_assert!` fires so developers notice in
    // debug builds; in release builds the drop is silent.
    let mut seen = HashSet::with_capacity(slices.len());
    let valid: Vec<&PieSlice> = slices
        .iter()
        .filter(|s| s.is_valid())
        .filter(|s| {
            // Reject the reserved aggregation id.
            if s.id.as_str() == RESERVED_OTHER_ID {
                log::warn!(
                    target: "enzui::chart",
                    "[enzui-widgets] PieSlice id {RESERVED_OTHER_ID:?} is reserved for \
                     the aggregated tail — dropped; use a different id",
                );
                return false;
            }
            // Reject empty ids — they are not meaningfully selectable.
            if s.id.as_str().is_empty() {
                log::warn!(
                    target: "enzui::chart",
                    "[enzui-widgets] PieSlice has an empty id — \
                     dropped; every slice needs a non-empty stable identifier",
                );
                return false;
            }
            // Reject duplicate ids — first occurrence wins.
            if !seen.insert(&s.id) {
                log::warn!(
                    target: "enzui::chart",
                    "[enzui-widgets] PieSlice id {:?} appears more \
                     than once — duplicate dropped; use unique ids",
                    s.id.as_str(),
                );
                return false;
            }
            true
        })
        .collect();

    // Summed in f64: legal f32 values (two 2e38 slices) would overflow an f32
    // total to infinity. Duplicates are already dropped, so they cannot
    // inflate it.
    let total: f64 = valid.iter().map(|s| f64::from(s.value)).sum();
    if !total.is_finite() || total <= 0.0 {
        return Vec::new();
    }

    // Bound the rendered/described set: keep the first MAX_SLICES - 1 and
    // aggregate the rest, so the tail is represented rather than dropped.
    let (head, tail): (&[&PieSlice], &[&PieSlice]) = if valid.len() > MAX_SLICES {
        valid.split_at(MAX_SLICES - 1)
    } else {
        (&valid[..], &[])
    };

    let mut out = Vec::with_capacity(head.len() + usize::from(!tail.is_empty()));
    let mut acc = 0.0f32;
    for (i, s) in head.iter().enumerate() {
        // The ratio value/total is well-conditioned: both operands are finite
        // and the result lies in [0, 1], so narrowing it to f32 for storage in
        // `PieRow::fraction` loses only sub-ULP precision and cannot overflow.
        // Computing it in f64 (rather than f32) is what avoids the empty-chart
        // bug — the division itself, not just the sum, would otherwise be
        // f32(inf) / f32(inf) = NaN when the total overflows.
        let fraction = (f64::from(s.value) / total) as f32;
        out.push(ResolvedSlice {
            id: s.id.clone(),
            label: s.label.clone(),
            value: s.value,
            fraction,
            start: acc,
            palette_index: i,
            color: s.color,
            is_other: false,
        });
        acc += fraction;
    }
    if !tail.is_empty() {
        // Summed in f64 like the total, then saturated to a finite f32.
        let value = sane_geom((tail.iter().map(|s| f64::from(s.value)).sum::<f64>()) as f32);
        out.push(ResolvedSlice {
            id: SliceId::new(RESERVED_OTHER_ID),
            label: enzui_core::localized!("enzui-pie-chart-slices", "count" => tail.len()),
            value,
            // Use the remaining share rather than value/total so the sweeps
            // always close the circle exactly despite float accumulation.
            fraction: (1.0 - acc).max(0.0),
            start: acc,
            palette_index: head.len(),
            color: None,
            is_other: true,
        });
    }
    out
}

// The public geometry helpers saturate every scalar to a finite value
// (`NaN` to 0), so no input can hand the renderer a non-finite vertex.

// Saturate a scalar to the finite range `[-f32::MAX, f32::MAX]`,
// mapping `NaN` to `0.0` (a NaN has no direction to preserve).
// `±inf` maps to the signed boundary so a huge translation still
// points in its original direction rather than collapsing to the
// origin.
//
// The pie-chart mirror of `Placement::saturate_finite`.
#[inline]
#[must_use]
fn sane_geom(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        // `f32::clamp` is well-defined for ±inf vs finite lo/hi: it clamps
        // to the nearest bound, which is exactly what saturation is.
        v.clamp(-f32::MAX, f32::MAX)
    }
}

/// Colour + geometry tokens for the pie chart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PieChartTokens {
    /// Thin separator stroked between slices.
    pub separator_color: Color,
    /// Separator width.
    pub separator_width: f32,
    /// Empty-state text colour (`text-muted`).
    pub empty_color: Color,
    /// Categorical series palette used when a slice has no colour override.
    pub palette: ChartPalette,
    /// Colour blended over the caller-selected slice (there is no hover; the
    /// highlight is driven by [`EnzPieChart::selected`], not by the pointer).
    pub highlight_color: Color,
}

impl PieChartTokens {
    /// Derive tokens from a runtime palette (the normal path).
    pub fn from_palette(p: &Palette) -> Self {
        Self {
            separator_color: p.surface,
            separator_width: SEPARATOR_WIDTH,
            empty_color: p.text_muted,
            palette: ChartPalette::from_palette(p),
            highlight_color: p.text,
        }
    }
    /// Tokens for the built-in dark palette.
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }
    /// Tokens for the built-in light palette.
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
    /// Resolve a slice's fill: override when opaque, else the palette entry.
    pub fn fill_for(&self, slice: &ResolvedSlice) -> Color {
        self.palette
            .resolve_or_series(slice.color, slice.palette_index)
    }
}

/// Legend rows for the resolved slices, coloured by the same rule as the
/// wedges, so a legend beside the chart always matches it. Takes
/// [`ResolvedSlice`] because the synthetic "Other" slice exists only after
/// [`resolve_slices`].
#[must_use]
pub fn legend_entries_for(
    slices: &[ResolvedSlice],
    palette: &ChartPalette,
) -> Vec<ChartLegendEntry> {
    slices
        .iter()
        .map(|s| {
            // The same call the wedge uses
            // (`PieChartTokens::fill_for`). Deliberately not
            // `if let Some(c) = s.color { .. } else { .. }` —
            // that would be a second colour rule.
            let color = palette.resolve_or_series(s.color, s.palette_index);
            ChartLegendEntry::new(s.label.clone(), color)
        })
        .collect()
}

/// Default separator width.
pub const SEPARATOR_WIDTH: f32 = 1.0;
/// Default plot size when the caller does not constrain it.
pub const DEFAULT_SIZE: f32 = 200.0;
// Opacity of the caller-selection highlight overlay.
const HIGHLIGHT_OPACITY: f32 = 0.18;

/// The angle in radians of a turn fraction measured clockwise from
/// 12 o'clock in iced's y-down space. Non-finite input yields `0.0`, so the
/// result is always finite.
#[must_use]
pub fn angle_at(fraction: f32) -> f32 {
    sane_geom(sane_geom(fraction) * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2)
}

/// Clamp a donut ratio into `[0, MAX_INNER_RATIO]`; non-finite becomes 0.
#[must_use]
pub fn sane_inner_ratio(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, MAX_INNER_RATIO)
    } else {
        0.0
    }
}

/// The largest centred square that fits `bounds` — the plot area:
/// `min(width, height)`, offset by half the leftover.
///
/// Non-finite `width`/`height` are saturated at `[-f32::MAX, f32::MAX]`
/// before use. Without saturation an infinite bound produced
/// `side = inf`, then `(inf - inf)/2 = NaN` for `x`/`y` — a NaN rectangle
/// the renderer will not tolerate. The `.max(0.0)` guard on `side` treats
/// a negative width the same as zero.
#[must_use]
pub fn plot_square(width: f32, height: f32) -> Rectangle {
    let w = sane_geom(width);
    let h = sane_geom(height);
    let side = sane_geom(w.min(h).max(0.0));
    Rectangle {
        x: sane_geom((w - side) / 2.0),
        y: sane_geom((h - side) / 2.0),
        width: side,
        height: side,
    }
}

// Point on the circle centred at `center` for polar
// `(radius, angle)`.
//
// Every scalar is saturated with [`sane_geom`] before the
// arithmetic so a non-finite radius or angle cannot produce a
// `NaN` point. `inf * cos(θ)` would otherwise flow directly into
// the returned `Point`, which iced then hands to the renderer as
// a NaN vertex.
fn polar(center: Point, radius: f32, angle: f32) -> Point {
    let cx = sane_geom(center.x);
    let cy = sane_geom(center.y);
    let r = sane_geom(radius);
    let a = sane_geom(angle);
    Point::new(sane_geom(cx + r * a.cos()), sane_geom(cy + r * a.sin()))
}

// Appends an arc from `a0` to `a1` (negative sweeps run counter-clockwise)
// to the current sub-path. `Builder::arc` cannot be used: it starts a new
// sub-path, which breaks wedges and donut segments. Emits cubic Béziers of
// at most a quarter turn each.
fn arc_to_current(b: &mut canvas::path::Builder, center: Point, radius: f32, a0: f32, a1: f32) {
    let sweep = a1 - a0;
    if radius <= 0.0 || sweep.abs() < 1e-6 {
        return;
    }
    // Segment count: at most a quarter turn each, so the Bézier error stays
    // negligible even for a full-circle single slice.
    let steps = (sweep.abs() / std::f32::consts::FRAC_PI_2).ceil().max(1.0);
    let step = sweep / steps;
    // Control-point handle length for a circular arc of angle `step`.
    let k = 4.0 / 3.0 * (step / 4.0).tan();

    let mut theta = a0;
    for _ in 0..(steps as u32) {
        let next = theta + step;
        let p0 = polar(center, radius, theta);
        let p1 = polar(center, radius, next);
        // Tangents at the endpoints, scaled by the handle length. Every
        // component is saturated: with a saturated `radius`, the arithmetic
        // `p0.x - k * radius * theta.sin()` can still overflow when radius
        // is near `f32::MAX` (two near-f32::MAX summands can sum to inf).
        // `polar` already saturates its output, so the bezier controls are
        // the last place a non-finite scalar could escape into lyon.
        let c0 = Point::new(
            sane_geom(p0.x - k * radius * theta.sin()),
            sane_geom(p0.y + k * radius * theta.cos()),
        );
        let c1 = Point::new(
            sane_geom(p1.x + k * radius * next.sin()),
            sane_geom(p1.y - k * radius * next.cos()),
        );
        b.bezier_curve_to(c0, c1, p1);
        theta = next;
    }
}

/// The two endpoints of the radial separator at `start`: from the centre to
/// the rim for a pie, across the ring for a donut. Only this edge is stroked,
/// so a thin ring keeps its thickness. Non-finite inputs are saturated and
/// negative radii treated as zero.
#[must_use]
pub fn separator_endpoints(center: Point, r_out: f32, r_in: f32, start: f32) -> (Point, Point) {
    let r_out = sane_geom(r_out).max(0.0);
    let r_in = sane_geom(r_in).max(0.0);
    let a = angle_at(start);
    (polar(center, r_in, a), polar(center, r_out, a))
}

/// Build the path for the radial separator at sweep fraction `start`: a single
/// straight line on the boundary between a slice and its predecessor. Every
/// contiguous slice starts where the previous ended, so drawing this line at
/// each slice's `start` draws every interior boundary exactly once; because
/// the first slice starts at `0.0`, that same line is the wrap-around boundary
/// between the last slice and the first. A single slice has `start == 0.0` as
/// its only boundary and no adjacent neighbour, so the caller must not draw it
/// (guarded by `slices.len() > 1`).
#[must_use]
pub fn separator_path(center: Point, r_out: f32, r_in: f32, start: f32) -> Path {
    let (inner, outer) = separator_endpoints(center, r_out, r_in, start);
    Path::new(|b| {
        b.move_to(inner);
        b.line_to(outer);
    })
}

/// Build the path for one slice: a wedge (pie) or an annular segment (donut).
///
/// `pub(crate)` because [`crate::gauge`]'s compact-disc fallback needs
/// exactly this wedge — same 12 o'clock origin (see
/// `angle_at_starts_at_twelve_oclock`, which pins `angle_at(0.0) ==
/// gauge::START_ANGLE`), same clockwise direction. Reusing it rather than
/// re-deriving a sector in `gauge.rs` keeps the `arc_to_current` sub-path trap
/// documented above solved in one place instead of two.
pub(crate) fn slice_path(center: Point, r_out: f32, r_in: f32, start: f32, sweep: f32) -> Path {
    // Saturate the centre and radii, and clamp the angles to one turn: a
    // non-finite or enormous input otherwise reaches lyon, which panics on a
    // non-finite point or spins on an unbounded sweep.
    let center = Point::new(sane_geom(center.x), sane_geom(center.y));
    let r_out = sane_geom(r_out).max(0.0);
    let r_in = sane_geom(r_in).max(0.0);
    let start = sane_geom(start).clamp(-1.0, 1.0);
    let sweep = sane_geom(sweep).clamp(-1.0, 1.0);
    let a0 = angle_at(start);
    let a1 = angle_at(start + sweep);
    Path::new(|b| {
        if r_in > 0.0 {
            // Annular segment: outer arc clockwise, radial line in, inner arc
            // back counter-clockwise, close.
            b.move_to(polar(center, r_out, a0));
            arc_to_current(b, center, r_out, a0, a1);
            b.line_to(polar(center, r_in, a1));
            arc_to_current(b, center, r_in, a1, a0);
        } else {
            // Wedge: centre, out to the arc start, arc, close back to centre.
            b.move_to(center);
            b.line_to(polar(center, r_out, a0));
            arc_to_current(b, center, r_out, a0, a1);
        }
        b.close();
    })
}

// The inputs that change the static wedge geometry. Deliberately
// excludes the selection highlight so a selection change never
// retessellates the base wedges. There is no pointer hover;
// "selection" here is the caller-driven
// [`EnzPieChart::selected`] id.
#[derive(Debug, Clone, PartialEq)]
struct BaseKey {
    slices: Vec<(f32, f32, [u8; 4])>,
    inner_ratio: f32,
    separator: Option<(Color, f32)>,
}

// What the overlay cache is keyed on: the selected slice's `(start, sweep)`,
// the donut's `inner_ratio` (which makes the segment annular) and the
// `highlight_color` (its fill). Leaving any out returns a stale overlay when
// only that input changes. Colours are discretised to `[u8; 4]` RGBA so they
// hash and compare deterministically, which an f32 `Color` does not.
#[derive(Debug, Clone, PartialEq)]
struct OverlayKey {
    slice: Option<(f32, f32)>,
    inner_ratio: f32,
    highlight_color: [u8; 4],
}

/// Cache state: static wedges and the highlight overlay are cached separately
/// so a selection change only invalidates the (cheap) overlay. (The highlight
/// is caller-driven; the widget does no pointer hover — see the module note.)
#[derive(Debug, Default)]
pub struct PieChartState {
    base: Cache,
    overlay: Cache,
    base_key: RefCell<Option<BaseKey>>,
    overlay_key: RefCell<Option<OverlayKey>>,
}

#[derive(Debug, Clone)]
struct PieChartProgram {
    slices: Vec<ResolvedSlice>,
    inner_ratio: f32,
    show_separator: bool,
    pinned_tokens: Option<PieChartTokens>,
    // The highlighted slice's (start, sweep), resolved from the
    // selected id.
    highlight: Option<(f32, f32)>,
}

impl PieChartProgram {
    fn tokens(&self, theme: &iced::Theme) -> PieChartTokens {
        crate::theme_lookup::resolve_pinned(
            crate::theme_lookup::PIN_SITE_PIE_CHART,
            self.pinned_tokens,
            theme,
            PieChartTokens::from_palette,
        )
    }

    fn base_key(&self, t: &PieChartTokens) -> BaseKey {
        BaseKey {
            slices: self
                .slices
                .iter()
                .map(|s| {
                    let c = t.fill_for(s);
                    (
                        s.start,
                        s.fraction,
                        [
                            (c.r * 255.0) as u8,
                            (c.g * 255.0) as u8,
                            (c.b * 255.0) as u8,
                            (c.a * 255.0) as u8,
                        ],
                    )
                })
                .collect(),
            inner_ratio: self.inner_ratio,
            separator: self
                .show_separator
                .then_some((t.separator_color, t.separator_width)),
        }
    }
}

impl<Message> Program<Message> for PieChartProgram {
    type State = PieChartState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        theme: &iced::Theme,
        bounds: Rectangle,
        // The cursor is intentionally ignored: this widget does no pointer hit
        // testing. Selection is caller-driven via `EnzPieChart::selected`.
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let t = self.tokens(theme);
        // `plot_square` already saturates its inputs; the derived scalars
        // stay finite by construction and no additional guard is needed here
        // Non-finite geometry must not reach the renderer.
        let plot = plot_square(bounds.width, bounds.height);
        let center = Point::new(plot.x + plot.width / 2.0, plot.y + plot.height / 2.0);
        let r_out = plot.width / 2.0;
        let r_in = self.inner_ratio * r_out;

        // Static wedges: invalidated only by geometry/colour, never by the
        // caller-driven selection highlight.
        let key = self.base_key(&t);
        if state.base_key.borrow().as_ref() != Some(&key) {
            state.base.clear();
            *state.base_key.borrow_mut() = Some(key);
        }
        let base = state.base.draw(renderer, bounds.size(), |frame| {
            note_wedge_tessellation();
            if r_out <= 0.0 {
                return;
            }
            // Fill every wedge first, then draw separators on the radial
            // boundaries only. Filling first keeps a later slice's fill from
            // painting over an earlier separator; the separators, being the
            // last thing drawn, sit cleanly on the seams.
            for s in &self.slices {
                if s.fraction <= 0.0 {
                    continue;
                }
                let path = slice_path(center, r_out, r_in, s.start, s.fraction);
                frame.fill(&path, t.fill_for(s));
            }
            // Stroke only the radial seam between adjacent slices, so a thin
            // donut keeps its full thickness. A single slice has no seam.
            let sep_w = sane_geom(t.separator_width);
            if self.show_separator && self.slices.len() > 1 && sep_w > 0.0 {
                let stroke = Stroke::default()
                    .with_width(sep_w)
                    .with_color(t.separator_color);
                for s in &self.slices {
                    if s.fraction <= 0.0 {
                        continue;
                    }
                    // Draw the boundary at each slice's start. Contiguous
                    // starts mean this covers every interior seam once; the
                    // slice starting at 0.0 draws the last->first wrap-around
                    // boundary.
                    frame.stroke(&separator_path(center, r_out, r_in, s.start), stroke);
                }
            }
        });

        // The highlight has its own cache, keyed by every input of its
        // geometry, so a selection change redraws only the overlay.
        let overlay_key = OverlayKey {
            slice: self.highlight,
            inner_ratio: self.inner_ratio,
            highlight_color: [
                (t.highlight_color.r * 255.0) as u8,
                (t.highlight_color.g * 255.0) as u8,
                (t.highlight_color.b * 255.0) as u8,
                (t.highlight_color.a * 255.0) as u8,
            ],
        };
        if state.overlay_key.borrow().as_ref() != Some(&overlay_key) {
            state.overlay.clear();
            *state.overlay_key.borrow_mut() = Some(overlay_key);
        }
        let overlay = state.overlay.draw(renderer, bounds.size(), |frame| {
            note_highlight_tessellation();
            if let Some((start, sweep)) = self.highlight
                && r_out > 0.0
                && sweep > 0.0
            {
                let path = slice_path(center, r_out, r_in, start, sweep);
                let mut c = t.highlight_color;
                c.a = HIGHLIGHT_OPACITY;
                frame.fill(&path, c);
            }
        });

        vec![base, overlay]
    }
}

/// How the host's selected id resolved against the drawn slices. A slice
/// aggregated into "Other" is reported as [`Aggregated`](Self::Aggregated)
/// with the requested id, so the host can say the selection is inside
/// "Other".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedSelection {
    /// Nothing is selected, or the caller’s id does not name any legal slice
    /// in the input (typo, empty id, reserved id, non-finite/zero value, or
    /// dropped as a duplicate).
    None,
    /// The id names a wedge that is drawn. The highlight paints
    /// that wedge.
    Direct(SliceId),
    /// The id names a legal input slice, but that slice was
    /// aggregated into the synthetic “Other” wedge. The highlight
    /// paints the Other wedge, preserving the caller's selection
    /// identity onto the wedge that contains the requested slice.
    Aggregated {
        /// The id the caller passed to [`EnzPieChart::selected`].
        /// Carried verbatim so a consumer can render explanatory
        /// copy without maintaining a second copy of the id.
        requested: SliceId,
    },
}

impl ResolvedSelection {
    /// The id the highlight geometry paints against, if any.
    ///
    /// * [`Direct`](Self::Direct) → the direct id.
    /// * [`Aggregated`](Self::Aggregated) → the reserved “Other”
    ///   id ([`RESERVED_OTHER_ID`]) — the wedge the highlight
    ///   paints in the aggregated case.
    /// * [`None`](Self::None) → `None`.
    #[must_use]
    pub fn painted_id(&self) -> Option<SliceId> {
        match self {
            Self::None => None,
            Self::Direct(id) => Some(id.clone()),
            Self::Aggregated { .. } => Some(SliceId::new(RESERVED_OTHER_ID)),
        }
    }

    /// The caller’s original id, if a selection was made.
    #[must_use]
    pub fn requested_id(&self) -> Option<&SliceId> {
        match self {
            Self::None => None,
            Self::Direct(id) => Some(id),
            Self::Aggregated { requested } => Some(requested),
        }
    }
}

/// An accessible, bounded pie / donut chart.
#[derive(Debug, Clone)]
#[must_use]
pub struct EnzPieChart {
    title: String,
    slices: Vec<PieSlice>,
    inner_ratio: f32,
    size: Option<f32>,
    show_separator: bool,
    selected: Option<SliceId>,
    empty_text: String,
    tokens: Option<PieChartTokens>,
}

impl EnzPieChart {
    /// Carve a donut hole. `ratio` is clamped to `[0, MAX_INNER_RATIO]`;
    /// non-finite becomes 0 (a full pie).
    pub fn donut(mut self, ratio: f32) -> Self {
        self.inner_ratio = sane_inner_ratio(ratio);
        self
    }

    /// Pin the plot to a fixed square side in px. By default the chart fills
    /// the available space and centres the largest square that fits.
    pub fn size(mut self, size: f32) -> Self {
        self.size = enzui_core::metrics::FinitePositive::new(size)
            .map(enzui_core::metrics::FinitePositive::get);
        self
    }

    /// Whether to stroke a thin separator between slices (default true).
    pub fn separator(mut self, show: bool) -> Self {
        self.show_separator = show;
        self
    }

    /// Highlights the slice with this id. An unknown id highlights nothing.
    /// A real slice that was aggregated into "Other" highlights the "Other"
    /// wedge; [`resolved_selection`](EnzPieChart::resolved_selection) reports
    /// it as [`Aggregated`](ResolvedSelection::Aggregated).
    pub fn selected(mut self, id: impl Into<SliceId>) -> Self {
        self.selected = Some(id.into());
        self
    }

    /// Text shown when there is nothing valid to plot.
    pub fn empty_text(mut self, t: impl Into<String>) -> Self {
        self.empty_text = t.into();
        self
    }

    /// Pin the colour tokens (skips runtime-theme resolution).
    pub fn tokens(mut self, tokens: PieChartTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// The validated, bounded, share-resolved slices this chart draws. A
    /// host rendering an accessible data table from these rows matches what
    /// is drawn and spoken, because all three read this.
    #[must_use]
    pub fn data_table(&self) -> Vec<ResolvedSlice> {
        resolve_slices(&self.slices)
    }

    /// How the caller's `selected` id resolves against the rendered rows:
    /// the event a host queries to react when a selection lands on the
    /// aggregated tail rather than a drawn wedge. It mirrors the geometry:
    /// `None` when nothing is selected, for an id that is not a slice (a typo
    /// or stale id), and for the reserved [`RESERVED_OTHER_ID`], which is never
    /// a legal caller id; [`Direct`](ResolvedSelection::Direct) for a drawn
    /// wedge; [`Aggregated`](ResolvedSelection::Aggregated) for a real slice
    /// folded into "Other", which highlights the Other wedge. Callers that
    /// want the selection silently cleared treat `Aggregated` as clear;
    /// callers that explain the collapse render a chip naming
    /// `Aggregated::requested`.
    #[must_use]
    pub fn resolved_selection(&self) -> ResolvedSelection {
        let Some(id) = self.selected.as_ref() else {
            return ResolvedSelection::None;
        };
        // The reserved id is never a legal caller selection: the widget owns
        // it as the synthetic aggregation key, so accepting it here would
        // let a caller “select Other” directly and confuse `Aggregated`’s
        // documented meaning (“your requested slice is inside Other”).
        if id.as_str() == RESERVED_OTHER_ID || id.as_str().is_empty() {
            return ResolvedSelection::None;
        }
        let rows = self.data_table();
        // A drawn slice, excluding the synthetic "Other" row.
        if rows.iter().any(|r| !r.is_other && &r.id == id) {
            return ResolvedSelection::Direct(id.clone());
        }
        // Not drawn: aggregated if it is a valid input slice and the chart
        // has an "Other" row; otherwise unknown.
        let has_other = rows.last().is_some_and(|r| r.is_other);
        if !has_other {
            return ResolvedSelection::None;
        }
        let survives_validation = self.slices.iter().any(|s| s.is_valid() && &s.id == id);
        if survives_validation {
            ResolvedSelection::Aggregated {
                requested: id.clone(),
            }
        } else {
            ResolvedSelection::None
        }
    }

    /// Theme-coherent legend entries for this chart: one
    /// [`ChartLegendEntry`] per drawn slice, including the aggregated
    /// remainder, with swatches resolved by the same
    /// [`resolve_or_series`](ChartPalette::resolve_or_series) call the wedges
    /// use, so the legend cannot disagree with the picture. It uses the
    /// tokens the chart draws with (pinned [`tokens`](EnzPieChart::tokens) or
    /// [`from_palette`](PieChartTokens::from_palette) from the runtime theme),
    /// so the colours track automatically. See [`legend_entries_for`].
    #[must_use]
    pub fn legend_entries(&self, tokens: &PieChartTokens) -> Vec<ChartLegendEntry> {
        legend_entries_for(&self.data_table(), &tokens.palette)
    }

    /// Whether the chart has anything valid to plot.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.data_table().is_empty()
    }

    /// The bounded accessible summary: up to [`MAX_DESCRIBED_SLICES`] slices
    /// named with their share, then an aggregated remainder.
    #[must_use]
    pub fn summary(&self) -> String {
        let rows = self.data_table();
        if rows.is_empty() {
            return enzui_core::localization::text("enzui-pie-chart-no-data");
        }
        let described = rows.len().min(MAX_DESCRIBED_SLICES);
        let mut parts: Vec<String> = rows
            .iter()
            .take(described)
            .map(|r| format!("{}: {:.1}%", r.label, r.percent()))
            .collect();
        if rows.len() > described {
            let rest: f32 = rows[described..].iter().map(|r| r.fraction).sum();
            parts.push(enzui_core::localized!("enzui-pie-chart-more", "count" => rows.len() - described, "percentage" => format!("{:.1}", rest * 100.0)));
        }
        enzui_core::localized!("enzui-pie-chart-summary", "count" => rows.len(), "summary" => parts.join(", "))
    }

    /// The accessibility semantics published: one `Group` node
    /// with the required title as its name and the bounded summary as its
    /// description.
    pub fn semantics(&self) -> Semantics {
        Semantics::new(SemRole::Group)
            .label(self.title.clone())
            .description(self.summary())
    }

    /// Convert into an iced [`Element`].
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: an `other` slice implies a non-empty list"
    )]
    pub fn into_element<'a, Message: 'a>(self) -> Element<'a, Message> {
        let resolved = resolve_slices(&self.slices);
        let semantics = self.semantics();
        let pinned = self.tokens;

        let (w, h) = match self.size {
            Some(s) => (Length::Fixed(s), Length::Fixed(s)),
            None => (Length::Fill, Length::Fixed(DEFAULT_SIZE)),
        };

        let body: Element<'a, Message> = if resolved.is_empty() {
            // Empty state , centred in the plot box.
            let msg = if self.empty_text.is_empty() {
                String::new()
            } else {
                self.empty_text
            };
            let ink = move |theme: &iced::Theme| {
                let t = pinned.unwrap_or_else(|| {
                    crate::theme_lookup::resolve(theme, PieChartTokens::from_palette)
                });
                iced::widget::text::Style {
                    color: Some(t.empty_color),
                }
            };
            container(center(
                text(msg)
                    .size(enzui_theme::text_scale::ui_px(12.0))
                    .style(ink),
            ))
            .width(w)
            .height(h)
            .into()
        } else {
            // Resolve by id; an aggregated slice highlights "Other".
            let highlight = self.selected.as_ref().and_then(|id| {
                // Reserved and empty ids are never a legal caller selection
                // (see `resolved_selection`): consistently ignored here so
                // the drawn highlight cannot disagree with the observation.
                if id.as_str() == RESERVED_OTHER_ID || id.as_str().is_empty() {
                    return None;
                }
                // 1) The id names a drawn, non-Other row.
                if let Some(s) = resolved.iter().find(|s| !s.is_other && &s.id == id) {
                    return Some((s.start, s.fraction));
                }
                // 2) The id names a legal input slice that was
                //    aggregated. Highlight the Other wedge — the
                //    wedge that contains the requested slice —
                //    rather than clearing without notice.
                let has_other = resolved.last().is_some_and(|r| r.is_other);
                let survives_validation = self.slices.iter().any(|s| s.is_valid() && &s.id == id);
                if has_other && survives_validation {
                    let other = resolved.last().expect("has_other implies non-empty");
                    return Some((other.start, other.fraction));
                }
                None
            });
            let program = PieChartProgram {
                slices: resolved,
                inner_ratio: self.inner_ratio,
                show_separator: self.show_separator,
                pinned_tokens: pinned,
                highlight,
            };
            canvas::Canvas::new(program).width(w).height(h).into()
        };

        crate::semantics_container::SemanticsContainer::leaf(body, semantics).into()
    }
}

impl<'a, Message: 'a> From<EnzPieChart> for Element<'a, Message> {
    fn from(c: EnzPieChart) -> Self {
        c.into_element()
    }
}

/// Construct a pie chart. The `title` is required — it is the
/// chart's accessible name, not decoration.
pub fn enz_pie_chart(
    title: impl Into<String>,
    slices: impl IntoIterator<Item = PieSlice>,
) -> EnzPieChart {
    EnzPieChart {
        title: title.into(),
        slices: slices.into_iter().collect(),
        inner_ratio: 0.0,
        size: None,
        show_separator: true,
        selected: None,
        empty_text: String::new(),
        tokens: None,
    }
}
