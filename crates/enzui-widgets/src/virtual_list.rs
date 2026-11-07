//! Keyed scrolling rows with viewport-bounded realization and clipped height slots.
//!
//! Rows are built eagerly using [`EnzVirtualList::viewport_height`] (default
//! 600 px), scroll offset and overscan (default five items per side). The caller
//! must supply a viewport height matching layout. Spacers reserve unrealized
//! content height; stable provider keys retain row state across index changes.
//! Iterator construction consumes all heights; [`enz_virtual_list_with_geometry`]
//! reuses borrowed or reference-counted geometry without rebuilding its prefix sums.
//!
//! Owned providers need only survive row construction; finished elements retain
//! row content. Cache entries survive reorder by key, but callers must invalidate
//! entries when content changes behind a stable key. Measurement indices belong
//! to the sequence that produced them: apply before reordering, or discard them.

use crate::row_measure::RowMeasurement;
use crate::virtual_rows::{RealisedRow, RowChrome, realise_keyed_rows};
pub use crate::virtual_scroll::{RevealAlignment, reconcile_measurements};
use enzui_core::virtual_sequence::{
    HeightModel, ItemCache, KeyedAnchor, KeyedProvider, RealizationWindow, SequenceGeometry,
    SequenceKey, VariableHeightModel,
};
use iced::widget::scrollable;
use iced::widget::scrollable::Viewport;
use iced::{Element, Length, Padding};
use std::cell::RefCell;
use std::rc::Rc;
// A missing provider key falls back to positional identity for that row.
fn resolved_row_key<Item>(provider: &dyn KeyedProvider<Item>, index: u64) -> String {
    provider
        .key(index)
        .map_or_else(|| format!("index::{index}"), |k| k.as_str().to_owned())
}

/// Borrowed or reference-counted row source used during element construction.
/// Borrowed providers must outlive the returned element's declared lifetime; an
/// owned provider permits returning rows derived locally inside a view.
pub enum ProviderHandle<'a, Item> {
    /// Borrow the caller's provider for this build.
    Borrowed(&'a dyn KeyedProvider<Item>),
    /// Own the provider through row construction, then release it.
    Owned(Rc<dyn KeyedProvider<Item> + 'a>),
}

impl<'a, Item> ProviderHandle<'a, Item> {
    fn as_provider(&self) -> &dyn KeyedProvider<Item> {
        match self {
            ProviderHandle::Borrowed(p) => *p,
            ProviderHandle::Owned(p) => p.as_ref(),
        }
    }
}

impl<'a, Item, P> From<&'a P> for ProviderHandle<'a, Item>
where
    P: KeyedProvider<Item> + 'a,
{
    fn from(provider: &'a P) -> Self {
        ProviderHandle::Borrowed(provider)
    }
}

impl<'a, Item> From<&'a dyn KeyedProvider<Item>> for ProviderHandle<'a, Item> {
    fn from(provider: &'a dyn KeyedProvider<Item>) -> Self {
        ProviderHandle::Borrowed(provider)
    }
}

impl<'a, Item, P> From<Rc<P>> for ProviderHandle<'a, Item>
where
    P: KeyedProvider<Item> + 'a,
{
    fn from(provider: Rc<P>) -> Self {
        ProviderHandle::Owned(provider)
    }
}

