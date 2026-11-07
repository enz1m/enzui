//! End-to-end file-icon resolution: the consumer that drives the icon
//! primitives through both caches.
//!
//! [`FileIconResolver`] takes a filename plus the active theme and scale and
//! returns a [`ResolvedHandle`], going through the MIME/theme cache and the
//! handle cache on every call, so that:
//!
//! - the same filename with the same fingerprint under the same generation
//!   returns the same handle without re-resolving;
//! - a changed fingerprint (an in-place file replacement) returns a new handle;
//! - a generation bump with an unchanged fingerprint promotes the entry through
//!   the [`HandleCache`] rather than re-resolving;
//! - a single-flight guard coalesces lookups for the same in-progress path.
//!
//! The two in-flight seams differ in reachability. The content-sniff seam is
//! live: a filename with no confident glob match consults the [`SniffCache`]
//! and, on a miss, dispatches an off-thread sniff through a [`SniffDispatcher`]
//! and returns [`ResolveOutcome::InFlight`], with answers fed back through
//! [`FileIconResolver::complete_sniff`]. The handle-cache in-flight arm stays
//! unreachable while handle resolution is synchronous, because `resolve` marks
//! a path in flight and clears the marker within the same call.
//!
//! For real paths, pass
//! [`FsFingerprintSource`](crate::handle_cache::FsFingerprintSource) to
//! [`FileIconResolver::resolve`]: on Unix it includes inode identity, size and
//! nanosecond mtime, so an atomic same-path replacement invalidates a warm
//! handle. It performs synchronous metadata I/O, so a large directory view
//! should collect the same values off-thread and expose them through a snapshot
//! [`FingerprintSource`].

use std::path::Path;
use std::path::PathBuf;

use crate::handle_cache::{
    Fingerprint, FingerprintSource, HandleCache, HandleLookup, MimeThemeCache, MimeThemeKey,
    MimeThemeLookup, MimeThemeResult, ResolutionKey, ResolvedHandle,
};
use crate::icon_theme::ThemeSet;
use crate::mime::{Certainty, GlobTable, MimeMatch};
use crate::sniff::{
    Admission, Generation, Lookup as SniffLookup, SniffAttempt, SniffCache, SniffOutcome,
};

/// The off-UI-thread dispatch seam for content sniffing.
///
/// [`FileIconResolver::resolve`] consults the sniffer only when the filename
/// yields no confident MIME match. Sniffing reads bytes, which must never
/// happen on a render path, so the resolver hands the path to this dispatcher,
/// in production a thin wrapper over [`crate::sniff::worker::SniffWorker`], and
/// returns [`ResolveOutcome::InFlight`]; the answer comes back through
/// [`FileIconResolver::complete_sniff`].
///
/// `dispatch` must be non-blocking: it enqueues a request and returns, and must
/// never read the file itself.
pub trait SniffDispatcher {
    /// Enqueue one exact `attempt`, to be completed later and fed back via
    /// [`FileIconResolver::complete_sniff`]. Returns `false` if the
    /// dispatch could not be enqueued (e.g. the worker has shut down), in which
    /// case the resolver degrades to the glob fallback rather than hanging.
    ///
    /// This call must not block or read the file — it only schedules work.
    fn dispatch(&self, attempt: SniffAttempt) -> bool;
}

/// The end-to-end resolution seam. In production this holds the pieces that
/// perform the actual lookup (a `ThemeSet`, a `GlobTable`, an icon-id
/// allocator); in tests it counts invocations so single-flight and
/// exactly-once are directly observable.
///
/// A `Resolver` is called only on a cache miss — that is the contract
/// [`FileIconResolver`] enforces, and the source of the exactly-once guarantee.
pub trait Resolver {
    /// Resolve `mime_match` for the given theme name and (icon size, scale).
    /// Called only on a [`MimeThemeCache`] miss. `themes` is the theme set
    /// held by the [`FileIconResolver`], so the implementation can consult
    /// [`ThemeSet::best_directory`] without needing its own copy.
    fn resolve_mime_theme(
        &self,
        mime_match: &MimeMatch,
        theme: &str,
        icon_size: u16,
        scale: u16,
        themes: &ThemeSet,
    ) -> MimeThemeResult;

    /// Assign the opaque handle id for a fully-resolved
    /// (path, fingerprint, generation, MIME, icon-name) tuple. Called only on
    /// a [`HandleCache`] miss.
    fn allocate_handle_id(&self, path: &Path, mime: &str, icon_name: &str) -> u64;
}

