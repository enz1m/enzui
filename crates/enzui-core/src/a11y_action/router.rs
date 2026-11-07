//! Thread-safe per-window routing and bounded queuing of resolved accessibility actions.

use super::*;
use crate::collect::NodeIdMap;
use accesskit::{Action, ActionData, NodeId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

/// Shared bridge from AccessKit requests to queued application actions.
///
/// Clones share per-window maps, the pending queue, budget, counters, and wake
/// registration. Queue and text-payload limits are enforced by `handle`.
#[derive(Clone, Default)]
pub struct ActionRouter {
    maps: Arc<Mutex<HashMap<u64, NodeIdMap>>>,
    // `actions[i]` and `windows[i]` describe the same queued request.
    pending: Arc<Mutex<PendingQueue>>,
    budget: Arc<Mutex<ActionRouterBudget>>,
    dropped_full: Arc<std::sync::atomic::AtomicU64>,
    refused_oversize: Arc<std::sync::atomic::AtomicU64>,
    coalesced: Arc<std::sync::atomic::AtomicU64>,
    // Keep only a weak callback here so a callback capturing a router clone cannot form an ownership cycle.
    wake: Arc<Mutex<Option<WeakWake>>>,
}

/// The router's wake callback, held weakly.
type WeakWake = Weak<dyn Fn() + Send + Sync>;

/// Keeps a router wake callback alive until this registration is dropped.
///
/// Dropping it disconnects the weak callback; `ActionRouter::clear_wake` can
/// disconnect it earlier.
#[must_use = "dropping the registration immediately disconnects the wake callback"]
pub struct WakeRegistration {
    _wake: Arc<dyn Fn() + Send + Sync>,
}

/// A resolved accessibility action paired with the window that published its target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowedAccessibilityAction {
    /// Numeric token for the target window.
    pub window: u64,
    /// Action resolved in the window node map.
    pub action: AccessibilityAction,
}

impl std::fmt::Debug for WakeRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WakeRegistration").finish_non_exhaustive()
    }
}

#[derive(Default)]
struct PendingQueue {
    actions: Vec<AccessibilityAction>,
    windows: Vec<u64>,
}

impl PendingQueue {
    fn len(&self) -> usize {
        debug_assert_eq!(
            self.actions.len(),
            self.windows.len(),
            "every action has exactly one window"
        );
        self.actions.len()
    }
}

impl std::fmt::Debug for ActionRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActionRouter")
            .field("pending", &self.pending.lock().map_or(0, |p| p.len()))
            .field("wake_installed", &self.wake_installed())
            .field("dropped_full", &self.dropped_full())
            .field("refused_oversize", &self.refused_oversize())
            .field("coalesced", &self.coalesced())
            .finish_non_exhaustive()
    }
}

impl ActionRouter {
    /// A new router with an empty map and no pending actions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Installs the wake callback invoked after a successful queue-growing enqueue.
    ///
    /// The callback is shared by all router clones and may run on the AccessKit
    /// thread. The returned registration must remain alive. A later installation
    /// replaces the current callback.
    pub fn set_wake<F>(&self, wake: F) -> WakeRegistration
    where
        F: Fn() + Send + Sync + 'static,
    {
        // The registration owns the callback; the router stores a weak reference to avoid cycles.
        let new = Arc::new(wake) as Arc<dyn Fn() + Send + Sync>;
        let weak = Arc::downgrade(&new);
        if let Ok(mut slot) = self.wake.lock() {
            *slot = Some(weak);
        }
        WakeRegistration { _wake: new }
    }

    /// Disconnects the shared wake callback.
    ///
    /// A callback already upgraded by an enqueue may still finish.
    pub fn clear_wake(&self) {
        if let Ok(mut slot) = self.wake.lock() {
            *slot = None;
        }
    }

    /// Returns whether the shared wake callback is still live.
    pub fn wake_installed(&self) -> bool {
        self.wake
            .lock()
            .is_ok_and(|w| w.as_ref().and_then(Weak::upgrade).is_some())
    }

    /// Replaces the trust-boundary budget shared by all router clones.
    pub fn set_budget(&self, budget: ActionRouterBudget) {
        if let Ok(mut b) = self.budget.lock() {
            *b = budget;
        }
    }

    /// The budget currently in force. Never fails; returns
    /// [`ActionRouterBudget::DEFAULT`] if the lock is poisoned.
    pub fn budget(&self) -> ActionRouterBudget {
        self.budget
            .lock()
            .map_or(ActionRouterBudget::DEFAULT, |b| *b)
    }

