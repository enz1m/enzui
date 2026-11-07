//! Renderer-neutral marquee geometry, indexed hit-testing, and gesture lifecycle.
//!
//! Coordinates must share one logical space. [`MarqueeIndex::query_into`] writes results into caller-provided storage.

use crate::drag::ItemId;
use crate::metrics::{FiniteNonNeg, OrderedRangeF32};
use crate::semantics::{Role, Semantics};

/// An axis-aligned selection band; [`MarqueeBand::from_points`] normalizes its inputs.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MarqueeBand {
    /// Horizontal origin.
    pub x: f32,
    /// Vertical origin.
    pub y: f32,
    /// Horizontal extent.
    pub width: f32,
    /// Vertical extent.
    pub height: f32,
}

impl MarqueeBand {
    /// Builds a band from two corners in either order, replacing non-finite coordinates with zero.
    #[must_use]
    pub fn from_points(press: (f32, f32), current: (f32, f32)) -> Self {
        let fix = |v: f32| if v.is_finite() { v } else { 0.0 };
        let xs = OrderedRangeF32::new_sorted(fix(press.0), fix(current.0))
            .unwrap_or(const { OrderedRangeF32::new(0.0, 0.0).expect("0..0 is ordered") });
        let ys = OrderedRangeF32::new_sorted(fix(press.1), fix(current.1))
            .unwrap_or(const { OrderedRangeF32::new(0.0, 0.0).expect("0..0 is ordered") });
        Self {
            x: xs.lo(),
            y: ys.lo(),
            width: xs.hi() - xs.lo(),
            height: ys.hi() - ys.lo(),
        }
    }

    /// Returns `x + width`.
    #[must_use]
    pub fn right(self) -> f32 {
        self.x + self.width
    }

    /// Returns `y + height`.
    #[must_use]
    pub fn bottom(self) -> f32 {
        self.y + self.height
    }

    /// Returns whether both extents are non-positive.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.width <= 0.0 && self.height <= 0.0
    }

    /// Tests strict axis-aligned overlap; edge contact does not intersect.
    #[must_use]
    pub fn intersects(self, rect: [f32; 4]) -> bool {
        if self.is_empty() {
            return false;
        }
        let [ix, iy, iw, ih] = rect;
        ix < self.right() && ix + iw > self.x && iy < self.bottom() && iy + ih > self.y
    }
}

#[must_use]
fn normalize_rect(rect: [f32; 4]) -> [f32; 4] {
    let fix = |v: f32| if v.is_finite() { v } else { 0.0 };
    let (x, y, w, h) = (fix(rect[0]), fix(rect[1]), fix(rect[2]), fix(rect[3]));
    let (x, w) = if w < 0.0 { (x + w, -w) } else { (x, w) };
    let (y, h) = if h < 0.0 { (y + h, -h) } else { (y, h) };
    [x, y, w, h]
}

// Cap the bucket table relative to item count so distant outliers cannot inflate that table superlinearly.
const MAX_CELLS_PER_ITEM: usize = 4;

// Keep small collections from degenerating immediately to a single bucket.
const MIN_CELLS: usize = 16;

const MAX_MEMBERSHIPS_PER_ITEM: usize = 8;

/// A uniform-grid spatial index over item rectangles.
///
/// The grid retains at most eight rectangle-to-bucket memberships per item.
/// Layouts over that O(n) budget use an exact linear scan instead; both paths
/// preserve input ordering. Queries reuse caller-provided storage.
#[derive(Debug, Clone)]
pub struct MarqueeIndex {
    rects: Vec<[f32; 4]>,
    origin: (f32, f32),
    // Positive finite cell edge length used for grid arithmetic.
    cell: f32,
    cols: usize,
    rows: usize,
    // CSR offsets delimit each bucket's range in `items`.
    starts: Vec<u32>,
    items: Vec<u32>,
    stamp: Vec<u32>,
    generation: u32,
}

