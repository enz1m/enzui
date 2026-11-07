//! Native Wayland drag offers on the application's existing connection.
//!
//! A source needs an active pointer or touch grab. A destination accepts a MIME
//! type while hovering, reads after the drop, and calls [`ReceivedDrop::finish`]
//! after committing the operation. Dropping a source or an unfinished received
//! drop cancels it. Move success is reported only after destination completion.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{ClipboardError, MimeType, Pasted, Payload, SeatSelector};
pub use wayland_protocols::xdg::shell::client::xdg_toplevel::XdgToplevel;

pub(crate) const EVENT_CAPACITY: usize = 64;
pub(crate) const MAX_MIMES: usize = 64;
pub(crate) const MAX_MIME_BYTES: usize = 1024;

/// Returns a Wayland window's protocol surface id while its handle is live.
///
/// Use this from the window runtime to map incoming events to application
/// windows. Remove that mapping when the window closes; protocol ids may be
/// reused after destruction. Non-Wayland or unavailable handles return `None`.
#[expect(
    unsafe_code,
    reason = "reads a libwayland proxy id under a live borrowed window handle"
)]
pub fn surface_id(window: &(impl raw_window_handle::HasWindowHandle + ?Sized)) -> Option<u32> {
    use wayland_client::Proxy as _;
    let handle = window.window_handle().ok()?;
    let raw_window_handle::RawWindowHandle::Wayland(wayland) = handle.as_raw() else {
        return None;
    };
    // SAFETY: the borrowed window handle keeps the wl_surface proxy valid until
    // the temporary ObjectId is dropped; only its numeric id escapes this scope.
    let object = unsafe {
        wayland_client::backend::ObjectId::from_ptr(
            wayland_client::protocol::wl_surface::WlSurface::interface(),
            wayland.surface.as_ptr().cast(),
        )
    }
    .ok()?;
    Some(object.protocol_id())
}

/// An opaque offer identity, invalidated on leave, cancellation or completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DragId(pub(crate) u64);

impl DragId {
    /// An identity for code that drives a destination without a compositor
    /// (tests of drop handling); the runtime only knows the ones it issued.
    #[doc(hidden)]
    #[must_use]
    pub const fn detached(value: u64) -> Self {
        Self(value)
    }
}

/// An operation negotiated by the compositor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragAction {
    /// Duplicate the source data.
    Copy,
    /// Transfer ownership after successful destination completion.
    Move,
}

/// The copy/move operations permitted by a source or destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DragActions(pub(crate) u32);

impl DragActions {
    /// Refuse every operation.
    pub const NONE: Self = Self(0);
    /// Permit copying only.
    pub const COPY: Self = Self(1);
    /// Permit moving only.
    pub const MOVE: Self = Self(2);
    /// Permit copying and moving.
    pub const BOTH: Self = Self(3);

    /// Whether this set permits the operation.
    pub fn allows(self, action: DragAction) -> bool {
        self.0 & action.bit() != 0
    }
}

impl DragAction {
    pub(crate) fn bit(self) -> u32 {
        match self {
            Self::Copy => 1,
            Self::Move => 2,
        }
    }
}

/// Native destination events; coordinates are surface-local logical pixels.
#[derive(Debug, Clone, PartialEq)]
pub enum DragEvent {
    /// A peer entered a surface. Accept a supported type before the drop.
    Entered {
        /// Offer identity used for subsequent requests.
        id: DragId,
        /// Wayland surface object id on this application's connection.
        surface: u32,
        /// Compositor seat name.
        seat: String,
        /// Surface-local pointer or touch position.
        position: (f64, f64),
        /// Bounded advertised MIME names, preserving wire spelling.
        mimes: Vec<MimeType>,
        /// Operations the source permits.
        actions: DragActions,
    },
    /// The current position changed; adjacent motion events may coalesce.
    Motion {
        /// Active offer identity.
        id: DragId,
        /// Surface-local pointer or touch position.
        position: (f64, f64),
    },
    /// The compositor selected or refused an operation.
    Action {
        /// Active offer identity.
        id: DragId,
        /// `None` means that no common operation is selected.
        action: Option<DragAction>,
    },
    /// The pointer was released over an accepted destination.
    Dropped {
        /// Offer identity ready for [`NativeDragClient::receive`].
        id: DragId,
        /// Compositor-selected operation.
        action: DragAction,
    },
    /// The offer left without a drop or was cancelled.
    Left {
        /// Invalidated offer identity.
        id: DragId,
    },
    /// Event pressure invalidated every incoming offer; discard destination feedback.
    Reset,
}

