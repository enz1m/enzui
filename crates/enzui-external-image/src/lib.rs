//! Draws images another thread or process rendered, straight from GPU memory.
//!
//! A web engine, a video decoder or a camera hands over each frame as a
//! DMA-BUF: file descriptors naming GPU memory, with the frame's size, its
//! DRM format and modifier, and each plane's offset and stride. This crate
//! imports that memory on the renderer's own device and samples it in place,
//! so a frame never travels through the CPU.
//!
//! ```no_run
//! use enzui_external_image::{DmaBuf, ExternalFrame, Lease, Plane, enz_external_image, fourcc};
//! # fn frame_from_producer() -> (std::os::fd::OwnedFd, u64) { unimplemented!() }
//! # #[derive(Clone)] enum Message {}
//! let (fd, allocation) = frame_from_producer();
//! let frame = ExternalFrame::new(
//!     DmaBuf {
//!         width: 1280,
//!         height: 720,
//!         format: fourcc::ARGB8888,
//!         modifier: 0, // DRM_FORMAT_MOD_LINEAR
//!         planes: vec![Plane::new(fd, 0, 1280 * 4)],
//!         allocation,
//!     },
//!     // Called once nothing shows or samples this frame: hand it back.
//!     Lease::new(move || println!("allocation {allocation} is free again")),
//! );
//! let view: iced::Element<'_, Message> = enz_external_image(&frame).into();
//! ```
//!
//! # Placement
//!
//! Images are centered within their widget by default. Combine
//! [`ContentFit::None`](iced::ContentFit::None) with
//! [`align_x`](ExternalImage::align_x) and [`align_y`](ExternalImage::align_y)
//! to keep native-size content at the top-left while resizing its viewport.
//! Image dimensions are logical units, so output scaling still applies.
//! Content outside the widget is clipped, including CPU fallback images.
//! A pending replacement keeps the dimensions of the buffer still on screen.
//!
//! # Ownership
//!
//! A frame's [`Lease`] is released once, when the last reference to its
//! [`ExternalFrame`] is gone: the application's (drop it when a newer frame
//! arrives), any recorded scene's that could still replay it, and the
//! renderer's, which each keep theirs until the GPU has finished every frame
//! that sampled it. Until then the producer must not write to the
//! allocation. Imports are cached by [`DmaBuf::allocation`]: a producer that
//! cycles a pool of buffers is imported once per buffer, and up to
//! [`CACHED_ALLOCATIONS`] imports are kept. An import no widget shows is
//! released when the widget that showed it last is gone, after
//! [`IDLE_FRAMES`] frames, or when the unused imports pass
//! [`UNUSED_IMPORT_BUDGET`] bytes; always after the GPU has finished with it.
//! A widget drawn again unchanged reuses its binding and writes nothing to
//! the GPU. [`stats`] counts all of this.
//!
//! # What imports
//!
//! A Vulkan device opened with DMA-BUF import (the renderer does this when
//! the GPU supports it; see [`support`]), the formats in [`fourcc`], and
//! modifiers with one memory plane that the device can sample. Compressed
//! layouts with an auxiliary plane are refused: [`support`] lists what a
//! producer may allocate. A frame that cannot be imported reports why through
//! [`ExternalFrame::status`] and shows the widget's
//! [`fallback`](ExternalImage::fallback) image, if any; so does the software
//! renderer, which never imports.
//!
//! # Producer writes
//!
//! A frame is sampled only once the buffer's implicit fences say the
//! producer's writes have finished. A new frame still being written is
//! [`Status::Writing`]: the frame shown before stays on screen and the widget
//! asks for another frame to try again. One still unfinished after
//! [`WRITE_WAIT_LIMIT`], or whose buffer reports a polling error, is refused
//! with [`Refusal::Unwritten`] and shows its fallback. Either way its lease is
//! released as for any other frame.
//!
//! A streaming producer (a web view, a video decoder) publishes into an
//! [`ExternalFrameSource`] shown by [`enz_external_image_source`]: each new
//! frame then redraws only the image, with no application message and no view
//! rebuild, as long as nothing else in the window changed.
//!
//! A producer that paces itself on the host's consumption builds its frames
//! with [`ExternalFrame::with_on_encoded`]: the callback runs once, when a
//! draw first records the frame into the renderer's commands.

