//! Provider-backed virtual grid with fixed tile heights and adaptive columns.
//!
//! Layout realises visible rows plus pixel overscan and reserves the remaining
//! height with spacers. Columns come from the grid's allocation, or from a
//! usable [`EnzVirtualGrid::resolved`] value supplied by the caller. Rows have
//! fixed tile height plus vertical gap, including a trailing gap on the last
//! row. Missing provider items are omitted from the row rather than reserving
//! empty cells; following cells in that row move left.
//!
//! Retain an [`ItemCache`] in application state and lend it through
//! [`EnzVirtualGrid::cache`] to reuse resources across layout passes. Without
//! a cache the provider is called for every realised item each pass. Eviction
//! drops the cache's references; resources held by providers, rendered elements
//! or other clones remain alive. Builders receive borrowed tiles but can clone
//! them, so bounded cache size alone does not prove bounded resource retention.
//!
//! [`EnzVirtualGrid::self_scrolling`] publishes actual scroll offsets and requests
//! rebuilds when they change. Programmatic scroll operations still need a stable
//! [`EnzVirtualGrid::id`]. Writing the translation cell alone does not move the
//! mounted scrollable: its next publication overwrites the cell with the actual
//! offset. Reflow and reveal calculate a realisation offset and write that cell;
//! neither issues a scroll operation. The host must also move the scrollable
//! when applying those offsets to the displayed viewport.
//!
//! [`EnzVirtualGrid::reflow_anchor`] retains a keyed anchor across column changes;
//! [`EnzVirtualGrid::reveal_key`] consumes a pending keyed reveal once. Their
//! cells must survive rebuilds. [`EnzVirtualGrid::semantics`] adds a named grid
//! and realised grid-cell nodes with logical position/count; selection remains
//! caller-owned through [`EnzVirtualGrid::selection`]. Tile trees reconcile
//! positionally; keyed caching and selection do not give tile widgets keyed
//! retained state. Variable tile heights are unsupported.

use std::cell::RefCell;
use std::rc::Rc;

use enzui_core::semantics::{Role, Semantics, SetPosition};
use enzui_core::virtual_grid::{Reflow, ResolvedTileGrid, TileGrid, TileProvider};
use enzui_core::virtual_sequence::{Anchor, ItemCache, KeyedAnchor, PixelOverscan};
use iced::widget::scrollable::Viewport;
use iced::widget::{Column, Row, Space, container, responsive, scrollable};
use iced::{Element, Length};

type TileBuilder<'a, Message, Tile> = Box<dyn Fn(u64, &Tile) -> Element<'a, Message> + 'a>;

const DEFAULT_CACHE_WINDOWS: usize = 3;

const DEFAULT_OVERSCAN_PX: f32 = 64.0;

/// A virtualised, adaptive spatial grid over a [`TileProvider`].
///
/// Construct with [`enz_virtual_grid`]. See the module docs for the resource
/// ownership contract, which is the reason this widget exists.
#[must_use]
pub struct EnzVirtualGrid<'a, Message, Tile: Clone> {
    provider: &'a dyn TileProvider<Tile>,
    grid: TileGrid,
    adaptive_stretch: bool,
    resolved_geometry: Option<&'a std::cell::Cell<enzui_core::virtual_grid::ResolvedTileGrid>>,
    preresolved: Option<ResolvedTileGrid>,
    tile_builder: TileBuilder<'a, Message, Tile>,
    overscan_px: f32,
    viewport_height: f32,
    scroll_offset: f32,
    cache: Option<&'a RefCell<ItemCache<Tile>>>,
    cache_capacity: Option<usize>,
    self_scroll: Option<crate::scroll_view::ScrollTranslation>,
    id: Option<iced::widget::Id>,
    keyboard_scroll: bool,
    scrollbar_tokens: enzui_theme::ScrollbarTokens,
    scrollbar_insets: iced::Padding,
    reflow_anchor: Option<&'a RefCell<Option<KeyedAnchor>>>,
    reflow_columns: Option<&'a std::cell::Cell<usize>>,
    reveal: Option<&'a RefCell<Option<enzui_core::virtual_sequence::SequenceKey>>>,
    selection: Option<&'a dyn Fn(&enzui_core::virtual_sequence::SequenceKey) -> bool>,
    semantics: bool,
    label: Option<String>,
    width: Length,
    height: Length,
    on_scroll: Option<Rc<dyn Fn(Viewport) -> Message + 'a>>,
}

