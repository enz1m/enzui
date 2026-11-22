//! Streaming frame handover after asynchronous image-allocation results.
//!
//! Each offer creates one handle and an allocation task. The shown handle remains
//! available until a newer completion is accepted; completions older than the
//! shown token are ignored. Offers do not cancel or bound concurrent uploads, so
//! hosts must control their rate or use [`LatestFramePresenter`]. Frame
//! generations are caller-owned.
//!
//! Successful allocations are retained with the handle. Allocation failures also
//! install the offered handle, leaving upload-on-draw behavior to the renderer.
//! [`FramePresenter::clear`] rejects results offered before the clear. Tokens use
//! a wrapping counter with ordinary ordering; recreate the presenter before wrap.
//!
//! Each window has its own renderer and image cache. A presenter targeted with
//! [`FramePresenter::for_window`] allocates in the renderer of the window that
//! draws its frames, so an accepted frame is resident there; an untargeted
//! presenter allocates in an unspecified window's renderer. A large frame
//! allocated in another renderer is uploaded off the render thread on first
//! draw and shows nothing until it lands. A targeted allocation fails, and so
//! still completes, when its window is missing or closes or its renderer is
//! replaced first.

mod latest;
pub use latest::{FrameHandover, LatestFramePresenter};

use enzui_core::media::Rgba8Frame;
use iced::Task;
use iced::widget::image::{Allocation, Handle};

/// A frame whose upload finished (or failed); hand it to
/// [`FramePresenter::ready`].
#[derive(Debug, Clone)]
pub struct ReadyFrame {
    token: u64,
    handle: Handle,
    width: u32,
    height: u32,
    allocation: Result<Allocation, iced::widget::image::Error>,
}

#[derive(Debug, Clone)]
struct Shown {
    handle: Handle,
    width: u32,
    height: u32,
    _allocation: Option<Allocation>,
}

/// Retains a shown frame and accepts newer image-allocation completions, including failures.
#[derive(Debug, Default)]
pub struct FramePresenter {
    shown: Option<Shown>,
    latest: u64,
    shown_token: u64,
    window: Option<iced::window::Id>,
}

impl FramePresenter {
    /// A presenter showing nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A presenter showing nothing that allocates in `window`'s renderer.
    #[must_use]
    pub fn for_window(window: iced::window::Id) -> Self {
        Self {
            window: Some(window),
            ..Self::default()
        }
    }

    /// Creates an allocation task that returns a `ReadyFrame` on success or failure.
    /// Takes ownership of pixel bytes without copying them; it does not replace the shown frame.
    /// Offers do not cancel earlier tasks or validate frame generation.
    pub fn offer(&mut self, frame: Rgba8Frame) -> Task<ReadyFrame> {
        self.latest = self.latest.wrapping_add(1);
        let token = self.latest;
        let (width, height) = (frame.width(), frame.height());
        let handle = Handle::from_rgba(width, height, frame.into_pixels());
        let uploaded = handle.clone();
        let allocation = match self.window {
            Some(window) => iced::widget::image::allocate_in(window, handle),
            None => iced::widget::image::allocate(handle),
        };
        allocation.map(move |allocation| ReadyFrame {
            token,
            handle: uploaded.clone(),
            width,
            height,
            allocation,
        })
    }

    /// Installs a completion newer than the shown token and returns whether it was accepted.
    /// A newer offer still pending does not block this completion. Failed allocations
    /// install the handle without retaining an allocation.
    pub fn ready(&mut self, ready: ReadyFrame) -> bool {
        if ready.token <= self.shown_token {
            return false;
        }
        self.shown_token = ready.token;
        self.shown = Some(Shown {
            handle: ready.handle,
            width: ready.width,
            height: ready.height,
            // Retaining a successful allocation keeps the shown handle resident.
            _allocation: ready.allocation.ok(),
        });
        true
    }

    /// The handle to draw, if a frame has been shown.
    #[must_use]
    pub fn handle(&self) -> Option<&Handle> {
        self.shown.as_ref().map(|shown| &shown.handle)
    }

    /// The shown frame's size in pixels.
    #[must_use]
    pub fn size(&self) -> Option<(u32, u32)> {
        self.shown.as_ref().map(|shown| (shown.width, shown.height))
    }

    /// The shown frame's RGBA pixels (for saving a frame).
    #[must_use]
    pub fn pixels(&self) -> Option<&[u8]> {
        match self.shown.as_ref().map(|shown| &shown.handle) {
            Some(Handle::Rgba { pixels, .. }) => Some(pixels),
            _ => None,
        }
    }

    /// Clears the shown frame and rejects completions for all offers made so far.
    /// Pending tasks continue running; subsequent offers may be accepted.
    pub fn clear(&mut self) {
        self.shown = None;
        self.shown_token = self.latest;
    }
}
