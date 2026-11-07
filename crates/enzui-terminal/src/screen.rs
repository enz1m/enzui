//! The cell grid a terminal emulator writes and the renderer draws.
//!
//! The emulator owns parsing, scrollback and modes; it writes what is on
//! screen into a [`TerminalScreen`] from any thread. Each write marks the rows
//! it touched, and the renderer rebuilds and uploads only those rows.
//!
//! A write wakes the window the screen is shown in directly, at most once per
//! frame, so a burst of output costs one redraw rather than one per chunk.
//! The wake reaches the terminal view as an event and redraws the window
//! without calling the app's `update` or rebuilding its `view`; an app needs
//! no subscription to keep a terminal live.

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// An sRGB colour with straight alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgba(pub [u8; 4]);

impl Rgba {
    /// An opaque colour.
    #[must_use]
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self([r, g, b, 255])
    }

    /// No colour: on a cell background, the terminal's own background.
    pub const NONE: Self = Self([0, 0, 0, 0]);

    /// Whether this is [`Self::NONE`] or otherwise fully transparent.
    #[must_use]
    pub const fn is_none(self) -> bool {
        self.0[3] == 0
    }

    /// Packed as `0xAABBGGRR`, the byte order the shaders read.
    #[must_use]
    pub const fn packed(self) -> u32 {
        u32::from_le_bytes(self.0)
    }
}

impl From<iced::Color> for Rgba {
    fn from(color: iced::Color) -> Self {
        let [r, g, b, a] = color.into_rgba8();
        Self([r, g, b, a])
    }
}

/// Text attributes of one cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct CellFlags(u16);

impl CellFlags {
    /// Bold weight.
    pub const BOLD: Self = Self(1);
    /// Italic style.
    pub const ITALIC: Self = Self(1 << 1);
    /// A single underline.
    pub const UNDERLINE: Self = Self(1 << 2);
    /// A double underline.
    pub const DOUBLE_UNDERLINE: Self = Self(1 << 3);
    /// A curly underline (undercurl).
    pub const CURLY_UNDERLINE: Self = Self(1 << 4);
    /// A dotted underline.
    pub const DOTTED_UNDERLINE: Self = Self(1 << 5);
    /// A dashed underline.
    pub const DASHED_UNDERLINE: Self = Self(1 << 6);
    /// Struck through.
    pub const STRIKETHROUGH: Self = Self(1 << 7);
    /// Foreground drawn at reduced intensity.
    pub const DIM: Self = Self(1 << 8);
    /// The glyph is not drawn; its background still is.
    pub const HIDDEN: Self = Self(1 << 9);
    /// The first cell of a double-width character.
    pub const WIDE: Self = Self(1 << 10);
    /// The second cell of a double-width character; it draws nothing itself.
    pub const WIDE_SPACER: Self = Self(1 << 11);

    /// No attributes.
    pub const NONE: Self = Self(0);

    /// Whether every flag in `other` is set.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0 && other.0 != 0
    }

    /// Whether any flag in `other` is set.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    /// The union of both sets.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Any underline style.
    pub const ANY_UNDERLINE: Self = Self(
        Self::UNDERLINE.0
            | Self::DOUBLE_UNDERLINE.0
            | Self::CURLY_UNDERLINE.0
            | Self::DOTTED_UNDERLINE.0
            | Self::DASHED_UNDERLINE.0,
    );
}

impl std::ops::BitOr for CellFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

impl std::ops::BitOrAssign for CellFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = self.union(rhs);
    }
}

/// One cell of the grid, with its colours already resolved to sRGB.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Cell {
    /// The cell's first character; a space draws nothing.
    pub ch: char,
    /// Combining characters that follow `ch` in the same grapheme, if any.
    pub combining: Option<Box<str>>,
    /// The foreground colour.
    pub fg: Rgba,
    /// The background colour; [`Rgba::NONE`] shows the terminal background.
    pub bg: Rgba,
    /// The underline colour; [`Rgba::NONE`] follows `fg`.
    pub underline: Rgba,
    /// Attributes.
    pub flags: CellFlags,
}

impl Default for Cell {
    fn default() -> Self {
        Self::blank(Rgba::rgb(255, 255, 255))
    }
}

impl Cell {
    /// A space with `fg` on the default background.
    #[must_use]
    pub const fn blank(fg: Rgba) -> Self {
        Self {
            ch: ' ',
            combining: None,
            fg,
            bg: Rgba::NONE,
            underline: Rgba::NONE,
            flags: CellFlags::NONE,
        }
    }

