//! Virtual-sequence geometry, keyed anchoring, realization, placement, and item provisioning.
//!
//! Uniform geometry stores constant state; variable geometry stores `len + 1` f64 prefix edges. Lookup uses half-open intervals. Rounded coordinates use f32, while explicit unrounded edges and placement arithmetic retain f64. Zero dimensions are literal; non-finite and negative dimensions degrade to zero.

use core::ops::Range;

mod item_cache;
pub use item_cache::ItemCache;

/// Constant-storage geometry for a uniformly sized sequence, with logarithmic index lookup and viewport-bounded ranges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeightModel {
    len: usize,
    item_height: f64,
}

pub(crate) fn sane_dim(v: f32) -> f64 {
    if v.is_finite() && v > 0.0 {
        f64::from(v)
    } else {
        0.0
    }
}

impl HeightModel {
    /// Creates a uniform model, mapping non-finite or negative heights to zero and preserving literal zero.
    #[must_use]
    pub fn new(count: usize, item_height: f32) -> Self {
        HeightModel {
            len: count,
            item_height: sane_dim(item_height),
        }
    }

    /// The logical item count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the sequence holds no items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the sanitized uniform item height.
    #[must_use]
    pub fn item_height(&self) -> f32 {
        self.item_height as f32
    }

    /// Returns the total content height in logical pixels.
    #[must_use]
    pub fn total_height(&self) -> f32 {
        self.total_height_f64() as f32
    }

    fn total_height_f64(&self) -> f64 {
        self.item_height * self.len as f64
    }

    // Use f32-rounded public edges so index lookup agrees exactly with offset_of.
    fn edge(&self, index: usize) -> f64 {
        f64::from(self.offset_of(index))
    }

