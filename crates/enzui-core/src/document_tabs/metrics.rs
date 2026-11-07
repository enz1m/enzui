//! Viewport filtering and pinned-grid geometry for document tabs.

use super::*;
use crate::virtual_sequence::{RealizationWindow, partition_point_over, sane_dim};
use core::ops::Range;

/// Tabs intersecting a strip viewport: every pinned tab plus a half-open session range.
///
/// `leading_offset` locates the first session tab relative to the session viewport.
/// Zero-width tabs may make the returned session range large even though its nonzero-width
/// content remains extent-bounded.
#[derive(Debug, Clone, PartialEq)]
pub struct VisibleTabs {
    /// Half-open range in session order; an empty range retains its position.
    pub session: Range<usize>,
    /// Number of pinned tabs, all of which remain visible at every scroll offset.
    pub pinned_count: usize,
    /// Leading edge of `session.start` relative to the session viewport; negative for a
    /// leading-edge straddle and zero for an empty range.
    pub leading_offset: f32,
}

impl VisibleTabs {
    /// Number of session tabs in the visible or realized range.
    #[must_use]
    pub fn session_len(&self) -> usize {
        self.session.end.saturating_sub(self.session.start)
    }

    /// Total touched tabs: all pinned tabs plus the session range.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pinned_count + self.session_len()
    }

    /// Whether nothing at all is visible — no pinned tabs and an empty window.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Prefix-sum geometry for viewport filtering and hit testing document tabs.
///
/// Pinned tabs consume leading extent before the session viewport, and invalid dimensions
/// degrade to zero. Session viewport filtering and one-dimensional strip hit testing use
/// public `f32`-rounded edges and run in `O(log n)`; pinned-grid row lookup is linear in the
/// number of rows.
#[derive(Debug, Clone, PartialEq)]
pub struct StripMetrics {
    session_prefix: Vec<f64>,
    session_keys: Vec<DocumentKey>,
    // Each pinned cell stores its row’s leading edge; the final entry stores total along-axis
    // extent.
    pinned_prefix: Vec<f64>,
    pinned_keys: Vec<DocumentKey>,
    pinned_grid: Option<PinnedGrid>,
}

/// Cross-axis geometry for a row-major pinned grid.
///
/// Row positions are stored by [`StripMetrics`]; this type stores column edges and resolved
/// column count.
#[derive(Debug, Clone, PartialEq)]
pub struct PinnedGrid {
    columns: usize,
    // Cumulative edges of the occupied columns only, each sized to its widest cell, always
    // holding at least the leading zero edge; a declared column past the last pinned cell is
    // empty, so it shares the final stored edge.
    column_prefix: Vec<f64>,
}

impl PinnedGrid {
    /// Number of columns, always `>= 1`.
    #[must_use]
    pub fn columns(&self) -> usize {
        self.columns
    }

    /// The cross-axis leading edge of column `c`, clamped to the cluster's
    /// cross-axis extent for every empty and out-of-range column.
    #[must_use]
    pub fn column_offset(&self, c: usize) -> f32 {
        self.column_prefix[c.min(self.last_edge())] as f32
    }

    /// The cross-axis extent of the whole cluster: the sum of the occupied column widths.
    #[must_use]
    pub fn cross_extent(&self) -> f32 {
        self.column_prefix[self.last_edge()] as f32
    }

    fn last_edge(&self) -> usize {
        self.column_prefix.len() - 1
    }
}