    /// `ch` in `fg` on the default background.
    #[must_use]
    pub const fn new(ch: char, fg: Rgba) -> Self {
        Self {
            ch,
            combining: None,
            fg,
            bg: Rgba::NONE,
            underline: Rgba::NONE,
            flags: CellFlags::NONE,
        }
    }

    /// Whether the cell draws a glyph.
    #[must_use]
    pub fn has_glyph(&self) -> bool {
        !(self.ch == ' ' && self.combining.is_none())
            && self.ch != '\0'
            && !self
                .flags
                .intersects(CellFlags::HIDDEN.union(CellFlags::WIDE_SPACER))
    }
}

/// How the cursor is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum CursorShape {
    /// A filled cell; the glyph under it takes the cursor's text colour.
    #[default]
    Block,
    /// An outline of the cell, as an unfocused terminal shows it.
    HollowBlock,
    /// A vertical bar at the cell's left edge.
    Beam,
    /// A bar along the cell's bottom.
    Underline,
}

/// The cursor, in viewport cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Cursor {
    /// Column.
    pub col: u16,
    /// Row.
    pub row: u16,
    /// Shape.
    pub shape: CursorShape,
    /// Whether it is drawn.
    pub visible: bool,
}

/// A selection between two cells, inclusive, in reading order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Selection {
    /// The first selected cell, `(col, row)`.
    pub start: (u16, u16),
    /// The last selected cell, `(col, row)`.
    pub end: (u16, u16),
    /// Whether whole rectangles are selected rather than reading-order runs.
    pub block: bool,
}

impl Selection {
    /// Whether `(col, row)` is inside.
    #[must_use]
    pub fn contains(&self, col: u16, row: u16) -> bool {
        if self.block {
            let (c0, c1) = (self.start.0.min(self.end.0), self.start.0.max(self.end.0));
            return row >= self.start.1 && row <= self.end.1 && col >= c0 && col <= c1;
        }
        let at = (row, col);
        at >= (self.start.1, self.start.0) && at <= (self.end.1, self.end.0)
    }

    /// The rows it covers.
    #[must_use]
    pub fn rows(&self) -> std::ops::RangeInclusive<u16> {
        self.start.1..=self.end.1
    }
}

/// Terminal modes the widget needs to know about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Modes {
    /// The application asked for mouse reports, so a plain press goes to it
    /// rather than starting a selection (Shift still selects).
    pub mouse_reporting: bool,
    /// The alternate screen is active, so the wheel goes to the application.
    pub alternate_screen: bool,
}

/// What is on screen, as the emulator last wrote it.
#[derive(Debug)]
pub struct ScreenState {
    cols: u16,
    rows: u16,
    cells: Vec<Cell>,
    dirty: Vec<bool>,
    row_revision: Vec<u64>,
    revision: u64,
    cursor: Cursor,
    selection: Option<Selection>,
    modes: Modes,
    fg: Rgba,
}

impl ScreenState {
    fn new(cols: u16, rows: u16, fg: Rgba) -> Self {
        let len = usize::from(cols) * usize::from(rows);
        Self {
            cols,
            rows,
            cells: vec![Cell::blank(fg); len],
            dirty: vec![true; usize::from(rows)],
            row_revision: vec![1; usize::from(rows)],
            revision: 1,
            cursor: Cursor {
                visible: true,
                ..Cursor::default()
            },
            selection: None,
            modes: Modes::default(),
            fg,
        }
    }

    /// Columns.
    #[must_use]
    pub const fn cols(&self) -> u16 {
        self.cols
    }

    /// Rows.
    #[must_use]
    pub const fn rows(&self) -> u16 {
        self.rows
    }

    /// The cells of row `y`.
    ///
    /// # Panics
    /// When `y` is not a row.
    #[must_use]
    pub fn row(&self, y: u16) -> &[Cell] {
        let start = usize::from(y) * usize::from(self.cols);
        &self.cells[start..start + usize::from(self.cols)]
    }

    /// The cell at `(col, row)`, if inside the grid.
    #[must_use]
    pub fn cell(&self, col: u16, row: u16) -> Option<&Cell> {
        (col < self.cols && row < self.rows)
            .then(|| &self.cells[usize::from(row) * usize::from(self.cols) + usize::from(col)])
    }

    /// The cursor.
    #[must_use]
    pub const fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// The selection.
    #[must_use]
    pub const fn selection(&self) -> Option<Selection> {
        self.selection
    }

    /// The modes.
    #[must_use]
    pub const fn modes(&self) -> Modes {
        self.modes
    }

    /// Increments on every change, so a reader can tell nothing moved.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Increments when row `y` changes.
    #[must_use]
    pub fn row_revision(&self, y: u16) -> u64 {
        self.row_revision[usize::from(y)]
    }