    fn partition_point(&self, pred: impl Fn(f64) -> bool) -> usize {
        let mut lo = 0_usize;
        let mut hi = self.len;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if pred(self.edge(mid)) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Returns an item’s top edge, clamped to the content end for past-end indices.
    #[must_use]
    pub fn offset_of(&self, index: usize) -> f32 {
        self.offset_of_f64(index) as f32
    }

    /// Returns the item’s top edge before conversion to f32, clamped to the content end.
    #[must_use]
    pub fn offset_of_f64(&self, index: usize) -> f64 {
        index.min(self.len) as f64 * self.item_height
    }

    /// Returns the item whose half-open interval contains `offset`.
    ///
    /// Returns `None` for empty or zero-height models, invalid offsets, and offsets at or beyond the content end.
    #[must_use]
    pub fn index_at(&self, offset: f32) -> Option<usize> {
        if self.len == 0 || self.item_height == 0.0 || !offset.is_finite() || offset < 0.0 {
            return None;
        }
        let offset = f64::from(offset);
        if offset >= self.total_height_f64() {
            return None;
        }
        // Select the last top edge at or below the offset; validated bounds guarantee a predecessor.
        let first_above = self.partition_point(|edge| edge <= offset);
        debug_assert!(first_above >= 1, "edge(0) is 0 and offset >= 0");
        Some(first_above - 1)
    }

    /// Returns the half-open item range intersecting the viewport.
    ///
    /// Invalid or zero viewport heights and zero-extent models yield `0..0`; invalid tops start at zero; finite tops at or past the content end yield `len..len`.
    #[must_use]
    pub fn range_for(&self, viewport_top: f32, viewport_height: f32) -> Range<usize> {
        let height = sane_dim(viewport_height);
        if self.len == 0 || self.item_height == 0.0 || height == 0.0 {
            return 0..0;
        }
        let top = if viewport_top.is_finite() && viewport_top > 0.0 {
            f64::from(viewport_top)
        } else {
            0.0
        };
        let total = self.total_height_f64();
        if top >= total {
            // A finite top beyond content returns `len..len` to preserve the empty range’s position.
            return self.len..self.len;
        }
        let bottom = (top + height).min(total);
        let start = self.partition_point(|edge| edge <= top).saturating_sub(1);
        // Exclude an item whose top edge equals the half-open viewport bottom.
        let end = self.partition_point(|edge| edge < bottom);
        start..end.clamp(start, self.len)
    }
}

pub(crate) fn partition_point_over(
    len: usize,
    edge: impl Fn(usize) -> f64,
    pred: impl Fn(f64) -> bool,
) -> usize {
    let mut lo = 0_usize;
    let mut hi = len;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if pred(edge(mid)) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

/// Prefix-sum geometry for variable-height items.
///
/// It stores `len + 1` f64 edges, uses logarithmic lookup, and follows the same half-open and sanitization rules as [`HeightModel`].
#[derive(Debug, Clone, PartialEq)]
pub struct VariableHeightModel {
    // `prefix[i]` is item i’s top edge; the nonempty f64 vector is monotonic and ends with total height.
    prefix: Vec<f64>,
}

impl VariableHeightModel {
    /// Creates a model from ordered heights, mapping each non-finite or negative value to zero.
    #[must_use]
    pub fn new(heights: impl IntoIterator<Item = f32>) -> Self {
        let iter = heights.into_iter();
        let (lower, _) = iter.size_hint();
        let mut prefix = Vec::with_capacity(lower + 1);
        let mut acc = 0.0_f64;
        prefix.push(acc);
        for h in iter {
            acc += sane_dim(h);
            prefix.push(acc);
        }
        VariableHeightModel { prefix }
    }

    /// Creates a variable model with `count` equal initial heights. This allocates `count + 1` prefix edges.
    #[must_use]
    pub fn uniform(count: usize, item_height: f32) -> Self {
        let h = sane_dim(item_height);
        let mut prefix = Vec::with_capacity(count + 1);
        let mut acc = 0.0_f64;
        prefix.push(acc);
        for _ in 0..count {
            acc += h;
            prefix.push(acc);
        }
        VariableHeightModel { prefix }
    }

    /// The logical item count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.prefix.len() - 1
    }

    /// Whether the sequence holds no items.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns the stored item height, or `0.0` for a past-end index.
    #[must_use]
    pub fn height_of(&self, index: usize) -> f32 {
        if index >= self.len() {
            return 0.0;
        }
        (self.prefix[index + 1] - self.prefix[index]) as f32
    }

    /// Returns the sum of all sanitized item heights.
    #[must_use]
    pub fn total_height(&self) -> f32 {
        self.total_height_f64() as f32
    }

    fn total_height_f64(&self) -> f64 {
        self.prefix[self.len()]
    }

    // Search over f32-rounded public edges so lookup agrees with offset_of.
    fn edge(&self, index: usize) -> f64 {
        f64::from(self.offset_of(index))
    }

    /// Returns an item’s top edge, clamped to the content end.
    #[must_use]
    pub fn offset_of(&self, index: usize) -> f32 {
        self.offset_of_f64(index) as f32
    }

    /// Returns an item’s stored f64 top edge, clamped to the content end.
    #[must_use]
    pub fn offset_of_f64(&self, index: usize) -> f64 {
        self.prefix[index.min(self.len())]
    }

    /// Returns the item whose half-open interval contains `offset`.
    ///
    /// Zero-height rows own no offset. Invalid offsets, zero total height, and offsets at or beyond the content end return `None`.
    #[must_use]
    pub fn index_at(&self, offset: f32) -> Option<usize> {
        if self.is_empty() || !offset.is_finite() || offset < 0.0 {
            return None;
        }
        let offset = f64::from(offset);
        if offset >= self.total_height_f64() {
            return None;
        }
        // Shared top edges select the last row, skipping zero-height intervals that own no offset.
        let first_above = partition_point_over(self.len(), |i| self.edge(i), |edge| edge <= offset);
        debug_assert!(first_above >= 1, "edge(0) is 0 and offset >= 0");
        Some(first_above - 1)
    }

    /// Returns the half-open item range intersecting the viewport.
    ///
    /// Invalid or zero viewport heights and zero-total models yield `0..0`; invalid tops start at zero; finite tops at or past the end yield `len..len`.
    #[must_use]
    pub fn range_for(&self, viewport_top: f32, viewport_height: f32) -> Range<usize> {
        let height = sane_dim(viewport_height);
        let total = self.total_height_f64();
        // Return `0..0` for zero total height to match a zero-height uniform model.
        if self.is_empty() || total == 0.0 || height == 0.0 {
            return 0..0;
        }
        let top = if viewport_top.is_finite() && viewport_top > 0.0 {
            f64::from(viewport_top)
        } else {
            0.0
        };
        if top >= total {
            return self.len()..self.len();
        }
        let bottom = (top + height).min(total);
        let len = self.len();
        let start =
            partition_point_over(len, |i| self.edge(i), |edge| edge <= top).saturating_sub(1);
        let end = partition_point_over(len, |i| self.edge(i), |edge| edge < bottom);
        start..end.clamp(start, len)
    }

    /// Updates one height and returns its delta.
    ///
    /// This costs `O(len - index)`, maps invalid heights to zero, and returns zero for a past-end index.
    pub fn set_height(&mut self, index: usize, new_height: f32) -> f64 {
        let len = self.len();
        if index >= len {
            return 0.0;
        }
        let new_h = sane_dim(new_height);
        let old_h = self.prefix[index + 1] - self.prefix[index];
        let delta = new_h - old_h;
        if delta == 0.0 {
            return 0.0;
        }
        for i in (index + 1)..=len {
            self.prefix[i] += delta;
        }
        delta
    }

    /// Applies measured heights and returns the total pixel correction needed to keep the anchor row’s top edge fixed.
    ///
    /// Only deltas for rows strictly above `anchor_index` contribute; rows at or below it do not. An empty batch returns zero.
    pub fn apply_measurements(
        &mut self,
        measurements: impl IntoIterator<Item = (usize, f32)>,
        anchor_index: usize,
    ) -> f64 {
        let len = self.len();
        // A row measured twice ends at its last height, as applying them in
        // turn would leave it.
        let mut heights: Vec<(usize, f64)> = measurements
            .into_iter()
            .filter(|&(index, _)| index < len)
            .map(|(index, height)| (index, sane_dim(height)))
            .collect();
        heights.reverse();
        heights.sort_by_key(|&(index, _)| index);
        heights.dedup_by_key(|&mut (index, _)| index);
        let deltas: Vec<(usize, f64)> = heights
            .into_iter()
            .map(|(index, height)| {
                (
                    index,
                    height - (self.prefix[index + 1] - self.prefix[index]),
                )
            })
            .filter(|&(_, delta)| delta != 0.0)
            .collect();
        let Some(&(first, _)) = deltas.first() else {
            return 0.0;
        };
        // One sweep: each edge moves by the deltas of the rows above it.
        let mut running = 0.0_f64;
        let mut next = deltas.iter().peekable();
        for edge in (first + 1)..=len {
            while let Some(&&(index, delta)) = next.peek()
                && index < edge
            {
                running += delta;
                next.next();
            }
            self.prefix[edge] += running;
        }
        deltas
            .iter()
            .filter(|&&(index, _)| index < anchor_index)
            .map(|&(_, delta)| delta)
            .sum()
    }
}

/// A signed pixel adjustment that preserves an anchored row’s viewport position after height measurements.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasurementCorrection {
    /// The total delta in pixels to add to the scroll offset.
    pub delta: f64,
}

