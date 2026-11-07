//! Single-worker ownership for bounded Secret Service mutations.
//!
//! [`CredentialMutationOwner::new`] returns the owner together with a cloneable
//! [`CredentialMutationStore`]. Applications must retain the owner for at least
//! as long as every store clone. Dropping the owner closes admission and moves
//! its exact worker handle to the process-wide credential reaper.

use std::collections::HashSet;
use std::fmt;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use futures_util::future::{Either, select};
use tokio::sync::{mpsc, oneshot};

use super::{
    CredentialKey, CredentialOperation, CredentialOutcome, CredentialPresence, CredentialStore,
    PendingCredentialOp, Secret, SecretTransport, TransportCancel, TransportFuture,
    map_transport_outcome,
};

/// Maximum admitted mutations, including the active wire call.
pub const CREDENTIAL_MUTATION_CAPACITY: usize = 16;

/// Maximum exact keys that may await provider reconciliation.
pub const CREDENTIAL_UNCERTAINTY_CAPACITY: usize = 16;

/// Maximum mutation-worker handles for which process reaper capacity can be reserved.
pub const CREDENTIAL_REAPER_CAPACITY: usize = 16;

const MUTATION_DEADLINE: Duration = Duration::from_secs(30);
const GENERATION_EXHAUSTED: &str = "credential mutation generation exhausted";
const PRE_WIRE_DEADLINE: &str = "credential mutation deadline expired before provider entry";

#[derive(Debug)]
struct OwnerState {
    accepting: bool,
    admitted: usize,
    wire_active: bool,
    drained: bool,
    worker_panicked: bool,
    last_generation: u64,
    uncertain: HashSet<CredentialKey>,
}

impl Default for OwnerState {
    fn default() -> Self {
        Self {
            accepting: true,
            admitted: 0,
            wire_active: false,
            drained: false,
            worker_panicked: false,
            last_generation: 0,
            uncertain: HashSet::with_capacity(CREDENTIAL_UNCERTAINTY_CAPACITY),
        }
    }
}

struct SharedOwner {
    state: Mutex<OwnerState>,
    shutdown: AtomicBool,
}

impl SharedOwner {
    fn lock(&self) -> std::sync::MutexGuard<'_, OwnerState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// A nonblocking view of mutation-owner lifecycle and admission state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CredentialMutationSnapshot {
    accepting: bool,
    admitted: usize,
    wire_active: bool,
    drained: bool,
    worker_panicked: bool,
    last_generation: u64,
    uncertain_keys: usize,
}

impl CredentialMutationSnapshot {
    /// Whether the owner accepts another mutation.
    pub fn is_accepting(self) -> bool {
        self.accepting
    }

    /// Number of admitted operations, including a wire-active operation.
    pub fn admitted(self) -> usize {
        self.admitted
    }

    /// Whether one provider mutation is currently active.
    pub fn is_wire_active(self) -> bool {
        self.wire_active
    }

    /// Whether the worker has settled all accepted operations and exited.
    pub fn is_drained(self) -> bool {
        self.drained
    }

    /// Whether the worker converted a caught panic into shutdown settlement.
    pub fn worker_panicked(self) -> bool {
        self.worker_panicked
    }

    /// Last admitted nonzero generation, or zero before the first admission.
    pub fn last_generation(self) -> u64 {
        self.last_generation
    }

    /// Number of exact keys requiring explicit reconciliation.
    pub fn uncertain_keys(self) -> usize {
        self.uncertain_keys
    }
}

struct AdmissionPermit {
    shared: Arc<SharedOwner>,
}

impl Drop for AdmissionPermit {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        state.admitted = state.admitted.saturating_sub(1);
    }
}

enum MutationKind {
    Set(Secret),
    Delete,
    Reconcile,
}

impl MutationKind {
    fn operation(&self) -> CredentialOperation {
        match self {
            Self::Set(_) => CredentialOperation::Set,
            Self::Delete => CredentialOperation::Delete,
            Self::Reconcile => CredentialOperation::Get,
        }
    }

    fn is_reconcile(&self) -> bool {
        matches!(self, Self::Reconcile)
    }
}

