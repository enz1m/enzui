//! Stable row and column identity, query generations, providers, and keyed selection for tables.
//!
//! Rows use [`crate::virtual_sequence::SequenceKey`]; columns use [`ColumnKey`]. [`TableSession`] rejects responses from superseded query generations.

use core::ops::Range;

use crate::virtual_sequence::{KeyedProvider, SequenceKey};

/// Stable logical identity for a table column across insertion, removal, and reorder.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ColumnKey(String);

impl ColumnKey {
    /// Creates a key from a caller-supplied logical column identifier.
    #[must_use]
    pub fn new(logical: impl Into<String>) -> Self {
        ColumnKey(logical.into())
    }

    /// The logical key as a string slice, for id derivation
    /// ([`crate::identity::derive_id`]) or display.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for ColumnKey {
    fn from(s: &str) -> Self {
        ColumnKey(s.to_owned())
    }
}

impl From<String> for ColumnKey {
    fn from(s: String) -> Self {
        ColumnKey(s)
    }
}

/// Sort state for one column; [`Self::None`] means sortable but currently unsorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortDirection {
    /// Sortable, currently unsorted.
    #[default]
    None,
    /// Ascending.
    Ascending,
    /// Descending.
    Descending,
}

impl SortDirection {
    /// The direction that follows this one when a header is activated:
    /// `None → Ascending → Descending → Ascending`.
    ///
    /// Once a column is sorted, toggling stays between the two real directions
    /// rather than cycling back through [`None`](SortDirection::None) — clearing
    /// a sort is a separate, explicit act, not a third of every toggle.
    #[must_use]
    pub fn toggled(self) -> SortDirection {
        match self {
            SortDirection::None | SortDirection::Descending => SortDirection::Ascending,
            SortDirection::Ascending => SortDirection::Descending,
        }
    }
}

/// Query-generation identity within one [`TableSession`].
///
/// Fresh sessions begin at [`Self::FIRST`], and [`Self::next`] returns the wrapping successor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Generation(u64);

impl Generation {
    /// The first generation, carried by a fresh [`TableSession`] before any
    /// invalidation.
    pub const FIRST: Generation = Generation(0);

    /// Returns the wrapping successor generation.
    #[must_use]
    pub fn next(self) -> Generation {
        Generation(self.0.wrapping_add(1))
    }

    /// The raw counter, for diagnostics and stable-id derivation only. Never
    /// compare tables' generations across sessions — a generation is meaningful
    /// only within the session that mints it.
    #[must_use]
    pub fn raw(self) -> u64 {
        self.0
    }
}

impl Default for Generation {
    fn default() -> Self {
        Generation::FIRST
    }
}

/// A payload stamped with the generation under which it was issued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryToken<T> {
    generation: Generation,
    payload: T,
}

impl<T> QueryToken<T> {
    /// Stamp `payload` with `generation`. Normally minted by
    /// [`TableSession::issue`]; public so an async worker can reconstruct one
    /// from a generation it carried across the task boundary.
    #[must_use]
    pub fn new(generation: Generation, payload: T) -> Self {
        QueryToken {
            generation,
            payload,
        }
    }

    /// The generation this token was stamped with.
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// Borrows the payload without validating freshness.
    ///
    /// Use [`TableSession::validate`] before using it as current table content.
    #[must_use]
    pub fn peek(&self) -> &T {
        &self.payload
    }

    /// Consumes the token without validating freshness, returning its generation and payload.
    #[must_use]
    pub fn into_parts(self) -> (Generation, T) {
        (self.generation, self.payload)
    }
}

/// A generation mismatch returned when validating a stale token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stale {
    /// The generation the stale token was stamped with.
    pub token: Generation,
    /// The session's current generation, which the token failed to match.
    pub current: Generation,
}

impl core::fmt::Display for Stale {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "stale table read: token generation {} != current generation {}",
            self.token.0, self.current.0
        )
    }
}

impl std::error::Error for Stale {}

/// Provides keyed rows and columns with on-demand cells.
///
/// Row identity comes from [`KeyedProvider`]. Column and cell access are total and return `None` out of range. The trait is object-safe.
pub trait TableProvider<Cell, RowSummary = SequenceKey>: KeyedProvider<RowSummary> {
    /// Returns the logical column count, which may be zero.
    fn column_count(&self) -> usize;

    /// Returns the stable key at `column`, or `None` when out of range.
    fn column_key(&self, column: usize) -> Option<ColumnKey>;

    /// Returns the current index of `key`, or `None` if absent.
    ///
    /// The default scans columns linearly and returns the first match; providers may override it.
    fn column_index_of(&self, key: &ColumnKey) -> Option<usize> {
        let count = self.column_count();
        let mut column = 0usize;
        while column < count {
            if self.column_key(column).as_ref() == Some(key) {
                return Some(column);
            }
            column += 1;
        }
        None
    }

