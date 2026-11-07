//! File-chooser surface — the seam an app crosses to reach the
//! XDG Desktop Portal.
//!
//! # Why this module exists
//!
//! The portal implementation lives in `enzui_core::portal`; apps must not
//! depend on that crate
//! directly to construct a backend, start a request, or shape options —
//! precisely the *"reach past the framework"* pattern an app correctly
//! refused. Every app doing that would then
//! also hand-roll the same polling loop.
//!
//! This module re-exports the whole portal surface an app needs, and adds a
//! [`completion_subscription`] that delivers the portal's answer as an event
//! plus a [`pump`] helper that owns the poll / late-cancel-adoption / settle
//! sequence. Both are behind the `portal-dbus` feature only so that a
//! default build has no portal cost and no D-Bus code linked; the module
//! is present in every build so an app that opts in gets one obvious entry
//! point rather than digging through `enzui-core`.
//!
//! # How an app uses it
//!
//! 1. Enable the feature: `enzui-widgets = { …, features = ["portal-dbus"] }`.
//! 2. Hold a `DbusBackend` (behind the `portal-dbus` feature) and an
//!    `Option<PendingRequest>` in `App` state.
//! 3. Start a request with [`FileRequest::new`](crate::file_chooser::FileRequest::new) → [`FileRequest::start`](crate::file_chooser::FileRequest::start) on a
//!    user gesture.
//! 4. Return [`completion_subscription`] from `App::subscription`
//!    unconditionally — it does its own gating, and an app-side
//!    `is_pending` guard in front of it drops the wake that performs a
//!    cancelled request's owed close — and call [`pump`] when it wakes. The subscription is
//!    event-driven: it wakes when the portal answers, not on a clock and
//!    not on `window::frames()`.
//! 5. The `Task` [`pump`] returns is either `iced::Task::none` (still
//!    waiting) or a single-shot `iced::Task::done` carrying the terminal
//!    outcome. Match on it in `update` to react.
//!
//! `pump` takes the backend by `&mut` because a `DbusBackend` is not `Send` —
//! it holds a `tokio::runtime::Runtime`
//! (`enzui_core::portal::dbus_backend`), so it cannot cross into an async
//! `Task::perform`. The app owns the backend and calls `pump` from `update`.
//! The key requirement is **what makes `update` run**: the portal's own
//! completion edge, not a compositor frame clock and not a periodic timer.
//! There is no "tick every frame" step in this recipe, and adding one
//! reintroduces the settled-redraw defect class.
//!
//! # What is re-exported and why
//!
//! The re-exports below are the whole *user-visible* portal API — every type
//! that appears in a `FileRequest`, its start error, or the terminal outcome.
//! An app should not need `use enzui_core::…` for a file chooser.
//! `PortalBackend` itself is re-exported too, because [`pump`] takes it by
//! trait object and callers need the name to write the signature.
//!
//! # Feature gating
//!
//! The module itself — this documentation, the whole re-export surface, and
//! the [`pump`] helper — is **present in every
//! build**, so the discovery links in the crate root (`lib.rs`) land here
//! whether or not `portal-dbus` is enabled. That is deliberate: the app author
//! most likely to search for "native file dialog" is the one who has *not* yet
//! turned the feature on, and they must still reach a page that tells them how.
//!
//! Only `DbusBackend` and its `DbusFailure` start error are behind
//! the `portal-dbus` feature, because they are the parts that pull in the real
//! D-Bus stack (`ashpd`/`tokio`); a default build links none of it. Everything
//! else here — [`FileRequest`],
//! [`ChooserOptions`], the outcome types,
//! and `pump` — is pure logic that already lives in `enzui-core` in every
//! build, so re-exporting it costs nothing.

