//! Runtime bootstrap and accessibility action delivery for enzUI applications.
//!
//! [`run`] installs the supported accessibility bridge before entering the event loop. On supported Linux builds, trees and actions are keyed by exact window ids; other builds expose inert counterparts so application update code remains configuration-independent.

#[cfg(feature = "native-invalidation")]
pub mod native_invalidation;

/// Identifies the action batch represented by an accessibility wake.
///
/// Pass it to [`drain_accessibility_wake`]; a consumed or purged batch returns `None`.
#[cfg(all(feature = "accesskit", target_os = "linux"))]
#[derive(Clone)]
pub struct AccessibilityWake(std::sync::Arc<enabled::WakeBatch>);

/// Disabled-build counterpart. No receipt can be constructed because the wake
/// subscription is inert, while an application's message enum still compiles.
#[cfg(not(all(feature = "accesskit", target_os = "linux")))]
#[derive(Clone, PartialEq, Eq)]
pub struct AccessibilityWake(std::convert::Infallible);

/// A message for a runtime service an adapter installed, opaque to the
/// application: the adapter that sent it downcasts it back. Two messages are
/// equal when they are the same message.
#[derive(Clone)]
pub struct ServiceMessage(std::sync::Arc<dyn std::any::Any + Send + Sync>);

impl ServiceMessage {
    /// Wraps `message`.
    pub fn new<T: std::any::Any + Send + Sync>(message: T) -> Self {
        Self(std::sync::Arc::new(message))
    }

    /// The message, when it is a `T`.
    #[must_use]
    pub fn downcast_ref<T: std::any::Any>(&self) -> Option<&T> {
        self.0.downcast_ref()
    }
}

impl PartialEq for ServiceMessage {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ServiceMessage {}

impl std::fmt::Debug for ServiceMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceMessage").finish_non_exhaustive()
    }
}

impl std::fmt::Debug for AccessibilityWake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessibilityWake").finish_non_exhaustive()
    }
}

#[cfg(all(feature = "accesskit", target_os = "linux"))]
impl PartialEq for AccessibilityWake {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

#[cfg(all(feature = "accesskit", target_os = "linux"))]
impl Eq for AccessibilityWake {}

/// Which registration a [`install_accessibility`] call performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessibilityOwnership {
    /// This call installed the process-global bridge.
    Seated,
    /// This owner had already installed it; nothing was re-registered.
    AlreadySeated,
    /// Native accessibility registration is unavailable in this build.
    Inert,
}

/// Reports an attempted accessibility registration by a second owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessibilityConflict {
    /// The owner that already holds the registration.
    pub owner: &'static str,
    /// The owner that was refused.
    pub requested: &'static str,
}

#[cfg(all(feature = "accesskit", target_os = "linux"))]
pub(crate) const FOREIGN_OWNER: &str = "an unmanaged iced_winit::accessibility registration";

#[cfg(all(feature = "accesskit", target_os = "linux"))]
pub(crate) const BOOTSTRAP_OWNER: &str = "enzui_core::app_runtime::run";

impl std::fmt::Display for AccessibilityConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the process-global accessibility bridge is already owned by {}, so {} was refused. \
             iced_winit's registrations are single-writer: keeping the first one silently would \
             leave this process with an inert or unroutable accessibility tree. Install exactly \
             one owner — ordinarily enzui_core::app_runtime::run — and consume actions with \
             app_runtime::accessibility_wake_subscription + drain_accessibility_wake instead \
             of registering a second collector.",
            self.owner, self.requested,
        )
    }
}

impl std::error::Error for AccessibilityConflict {}

/// Runs a blocking iced event loop within enzUI runtime services.
///
/// Accessibility registration completes before `event_loop` is invoked, and the closure result is returned unchanged.
///
/// # Panics
///
/// Panics if a different owner already registered the process-global accessibility bridge.
#[allow(
    clippy::panic,
    reason = "a second accessibility owner is a programming error, so startup stops"
)]
pub fn run<R>(event_loop: impl FnOnce() -> R) -> R {
    run_bootstrapped(
        || {
            if let Err(conflict) = install_accessibility() {
                panic!("{conflict}");
            }
        },
        event_loop,
    )
}

fn run_bootstrapped<R>(bootstrap: impl FnOnce(), event_loop: impl FnOnce() -> R) -> R {
    bootstrap();
    crate::widget_runtime::run(event_loop)
}

