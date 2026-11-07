//! The [`EventSink`] trait through which backends push raw events into the
//! service.

use super::types::RawEvent;

/// A destination for raw events from a directory-watch backend.
///
/// Backends retain the service-provided sink and may call it from any thread,
/// including during watch installation. Its service adapter ignores expired
/// watch tokens and events received after the service has dropped.
pub trait EventSink: Send + Sync + 'static {
    /// Deliver one raw event to the service.
    ///
    /// The service adapter locks shared state and applies bounds before enqueueing.
    /// Excess changes mark overflow for the next delivery. Mutex contention can
    /// block the producer; the adapter does not promise a non-blocking call.
    /// A caller-supplied sink can likewise stall every watch served by its
    /// backend's producer thread. Backend teardown may detach that thread until
    /// the call returns.
    fn push(&self, event: RawEvent);
}