pub use enzui_core::portal::backend::{
    BackendError, BackendReply, PortalBackend, PortalOperation, RequestId,
};
pub use enzui_core::portal::cancel::PortalCancel;
pub use enzui_core::portal::chooser::{ChooserOptions, FileFilter, FilterRule, failure_text};
pub use enzui_core::portal::completion::{CompletionSignal, PortalWake};
/// The real D-Bus portal backend. Behind the `portal-dbus` feature because it
/// spawns a `tokio::runtime::Runtime` and links `ashpd`; see the module docs.
#[cfg(feature = "portal-dbus")]
pub use enzui_core::portal::dbus_backend::{DbusBackend, DbusFailure};
pub use enzui_core::portal::op::{FileRequest, FileResult, FileUri, PendingRequest};
pub use enzui_core::portal::parent::{ParentHandle, PortalParent};
pub use enzui_core::portal::{CancelReason, PortalOutcome, PortalResponse};

pub mod fallback;
mod fallback_backend;
mod session;
pub use fallback::{FallbackChooser, FallbackEvent};
pub use fallback_backend::FallbackBackend;
pub use session::{ChooserParent, ChooserSession};

/// The identity of a completion subscription: which request it belongs to.
///
/// iced identifies a subscription by hashing its data, and keeps a recipe alive
/// across `subscription()` calls whose hash is unchanged. Only `key` is hashed
/// — the signal is carried along so the stream builder (a plain `fn` pointer,
/// which cannot capture) can reach it. That means a new request (new key)
/// tears the old recipe down and builds a fresh one, while a re-render during
/// one live request reuses the existing stream instead of restarting it.
#[derive(Clone)]
struct CompletionKey {
    key: u64,
    signal: CompletionSignal,
}

impl std::hash::Hash for CompletionKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Deliberately not the signal: two renders of the same live request
        // must hash equal, or iced would rebuild the stream every frame and
        // reintroduce exactly the churn this replaces.
        self.key.hash(state);
    }
}

/// An event-driven subscription that wakes once per portal event while a
/// chooser request is live.
///
/// # Why this exists
///
/// Driving [`pump`] from `iced::window::frames()` or a 30/50/100 ms timer
/// asks for work when nothing has happened: with a
/// chooser open, a frame-mapped tick requests a redraw *every compositor
/// frame*, which is the settled-redraw defect class. A portal reply is
/// an event that arrives on the backend's own background task, so the correct
/// cadence is one wake per event.
///
/// # Gating
///
/// Returns `iced::Subscription::none` unless a request is actually
/// unsettled ([`PendingRequest::is_unsettled`]) and the backend has an
/// asynchronous side to wake from ([`PortalBackend::completion_signal`]). So
/// An app with no chooser open holds no subscription at all, and an in-process
/// test backend (whose replies are already there) is driven by [`pump`]
/// directly, as before.
///
/// The gate is `is_unsettled`, not [`PendingRequest::is_pending`]: an
/// application-side cancel (including one through a `Send`
/// [`cancel_handle`](PendingRequest::cancel_handle) clone) makes the request
/// terminal while the native dialog is still open and its `Request::close()`
/// still owed. Gating on `is_pending` lets the next `subscription()` rebuild
/// drop the wake source in exactly that window, so [`pump`] never runs, the
/// close is never claimed, and the portal dialog stays open.
///
/// # Cadence
///
/// The stream emits one priming wake when it starts, then exactly one wake
/// per subsequent portal event. The priming wake closes the start-up race: the
/// portal may answer between `start()` and the executor's first poll of this
/// stream, and an event-only stream would then sleep forever on a request that
/// is already finished. It is a single bounded extra `pump`, not a clock.
///
/// Dropping the subscription (the request settled, or the app dropped its
/// picker) drops the stream, which deregisters its waker from the signal —
/// there is nothing left to leak.
///
/// # Message shape
///
/// Yields [`PortalWake`]; map it to an app message:
///
/// ```ignore
/// file_chooser::completion_subscription(self.generation, self.pending.as_ref(), self.backend.as_deref())
///     .map(|_| Message::PortalTick)
/// ```
pub fn completion_subscription(
    key: u64,
    pending: Option<&PendingRequest>,
    backend: Option<&dyn PortalBackend>,
) -> iced::Subscription<PortalWake> {
    // Gate 1: no unsettled request means no subscription at all. Terminal but
    // unclaimed-close counts as unsettled — that wake is what performs the
    // close.
    if !pending.is_some_and(PendingRequest::is_unsettled) {
        return iced::Subscription::none();
    }
    // Gate 2: a backend with no asynchronous side has no edge to wake on.
    let Some(signal) = backend.and_then(PortalBackend::completion_signal) else {
        return iced::Subscription::none();
    };

    completion_signal_subscription(key, signal)
}