fn pinned_layout(
    along_widths: &[f64],
    cross_widths: Option<&[f64]>,
    columns: usize,
) -> (Vec<f64>, Option<PinnedGrid>) {
    let cols = crate::metrics::RowMajorColumns::new(columns, along_widths.len());
    let columns = cols.declared();
    if columns == 1 {
        let mut prefix = Vec::with_capacity(along_widths.len() + 1);
        let mut acc = 0.0_f64;
        prefix.push(0.0);
        for &w in along_widths {
            acc += w;
            prefix.push(acc);
        }
        return (prefix, None);
    }

    let n = along_widths.len();
    let rows = n.div_ceil(columns);

    let mut col_width = vec![0.0_f64; cols.occupied()];
    let mut row_extent = vec![0.0_f64; rows];
    for (i, &w) in along_widths.iter().enumerate() {
        let c = i % columns;
        let r = i / columns;
        let cw = cross_widths.map_or(w, |xs| xs[i]);
        if cw > col_width[c] {
            col_width[c] = cw;
        }
        if w > row_extent[r] {
            row_extent[r] = w;
        }
    }

    let mut column_prefix = Vec::with_capacity(cols.occupied_edges());
    let mut acc = 0.0_f64;
    column_prefix.push(0.0);
    for &cw in &col_width {
        acc += cw;
        column_prefix.push(acc);
    }

    let mut row_prefix = Vec::with_capacity(rows + 1);
    let mut acc = 0.0_f64;
    row_prefix.push(0.0);
    for &re in &row_extent {
        acc += re;
        row_prefix.push(acc);
    }

    let mut pinned_prefix = Vec::with_capacity(n + 1);
    for i in 0..n {
        pinned_prefix.push(row_prefix[i / columns]);
    }
    pinned_prefix.push(row_prefix[rows]);

    (
        pinned_prefix,
        Some(PinnedGrid {
            columns,
            column_prefix,
        }),
    )
}

impl StripMetrics {
    /// Measures tab extents once and builds logarithmic viewport queries.
    ///
    /// Non-finite and negative widths degrade to zero.
    #[must_use]
    pub fn new(tabs: &DocumentTabs, width_of: impl Fn(&DocumentItem) -> f32) -> Self {
        Self::new_gridded(tabs, width_of, 1)
    }

    /// Measures tabs with a row-major pinned grid.
    ///
    /// Zero or one requested column uses the single-column layout. `width_of` supplies both
    /// along- and cross-axis cell extents; use [`Self::new_gridded_with_cross`] when those
    /// differ.
    #[must_use]
    pub fn new_gridded(
        tabs: &DocumentTabs,
        width_of: impl Fn(&DocumentItem) -> f32,
        columns: usize,
    ) -> Self {
        let mut session_prefix = Vec::with_capacity(tabs.len() + 1);
        let mut session_keys = Vec::new();
        let mut pinned_keys = Vec::new();
        let mut pinned_widths: Vec<f64> = Vec::new();
        session_prefix.push(0.0);
        let mut session_acc = 0.0_f64;
        for item in tabs.items() {
            let w = sane_dim(width_of(item));
            if item.pinned {
                pinned_widths.push(w);
                pinned_keys.push(item.key.clone());
            } else {
                session_acc += w;
                session_prefix.push(session_acc);
                session_keys.push(item.key.clone());
            }
        }

        let (pinned_prefix, pinned_grid) = pinned_layout(&pinned_widths, None, columns);

        StripMetrics {
            session_prefix,
            session_keys,
            pinned_prefix,
            pinned_keys,
            pinned_grid,
        }
    }

    /// Measures tabs with separate along- and cross-axis extents for pinned-grid cells.
    ///
    /// `cross_width_of` is used only by a multi-column pinned grid. Non-finite and negative
    /// extents degrade to zero.
    #[must_use]
    pub fn new_gridded_with_cross(
        tabs: &DocumentTabs,
        width_of: impl Fn(&DocumentItem) -> f32,
        cross_width_of: impl Fn(&DocumentItem) -> f32,
        columns: usize,
    ) -> Self {
        let mut session_prefix = Vec::with_capacity(tabs.len() + 1);
        let mut session_keys = Vec::new();
        let mut pinned_keys = Vec::new();
        let mut pinned_widths: Vec<f64> = Vec::new();
        let mut pinned_cross: Vec<f64> = Vec::new();
        session_prefix.push(0.0);
        let mut session_acc = 0.0_f64;
        for item in tabs.items() {
            let w = sane_dim(width_of(item));
            if item.pinned {
                pinned_widths.push(w);
                pinned_cross.push(sane_dim(cross_width_of(item)));
                pinned_keys.push(item.key.clone());
            } else {
                session_acc += w;
                session_prefix.push(session_acc);
                session_keys.push(item.key.clone());
            }
        }

        let (pinned_prefix, pinned_grid) =
            pinned_layout(&pinned_widths, Some(&pinned_cross), columns);

        StripMetrics {
            session_prefix,
            session_keys,
            pinned_prefix,
            pinned_keys,
            pinned_grid,
        }
    }

