//! Native cross-application drag and drop through the default Wayland provider.
//!
//! Connect after a window opens. Include [`NativeDragRuntime::subscription`] in
//! the application's subscriptions, accept a supported MIME/action on enter,
//! and read after drop. Finish the received value only after committing the
//! destination operation. Internal [`enzui_core::drag`] sessions remain local.
//! Non-Wayland or explicitly replaced clipboard providers return unavailable.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use enzui_clipboard::drag::{DragEvents, NativeDragClient};
use enzui_clipboard::{ClipboardError, MimeType, Payload, SeatSelector};
use iced_futures::Subscription;

pub use enzui_clipboard::drag::surface_id;
pub use enzui_clipboard::drag::{
    DragAction, DragActions, DragEvent, DragIcon, DragId, DragOutcome, ReceivedDrop, SourceDrag,
    XdgToplevel,
};

use crate::file_transfer_async::{registered_sender, request};

static NEXT_RUNTIME: AtomicU64 = AtomicU64::new(1);

/// An application's native drag connection and shared destination event queue.
///
/// Clones share one logical event consumer. The default application startup
/// installs its transport together with the ordinary Wayland clipboard. This
/// type never opens another display connection or uses data-control protocols.
#[derive(Clone, Debug)]
pub struct NativeDragRuntime {
    client: NativeDragClient,
    events: DragEvents,
    id: u64,
}

impl NativeDragRuntime {
    /// Obtains native drag access after the application display is connected.
    ///
    /// Returns `None` for absent registration, queue pressure or budget expiry.
    /// Transport failures, including another active event consumer, remain typed.
    pub async fn connect(budget: Duration) -> Option<Result<Self, ClipboardError>> {
        request(registered_sender(), budget, |transport| {
            let client = transport
                .native_drag()
                .ok_or_else(|| ClipboardError::Unavailable {
                    reason: "the installed clipboard provider has no native Wayland drag transport"
                        .to_owned(),
                })?;
            let events = client.listen()?;
            let id = NEXT_RUNTIME
                .try_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| ClipboardError::Stopped)?;
            Ok(Self { client, events, id })
        })
        .await
    }

    /// Delivers bounded destination events to the application's reducer.
    ///
    /// Keep this subscription active while accepting native drops. Positions
    /// are surface-local logical pixels; surface ids identify Wayland objects
    /// on the application connection. Dropped data is read explicitly.
    pub fn subscription(&self) -> Subscription<DragEvent> {
        Subscription::run_with(self.clone(), |runtime| {
            let events = runtime.events.clone();
            futures::stream::poll_fn(move |cx| events.poll_next(cx))
        })
    }

    /// Starts a source from the current pointer or touch grab on the selected seat.
    ///
    /// Queue and budget refusals return `None`; transport refusals remain typed.
    /// Dropping the returned operation or its terminal future requests cancellation.
    pub async fn start(
        &self,
        payload: Payload,
        actions: DragActions,
        seat: SeatSelector,
        budget: Duration,
    ) -> Option<Result<SourceDrag, ClipboardError>> {
        let client = self.client.clone();
        request(registered_sender(), budget, move |_| {
            client.start(payload, actions, seat)
        })
        .await
    }

    /// [`Self::start`] with `icon` under the pointer while dragging.
    ///
    /// Queue and budget refusals return `None`; transport refusals remain typed.
    pub async fn start_with_icon(
        &self,
        payload: Payload,
        actions: DragActions,
        seat: SeatSelector,
        icon: Option<DragIcon>,
        budget: Duration,
    ) -> Option<Result<SourceDrag, ClipboardError>> {
        let client = self.client.clone();
        request(registered_sender(), budget, move |_| {
            client.start_with_icon(payload, actions, seat, icon)
        })
        .await
    }

    /// Moves a window with this application's ongoing drag, so a torn-off tab
    /// follows the pointer as its own window and stays where the drag ends.
    ///
    /// `window` is what `iced::window::wayland_toplevel` resolves to for the
    /// window to carry; `offset` is where the pointer grips it, in its logical
    /// pixels. A refusal (no `xdg_toplevel_drag_v1`, no drag under way, not a
    /// Wayland window) remains typed; admission or budget refusal returns
    /// `None`. Where it is refused, place the new window yourself on drop.
    pub async fn attach_window(
        &self,
        window: Option<Box<dyn std::any::Any + Send>>,
        offset: (i32, i32),
        budget: Duration,
    ) -> Option<Result<(), ClipboardError>> {
        let toplevel = match window.map(<Box<dyn std::any::Any + Send>>::downcast::<XdgToplevel>) {
            Some(Ok(toplevel)) => *toplevel,
            _ => {
                return Some(Err(ClipboardError::Unavailable {
                    reason: "not a Wayland window".to_owned(),
                }));
            }
        };
        let client = self.client.clone();
        request(registered_sender(), budget, move |_| {
            client.attach_window(toplevel, offset)
        })
        .await
    }

    /// Selects a MIME type and copy/move policy while an offer is hovering.
    ///
    /// Unsupported MIME/actions, stale offers and transport failures remain
    /// typed; admission or budget refusal returns `None`.
    pub async fn accept(
        &self,
        id: DragId,
        preferences: Vec<MimeType>,
        actions: DragActions,
        preferred: DragAction,
        budget: Duration,
    ) -> Option<Result<MimeType, ClipboardError>> {
        let client = self.client.clone();
        request(registered_sender(), budget, move |_| {
            client.accept(id, preferences, actions, preferred)
        })
        .await
    }

    /// Refuses an incoming offer without transferring bytes.
    ///
    /// Stale identities and transport failures remain typed; admission or
    /// budget refusal returns `None`.
    pub async fn reject(&self, id: DragId, budget: Duration) -> Option<Result<(), ClipboardError>> {
        let client = self.client.clone();
        request(registered_sender(), budget, move |_| client.reject(id)).await
    }

    /// Receives an accepted drop once, using the shared byte and transfer limits.
    ///
    /// Inspect [`ReceivedDrop::action`] for copy/move semantics. A file payload's
    /// clipboard intent does not override the compositor-selected drag action.
    /// Finish only after successful processing. Failures remain typed; admission
    /// or budget refusal returns `None` and does not imply rollback.
    pub async fn receive(
        &self,
        id: DragId,
        budget: Duration,
    ) -> Option<Result<ReceivedDrop, ClipboardError>> {
        let client = self.client.clone();
        request(registered_sender(), budget, move |_| client.receive(id)).await
    }
}

impl std::hash::Hash for NativeDragRuntime {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&self.id, state);
    }
}

/// Waits for destination completion or cancellation without blocking the UI.
///
/// Dropping this future cancels the source. A pointer release alone does not
/// report success, and a move is complete only after destination confirmation.
///
/// # Errors
/// Returns connection or dispatch shutdown failures.
pub async fn source_outcome(source: SourceDrag) -> Result<DragOutcome, ClipboardError> {
    loop {
        if let Some(outcome) = source.try_result()? {
            return Ok(outcome);
        }
        futures_timer::Delay::new(Duration::from_millis(8)).await;
    }
}
