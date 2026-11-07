//! D-Bus backend for ordered XDG ScreenCast sessions.
//!
//! Readiness uses bounded D-Bus activation and follows portal-owner changes. Each handshake step subscribes before making its method call, and session `Closed` signals are reported as out-of-band revocations.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use tokio::runtime::Runtime;
use tokio::sync::mpsc;

use ashpd::zbus;
use ashpd::zvariant::{OwnedObjectPath, OwnedValue, Value};

use super::{ActivationResult, Availability, DbusBackend, ReadinessConfig};
use crate::portal::parent::ParentHandle;
use crate::portal::pipewire::PipeWireRemote;
use crate::portal::runtime::{RuntimeInitError, build_runtime};
use crate::portal::session::{
    HandshakeStep, PipeWireStream, Revocation, SessionBackend, SessionError, SessionId, StepReply,
};
use crate::portal::{CancelReason, PortalResponse};

const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const SCREENCAST_IFACE: &str = "org.freedesktop.portal.ScreenCast";
const REQUEST_IFACE: &str = "org.freedesktop.portal.Request";
const SESSION_IFACE: &str = "org.freedesktop.portal.Session";

// Admission reserves create, one drive, and close slots so accepted sessions can always progress and cancel.
const SESSION_COMMAND_QUEUE_CAPACITY: usize = 64;

const SLOTS_PER_SESSION: usize = 3;

type SessionPermit = mpsc::OwnedPermit<SessionCommand>;

type SessionBudget = (SessionPermit, SessionPermit, SessionPermit);

const _: () = assert!(
    SESSION_COMMAND_QUEUE_CAPACITY >= SLOTS_PER_SESSION,
    "the command queue must hold every slot of a session"
);

// Allocate handle tokens process-wide so backend instances cannot share request paths.
static SESSION_TOKEN_COUNTER: AtomicU64 = AtomicU64::new(0);

// Bound each `Session.Close` call so an unresponsive compositor cannot wedge the command loop.
const CLOSE_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

// Bound the synchronous `OpenPipeWireRemote` call because it has no request object that can be closed.
const OPEN_PIPEWIRE_REMOTE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

// Pointer identity authenticates capabilities issued by one backend instance.
#[derive(Debug)]
struct BackendIncarnation;

// Binds a capability to one backend/session incarnation despite reusable numeric session ids.
#[derive(Debug)]
struct RemoteSessionIncarnation {
    backend: Arc<BackendIncarnation>,
}

#[derive(Debug, Default)]
struct RemoteOpenCancelState {
    cancelled: AtomicBool,
    claimed: AtomicBool,
}

/// Cancels one backend-issued PipeWire-remote open attempt.
///
/// Clones share cancellation state. Backend and session identity plus a per-open generation prevent cross-backend or stale handles from authorizing a call.
#[derive(Debug, Clone)]
pub struct RemoteOpenCancel {
    session: SessionId,
    incarnation: Arc<RemoteSessionIncarnation>,
    generation: u64,
    state: Arc<RemoteOpenCancelState>,
}

impl RemoteOpenCancel {
    /// Request cancellation of this exact remote-open attempt. Idempotent.
    pub fn cancel(&self) {
        self.state.cancelled.store(true, Ordering::Release);
    }

    /// Whether cancellation has been requested through this handle or a clone.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire)
    }

    fn try_claim(&self) -> bool {
        self.state
            .claimed
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn shared_cancelled(&self) -> Arc<RemoteOpenCancelState> {
        Arc::clone(&self.state)
    }
}