    /// How many session (unpinned) tabs the strip holds.
    #[must_use]
    pub fn session_len(&self) -> usize {
        self.session_prefix.len() - 1
    }

    /// Returns the number of pinned tabs.
    #[must_use]
    pub fn pinned_count(&self) -> usize {
        self.pinned_prefix.len() - 1
    }

    /// The session tabs' keys, in strip order.
    #[must_use]
    pub fn session_keys(&self) -> &[DocumentKey] {
        &self.session_keys
    }

    /// The pinned tabs' keys, in strip order.
    #[must_use]
    pub fn pinned_keys(&self) -> &[DocumentKey] {
        &self.pinned_keys
    }

    /// Returns the pinned cluster’s total along-axis extent.
    #[must_use]
    pub fn pinned_extent(&self) -> f32 {
        self.pinned_extent_f64() as f32
    }

    fn pinned_extent_f64(&self) -> f64 {
        self.pinned_prefix[self.pinned_count()]
    }

    /// Returns the session strip’s total along-axis extent.
    #[must_use]
    pub fn session_extent(&self) -> f32 {
        self.session_extent_f64() as f32
    }

    fn session_extent_f64(&self) -> f64 {
        self.session_prefix[self.session_len()]
    }

    /// Returns the combined pinned and session extent.
    #[must_use]
    pub fn total_extent(&self) -> f32 {
        (self.pinned_extent_f64() + self.session_extent_f64()) as f32
    }

    /// Returns a session tab’s sanitized extent, or zero when `index` is out of range.
    #[must_use]
    pub fn session_width_of(&self, index: usize) -> f32 {
        if index >= self.session_len() {
            return 0.0;
        }
        (self.session_prefix[index + 1] - self.session_prefix[index]) as f32
    }

    /// Returns the difference between adjacent stored pinned edges, or zero out of range.
    ///
    /// In a multi-column grid, cells sharing a row edge may report zero.
    #[must_use]
    pub fn pinned_width_of(&self, index: usize) -> f32 {
        if index >= self.pinned_count() {
            return 0.0;
        }
        (self.pinned_prefix[index + 1] - self.pinned_prefix[index]) as f32
    }

    /// Returns a pinned tab’s leading edge, clamped to the cluster end for an out-of-range index.
    #[must_use]
    pub fn pinned_offset_of(&self, index: usize) -> f32 {
        self.pinned_prefix[index.min(self.pinned_count())] as f32
    }

    /// Returns a session tab’s leading edge, clamped to the session end for an out-of-range index.
    #[must_use]
    pub fn session_offset_of(&self, index: usize) -> f32 {
        self.session_offset_of_f64(index) as f32
    }

    fn session_offset_of_f64(&self, index: usize) -> f64 {
        self.session_prefix[index.min(self.session_len())]
    }

    pub(crate) fn session_edge(&self, index: usize) -> f64 {
        #[cfg(test)]
        edge_probes::bump();
        f64::from(self.session_offset_of(index))
    }

    fn pinned_edge(&self, index: usize) -> f64 {
        #[cfg(test)]
        edge_probes::bump();
        f64::from(self.pinned_prefix[index.min(self.pinned_count())] as f32)
    }

    /// Returns nonnegative session viewport extent after subtracting the pinned cluster.
    #[must_use]
    pub fn available_extent(&self, extent: f32) -> f32 {
        self.available_extent_f64(extent) as f32
    }

    fn available_extent_f64(&self, extent: f32) -> f64 {
        (sane_dim(extent) - self.pinned_extent_f64()).max(0.0)
    }

    /// Returns the maximum session scroll offset, or zero when all session content fits.
    #[must_use]
    pub fn max_scroll_offset(&self, extent: f32) -> f32 {
        (self.session_extent_f64() - self.available_extent_f64(extent)).max(0.0) as f32
    }