    /// Returns the lifetime count of enqueues refused because the queue was full.
    pub fn dropped_full(&self) -> u64 {
        self.dropped_full.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Returns the lifetime count of oversized live `SetValue` text payloads detected.
    ///
    /// The count increments before queue-capacity handling and may therefore exceed
    /// the number of drainable `SetValueRefused` actions.
    pub fn refused_oversize(&self) -> u64 {
        self.refused_oversize
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Returns the lifetime number of adjacent same-target `SetValue` enqueues coalesced in place.
    ///
    /// Coalescing neither grows the queue nor schedules another wake.
    pub fn coalesced(&self) -> u64 {
        self.coalesced.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Replaces the routing map for one window without changing other windows.
    ///
    /// Single-window hosts may use `A11Y_UNKEYED_WINDOW` only on both publication
    /// and request paths.
    pub fn set_map(&self, window: u64, map: NodeIdMap) {
        if let Ok(mut maps) = self.maps.lock() {
            maps.insert(window, map);
        }
    }

    /// Removes a window map and all queued actions for that window.
    ///
    /// Returns whether anything was removed. Removal is atomic with respect to
    /// request resolution and enqueue for the same window.
    pub fn drop_window(&self, window: u64) -> bool {
        // If the map lock is poisoned, fail closed rather than taking the queue lock out of order.
        let Ok(mut maps) = self.maps.lock() else {
            return false;
        };
        let dropped_map = maps.remove(&window).is_some();

        let purged_pending = match self.pending.lock() {
            Ok(mut pending) => {
                let before = pending.actions.len();
                let mut kept_actions = Vec::with_capacity(before);
                let mut kept_windows = Vec::with_capacity(before);
                let queued_actions = std::mem::take(&mut pending.actions);
                let queued_windows = std::mem::take(&mut pending.windows);
                for (queued, action) in queued_windows.into_iter().zip(queued_actions) {
                    if queued != window {
                        kept_windows.push(queued);
                        kept_actions.push(action);
                    }
                }
                pending.windows = kept_windows;
                pending.actions = kept_actions;
                pending.actions.len() != before
            }
            Err(_) => false,
        };

        drop(maps);

        dropped_map || purged_pending
    }

    /// Resolves a request against one window and enqueues the resulting action.
    ///
    /// Returns `None` for an unroutable target or a full queue. Oversized live
    /// `SetValue` text is represented as `SetValueRefused` without copying the
    /// payload and is enqueued only when queue capacity permits. Resolution and
    /// enqueue are atomic with `drop_window` for the same window.
    pub fn handle(
        &self,
        window: u64,
        action: Action,
        target: NodeId,
        data: Option<&ActionData>,
    ) -> Option<AccessibilityAction> {
        let budget = self.budget();
        // Hold `maps` through enqueue and acquire `pending` second so window removal cannot be followed by a stale enqueue.
        let maps = self.maps.lock().ok()?;
        let resolved = {
            let map = maps.get(&window)?;

            // Check live `SetValue` text before resolution copies it; non-settable targets remain `Unhandled`.
            if action == Action::SetValue {
                if let Some(ActionData::Value(v)) = data {
                    let len = v.len();
                    if len > budget.max_payload_bytes {
                        let live = matches!(
                            map.routing(target),
                            Some(r) if !r.disabled && !r.read_only && (r.text_editable || r.numeric_settable)
                        );
                        if live {
                            let id = map.widget_id(target)?.clone();
                            AccessibilityAction::SetValueRefused {
                                id,
                                len,
                                cap: budget.max_payload_bytes,
                            }
                        } else {
                            resolve_action(action, target, data, map)?
                        }
                    } else {
                        resolve_action(action, target, data, map)?
                    }
                } else {
                    resolve_action(action, target, data, map)?
                }
            } else {
                resolve_action(action, target, data, map)?
            }
        };

        #[cfg(test)]
        super::interleave::before_enqueue();

        if matches!(resolved, AccessibilityAction::SetValueRefused { .. }) {
            self.refused_oversize
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }

        // Coalesce only an adjacent `SetValue` for the same window and widget; otherwise enforce the count cap and wake only after queue growth.
        let retained_and_grew = if let Ok(mut pending) = self.pending.lock() {
            let coalesce_target = match &resolved {
                AccessibilityAction::SetValue { id, .. } => pending
                    .actions
                    .last()
                    .zip(pending.windows.last().copied())
                    .and_then(|(last, last_window)| match last {
                        AccessibilityAction::SetValue { id: last_id, .. }
                            if last_id == id && last_window == window =>
                        {
                            Some(pending.actions.len() - 1)
                        }
                        _ => None,
                    }),
                _ => None,
            };
            if let Some(i) = coalesce_target {
                pending.actions[i] = resolved.clone();
                self.coalesced
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                false
            } else if pending.len() < budget.max_pending {
                pending.actions.push(resolved.clone());
                pending.windows.push(window);
                true
            } else {
                self.dropped_full
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return None;
            }
        } else {
            false
        };

        drop(maps);

        // Wake only after queue growth, upgrading the shared weak callback and releasing its lock before invoking application code.
        if retained_and_grew {
            let wake = self
                .wake
                .lock()
                .ok()
                .and_then(|slot| slot.as_ref().and_then(Weak::upgrade));
            if let Some(wake) = wake {
                wake();
            }
        }
        Some(resolved)
    }

    /// Drains all pending actions, without window tokens, for single-window consumers.
    pub fn drain(&self) -> Vec<AccessibilityAction> {
        self.drain_windowed()
            .into_iter()
            .map(|routed| routed.action)
            .collect()
    }

    /// Drains all pending actions with their publishing window.
    ///
    /// Multi-window consumers must use this form to dispatch each action to its
    /// originating window.
    pub fn drain_windowed(&self) -> Vec<WindowedAccessibilityAction> {
        match self.pending.lock() {
            Ok(mut pending) => {
                let actions = std::mem::take(&mut pending.actions);
                let windows = std::mem::take(&mut pending.windows);
                windows
                    .into_iter()
                    .zip(actions)
                    .map(|(window, action)| WindowedAccessibilityAction { window, action })
                    .collect()
            }
            Err(_) => Vec::new(),
        }
    }

    pub(crate) fn pending_is_empty(&self) -> bool {
        self.pending
            .lock()
            .is_ok_and(|pending| pending.actions.is_empty())
    }

    /// Returns the number of routable entries published for `window`, or zero if none.
    pub fn routable_len(&self, window: u64) -> usize {
        self.maps
            .lock()
            .map_or(0, |maps| maps.get(&window).map_or(0, NodeIdMap::len))
    }

    /// Returns the number of windows with published routing maps.
    pub fn window_count(&self) -> usize {
        self.maps.lock().map_or(0, |maps| maps.len())
    }
}
