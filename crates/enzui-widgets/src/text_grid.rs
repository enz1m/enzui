//! EnzTextGrid: a single-row dense text grid for TUI-style output.
//!
//! A row of fixed-width monospaced cells, each with its own tone / colour / background
//! / bold / blank flags. Callers stack rows vertically (`column![]`) to build
//! terminals, log viewers, log heat-maps, etc.
//!
//! # Why a custom widget
//!
//! A monospace `text()` run drifts box-drawing glyphs off-column on fractional
//! advances; a `row!` of fixed containers still cannot promise device-pixel
//! starts under fractional scales, nor one shared snapped geometry for
//! backgrounds and glyphs. `TextGridWidget` is a small inert leaf with this
//! contract:
//!
//! - column `x = snap_down(abs, scale, ancestor_tx)` — every column on a
//!   device pixel across DPI, fractional origins and ancestor translations;
//! - scale from `Renderer::scale()`, `1.0` fallback when unavailable
//!   (still a consistent cross-row grid);
//! - background quad and glyph box derive from the same snapped rectangle —
//!   never disagreeing;
//! - adjacent backgrounds tile gapless: N's right edge is N+1's left edge;
//! - outer width exactly `advance_cells * cell_width` — rows flush, columns
//!   aligned across rows;
//! - one grapheme per cell; wider cells only via explicit `.span(n)` /
//!   `.auto_span()` — width is never inferred;
//! - blank cells paint background, no glyph;
//! - tones resolve from the runtime theme;
//! - one row-level a11y label (derived, overridable); per-cell glyphs are
//!   decorative.
//!
//! # Pointer routing
//!
//! The grid routes pointer events in columns, the caller's unit (see the
//! doctest on the builder for the wiring shape).
//!
//! The widget reports; it does not own a selection: one row, no scrollback /
//! reflow / wrapped lines — a widget-owned anchor pair would be a selection
//! model on a view rebuilt every frame. The app owns the model; this is the
//! feeding seam.
//!
//! Edges report a saturating column plus an explicit [`TextGridEdge`] — never
//! `None`, never a silent clamp: past-last saturates with `AfterLast`
//! ("select to end of line"), pre-first with `BeforeFirst`, vertical distance
//! as signed whole-cell [`TextGridPointer::row_offset`] (`0` inside). A
//! same-pitch row stack turns an escaped drag into a real (row, column)
//! without the widget knowing the stack. `None` would discard the coordinate
//! exactly when a drag needs it; an unflagged clamp could not distinguish
//! "on the last column" from "200 px past a short line" — a terminal's
//! swallow-rest-of-line decision.
//!
//! Presses report only inside the row; drag/release report wherever the
//! gesture goes (that is drag-to-the-edge). Hover fires inside-only,
//! on column change, never mid-drag. `mouse_interaction` is `Text` over a
//! press/drag-wired grid; [`EnzTextGrid::cell_interaction`] overrides per
//! column (OSC-8 links).

pub use enzui_core::grapheme_cell::InlineGrapheme;
use enzui_theme::Palette;
use iced::advanced::text::paragraph::{self, Paragraph as _};
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Layout, Widget, layout, mouse, renderer, text};
use iced::{Background, Color, Element, Length, Rectangle, Size};
use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
mod grid_widget;

// Per-thread glyph shape cache shared by all grid instances.
//
// Each widget keeps shaped paragraphs in `tree::State` — allocation-free on
// redraw-only passes, but dead whenever iced rebuilds the `Tree` node (a
// sibling appearing above shifts the child index; the next `layout`
// re-shapes a terminal screenful through cosmic-text, ~250-300 ms).
//
// This cache outlives the rebuild: thread-living, keyed by the same
// `ShapeKey`, so a rebuilt tree re-seeds by cloning a shaped paragraph (a
// refcount bump) instead of re-running the shaper. Bounded by
// `SHARED_SHAPE_BUDGET` glyphs — a monospace alphabet is tiny; the budget
// only guards adversarial churn. Generic over `P` via `Box<dyn Any>` keyed
// by `TypeId` (one concrete `P` per process in practice).

// Sized for a full-screen monospace mix (200 cols × 30 rows fits
// with headroom for CJK / box-draw / emoji). At the bound the
// policy is insert-only: a new glyph reshapes each time, but the
// already-cached working set never evicts, so a sidebar toggle
// over stationary content stays reshape-free.
const SHARED_SHAPE_BUDGET: usize = 8192;

thread_local! {
    // `TypeId(Plain<P>) -> ShapeMap<Plain<P>>`, boxed to erase `P`.
    static SHARED_SHAPE: RefCell<HashMap<TypeId, Box<dyn Any>, std::hash::BuildHasherDefault<FxHasher>>> =
        RefCell::new(HashMap::default());
}

// Cumulative shaper-invocation count for this thread's cache.
// Thread-local (not a process-global atomic) so concurrent tests
// cannot read each other's shaping work; the cache-contract
// relayout-reshape test asserts a structural relayout does not
// bump this.
mod shape_call_counter {
    use std::cell::Cell;

    thread_local! {
        pub(super) static SHARED_SHAPE_CALLS: Cell<u64> = const { Cell::new(0) };
    }
}

/// Bump this thread's shaper-invocation count.
#[inline]
pub(crate) fn note_shared_shape_call() {
    shape_call_counter::SHARED_SHAPE_CALLS.with(|c| c.set(c.get().wrapping_add(1)));
}

// Returns a clone (cheap: the cosmic-text buffer is refcounted).
fn shared_shape_get<P>(probe: &dyn ShapeKeyLookup) -> Option<paragraph::Plain<P>>
where
    P: text::Paragraph + Clone + 'static,
{
    SHARED_SHAPE.with(|cell| {
        let map = cell.borrow();
        let per_type = map.get(&TypeId::of::<paragraph::Plain<P>>())?;
        let typed = per_type
            .downcast_ref::<ShapeMap<paragraph::Plain<P>>>()
            .expect("shared shape cache keyed by TypeId of its own value type");
        typed.get(probe).cloned()
    })
}

fn shared_shape_put<P>(key: ShapeKey, para: paragraph::Plain<P>)
where
    P: text::Paragraph + Clone + 'static,
{
    SHARED_SHAPE.with(|cell| {
        let mut map = cell.borrow_mut();
        let per_type = map
            .entry(TypeId::of::<paragraph::Plain<P>>())
            .or_insert_with(|| {
                Box::new(ShapeMap::<paragraph::Plain<P>>::default()) as Box<dyn Any>
            });
        let typed = per_type
            .downcast_mut::<ShapeMap<paragraph::Plain<P>>>()
            .expect("shared shape cache keyed by TypeId of its own value type");
        // Insert-only-until-full: stop admitting past the bound, never
        // drop cached. Re-inserting a present key (same glyph, same metrics)
        // is a no-op keeping a live glyph live — never starved by the bound.
        // Graceful: the hot set stays reshape-free; only genuinely-new glyphs
        // past the bound pay un-cached, only for themselves.
        if typed.len() >= SHARED_SHAPE_BUDGET && !typed.contains_key(&key) {
            return;
        }
        typed.insert(key, para);
    });
}

/// A non-glyph mark painted over one cell, in its own ink.
///
/// Callers pick the shape from a runtime value (cursor setting, diagnostic
/// severity, OSC 22 request).
///
/// Ink is required: a glyph-colour-inheriting decoration vanishes on
/// selected/reverse-video cells. A block cursor is not a decoration — it is
/// the cell's `bg`/`color` swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextGridCellDecoration {
    /// Paint no decoration.
    #[default]
    None,
    /// A horizontal rule along the cell's bottom edge, spanning
    /// the cell's full (possibly multi-column) width.
    Underline,
    /// A vertical rule along the cell's leading (left) edge,
    /// spanning the cell's full height.
    LeadingBar,
}