async fn poll_remote_open_cancelled(cancel: &RemoteOpenCancelState) {
    while !cancel.cancelled.load(Ordering::Acquire) {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

// Raised once connection and activation-aware readiness settle, including failure.
struct Readiness {
    settled: Mutex<bool>,
    cv: Condvar,
}

impl Default for Readiness {
    fn default() -> Self {
        Self {
            settled: Mutex::new(false),
            cv: Condvar::new(),
        }
    }
}

impl Readiness {
    fn signal(&self) {
        let mut settled = self
            .settled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *settled = true;
        self.cv.notify_all();
    }

    fn wait_until_settled(&self, timeout: std::time::Duration) -> bool {
        let started = std::time::Instant::now();
        let deadline = crate::monotonic_ms::checked_instant_deadline(
            started,
            timeout,
            crate::monotonic_ms::Policy::Clamp,
        );
        let mut settled = self
            .settled
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while !*settled {
            let Some(deadline) = deadline else {
                settled = self
                    .cv
                    .wait(settled)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                continue;
            };
            let now = std::time::Instant::now();
            if now >= deadline {
                return false;
            }
            let (guard, result) = self
                .cv
                .wait_timeout(settled, deadline.saturating_duration_since(now))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            settled = guard;
            if result.timed_out() {
                return *settled;
            }
        }
        true
    }
}

enum SessionCommand {
    Create {
        id: SessionId,
        generation: u64,
        portal_owner: String,
        request_path: OwnedObjectPath,
        remote_incarnation: Arc<RemoteSessionIncarnation>,
    },
    Drive {
        id: SessionId,
        generation: u64,
        step: HandshakeStep,
        request_path: OwnedObjectPath,
    },
    Close {
        id: SessionId,
    },
}

struct ActiveSession {
    generation: u64,
    portal_owner: String,
    session_handle: Option<OwnedObjectPath>,
    // Capability identity for this exact backend/session incarnation.
    remote_incarnation: Arc<RemoteSessionIncarnation>,
    // Incrementing the generation invalidates all earlier remote-open handles for this live session.
    remote_open_generation: u64,
    // Set only after Start succeeds; a session path alone is too early for remote opening.
    remote_open_ready: bool,
    // Local close sets this flag so the revocation watcher can retire without a `Closed` signal.
    cancel: Arc<AtomicBool>,
    // Own every overlapping step task so close can abort all futures waiting on portal responses.
    step_tasks: Vec<tokio::task::JoinHandle<()>>,
    revocation_task: Option<tokio::task::JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum TaskKind {
    Step,
    Revocation,
}

// Registers a live per-session task; dropping the guard retires that task’s authority to park replies.
struct TaskSentinel {
    shared: Arc<Mutex<SessionShared>>,
    id: SessionId,
    kind: TaskKind,
}

impl TaskSentinel {
    fn register(shared: &Arc<Mutex<SessionShared>>, id: SessionId, kind: TaskKind) -> Self {
        *lock_session_shared(shared)
            .live_tasks
            .entry((id, kind))
            .or_insert(0) += 1;
        Self {
            shared: Arc::clone(shared),
            id,
            kind,
        }
    }
}

impl Drop for TaskSentinel {
    fn drop(&mut self) {
        let mut s = lock_session_shared(&self.shared);
        if let Some(n) = s.live_tasks.get_mut(&(self.id, self.kind)) {
            *n -= 1;
            if *n == 0 {
                s.live_tasks.remove(&(self.id, self.kind));
            }
        }
        prune_tombstone_if_quiescent(&mut s, self.id);
    }
}

// Remove a closed-session tombstone only after all step and revocation sentinels for that id are gone.
fn prune_tombstone_if_quiescent(s: &mut SessionShared, id: SessionId) {
    if s.live_tasks.contains_key(&(id, TaskKind::Step))
        || s.live_tasks.contains_key(&(id, TaskKind::Revocation))
    {
        return;
    }
    s.closed.remove(&id);
    // The session is fully retired, so the parent handle it would have sent
    // is dropped with it rather than kept for an id that can never recur.
    s.parents.remove(&id);
}

async fn poll_cancelled(cancel: &Arc<AtomicBool>) {
    while !cancel.load(Ordering::SeqCst) {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

#[derive(Default)]
struct SessionShared {
    // The exported parent handle a session's `Start` should carry. Absent
    // means an explicitly unparented handshake.
    parents: HashMap<SessionId, String>,
    step_replies: HashMap<SessionId, StepReply>,
    revocations: HashMap<SessionId, Revocation>,
    active: HashMap<SessionId, ActiveSession>,
    // Reject late results for closed ids; prune each tombstone after its last task sentinel drops.
    closed: HashSet<SessionId>,
    // Refcount live tasks by session and kind because handshake steps may overlap.
    live_tasks: HashMap<(SessionId, TaskKind), usize>,
    // Availability requires both a connection and a live portal generation.
    available: bool,
    unavailable_reason: Option<String>,
    unique_name: Option<String>,
    // `None` means activation-aware readiness is still in flight.
    portal_present: Option<bool>,
    portal_owner: Option<String>,
    portal_generation: u64,
    connection: Option<zbus::Connection>,
}

// On poison, discard partial exchanges and publish typed unavailability instead of unwinding through callbacks.
fn lock_session_shared(shared: &Mutex<SessionShared>) -> std::sync::MutexGuard<'_, SessionShared> {
    match shared.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            let mut state = poisoned.into_inner();
            state.step_replies.clear();
            state.revocations.clear();
            state.active.clear();
            state.closed.clear();
            state.live_tasks.clear();
            state.available = false;
            state.unique_name = None;
            state.portal_present = Some(false);
            state.connection = None;
            state.unavailable_reason = Some("portal session worker state lock was poisoned".into());
            state
        }
    }
}

/// A [`SessionBackend`] that talks to the real XDG Desktop Portal ScreenCast
/// interface over D-Bus.
///
/// Available only with the `portal-dbus` feature.
pub struct DbusSessionBackend {
    next_id: u64,
    token_counter: Arc<AtomicU64>,
    incarnation: Arc<BackendIncarnation>,
    shared: Arc<Mutex<SessionShared>>,
    cmd_tx: mpsc::Sender<SessionCommand>,
    // Reserve each admitted session’s Close slot against later queue saturation.
    close_permits: HashMap<SessionId, SessionPermit>,
    // Reserve the next sequential drive slot so later queue pressure cannot starve an admitted session.
    drive_permits: HashMap<SessionId, SessionPermit>,
    // Retired session ids behave as unknown, and monotonic allocation prevents reuse.
    in_flight: std::collections::HashSet<SessionId>,
    session_generations: HashMap<SessionId, u64>,
    closed_count: usize,
    probed: bool,
    pipewire_remote_timeout: std::time::Duration,
    ready: Arc<Readiness>,
    // Owned so drop can abort without waiting for the command queue to drain.
    worker: tokio::task::JoinHandle<()>,
    _runtime: Runtime,
}

impl DbusSessionBackend {
    /// Creates a D-Bus session backend on the ambient session bus.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeInitError`] if the async runtime cannot be created.
    pub fn new() -> Result<Self, RuntimeInitError> {
        Self::with_address(None)
    }

    /// Creates a D-Bus session backend for an optional explicit bus address.
    ///
    /// `None` selects the ambient session bus.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeInitError`] if the async runtime cannot be created.
    pub fn with_address(address: Option<String>) -> Result<Self, RuntimeInitError> {
        let shared = Arc::new(Mutex::new(SessionShared::default()));
        let (cmd_tx, cmd_rx) = mpsc::channel(SESSION_COMMAND_QUEUE_CAPACITY);
        let ready = Arc::new(Readiness::default());

        let rt = build_runtime()?;
        #[cfg(test)]
        let readiness_config = super::readiness_hook::take();
        #[cfg(not(test))]
        let readiness_config = ReadinessConfig::default();

        let shared_for_loop = Arc::clone(&shared);
        let ready_for_loop = Arc::clone(&ready);
        let worker = rt.spawn(async move {
            Self::command_loop(
                cmd_rx,
                shared_for_loop,
                ready_for_loop,
                address,
                readiness_config,
            )
            .await;
        });

        Ok(Self {
            next_id: 0,
            token_counter: {
                let base = SESSION_TOKEN_COUNTER.fetch_add(1_000_000, Ordering::SeqCst);
                Arc::new(AtomicU64::new(base))
            },
            incarnation: Arc::new(BackendIncarnation),
            shared,
            cmd_tx,
            close_permits: HashMap::new(),
            drive_permits: HashMap::new(),
            in_flight: std::collections::HashSet::new(),
            session_generations: HashMap::new(),
            closed_count: 0,
            probed: false,
            pipewire_remote_timeout: OPEN_PIPEWIRE_REMOTE_TIMEOUT,
            ready,
            worker,
            _runtime: rt,
        })
    }

    /// Waits for the background availability result, up to `timeout`.
    ///
    /// Returns `true` once either availability or unavailability is known, and `false` on timeout.
    #[must_use]
    pub fn wait_until_settled(&self, timeout: std::time::Duration) -> bool {
        self.ready.wait_until_settled(timeout)
    }

    // Compute the portal’s deterministic request path so subscription can precede the method call.
    fn request_path(unique_name: &str, handle_token: &str) -> OwnedObjectPath {
        let sanitized = unique_name.trim_start_matches(':').replace('.', "_");
        let path = format!("/org/freedesktop/portal/desktop/request/{sanitized}/{handle_token}");
        OwnedObjectPath::try_from(path).expect("computed request path is always valid")
    }

    fn probe_availability(&mut self) -> Availability {
        self.probed = true;
        let s = lock_session_shared(&self.shared);
        Availability::classify(
            s.available,
            s.unique_name.as_deref(),
            s.portal_present,
            s.unavailable_reason.as_deref(),
            s.portal_owner.as_deref(),
            s.portal_generation,
        )
    }

    fn next_token(&self) -> String {
        let n = self.token_counter.fetch_add(1, Ordering::SeqCst);
        format!("enzui_sc_{n}")
    }

    fn unique_name(&self) -> Option<String> {
        lock_session_shared(&self.shared).unique_name.clone()
    }

    fn try_reserve_slot(&self) -> Result<SessionPermit, SessionError> {
        use mpsc::error::TrySendError;

        self.cmd_tx
            .clone()
            .try_reserve_owned()
            .map_err(|error| match error {
                TrySendError::Full(_) => SessionError::ResourceExhausted,
                TrySendError::Closed(_) => SessionError::Unavailable,
            })
    }

    // Reserve create, drive, and close permits together so every admitted session can progress and cancel.
    fn reserve_session_budget(&self) -> Result<SessionBudget, SessionError> {
        let create = self.try_reserve_slot()?;
        let drive = self.try_reserve_slot()?;
        let close = self.try_reserve_slot()?;
        Ok((create, drive, close))
    }

    async fn command_loop(
        mut cmd_rx: mpsc::Receiver<SessionCommand>,
        shared: Arc<Mutex<SessionShared>>,
        ready: Arc<Readiness>,
        address: Option<String>,
        readiness_config: ReadinessConfig,
    ) {
        let setup = tokio::time::timeout(readiness_config.total_timeout, async {
            let connection = DbusBackend::connect_pub(address).await?;
            let changes = DbusBackend::owner_change_stream(&connection).await?;
            let activation = DbusBackend::activate_portal(&connection, readiness_config).await;
            Ok::<_, zbus::Error>((connection, changes, activation))
        })
        .await;
        let (connection, mut changes) = match setup {
            Ok(Ok((connection, changes, activation))) => {
                {
                    let mut state = lock_session_shared(&shared);
                    state.available = true;
                    state.unique_name = connection.unique_name().map(ToString::to_string);
                    state.connection = Some(connection.clone());
                }
                match activation {
                    ActivationResult::Owned(owner) => Self::publish_owner(&shared, owner),
                    ActivationResult::Unavailable(reason) => {
                        let mut state = lock_session_shared(&shared);
                        state.portal_present = Some(false);
                        state.unavailable_reason = Some(reason);
                    }
                }
                (connection, changes)
            }
            Ok(Err(error)) => {
                let mut state = lock_session_shared(&shared);
                state.available = false;
                state.portal_present = Some(false);
                state.unavailable_reason = Some(format!("portal readiness failed: {error}"));
                drop(state);
                ready.signal();
                return;
            }
            Err(_) => {
                let mut state = lock_session_shared(&shared);
                state.available = false;
                state.portal_present = Some(false);
                state.unavailable_reason =
                    Some("portal readiness exceeded its total deadline".to_owned());
                drop(state);
                ready.signal();
                return;
            }
        };
        ready.signal();

        use futures_util::StreamExt;
        loop {
            use futures_util::future::{Either, select};
            let change = changes.next();
            let command = cmd_rx.recv();
            futures_util::pin_mut!(change, command);
            match select(change, command).await {
                Either::Left((change, _)) => {
                    let Some(signal) = change else {
                        Self::publish_owner_loss(&shared, None).await;
                        let mut state = lock_session_shared(&shared);
                        state.available = false;
                        state.portal_present = Some(false);
                        state.unavailable_reason =
                            Some("portal owner monitor disconnected".to_owned());
                        break;
                    };
                    let Ok((name, old_owner, new_owner)) =
                        signal.body().deserialize::<(String, String, String)>()
                    else {
                        continue;
                    };
                    if name != PORTAL_DEST {
                        continue;
                    }
                    if !old_owner.is_empty() {
                        let replaced = Self::publish_owner_loss(&shared, Some(&old_owner)).await;
                        if replaced && !new_owner.is_empty() {
                            Self::publish_owner(&shared, new_owner);
                            continue;
                        }
                        if !replaced {
                            continue;
                        }
                    } else if !new_owner.is_empty() {
                        let current = lock_session_shared(&shared).portal_owner.clone();
                        if DbusBackend::owner_change_matches(
                            current.as_deref(),
                            &old_owner,
                            &new_owner,
                        ) {
                            Self::publish_owner(&shared, new_owner);
                        }
                        continue;
                    }
                    if new_owner.is_empty() {
                        match DbusBackend::activate_portal(&connection, readiness_config).await {
                            ActivationResult::Owned(owner) => Self::publish_owner(&shared, owner),
                            ActivationResult::Unavailable(reason) => {
                                let mut state = lock_session_shared(&shared);
                                if state.portal_owner.is_none() {
                                    state.portal_present = Some(false);
                                    state.unavailable_reason = Some(reason);
                                }
                            }
                        }
                    }
                }
                Either::Right((Some(command), _)) => {
                    Self::handle_command(command, &connection, &shared).await;
                }
                Either::Right((None, _)) => break,
            }
        }
    }

    async fn handle_command(
        cmd: SessionCommand,
        connection: &zbus::Connection,
        shared: &Arc<Mutex<SessionShared>>,
    ) {
        match cmd {
            SessionCommand::Create {
                id,
                generation,
                portal_owner,
                request_path,
                remote_incarnation,
            } => {
                {
                    let mut state = lock_session_shared(shared);
                    if state.portal_owner.is_none() || state.portal_generation != generation {
                        state.revocations.insert(id, Revocation::Revoked);
                        return;
                    }
                    state.active.insert(
                        id,
                        ActiveSession {
                            generation,
                            portal_owner,
                            session_handle: None,
                            remote_incarnation,
                            remote_open_generation: 0,
                            remote_open_ready: false,
                            cancel: Arc::new(AtomicBool::new(false)),
                            step_tasks: Vec::new(),
                            revocation_task: None,
                        },
                    );
                }
                Self::spawn_step(
                    connection,
                    shared,
                    id,
                    generation,
                    HandshakeStep::CreateSession,
                    request_path,
                );
            }
            SessionCommand::Drive {
                id,
                generation,
                step,
                request_path,
            } => {
                let generation_is_live = {
                    let state = lock_session_shared(shared);
                    state.portal_owner.is_some() && state.portal_generation == generation
                };
                if !generation_is_live {
                    lock_session_shared(shared)
                        .revocations
                        .insert(id, Revocation::Revoked);
                } else {
                    Self::spawn_step(connection, shared, id, generation, step, request_path);
                }
            }
            SessionCommand::Close { id } => {
                let handle = Self::apply_close_state(shared, id);
                if let Some((portal_owner, path)) = handle {
                    let _ = tokio::time::timeout(
                        CLOSE_CALL_TIMEOUT,
                        Self::close_session_call(connection, &portal_owner, &path),
                    )
                    .await;
                }
            }
        }
    }

    fn publish_owner(shared: &Arc<Mutex<SessionShared>>, owner: String) {
        let mut state = lock_session_shared(shared);
        if state.portal_owner.as_deref() == Some(owner.as_str()) {
            return;
        }
        state.portal_generation = state.portal_generation.saturating_add(1);
        state.portal_owner = Some(owner);
        state.portal_present = Some(true);
        state.unavailable_reason = None;
    }

    async fn publish_owner_loss(
        shared: &Arc<Mutex<SessionShared>>,
        expected_owner: Option<&str>,
    ) -> bool {
        let active = {
            let mut state = lock_session_shared(shared);
            if expected_owner.is_some_and(|owner| state.portal_owner.as_deref() != Some(owner)) {
                return false;
            }
            state.portal_owner = None;
            state.portal_present = None;
            state.unavailable_reason = None;
            let ids = state.active.keys().copied().collect::<Vec<_>>();
            for id in &ids {
                state.closed.insert(*id);
                state.step_replies.remove(id);
            }
            state.active.drain().collect::<Vec<_>>()
        };
        let terminated_ids = active.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        for (_, session) in &active {
            session.cancel.store(true, Ordering::SeqCst);
            for task in &session.step_tasks {
                task.abort();
            }
            if let Some(task) = &session.revocation_task {
                task.abort();
            }
        }
        for (_, session) in active {
            for task in session.step_tasks {
                let _ = task.await;
            }
            if let Some(task) = session.revocation_task {
                let _ = task.await;
            }
        }
        let mut state = lock_session_shared(shared);
        for id in terminated_ids {
            state.revocations.insert(id, Revocation::Revoked);
            prune_tombstone_if_quiescent(&mut state, id);
        }
        true
    }

    fn spawn_step(
        connection: &zbus::Connection,
        shared: &Arc<Mutex<SessionShared>>,
        id: SessionId,
        generation: u64,
        step: HandshakeStep,
        request_path: OwnedObjectPath,
    ) {
        let portal_owner = {
            let state = lock_session_shared(shared);
            let Some(active) = state
                .active
                .get(&id)
                .filter(|active| active.generation == generation)
            else {
                return;
            };
            active.portal_owner.clone()
        };
        let conn = connection.clone();
        let shared_task = Arc::clone(shared);
        // Register task ownership before spawning so close cannot miss it.
        let sentinel = TaskSentinel::register(shared, id, TaskKind::Step);
        let handle = tokio::spawn(async move {
            let _sentinel = sentinel;
            Self::run_step(
                &conn,
                &portal_owner,
                id,
                generation,
                step,
                &request_path,
                &shared_task,
            )
            .await;
        });
        let mut s = lock_session_shared(shared);
        match s.active.get_mut(&id) {
            Some(active) if active.generation == generation => {
                active.step_tasks.retain(|task| !task.is_finished());
                active.step_tasks.push(handle);
            }
            Some(_) | None => handle.abort(),
        }
    }

    // Subscribe to each Response before its method call; install the session `Closed` watcher as soon as CreateSession returns a handle.
    async fn run_step(
        connection: &zbus::Connection,
        portal_owner: &str,
        id: SessionId,
        generation: u64,
        step: HandshakeStep,
        request_path: &OwnedObjectPath,
        shared: &Arc<Mutex<SessionShared>>,
    ) {
        // A later step waits for CreateSession’s handle because step tasks may overlap.
        let session_handle = if step == HandshakeStep::CreateSession {
            None
        } else {
            match Self::await_session_handle(shared, id).await {
                Some(h) => Some(h),
                None => {
                    Self::park_step(shared, id, generation, StepReply::Failed);
                    return;
                }
            }
        };

        let handle_token = request_path
            .as_str()
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_owned();

        // The parent is read once, here, so the value the wire carries is the
        // one this session was given rather than whatever is current when the
        // call is finally made.
        let parent_window = lock_session_shared(shared)
            .parents
            .get(&id)
            .cloned()
            .unwrap_or_default();

        // `subscribe_then_call` is the sole owner of subscribe-before-call ordering.
        let seam = ZbusStepChannel {
            connection,
            portal_owner,
            request_path,
            step,
            handle_token: &handle_token,
            session_handle: session_handle.as_ref(),
            parent_window: &parent_window,
        };
        let Ok(mut responses) = subscribe_then_call(&seam).await else {
            return Self::park_step(shared, id, generation, StepReply::Failed);
        };

        use futures_util::StreamExt;
        let Some(signal) = responses.next().await else {
            return Self::park_step(shared, id, generation, StepReply::Failed);
        };
        let body = signal.body();
        let (code, results): (u32, HashMap<String, OwnedValue>) = match body.deserialize() {
            Ok(v) => v,
            Err(_) => return Self::park_step(shared, id, generation, StepReply::Failed),
        };

        match PortalResponse::from_code(code) {
            PortalResponse::Success => Self::handle_success(
                connection,
                portal_owner,
                id,
                generation,
                step,
                &results,
                shared,
            ),
            PortalResponse::Cancelled => {
                Self::park_step(shared, id, generation, StepReply::UserCancelled);
            }
            _ => Self::park_step(shared, id, generation, StepReply::Failed),
        }
    }

    // Wait briefly for CreateSession’s handle, but stop if the session closes or the deadline expires.
    async fn await_session_handle(
        shared: &Arc<Mutex<SessionShared>>,
        id: SessionId,
    ) -> Option<OwnedObjectPath> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            {
                let s = lock_session_shared(shared);
                match s.active.get(&id) {
                    Some(a) => {
                        if let Some(h) = &a.session_handle {
                            return Some(h.clone());
                        }
                    }
                    None => return None,
                }
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    fn handle_success(
        connection: &zbus::Connection,
        portal_owner: &str,
        id: SessionId,
        generation: u64,
        step: HandshakeStep,
        results: &HashMap<String, OwnedValue>,
        shared: &Arc<Mutex<SessionShared>>,
    ) {
        match step {
            HandshakeStep::CreateSession => {
                let Some(handle) = results
                    .get("session_handle")
                    .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
                    .and_then(|s| OwnedObjectPath::try_from(s).ok())
                else {
                    return Self::park_step(shared, id, generation, StepReply::Failed);
                };
                {
                    let mut s = lock_session_shared(shared);
                    if let Some(a) = s
                        .active
                        .get_mut(&id)
                        .filter(|active| active.generation == generation)
                    {
                        a.session_handle = Some(handle.clone());
                    } else {
                        return;
                    }
                }
                Self::spawn_revocation_watch(
                    connection,
                    portal_owner,
                    id,
                    generation,
                    handle,
                    Arc::clone(shared),
                );
                // Do not park CreateSession: its handle is shared state, and its reply slot belongs to the next driven step.
            }
            HandshakeStep::SelectSources => {
                Self::park_step(shared, id, generation, StepReply::Ongoing);
            }
            HandshakeStep::Start => {
                let streams = parse_streams(results);
                {
                    let mut state = lock_session_shared(shared);
                    let Some(active) = state
                        .active
                        .get_mut(&id)
                        .filter(|active| active.generation == generation)
                    else {
                        return;
                    };
                    active.remote_open_ready = true;
                }
                Self::park_step(shared, id, generation, StepReply::Streams(streams));
            }
        }
    }

    fn spawn_revocation_watch(
        connection: &zbus::Connection,
        portal_owner: &str,
        id: SessionId,
        generation: u64,
        session_handle: OwnedObjectPath,
        shared: Arc<Mutex<SessionShared>>,
    ) {
        // Give each watcher a local cancel flag so close can retire it without a compositor signal.
        let cancel = {
            let s = lock_session_shared(&shared);
            match s
                .active
                .get(&id)
                .filter(|active| active.generation == generation)
            {
                Some(a) => Arc::clone(&a.cancel),
                None => return,
            }
        };
        let conn = connection.clone();
        let portal_owner = portal_owner.to_owned();
        let sentinel = TaskSentinel::register(&shared, id, TaskKind::Revocation);
        let shared_task = Arc::clone(&shared);
        let cancel_task = Arc::clone(&cancel);
        let handle = tokio::spawn(async move {
            let _sentinel = sentinel;
            let Ok(proxy) =
                zbus::Proxy::new(&conn, portal_owner, session_handle.clone(), SESSION_IFACE).await
            else {
                return;
            };
            let Ok(mut closed) = proxy.receive_signal("Closed").await else {
                return;
            };
            use futures_util::StreamExt;
            use futures_util::future::{Either, select};
            // Race `Closed` against local cancellation so a silent compositor cannot retain the watcher.
            let next = closed.next();
            let cancelled = poll_cancelled(&cancel_task);
            futures_util::pin_mut!(next, cancelled);
            if let Either::Left((signal, _)) = select(next, cancelled).await
                && signal.is_some()
            {
                Self::retire_server_revocation(&shared_task, id, generation);
            }
        });
        if let Some(a) = lock_session_shared(&shared)
            .active
            .get_mut(&id)
            .filter(|active| active.generation == generation)
        {
            if let Some(prev) = a.revocation_task.replace(handle) {
                prev.abort();
            }
        } else {
            cancel.store(true, Ordering::SeqCst);
            handle.abort();
        }
    }

    fn retire_server_revocation(
        shared: &Arc<Mutex<SessionShared>>,
        id: SessionId,
        generation: u64,
    ) {
        let mut state = lock_session_shared(shared);
        if state
            .active
            .get(&id)
            .is_none_or(|active| active.generation != generation)
        {
            return;
        }
        state.closed.insert(id);
        state.step_replies.remove(&id);
        let Some(active) = state.active.remove(&id) else {
            return;
        };
        active.cancel.store(true, Ordering::SeqCst);
        for task in active.step_tasks {
            task.abort();
        }
        drop(active.revocation_task);
        state.revocations.insert(id, Revocation::Revoked);
        prune_tombstone_if_quiescent(&mut state, id);
    }

    async fn call_step(
        connection: &zbus::Connection,
        portal_owner: &str,
        step: HandshakeStep,
        handle_token: &str,
        session_handle: Option<&OwnedObjectPath>,
        parent_window: &str,
    ) -> Result<(), zbus::Error> {
        match step {
            HandshakeStep::CreateSession => {
                let mut options: HashMap<&str, Value<'_>> = HashMap::new();
                options.insert("handle_token", Value::from(handle_token));
                let session_token = format!("{handle_token}_s");
                options.insert("session_handle_token", Value::from(session_token.as_str()));
                connection
                    .call_method(
                        Some(portal_owner),
                        PORTAL_PATH,
                        Some(SCREENCAST_IFACE),
                        "CreateSession",
                        &(options,),
                    )
                    .await
                    .map(|_| ())
            }
            HandshakeStep::SelectSources => {
                let session = session_handle.expect("SelectSources requires a session handle");
                let mut options: HashMap<&str, Value<'_>> = HashMap::new();
                options.insert("handle_token", Value::from(handle_token));
                // ScreenCast source type bit 1 requests monitor sources.
                options.insert("types", Value::from(1u32));
                connection
                    .call_method(
                        Some(portal_owner),
                        PORTAL_PATH,
                        Some(SCREENCAST_IFACE),
                        "SelectSources",
                        &(session.as_ref(), options),
                    )
                    .await
                    .map(|_| ())
            }
            HandshakeStep::Start => {
                let session = session_handle.expect("Start requires a session handle");
                let mut options: HashMap<&str, Value<'_>> = HashMap::new();
                options.insert("handle_token", Value::from(handle_token));
                // The exported parent for this session, or the empty string,
                // which the portal reads as explicitly unparented.
                connection
                    .call_method(
                        Some(portal_owner),
                        PORTAL_PATH,
                        Some(SCREENCAST_IFACE),
                        "Start",
                        &(session.as_ref(), parent_window, options),
                    )
                    .await
                    .map(|_| ())
            }
        }
    }

    async fn close_session_call(
        connection: &zbus::Connection,
        portal_owner: &str,
        session_handle: &OwnedObjectPath,
    ) -> Result<(), zbus::Error> {
        connection
            .call_method(
                Some(portal_owner),
                session_handle.as_str(),
                Some(SESSION_IFACE),
                "Close",
                &(),
            )
            .await
            .map(|_| ())
    }

    /// Issues a cancellation handle for the next PipeWire-remote open on this live session.
    ///
    /// Issuing another handle invalidates earlier generations, and handles are bound to their creating backend and session.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::OutOfOrder`] unless the session is ready, or [`SessionError::ResourceExhausted`] if generations are exhausted.
    pub fn remote_open_cancel(&self, session: SessionId) -> Result<RemoteOpenCancel, SessionError> {
        if !self.in_flight.contains(&session) {
            return Err(SessionError::OutOfOrder);
        }
        let mut shared = lock_session_shared(&self.shared);
        let Some(active) = shared.active.get_mut(&session) else {
            return Err(SessionError::OutOfOrder);
        };
        if active.session_handle.is_none()
            || !active.remote_open_ready
            || !Arc::ptr_eq(&active.remote_incarnation.backend, &self.incarnation)
        {
            return Err(SessionError::OutOfOrder);
        }
        active.remote_open_generation = active
            .remote_open_generation
            .checked_add(1)
            .ok_or(SessionError::ResourceExhausted)?;
        Ok(RemoteOpenCancel {
            session,
            incarnation: Arc::clone(&active.remote_incarnation),
            generation: active.remote_open_generation,
            state: Arc::new(RemoteOpenCancelState::default()),
        })
    }

    /// Opens and owns the PipeWire remote for a completed session.
    ///
    /// The call validates that `cancel` belongs to this backend, session, and current open generation, then races the wire call against cancellation and a bounded timeout. The returned [`PipeWireRemote`] closes its fd on drop.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::OutOfOrder`] for an invalid, stale, reused, or not-ready capability; [`SessionError::Unavailable`] without a connection; [`SessionError::TimedOut`] on deadline; [`SessionError::Cancelled`] on cancellation; or [`SessionError::Failed`] for wire or fd-adoption failures.
    pub fn open_pipewire_remote(
        &self,
        session: SessionId,
        cancel: &RemoteOpenCancel,
    ) -> Result<PipeWireRemote, SessionError> {
        if !self.in_flight.contains(&session) {
            return Err(SessionError::OutOfOrder);
        }
        // Authenticate backend, session, and generation under the state lock before reading cancellation.
        let (handle, portal_owner, owner_generation) = {
            let s = lock_session_shared(&self.shared);
            let Some(active) = s.active.get(&session) else {
                return Err(SessionError::OutOfOrder);
            };
            if cancel.session != session
                || !Arc::ptr_eq(&cancel.incarnation.backend, &self.incarnation)
                || !Arc::ptr_eq(&cancel.incarnation, &active.remote_incarnation)
                || cancel.generation != active.remote_open_generation
                || active.generation != s.portal_generation
                || s.portal_owner.is_none()
                || !cancel.try_claim()
            {
                return Err(SessionError::OutOfOrder);
            }
            let Some(handle) = active.session_handle.clone() else {
                return Err(SessionError::OutOfOrder);
            };
            (handle, active.portal_owner.clone(), active.generation)
        };
        let Some(connection) = self.connection() else {
            return Err(SessionError::Unavailable);
        };
        // Pre-cancellation applies to the first valid claim; the claim bit rejects later reuse.
        if cancel.is_cancelled() {
            return Err(SessionError::Cancelled);
        }
        let cancel_state = cancel.shared_cancelled();
        let timeout = self.pipewire_remote_timeout;
        let result = self._runtime.block_on(async move {
            use futures_util::future::{Either, select};
            let call_fut = Self::open_pipewire_remote_call(&connection, &portal_owner, &handle);
            let cancelled_fut = poll_remote_open_cancelled(&cancel_state);
            let timed_out_fut = tokio::time::sleep(timeout);
            futures_util::pin_mut!(call_fut, cancelled_fut, timed_out_fut);
            // Race call, cancellation, and timeout in place so losing futures are dropped without detached tasks.
            let cancel_or_timeout = select(cancelled_fut, timed_out_fut);
            match select(call_fut, cancel_or_timeout).await {
                Either::Left((result, _cot)) => result,
                Either::Right((Either::Left(((), _timed_out)), _call)) => {
                    Err(SessionError::Cancelled)
                }
                Either::Right((Either::Right(((), _cancelled)), _call)) => {
                    Err(SessionError::TimedOut)
                }
            }
        });
        if result.is_ok() {
            let state = lock_session_shared(&self.shared);
            if state.portal_generation != owner_generation
                || state.portal_owner.is_none()
                || state
                    .active
                    .get(&session)
                    .is_none_or(|active| active.generation != owner_generation)
            {
                return Err(SessionError::Unavailable);
            }
        }
        result
    }

    async fn open_pipewire_remote_call(
        connection: &zbus::Connection,
        portal_owner: &str,
        session_handle: &OwnedObjectPath,
    ) -> Result<PipeWireRemote, SessionError> {
        let mut options: HashMap<&str, Value<'_>> = HashMap::new();
        let reply = connection
            .call_method(
                Some(portal_owner),
                PORTAL_PATH,
                Some(SCREENCAST_IFACE),
                "OpenPipeWireRemote",
                &(session_handle.as_ref(), &mut options),
            )
            .await
            .map_err(|_| SessionError::Failed)?;
        let fd: ashpd::zvariant::OwnedFd = reply
            .body()
            .deserialize()
            .map_err(|_| SessionError::Failed)?;
        let owned: std::os::fd::OwnedFd = fd.into();
        // Verify `FD_CLOEXEC` in place and fail closed if normalization fails.
        PipeWireRemote::from_owned_fd(owned).map_err(|_| SessionError::Failed)
    }

    fn connection(&self) -> Option<zbus::Connection> {
        lock_session_shared(&self.shared).connection.clone()
    }

    // Retire local session state, abort owned tasks, and retain the late-result tombstone until all task sentinels drop.
    fn apply_close_state(
        shared: &Arc<Mutex<SessionShared>>,
        id: SessionId,
    ) -> Option<(String, OwnedObjectPath)> {
        let mut s = lock_session_shared(shared);
        // Mark the id closed before discarding parked results so late tasks are refused.
        s.closed.insert(id);
        s.step_replies.remove(&id);
        s.revocations.remove(&id);
        let handle = match s.active.remove(&id) {
            Some(a) => {
                // Abort every owned task; dropping its join handle would only detach it.
                a.cancel.store(true, Ordering::SeqCst);
                for task in a.step_tasks {
                    task.abort();
                }
                if let Some(task) = a.revocation_task {
                    task.abort();
                }
                a.session_handle.map(|path| (a.portal_owner, path))
            }
            None => None,
        };
        // Prune now only if no task sentinel remains; otherwise the final guard drop prunes.
        prune_tombstone_if_quiescent(&mut s, id);
        handle
    }

    fn park_step(
        shared: &Arc<Mutex<SessionShared>>,
        id: SessionId,
        generation: u64,
        reply: StepReply,
    ) {
        let mut s = lock_session_shared(shared);
        if s.closed.contains(&id)
            || s.active
                .get(&id)
                .is_none_or(|active| active.generation != generation)
        {
            return;
        }
        s.step_replies.entry(id).or_insert(reply);
    }
}

#[derive(Debug, PartialEq, Eq)]
enum StepChannelError {
    Subscribe,
    Call,
}

trait StepChannel {
    type Subscription;

    // Must complete before `call` so a fast Response cannot be lost.
    async fn subscribe(&self) -> Result<Self::Subscription, ()>;

    async fn call(&self) -> Result<(), ()>;
}

async fn subscribe_then_call<C: StepChannel>(
    channel: &C,
) -> Result<C::Subscription, StepChannelError> {
    let subscription = channel
        .subscribe()
        .await
        .map_err(|()| StepChannelError::Subscribe)?;

    channel.call().await.map_err(|()| StepChannelError::Call)?;

    Ok(subscription)
}

struct ZbusStepChannel<'a> {
    connection: &'a zbus::Connection,
    portal_owner: &'a str,
    request_path: &'a OwnedObjectPath,
    step: HandshakeStep,
    handle_token: &'a str,
    session_handle: Option<&'a OwnedObjectPath>,
    parent_window: &'a str,
}

impl StepChannel for ZbusStepChannel<'_> {
    type Subscription = zbus::proxy::SignalStream<'static>;

    async fn subscribe(&self) -> Result<Self::Subscription, ()> {
        let request_proxy = zbus::Proxy::new(
            self.connection,
            self.portal_owner.to_owned(),
            self.request_path.clone(),
            REQUEST_IFACE,
        )
        .await
        .map_err(|_| ())?;
        // The owned object path gives the returned signal stream a static lifetime after the proxy drops.
        request_proxy
            .receive_signal("Response")
            .await
            .map_err(|_| ())
    }

    async fn call(&self) -> Result<(), ()> {
        DbusSessionBackend::call_step(
            self.connection,
            self.portal_owner,
            self.step,
            self.handle_token,
            self.session_handle,
            self.parent_window,
        )
        .await
        .map_err(|_| ())
    }
}

// Decodes ScreenCast `a(ua{sv})` streams with optional `(ii)` position and size properties.
fn parse_streams(results: &HashMap<String, OwnedValue>) -> Vec<PipeWireStream> {
    let Some(streams_val) = results.get("streams") else {
        return vec![];
    };
    let Ok(cloned) = streams_val.try_clone() else {
        return vec![];
    };
    let Ok(raw): Result<Vec<(u32, HashMap<String, OwnedValue>)>, _> = cloned.try_into() else {
        return vec![];
    };
    raw.into_iter()
        .map(|(node_id, props)| PipeWireStream {
            node_id,
            position: props
                .get("position")
                .and_then(|v| <(i32, i32)>::try_from(v.try_clone().ok()?).ok()),
            // ScreenCast encodes size as signed `(ii)`; valid nonnegative values become unsigned dimensions.
            size: props.get("size").and_then(|v| {
                let (w, h) = <(i32, i32)>::try_from(v.try_clone().ok()?).ok()?;
                Some((w.max(0) as u32, h.max(0) as u32))
            }),
        })
        .collect()
}

impl Drop for DbusSessionBackend {
    fn drop(&mut self) {
        // Abort the worker so shutdown does not depend on draining the command queue.
        self.worker.abort();
        // Cancel watchers and abort all per-session tasks before releasing shared state.
        {
            let mut s = lock_session_shared(&self.shared);
            #[allow(
                clippy::iter_over_hash_type,
                reason = "every entry gets the same treatment, so the order does not matter"
            )]
            for (_id, active) in s.active.drain() {
                active.cancel.store(true, Ordering::SeqCst);
                for task in active.step_tasks {
                    task.abort();
                }
                if let Some(task) = active.revocation_task {
                    task.abort();
                }
            }
        }
    }
}