/// The stitched-together file-icon resolver: filename to MIME to themed icon
/// directory to handle, with both caches in the path and single-flight
/// coalescing.
///
/// Instances are neither `Send` nor `Sync`; they own their caches and a boxed
/// resolver. The internal policy is pure, so any concurrency wrapper belongs
/// outside.
#[must_use]
pub struct FileIconResolver {
    globs: GlobTable,
    themes: ThemeSet,
    theme_name: String,
    icon_size: u16,
    scale: u16,
    mime_theme_cache: MimeThemeCache,
    handle_cache: HandleCache,
    // Content-sniff policy layer. `None` leaves `resolve` glob-only, so a caller
    // with no dispatcher never opens a file. Its generation is kept in lock-step
    // with the handle cache's, so a theme or scale change invalidates sniff
    // results too.
    sniff: Option<SniffState>,
}

/// The sniff wiring held by a [`FileIconResolver`]: the pure policy cache plus
/// the non-blocking dispatch seam. Held together because they are only ever
/// used together, and only when a sniffer has been configured.
struct SniffState {
    cache: SniffCache,
    dispatcher: Box<dyn SniffDispatcher>,
}

/// The outcome of a single `resolve` call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolveOutcome {
    /// The resolver produced a handle (either from cache or by resolving).
    Resolved(ResolvedHandle),
    /// A resolution for this path is already in flight; the caller should wait
    /// for it to complete or re-poll.
    ///
    /// Reached through the content-sniff path: when the filename yields no
    /// confident glob match, a sniff-cache miss dispatches an off-thread sniff,
    /// and a second `resolve` for the same pending path coalesces on the sniff
    /// cache's in-flight marker. The handle-cache in-flight arm is unreachable
    /// while handle resolution is synchronous.
    InFlight,
    /// The fingerprint source could not produce a fingerprint for `path` (the
    /// file went away, permission denied, …). Without a fingerprint the cache
    /// cannot decide staleness, so it refuses to serve a possibly-stale entry.
    Unavailable,
}

impl FileIconResolver {
    /// Build a resolver around the given theme set and glob table, with the
    /// caches sized to `cache_capacity` each.
    pub fn new(
        globs: GlobTable,
        themes: ThemeSet,
        theme_name: impl Into<String>,
        icon_size: u16,
        scale: u16,
        cache_capacity: usize,
    ) -> Self {
        let theme_name = theme_name.into();
        let mut handle_cache = HandleCache::new(cache_capacity);
        // Stamp the handle cache with the resolution inputs it is about to
        // serve, so a later `set_theme`/`set_scale` is a *change* of key rather
        // than the first time one is set.
        handle_cache.set_resolution_key(ResolutionKey::of(&theme_name, icon_size, scale));
        Self {
            globs,
            themes,
            theme_name,
            icon_size,
            scale,
            mime_theme_cache: MimeThemeCache::new(cache_capacity),
            handle_cache,
            sniff: None,
        }
    }

    /// Attach a content sniffer, enabling the sniff-consult path.
    ///
    /// Without this, `resolve` is glob-only and never opens a file. With it, a
    /// filename that yields no confident glob match (i.e. the glob resolved
    /// only to the [`Certainty::Fallback`] `application/octet-stream`) consults
    /// the [`SniffCache`]. `dispatcher` is the non-blocking seam to the
    /// off-thread sniff worker; `sniff_cache_capacity` bounds the sniff cache.
    ///
    /// The sniff cache is initialised at the resolver's current generation so
    /// its invalidation stays in lock-step with the handle cache.
    pub fn with_sniffer(
        mut self,
        dispatcher: impl SniffDispatcher + 'static,
        sniff_cache_capacity: usize,
    ) -> Self {
        let mut cache = SniffCache::new(sniff_cache_capacity);
        // Align the sniff cache's generation with the handle cache's so a later
        // `bump_generation` invalidates both consistently.
        while cache.generation() < self.handle_cache.generation() {
            cache.bump_generation();
        }
        self.sniff = Some(SniffState {
            cache,
            dispatcher: Box::new(dispatcher),
        });
        self
    }

    /// Feed a completed off-thread sniff back into the resolver's sniff cache.
    /// The opaque `attempt` is carried unchanged by
    /// [`SniffCompletion`](crate::sniff::worker::SniffCompletion); only the
    /// exact identity that remains active can complete. Stale, superseded, and
    /// duplicate terminal completions are rejected.
    ///
    /// A no-op (returns `false`) if no sniffer is configured.
    pub fn complete_sniff(&mut self, attempt: &SniffAttempt, outcome: SniffOutcome) -> bool {
        match &mut self.sniff {
            Some(state) => state.cache.complete(attempt, outcome),
            None => false,
        }
    }

