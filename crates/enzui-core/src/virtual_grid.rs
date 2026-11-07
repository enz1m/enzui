//! Geometry for a one-axis-scrolling, fixed-tile adaptive grid.
//!
//! Width determines the column count, rows map to item ranges, and [`TileProvider`] creates resources on demand for bounded external caching.

use core::ops::Range;

use crate::virtual_sequence::{Anchor, HeightModel, KeyedAnchor, KeyedProvider};

/// A keyed provider that creates shared tile resources on demand.
///
/// Implementations must return `None` out of range and must not retain strong references to created resources, so cache eviction can release them.
pub trait TileProvider<Tile>: KeyedProvider<Tile> {
    /// Returns the preferred maximum number of live cached tiles.
    ///
    /// `Some(0)` disables retention; `None` lets the consumer derive a bound. An explicit consumer capacity takes precedence.
    fn retention_hint(&self) -> Option<usize> {
        None
    }
}

/// Fixed tile extents and spacing stored in `f64` to preserve deep-offset arithmetic.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileGrid {
    tile_width: f64,
    tile_height: f64,
    gap_x: f64,
    gap_y: f64,
}

/// Adaptive-grid geometry resolved for one concrete allocation.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ResolvedTileGrid {
    /// Actual inner width supplied by layout.
    pub width: f32,
    /// Number of columns that fit the minimum tile footprint.
    pub columns: usize,
    /// Equal stretched width assigned to every tile.
    pub tile_width: f32,
    /// Fixed tile height.
    pub tile_height: f32,
    /// Horizontal gap between tiles.
    pub gap_x: f32,
    /// Vertical gap between rows.
    pub gap_y: f32,
}

impl ResolvedTileGrid {
    /// Returns whether the resolution has usable width and at least one column.
    #[must_use]
    pub fn is_resolved(self) -> bool {
        self.width.is_finite() && self.width > 0.0 && self.columns > 0
    }

    /// Rebuild the concrete [`TileGrid`] consumed by realization/reveal.
    #[must_use]
    pub fn grid(self) -> TileGrid {
        TileGrid::new(self.tile_width, self.tile_height).with_spacing(self.gap_x, self.gap_y)
    }
}

fn sane_len(v: f32) -> f64 {
    if v.is_finite() && v > 0.0 {
        f64::from(v)
    } else {
        0.0
    }
}

impl TileGrid {
    /// Creates an unspaced grid, sanitizing non-finite or negative extents to zero.
    #[must_use]
    pub fn new(tile_width: f32, tile_height: f32) -> Self {
        TileGrid {
            tile_width: sane_len(tile_width),
            tile_height: sane_len(tile_height),
            gap_x: 0.0,
            gap_y: 0.0,
        }
    }

    /// Sets per-axis spacing, preserving zero and converting non-finite or negative values to zero.
    #[must_use]
    pub fn with_spacing(mut self, gap_x: f32, gap_y: f32) -> Self {
        self.gap_x = sane_len(gap_x);
        self.gap_y = sane_len(gap_y);
        self
    }

    /// Tile width in logical pixels.
    #[must_use]
    pub fn tile_width(&self) -> f32 {
        self.tile_width as f32
    }

    /// Tile height in logical pixels.
    #[must_use]
    pub fn tile_height(&self) -> f32 {
        self.tile_height as f32
    }

    /// Horizontal gap between adjacent tiles.
    #[must_use]
    pub fn gap_x(&self) -> f32 {
        self.gap_x as f32
    }

    /// Vertical gap between adjacent rows.
    #[must_use]
    pub fn gap_y(&self) -> f32 {
        self.gap_y as f32
    }

    /// Returns the top-to-top row pitch: tile height plus vertical spacing.
    #[must_use]
    pub fn row_pitch(&self) -> f32 {
        (self.tile_height + self.gap_y) as f32
    }

    /// Returns the number of columns fitting `width`.
    ///
    /// Zero or invalid width and zero tile pitch yield `0`; a positive width narrower than one tile yields one clipped column.
    #[must_use]
    pub fn columns_for_width(&self, width: f32) -> usize {
        let width = sane_len(width);
        let pitch = self.tile_width + self.gap_x;
        if width <= 0.0 || pitch <= 0.0 {
            return 0;
        }
        let fit = ((width + self.gap_x) / pitch).floor();
        if fit < 1.0 {
            return 1;
        }
        if fit >= usize::MAX as f64 {
            usize::MAX
        } else {
            fit as usize
        }
    }

    /// Resolves fixed-minimum tiles into a column count and equal tile width for this allocation.
    #[must_use]
    pub fn stretch_for_width(&self, width: f32) -> ResolvedTileGrid {
        let width = sane_len(width) as f32;
        let columns = self.columns_for_width(width);
        let gaps = self.gap_x() * columns.saturating_sub(1) as f32;
        let tile_width = if columns == 0 {
            0.0
        } else {
            ((width - gaps) / columns as f32).max(self.tile_width())
        };
        ResolvedTileGrid {
            width,
            columns,
            tile_width,
            tile_height: self.tile_height(),
            gap_x: self.gap_x(),
            gap_y: self.gap_y(),
        }
    }

