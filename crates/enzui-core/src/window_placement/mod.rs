//! Application-keyed window placement persistence.
//!
//! [`session`] keeps the compositor's window session, which restores window
//! positions where Wayland lets the compositor do it.
//!
//! Stable keys survive process restarts. Disk dimensions are validated while loading and sanitized when an initial placement is requested. Normal size survives maximization, stale asynchronous replies are rejected, updates coalesce per key, and durable writes use atomic XDG storage. Position and minimized state are not persisted.

use crate::atomic_state::{self, StateLoad};
use iced_core::Size;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub mod session;

/// A stable application-defined window-role identifier that survives process restarts.
///
/// Keys are opaque strings; [`Self::new`] trims whitespace and rejects empty values and storage delimiters.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlacementKey(String);

impl PlacementKey {
    /// Constructs a trimmed key, rejecting empty input and interior `\n`, `\t`, `=`, or NUL delimiters.
    ///
    /// Interior tabs are rejected because they split the tab-separated record format. Leading and trailing whitespace is trimmed.
    pub fn new<S: Into<String>>(s: S) -> Option<Self> {
        let raw = s.into();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        if trimmed
            .chars()
            .any(|c| c == '\n' || c == '\t' || c == '=' || c == '\0')
        {
            return None;
        }
        Some(Self(trimmed.to_owned()))
    }

    /// The stored key text, guaranteed non-empty and free of the delimiters.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// No `From<&str>` implementation: callers must choose fallible validation or explicit lossy sanitization.

/// Maps an application-owned window-role type to a stable persistence key.
pub trait PlacementKeyed {
    /// Returns a key stable across launches for the same logical window role.
    fn placement_key(&self) -> PlacementKey;
}

impl PlacementKey {
    /// Builds a total lossy key by replacing reserved delimiters with `_` and using `_` for empty input.
    ///
    /// Distinct inputs may collide after sanitization.
    #[must_use]
    pub fn sanitized<S: Into<String>>(s: S) -> Self {
        let raw: String = s.into();
        let replaced: String = raw
            .chars()
            .map(|c| match c {
                '\n' | '\t' | '=' | '\0' => '_',
                other => other,
            })
            .collect();
        Self::new(replaced).unwrap_or_else(|| Self("_".to_owned()))
    }
}

/// Persisted size and maximized state for one window key.
///
/// `normal_size` remains the unmaximized restore size; position and minimized state are not stored.
#[derive(Debug, Clone, Copy, PartialEq)]
#[must_use]
pub struct Placement {
    /// Unmaximized restore size. After [`Placement::sanitize`], each axis is finite and lies between [`MIN_SIZE`] and the sanitized output bound.
    pub normal_size: (f32, f32),
    /// Whether the window is currently maximized.
    pub maximized: bool,
}

impl Placement {
    /// The 800×600 unmaximized first-run default.
    pub const DEFAULT_MAIN: Placement = Placement {
        normal_size: (800.0, 600.0),
        maximized: false,
    };

    /// Builds an unmaximized placement from a known width and the framework’s default height.
    ///
    /// For live updates, prefer [`PlacementTracker::record_width`] to preserve the stored height.
    pub const fn from_width(width: f32) -> Placement {
        Placement {
            normal_size: (width, Placement::DEFAULT_MAIN.normal_size.1),
            maximized: false,
        }
    }

    /// Returns whether both axes are finite and strictly positive.
    ///
    /// Values below [`MIN_SIZE`] are valid measurements that may later be clamped; zero, negative, and non-finite values are placeholders.
    #[must_use]
    pub fn has_positive_size(&self) -> bool {
        let (w, h) = self.normal_size;
        w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0
    }

    /// Convert into an `iced::Size` suitable for `iced::window::Settings::size`.
    pub fn to_iced_size(&self) -> Size {
        Size::new(self.normal_size.0, self.normal_size.1)
    }

