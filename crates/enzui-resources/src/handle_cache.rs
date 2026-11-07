use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;

use iced_core::image::Handle;

/// A least-recently-used image-handle cache with a backing-byte budget.
///
/// RGBA handles charge their pixel bytes; encoded handles charge their encoded
/// bytes; path handles charge zero. The budget does not bound entry overhead,
/// renderer-decoded memory, or handles retained by callers after eviction.
/// Inserting over budget evicts the least recently used entries first; a handle
/// larger than the entire budget is refused without evicting other keys.
///
/// ```
/// use enzui_resources::HandleCache;
/// use iced_core::image::Handle;
///
/// let mut cache = HandleCache::new(1024);
/// let pixels = vec![0u8; 16 * 16 * 4];
/// cache.insert("a", Handle::from_rgba(16, 16, pixels));
/// assert!(cache.get(&"a").is_some());
/// assert_eq!(cache.bytes(), 1024);
/// ```
#[derive(Debug)]
pub struct HandleCache<K> {
    budget: usize,
    bytes: usize,
    next_use: u64,
    entries: HashMap<K, Entry>,
    order: BTreeMap<u64, K>,
}

#[derive(Debug)]
struct Entry {
    handle: Handle,
    cost: usize,
    used: u64,
}

impl<K: Clone + Eq + Hash> HandleCache<K> {
    /// Start an empty cache with a backing-byte budget.
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            bytes: 0,
            next_use: 0,
            entries: HashMap::new(),
            order: BTreeMap::new(),
        }
    }

    /// The handle for `key`, marking it as just used.
    pub fn get(&mut self, key: &K) -> Option<Handle> {
        let use_now = self.tick();
        let entry = self.entries.get_mut(key)?;
        self.order.remove(&entry.used);
        entry.used = use_now;
        self.order.insert(use_now, key.clone());
        Some(entry.handle.clone())
    }

    /// Whether `key` is held, without marking it as used.
    pub fn contains(&self, key: &K) -> bool {
        self.entries.contains_key(key)
    }

    /// Keep `handle` under `key`, replacing what was there, then drop the
    /// entries used longest ago until the cache is within its budget. A
    /// handle larger than the whole budget is not kept.
    pub fn insert(&mut self, key: K, handle: Handle) {
        self.remove(&key);
        let cost = cost(&handle);
        if cost > self.budget {
            return;
        }
        let used = self.tick();
        self.bytes += cost;
        self.order.insert(used, key.clone());
        self.entries.insert(key, Entry { handle, cost, used });
        while self.bytes > self.budget {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            if let Some(entry) = self.entries.remove(&oldest) {
                self.bytes -= entry.cost;
            }
        }
    }

    /// Drop `key`'s handle, if held.
    pub fn remove(&mut self, key: &K) -> Option<Handle> {
        let entry = self.entries.remove(key)?;
        self.order.remove(&entry.used);
        self.bytes -= entry.cost;
        Some(entry.handle)
    }

    /// Drop every handle.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
    }

    /// The backing bytes charged to the current entries.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// The maximum backing bytes charged to this cache.
    pub fn budget(&self) -> usize {
        self.budget
    }

    /// How many handles are held.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no handle is held.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn tick(&mut self) -> u64 {
        self.next_use += 1;
        self.next_use
    }
}

fn cost(handle: &Handle) -> usize {
    match handle {
        Handle::Rgba { pixels, .. } => pixels.len(),
        Handle::Bytes(_, bytes) => bytes.len(),
        Handle::Path(..) => 0,
    }
}