impl<'a, Item> From<Rc<dyn KeyedProvider<Item> + 'a>> for ProviderHandle<'a, Item> {
    fn from(provider: Rc<dyn KeyedProvider<Item> + 'a>) -> Self {
        ProviderHandle::Owned(provider)
    }
}
/// Geometry retained by the caller, borrowed or shared during row construction.
/// Its order and count must match the provider. Models keep responsibility for
/// valid offsets and heights; [`VariableHeightModel`] sanitizes input heights.
pub enum GeometryHandle<'a> {
    /// Borrow a retained geometry model.
    Borrowed(&'a dyn SequenceGeometry),
    /// Share a retained geometry model without copying its prefix data.
    Owned(Rc<dyn SequenceGeometry + 'a>),
}
impl<'a, G: SequenceGeometry + 'a> From<&'a G> for GeometryHandle<'a> {
    fn from(geometry: &'a G) -> Self {
        Self::Borrowed(geometry)
    }
}
impl<'a> From<&'a dyn SequenceGeometry> for GeometryHandle<'a> {
    fn from(geometry: &'a dyn SequenceGeometry) -> Self {
        Self::Borrowed(geometry)
    }
}
impl<'a, G: SequenceGeometry + 'a> From<Rc<G>> for GeometryHandle<'a> {
    fn from(geometry: Rc<G>) -> Self {
        Self::Owned(geometry)
    }
}
impl<'a> From<Rc<dyn SequenceGeometry + 'a>> for GeometryHandle<'a> {
    fn from(geometry: Rc<dyn SequenceGeometry + 'a>) -> Self {
        Self::Owned(geometry)
    }
}
enum HeightMode<'a> {
    Uniform(HeightModel),
    Variable(VariableHeightModel),
    Retained(GeometryHandle<'a>),
}

impl HeightMode<'_> {
    fn geom(&self) -> &dyn SequenceGeometry {
        match self {
            HeightMode::Uniform(m) => m,
            HeightMode::Variable(m) => m,
            HeightMode::Retained(GeometryHandle::Borrowed(m)) => *m,
            HeightMode::Retained(GeometryHandle::Owned(m)) => m.as_ref(),
        }
    }
}
type RowBuilder<'a, Message, Item> = Box<dyn Fn(u64, &Item) -> Element<'a, Message> + 'a>;

/// Scrolling list that eagerly builds only its visible and overscan rows.
/// Uniform and variable-height constructors share keyed row reconciliation.
/// Keep stable logical keys across reorder, and retain any [`ItemCache`] across
/// views. Cache hits clone items; invalidate entries when their keyed content changes.
#[must_use]
pub struct EnzVirtualList<'a, Message, Item: Clone> {
    mode: HeightMode<'a>,
    provider: ProviderHandle<'a, Item>,
    row_builder: RowBuilder<'a, Message, Item>,
    overscan: usize,
    width: Length,
    height: Length,
    on_scroll: Option<Box<dyn Fn(Viewport) -> Message + 'a>>,
    viewport_height: f32,
    scroll_offset: f32,
    cache: Option<&'a RefCell<ItemCache<Item>>>,
    cache_capacity: Option<usize>,
    self_scroll: Option<crate::scroll_view::ScrollTranslation>,
    id: Option<iced::widget::Id>,
    keyboard_scroll: bool,
    scrollbar_tokens: enzui_theme::ScrollbarTokens,
    scrollbar_insets: iced::Padding,
    on_measure: Option<Rc<dyn Fn(RowMeasurement) -> Message + 'a>>,
}

/// Build uniform-height rows from a borrowed or owned provider.
/// Non-finite or negative item heights become zero; zero has no content extent.
pub fn enz_virtual_list<'a, Message, Item, F>(
    provider: impl Into<ProviderHandle<'a, Item>>,
    item_height: f32,
    row_builder: F,
) -> EnzVirtualList<'a, Message, Item>
where
    Message: 'a,
    Item: Clone + 'a,
    F: Fn(u64, &Item) -> Element<'a, Message> + 'a,
{
    let provider = provider.into();
    let count = provider.as_provider().len() as usize;
    EnzVirtualList {
        mode: HeightMode::Uniform(HeightModel::new(count, item_height)),
        provider,
        row_builder: Box::new(row_builder),
        overscan: 5,
        width: Length::Fill,
        height: Length::Fill,
        on_scroll: None,
        viewport_height: 600.0,
        scroll_offset: 0.0,
        cache: None,
        cache_capacity: None,
        self_scroll: None,
        id: None,
        keyboard_scroll: true,
        scrollbar_tokens: enzui_theme::ScrollbarTokens::DEFAULT,
        scrollbar_insets: iced::Padding::ZERO,
        on_measure: None,
    }
}

