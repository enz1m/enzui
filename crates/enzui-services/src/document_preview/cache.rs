//! Bounded LRU cache for document previews, keyed by file identity and preview variant.

use super::RenderedPreview;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Identifies the PDF page or video timestamp represented by a preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreviewVariant {
    /// A PDF page number (0-indexed).
    PdfPage(u32),
    /// A video timestamp in milliseconds.
    VideoTimestampMs(u64),
}

/// Identifies a cached preview by path, modification time, file size, and variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PreviewCacheKey {
    /// The file path supplied when the key was built.
    pub path: PathBuf,
    /// The file's modification time at the time of rendering.
    pub mtime: SystemTime,
    /// The file's size in bytes at the time of rendering.
    pub file_size: u64,
    /// What was rendered (which page, which timestamp).
    pub variant: PreviewVariant,
}

impl PreviewCacheKey {
    /// Build a cache key by stat'ing the file.
    ///
    /// Returns `None` if the file cannot be stat'd (deleted, permission denied).
    pub fn from_path(path: &Path, variant: PreviewVariant) -> Option<PreviewCacheKey> {
        let meta = std::fs::metadata(path).ok()?;
        Some(PreviewCacheKey {
            path: path.to_path_buf(),
            mtime: meta.modified().ok()?,
            file_size: meta.len(),
            variant,
        })
    }

    /// Builds a key from explicit path metadata and preview variant.
    pub fn explicit(
        path: PathBuf,
        mtime: SystemTime,
        file_size: u64,
        variant: PreviewVariant,
    ) -> Self {
        PreviewCacheKey {
            path,
            mtime,
            file_size,
            variant,
        }
    }
}

/// A bounded cache that evicts the least-recently-used preview at capacity.
pub struct PreviewCache {
    entries: HashMap<PreviewCacheKey, RenderedPreview>,
    order: VecDeque<PreviewCacheKey>,
    capacity: usize,
    render_count: u64,
}

impl PreviewCache {
    /// Creates a cache with the given capacity; zero disables retention.
    pub fn new(capacity: usize) -> Self {
        PreviewCache {
            entries: HashMap::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            capacity,
            render_count: 0,
        }
    }

    /// Look up a cached preview. Returns `None` on a miss.
    ///
    /// On a hit, the entry is promoted to most-recently-used.
    pub fn get(&mut self, key: &PreviewCacheKey) -> Option<RenderedPreview> {
        let preview = self.entries.get(key)?.clone();
        // The order holds each key once, so promotion stops at its one match.
        if let Some(at) = self.order.iter().position(|k| k == key)
            && let Some(promoted) = self.order.remove(at)
        {
            self.order.push_back(promoted);
        }
        Some(preview)
    }

    /// Inserts or replaces a preview and increments [`render_count`](Self::render_count).
    ///
    /// A new key evicts the least-recently-used entry when the nonzero capacity is full.
    pub fn insert(&mut self, key: PreviewCacheKey, preview: RenderedPreview) {
        self.render_count += 1;

        if self.entries.contains_key(&key) {
            self.order.retain(|k| k != &key);
        } else if self.capacity > 0
            && self.entries.len() >= self.capacity
            && let Some(evicted) = self.order.pop_front()
        {
            self.entries.remove(&evicted);
        }

        if self.capacity > 0 {
            self.entries.insert(key.clone(), preview);
            self.order.push_back(key);
        }
    }

    /// Returns the number of calls to [`Self::insert`] since construction.
    #[must_use]
    pub fn render_count(&self) -> u64 {
        self.render_count
    }

    /// The number of entries currently in the cache.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The maximum number of entries this cache will hold.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Remove all entries from the cache. Does not reset `render_count`.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(page: u32) -> PreviewCacheKey {
        PreviewCacheKey::explicit(
            "/doc.pdf".into(),
            SystemTime::UNIX_EPOCH,
            1,
            PreviewVariant::PdfPage(page),
        )
    }

    fn preview() -> RenderedPreview {
        RenderedPreview::new(1, 1, vec![0; 4]).expect("one pixel")
    }

    #[test]
    fn a_hit_is_promoted_past_eviction() {
        let mut cache = PreviewCache::new(2);
        cache.insert(key(0), preview());
        cache.insert(key(1), preview());
        assert!(cache.get(&key(0)).is_some());
        cache.insert(key(2), preview());
        assert!(cache.get(&key(1)).is_none(), "the least recently used goes");
        assert!(cache.get(&key(0)).is_some());
        assert!(cache.get(&key(2)).is_some());
        assert!(cache.get(&key(9)).is_none());
    }
}