    /// Returns every pinned tab and the half-open session range intersecting the viewport.
    ///
    /// Tabs straddling either edge are included. Invalid or nonpositive extents and a pinned
    /// cluster filling the strip produce an empty session range; invalid or negative scroll
    /// offsets use the leading edge.
    #[must_use]
    pub fn visible(&self, scroll_offset: f32, extent: f32) -> VisibleTabs {
        self.realize(scroll_offset, extent, RealizationWindow::new(0))
    }

    /// Expands the visible session range by `window` while keeping empty ranges empty.
    ///
    /// `leading_offset` refers to the expanded range’s first tab.
    #[must_use]
    pub fn realize(
        &self,
        scroll_offset: f32,
        extent: f32,
        window: RealizationWindow,
    ) -> VisibleTabs {
        let session = window.expand(
            self.session_range(scroll_offset, extent),
            self.session_len(),
        );
        let leading_offset = if session.start >= session.end {
            0.0
        } else {
            // Keep subtraction in `f64` until producing the public `f32` coordinate.
            (self.session_offset_of_f64(session.start) - self.sane_offset_f64(scroll_offset)) as f32
        };
        VisibleTabs {
            session,
            pinned_count: self.pinned_count(),
            leading_offset,
        }
    }

    fn sane_offset_f64(&self, scroll_offset: f32) -> f64 {
        if scroll_offset.is_finite() && scroll_offset > 0.0 {
            f64::from(scroll_offset)
        } else {
            0.0
        }
    }

    fn session_range(&self, scroll_offset: f32, extent: f32) -> Range<usize> {
        let len = self.session_len();
        let available = self.available_extent_f64(extent);
        let total = self.session_extent_f64();
        if len == 0 || total == 0.0 || available == 0.0 {
            return 0..0;
        }
        let leading = self.sane_offset_f64(scroll_offset);
        if leading >= total {
            // Preserve an after-content empty window as `len..len` rather than `0..0`.
            return len..len;
        }
        let trailing = (leading + available).min(total);
        let start = partition_point_over(len, |i| self.session_edge(i), |edge| edge <= leading)
            .saturating_sub(1);
        let end = partition_point_over(len, |i| self.session_edge(i), |edge| edge < trailing);
        start..end.clamp(start, len)
    }

    /// Returns the tab under strip coordinate `x` using half-open pinned and session geometry.
    ///
    /// Returns `None` for invalid or out-of-bounds coordinates, empty cells, and positions
    /// beyond content. Any returned key belongs to [`Self::visible`] for the same offset and
    /// extent.
    #[must_use]
    pub fn key_at(&self, scroll_offset: f32, extent: f32, x: f32) -> Option<&DocumentKey> {
        if !x.is_finite() || x < 0.0 || f64::from(x) >= sane_dim(extent) {
            return None;
        }
        let x = f64::from(x);
        let pinned_extent = self.pinned_extent_f64();
        if x < pinned_extent {
            // For repeated zero-width edges, choose the last candidate whose interval may contain `x`.
            let first_above =
                partition_point_over(self.pinned_count(), |i| self.pinned_edge(i), |e| e <= x);
            return first_above
                .checked_sub(1)
                .and_then(|i| self.pinned_keys.get(i));
        }
        let session_x = x - pinned_extent + self.sane_offset_f64(scroll_offset);
        if session_x >= self.session_extent_f64() {
            return None;
        }
        let len = self.session_len();
        let first_above = partition_point_over(len, |i| self.session_edge(i), |e| e <= session_x);
        first_above
            .checked_sub(1)
            .and_then(|i| self.session_keys.get(i))
    }