    /// Realizes the cell at `(row, column)`, returning `None` when either coordinate is out of range.
    fn cell(&self, row: u64, column: usize) -> Option<Cell>;
}

/// Conveniences every [`TableProvider`] gets for free and none can override.
///
/// Blanket-implemented, so these are derived facts rather than overridable
/// policy — the same pattern [`crate::virtual_sequence::ItemProviderExt`] uses
/// so `is_empty` cannot be shipped disagreeing with `len`.
pub trait TableProviderExt<Cell, RowSummary = SequenceKey>:
    TableProvider<Cell, RowSummary>
{
    /// Whether the table has no columns. Always exactly `column_count() == 0`.
    fn has_no_columns(&self) -> bool {
        self.column_count() == 0
    }

    /// Returns whether either table axis is empty, so no cells can be realized.
    fn realises_nothing(&self) -> bool {
        self.len() == 0 || self.column_count() == 0
    }

    /// Realizes one row across `columns` in order, truncating at the column count and skipping empty or reversed ranges.
    fn row_cells(&self, row: u64, columns: Range<usize>) -> Vec<Cell> {
        let count = self.column_count();
        let start = columns.start.min(count);
        let end = columns.end.clamp(start, count);
        let mut out = Vec::with_capacity(end - start);
        for column in start..end {
            if let Some(cell) = self.cell(row, column) {
                out.push(cell);
            }
        }
        out
    }
}

impl<Cell, RowSummary, P> TableProviderExt<Cell, RowSummary> for P where
    P: TableProvider<Cell, RowSummary> + ?Sized
{
}

/// Default row-summary type for a [`TableProvider`], using the row’s [`SequenceKey`].
pub type RowKeySummary = SequenceKey;

/// Durable application-owned sort and opaque filter state for a table.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableQuery {
    sort_column: Option<ColumnKey>,
    sort_direction: SortDirection,
    filter: Option<String>,
}

impl TableQuery {
    /// An unsorted, unfiltered query — the default view.
    #[must_use]
    pub fn new() -> Self {
        TableQuery::default()
    }

    /// The column this query is sorted by, or `None` if unsorted.
    #[must_use]
    pub fn sort_column(&self) -> Option<&ColumnKey> {
        self.sort_column.as_ref()
    }

    /// The current sort direction. Meaningful only when
    /// [`sort_column`](Self::sort_column) is `Some`; [`SortDirection::None`]
    /// otherwise.
    #[must_use]
    pub fn sort_direction(&self) -> SortDirection {
        self.sort_direction
    }

    /// The current opaque filter, or `None` if unfiltered.
    #[must_use]
    pub fn filter(&self) -> Option<&str> {
        self.filter.as_deref()
    }

    /// Sets the sort column and direction, consuming and returning `self`.
    #[must_use]
    pub fn sorted_by(mut self, column: ColumnKey, direction: SortDirection) -> Self {
        self.sort_column = Some(column);
        self.sort_direction = direction;
        self
    }

    /// Clear any sort — return to the unsorted view.
    #[must_use]
    pub fn unsorted(mut self) -> Self {
        self.sort_column = None;
        self.sort_direction = SortDirection::None;
        self
    }

    /// Set (or, with `None`, clear) the opaque filter.
    #[must_use]
    pub fn filtered(mut self, filter: Option<String>) -> Self {
        self.filter = filter;
        self
    }
}

/// Owns durable query state and validates generation-stamped payloads.
///
/// Realized cells, viewports, and caches remain outside the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSession {
    generation: Generation,
    query: TableQuery,
}

impl Default for TableSession {
    fn default() -> Self {
        TableSession::new()
    }
}

impl TableSession {
    /// A fresh session: [`Generation::FIRST`] and the default (unsorted,
    /// unfiltered) query.
    #[must_use]
    pub fn new() -> Self {
        TableSession {
            generation: Generation::FIRST,
            query: TableQuery::new(),
        }
    }

    /// A session starting from an existing durable query, at
    /// [`Generation::FIRST`].
    #[must_use]
    pub fn with_query(query: TableQuery) -> Self {
        TableSession {
            generation: Generation::FIRST,
            query,
        }
    }

    /// The current generation.
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// The current durable query.
    #[must_use]
    pub fn query(&self) -> &TableQuery {
        &self.query
    }

    /// Replaces the query and advances the generation, even when the new query compares equal.
    pub fn set_query(&mut self, query: TableQuery) -> Generation {
        self.query = query;
        self.generation = self.generation.next();
        self.generation
    }

    /// Advances the generation without changing the query, invalidating reads after structural insertion, deletion, or reorder.
    pub fn invalidate(&mut self) -> Generation {
        self.generation = self.generation.next();
        self.generation
    }

