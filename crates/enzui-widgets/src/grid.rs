//! Fixed-column, row-major layouts with shared column tracks.
//!
//! [`enz_grid`] sizes each column to its widest child across all rows by
//! default; a column whose children fill ([`Length::Fill`] or
//! [`Length::FillPortion`] width) takes a share of the width the other
//! columns leave, by portion and never narrower than its content.
//! [`EnzGrid::stretch_cells`] instead divides the available width equally
//! between the declared columns. Both modes give children tight width
//! limits; a short final row keeps the same tracks without placeholder children.
//! Content-derived tracks can together exceed the available width; the grid
//! does not clip children to their cells.
//!
//! Zero columns become one. Storage depends on the children rather than the
//! declared column count, but all declared columns still contribute gaps and,
//! when stretching, equal width shares, including unoccupied columns.
//!
//! Defaults are 8-pixel row and column gaps, zero padding, [`Length::Fill`]
//! width and [`Length::Shrink`] height. Gaps and insets accept raw pixels;
//! negative or non-finite values become zero. Each row takes its tallest
//! child's height. An empty grid has zero intrinsic height, including when
//! vertical padding is set.
//!
//! [`EnzGrid`] reconciles children by position. [`KeyedGrid`] always uses
//! equal-width columns and reconciles children by key to retain state across
//! reorderings and responsive reflow.

use iced::Event;
use iced::advanced::layout;
use iced::advanced::layout::Measure as _;
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::Operation;
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::{Layout, Shell, Widget};
use iced::{Element, Length, Rectangle, Size, Vector};

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Build a content-sized column grid from row-major children.
///
/// Zero columns become one; see the [module documentation](self) for defaults.
pub fn enz_grid<'a, Message>(
    columns: usize,
    children: Vec<Element<'a, Message>>,
) -> EnzGrid<'a, Message>
where
    Message: 'a,
{
    EnzGrid {
        columns,
        children,
        row_gap: 8.0,
        col_gap: 8.0,
        pad_left: 0.0,
        pad_right: 0.0,
        pad_top: 0.0,
        pad_bottom: 0.0,
        width: Length::Fill,
        height: Length::Shrink,
        stretch_cells: false,
    }
}

/// Builder returned by [`enz_grid`].
#[must_use]
pub struct EnzGrid<'a, Message> {
    columns: usize,
    children: Vec<Element<'a, Message>>,
    row_gap: f32,
    col_gap: f32,
    pad_left: f32,
    pad_right: f32,
    pad_top: f32,
    pad_bottom: f32,
    width: Length,
    height: Length,
    stretch_cells: bool,
}

fn sane_gap(px: f32) -> f32 {
    enzui_core::metrics::FiniteNonNeg::new_or(px, 0.0).get()
}

// Grid columns and form labels share the same Shrink-width measurement.
pub(crate) fn measure_intrinsic_size<Message>(
    child: &mut Element<'_, Message>,
    tree: &mut Tree,
    renderer: &iced::Renderer,
    bounds: Size,
) -> Size {
    let limits = layout::Limits::new(Size::ZERO, bounds).width(Length::Shrink);
    child
        .as_widget_mut()
        .measure(tree, renderer, &limits)
        .size()
}

