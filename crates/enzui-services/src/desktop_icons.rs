//! Off-thread PNG/SVG desktop icon lookup with bounded handle retention.
//!
//! Capture [`IconContext`] at boot, retain one [`DesktopIconService`], and deliver
//! [`DesktopIconService::resolve`] through the application's task dispatcher.
//! No matching artwork is `None`; read failures are distinct I/O errors. The
//! caller chooses its bundled fallback. Theme changes invalidate results by
//! epoch, so discard replies failing `is_current`.
//! SVG retains its original colors; PNG is decoded off-thread to RGBA before
//! delivery. Cache bytes count SVG source or decoded RGBA; renderer SVG decode
//! and GPU caches are separate. PNG admission caps dimensions at 4096 per axis
//! and decoded RGBA at 16 MiB.
//!
//! Lookup uses the [freedesktop icon-theme algorithm](https://specifications.freedesktop.org/icon-theme/latest/)
//! for PNG/SVG, including first-index precedence and scale-aware size matching.
//! XPM is not decoded. Refresh after installing/replacing artwork in place.

mod loader;
#[cfg(feature = "desktop-icon-portal")]
pub mod portal;

use crate::blocking_task::{BlockingTask, BlockingTaskOwner};
use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

/// Captured icon search roots in precedence order; paths may be non-UTF-8.
#[derive(Clone, Debug)]
pub struct IconContext {
    roots: Vec<PathBuf>,
}

impl IconContext {
    /// Use absolute roots verbatim; the service rejects empty or oversized lists.
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self { roots }
    }

    /// Capture HOME/XDG data roots, including legacy `.icons` and system pixmaps.
    pub fn from_env() -> Self {
        Self::from_xdg(&enzui_icons::mime::xdg::Globs2Env::from_env())
    }

    /// Resolve captured HOME/XDG values without reading the process environment.
    pub fn from_xdg(env: &enzui_icons::mime::xdg::Globs2Env) -> Self {
        let mut roots = Vec::new();
        if let Some(home) = env
            .home
            .as_deref()
            .map(std::path::Path::new)
            .filter(|p| p.is_absolute())
        {
            roots.push(home.join(".icons"));
        }
        for path in enzui_icons::mime::xdg::globs2_candidates_os(
            env.data_home.as_deref(),
            env.data_dirs.as_deref(),
            env.home.as_deref(),
        ) {
            if let Some(root) = path.parent().and_then(std::path::Path::parent) {
                roots.push(root.join("icons"));
            }
        }
        roots.push(PathBuf::from("/usr/share/pixmaps"));
        let mut unique = Vec::new();
        for root in roots {
            if !unique.contains(&root) {
                unique.push(root);
            }
        }
        Self::new(unique)
    }

    /// The captured roots, with no filesystem access.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }
}

/// Retained entry and source/pixel-byte limits, including missing-icon entries.
#[derive(Clone, Copy, Debug)]
pub struct IconCacheLimits {
    /// Maximum retained lookup keys (1..=4096).
    pub entries: usize,
    /// Maximum SVG source or decoded PNG bytes retained across all handles (1..=256 MiB).
    pub bytes: usize,
    /// Maximum encoded bytes read for one icon (1..=4 MiB).
    pub file_bytes: usize,
}
impl Default for IconCacheLimits {
    fn default() -> Self {
        Self {
            entries: 128,
            bytes: 16 * 1024 * 1024,
            file_bytes: 512 * 1024,
        }
    }
}

/// Renderer-neutral handles accepted by shared label/icon-slot artwork setters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DesktopIcon {
    /// Original SVG artwork, without a uniform tint.
    Svg(iced_core::svg::Handle),
    /// Decoded PNG pixels, without recoloring.
    Image(iced_core::image::Handle),
}