    /// The text of row `y` with trailing blanks removed.
    #[must_use]
    pub fn row_text(&self, y: u16) -> String {
        let mut text = String::with_capacity(usize::from(self.cols));
        for cell in self.row(y) {
            if cell.flags.contains(CellFlags::WIDE_SPACER) {
                continue;
            }
            text.push(if cell.ch == '\0' { ' ' } else { cell.ch });
            if let Some(combining) = &cell.combining {
                text.push_str(combining);
            }
        }
        text.truncate(text.trim_end().len());
        text
    }

    /// The selected text, rows joined by newlines.
    #[must_use]
    pub fn selection_text(&self) -> Option<String> {
        let selection = self.selection?;
        let mut out = String::new();
        for y in selection.rows() {
            if y >= self.rows {
                break;
            }
            let mut line = String::new();
            for (x, cell) in self.row(y).iter().enumerate() {
                let x = x as u16;
                if !selection.contains(x, y) || cell.flags.contains(CellFlags::WIDE_SPACER) {
                    continue;
                }
                line.push(if cell.ch == '\0' { ' ' } else { cell.ch });
                if let Some(combining) = &cell.combining {
                    line.push_str(combining);
                }
            }
            if y != selection.start.1 {
                out.push('\n');
            }
            out.push_str(line.trim_end());
        }
        Some(out)
    }

    fn touch(&mut self, y: u16) {
        if let Some(dirty) = self.dirty.get_mut(usize::from(y)) {
            *dirty = true;
            self.revision += 1;
            self.row_revision[usize::from(y)] = self.revision;
        }
    }

    fn touch_all(&mut self) {
        for y in 0..self.rows {
            self.touch(y);
        }
    }

    /// Takes the set of rows changed since the last call; `None` when nothing
    /// changed. The renderer calls this.
    pub(crate) fn take_damage(&mut self) -> Option<Vec<u16>> {
        let rows: Vec<u16> = self
            .dirty
            .iter_mut()
            .enumerate()
            .filter_map(|(y, dirty)| std::mem::take(dirty).then_some(y as u16))
            .collect();
        (!rows.is_empty()).then_some(rows)
    }

    /// Marks every row changed, as after the renderer lost its buffers.
    pub(crate) fn damage_all(&mut self) {
        self.touch_all();
    }

    /// Marks row `y` changed.
    pub(crate) fn damage_row(&mut self, y: u16) {
        self.touch(y);
    }
}

/// A write into the screen. Every method marks the rows it changes.
pub struct ScreenWriter<'a> {
    state: &'a mut ScreenState,
}

impl ScreenWriter<'_> {
    /// Columns.
    #[must_use]
    pub fn cols(&self) -> u16 {
        self.state.cols
    }

    /// Rows.
    #[must_use]
    pub fn rows(&self) -> u16 {
        self.state.rows
    }

    /// Resizes the grid, keeping the top-left cells and blanking the rest.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        if (cols, rows) == (self.state.cols, self.state.rows) {
            return;
        }
        let blank = Cell::blank(self.state.fg);
        let mut cells = vec![blank; usize::from(cols) * usize::from(rows)];
        for y in 0..rows.min(self.state.rows) {
            for x in 0..cols.min(self.state.cols) {
                cells[usize::from(y) * usize::from(cols) + usize::from(x)] = self.state.cells
                    [usize::from(y) * usize::from(self.state.cols) + usize::from(x)]
                .clone();
            }
        }
        self.state.cols = cols;
        self.state.rows = rows;
        self.state.cells = cells;
        self.state.dirty = vec![true; usize::from(rows)];
        self.state.row_revision = vec![0; usize::from(rows)];
        self.state.selection = None;
        self.state.touch_all();
    }

    /// The cells of row `y`, marked changed.
    ///
    /// # Panics
    /// When `y` is not a row.
    pub fn row_mut(&mut self, y: u16) -> &mut [Cell] {
        assert!(
            y < self.state.rows,
            "row {y} outside {} rows",
            self.state.rows
        );
        self.state.touch(y);
        let cols = usize::from(self.state.cols);
        let start = usize::from(y) * cols;
        &mut self.state.cells[start..start + cols]
    }

    /// Replaces row `y` when it differs, so an emulator can rewrite every row
    /// each update and still damage only the ones that changed.
    pub fn set_row(&mut self, y: u16, cells: impl IntoIterator<Item = Cell>) {
        if y >= self.state.rows {
            return;
        }
        let cols = usize::from(self.state.cols);
        let start = usize::from(y) * cols;
        let mut changed = false;
        let blank = Cell::blank(self.state.fg);
        let mut incoming = cells.into_iter();
        for x in 0..cols {
            let cell = incoming.next().unwrap_or_else(|| blank.clone());
            let slot = &mut self.state.cells[start + x];
            if *slot != cell {
                *slot = cell;
                changed = true;
            }
        }
        if changed {
            self.state.touch(y);
        }
    }

    /// Sets one cell, damaging its row only when it changed.
    pub fn set_cell(&mut self, col: u16, row: u16, cell: Cell) {
        if col >= self.state.cols || row >= self.state.rows {
            return;
        }
        let index = usize::from(row) * usize::from(self.state.cols) + usize::from(col);
        if self.state.cells[index] != cell {
            self.state.cells[index] = cell;
            self.state.touch(row);
        }
    }

    /// Moves or restyles the cursor.
    pub fn set_cursor(&mut self, cursor: Cursor) {
        let old = self.state.cursor;
        if old != cursor {
            self.state.cursor = cursor;
            self.state.touch(old.row);
            self.state.touch(cursor.row);
        }
    }

    /// Replaces the selection.
    pub fn set_selection(&mut self, selection: Option<Selection>) {
        let old = self.state.selection;
        if old == selection {
            return;
        }
        self.state.selection = selection;
        for range in [old, selection].into_iter().flatten().map(|s| s.rows()) {
            for y in range {
                self.state.touch(y);
            }
        }
    }

    /// Sets the modes.
    pub fn set_modes(&mut self, modes: Modes) {
        if self.state.modes != modes {
            self.state.modes = modes;
            self.state.revision += 1;
        }
    }

    /// The foreground a resized or blank cell starts with.
    pub fn set_default_fg(&mut self, fg: Rgba) {
        self.state.fg = fg;
    }

    /// Reads the state being written.
    #[must_use]
    pub fn state(&self) -> &ScreenState {
        self.state
    }
}