/// Terminal result of an outgoing native drag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragOutcome {
    /// The destination confirmed successful processing.
    Finished(DragAction),
    /// No destination completed the operation.
    Cancelled,
}

/// An outgoing operation kept alive until completion or explicit cancellation.
#[derive(Debug)]
pub struct SourceDrag {
    pub(crate) receiver: Receiver<Result<DragOutcome, ClipboardError>>,
    pub(crate) cancelled: Arc<AtomicBool>,
}

impl SourceDrag {
    /// Polls the terminal result without blocking; each result is delivered once.
    ///
    /// # Errors
    /// Returns transport shutdown or connection failure.
    pub fn try_result(&self) -> Result<Option<DragOutcome>, ClipboardError> {
        match self.receiver.try_recv() {
            Ok(result) => result.map(Some),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(ClipboardError::Stopped),
        }
    }

    /// Requests cancellation without blocking the calling thread.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl Drop for SourceDrag {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Default, Debug)]
pub(crate) struct EventQueue {
    events: Mutex<VecDeque<DragEvent>>,
    pub(crate) closed: AtomicBool,
    waker: Mutex<Option<std::task::Waker>>,
}

impl EventQueue {
    pub(crate) fn push(&self, event: DragEvent) -> bool {
        let Ok(mut events) = self.events.lock() else {
            return false;
        };
        if let DragEvent::Motion { id, .. } = &event
            && matches!(events.back(), Some(DragEvent::Motion { id: previous, .. }) if id == previous)
        {
            events.pop_back();
        }
        if events.len() >= EVENT_CAPACITY {
            events.clear();
            events.push_back(DragEvent::Reset);
            drop(events);
            self.wake();
            return false;
        }
        events.push_back(event);
        drop(events);
        self.wake();
        true
    }

    fn wake(&self) {
        let waker = self.waker.lock().ok().and_then(|mut slot| slot.take());
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    pub(crate) fn close(&self) {
        {
            let _events = self.events.lock();
            self.closed.store(true, Ordering::Release);
        }
        self.wake();
    }
}

/// A bounded incoming event queue with one logical consumer.
///
/// Clones share the same queue rather than receiving duplicate events. Dropping
/// the last clone cancels outstanding incoming offers on the next dispatch tick.
#[derive(Clone, Debug)]
pub struct DragEvents(pub(crate) Arc<EventQueue>);

impl DragEvents {
    /// Removes the oldest pending event without blocking on protocol I/O.
    pub fn try_next(&self) -> Option<DragEvent> {
        self.0.events.lock().ok()?.pop_front()
    }

    /// Whether dispatch stopped; queued events may still be drained.
    pub fn is_closed(&self) -> bool {
        self.0.closed.load(Ordering::Acquire)
    }

