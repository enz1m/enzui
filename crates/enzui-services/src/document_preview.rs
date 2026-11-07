//! Bounded PDF-page and video-poster preview decoding.
//!
//! Backends are feature-gated and return [`PreviewError::Unsupported`] when unavailable.
//! Inputs are admitted before native parsing, planned PDF raster dimensions are checked
//! before allocation, and decoded frames are checked before return. Public decode calls are
//! blocking.

pub mod cache;
#[cfg(feature = "gstreamer")]
pub mod gst_poster;
pub mod offline_video;
#[cfg(feature = "pdf-preview")]
pub mod pdfium;
#[cfg(test)]
mod tests;

use crate::decode_limits::{DecodeLimits, RefusedVerdict};
use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// A validated RGBA8 preview with explicit pixel dimensions.
#[derive(Clone, PartialEq, Eq)]
pub struct RenderedPreview {
    width: u32,
    height: u32,
    pixels: Arc<Vec<u8>>,
}

impl RenderedPreview {
    /// Constructs a preview after validating nonzero dimensions and an exact checked `width *
    /// height * 4` byte length.
    ///
    /// # Errors
    ///
    /// Returns [`PreviewError::DecodeFailed`] for degenerate dimensions, arithmetic overflow,
    /// or a length mismatch.
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Result<Self, PreviewError> {
        if width == 0 || height == 0 {
            return Err(PreviewError::DecodeFailed {
                message: format!("zero dimension in RenderedPreview: {width}x{height}"),
            });
        }
        let pixels_u64 = u64::from(width) * u64::from(height);
        let Some(expected_u64) = pixels_u64.checked_mul(4) else {
            return Err(PreviewError::DecodeFailed {
                message: format!("RGBA8 buffer size overflows u64 for {width}x{height}"),
            });
        };
        let actual = u64::try_from(pixels.len()).unwrap_or(u64::MAX);
        if actual != expected_u64 {
            return Err(PreviewError::DecodeFailed {
                message: format!(
                    "RGBA8 buffer length {actual} does not match \
                     {width}x{height}x4 = {expected_u64}",
                ),
            });
        }
        Ok(RenderedPreview {
            width,
            height,
            pixels: Arc::new(pixels),
        })
    }

    /// The preview width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The preview height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The RGBA8 pixel bytes, `width * height * 4` long.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Returns whether at least one pixel byte is nonzero.
    #[must_use]
    pub fn has_nonblank_pixels(&self) -> bool {
        self.pixels.iter().any(|&b| b != 0)
    }
}

impl fmt::Debug for RenderedPreview {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RenderedPreview")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bytes", &self.pixels.len())
            .finish()
    }
}

/// A typed reason that a document preview could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum PreviewError {
    /// The requested preview backend or format is unavailable.
    Unsupported {
        /// A short description of what is not supported.
        what: String,
    },
    /// A [`DecodeLimits`] check refused the encoded input or decoded output.
    LimitExceeded {
        /// The non-allow verdict that refused the preview.
        verdict: RefusedVerdict,
    },
    /// The input opened but could not be decoded or rendered.
    DecodeFailed {
        /// A human-readable error message.
        message: String,
    },
    /// Cooperative worker cancellation interrupted staging or backend waits.
    Cancelled,
    /// The preview input could not be read.
    Io {
        /// A human-readable I/O error message.
        message: String,
    },
    /// The requested zero-based PDF page does not exist in the valid document.
    PageOutOfRange {
        /// The 0-based page index that was requested.
        requested: u32,
        /// The total number of pages in the document.
        page_count: u32,
    },
}

impl fmt::Display for PreviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PreviewError::Unsupported { what } => write!(f, "unsupported: {what}"),
            PreviewError::LimitExceeded { verdict } => {
                write!(f, "decode limit exceeded: {}", verdict.reason())
            }
            PreviewError::DecodeFailed { message } => write!(f, "decode failed: {message}"),
            PreviewError::Cancelled => f.write_str("preview cancelled"),
            PreviewError::Io { message } => write!(f, "I/O error: {message}"),
            PreviewError::PageOutOfRange {
                requested,
                page_count,
            } => write!(
                f,
                "page {requested} is out of range: document has {page_count} page(s)"
            ),
        }
    }
}