struct Shared {
    id: u64,
    state: Mutex<ScreenState>,
    pending: AtomicBool,
    /// The runtime waker of the window showing this screen.
    window: Mutex<Option<iced_graphics::core::shell::Waker>>,
    wake: Mutex<Option<futures::channel::mpsc::Sender<()>>>,
    /// Where the view last drew the grid, for telling what each row paints.
    paint: Mutex<PaintState>,
}

/// The view's last drawing of a screen, as its rows' signatures need it.
#[derive(Debug, Default)]
pub(crate) struct PaintState {
    /// The style last drawn, and a count that moves when it changes.
    pub(crate) style: Option<Arc<crate::TerminalStyle>>,
    pub(crate) style_generation: u64,
    /// The cell height and the grid's top padding, in logical pixels.
    pub(crate) rows: Option<(f32, f32)>,
}

/// A shared handle to one terminal's screen; clones refer to the same grid.
///
/// A screen has one view at a time: the view takes the screen's damage when it
/// draws and registers its window to be woken, so a second view (another
/// window, or the same window twice) would miss changes and share the first
/// one's GPU state. Clones are for the producer (the emulator side) and the
/// one view; to mirror a terminal, feed a second screen. Debug builds assert
/// against two views in one frame.
#[derive(Clone)]
pub struct TerminalScreen {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for TerminalScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalScreen")
            .field("id", &self.shared.id)
            .finish_non_exhaustive()
    }
}

impl PartialEq for TerminalScreen {
    fn eq(&self, other: &Self) -> bool {
        self.shared.id == other.shared.id
    }
}

impl Eq for TerminalScreen {}

impl Hash for TerminalScreen {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.shared.id.hash(state);
    }
}