/// One wake per event on `signal`, plus a priming wake, keyed by `key`:
/// the same wake source as [`completion_subscription`], for any operation
/// that publishes a [`CompletionSignal`] (an OpenURI request, say).
pub fn completion_signal_subscription(
    key: u64,
    signal: CompletionSignal,
) -> iced::Subscription<PortalWake> {
    iced::Subscription::run_with(CompletionKey { key, signal }, |data| {
        let signal = data.signal.clone();
        iced::futures::stream::unfold(
            // `None` marks "not yet primed"; the first poll emits the priming
            // wake and records the generation it started from.
            (signal, None::<u64>),
            async |(signal, seen): (CompletionSignal, Option<u64>)| {
                match seen {
                    None => {
                        // Capture before yielding, so any event that lands
                        // between here and the next await is still newer than
                        // what we recorded and cannot be missed.
                        let generation = signal.generation();
                        Some((PortalWake { generation }, (signal, Some(generation))))
                    }
                    Some(seen) => {
                        let generation = signal.wait_after(seen).await;
                        Some((PortalWake { generation }, (signal, Some(generation))))
                    }
                }
            },
        )
    })
}

/// Drives `pending` toward a terminal outcome, returning a `Task` carrying
/// that outcome once reached.
///
/// This is the core invariant: the polling / late-cancel-adoption /
/// close-on-cancel sequence lived only in the probe. Every app would have
/// rewritten it identically; a bug in that rewrite (forgetting `settle`,
/// dropping a cross-thread cancel) is silent, because a pump loop that skips
/// its `settle` leaves the dialog owed a close that never happens — the *user*
/// sees a request that stays "pending" forever, on the server side, and no
/// assertion in the app catches it.
///
/// # What it does, precisely
///
/// - Polls `backend` for `pending`'s reply. If a terminal reply arrived, the
///   outcome is returned in a `Task::done`. `PendingRequest::poll` already
///   refuses to overwrite a prior cancellation with a late success — this
///   helper does not paper over that; a late reply after cancel simply
///   produces no `Task`.
/// - Adopts a cross-thread cancel: if [`PortalCancel::cancel`] was called on a
///   `Send` clone (`pending.cancel_handle().cancel(reason)`), `poll` returns
///   `None` because the request is already terminal, but the terminal *state*
///   the handle recorded is visible through `pending.cancel_reason()`. Without
///   this branch, a UI showing "pending…" would stay stuck after an
///   application-side cancel. The adoption is made by the call that seals the
///   request and takes the request's one-shot delivery claim, so a recorded
///   cancellation is reported once however often the same `pending`, or any
///   clone of it, is pumped afterwards.
/// - Performs any owed `Request::close()` via [`PendingRequest::settle`],
///   which is idempotent (the one-shot lives in `PortalCancel`), so calling
///   `pump` in a tight loop is safe and correct.
///
/// # Message shape
///
/// The caller supplies `on_outcome`: a function converting a terminal
/// [`PortalOutcome`] into the app's message type. This keeps the helper free
/// of any app-specific `Message` enum while producing a real `iced::Task`
/// the app can return straight from `update`.
///
/// # Return
///
/// - `iced::Task::none` while the request is still live and no outcome has
///   arrived — the app goes back to sleep and `pump`s again on the next portal
///   event delivered by [`completion_subscription`].
/// - `Task::done(on_outcome(outcome))` on the exactly-one tick where a
///   terminal outcome is observed. Subsequent calls with the same `pending`
///   return `Task::none` for every terminal route — a consumed reply and an
///   adopted cancellation alike — so an app that forgets to drop `pending`
///   after a terminal tick does not double-emit. The claim is shared by every
///   clone of the request, so two owners pumping concurrently also produce one
///   message between them.
pub fn pump<Message: Send + 'static>(
    pending: &mut PendingRequest,
    backend: &mut dyn PortalBackend,
    on_outcome: impl FnOnce(PortalOutcome<FileResult>) -> Message,
) -> iced::Task<Message> {
    match pump_outcome(pending, backend) {
        Some(outcome) => iced::Task::done(on_outcome(outcome)),
        None => iced::Task::none(),
    }
}

