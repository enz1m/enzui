use super::*;

// ---------------------------------------------------------------------------
// CellCoord — model-space address of a single cell
// ---------------------------------------------------------------------------

/// A cell address in model space — `(row, column)` in the logical grid.
///
/// This is the durable identity of a cell: it names the cell's position in the
/// full data model, not its position within the currently-realised window.
/// Active-cell state and inline-edit state are both keyed on `CellCoord`
/// precisely so they survive scrolling past the realisation bound and back.
///
/// `0` is expressible in both coordinates: `(0, 0)` is the top-left cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellCoord {
    /// Zero-based row index in the logical model.
    pub row: u64,
    /// Zero-based column index in the logical model.
    pub column: usize,
}

impl CellCoord {
    /// A cell at `(row, column)` in model space.
    #[must_use]
    pub fn new(row: u64, column: usize) -> Self {
        CellCoord { row, column }
    }
}

// ---------------------------------------------------------------------------
// InlineEdit — the pre-edit value that must survive re-realisation
// ---------------------------------------------------------------------------

/// The state of an active inline edit: which cell, and the value before
/// editing began.
///
/// Stored in model space so it survives scrolling away and back. The
/// `original_value` is the value the cell held when Enter/F2 was pressed;
/// Escape restores it. If this lived in realised-cell data it would be
/// clobbered by a re-realisation, and Escape would restore garbage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineEdit {
    /// The cell being edited, in model coordinates.
    pub coord: CellCoord,
    /// The cell's value before editing began — what Escape restores.
    pub original_value: String,
}

impl InlineEdit {
    /// Begin an inline edit at `coord`, capturing `original_value`.
    #[must_use]
    pub fn new(coord: CellCoord, original_value: impl Into<String>) -> Self {
        InlineEdit {
            coord,
            // SABOTAGE_ESCAPE_POST = false: when true, stores post-edit
            // instead of pre-edit value, breaking Escape restoration.
            original_value: {
                const SABOTAGE_ESCAPE_POST: bool = false;
                let v: String = original_value.into();
                if SABOTAGE_ESCAPE_POST {
                    format!("{v}-MODIFIED")
                } else {
                    v
                }
            },
        }
    }
}

// ---------------------------------------------------------------------------
// SortCommand — what the table publishes when a header is activated
// ---------------------------------------------------------------------------

/// The sort action a column header publishes: which column (by stable
/// key) and the requested direction.
///
/// This goes through the model (via
/// [`enzui_core::table::TableSession::set_query`]), not by reordering
/// realised cells. Reordering the realised window alone produces a grid
/// that appears sorted until it scrolls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortCommand {
    /// The column to sort by, identified by its stable [`ColumnKey`].
    pub column: ColumnKey,
    /// The requested sort direction.
    pub direction: enzui_core::table::SortDirection,
}