/// A lookup reply stamped with the theme/content epoch it observed.
#[derive(Clone, Debug)]
pub struct IconResolution {
    epoch: u64,
    icon: Option<DesktopIcon>,
    path: Option<PathBuf>,
}
impl IconResolution {
    /// The theme/content epoch; compare with [`DesktopIconService::is_current`].
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    /// The cached handle, or `None` when no supported artwork exists.
    pub fn icon(&self) -> Option<&DesktopIcon> {
        self.icon.as_ref()
    }
    /// The selected file, retained for diagnostics without another stat call.
    pub fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }
}

/// A terminal lookup refusal; the application may retain its bundled fallback.
#[derive(Debug)]
#[non_exhaustive]
pub enum IconError {
    /// A name, size or scale is outside the supported request contract.
    InvalidRequest,
    /// A theme, index, probe or file budget was exceeded.
    Limit,
    /// The selected artwork could not be read as a bounded regular file.
    Io(std::io::Error),
    /// Supported artwork could not be decoded as a PNG.
    Decode(image::ImageError),
    /// A theme refresh superseded this running lookup.
    Superseded,
}
impl std::fmt::Display for IconError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest => f.write_str("invalid desktop icon request"),
            Self::Limit => f.write_str("desktop icon budget exceeded"),
            Self::Io(error) => error.fmt(f),
            Self::Decode(error) => error.fmt(f),
            Self::Superseded => f.write_str("desktop icon lookup superseded"),
        }
    }
}
impl std::error::Error for IconError {}

#[derive(Clone)]
struct ThemeEpoch {
    name: String,
    epoch: u64,
}
#[derive(Clone, PartialEq, Eq)]
struct Key {
    name: String,
    size: u16,
    scale: u16,
}
struct Cached {
    key: Key,
    resolution: IconResolution,
    bytes: usize,
}
#[derive(Default)]
struct Cache {
    epoch: u64,
    themes: Option<loader::LoadedThemes>,
    entries: VecDeque<Cached>,
    bytes: usize,
}

struct Inner {
    context: IconContext,
    limits: IconCacheLimits,
    theme: Mutex<ThemeEpoch>,
    cache: Mutex<Cache>,
}

/// One blocking worker and a 16-slot queue; lookup and cache mutation stay off the view thread.
pub struct DesktopIconService {
    inner: Arc<Inner>,
    worker: BlockingTaskOwner,
}
impl DesktopIconService {
    /// Start the worker without reading theme metadata or icon files.
    ///
    /// # Errors
    /// Returns an invalid-input error for invalid roots/theme/limits, or a thread-start error.
    pub fn new(
        context: IconContext,
        theme: impl Into<String>,
        limits: IconCacheLimits,
    ) -> std::io::Result<Self> {
        let name = theme.into();
        if context.roots.is_empty()
            || context.roots.len() > 66
            || context.roots.iter().any(|p| !p.is_absolute())
            || !loader::valid_name(&name)
            || limits.entries == 0
            || limits.entries > 4096
            || limits.bytes == 0
            || limits.bytes > 256 * 1024 * 1024
            || limits.file_bytes == 0
            || limits.file_bytes > 4 * 1024 * 1024
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid desktop icon context or limits",
            ));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                context,
                limits,
                theme: Mutex::new(ThemeEpoch { name, epoch: 1 }),
                cache: Mutex::new(Cache::default()),
            }),
            worker: BlockingTaskOwner::new(
                "desktop-icons",
                NonZeroUsize::MIN,
                const { NonZeroUsize::new(16).expect("16 is not zero") },
            )?,
        })
    }

    /// Invalidate artwork after an in-place install/change, without filesystem access.
    pub fn refresh(&self) {
        let mut theme = self
            .inner
            .theme
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        theme.epoch = theme.epoch.wrapping_add(1);
    }

    /// Change the selected theme and invalidate earlier replies; return whether it changed.
    ///
    /// # Errors
    /// Returns [`IconError::InvalidRequest`] for invalid theme names.
    pub fn set_theme(&self, name: impl Into<String>) -> Result<bool, IconError> {
        self.inner.set_theme(name.into())
    }

    /// The current selection, without filesystem access.
    pub fn theme(&self) -> String {
        self.inner
            .theme
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .name
            .clone()
    }

    /// Whether a reply still belongs to the current theme/content epoch.
    pub fn is_current(&self, resolution: &IconResolution) -> bool {
        self.inner
            .theme
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .epoch
            == resolution.epoch
    }

    /// Submit a lookup; size is logical pixels and scale is an integer display scale.
    ///
    /// Both must be nonzero, with size×scale at most 4096. The outer settlement
    /// carries worker queue/cancellation errors; its value carries lookup errors.
    /// Missing icons are cached and returned as `IconResolution::icon() == None`.
    pub fn resolve(
        &self,
        generation: u64,
        name: impl Into<String>,
        size: u16,
        scale: u16,
    ) -> BlockingTask<Result<IconResolution, IconError>> {
        let inner = Arc::clone(&self.inner);
        let name = name.into();
        self.worker.submit(generation, move |cancel| {
            if cancel.is_cancelled() {
                return Err(IconError::Superseded);
            }
            if !loader::valid_name(&name)
                || size == 0
                || scale == 0
                || u32::from(size) * u32::from(scale) > 4096
            {
                return Err(IconError::InvalidRequest);
            }
            inner.resolve(Key { name, size, scale })
        })
    }
}