impl MeasurementCorrection {
    /// Applies this correction to a scroll offset.
    #[must_use]
    pub fn apply(self, scroll_offset: f32) -> f32 {
        (f64::from(scroll_offset) + self.delta) as f32
    }

    /// Whether this correction is a no-op.
    #[must_use]
    pub fn is_zero(self) -> bool {
        self.delta == 0.0
    }
}

/// An object-safe source that realizes owned items on demand.
///
/// Implementations may report very large logical lengths without storing every item. [`ItemProvider::item`] must be total and return `None` out of range; expensive owned items should use cheaply cloned shared handles.
#[allow(
    clippy::len_without_is_empty,
    reason = "a virtual length is the whole contract; emptiness is `len() == 0`"
)]
pub trait ItemProvider<Item> {
    /// Returns the logical item count, which may be large without realizing any items.
    fn len(&self) -> u64;

    /// Realizes the item at `index`, returning `None` at or beyond [`len`](Self::len) without panicking or clamping.
    fn item(&self, index: u64) -> Option<Item>;
}

/// Derived conveniences blanket-implemented for every [`ItemProvider`].
pub trait ItemProviderExt<Item>: ItemProvider<Item> {
    /// Whether the provider holds no items.
    ///
    /// Always exactly `len() == 0`, by construction rather than by convention.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Realizes every available item in `range` in order.
    ///
    /// Empty and reversed ranges yield nothing, and ranges past [`ItemProvider::len`] are truncated. Cost and allocation are proportional to the entire clamped range, so callers must provide a viewport-bounded range.
    fn items_in(&self, range: Range<u64>) -> Vec<Item> {
        let end = range.end.min(self.len());
        if range.start >= end {
            return Vec::new();
        }
        // Reserve the full clamped range so impossible requests fail before an unbounded realization loop.
        let count = usize::try_from(end - range.start).unwrap_or(usize::MAX);
        let mut out = Vec::with_capacity(count);
        for index in range.start..end {
            if let Some(item) = self.item(index) {
                out.push(item);
            }
        }
        out
    }
}

