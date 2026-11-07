//! Direct desktop notifications on a privately owned D-Bus connection.
//!
//! Enable `notification-freedesktop` and pass this backend to
//! [`DesktopNotificationClient`]. There is no
//! automatic fallback from the portal: applications choose their transport.
//! Inline replies use the KDE-compatible `NotificationReplied` extension and
//! require the server's `actions` and `inline-reply` capabilities.
//! An installed D-Bus-activatable notification service is started when no
//! owner is running. Service replacement invalidates registrations; re-send
//! durable ids after a ready [`NotificationEvent::ServiceChanged`] event.
//! Desktop policy still owns presentation, history, DND, and lock-screen behavior.

use super::*;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::{mpsc, watch};

mod worker;

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const SIGNAL_QUEUE_CAPACITY: usize = 128;

#[derive(Clone)]
struct Ready {
    owner: String,
    generation: u64,
    capabilities: HashSet<String>,
}

#[derive(Default)]
struct State {
    initialising: bool,
    ready: Option<Ready>,
    generation: u64,
    events: VecDeque<NotificationBackendEvent>,
    in_flight: HashSet<NotificationOperationId>,
    next_operation: u64,
    dropped: u64,
    rejected: u64,
    completion: CompletionSignal,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl State {
    fn publish(&mut self, event: NotificationBackendEvent) {
        if matches!(event, NotificationBackendEvent::Action(_))
            && self
                .events
                .iter()
                .filter(|event| matches!(event, NotificationBackendEvent::Action(_)))
                .count()
                >= NOTIFICATION_ACTION_QUEUE_CAPACITY
        {
            self.dropped = self.dropped.saturating_add(1);
        } else {
            self.events.push_back(event);
        }
    }

    fn owner_changed(&mut self, ready: Option<Ready>) {
        self.initialising = false;
        self.ready = ready;
        self.events.retain(|event| {
            !matches!(
                event,
                NotificationBackendEvent::OwnerChanged { .. }
                    | NotificationBackendEvent::Action(_)
                    | NotificationBackendEvent::Closed { .. }
            )
        });
        self.publish(NotificationBackendEvent::OwnerChanged {
            generation: self.generation,
            available: self.ready.is_some(),
        });
    }
}

fn publish(shared: &Mutex<State>, event: NotificationBackendEvent) {
    let completion = {
        let mut state = lock(shared);
        state.publish(event);
        state.completion.clone()
    };
    completion.notify();
}

fn owner_changed(shared: &Mutex<State>, generation: u64, ready: Option<Ready>) {
    let completion = {
        let mut state = lock(shared);
        state.generation = generation;
        state.owner_changed(ready);
        state.completion.clone()
    };
    completion.notify();
}

struct Command {
    operation: NotificationOperationId,
    ready: Ready,
    payload: NotificationBackendCommand,
}

/// Opt-in freedesktop transport with bounded command and action delivery.
///
/// Construction returns before connecting. Include the client's wake
/// subscription and drain its events to settle readiness. Each instance owns
/// its connection, runtime, and worker. It requests standard D-Bus activation
/// when no notification service owns the name. Dropping it cancels pending
/// calls and leaves posted notifications to desktop policy. Encoded icons, portal button
/// purposes, and portal-only display hints return terminal unsupported
/// operation events, including requests deferred during startup.
/// Themed icons use the first supplied name; high and urgent priority both map
/// to freedesktop urgency 2. Each bus call has a five-second deadline.
pub struct FreedesktopNotificationBackend {
    state: Arc<Mutex<State>>,
    commands: mpsc::Sender<Command>,
    shutdown: watch::Sender<bool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl FreedesktopNotificationBackend {
    /// Starts a private connection to the ambient session bus.
    ///
    /// # Errors
    /// Returns an I/O error if the worker thread cannot be created. Deferred
    /// bus or negotiation failures arrive as unavailable owner events.
    pub fn new(application_id: AppId) -> std::io::Result<Self> {
        Self::with_address(application_id, None)
    }

    /// Starts a private connection on an explicit bus, or the session bus for `None`.
    ///
    /// `application_id` supplies the `desktop-entry` hint; use the same identity
    /// when constructing the typed client.
    ///
    /// # Errors
    /// Returns an I/O error if the worker thread cannot be created.
    pub fn with_address(application_id: AppId, address: Option<String>) -> std::io::Result<Self> {
        let state = Arc::new(Mutex::new(State {
            initialising: true,
            ..State::default()
        }));
        let (commands, receiver) = mpsc::channel(MAX_PENDING_NOTIFICATION_OPERATIONS);
        let (shutdown, mut stop) = watch::channel(false);
        let shared = Arc::clone(&state);
        let worker = std::thread::Builder::new()
            .name("desktop-notifications".into())
            .spawn(move || {
                if let Ok(runtime) = tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    runtime.block_on(async {
                        tokio::select! {
                            _ = worker::run(
                                application_id,
                                address,
                                receiver,
                                Arc::clone(&shared),
                            ) => {},
                            _ = stop.changed() => {},
                        }
                    });
                }
                let mut state = lock(&shared);
                state.generation = state.generation.saturating_add(1);
                state.owner_changed(None);
                let generation = state.generation;
                let unfinished: Vec<_> = state.in_flight.iter().copied()
                    .filter(|operation| !state.events.iter().any(|event| {
                        matches!(event, NotificationBackendEvent::Operation { operation: done, .. } if done == operation)
                    }))
                    .collect();
                for operation in unfinished {
                    state.publish(NotificationBackendEvent::Operation { operation, owner_generation: generation, outcome: NotificationBackendOutcome::Unavailable });
                }
                let completion = state.completion.clone();
                drop(state);
                completion.notify();
            })?;
        Ok(Self {
            state,
            commands,
            shutdown,
            worker: Some(worker),
        })
    }
}

impl NotificationBackend for FreedesktopNotificationBackend {
    fn submit_notification(
        &mut self,
        payload: NotificationBackendCommand,
    ) -> Result<NotificationOperationId, NotificationBackendError> {
        let mut state = lock(&self.state);
        if state.initialising {
            return Err(NotificationBackendError::Initialising);
        }
        let ready = state
            .ready
            .clone()
            .ok_or(NotificationBackendError::Unavailable)?;
        if let NotificationBackendCommand::Send(notice) = &payload {
            notice
                .validate()
                .map_err(|_| NotificationBackendError::Failed)?;
        }
        if state.in_flight.len() >= MAX_PENDING_NOTIFICATION_OPERATIONS {
            return Err(NotificationBackendError::ResourceExhausted);
        }
        let operation = NotificationOperationId::new(
            state
                .next_operation
                .checked_add(1)
                .ok_or(NotificationBackendError::ResourceExhausted)?,
        );
        self.commands
            .try_send(Command {
                operation,
                ready,
                payload,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => NotificationBackendError::ResourceExhausted,
                mpsc::error::TrySendError::Closed(_) => NotificationBackendError::Unavailable,
            })?;
        state.next_operation = operation.get();
        state.in_flight.insert(operation);
        Ok(operation)
    }

    fn poll_notification_event(&mut self) -> Option<NotificationBackendEvent> {
        let mut state = lock(&self.state);
        let event = state.events.pop_front()?;
        if let NotificationBackendEvent::Operation { operation, .. } = &event {
            state.in_flight.remove(operation);
        }
        Some(event)
    }

    fn notification_completion_signal(&self) -> Option<CompletionSignal> {
        Some(lock(&self.state).completion.clone())
    }

    fn notification_stats(&self) -> NotificationBackendStats {
        let state = lock(&self.state);
        NotificationBackendStats {
            pending_actions: state
                .events
                .iter()
                .filter(|event| matches!(event, NotificationBackendEvent::Action(_)))
                .count(),
            action_capacity: NOTIFICATION_ACTION_QUEUE_CAPACITY,
            dropped_actions: state.dropped,
            rejected_actions: state.rejected,
        }
    }
}

impl Drop for FreedesktopNotificationBackend {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl fmt::Debug for FreedesktopNotificationBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let state = lock(&self.state);
        f.debug_struct("FreedesktopNotificationBackend")
            .field("initialising", &state.initialising)
            .field("available", &state.ready.is_some())
            .field("in_flight", &state.in_flight.len())
            .finish()
    }
}