    /// The current handle-cache generation, exposed for tests and for the
    /// caller that owns theme/scale changes to bump.
    pub fn generation(&self) -> Generation {
        self.handle_cache.generation()
    }

    /// Bump the generation on both caches. Callers do this when the active
    /// theme or scale changes: [`MimeThemeCache`] discards all entries (the
    /// key includes theme+scale so nothing survives), [`HandleCache`]
    /// promotes entries whose fingerprint is unchanged instead of discarding.
    pub fn bump_generation(&mut self) {
        self.handle_cache.bump_generation();
        self.mime_theme_cache.bump_generation();
        // A theme or scale change must invalidate a content-sniff result too,
        // since the resolved icon depends on the theme.
        if let Some(state) = &mut self.sniff {
            state.cache.bump_generation();
        }
    }

    /// Re-stamp the handle cache with the current (theme, icon size, scale).
    ///
    /// This is what makes a theme or scale change reach already-resolved paths:
    /// a generation bump alone promotes an unchanged-fingerprint entry, so every
    /// warm row would keep the previous theme's handle.
    fn restamp_resolution(&mut self) {
        self.handle_cache.set_resolution_key(ResolutionKey::of(
            &self.theme_name,
            self.icon_size,
            self.scale,
        ));
    }

    /// Switch the active theme. Bumps the generation on both caches and
    /// re-keys the handle cache, so warm handles resolved under the previous
    /// theme are re-resolved rather than promoted.
    pub fn set_theme(&mut self, theme: impl Into<String>) {
        self.theme_name = theme.into();
        self.restamp_resolution();
        self.bump_generation();
    }

    /// Set the display scale. Bumps the generation on both caches and re-keys
    /// the handle cache, so warm handles resolved at the previous scale are
    /// re-resolved rather than promoted.
    pub fn set_scale(&mut self, scale: u16) {
        self.scale = scale;
        self.restamp_resolution();
        self.bump_generation();
    }

    /// The number of entries currently in the handle cache (for tests and
    /// diagnostics).
    pub fn handle_cache_len(&self) -> usize {
        self.handle_cache.len()
    }

    /// The number of entries currently in the MIME/theme cache.
    pub fn mime_theme_cache_len(&self) -> usize {
        self.mime_theme_cache.len()
    }