impl MarqueeIndex {
    fn linear(rects: Vec<[f32; 4]>) -> Self {
        Self {
            rects,
            origin: (0.0, 0.0),
            cell: 1.0,
            cols: 0,
            rows: 0,
            starts: vec![0],
            items: Vec::new(),
            stamp: Vec::new(),
            generation: 0,
        }
    }

    /// Builds an index without reordering items, so results remain input positions.
    #[must_use]
    pub fn build(rects: &[[f32; 4]]) -> Self {
        let rects: Vec<[f32; 4]> = rects.iter().copied().map(normalize_rect).collect();
        let n = rects.len();

        if n == 0 || u32::try_from(n).is_err() {
            return Self::linear(rects);
        }

        let (mut min_x, mut min_y) = (f32::MAX, f32::MAX);
        let (mut max_x, mut max_y) = (f32::MIN, f32::MIN);
        let (mut sum_w, mut sum_h) = (0.0f64, 0.0f64);
        for r in &rects {
            min_x = min_x.min(r[0]);
            min_y = min_y.min(r[1]);
            max_x = max_x.max(r[0] + r[2]);
            max_y = max_y.max(r[1] + r[3]);
            sum_w += f64::from(r[2]);
            sum_h += f64::from(r[3]);
        }
        let extent_w = (max_x - min_x).max(0.0);
        let extent_h = (max_y - min_y).max(0.0);

        // Size cells near typical item dimensions to balance duplicate buckets against irrelevant neighbors.
        let mean_w = (sum_w / n as f64) as f32;
        let mean_h = (sum_h / n as f64) as f32;
        let mut cell = mean_w.max(mean_h);
        if !cell.is_finite() || cell <= 0.0 {
            // Zero-sized items (points) carry no scale of their own; fall back
            // to spreading the collection's extent over roughly n cells.
            cell = (extent_w.max(extent_h) / (n as f32).sqrt().max(1.0)).max(1.0);
        }

        let Some(max_cells) = n
            .checked_mul(MAX_CELLS_PER_ITEM)
            .map(|count| count.max(MIN_CELLS))
        else {
            return Self::linear(rects);
        };
        let mut cols = ((extent_w / cell).ceil() as usize).max(1);
        let mut rows = ((extent_h / cell).ceil() as usize).max(1);
        // Scale cell size by the square root of grid overshoot to reach the bucket budget quickly.
        while cols.saturating_mul(rows) > max_cells {
            let overshoot = (cols as f64 * rows as f64) / max_cells as f64;
            cell *= (overshoot.sqrt() as f32).max(1.000_01);
            cols = ((extent_w / cell).ceil() as usize).max(1);
            rows = ((extent_h / cell).ceil() as usize).max(1);
        }

        let origin = (min_x, min_y);
        let Some(bucket_count) = cols.checked_mul(rows) else {
            return Self::linear(rects);
        };
        let Some(starts_len) = bucket_count.checked_add(1) else {
            return Self::linear(rects);
        };
        let max_offset = usize::try_from(u32::MAX).unwrap_or(usize::MAX);
        let Some(membership_budget) = n
            .checked_mul(MAX_MEMBERSHIPS_PER_ITEM)
            .map(|count| count.min(max_offset))
        else {
            return Self::linear(rects);
        };

        let mut total = 0usize;
        for &rect in &rects {
            let Some(span) = cell_span(rect, origin, cell, cols, rows) else {
                return Self::linear(rects);
            };
            let Some(cardinality) = span_cardinality(span) else {
                return Self::linear(rects);
            };
            let Some(next_total) = checked_membership_total(total, cardinality, membership_budget)
            else {
                return Self::linear(rects);
            };
            total = next_total;
        }

        let mut starts = vec![0u32; starts_len];
        for &rect in &rects {
            let Some((c0, r0, c1, r1)) = cell_span(rect, origin, cell, cols, rows) else {
                return Self::linear(rects);
            };
            for row in r0..=r1 {
                for col in c0..=c1 {
                    let offset = row * cols + col + 1;
                    let Some(next) = starts[offset].checked_add(1) else {
                        return Self::linear(rects);
                    };
                    starts[offset] = next;
                }
            }
        }
        for b in 0..bucket_count {
            let Some(next) = starts[b + 1].checked_add(starts[b]) else {
                return Self::linear(rects);
            };
            starts[b + 1] = next;
        }

        let mut cursor = starts.clone();
        let mut items = vec![0u32; total];
        for (raw_index, rect) in (0u32..).zip(rects.iter()) {
            let Some((c0, r0, c1, r1)) = cell_span(*rect, origin, cell, cols, rows) else {
                return Self::linear(rects);
            };
            for row in r0..=r1 {
                for col in c0..=c1 {
                    let b = row * cols + col;
                    let Ok(position) = usize::try_from(cursor[b]) else {
                        return Self::linear(rects);
                    };
                    items[position] = raw_index;
                    let Some(next) = cursor[b].checked_add(1) else {
                        return Self::linear(rects);
                    };
                    cursor[b] = next;
                }
            }
        }

        Self {
            rects,
            origin,
            cell,
            cols,
            rows,
            starts,
            items,
            stamp: vec![0; n],
            generation: 0,
        }
    }