    /// Returns a finite placement clamped to [`MIN_SIZE`] and sanitized output bounds.
    ///
    /// Positive infinity means no display constraint and uses [`MAX_SIZE`]; other non-finite, negative, or undersized bounds collapse to [`MIN_SIZE`].
    pub fn sanitize(&self, output_bounds: (f32, f32)) -> Placement {
        let bounds = sane_bounds(output_bounds);
        let (w, h) = self.normal_size;
        let sane_w = clamp_axis(w, bounds.0);
        let sane_h = clamp_axis(h, bounds.1);
        Placement {
            normal_size: (sane_w, sane_h),
            maximized: self.maximized,
        }
    }
}

/// Minimum window edge in logical pixels, used to avoid degenerate backend sizes.
pub const MIN_SIZE: f32 = 200.0;

/// Absolute per-axis sanity ceiling, including when output bounds are unknown.
pub const MAX_SIZE: f32 = 32_768.0;

/// Output bounds indicating that no display constraint is currently known.
///
/// Sanitization maps this sentinel to [`MAX_SIZE`]; prefer [`PlacementTracker::initial_unbounded`] at tracker call sites.
pub const UNKNOWN_OUTPUT_BOUNDS: (f32, f32) = (f32::INFINITY, f32::INFINITY);

fn clamp_axis(v: f32, hi: f32) -> f32 {
    if !v.is_finite() {
        return MIN_SIZE.min(hi);
    }
    if v < MIN_SIZE {
        return MIN_SIZE.min(hi);
    }
    if v > hi {
        return hi;
    }
    v
}

fn sane_bounds((w, h): (f32, f32)) -> (f32, f32) {
    let clamp_bound = |b: f32| {
        if b == f32::INFINITY {
            MAX_SIZE
        } else if !b.is_finite() || b < MIN_SIZE {
            MIN_SIZE
        } else if b > MAX_SIZE {
            MAX_SIZE
        } else {
            b
        }
    };
    (clamp_bound(w), clamp_bound(h))
}

// Records refused placeholder resizes in first-seen order in every build profile.
static PLACEHOLDER_RESIZES: std::sync::Mutex<Vec<(String, (f32, f32))>> =
    std::sync::Mutex::new(Vec::new());

/// Returns the first refused placeholder resize per key, in first-seen order.
///
/// Compositors may emit zero-sized resizes, so refusal is recorded in release builds and reported once per key instead of panicking.
#[must_use]
pub fn placeholder_resizes() -> Vec<(String, (f32, f32))> {
    PLACEHOLDER_RESIZES
        .lock()
        .map(|v| v.clone())
        .unwrap_or_default()
}

// Records the first refused placeholder per key and prints it once, in all build profiles.
fn report_placeholder_resize(key: &PlacementKey, size: (f32, f32)) {
    let Ok(mut seen) = PLACEHOLDER_RESIZES.lock() else {
        // A poisoned mutex means a previous panic happened while holding it.
        // Reporting is diagnostics; never let it escalate.
        return;
    };
    let first_for_this_key = !seen.iter().any(|(k, _)| k == key.as_str());
    if first_for_this_key {
        seen.push((key.as_str().to_owned(), size));
        log::warn!(
            target: "enzui::placement",
            "enzui placement: ignored a resize of {size:?} for window key \
             {:?} — an axis was zero, negative or non-finite, so this is a \
             placeholder rather than a measurement (a minimize delivers 0x0). \
             The last good geometry is kept. If your app only tracks a width, \
             call `PlacementTracker::record_width` instead of passing 0.0 for \
             the height.",
            key.as_str(),
        );
    }
}

/// An opaque key-bound token for one asynchronous placement query.
///
/// Tokens are one-shot, invalidated by later mutation or query replacement, and never reused; zero is reserved as an inert exhaustion token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct QueryToken {
    // Zero is reserved and never registered as outstanding.
    nonce: u64,
}