impl std::error::Error for PreviewError {}

impl From<std::io::Error> for PreviewError {
    fn from(e: std::io::Error) -> Self {
        PreviewError::Io {
            message: e.to_string(),
        }
    }
}

// An admitted preview input whose decoder handle is an immutable snapshot on Linux.
pub(super) struct CheckedPreviewFile {
    pub(super) file: File,
    #[cfg(all(not(target_os = "linux"), feature = "gstreamer"))]
    original_path: std::path::PathBuf,
}

impl CheckedPreviewFile {
    // Return a stable descriptor path that resolves to the admitted snapshot on Linux.
    #[allow(
        dead_code,
        reason = "used only with the `gstreamer` feature and by tests"
    )]
    #[cfg(all(target_os = "linux", any(feature = "gstreamer", test)))]
    pub(super) fn decoder_path(&self) -> std::path::PathBuf {
        use std::os::fd::AsRawFd;

        std::path::PathBuf::from(format!("/proc/self/fd/{}", self.file.as_raw_fd()))
    }

    #[cfg(all(not(target_os = "linux"), feature = "gstreamer"))]
    pub(super) fn decoder_path(&self) -> std::path::PathBuf {
        self.original_path.clone()
    }
}

#[cfg(unix)]
fn open_preview_file(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(enzui_security::platform::libc_o_nonblock())
        .open(path)
}

#[cfg(not(unix))]
fn open_preview_file(path: &Path) -> std::io::Result<File> {
    File::open(path)
}

// Open and size-check once; on Linux, return a sealed snapshot and refuse source mutation
// observed during staging.
fn open_checked_file(
    path: &Path,
    limits: &DecodeLimits,
) -> Result<(CheckedPreviewFile, u64), PreviewError> {
    open_checked_file_cancellable(path, limits, &|| false)
}

fn open_checked_file_cancellable(
    path: &Path,
    limits: &DecodeLimits,
    cancelled: &dyn Fn() -> bool,
) -> Result<(CheckedPreviewFile, u64), PreviewError> {
    if cancelled() {
        return Err(PreviewError::Cancelled);
    }
    let file = open_preview_file(path)?;
    let admission_meta = file.metadata()?;
    if !admission_meta.is_file() {
        return Err(PreviewError::Io {
            message: "preview source is not a regular file".into(),
        });
    }
    let file_size = admission_meta.len();
    let verdict = limits.check_encoded(file_size);
    if let Some(refused) = RefusedVerdict::new(verdict) {
        return Err(PreviewError::LimitExceeded { verdict: refused });
    }
    #[cfg(target_os = "linux")]
    let file = stage_admitted_snapshot(file, &admission_meta, file_size, cancelled)?;
    #[cfg(not(target_os = "linux"))]
    let _ = admission_meta;
    Ok((
        CheckedPreviewFile {
            file,
            #[cfg(all(not(target_os = "linux"), feature = "gstreamer"))]
            original_path: path.to_path_buf(),
        },
        file_size,
    ))
}

