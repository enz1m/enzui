//! `EnzTable`: a bounded 2D virtualised data grid consuming
//! [`enzui_core::grid_viewport::GridViewport`].
//!
//! # The property this widget exists to preserve
//!
//! The number of child elements built per frame depends on the viewport size
//! and the local track extents — never on the total row × column count.
//! Under `Length::Fill` the realization window resolves inside iced's layout
//! pass from the actual allocation; fixed dimensions retain deterministic
//! eager realization. [`TableViewportState`] publishes the last allocation
//! for update-time navigation tolerating one-frame lag (row overscan covers
//! the transition). A fixed-row body uses the largest whole-row band; the
//! fractional remainder stays quiet while Fill size and scrollbar own the
//! full allocation.
//!
//! # Architecture
//!
//! This widget composes rather than implementing `Widget` by hand. It builds
//! its cell window from `GridViewport::place` for bounded cell realisation. What
//! it adds over a plain virtual list is the column axis: shared header/body
//! tracks, a frozen column prefix, and a bounded rectangular cell window.
//!
//! # Scroll input
//!
//! The table does not own a `Scrollable`: the offset is caller state fed
//! back through [`EnzTable::scroll_x`]/[`EnzTable::scroll_y`], and
//! [`EnzTable::on_scroll`] publishes the wheel delta in pixels (iced
//! sign convention; `Lines` converted via [`WHEEL_LINE_PX`]). The caller
//! integrates, clamps, and hands it back; the next frame closes the loop. A
//! gesture publishes and captures only when an axis can move — at an
//! endpoint, or when content fits, or with no callback, the wheel stays
//! uncaptured for the nearest scrollable ancestor.
//!
//! # Keyboard ownership
//!
//! The table routes Arrow (active cell), Enter/F2 (edit start/commit), Escape
//! (edit cancel) and <kbd>Ctrl+Enter</kbd> (sort the active column). Every
//! other Ctrl/Alt/Logo chord bubbles untouched, while Shift is widget-owned
//! like the bare press. The claimed keys live in one typed vocabulary,
//! [`TABLE_KEY_SURFACE`] resolved through [`table_key_action`] (with
//! [`table_owns_key`] its boolean face) — so an embedding application
//! patches nothing back, and the surface cannot drift between router,
//! predicate and tests.
//!
//! # Frozen columns
//!
//! With `frozen_columns > 0` the first N columns pin left while the rest
//! scroll. Both regions share one column-track solution, so header and body
//! always align — realised by splitting placed cells into a frozen prefix
//! and a scrollable body in separate clipped containers. Header and body
//! rows use the same geometry model, so every column's header width matches
//! the body exactly; the header renders once and stays fixed vertically.
//!
//! # Counting provider
//!
//! The honest instrument is a [`TableProvider`] with interior-mutable call
//! counters. Asserting through the model re-asserts the model's own policy;
//! asserting through the widget proves the wiring is live.

use enzui_core::grid_viewport::{GridViewport, PlacedCell};
use enzui_core::metrics::{FiniteF32, FiniteNonNeg};
use enzui_core::semantics::{self, GridPosition, Role, Semantics};
use enzui_core::table::{ColumnKey, SortDirection, TableProvider};
use enzui_core::virtual_sequence::{Anchor, HeightModel, VariableHeightModel};
use iced::keyboard;
use iced::mouse;
use iced::widget::{Column, Row, Space, container, mouse_area, row};
use iced::{Background, Border, Color, Element, Length, Size};
use std::rc::Rc;

/// How many pixels one wheel "line" scrolls: `16.0 * 3` — three lines of
/// a nominal 16px row, the common desktop convention.
///
/// Line-based wheel deltas carry no intrinsic pixel size, so the widget picks
/// one for the [`EnzTable::on_scroll`] callback.
pub const WHEEL_LINE_PX: f32 = 48.0;

/// Trailing-edge width (logical px) the scrollbar chrome reserves for its
/// track/thumb lane. The body lays out this much narrower so the last column
/// is never clipped; a caller filling the table with its own columns should
/// lay out across `table_width - table::SCROLLBAR_GUTTER`.
///
/// Derived from [`enzui_theme::ScrollbarTokens::DEFAULT`]'s content gutter.
pub const SCROLLBAR_GUTTER: f32 = scrollbar::GUTTER_PX;

/// Largest whole-row viewport that fits in an allocated fixed-row body.
///
/// This policy is local to `EnzTable`'s [`HeightModel`] row axis.
/// [`VariableHeightModel`] is used here for column widths, not as an alternate
/// table-row model; variable-height virtual surfaces keep their allocated
/// viewport unchanged.
fn fixed_row_viewport_height(allocated: f32, row_height: f32) -> f32 {
    if !allocated.is_finite() || allocated <= 0.0 {
        return 0.0;
    }
    if !row_height.is_finite() || row_height <= 0.0 {
        return allocated;
    }

    // Keep the division in the same f32 coordinate space as iced. Widening
    // only the division can turn an allocation produced by `n * row_height`
    // into `n - epsilon` and incorrectly discard an exact final row.
    let rows = (allocated / row_height).floor();
    (rows * row_height).min(allocated)
}

// model-space types lifted into a sibling module.
// Re-exported so callers and the test module keep reaching them unqualified.
mod model;

pub use model::*;

// responsive column priority and the row-detail fold destination.
// See `fold.rs` for why the value projection is a constructor argument and
// how the conservation property holds by construction.
mod fold;

pub use fold::*;

use fold::FoldRow;

// the row-level hover surface. See `row_hover.rs` for the four design
// decisions (paint over, band-based hover, on by default, transparent).
mod row_hover;

use row_hover::RowHover;

// the scrollbar chrome. See `scrollbar.rs` for the four design decisions
// (thumb reflects real extent, no chrome when content fits, gutter clears the
// last column, drag agrees with the wheel).
mod scrollbar;

use scrollbar::TableScrollbar;

// keep a full parent-owned table allocation while clipping fixed rows
// to a whole-row body and positioning overscan outside that clip.
mod body_viewport;

use body_viewport::TableBodyViewport;

// header-edge drag handles, double-click fit and the keyboard step.
mod column_resize;

pub use column_resize::{COLUMN_RESIZE_STEP, ColumnResize, DEFAULT_MIN_COLUMN_WIDTH};
use column_resize::{FitRunner, MeasureCell, ResizeCallback, ResizeHandle};

//  test seam: re-export the doc-hidden constructor so
// `layout_spill_widget_sweep` can put a bare `TableScrollbar` at the root of
// an `Element` and gate its `layout()` in isolation. Production path is
// `EnzTable::scrollbar` / `EnzTable::on_scroll_to`.
#[doc(hidden)]
pub use scrollbar::__test_only_table_scrollbar;

use crate::semantics_container::SemanticsContainer;
use crate::wheel_capture::{offset_can_move, wheel_capture};

// ---------------------------------------------------------------------------
// Keyboard ownership
// ---------------------------------------------------------------------------

/// One value per keyboard gesture the table router owns — the whole command
/// vocabulary of the widget, in one type.
///
/// Router, ownership predicate and test matrix all resolve through
/// [`table_key_action`]: a match on this enum is exhaustive, so adding a
/// command forces every consumer to handle it and the claimed set cannot
/// drift between the three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TableKeyAction {
    /// Move the active cell up one row (<kbd>ArrowUp</kbd>).
    NavigateUp,
    /// Move the active cell down one row (<kbd>ArrowDown</kbd>).
    NavigateDown,
    /// Move the active cell left one column (<kbd>ArrowLeft</kbd>).
    NavigateLeft,
    /// Move the active cell right one column (<kbd>ArrowRight</kbd>).
    NavigateRight,
    /// Start an edit on the active cell, or commit the in-flight edit
    /// (<kbd>Enter</kbd>).
    EditEnterOrCommit,
    /// Start an edit on the active cell (<kbd>F2</kbd>).
    EditStart,
    /// Cancel the in-flight edit (<kbd>Escape</kbd>).
    EditCancel,
    /// Sort the active cell's column (<kbd>Ctrl+Enter</kbd>).
    Sort,
    /// Narrow the active cell's column by [`COLUMN_RESIZE_STEP`]
    /// (<kbd>Alt+Shift+Left</kbd>).
    NarrowColumn,
    /// Widen the active cell's column by [`COLUMN_RESIZE_STEP`]
    /// (<kbd>Alt+Shift+Right</kbd>).
    WidenColumn,
}

/// The table's bare key surface: the seven named keys it owns with no
/// application modifier, each paired with the action it commands.
///
/// The single production enumeration: nothing else lists the keys, so the
/// length is typed at seven and adding or removing one is a
/// compile-visible change rippling into every consumer.
pub const TABLE_KEY_SURFACE: [(keyboard::key::Named, TableKeyAction); 7] = {
    use TableKeyAction as A;
    use keyboard::key::Named as N;
    [
        (N::ArrowUp, A::NavigateUp),
        (N::ArrowDown, A::NavigateDown),
        (N::ArrowLeft, A::NavigateLeft),
        (N::ArrowRight, A::NavigateRight),
        (N::Enter, A::EditEnterOrCommit),
        (N::F2, A::EditStart),
        (N::Escape, A::EditCancel),
    ]
};

