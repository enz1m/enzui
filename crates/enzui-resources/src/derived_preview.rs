//! Optional, versioned storage and request scheduling for local derived rasters.
//!
//! Derivation and source identity belong to the caller. [`PreviewStore`] performs
//! synchronous file I/O and must run on an owned blocking worker. Storage failures
//! are reported alongside usable pixels; they do not turn a successful derivation
//! into a failed preview. PNG entries retain alpha and contain no source metadata.
//!
//! A root is exclusively owned by this cache format. Cooperating instances use a
//! retained advisory lock for publication, census, persistent recency and clear.
//! Derivation runs outside that lock; separate processes may derive the same miss.
//! The persisted epoch prevents a clear from being undone by older derivations.
//! The first operation initializes the root-wide disk policy; later opens respect
//! it until `set_disk_budget` changes it. Zero retains only coordination files.
//! Hostile same-user writers are outside the advisory-lock contract.

use crate::cache_identity::digest_bytes_with_domain;
use enzui_security::secure_dir::SecureRoot;
use enzui_security::tempstage::RootTempStage;
use iced_core::image::Handle;
use image::ImageEncoder;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

const FORMAT: &[u8] = b"enzui-derived-preview/png-rgba-v1";
const STATE: &str = "epoch";
const LOCK: &str = "lock";
const HEADER: usize = 88;
const BLOCK: u64 = 4096;

/// A caller's source version, target bounds, recipe and decoder version.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PreviewKey {
    digest: [u8; 32],
    width: u32,
    height: u32,
}

impl PreviewKey {
    /// Build an unambiguous identity. `source_version` must change whenever the
    /// source changes; `algorithm_version` must cover every derivation dependency.
    /// Zero target bounds are refused by [`PreviewStore::get_or_derive`].
    pub fn new(
        source_version: &[u8],
        target: (u32, u32),
        recipe: &[u8],
        algorithm_version: &[u8],
    ) -> Self {
        let mut material = Vec::new();
        for part in [source_version, recipe, algorithm_version] {
            material.extend_from_slice(&(part.len() as u64).to_be_bytes());
            material.extend_from_slice(part);
        }
        material.extend_from_slice(&target.0.to_be_bytes());
        material.extend_from_slice(&target.1.to_be_bytes());
        Self {
            digest: digest_bytes_with_domain(FORMAT, &material),
            width: target.0,
            height: target.1,
        }
    }

    /// Requested maximum dimensions; derivations may preserve aspect ratio.
    pub fn target(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn name(&self) -> OsString {
        let mut name = String::with_capacity(68);
        for byte in self.digest {
            use std::fmt::Write;
            let _ = write!(name, "{byte:02x}");
        }
        name.push_str(".png");
        name.into()
    }
}

/// Disk and decoded allocation limits, chosen by the application.
#[derive(Clone, Copy, Debug)]
pub struct PreviewStoreConfig {
    /// Total conservative disk bytes, including coordination and a staging reserve.
    /// Zero disables and prunes previews; coordination remains reported in usage.
    pub disk_bytes: u64,
    /// Maximum RGBA bytes of one derived or reloaded preview.
    pub max_preview_rgba_bytes: u64,
}

/// Observed allocated entry usage and permanent coordination overhead.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PreviewUsage {
    /// Total conservative allocation, including coordination and staging reserve.
    pub disk_bytes: u64,
    /// Root, advisory lock and epoch allocation; retained even when disabled.
    pub coordination_bytes: u64,
    /// Persisted raster count.
    pub entries: usize,
}

/// A usable raster and the independent cache outcome.
#[derive(Clone, Debug)]
pub struct PreviewResult {
    /// Resolved RGBA pixels, ready for rendering.
    pub handle: Handle,
    /// Whether the pixels were loaded without invoking the derivation.
    pub disk_hit: bool,
    /// Storage refusal or failure, if derivation succeeded without caching.
    pub cache_error: Option<String>,
}

/// Why no usable preview was produced.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PreviewError {
    /// Caller validation no longer matches the request's source version.
    SourceChanged,
    /// Cancellation was observed before publication.
    Cancelled,
    /// A target or returned RGBA buffer violates the request's bounds.
    InvalidRaster,
    /// Caller derivation failed.
    Derivation(String),
    /// The application pool refused or interrupted the flight.
    Worker(enzui_core::blocking_task::BlockingTaskError),
}