// The unit tests share the integration tests' GBM helper, which names this
// crate.
#[cfg(test)]
extern crate self as enzui_external_image;
#[cfg(test)]
#[path = "../tests/gbm/mod.rs"]
mod gbm;

mod device;
mod import;
mod pipeline;
mod stats;
mod widget;

use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};

pub use device::{DeviceMatch, DrmDevice, DrmNode};
pub use stats::{Stats, set_unused_import_budget, stats};
pub use widget::{
    ExternalImage, ExternalImageProbe, enz_external_image, enz_external_image_probe,
    enz_external_image_source,
};

/// How many distinct allocations stay imported at most.
pub const CACHED_ALLOCATIONS: usize = 32;

/// How many bytes of imports no widget shows a renderer keeps for reuse by
/// default: 256 MiB, room for a few 4K buffers. See
/// [`set_unused_import_budget`].
pub const UNUSED_IMPORT_BUDGET: u64 = 256 * 1024 * 1024;

/// How many frames an import no widget shows is kept for reuse before it is
/// released.
pub const IDLE_FRAMES: u64 = 120;

/// How long a new frame may stay [`Status::Writing`] before it is refused
/// with [`Refusal::Unwritten`].
pub const WRITE_WAIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(1);

/// DRM four-character format codes this crate imports.
pub mod fourcc {
    /// `AR24`: 8-bit B, G, R, A in memory order, alpha premultiplied.
    pub const ARGB8888: u32 = u32::from_le_bytes(*b"AR24");
    /// `XR24`: as [`ARGB8888`] with the alpha byte ignored.
    pub const XRGB8888: u32 = u32::from_le_bytes(*b"XR24");
    /// `AB24`: 8-bit R, G, B, A in memory order, alpha premultiplied.
    pub const ABGR8888: u32 = u32::from_le_bytes(*b"AB24");
    /// `XB24`: as [`ABGR8888`] with the alpha byte ignored.
    pub const XBGR8888: u32 = u32::from_le_bytes(*b"XB24");
}

/// One memory plane of a [`DmaBuf`].
#[derive(Debug, Clone)]
pub struct Plane {
    /// The DMA-BUF file descriptor. The import duplicates it; the producer
    /// keeps its own.
    pub fd: Arc<OwnedFd>,
    /// Where the plane starts in the buffer, in bytes.
    pub offset: u32,
    /// Bytes from one row to the next.
    pub stride: u32,
}

impl Plane {
    /// A plane of `fd` at `offset`, `stride` bytes per row.
    pub fn new(fd: OwnedFd, offset: u32, stride: u32) -> Self {
        Self {
            fd: Arc::new(fd),
            offset,
            stride,
        }
    }
}

/// A frame in GPU memory, as a DMA-BUF.
#[derive(Debug, Clone)]
pub struct DmaBuf {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The DRM fourcc; see [`fourcc`].
    pub format: u32,
    /// The DRM format modifier: the memory layout (`0` is linear).
    pub modifier: u64,
    /// The memory planes, in order.
    pub planes: Vec<Plane>,
    /// Identifies the buffer across frames: the same allocation presented
    /// again is not imported again. Two different buffers must never share
    /// one while either is shown.
    pub allocation: u64,
}

/// The producer's hold on a frame's buffer, released once the GPU is done
/// with it; see the [crate documentation](crate#ownership).
pub struct Lease(Mutex<Option<Box<dyn FnOnce() + Send>>>);

impl Lease {
    /// A lease that calls `release` once, on whichever thread drops the last
    /// reference to its frame (the UI thread, a render thread, or a
    /// GPU-completion callback).
    pub fn new(release: impl FnOnce() + Send + 'static) -> Self {
        Self(Mutex::new(Some(Box::new(release))))
    }

    /// A lease with nothing to release.
    pub fn none() -> Self {
        Self(Mutex::new(None))
    }

    fn release(&self) {
        let release = self.0.lock().ok().and_then(|mut release| release.take());
        if let Some(release) = release {
            release();
        }
    }