/// Build variable-height rows from a borrowed or owned provider.
/// Heights must match provider order and count; construction consumes the entire
/// sequence. Non-finite or negative heights become zero.
pub fn enz_virtual_list_variable<'a, Message, Item, F>(
    provider: impl Into<ProviderHandle<'a, Item>>,
    heights: impl IntoIterator<Item = f32>,
    row_builder: F,
) -> EnzVirtualList<'a, Message, Item>
where
    Message: 'a,
    Item: Clone + 'a,
    F: Fn(u64, &Item) -> Element<'a, Message> + 'a,
{
    EnzVirtualList {
        mode: HeightMode::Variable(VariableHeightModel::new(heights)),
        provider: provider.into(),
        row_builder: Box::new(row_builder),
        overscan: 5,
        width: Length::Fill,
        height: Length::Fill,
        on_scroll: None,
        viewport_height: 600.0,
        scroll_offset: 0.0,
        cache: None,
        cache_capacity: None,
        self_scroll: None,
        id: None,
        keyboard_scroll: true,
        scrollbar_tokens: enzui_theme::ScrollbarTokens::DEFAULT,
        scrollbar_insets: iced::Padding::ZERO,
        on_measure: None,
    }
}

/// Build rows using existing geometry without enumerating heights or copying the model.
///
/// Pass `&VariableHeightModel` or `Rc<VariableHeightModel>` retained in application
/// state. Provider keys, anchors, caches and measurement receipts use the same
/// contracts as [`enz_virtual_list_variable`]. Geometry must match provider order
/// and count. Borrowed geometry shares the element's declared lifetime.
pub fn enz_virtual_list_with_geometry<'a, Message, Item, F>(
    provider: impl Into<ProviderHandle<'a, Item>>,
    geometry: impl Into<GeometryHandle<'a>>,
    row_builder: F,
) -> EnzVirtualList<'a, Message, Item>
where
    Message: 'a,
    Item: Clone + 'a,
    F: Fn(u64, &Item) -> Element<'a, Message> + 'a,
{
    let mut list = enz_virtual_list(provider, 0.0, row_builder);
    list.mode = HeightMode::Retained(geometry.into());
    list
}

impl<'a, Message, Item: Clone> EnzVirtualList<'a, Message, Item> {
    /// Set extra items per side of the visible range; defaults to five.
    /// Zero is literal, and the range clamps at sequence boundaries.
    pub fn overscan(mut self, n: usize) -> Self {
        self.overscan = n;
        self
    }

    /// Set container width; defaults to [`Length::Fill`].
    pub fn width(mut self, w: Length) -> Self {
        self.width = w;
        self
    }

    /// Set container height; defaults to [`Length::Fill`].
    /// Self-scrolling overrides this with a fixed [`Self::viewport_height`].
    pub fn height(mut self, h: Length) -> Self {
        self.height = h;
        self
    }

    /// Set realization height in logical pixels; defaults to 600.
    /// Keep it synchronized with layout, for example using [`Self::on_scroll`].
    /// The callback does not update this builder automatically. Invalid or zero
    /// heights realize no rows.
    pub fn viewport_height(mut self, h: f32) -> Self {
        self.viewport_height = h;
        self
    }

    /// Set the realization offset in logical pixels; defaults to zero.
    /// Positive offsets also seed newly created scrollable state, including inert
    /// backgrounds. Existing state continues its own scrolling; coordinate updates
    /// with scroll operations or use [`Self::self_scrolling`].
    pub fn scroll_offset(mut self, offset: f32) -> Self {
        self.scroll_offset = offset;
        self
    }

    /// Use a retained [`ScrollTranslation`](crate::scroll_view::ScrollTranslation)
    /// for the realization offset, overriding [`Self::scroll_offset`]. Offset changes
    /// request rebuilding even while the application is idle. The scrollable height
    /// is fixed to [`Self::viewport_height`], which the caller must keep current.
    pub fn self_scrolling(mut self, cell: crate::scroll_view::ScrollTranslation) -> Self {
        self.self_scroll = Some(cell);
        self
    }