/// Semantic tone for a cell's foreground. Resolved against the runtime theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextGridCellTone {
    /// Primary `text` colour.
    #[default]
    Default,
    /// Muted secondary text (`text_muted`).
    Muted,
    /// Subtle tertiary text (`text_subtle`).
    Subtle,
    /// Accent colour.
    Accent,
    /// Success tone.
    Success,
    /// Warning tone.
    Warning,
    /// Error tone.
    Error,
}

/// One grapheme cluster in the grid, preserving combining marks, variation
/// selectors and ZWJ emoji as a single cell.
///
/// Not struct-literal writable; [`Self::glyph`] alone constructs, so
/// normalization cannot be bypassed.
#[derive(Debug, Clone)]
#[must_use]
pub struct TextGridCell {
    // Private so struct-literal callers cannot bypass
    // [`Self::glyph`]'s one-grapheme normalization. Type is
    // `InlineGrapheme` (24 bytes, holds clusters up to 22 bytes
    // inline; spills to heap without truncating).
    grapheme: InlineGrapheme,
    /// Semantic tone (used unless `color` is supplied).
    pub tone: TextGridCellTone,
    /// Explicit foreground colour. `None` = use `tone` mapping.
    pub color: Option<Color>,
    /// Per-cell background. `None` = keep grid background (transparent).
    pub bg: Option<Color>,
    // Set by [`Self::subtle_on`]; kept separate from `bg` so a
    // cell that also fills keeps its ink.
    subtle_backdrop: Option<Color>,
    /// Whether to render the glyph with a bold font weight.
    pub bold: bool,
    /// Render nothing (space); overrides `grapheme`.
    pub blank: bool,
    // Only the span setters may change this nonzero column advance.
    span: usize,
    // Columns this glyph may be shaped/painted across, independent
    // of the advance. `None` = the advance. Paired
    // with [`Self::paint_cells`] so `0` is unrepresentable.
    paint_span: Option<usize>,
    // Paired with `decoration_ink` by [`Self::decoration`] alone,
    // so "shape with no colour" (which would fall back to glyph
    // ink and paint invisibly on a reverse-video cell) cannot be
    // built.
    decoration: TextGridCellDecoration,
    // `None` exactly when `decoration` is `None`.
    decoration_ink: Option<Color>,
}

impl TextGridCell {
    /// Construct a cell from a string, keeping only its **first grapheme
    /// cluster** (so `"e\u{0301}"` — e + combining acute — is one cell). An
    /// empty string produces a blank cell.
    pub fn glyph(s: impl AsRef<str>) -> Self {
        let s = s.as_ref();
        let mut chars = s.chars();
        // One scalar is always exactly one grapheme cluster.
        let first = if chars.next().is_some() && chars.next().is_none() {
            s
        } else {
            s.graphemes(true).next().unwrap_or("")
        };
        Self::from_grapheme(first)
    }

    /// A cell for a grapheme the caller already holds as one cluster, such
    /// as a terminal's cell buffer: no segmentation and no allocation. An
    /// empty grapheme makes a blank cell.
    pub fn cluster(grapheme: InlineGrapheme) -> Self {
        let blank = grapheme.is_empty();
        Self {
            blank,
            grapheme,
            ..Self::from_grapheme("")
        }
    }

    fn from_grapheme(first: &str) -> Self {
        let blank = first.is_empty();
        Self {
            grapheme: InlineGrapheme::new(first),
            tone: TextGridCellTone::default(),
            color: None,
            bg: None,
            subtle_backdrop: None,
            bold: false,
            blank,
            span: 1,
            paint_span: None,
            decoration: TextGridCellDecoration::None,
            decoration_ink: None,
        }
    }

    /// Construct a cell from a single `char`, as [`Self::glyph`] would,
    /// without allocating or segmenting graphemes.
    pub fn ch(c: char) -> Self {
        let mut utf8 = [0u8; 4];
        Self::from_grapheme(c.encode_utf8(&mut utf8))
    }
    /// A blank / space cell (background only, no glyph).
    pub fn blank() -> Self {
        Self {
            grapheme: InlineGrapheme::empty(),
            tone: TextGridCellTone::default(),
            color: None,
            bg: None,
            subtle_backdrop: None,
            bold: false,
            blank: true,
            span: 1,
            paint_span: None,
            decoration: TextGridCellDecoration::None,
            decoration_ink: None,
        }
    }
    /// Set the semantic foreground tone.
    pub fn tone(mut self, tone: TextGridCellTone) -> Self {
        self.tone = tone;
        self
    }
    /// Paint this cell as subtle text resolved against `backdrop`.
    ///
    /// Backdrop-aware counterpart of `.tone(Subtle)`: draws through
    /// [`Palette::text_subtle_on`] so tertiary glyphs over selected-row text
    /// clear 4.5:1. Explicit by design — legacy callers keep the raw token
    /// with `bg` set unless opting in here. Explicit [`Self::color`] wins.
    pub fn subtle_on(mut self, backdrop: Color) -> Self {
        self.tone = TextGridCellTone::Subtle;
        self.subtle_backdrop = Some(backdrop);
        self
    }

    /// Override the foreground colour.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }
    /// Fill this cell with a background colour.
    pub fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }
    /// Enable or disable bold glyph rendering.
    pub fn bold(mut self, on: bool) -> Self {
        self.bold = on;
        self
    }

    /// Paint `shape` over this cell in `ink`.
    ///
    /// Drawn last, over the glyph; changes neither glyph, colour, background
    /// nor a11y label — AT reads the row's graphemes only. `ink` is
    /// independent of [`color`](Self::color)/[`tone`](Self::tone);
    /// [`TextGridCellDecoration::None`] clears shape *and* ink, so a runtime
    /// mapping needs no "off" branch.
    pub fn decoration(mut self, shape: TextGridCellDecoration, ink: Color) -> Self {
        match shape {
            TextGridCellDecoration::None => {
                self.decoration = TextGridCellDecoration::None;
                self.decoration_ink = None;
            }
            shape => {
                self.decoration = shape;
                self.decoration_ink = Some(ink);
            }
        }
        self
    }

    /// Set how many grid columns this cell occupies (explicit continuation).
    ///
    /// Defaults to one column; zero is clamped to one. A wider span reserves
    /// multiple columns without splitting the grapheme. Unicode width is not
    /// inferred unless [`Self::auto_span`] is used.
    pub fn span(mut self, columns: usize) -> Self {
        self.span = columns.max(1);
        self
    }

    /// Columns this cell's glyph may be shaped and painted across,
    /// starting at its own left edge. Defaults to
    /// [`advance_cells`](Self::advance_cells): no opt-in, paint box = cell.
    ///
    /// Distinct from [`span`](Self::span): a wide cluster keeps `span == 1`
    /// (column indexing and block cursor aligned) and grants paint columns
    /// here; the continuation cell stays blank, so nothing overdraws. `0`
    /// clamps to `1`; below-advance raises to advance (never clipped inside
    /// its own cell).
    pub fn paint_cells(mut self, columns: usize) -> Self {
        self.paint_span = Some(columns.max(1));
        self
    }

    /// The number of columns this cell's glyph is painted across — its
    /// [`paint_cells`](Self::paint_cells) opt-in, or its advance.
    pub fn glyph_columns(&self) -> usize {
        self.paint_span.unwrap_or(0).max(self.advance_cells())
    }

    /// Opt into Unicode-width-derived span: East-Asian wide / fullwidth
    /// graphemes occupy 2 columns, everything else 1.
    ///
    /// The compatibility automatic behaviour as an explicit caller choice —
    /// one-column-per-cell stays the default, CJK double-width is per-cell
    /// opt-in.
    pub fn auto_span(mut self) -> Self {
        self.span = if self.blank || self.grapheme.is_empty() {
            1
        } else {
            match UnicodeWidthStr::width(self.grapheme.as_str()) {
                0 | 1 => 1,
                _ => 2,
            }
        };
        self
    }

    /// The normalized grapheme rendered by this cell.
    pub fn grapheme(&self) -> &str {
        self.grapheme.as_str()
    }

    /// `true` when this cell's grapheme cost no heap allocation.
    /// Exposed so a caller can gate the allocation-freedom claim
    /// externally. Only clusters longer than
    /// [`INLINE_CAPACITY`](enzui_core::grapheme_cell::INLINE_CAPACITY)
    /// bytes are `false`.
    pub fn grapheme_is_inline(&self) -> bool {
        self.grapheme.is_inline()
    }

    /// The decoration painted over this cell, [`None`] by default.
    ///
    /// [`None`]: TextGridCellDecoration::None
    pub fn decoration_shape(&self) -> TextGridCellDecoration {
        self.decoration
    }

    /// The decoration's ink, `Some` exactly when
    /// [`decoration_shape`](Self::decoration_shape) is not [`None`](TextGridCellDecoration::None).
    pub fn decoration_ink(&self) -> Option<Color> {
        self.decoration_ink
    }

    /// Number of grid columns this cell advances.
    ///
    /// The explicit [`span`](Self::span), default `1`. Unicode width is not
    /// inferred — double-width CJK opts in via [`span`](Self::span)/[`auto_span`](Self::auto_span).
    /// `usize`: a row may exceed `u16::MAX` columns.
    pub fn advance_cells(&self) -> usize {
        self.span.max(1)
    }
}