    /// Polls the queue, registering a wake when the next event arrives.
    pub fn poll_next(&self, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<DragEvent>> {
        let Ok(mut events) = self.0.events.lock() else {
            return std::task::Poll::Ready(None);
        };
        if let Some(event) = events.pop_front() {
            return std::task::Poll::Ready(Some(event));
        }
        if self.is_closed() {
            return std::task::Poll::Ready(None);
        }
        if let Ok(mut slot) = self.0.waker.lock() {
            *slot = Some(cx.waker().clone());
        }
        std::task::Poll::Pending
    }
}

/// Received bytes awaiting an explicit destination commit.
///
/// Rejecting or dropping this value never tells the source to remove its data.
#[derive(Debug)]
pub struct ReceivedDrop {
    pub(crate) data: Pasted,
    pub(crate) action: DragAction,
    pub(crate) accepted: Arc<AtomicBool>,
    pub(crate) cancelled: Arc<AtomicBool>,
}

impl ReceivedDrop {
    /// The transferred MIME type and bounded bytes.
    pub fn data(&self) -> &Pasted {
        &self.data
    }

    /// The compositor-selected operation; file clipboard intent is independent.
    pub fn action(&self) -> DragAction {
        self.action
    }

    /// Confirms successful destination processing and permits source completion.
    pub fn finish(self) {
        self.accepted.store(true, Ordering::Release);
    }
}

impl Drop for ReceivedDrop {
    fn drop(&mut self) {
        if !self.accepted.load(Ordering::Acquire) {
            self.cancelled.store(true, Ordering::Release);
        }
    }
}

/// The image under the pointer while this application drags, with the
/// pixel that sits at the pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DragIcon {
    pub(crate) argb: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) hotspot: (i32, i32),
    pub(crate) scale: u32,
}

/// The longest side a drag icon may have, in buffer pixels.
pub const MAX_DRAG_ICON_SIDE: u32 = 1024;

impl DragIcon {
    /// An icon from straight RGBA pixels, `width * height * 4` bytes, drawn
    /// at `scale` buffer pixels per logical pixel (`2` on a 2x display), with
    /// `hotspot` (in buffer pixels) at the pointer. A hotspot outside the
    /// image keeps the icon off the drop point: `(-12, -12)` shows it below
    /// and right of the pointer.
    ///
    /// # Errors
    /// `Unavailable` when the bytes do not match the size, a side is zero or
    /// over [`MAX_DRAG_ICON_SIDE`], or the scale is zero.
    pub fn from_rgba(
        rgba: &[u8],
        width: u32,
        height: u32,
        hotspot: (i32, i32),
        scale: u32,
    ) -> Result<Self, ClipboardError> {
        let refused = |reason: &str| ClipboardError::Unavailable {
            reason: reason.to_owned(),
        };
        if width == 0 || height == 0 || width > MAX_DRAG_ICON_SIDE || height > MAX_DRAG_ICON_SIDE {
            return Err(refused("drag icon sides must be 1 to 1024 pixels"));
        }
        if rgba.len() as u64 != u64::from(width) * u64::from(height) * 4 {
            return Err(refused("drag icon bytes do not match its size"));
        }
        if scale == 0 {
            return Err(refused("drag icon scale must be at least 1"));
        }
        // Wayland's ARGB8888 is premultiplied and little-endian: B, G, R, A.
        let argb = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|pixel| {
                let alpha = u16::from(pixel[3]);
                let premultiply = |channel: u8| ((u16::from(channel) * alpha + 127) / 255) as u8;
                [
                    premultiply(pixel[2]),
                    premultiply(pixel[1]),
                    premultiply(pixel[0]),
                    pixel[3],
                ]
            })
            .collect();
        Ok(Self {
            argb,
            width,
            height,
            hotspot,
            scale,
        })
    }

    /// The width in buffer pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The height in buffer pixels.
    pub fn height(&self) -> u32 {
        self.height
    }
}