    fn is_released(&self) -> bool {
        self.0.lock().map_or(true, |release| release.is_none())
    }
}

impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lease")
            .field("released", &self.is_released())
            .finish()
    }
}

/// A shown frame: its buffer, the producer's lease on it, and how the import
/// went.
#[derive(Debug)]
pub struct ExternalFrame {
    buffer: DmaBuf,
    lease: Lease,
    status: Mutex<Status>,
    /// When the renderer first found its producer still writing it.
    writing_since: Mutex<Option<std::time::Instant>>,
    /// Called once, the first time a draw encodes this frame.
    on_encoded: OnEncoded,
}

/// [`ExternalFrame::with_on_encoded`]'s callback, until it is called.
struct OnEncoded(Mutex<Option<Box<dyn FnOnce() + Send>>>);

impl std::fmt::Debug for OnEncoded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pending = self.0.lock().is_ok_and(|callback| callback.is_some());
        f.debug_struct("OnEncoded")
            .field("pending", &pending)
            .finish()
    }
}

impl ExternalFrame {
    /// A frame to show; hand the same [`Arc`] to every view that shows it.
    pub fn new(buffer: DmaBuf, lease: Lease) -> Arc<Self> {
        Arc::new(Self {
            buffer,
            lease,
            status: Mutex::new(Status::Pending),
            writing_since: Mutex::new(None),
            on_encoded: OnEncoded(Mutex::new(None)),
        })
    }

    /// A frame to show, as [`new`](Self::new), that calls `on_encoded` once,
    /// the first time a draw encodes it: the frame's binding is recorded into
    /// the renderer's commands for the first time.
    ///
    /// That is command encoding, not GPU completion and not presentation on
    /// screen. Preparing the frame without drawing it, a frame still being
    /// written ([`Status::Writing`]) and a refused one do not count, and
    /// drawing it again, in this widget or another, does not call it again.
    /// A frame dropped before it is drawn drops `on_encoded` uncalled. It is
    /// called with no lock of the frame's held, on the thread that encodes.
    ///
    /// The lease and the readiness check are unchanged: the producer still
    /// must not write the buffer until the lease is released.
    pub fn with_on_encoded(
        buffer: DmaBuf,
        lease: Lease,
        on_encoded: impl FnOnce() + Send + 'static,
    ) -> Arc<Self> {
        let frame = Self::new(buffer, lease);
        if let Ok(mut slot) = frame.on_encoded.0.lock() {
            *slot = Some(Box::new(on_encoded));
        }
        frame
    }

    /// Calls [`with_on_encoded`](Self::with_on_encoded)'s callback, if it has
    /// not been called, outside the frame's lock.
    fn encoded(&self) {
        let callback = self
            .on_encoded
            .0
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        if let Some(callback) = callback {
            callback();
        }
    }

    /// The buffer.
    pub fn buffer(&self) -> &DmaBuf {
        &self.buffer
    }

    /// How the import went, as of the last frame drawn.
    pub fn status(&self) -> Status {
        self.status
            .lock()
            .map_or(Status::Pending, |status| status.clone())
    }

    /// How long the renderer has found this frame still being written,
    /// starting the count on the first call.
    fn writing_for(&self) -> std::time::Duration {
        self.writing_since
            .lock()
            .map_or(std::time::Duration::ZERO, |mut since| {
                since.get_or_insert_with(std::time::Instant::now).elapsed()
            })
    }

    fn set_status(&self, status: Status) {
        if let Ok(mut current) = self.status.lock() {
            *current = status;
        }
    }
}

impl Drop for ExternalFrame {
    /// A frame nothing shows any more gives its buffer back.
    fn drop(&mut self) {
        self.lease.release();
    }
}