impl<'a, Message> EnzGrid<'a, Message>
where
    Message: 'a,
{
    /// Set both row and column gaps in pixels.
    pub fn gap(self, px: f32) -> Self {
        let px = sane_gap(px);
        self.row_gap(px).col_gap(px)
    }

    /// Set the vertical (row-to-row) gap in pixels.
    pub fn row_gap(mut self, px: f32) -> Self {
        self.row_gap = sane_gap(px);
        self
    }

    /// Set the horizontal (cell-to-cell) gap in pixels.
    pub fn col_gap(mut self, px: f32) -> Self {
        self.col_gap = sane_gap(px);
        self
    }

    /// Set all four edge insets in pixels.
    pub fn padding(self, px: f32) -> Self {
        let px = sane_gap(px);
        self.padding_x(px).padding_y(px)
    }

    /// Set horizontal (left + right) insets in pixels.
    pub fn padding_x(mut self, px: f32) -> Self {
        let px = sane_gap(px);
        self.pad_left = px;
        self.pad_right = px;
        self
    }

    /// Set vertical (top + bottom) insets in pixels.
    pub fn padding_y(mut self, px: f32) -> Self {
        let px = sane_gap(px);
        self.pad_top = px;
        self.pad_bottom = px;
        self
    }

    /// Set per-edge insets in pixels.
    pub fn paddings(mut self, left: f32, right: f32, top: f32, bottom: f32) -> Self {
        self.pad_left = sane_gap(left);
        self.pad_right = sane_gap(right);
        self.pad_top = sane_gap(top);
        self.pad_bottom = sane_gap(bottom);
        self
    }

    /// Override the outer width (default [`Length::Fill`]).
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Override the outer height (default [`Length::Shrink`]).
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Divide the padded width, minus column gaps, equally between columns.
    ///
    /// Defaults to `false`: each column uses its widest measured child, and
    /// columns of filling children share the width left over.
    /// Both modes give children tight width limits.
    pub fn stretch_cells(mut self, on: bool) -> Self {
        self.stretch_cells = on;
        self
    }
}

// ---------------------------------------------------------------------------
// EnzGrid: custom coordinated layout with shared column tracks
// ---------------------------------------------------------------------------