    /// Returns the number of rows needed for `item_count`; returns `0` when `columns` is zero.
    #[must_use]
    pub fn rows_for(&self, item_count: u64, columns: usize) -> usize {
        if columns == 0 || item_count == 0 {
            return 0;
        }
        let columns = columns as u64;
        let rows = item_count.div_ceil(columns);
        usize::try_from(rows).unwrap_or(usize::MAX)
    }

    /// Returns a uniform row model using the top-to-top row pitch.
    ///
    /// The final track includes trailing vertical spacing.
    #[must_use]
    pub fn row_model(&self, item_count: u64, columns: usize) -> HeightModel {
        HeightModel::new(self.rows_for(item_count, columns), self.row_pitch())
    }

    /// Returns the `(row, column)` for `index`, or `None` when `columns` is zero. This method does not validate `index` against provider length.
    #[must_use]
    pub fn cell_of(&self, index: u64, columns: usize) -> Option<(u64, usize)> {
        if columns == 0 {
            return None;
        }
        let c = columns as u64;
        Some((index / c, (index % c) as usize))
    }

    /// Maps row indices to item indices, clamped to `item_count`.
    ///
    /// Empty or reversed ranges remain empty at their mapped position; zero columns yield `0..0`.
    #[must_use]
    pub fn items_in_rows(&self, rows: Range<usize>, columns: usize, item_count: u64) -> Range<u64> {
        if columns == 0 {
            return 0..0;
        }
        let c = columns as u64;
        let start = (rows.start as u64).saturating_mul(c).min(item_count);
        if rows.start >= rows.end {
            return start..start;
        }
        let end = (rows.end as u64).saturating_mul(c).min(item_count);
        start..end.max(start)
    }
}

#[cfg(test)]
mod tests;

/// The anchored item position and scroll offset after a column-count change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reflow {
    /// The anchored item’s row under the new column count.
    pub row: usize,
    /// The anchored item’s column under the new column count.
    pub column: usize,
    /// The non-negative scroll offset obtained by subtracting the anchor’s intra-item offset from its new row top.
    pub scroll_top: f32,
}

impl TileGrid {
    /// Captures the top-left visible item as a keyed anchor for later reflow.
    ///
    /// Returns `None` when there are no columns, items, or content at `viewport_top`.
    #[must_use]
    pub fn capture<Tile, P>(
        &self,
        provider: &P,
        columns: usize,
        viewport_top: f32,
    ) -> Option<KeyedAnchor>
    where
        P: KeyedProvider<Tile> + ?Sized,
    {
        let count = provider.len();
        let rows = self.row_model(count, columns);
        // Delegate offset capture to `Anchor`; re-deriving row positions in f32 loses pixels at depth.
        let anchor = Anchor::from_scroll_top(&rows, viewport_top);
        if anchor.index() >= rows.len() {
            return None;
        }
        let first_item = (anchor.index() as u64).checked_mul(columns as u64)?;
        let key = provider.key(first_item)?;
        Some(KeyedAnchor::new(key, anchor.offset_in_item()))
    }

    /// Resolves a keyed anchor under a new column count.
    ///
    /// Returns `None` for zero columns or a missing key. The scroll offset is
    /// clamped to zero when preserving the intra-item offset would require negative
    /// scrolling.
    #[must_use]
    pub fn reflow<Tile, P>(
        &self,
        provider: &P,
        anchor: &KeyedAnchor,
        columns: usize,
    ) -> Option<Reflow>
    where
        P: KeyedProvider<Tile> + ?Sized,
    {
        if columns == 0 {
            return None;
        }
        let index = provider.index_of(anchor.key())?;
        let (row, column) = self.cell_of(index, columns)?;
        let row = usize::try_from(row).ok()?;
        let rows = self.row_model(provider.len(), columns);
        // Every pixel offset comes from the geometry model, never from a
        // hand-rolled `row * pitch` here.
        let row_top = rows.offset_of_f64(row);
        let scroll_top = (row_top - f64::from(anchor.offset_in_item())).max(0.0);
        Some(Reflow {
            row,
            column,
            scroll_top: scroll_top as f32,
        })
    }

    /// Returns the minimal scroll offset needed to reveal `index`, or `None` if it is fully visible.
    ///
    /// Rows above align at the top and rows below align at the bottom; a row taller than the viewport aligns at the top.
    #[must_use]
    pub fn reveal(
        &self,
        index: u64,
        item_count: u64,
        columns: usize,
        viewport_top: f32,
        viewport_height: f32,
    ) -> Option<f32> {
        if columns == 0 || index >= item_count {
            return None;
        }
        let (row, _) = self.cell_of(index, columns)?;
        let row = usize::try_from(row).ok()?;
        let rows = self.row_model(item_count, columns);
        if row >= rows.len() {
            return None;
        }
        let top = rows.offset_of_f64(row);
        // Visibility uses inked tile height; including the trailing gap would scroll a fully visible bottom row.
        let bottom = top + self.tile_height;
        let current = if viewport_top.is_finite() && viewport_top > 0.0 {
            f64::from(viewport_top)
        } else {
            0.0
        };
        let height = sane_len(viewport_height);
        if top < current {
            return Some(top as f32);
        }
        if bottom > current + height {
            let target = (bottom - height).max(0.0).min(top);
            return Some(target as f32);
        }
        None
    }
}