/// Installs the process-global native accessibility bridge.
///
/// Repeated installation by this owner returns [`AccessibilityOwnership::AlreadySeated`]; a different owner returns [`AccessibilityConflict`].
///
/// # Errors
///
/// [`AccessibilityConflict`] when another owner already installed the bridge.
#[cfg(all(feature = "accesskit", target_os = "linux"))]
pub fn install_accessibility() -> Result<AccessibilityOwnership, AccessibilityConflict> {
    seat_accessibility(BOOTSTRAP_OWNER, || {
        // Install the wake route before an application can create its first subscription.
        install_router_wake();
        // Register all bridge callbacks atomically so tree publication, routing, and close cleanup cannot acquire different owners.
        iced_winit::accessibility::try_register_bridge(iced_winit::accessibility::Bridge::new(
            |window, title, run| {
                Some(build_tree(
                    accessibility_router(),
                    window_identities(),
                    title,
                    window,
                    run,
                ))
            },
            |request| {
                route_request(accessibility_router(), pipeline_transition(), request);
            },
            |window| {
                close_window(
                    accessibility_router(),
                    window_identities(),
                    wake_channel(),
                    pipeline_transition(),
                    window,
                );
            },
        ))
    })
}

/// Returns [`AccessibilityOwnership::Inert`] when native accessibility is unavailable.
///
/// # Errors
/// This fallback always succeeds.
#[cfg(not(all(feature = "accesskit", target_os = "linux")))]
#[inline]
pub fn install_accessibility() -> Result<AccessibilityOwnership, AccessibilityConflict> {
    Ok(AccessibilityOwnership::Inert)
}

/// Returns an empty accessibility wake subscription when native accessibility is unavailable.
#[cfg(not(all(feature = "accesskit", target_os = "linux")))]
#[inline]
pub fn accessibility_wake_subscription<Message>(
    _on_wake: fn(AccessibilityWake) -> Message,
) -> iced_futures::Subscription<Message>
where
    Message: Send + 'static,
{
    iced_futures::Subscription::none()
}

/// Accessibility actions in a configuration-independent application vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccessibilityActionKind {
    /// Move keyboard focus to the widget.
    Focus,
    /// Activate the widget — the Enter/Space/click path.
    Activate,
    /// Bump a range widget's value up one step.
    Increment,
    /// Bump a range widget's value down one step.
    Decrement,
    /// Replace an editable widget's value; the text is
    /// [`RoutedAccessibilityAction::value`].
    SetValue,
    /// A `SetValue` request rejected because its payload exceeded the router limit.
    SetValueRefused,
    /// Open the widget's context menu (the right-click path).
    ShowContextMenu,
    /// Expand a collapsed disclosure control.
    Expand,
    /// Collapse an expanded disclosure control.
    Collapse,
    /// Reveal the widget inside its nearest ancestor scrollable.
    ScrollIntoView,
    /// Move the caret or select text in a text input; the range is
    /// [`RoutedAccessibilityAction::text_selection`].
    SetTextSelection,
    /// A resolved widget action with no supported interaction mapping.
    Unhandled,
}

/// A resolved accessibility action paired with the exact window that published its target.
///
/// Apply it only in [`Self::window`], for example through [`window_operation_task`]. In builds without native accessibility, the counterpart type cannot be constructed.
#[cfg(all(feature = "accesskit", target_os = "linux"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedAccessibilityAction {
    window: iced_core::window::Id,
    action: crate::a11y_action::AccessibilityAction,
}

#[cfg(all(feature = "accesskit", target_os = "linux"))]
impl RoutedAccessibilityAction {
    /// Pairs a resolved action with the window whose published tree resolved it.
    ///
    /// [`drain_accessibility_actions`] already returns paired actions; this is for a host that
    /// operates its own [`ActionRouter`](crate::a11y_action::ActionRouter) and needs the same
    /// application vocabulary.
    pub fn new(
        window: iced_core::window::Id,
        action: crate::a11y_action::AccessibilityAction,
    ) -> Self {
        Self { window, action }
    }

    /// Returns the exact window that published the target node.
    pub fn window(&self) -> iced_core::window::Id {
        self.window
    }