/// Give the columns whose children fill (`fills[c] > 0`) the width the other
/// columns' content leaves of `room`, by portion, never narrower than their
/// own content: a fill column wider than its share keeps its content width
/// and the rest share what remains. Unbounded room leaves content widths.
fn share_fill_columns(tracks: &mut [f32], fills: &[u16], room: f32) {
    if !room.is_finite() {
        return;
    }
    let mut open: Vec<usize> = (0..tracks.len()).filter(|&c| fills[c] > 0).collect();
    let fixed: f32 = (0..tracks.len())
        .filter(|&c| fills[c] == 0)
        .map(|c| tracks[c])
        .sum();
    let mut left = (room - fixed).max(0.0);
    while !open.is_empty() {
        let portions: f32 = open.iter().map(|&c| f32::from(fills[c])).sum();
        let unit = left / portions;
        let (wide, narrow): (Vec<usize>, Vec<usize>) = open
            .iter()
            .partition(|&&c| tracks[c] > unit * f32::from(fills[c]));
        if wide.is_empty() {
            for c in narrow {
                tracks[c] = unit * f32::from(fills[c]);
            }
            return;
        }
        left = (left - wide.iter().map(|&c| tracks[c]).sum::<f32>()).max(0.0);
        open = narrow;
    }
}
//
impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzGrid<'_, Message> {
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let limits = limits.width(self.width).height(self.height);
        let max = limits.max();

        let avail_w = (max.width - self.pad_left - self.pad_right).max(0.0);
        let content_max_h = (max.height - self.pad_top - self.pad_bottom).max(0.0);

        if self.children.is_empty() {
            let inner = Size::new(avail_w, 0.0);
            let size = limits.resolve(self.width, self.height, inner);
            return layout::Node::with_children(size, vec![]).apply(tree);
        }

        // Gaps, placement and content width follow the declared count; only the
        // reachable columns size the per-column vectors.
        let n = self.children.len();
        let cols = enzui_core::metrics::RowMajorColumns::new(self.columns, n);
        let columns = cols.declared();
        let occupied = cols.occupied();
        let rows = n.div_ceil(columns);
        let total_gap_w = self.col_gap * (columns as f32 - 1.0).max(0.0);

        // Track widths for the reachable columns, and the width the unreachable
        // declared ones still add: an equal share each when stretching, nothing
        // when tracks come from content.
        let (col_w, empty_tracks_w): (Vec<f32>, f32) = if self.stretch_cells {
            let cell_w = ((avail_w - total_gap_w) / columns as f32).max(0.0);
            (vec![cell_w; occupied], cell_w * (columns - occupied) as f32)
        } else {
            let mut tracks = vec![0.0f32; occupied];
            // The largest fill portion any child of each column asks for.
            let mut fills = vec![0u16; occupied];
            for (i, child) in self.children.iter_mut().enumerate() {
                let c = i % columns;
                fills[c] = fills[c].max(child.as_widget().size().width.fill_factor());
                let w = measure_intrinsic_size(
                    child,
                    &mut tree.children[i],
                    renderer,
                    Size::new(avail_w.max(0.0), content_max_h),
                )
                .width;
                if w.is_finite() && w > tracks[c] {
                    tracks[c] = w;
                }
            }
            share_fill_columns(&mut tracks, &fills, avail_w - total_gap_w);
            (tracks, 0.0)
        };

        let mut col_x = vec![0.0f32; occupied];
        {
            let mut x = self.pad_left;
            for (x_slot, w) in col_x.iter_mut().zip(col_w.iter()) {
                *x_slot = x;
                x += w + self.col_gap;
            }
        }

        let mut child_nodes: Vec<layout::Node> = Vec::with_capacity(n);
        let mut row_heights: Vec<f32> = vec![0.0f32; rows];
        for (i, child) in self.children.iter_mut().enumerate() {
            let c = i % columns;
            let cw = col_w[c];
            let cell_limits = layout::Limits::new(Size::new(cw, 0.0), Size::new(cw, content_max_h));
            let node = child
                .as_widget_mut()
                .measure(&mut tree.children[i], renderer, &cell_limits);
            let r = i / columns;
            row_heights[r] = row_heights[r].max(node.size().height);
            child_nodes.push(node);
        }

        let mut positioned = Vec::with_capacity(n);
        let mut y = self.pad_top;
        for (r, rh) in row_heights.iter().enumerate() {
            for (c, x) in col_x.iter().enumerate() {
                // Checked arithmetic prevents wrapping onto an already-positioned child.
                let Some(i) = r.checked_mul(columns).and_then(|base| base.checked_add(c)) else {
                    break;
                };
                if i >= n {
                    break;
                }
                positioned
                    .push(std::mem::take(&mut child_nodes[i]).move_to(iced::Point::new(*x, y)));
            }
            y += rh + self.row_gap;
        }

        let content_h: f32 = row_heights.iter().sum::<f32>()
            + self.row_gap * (rows as f32 - 1.0).max(0.0)
            + self.pad_top
            + self.pad_bottom;
        let tracks_w: f32 = col_w.iter().sum::<f32>() + empty_tracks_w + total_gap_w;
        let content_w = (tracks_w + self.pad_left + self.pad_right).max(0.0);

        let size = limits.resolve(self.width, self.height, Size::new(content_w, content_h));
        layout::Node::with_children(size, positioned).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            for (child, (lay, state)) in self
                .children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
            {
                child
                    .as_widget_mut()
                    .operate(state, lay, viewport, renderer, operation);
            }
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        for (child, (lay, state)) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
        {
            child
                .as_widget_mut()
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
        cursor: iced::advanced::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        for (child, (lay, state)) in self.children.iter().zip(layout.iter(&tree.children)) {
            child
                .as_widget()
                .draw(state, renderer, theme, style, lay, cursor, viewport);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
        self.children
            .iter()
            .zip(layout.iter(&tree.children))
            .map(|(child, (lay, state))| {
                child
                    .as_widget()
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
            &mut self.children,
            tree,
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message> EnzGrid<'a, Message>
where
    Message: 'a,
{
    /// Convert the builder into an [`Element`] without relying on type inference.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzGrid<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(g: EnzGrid<'a, Message>) -> Self {
        Element::new(g)
    }
}

// ---------------------------------------------------------------------------
// KeyedGrid: flat keyed grid with stable identity across reflow
// ---------------------------------------------------------------------------

/// An equal-width column grid that retains child state by reconciliation key.
///
/// State follows each key through insertions, removals, reorderings and changes
/// to the column count. Reusing a key for a different widget type resets its
/// state. Duplicate keys match in occurrence order; prefer unique keys.
///
/// Used by [`enz_responsive_grid_keyed`](crate::responsive_grid::enz_responsive_grid_keyed).
#[must_use]
pub struct KeyedGrid<'a, Message> {
    columns: usize,
    keys: Vec<String>,
    children: Vec<Element<'a, Message>>,
    col_gap: f32,
    row_gap: f32,
    pad_left: f32,
    pad_right: f32,
    pad_top: f32,
    pad_bottom: f32,
    width: Length,
    height: Length,
    allow_duplicate_keys: bool,
}

struct KeyedGridState {
    keys: Vec<String>,
}

/// Build an equal-width column grid from keyed, row-major children.
///
/// Zero columns become one. Defaults are 8-pixel gaps, zero padding,
/// [`Length::Fill`] width and [`Length::Shrink`] height. Negative or non-finite
/// gaps and insets become zero.
///
/// # Panics
///
/// In debug builds, reconciliation panics on duplicate keys unless
/// [`KeyedGrid::allow_duplicate_keys`] is set. Duplicates match in occurrence
/// order in release builds.
pub fn enz_keyed_grid<'a, Message>(
    columns: usize,
    keyed_children: Vec<(String, Element<'a, Message>)>,
) -> KeyedGrid<'a, Message>
where
    Message: 'a,
{
    let (keys, children): (Vec<String>, Vec<Element<'a, Message>>) =
        keyed_children.into_iter().unzip();
    KeyedGrid {
        columns: columns.max(1),
        keys,
        children,
        col_gap: 8.0,
        row_gap: 8.0,
        pad_left: 0.0,
        pad_right: 0.0,
        pad_top: 0.0,
        pad_bottom: 0.0,
        width: Length::Fill,
        height: Length::Shrink,
        allow_duplicate_keys: false,
    }
}

impl<'a, Message> KeyedGrid<'a, Message>
where
    Message: 'a,
{
    /// Disable the debug assertion for duplicate keys.
    ///
    /// Duplicates still reconcile in occurrence order.
    pub fn allow_duplicate_keys(mut self) -> Self {
        self.allow_duplicate_keys = true;
        self
    }

    /// Set both row and column gaps in pixels.
    pub fn gap(self, px: f32) -> Self {
        let px = sane_gap(px);
        self.row_gap(px).col_gap(px)
    }

    /// Set the vertical (row-to-row) gap in pixels.
    pub fn row_gap(mut self, px: f32) -> Self {
        self.row_gap = sane_gap(px);
        self
    }

    /// Set the horizontal (cell-to-cell) gap in pixels.
    pub fn col_gap(mut self, px: f32) -> Self {
        self.col_gap = sane_gap(px);
        self
    }

    /// Set all four edge insets in pixels.
    pub fn padding(self, px: f32) -> Self {
        let px = sane_gap(px);
        self.padding_x(px).padding_y(px)
    }

    /// Set horizontal (left + right) insets in pixels.
    pub fn padding_x(mut self, px: f32) -> Self {
        let px = sane_gap(px);
        self.pad_left = px;
        self.pad_right = px;
        self
    }

    /// Set vertical (top + bottom) insets in pixels.
    pub fn padding_y(mut self, px: f32) -> Self {
        let px = sane_gap(px);
        self.pad_top = px;
        self.pad_bottom = px;
        self
    }

    /// Override the outer width (default [`Length::Fill`]).
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Override the outer height (default [`Length::Shrink`]).
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for KeyedGrid<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<KeyedGridState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(KeyedGridState {
            keys: self.keys.clone(),
        })
    }

    fn diff(&mut self, tree: &mut Tree) {
        debug_assert!(
            self.allow_duplicate_keys || crate::keyed_diff::duplicate_keys(&self.keys).is_empty(),
            "KeyedGrid: duplicate reconciliation key(s) {:?} in {:?} — duplicates \
             reconcile positionally among themselves, which is the bug keying \
             removes. Disambiguate with enzui_core::identity::KeyedIds, or \
             call .allow_duplicate_keys() if this is deliberate.",
            crate::keyed_diff::duplicate_keys(&self.keys),
            self.keys,
        );

        let state = tree.state.downcast_mut::<KeyedGridState>();

        crate::keyed_diff::reconcile_children_by_key(
            &mut tree.children,
            &state.keys,
            &self.keys,
            &mut self.children,
            |child_tree, child| child.as_widget_mut().diff(child_tree),
            |child| child.as_widget().tag(),
            |child| Tree::new(child.as_widget()),
        );

        if state.keys != self.keys {
            state.keys.clone_from(&self.keys);
        }
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let limits = limits.width(self.width).height(self.height);
        let max = limits.max();

        if self.children.is_empty() {
            let inner = Size::new((max.width - self.pad_left - self.pad_right).max(0.0), 0.0);
            let size = limits.resolve(self.width, self.height, inner);
            return layout::Node::with_children(size, vec![]).apply(tree);
        }

        let columns = self.columns;
        let n = self.children.len();
        let rows = n.div_ceil(columns);
        let avail_w = (max.width - self.pad_left - self.pad_right).max(0.0);

        let total_gap_w = self.col_gap * (columns as f32 - 1.0).max(0.0);
        let cell_w = ((avail_w - total_gap_w) / columns as f32).max(0.0);
        let cell_limits =
            layout::Limits::new(Size::new(cell_w, 0.0), Size::new(cell_w, max.height));

        let mut child_nodes = Vec::with_capacity(n);
        let mut row_heights: Vec<f32> = vec![0.0f32; rows];
        for (i, child) in self.children.iter_mut().enumerate() {
            let node = child
                .as_widget_mut()
                .measure(&mut tree.children[i], renderer, &cell_limits);
            let r = i / columns;
            row_heights[r] = row_heights[r].max(node.size().height);
            child_nodes.push(node);
        }

        let mut y_offset = self.pad_top;
        let mut positioned = Vec::with_capacity(n);
        for (r, rh) in row_heights.iter().enumerate() {
            let mut x_offset = self.pad_left;
            for c in 0..columns {
                let i = r * columns + c;
                if i >= n {
                    break;
                }
                positioned.push(
                    child_nodes[i]
                        .clone()
                        .move_to(iced::Point::new(x_offset, y_offset)),
                );
                x_offset += cell_w + self.col_gap;
            }
            y_offset += rh + self.row_gap;
        }

        let content_h: f32 = row_heights.iter().sum::<f32>()
            + self.row_gap * (rows as f32 - 1.0).max(0.0)
            + self.pad_top
            + self.pad_bottom;

        let size = limits.resolve(self.width, self.height, Size::new(max.width, content_h));

        layout::Node::with_children(size, positioned).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            for (child, (lay, state)) in self
                .children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
            {
                child
                    .as_widget_mut()
                    .operate(state, lay, viewport, renderer, operation);
            }
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        for (child, (lay, state)) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
        {
            child
                .as_widget_mut()
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
        cursor: iced::advanced::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        for (child, (lay, state)) in self.children.iter().zip(layout.iter(&tree.children)) {
            child
                .as_widget()
                .draw(state, renderer, theme, style, lay, cursor, viewport);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
        self.children
            .iter()
            .zip(layout.iter(&tree.children))
            .map(|(child, (lay, state))| {
                child
                    .as_widget()
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
            &mut self.children,
            tree,
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message> KeyedGrid<'a, Message>
where
    Message: 'a,
{
    /// Convert the builder into an [`Element`] without relying on type inference.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message> From<KeyedGrid<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(grid: KeyedGrid<'a, Message>) -> Self {
        Element::new(grid)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