/// Resolve a key press to the [`TableKeyAction`] it commands, or `None` when
/// the press belongs to the application or an active child editor.
///
/// The table owns its seven [`TABLE_KEY_SURFACE`] keys **bare or with
/// <kbd>Shift</kbd>**; <kbd>Ctrl</kbd>/<kbd>Alt</kbd>/<kbd>Logo</kbd> bubble
/// untouched — *except* the reserved commands: <kbd>Ctrl+Enter</kbd>
/// re-targets Enter to [`TableKeyAction::Sort`] (plain Enter already runs the
/// edit lifecycle, so the keyboard sort route needs a modifier), and
/// <kbd>Alt+Shift+Left</kbd>/<kbd>Right</kbd> resize the active column.
/// The router's single ownership decision: no per-app patch needed.
pub fn table_key_action(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
) -> Option<TableKeyAction> {
    let keyboard::Key::Named(named) = key else {
        return None;
    };
    let bare = TABLE_KEY_SURFACE
        .iter()
        .find_map(|(surface_key, action)| (surface_key == named).then_some(*action))?;

    // The one reserved modifier-bearing command: Ctrl+Enter (Shift indifferent)
    // re-targets Enter to sorting the active cell's column.
    if *named == keyboard::key::Named::Enter
        && modifiers.control()
        && !modifiers.alt()
        && !modifiers.logo()
    {
        return Some(TableKeyAction::Sort);
    }

    // Alt+Shift+Left/Right resize the active cell's column.
    if modifiers.alt() && modifiers.shift() && !modifiers.control() && !modifiers.logo() {
        return match named {
            keyboard::key::Named::ArrowLeft => Some(TableKeyAction::NarrowColumn),
            keyboard::key::Named::ArrowRight => Some(TableKeyAction::WidenColumn),
            _ => None,
        };
    }

    // Any other application chord bubbles to the application.
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }

    // Bare or Shift-modified: the key's bare action.
    Some(bare)
}

/// Whether the table's keyboard router owns `key` pressed with `modifiers`.
///
/// True exactly when [`table_key_action`] resolves a command; see it for the
/// ownership rule.
pub fn table_owns_key(key: &keyboard::Key, modifiers: keyboard::Modifiers) -> bool {
    table_key_action(key, modifiers).is_some()
}

// ---------------------------------------------------------------------------
// Builder types
// ---------------------------------------------------------------------------

/// How a cell is rendered. The caller supplies a closure mapping
/// `(row_index, column_index, cell_value)` to an `Element`.
type CellBuilder<'a, Message, Cell> = Rc<dyn Fn(u64, usize, &Cell) -> Element<'a, Message> + 'a>;

/// How a header cell is rendered. Maps `(column_index, column_key)` to an
/// `Element`.
type HeaderBuilder<'a, Message> = Rc<dyn Fn(usize, &ColumnKey) -> Element<'a, Message> + 'a>;

/// Decides whether a model-space row is selected.
type RowSelection<'a> = Rc<dyn Fn(u64) -> bool + 'a>;

/// Builds the accessible label and activation message for a model-space row.
type RowActivation<'a, Message> = Rc<dyn Fn(u64) -> (String, Message) + 'a>;

/// The result of a single frame's realization: how many cells were built, what
/// the window was, and the viewport-local placements. Exposed for test
/// instrumentation.
#[derive(Debug, Clone)]
pub struct RealizationInfo {
    /// How many body cells were realized (placed × provider hits).
    pub cell_count: usize,
    /// The row range realized.
    pub row_range: core::ops::Range<usize>,
    /// The column range realized.
    pub column_range: core::ops::Range<usize>,
}

// ---------------------------------------------------------------------------
// EnzTable
// ---------------------------------------------------------------------------

/// The last finite allocation an [`EnzTable`] used for realization.
///
/// Keep this in application state and pass it to [`EnzTable::viewport_state`]
/// when update-time navigation or scroll clamping needs the table's resolved
/// geometry. Callers can read it, but only the table can publish a new value.
/// Before the first layout, [`TableViewportState::resolved`] returns `None`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TableViewport {
    /// Full table allocation, including the header.
    pub size: Size,
    /// Effective body height used by realization, scrolling, and scrollbar
    /// geometry. For fixed-height rows this is the largest whole-row band that
    /// fits below the header; any remaining allocation stays quiet below it.
    pub body_height: f32,
}

/// Stable read-only seam for an [`EnzTable`]'s last resolved viewport.
#[derive(Debug, Default)]
pub struct TableViewportState(std::cell::Cell<Option<TableViewport>>);

impl TableViewportState {
    /// Create an unresolved viewport state.
    #[must_use]
    pub const fn new() -> Self {
        Self(std::cell::Cell::new(None))
    }

    /// Return the last allocation consumed by table realization.
    #[must_use]
    pub fn resolved(&self) -> Option<TableViewport> {
        self.0.get()
    }

    fn publish(&self, viewport: TableViewport) {
        self.0.set(Some(viewport));
    }
}

/// A bounded 2D virtualised data grid.
///
/// Construct with [`enz_table`]. See the [module docs](self) for the architecture
/// and the bounded-realization property.
pub struct EnzTable<'a, Message, Cell> {
    row_geometry: HeightModel,
    /// Shared, not copied, by each rebuild a responsive table makes.
    column_geometry: Rc<VariableHeightModel>,
    provider: &'a dyn TableProvider<Cell>,
    cell_builder: CellBuilder<'a, Message, Cell>,
    header_builder: Option<HeaderBuilder<'a, Message>>,
    frozen_columns: usize,
    row_overscan: usize,
    column_overscan: usize,
    width: Length,
    height: Length,
    scroll_x: f32,
    scroll_y: f32,
    on_scroll: Option<Rc<dyn Fn(f32, f32) -> Message + 'a>>,
    header_height: f32,
    header_gap: f32,
    /// Publishes the finite allocation used for realization during layout.
    viewport_state: Option<&'a TableViewportState>,
    /// Optional handle for the last realization info (for test instrumentation).
    realization_sink: Option<&'a std::cell::RefCell<Option<RealizationInfo>>>,
    // --- active cell, inline edit, column sort -------------------------
    /// The currently active (focused) cell in model space, or `None`.
    active_cell: Option<CellCoord>,
    /// Callback when the active cell changes (e.g. via keyboard navigation).
    on_active_cell_change: Option<Rc<dyn Fn(CellCoord) -> Message + 'a>>,
    /// The current inline edit state, or `None` if no cell is being edited.
    /// Lives in the caller’s application state and is lent here so it survives
    /// re-realisation.
    editing: Option<&'a InlineEdit>,
    /// Callback when an inline edit is committed (Enter or blur).
    /// Receives the cell coordinate and the committed value.
    on_edit_commit: Option<Rc<dyn Fn(CellCoord, String) -> Message + 'a>>,
    /// Callback when an inline edit is cancelled (Escape).
    /// Receives the cell coordinate and the original (pre-edit) value.
    on_edit_cancel: Option<Rc<dyn Fn(CellCoord, String) -> Message + 'a>>,
    /// Callback when an inline edit is started (Enter/F2 on the active cell).
    /// Receives the cell coordinate and the current cell value.
    on_edit_start: Option<Rc<dyn Fn(CellCoord, String) -> Message + 'a>>,
    /// Callback when a column header is activated for sorting.
    /// Receives a [`SortCommand`] so the caller can route it through
    /// `TableSession::set_query` and the model.
    on_column_sort: Option<Rc<dyn Fn(SortCommand) -> Message + 'a>>,
    /// The column the table is currently sorted by and in which direction,
    /// or `None` if the table is unsorted. Lent by the caller so the widget can
    /// (1) toggle the direction when a header is activated and (2) announce
    /// `aria-sort` on the matching `Role::ColumnHeader`. Set via
    /// [`EnzTable::sort_state`].
    sort_state: Option<(ColumnKey, SortDirection)>,
    /// Test instrumentation: the active cell coordinate seen during the
    /// last realization, or `None` if no active cell was set.
    active_cell_sink: Option<&'a std::cell::RefCell<Option<CellCoord>>>,
    /// Test instrumentation: how many cells were highlighted with the
    /// active-cell border during the last realization.
    highlight_count_sink: Option<&'a std::cell::Cell<usize>>,
    // --- row-level state surfaces -------------------------------
    /// Whether the hovered row is washed with the hover overlay.
    /// Defaults to `true`; see [`EnzTable::row_hover`].
    row_hover: bool,
    /// Optional model-space selection predicate. When present, every realised
    /// row publishes selected state and selected rows receive one continuous
    /// row-wide fill; see [`EnzTable::row_selection`].
    row_selection: Option<RowSelection<'a>>,
    /// Optional row-level activation. When present, hover, press and selection
    /// are painted once around the assembled row instead of by each cell.
    row_activation: Option<RowActivation<'a, Message>>,
    /// Corner radius for row-level hover/press/selection surfaces.
    row_radius: f32,
    // --- scrollbar chrome ----------------------------------------------
    /// Whether scrollbar chrome is requested when content overflows. Chrome
    /// is painted only when [`EnzTable::on_scroll_to`] is also wired, so a
    /// grabbable-looking thumb can never be inert.
    scrollbar: bool,
    /// Callback publishing a new absolute scroll offset (px) when the thumb
    /// is dragged. `None` suppresses the interactive chrome entirely.
    on_scroll_to: Option<Rc<dyn Fn(f32) -> Message + 'a>>,
    // --- responsive column priority + row-detail fold ----------------
    /// The declared column priority and its fold destination, or `None` — the
    /// default — for exactly the default behaviour.
    column_fold: Option<ColumnFold<'a, Message, Cell>>,
    // --- column resizing -------------------------------------------------
    /// Publishes the width a header drag, fit or keyboard step asks for;
    /// `None` leaves the columns fixed.
    on_column_resize: Option<ResizeCallback<'a, Message>>,
    /// The narrowest a resize makes a column.
    min_column_width: f32,
}