struct MutationRequest {
    key: CredentialKey,
    admitted_at: Instant,
    cancel: TransportCancel,
    kind: MutationKind,
    response: oneshot::Sender<CredentialOutcome>,
    _permit: AdmissionPermit,
}

enum WorkerMessage {
    Mutation(MutationRequest),
    Shutdown,
}

struct MutationReply {
    ready: Option<CredentialOutcome>,
    receiver: Option<oneshot::Receiver<CredentialOutcome>>,
}

impl MutationReply {
    fn ready(outcome: CredentialOutcome) -> Self {
        Self {
            ready: Some(outcome),
            receiver: None,
        }
    }

    fn waiting(receiver: oneshot::Receiver<CredentialOutcome>) -> Self {
        Self {
            ready: None,
            receiver: Some(receiver),
        }
    }
}

impl Future for MutationReply {
    type Output = CredentialOutcome;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if let Some(outcome) = self.ready.take() {
            return Poll::Ready(outcome);
        }
        let receiver = self
            .receiver
            .as_mut()
            .expect("mutation reply polled after completion");
        match Pin::new(receiver).poll(cx) {
            Poll::Ready(Ok(outcome)) => {
                self.receiver = None;
                Poll::Ready(outcome)
            }
            Poll::Ready(Err(_)) => {
                self.receiver = None;
                Poll::Ready(CredentialOutcome::OwnerShutdown)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

/// Cloneable credential-store handle whose mutations are owned by one retained owner.
#[derive(Clone)]
pub struct CredentialMutationStore {
    transport: Arc<dyn SecretTransport>,
    dispatcher: MutationDispatcher,
}

impl fmt::Debug for CredentialMutationStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialMutationStore")
            .field("transport", &"<dyn SecretTransport>")
            .finish_non_exhaustive()
    }
}

impl CredentialMutationStore {
    /// Reconciles one uncertain key by reading its current provider state.
    ///
    /// A definitive retrieved or not-found result clears the key's uncertainty.
    /// Other outcomes leave the key uncertain and later mutations cannot report
    /// settled success until a reconciliation confirms provider state.
    pub fn reconcile(&self, key: CredentialKey) -> PendingCredentialOp {
        self.dispatcher.dispatch(key, MutationKind::Reconcile)
    }
}

impl CredentialStore for CredentialMutationStore {
    fn get(&self, key: CredentialKey) -> PendingCredentialOp {
        let transport = Arc::clone(&self.transport);
        let call_key = key.clone();
        PendingCredentialOp::with_transport_cancel(
            CredentialOperation::Get,
            key,
            move |cancel| async move {
                map_transport_outcome(transport.get_cancellable(&call_key, cancel).await)
            },
        )
    }

    fn set(&self, key: CredentialKey, secret: Secret) -> PendingCredentialOp {
        self.dispatcher.dispatch(key, MutationKind::Set(secret))
    }

    fn delete(&self, key: CredentialKey) -> PendingCredentialOp {
        self.dispatcher.dispatch(key, MutationKind::Delete)
    }

    fn contains(&self, key: &CredentialKey) -> CredentialPresence {
        self.transport.contains(key)
    }
}

#[derive(Clone)]
struct MutationDispatcher {
    sender: mpsc::Sender<WorkerMessage>,
    shared: Arc<SharedOwner>,
}

impl MutationDispatcher {
    fn dispatch(&self, key: CredentialKey, kind: MutationKind) -> PendingCredentialOp {
        let operation = kind.operation();
        let pending_key = key.clone();
        let dispatcher = self.clone();
        PendingCredentialOp::with_transport_cancel(operation, pending_key, move |cancel| {
            dispatcher.admit(key, kind, cancel)
        })
    }