/// Split `text` into one cell per grapheme cluster, each reporting
/// the columns it occupies.
///
/// Per-`char` splits tear clusters: a flag becomes `US`, a ZWJ family
/// becomes people, combining marks become stray accents. Each cell carries
/// [`auto_span`](TextGridCell::auto_span), so cursor and outer width align
/// with paint. Output sums to `UnicodeWidthStr::width(text)`.
pub fn cluster_cells(text: &str) -> Vec<TextGridCell> {
    text.graphemes(true)
        .map(|g| {
            // A cluster that trims to nothing is a blank cell (a space, a tab):
            // the grid paints no glyph but keeps the column slot, so column N is
            // the same pixel on every row. `auto_span` already returns 1 for a
            // blank, so the column arithmetic is unaffected.
            if g.trim().is_empty() {
                TextGridCell::blank()
            } else {
                TextGridCell::glyph(g).auto_span()
            }
        })
        .collect()
}

/// How many grid columns `text` occupies when laid out by
/// [`cluster_cells`] — i.e. where a cursor just past it belongs.
///
/// `chars().count()` is wrong both ways: combining marks overcount (2 vs 1),
/// CJK undercounts (1 vs 2). Defined as the same per-cell-span sum, so cursor
/// placement cannot drift from paint.
pub fn cluster_columns(text: &str) -> usize {
    cluster_cells(text)
        .iter()
        .map(TextGridCell::advance_cells)
        .sum()
}

// Uses the smaller cell dimension so a bar in a narrow column and
// an underline in a short row have the same weight. Clamped:
// >= 1 logical px, <= the dimension it crosses.
fn decoration_thickness(cell_w: f32, cell_h: f32) -> f32 {
    let base = cell_w.min(cell_h);
    if !base.is_finite() || base <= 0.0 {
        return 1.0;
    }
    (base / 8.0).round().max(1.0).min(base)
}

// Free function of the already-snapped rect so tests reuse the
// exact geometry `draw` submits; `None` short-circuits before
// allocating a quad.
fn decoration_rect(
    shape: TextGridCellDecoration,
    cell_rect: Rectangle,
    token_w: f32,
    token_h: f32,
) -> Option<Rectangle> {
    let t = decoration_thickness(token_w, token_h);
    match shape {
        TextGridCellDecoration::None => None,
        // Bottom edge, full cell width — a cell spanning two columns is
        // underlined across both, because the mark belongs to the cell.
        TextGridCellDecoration::Underline => {
            let t = t.min(cell_rect.height.max(0.0));
            Some(Rectangle {
                x: cell_rect.x,
                y: cell_rect.y + cell_rect.height - t,
                width: cell_rect.width,
                height: t,
            })
        }
        // Leading edge, full cell height.
        TextGridCellDecoration::LeadingBar => Some(Rectangle {
            x: cell_rect.x,
            y: cell_rect.y,
            width: t.min(cell_rect.width.max(0.0)),
            height: cell_rect.height,
        }),
    }
}

/// Geometry tokens for the grid. Colours are resolved from the
/// runtime theme at draw time (this type carries geometry only),
/// so unpinned tones follow light/dark.
///
/// Defaults: `cell-width = 8 px`, `cell-height = 16 px`,
/// `font-size = footnote (12 px)`, monospace family.
#[derive(Debug, Clone, Copy)]
#[must_use]
pub struct TextGridTokens {
    /// Character cell width in px.
    pub cell_width: f32,
    /// Character cell height in px.
    pub cell_height: f32,
    /// Monospace text size in px (`footnote` = 12).
    pub font_size: f32,
    /// Optional explicit mono font. `None` = the backend's `Family::Monospace`.
    pub font: Option<iced::Font>,
    /// Per-side horizontal headroom, in px, that a glyph may be
    /// shaped and painted into beyond its own cell. `0.0` by default; for
    /// PUA icons a font draws wider than one column (Nerd Font / powerline).
    ///
    /// Shaping box and clip grow per side; cell rectangle (background,
    /// decoration, snapping, advance) is unchanged — no column moves. Ink
    /// may overlap a neighbour's, so this is opt-in px, never global.
    pub glyph_bleed_x: f32,
}

impl Default for TextGridTokens {
    fn default() -> Self {
        Self {
            cell_width: 8.0,
            cell_height: 16.0,
            font_size: 12.0,
            font: None,
            glyph_bleed_x: 0.0,
        }
    }
}

impl TextGridTokens {
    /// Alias of [`default`](Self::default) (geometry only).
    pub fn dark() -> Self {
        Self::default()
    }
    /// Alias of [`default`](Self::default) (geometry only).
    pub fn light() -> Self {
        Self::default()
    }
    /// Override the monospace font used to shape grid cells.
    pub fn font(mut self, font: iced::Font) -> Self {
        self.font = Some(font);
        self
    }

    /// Set [`glyph_bleed_x`](Self::glyph_bleed_x), the per-side horizontal
    /// paint headroom for oversized glyphs.
    ///
    /// Non-finite or negative input falls back to `0.0`. Clamped to one cell
    /// width per side at use, so a bleed can never reach past the immediately
    /// adjacent column and turn one bad glyph into an unreadable row.
    pub fn glyph_bleed_x(mut self, px: f32) -> Self {
        self.glyph_bleed_x = enzui_core::metrics::FiniteNonNeg::new_or(px, 0.0).get();
        self
    }

    /// The bleed this token set applies, clamped against the cell
    /// width. Read by both the shaping box and the clip rectangle
    /// so the two cannot disagree.
    pub(crate) fn effective_bleed_x(&self) -> f32 {
        self.glyph_bleed_x.min(self.cell_width.max(0.0))
    }

    fn sane(mut self) -> Self {
        if !(self.cell_width.is_finite() && self.cell_width > 0.0) {
            self.cell_width = 8.0;
        }
        if !(self.cell_height.is_finite() && self.cell_height > 0.0) {
            self.cell_height = 16.0;
        }
        if !(self.font_size.is_finite() && self.font_size > 0.0) {
            self.font_size = 12.0;
        }
        if !(self.glyph_bleed_x.is_finite() && self.glyph_bleed_x >= 0.0) {
            self.glyph_bleed_x = 0.0;
        }
        self
    }

    // Nerd Font folder faces are unusually wide but vertically
    // short in the proportional fallback: apply the scale only
    // to that known set, so unrelated PUA icons (e.g. an already-
    // square clock glyph) do not oversize while the folders can
    // fill their paint box.
    fn glyph_font_size(&self, grapheme: &str) -> f32 {
        const NERD_FOLDER_SCALE: f32 = 1.5;
        if self.effective_bleed_x() > 0.0 && is_nerd_font_folder(grapheme) {
            (self.font_size * NERD_FOLDER_SCALE).min(self.cell_height)
        } else {
            self.font_size
        }
    }