    /// Resolves a visible range to pinned keys followed by session keys, truncating stale
    /// out-of-range indices.
    pub fn visible_keys<'a>(
        &'a self,
        visible: &VisibleTabs,
    ) -> impl Iterator<Item = &'a DocumentKey> {
        let n = self.session_keys.len();
        let start = visible.session.start.min(n);
        let end = visible.session.end.clamp(start, n);
        self.pinned_keys
            .iter()
            .chain(&self.session_keys[start..end])
    }

    /// Returns cross-axis grid geometry, or `None` for a single-column pinned cluster.
    #[must_use]
    pub fn pinned_grid(&self) -> Option<&PinnedGrid> {
        self.pinned_grid.as_ref()
    }

    /// Returns the resolved pinned column count, using one for single-column layout.
    #[must_use]
    pub fn pinned_columns(&self) -> usize {
        self.pinned_grid.as_ref().map_or(1, PinnedGrid::columns)
    }

    /// Returns `ceil(pinned_count / columns)`, or zero for an empty cluster.
    #[must_use]
    pub fn pinned_rows(&self) -> usize {
        self.pinned_count().div_ceil(self.pinned_columns())
    }

    /// Returns the pinned cluster’s cross-axis extent: its widest cell for one column,
    /// otherwise the sum of column widths.
    #[must_use]
    pub fn pinned_cross_extent(&self) -> f32 {
        match &self.pinned_grid {
            Some(grid) => grid.cross_extent(),
            // The single-column path stores no cross edges, so its widest along-axis cell supplies cross
            // extent.
            None => (0..self.pinned_count())
                .map(|i| self.pinned_width_of(i))
                .fold(0.0_f32, f32::max),
        }
    }

    /// Returns the row containing a pinned cell, using row-major packing.
    #[must_use]
    pub fn pinned_row_of(&self, index: usize) -> usize {
        index / self.pinned_columns()
    }

    /// Returns the column containing a pinned cell, using row-major packing.
    #[must_use]
    pub fn pinned_col_of(&self, index: usize) -> usize {
        index % self.pinned_columns()
    }

    /// Returns a pinned cell’s `(row_extent, column_extent)`, or `(0.0, 0.0)` when out of range.
    #[must_use]
    pub fn pinned_cell_box(&self, index: usize) -> (f32, f32) {
        if index >= self.pinned_count() {
            return (0.0, 0.0);
        }
        let columns = self.pinned_columns();
        let row = index / columns;
        let this_edge = f64::from(self.pinned_offset_of(row * columns));
        let next_first = (row + 1) * columns;
        let next_edge = if next_first >= self.pinned_count() {
            self.pinned_extent_f64()
        } else {
            f64::from(self.pinned_offset_of(next_first))
        };
        let along = (next_edge - this_edge).max(0.0) as f32;
        let cross = match &self.pinned_grid {
            Some(grid) => {
                let c = index % columns;
                grid.column_offset(c + 1) - grid.column_offset(c)
            }
            None => self.pinned_cross_extent(),
        };
        (along, cross)
    }

    /// Returns the pinned key under a row-axis and column-axis coordinate.
    ///
    /// Coordinates are half-open; invalid, out-of-bounds, and unoccupied remainder cells return
    /// `None`.
    #[must_use]
    pub fn pinned_key_at(&self, along: f32, cross: f32) -> Option<&DocumentKey> {
        if !along.is_finite() || along < 0.0 || !cross.is_finite() || cross < 0.0 {
            return None;
        }
        let count = self.pinned_count();
        if count == 0 {
            return None;
        }
        let along = f64::from(along);
        let cross = f64::from(cross);
        let columns = self.pinned_columns();

        if cross >= f64::from(self.pinned_cross_extent()) {
            return None;
        }
        if along >= self.pinned_extent_f64() {
            return None;
        }

        let col = match &self.pinned_grid {
            Some(grid) => {
                let first_above = partition_point_over(
                    columns,
                    |c| f64::from(grid.column_offset(c)),
                    |edge| edge <= cross,
                );
                first_above.checked_sub(1)?
            }
            None => 0,
        };

        let rows = self.pinned_rows();
        let mut row = 0usize;
        for r in 0..rows {
            let edge = f64::from(self.pinned_offset_of(r * columns));
            if edge <= along {
                row = r;
            } else {
                break;
            }
        }

        let index = row * columns + col;
        self.pinned_keys.get(index)
    }
}

#[cfg(test)]
pub(crate) mod edge_probes {
    use std::cell::Cell;

    thread_local! {
        static COUNT: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) fn bump() {
        COUNT.with(|c| c.set(c.get() + 1));
    }
}