pub(crate) enum DragCommand {
    Listen(Sender<Result<DragEvents, ClipboardError>>),
    Start {
        payload: Payload,
        actions: DragActions,
        seat: SeatSelector,
        icon: Option<DragIcon>,
        reply: Sender<Result<SourceDrag, ClipboardError>>,
    },
    AttachWindow {
        toplevel: XdgToplevel,
        offset: (i32, i32),
        reply: Sender<Result<(), ClipboardError>>,
    },
    Accept {
        id: DragId,
        preferences: Vec<MimeType>,
        actions: DragActions,
        preferred: DragAction,
        reply: Sender<Result<MimeType, ClipboardError>>,
    },
    Reject {
        id: DragId,
        reply: Sender<Result<(), ClipboardError>>,
    },
    Receive {
        id: DragId,
        reply: Sender<Result<ReceivedDrop, ClipboardError>>,
    },
}

impl DragCommand {
    pub(crate) fn refuse(self, error: ClipboardError) {
        match self {
            Self::Listen(reply) => {
                let _ = reply.send(Err(error));
            }
            Self::Start { reply, .. } => {
                let _ = reply.send(Err(error));
            }
            Self::AttachWindow { reply, .. } => {
                let _ = reply.send(Err(error));
            }
            Self::Accept { reply, .. } => {
                let _ = reply.send(Err(error));
            }
            Self::Reject { reply, .. } => {
                let _ = reply.send(Err(error));
            }
            Self::Receive { reply, .. } => {
                let _ = reply.send(Err(error));
            }
        }
    }
}

/// Synchronous native drag requests over the adopted connection's bounded queue.
///
/// Run requests on a worker, or use `enzui_clipboard_runtime::native_drag` from
/// an iced application. A result timeout does not undo a request already begun.
#[derive(Clone)]
pub struct NativeDragClient {
    pub(crate) submit: Arc<dyn Fn(DragCommand) -> Result<(), ClipboardError> + Send + Sync>,
    pub(crate) timeout: Duration,
}