/// The latest frame a streaming producer published, shared with the
/// widget that shows it ([`enz_external_image_source`]).
///
/// [`publish`](Self::publish) replaces the frame and wakes the window
/// showing it, which draws the new frame in a content-only redraw: no
/// application message, no view rebuild and no widget draw, only the image's
/// own preparation, as long as nothing else in the window changed and the
/// frame keeps its size. The application never sees these frames; it only
/// builds the widget once with the source.
///
/// The source holds one frame. A frame it replaces is released once nothing
/// shows it any more and the GPU work that sampled it has finished, as for
/// any [`ExternalFrame`]. A new frame still being written keeps the one shown
/// before on screen, and a refused one shows the widget's fallback.
/// [`clear`](Self::clear) shows nothing.
///
/// Publishing before any widget shows the source is fine: the first draw
/// shows the latest frame.
#[derive(Clone)]
pub struct ExternalFrameSource {
    shared: Arc<SourceShared>,
}

struct SourceShared {
    latest: Mutex<Option<Arc<ExternalFrame>>>,
    /// Published since the last preparation read it.
    pending: std::sync::atomic::AtomicBool,
    /// The windows of the widgets showing this source, by widget.
    windows: Mutex<Vec<(u64, iced::advanced::shell::Waker)>>,
}

impl std::fmt::Debug for ExternalFrameSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalFrameSource")
            .field(
                "latest",
                &self.latest().map(|frame| frame.buffer().allocation),
            )
            .finish_non_exhaustive()
    }
}

impl Default for ExternalFrameSource {
    fn default() -> Self {
        Self::new()
    }
}

impl ExternalFrameSource {
    /// A source showing nothing yet.
    pub fn new() -> Self {
        Self {
            shared: Arc::new(SourceShared {
                latest: Mutex::new(None),
                pending: std::sync::atomic::AtomicBool::new(false),
                windows: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Shows `frame` next, in place of the frame before, and wakes the
    /// windows showing this source. Callable from any thread.
    pub fn publish(&self, frame: Arc<ExternalFrame>) {
        self.replace(Some(frame));
    }

    /// Shows nothing; the frame shown is released as any replaced frame is.
    pub fn clear(&self) {
        self.replace(None);
    }

    /// The frame the source shows next.
    pub fn latest(&self) -> Option<Arc<ExternalFrame>> {
        self.shared
            .latest
            .lock()
            .ok()
            .and_then(|latest| latest.clone())
    }

    /// Whether this and `other` are the same source.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    fn replace(&self, frame: Option<Arc<ExternalFrame>>) {
        // The replaced frame drops outside the lock: its lease may call back.
        let replaced = match self.shared.latest.lock() {
            Ok(mut latest) => std::mem::replace(&mut *latest, frame),
            Err(_) => return,
        };
        drop(replaced);
        self.shared
            .pending
            .store(true, std::sync::atomic::Ordering::Release);
        let wakers: Vec<_> = self
            .shared
            .windows
            .lock()
            .map(|windows| windows.iter().map(|(_, waker)| waker.clone()).collect())
            .unwrap_or_default();
        for waker in wakers {
            waker.wake();
        }
    }

    /// The frame to prepare, clearing the pending mark.
    fn take_for_frame(&self) -> Option<Arc<ExternalFrame>> {
        self.shared
            .pending
            .store(false, std::sync::atomic::Ordering::Release);
        self.latest()
    }

    /// Whether a frame was published since the last preparation read one.
    fn is_pending(&self) -> bool {
        self.shared
            .pending
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// Wakes `waker` on each publish for the widget `instance`.
    fn register(&self, instance: u64, waker: &iced::advanced::shell::Waker) {
        if let Ok(mut windows) = self.shared.windows.lock() {
            windows.retain(|(widget, _)| *widget != instance);
            windows.push((instance, waker.clone()));
        }
    }
}

/// How importing a frame went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Not drawn yet.
    Pending,
    /// Its producer is still writing it: the frame shown before stays on
    /// screen, and the widget tries again on the next frame, for up to
    /// [`WRITE_WAIT_LIMIT`].
    Writing,
    /// Imported and drawn from GPU memory.
    Imported,
    /// Not importable; the widget's fallback, if any, is shown.
    Refused(Refusal),
}

/// Why a frame was not imported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The renderer cannot import DMA-BUF images: it is not Vulkan, or its
    /// device lacks the extensions, or it renders in software.
    NoImport,
    /// The format is not one of [`fourcc`].
    Format(u32),
    /// The device cannot sample this format with this modifier, or the
    /// modifier needs more memory planes than this crate imports.
    Modifier {
        /// The format.
        format: u32,
        /// The modifier.
        modifier: u64,
    },
    /// The buffer's plane count does not match its modifier's.
    Planes {
        /// Planes given.
        given: usize,
        /// Planes the modifier has.
        expected: usize,
    },
    /// Wider or taller than the device can sample, or empty.
    Size {
        /// Width.
        width: u32,
        /// Height.
        height: u32,
    },
    /// The driver refused the import.
    Driver(String),
    /// The producer's writes to the buffer could not be confirmed finished:
    /// still unfinished after [`WRITE_WAIT_LIMIT`], or polling the buffer
    /// failed. The frame is never sampled.
    Unwritten(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = |format: u32| String::from_utf8_lossy(&format.to_le_bytes()).into_owned();
        match self {
            Self::NoImport => write!(f, "the renderer cannot import DMA-BUF images"),
            Self::Format(format) => write!(f, "format {} is not importable", name(*format)),
            Self::Modifier { format, modifier } => write!(
                f,
                "format {} with modifier {modifier:#x} is not importable",
                name(*format)
            ),
            Self::Planes { given, expected } => {
                write!(f, "{given} planes given, the modifier has {expected}")
            }
            Self::Size { width, height } => write!(f, "{width}x{height} is not importable"),
            Self::Driver(reason) => write!(f, "the driver refused the import: {reason}"),
            Self::Unwritten(reason) => {
                write!(f, "the producer's writes did not finish: {reason}")
            }
        }
    }
}

/// What the renderer's device imports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Support {
    /// For each importable format, the modifiers a producer may allocate.
    pub formats: Vec<(u32, Vec<u64>)>,
    /// The largest width and height the device samples.
    pub max_extent: u32,
    /// The DRM device these formats were queried on, when the driver names
    /// it (`VK_EXT_physical_device_drm`); `None` when it does not. A producer
    /// compares it with its own allocation device and treats
    /// [`DeviceMatch::Unknown`] like a different device: the import may
    /// still succeed, through a copy the driver makes.
    pub device: Option<DrmDevice>,
}

impl Support {
    /// The importable modifiers of `format`; empty when it is not importable.
    pub fn modifiers(&self, format: u32) -> &[u64] {
        self.formats
            .iter()
            .find(|(candidate, _)| *candidate == format)
            .map_or(&[], |(_, modifiers)| modifiers.as_slice())
    }