// Copy exactly the admitted extent, reject short or metadata-changing copies, then seal the
// snapshot against mutation.
#[cfg(target_os = "linux")]
fn stage_admitted_snapshot(
    source: File,
    admission_meta: &std::fs::Metadata,
    admitted_size: u64,
    cancelled: &dyn Fn() -> bool,
) -> Result<File, PreviewError> {
    use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, memfd_create};
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::os::unix::fs::MetadataExt;

    #[cfg(test)]
    stage_hook::fire(stage_hook::Point::BeforeCopy);

    let memfd = memfd_create(
        "enzui-preview-snapshot",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
    )
    .map_err(|e| PreviewError::Io {
        message: format!("preview snapshot memfd_create failed: {e}"),
    })?;
    let mut sealed = File::from(memfd);
    let mut source = source;
    source.seek(SeekFrom::Start(0))?;
    // Bound the copy to the admitted length; reject early EOF below.
    let mut taken = (&mut source).take(admitted_size);
    let mut buffer = vec![0u8; 64 * 1024];
    let mut copied: u64 = 0;
    #[cfg(test)]
    let mut mid_hook_fired = false;
    loop {
        if cancelled() {
            return Err(PreviewError::Cancelled);
        }
        let read = taken.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        sealed.write_all(&buffer[..read])?;
        copied = copied.saturating_add(read as u64);
        #[cfg(test)]
        if !mid_hook_fired {
            stage_hook::fire(stage_hook::Point::MidCopyAfterFirstRead);
            mid_hook_fired = true;
        }
        if copied >= admitted_size {
            break;
        }
    }
    #[cfg(test)]
    let _ = mid_hook_fired;
    if copied != admitted_size {
        return Err(PreviewError::Io {
            message: format!(
                "preview source shortened during snapshot staging (admitted {admitted_size}, staged {copied})"
            ),
        });
    }
    // Reject any source identity, size, mtime, or ctime change observed during the copy.
    let post_meta = source.metadata()?;
    if post_meta.dev() != admission_meta.dev()
        || post_meta.ino() != admission_meta.ino()
        || post_meta.len() != admission_meta.len()
        || post_meta.mtime() != admission_meta.mtime()
        || post_meta.mtime_nsec() != admission_meta.mtime_nsec()
        || post_meta.ctime() != admission_meta.ctime()
        || post_meta.ctime_nsec() != admission_meta.ctime_nsec()
    {
        return Err(PreviewError::Io {
            message: "preview source metadata changed during snapshot staging".into(),
        });
    }
    sealed.flush()?;
    sealed.seek(SeekFrom::Start(0))?;
    fcntl_add_seals(
        &sealed,
        SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL,
    )
    .map_err(|e| PreviewError::Io {
        message: format!("preview snapshot seal failed: {e}"),
    })?;
    Ok(sealed)
}

#[cfg(test)]
pub(super) mod stage_hook {
    use std::cell::RefCell;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Point {
        BeforeCopy,
        MidCopyAfterFirstRead,
    }

    type StageHook = Box<dyn FnMut(Point)>;

    thread_local! {
        static HOOK: RefCell<Option<StageHook>> = const { RefCell::new(None) };
    }

    pub(crate) fn install(f: impl FnMut(Point) + 'static) {
        HOOK.with(|h| *h.borrow_mut() = Some(Box::new(f)));
    }

    pub(crate) fn clear() {
        HOOK.with(|h| *h.borrow_mut() = None);
    }

    // Remove the hook while invoking it to permit reentrant `fire` calls.
    pub(crate) fn fire(point: Point) {
        let hook = HOOK.with(|h| h.borrow_mut().take());
        if let Some(mut f) = hook {
            f(point);
            HOOK.with(|h| *h.borrow_mut() = Some(f));
        }
    }

    // Clears the thread-local hook on drop, including during unwinding.
    pub(crate) struct Guard;

    impl Drop for Guard {
        fn drop(&mut self) {
            clear();
        }
    }
}

/// Checks a file’s encoded size before decoding.
///
/// # Errors
///
/// Returns I/O failures or [`PreviewError::LimitExceeded`] when the encoded-size ceiling is
/// exceeded.
pub fn check_file_limits(path: &Path, limits: &DecodeLimits) -> Result<u64, PreviewError> {
    open_checked_file(path, limits).map(|(checked, size)| {
        drop(checked.file);
        size
    })
}

/// Default desired width for PDF preview rasters; decode limits may reduce it.
pub const DEFAULT_PDF_PREVIEW_WIDTH: u32 = 512;

/// A positive raster size admitted by [`plan_raster_target`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RasterTarget {
    width: u32,
    height: u32,
}

impl RasterTarget {
    /// Planned width in pixels.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    /// Planned height in pixels.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }
}