/// Coalesces placement updates per key and rejects stale, cross-key, or replayed query replies.
///
/// The tracker's record and query methods perform no filesystem I/O. These
/// are the synchronous filesystem boundaries, each blocking on the disk:
///
/// - [`PlacementTracker::flush_to`] writes the pending placements durably.
/// - [`flush_shared`] flushes a shared tracker through the atomic writer.
/// - [`save_atomic`] replaces a placement file atomically.
/// - [`load`] reads a placement file.
///
/// [`flush_shared_with`] is an injected transaction seam whose closure may
/// perform no I/O.
#[derive(Debug, Default)]
pub struct PlacementTracker {
    // Newest unpersisted placement per key.
    pending: BTreeMap<PlacementKey, Placement>,
    // Last placement successfully flushed per key.
    last_persisted: BTreeMap<PlacementKey, Placement>,
    // Latest unspent query nonce per key; mutations, superseding queries, or successful replies remove it.
    outstanding: BTreeMap<PlacementKey, u64>,
    // Monotonic nonce source shared across keys; exhaustion yields reserved zero.
    next_nonce: u64,
    // Short critical sections around snapshot/commit atomicity; never held while a writer runs.
    durability: std::sync::Arc<Mutex<()>>,
    // Serializes publisher threads across the writer. Held while the injected writer runs so
    // that concurrent flushes cannot let an older snapshot's on-disk rename land after a newer
    // one's. Distinct from `durability` so a reentrant flush from inside a writer neither waits
    // on nor conflicts with the durability mutex.
    write_gate: std::sync::Arc<Mutex<()>>,
}

impl PlacementTracker {
    /// Creates an empty tracker; production callers should normally seed one with [`Self::with_loaded`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Seeds `last_persisted` from a loaded snapshot without creating pending writes.
    pub fn with_loaded(loaded: BTreeMap<PlacementKey, Placement>) -> Self {
        Self {
            pending: BTreeMap::new(),
            last_persisted: loaded,
            outstanding: BTreeMap::new(),
            next_nonce: 0,
            durability: std::sync::Arc::default(),
            write_gate: std::sync::Arc::default(),
        }
    }

    /// Return the placement currently associated with `key`, whether it comes
    /// from a pending edit or the last persisted snapshot. Pending wins.
    pub fn get(&self, key: &PlacementKey) -> Option<Placement> {
        self.pending
            .get(key)
            .copied()
            .or_else(|| self.last_persisted.get(key).copied())
    }

    /// Returns stored or default placement sanitized against known `output_bounds`.
    ///
    /// Use [`Self::initial_unbounded`] when no output size is known.
    pub fn initial(
        &self,
        key: &PlacementKey,
        default: Placement,
        output_bounds: (f32, f32),
    ) -> Placement {
        self.get(key).unwrap_or(default).sanitize(output_bounds)
    }

    /// Returns initial placement when no output size is yet known.
    ///
    /// The result is sanitized against [`UNKNOWN_OUTPUT_BOUNDS`], remaining finite and within [`MIN_SIZE`]..=[`MAX_SIZE`] without inventing a display ceiling.
    pub fn initial_unbounded(&self, key: &PlacementKey, default: Placement) -> Placement {
        self.initial(key, default, UNKNOWN_OUTPUT_BOUNDS)
    }

    /// Records the newest measured size for `key` without writing it immediately.
    ///
    /// Zero, negative, or non-finite axes are refused and reported through [`placeholder_resizes`]. Resizes received while the tracked state is maximized do not replace `normal_size`.
    pub fn record_resize(&mut self, key: PlacementKey, size: (f32, f32)) {
        if !(Placement {
            normal_size: size,
            maximized: false,
        })
        .has_positive_size()
        {
            // Preserve the last measured geometry when either axis is a placeholder.
            report_placeholder_resize(&key, size);
            return;
        }
        self.bump_generation(&key);
        let current = self.get(&key).unwrap_or(Placement::DEFAULT_MAIN);
        let placement = Placement {
            // Resize reports while maximized must not overwrite the restore size.
            normal_size: if current.maximized {
                current.normal_size
            } else {
                size
            },
            maximized: current.maximized,
        };
        self.pending.insert(key, placement);
    }

    /// Records a measured width while preserving the key’s stored height, or using the default height when no placement exists.
    ///
    /// It follows the same coalescing, query invalidation, and maximized-state rules as [`Self::record_resize`].
    pub fn record_width(&mut self, key: PlacementKey, width: f32) {
        let height = self
            .get(&key)
            .unwrap_or(Placement::DEFAULT_MAIN)
            .normal_size
            .1;
        self.record_resize(key, (width, height));
    }