    /// How the renderer's device compares with the DRM node `node` a
    /// producer allocates on; [`DeviceMatch::Unknown`] when either side
    /// cannot be identified.
    pub fn compare_node(&self, node: std::os::fd::BorrowedFd<'_>) -> DeviceMatch {
        match (self.device, DrmDevice::of_node(node)) {
            (Some(mine), Some(theirs)) => mine.compare(&theirs),
            _ => DeviceMatch::Unknown,
        }
    }
}

static SUPPORT: Mutex<Option<Option<Support>>> = Mutex::new(None);

/// What the renderer imports: `None` until it has drawn an external image or
/// an [`ExternalImageProbe`] once (a renderer that draws neither never needs
/// to tell), then `Some(None)` when it imports nothing and
/// `Some(Some(support))` otherwise. A producer choosing its first buffer
/// mounts the probe and waits for its message.
///
/// The process has one answer: the windows of an application share one
/// renderer device. When that device is replaced (after a device loss), the
/// new device's support replaces the old once it draws an external image or
/// a probe again; a producer that negotiated with the old device checks
/// again, for example when a frame is refused.
pub fn support() -> Option<Option<Support>> {
    SUPPORT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Records what the GPU renderer's current device imports.
pub(crate) fn record_support(support: Option<Support>) {
    *SUPPORT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(support);
}

/// Records that a renderer without imports drew, unless a device already
/// answered.
pub(crate) fn record_no_import() {
    let _ = SUPPORT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert(None);
}

/// How many DMA-BUF imports this process has made. A producer cycling a
/// pool of buffers sees it rise once per buffer, not once per frame.
pub fn imports_made() -> u64 {
    stats().imports_made
}