    /// Number of indexed items.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rects.len()
    }

    /// Whether the index holds no items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty()
    }

    /// The repaired rectangle at `index`, if any.
    #[must_use]
    pub fn rect(&self, index: usize) -> Option<[f32; 4]> {
        self.rects.get(index).copied()
    }

    /// Returns the chosen grid-cell edge length, or `1.0` for the linear fallback.
    #[must_use]
    pub fn cell_size(&self) -> f32 {
        self.cell
    }

    /// Number of grid buckets, or zero while using the linear fallback.
    #[must_use]
    pub fn bucket_count(&self) -> usize {
        self.cols * self.rows
    }

    /// Number of retained rectangle-to-bucket memberships.
    #[must_use]
    pub fn retained_memberships(&self) -> usize {
        self.items.len()
    }

    /// Writes covered item indices to `out` in ascending order, clearing it first.
    ///
    /// Existing vector capacity is reused and may grow when insufficient.
    pub fn query_into(&mut self, band: MarqueeBand, out: &mut Vec<usize>) {
        out.clear();
        if band.is_empty() || self.rects.is_empty() {
            return;
        }
        if self.cols == 0 {
            self.query_linear_into(band, out);
            return;
        }

        // On generation wrap, clear all stamps before reusing zero so no old stamp can compare equal.
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.stamp.iter_mut().for_each(|s| *s = 0);
            self.generation = 1;
        }
        let stamp_gen = self.generation;

        let band_rect = [band.x, band.y, band.width, band.height];
        let Some((c0, r0, c1, r1)) =
            cell_span(band_rect, self.origin, self.cell, self.cols, self.rows)
        else {
            self.query_linear_into(band, out);
            return;
        };

        for row in r0..=r1 {
            let base = row * self.cols;
            for col in c0..=c1 {
                let b = base + col;
                let (lo, hi) = (self.starts[b] as usize, self.starts[b + 1] as usize);
                for &raw in &self.items[lo..hi] {
                    let i = raw as usize;
                    if self.stamp[i] == stamp_gen {
                        continue;
                    }
                    self.stamp[i] = stamp_gen;
                    if band.intersects(self.rects[i]) {
                        out.push(i);
                    }
                }
            }
        }

        out.sort_unstable();
    }

    fn query_linear_into(&self, band: MarqueeBand, out: &mut Vec<usize>) {
        for (i, rect) in self.rects.iter().enumerate() {
            if band.intersects(*rect) {
                out.push(i);
            }
        }
    }

    /// Returns covered ids by allocating and cloning from `ids`.
    ///
    /// Use [`Self::query_into`] for repeated pointer-move queries.
    #[must_use]
    pub fn covered_ids(&mut self, band: MarqueeBand, ids: &[ItemId]) -> Vec<ItemId> {
        let mut hits = Vec::new();
        self.query_into(band, &mut hits);
        hits.into_iter()
            .filter_map(|i| ids.get(i).cloned())
            .collect()
    }
}