impl<Item, P: ItemProvider<Item> + ?Sized> ItemProviderExt<Item> for P {}

/// Object-safe geometry shared by uniform and variable-height sequence models.
///
/// Implementations provide item count, offsets, lookup, and visible ranges with the same half-open coordinate contract.
#[allow(
    clippy::len_without_is_empty,
    reason = "a virtual length is the whole contract; emptiness is `len() == 0`"
)]
pub trait SequenceGeometry {
    /// The logical item count.
    fn len(&self) -> usize;

    /// Total content height.
    fn total_height(&self) -> f32;

    /// The y-offset of item `index`'s top edge, clamped to the content end.
    fn offset_of(&self, index: usize) -> f32;

    /// Returns the item’s unrounded top edge when available.
    ///
    /// Implementations should satisfy `offset_of_f64(i) as f32 == offset_of(i)`. The default promotes the f32 result and is therefore correct but lossy.
    fn offset_of_f64(&self, index: usize) -> f64 {
        f64::from(self.offset_of(index))
    }

    /// Which item occupies `offset`, or `None` if none does.
    fn index_at(&self, offset: f32) -> Option<usize>;

    /// The exact visible index range for a viewport.
    fn range_for(&self, viewport_top: f32, viewport_height: f32) -> Range<usize>;
}

impl SequenceGeometry for HeightModel {
    fn len(&self) -> usize {
        HeightModel::len(self)
    }
    fn total_height(&self) -> f32 {
        HeightModel::total_height(self)
    }
    fn offset_of(&self, index: usize) -> f32 {
        HeightModel::offset_of(self, index)
    }
    fn offset_of_f64(&self, index: usize) -> f64 {
        HeightModel::offset_of_f64(self, index)
    }
    fn index_at(&self, offset: f32) -> Option<usize> {
        HeightModel::index_at(self, offset)
    }
    fn range_for(&self, viewport_top: f32, viewport_height: f32) -> Range<usize> {
        HeightModel::range_for(self, viewport_top, viewport_height)
    }
}

impl SequenceGeometry for VariableHeightModel {
    fn len(&self) -> usize {
        VariableHeightModel::len(self)
    }
    fn total_height(&self) -> f32 {
        VariableHeightModel::total_height(self)
    }
    fn offset_of(&self, index: usize) -> f32 {
        VariableHeightModel::offset_of(self, index)
    }
    fn offset_of_f64(&self, index: usize) -> f64 {
        VariableHeightModel::offset_of_f64(self, index)
    }
    fn index_at(&self, offset: f32) -> Option<usize> {
        VariableHeightModel::index_at(self, offset)
    }
    fn range_for(&self, viewport_top: f32, viewport_height: f32) -> Range<usize> {
        VariableHeightModel::range_for(self, viewport_top, viewport_height)
    }
}

/// A scroll position represented by an item index and a pixel offset within that item.
///
/// Re-resolving against changed heights preserves content position, but this index anchor must not be persisted across item reordering; use [`KeyedAnchor`] for mutations. Invalid sub-item offsets degrade to zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    index: usize,
    offset_in_item: f32,
}

fn expressible_offset(v: f32) -> bool {
    v.is_finite() && v >= 0.0
}

impl Default for Anchor {
    fn default() -> Self {
        Anchor::top()
    }
}

impl Anchor {
    /// Creates an anchor, mapping negative or non-finite sub-item offsets to zero.
    #[must_use]
    pub fn new(index: usize, offset_in_item: f32) -> Self {
        Anchor {
            index,
            offset_in_item: if expressible_offset(offset_in_item) {
                offset_in_item
            } else {
                0.0
            },
        }
    }

    /// The anchor at the very top of the content: item `0`, offset `0`.
    #[must_use]
    pub fn top() -> Self {
        Anchor {
            index: 0,
            offset_in_item: 0.0,
        }
    }

    /// The anchored item's index.
    #[must_use]
    pub fn index(&self) -> usize {
        self.index
    }

    /// Pixels below the anchored item's top edge.
    #[must_use]
    pub fn offset_in_item(&self) -> f32 {
        self.offset_in_item
    }