    /// Stable `Id` on the internal scrollable so `scroll_to` / `scroll_by`
    /// can target it.
    pub fn id(mut self, id: iced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Receive vertical offset or viewport-extent changes as iced [`Viewport`].
    /// Width-only relayout does not fire the callback. The caller must apply any
    /// new offset and height to subsequent list builds.
    pub fn on_scroll(mut self, f: impl Fn(Viewport) -> Message + 'a) -> Self {
        self.on_scroll = Some(Box::new(f));
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

    /// Enable PageUp/PageDown/Home/End scrolling; defaults to enabled.
    /// Control, Alt and Logo leave keys for the application; Shift is accepted.
    /// Arrows and Space are unhandled. Scrolling retains the focused row.
    pub fn keyboard_scroll(mut self, enabled: bool) -> Self {
        self.keyboard_scroll = enabled;
        self
    }

    /// Report intrinsic heights for realized rows on redraw.
    /// This installs measurement for either constructor, but only variable-height
    /// models accept corrections through [`reconcile_measurements`]. Retain corrected
    /// heights and offset in application state. Reports use frame-local indices: apply
    /// before changing sequence order, or discard them. Unchanged reports are not retried.
    pub fn on_measure(mut self, f: impl Fn(RowMeasurement) -> Message + 'a) -> Self {
        self.on_measure = Some(Rc::new(f));
        self
    }

    /// Resolve rows through a retained caller-owned [`ItemCache`].
    /// Hits clone; misses fetch and insert. An initially zero-capacity cache adopts
    /// three realized windows on its first nonempty build, unless overridden by
    /// [`Self::cache_capacity`]. The derived capacity does not grow automatically
    /// with later viewport changes. Borrowing is transient per row.
    pub fn cache(mut self, cache: &'a RefCell<ItemCache<Item>>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Explicit cache capacity, overriding the visible-window default. `0`
    /// is literal: no caching, every row resolved per frame, rendering
    /// unaffected.
    pub fn cache_capacity(mut self, capacity: usize) -> Self {
        self.cache_capacity = Some(capacity);
        self
    }
}

/// Return a clamped absolute offset revealing a stable key under the alignment.
/// Returns `None` if the key is absent or its index is outside the geometry.
/// Use a targeted scroll operation and update realization offset together.
/// Key lookup uses the provider's reverse index, whose default scans all rows.
#[must_use]
pub fn reveal_offset<Item>(
    provider: &dyn KeyedProvider<Item>,
    geometry: &(impl SequenceGeometry + ?Sized),
    key: &SequenceKey,
    viewport_height: f32,
    current_scroll_top: f32,
    alignment: RevealAlignment,
) -> Option<f32> {
    let index = usize::try_from(provider.index_of(key)?).ok()?;
    crate::virtual_scroll::reveal_offset_at(
        geometry,
        index,
        viewport_height,
        current_scroll_top,
        alignment,
    )
}

/// Capture a stable key and intra-row pixel offset before changing sequence order.
/// Provider and geometry must describe the same sequence. Returns `None` when
/// the geometry-derived index has no provider key. Resolve after mutation with
/// [`resolve_anchor`] and update both realization and scrolling.
#[must_use]
pub fn capture_anchor<Item>(
    provider: &dyn KeyedProvider<Item>,
    geometry: &(impl SequenceGeometry + ?Sized),
    scroll_top: f32,
) -> Option<KeyedAnchor> {
    KeyedAnchor::capture(provider, geometry, scroll_top)
}

/// Resolve a captured key against current provider order and geometry.
/// Returns `None` if the key was removed; the caller chooses a fallback.
/// The returned offset clamps below zero but not to the scrollable maximum.
#[must_use]
pub fn resolve_anchor<Item>(
    anchor: &KeyedAnchor,
    provider: &dyn KeyedProvider<Item>,
    geometry: &(impl SequenceGeometry + ?Sized),
) -> Option<f32> {
    Some(anchor.scroll_top(provider, geometry)?.max(0.0))
}
// Reserve nearby windows so small scroll-back gestures can reuse items.
const DEFAULT_CACHE_WINDOWS: usize = 3;

impl<'a, Message, Item: Clone + 'a> EnzVirtualList<'a, Message, Item>
where
    Message: Clone + 'a,
{
    /// Eagerly build keyed, clipped row slots inside a vertical scrollable.
    /// Leading and trailing spacers preserve full content extent. Missing provider
    /// items leave empty slots. The realization viewport is caller-supplied, not
    /// measured during layout. Extreme f32 content coordinates can lose row precision.
    pub fn into_element(mut self) -> Element<'a, Message> {
        let cache = self.cache.take();
        let on_measure = self.on_measure.take();
        let requested_capacity = self.cache_capacity;
        let row_builder = self.row_builder;
        let provider_handle = self.provider;
        let provider = provider_handle.as_provider();
        let geom = self.mode.geom();
        let effective_offset = match &self.self_scroll {
            Some(cell) => cell.get().y,
            None => self.scroll_offset,
        };
        let realized = RealizationWindow::new(self.overscan).realize(
            geom,
            &enzui_core::virtual_sequence::Anchor::from_scroll_top(geom, effective_offset),
            self.viewport_height,
        );
        if let Some(cache) = cache {
            let mut c = cache.borrow_mut();
            match requested_capacity {
                Some(n) => {
                    if c.capacity() != n {
                        c.set_capacity(n);
                    }
                }
                None => {
                    if c.capacity() == 0 && !realized.is_empty() {
                        c.set_capacity(realized.len().saturating_mul(DEFAULT_CACHE_WINDOWS));
                    }
                }
            }
        }
        let col = realise_keyed_rows(
            geom,
            self.viewport_height,
            effective_offset,
            self.overscan,
            RowChrome {
                padding: Padding::ZERO,
                on_measure,
            },
            |index, _height| {
                let resolved = match cache {
                    Some(cache) => cache.borrow_mut().item(provider, index),
                    None => provider.item(index),
                }?;
                Some(RealisedRow {
                    key: resolved_row_key(provider, index),
                    content: row_builder(index, &resolved),
                })
            },
        );
        // An unbounded scrollable would make viewport equal content and clamp offsets to zero.
        let sb_height = if self.self_scroll.is_some() {
            Length::Fixed(self.viewport_height)
        } else {
            self.height
        };
        let bar = crate::scroll_view::overlay_scrollbar_with_tokens(self.scrollbar_tokens);
        let mut sb = crate::scroll_view::enz_scrollable(col)
            .direction(scrollable::Direction::Vertical(bar))
            .scrollbar_insets(self.scrollbar_insets)
            .width(self.width)
            .height(sb_height);
        // Seed only newly created scroll state; retained gesture state owns later offsets.
        if self.self_scroll.is_none() && self.scroll_offset > 0.0 {
            sb = sb.initial_offset(scrollable::AbsoluteOffset {
                x: 0.0,
                y: self.scroll_offset,
            });
        }

        if let Some(id) = self.id {
            sb = sb.id(id);
        }
        if let Some(f) = self.on_scroll {
            sb = sb.on_vertical_scroll(move |scroll: iced::widget::scrollable::Scroll| {
                f(scroll.viewport)
            });
        }
        let inner: Element<'a, Message> = match self.self_scroll {
            Some(cell) => crate::scroll_view::publish_translation_of(sb, cell),
            None => sb.into(),
        };
        if self.keyboard_scroll {
            crate::scroll_view::keyboard_scroll_of(inner, crate::scroll_view::Axis::Vertical)
        } else {
            crate::scroll_view::contain_wheel(inner, crate::scroll_view::Axis::Vertical)
        }
    }
}

impl<'a, Message: Clone + 'a, Item: Clone + 'a> From<EnzVirtualList<'a, Message, Item>>
    for Element<'a, Message>
{
    fn from(vl: EnzVirtualList<'a, Message, Item>) -> Self {
        vl.into_element()
    }
}