impl std::fmt::Display for PreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for PreviewError {}

/// Worker-only, caller-root persistent storage. Opening failure disables storage.
pub struct PreviewStore {
    root: Option<SecureRoot>,
    config: PreviewStoreConfig,
    failure: Option<String>,
}

impl PreviewStore {
    /// Adopt a private root on a blocking worker. Every operation remains
    /// descriptor-relative; an unsafe or replaced root becomes a cache miss.
    pub fn open(root: impl AsRef<Path>, config: PreviewStoreConfig) -> Self {
        match SecureRoot::open(root.as_ref()) {
            Ok(root) => Self {
                root: Some(root),
                config,
                failure: None,
            },
            Err(error) => Self {
                root: None,
                config,
                failure: Some(format!("{error:?}")),
            },
        }
    }

    /// Load or derive a bounded RGBA raster. `source_valid` must verify the
    /// captured fingerprint before reuse and after derivation. Both callbacks
    /// execute on the calling worker, never on an implicit thread. Pass the
    /// supplied cancellation callback into cooperative decoders. Derivers are
    /// trusted code and must bound their own allocations before producing pixels.
    ///
    /// # Errors
    /// Returns a source-version, cancellation, raster-bound or caller derivation error.
    pub fn get_or_derive(
        &self,
        key: &PreviewKey,
        visible: &[PreviewKey],
        source_valid: impl Fn() -> bool,
        cancelled: impl Fn() -> bool,
        derive: impl FnOnce(&dyn Fn() -> bool) -> Result<Handle, PreviewError>,
    ) -> Result<PreviewResult, PreviewError> {
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        if !source_valid() {
            return Err(PreviewError::SourceChanged);
        }
        if key.width == 0 || key.height == 0 {
            return Err(PreviewError::InvalidRaster);
        }
        let mut failure = self.failure.clone();
        let mut epoch = None;
        if let Some(root) = &self.root {
            match with_lock(root, || {
                let state = read_state(root, self.config.disk_bytes)?;
                epoch = Some(state.0);
                prune(root, entry_budget(root, state.2)?, visible, 0)?;
                if state.2 == 0 {
                    return Ok(None);
                }
                match read_raster(root, key, self.config.max_preview_rgba_bytes) {
                    // The newest entry stays the newest without being
                    // written again: no order or eviction can change.
                    Ok(Some((handle, _, clock))) if clock == state.1 => Ok(Some((handle, None))),
                    Ok(Some((handle, bytes, _))) => {
                        let recency = (|| {
                            prune(
                                root,
                                entry_budget(root, state.2)?,
                                visible,
                                allocated_len((HEADER + bytes.len()) as u64),
                            )?;
                            let clock = next_clock(root, state)?;
                            write_entry(root, key, clock, &bytes)?;
                            prune(root, entry_budget(root, state.2)?, visible, 0)
                        })();
                        Ok(Some((
                            handle,
                            recency.err().map(|error: io::Error| error.to_string()),
                        )))
                    }
                    Ok(None) => Ok(None),
                    Err(_) => {
                        root.unlink_entry(&key.name())?;
                        Ok(None)
                    }
                }
            }) {
                Ok(Some((handle, cache_error))) => {
                    if cancelled() {
                        return Err(PreviewError::Cancelled);
                    }
                    if !source_valid() {
                        return Err(PreviewError::SourceChanged);
                    }
                    return Ok(PreviewResult {
                        handle,
                        disk_hit: true,
                        cache_error,
                    });
                }
                Ok(None) => (),
                Err(error) => failure = Some(error.to_string()),
            }
        }
        let handle = derive(&cancelled)?;
        validate_handle(&handle, key, self.config.max_preview_rgba_bytes)?;
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        if !source_valid() {
            return Err(PreviewError::SourceChanged);
        }
        if let (Some(root), Some(epoch)) = (&self.root, epoch) {
            let stored = encode(&handle).and_then(|bytes| {
                with_lock(root, || {
                    let state = read_state(root, self.config.disk_bytes)?;
                    if state.0 != epoch || state.2 == 0 || cancelled() {
                        return Ok(());
                    }
                    if !source_valid() {
                        return Err(io::Error::other("source changed before cache publication"));
                    }
                    let reserve = allocated_len((HEADER + bytes.len()) as u64);
                    let budget = entry_budget(root, state.2)?;
                    if reserve > budget {
                        return Ok(());
                    }
                    prune(root, budget, visible, reserve)?;
                    let clock = next_clock(root, state)?;
                    write_entry(root, key, clock, &bytes)?;
                    prune(root, entry_budget(root, state.2)?, visible, 0)
                })
            });
            if let Err(error) = stored {
                failure = Some(error.to_string());
            }
        }
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        if !source_valid() {
            return Err(PreviewError::SourceChanged);
        }
        Ok(PreviewResult {
            handle,
            disk_hit: false,
            cache_error: failure,
        })
    }