// Manual so cloning the `Rc<dyn Fn(.. &Cell)>` callbacks does not impose a
// spurious `Cell: Clone` API bound. Responsive layout needs a fresh owned
// builder on every pass because each pass produces a new child tree.
impl<Message, Cell> Clone for EnzTable<'_, Message, Cell> {
    fn clone(&self) -> Self {
        Self {
            row_geometry: self.row_geometry,
            column_geometry: Rc::clone(&self.column_geometry),
            provider: self.provider,
            cell_builder: Rc::clone(&self.cell_builder),
            header_builder: self.header_builder.clone(),
            frozen_columns: self.frozen_columns,
            row_overscan: self.row_overscan,
            column_overscan: self.column_overscan,
            width: self.width,
            height: self.height,
            scroll_x: self.scroll_x,
            scroll_y: self.scroll_y,
            on_scroll: self.on_scroll.clone(),
            header_height: self.header_height,
            header_gap: self.header_gap,
            viewport_state: self.viewport_state,
            realization_sink: self.realization_sink,
            active_cell: self.active_cell,
            on_active_cell_change: self.on_active_cell_change.clone(),
            editing: self.editing,
            on_edit_commit: self.on_edit_commit.clone(),
            on_edit_cancel: self.on_edit_cancel.clone(),
            on_edit_start: self.on_edit_start.clone(),
            on_column_sort: self.on_column_sort.clone(),
            sort_state: self.sort_state.clone(),
            active_cell_sink: self.active_cell_sink,
            highlight_count_sink: self.highlight_count_sink,
            row_hover: self.row_hover,
            row_selection: self.row_selection.clone(),
            row_activation: self.row_activation.clone(),
            row_radius: self.row_radius,
            scrollbar: self.scrollbar,
            on_scroll_to: self.on_scroll_to.clone(),
            column_fold: self.column_fold.clone(),
            on_column_resize: self.on_column_resize.clone(),
            min_column_width: self.min_column_width,
        }
    }
}

/// Construct a new [`EnzTable`].
///
/// # Arguments
///
/// - `provider`: rows, columns, and cells.
/// - `row_geometry`: the row axis geometry (uniform heights).
/// - `column_geometry`: the column axis geometry (may be variable widths).
/// - `cell_builder`: `(row, column, &cell)` to an `Element`.
pub fn enz_table<'a, Message, Cell, F>(
    provider: &'a dyn TableProvider<Cell>,
    row_geometry: HeightModel,
    column_geometry: VariableHeightModel,
    cell_builder: F,
) -> EnzTable<'a, Message, Cell>
where
    Message: 'a,
    Cell: 'a,
    F: Fn(u64, usize, &Cell) -> Element<'a, Message> + 'a,
{
    EnzTable {
        row_geometry,
        column_geometry: Rc::new(column_geometry),
        provider,
        cell_builder: Rc::new(cell_builder),
        header_builder: None,
        frozen_columns: 0,
        row_overscan: 2,
        column_overscan: 1,
        width: Length::Fill,
        height: Length::Fill,
        scroll_x: 0.0,
        scroll_y: 0.0,
        on_scroll: None,
        header_height: 32.0,
        header_gap: 0.0,
        viewport_state: None,
        realization_sink: None,
        active_cell: None,
        on_active_cell_change: None,
        editing: None,
        on_edit_commit: None,
        on_edit_cancel: None,
        on_edit_start: None,
        on_column_sort: None,
        sort_state: None,
        active_cell_sink: None,
        highlight_count_sink: None,
        // a data grid is expected to light the row under the pointer, so
        // this is the default rather than something apps must remember.
        row_hover: true,
        row_selection: None,
        row_activation: None,
        row_radius: 10.0,
        // a scrollable list is expected to show a scrollbar, so
        // chrome is requested by default. It paints only once `on_scroll_to`
        // is wired; an inert thumb would be a false interaction promise.
        scrollbar: true,
        on_scroll_to: None,
        // no priority declared -> no fold, no responsive column set,
        // no extra widget in the tree. Today's behaviour is the default.
        column_fold: None,
        on_column_resize: None,
        min_column_width: DEFAULT_MIN_COLUMN_WIDTH,
    }
}