/// Construct a virtual grid over fixed-size provider tiles.
///
/// The builder runs for each available realised tile on every layout pass.
/// Keep decoding in the provider and retain an [`ItemCache`] across views to
/// reuse resources; the builder borrows each fetched or cached tile.
pub fn enz_virtual_grid<'a, Message, Tile, F>(
    provider: &'a dyn TileProvider<Tile>,
    grid: TileGrid,
    tile_builder: F,
) -> EnzVirtualGrid<'a, Message, Tile>
where
    Message: 'a,
    Tile: Clone + 'a,
    F: Fn(u64, &Tile) -> Element<'a, Message> + 'a,
{
    EnzVirtualGrid {
        provider,
        grid,
        adaptive_stretch: false,
        resolved_geometry: None,
        preresolved: None,
        tile_builder: Box::new(tile_builder),
        overscan_px: DEFAULT_OVERSCAN_PX,
        viewport_height: 600.0,
        scroll_offset: 0.0,
        cache: None,
        cache_capacity: None,
        self_scroll: None,
        id: None,
        keyboard_scroll: true,
        scrollbar_tokens: enzui_theme::text_defaults::scrollbar(),
        scrollbar_insets: iced::Padding::ZERO,
        reflow_anchor: None,
        reflow_columns: None,
        reveal: None,
        selection: None,
        semantics: false,
        label: None,
        width: Length::Fill,
        height: Length::Fill,
        on_scroll: None,
    }
}

impl<'a, Message, Tile: Clone> EnzVirtualGrid<'a, Message, Tile> {
    /// Resolve columns and equal tile widths from the widget's own allocated
    /// width and publish that single answer to the caller-owned cell. The cell
    /// can then drive marquee, thumbnail scheduling and keyboard movement
    /// without a second window-width calculation.
    pub fn adaptive_stretch(
        mut self,
        resolved: &'a std::cell::Cell<enzui_core::virtual_grid::ResolvedTileGrid>,
    ) -> Self {
        self.adaptive_stretch = true;
        self.resolved_geometry = Some(resolved);
        self
    }

    /// Use caller-resolved columns and tile geometry for this build.
    ///
    /// A value satisfying [`ResolvedTileGrid::is_resolved`] takes precedence over
    /// local resolution and [`Self::adaptive_stretch`]. Otherwise layout resolves
    /// from its own allocation. If a geometry publication cell is configured, it
    /// receives the result actually used. This avoids re-dividing stretched tile
    /// widths under a slightly different child allocation.
    pub fn resolved(mut self, resolved: ResolvedTileGrid) -> Self {
        self.preresolved = Some(resolved);
        self
    }

    /// Set vertical overscan per viewport edge in logical pixels (default 64).
    /// Zero, negative and nonfinite values add no overscan.
    pub fn overscan_px(mut self, px: f32) -> Self {
        self.overscan_px = px;
        self
    }

    /// Set fallback realisation height in logical pixels (default 600).
    /// Used only when allocated height is negative or nonfinite. Zero allocation
    /// realises nothing; invalid or nonpositive fallback values also yield no rows.
    pub fn viewport_height(mut self, h: f32) -> Self {
        self.viewport_height = h;
        self
    }

    /// Set the realisation offset in logical pixels (default zero).
    ///
    /// [`Self::self_scrolling`] overrides it. Without that cell, a positive offset
    /// also seeds newly created scrollable state; changing it does not reposition
    /// an existing mounted scrollable. Use ID-targeted scroll operations for that.
    pub fn scroll_offset(mut self, offset: f32) -> Self {
        self.scroll_offset = offset;
        self
    }