/// [`pump`] without the `Task`: the terminal outcome, once.
fn pump_outcome(
    pending: &mut PendingRequest,
    backend: &mut dyn PortalBackend,
) -> Option<PortalOutcome<FileResult>> {
    let owed_settlement = pending.is_unsettled();

    // Poll first: the common case is that the request is still live and this
    // returns None.
    let mut outcome = pending.poll(backend);

    // `poll` returns None for an already-terminal request by design (a late
    // reply after a cancel must not overwrite the cancel), so adoption is the
    // only way for `pump` to observe an off-thread `PortalCancel::cancel`.
    let adopting = outcome.is_none();

    // Perform any owed close, unconditionally. `settle` is idempotent — the
    // one-shot lives in `PortalCancel` — so a caller that pumps in a loop or
    // calls twice from two places does not double-close.
    pending.settle(backend);

    // The claim is taken last: a call that did not seal the request leaves it
    // for whichever call does, and a cancellation is never reported twice.
    if adopting
        && owed_settlement
        && !pending.is_unsettled()
        && let Some(reason) = pending.claim_cancel_report()
    {
        outcome = Some(PortalOutcome::Cancelled(reason));
    }

    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use enzui_core::portal::backend::MockBackend;

    // These tests exercise the helper against the in-process `MockBackend`;
    // there is no D-Bus, no dialog, no session bus. The `DbusBackend`
    // re-export is a link-only check — if it disappeared, `use` at the top
    // of the file would fail compile before any test ran.

    fn live(_: iced::window::Id) -> bool {
        true
    }

    fn start(op: PortalOperation, backend: &mut MockBackend) -> PendingRequest {
        FileRequest::new(op, PortalParent::None)
            .start(backend, live)
            .expect("mock start")
    }

    /// A cross-thread cancel through the `Send` handle records a terminal
    /// state that `poll` deliberately cannot see (a late reply must not
    /// overwrite a cancel). `pump` adopts it via `cancel_reason()` and
    /// emits an outcome so the app UI does not stay stuck "pending"
    /// after an application-side cancel.
    ///
    /// The check that catches a missing adoption branch is that
    /// `on_outcome` runs: `is_pending()` alone reads shared state that
    /// the cancel handle already flipped and so would pass even if `pump`
    /// silently swallowed the cancel. Capturing whether `on_outcome` was
    /// invoked is what proves the helper produced a Task with the outcome.
    ///
    /// Sabotage: remove the `cancel_reason()` adoption branch in `pump`
    /// and this test reddens — `captured` stays `None` because `outcome`
    /// is `None` and `on_outcome` is never called.
    #[test]
    fn pump_adopts_offthread_cancel() {
        use std::cell::RefCell;

        let mut backend = MockBackend::new();
        let mut pending = start(PortalOperation::OpenFile, &mut backend);

        // Cancel through the Send clone, as another thread would.
        let handle = pending.cancel_handle();
        assert!(handle.cancel(CancelReason::Application));

        let captured: RefCell<Option<PortalOutcome<FileResult>>> = RefCell::new(None);
        let _task = pump(&mut pending, &mut backend, |o| {
            *captured.borrow_mut() = Some(o.clone());
            o
        });

        // The cross-thread cancel drove the shared state terminal, so this
        // side of the test would pass with or without the adoption branch
        // — keep it as a smoke check.
        assert!(!pending.is_pending());
        assert_eq!(pending.cancel_reason(), Some(CancelReason::Application));

        // The property the adoption branch is responsible for: the pump
        // produced an outcome. Without the branch, `outcome` stays None
        // (poll returned None on the already-terminal request) and
        // `on_outcome` is never called.
        assert_eq!(
            captured.into_inner(),
            Some(PortalOutcome::Cancelled(CancelReason::Application)),
            "pump must adopt a cross-thread cancel as its outcome",
        );
    }
}