impl SessionBackend for DbusSessionBackend {
    fn create_session(&mut self) -> Result<SessionId, SessionError> {
        let Availability::Available {
            unique_name,
            portal_owner,
            generation,
        } = self.probe_availability()
        else {
            return Err(SessionError::Unavailable);
        };

        let (create_permit, drive_permit, close_permit) = self.reserve_session_budget()?;

        self.next_id += 1;
        let id = SessionId::new(self.next_id);
        let token = self.next_token();
        let request_path = Self::request_path(&unique_name, &token);
        let remote_incarnation = Arc::new(RemoteSessionIncarnation {
            backend: Arc::clone(&self.incarnation),
        });

        self.in_flight.insert(id);
        self.session_generations.insert(id, generation);
        self.close_permits.insert(id, close_permit);
        self.drive_permits.insert(id, drive_permit);
        create_permit.send(SessionCommand::Create {
            id,
            generation,
            portal_owner,
            request_path,
            remote_incarnation,
        });
        Ok(id)
    }

    fn drive_step(&mut self, session: SessionId, step: HandshakeStep) -> Result<(), SessionError> {
        // Treat unknown or retired session ids as `OutOfOrder`; monotonic ids prevent stale reuse.
        if !self.in_flight.contains(&session) {
            return Err(SessionError::OutOfOrder);
        }
        let Some(unique) = self.unique_name() else {
            return Err(SessionError::Unavailable);
        };
        let Some(&generation) = self.session_generations.get(&session) else {
            return Err(SessionError::OutOfOrder);
        };
        let token = self.next_token();
        let request_path = Self::request_path(&unique, &token);
        let command = SessionCommand::Drive {
            id: session,
            generation,
            step,
            request_path,
        };
        // Spend the reserved drive slot, then replace it; saturation remains a retryable refusal.
        if let Some(permit) = self.drive_permits.remove(&session) {
            permit.send(command);
            if let Ok(next) = self.try_reserve_slot() {
                self.drive_permits.insert(session, next);
            }
            return Ok(());
        }
        self.cmd_tx.try_send(command).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => SessionError::ResourceExhausted,
            mpsc::error::TrySendError::Closed(_) => SessionError::Unavailable,
        })
    }

    fn poll_step(&mut self, session: SessionId) -> Option<StepReply> {
        if !self.in_flight.contains(&session) {
            return None;
        }
        // Do not retire on step replies; only session close ends the session lifecycle.
        lock_session_shared(&self.shared)
            .step_replies
            .remove(&session)
    }

    fn set_parent(&mut self, session: SessionId, parent: ParentHandle) {
        // Recorded only for a session this backend still owns, so a handle
        // offered for a retired id cannot be sent on a later session that
        // happens to be driven next.
        if !self.in_flight.contains(&session) {
            return;
        }
        let _ = lock_session_shared(&self.shared)
            .parents
            .insert(session, parent.as_str().to_owned());
    }

    fn poll_revocation(&mut self, session: SessionId) -> Option<Revocation> {
        if !self.in_flight.contains(&session) {
            return None;
        }
        let revocation = lock_session_shared(&self.shared)
            .revocations
            .remove(&session);
        if revocation.is_some() {
            self.in_flight.remove(&session);
            self.session_generations.remove(&session);
            self.close_permits.remove(&session);
            self.drive_permits.remove(&session);
            self.closed_count = self.closed_count.saturating_add(1);
        }
        revocation
    }

    fn close_session(&mut self, session: SessionId, reason: CancelReason) {
        if !self.in_flight.contains(&session) {
            let _ = reason;
            return;
        }
        let _ = reason;
        // Send Close with its reserved permit so queue saturation cannot lose it.
        if self.cmd_tx.is_closed() {
            return;
        }
        let Some(permit) = self.close_permits.remove(&session) else {
            return;
        };
        permit.send(SessionCommand::Close { id: session });
        self.drive_permits.remove(&session);
        self.session_generations.remove(&session);
        self.in_flight.remove(&session);
        self.closed_count = self.closed_count.saturating_add(1);
    }

    fn describe(&self) -> String {
        let s = lock_session_shared(&self.shared);
        let availability = Availability::classify(
            s.available,
            s.unique_name.as_deref(),
            s.portal_present,
            s.unavailable_reason.as_deref(),
            s.portal_owner.as_deref(),
            s.portal_generation,
        );
        let status = if !self.probed {
            "not yet probed".to_owned()
        } else {
            match availability {
                Availability::Initialising => "initialising".to_owned(),
                Availability::Available { .. } => "real portal".to_owned(),
                Availability::Unavailable if s.portal_present == Some(false) && s.available => {
                    "UNAVAILABLE: no activatable portal implementation on the bus".to_owned()
                }
                Availability::Unavailable => format!(
                    "UNAVAILABLE: {}",
                    s.unavailable_reason.as_deref().unwrap_or("unknown")
                ),
            }
        };
        format!(
            "D-Bus ScreenCast (zbus, {status}) — {} in flight, {} closed",
            self.in_flight.len(),
            self.closed_count
        )
    }
}

