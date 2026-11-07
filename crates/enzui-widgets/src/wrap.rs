//! EnzWrap: model-driven, fixed-cell badge/chip inventory.
//!
//! A model-driven badge inventory on a fixed-cell tiling model: every chip fills an
//! `item_width × item_height` cell, the column count derives from the rendered
//! width, and the container height grows with the row count. Use for tag rails
//! / chip bars / filter pills driven by a `[EnzBadgeView]` model.
//!
//! This is deliberately not arbitrary natural-width wrapping — that is
//! [`crate::flow::EnzFlow`], a separate widget for variable-width children.
//! EnzWrap is
//! the fixed-cell inventory; the two are distinct contracts, not substitutes.
//!
//! ## Fixed-cell layout (custom `Widget`)
//!
//! A focused custom single-pass layout `Widget` is justified here: it derives
//! the column count from the resolved width and places each `item_width`-wide
//! cell deterministically, predictably —
//!
//! ```text
//! columns        = max(1, floor((w + spacing_x) / (item_width + spacing_x)))
//! rows           = ceil(len / columns)
//! last_row_count = len - (rows - 1) * columns
//! height         = len==0 ? item_height
//!                         : rows*item_height + (rows-1)*spacing_y
//! ```
//!
//! Per-row main-axis justification distributes the leftover horizontal slack
//! `row_slack(count) = w - (count*item_width + (count-1)*spacing_x)`:
//!
//! - [`WrapJustify::Start`] — cells at the leading edge (default; plain
//!   left-to-right packing).
//! - [`WrapJustify::Center`] — each row's cells centred in its slack.
//! - [`WrapJustify::End`] — cells packed at the trailing edge.
//! - [`WrapJustify::SpaceBetween`] — first cell flush left, last flush right,
//!   inter-cell gaps grown evenly (a single-item row falls back to `Start`).
//! - [`WrapJustify::SpreadLine`] — like `SpaceBetween`, but the final row stays
//!   start-aligned like a paragraph's last line (AdwWrapBox `SPREAD`).
//!
//! Slack is computed from the resolved component width (not the widest
//! intrinsic row), so single-row `Center`/`End` genuinely centre / trail the
//! cells inside the parent, and every row justifies against the same width.
//! The frame clips (`clip: true`), and an empty inventory still reserves one
//! `item_height` of height.

use iced::advanced::layout::Measure as _;
use iced::advanced::renderer::Renderer as _;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

use crate::badge::{BadgeTone, enz_badge};
use enzui_theme::SpacingScale;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// One chip in an [`EnzWrap`] inventory: a title, a `selected` flag, an optional per-item
/// `tone` override, and an optional leading `icon`.
#[derive(Debug, Clone, Default)]
#[must_use]
pub struct EnzBadgeView {
    /// Chip label.
    pub title: String,
    /// Selected chips render with the accent tone unless a `tone` override wins.
    pub selected: bool,
    /// Per-item tone override. `None` falls back to selected-accent / the
    /// wrap-level tone (the `tone != "" ? tone : selected ? accent : root.tone`).
    pub tone: Option<BadgeTone>,
    /// Optional leading icon (empty = text-only).
    pub icon: String,
}

impl EnzBadgeView {
    /// A chip with the given title.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Self::default()
        }
    }

    /// Mark this chip selected (accent tone unless a `tone` override wins).
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Override this chip's tone (wins over `selected` and the wrap-level tone).
    pub fn tone(mut self, tone: BadgeTone) -> Self {
        self.tone = Some(tone);
        self
    }

    /// Set a leading icon by name (empty string omits it).
    pub fn icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = icon.into();
        self
    }

    /// Resolve the effective tone given the wrap-level default: per-item tone
    /// wins, else selected → accent, else the wrap-level tone .
    fn resolved_tone(&self, wrap_tone: BadgeTone) -> BadgeTone {
        self.tone.unwrap_or(if self.selected {
            BadgeTone::Accent
        } else {
            wrap_tone
        })
    }
}

// ---------------------------------------------------------------------------
// Justify
// ---------------------------------------------------------------------------

/// Per-row horizontal packing of the fixed-width cells within the leftover slack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WrapJustify {
    /// Cells packed at the leading edge (default).
    #[default]
    Start,
    /// Each row's cells centred within the available width.
    Center,
    /// Cells packed at the trailing edge.
    End,
    /// First cell flush left, last flush right, gaps grown evenly. A single-item
    /// row falls back to `Start`.
    SpaceBetween,
    /// Like `SpaceBetween`, but the final (short) row stays start-aligned like a
    /// paragraph's last line.
    SpreadLine,
}