    /// Records maximized state while preserving `normal_size`; record the pre-maximize size before setting `true`.
    pub fn record_maximized(&mut self, key: PlacementKey, maximized: bool) {
        self.bump_generation(&key);
        let current = self.get(&key).unwrap_or(Placement::DEFAULT_MAIN);
        self.pending.insert(
            key,
            Placement {
                normal_size: current.normal_size,
                maximized,
            },
        );
    }

    /// Starts the latest asynchronous query for `key` and returns a key-bound one-shot token.
    ///
    /// A later query or mutation for the key invalidates it. Cross-key or replayed delivery is refused without consuming other state. Exhaustion returns inert zero.
    pub fn begin_query(&mut self, key: &PlacementKey) -> QueryToken {
        // Mint tokens unique within this tracker; zero is reserved and never stored.
        let Some(nonce) = self.next_nonce.checked_add(1) else {
            return QueryToken { nonce: 0 };
        };
        self.next_nonce = nonce;
        self.outstanding.insert(key.clone(), nonce);
        QueryToken { nonce }
    }

    /// Applies a query reply only when `token` is the current unspent token for `key`.
    ///
    /// Returns `false` for cross-key, invalidated, superseded, or replayed tokens without consuming unrelated state.
    pub fn apply_query_reply(
        &mut self,
        key: &PlacementKey,
        token: QueryToken,
        reply: Placement,
    ) -> bool {
        // Key-local removal enforces both key binding and one-shot consumption.
        match self.outstanding.get(key).copied() {
            Some(current) if current == token.nonce => {
                self.outstanding.remove(key);
                self.pending.insert(key.clone(), reply);
                true
            }
            _ => false,
        }
    }

    /// Whether at least one key has an unsaved change.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Returns the coalesced pending `(key, placement)` entries.
    pub fn pending_snapshot(&self) -> BTreeMap<PlacementKey, Placement> {
        self.pending.clone()
    }

    // Captures the exact merged write snapshot without draining pending state; returns `None` when nothing is pending.
    fn begin_flush(&mut self) -> Option<FlushTransaction> {
        if self.pending.is_empty() {
            return None;
        }
        // Publish persisted and pending keys together, with pending values winning.
        let mut snapshot = self.last_persisted.clone();
        for (k, v) in &self.pending {
            snapshot.insert(k.clone(), *v);
        }
        Some(FlushTransaction {
            snapshot,
            written: self.pending.clone(),
        })
    }

    // Promotes only written values that remain unchanged; newer mutations stay pending and later commits are not regressed.
    fn commit_flush(&mut self, transaction: &FlushTransaction) -> usize {
        let mut promoted = 0;
        for (key, written) in &transaction.written {
            match self.pending.get(key) {
                Some(current) if current == written => {
                    self.last_persisted.insert(key.clone(), *written);
                    self.pending.remove(key);
                    promoted += 1;
                }
                Some(_) => {
                    // Keep the newer value pending while remembering what this transaction wrote.
                    self.last_persisted.insert(key.clone(), *written);
                }
                None => {
                    // Do not overwrite state already committed by a later publisher.
                }
            }
        }
        promoted
    }

    // Returns the durability lock, held only around snapshot/commit atomicity and released before any writer runs.
    fn durability_lock(&self) -> std::sync::Arc<Mutex<()>> {
        std::sync::Arc::clone(&self.durability)
    }

    // Returns the write-gate lock, which serializes publisher threads across the injected writer.
    fn write_gate_lock(&self) -> std::sync::Arc<Mutex<()>> {
        std::sync::Arc::clone(&self.write_gate)
    }