fn rounded_ratio(primary: u32, numerator: u64, denominator: u64) -> Option<u128> {
    let product = u128::from(primary).checked_mul(u128::from(numerator))?;
    let half = u128::from(denominator) / 2;
    product
        .checked_add(half)
        .map(|rounded| rounded / u128::from(denominator))
}

fn raster_candidate(
    primary: u32,
    landscape: bool,
    requested_width: u32,
    aspect_width: u64,
    aspect_height: u64,
    limits: &DecodeLimits,
) -> Option<RasterTarget> {
    let (width, height) = if landscape {
        let height = rounded_ratio(primary, aspect_height, aspect_width)?.max(1);
        (primary, u32::try_from(height).ok()?)
    } else {
        let width = rounded_ratio(primary, aspect_width, aspect_height)?.max(1);
        (u32::try_from(width).ok()?, primary)
    };
    if width > requested_width || limits.check_decoded(width, height).is_refused() {
        return None;
    }
    Some(RasterTarget { width, height })
}

fn no_positive_raster_target(limits: &DecodeLimits) -> PreviewError {
    match RefusedVerdict::new(limits.check_decoded(1, 1)) {
        Some(verdict) => PreviewError::LimitExceeded { verdict },
        None => PreviewError::DecodeFailed {
            message: "could not plan a positive raster target".into(),
        },
    }
}

/// Plans the largest aspect-preserving raster no wider than `requested_width`.
///
/// Dimensions use nearest rounding with ties upward and satisfy every decoded-output limit.
///
/// # Errors
///
/// Returns a typed limit refusal when even 1×1 is disallowed, or
/// [`PreviewError::DecodeFailed`] for invalid inputs or arithmetic overflow.
pub fn plan_raster_target(
    requested_width: u32,
    aspect_width: u64,
    aspect_height: u64,
    limits: &DecodeLimits,
) -> Result<RasterTarget, PreviewError> {
    if requested_width == 0 {
        return Err(PreviewError::DecodeFailed {
            message: "PDF preview requested width must be positive".into(),
        });
    }
    if aspect_width == 0 || aspect_height == 0 {
        return Err(PreviewError::DecodeFailed {
            message: format!(
                "PDF page aspect ratio must be positive, got {aspect_width}:{aspect_height}"
            ),
        });
    }

    // Admit 1×1 first so impossible policies return their specific limit refusal.
    if limits.check_decoded(1, 1).is_refused() {
        return Err(no_positive_raster_target(limits));
    }

    let landscape = aspect_width >= aspect_height;
    let width_ceiling = requested_width.min(limits.max_width);
    let upper = if landscape {
        width_ceiling
    } else {
        let desired_height = rounded_ratio(width_ceiling, aspect_height, aspect_width)
            .ok_or_else(|| PreviewError::DecodeFailed {
                message: "PDF raster target arithmetic overflow".into(),
            })?
            .max(1);
        let bounded = desired_height.min(u128::from(limits.max_height));
        u32::try_from(bounded).map_err(|_| PreviewError::DecodeFailed {
            message: "PDF raster target does not fit in u32".into(),
        })?
    };

    // Candidate dimensions grow monotonically, so binary search finds the largest admitted width.
    let mut low = 1_u64;
    let mut high = u64::from(upper.max(1));
    while low < high {
        let midpoint = low + (high - low).div_ceil(2);
        let primary = u32::try_from(midpoint).map_err(|_| PreviewError::DecodeFailed {
            message: "PDF raster target does not fit in u32".into(),
        })?;
        if raster_candidate(
            primary,
            landscape,
            requested_width,
            aspect_width,
            aspect_height,
            limits,
        )
        .is_some()
        {
            low = midpoint;
        } else {
            high = midpoint - 1;
        }
    }

    let primary = u32::try_from(low).map_err(|_| PreviewError::DecodeFailed {
        message: "PDF raster target does not fit in u32".into(),
    })?;
    raster_candidate(
        primary,
        landscape,
        requested_width,
        aspect_width,
        aspect_height,
        limits,
    )
    .ok_or_else(|| no_positive_raster_target(limits))
}