    fn admit(
        &self,
        key: CredentialKey,
        kind: MutationKind,
        cancel: TransportCancel,
    ) -> MutationReply {
        let (response, receiver) = oneshot::channel();
        let mut state = self.shared.lock();
        if !state.accepting || self.shared.shutdown.load(Ordering::SeqCst) {
            drop(state);
            drop(kind);
            return MutationReply::ready(CredentialOutcome::OwnerShutdown);
        }
        if state.admitted >= CREDENTIAL_MUTATION_CAPACITY
            || (state.uncertain.len() >= CREDENTIAL_UNCERTAINTY_CAPACITY
                && !state.uncertain.contains(&key))
        {
            drop(state);
            drop(kind);
            return MutationReply::ready(CredentialOutcome::Saturated);
        }
        let Some(generation) = state.last_generation.checked_add(1) else {
            drop(state);
            drop(kind);
            return MutationReply::ready(CredentialOutcome::Failed(
                GENERATION_EXHAUSTED.to_owned(),
            ));
        };

        state.last_generation = generation;
        state.admitted += 1;
        let request = MutationRequest {
            key,
            admitted_at: Instant::now(),
            cancel,
            kind,
            response,
            _permit: AdmissionPermit {
                shared: Arc::clone(&self.shared),
            },
        };
        if let Err(error) = self.sender.try_send(WorkerMessage::Mutation(request)) {
            state.accepting = false;
            drop(state);
            if let WorkerMessage::Mutation(request) = error.into_inner() {
                let _ = request.response.send(CredentialOutcome::OwnerShutdown);
            }
            return MutationReply::waiting(receiver);
        }
        drop(state);
        MutationReply::waiting(receiver)
    }
}

struct OwnedMutationWorker {
    thread: JoinHandle<()>,
    reaper_reservation: ReaperReservation,
}

/// Owns the sole Secret Service mutation runtime and worker.
///
/// The returned store may be cloned, but this owner must be retained. Admission
/// is globally capped at 16 operations: one wire-active and fifteen queued.
/// Mutations are globally serial and therefore FIFO for each exact key.
pub struct CredentialMutationOwner {
    sender: mpsc::Sender<WorkerMessage>,
    shared: Arc<SharedOwner>,
    worker: Option<OwnedMutationWorker>,
}

impl fmt::Debug for CredentialMutationOwner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialMutationOwner")
            .field("snapshot", &self.snapshot())
            .finish_non_exhaustive()
    }
}

impl CredentialMutationOwner {
    /// Starts one current-thread Tokio runtime and returns its cloneable store handle.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if reaper capacity is unavailable, the reaper or
    /// mutation worker cannot start, or the Tokio runtime cannot be built.
    pub fn new(transport: Arc<dyn SecretTransport>) -> io::Result<(Self, CredentialMutationStore)> {
        Self::new_with_deadline(transport, MUTATION_DEADLINE)
    }

    fn new_with_deadline(
        transport: Arc<dyn SecretTransport>,
        deadline: Duration,
    ) -> io::Result<(Self, CredentialMutationStore)> {
        let reaper_reservation = credential_reaper()?.reserve()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let shared = Arc::new(SharedOwner {
            state: Mutex::new(OwnerState::default()),
            shutdown: AtomicBool::new(false),
        });
        let (sender, receiver) = mpsc::channel(CREDENTIAL_MUTATION_CAPACITY);
        let dispatcher = MutationDispatcher {
            sender: sender.clone(),
            shared: Arc::clone(&shared),
        };
        let store = CredentialMutationStore {
            transport: Arc::clone(&transport),
            dispatcher,
        };
        let worker_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("enzui-credential-mutations".to_owned())
            .spawn(move || {
                let mut worker = MutationWorker {
                    transport,
                    receiver,
                    shared: worker_shared,
                    deadline,
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runtime.block_on(worker.run());
                }));
                if result.is_err() {
                    worker.settle_after_panic();
                }
            })?;

        Ok((
            Self {
                sender,
                shared,
                worker: Some(OwnedMutationWorker {
                    thread,
                    reaper_reservation,
                }),
            },
            store,
        ))
    }

    /// Returns a nonblocking lifecycle snapshot.
    pub fn snapshot(&self) -> CredentialMutationSnapshot {
        let state = self.shared.lock();
        CredentialMutationSnapshot {
            accepting: state.accepting,
            admitted: state.admitted,
            wire_active: state.wire_active,
            drained: state.drained,
            worker_panicked: state.worker_panicked,
            last_generation: state.last_generation,
            uncertain_keys: state.uncertain.len(),
        }
    }

    /// Stops admission and requests nonblocking settlement of queued operations.
    ///
    /// The active provider call, if any, remains owned and awaited. Use
    /// [`Self::snapshot`] to observe eventual drain without blocking the caller.
    pub fn drain(&self) -> CredentialMutationSnapshot {
        let first = {
            let mut state = self.shared.lock();
            state.accepting = false;
            !self.shared.shutdown.swap(true, Ordering::SeqCst)
        };
        if first {
            let _ = self.sender.try_send(WorkerMessage::Shutdown);
        }
        self.snapshot()
    }
}