    // Folder glyphs need the full adjacent-cell gutter at their
    // enlarged size; every other glyph keeps the caller's budget.
    fn glyph_paint_bleed_x(&self, grapheme: &str) -> f32 {
        let bleed = self.effective_bleed_x();
        if is_nerd_font_folder(grapheme) {
            (bleed * 2.0).min(self.cell_width.max(0.0))
        } else {
            bleed
        }
    }

    fn resolved_font(&self, bold: bool) -> iced::Font {
        // ensure the upright-face monospace fallback is installed at first
        // use so a text grid renders upright without any app opt-in. This runs
        // whether or not an explicit font override is set, since an override may
        // still be `Family::Monospace`.
        crate::monospace_fallback::ensure_monospace_fallback();
        let mut f = self.font.unwrap_or(iced::Font {
            family: iced::font::Family::Monospace,
            ..iced::Font::DEFAULT
        });
        f.weight = if bold {
            iced::font::Weight::Semibold
        } else {
            iced::font::Weight::Normal
        };
        f
    }
}

/// Every tone's ink, raised for one backdrop. A grid on a canvas resolves
/// this once per draw for the canvas and per cell only for a cell with its own
/// background, so a full terminal screen does not search per glyph.
#[derive(Debug, Clone, Copy)]
struct ToneInks([Color; 7]);

// The palette colours `ToneInks::resolve` reads, then the backdrop.
type ToneInksKey = [[u32; 4]; 8];

thread_local! {
    // Every row of a terminal is its own grid drawn against the same palette
    // and canvas, so one remembered entry turns 50 searches per frame into one.
    static LAST_TONE_INKS: std::cell::Cell<Option<(ToneInksKey, ToneInks)>> =
        const { std::cell::Cell::new(None) };
}

impl ToneInks {
    fn on(p: &Palette, backdrop: Color) -> Self {
        let bits = |c: Color| [c.r.to_bits(), c.g.to_bits(), c.b.to_bits(), c.a.to_bits()];
        let key = [
            bits(p.text),
            bits(p.text_muted),
            bits(p.text_subtle),
            bits(p.accent),
            bits(p.tone_success_fg),
            bits(p.tone_warning_fg),
            bits(p.tone_error_fg),
            bits(backdrop),
        ];
        if let Some((known, inks)) = LAST_TONE_INKS.get()
            && known == key
        {
            return inks;
        }
        let inks = Self::resolve(p, backdrop);
        LAST_TONE_INKS.set(Some((key, inks)));
        inks
    }

    fn resolve(p: &Palette, backdrop: Color) -> Self {
        use enzui_theme::{ContrastFloor, SemanticTone, raise_to_floor};
        #[cfg(test)]
        tone_inks_resolved::note();
        let raise = |ink| raise_to_floor(ink, backdrop, ContrastFloor::Text).color();
        Self([
            raise(p.text),
            p.text_muted_on(backdrop),
            p.text_subtle_on(backdrop),
            raise(p.accent),
            p.tone_on(SemanticTone::Success, backdrop).color(),
            p.tone_on(SemanticTone::Warning, backdrop).color(),
            p.tone_on(SemanticTone::Error, backdrop).color(),
        ])
    }

    fn of(&self, tone: TextGridCellTone) -> Color {
        self.0[match tone {
            TextGridCellTone::Default => 0,
            TextGridCellTone::Muted => 1,
            TextGridCellTone::Subtle => 2,
            TextGridCellTone::Accent => 3,
            TextGridCellTone::Success => 4,
            TextGridCellTone::Warning => 5,
            TextGridCellTone::Error => 6,
        }]
    }
}

// Explicit `color` wins over `tone`. On a declared canvas, a tone is raised
// against what is actually under the glyph: the cell's own background, or else
// the canvas.
//
// `backgrounds` memoises the inks of each distinct cell background met in one
// draw: a selection or a status bar fills many cells with one colour, and the
// contrast search must not run once per glyph.
fn resolve_fg_on(
    p: &Palette,
    cell: &TextGridCell,
    canvas: Option<&ToneInks>,
    backgrounds: &mut Vec<(Color, ToneInks)>,
) -> Color {
    const MEMO: usize = 8;

    if let Some(c) = cell.color {
        return c;
    }
    if let Some(bg) = cell.bg.filter(|_| canvas.is_some()) {
        if let (TextGridCellTone::Subtle, Some(backdrop)) = (cell.tone, cell.subtle_backdrop) {
            return p.text_subtle_on(backdrop);
        }
        if let Some((_, inks)) = backgrounds.iter().find(|(known, _)| *known == bg) {
            return inks.of(cell.tone);
        }
        let inks = ToneInks::on(p, bg);
        if backgrounds.len() < MEMO {
            backgrounds.push((bg, inks));
        }
        return inks.of(cell.tone);
    }
    if let Some(inks) = canvas {
        return match (cell.tone, cell.subtle_backdrop) {
            (TextGridCellTone::Subtle, Some(backdrop)) => p.text_subtle_on(backdrop),
            (tone, _) => inks.of(tone),
        };
    }
    resolve_fg(p, cell)
}

// Explicit `color` wins over `tone`.
fn resolve_fg(p: &Palette, cell: &TextGridCell) -> Color {
    if let Some(c) = cell.color {
        return c;
    }
    match cell.tone {
        TextGridCellTone::Default => p.text,
        TextGridCellTone::Muted => p.text_muted,
        TextGridCellTone::Subtle => cell
            .subtle_backdrop
            .map_or(p.text_subtle, |backdrop| p.text_subtle_on(backdrop)),
        TextGridCellTone::Accent => p.accent,
        TextGridCellTone::Success => p.success,
        TextGridCellTone::Warning => p.warning,
        TextGridCellTone::Error => p.error,
    }
}

// Include the layout origin and later renderer translation when snapping, then
// map back to layout space; snapping only a column offset misses fractional parents.
fn snap_down(x: f32, scale: f32, ancestor_tx: f32) -> f32 {
    let s = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let tx = if ancestor_tx.is_finite() {
        ancestor_tx
    } else {
        0.0
    };
    ((x + tx) * s).floor() / s - tx
}

// `Renderer::scale` is `None` before the first shell hint and always on
// the `tiny-skia` backend (also the headless test renderer). Then snap at
// 1.0: still a consistent grid (every column of every row on the same
// logical coordinate), not device-pixel-aligned.
fn renderer_scale<R: iced::advanced::Renderer>(renderer: &R) -> f32 {
    renderer.scale().map_or(1.0, |s| {
        let t = s.total();
        if t.is_finite() && t > 0.0 { t } else { 1.0 }
    })
}

/// Which side of the row a reported pointer position fell outside of.
///
/// Horizontal companion to [`TextGridPointer::column`]: the column stays a
/// real in-range column; this says whether the pointer was over it.
/// `AfterLast` as "select through end of line" is the terminal behaviour;
/// ignoring it still yields a usable coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextGridEdge {
    /// The pointer was horizontally over the reported column.
    #[default]
    Inside,
    /// The pointer was left of column 0; `column` saturated to `0`.
    BeforeFirst,
    /// The pointer was right of the last column; `column` saturated to the last
    /// column. Also reported for an empty grid, which has no column at all
    /// (`column` is then `0` and indexes nothing).
    AfterLast,
}

/// A pointer position expressed in the grid's own cell coordinates.
///
/// `column` counts columns, not cells: a double-width cell occupies two, so
/// the value aligns with a terminal cursor column and [`cluster_columns`],
/// not a cell-slice index. See the module pointer-routing docs for the edge
/// policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextGridPointer {
    /// Column under the pointer, saturated into `0..columns`. Never out of
    /// range; check [`edge`](Self::edge) to learn whether it was saturated.
    pub column: usize,
    /// Signed vertical distance from this row, in whole cell heights: `0`
    /// inside the row, `-1` one row above, `2` two rows below. A caller
    /// stacking rows on one pitch adds this to its own row index.
    pub row_offset: i32,
    /// Whether the pointer was horizontally inside the row.
    pub edge: TextGridEdge,
}