    /// Keep realization synchronized with scrolling without application messages.
    ///
    /// Retain the same [`ScrollTranslation`](crate::scroll_view::ScrollTranslation)
    /// across views. Changed offsets request widget rebuilds, as with
    /// [`EnzVirtualList::self_scrolling`](crate::virtual_list::EnzVirtualList::self_scrolling).
    pub fn self_scrolling(mut self, cell: crate::scroll_view::ScrollTranslation) -> Self {
        self.self_scroll = Some(cell);
        self
    }

    /// Attach a stable `iced::widget::Id` to the internal scrollable so it can
    /// be targeted by `scroll_to`/`scroll_by`.
    pub fn id(mut self, id: iced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Callback when the grid is scrolled — delivers the iced `Viewport`.
    pub fn on_scroll(mut self, f: impl Fn(Viewport) -> Message + 'a) -> Self {
        self.on_scroll = Some(Rc::new(f));
        self
    }

    /// Override scrollbar geometry tokens for the internal scrollable.
    pub fn scrollbar_tokens(mut self, tokens: enzui_theme::ScrollbarTokens) -> Self {
        self.scrollbar_tokens = tokens.normalized();
        self
    }

    /// Inset scrollbar paint and pointer targets without padding scroll content.
    /// Defaults to zero. Only top and bottom apply to this vertical track.
    /// Values are logical pixels; negative and nonfinite
    /// values become zero. Oversized insets clamp to the available track,
    /// leading edge first; collapsed tracks cannot be grabbed.
    /// Use [`crate::app_shell::ShellGeometry::scrollbar_safe_area`] in a
    /// geometry-aware shell content closure to clear floating chrome.
    pub fn scrollbar_insets(mut self, insets: impl Into<iced::Padding>) -> Self {
        self.scrollbar_insets = insets.into();
        self
    }

    /// Enable widget-owned PageUp/PageDown/Home/End scrolling (default true).
    /// Page distance comes from the laid-out viewport; Home/End move to the
    /// scrollable extent's ends. Ctrl/Alt/Logo chords are left to the host;
    /// Shift is accepted. Space and arrow keys are not handled. Focus ownership
    /// and edge handoff follow [`crate::scroll_view::EnzScrollView::keyboard_scroll`].
    /// Paging does not change tile focus or selection.
    pub fn keyboard_scroll(mut self, enabled: bool) -> Self {
        self.keyboard_scroll = enabled;
        self
    }

    /// Set outer width (default [`Length::Fill`]).
    pub fn width(mut self, w: Length) -> Self {
        self.width = w;
        self
    }

    /// Set outer height (default [`Length::Fill`]).
    pub fn height(mut self, h: Length) -> Self {
        self.height = h;
        self
    }

    /// Reuse tiles through a caller-owned keyed [`ItemCache`] retained across views.
    ///
    /// Tile fetches occur on cache misses; successful tiles are cloned from
    /// cache entries, and missing results are not cached. Explicit
    /// [`Self::cache_capacity`] wins over [`TileProvider::retention_hint`], including
    /// zero from either source. Otherwise existing nonzero capacity is preserved;
    /// a zero-capacity cache is initialized from the first nonempty realised
    /// window at three times its item count. This automatic bound is not resized
    /// on later viewport changes. Eviction drops cached references, not clones
    /// held elsewhere.
    ///
    /// # Panics
    ///
    /// Layout panics if the cache is already borrowed or provider lookup
    /// attempts to borrow it again while the grid holds its mutable borrow.
    pub fn cache(mut self, cache: &'a RefCell<ItemCache<Tile>>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Set the cache's capacity explicitly, overriding both the provider hint
    /// and the window-derived default. `0` is honoured literally.
    pub fn cache_capacity(mut self, capacity: usize) -> Self {
        self.cache_capacity = Some(capacity);
        self
    }

    /// Calculate keyed realisation compensation when the column count changes.
    ///
    /// Retain both cells across views, initially `None` and zero. Each pass captures
    /// the first item in the top visible row; a column change resolves the prior
    /// key through [`TileGrid::reflow`] before capturing a new anchor. Its current
    /// formula subtracts the stored intra-row offset from the new row top, clamped
    /// at zero; it does not preserve the original signed viewport displacement.
    /// The item can also occupy a different column. Sequence changes at the same
    /// column count do not trigger compensation. Failed captures retain the prior
    /// anchor.
    ///
    /// The result drives the realised band and, when configured, writes the
    /// [`Self::self_scrolling`] cell. It does not move the mounted scrollable; the
    /// host must apply a matching scroll operation for viewport compensation.
    pub fn reflow_anchor(
        mut self,
        anchor: &'a RefCell<Option<KeyedAnchor>>,
        columns: &'a std::cell::Cell<usize>,
    ) -> Self {
        self.reflow_anchor = Some(anchor);
        self.reflow_columns = Some(columns);
        self
    }

    /// Consume a pending stable-key reveal and calculate its realisation offset.
    ///
    /// Requires [`Self::self_scrolling`]; without it the request remains pending.
    /// Requests are taken once per layout, including absent or already-visible
    /// keys. A successful reveal writes the translation cell and realises the
    /// matching band, but does not issue a scroll operation to the mounted control.
    /// Use the grid ID to apply the target offset to the displayed viewport.
    /// Reflow anchor capture precedes reveal handling in the same pass.
    pub fn reveal_key(
        mut self,
        pending: &'a RefCell<Option<enzui_core::virtual_sequence::SequenceKey>>,
    ) -> Self {
        self.reveal = Some(pending);
        self
    }

    /// Publish caller-owned keyed selection through realised cell semantics.
    ///
    /// Requires [`Self::semantics`]. The predicate runs only for available realised
    /// tiles whose provider returns a key; it does not paint selection or retain
    /// a selected set inside the grid.
    pub fn selection(
        mut self,
        is_selected: &'a dyn Fn(&enzui_core::virtual_sequence::SequenceKey) -> bool,
    ) -> Self {
        self.selection = Some(is_selected);
        self
    }

    /// Publish a named Grid group and realised GridCell descendants.
    ///
    /// Each cell reports its one-based logical item position and total provider
    /// count, including unavailable items. Selected state is supplied by
    /// [`Self::selection`]; this adds neither keyboard tile navigation nor stable
    /// widget IDs. Semantics are disabled by default.
    pub fn semantics(mut self, label: impl Into<String>) -> Self {
        self.semantics = true;
        self.label = Some(label.into());
        self
    }
}

impl<'a, Message, Tile> EnzVirtualGrid<'a, Message, Tile>
where
    Message: Clone + 'a,
    Tile: Clone + 'a,
{
    /// Build a grid that realises the visible band plus configured overscan.
    ///
    /// A responsive layout closure rebuilds the body on each layout pass, using
    /// shared provider/cache/callback handles that remain usable across relayouts.
    pub fn into_element(self) -> Element<'a, Message> {
        let EnzVirtualGrid {
            provider,
            grid,
            adaptive_stretch,
            resolved_geometry,
            preresolved,
            tile_builder,
            overscan_px,
            viewport_height,
            scroll_offset,
            cache,
            cache_capacity,
            self_scroll,
            id,
            keyboard_scroll,
            reflow_anchor,
            reflow_columns,
            reveal,
            selection,
            semantics,
            label,
            width,
            height,
            on_scroll,
            scrollbar_tokens,
            scrollbar_insets,
        } = self;

        let body = responsive(move |size| {
            // Re-resolving stretched tiles can disagree with the caller's column allocation.
            let resolved = match preresolved {
                Some(caller) if caller.is_resolved() => caller,
                _ if adaptive_stretch => grid.stretch_for_width(size.width),
                _ => ResolvedTileGrid {
                    width: size.width,
                    columns: grid.columns_for_width(size.width),
                    tile_width: grid.tile_width(),
                    tile_height: grid.tile_height(),
                    gap_x: grid.gap_x(),
                    gap_y: grid.gap_y(),
                },
            };
            if let Some(cell) = resolved_geometry {
                cell.set(resolved);
            }
            let grid = resolved.grid();
            let columns = resolved.columns;
            let count = provider.len();
            let rows = grid.row_model(count, columns);

            // Infinite allocations need a finite fallback to avoid realising the entire collection.
            let viewport_h = if size.height.is_finite() && size.height >= 0.0 {
                size.height
            } else {
                viewport_height
            };

            let offset = match &self_scroll {
                Some(cell) => cell.get().y,
                None => scroll_offset,
            };

            let offset = match (reflow_anchor, reflow_columns) {
                (Some(anchor_cell), Some(prev_columns)) => {
                    let previous = prev_columns.get();
                    // A steady column count must leave reader scrolling in control.
                    let compensated = if previous != 0 && previous != columns {
                        anchor_cell
                            .borrow()
                            .as_ref()
                            .and_then(|a| grid.reflow(provider, a, columns))
                            .map(|r: Reflow| r.scroll_top)
                    } else {
                        None
                    };
                    let effective = compensated.unwrap_or(offset);
                    if let (Some(cell), Some(v)) = (&self_scroll, compensated) {
                        cell.set(iced::Vector::new(0.0, v));
                    }
                    if columns > 0 {
                        let fresh = grid.capture(provider, columns, effective);
                        if fresh.is_some() {
                            *anchor_cell.borrow_mut() = fresh;
                        }
                    }
                    prev_columns.set(columns);
                    effective
                }
                _ => offset,
            };

            let offset = match (reveal, &self_scroll) {
                (Some(pending), Some(cell)) => {
                    // Release the request borrow before provider lookup; each request is one-shot.
                    let requested = pending.borrow_mut().take();
                    match requested
                        .as_ref()
                        .and_then(|key| provider.index_of(key))
                        .and_then(|index| grid.reveal(index, count, columns, offset, viewport_h))
                    {
                        Some(target) => {
                            cell.set(iced::Vector::new(0.0, target));
                            target
                        }
                        None => offset,
                    }
                }
                _ => offset,
            };

            let anchor = Anchor::from_scroll_top(&rows, offset);
            let band = PixelOverscan::new(overscan_px).realize(&rows, &anchor, viewport_h);

            let items = grid.items_in_rows(band.clone(), columns, count);

            if let Some(cache) = cache {
                let mut c = cache.borrow_mut();
                let window =
                    usize::try_from(items.end.saturating_sub(items.start)).unwrap_or(usize::MAX);
                match cache_capacity.or_else(|| provider.retention_hint()) {
                    Some(n) => {
                        if c.capacity() != n {
                            c.set_capacity(n);
                        }
                    }
                    None => {
                        // An empty layout must not adopt a zero retention bound.
                        if c.capacity() == 0 && window > 0 {
                            c.set_capacity(window.saturating_mul(DEFAULT_CACHE_WINDOWS));
                        }
                    }
                }
            }

            let tile_w = Length::Fixed(grid.tile_width());
            let tile_h = Length::Fixed(grid.tile_height());
            let pitch = grid.row_pitch();

            let mut children: Vec<Element<'_, Message>> = Vec::with_capacity(band.len() + 2);

            let leading = rows.offset_of(band.start);
            if leading > 0.0 {
                children.push(
                    Space::new()
                        .unrealized()
                        .width(Length::Fill)
                        .height(leading)
                        .into(),
                );
            }

            for row in band.clone() {
                let row_items = grid.items_in_rows(row..row + 1, columns, count);
                let realized = usize::try_from(row_items.end.saturating_sub(row_items.start))
                    .unwrap_or(usize::MAX);
                let mut cells: Vec<Element<'_, Message>> = Vec::with_capacity(
                    enzui_core::metrics::RowMajorColumns::new(columns, realized).occupied(),
                );
                for index in row_items {
                    // End the cache borrow before calling application tile builders.
                    let resolved = match cache {
                        Some(cache) => cache.borrow_mut().item(provider, index),
                        None => provider.item(index),
                    };
                    if let Some(tile) = resolved {
                        let cell: Element<'_, Message> = container(tile_builder(index, &tile))
                            .width(tile_w)
                            .height(tile_h)
                            .into();
                        cells.push(if semantics {
                            let position = SetPosition {
                                index: usize::try_from(index.saturating_add(1))
                                    .unwrap_or(usize::MAX),
                                count: usize::try_from(count).unwrap_or(usize::MAX),
                            };
                            let mut cell_semantics =
                                Semantics::new(Role::GridCell).set_position(position);
                            if let Some(is_selected) = selection
                                && let Some(key) = provider.key(index)
                            {
                                cell_semantics = cell_semantics.selected(is_selected(&key));
                            }
                            crate::semantics_container::SemanticsContainer::new(
                                cell,
                                cell_semantics,
                            )
                            .into()
                        } else {
                            cell
                        });
                    }
                }
                children.push(
                    container(Row::with_children(cells).spacing(grid.gap_x()))
                        .width(Length::Fill)
                        // Put gaps inside row pitches; Column spacing would also shift the spacers.
                        .height(Length::Fixed(pitch))
                        .into(),
                );
            }

            let used = leading + band.len() as f32 * pitch;
            let trailing = (rows.total_height() - used).max(0.0);
            if trailing > 0.0 {
                children.push(
                    Space::new()
                        .unrealized()
                        .width(Length::Fill)
                        .height(trailing)
                        .into(),
                );
            }

            let column = Column::with_children(children).width(Length::Fill);
            let bar = crate::scroll_view::overlay_scrollbar_with_tokens(scrollbar_tokens);
            let mut sb = crate::scroll_view::enz_scrollable(column)
                .direction(scrollable::Direction::Vertical(bar))
                .scrollbar_insets(scrollbar_insets)
                .width(Length::Fill)
                .height(Length::Fill);
            // Seed fresh state only; mounted offsets remain owned by gestures and operations.
            if self_scroll.is_none() && scroll_offset > 0.0 {
                sb = sb.initial_offset(scrollable::AbsoluteOffset {
                    x: 0.0,
                    y: scroll_offset,
                });
            }
            if let Some(id) = id.clone() {
                sb = sb.id(id);
            }
            if let Some(f) = &on_scroll {
                let f = Rc::clone(f);
                sb = sb
                    .on_scroll(move |scroll: iced::widget::scrollable::Scroll| f(scroll.viewport));
            }
            let inner: Element<'_, Message> = match &self_scroll {
                Some(cell) => crate::scroll_view::publish_translation_of(sb, cell.clone()),
                None => sb.into(),
            };
            let scrolled = if keyboard_scroll {
                crate::scroll_view::keyboard_scroll_of(inner, crate::scroll_view::Axis::Vertical)
            } else {
                crate::scroll_view::contain_wheel(inner, crate::scroll_view::Axis::Vertical)
            };
            if semantics {
                crate::semantics_container::SemanticsContainer::new(
                    scrolled,
                    Semantics::new(Role::Grid).label(label.clone().unwrap_or_default()),
                )
                .into()
            } else {
                scrolled
            }
        });

        container(body).width(width).height(height).into()
    }
}

impl<'a, Message, Tile> From<EnzVirtualGrid<'a, Message, Tile>> for Element<'a, Message>
where
    Message: Clone + 'a,
    Tile: Clone + 'a,
{
    fn from(g: EnzVirtualGrid<'a, Message, Tile>) -> Self {
        g.into_element()
    }
}