fn checked_membership_total(total: usize, cardinality: usize, budget: usize) -> Option<usize> {
    total
        .checked_add(cardinality)
        .filter(|&next| next <= budget)
}

fn span_cardinality((c0, r0, c1, r1): (usize, usize, usize, usize)) -> Option<usize> {
    let width = c1.checked_sub(c0)?.checked_add(1)?;
    let height = r1.checked_sub(r0)?.checked_add(1)?;
    width.checked_mul(height)
}

fn cell_span(
    rect: [f32; 4],
    origin: (f32, f32),
    cell: f32,
    cols: usize,
    rows: usize,
) -> Option<(usize, usize, usize, usize)> {
    let to_cell = |v: f32, o: f32, max: usize| -> Option<usize> {
        if max == 0 {
            return Some(0);
        }
        let idx = ((v - o) / cell).floor();
        if !idx.is_finite() {
            None
        } else if idx < 0.0 {
            Some(0)
        } else if idx >= max as f32 {
            Some(max - 1)
        } else {
            Some(idx as usize)
        }
    };
    let c0 = to_cell(rect[0], origin.0, cols)?;
    let r0 = to_cell(rect[1], origin.1, rows)?;
    let c1 = to_cell(rect[0] + rect[2], origin.0, cols)?;
    let r1 = to_cell(rect[1] + rect[3], origin.1, rows)?;
    Some((c0.min(c1), r0.min(r1), c0.max(c1), r0.max(r1)))
}

/// Writes covered indices using a linear scan, clearing `out` first.
pub fn covered_linear(band: MarqueeBand, rects: &[[f32; 4]], out: &mut Vec<usize>) {
    out.clear();
    if band.is_empty() {
        return;
    }
    for (i, r) in rects.iter().enumerate() {
        if band.intersects(normalize_rect(*r)) {
            out.push(i);
        }
    }
}

/// Controls how a completed marquee combines covered items with existing selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SelectionMode {
    /// No modifier: the covered set becomes the selection.
    #[default]
    Replace,
    /// Shift: union. Already-selected items stay selected.
    Add,
    /// Ctrl: symmetric difference, toggling every covered item.
    Toggle,
}

impl SelectionMode {
    /// Maps modifiers to selection mode, with Shift taking precedence over Ctrl.
    #[must_use]
    pub fn from_modifiers(shift: bool, ctrl: bool) -> Self {
        if shift {
            Self::Add
        } else if ctrl {
            Self::Toggle
        } else {
            Self::Replace
        }
    }

    /// Whether this mode preserves items outside the band.
    #[must_use]
    pub fn is_additive(self) -> bool {
        matches!(self, Self::Add | Self::Toggle)
    }

    /// Applies this mode into `out`, preserving base order before newly covered items.
    pub fn apply_into(self, base: &[ItemId], covered: &[ItemId], out: &mut Vec<ItemId>) {
        out.clear();
        match self {
            Self::Replace => out.extend(covered.iter().cloned()),
            Self::Add => {
                out.extend(base.iter().cloned());
                for id in covered {
                    if !base.contains(id) {
                        out.push(id.clone());
                    }
                }
            }
            Self::Toggle => {
                for id in base {
                    if !covered.contains(id) {
                        out.push(id.clone());
                    }
                }
                for id in covered {
                    if !base.contains(id) {
                        out.push(id.clone());
                    }
                }
            }
        }
    }

    /// Allocating form of [`Self::apply_into`], for the single commit step.
    #[must_use]
    pub fn apply(self, base: &[ItemId], covered: &[ItemId]) -> Vec<ItemId> {
        let mut out = Vec::new();
        self.apply_into(base, covered, &mut out);
        out
    }
}

/// Drag threshold in logical pixels where the desktop sets none (`4px`);
/// widgets use the desktop's
/// ([`desktop_settings::input`](crate::desktop_settings::input)).
pub const DEFAULT_DRAG_THRESHOLD: f32 = iced_core::preferences::Preferences::DEFAULT.drag_threshold;