impl WrapJustify {
    /// Whether cells on a row spread their slack into inter-cell gaps. `last` is
    /// true for the final row (`spreads`).
    fn spreads(self, last: bool) -> bool {
        matches!(self, WrapJustify::SpaceBetween)
            || (matches!(self, WrapJustify::SpreadLine) && !last)
    }
}

// ---------------------------------------------------------------------------
// Widget
// ---------------------------------------------------------------------------

/// A model-driven fixed-cell badge inventory. See the module docs.
#[must_use]
pub struct EnzWrap<'a, Message> {
    items: Vec<EnzBadgeView>,
    cells: Vec<Element<'a, Message>>,
    len: usize,
    // No `wrap_tone` field. `.tone(t)` rebuilds `cells` with `t`
    // immediately; the tone reaches the cells through that rebuild, not
    // through a stored copy. A stored `wrap_tone` read only by the setter's
    // own assignment would be a dead setter (stored, never read from
    // render/layout).
    item_width: f32,
    item_height: f32,
    spacing_x: f32,
    spacing_y: f32,
    justify: WrapJustify,
}

fn build_cells<'a, Message: 'a>(
    items: &[EnzBadgeView],
    wrap_tone: BadgeTone,
) -> Vec<Element<'a, Message>> {
    items
        .iter()
        .map(|it| {
            let mut b = enz_badge(&it.title).tone(it.resolved_tone(wrap_tone));
            if !it.icon.is_empty() {
                b = b.icon(it.icon.clone());
            }
            b.into_element()
        })
        .collect()
}

/// Build a fixed-cell badge inventory from a `[EnzBadgeView]` model.
pub fn enz_wrap<'a, Message>(items: Vec<EnzBadgeView>) -> EnzWrap<'a, Message>
where
    Message: 'a,
{
    // `sm` resolves here: it becomes `spacing_x` / `spacing_y`, which the wrap
    // widget consumes in its own `layout()` cell-geometry arithmetic (see the
    // column/row formulas in this module's docs). No `&Theme` exists there.
    let sm = SpacingScale::default().sm();
    let cells = build_cells(&items, BadgeTone::Default);
    EnzWrap {
        len: items.len(),
        items,
        cells,
        item_width: 96.0,
        item_height: 26.0,
        spacing_x: sm,
        spacing_y: sm,
        justify: WrapJustify::Start,
    }
}

impl<'a, Message> EnzWrap<'a, Message>
where
    Message: 'a,
{
    /// Set the wrap-level default tone — applied to non-selected chips that have
    /// no per-item tone override. Rebuilds the cells.
    ///
    /// The tone is not stored on the wrap: cells are rebuilt here, and the
    /// tone reaches paint and a11y through the rebuilt `EnzBadge` children.
    pub fn tone(mut self, tone: BadgeTone) -> Self {
        self.cells = build_cells(&self.items, tone);
        self
    }

    /// Fixed cell width in px (default 96). Non-finite / negative → 0.
    pub fn item_width(mut self, w: f32) -> Self {
        self.item_width = sanitize_len(w);
        self
    }

    /// Fixed cell height in px (default 26). Non-finite / negative → 0.
    pub fn item_height(mut self, h: f32) -> Self {
        self.item_height = sanitize_len(h);
        self
    }

    /// Horizontal spacing between cells (default spacing-sm = 8). Sanitised.
    pub fn spacing_x(mut self, s: f32) -> Self {
        self.spacing_x = sanitize_len(s);
        self
    }

    /// Vertical spacing between rows (default spacing-sm = 8). Sanitised.
    pub fn spacing_y(mut self, s: f32) -> Self {
        self.spacing_y = sanitize_len(s);
        self
    }

    /// Per-row justification (default `Start`).
    pub fn justify(mut self, justify: WrapJustify) -> Self {
        self.justify = justify;
        self
    }

    // ---- layout math ----

    fn columns(&self, width: f32) -> usize {
        if self.item_width <= 0.0 {
            return 1;
        }
        let c = ((width + self.spacing_x) / (self.item_width + self.spacing_x)).floor();
        (c as isize).max(1) as usize
    }

    fn rows(&self, columns: usize) -> usize {
        if self.len == 0 {
            0
        } else {
            self.len.div_ceil(columns)
        }
    }

    fn frame_height(&self, rows: usize) -> f32 {
        if self.len == 0 {
            self.item_height
        } else {
            rows as f32 * self.item_height + (rows.saturating_sub(1)) as f32 * self.spacing_y
        }
    }

    /// Cells in the row holding `index` (short only on the last row).
    fn row_count(&self, index: usize, columns: usize, rows: usize, last_row_count: usize) -> usize {
        if index / columns == rows - 1 {
            last_row_count
        } else {
            columns
        }
    }

    fn is_last_row(&self, index: usize, columns: usize, rows: usize) -> bool {
        index / columns == rows - 1
    }

    fn row_slack(&self, count: usize, width: f32) -> f32 {
        width - (count as f32 * self.item_width + (count.saturating_sub(1)) as f32 * self.spacing_x)
    }

    fn row_offset(&self, count: usize, last: bool, width: f32) -> f32 {
        if self.justify.spreads(last) {
            0.0
        } else {
            match self.justify {
                WrapJustify::Center => (self.row_slack(count, width) / 2.0).max(0.0),
                WrapJustify::End => self.row_slack(count, width).max(0.0),
                _ => 0.0,
            }
        }
    }

    fn row_gap(&self, count: usize, last: bool, width: f32) -> f32 {
        if self.justify.spreads(last) && count > 1 {
            (self.row_slack(count, width) / (count - 1) as f32).max(0.0)
        } else {
            0.0
        }
    }
}