    /// Atomically writes all pending placements to `path` as a synchronous durable operation.
    ///
    /// Success selectively promotes the written values. Failure preserves pending state; a parent-directory sync error may be reported after rename made the new file visible. Direct and shared flushes share the write-gate so cross-thread publishers serialize against each other without ever holding the durability mutex across the durable write.
    ///
    /// # Errors
    ///
    /// An I/O error from the atomic write.
    pub fn flush_to(&mut self, path: &Path) -> io::Result<()> {
        // Serializes with `flush_shared_with` across the durable write through a shared
        // write-gate. `&mut self` alone excludes reentry into `flush_to`; the shared gate is
        // needed only to sequence against publishers on other threads.
        let write_gate = self.write_gate_lock();
        let writing = write_gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let transaction = {
            let durability = self.durability_lock();
            let publishing = durability
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(transaction) = self.begin_flush() else {
                drop(publishing);
                drop(writing);
                return Ok(());
            };
            drop(publishing);
            transaction
        };
        // Neither the tracker nor the durability mutex is live for the durable write.
        save_atomic(path, transaction.snapshot())?;
        // Release the write-gate before commit to match `flush_shared_with`'s ordering; commit
        // remains under a brief durability lock only.
        drop(writing);
        let durability = self.durability_lock();
        let _publishing = durability
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.commit_flush(&transaction);
        Ok(())
    }

    // Every mutation clears the key’s outstanding query token.
    fn bump_generation(&mut self, key: &PlacementKey) {
        self.outstanding.remove(key);
    }
}

/// Placement operations keyed by an application-owned window-role type.
///
/// Methods forward through [`PlacementKeyed::placement_key`] to the corresponding key-based tracker operation.
impl PlacementTracker {
    /// [`PlacementTracker::get`], keyed by the app's own window role.
    pub fn get_for<K: PlacementKeyed + ?Sized>(&self, window: &K) -> Option<Placement> {
        self.get(&window.placement_key())
    }

    /// [`PlacementTracker::initial`], keyed by the app's own window role.
    pub fn initial_for<K: PlacementKeyed + ?Sized>(
        &self,
        window: &K,
        default: Placement,
        output_bounds: (f32, f32),
    ) -> Placement {
        self.initial(&window.placement_key(), default, output_bounds)
    }

    /// [`PlacementTracker::initial_unbounded`], keyed by the application’s window role.
    pub fn initial_unbounded_for<K: PlacementKeyed + ?Sized>(
        &self,
        window: &K,
        default: Placement,
    ) -> Placement {
        self.initial_unbounded(&window.placement_key(), default)
    }

    /// [`PlacementTracker::record_resize`], keyed by the app's own window role.
    pub fn record_resize_for<K: PlacementKeyed + ?Sized>(&mut self, window: &K, size: (f32, f32)) {
        self.record_resize(window.placement_key(), size);
    }

    /// [`PlacementTracker::record_width`], keyed by the app's own window role.
    pub fn record_width_for<K: PlacementKeyed + ?Sized>(&mut self, window: &K, width: f32) {
        self.record_width(window.placement_key(), width);
    }

    /// [`PlacementTracker::record_maximized`], keyed by the app's own window
    /// role.
    pub fn record_maximized_for<K: PlacementKeyed + ?Sized>(
        &mut self,
        window: &K,
        maximized: bool,
    ) {
        self.record_maximized(window.placement_key(), maximized);
    }

    /// [`PlacementTracker::begin_query`], keyed by the app's own window role.
    pub fn begin_query_for<K: PlacementKeyed + ?Sized>(&mut self, window: &K) -> QueryToken {
        self.begin_query(&window.placement_key())
    }

    /// [`PlacementTracker::apply_query_reply`], keyed by the app's own window
    /// role.
    pub fn apply_query_reply_for<K: PlacementKeyed + ?Sized>(
        &mut self,
        window: &K,
        token: QueryToken,
        reply: Placement,
    ) -> bool {
        self.apply_query_reply(&window.placement_key(), token, reply)
    }
}

// In-flight flush containing the full snapshot and pending subset eligible for commit.
#[derive(Debug, Clone, PartialEq)]
struct FlushTransaction {
    // Full published snapshot: persisted and pending values, with pending winning.
    snapshot: BTreeMap<PlacementKey, Placement>,
    // Pending subset eligible for selective commit.
    written: BTreeMap<PlacementKey, Placement>,
}

