//! Content-versioned handle caching with stale-handle invalidation.
//!
//! A resolved handle (MIME type plus themed icon path) carries the
//! [`Generation`], [`Fingerprint`] and [`ResolutionKey`] it was resolved under.
//! Both staleness axes are load-bearing: the fingerprint catches an in-place
//! file replacement, and the resolution key catches a theme, icon-size or scale
//! change that alters the resolved icon for a byte-identical file. An entry
//! survives a generation bump only while both are unchanged, which keeps
//! untouched files warm when a single file is replaced.
//!
//! Pure policy over injected values: no filesystem access, no threads, no
//! `Task`. [`FingerprintSource`] is the seam, with [`FsFingerprintSource`] as
//! the production implementation.
//!
//! [`SniffCache`](super::sniff::SniffCache) caches the sniff outcome, the
//! expensive I/O step; this caches the resolved handle, the step a renderer
//! repeats every frame. Handles here are opaque ids, so decode budgets belong
//! to the layer that turns them into pixels.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use super::sniff::Generation;

/// An opaque content fingerprint over whatever distinguishes two versions of a
/// path. Equal fingerprints let the cache reuse a handle; unequal ones mean the
/// content changed and the handle is stale.
///
/// What is hashed is deliberately unspecified: the filesystem-touching part
/// lives behind [`FingerprintSource`].
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct Fingerprint(u64);

impl Fingerprint {
    /// Create a fingerprint from a raw hash value.
    pub fn new(hash: u64) -> Self {
        Self(hash)
    }

    /// The raw hash value.
    pub fn get(&self) -> u64 {
        self.0
    }
}

/// An opaque digest of the resolution inputs a handle was produced under: the
/// active icon-theme name, the requested icon size and the display scale. Equal
/// keys mean the pipeline would pick the same theme directory and icon name
/// again.
///
/// [`ResolutionKey::of`] is the digest the shipped
/// [`FileIconResolver`](crate::resolver::FileIconResolver) uses;
/// [`ResolutionKey::new`] lets a caller mix in further inputs, such as a
/// high-contrast variant or a per-monitor policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct ResolutionKey(u64);

impl ResolutionKey {
    /// Create a resolution key from a raw digest.
    pub fn new(digest: u64) -> Self {
        Self(digest)
    }

    /// The digest of the inputs the shipped resolver varies on: theme name,
    /// icon size and display scale.
    pub fn of(theme: &str, icon_size: u16, scale: u16) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        // Domain-separate from `Fingerprint`, which hashes a different tuple
        // into the same width.
        "enzui-icon-resolution-v1".hash(&mut hasher);
        theme.hash(&mut hasher);
        icon_size.hash(&mut hasher);
        scale.hash(&mut hasher);
        Self(hasher.finish())
    }

    /// The raw digest value.
    pub fn get(&self) -> u64 {
        self.0
    }
}

/// The seam for computing a file's fingerprint. [`FsFingerprintSource`]
/// touches the filesystem; tests can inject a fake.
pub trait FingerprintSource {
    /// Compute the current fingerprint for `path`, or `None` if the path is
    /// inaccessible. A `None` is treated as "fingerprint unknown → cannot
    /// reuse a cached handle".
    fn fingerprint(&self, path: &Path) -> Option<Fingerprint>;
}

/// Filesystem-backed fingerprints for production use.
///
/// On Unix the fingerprint covers device and inode identity, size and
/// nanosecond modification time. Device and inode are load-bearing: an atomic
/// same-path replacement can preserve both size and timestamp. On targets
/// without a portable file identity this falls back to size and the full
/// [`std::time::SystemTime`] modification time.
///
/// Uses [`std::fs::metadata`], so a final symlink is followed. A missing or
/// inaccessible path yields `None`, which prevents stale-handle reuse. Metadata
/// access is synchronous: gather fingerprints for a large directory away from
/// the UI thread and expose them through a snapshot [`FingerprintSource`].
#[derive(Clone, Copy, Debug, Default)]
pub struct FsFingerprintSource;

impl FingerprintSource for FsFingerprintSource {
    fn fingerprint(&self, path: &Path) -> Option<Fingerprint> {
        let metadata = std::fs::metadata(path).ok()?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();

        // Domain-separate this tuple from fingerprints a caller may hash for a
        // different purpose before passing them to `Fingerprint::new`.
        "enzui-file-fingerprint-v1".hash(&mut hasher);

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            metadata.dev().hash(&mut hasher);
            metadata.ino().hash(&mut hasher);
            metadata.mtime().hash(&mut hasher);
            metadata.mtime_nsec().hash(&mut hasher);
            metadata.len().hash(&mut hasher);
        }