    /// Changes the item index without changing the sub-item offset.
    #[must_use]
    pub fn with_index(self, index: usize) -> Self {
        Anchor { index, ..self }
    }

    /// Changes the sub-item offset, preserving the current value when the new value is negative or non-finite.
    #[must_use]
    pub fn with_offset_in_item(self, offset_in_item: f32) -> Self {
        Anchor {
            offset_in_item: if expressible_offset(offset_in_item) {
                offset_in_item
            } else {
                self.offset_in_item
            },
            ..self
        }
    }

    /// Captures the anchor at `viewport_top`.
    ///
    /// At or beyond the content end this returns `(len, 0)`; if no item owns the offset for another reason, it returns [`Anchor::top`].
    #[must_use]
    pub fn from_scroll_top(geom: &(impl SequenceGeometry + ?Sized), viewport_top: f32) -> Self {
        if let Some(index) = geom.index_at(viewport_top) {
            // `index_at` guarantees a non-negative offset smaller than the containing item’s height.
            return Anchor::new(index, viewport_top - geom.offset_of(index));
        }
        if viewport_top.is_finite() && viewport_top >= geom.total_height() {
            return Anchor::new(geom.len(), 0.0);
        }
        Anchor::top()
    }

    /// Returns the pixel scroll offset named by this anchor under `geom`.
    ///
    /// The result is not clamped to content bounds, because clamping would alter the stored content-relative position.
    #[must_use]
    pub fn scroll_top(&self, geom: &(impl SequenceGeometry + ?Sized)) -> f32 {
        self.scroll_top_f64(geom) as f32
    }

    /// Returns the unrounded pixel scroll offset named by this anchor.
    #[must_use]
    pub fn scroll_top_f64(&self, geom: &(impl SequenceGeometry + ?Sized)) -> f64 {
        geom.offset_of_f64(self.index) + f64::from(self.offset_in_item)
    }

    /// Iterates placements for `range` relative to the viewport top.
    ///
    /// The range is clamped to the geometry, empty or reversed ranges yield nothing, and leading partial or overscan items may have negative offsets.
    pub fn place<'g, G>(&self, geom: &'g G, range: Range<usize>) -> PlacedItems<'g, G>
    where
        G: SequenceGeometry + ?Sized,
    {
        PlacedItems::new(geom, self.scroll_top_f64(geom), range)
    }

    /// Returns the visible item range for this anchor and viewport height, without overscan.
    #[must_use]
    pub fn resolve(
        &self,
        geom: &(impl SequenceGeometry + ?Sized),
        viewport_height: f32,
    ) -> Range<usize> {
        geom.range_for(self.scroll_top(geom), viewport_height)
    }
}

/// A stable logical item key used to preserve identity across insertion, removal, and reordering.
///
/// The API intentionally provides no numeric conversion because positional keys do not survive mutation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SequenceKey(String);

impl SequenceKey {
    /// Creates a key from a stable logical identity rather than a sequence position.
    #[must_use]
    pub fn new(logical: impl Into<String>) -> Self {
        SequenceKey(logical.into())
    }

    /// Returns the logical key as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for SequenceKey {
    fn from(s: &str) -> Self {
        SequenceKey(s.to_owned())
    }
}

impl From<String> for SequenceKey {
    fn from(s: String) -> Self {
        SequenceKey(s)
    }
}

/// An [`ItemProvider`] that assigns each item a stable logical key.
///
/// A key must continue to identify the same content when indices change.
pub trait KeyedProvider<Item>: ItemProvider<Item> {
    /// Returns the stable key at `index`, or `None` at or beyond the provider length.
    fn key(&self, index: u64) -> Option<SequenceKey>;

    /// Finds the current index for `key`.
    ///
    /// The default scans in `O(len)` and returns the first match; providers with a reverse index should override it.
    fn index_of(&self, key: &SequenceKey) -> Option<u64> {
        let len = self.len();
        let mut index = 0u64;
        while index < len {
            if self.key(index).as_ref() == Some(key) {
                return Some(index);
            }
            index += 1;
        }
        None
    }
}

/// A key and intra-item pixel offset that preserve scroll position across sequence mutations.
///
/// Resolution returns `None` when the keyed item no longer exists. Invalid offsets degrade to zero.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyedAnchor {
    key: SequenceKey,
    offset_in_item: f32,
}