impl TerminalScreen {
    /// A blank `cols` by `rows` screen whose blank cells use `fg`.
    #[must_use]
    pub fn new(cols: u16, rows: u16, fg: Rgba) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            shared: Arc::new(Shared {
                id: NEXT.fetch_add(1, Ordering::Relaxed),
                state: Mutex::new(ScreenState::new(cols.max(1), rows.max(1), fg)),
                pending: AtomicBool::new(false),
                window: Mutex::new(None),
                wake: Mutex::new(None),
                paint: Mutex::new(PaintState::default()),
            }),
        }
    }

    /// A stable identity, distinct for every screen.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.shared.id
    }

    /// Writes into the screen and wakes the window if anything changed.
    pub fn update<R>(&self, write: impl FnOnce(&mut ScreenWriter<'_>) -> R) -> R {
        let (result, changed) = {
            let mut state = self.lock();
            let before = state.revision;
            let result = write(&mut ScreenWriter { state: &mut state });
            (result, state.revision != before)
        };
        if changed {
            self.wake();
        }
        result
    }

    /// Reads the screen.
    pub fn read<R>(&self, read: impl FnOnce(&ScreenState) -> R) -> R {
        read(&self.lock())
    }

    /// The view's last drawing of this screen.
    pub(crate) fn paint_state(&self) -> MutexGuard<'_, PaintState> {
        self.shared
            .paint
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, ScreenState> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn wake(&self) {
        if self.shared.pending.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Some(window) = self
            .shared
            .window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            window.wake();
        }
        let mut wake = self
            .shared
            .wake
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(sender) = wake.as_mut()
            && sender.try_send(()).is_err()
            && sender.is_closed()
        {
            *wake = None;
        }
    }

    /// Locks the screen for drawing a frame and lets the next write wake the
    /// window again, so writes between two frames cost one wake.
    ///
    /// The flag clears under the lock writers take, before the frame takes
    /// its damage: a write after this point finds it clear and wakes. Clearing
    /// it after drawing instead left a write that landed mid-frame undrawn
    /// until the next input.
    pub(crate) fn lock_for_frame(&self) -> MutexGuard<'_, ScreenState> {
        let state = self.lock();
        self.shared.pending.store(false, Ordering::Release);
        state
    }

    /// Whether a write is waiting to be drawn.
    pub(crate) fn pending(&self) -> bool {
        self.shared.pending.load(Ordering::Acquire)
    }

    /// Wakes `waker`'s window on the next write. The view calls this.
    pub(crate) fn set_window(&self, waker: &iced_graphics::core::shell::Waker) {
        *self
            .shared
            .window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(waker.clone());
    }

    /// Marks `rows` changed and wakes the window, so they draw again next
    /// frame. The renderer calls this for rows whose glyphs it did not finish.
    pub(crate) fn redraw_rows(&self, rows: &[u16]) {
        {
            let mut state = self.lock();
            for &y in rows {
                state.damage_row(y);
            }
        }
        self.wake();
    }

    /// A subscription that yields `()` when the screen changed, for an app
    /// that wants a message too (to refresh a title, say). The view redraws
    /// itself without it. Writes before the next frame share one wake.
    pub fn wakes(&self) -> iced::Subscription<()> {
        iced::Subscription::run_with(self.clone(), |screen| {
            let (sender, receiver) = futures::channel::mpsc::channel(1);
            *screen
                .shared
                .wake
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sender);
            screen.shared.pending.store(false, Ordering::Release);
            receiver
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str) -> Vec<Cell> {
        text.chars()
            .map(|ch| Cell::new(ch, Rgba::rgb(255, 255, 255)))
            .collect()
    }

    #[test]
    fn only_rows_that_changed_are_damaged() {
        let screen = TerminalScreen::new(5, 3, Rgba::rgb(255, 255, 255));
        screen.update(|w| {
            w.set_row(0, row("hello"));
            w.set_row(1, row("world"));
        });
        screen.lock().take_damage();

        screen.update(|w| {
            w.set_row(0, row("hello"));
            w.set_row(1, row("there"));
            w.set_cell(4, 2, Cell::new(' ', Rgba::rgb(255, 255, 255)));
        });
        assert_eq!(
            screen.lock().take_damage(),
            Some(vec![1]),
            "an identical row is not damage"
        );
        assert_eq!(screen.lock().take_damage(), None);

        screen.update(|w| {
            w.set_cursor(Cursor {
                col: 2,
                row: 2,
                shape: CursorShape::Block,
                visible: true,
            });
        });
        assert_eq!(
            screen.lock().take_damage(),
            Some(vec![0, 2]),
            "the cursor's old and new rows"
        );
    }

    #[test]
    fn a_write_after_the_frame_took_its_damage_wakes_the_window() {
        use std::sync::atomic::AtomicUsize;
        let screen = TerminalScreen::new(4, 2, Rgba::rgb(255, 255, 255));
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&wakes);
        screen.set_window(&iced_graphics::core::shell::Waker::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));

        screen.update(|w| w.set_row(0, row("one")));
        screen.update(|w| w.set_row(1, row("two")));
        assert_eq!(
            wakes.load(Ordering::SeqCst),
            1,
            "writes before a frame share one wake"
        );

        // The renderer draws: it takes the damage, then a write lands.
        let damage = screen.lock_for_frame().take_damage();
        assert_eq!(damage, Some(vec![0, 1]));
        screen.update(|w| w.set_row(0, row("three")));
        assert_eq!(
            wakes.load(Ordering::SeqCst),
            2,
            "the late write wakes the window again"
        );
    }
}