    /// Observe storage usage on a blocking worker.
    ///
    /// # Errors
    /// Returns a root, lock or incomplete-census error rather than partial usage.
    pub fn usage(&self) -> io::Result<PreviewUsage> {
        let root = self
            .root
            .as_ref()
            .ok_or_else(|| io::Error::other("cache unavailable"))?;
        with_lock(root, || usage(root))
    }

    /// Advance the persisted clear epoch and remove entries on a blocking worker.
    /// Running derivations from every cooperating instance lose publication rights.
    ///
    /// # Errors
    /// Returns a root, lock, epoch or removal error; incomplete removal is reported.
    pub fn clear(&self) -> io::Result<()> {
        let root = self
            .root
            .as_ref()
            .ok_or_else(|| io::Error::other("cache unavailable"))?;
        with_lock(root, || {
            let (epoch, clock, budget) = read_state(root, self.config.disk_bytes)?;
            write_state(
                root,
                (
                    epoch
                        .checked_add(1)
                        .ok_or_else(|| io::Error::other("epoch exhausted"))?,
                    clock,
                    budget,
                ),
            )?;
            prune(root, 0, &[], 0)
        })
    }

    /// Change the disk limit and immediately prune on the calling worker.
    ///
    /// # Errors
    /// Returns a storage error if the new bound cannot be enforced.
    pub fn set_disk_budget(&mut self, bytes: u64, visible: &[PreviewKey]) -> io::Result<()> {
        self.config.disk_bytes = bytes;
        let root = self
            .root
            .as_ref()
            .ok_or_else(|| io::Error::other("cache unavailable"))?;
        with_lock(root, || {
            let (epoch, clock, _) = read_state(root, bytes)?;
            write_state(
                root,
                (
                    epoch
                        .checked_add(1)
                        .ok_or_else(|| io::Error::other("epoch exhausted"))?,
                    clock,
                    bytes,
                ),
            )?;
            prune(root, entry_budget(root, bytes)?, visible, 0)
        })
    }
}

fn validate_handle(handle: &Handle, key: &PreviewKey, cap: u64) -> Result<(), PreviewError> {
    let Handle::Rgba {
        width,
        height,
        pixels,
        ..
    } = handle
    else {
        return Err(PreviewError::InvalidRaster);
    };
    let expected = u64::from(*width)
        .checked_mul(u64::from(*height))
        .and_then(|x| x.checked_mul(4));
    if *width == 0
        || *height == 0
        || *width > key.width
        || *height > key.height
        || expected != Some(pixels.len() as u64)
        || expected.is_none_or(|x| x > cap)
    {
        return Err(PreviewError::InvalidRaster);
    }
    Ok(())
}

fn with_lock<T>(root: &SecureRoot, operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    root.revalidate()
        .map_err(|e| io::Error::other(format!("{e:?}")))?;
    let file = match root.create_entry_private(OsStr::new(LOCK)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            root.open_entry_read(OsStr::new(LOCK))?
        }
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("cache lock is not a regular file"));
    }
    file.lock()?;
    let result = operation();
    let released = file.unlock();
    result.and_then(|value| released.map(|()| value))
}

/// The big-endian `u64` at `at`, if `bytes` holds one there.
fn be_u64(bytes: &[u8], at: usize) -> Option<u64> {
    let end = at.checked_add(8)?;
    bytes.get(at..end)?.try_into().ok().map(u64::from_be_bytes)
}