fn sanitize_len(v: f32) -> f32 {
    if v.is_finite() { v.max(0.0) } else { 0.0 }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzWrap<'_, Message> {
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.cells);
    }

    /// An `EnzWrap` is never void — even empty it reserves one item's
    /// height.
    ///
    /// `is_void` lets a container elide a child *before layout runs*.
    /// That is exactly wrong here: this widget's documented contract (see the
    /// module docs, and the private `frame_height` helper) is that an empty
    /// inventory still reserves one `item_height` so a list that empties does not
    /// collapse and jump the surrounding UI: the empty wrap is a real
    /// one-item-tall box, not nothing. The reserved height is load-bearing, so
    /// honouring the contract wins over the layout slot it costs.
    ///
    /// Returning `all cells void` would report `true` for an empty model and let
    /// `Row`/`Column`/`Stack` remove the wrap before layout, silently defeating
    /// the reserved height. The invariant therefore requires `false`.
    fn is_void(&self) -> bool {
        false
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fill,
            height: Length::Shrink,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let limits = limits.width(Length::Fill);
        let max = limits.max();
        let width = if max.width.is_finite() {
            max.width
        } else {
            // Unbounded parent: fall back to one full row of cells.
            self.len.max(1) as f32 * self.item_width
                + (self.len.saturating_sub(1)) as f32 * self.spacing_x
        };

        let columns = self.columns(width);
        let rows = self.rows(columns);
        let last_row_count = if self.len == 0 {
            0
        } else {
            self.len - (rows - 1) * columns
        };
        let frame_h = self.frame_height(rows);

        // Precompute every cell's top-left (avoids borrowing self during the
        // cells iter_mut below).
        let placements: Vec<iced::Point> = (0..self.cells.len())
            .map(|index| {
                let count = self.row_count(index, columns, rows, last_row_count);
                let last = self.is_last_row(index, columns, rows);
                let col = index % columns;
                let row = index / columns;
                let x = self.row_offset(count, last, width)
                    + col as f32
                        * (self.item_width + self.spacing_x + self.row_gap(count, last, width));
                let y = row as f32 * (self.item_height + self.spacing_y);
                iced::Point::new(x, y)
            })
            .collect();

        // Fix the cell to *exactly* item_width × item_height.
        //
        // The previous limits were `Limits::new(Size::ZERO, Size::new(iw, ih))`
        // — a maximum with a zero minimum. A short chip ("A") would then lay
        // out at its natural 32x24 size inside a 120x40 slot, leaving the
        // painted background, borders, hover target and focus ring stranded
        // inside the slot pitch. Setting min == max forces `resolve()` to
        // clamp intrinsic child sizes UP to the slot size (see
        // `Limits::resolve_width/height`: `intrinsic.min(max).max(min)`).
        // `EnzBadge`'s outer `container` has `Length::Fixed(height)` and a
        // Shrink-ish min-width width, both of which route through `resolve()`
        // and honour the tightened `min`, so no change is needed in badge.rs.
        //
        // The wrap's own reported size stays `Size::new(width, frame_h)` —
        // it is derived from the parent's `max.width` and the row grid, not
        // from the children, so a Fill child cannot push it wider than the
        // parent bound.
        let cell_size = Size::new(self.item_width, self.item_height);
        let cell_limits = layout::Limits::new(cell_size, cell_size);

        let mut nodes = Vec::with_capacity(self.cells.len());
        for (index, cell) in self.cells.iter_mut().enumerate() {
            let child_node =
                cell.as_widget_mut()
                    .measure(&mut tree.children[index], renderer, &cell_limits);
            // Fix the cell to item_width × item_height and place it. The frame
            // clips in draw() , so overflow is contained.
            nodes.push(child_node.move_to(placements[index]));
        }

        // Reconcile the intrinsic frame size with the height the parent offered.
        // `frame_h` is the row-grid's intrinsic height; returning it raw from a
        // custom `layout` that never calls `resolve` would violate a finite
        // parent bound.
        //
        // Resolve as `Shrink`/`Shrink` against the already-computed
        // `intrinsic`, not against the widget's advertised `Length::Fill`
        // width. That is deliberate: `resolve_width(Fill, ..)` returns
        // `max.width` unconditionally, which is `∞` under an unbounded parent
        // and would destroy the one-full-row fallback computed above by
        // degrading a valid infinite max into a bogus concrete size. With
        // `Shrink`, `resolve_width` returns `width.min(max.width).max(min)`:
        //   - finite max: `width == max.width` → unchanged;
        //   - infinite max: `width.min(∞)` → the fallback, unchanged.
        // Height likewise: `frame_h.min(max.height).max(min.height)` honours a
        // finite height bound literally while leaving the unbounded case at the
        // intrinsic `frame_h`.
        let intrinsic = Size::new(width, frame_h);
        let size = limits.resolve(Length::Shrink, Length::Shrink, intrinsic);

        layout::Node::with_children(size, nodes).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        // Clip operations to the visible frame so focus traversal
        // and accessibility collection cannot reach cells that `draw` clips
        // away. Same pattern as `AdaptiveRow::operate` and `InertClip`.
        let clip = layout.bounds();
        let mut clipped = crate::inert_clip::ClipOperation::new(operation, clip);
        for (cell, (lay, state)) in self
            .cells
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
        {
            cell.as_widget_mut()
                .operate(state, lay, viewport, renderer, &mut clipped);
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // A cell visually clipped by the frame must not receive
        // pointer events. Levitate the cursor when it is outside the frame
        // so every child sees it as off-widget.
        let cursor = crate::inert_clip::cursor_within(cursor, layout.bounds());
        for (cell, (lay, state)) in self
            .cells
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
        {
            cell.as_widget_mut()
                .update(state, event, lay, cursor, renderer, shell, viewport);
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        // Clip the whole inventory to its frame .
        let bounds = layout.bounds();
        let Some(clip) = bounds.intersection(viewport) else {
            return;
        };
        renderer.with_layer(clip, |renderer| {
            for (cell, (lay, state)) in self.cells.iter().zip(layout.iter(&tree.children)) {
                cell.as_widget()
                    .draw(state, renderer, theme, style, lay, cursor, &clip);
            }
        });
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        // Match the draw clip so the cursor shape cannot come from
        // a cell that is visually clipped away.
        let cursor = crate::inert_clip::cursor_within(cursor, layout.bounds());
        self.cells
            .iter()
            .zip(layout.iter(&tree.children))
            .map(|(cell, (lay, state))| {
                cell.as_widget()
                    .mouse_interaction(state, lay, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        overlay::from_children(
            &mut self.cells,
            tree,
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message> EnzWrap<'a, Message>
where
    Message: 'a,
{
    /// Finish the builder and hand back the [`Element`], which is step 3 of
    /// the build pattern every widget in this crate follows (`docs/widgets.md`).
    ///
    /// This forwards to the `From<Self> for Element` impl below, so the two
    /// spellings are one implementation and cannot drift apart. Prefer this
    /// one inside `row![]` / `column![]`, where `.into()` has no target type
    /// to infer.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzWrap<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(w: EnzWrap<'a, Message>) -> Self {
        Element::new(w)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {

    // ---- column / row derivation ----

    // ---- justify slack math ----

    // ---- model precedence ----

    // ---- sanitisation ----

    // ---- The returned node honours the offered height -------------------------

    // ---- Dead setters: `.tone(t)` must reach paint/a11y -----------------
    //
    // An `EnzWrap` with a stored `wrap_tone` field that nothing reads is the
    // defect shape this test guards against. The setter's only *live* work is
    // `self.cells = build_cells(&items, t)`, and without a rendering test
    // the stored value could silently rot. The a11y label
    // (`badge_semantics(tone)` -> `"warning badge"` / `"error badge"` /
    // etc, via `EnzBadgeView::resolved_tone`) is the cheapest observable
    // proof that the tone reaches the cells and, through them, the surface.
}