impl<'a, Message, Cell> EnzTable<'a, Message, Cell>
where
    Message: Clone + 'a,
    Cell: 'a,
{
    /// Set the number of frozen (pinned) columns on the left.
    ///
    /// This may not be combined with [`Self::on_row_activate`]: frozen and
    /// scrolling panes contain separate physical row elements, so they cannot
    /// expose one honest logical-row action. The combination is rejected when
    /// the element is built.
    #[must_use]
    pub fn frozen_columns(mut self, n: usize) -> Self {
        self.frozen_columns = n;
        self
    }

    /// Set the row overscan (extra rows above/below the viewport).
    #[must_use]
    pub fn row_overscan(mut self, n: usize) -> Self {
        self.row_overscan = n;
        self
    }

    /// Set the column overscan (extra columns left/right of the viewport).
    #[must_use]
    pub fn column_overscan(mut self, n: usize) -> Self {
        self.column_overscan = n;
        self
    }

    /// Set the widget width.
    #[must_use]
    pub fn width(mut self, w: Length) -> Self {
        self.width = w;
        self
    }

    /// Set the widget height.
    #[must_use]
    pub fn height(mut self, h: Length) -> Self {
        self.height = h;
        self
    }

    /// Set the horizontal scroll offset.
    ///
    /// A signed position on the column axis: negatives name content before
    /// the origin, so this is a [`FiniteF32`] — only `NaN`/`±inf` are
    /// rejected, degrading to `0.0`. `0.0` is honoured literally. Keeps a
    /// poisoned offset out of the realization math that selects cells.
    #[must_use]
    pub fn scroll_x(mut self, x: f32) -> Self {
        self.scroll_x = FiniteF32::new_or(x, 0.0).get();
        self
    }

    /// Set the vertical scroll offset.
    ///
    /// As [`Self::scroll_x`], on the row axis: a [`FiniteF32`] that rejects only
    /// `NaN`/`±inf` (degrading to `0.0`) while passing any finite value,
    /// including negatives, through unchanged.
    #[must_use]
    pub fn scroll_y(mut self, y: f32) -> Self {
        self.scroll_y = FiniteF32::new_or(y, 0.0).get();
        self
    }

    /// Set the header row height.
    ///
    /// A non-negative extent reaching layout as `Length::Fixed`, so a
    /// [`FiniteNonNeg`]: `NaN`, `±inf` and negatives degrade to the `32.0`
    /// default, while `0.0` is honoured literally (no header row, not
    /// "unset").
    #[must_use]
    pub fn header_height(mut self, h: f32) -> Self {
        self.header_height = FiniteNonNeg::new_or(h, 32.0).get();
        self
    }

    /// Set a callback for wheel-scroll gestures.
    ///
    /// The table holds no offset of its own; this is the input half of the
    /// caller-owned loop. When the pointer is over the table and the wheel
    /// turns, the closure gets the wheel delta in pixels as
    /// `(dx, dy)` (iced sign convention; lines converted via
    /// [`WHEEL_LINE_PX`]). The caller integrates (usually `offset - dy`),
    /// clamps, and hands it back as `scroll_y` next frame. Runs only when an
    /// axis can move; at an endpoint, or when content fits, the wheel stays
    /// uncaptured. Unset: likewise untouched.
    #[must_use]
    pub fn on_scroll(mut self, f: impl Fn(f32, f32) -> Message + 'a) -> Self {
        self.on_scroll = Some(Rc::new(f));
        self
    }

    /// Leave quiet space below the header divider, outside row paint and scrolling.
    /// Defaults to zero; negative and nonfinite values use zero.
    #[must_use]
    pub fn header_gap(mut self, gap: f32) -> Self {
        self.header_gap = FiniteNonNeg::new_or(gap, 0.0).get();
        self
    }

    /// Let people resize columns: each header gets a drag strip on its
    /// trailing edge (with the resize cursor), a double-click there fits the
    /// column to its header and realised cells, and
    /// <kbd>Alt+Shift+Left</kbd>/<kbd>Right</kbd> narrow or widen the active
    /// cell's column by [`COLUMN_RESIZE_STEP`].
    ///
    /// The table keeps no widths: write [`ColumnResize::width`] into the
    /// column geometry, and store it when [`ColumnResize::finished`] is set.
    /// Needs a [`header`](Self::header).
    #[must_use]
    pub fn on_column_resize(mut self, f: impl Fn(ColumnResize) -> Message + 'a) -> Self {
        self.on_column_resize = Some(Rc::new(f));
        self
    }

    /// The narrowest a resize makes a column; default
    /// [`DEFAULT_MIN_COLUMN_WIDTH`].
    #[must_use]
    pub fn min_column_width(mut self, width: f32) -> Self {
        self.min_column_width = FiniteNonNeg::new_or(width, DEFAULT_MIN_COLUMN_WIDTH).get();
        self
    }

    /// Set a header builder. If set, a header row is rendered above the body
    /// using shared column tracks.
    #[must_use]
    pub fn header<F>(mut self, f: F) -> Self
    where
        F: Fn(usize, &ColumnKey) -> Element<'a, Message> + 'a,
    {
        self.header_builder = Some(Rc::new(f));
        self
    }

    /// Publish the actual outer allocation and effective body viewport.
    ///
    /// For fixed-height rows, `body_height` rounds down to a whole-row band
    /// while `size` stays the untouched parent allocation — for update-time
    /// scroll clamping and keyboard reveal. Read-only: callers retain the
    /// state and read [`resolved`](TableViewportState::resolved) while
    /// publication stays internal. May lag one frame; paint and realization
    /// always use the current allocation.
    #[must_use]
    pub fn viewport_state(mut self, state: &'a TableViewportState) -> Self {
        self.viewport_state = Some(state);
        self
    }

    /// Attach a `RefCell<Option<RealizationInfo>>` for test instrumentation.
    /// The widget writes its realization counts here every frame.
    #[must_use]
    pub fn realization_sink(
        mut self,
        sink: &'a std::cell::RefCell<Option<RealizationInfo>>,
    ) -> Self {
        self.realization_sink = Some(sink);
        self
    }

    // --- active cell, inline edit, column sort -------------------------

    /// Set the currently active (focused) cell in model space.
    ///
    /// The active cell is distinct from selection: it is the one cell that has
    /// keyboard focus and will receive Enter/F2 to begin editing. It is stored
    /// in model coordinates so it survives scrolling past the realisation
    /// bound and back.
    #[must_use]
    pub fn active_cell(mut self, coord: CellCoord) -> Self {
        self.active_cell = Some(coord);
        self
    }

    /// Callback when the active cell changes (keyboard navigation).
    #[must_use]
    pub fn on_active_cell_change(mut self, f: impl Fn(CellCoord) -> Message + 'a) -> Self {
        self.on_active_cell_change = Some(Rc::new(f));
        self
    }

    /// Lend the current inline-edit state.
    ///
    /// The `InlineEdit` carries the cell coordinate and the pre-edit value,
    /// both in model space. The pre-edit value must survive a re-realisation
    /// so Escape can restore it; that is why it lives in the caller’s
    /// application state and is lent here.
    #[must_use]
    pub fn editing(mut self, edit: &'a InlineEdit) -> Self {
        self.editing = Some(edit);
        self
    }

    /// Callback when an inline edit begins (Enter/F2 on the active cell).
    /// Receives the cell coordinate and the cell’s current value.
    #[must_use]
    pub fn on_edit_start(mut self, f: impl Fn(CellCoord, String) -> Message + 'a) -> Self {
        self.on_edit_start = Some(Rc::new(f));
        self
    }

    /// Callback when an inline edit is committed (Enter or blur).
    /// Receives the cell coordinate and the committed value.
    #[must_use]
    pub fn on_edit_commit(mut self, f: impl Fn(CellCoord, String) -> Message + 'a) -> Self {
        self.on_edit_commit = Some(Rc::new(f));
        self
    }

    /// Callback when an inline edit is cancelled (Escape).
    /// Receives the cell coordinate and the original (pre-edit) value.
    #[must_use]
    pub fn on_edit_cancel(mut self, f: impl Fn(CellCoord, String) -> Message + 'a) -> Self {
        self.on_edit_cancel = Some(Rc::new(f));
        self
    }

    /// Callback when a column header is activated for sorting.
    ///
    /// Receives a [`SortCommand`] carrying the column's stable [`ColumnKey`]
    /// and the requested [`SortDirection`].
    /// Route it through the model
    /// ([`set_query`](enzui_core::table::TableSession::set_query)), not
    /// by reordering realised cells.
    #[must_use]
    pub fn on_column_sort(mut self, f: impl Fn(SortCommand) -> Message + 'a) -> Self {
        self.on_column_sort = Some(Rc::new(f));
        self
    }

    /// Declare the column the table is currently sorted by and its
    /// direction. `None` (default) = unsorted.
    ///
    /// The single source driving what bare [`on_column_sort`](Self::on_column_sort)
    /// cannot: the header tree publishes `aria-sort` on the matching
    /// [`Role::ColumnHeader`], and activating a header toggles the *current*
    /// direction instead of always re-sorting ascending.
    #[must_use]
    pub fn sort_state(mut self, state: Option<(ColumnKey, SortDirection)>) -> Self {
        self.sort_state = state;
        self
    }

    /// Attach a sink for the active cell coordinate, for test
    /// instrumentation. The widget writes the active cell here every frame.
    #[must_use]
    pub fn active_cell_sink(mut self, sink: &'a std::cell::RefCell<Option<CellCoord>>) -> Self {
        self.active_cell_sink = Some(sink);
        self
    }

    /// Attach a counter for highlighted cells, for test instrumentation.
    /// The widget writes the number of highlighted (border-drawn) cells here
    /// every frame.
    #[must_use]
    pub fn highlight_count_sink(mut self, sink: &'a std::cell::Cell<usize>) -> Self {
        self.highlight_count_sink = Some(sink);
        self
    }

    /// Turn the row-level hover wash on or off. Defaults to `true`.
    ///
    /// Pointing inside a realised row washes that rectangle; the wash paints
    /// over the row in its own layer, so selection fill, striping and the
    /// active-cell border all stay visible under it. Frozen prefix and body
    /// are separate rectangles with independent feedback. Before this, a
    /// per-cell pressable lit exactly one cell of the row — only the row
    /// element knows its horizontal extent.
    ///
    /// `false` for a static read-only grid (a tint would promise a missing
    /// affordance), or where cells legitimately own their own hover. The
    /// default models one row-wide click target.
    #[must_use]
    pub fn row_hover(mut self, on: bool) -> Self {
        self.row_hover = on;
        self
    }

    /// Paint and publish selection at the row level.
    ///
    /// The predicate takes the stable model-space row index, evaluated only
    /// for realised rows; single and multiple selection share the API. With
    /// reorderable rows, resolve through the row model's key — never treat
    /// the index as durable identity. A selected row gets one
    /// `surface_selected` background behind the whole assembled row; the
    /// hover wash composites over it, the active-cell border above both.
    /// Every realised row publishes `selected`, so selection survives in the
    /// tree without per-cell pressables. Omit for a non-selectable table.
    #[must_use]
    pub fn row_selection(mut self, selected: impl Fn(u64) -> bool + 'a) -> Self {
        self.row_selection = Some(Rc::new(selected));
        self
    }

    /// Make each assembled row the one activation and interaction surface.
    /// One independently-pressed rectangle per cell is avoided while keeping
    /// a single named action for assistive technology.
    /// [`row_hover`](Self::row_hover)`(false)` still suppresses hover paint
    /// while retaining press, activation and selection.
    ///
    /// # Panics
    ///
    /// Building panics with non-zero [`frozen_columns`](Self::frozen_columns):
    /// frozen and scrolling panes hold separate physical row elements, so
    /// neither can own the one logical-row action.
    #[must_use]
    pub fn on_row_activate(mut self, activate: impl Fn(u64) -> (String, Message) + 'a) -> Self {
        self.row_activation = Some(Rc::new(activate));
        self
    }

    /// Set the radius of row hover, pressed and selected states.
    ///
    /// `NaN`, either infinity, and negative values degrade to the 10 px
    /// row-surface default. Zero is valid and paints square row state surfaces;
    /// every finite non-negative radius is forwarded unchanged.
    #[must_use]
    pub fn row_radius(mut self, radius: f32) -> Self {
        self.row_radius = FiniteNonNeg::new_or(radius, 10.0).get();
        self
    }

    /// Request interactive scrollbar chrome. Defaults to `true`, but
    /// chrome paints only when [`on_scroll_to`](Self::on_scroll_to) is wired.
    ///
    /// When requested, wired, and overflowing, the table provides the overlay
    /// thumb in a trailing 12 px band, driven by the caller-owned offset —
    /// hidden at rest, revealed on point-or-drag, no-op when everything fits.
    /// Unwired suppression is deliberate: a thumb looks grabbable, so painting
    /// one without a callback promises an unperformable interaction.
    #[must_use]
    pub fn scrollbar(mut self, on: bool) -> Self {
        self.scrollbar = on;
        self
    }

    /// Set a callback for absolute scroll positions produced by dragging
    /// the scrollbar thumb.
    ///
    /// The drag half of the loop ([`on_scroll`](Self::on_scroll) is the wheel
    /// half, publishing a *delta*): invoked with the new absolute offset
    /// in px, mapped from thumb travel onto the content extent. Route through
    /// the same clamp as the wheel path, so drag-bottom and wheel-bottom
    /// agree.
    #[must_use]
    pub fn on_scroll_to(mut self, f: impl Fn(f32) -> Message + 'a) -> Self {
        self.on_scroll_to = Some(Rc::new(f));
        self
    }

    /// Declare a column priority order and its fold destination.
    ///
    /// Without it every column keeps its track at every width. With it, the
    /// table measures its own allocation (not an app breakpoint) and
    /// yields columns in order, projecting values into a per-row detail
    /// region where they stay named, reachable and operable. See
    /// [`ColumnFold`] and [`plan_columns`].
    ///
    /// # Two things change under a declared fold
    ///
    /// * The column axis realises in full. A conserved value must be
    ///   *had*; a viewport-sized window lacks the folded-out columns, and
    ///   the provider contract bounds columns to dozens. The row axis
    ///   stays bounded as before.
    /// * Frozen columns are ignored. A frozen prefix and a priority fold
    ///   are two answers to "what stays when space runs out"; they cannot
    ///   both measure the one allocation. Declaring a fold supersedes
    ///   [`frozen_columns`](Self::frozen_columns), never silently disagreeing.
    #[must_use]
    pub fn column_fold(mut self, fold: ColumnFold<'a, Message, Cell>) -> Self {
        self.column_fold = Some(fold);
        self
    }

    /// Build the element tree. This is where bounded realization happens.
    ///
    /// Fixed-size tables keep their deterministic eager path. Any responsive
    /// axis rebuilds inside iced's `responsive` widget, whose closure receives
    /// the actual allocation every layout pass — so that allocation, not an
    /// app's chrome estimate, authorises the realised windows and scrollbar
    /// geometry.
    ///
    /// # Panics
    ///
    /// When row activation is combined with frozen columns.
    pub fn into_element(self) -> Element<'a, Message>
    where
        Message: 'a,
    {
        // Refused at build, not at first layout: a `responsive` table would
        // otherwise defer the panic until the view is laid out.
        assert!(
            self.row_activation.is_none() || self.frozen_columns == 0,
            "EnzTable::on_row_activate does not support frozen_columns: frozen and scrolling panes cannot both own one logical-row action"
        );
        let width = self.width;
        let height = self.height;
        if let (Length::Fixed(width), Length::Fixed(height)) = (width, height) {
            return self.into_element_at(Size::new(width, height));
        }

        iced::widget::responsive(move |allocation| self.clone().into_element_at(allocation))
            .width(width)
            .height(height)
            .into()
    }

    fn into_element_at(mut self, allocation: Size) -> Element<'a, Message> {
        // An unbounded parent can report infinity. Preserve the default
        // deterministic fallback in that case; finite zero is real allocation.
        let viewport_w = if allocation.width.is_finite() && allocation.width >= 0.0 {
            allocation.width
        } else {
            match self.width {
                Length::Fixed(w) => w,
                _ => 800.0,
            }
        };
        let viewport_total_h = if allocation.height.is_finite() && allocation.height >= 0.0 {
            allocation.height
        } else {
            match self.height {
                Length::Fixed(h) => h,
                _ => 600.0 + self.header_height,
            }
        };
        let header_gap = if self.header_builder.is_some() {
            self.header_gap
        } else {
            0.0
        };
        let allocated_body_h = (viewport_total_h - self.header_height - header_gap).max(0.0);
        let viewport_h =
            fixed_row_viewport_height(allocated_body_h, self.row_geometry.item_height());
        if let Some(state) = self.viewport_state {
            state.publish(TableViewport {
                size: Size::new(viewport_w, viewport_total_h),
                body_height: viewport_h,
            });
        }

        // scrollbar inputs, captured before `self` is consumed by the
        // keyboard closure below. `content_height` is the TOTAL row extent
        // (not the realised window) so the thumb reflects the real content
        // extent; `body_viewport_h` is the effective whole-row body used for
        // range and thumb fraction. The wrapper still occupies the complete
        // allocated body, so the physical track does not shrink.
        let on_scroll_to = self.on_scroll_to;
        // chrome is an interaction promise. Never paint it unless the
        // absolute-offset callback makes dragging real. Keeping the existing
        // optional builder avoids a app-wide compile break while making the
        // default unwired state honest; adopters gain the bar by wiring one
        // callback through the same clamp as their wheel path.
        let scrollbar_on = self.scrollbar && on_scroll_to.is_some();
        let scrollbar_content_height = self.row_geometry.total_height();
        let scrollbar_viewport_h = viewport_h;
        let scrollbar_scroll_y = self.scroll_y;

        // a declared fold supersedes the frozen prefix (see
        // `column_fold`), so the whole row is one measured region.
        let fold = self.column_fold.take();
        let frozen = if fold.is_some() {
            0
        } else {
            self.frozen_columns.min(self.column_geometry.len())
        };
        // Compute frozen column total width.
        let frozen_width: f32 = (0..frozen).map(|i| self.column_geometry.height_of(i)).sum();

        // The scrollable body viewport excludes frozen columns.
        let body_viewport_w = (viewport_w - frozen_width).max(0.0);

        // Build the GridViewport for the scrollable (non-frozen) columns.
        let row_anchor = Anchor::from_scroll_top(&self.row_geometry, self.scroll_y);
        let body_vp = GridViewport::new()
            .with_row_anchor(row_anchor)
            .with_column_anchor(Anchor::from_scroll_top(
                &*self.column_geometry,
                self.scroll_x + frozen_width,
            ))
            .with_row_overscan(self.row_overscan)
            .with_column_overscan(self.column_overscan);

        let body_vp_size = Size::new(body_viewport_w, viewport_h);
        let body_window = body_vp.realize(&self.row_geometry, &*self.column_geometry, body_vp_size);

        // Also get the row window for frozen columns (same rows, columns 0..frozen).
        let frozen_vp = GridViewport::new()
            .with_row_anchor(row_anchor)
            .with_column_anchor_index(0)
            .with_row_overscan(self.row_overscan)
            .with_column_overscan(0);

        let frozen_vp_size = Size::new(frozen_width, viewport_h);
        let frozen_window = if frozen > 0 {
            Some(frozen_vp.realize(&self.row_geometry, &*self.column_geometry, frozen_vp_size))
        } else {
            None
        };

        // Collect realized body cells.
        //
        // Under a declared fold the COLUMN axis realises in full for the
        // realised rows — an unconserved value was never realised. The row
        // axis is untouched, so the headline property survives.
        let body_cells: Vec<PlacedCell> = if fold.is_some() {
            let column_count = self.column_geometry.len();
            body_window
                .rows
                .clone()
                .flat_map(|row| {
                    (0..column_count).map(move |column| PlacedCell {
                        row,
                        column,
                        bounds: iced::Rectangle::new(iced::Point::ORIGIN, Size::ZERO),
                    })
                })
                .collect()
        } else {
            body_vp
                .place(&self.row_geometry, &*self.column_geometry, body_vp_size)
                .filter(|p| p.column >= frozen) // exclude frozen columns from scrollable body
                .collect()
        };

        // Collect realized frozen cells.
        let frozen_cells: Vec<PlacedCell> = if let Some(ref _fw) = frozen_window {
            frozen_vp
                .place(&self.row_geometry, &*self.column_geometry, frozen_vp_size)
                .filter(|p| p.column < frozen)
                .collect()
        } else {
            Vec::new()
        };

        // Total realized cell count (for instrumentation).
        let total_realized = body_cells.len() + frozen_cells.len();

        // Report realization info.
        let all_rows = {
            let body_rows = &body_window.rows;
            if let Some(ref fw) = frozen_window {
                let fr = &fw.rows;
                fr.start.min(body_rows.start)..fr.end.max(body_rows.end)
            } else {
                body_rows.clone()
            }
        };
        let all_cols = {
            if let Some(ref fw) = frozen_window {
                fw.columns.start.min(body_window.columns.start)
                    ..fw.columns.end.max(body_window.columns.end)
            } else {
                body_window.columns.clone()
            }
        };

        if let Some(sink) = self.realization_sink {
            *sink.borrow_mut() = Some(RealizationInfo {
                cell_count: total_realized,
                row_range: all_rows.clone(),
                column_range: all_cols.clone(),
            });
        }

        // The realised range includes leading overscan. Position its first row
        // at its model-space offset so those rows remain mounted but paint
        // above the viewport instead of stealing visible slots. The body
        // viewport clips that negative portion below.
        let realized_content_y = row_anchor
            .place(&self.row_geometry, all_rows.clone())
            .next()
            .map_or(0.0, |placement| placement.y_offset);

        // Write the active cell coordinate to the sink in MODEL space, so it
        // survives scrolling past the realisation bound. A window-relative
        // implementation writes `None` once the cell leaves the window.
        //
        // SABOTAGE_ACTIVE_WINDOW = false: when true, the cell reports only
        // inside the realised window (the defect).
        const SABOTAGE_ACTIVE_WINDOW: bool = false;
        if let Some(sink) = self.active_cell_sink {
            if SABOTAGE_ACTIVE_WINDOW {
                *sink.borrow_mut() = self.active_cell.filter(|c| {
                    all_rows.contains(&(c.row as usize)) && all_cols.contains(&c.column)
                });
            } else {
                *sink.borrow_mut() = self.active_cell;
            }
        }

        // Logical grid dimensions for semantic GridPosition.
        let logical_rows = self.row_geometry.len();
        let logical_cols = self.column_geometry.len();

        // The active cell in model space; only realized cells matching it
        // get the highlight.
        //
        // SABOTAGE_HIGHLIGHT_WINDOW = false: when true, window-relative
        // indices highlight the wrong cell after scrolling away and back.
        const SABOTAGE_HIGHLIGHT_WINDOW: bool = false;
        let active_cell = self.active_cell;

        // The active-cell highlight border width.
        const ACTIVE_CELL_BORDER_WIDTH: f32 = 2.0;

        /// Whether a placed cell matches the active cell in model space.
        fn is_active(p: &PlacedCell, active: Option<CellCoord>) -> bool {
            match active {
                Some(c) => {
                    if SABOTAGE_HIGHLIGHT_WINDOW {
                        // Defective: compare against realized-window index.
                        // This would highlight index 0 of the window rather
                        // than the model row. Scroll-away-and-back breaks.
                        false
                    } else {
                        p.row as u64 == c.row && p.column == c.column
                    }
                }
                None => false,
            }
        }

        // Helper: build a cell Element with GridCell semantics — a
        // `SemanticsContainer::new` group carrying `Role::GridCell` and
        // its `GridPosition`, so AT announces "row 900001 of 1000000, column
        // 3 of 50". Only REALIZED cells get nodes: bounded by the viewport.
        //
        // A group, not a terminal leaf: a leaf publishes the node without
        // traversing into the element, so the drawn text never reaches the
        // collector and — `GridCell` taking its name from content — every
        // positioned cell stays nameless. Descending names the cell after
        // its drawn string and claims (not duplicates) the loose label, so
        // the bounded-count properties hold; a textless cell stays nameless
        // as before.
        //
        // The active cell's node carries `.focused(true)` (observable
        // highlight, not just the test sink). Under a declared fold the same
        // single `provider.cell` call feeds the projection — no doubled
        // traffic, `None` text when nothing folds. Row selection resolves
        // once per model row for both paint and semantics.
        let row_selection = self.row_selection.take();
        let selection_enabled = row_selection.is_some();
        let is_row_selected = |row: u64| row_selection.as_ref().is_some_and(|f| f(row));

        let fold_value = fold.as_ref().map(|f| &f.value);
        let build_cell = |p: &PlacedCell| -> Option<(Element<'a, Message>, Option<String>)> {
            let cell_val = self.provider.cell(p.row as u64, p.column)?;
            let folded_text = fold_value.map(|v| v(p.row as u64, p.column, &cell_val));
            let el = (self.cell_builder)(p.row as u64, p.column, &cell_val);
            let el = if self.on_column_resize.is_some() {
                MeasureCell::new(el, p.column).into()
            } else {
                el
            };
            let mut sem = Semantics::new(Role::GridCell).grid_position(GridPosition {
                row: p.row,
                column: p.column,
                row_count: logical_rows,
                column_count: logical_cols,
            });
            if is_active(p, active_cell) {
                sem = sem.focused(true);
            }
            Some((SemanticsContainer::new(el, sem).into(), folded_text))
        };

        // count highlighted cells for test instrumentation.
        let mut highlight_count: usize = 0;

        // Build body rows grouped by row index.
        let body_rows = group_by_row(&body_cells);
        let frozen_rows = group_by_row(&frozen_cells);

        // Get row heights from geometry.
        let row_h = self.row_geometry.item_height();

        // Wrap one assembled row in its state surfaces — applied to the ROW
        // element, never per cell: only the row knows its horizontal extent,
        // and per-cell surfaces light exactly one cell (the whole defect).
        // The tint resolves from the runtime theme every draw from the
        // single `Palette::overlay_hover` source the rest of the toolkit
        // reads, so a custom palette moves the table too.
        let row_hover_on = self.row_hover;
        let row_activation = self.row_activation.as_ref();
        let row_radius = self.row_radius;
        let wrap_row = move |row_model: u64,
                             row_el: Element<'a, Message>,
                             selected: bool|
              -> Element<'a, Message> {
            if let Some(activate) = row_activation {
                let (label, message) = activate(row_model);
                let mut surface = crate::pressable::enz_pressable(row_el, message)
                    .style(crate::pressable::PressableStyle::Row)
                    .embedded(true)
                    .aria_label(label)
                    .selected(selection_enabled && selected)
                    .radius(row_radius)
                    .width(Length::Fill)
                    .content_align_x(iced::Alignment::Start)
                    .min_height(row_h)
                    .min_height_center();
                if !row_hover_on {
                    surface =
                        surface.surface_colors_resolved(|_| crate::pressable::SurfaceColors {
                            hover_background: Some(Color::TRANSPARENT),
                            ..Default::default()
                        });
                }
                return surface.into_element();
            }
            // The selected fill belongs to the assembled row, never its cells.
            // `Length::Fill` includes transparent cell padding/gaps and any
            // trailing row space in the one continuous background band.
            let row_el: Element<'a, Message> = if selection_enabled && selected {
                container(row_el)
                    .width(Length::Fill)
                    .style(move |theme: &iced::Theme| container::Style {
                        background: Some(Background::Color(
                            crate::theme_lookup::palette(theme).surface_selected,
                        )),
                        border: Border {
                            radius: row_radius.into(),
                            ..Border::default()
                        },
                        ..Default::default()
                    })
                    .into()
            } else {
                // Full width like a selected row, so the hover band and the
                // rule under it span the table as the header's rule does.
                container(row_el).width(Length::Fill).into()
            };
            if !row_hover_on {
                return row_el;
            }
            // Hover is outside selection so its translucent overlay paints on
            // top. The selected fill remains the base state rather than being
            // replaced by the hover colour.
            RowHover::new(
                row_el,
                Box::new(|theme: &iced::Theme| crate::theme_lookup::palette(theme).overlay_hover()),
                row_radius,
            )
            .rule(true)
            .into()
        };

        // the candidate set and the declared yield order. Both are
        // properties of the table, not of a row, so they are resolved once and
        // shared by every row and by the header — a per-row resolution could
        // disagree between header and body and misalign the tracks.
        let fold_candidates: Vec<FoldCandidate> = (0..logical_cols)
            .map(|column| FoldCandidate {
                column,
                width: self.column_geometry.height_of(column),
            })
            .collect();
        let fold_order: Vec<usize> = match fold.as_ref() {
            Some(f) => priority_order(&f.priority, &fold_candidates, |key| {
                self.provider.column_index_of(key)
            }),
            None => Vec::new(),
        };
        // The body sits inside the scrollbar chrome, which lays it out one
        // gutter narrower. The header does not. Both must plan from the
        // same number or the header would keep a column the body folded.
        let header_width_adjust = if scrollbar_on { SCROLLBAR_GUTTER } else { 0.0 };

        // Build the body grid. Each row is wrapped in Role::GridRow.
        let mut body_row_elems: Vec<Element<'a, Message>> = Vec::new();
        for (row_idx, cells) in &body_rows {
            let mut cell_elems: Vec<Element<'a, Message>> = Vec::new();
            let mut fold_slots: Vec<(FoldCandidate, Element<'a, Message>)> = Vec::new();
            let mut fold_entries: Vec<(usize, Element<'a, Message>)> = Vec::new();
            let row_model = *row_idx as u64;
            // the entry id is derived from the ROW KEY and the COLUMN
            // KEY — the field — never from the sampled value. A table that
            // refreshes every second would otherwise throw keyboard focus away
            // whenever a number changed.
            let row_key = self
                .provider
                .key(row_model)
                .map_or_else(|| format!("row-{row_model}"), |k| k.as_str().to_owned());
            for p in cells {
                let w = self.column_geometry.height_of(p.column);
                if let Some((el, folded_text)) = build_cell(p) {
                    // highlight the active cell with a border.
                    let active = is_active(p, active_cell);
                    if active {
                        highlight_count += 1;
                    }
                    let cell_container = if active {
                        container(el)
                            .width(Length::Fixed(w))
                            .height(Length::Fixed(row_h))
                            .style(|theme: &iced::Theme| container::Style {
                                border: Border {
                                    color: crate::theme_lookup::palette(theme).focus_ring,
                                    width: ACTIVE_CELL_BORDER_WIDTH,
                                    radius: 0.0.into(),
                                },
                                ..Default::default()
                            })
                    } else {
                        container(el)
                            .width(Length::Fixed(w))
                            .height(Length::Fixed(row_h))
                    };
                    let Some(f) = fold.as_ref() else {
                        cell_elems.push(cell_container.into());
                        continue;
                    };
                    let Some(key) = self.provider.column_key(p.column) else {
                        continue;
                    };
                    let value = folded_text.unwrap_or_default();
                    fold_slots.push((
                        FoldCandidate {
                            column: p.column,
                            width: w,
                        },
                        cell_container.into(),
                    ));
                    fold_entries.push((
                        p.column,
                        build_fold_entry(f, &row_key, row_model, &key, &value),
                    ));
                }
            }
            let selected = is_row_selected(row_model);
            let row_el: Element<'a, Message> = match fold.as_ref() {
                Some(f) => wrap_row(
                    row_model,
                    FoldRow::new(
                        fold_slots,
                        fold_entries,
                        fold_order.clone(),
                        row_h,
                        f.entry_height,
                        0.0,
                    )
                    .into(),
                    selected,
                ),
                None => wrap_row(row_model, Row::with_children(cell_elems).into(), selected),
            };
            let mut row_semantics = Semantics::new(Role::GridRow);
            if selection_enabled {
                row_semantics = row_semantics.selected(selected);
            }
            body_row_elems.push(SemanticsContainer::new(row_el, row_semantics).into());
        }
        let body_col: Element<'a, Message> = Column::with_children(body_row_elems).into();

        // Build the frozen grid. Same semantic treatment: GridRow + GridCell.
        let frozen_col: Option<Element<'a, Message>> = if frozen > 0 {
            let mut frozen_row_elems: Vec<Element<'a, Message>> = Vec::new();
            for (row_idx, cells) in &frozen_rows {
                let mut cell_elems: Vec<Element<'a, Message>> = Vec::new();
                for p in cells {
                    let w = self.column_geometry.height_of(p.column);
                    if let Some((el, _)) = build_cell(p) {
                        let active = is_active(p, active_cell);
                        if active {
                            highlight_count += 1;
                        }
                        let cell_container = if active {
                            container(el)
                                .width(Length::Fixed(w))
                                .height(Length::Fixed(row_h))
                                .style(|theme: &iced::Theme| container::Style {
                                    border: Border {
                                        color: crate::theme_lookup::palette(theme).focus_ring,
                                        width: ACTIVE_CELL_BORDER_WIDTH,
                                        radius: 0.0.into(),
                                    },
                                    ..Default::default()
                                })
                        } else {
                            container(el)
                                .width(Length::Fixed(w))
                                .height(Length::Fixed(row_h))
                        };
                        cell_elems.push(cell_container.into());
                    }
                }
                let selected = is_row_selected(*row_idx as u64);
                let row_el: Element<'a, Message> = wrap_row(
                    *row_idx as u64,
                    Row::with_children(cell_elems).into(),
                    selected,
                );
                let mut row_semantics = Semantics::new(Role::GridRow);
                if selection_enabled {
                    row_semantics = row_semantics.selected(selected);
                }
                frozen_row_elems.push(SemanticsContainer::new(row_el, row_semantics).into());
            }
            Some(
                container(Column::with_children(frozen_row_elems))
                    .width(Length::Fixed(frozen_width))
                    .into(),
            )
        } else {
            None
        };

        // write the highlight count to the sink.
        if let Some(sink) = self.highlight_count_sink {
            sink.set(highlight_count);
        }

        // Column sort. `on_column_sort` was a dead builder — stored and read
        // nowhere, so headers published a pressless `Role::ColumnHeader` and
        // activating one sorted nothing. Here a header press (pointer) and
        // Ctrl+Enter on the active column (keyboard) both invoke the callback
        // with a `SortCommand`, and the header announces `aria-sort`.
        //
        // A sortable header wraps in a `mouse_area`, not a `focus_button`:
        // the table always keeps one `GridCell` `.focused(true)`, and a
        // focusable header would put two focused nodes in one tree, tripping
        // the two-focus `debug_assert!`. The area publishes no focus node, so
        // both coexist.
        let on_column_sort = self.on_column_sort;
        let sort_state = self.sort_state.clone();
        // The a11y sort direction to publish on a header for column `key`:
        // the sorted column announces its live direction, every other sortable
        // column announces `Unsorted`, and when the table is not sortable at
        // all (no `on_column_sort`) the property stays absent.
        let header_sort_dir = |key: &ColumnKey| -> Option<semantics::SortDirection> {
            // Not sortable at all -> property absent.
            on_column_sort.as_ref()?;
            match &sort_state {
                Some((sorted_key, dir)) if sorted_key == key => Some(match dir {
                    SortDirection::Ascending => semantics::SortDirection::Ascending,
                    SortDirection::Descending => semantics::SortDirection::Descending,
                    SortDirection::None => semantics::SortDirection::Unsorted,
                }),
                _ => Some(semantics::SortDirection::Unsorted),
            }
        };
        // The direction a press on column `key` should request: toggle the
        // current direction if `key` is already the sorted column, else start
        // ascending. This is what makes repeated activation flip Asc<->Desc.
        let header_next_dir = |key: &ColumnKey| -> SortDirection {
            match &sort_state {
                Some((sorted_key, dir)) if sorted_key == key => dir.toggled(),
                _ => SortDirection::Ascending,
            }
        };

        // Build the header row on shared column tracks; each header cell
        // gets `Role::ColumnHeader`, named by the builder's content through a
        // leaf (exactly one node per header — no double-announce).
        //
        // SABOTAGE_DROP_SORT_INVOKE = false: when true, a header press
        // publishes no sort command (the default dead builder).
        const SABOTAGE_DROP_SORT_INVOKE: bool = false;
        let header_row: Option<Element<'a, Message>> = if let Some(ref hb) = self.header_builder {
            let col_count = self.column_geometry.len();
            let header_height = self.header_height;
            // Build one header cell: the label from the caller's builder, a
            // `Role::ColumnHeader` carrying the label and (when sortable) its
            // `aria-sort` state, wrapped in a `mouse_area` that publishes the
            // toggled `SortCommand` on press.
            let build_header_cell = |c: usize, key: &ColumnKey| -> Element<'a, Message> {
                let w = self.column_geometry.height_of(c);
                let header_el = hb(c, key);
                let header_el = if self.on_column_resize.is_some() {
                    MeasureCell::new(header_el, c).into()
                } else {
                    header_el
                };
                // Named by the title the builder draws, not by the column key:
                // the collector gives an unnamed header its first text.
                let mut sem = Semantics::new(Role::ColumnHeader);
                if let Some(dir) = header_sort_dir(key) {
                    sem = sem.sort(dir);
                }
                let inner: Element<'a, Message> =
                    container(SemanticsContainer::new(header_el, sem))
                        .width(Length::Fixed(w))
                        .height(Length::Fixed(header_height))
                        .into();
                let inner = match self.on_column_resize.as_ref() {
                    Some(on_resize) => ResizeHandle::new(
                        inner,
                        c,
                        key.clone(),
                        w,
                        self.min_column_width,
                        Rc::clone(on_resize),
                    )
                    .into(),
                    None => inner,
                };
                // Only wrap in a pressable when the caller actually registered a
                // sort callback; a non-sortable table's headers stay inert.
                match on_column_sort.as_ref() {
                    Some(cb) if !SABOTAGE_DROP_SORT_INVOKE => {
                        let msg = cb(SortCommand {
                            column: key.clone(),
                            direction: header_next_dir(key),
                        });
                        mouse_area(inner).on_press(msg).into()
                    }
                    _ => inner,
                }
            };

            // Frozen header columns.
            let mut frozen_header_cells: Vec<Element<'a, Message>> = Vec::new();
            for c in 0..frozen.min(col_count) {
                if let Some(key) = self.provider.column_key(c) {
                    frozen_header_cells.push(build_header_cell(c, &key));
                }
            }

            // Scrollable header columns: only those in the body window.
            //
            // Under a declared fold every column is a candidate — header and
            // body run the identical plan over identical candidates, so a
            // folded column loses its header and its cells together.
            let mut scroll_header_cells: Vec<Element<'a, Message>> = Vec::new();
            let mut header_fold_slots: Vec<(FoldCandidate, Element<'a, Message>)> = Vec::new();
            let header_columns: Vec<usize> = if fold.is_some() {
                (0..col_count).collect()
            } else {
                body_window.columns.collect()
            };
            for c in header_columns {
                if c < frozen {
                    continue;
                }
                if let Some(key) = self.provider.column_key(c) {
                    let cell = build_header_cell(c, &key);
                    if fold.is_some() {
                        header_fold_slots.push((
                            FoldCandidate {
                                column: c,
                                width: self.column_geometry.height_of(c),
                            },
                            cell,
                        ));
                    } else {
                        scroll_header_cells.push(cell);
                    }
                }
            }

            let mut header_parts: Vec<Element<'a, Message>> = Vec::new();
            if !frozen_header_cells.is_empty() {
                header_parts.push(
                    container(Row::with_children(frozen_header_cells))
                        .width(Length::Fixed(frozen_width))
                        .into(),
                );
            }
            if fold.is_some() {
                header_parts.push(
                    FoldRow::new(
                        header_fold_slots,
                        Vec::new(),
                        fold_order,
                        header_height,
                        0.0,
                        header_width_adjust,
                    )
                    .into(),
                );
            } else {
                header_parts.push(Row::with_children(scroll_header_cells).into());
            }

            // Wrap the header row itself in a GridRow.
            Some(
                SemanticsContainer::new(
                    container(Row::with_children(header_parts))
                        .height(Length::Fixed(self.header_height)),
                    Semantics::new(Role::GridRow),
                )
                .into(),
            )
        } else {
            None
        };

        // Compose the body: frozen | scrollable, side by side.
        let body_content: Element<'a, Message> = if let Some(frozen_el) = frozen_col {
            row![frozen_el, body_col].into()
        } else {
            body_col
        };

        // Wrap the body in the scrollbar chrome: trailing gutter (last
        // column never clipped), track+thumb from the caller-owned offset
        // when overflowing, thumb drag back to an absolute offset. No-op when
        // content fits or `.scrollbar(false)`. BODY only, below the header —
        // the thumb travels the row band, not the header row.
        let body_element: Element<'a, Message> = {
            // The row viewport owns only the effective whole-row height. A
            // Fill spacer retains the allocation's fractional remainder below
            // it, so parent geometry and the scrollbar track keep the full
            // allocated body while no row glyph can enter that quiet band.
            let row_viewport: Element<'a, Message> =
                TableBodyViewport::new(body_content, viewport_h, realized_content_y).into();
            let body_container: Element<'a, Message> = container(Column::with_children(vec![
                row_viewport,
                Space::new().height(Length::Fill).into(),
            ]))
            .width(self.width)
            .height(Length::Fill)
            .into();
            if scrollbar_on {
                TableScrollbar::new(
                    body_container,
                    scrollbar_content_height,
                    scrollbar_viewport_h,
                    scrollbar_scroll_y,
                    Box::new(scrollbar::default_thumb_color),
                    on_scroll_to.map(|callback| {
                        Box::new(move |offset| callback(offset)) as Box<dyn Fn(f32) -> Message + 'a>
                    }),
                )
                .into()
            } else {
                body_container
            }
        };

        // Stack header (if any) above body.
        let mut layout_parts: Vec<Element<'a, Message>> = Vec::new();
        if let Some(hdr) = header_row {
            layout_parts.push(hdr);
            // A hairline under the header, as data tables everywhere draw it.
            layout_parts.push(
                container(iced::widget::Space::new())
                    .width(Length::Fill)
                    .height(Length::Fixed(1.0))
                    .style(|theme: &iced::Theme| container::Style {
                        background: Some(Background::Color(
                            crate::theme_lookup::palette(theme).border_strong,
                        )),
                        ..Default::default()
                    })
                    .into(),
            );
        }
        if self.header_builder.is_some() && self.header_gap > 0.0 {
            layout_parts.push(iced::widget::Space::new().height(self.header_gap).into());
        }
        layout_parts.push(body_element);

        // Wrap the entire table in Role::Grid so AT recognises it as a
        // two-dimensional data structure. The row/column counts are the
        // LOGICAL totals, but the published child nodes are only those
        // for the realized viewport — bounded by design.
        let grid_semantics = Semantics::new(Role::Grid).label(enzui_core::localized!("enzui-table-data-grid-rows-columns", "rows" => logical_rows, "columns" => logical_cols));

        let grid_element: Element<'a, Message> = SemanticsContainer::new(
            container(Column::with_children(layout_parts))
                .width(self.width)
                .height(self.height),
            grid_semantics,
        )
        .into();

        // Wrap the grid in a keyboard-event handler (arrows, Enter/F2,
        // Escape, column sort), intercepting after the child has seen events
        // — the `keyboard_capture_unscoped` pattern.
        //
        // SABOTAGE_SWALLOW_ARROW = false: when true, arrow keys are ignored.
        const SABOTAGE_SWALLOW_ARROW: bool = false;
        // SABOTAGE_CLAMP_WRONG = false: when true, active cell can leave
        // the model bounds (no clamping at edges).
        const SABOTAGE_CLAMP_WRONG: bool = false;
        // SABOTAGE_COMMIT_ON_ESCAPE = false: when true, Escape commits
        // instead of cancelling.
        const SABOTAGE_COMMIT_ON_ESCAPE: bool = false;

        let on_active_cell_change = self.on_active_cell_change;
        let on_edit_start = self.on_edit_start;
        let on_edit_commit = self.on_edit_commit;
        let on_edit_cancel = self.on_edit_cancel;
        let editing = self.editing.cloned();
        // Scroll input: moved out of `self` here so the keyboard closure below
        // can consume the rest of `self` while we still hold the callback.
        let on_scroll = self.on_scroll;
        let wheel_scroll_x = self.scroll_x;
        let wheel_scroll_y = self.scroll_y;
        let wheel_max_x = (self.column_geometry.total_height() - viewport_w).max(0.0);
        let wheel_max_y = (self.row_geometry.total_height() - viewport_h).max(0.0);

        // Keyboard sort: headers are not focusable here (that would steal
        // focus from the always-focused active cell), so the keyboard route
        // sorts the active cell's column via `Ctrl+Enter` (plain Enter runs
        // the edit lifecycle). The column key resolves now so the closure
        // need not borrow `self.provider`.
        //
        // SABOTAGE_DROP_KB_SORT = false: when true, the Ctrl+Enter branch is
        // dropped and the keyboard sort route is dead.
        const SABOTAGE_DROP_KB_SORT: bool = false;
        let kb_active_col_key: Option<ColumnKey> =
            active_cell.and_then(|ac| self.provider.column_key(ac.column));
        // The keyboard resize step: the active column, its key and its width.
        let on_column_resize = self.on_column_resize.clone();
        let min_column_width = self.min_column_width;
        let kb_resize_target: Option<(usize, ColumnKey, f32)> = active_cell
            .zip(kb_active_col_key.clone())
            .map(|(ac, key)| (ac.column, key, self.column_geometry.height_of(ac.column)));
        // The direction a keyboard sort of the active column requests: toggle
        // when that column is already the sorted one, else ascending. Computed
        // now while `sort_state` is still owned here (it moves into the header
        // Elements' captured messages, but `sort_state` itself is still live).
        let kb_sort_command: Option<SortCommand> = kb_active_col_key.map(|key| {
            let direction = match &sort_state {
                Some((sorted_key, dir)) if *sorted_key == key => dir.toggled(),
                _ => SortDirection::Ascending,
            };
            SortCommand {
                column: key,
                direction,
            }
        });

        // Wrap the grid in a keyboard-event handler (`keyboard_capture_unscoped`,
        // as the date picker does) for Arrow/Enter/F2/Escape.
        //
        // `on_edit_start` gets `(CellCoord, String)` with an EMPTY string: `Cell`
        // is generic so `to_string()` is unavailable here. The coord is the
        // durable identity; the caller looks the value up in their own model.
        let keyboard_captured = crate::keyboard_capture::keyboard_capture_unscoped(
            grid_element,
            move |key, modifiers| {
                // Resolve the press through the one production vocabulary.
                // `None` means the application — or an active child editor,
                // which already saw the key before this unscoped wrapper — owns
                // it, so the fallback must leave it untouched. The match below
                // is exhaustive over `TableKeyAction`, so no command can be
                // silently dropped.
                let action = table_key_action(key, modifiers)?;

                match action {
                    TableKeyAction::NavigateDown => {
                        if SABOTAGE_SWALLOW_ARROW {
                            return None;
                        }
                        let ac = active_cell?;
                        let new_row = if SABOTAGE_CLAMP_WRONG {
                            ac.row.wrapping_add(1)
                        } else {
                            // Clamp at last row.
                            let max_row = (logical_rows as u64).saturating_sub(1);
                            if ac.row >= max_row {
                                return None;
                            }
                            ac.row + 1
                        };
                        let new = CellCoord::new(new_row, ac.column);
                        on_active_cell_change.as_ref().map(|f| f(new))
                    }
                    TableKeyAction::NavigateUp => {
                        if SABOTAGE_SWALLOW_ARROW {
                            return None;
                        }
                        let ac = active_cell?;
                        if ac.row == 0 {
                            return None;
                        }
                        let new = CellCoord::new(ac.row - 1, ac.column);
                        on_active_cell_change.as_ref().map(|f| f(new))
                    }
                    TableKeyAction::NavigateRight => {
                        if SABOTAGE_SWALLOW_ARROW {
                            return None;
                        }
                        let ac = active_cell?;
                        let max_col = logical_cols.saturating_sub(1);
                        if ac.column >= max_col {
                            return None;
                        }
                        let new = CellCoord::new(ac.row, ac.column + 1);
                        on_active_cell_change.as_ref().map(|f| f(new))
                    }
                    TableKeyAction::NavigateLeft => {
                        if SABOTAGE_SWALLOW_ARROW {
                            return None;
                        }
                        let ac = active_cell?;
                        if ac.column == 0 {
                            return None;
                        }
                        let new = CellCoord::new(ac.row, ac.column - 1);
                        on_active_cell_change.as_ref().map(|f| f(new))
                    }
                    // Ctrl+Enter (resolved to `Sort` by `table_key_action`)
                    // sorts the active cell's column through `on_column_sort`.
                    // This is the keyboard route to sorting, distinct from a
                    // pointer press, so the always-focused active cell keeps
                    // focus rather than a header stealing it.
                    TableKeyAction::Sort => {
                        if SABOTAGE_DROP_KB_SORT {
                            return None;
                        }
                        if let (Some(cb), Some(cmd)) =
                            (on_column_sort.as_ref(), kb_sort_command.as_ref())
                        {
                            return Some(cb(cmd.clone()));
                        }
                        None
                    }
                    TableKeyAction::NarrowColumn | TableKeyAction::WidenColumn => {
                        let on_resize = on_column_resize.as_ref()?;
                        let (column, key, width) = kb_resize_target.clone()?;
                        let step = if action == TableKeyAction::WidenColumn {
                            COLUMN_RESIZE_STEP
                        } else {
                            -COLUMN_RESIZE_STEP
                        };
                        let resized = (width + step).max(min_column_width);
                        (resized != width).then(|| {
                            on_resize(ColumnResize {
                                column,
                                key,
                                width: resized,
                                finished: true,
                            })
                        })
                    }
                    // Enter: begin edit (if not editing) or commit (if editing).
                    TableKeyAction::EditEnterOrCommit => {
                        if let Some(ref edit) = editing {
                            // Commit the edit.
                            on_edit_commit
                                .as_ref()
                                .map(|f| f(edit.coord, edit.original_value.clone()))
                        } else {
                            // Start editing the active cell.
                            let ac = active_cell?;
                            on_edit_start.as_ref().map(|f| f(ac, String::new()))
                        }
                    }
                    // F2: begin edit.
                    TableKeyAction::EditStart => {
                        if editing.is_some() {
                            return None;
                        }
                        let ac = active_cell?;
                        on_edit_start.as_ref().map(|f| f(ac, String::new()))
                    }
                    // Escape: cancel edit.
                    TableKeyAction::EditCancel => {
                        let edit = editing.as_ref()?;
                        if SABOTAGE_COMMIT_ON_ESCAPE {
                            on_edit_commit
                                .as_ref()
                                .map(|f| f(edit.coord, edit.original_value.clone()))
                        } else {
                            on_edit_cancel
                                .as_ref()
                                .map(|f| f(edit.coord, edit.original_value.clone()))
                        }
                    }
                }
            },
        );

        // Scroll input. The caller owns the offset, so this wrapper mirrors
        // iced Scrollable's movement gate: publish and capture only when the
        // requested movement changes a clamped axis. Otherwise the nearest
        // scrollable ancestor gets the same event.
        // A double-clicked header edge is fitted after the event reached it.
        let keyboard_captured = match self.on_column_resize {
            Some(on_resize) => {
                FitRunner::new(keyboard_captured, min_column_width, on_resize).into()
            }
            None => keyboard_captured,
        };
        match on_scroll {
            Some(cb) => wheel_capture(keyboard_captured, move |delta| {
                let (dx, dy) = match delta {
                    mouse::ScrollDelta::Lines { x, y } => (x * WHEEL_LINE_PX, y * WHEEL_LINE_PX),
                    mouse::ScrollDelta::Pixels { x, y } => (x, y),
                };
                (offset_can_move(wheel_scroll_x, wheel_max_x, dx)
                    || offset_can_move(wheel_scroll_y, wheel_max_y, dy))
                .then(|| cb(dx, dy))
            }),
            None => keyboard_captured,
        }
    }
}