fn read_state(root: &SecureRoot, budget: u64) -> io::Result<(u64, u64, u64)> {
    let mut file = match root.open_entry_read(OsStr::new(STATE)) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            write_state(root, (0, 0, budget))?;
            return Ok((0, 0, budget));
        }
        Err(error) => return Err(error),
    };
    if !file.metadata()?.is_file() || file.metadata()?.len() != 24 {
        return Err(io::Error::other("invalid cache epoch"));
    }
    let mut word = || -> io::Result<u64> {
        let mut bytes = [0; 8];
        file.read_exact(&mut bytes)?;
        Ok(u64::from_be_bytes(bytes))
    };
    Ok((word()?, word()?, word()?))
}
fn write_state(root: &SecureRoot, state: (u64, u64, u64)) -> io::Result<()> {
    let mut stage = RootTempStage::create(root, OsStr::new(STATE))?;
    stage.write_all(&state.0.to_be_bytes())?;
    stage.write_all(&state.1.to_be_bytes())?;
    stage.write_all(&state.2.to_be_bytes())?;
    stage.commit()
}
fn next_clock(root: &SecureRoot, state: (u64, u64, u64)) -> io::Result<u64> {
    let clock = state
        .1
        .checked_add(1)
        .ok_or_else(|| io::Error::other("recency clock exhausted"))?;
    write_state(root, (state.0, clock, state.2))?;
    Ok(clock)
}
fn encode(handle: &Handle) -> io::Result<Vec<u8>> {
    let Handle::Rgba {
        width,
        height,
        pixels,
        ..
    } = handle
    else {
        return Err(io::Error::other("unresolved raster"));
    };
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(pixels, *width, *height, image::ExtendedColorType::Rgba8)
        .map_err(io::Error::other)?;
    Ok(bytes)
}
fn write_entry(root: &SecureRoot, key: &PreviewKey, clock: u64, png: &[u8]) -> io::Result<()> {
    let mut stage = RootTempStage::create(root, &key.name())?;
    stage.write_all(b"ENZDP001")?;
    let mut identity = Vec::with_capacity(48);
    identity.extend_from_slice(&key.digest);
    identity.extend_from_slice(&clock.to_be_bytes());
    identity.extend_from_slice(&(png.len() as u64).to_be_bytes());
    stage.write_all(&identity)?;
    stage.write_all(&entry_digest(&identity, png))?;
    stage.write_all(png)?;
    stage.commit()
}
fn entry_digest(identity: &[u8], png: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(b"enzui-derived-preview/entry-v1");
    hash.update((identity.len() as u64).to_be_bytes());
    hash.update(identity);
    hash.update((png.len() as u64).to_be_bytes());
    hash.update(png);
    hash.finalize().into()
}
fn read_raster(
    root: &SecureRoot,
    key: &PreviewKey,
    cap: u64,
) -> io::Result<Option<(Handle, Vec<u8>, u64)>> {
    let mut file = match root.open_entry_read(&key.name()) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let size = file.metadata()?;
    if !size.is_file()
        || size.len() < HEADER as u64
        || size.len() > cap.saturating_mul(2).saturating_add(65536)
    {
        return Err(io::Error::other("invalid preview entry size"));
    }
    let mut header = [0; HEADER];
    file.read_exact(&mut header)?;
    if &header[..8] != b"ENZDP001"
        || header[8..40] != key.digest
        || be_u64(&header, 48) != Some(size.len() - HEADER as u64)
    {
        return Err(io::Error::other("invalid preview identity"));
    }
    let mut png = Vec::new();
    file.take(size.len() - HEADER as u64 + 1)
        .read_to_end(&mut png)?;
    if header[56..88] != entry_digest(&header[8..56], &png) {
        return Err(io::Error::other("invalid preview checksum"));
    }
    let limits = enzui_core::decode_limits::DecodeLimits {
        max_encoded_bytes: cap.saturating_mul(2).saturating_add(65536),
        max_width: key.width,
        max_height: key.height,
        max_pixels: cap / 4,
        max_rgba_bytes: cap,
    };
    let handle = crate::decode_static(&png, &limits, || false)
        .map_err(|error| io::Error::other(error.to_string()))?
        .handle;
    validate_handle(&handle, key, cap).map_err(io::Error::other)?;
    let clock = be_u64(&header, 40).ok_or_else(|| io::Error::other("invalid preview clock"))?;
    Ok(Some((handle, png, clock)))
}
fn allocated_len(size: u64) -> u64 {
    size.saturating_add(BLOCK - 1) / BLOCK * BLOCK
}
fn allocated(file: &File) -> io::Result<u64> {
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(io::Error::other("non-regular cache entry"));
    }
    Ok(meta
        .blocks()
        .saturating_mul(512)
        .max(allocated_len(meta.len()).max(BLOCK)))
}
fn entries(root: &SecureRoot) -> io::Result<Vec<(OsString, u64, u64)>> {
    let mut result = Vec::new();
    for entry in root.entry_names()? {
        if entry.name == LOCK || entry.name == STATE {
            continue;
        }
        let file = root.open_entry_read(&entry.name)?;
        let bytes = allocated(&file)?;
        let mut header = [0; 48];
        let clock = if (&file).read_exact(&mut header).is_ok() && &header[..8] == b"ENZDP001" {
            be_u64(&header, 40).unwrap_or(0)
        } else {
            0
        };
        result.push((entry.name, bytes, clock));
    }
    Ok(result)
}
fn prune(root: &SecureRoot, budget: u64, visible: &[PreviewKey], reserve: u64) -> io::Result<()> {
    let entries = entries(root)?;
    let mut bytes = entries.iter().try_fold(reserve, |sum, (_, size, _)| {
        sum.checked_add(*size)
            .ok_or_else(|| io::Error::other("cache census overflow"))
    })?;
    if bytes <= budget {
        return Ok(());
    }
    // Evict unprotected entries first, oldest clock first; names are unique,
    // so the order is total.
    let protected: HashSet<OsString> = visible.iter().map(PreviewKey::name).collect();
    let mut entries: Vec<_> = entries
        .into_iter()
        .map(|(name, size, clock)| (protected.contains(&name), clock, name, size))
        .collect();
    entries.sort_unstable_by(|a, b| (a.0, a.1, &a.2).cmp(&(b.0, b.1, &b.2)));
    for (_, _, name, size) in entries {
        if bytes <= budget {
            break;
        }
        root.unlink_entry(&name)?;
        bytes -= size;
    }
    if bytes > budget {
        return Err(io::Error::other("entry reservation exceeds disk budget"));
    }
    Ok(())
}
fn usage(root: &SecureRoot) -> io::Result<PreviewUsage> {
    use std::os::fd::AsFd;
    let entries = entries(root)?;
    let directory = root.ordinary_directory()?;
    let directory = File::from(directory.as_fd().try_clone_to_owned()?);
    let mut coordination = directory
        .metadata()?
        .blocks()
        .saturating_mul(512)
        .max(BLOCK);
    for name in [LOCK, STATE] {
        match root.open_entry_read(OsStr::new(name)) {
            Ok(file) => coordination = coordination.saturating_add(allocated(&file)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => (),
            Err(error) => return Err(error),
        }
    }
    Ok(PreviewUsage {
        disk_bytes: entries
            .iter()
            .try_fold(0u64, |sum, (_, size, _)| {
                sum.checked_add(*size)
                    .ok_or_else(|| io::Error::other("cache census overflow"))
            })?
            .saturating_add(coordination)
            .saturating_add(2 * BLOCK),
        coordination_bytes: coordination,
        entries: entries.len(),
    })
}

fn entry_budget(root: &SecureRoot, budget: u64) -> io::Result<u64> {
    let usage = usage(root)?;
    Ok(budget.saturating_sub(usage.coordination_bytes.saturating_add(2 * BLOCK)))
}

use enzui_core::blocking_task::{BlockingTask, BlockingTaskDispatcher, BlockingTaskSettlement};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

/// Admission outcome; identical queued or running keys share one flight.
#[derive(Clone, Debug)]
pub enum PreviewAdmission {
    /// Stable decoded handle retained within the separate RAM budget.
    Ready(Handle),
    /// Flight identity, shared by matching requests until completion.
    Pending(u64),
    /// The bounded waiting queue is full.
    Busy,
}

/// Bounded request scheduler over an application-owned blocking dispatcher.
///
/// Enqueue requests, call [`dispatch`](Self::dispatch), deliver each returned
/// task's settlement through the application runtime, then call
/// [`complete`](Self::complete) and dispatch again. No I/O runs in these update
/// methods. Source versions must be captured off-thread and current when enqueued;
/// RAM reuse trusts the key, while disk reuse revalidates through the callback.
/// Visible and recent hints retain at most 64 keys each. Work is generated only
/// for explicitly enqueued requests. Hints alone never generate previews.
///
/// The caller retains the pool owner. Its worker count bounds shared concurrency;
/// `max_running` further bounds this service's submissions. A full shared pool
/// settles a task as busy through the normal blocking-task settlement.
pub struct DerivedPreviews {
    dispatcher: BlockingTaskDispatcher,
    root: PathBuf,
    config: PreviewStoreConfig,
    ram: crate::HandleCache<PreviewKey>,
    queue: VecDeque<QueuedPreview>,
    running: HashMap<u64, (PreviewKey, u64)>,
    keys: HashMap<PreviewKey, u64>,
    visible: HashSet<PreviewKey>,
    recent: HashSet<PreviewKey>,
    max_running: usize,
    queue_capacity: usize,
    next_id: u64,
    epoch: Arc<AtomicU64>,
}
type PreviewDerivation = Box<dyn FnOnce(&dyn Fn() -> bool) -> Result<Handle, PreviewError> + Send>;

struct QueuedPreview {
    id: u64,
    key: PreviewKey,
    valid: Box<dyn Fn() -> bool + Send>,
    derive: PreviewDerivation,
    epoch: u64,
}

impl DerivedPreviews {
    /// Construct without touching disk or spawning workers. Zero running capacity
    /// is normalized to one; zero queue capacity refuses every uncached request.
    pub fn new(
        dispatcher: BlockingTaskDispatcher,
        root: PathBuf,
        config: PreviewStoreConfig,
        decoded_ram_bytes: usize,
        max_running: usize,
        queue_capacity: usize,
    ) -> Self {
        Self {
            dispatcher,
            root,
            config,
            ram: crate::HandleCache::new(decoded_ram_bytes),
            queue: VecDeque::new(),
            running: HashMap::new(),
            keys: HashMap::new(),
            visible: HashSet::new(),
            recent: HashSet::new(),
            max_running: max_running.max(1),
            queue_capacity,
            next_id: 0,
            epoch: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Replace bounded visibility/recency hints. Visible work is dispatched before
    /// recent work, and recent work before other admitted work; ties remain FIFO.
    pub fn hints(
        &mut self,
        visible: impl IntoIterator<Item = PreviewKey>,
        recent: impl IntoIterator<Item = PreviewKey>,
    ) {
        self.visible = visible.into_iter().take(64).collect();
        self.recent = recent.into_iter().take(64).collect();
    }

    /// Admit an explicit request. Coalesced requests retain the first callbacks.
    /// The validator runs only on the worker; a RAM hit trusts the source version.
    pub fn request(
        &mut self,
        key: PreviewKey,
        source_valid: impl Fn() -> bool + Send + 'static,
        derive: impl FnOnce(&dyn Fn() -> bool) -> Result<Handle, PreviewError> + Send + 'static,
    ) -> PreviewAdmission {
        if let Some(handle) = self.ram.get(&key) {
            return PreviewAdmission::Ready(handle);
        }
        if let Some(id) = self.keys.get(&key) {
            return PreviewAdmission::Pending(*id);
        }
        if self.queue.len() >= self.queue_capacity {
            return PreviewAdmission::Busy;
        }
        let Some(id) = self.next_id.checked_add(1) else {
            return PreviewAdmission::Busy;
        };
        self.next_id = id;
        self.keys.insert(key.clone(), id);
        self.queue.push_back(QueuedPreview {
            id,
            key,
            valid: Box::new(source_valid),
            derive: Box::new(derive),
            epoch: self.epoch.load(Ordering::Acquire),
        });
        PreviewAdmission::Pending(id)
    }

    /// Submit the highest-priority waiting work into available service slots.
    /// Returned task generations are the flight IDs accepted by `complete`.
    pub fn dispatch(&mut self) -> Vec<BlockingTask<Result<PreviewResult, PreviewError>>> {
        let mut tasks = Vec::new();
        while self.running.len() < self.max_running {
            let Some(work) = self
                .queue
                .iter()
                .enumerate()
                .min_by_key(|(index, work)| {
                    (
                        !self.visible.contains(&work.key),
                        !self.recent.contains(&work.key),
                        *index,
                    )
                })
                .map(|(index, _)| index)
                .and_then(|index| self.queue.remove(index))
            else {
                break;
            };
            self.running.insert(work.id, (work.key.clone(), work.epoch));
            let root = self.root.clone();
            let config = self.config;
            let visible: Vec<_> = self.visible.iter().cloned().collect();
            let epoch = Arc::clone(&self.epoch);
            tasks.push(self.dispatcher.submit(work.id, move |context| {
                let cancelled =
                    || context.is_cancelled() || epoch.load(Ordering::Acquire) != work.epoch;
                if cancelled() {
                    return Err(PreviewError::Cancelled);
                }
                let store = PreviewStore::open(root, config);
                store.get_or_derive(&work.key, &visible, work.valid, cancelled, work.derive)
            }));
        }
        tasks
    }

    /// Release a flight and retain successful, current-generation pixels.
    /// Returns its key and outcome so an application can fan out to its consumers.
    /// Unknown or retired flight settlements are ignored.
    pub fn complete(
        &mut self,
        settlement: BlockingTaskSettlement<Result<PreviewResult, PreviewError>>,
    ) -> Option<(PreviewKey, Result<PreviewResult, PreviewError>)> {
        let id = settlement.generation();
        let (key, epoch) = self.running.remove(&id)?;
        if self.keys.get(&key) == Some(&id) {
            self.keys.remove(&key);
        }
        if epoch != self.epoch.load(Ordering::Acquire) {
            return None;
        }
        let result = settlement
            .into_result()
            .unwrap_or_else(|error| Err(PreviewError::Worker(error)));
        if let Ok(result) = &result {
            self.ram.insert(key.clone(), result.handle.clone());
        }
        Some((key, result))
    }

    /// Current decoded RAM bytes; independent of disk usage and staging.
    pub fn decoded_ram_bytes(&self) -> usize {
        self.ram.bytes()
    }

    /// Replace the RAM budget and discard retained decoded handles.
    pub fn set_decoded_ram_budget(&mut self, bytes: usize) {
        self.ram = crate::HandleCache::new(bytes);
    }

    /// Observe disk usage through the existing application worker pool.
    pub fn disk_usage(&self) -> BlockingTask<io::Result<PreviewUsage>> {
        let root = self.root.clone();
        let config = self.config;
        self.dispatcher
            .submit(0, move |_| PreviewStore::open(root, config).usage())
    }

    /// Retire admitted work and apply a root-wide disk budget on a worker.
    /// Await successful settlement before treating the policy change as complete.
    /// A zero budget prunes entries; coordination overhead remains observable.
    pub fn set_disk_budget(&mut self, bytes: u64) -> BlockingTask<io::Result<()>> {
        self.retire();
        self.config.disk_bytes = bytes;
        let root = self.root.clone();
        let config = self.config;
        let visible: Vec<_> = self.visible.iter().cloned().collect();
        self.dispatcher.submit(0, move |_| {
            PreviewStore::open(root, config).set_disk_budget(bytes, &visible)
        })
    }

    fn retire(&mut self) {
        self.epoch.fetch_add(1, Ordering::AcqRel);
        self.ram.clear();
        self.queue.clear();
        self.keys.clear();
    }

    /// Retire admitted work immediately and clear disk on the owned pool.
    /// The caller must await this task before considering the disk clear complete.
    /// Submitted worker slots remain reserved until their settlements are delivered.
    pub fn clear(&mut self) -> BlockingTask<io::Result<()>> {
        self.retire();
        let root = self.root.clone();
        let config = self.config;
        self.dispatcher
            .submit(0, move |_| PreviewStore::open(root, config).clear())
    }
}
