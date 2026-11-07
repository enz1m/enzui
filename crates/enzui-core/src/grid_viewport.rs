//! Composes independent one-dimensional row and column windows for virtualized grids.
//!
//! Realization is viewport-bounded, and placement uses each axis model’s anchor.

use core::ops::Range;

use iced_core::{Point, Rectangle, Size};

use crate::virtual_sequence::{Anchor, RealizationWindow, SequenceGeometry};

/// The realised index ranges for both axes: which cells to build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridWindow {
    /// The realised row indices, visible plus row overscan.
    pub rows: Range<usize>,
    /// The realised column indices, visible plus column overscan.
    pub columns: Range<usize>,
}

impl GridWindow {
    /// Returns `rows.len() * columns.len()`, saturating on overflow.
    #[must_use]
    pub fn cells(&self) -> usize {
        self.rows.len().saturating_mul(self.columns.len())
    }

    /// Whether either axis is empty, i.e. no cell is realised.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() || self.columns.is_empty()
    }
}

/// Where one realised cell sits, in viewport-local coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedCell {
    /// The cell's row index.
    pub row: usize,
    /// The cell's column index.
    pub column: usize,
    /// The cell's rectangle, relative to the viewport's top-left corner.
    pub bounds: Rectangle,
}

/// A 2D viewport over a grid: independent row and column anchors and overscans.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GridViewport {
    row_anchor: Anchor,
    column_anchor: Anchor,
    row_window: RealizationWindow,
    column_window: RealizationWindow,
}

impl GridViewport {
    /// A viewport at the grid's top-left corner with no overscan on either axis.
    #[must_use]
    pub fn new() -> Self {
        GridViewport {
            row_anchor: Anchor::top(),
            column_anchor: Anchor::top(),
            row_window: RealizationWindow::new(0),
            column_window: RealizationWindow::new(0),
        }
    }

    /// The row axis' scroll anchor.
    #[must_use]
    pub fn row_anchor(&self) -> &Anchor {
        &self.row_anchor
    }

    /// The column axis' scroll anchor.
    #[must_use]
    pub fn column_anchor(&self) -> &Anchor {
        &self.column_anchor
    }

    /// The row axis' overscan, in rows per side.
    #[must_use]
    pub fn row_overscan(&self) -> usize {
        self.row_window.overscan()
    }

    /// The column axis' overscan, in columns per side.
    #[must_use]
    pub fn column_overscan(&self) -> usize {
        self.column_window.overscan()
    }

    /// Replace the row anchor.
    #[must_use]
    pub fn with_row_anchor(mut self, anchor: Anchor) -> Self {
        self.row_anchor = anchor;
        self
    }

    /// Replace the column anchor.
    #[must_use]
    pub fn with_column_anchor(mut self, anchor: Anchor) -> Self {
        self.column_anchor = anchor;
        self
    }

    /// Anchor the row axis on `index`, keeping the existing sub-item offset.
    #[must_use]
    pub fn with_row_anchor_index(mut self, index: usize) -> Self {
        self.row_anchor = self.row_anchor.with_index(index);
        self
    }

    /// Anchor the column axis on `index`, keeping the existing sub-item offset.
    #[must_use]
    pub fn with_column_anchor_index(mut self, index: usize) -> Self {
        self.column_anchor = self.column_anchor.with_index(index);
        self
    }

    /// Set the row overscan, in rows per side. `0` is honoured literally.
    #[must_use]
    pub fn with_row_overscan(mut self, overscan: usize) -> Self {
        self.row_window = RealizationWindow::new(overscan);
        self
    }

    /// Set the column overscan, in columns per side. `0` is honoured literally.
    #[must_use]
    pub fn with_column_overscan(mut self, overscan: usize) -> Self {
        self.column_window = RealizationWindow::new(overscan);
        self
    }

    /// The cells to build for `viewport`: the visible rows and columns plus each axis' own
    /// overscan.
    #[must_use]
    pub fn realize<R, C>(&self, rows: &R, columns: &C, viewport: Size) -> GridWindow
    where
        R: SequenceGeometry + ?Sized,
        C: SequenceGeometry + ?Sized,
    {
        GridWindow {
            rows: self
                .row_window
                .realize(rows, &self.row_anchor, viewport.height),
            columns: self
                .column_window
                .realize(columns, &self.column_anchor, viewport.width),
        }
    }

    /// [`realize`](Self::realize) and placement in one call: which cells to build and where
    /// each one goes.
    pub fn place<'g, R, C>(
        &self,
        rows: &'g R,
        columns: &'g C,
        viewport: Size,
    ) -> PlacedCells<'g, R, C>
    where
        R: SequenceGeometry + ?Sized,
        C: SequenceGeometry + ?Sized,
    {
        let window = self.realize(rows, columns, viewport);
        self.place_window(rows, columns, window)
    }

    /// Placement for an already-computed window.
    pub fn place_window<'g, R, C>(
        &self,
        rows: &'g R,
        columns: &'g C,
        window: GridWindow,
    ) -> PlacedCells<'g, R, C>
    where
        R: SequenceGeometry + ?Sized,
        C: SequenceGeometry + ?Sized,
    {
        PlacedCells::new(rows, columns, self.row_anchor, self.column_anchor, window)
    }
}

/// An iterator over the [`PlacedCell`]s of a [`GridWindow`], in row-major order.
#[derive(Debug)]
pub struct PlacedCells<'g, R: SequenceGeometry + ?Sized, C: SequenceGeometry + ?Sized> {
    rows: crate::virtual_sequence::PlacedItems<'g, R>,
    strip: Vec<crate::virtual_sequence::Placement>,
    current: Option<crate::virtual_sequence::Placement>,
    next_column: usize,
    remaining: usize,
    _columns: core::marker::PhantomData<&'g C>,
}

impl<'g, R: SequenceGeometry + ?Sized, C: SequenceGeometry + ?Sized> PlacedCells<'g, R, C> {
    fn new(
        rows: &'g R,
        columns: &'g C,
        row_anchor: Anchor,
        column_anchor: Anchor,
        window: GridWindow,
    ) -> Self {
        let strip: Vec<_> = column_anchor
            .place(columns, window.columns.clone())
            .collect();
        let row_items = row_anchor.place(rows, window.rows);
        let remaining = row_items.len().saturating_mul(strip.len());
        PlacedCells {
            rows: row_items,
            strip,
            current: None,
            next_column: 0,
            remaining,
            _columns: core::marker::PhantomData,
        }
    }
}

impl<R: SequenceGeometry + ?Sized, C: SequenceGeometry + ?Sized> Iterator
    for PlacedCells<'_, R, C>
{
    type Item = PlacedCell;

    fn next(&mut self) -> Option<PlacedCell> {
        if self.strip.is_empty() {
            return None;
        }
        loop {
            let row = match self.current {
                Some(row) => row,
                None => {
                    let row = self.rows.next()?;
                    self.current = Some(row);
                    self.next_column = 0;
                    row
                }
            };
            if self.next_column >= self.strip.len() {
                self.current = None;
                continue;
            }
            let column = self.strip[self.next_column];
            self.next_column += 1;
            self.remaining = self.remaining.saturating_sub(1);
            return Some(PlacedCell {
                row: row.index,
                column: column.index,
                bounds: Rectangle::new(
                    Point::new(column.y_offset, row.y_offset),
                    Size::new(column.height, row.height),
                ),
            });
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<R: SequenceGeometry + ?Sized, C: SequenceGeometry + ?Sized> ExactSizeIterator
    for PlacedCells<'_, R, C>
{
}