impl Inner {
    fn set_theme(&self, name: String) -> Result<bool, IconError> {
        if !loader::valid_name(&name) {
            return Err(IconError::InvalidRequest);
        }
        let mut theme = self.theme.lock().unwrap_or_else(PoisonError::into_inner);
        if name == theme.name {
            return Ok(false);
        }
        theme.name = name;
        theme.epoch = theme.epoch.wrapping_add(1);
        Ok(true)
    }
    fn resolve(&self, key: Key) -> Result<IconResolution, IconError> {
        let theme = self
            .theme
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let mut cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        if cache.epoch != theme.epoch {
            *cache = Cache {
                epoch: theme.epoch,
                ..Cache::default()
            };
        }
        if let Some(entry) = cache
            .entries
            .iter()
            .position(|e| e.key == key)
            .and_then(|pos| cache.entries.remove(pos))
        {
            let result = entry.resolution.clone();
            cache.entries.push_back(entry);
            if self
                .theme
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .epoch
                != theme.epoch
            {
                return Err(IconError::Superseded);
            }
            return Ok(result);
        }
        let themes = if let Some(themes) = &cache.themes {
            themes
        } else {
            cache
                .themes
                .insert(loader::LoadedThemes::load(&self.context, &theme.name)?)
        };
        let path = themes.find(&self.context, &theme.name, &key.name, key.size, key.scale)?;
        let (icon, bytes) = if let Some(path) = &path {
            let bytes =
                loader::read_regular(path, self.limits.file_bytes).map_err(IconError::Io)?;
            if path.extension().is_some_and(|ext| ext == "svg") {
                let len = bytes.len();
                (
                    Some(DesktopIcon::Svg(iced_core::svg::Handle::from_memory(bytes))),
                    len,
                )
            } else {
                let (handle, len) = loader::decode_png(&bytes, self.limits.file_bytes)?;
                (Some(DesktopIcon::Image(handle)), len)
            }
        } else {
            (None, 0)
        };
        if self
            .theme
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .epoch
            != theme.epoch
        {
            return Err(IconError::Superseded);
        }
        let resolution = IconResolution {
            epoch: theme.epoch,
            icon,
            path,
        };
        if bytes <= self.limits.bytes {
            while cache.entries.len() >= self.limits.entries
                || cache.bytes + bytes > self.limits.bytes
            {
                let Some(old) = cache.entries.pop_front() else {
                    break;
                };
                cache.bytes -= old.bytes;
            }
            cache.bytes += bytes;
            cache.entries.push_back(Cached {
                key,
                resolution: resolution.clone(),
                bytes,
            });
        }
        Ok(resolution)
    }
}