        #[cfg(not(unix))]
        {
            metadata.len().hash(&mut hasher);
            metadata.modified().ok()?.hash(&mut hasher);
        }

        Some(Fingerprint::new(hasher.finish()))
    }
}

/// A resolved handle: the result of the full resolution pipeline for one path
/// (MIME, icon name, theme directory, asset path).
///
/// It carries the fingerprint it was resolved under, so a lookup detects
/// staleness without re-resolving, and it names which asset to use rather than
/// holding decoded pixels.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ResolvedHandle {
    /// An opaque identifier for the resolved asset. In a real renderer this
    /// would be an index into a texture atlas or an `iced::widget::image::Handle`.
    pub id: u64,
    /// The MIME type determined for this path.
    pub mime: String,
    /// The icon name resolved from the MIME type.
    pub icon_name: String,
}

impl ResolvedHandle {
    /// Create a resolved handle.
    pub fn new(id: u64, mime: impl Into<String>, icon_name: impl Into<String>) -> Self {
        Self {
            id,
            mime: mime.into(),
            icon_name: icon_name.into(),
        }
    }
}

// A cache entry stamped with its generation and fingerprint.
#[derive(Clone, Debug)]
struct Entry {
    generation: Generation,
    fingerprint: Fingerprint,
    // The resolution inputs this handle was produced under. A lookup under a
    // different key is a miss, never a promotion.
    resolution: ResolutionKey,
    handle: ResolvedHandle,
}

// An in-flight dispatch. Both stamps are checked on completion, so an answer
// computed for a theme the caller has since left is dropped instead of being
// stored under the new key.
#[derive(Clone, Copy, Debug)]
struct InFlight {
    generation: Generation,
    resolution: ResolutionKey,
}

/// What a lookup found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HandleLookup {
    /// A live cached handle whose fingerprint matches the current one.
    Hit(ResolvedHandle),
    /// No live entry — the caller should resolve and insert.
    Miss,
    /// A resolution for this path is already in flight for the current
    /// generation, so a second lookup must coalesce rather than dispatch a
    /// duplicate resolution.
    ///
    /// A synchronous resolver marks and completes within one `&mut self` call,
    /// so this arm is observable only once resolution is split into a dispatch
    /// and a later completion. See
    /// [`ResolveOutcome::InFlight`](crate::resolver::ResolveOutcome::InFlight).
    InFlight,
}

/// A bounded, generation-and-fingerprint-keyed handle cache with single-flight
/// coalescing. Pure policy, no I/O.
///
/// An entry is live while its fingerprint matches the current one for that path
/// and its [`ResolutionKey`] matches the cache's. A generation bump alone
/// promotes such an entry rather than discarding it, unlike `SniffCache`. A
/// bump that accompanies a resolution-key change, which is what
/// [`FileIconResolver::set_theme`](crate::resolver::FileIconResolver::set_theme)
/// and [`set_scale`](crate::resolver::FileIconResolver::set_scale) do, is not
/// promoted through: that would serve the previous theme's icon forever.
///
/// Entries are bounded by capacity with LRU eviction.
pub struct HandleCache {
    generation: Generation,
    resolution: ResolutionKey,
    entries: HashMap<PathBuf, Entry>,
    lru: VecDeque<PathBuf>,
    in_flight: HashMap<PathBuf, InFlight>,
    capacity: usize,
}

impl HandleCache {
    /// A cache bounded to `capacity` entries, starting at [`Generation::ZERO`].
    /// Capacity is clamped to a minimum of 1.
    pub fn new(capacity: usize) -> Self {
        Self {
            generation: Generation::ZERO,
            resolution: ResolutionKey::default(),
            entries: HashMap::new(),
            lru: VecDeque::new(),
            in_flight: HashMap::new(),
            capacity: capacity.max(1),
        }
    }

    /// The current generation.
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// The cache's capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// The resolution inputs (theme / icon size / scale) entries are currently
    /// keyed on.
    pub fn resolution_key(&self) -> ResolutionKey {
        self.resolution
    }