/// Lifecycle phase of a marquee gesture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MarqueePhase {
    /// The pointer is down but has not crossed the drag threshold.
    Pending,
    /// The threshold was crossed; a band is live.
    Active,
    /// The gesture has been committed or cancelled; pointer movement is ignored.
    Finished,
}

/// How a marquee gesture ended.
#[derive(Debug, Clone, PartialEq)]
pub enum MarqueeOutcome {
    /// A completed drag with its final band and captured mode.
    Committed {
        /// The final normalized band.
        band: MarqueeBand,
        /// The modifier policy captured at press time.
        mode: SelectionMode,
    },
    /// The pointer was released before crossing the threshold.
    Clicked {
        /// The modifier policy captured at press time.
        mode: SelectionMode,
    },
    /// Escape, lost capture, or explicit cancellation without a commit.
    Cancelled,
}

/// Transient state for a deadband-gated marquee gesture.
#[derive(Debug, Clone, PartialEq)]
pub struct MarqueeSession {
    origin: (f32, f32),
    current: (f32, f32),
    mode: SelectionMode,
    threshold: f32,
    phase: MarqueePhase,
}

impl MarqueeSession {
    /// Begins a marquee and captures its selection mode at press time.
    ///
    /// Non-finite or negative thresholds are normalized.
    #[must_use]
    pub fn begin(origin: (f32, f32), mode: SelectionMode, threshold: f32) -> Self {
        let fix = |v: f32| if v.is_finite() { v } else { 0.0 };
        let origin = (fix(origin.0), fix(origin.1));
        Self {
            origin,
            current: origin,
            mode,
            threshold: FiniteNonNeg::new_or(
                threshold,
                crate::desktop_settings::input().drag_threshold,
            )
            .get(),
            phase: MarqueePhase::Pending,
        }
    }

    /// The current phase.
    #[must_use]
    pub fn phase(&self) -> MarqueePhase {
        self.phase
    }

    /// Whether a band is live and should be drawn.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.phase == MarqueePhase::Active
    }

    /// The modifier policy captured at press time.
    #[must_use]
    pub fn mode(&self) -> SelectionMode {
        self.mode
    }

    /// Returns the band from the press origin to the latest pointer position, regardless of phase.
    #[must_use]
    pub fn band(&self) -> MarqueeBand {
        MarqueeBand::from_points(self.origin, self.current)
    }

    /// Updates the pointer and returns a live band after radial distance exceeds the threshold.
    pub fn pointer_moved(&mut self, position: (f32, f32)) -> Option<MarqueeBand> {
        if self.phase == MarqueePhase::Finished {
            return None;
        }
        let fix = |v: f32| if v.is_finite() { v } else { 0.0 };
        self.current = (fix(position.0), fix(position.1));

        if self.phase == MarqueePhase::Pending {
            let dx = self.current.0 - self.origin.0;
            let dy = self.current.1 - self.origin.1;
            if dx * dx + dy * dy > self.threshold * self.threshold {
                self.phase = MarqueePhase::Active;
            }
        }
        self.is_active().then(|| self.band())
    }

    /// Finishes the session as a committed drag or a background click.
    pub fn release(&mut self) -> MarqueeOutcome {
        let outcome = if self.is_active() {
            MarqueeOutcome::Committed {
                band: self.band(),
                mode: self.mode,
            }
        } else {
            MarqueeOutcome::Clicked { mode: self.mode }
        };
        self.phase = MarqueePhase::Finished;
        outcome
    }

    /// Finishes the session as cancelled without producing a committable band.
    pub fn cancel(&mut self) -> MarqueeOutcome {
        self.phase = MarqueePhase::Finished;
        MarqueeOutcome::Cancelled
    }
}

/// Builds collection semantics reporting selected and total item counts.
pub fn selection_semantics(label: Option<String>, selected: usize, total: usize) -> Semantics {
    Semantics::new(Role::Group)
        .label_opt(label)
        .value(crate::localized!("enzui-selection-count", "selected" => selected, "total" => total))
        .selected(selected > 0)
}