    /// Stamps `payload` with the session’s current generation.
    #[must_use]
    pub fn issue<T>(&self, payload: T) -> QueryToken<T> {
        QueryToken::new(self.generation, payload)
    }

    /// Whether `token` was issued under the session's current generation.
    ///
    /// The cheap predicate form of [`validate`](Self::validate), for a caller
    /// that wants to test freshness without consuming the token.
    #[must_use]
    pub fn is_current<T>(&self, token: &QueryToken<T>) -> bool {
        token.generation() == self.generation
    }

    /// Returns the payload when its generation exactly matches the session; otherwise returns [`Stale`] with the payload.
    ///
    /// # Errors
    ///
    /// The [`Stale`] marker with the value when the token's query was superseded.
    pub fn validate<T>(&self, token: QueryToken<T>) -> Result<T, (Stale, T)> {
        let (token_gen, payload) = token.into_parts();
        if token_gen == self.generation {
            Ok(payload)
        } else {
            Err((
                Stale {
                    token: token_gen,
                    current: self.generation,
                },
                payload,
            ))
        }
    }
}

/// Controls whether keyed row selection contains at most one key or multiple independent keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMode {
    /// At most one row selected. Selecting a second row replaces the first.
    #[default]
    Single,
    /// Any number of rows selected independently.
    Multi,
}

/// Stores selected rows by stable [`SequenceKey`].
///
/// Missing keys persist until [`Self::reconcile`] is called for deletion. Selection changes do not advance [`TableSession`] generations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableSelection {
    mode: SelectionMode,
    selected: std::collections::HashSet<SequenceKey>,
}

impl TableSelection {
    /// A new, empty single-select selection.
    #[must_use]
    pub fn new() -> Self {
        TableSelection::default()
    }

    /// A new, empty selection in `mode`.
    #[must_use]
    pub fn with_mode(mode: SelectionMode) -> Self {
        TableSelection {
            mode,
            selected: std::collections::HashSet::new(),
        }
    }

    /// The current selection mode.
    #[must_use]
    pub fn mode(&self) -> SelectionMode {
        self.mode
    }

    /// Returns the number of selected rows.
    #[must_use]
    pub fn len(&self) -> usize {
        self.selected.len()
    }

    /// Whether no row is selected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    /// Returns whether `key` is selected, independent of its current row position.
    #[must_use]
    pub fn contains(&self, key: &SequenceKey) -> bool {
        self.selected.contains(key)
    }

    /// Selects `key`, replacing the selection in single mode or adding it in multi mode. Returns whether state changed.
    pub fn select(&mut self, key: SequenceKey) -> bool {
        match self.mode {
            SelectionMode::Single => {
                if self.selected.len() == 1 && self.selected.contains(&key) {
                    return false;
                }
                self.selected.clear();
                self.selected.insert(key);
                true
            }
            SelectionMode::Multi => self.selected.insert(key),
        }
    }

    /// Deselects `key`, returning whether it was previously selected.
    pub fn deselect(&mut self, key: &SequenceKey) -> bool {
        self.selected.remove(key)
    }

    /// Toggles `key` and returns its resulting membership. In single mode, selecting it replaces any other key.
    pub fn toggle(&mut self, key: SequenceKey) -> bool {
        if self.selected.contains(&key) {
            self.selected.remove(&key);
            false
        } else {
            self.select(key);
            true
        }
    }

    /// Clears all selected keys and returns whether the selection was non-empty.
    pub fn clear(&mut self) -> bool {
        let had = !self.selected.is_empty();
        self.selected.clear();
        had
    }

    /// Borrow the raw selected-key set, for a caller that needs to iterate it.
    ///
    /// Order is unspecified (it is a `HashSet`); a widget that needs the
    /// selection in display order resolves each key to its current index and
    /// sorts by that, exactly as a positional range would be resolved.
    #[must_use]
    pub fn keys(&self) -> &std::collections::HashSet<SequenceKey> {
        &self.selected
    }

    /// Removes selected keys absent from `present` and returns the number removed.
    ///
    /// This is intended for known deletions; absent filtered keys otherwise persist. Runs in `O(selected)` time.
    pub fn reconcile(&mut self, present: &std::collections::HashSet<SequenceKey>) -> usize {
        let before = self.selected.len();
        self.selected.retain(|k| present.contains(k));
        before - self.selected.len()
    }

    /// Returns selected keys in the order supplied by `order`, skipping keys absent from that sequence.
    #[must_use]
    pub fn selected_in_order(&self, order: &[SequenceKey]) -> Vec<SequenceKey> {
        order
            .iter()
            .filter(|k| self.selected.contains(k))
            .cloned()
            .collect()
    }
}