impl TextGridPointer {
    /// `true` when the pointer was genuinely over the reported cell — inside
    /// horizontally and on this row vertically.
    #[must_use]
    pub fn is_inside(&self) -> bool {
        self.edge == TextGridEdge::Inside && self.row_offset == 0
    }
}

// Nerd Font folder codepoints (fa-folder, fa-folder-open, custom
// and Material variants). Variation selectors are ignored.
fn is_nerd_font_folder(grapheme: &str) -> bool {
    let mut chars = grapheme.chars();
    let Some(base) = chars.next() else {
        return false;
    };
    let folder = matches!(base as u32, 0xE5FF | 0xF07B | 0xF07C | 0xF024B);
    folder && chars.all(|ch| matches!(ch as u32, 0xFE00..=0xFE0F | 0xE0100..=0xE01EF))
}

// Map a position in absolute layout coordinates onto this row's
// cell coordinates. Free function so it is unit-testable and so
// event arms cannot each grow their own copy. A non-positive /
// non-finite `cell_width` degrades to column 0 rather than
// dividing by zero.
fn pointer_at(
    at: iced::Point,
    bounds: Rectangle,
    columns: usize,
    cell_width: f32,
    cell_height: f32,
) -> TextGridPointer {
    let columns_i32 = i32::try_from(columns).unwrap_or(i32::MAX);
    let usable = |v: f32| v.is_finite() && v > 0.0;

    let (column, edge) = if columns == 0 || !usable(cell_width) || !(at.x - bounds.x).is_finite() {
        // No column exists (empty row), or no usable pitch: there is nothing to
        // be inside of, so say so instead of inventing column 0 as a hit.
        (0usize, TextGridEdge::AfterLast)
    } else {
        let raw = enzui_core::metrics::to_i32_saturating(((at.x - bounds.x) / cell_width).floor());
        if raw < 0 {
            (0usize, TextGridEdge::BeforeFirst)
        } else if raw >= columns_i32 {
            (columns - 1, TextGridEdge::AfterLast)
        } else {
            (usize::try_from(raw).unwrap_or(0), TextGridEdge::Inside)
        }
    };

    let row_offset = if usable(cell_height) && (at.y - bounds.y).is_finite() {
        enzui_core::metrics::to_i32_saturating(((at.y - bounds.y) / cell_height).floor())
    } else {
        0
    };

    TextGridPointer {
        column,
        row_offset,
        edge,
    }
}

// Leaf widget: paints the snapped grid, routes pointer events in
// cell coordinates.
struct TextGridWidget<'a, Message> {
    cells: Arc<[TextGridCell]>,
    tokens: TextGridTokens,
    advance: usize,
    canvas: Option<Color>,
    on_press: Option<Box<dyn Fn(TextGridPointer) -> Message + 'a>>,
    on_drag: Option<Box<dyn Fn(TextGridPointer) -> Message + 'a>>,
    on_release: Option<Box<dyn Fn(TextGridPointer) -> Message + 'a>>,
    on_hover: Option<Box<dyn Fn(TextGridPointer) -> Message + 'a>>,
    cell_interaction: Option<Box<dyn Fn(usize) -> mouse::Interaction + 'a>>,
    revision: Option<u64>,
}

impl<'a, Message> TextGridWidget<'a, Message> {
    fn new(cells: Arc<[TextGridCell]>, tokens: TextGridTokens) -> Self {
        let advance: usize = cells.iter().map(TextGridCell::advance_cells).sum();
        Self {
            cells,
            tokens,
            advance,
            canvas: None,
            on_press: None,
            on_drag: None,
            on_release: None,
            on_hover: None,
            cell_interaction: None,
            revision: None,
        }
    }

    // `true` when nothing is wired: `update` must stay a no-op so
    // a purely decorative grid cannot swallow a press.
    fn is_inert(&self) -> bool {
        self.on_press.is_none()
            && self.on_drag.is_none()
            && self.on_release.is_none()
            && self.on_hover.is_none()
            && self.cell_interaction.is_none()
    }

    fn pointer_at(&self, at: iced::Point, bounds: Rectangle) -> TextGridPointer {
        pointer_at(
            at,
            bounds,
            self.advance,
            self.tokens.cell_width,
            self.tokens.cell_height,
        )
    }

    fn outer_size(&self) -> Size {
        Size::new(
            self.advance as f32 * self.tokens.cell_width,
            self.tokens.cell_height,
        )
    }
}

/// A shape map: every visible cell probes one every frame, so it hashes
/// with [`FxHasher`] rather than SipHash.
type ShapeMap<V> = HashMap<ShapeKey, V, std::hash::BuildHasherDefault<FxHasher>>;

/// The Fx hash (rustc's): a rotate, xor and multiply per word.
///
/// The shape maps are process-local, bounded by [`SHARED_SHAPE_BUDGET`] and
/// keyed by the app's own rendered graphemes, so SipHash's flooding
/// resistance buys nothing there and cost ~15 % of a typing frame.
#[derive(Default)]
struct FxHasher {
    hash: u64,
}

impl FxHasher {
    const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(Self::SEED);
    }
}

impl std::hash::Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        let (chunks, rest) = bytes.as_chunks::<8>();
        for chunk in chunks {
            self.add(u64::from_le_bytes(*chunk));
        }
        if !rest.is_empty() {
            let mut word = [0u8; 8];
            word[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(word));
        }
    }
    fn write_u8(&mut self, i: u8) {
        self.add(u64::from(i));
    }
    fn write_u16(&mut self, i: u16) {
        self.add(u64::from(i));
    }
    fn write_u32(&mut self, i: u32) {
        self.add(u64::from(i));
    }
    fn write_u64(&mut self, i: u64) {
        self.add(i);
    }
    fn write_usize(&mut self, i: usize) {
        self.add(i as u64);
    }
    fn finish(&self) -> u64 {
        self.hash
    }
}

// Cache key: shape is glyph + type metrics only. Colour is excluded —
// `fill_paragraph` takes it separately, so recolour-only cells still hit
// (`bg`/`tone` likewise). Omitting an included field would silently return
// the wrong glyph. `f32`s store as `to_bits()` (exact; NaN pre-excluded by
// `TextGridTokens::sane`). Owned/borrowed share one definition via
// [`ShapeKeyLookup`].
#[derive(Clone)]
struct ShapeKey {
    // Empty never reaches the cache (blank cells shape nothing).
    grapheme: String,
    font_size_bits: u32,
    // Drives both `line_height` and `bounds.height`.
    cell_height_bits: u32,
    // `(span * cell_width).to_bits()`; centring reads it.
    cell_w_bits: u32,
    font: iced::Font,
    // `hint_factor.map(f32::to_bits)`; `None` is distinct from `Some`.
    hint_bits: Option<u32>,
}

// Borrowed probe: the steady state is every-cell-hit, so the
// lookup path must not clone a `String` per cell. Identity is
// shared with the owned form through [`ShapeKeyLookup`]; both
// impls delegate to the same method so a borrowed probe finds
// what an owned key inserted.
#[derive(Clone, Copy)]
struct ShapeKeyRef<'a> {
    grapheme: &'a str,
    font_size_bits: u32,
    cell_height_bits: u32,
    cell_w_bits: u32,
    font: iced::Font,
    hint_bits: Option<u32>,
}

impl<'a> ShapeKeyRef<'a> {
    // Called only on a cache miss; the paragraph is being shaped
    // anyway, so one `String` allocation is irrelevant here.
    fn to_owned_key(self) -> ShapeKey {
        ShapeKey {
            grapheme: self.grapheme.to_owned(),
            font_size_bits: self.font_size_bits,
            cell_height_bits: self.cell_height_bits,
            cell_w_bits: self.cell_w_bits,
            font: self.font,
            hint_bits: self.hint_bits,
        }
    }
}