impl std::fmt::Debug for DbusSessionBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DbusSessionBackend")
            .field("next_id", &self.next_id)
            .field("in_flight", &self.in_flight.len())
            .field("closed_count", &self.closed_count)
            .finish()
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod subscribe_then_call_tests {
    use std::sync::{Arc, Mutex};

    use super::{StepChannel, subscribe_then_call};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Action {
        Subscribe,
        Call,
    }

    struct RecordingChannel {
        log: Arc<Mutex<Vec<Action>>>,
        fail_subscribe: bool,
    }

    impl StepChannel for RecordingChannel {
        type Subscription = ();

        async fn subscribe(&self) -> Result<Self::Subscription, ()> {
            self.log.lock().unwrap().push(Action::Subscribe);
            if self.fail_subscribe {
                return Err(());
            }
            Ok(())
        }

        async fn call(&self) -> Result<(), ()> {
            self.log.lock().unwrap().push(Action::Call);
            Ok(())
        }
    }

    fn block<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("current-thread runtime")
            .block_on(fut)
    }

    #[test]
    fn subscribe_precedes_call() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let channel = RecordingChannel {
            log: Arc::clone(&log),
            fail_subscribe: false,
        };

        let result = block(subscribe_then_call(&channel));
        assert!(result.is_ok(), "the correct order must succeed");

        let trace = log.lock().unwrap().clone();
        assert_eq!(
            trace,
            vec![Action::Subscribe, Action::Call],
            "the subscription MUST be established before the method call is \
             issued — a fast portal reply racing into the gap between a late \
             subscription and the call is lost forever"
        );
        let sub = trace.iter().position(|a| *a == Action::Subscribe);
        let call = trace.iter().position(|a| *a == Action::Call);
        assert!(
            matches!((sub, call), (Some(s), Some(c)) if s < c),
            "both actions must occur and subscribe must precede call, got {trace:?}"
        );
    }
}

#[cfg(test)]
mod mock_screencast;