impl std::fmt::Debug for NativeDragClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeDragClient")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl NativeDragClient {
    fn request<T>(
        &self,
        build: impl FnOnce(Sender<Result<T, ClipboardError>>) -> DragCommand,
    ) -> Result<T, ClipboardError> {
        let (tx, rx) = mpsc::channel();
        (self.submit)(build(tx))?;
        rx.recv_timeout(self.timeout).map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => ClipboardError::TransferStalled {
                after: self.timeout,
            },
            mpsc::RecvTimeoutError::Disconnected => ClipboardError::Stopped,
        })?
    }

    /// Registers the single destination event consumer on this connection.
    ///
    /// # Errors
    /// Returns `Busy` if a live consumer exists, or a worker/transport refusal.
    pub fn listen(&self) -> Result<DragEvents, ClipboardError> {
        self.request(DragCommand::Listen)
    }

    /// Starts a source using the seat's current pointer or touch grab.
    ///
    /// Keyboard focus and historical input serials grant no drag authority.
    /// If neither pointer leave nor a source event confirms compositor activity
    /// within two seconds, the source is cancelled. This also bounds silently
    /// ignored serials; an unconfirmed touch drag may expire outside any target.
    /// Normal delivery of the initiating pointer button's release before a
    /// native drop cancels the source; later clicks after drop do not.
    ///
    /// # Errors
    /// Returns `NotFocused` without a live grab, `Busy` for a competing drag,
    /// or a payload, action, worker or transport refusal.
    pub fn start(
        &self,
        payload: Payload,
        actions: DragActions,
        seat: SeatSelector,
    ) -> Result<SourceDrag, ClipboardError> {
        self.start_with_icon(payload, actions, seat, None)
    }

    /// [`Self::start`] with `icon` under the pointer while dragging.
    ///
    /// # Errors
    /// As [`Self::start`]; the icon itself was validated when made.
    pub fn start_with_icon(
        &self,
        payload: Payload,
        actions: DragActions,
        seat: SeatSelector,
        icon: Option<DragIcon>,
    ) -> Result<SourceDrag, ClipboardError> {
        self.request(|reply| DragCommand::Start {
            payload,
            actions,
            seat,
            icon,
            reply,
        })
    }

    /// Moves a window with this application's ongoing drag
    /// (`xdg_toplevel_drag_v1`), as a tab torn out of a tab bar follows the
    /// pointer as its own window. `toplevel` is the window's `xdg_toplevel`
    /// on this connection (`iced::window::wayland_toplevel`); `offset` is where
    /// the pointer grips it, in the window's logical pixels. The compositor
    /// places the window where the drag ends.
    ///
    /// # Errors
    /// `Unavailable` when the compositor offers no `xdg_toplevel_drag_v1` or
    /// no drag of this application is under way, or a worker/transport
    /// failure.
    pub fn attach_window(
        &self,
        toplevel: XdgToplevel,
        offset: (i32, i32),
    ) -> Result<(), ClipboardError> {
        self.request(|reply| DragCommand::AttachWindow {
            toplevel,
            offset,
            reply,
        })
    }

    /// Accepts the first matching MIME and allowed operation while hovering.
    ///
    /// # Errors
    /// Returns a refusal for stale/dropped offers, disjoint MIME/actions, or an
    /// unsupported legacy operation, plus worker and transport errors.
    pub fn accept(
        &self,
        id: DragId,
        preferences: Vec<MimeType>,
        actions: DragActions,
        preferred: DragAction,
    ) -> Result<MimeType, ClipboardError> {
        self.request(|reply| DragCommand::Accept {
            id,
            preferences,
            actions,
            preferred,
            reply,
        })
    }

    /// Refuses an offer and invalidates it without transferring data.
    ///
    /// # Errors
    /// Returns a refusal for an expired identity or worker/transport failure.
    pub fn reject(&self, id: DragId) -> Result<(), ClipboardError> {
        self.request(|reply| DragCommand::Reject { id, reply })
    }

    /// Reads an accepted drop once, retaining it until destination commit.
    ///
    /// # Errors
    /// Returns a refusal before drop, after consumption, or on bounded transfer,
    /// worker or connection failure.
    pub fn receive(&self, id: DragId) -> Result<ReceivedDrop, ClipboardError> {
        self.request(|reply| DragCommand::Receive { id, reply })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use std::task::{Context, Poll, Wake, Waker};

    struct WakeCount(AtomicUsize);
    impl Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }

    #[test]
    fn native_drag_event_pressure_resets_and_motion_coalesces() {
        let events = DragEvents(Arc::new(EventQueue::default()));
        for index in 0..100 {
            assert!(events.0.push(DragEvent::Motion {
                id: DragId(1),
                position: (f64::from(index), 0.0)
            }));
        }
        assert_eq!(
            events.try_next(),
            Some(DragEvent::Motion {
                id: DragId(1),
                position: (99.0, 0.0)
            })
        );
        assert_eq!(events.try_next(), None);
        for index in 0..EVENT_CAPACITY {
            assert!(events.0.push(DragEvent::Left {
                id: DragId(u64::try_from(index).unwrap())
            }));
        }
        assert!(!events.0.push(DragEvent::Left { id: DragId(100) }));
        assert_eq!(events.try_next(), Some(DragEvent::Reset));
        assert_eq!(events.try_next(), None);
    }

    #[test]
    fn native_drag_event_subscription_wakes_for_delivery_and_shutdown() {
        let events = DragEvents(Arc::new(EventQueue::default()));
        let count = Arc::new(WakeCount(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&count));
        let mut context = Context::from_waker(&waker);
        assert_eq!(events.poll_next(&mut context), Poll::Pending);
        events.0.push(DragEvent::Reset);
        assert_eq!(count.0.load(Ordering::Acquire), 1);
        assert_eq!(
            events.poll_next(&mut context),
            Poll::Ready(Some(DragEvent::Reset))
        );
        assert_eq!(events.poll_next(&mut context), Poll::Pending);
        events.0.close();
        assert_eq!(count.0.load(Ordering::Acquire), 2);
        assert_eq!(events.poll_next(&mut context), Poll::Ready(None));
    }
}