// Implemented by both [`ShapeKey`] and [`ShapeKeyRef`] so the
// owned and borrowed forms hash and compare identically — a
// silent divergence would keep the cache from ever hitting.
trait ShapeKeyLookup {
    fn grapheme(&self) -> &str;
    fn font_size_bits(&self) -> u32;
    fn cell_height_bits(&self) -> u32;
    fn cell_w_bits(&self) -> u32;
    fn font(&self) -> iced::Font;
    fn hint_bits(&self) -> Option<u32>;
}

impl ShapeKeyLookup for ShapeKey {
    fn grapheme(&self) -> &str {
        &self.grapheme
    }
    fn font_size_bits(&self) -> u32 {
        self.font_size_bits
    }
    fn cell_height_bits(&self) -> u32 {
        self.cell_height_bits
    }
    fn cell_w_bits(&self) -> u32 {
        self.cell_w_bits
    }
    fn font(&self) -> iced::Font {
        self.font
    }
    fn hint_bits(&self) -> Option<u32> {
        self.hint_bits
    }
}

impl ShapeKeyLookup for ShapeKeyRef<'_> {
    fn grapheme(&self) -> &str {
        self.grapheme
    }
    fn font_size_bits(&self) -> u32 {
        self.font_size_bits
    }
    fn cell_height_bits(&self) -> u32 {
        self.cell_height_bits
    }
    fn cell_w_bits(&self) -> u32 {
        self.cell_w_bits
    }
    fn font(&self) -> iced::Font {
        self.font
    }
    fn hint_bits(&self) -> Option<u32> {
        self.hint_bits
    }
}

// `iced::Font` is `PartialEq` but not `Hash`. Exhaustive by
// construction: adding a field upstream makes this fail to
// compile rather than letting it be dropped from the key.
fn font_identity(f: iced::Font) -> (u64, u16, u8, u8, u8) {
    let iced::Font {
        family,
        weight,
        stretch,
        style,
        numeric,
    } = f;
    let family_bits = match family {
        iced::font::Family::Name(n) => {
            // Hash the name's bytes into a u64 so the tuple stays `Copy`.
            use std::hash::{Hash, Hasher};
            let mut h = FxHasher::default();
            n.hash(&mut h);
            // Tag so a family literally named the same as a generic's tag
            // cannot collide with that generic.
            h.finish() | (1 << 63)
        }
        iced::font::Family::SansSerif => 1,
        iced::font::Family::Serif => 2,
        iced::font::Family::Cursive => 3,
        iced::font::Family::Fantasy => 4,
        iced::font::Family::Monospace => 5,
    };
    (
        family_bits,
        weight as u16,
        stretch as u8,
        style as u8,
        numeric as u8,
    )
}

impl std::hash::Hash for dyn ShapeKeyLookup + '_ {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.grapheme().hash(state);
        self.font_size_bits().hash(state);
        self.cell_height_bits().hash(state);
        self.cell_w_bits().hash(state);
        font_identity(self.font()).hash(state);
        self.hint_bits().hash(state);
    }
}

impl PartialEq for dyn ShapeKeyLookup + '_ {
    fn eq(&self, other: &Self) -> bool {
        self.grapheme() == other.grapheme()
            && self.font_size_bits() == other.font_size_bits()
            && self.cell_height_bits() == other.cell_height_bits()
            && self.cell_w_bits() == other.cell_w_bits()
            && self.font() == other.font()
            && self.hint_bits() == other.hint_bits()
    }
}

impl Eq for dyn ShapeKeyLookup + '_ {}

impl std::hash::Hash for ShapeKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Delegate to the one shared definition, so an owned key and a borrowed
        // probe cannot hash differently.
        <dyn ShapeKeyLookup>::hash(self, state);
    }
}

impl PartialEq for ShapeKey {
    fn eq(&self, other: &Self) -> bool {
        <dyn ShapeKeyLookup>::eq(self, other)
    }
}

impl Eq for ShapeKey {}

impl<'a> std::borrow::Borrow<dyn ShapeKeyLookup + 'a> for ShapeKey {
    fn borrow(&self) -> &(dyn ShapeKeyLookup + 'a) {
        self
    }
}

// Per-widget dedup store: per-cell `Vec<Plain>` re-shapes on every content
// change (O(cells)/frame; 200x54 scrolling pegged a core). [`ShapeKey`]
// collapses to one shape per distinct (glyph, metrics); `order` maps cells
// to paragraphs so `draw` never rehashes.
struct GridState<P: text::Paragraph> {
    // Insertion order. `order` indexes here so `draw` needs no
    // hashing and no key.
    paragraphs: Vec<paragraph::Plain<P>>,
    // Owns the only copy of each key.
    shaped: ShapeMap<usize>,
    // Parallel to `self.cells`; `None` for blank/empty cells.
    // A `usize` (rather than a `ShapeKey`) keeps the steady-state
    // per-cell cost at one hash lookup with no allocation.
    order: Vec<Option<usize>>,
    // Cumulative shape-miss count. The relayout-reshape test
    // asserts on this counter directly (wall-clock is too noisy;
    // allocation proxies measure the wrong thing).
    shape_calls: usize,
    // Paired with `shape_calls` so a test can assert a real hit
    // rate rather than an absence of misses (which a widget that
    // drew nothing would also satisfy).
    cache_hits: usize,
    // Shape-map lookups, which the ASCII table exists to avoid.
    map_probes: usize,
    // A gesture that started on this row is live. Set by a press
    // inside the bounds, cleared by the release. Lets drag /
    // release be reported outside the row while a press elsewhere
    // in the window is still not this row's press.
    pressed: bool,
    // Last column published via `on_cell_hover`; suppresses
    // republish when the pointer moves within one cell.
    last_hover: Option<usize>,
    // Direct index for one-byte ASCII cells, so a terminal-sized grid does
    // not hash a key per cell per frame. Valid for `ascii_frame` only.
    ascii: AsciiShapes,
    ascii_frame: Option<AsciiFrame>,
    // The caller's content revision and every shaping input `order` was
    // resolved under; a frame matching all of them reuses `order` as is.
    prepared: Option<PreparedRow>,
    // Frames that reused `order` through a matching content revision.
    revision_reuses: usize,
}

#[derive(Clone, Copy, PartialEq)]
struct PreparedRow {
    revision: u64,
    frame: AsciiFrame,
    font_size_bits: u32,
    cell_width_bits: u32,
    bleed_bits: u32,
    cells: usize,
}

// One slot per (bold, byte). Each entry keeps the metrics it was shaped at
// and is used only when the cell's metrics match exactly.
type AsciiShapes = Box<[Option<AsciiShape>; 256]>;

#[derive(Clone, Copy)]
struct AsciiShape {
    paragraph: usize,
    font_size_bits: u32,
    cell_w_bits: u32,
}

// What every ASCII entry shares within a frame.
#[derive(Clone, Copy, PartialEq)]
struct AsciiFrame {
    cell_height_bits: u32,
    hint_bits: Option<u32>,
    regular: (u64, u16, u8, u8, u8),
    bold: (u64, u16, u8, u8, u8),
}

fn ascii_slot(grapheme: &str, bold: bool) -> Option<usize> {
    match grapheme.as_bytes() {
        [byte] if byte.is_ascii() => Some(usize::from(*byte) | (usize::from(bold) << 7)),
        _ => None,
    }
}

impl<P: text::Paragraph> Default for GridState<P> {
    fn default() -> Self {
        Self {
            paragraphs: Vec::new(),
            shaped: ShapeMap::default(),
            order: Vec::new(),
            shape_calls: 0,
            cache_hits: 0,
            map_probes: 0,
            pressed: false,
            last_hover: None,
            ascii: Box::new([None; 256]),
            ascii_frame: None,
            prepared: None,
            revision_reuses: 0,
        }
    }
}