impl Drop for CredentialMutationOwner {
    fn drop(&mut self) {
        let _ = self.drain();
        if let Some(worker) = self.worker.take() {
            let OwnedMutationWorker {
                thread,
                reaper_reservation,
            } = worker;
            reaper_reservation.transfer(thread);
        }
    }
}

struct MutationWorker {
    transport: Arc<dyn SecretTransport>,
    receiver: mpsc::Receiver<WorkerMessage>,
    shared: Arc<SharedOwner>,
    deadline: Duration,
}

impl MutationWorker {
    async fn run(&mut self) {
        while let Some(message) = self.receiver.recv().await {
            match message {
                WorkerMessage::Mutation(request) => {
                    if self.shared.shutdown.load(Ordering::SeqCst) {
                        let _ = request.response.send(CredentialOutcome::OwnerShutdown);
                    } else {
                        self.process(request).await;
                    }
                }
                WorkerMessage::Shutdown => {}
            }
            if self.shared.shutdown.load(Ordering::SeqCst) {
                self.settle_queued();
                break;
            }
        }
        let mut state = self.shared.lock();
        state.accepting = false;
        state.wire_active = false;
        state.drained = true;
    }

    async fn process(&self, request: MutationRequest) {
        let MutationRequest {
            key,
            admitted_at,
            cancel,
            kind,
            response,
            _permit,
        } = request;

        let uncertainty_terminal = {
            let state = self.shared.lock();
            if state.uncertain.contains(&key) && !kind.is_reconcile() {
                Some(CredentialOutcome::MutationUncertain)
            } else if !state.uncertain.contains(&key)
                && state.uncertain.len() >= CREDENTIAL_UNCERTAINTY_CAPACITY
            {
                Some(CredentialOutcome::Saturated)
            } else {
                None
            }
        };
        if let Some(outcome) = uncertainty_terminal {
            let _ = response.send(outcome);
            return;
        }
        if admitted_at.elapsed() >= self.deadline {
            let _ = response.send(CredentialOutcome::Failed(PRE_WIRE_DEADLINE.to_owned()));
            return;
        }
        let pre_wire_terminal = {
            let mut state = self.shared.lock();
            if self.shared.shutdown.load(Ordering::SeqCst) {
                Some(CredentialOutcome::OwnerShutdown)
            } else if !cancel.try_enter_wire() {
                Some(CredentialOutcome::Cancelled)
            } else {
                state.wire_active = true;
                None
            }
        };
        if let Some(outcome) = pre_wire_terminal {
            let _ = response.send(outcome);
            return;
        }
        let reconcile = kind.is_reconcile();
        let wire = self.start_wire(&key, kind);
        let remaining = self.deadline.saturating_sub(admitted_at.elapsed());
        let outcome = match select(wire, Box::pin(tokio::time::sleep(remaining))).await {
            Either::Left((transport, _deadline)) => {
                if admitted_at.elapsed() >= self.deadline {
                    self.shared.lock().uncertain.insert(key);
                    drop(transport);
                    CredentialOutcome::MutationUncertain
                } else {
                    let outcome = map_transport_outcome(transport);
                    if reconcile
                        && matches!(
                            &outcome,
                            CredentialOutcome::Retrieved(_) | CredentialOutcome::NotFound
                        )
                    {
                        self.shared.lock().uncertain.remove(&key);
                    }
                    outcome
                }
            }
            Either::Right((_elapsed, wire)) => {
                self.shared.lock().uncertain.insert(key);
                let _ = response.send(CredentialOutcome::MutationUncertain);
                let _ = wire.await;
                let mut state = self.shared.lock();
                state.wire_active = false;
                return;
            }
        };
        {
            let mut state = self.shared.lock();
            state.wire_active = false;
        }
        let _ = response.send(outcome);
    }