    /// The widget the action resolved to, inside that window's tree.
    pub fn widget(&self) -> &iced_core::widget::Id {
        self.action.id()
    }

    /// Which action was requested, in the `cfg`-free vocabulary.
    pub fn kind(&self) -> AccessibilityActionKind {
        use crate::a11y_action::AccessibilityAction as A;

        match &self.action {
            A::Focus(_) => AccessibilityActionKind::Focus,
            A::Activate(_) => AccessibilityActionKind::Activate,
            A::Increment(_) => AccessibilityActionKind::Increment,
            A::Decrement(_) => AccessibilityActionKind::Decrement,
            A::SetValue { .. } => AccessibilityActionKind::SetValue,
            A::SetValueRefused { .. } => AccessibilityActionKind::SetValueRefused,
            A::ShowContextMenu(_) => AccessibilityActionKind::ShowContextMenu,
            A::Expand(_) => AccessibilityActionKind::Expand,
            A::Collapse(_) => AccessibilityActionKind::Collapse,
            A::ScrollIntoView(_) => AccessibilityActionKind::ScrollIntoView,
            A::SetTextSelection { .. } => AccessibilityActionKind::SetTextSelection,
            A::Unhandled { .. } => AccessibilityActionKind::Unhandled,
        }
    }

    /// Returns the unvalidated proposed text for `SetValue`, or `None` for other actions.
    pub fn value(&self) -> Option<&str> {
        match &self.action {
            crate::a11y_action::AccessibilityAction::SetValue { value, .. } => Some(value),
            _ => None,
        }
    }

    /// Returns the selection anchor and caret position for `SetTextSelection`,
    /// or `None` for other actions.
    pub fn text_selection(&self) -> Option<(iced_core::text::Position, iced_core::text::Position)> {
        match &self.action {
            crate::a11y_action::AccessibilityAction::SetTextSelection { anchor, focus, .. } => {
                Some((*anchor, *focus))
            }
            _ => None,
        }
    }

    /// Returns the full AccessKit-aware action, including refusal and unmapped-action details.
    pub fn action(&self) -> &crate::a11y_action::AccessibilityAction {
        &self.action
    }
}

/// Uninhabited counterpart used when the native accessibility bridge is unavailable.
#[cfg(not(all(feature = "accesskit", target_os = "linux")))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedAccessibilityAction(std::convert::Infallible);

#[cfg(not(all(feature = "accesskit", target_os = "linux")))]
impl RoutedAccessibilityAction {
    /// Unreachable in a build with no accessibility bridge.
    pub fn window(&self) -> iced_core::window::Id {
        match self.0 {}
    }

    /// Unreachable in a build with no accessibility bridge.
    pub fn widget(&self) -> &iced_core::widget::Id {
        match self.0 {}
    }

    /// Unreachable in a build with no accessibility bridge.
    pub fn kind(&self) -> AccessibilityActionKind {
        match self.0 {}
    }

    /// Unreachable in a build with no accessibility bridge.
    pub fn value(&self) -> Option<&str> {
        match self.0 {}
    }

    /// Unreachable in a build with no accessibility bridge.
    pub fn text_selection(&self) -> Option<(iced_core::text::Position, iced_core::text::Position)> {
        match self.0 {}
    }
}

/// Drains all resolved actions with their exact publishing windows and acknowledges the current wake level.
///
/// Use [`drain_accessibility_wake`] when handling a wake receipt. Unsupported builds return an empty vector.
#[cfg(all(feature = "accesskit", target_os = "linux"))]
pub fn drain_accessibility_actions() -> Vec<RoutedAccessibilityAction> {
    enabled::drain_routed_and_acknowledge(
        enabled::accessibility_router(),
        enabled::window_identities(),
        enabled::wake_channel(),
        enabled::pipeline_transition(),
    )
}

/// Disabled-build counterpart: nothing can queue an action, so there is
/// nothing to drain and no state to hold.
#[cfg(not(all(feature = "accesskit", target_os = "linux")))]
#[inline]
pub fn drain_accessibility_actions() -> Vec<RoutedAccessibilityAction> {
    Vec::new()
}