impl<Message> TextGridWidget<'_, Message> {
    // Cost is O(distinct keys), not O(cells) — the map persists
    // in the widget tree, so a row drawn from a small alphabet
    // shapes each glyph at most once and hits the cache on every
    // subsequent frame. Called from `layout`, and from `draw`
    // only via the already-populated cache.
    fn prepare<Renderer>(&self, state: &mut GridState<Renderer::Paragraph>, renderer: &Renderer)
    where
        Renderer: text::Renderer,
        Renderer::Paragraph: Clone + 'static,
    {
        let ch = self.tokens.cell_height;
        let hint = renderer.hint_factor();
        let hint_bits = hint.map(f32::to_bits);
        let regular_font = self.tokens.resolved_font(false);
        let bold_font = self.tokens.resolved_font(true);
        let frame = AsciiFrame {
            cell_height_bits: ch.to_bits(),
            hint_bits,
            regular: font_identity(regular_font),
            bold: font_identity(bold_font),
        };
        if state.ascii_frame != Some(frame) {
            state.ascii.fill(None);
            state.ascii_frame = Some(frame);
        }
        let prepared = self.revision.map(|revision| PreparedRow {
            revision,
            frame,
            font_size_bits: self.tokens.font_size.to_bits(),
            cell_width_bits: self.tokens.cell_width.to_bits(),
            bleed_bits: self.tokens.effective_bleed_x().to_bits(),
            cells: self.cells.len(),
        });
        if prepared.is_some() && state.prepared == prepared {
            state.revision_reuses += 1;
            return;
        }
        state.prepared = prepared;

        state.order.clear();
        state.order.reserve(self.cells.len());

        for cell in self.cells.iter() {
            // Blank / empty cells shape nothing and paint no glyph.
            if cell.blank || cell.grapheme.is_empty() {
                state.order.push(None);
                continue;
            }
            // Shape into the paint box, not the advance box. Without a paint
            // bleed the two are the same width.
            let cell_w = cell.glyph_columns() as f32 * self.tokens.cell_width
                + 2.0 * self.tokens.glyph_paint_bleed_x(cell.grapheme.as_str());
            let font = if cell.bold { bold_font } else { regular_font };
            let glyph_font_size = self.tokens.glyph_font_size(cell.grapheme.as_str());
            let ascii = ascii_slot(cell.grapheme.as_str(), cell.bold);
            if let Some(shape) = ascii.and_then(|slot| state.ascii[slot])
                && shape.font_size_bits == glyph_font_size.to_bits()
                && shape.cell_w_bits == cell_w.to_bits()
            {
                state.cache_hits += 1;
                state.order.push(Some(shape.paragraph));
                continue;
            }
            // Borrowed probe first: `HashMap::get` with a `ShapeKeyRef` hashes
            // the same bytes as the owned `ShapeKey` (see `ShapeKeyRef`), so the
            // steady-state path allocates nothing at all. The owned key — with
            // its `String` — is built only on an actual miss.
            let probe = ShapeKeyRef {
                grapheme: cell.grapheme.as_str(),
                font_size_bits: glyph_font_size.to_bits(),
                cell_height_bits: ch.to_bits(),
                cell_w_bits: cell_w.to_bits(),
                font,
                hint_bits,
            };

            state.map_probes += 1;
            let idx = if let Some(&i) = state.shaped.get(&probe as &dyn ShapeKeyLookup) {
                // Already shaped this exact (glyph, metrics) pair *in this
                // widget's own tree state* — the steady-state hot path.
                state.cache_hits += 1;
                i
            } else if let Some(para) =
                shared_shape_get::<Renderer::Paragraph>(&probe as &dyn ShapeKeyLookup)
            {
                // Own-cache miss, process-cache hit — the sidebar-toggle
                // case: iced dropped `GridState`, but the glyph lives in the
                // thread-local cache. Re-seed by cloning (refcount bump), not
                // re-shaping; `shape_calls` unbumped (no shaping) and not a
                // local `cache_hits` either — the cross-tree save the stall
                // fix turns on.
                state.cache_hits += 1;
                let i = state.paragraphs.len();
                state.paragraphs.push(para);
                state.shaped.insert(probe.to_owned_key(), i);
                i
            } else {
                let mut para = paragraph::Plain::<Renderer::Paragraph>::default();
                para.update(text::Text {
                    content: cell.grapheme.as_str(),
                    bounds: Size::new(cell_w, ch),
                    size: glyph_font_size.into(),
                    line_height: text::LineHeight::Absolute(ch.into()),
                    font,
                    align_x: text::Alignment::Center,
                    align_y: iced::alignment::Vertical::Center,
                    shaping: text::Shaping::Advanced,
                    wrapping: text::Wrapping::None,
                    ellipsis: text::Ellipsis::None,
                    hint_factor: hint,
                });
                state.shape_calls += 1;
                note_shared_shape_call();
                // Seed both caches: the local index for this widget's steady
                // state, and the process-wide cache so a future tree rebuild
                // (sidebar toggle) re-seeds from here instead of reshaping.
                shared_shape_put::<Renderer::Paragraph>(probe.to_owned_key(), para.clone());
                let i = state.paragraphs.len();
                state.paragraphs.push(para);
                state.shaped.insert(probe.to_owned_key(), i);
                i
            };
            if let Some(slot) = ascii {
                state.ascii[slot] = Some(AsciiShape {
                    paragraph: idx,
                    font_size_bits: glyph_font_size.to_bits(),
                    cell_w_bits: cell_w.to_bits(),
                });
            }
            state.order.push(Some(idx));
        }

        // Drop unreferenced paragraphs: churning glyphs (or a font-size
        // change) cannot grow unbounded, and stale-size paragraphs never
        // linger. Guarded on `len > distinct-in-use`: the steady state (all
        // in use) does no scan, no allocation. Rebuilds renumber — `order`
        // and map rewrite together.
        let mut used = vec![false; state.paragraphs.len()];
        let mut distinct = 0usize;
        for i in state.order.iter().flatten() {
            if !used[*i] {
                used[*i] = true;
                distinct += 1;
            }
        }
        if state.paragraphs.len() > distinct {
            // remap[old] = Some(new)
            let mut remap: Vec<Option<usize>> = vec![None; state.paragraphs.len()];
            let mut next = 0usize;
            for (old, keep) in used.iter().enumerate() {
                if *keep {
                    remap[old] = Some(next);
                    next += 1;
                }
            }
            let mut old_paras: Vec<Option<paragraph::Plain<Renderer::Paragraph>>> =
                std::mem::take(&mut state.paragraphs)
                    .into_iter()
                    .map(Some)
                    .collect();
            let mut rebuilt: Vec<paragraph::Plain<Renderer::Paragraph>> = Vec::with_capacity(next);
            for (old, slot) in remap.iter().enumerate() {
                if slot.is_some()
                    && let Some(p) = old_paras[old].take()
                {
                    rebuilt.push(p);
                }
            }
            state.paragraphs = rebuilt;
            state.shaped.retain(|_, i| match remap[*i] {
                Some(new) => {
                    *i = new;
                    true
                }
                None => false,
            });
            for i in state.order.iter_mut().flatten() {
                *i = remap[*i].expect("a referenced paragraph is always kept");
            }
            for entry in state.ascii.as_mut() {
                *entry = entry.and_then(|shape| {
                    remap[shape.paragraph].map(|paragraph| AsciiShape { paragraph, ..shape })
                });
            }
        }
    }
}

/// Theme access for `TextGridWidget` — any theme that can produce an enzUI
/// [`Palette`]. Blanket-implemented for `iced::Theme`.
pub trait TextGridTheme {
    /// The enzUI palette for the theme's current mode.
    fn enz_palette(&self) -> Palette;
}

impl TextGridTheme for iced::Theme {
    fn enz_palette(&self) -> Palette {
        crate::theme_lookup::palette(self)
    }
}