impl FlushTransaction {
    #[must_use]
    fn snapshot(&self) -> &BTreeMap<PlacementKey, Placement> {
        &self.snapshot
    }
}

fn lock_recovering(
    tracker: &Mutex<PlacementTracker>,
) -> std::sync::MutexGuard<'_, PlacementTracker> {
    tracker
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Flushes a shared tracker synchronously without holding its tracker mutex during the durable write.
///
/// # Errors
///
/// An I/O error from the atomic write.
pub fn flush_shared(tracker: &Mutex<PlacementTracker>, path: &Path) -> io::Result<()> {
    flush_shared_with(tracker, path, save_atomic)
}

// Per-tracker stack of publishers active on this thread. A bare boolean would coalesce every
// tracker's flush behind any other tracker's writer on the same thread; the key is the raw
// address of the tracker's own `Mutex<PlacementTracker>` so only a truly reentrant flush on the
// same tracker coalesces, and unrelated trackers publish independently.
thread_local! {
    static PLACEMENT_PUBLISHING: std::cell::RefCell<Vec<usize>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

// RAII helper that stacks the current tracker's identity while its `flush_shared_with` runs and
// unstacks it on drop, including on panic-unwinding through the writer.
struct PublishingGuard {
    key: usize,
}

impl PublishingGuard {
    // Returns `None` when this thread is already publishing the same tracker, coalescing the
    // reentrant call so it cannot wait on the write-gate this thread already holds for that
    // tracker. Reentry on a different tracker returns `Some` and proceeds normally.
    fn enter(tracker: &Mutex<PlacementTracker>) -> Option<Self> {
        let key = std::ptr::from_ref(tracker).cast::<()>() as usize;
        PLACEMENT_PUBLISHING.with(|cell| {
            let mut stack = cell.borrow_mut();
            if stack.contains(&key) {
                None
            } else {
                stack.push(key);
                Some(Self { key })
            }
        })
    }
}

impl Drop for PublishingGuard {
    fn drop(&mut self) {
        PLACEMENT_PUBLISHING.with(|cell| {
            let mut stack = cell.borrow_mut();
            if let Some(pos) = stack.iter().rposition(|&k| k == self.key) {
                stack.swap_remove(pos);
            }
        });
    }
}

/// Runs a flush transaction with an injected writer.
///
/// Neither the tracker mutex nor the durability mutex is live while the writer runs. Cross-thread
/// publishers are totally ordered by a per-tracker write-gate held only across the writer; the
/// gate is released before the tracker is reacquired for commit so a caller that already owns
/// the tracker while waiting on the gate cannot deadlock. A same-thread call that re-enters this
/// function on the same tracker coalesces to `Ok(())`; a reentrant call on a different tracker
/// publishes normally. Writer failure commits nothing and leaves the pending set intact for a
/// later retry.
///
/// # Errors
/// Returns the writer's `io::Error` verbatim; a parent-directory sync error can be reported
/// after the rename has already made the new destination visible.
///
/// # Panics
/// A panic from `write` propagates; the per-tracker reentry stack and both internal locks are
/// released on unwind. Poisoned tracker or lock guards are recovered in place.
pub fn flush_shared_with<W>(
    tracker: &Mutex<PlacementTracker>,
    path: &Path,
    write: W,
) -> io::Result<()>
where
    W: FnOnce(&Path, &BTreeMap<PlacementKey, Placement>) -> io::Result<()>,
{
    // A writer that re-enters `flush_shared_with` on the same tracker on this thread cannot
    // wait on the outer publisher; it coalesces to Ok, leaving its still-pending values for a
    // later flush. A reentrant call on a different tracker proceeds normally.
    let Some(_reentry) = PublishingGuard::enter(tracker) else {
        return Ok(());
    };

    // Serialize publisher threads across the writer without ever holding the durability mutex
    // through the durable write. Same-thread reentry on this tracker is impossible here because
    // the per-tracker publishing guard above already coalesced the recursive call to Ok.
    let write_gate = lock_recovering(tracker).write_gate_lock();
    let writing = write_gate
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let transaction = {
        let durability = lock_recovering(tracker).durability_lock();
        let publishing = durability
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut guard = lock_recovering(tracker);
        let Some(transaction) = guard.begin_flush() else {
            drop(guard);
            drop(publishing);
            drop(writing);
            return Ok(());
        };
        drop(guard);
        drop(publishing);
        transaction
    };

    // Writer runs with only the write-gate held. The durability mutex is not live, so a writer
    // that re-enters (coalesced above) or an unrelated thread taking the durability mutex briefly
    // (for example a `flush_to` snapshot on another thread) cannot deadlock this publisher.
    let result = write(path, transaction.snapshot());
    result?;

    // Release the write-gate before reacquiring the tracker for commit. Holding the write-gate
    // across a tracker acquire deadlocks a `flush_to` caller that already owns the tracker while
    // waiting on the write-gate; releasing here breaks that circular wait. Commit ordering is
    // still safe because `commit_flush` is idempotent under interleaved publishers: a later
    // commit that promoted our key first drops us into the None branch, and a pending value
    // mutated after the write leaves `last_persisted` at our snapshot and stays retryable.
    drop(writing);
    let durability = lock_recovering(tracker).durability_lock();
    let publishing = durability
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    lock_recovering(tracker).commit_flush(&transaction);
    drop(publishing);
    Ok(())
}

/// Allows a [`PlacementKey`] to use the role-keyed forwarding methods directly.
impl PlacementKeyed for PlacementKey {
    fn placement_key(&self) -> PlacementKey {
        self.clone()
    }
}

// Format identifier combined with `FILE_VERSION` by `atomic_state`.
const FORMAT_ID: &str = "placement";

// Format version; mismatched versions are not parsed.
const FILE_VERSION: u32 = 1;

/// The file name placement is stored under, inside the app's XDG state dir.
pub const STATE_FILE_NAME: &str = "placement.state";

/// Resolves `$XDG_STATE_HOME/<app>/placement.state`, falling back to `$HOME/.local/state/<app>/placement.state`.
///
/// Returns `Ok(None)` when there is no usable absolute root.
///
/// # Errors
/// Returns [`crate::persist::XdgPathError`] unless `app` is one ordinary path component.
pub fn default_state_path(app: &str) -> Result<Option<PathBuf>, crate::persist::XdgPathError> {
    atomic_state::xdg_state_path(app, STATE_FILE_NAME)
}

/// Resolves the application placement path through an injected [`crate::persist::XdgResolver`].
///
/// Returns `Ok(None)` for an unavailable root.
///
/// # Errors
/// Returns [`crate::persist::XdgPathError`] unless `app` is one ordinary path component.
pub fn state_path_with_resolver(
    resolver: &crate::persist::XdgResolver,
    app: &str,
) -> Result<Option<PathBuf>, crate::persist::XdgPathError> {
    resolver.resolve_state(app, STATE_FILE_NAME)
}

// Encodes the placement record body; `atomic_state` adds the framing header.
fn encode_body(snapshot: &BTreeMap<PlacementKey, Placement>) -> String {
    let mut buf = String::new();
    // Retain the advisory `keys=` line for compatibility; count mismatches do not reject a file.
    let _ = writeln!(buf, "keys={}", snapshot.len());
    for (k, p) in snapshot {
        // Records are tab-separated; validated keys cannot contain record delimiters.
        let _ = writeln!(
            buf,
            "key={}\twidth={}\theight={}\tmax={}",
            k.as_str(),
            p.normal_size.0,
            p.normal_size.1,
            u8::from(p.maximized),
        );
    }
    buf
}

fn parse_body_with_limits(
    body: &str,
    limits: atomic_state::LineBodyLimits,
) -> Result<BTreeMap<PlacementKey, Placement>, String> {
    // Accept bodies with or without the advisory `keys=` line.
    let mut lines = body.lines();
    let first = lines.next();
    let starts_with_count = matches!(first, Some(line) if line.starts_with("keys="));
    let records = if starts_with_count {
        body.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        body
    };

    let mut out = BTreeMap::new();
    let mut decoded = 0usize;
    atomic_state::visit_line_body(records, limits, |line| {
        decoded += 1;
        // Reject the body when any nonempty placement record is malformed.
        match parse_line(line) {
            Some((key, placement)) => {
                out.insert(key, placement);
                Ok(())
            }
            None => Err(format!("malformed line {}: {line:?}", decoded + 2)),
        }
    })
    .map_err(|error| error.to_string())?;
    Ok(out)
}

/// Atomically serializes a snapshot using a private no-follow temp file, file sync, rename, and parent-directory sync.
///
/// A parent-directory sync error may be returned after the new destination is visible. Other pre-rename failures leave the previous destination in place.
///
/// # Errors
///
/// An I/O error from staging, writing, renaming or synchronizing.
pub fn save_atomic(path: &Path, snapshot: &BTreeMap<PlacementKey, Placement>) -> io::Result<()> {
    let body = encode_body(snapshot);
    atomic_state::save_atomic(path, FORMAT_ID, FILE_VERSION, &body)
}

/// Loads placement state synchronously, returning defaults for absent, malformed, wrong-version, or unreadable files.
///
/// A framing mismatch may be read through the bounded alternate-header parser and then migrated best-effort with [`save_atomic`]; other refusals are not reread.
pub fn load(path: &Path) -> LoadOutcome {
    load_with_limits(
        path,
        atomic_state::MAX_STATE_FILE_BYTES,
        atomic_state::LineBodyLimits::PRODUCTION,
    )
}

fn load_with_limits(
    path: &Path,
    max_bytes: u64,
    line_limits: atomic_state::LineBodyLimits,
) -> LoadOutcome {
    // Load through the no-follow shared loader.
    match atomic_state::load_bounded(path, FORMAT_ID, FILE_VERSION, max_bytes) {
        (StateLoad::Loaded(body), _) => match parse_body_with_limits(&body, line_limits) {
            Ok(map) => LoadOutcome::Loaded(map),
            Err(note) => LoadOutcome::UsedDefaults { note },
        },
        (StateLoad::UsedDefaults { note }, _) => LoadOutcome::UsedDefaults { note },
    }
}

// Parses only finite positive dimensions, matching runtime resize validation.
fn parse_finite_positive(s: &str) -> Option<f32> {
    let v: f32 = s.parse().ok()?;
    (v.is_finite() && v > 0.0).then_some(v)
}

fn parse_line(line: &str) -> Option<(PlacementKey, Placement)> {
    #[cfg(test)]
    PLACEMENT_LINE_DECODES.with(|count| count.set(count.get() + 1));

    let mut key = None;
    let mut width = None;
    let mut height = None;
    let mut maxi = None;
    for part in line.split('\t') {
        let (name, value) = part.split_once('=')?;
        match name {
            "key" => key = PlacementKey::new(value),
            "width" => width = parse_finite_positive(value),
            "height" => height = parse_finite_positive(value),
            "max" => {
                maxi = match value {
                    "0" => Some(false),
                    "1" => Some(true),
                    _ => None,
                };
            }
            _ => {}
        }
    }
    Some((
        key?,
        Placement {
            normal_size: (width?, height?),
            maximized: maxi?,
        },
    ))
}

/// The result of a [`load`] call.
#[derive(Debug, Clone, PartialEq)]
pub enum LoadOutcome {
    /// Parse succeeded (possibly with an empty map, if the file was well-formed
    /// but held zero keys).
    Loaded(BTreeMap<PlacementKey, Placement>),
    /// Loading failed or found no usable state, so callers should continue with defaults; `note` is diagnostic only.
    UsedDefaults {
        /// Diagnostic reason for using defaults; callers must not branch on its text.
        note: String,
    },
}

impl LoadOutcome {
    /// Unwrap into a map, using an empty map for [`LoadOutcome::UsedDefaults`].
    pub fn into_map(self) -> BTreeMap<PlacementKey, Placement> {
        match self {
            LoadOutcome::Loaded(m) => m,
            LoadOutcome::UsedDefaults { .. } => BTreeMap::new(),
        }
    }
}

#[cfg(test)]
thread_local! {
    static PLACEMENT_LINE_DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod tests;