    /// Resolve `path` to a handle, going through both caches.
    ///
    /// The pipeline is:
    /// 1. Compute the current fingerprint via `fingerprints`; `None` gives
    ///    [`ResolveOutcome::Unavailable`], since staleness cannot be checked.
    /// 2. `handle_cache.lookup(path, fp)`: a hit returns the cached handle
    ///    without calling `resolver`, in-flight returns
    ///    [`ResolveOutcome::InFlight`], a miss continues.
    /// 3. Determine the MIME match from the filename, and consult the content
    ///    sniffer only under the precedence rule below.
    /// 4. `mime_theme_cache.lookup(key)`: a hit skips the theme traversal, a
    ///    miss calls `resolver.resolve_mime_theme` and inserts.
    /// 5. Mark the handle in flight, call `resolver.allocate_handle_id`,
    ///    complete the handle-cache insertion, and return the new handle.
    ///
    /// `resolver` is called only on a miss: two lookups with the same path,
    /// fingerprint and generation produce exactly one `allocate_handle_id` call.
    ///
    /// # Precedence: glob against sniff
    ///
    /// Only two cases are sniffed, both of them cases the filename cannot
    /// answer:
    ///
    /// 1. No match at all, where the glob table returns [`Certainty::Fallback`]
    ///    (`application/octet-stream`).
    /// 2. An ambiguous match, where two or more MIME types tie at equal top
    ///    precedence (`*.m` is both `text/x-objcsrc` and `text/x-matlab`) and
    ///    the filename resolves to one only by an arbitrary lexicographic
    ///    tiebreak. See
    ///    [`MimeResolution::is_ambiguous`](crate::mime::MimeResolution::is_ambiguous).
    ///
    /// A confident, unambiguous glob match therefore wins and the file is not
    /// opened. That keeps the sniff worker's cost for the names it can help
    /// with, and it resolves the mis-named file (a PNG called `photo.txt`) in
    /// favour of the name, matching the [`Certainty`] ladder where
    /// `FilenameLiteral` and `FilenameGlob` outrank [`Certainty::Content`].
    ///
    /// For a fallback or an ambiguous match, with a sniffer configured, the
    /// sniff cache decides:
    ///
    /// - a cached positive sniff replaces a bare fallback with
    ///   [`Certainty::Content`], and for an ambiguous match wins only if it
    ///   names one of the equal-precedence alternatives;
    /// - a cached negative sniff, or an ambiguous sniff naming anything outside
    ///   the alternatives, keeps the tiebreak primary;
    /// - a sniff-cache miss dispatches an off-thread sniff and returns
    ///   [`ResolveOutcome::InFlight`], with no blocking read here;
    /// - a sniff already in flight also returns `InFlight`, coalesced.
    ///
    /// If the dispatch fails because the worker is gone, `resolve` degrades to
    /// the tiebreak primary rather than hanging.
    pub fn resolve<R: Resolver, F: FingerprintSource>(
        &mut self,
        path: &Path,
        resolver: &R,
        fingerprints: &F,
    ) -> ResolveOutcome {
        // 1. Fingerprint: without one, staleness cannot be checked.
        let Some(fp) = fingerprints.fingerprint(path) else {
            return ResolveOutcome::Unavailable;
        };

        // 2. Handle cache.
        match self.handle_cache.lookup(path, &fp) {
            HandleLookup::Hit(handle) => return ResolveOutcome::Resolved(handle),
            // Unreachable while handle resolution is synchronous: the
            // `mark_in_flight` and `complete` calls below run inside this same
            // call, so the in-flight set is empty on entry. This is a different
            // seam from the content-sniff in-flight path.
            HandleLookup::InFlight => return ResolveOutcome::InFlight,
            HandleLookup::Miss => {}
        }

        // 3. MIME resolution from the filename (I/O-free), preserving ambiguity.
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let resolution = self.globs.resolve_ambiguous_or_fallback(filename);
        let mut mime_match = resolution.primary.clone();

        // 3b. Sniff only when the filename cannot give a single confident
        //     answer: a bare fallback, or an ambiguous hit where two or more
        //     types tie at the top precedence.
        let is_fallback = mime_match.certainty == Certainty::Fallback;
        let is_ambiguous = resolution.is_ambiguous();
        if (is_fallback || is_ambiguous)
            && let Some(state) = &mut self.sniff
        {
            match state.cache.lookup(path, &fp) {
                // A cached positive sniff.
                SniffLookup::Hit(SniffOutcome::Mime(mime)) => {
                    if is_fallback {
                        // Content beats a pure fallback: there was no filename
                        // evidence to respect.
                        mime_match = MimeMatch {
                            mime,
                            certainty: Certainty::Content,
                        };
                    } else if resolution.is_alternative(&mime) {
                        // Content may only pick among the equal-precedence
                        // alternatives the filename put forward.
                        mime_match = MimeMatch {
                            mime,
                            certainty: Certainty::Content,
                        };
                    }
                    // A sniff naming something outside the alternative set is
                    // inconclusive, so the tiebreak primary stands.
                }
                // A cached negative sniff keeps the tiebreak primary.
                SniffLookup::Hit(_) => {}
                // No cached answer: dispatch off-thread so this path stays
                // non-blocking; the answer arrives via `complete_sniff`.
                SniffLookup::Miss => {
                    // Admit into the bounded in-flight table before dispatching,
                    // so a saturated table degrades without queuing unbounded
                    // work. A rejected admission retains no path copy.
                    match state.cache.mark_in_flight(path, fp.clone()) {
                        Admission::Accepted(attempt) => {
                            if state.dispatcher.dispatch(attempt.clone()) {
                                return ResolveOutcome::InFlight;
                            }
                            // Dispatch failed (worker gone / queue full): roll
                            // back only this exact marker so a later frame retries,
                            // without clobbering any superseding attempt.
                            let _ = state.cache.abort_in_flight(&attempt);
                        }
                        // Already in flight for this generation: coalesce.
                        Admission::Busy => return ResolveOutcome::InFlight,
                        // In-flight table saturated: degrade to the tiebreak
                        // rather than dispatch unbounded work.
                        Admission::Full => {}
                    }
                }
                // A sniff for this path is already in flight for the current
                // generation: do not dispatch a duplicate, coalesce.
                SniffLookup::InFlight => return ResolveOutcome::InFlight,
            }
        }

        // 4. MIME/theme cache.
        let key = MimeThemeKey::new(
            mime_match.mime.clone(),
            self.theme_name.clone(),
            self.icon_size,
            self.scale,
        );
        let mime_theme_result = match self.mime_theme_cache.lookup(&key) {
            MimeThemeLookup::Hit(r) => r,
            MimeThemeLookup::Miss => {
                let r = resolver.resolve_mime_theme(
                    &mime_match,
                    &self.theme_name,
                    self.icon_size,
                    self.scale,
                    &self.themes,
                );
                self.mime_theme_cache.insert(key, r.clone());
                r
            }
        };

        // 5. Handle-cache completion under the dispatched generation;
        //    `complete` drops the result if the generation moved meanwhile.
        self.handle_cache.mark_in_flight(path);
        let dispatched_gen = self.handle_cache.generation();
        let icon_name = icon_name_of(&mime_theme_result);
        let id = resolver.allocate_handle_id(path, &mime_match.mime, icon_name);
        let handle = ResolvedHandle::new(id, mime_match.mime, icon_name.to_owned());
        let accepted = self
            .handle_cache
            .complete(path, dispatched_gen, fp.clone(), handle.clone());
        if !accepted {
            // The generation advanced during resolution: nothing was cached, but
            // the caller still gets a handle for this frame.
        }
        ResolveOutcome::Resolved(handle)
    }