/// Text-grid row builder.
///
/// Generic over `Message`: a grid can now publish pointer events.
/// A grid with no handler is byte-identical to the compatibility inert one — the
/// `Message` parameter is inferred from the surrounding `Element`, so existing
/// call sites are unchanged.
#[must_use]
pub struct EnzTextGrid<'a, Message> {
    cells: Arc<[TextGridCell]>,
    tokens: TextGridTokens,
    canvas: Option<Color>,
    aria_text: Option<String>,
    on_press: Option<Box<dyn Fn(TextGridPointer) -> Message + 'a>>,
    on_drag: Option<Box<dyn Fn(TextGridPointer) -> Message + 'a>>,
    on_release: Option<Box<dyn Fn(TextGridPointer) -> Message + 'a>>,
    on_hover: Option<Box<dyn Fn(TextGridPointer) -> Message + 'a>>,
    cell_interaction: Option<Box<dyn Fn(usize) -> mouse::Interaction + 'a>>,
    revision: Option<u64>,
}

impl<'a, Message> EnzTextGrid<'a, Message> {
    /// Pin the grid's geometry and font tokens. The grid has no theme source,
    /// so this fixed snapshot is used as given.
    pub fn tokens(mut self, tokens: TextGridTokens) -> Self {
        self.tokens = tokens;
        self
    }

    /// Declare the colour the grid is painted on, when it is not the
    /// application's page.
    ///
    /// Semantic tones normally resolve from the application palette, which
    /// assumes the page behind them. A grid on its own canvas (a terminal's
    /// scheme background, say) gets each tone raised to the 4.5:1 text floor
    /// against what is actually under the glyph: the cell's own background
    /// when it has one, else this canvas. Explicit cell colours are kept as
    /// given. Without a canvas the palette inks are used unchanged.
    pub fn canvas(mut self, canvas: Color) -> Self {
        self.canvas = Some(canvas);
        self
    }

    /// Report a left-button press on this row, in cell coordinates.
    ///
    /// Inside-only: the pointer always satisfies
    /// [`TextGridPointer::is_inside`]. Setting it claims the press
    /// (`capture_event`) and shows an I-beam. The selection's anchor; the
    /// widget keeps none of its own (see module pointer-routing docs).
    pub fn on_cell_press(mut self, f: impl Fn(TextGridPointer) -> Message + 'a) -> Self {
        self.on_press = Some(Box::new(f));
        self
    }

    /// Report pointer motion **while a gesture that started on this row is
    /// live**, in cell coordinates.
    ///
    /// Wherever the pointer goes, off-row included: the column saturates and
    /// [`edge`](TextGridPointer::edge)/[`row_offset`](TextGridPointer::row_offset)
    /// say where it landed — without that, drag-select to end-of-line or the
    /// next row is inexpressible.
    pub fn on_cell_drag(mut self, f: impl Fn(TextGridPointer) -> Message + 'a) -> Self {
        self.on_drag = Some(Box::new(f));
        self
    }

    /// Report the left-button release that ends a gesture started on this
    /// row. Same saturating coordinate policy as `on_cell_drag`.
    pub fn on_cell_release(mut self, f: impl Fn(TextGridPointer) -> Message + 'a) -> Self {
        self.on_release = Some(Box::new(f));
        self
    }

    /// Report the cell under the pointer when no gesture is live.
    ///
    /// Fires only inside the row and only when the column changes, so it is
    /// cheap enough for the per-frame path an OSC-8 link scanner needs.
    /// Suppressed during a drag, which owns the pointer.
    pub fn on_cell_hover(mut self, f: impl Fn(TextGridPointer) -> Message + 'a) -> Self {
        self.on_hover = Some(Box::new(f));
        self
    }

    /// Override the mouse cursor per column — e.g. `Interaction::Pointer`
    /// over an OSC-8 hyperlink, while the rest of the row keeps the I-beam.
    ///
    /// Consulted only while the pointer is inside the row. When unset, a grid
    /// with a press or drag handler reports `Interaction::Text` and an inert
    /// grid reports `Interaction::None`.
    pub fn cell_interaction(mut self, f: impl Fn(usize) -> mouse::Interaction + 'a) -> Self {
        self.cell_interaction = Some(Box::new(f));
        self
    }

    /// Mark the row's glyph content with a revision the caller bumps whenever
    /// any cell's grapheme, weight or span changes, as a terminal's line
    /// damage does.
    ///
    /// A frame that repeats the previous revision, with unchanged tokens and
    /// cell count, reuses that frame's shaped glyphs without reading the
    /// cells' graphemes. Colours, backgrounds and decorations are still read
    /// every frame, so a recolour needs no new revision. Reusing a revision
    /// for different glyphs paints the old glyphs.
    pub fn content_revision(mut self, revision: u64) -> Self {
        self.revision = Some(revision);
        self
    }

    /// Override the automatically-derived row-level accessible label.
    ///
    /// By default the row concatenates its normalized graphemes, with blank
    /// cells represented as spaces. Empty overrides are ignored and therefore
    /// retain the automatic label; use meaningful prose when the visual row is
    /// not the best spoken representation.
    pub fn aria_text(mut self, s: impl Into<String>) -> Self {
        let s = s.into();
        self.aria_text = if s.is_empty() { None } else { Some(s) };
        self
    }

    /// Build the grid element with its pointer handlers and accessible label.
    pub fn into_element(self) -> Element<'a, Message>
    where
        Message: 'a,
    {
        let tokens = self.tokens.sane();
        // Without `.aria_text` the row speaks its own cells, blanks as spaces;
        // derived before the cells move into the visual widget, and only then.
        let label = self
            .aria_text
            .filter(|label| !label.is_empty())
            .or_else(|| {
                let automatic: String = self
                    .cells
                    .iter()
                    .map(|cell| {
                        if cell.blank || cell.grapheme.is_empty() {
                            " "
                        } else {
                            cell.grapheme.as_str()
                        }
                    })
                    .collect();
                (!automatic.is_empty()).then_some(automatic)
            });
        let mut widget = TextGridWidget::new(self.cells, tokens);
        widget.canvas = self.canvas;
        widget.on_press = self.on_press;
        widget.on_drag = self.on_drag;
        widget.on_release = self.on_release;
        widget.on_hover = self.on_hover;
        widget.cell_interaction = self.cell_interaction;
        widget.revision = self.revision;
        let el: Element<'a, Message> = Element::new(widget);
        match label {
            Some(label) => crate::semantics_container::SemanticsContainer::leaf(
                el,
                enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Label)
                    .label(label),
            )
            .into(),
            None => el,
        }
    }
}

impl<'a, Message: 'a> From<EnzTextGrid<'a, Message>> for Element<'a, Message> {
    fn from(g: EnzTextGrid<'a, Message>) -> Self {
        g.into_element()
    }
}

/// Build a text-grid row from normalized cells and default tokens.
pub fn enz_text_grid<'a, Message>(
    cells: impl IntoIterator<Item = TextGridCell>,
) -> EnzTextGrid<'a, Message> {
    EnzTextGrid {
        cells: cells.into_iter().collect(),
        tokens: TextGridTokens::default(),
        canvas: None,
        aria_text: None,
        on_press: None,
        on_drag: None,
        on_release: None,
        on_hover: None,
        cell_interaction: None,
        revision: None,
    }
}

/// Build a text-grid row from cells the caller keeps between frames.
///
/// A terminal or log view that stores each row once can hand the same row
/// over every frame for the cost of an `Arc` clone, instead of collecting
/// fresh cells per row. Pair it with
/// [`content_revision`](EnzTextGrid::content_revision) so an unchanged row
/// also skips re-resolving its shapes.
pub fn enz_text_grid_shared<'a, Message>(cells: Arc<[TextGridCell]>) -> EnzTextGrid<'a, Message> {
    EnzTextGrid {
        cells,
        tokens: TextGridTokens::default(),
        canvas: None,
        aria_text: None,
        on_press: None,
        on_drag: None,
        on_release: None,
        on_hover: None,
        cell_interaction: None,
        revision: None,
    }
}

#[cfg(test)]
pub(crate) mod tone_inks_resolved {
    use std::cell::Cell;

    thread_local! {
        static COUNT: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) fn note() {
        COUNT.with(|c| c.set(c.get() + 1));
    }
}