/// Drains the batch identified by `receipt`.
///
/// Returns `None` if that batch was already consumed or purged. An older receipt never drains a newer batch.
#[cfg(all(feature = "accesskit", target_os = "linux"))]
pub fn drain_accessibility_wake(
    receipt: AccessibilityWake,
) -> Option<Vec<RoutedAccessibilityAction>> {
    enabled::drain_receipt(
        enabled::accessibility_router(),
        enabled::window_identities(),
        enabled::wake_channel(),
        enabled::pipeline_transition(),
        receipt,
    )
}

/// Disabled-build counterpart. No receipt is constructible and the
/// subscription emits no message, but the call site remains cfg-free.
#[cfg(not(all(feature = "accesskit", target_os = "linux")))]
#[inline]
pub fn drain_accessibility_wake(
    receipt: AccessibilityWake,
) -> Option<Vec<RoutedAccessibilityAction>> {
    match receipt.0 {}
}

/// Wakes the event loop for accessibility work outside the action router.
///
/// The wake uses the same coalesced receipt channel as routed actions, so a mailbox-only receipt drains as `Some` with no actions. Safe to call from any thread; unsupported builds do nothing.
#[cfg(all(feature = "accesskit", target_os = "linux"))]
pub fn notify_accessibility_wake() {
    enabled::wake_channel().notify();
}

/// Disabled-build counterpart: no wake channel exists, so there is nothing to
/// notify and no state to keep.
#[cfg(not(all(feature = "accesskit", target_os = "linux")))]
#[inline]
pub fn notify_accessibility_wake() {}

/// Runs a widget operation in exactly one live window.
///
/// If the window closes before execution, the operation is dropped rather than redirected.
pub fn window_operation_task<Message>(
    window: iced_core::window::Id,
    operation: impl iced_core::widget::Operation + 'static,
) -> iced_runtime::Task<Message> {
    iced_runtime::task::effect(iced_runtime::Action::<std::convert::Infallible>::widget_in(
        window, operation,
    ))
}

/// Focuses `widget` in exactly `window` using [`window_operation_task`].
pub fn window_focus_task<Message>(
    window: iced_core::window::Id,
    widget: iced_core::widget::Id,
) -> iced_runtime::Task<Message> {
    window_operation_task(
        window,
        iced_core::widget::operation::focusable::focus::<()>(widget),
    )
}