    /// A read-only view of the MIME determination for a filename. Reads the
    /// glob table only and touches neither cache.
    pub fn peek_mime(&self, filename: &str) -> MimeMatch {
        self.globs.resolve_or_fallback(filename)
    }

    /// The active theme name.
    pub fn theme_name(&self) -> &str {
        &self.theme_name
    }
}

/// Extract the terminal icon name from a MIME/theme resolution result.
fn icon_name_of(r: &MimeThemeResult) -> &str {
    match r {
        MimeThemeResult::Themed { icon_name, .. } => icon_name,
        MimeThemeResult::Bundled { icon_name } => icon_name,
    }
}

/// A convenience [`FingerprintSource`] over an in-memory map, so tests and
/// caller-side code that already has fingerprints in hand do not need to write
/// a trait impl.
///
/// The absence of a path is treated as `None` (the file is unavailable),
/// same as a filesystem-touching implementation would report.
pub struct MapFingerprintSource {
    map: std::collections::HashMap<PathBuf, Fingerprint>,
}

impl MapFingerprintSource {
    /// An empty source.
    pub fn new() -> Self {
        Self {
            map: std::collections::HashMap::new(),
        }
    }

    /// Set (or replace) the fingerprint at `path`. Returns `self` for chaining.
    pub fn set(&mut self, path: impl Into<PathBuf>, fp: Fingerprint) -> &mut Self {
        self.map.insert(path.into(), fp);
        self
    }

    /// Remove `path`. Subsequent lookups will return `None` (unavailable).
    pub fn remove(&mut self, path: &Path) {
        self.map.remove(path);
    }
}

impl Default for MapFingerprintSource {
    fn default() -> Self {
        Self::new()
    }
}

/// A working default [`Resolver`]: `resolve_mime_theme` asks the [`ThemeSet`]
/// for the best directory and picks an icon name from the MIME candidate chain
/// ([`icon_candidates`](crate::mime::icon_candidates)), and
/// `allocate_handle_id` is a monotonic in-process counter.
///
/// It exists so a caller can construct a [`FileIconResolver`] and start
/// resolving without writing their own [`Resolver`].
pub struct DefaultResolver {
    next_id: std::sync::atomic::AtomicU64,
}

impl DefaultResolver {
    /// A default resolver with ids starting at 1.
    pub fn new() -> Self {
        Self {
            next_id: std::sync::atomic::AtomicU64::new(1),
        }
    }
}

impl Default for DefaultResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Resolver for DefaultResolver {
    fn resolve_mime_theme(
        &self,
        mime_match: &MimeMatch,
        theme: &str,
        icon_size: u16,
        scale: u16,
        themes: &ThemeSet,
    ) -> MimeThemeResult {
        // The branches draw from different sources: a themed result looks up an
        // on-disk host theme, where the specific freedesktop name (`image-png`)
        // is the file to find, while a bundled result must walk the chain to the
        // first name in this crate's Lucide catalog, because the specific name
        // is not one.
        match themes.best_directory(theme, icon_size, scale) {
            Some((theme_name, dir_entry)) => {
                let specific = crate::mime::icon_candidates(&mime_match.mime, None)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| "file".to_owned());
                MimeThemeResult::Themed {
                    theme_name,
                    dir: dir_entry.dir,
                    icon_name: specific,
                }
            }
            None => MimeThemeResult::Bundled {
                icon_name: crate::mime::bundled_icon_candidate(&mime_match.mime, None),
            },
        }
    }

    fn allocate_handle_id(&self, _path: &Path, _mime: &str, _icon_name: &str) -> u64 {
        self.next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }
}

impl FingerprintSource for MapFingerprintSource {
    fn fingerprint(&self, path: &Path) -> Option<Fingerprint> {
        self.map.get(path).cloned()
    }
}