impl KeyedAnchor {
    /// Creates a keyed anchor, mapping negative or non-finite sub-item offsets to zero.
    #[must_use]
    pub fn new(key: SequenceKey, offset_in_item: f32) -> Self {
        KeyedAnchor {
            key,
            offset_in_item: if expressible_offset(offset_in_item) {
                offset_in_item
            } else {
                0.0
            },
        }
    }

    /// The anchored item's key.
    #[must_use]
    pub fn key(&self) -> &SequenceKey {
        &self.key
    }

    /// Pixels below the anchored item's top edge.
    #[must_use]
    pub fn offset_in_item(&self) -> f32 {
        self.offset_in_item
    }

    /// Captures the keyed item at `viewport_top`, preserving its intra-item offset. Returns `None` when no provider item owns that position.
    #[must_use]
    pub fn capture<Item, P, G>(provider: &P, geom: &G, viewport_top: f32) -> Option<Self>
    where
        P: KeyedProvider<Item> + ?Sized,
        G: SequenceGeometry + ?Sized,
    {
        let anchor = Anchor::from_scroll_top(geom, viewport_top);
        let key = provider.key(anchor.index() as u64)?;
        Some(KeyedAnchor::new(key, anchor.offset_in_item()))
    }

    /// Resolves the key to its current index while preserving the sub-item offset, or returns `None` if the key is absent.
    #[must_use]
    pub fn resolve<Item, P>(&self, provider: &P) -> Option<Anchor>
    where
        P: KeyedProvider<Item> + ?Sized,
    {
        let index = provider.index_of(&self.key)?;
        Some(Anchor::new(index as usize, self.offset_in_item))
    }

    /// Returns the scroll offset that preserves this keyed content under current geometry, or `None` if the key is absent.
    #[must_use]
    pub fn scroll_top<Item, P, G>(&self, provider: &P, geom: &G) -> Option<f32>
    where
        P: KeyedProvider<Item> + ?Sized,
        G: SequenceGeometry + ?Sized,
    {
        Some(self.resolve(provider)?.scroll_top(geom))
    }
}

/// Placement of one realized item in viewport-local coordinates.
///
/// Items above the viewport have negative offsets; callers are responsible for clipping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// The item's index in the sequence.
    pub index: usize,
    /// The item’s viewport-relative top edge, which may be negative.
    pub y_offset: f32,
    /// The item's height, from the same geometry that produced `y_offset`.
    pub height: f32,
}

impl Placement {
    /// The item's bottom edge, relative to the viewport's top edge.
    #[must_use]
    pub fn bottom(&self) -> f32 {
        self.y_offset + self.height
    }
}

/// An allocation-free, ascending iterator over placements in a realized range.
///
/// Offsets use f64 geometry and anchor arithmetic until one final f32 conversion. `y_offset + height` may differ from the next offset by one f32 ULP for non-representable heights.
#[derive(Debug)]
pub struct PlacedItems<'g, G: SequenceGeometry + ?Sized> {
    geom: &'g G,
    scroll_top: f64,
    next: usize,
    end: usize,
    edge: Option<f64>,
}

impl<'g, G: SequenceGeometry + ?Sized> PlacedItems<'g, G> {
    fn new(geom: &'g G, scroll_top: f64, range: Range<usize>) -> Self {
        let len = geom.len();
        let start = range.start.min(len);
        let end = range.end.clamp(start, len);
        PlacedItems {
            geom,
            scroll_top,
            next: start,
            end,
            edge: None,
        }
    }
}

impl<G: SequenceGeometry + ?Sized> Iterator for PlacedItems<'_, G> {
    type Item = Placement;

    fn next(&mut self) -> Option<Placement> {
        if self.next >= self.end {
            return None;
        }
        let index = self.next;
        let top = match self.edge {
            Some(carried) => carried,
            None => self.geom.offset_of_f64(index),
        };
        let bottom = self.geom.offset_of_f64(index + 1);
        self.edge = Some(bottom);
        self.next = index + 1;
        Some(Placement {
            index,
            // Preserve f64 subtraction until the final viewport-offset conversion.
            y_offset: (top - self.scroll_top) as f32,
            height: (bottom - top) as f32,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.end.saturating_sub(self.next);
        (n, Some(n))
    }
}

impl<G: SequenceGeometry + ?Sized> ExactSizeIterator for PlacedItems<'_, G> {}

/// An item-count margin added on each side of a nonempty visible range.
///
/// Zero realizes exactly the visible range, and an empty visible range remains empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RealizationWindow {
    overscan: usize,
}