    /// Point the cache at a new set of resolution inputs, returning whether
    /// they changed.
    ///
    /// Existing entries are invalidated lazily on lookup rather than discarded,
    /// so switching back and forth does not re-resolve entries nobody looks at.
    /// No entry stamped with the old key is served or promoted again, and a
    /// dispatch already in flight under it is dropped on completion.
    pub fn set_resolution_key(&mut self, resolution: ResolutionKey) -> bool {
        let changed = self.resolution != resolution;
        self.resolution = resolution;
        changed
    }

    /// The number of currently stored entries (may include stale ones not yet
    /// evicted).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache holds no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Advance the generation. Entries whose fingerprint and resolution key are
    /// unchanged are promoted on lookup rather than discarded.
    pub fn bump_generation(&mut self) {
        self.generation = self.generation.bump();
    }

    /// Look up `path` with the current fingerprint.
    ///
    /// - If there is a cached entry at the current generation whose fingerprint
    ///   and resolution key match → [`HandleLookup::Hit`].
    /// - If there is a cached entry at an older generation but both match →
    ///   the entry is promoted to the current generation and returned as a
    ///   [`HandleLookup::Hit`]. This is how unchanged files survive a
    ///   generation bump.
    /// - If the fingerprint differs (in-place replacement) → the stale
    ///   entry is removed and it is a [`HandleLookup::Miss`].
    /// - If the resolution key differs (the theme, icon size or scale moved
    ///   under the entry) → the entry is removed and it is a
    ///   [`HandleLookup::Miss`], even though the file is byte-identical.
    /// - No entry at all → check in-flight, otherwise [`HandleLookup::Miss`].
    pub fn lookup(&mut self, path: &Path, current_fp: &Fingerprint) -> HandleLookup {
        if let Some(entry) = self.entries.get_mut(path) {
            if entry.fingerprint == *current_fp && entry.resolution == self.resolution {
                entry.generation = self.generation;
                let handle = entry.handle.clone();
                self.touch(path);
                return HandleLookup::Hit(handle);
            }
            // Replaced in place, or the theme, size or scale moved: the stored
            // handle answers the wrong question.
            self.entries.remove(path);
            self.remove_from_lru(path);
            return HandleLookup::Miss;
        }
        if let Some(stamp) = self.in_flight.get(path)
            && stamp.generation == self.generation
            && stamp.resolution == self.resolution
        {
            return HandleLookup::InFlight;
        }
        HandleLookup::Miss
    }

    /// Record that a resolution for `path` has been dispatched under the
    /// current generation and resolution key.
    pub fn mark_in_flight(&mut self, path: &Path) {
        self.in_flight.insert(
            path.to_path_buf(),
            InFlight {
                generation: self.generation,
                resolution: self.resolution,
            },
        );
    }

    /// Record a completed resolution. Returns `true` if accepted, `false` if
    /// dropped: no dispatch of `path` under `dispatched_gen` is in flight
    /// (never marked, already completed, or a replay), the generation
    /// advanced past it, or the resolution inputs changed since it was
    /// marked.
    pub fn complete(
        &mut self,
        path: &Path,
        dispatched_gen: Generation,
        fingerprint: Fingerprint,
        handle: ResolvedHandle,
    ) -> bool {
        let dispatched_resolution = match self.in_flight.get(path) {
            Some(stamp) if stamp.generation == dispatched_gen => {
                let resolution = stamp.resolution;
                self.in_flight.remove(path);
                Some(resolution)
            }
            _ => None,
        };
        // Only the dispatch that was marked may complete, once: a duplicate or
        // unmarked completion could carry a handle resolved under other inputs.
        let Some(dispatched_resolution) = dispatched_resolution else {
            return false;
        };
        if dispatched_gen != self.generation {
            return false;
        }
        // A dispatch made under a different theme, size or scale would launder
        // a stale handle into a live entry.
        if dispatched_resolution != self.resolution {
            return false;
        }
        self.insert(
            path.to_path_buf(),
            Entry {
                generation: self.generation,
                fingerprint,
                resolution: self.resolution,
                handle,
            },
        );
        true
    }

    fn insert(&mut self, path: PathBuf, entry: Entry) {
        if self.entries.contains_key(&path) {
            self.entries.insert(path.clone(), entry);
            self.touch(&path);
            return;
        }
        self.entries.insert(path.clone(), entry);
        self.lru.push_back(path);
        self.evict_if_needed();
    }

    fn touch(&mut self, path: &Path) {
        if let Some(pos) = self.lru.iter().position(|p| p.as_path() == path) {
            let p = self.lru.remove(pos).expect("position just found");
            self.lru.push_back(p);
        }
    }