    fn start_wire(&self, key: &CredentialKey, kind: MutationKind) -> TransportFuture {
        match kind {
            MutationKind::Set(secret) => self.transport.set(key, secret),
            MutationKind::Delete => self.transport.delete(key),
            MutationKind::Reconcile => self.transport.get(key),
        }
    }

    fn settle_queued(&mut self) {
        while let Ok(message) = self.receiver.try_recv() {
            if let WorkerMessage::Mutation(request) = message {
                let _ = request.response.send(CredentialOutcome::OwnerShutdown);
            }
        }
    }

    fn settle_after_panic(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        {
            let mut state = self.shared.lock();
            state.accepting = false;
            state.wire_active = false;
            state.worker_panicked = true;
        }
        self.settle_queued();
        self.shared.lock().drained = true;
    }
}

enum ReaperSlotState {
    Free,
    Reserved,
    Stranded { _worker: JoinHandle<()> },
}

struct ReaperShared {
    slots: [Mutex<ReaperSlotState>; CREDENTIAL_REAPER_CAPACITY],
    transfers: AtomicUsize,
    failed: AtomicBool,
}

impl ReaperShared {
    fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| Mutex::new(ReaperSlotState::Free)),
            transfers: AtomicUsize::new(0),
            failed: AtomicBool::new(false),
        }
    }

    fn reserve_index(&self) -> io::Result<usize> {
        if self.failed.load(Ordering::SeqCst) {
            return Err(io::Error::other("credential reaper unavailable"));
        }
        for (index, slot) in self.slots.iter().enumerate() {
            let mut state = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if matches!(&*state, ReaperSlotState::Free) {
                *state = ReaperSlotState::Reserved;
                if self.failed.load(Ordering::SeqCst) {
                    *state = ReaperSlotState::Free;
                    return Err(io::Error::other("credential reaper unavailable"));
                }
                return Ok(index);
            }
        }
        Err(io::Error::other("credential reaper capacity exhausted"))
    }

    fn release(&self, index: usize) {
        let mut state = self.slots[index]
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(&*state, ReaperSlotState::Reserved) {
            *state = ReaperSlotState::Free;
        }
    }

    fn strand(&self, index: usize, worker: JoinHandle<()>) {
        let mut state = self.slots[index]
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        debug_assert!(
            matches!(&*state, ReaperSlotState::Reserved),
            "only a reserved slot is released"
        );
        *state = ReaperSlotState::Stranded { _worker: worker };
    }
}

struct ReapRequest {
    slot: usize,
    worker: JoinHandle<()>,
}

struct ReaperReservation {
    sender: std::sync::mpsc::SyncSender<ReapRequest>,
    shared: Arc<ReaperShared>,
    slot: usize,
    active: bool,
}

impl ReaperReservation {
    fn transfer(mut self, worker: JoinHandle<()>) {
        let request = ReapRequest {
            slot: self.slot,
            worker,
        };
        if self.shared.failed.load(Ordering::SeqCst) {
            self.shared.strand(request.slot, request.worker);
        } else {
            self.shared.transfers.fetch_add(1, Ordering::SeqCst);
            if self.shared.failed.load(Ordering::SeqCst) {
                self.shared.transfers.fetch_sub(1, Ordering::SeqCst);
                self.shared.strand(request.slot, request.worker);
            } else {
                let result = self.sender.try_send(request);
                self.shared.transfers.fetch_sub(1, Ordering::SeqCst);
                if let Err(error) = result {
                    self.shared.failed.store(true, Ordering::SeqCst);
                    let request = match error {
                        std::sync::mpsc::TrySendError::Full(request)
                        | std::sync::mpsc::TrySendError::Disconnected(request) => request,
                    };
                    self.shared.strand(request.slot, request.worker);
                }
            }
        }
        self.active = false;
    }
}