#[cfg(all(feature = "accesskit", target_os = "linux"))]
mod enabled {
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll};

    use iced_futures::Subscription;
    use iced_futures::futures::stream::{BoxStream, Stream, StreamExt};

    use super::{AccessibilityConflict, AccessibilityOwnership, FOREIGN_OWNER};
    use crate::a11y_action::{ActionRouter, WakeRegistration};

    // Guards one process-global registration and permits idempotence only for the same owner.
    pub(super) struct Registrar {
        owner: Mutex<Option<&'static str>>,
        seatings: AtomicUsize,
    }

    impl Registrar {
        pub(super) const fn new() -> Self {
            Self {
                owner: Mutex::new(None),
                seatings: AtomicUsize::new(0),
            }
        }

        pub(super) fn seat<E>(
            &self,
            owner: &'static str,
            foreign: impl Fn() -> bool,
            install: impl FnOnce() -> Result<(), E>,
        ) -> Result<AccessibilityOwnership, AccessibilityConflict> {
            let mut slot = self
                .owner
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match *slot {
                Some(existing) if existing == owner => {
                    let _ = self.seatings.fetch_add(1, Ordering::SeqCst);
                    Ok(AccessibilityOwnership::AlreadySeated)
                }
                Some(existing) => Err(AccessibilityConflict {
                    owner: existing,
                    requested: owner,
                }),
                None if foreign() => Err(AccessibilityConflict {
                    owner: FOREIGN_OWNER,
                    requested: owner,
                }),
                None => {
                    // An `Err` here means the platform registry was written
                    // between the `foreign` check and this call: report the
                    // conflict rather than claiming an ownership whose
                    // registration was discarded.
                    if install().is_err() {
                        return Err(AccessibilityConflict {
                            owner: FOREIGN_OWNER,
                            requested: owner,
                        });
                    }
                    *slot = Some(owner);
                    let _ = self.seatings.fetch_add(1, Ordering::SeqCst);
                    Ok(AccessibilityOwnership::Seated)
                }
            }
        }
    }

    pub(super) static REGISTRAR: Registrar = Registrar::new();

    pub(crate) fn seat_accessibility<E>(
        owner: &'static str,
        install: impl FnOnce() -> Result<(), E>,
    ) -> Result<AccessibilityOwnership, AccessibilityConflict> {
        // Either pre-existing callback constitutes a foreign owner because the bridge must be registered as one unit.
        REGISTRAR.seat(
            owner,
            || {
                platform_registration_exists(
                    iced_winit::accessibility::is_enabled(),
                    iced_winit::accessibility::has_request_handler(),
                    iced_winit::accessibility::has_window_closed_handler(),
                )
            },
            install,
        )
    }

    pub(super) fn platform_registration_exists(
        tree_builder: bool,
        request_handler: bool,
        window_closed: bool,
    ) -> bool {
        tree_builder || request_handler || window_closed
    }

    // Stores routes by the exact window that published each accessibility tree.
    pub(crate) fn accessibility_router() -> &'static ActionRouter {
        static ROUTER: std::sync::OnceLock<ActionRouter> = std::sync::OnceLock::new();
        ROUTER.get_or_init(ActionRouter::new)
    }

    // Maps router tokens back to the real window ids because `Id::as_u64` has no trusted inverse.
    #[derive(Default)]
    pub(super) struct WindowIdentities {
        exact: Mutex<std::collections::HashMap<u64, iced_core::window::Id>>,
        // Counts drained actions whose exact publishing window is no longer known.
        forgotten: AtomicUsize,
    }

    impl WindowIdentities {
        pub(super) fn new() -> Self {
            Self::default()
        }

        pub(super) fn remember(&self, window: iced_core::window::Id) {
            if let Ok(mut exact) = self.exact.lock() {
                let _ = exact.insert(window.as_u64(), window);
            }
        }

        pub(super) fn exact(&self, token: u64) -> Option<iced_core::window::Id> {
            self.exact.lock().ok().and_then(|exact| {
                let found = exact.get(&token).copied();
                if found.is_none() {
                    let _ = self.forgotten.fetch_add(1, Ordering::SeqCst);
                }
                found
            })
        }

        pub(super) fn forget(&self, window: iced_core::window::Id) {
            if let Ok(mut exact) = self.exact.lock() {
                let _ = exact.remove(&window.as_u64());
            }
        }
    }

    pub(super) fn window_identities() -> &'static WindowIdentities {
        static IDENTITIES: std::sync::OnceLock<WindowIdentities> = std::sync::OnceLock::new();
        IDENTITIES.get_or_init(WindowIdentities::new)
    }

    // Drop and count actions whose exact publishing window can no longer be recovered.
    pub(super) fn drain_routed(
        router: &ActionRouter,
        identities: &WindowIdentities,
    ) -> Vec<super::RoutedAccessibilityAction> {
        router
            .drain_windowed()
            .into_iter()
            .filter_map(|routed| {
                identities
                    .exact(routed.window)
                    .map(|window| super::RoutedAccessibilityAction {
                        window,
                        action: routed.action,
                    })
            })
            .collect()
    }

    // Serializes action publication and wake notification against queue drain and wake acknowledgement.
    pub(super) struct PipelineTransition {
        state: Mutex<()>,
    }

    impl PipelineTransition {
        pub(super) const fn new() -> Self {
            Self {
                state: Mutex::new(()),
            }
        }

        fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }
    }

    pub(super) fn pipeline_transition() -> &'static PipelineTransition {
        static TRANSITION: PipelineTransition = PipelineTransition::new();
        &TRANSITION
    }

    // Coalesces queued work into one batch level and terminates superseded stream generations.
    #[derive(Clone)]
    pub(super) struct WakeChannel {
        state: Arc<Mutex<WakeState>>,
    }

    enum WakeLevel {
        Idle,
        Pending(Arc<WakeBatch>),
        Delivered(Arc<WakeBatch>),
    }

    #[derive(Debug)]
    struct Generation;

    // Identifies a coalesced batch by `Arc` allocation identity, avoiding wraparound or forgery.
    #[derive(Debug)]
    pub(super) struct WakeBatch;

    struct WakeState {
        level: WakeLevel,
        generation: Arc<Generation>,
        waker: Option<std::task::Waker>,
        #[cfg(test)]
        delivered: u64,
    }

    struct WakeStream {
        state: Arc<Mutex<WakeState>>,
        generation: Arc<Generation>,
    }

    // Recover a poisoned level-state lock rather than permanently silencing accessibility wakes.
    fn wake_state(state: &Mutex<WakeState>) -> std::sync::MutexGuard<'_, WakeState> {
        state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    impl Stream for WakeStream {
        type Item = super::AccessibilityWake;

        fn poll_next(
            self: Pin<&mut Self>,
            context: &mut Context<'_>,
        ) -> Poll<Option<super::AccessibilityWake>> {
            let this = self.get_mut();
            let mut state = wake_state(&this.state);

            if !Arc::ptr_eq(&state.generation, &this.generation) {
                // Superseded. Pointer identity cannot wrap while this stream
                // still owns its token, so this is permanently terminal.
                return Poll::Ready(None);
            }

            match &state.level {
                WakeLevel::Pending(batch) => {
                    // Emission does not acknowledge the queue; the receipt lets a later update reject an already-consumed batch.
                    let batch = Arc::clone(batch);
                    state.level = WakeLevel::Delivered(Arc::clone(&batch));
                    #[cfg(test)]
                    {
                        state.delivered = state.delivered.saturating_add(1);
                    }
                    Poll::Ready(Some(super::AccessibilityWake(batch)))
                }
                WakeLevel::Idle | WakeLevel::Delivered(_) => {
                    state.waker = Some(context.waker().clone());
                    Poll::Pending
                }
            }
        }
    }

    impl WakeChannel {
        pub(super) fn new() -> Self {
            Self {
                state: Arc::new(Mutex::new(WakeState {
                    level: WakeLevel::Idle,
                    generation: Arc::new(Generation),
                    waker: None,
                    #[cfg(test)]
                    delivered: 0,
                })),
            }
        }

        // Publish a wake level and coalesce repeated notifications into the current batch.
        pub(super) fn notify(&self) {
            let waker = {
                let mut state = wake_state(&self.state);
                if matches!(&state.level, WakeLevel::Idle) {
                    state.level = WakeLevel::Pending(Arc::new(WakeBatch));
                    state.waker.take()
                } else {
                    None
                }
            };

            // Wake after releasing the lock because an inline executor may poll and reenter this mutex.
            if let Some(waker) = waker {
                waker.wake();
            }
        }

        pub(super) fn acknowledge_drain(&self) {
            let mut state = wake_state(&self.state);
            state.level = WakeLevel::Idle;
        }

        // Acknowledges only the delivered batch named by the receipt, leaving newer batches intact.
        fn acknowledge_receipt(&self, receipt: &super::AccessibilityWake) -> bool {
            let mut state = wake_state(&self.state);
            let current = matches!(
                &state.level,
                WakeLevel::Delivered(batch) if Arc::ptr_eq(batch, &receipt.0)
            );
            if current {
                state.level = WakeLevel::Idle;
            }
            current
        }

        // Checks receipt identity while the outer pipeline transition excludes queue changes.
        fn receipt_is_current(&self, receipt: &super::AccessibilityWake) -> bool {
            matches!(
                &wake_state(&self.state).level,
                WakeLevel::Delivered(batch) if Arc::ptr_eq(batch, &receipt.0)
            )
        }

        // Replace the active stream generation without changing the current wake level; the superseded stream is terminated.
        pub(super) fn take_stream(&self) -> BoxStream<'static, super::AccessibilityWake> {
            let (generation, superseded) = {
                let mut state = wake_state(&self.state);
                let generation = Arc::new(Generation);
                state.generation = Arc::clone(&generation);
                // The old task must be polled once more so it can observe the
                // token mismatch and terminate instead of waiting forever.
                (generation, state.waker.take())
            };

            // As in `notify`, wake outside the mutex: an executor may poll the
            // superseded stream inline, and that poll takes the same lock.
            if let Some(waker) = superseded {
                waker.wake();
            }

            WakeStream {
                state: Arc::clone(&self.state),
                generation,
            }
            .boxed()
        }
    }

    pub(super) fn wake_channel() -> &'static WakeChannel {
        static WAKE: std::sync::OnceLock<WakeChannel> = std::sync::OnceLock::new();
        WAKE.get_or_init(WakeChannel::new)
    }

    pub(super) fn install_router_wake() {
        static REGISTRATION: std::sync::OnceLock<WakeRegistration> = std::sync::OnceLock::new();
        let _ = REGISTRATION
            .get_or_init(|| accessibility_router().set_wake(|| wake_channel().notify()));
    }

    /// Subscribes to coalesced accessibility wake receipts.
    ///
    /// Pass each receipt to [`drain_accessibility_wake`](crate::app_runtime::drain_accessibility_wake). The wake is one
    /// member of the app's `Subscription::batch`, added beside the
    /// keyboard-focus stream that `docs/building-an-app.md` wires; replacing
    /// the whole subscription with this single call drops the keyboard half
    /// and silently loses Tab traversal.
    ///
    /// The keyboard member below is what a host binds when it owns traversal
    /// itself — the terminal/editor case, or
    /// [`crate::focus_traversal::TraversalBinding::Disabled`]. A host that
    /// enters the loop through the ordinary setup route already has Tab and
    /// Shift+Tab, and batches this wake beside its own feeds only; binding
    /// bare Tab there as well would advance focus twice per press.
    pub fn accessibility_wake_subscription<Message>(
        on_wake: fn(super::AccessibilityWake) -> Message,
    ) -> Subscription<Message>
    where
        Message: Send + 'static,
    {
        Subscription::run_with(on_wake, |on_wake| {
            let on_wake = *on_wake;
            wake_channel().take_stream().map(on_wake).boxed()
        })
    }

    pub(super) fn build_tree(
        router: &ActionRouter,
        identities: &WindowIdentities,
        title: Option<&str>,
        window: iced_core::window::Id,
        run: &mut dyn FnMut(&mut dyn iced_core::widget::Operation),
    ) -> accesskit::TreeUpdate {
        let (update, map) =
            crate::virtual_text::collect_frame_binding_providers(window, title, run);
        // Record the exact id before publishing the route map; draining drops actions whose token has no recorded id.
        identities.remember(window);
        router.set_map(window.as_u64(), map);
        update
    }

    pub(super) fn route_request(
        router: &ActionRouter,
        transition: &PipelineTransition,
        request: iced_winit::accessibility::Request,
    ) {
        let _transition = transition.lock();
        let action = request.request;
        let _ = router.handle(
            request.window.as_u64(),
            action.action,
            action.target_node,
            action.data.as_ref(),
        );
    }

    // Drain the whole router and acknowledge that queue state under one pipeline transition.
    pub(super) fn drain_routed_and_acknowledge(
        router: &ActionRouter,
        identities: &WindowIdentities,
        wake: &WakeChannel,
        transition: &PipelineTransition,
    ) -> Vec<super::RoutedAccessibilityAction> {
        let _transition = transition.lock();
        let drained = drain_routed(router, identities);
        wake.acknowledge_drain();
        drained
    }

    // Validate the delivered batch before draining while the pipeline transition excludes ordinary drains and newer enqueues.
    pub(super) fn drain_receipt(
        router: &ActionRouter,
        identities: &WindowIdentities,
        wake: &WakeChannel,
        transition: &PipelineTransition,
        receipt: super::AccessibilityWake,
    ) -> Option<Vec<super::RoutedAccessibilityAction>> {
        let _transition = transition.lock();
        if !wake.receipt_is_current(&receipt) {
            return None;
        }

        let drained = drain_routed(router, identities);
        let acknowledged = wake.acknowledge_receipt(&receipt);
        debug_assert!(
            acknowledged,
            "validated wake receipt changed under transition"
        );
        Some(drained)
    }

    // Purge one window and clear the wake level only when no queued actions remain.
    pub(super) fn close_window(
        router: &ActionRouter,
        identities: &WindowIdentities,
        wake: &WakeChannel,
        transition: &PipelineTransition,
        window: iced_core::window::Id,
    ) {
        let _transition = transition.lock();
        let _ = router.drop_window(window.as_u64());
        identities.forget(window);
        if router.pending_is_empty() {
            wake.acknowledge_drain();
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(feature = "accesskit", target_os = "linux"))]
pub(crate) use enabled::accessibility_router;
#[cfg(all(feature = "accesskit", target_os = "linux"))]
pub use enabled::accessibility_wake_subscription;
#[cfg(all(feature = "accesskit", target_os = "linux"))]
pub(crate) use enabled::seat_accessibility;
#[cfg(all(feature = "accesskit", target_os = "linux"))]
use enabled::{
    build_tree, close_window, install_router_wake, pipeline_transition, route_request,
    wake_channel, window_identities,
};