/// Extracts a limit-checked video frame after attempting to seek to `timestamp`.
///
/// A zero timestamp skips seeking; a failed seek falls back to initial playback. This blocking
/// operation requires the `gstreamer` feature; without it, it returns
/// [`PreviewError::Unsupported`].
///
/// # Errors
///
/// Returns [`PreviewError`] for unavailable support, I/O, decode failures, or
/// encoded/decoded limit refusals.
#[allow(
    unused_variables,
    reason = "unused when the `gstreamer` feature is off"
)]
pub fn video_poster_frame(
    path: &Path,
    timestamp: Duration,
    limits: &DecodeLimits,
) -> Result<RenderedPreview, PreviewError> {
    #[cfg(feature = "gstreamer")]
    {
        gst_poster::extract_poster_frame(path, timestamp, limits)
    }
    #[cfg(not(feature = "gstreamer"))]
    {
        Err(PreviewError::Unsupported {
            what: "video poster frame requires the `gstreamer` Cargo feature".into(),
        })
    }
}

/// Submits video-poster decoding to a bounded blocking-task owner.
pub fn dispatch_video_poster_frame(
    worker: &crate::blocking_task::BlockingTaskDispatcher,
    generation: u64,
    path: PathBuf,
    timestamp: Duration,
    limits: DecodeLimits,
) -> crate::blocking_task::BlockingTask<Result<RenderedPreview, PreviewError>> {
    worker.submit(generation, move |context| {
        #[cfg(feature = "gstreamer")]
        {
            gst_poster::extract_poster_frame_cancellable(&path, timestamp, &limits, || {
                context.is_cancelled()
            })
        }
        #[cfg(not(feature = "gstreamer"))]
        {
            let _ = context;
            video_poster_frame(&path, timestamp, &limits)
        }
    })
}

/// Renders a zero-based PDF page to a bounded RGBA8 preview.
///
/// This blocking operation requires `pdf-preview` and a compatible runtime `libpdfium`;
/// unavailable support returns [`PreviewError::Unsupported`]. Out-of-range pages return
/// [`PreviewError::PageOutOfRange`], and raster dimensions are reduced to satisfy decode
/// limits before allocation.
///
/// # Errors
///
/// Returns [`PreviewError`] for unavailable support, invalid page indices, I/O, decode
/// failures, or limit refusals.
pub fn render_pdf_page(
    path: &Path,
    page: u32,
    limits: &DecodeLimits,
) -> Result<RenderedPreview, PreviewError> {
    render_pdf_page_at_width(path, page, DEFAULT_PDF_PREVIEW_WIDTH, limits)
}

/// Renders a PDF page with an explicit desired width, reduced as needed to satisfy
/// [`DecodeLimits`].
///
/// This has the indexing, feature, blocking, and error behavior of [`render_pdf_page`].
///
/// # Errors
///
/// As [`render_pdf_page`].
#[allow(
    unused_variables,
    reason = "unused when the `pdf-preview` feature is off"
)]
pub fn render_pdf_page_at_width(
    path: &Path,
    page: u32,
    requested_width: u32,
    limits: &DecodeLimits,
) -> Result<RenderedPreview, PreviewError> {
    #[cfg(feature = "pdf-preview")]
    {
        pdfium::render_pdf_page_at_width(path, page, requested_width, limits)
    }
    #[cfg(not(feature = "pdf-preview"))]
    {
        Err(PreviewError::Unsupported {
            what: "PDF rendering requires the `pdf-preview` Cargo feature".into(),
        })
    }
}

/// Submits PDF-page decoding to a bounded blocking-task owner.
pub fn dispatch_pdf_page(
    worker: &crate::blocking_task::BlockingTaskDispatcher,
    generation: u64,
    path: PathBuf,
    page: u32,
    requested_width: u32,
    limits: DecodeLimits,
) -> crate::blocking_task::BlockingTask<Result<RenderedPreview, PreviewError>> {
    worker.submit(generation, move |_| {
        render_pdf_page_at_width(&path, page, requested_width, &limits)
    })
}