impl Drop for ReaperReservation {
    fn drop(&mut self) {
        if self.active {
            self.shared.release(self.slot);
        }
    }
}

struct CredentialReaper {
    sender: std::sync::mpsc::SyncSender<ReapRequest>,
    shared: Arc<ReaperShared>,
    _thread: Mutex<Option<JoinHandle<()>>>,
}

impl CredentialReaper {
    fn start() -> io::Result<Self> {
        let shared = Arc::new(ReaperShared::new());
        let worker_shared = Arc::clone(&shared);
        let (sender, receiver) =
            std::sync::mpsc::sync_channel::<ReapRequest>(CREDENTIAL_REAPER_CAPACITY);
        let thread = std::thread::Builder::new()
            .name("enzui-credential-reaper".to_owned())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    while let Ok(request) = receiver.recv() {
                        let ReapRequest { slot, worker } = request;
                        let _ = worker.join();
                        worker_shared.release(slot);
                    }
                }));
                worker_shared.failed.store(true, Ordering::SeqCst);
                while worker_shared.transfers.load(Ordering::SeqCst) != 0 {
                    std::thread::yield_now();
                }
                if result.is_err() {
                    while let Ok(request) = receiver.try_recv() {
                        worker_shared.strand(request.slot, request.worker);
                    }
                }
            })?;
        Ok(Self {
            sender,
            shared,
            _thread: Mutex::new(Some(thread)),
        })
    }

    fn reserve(&self) -> io::Result<ReaperReservation> {
        let slot = self.shared.reserve_index()?;
        Ok(ReaperReservation {
            sender: self.sender.clone(),
            shared: Arc::clone(&self.shared),
            slot,
            active: true,
        })
    }
}

fn credential_reaper() -> io::Result<&'static CredentialReaper> {
    static REAPER: OnceLock<Result<CredentialReaper, String>> = OnceLock::new();
    match REAPER.get_or_init(|| CredentialReaper::start().map_err(|error| error.to_string())) {
        Ok(reaper) => Ok(reaper),
        Err(reason) => Err(io::Error::other(reason.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::super::TransportOutcome;
    use super::*;

    struct RecordedTransport {
        calls: Arc<Mutex<Vec<CredentialOperation>>>,
    }

    impl SecretTransport for RecordedTransport {
        fn get(&self, _key: &CredentialKey) -> TransportFuture {
            Box::pin(async { TransportOutcome::NotFound })
        }

        fn set(&self, _key: &CredentialKey, secret: Secret) -> TransportFuture {
            drop(secret);
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(CredentialOperation::Set);
            Box::pin(async { TransportOutcome::Stored })
        }

        fn delete(&self, _key: &CredentialKey) -> TransportFuture {
            self.calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(CredentialOperation::Delete);
            Box::pin(async { TransportOutcome::Deleted })
        }
    }

    fn key(label: impl fmt::Display) -> CredentialKey {
        CredentialKey::new("com.enzui.owner-test", "owner", label.to_string())
    }

    fn test_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
    }

    fn await_op(runtime: &tokio::runtime::Runtime, op: PendingCredentialOp) -> CredentialOutcome {
        // The timeout's timer registers with the runtime when created, so it
        // is built inside `block_on`.
        runtime
            .block_on(async { tokio::time::timeout(Duration::from_secs(2), op).await })
            .expect("credential operation must settle within two seconds")
    }

    #[test]
    fn mutation_owner_same_key_set_delete_order() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let transport = Arc::new(RecordedTransport {
            calls: Arc::clone(&calls),
        });
        let (_owner, store) = CredentialMutationOwner::new(transport).expect("owner");
        let runtime = test_runtime();
        let target = key("same");
        let set = store.set(target.clone(), Secret::from_string("value".to_owned()));
        let delete = store.delete(target);

        assert_eq!(await_op(&runtime, set), CredentialOutcome::Stored);
        assert_eq!(await_op(&runtime, delete), CredentialOutcome::Deleted);
        assert_eq!(
            *calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec![CredentialOperation::Set, CredentialOperation::Delete]
        );
    }
}
