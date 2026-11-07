//! Bounded least-recently-used item cache keyed by stable sequence identity.

use core::ops::Range;
use std::collections::BTreeMap;

use super::{KeyedProvider, SequenceKey};

/// A bounded least-recently-used cache keyed by [`SequenceKey`].
///
/// Keyed entries survive index-shifting mutations; clear the cache only when content changes without changing keys. A capacity of `0` stores nothing. `item` and `items_in` clone cached values, so expensive items should use shared ownership.
#[derive(Debug, Clone)]
pub struct ItemCache<Item> {
    capacity: usize,
    pub(super) by_key: BTreeMap<SequenceKey, (u64, Item)>,
    // Mirrors `by_key`; its first entry is the least-recently-used key.
    by_stamp: BTreeMap<u64, SequenceKey>,
    // Recency ordering assumes the use counter does not wrap.
    clock: u64,
}

impl<Item> Default for ItemCache<Item> {
    /// Creates an empty cache with capacity `0`, so it retains no items.
    fn default() -> Self {
        ItemCache::new(0)
    }
}

impl<Item> ItemCache<Item> {
    /// Creates a cache holding at most `capacity` items; `0` disables caching.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        ItemCache {
            capacity,
            by_key: BTreeMap::new(),
            by_stamp: BTreeMap::new(),
            clock: 0,
        }
    }

    /// The configured bound, in items.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// How many items are currently cached. Never exceeds
    /// [`capacity`](Self::capacity).
    #[must_use]
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    /// Whether nothing is cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// Returns whether `key` is cached without updating recency.
    #[must_use]
    pub fn contains(&self, key: &SequenceKey) -> bool {
        self.by_key.contains_key(key)
    }

    /// Returns the cached item without updating recency.
    #[must_use]
    pub fn peek(&self, key: &SequenceKey) -> Option<&Item> {
        self.by_key.get(key).map(|(_, item)| item)
    }

    /// Returns the cached item and marks it most recently used; a miss returns `None`.
    pub fn get(&mut self, key: &SequenceKey) -> Option<&Item> {
        let (stamp, _) = self.by_key.get(key)?;
        let old = *stamp;
        self.clock += 1;
        let fresh = self.clock;
        self.by_stamp.remove(&old);
        self.by_stamp.insert(fresh, key.clone());
        let entry = self.by_key.get_mut(key)?;
        entry.0 = fresh;
        Some(&entry.1)
    }

    /// Stores an item and marks it most recently used, evicting the least-recently-used entry if needed. A capacity of `0` stores nothing.
    pub fn insert(&mut self, key: SequenceKey, item: Item) {
        if self.capacity == 0 {
            return;
        }
        if let Some((old, _)) = self.by_key.get(&key) {
            self.by_stamp.remove(old);
        }
        self.clock += 1;
        let stamp = self.clock;
        self.by_key.insert(key.clone(), (stamp, item));
        self.by_stamp.insert(stamp, key);
        self.evict_to_capacity();
    }

    /// Drop the entry at `key`, if any. Returns whether one was dropped.
    ///
    /// Use after that row's content changed in place.
    pub fn invalidate(&mut self, key: &SequenceKey) -> bool {
        match self.by_key.remove(key) {
            Some((stamp, _)) => {
                self.by_stamp.remove(&stamp);
                true
            }
            None => false,
        }
    }

    /// Drops all entries. Use this when content changes without changing keys; index shifts alone do not require clearing.
    pub fn clear(&mut self) {
        self.by_key.clear();
        self.by_stamp.clear();
    }

    /// Change the bound, evicting least-recently-used entries immediately if
    /// the new bound is smaller. `set_capacity(0)` empties the cache.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity;
        self.evict_to_capacity();
    }

    fn evict_to_capacity(&mut self) {
        while self.by_key.len() > self.capacity {
            let Some((_, victim)) = self.by_stamp.pop_first() else {
                debug_assert!(self.by_key.is_empty(), "by_stamp/by_key diverged");
                break;
            };
            self.by_key.remove(&victim);
        }
    }
}

impl<Item: Clone> ItemCache<Item> {
    /// Returns the item at `index`, using its stable key to reuse cached content.
    ///
    /// A hit updates recency. A miss calls the provider once and caches a returned item unless capacity is zero; `None` results are not cached.
    pub fn item<P>(&mut self, provider: &P, index: u64) -> Option<Item>
    where
        P: KeyedProvider<Item> + ?Sized,
    {
        let key = provider.key(index)?;
        if let Some(hit) = self.get(&key) {
            return Some(hit.clone());
        }
        let item = provider.item(index)?;
        self.insert(key, item.clone());
        Some(item)
    }

    /// Returns the available items in `range` in order, using stable-key cache entries where present.
    pub fn items_in<P>(&mut self, provider: &P, range: Range<u64>) -> Vec<Item>
    where
        P: KeyedProvider<Item> + ?Sized,
    {
        let end = range.end.min(provider.len());
        if range.start >= end {
            return Vec::new();
        }
        let count = usize::try_from(end - range.start).unwrap_or(usize::MAX);
        let mut out = Vec::with_capacity(count);
        for index in range.start..end {
            if let Some(item) = self.item(provider, index) {
                out.push(item);
            }
        }
        out
    }
}