    fn remove_from_lru(&mut self, path: &Path) {
        if let Some(pos) = self.lru.iter().position(|p| p.as_path() == path) {
            self.lru.remove(pos);
        }
    }

    fn evict_if_needed(&mut self) {
        while self.entries.len() > self.capacity {
            match self.lru.pop_front() {
                Some(victim) => {
                    self.entries.remove(&victim);
                }
                None => break,
            }
        }
    }
}

/// A bounded cache for MIME/theme resolution results, keyed by MIME type,
/// theme, icon size and scale and stamped with a generation.
///
/// It sits between the filename-based MIME resolver and the handle cache,
/// remembering the resolved icon directory instead of re-traversing the theme
/// inheritance chain. Because the key already includes theme and scale, a
/// generation bump invalidates every entry and nothing is promoted.
pub struct MimeThemeCache {
    generation: Generation,
    entries: HashMap<MimeThemeKey, MimeThemeEntry>,
    lru: VecDeque<MimeThemeKey>,
    capacity: usize,
}

/// The key for a MIME/theme resolution: the inputs whose change means the
/// resolution must be redone.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct MimeThemeKey {
    /// The MIME type string.
    pub mime: String,
    /// The active theme name.
    pub theme: String,
    /// The requested icon size.
    pub icon_size: u16,
    /// The display scale factor.
    pub scale: u16,
}

impl MimeThemeKey {
    /// Create a key.
    pub fn new(
        mime: impl Into<String>,
        theme: impl Into<String>,
        icon_size: u16,
        scale: u16,
    ) -> Self {
        Self {
            mime: mime.into(),
            theme: theme.into(),
            icon_size,
            scale,
        }
    }
}

/// The cached result of a MIME → theme resolution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MimeThemeEntry {
    generation: Generation,
    /// The resolved result.
    pub result: MimeThemeResult,
}

/// The result of resolving a MIME type against a theme: either a themed icon
/// directory path or a fallback to the bundled catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MimeThemeResult {
    /// A themed icon was found at this directory path.
    Themed {
        /// The theme that provided the match.
        theme_name: String,
        /// The directory within the theme.
        dir: String,
        /// The icon name to look for in that directory.
        icon_name: String,
    },
    /// No themed icon; fall back to the bundled catalog.
    Bundled {
        /// The icon name to look up in the bundled catalog.
        icon_name: String,
    },
}

/// What a MIME/theme lookup found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MimeThemeLookup {
    /// A live cached resolution.
    Hit(MimeThemeResult),
    /// No live entry.
    Miss,
}

impl MimeThemeCache {
    /// A cache bounded to `capacity` entries, starting at [`Generation::ZERO`].
    pub fn new(capacity: usize) -> Self {
        Self {
            generation: Generation::ZERO,
            entries: HashMap::new(),
            lru: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    /// The current generation.
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// Number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Advance the generation, invalidating all entries. MIME/theme resolution
    /// depends on theme and scale, both of which change with a generation
    /// bump, so no fingerprint-based promotion is possible here.
    pub fn bump_generation(&mut self) {
        self.generation = self.generation.bump();
    }

    /// Look up a MIME/theme resolution.
    pub fn lookup(&mut self, key: &MimeThemeKey) -> MimeThemeLookup {
        if let Some(entry) = self.entries.get(key)
            && entry.generation == self.generation
        {
            let result = entry.result.clone();
            self.touch(key);
            return MimeThemeLookup::Hit(result);
        }
        MimeThemeLookup::Miss
    }

    /// Insert a resolved result at the current generation.
    pub fn insert(&mut self, key: MimeThemeKey, result: MimeThemeResult) {
        let entry = MimeThemeEntry {
            generation: self.generation,
            result,
        };
        if self.entries.contains_key(&key) {
            self.entries.insert(key.clone(), entry);
            self.touch(&key);
            return;
        }
        self.entries.insert(key.clone(), entry);
        self.lru.push_back(key);
        self.evict_if_needed();
    }

    fn touch(&mut self, key: &MimeThemeKey) {
        if let Some(pos) = self.lru.iter().position(|k| k == key) {
            let k = self.lru.remove(pos).expect("position just found");
            self.lru.push_back(k);
        }
    }

    fn evict_if_needed(&mut self) {
        while self.entries.len() > self.capacity {
            match self.lru.pop_front() {
                Some(victim) => {
                    self.entries.remove(&victim);
                }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests;