impl RealizationWindow {
    /// Creates a per-side item overscan; `0` adds no items.
    #[must_use]
    pub fn new(overscan: usize) -> Self {
        RealizationWindow { overscan }
    }

    /// The configured overscan, in items per side.
    #[must_use]
    pub fn overscan(&self) -> usize {
        self.overscan
    }

    /// Expands a nonempty visible range on both sides, clamped to `0..len`; empty ranges are returned unchanged.
    #[must_use]
    pub fn expand(&self, visible: Range<usize>, len: usize) -> Range<usize> {
        if visible.start >= visible.end {
            return visible;
        }
        let start = visible.start.saturating_sub(self.overscan);
        let end = visible.end.saturating_add(self.overscan).min(len);
        start..end
    }

    /// Returns the viewport’s visible range expanded by this item overscan.
    #[must_use]
    pub fn realize(
        &self,
        geom: &(impl SequenceGeometry + ?Sized),
        anchor: &Anchor,
        viewport_height: f32,
    ) -> Range<usize> {
        self.expand(anchor.resolve(geom, viewport_height), geom.len())
    }

    /// Returns the realized range and its viewport-relative placements in one operation.
    pub fn place<'g, G>(
        &self,
        geom: &'g G,
        anchor: &Anchor,
        viewport_height: f32,
    ) -> PlacedItems<'g, G>
    where
        G: SequenceGeometry + ?Sized,
    {
        anchor.place(geom, self.realize(geom, anchor, viewport_height))
    }
}

/// A pixel-distance margin added on each side of a nonempty visible range.
///
/// Invalid or negative budgets degrade to zero. Over variable track sizes the extra track count varies to honor the pixel budget; empty visible ranges remain empty.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PixelOverscan {
    budget: f64,
}

impl Default for PixelOverscan {
    fn default() -> Self {
        PixelOverscan::new(0.0)
    }
}

impl PixelOverscan {
    /// Creates a per-side pixel overscan, mapping negative or non-finite budgets to zero.
    #[must_use]
    pub fn new(budget_px: f32) -> Self {
        PixelOverscan {
            budget: sane_dim(budget_px),
        }
    }

    /// The configured pixel budget per side.
    #[must_use]
    pub fn budget_px(&self) -> f32 {
        self.budget as f32
    }

    /// Expands a nonempty visible range by the pixel budget, clamped to the geometry.
    ///
    /// Leading and trailing tracks are included according to their top edges; empty ranges retain their position.
    #[must_use]
    pub fn expand(
        &self,
        visible: Range<usize>,
        geom: &(impl SequenceGeometry + ?Sized),
    ) -> Range<usize> {
        if visible.start >= visible.end {
            return visible;
        }
        let len = geom.len();
        // Compute the leading edge in f64 and saturate budgets larger than preceding content at zero.
        let visible_top = geom.offset_of_f64(visible.start);
        let lead_edge = visible_top - self.budget;
        let start = if lead_edge <= 0.0 {
            0
        } else {
            partition_point_over(len, |i| geom.offset_of_f64(i), |edge| edge <= lead_edge)
                .saturating_sub(1)
                .min(visible.start)
        };
        let visible_bottom = geom.offset_of_f64(visible.end);
        let trail_edge = visible_bottom + self.budget;
        let end = partition_point_over(len, |i| geom.offset_of_f64(i), |edge| edge < trail_edge)
            .max(visible.end)
            .min(len);
        start..end
    }

    /// Returns the viewport’s visible range expanded by a per-side pixel budget.
    #[must_use]
    pub fn realize(
        &self,
        geom: &(impl SequenceGeometry + ?Sized),
        anchor: &Anchor,
        viewport_height: f32,
    ) -> Range<usize> {
        self.expand(anchor.resolve(geom, viewport_height), geom)
    }

    /// Returns the pixel-overscanned range and its viewport-relative placements in one operation.
    pub fn place<'g, G>(
        &self,
        geom: &'g G,
        anchor: &Anchor,
        viewport_height: f32,
    ) -> PlacedItems<'g, G>
    where
        G: SequenceGeometry + ?Sized,
    {
        anchor.place(geom, self.realize(geom, anchor, viewport_height))
    }
}

#[cfg(test)]
mod tests;