impl<'a, Message, Cell> From<EnzTable<'a, Message, Cell>> for Element<'a, Message>
where
    Message: Clone + 'a,
    Cell: 'a,
{
    fn from(table: EnzTable<'a, Message, Cell>) -> Self {
        table.into_element()
    }
}

/// Build one fold-destination entry: a named, focusable, activatable
/// projection of a folded cell.
///
/// Four properties, in one place so no consumer coordinates them: the value
/// is in the accessible name (`"<column>: <value>"`); the entry stays
/// keyboard-focusable with or without an activation; activation publishes
/// [`FoldedField`] through [`ColumnFold::on_activate`]; and the widget id
/// derives from the field — `(row key, column key)` — not the sampled value,
/// so a changing value never moves focus.
pub(super) fn build_fold_entry<'a, Message, Cell>(
    fold: &ColumnFold<'a, Message, Cell>,
    row_key: &str,
    row: u64,
    column: &ColumnKey,
    value: &str,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    let label = fold_entry_label(column, value);
    let message = fold.on_activate.as_ref().map(|cb| {
        cb(FoldedField {
            row,
            column: column.clone(),
            value: value.to_owned(),
        })
    });
    crate::focus_button::focus_button_maybe(
        iced::widget::text(label.clone()).size(enzui_theme::text_scale::ui_px(11.0)),
        message,
    )
    .id(enzui_core::identity::derive_id(
        &format!("{}-{}", fold.id_prefix, row_key),
        column.as_str(),
    ))
    .width(Length::Fill)
    .height(Length::Fixed(fold.entry_height))
    .content_text(label)
    .semantics(fold_entry_semantics(column, value))
    .focusable_without_activation(true)
    .into()
}

/// Group placed cells by row index, preserving row-major order.
fn group_by_row(cells: &[PlacedCell]) -> Vec<(usize, Vec<&PlacedCell>)> {
    let mut groups: Vec<(usize, Vec<&PlacedCell>)> = Vec::new();
    for cell in cells {
        if let Some(last) = groups.last_mut().filter(|g| g.0 == cell.row) {
            last.1.push(cell);
            continue;
        }
        groups.push((cell.row, vec![cell]));
    }
    groups
}
