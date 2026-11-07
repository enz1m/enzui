//! Asynchronous credential storage with typed outcomes and application cancellation.
//!
//! Open the store an application should use with [`connect_for_app`]: the
//! Secret Service on a desktop, and inside a Flatpak or Snap sandbox (feature
//! `secret-portal`) a keyring file sealed with the master secret the
//! `org.freedesktop.portal.Secret` portal gives the application ([`portal`]).
//!
//! Stores are object-safe and return [`PendingCredentialOp`] futures. Missing services, policy denial, dismissed unlock prompts, application cancellation, and transport failure remain distinct outcomes. Implementations must not block or fall back to plaintext files. [`CredentialPresence`] permits non-unlocking existence checks.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

#[cfg(feature = "secret-service")]
pub mod mutation_owner;
#[cfg(feature = "secret-portal")]
pub mod portal;
#[cfg(feature = "secret-service")]
pub mod secret_service;
#[cfg(feature = "secret-service")]
pub use secret_service::{
    connect_for_app, connect_for_app_with_mutation_owner, system_transport, system_transport_with,
};

/// A credential key consisting of application namespace, owner, and per-owner label.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct CredentialKey {
    app_id: String,
    owner: String,
    label: String,
}

impl CredentialKey {
    /// Creates a credential key from its application, owner, and label components.
    pub fn new(
        app_id: impl Into<String>,
        owner: impl Into<String>,
        label: impl Into<String>,
    ) -> Self {
        Self {
            app_id: app_id.into(),
            owner: owner.into(),
            label: label.into(),
        }
    }

    /// The reverse-DNS application id.
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    /// The app-scoped owner id.
    pub fn owner(&self) -> &str {
        &self.owner
    }

    /// The credential label within the owner.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns the fixed Secret Service attribute schema in application, owner, label order.
    pub fn attributes(&self) -> [(&'static str, &str); 3] {
        [
            ("app-id", self.app_id.as_str()),
            ("owner", self.owner.as_str()),
            ("label", self.label.as_str()),
        ]
    }
}

// Credential keys are diagnostic metadata, not secret values, so their Debug output remains unredacted.
impl fmt::Debug for CredentialKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CredentialKey")
            .field("app_id", &self.app_id)
            .field("owner", &self.owner)
            .field("label", &self.label)
            .finish()
    }
}

/// Wipe-on-drop, non-cloneable secret bytes with redacted formatting.
pub use enzui_security::secret::Secret;

/// Identifies the credential operation associated with an outcome or pending task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CredentialOperation {
    /// Read the current value of a key.
    Get,
    /// Insert or overwrite the value at a key.
    Set,
    /// Remove a key.
    Delete,
}

/// The typed terminal outcome of one credential operation.
///
/// A dismissed unlock prompt is represented separately from transport failure.
#[derive(Debug)]
pub enum CredentialOutcome {
    /// The value was read successfully.
    Retrieved(Secret),
    /// The value was written.
    Stored,
    /// The key was removed, or was already absent; deletion is idempotent.
    Deleted,
    /// The store looked, and there was no such item.
    NotFound,
    /// No Secret Service is available; callers must not fall back to plaintext storage.
    Unavailable,
    /// The store is present but refused this operation (a system policy, a
    /// SELinux label, a Flatpak permission missing).
    PermissionDenied,
    /// The user dismissed the prompt to unlock a locked keyring; this is not a transport failure.
    UnlockCancelled,
    /// The caller invoked [`CancelHandle::cancel`] before the operation
    /// completed. Distinct from [`Self::UnlockCancelled`]: cancellation from
    /// the application, not from the user's unlock dialog.
    Cancelled,
    /// The mutation owner has admitted its fixed maximum of 16 operations.
    Saturated,
    /// The mutation owner is shutting down and did not enter the provider.
    OwnerShutdown,
    /// A mutation crossed its post-wire deadline, so provider state requires reconciliation.
    MutationUncertain,
    /// A transport or protocol failure with a short diagnostic string.
    ///
    /// The diagnostic is not a stable machine-readable value.
    Failed(String),
}

impl PartialEq for CredentialOutcome {
    fn eq(&self, other: &Self) -> bool {
        use CredentialOutcome::*;
        match (self, other) {
            (Retrieved(a), Retrieved(b)) => a.expose_secret() == b.expose_secret(),
            (Stored, Stored) => true,
            (Deleted, Deleted) => true,
            (NotFound, NotFound) => true,
            (Unavailable, Unavailable) => true,
            (PermissionDenied, PermissionDenied) => true,
            (UnlockCancelled, UnlockCancelled) => true,
            (Cancelled, Cancelled) => true,
            (Saturated, Saturated) => true,
            (OwnerShutdown, OwnerShutdown) => true,
            (MutationUncertain, MutationUncertain) => true,
            (Failed(a), Failed(b)) => a == b,
            _ => false,
        }
    }
}

impl CredentialOutcome {
    /// Returns `true` for retrieved, stored, and deleted outcomes.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Retrieved(_) | Self::Stored | Self::Deleted)
    }

    /// Returns `true` for application cancellation or dismissed unlocking.
    pub fn is_cancellation(&self) -> bool {
        matches!(self, Self::UnlockCancelled | Self::Cancelled)
    }

    /// Returns `true` only when no credential service is available.
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable)
    }

    /// Returns `true` when a mutation requires explicit provider reconciliation.
    pub fn is_uncertain(&self) -> bool {
        matches!(self, Self::MutationUncertain)
    }

    /// Returns a fixed, secret-free summary suitable for user-visible status.
    pub fn user_summary(&self) -> &'static str {
        match self {
            Self::Retrieved(_) => "Credential retrieved.",
            Self::Stored => "Credential stored.",
            Self::Deleted => "Credential deleted.",
            Self::NotFound => "Credential not found.",
            Self::Unavailable => "Credential service unavailable.",
            Self::PermissionDenied => "Credential operation denied.",
            Self::UnlockCancelled => "Credential unlock cancelled.",
            Self::Cancelled => "Credential operation cancelled.",
            Self::Saturated => "Credential service is busy.",
            Self::OwnerShutdown => "Credential service is shutting down.",
            Self::MutationUncertain => "Credential state needs confirmation.",
            Self::Failed(_) => "Credential operation failed.",
        }
    }
}

// One atomic terminal owner shared by the future and handles; only the first transition from pending can win.
#[derive(Debug, Default)]
struct CancelFlag {
    terminal: AtomicU8,
    // Set only during independently bounded protocol settlement, allowing post-cancel polling until cleanup returns.
    settling: AtomicBool,
    wakers: Mutex<Vec<Waker>>,
    // Separate slot for the transport’s cancellation waiter so outer polling registration cannot replace it.
    transport_wakers: Mutex<Vec<Waker>>,
}

impl CancelFlag {
    const PENDING: u8 = 0;
    const CANCELLED: u8 = 1;
    const COMPLETED: u8 = 2;
    const ABANDONED: u8 = 3;
    const WIRE_ACTIVE: u8 = 4;

    fn cancel(&self) -> bool {
        let first = self
            .terminal
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |state| {
                matches!(state, Self::PENDING | Self::WIRE_ACTIVE).then_some(Self::CANCELLED)
            })
            .is_ok();
        if first {
            // Detach under the mutex and wake afterward; a waker may synchronously re-enter this state.
            let wakers = {
                let mut wakers = self
                    .wakers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                std::mem::take(&mut *wakers)
            };
            for w in wakers {
                w.wake();
            }
            let transport = {
                let mut t = self
                    .transport_wakers
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                std::mem::take(&mut *t)
            };
            for w in transport {
                w.wake();
            }
        }
        first
    }

    // Replace the sole transport cancellation waiter; `cancel` drains and wakes this slot.
    fn register_transport(&self, waker: &Waker) {
        let mut wakers = self
            .transport_wakers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        wakers.clear();
        wakers.push(waker.clone());
    }

    fn complete(&self) -> bool {
        let first = self
            .terminal
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |state| {
                matches!(state, Self::PENDING | Self::WIRE_ACTIVE).then_some(Self::COMPLETED)
            })
            .is_ok();
        // Terminal completion always releases registered wakers, even if it loses arbitration.
        drop(self.take_wakers());
        first
    }

    // Abandon a dropped pending future without marking it cancelled, and release every retained waker.
    fn abandon(&self) {
        let _ = self
            .terminal
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |state| {
                matches!(state, Self::PENDING | Self::WIRE_ACTIVE).then_some(Self::ABANDONED)
            });
        drop(self.take_wakers());
    }

    #[cfg(feature = "secret-service")]
    fn try_enter_wire(&self) -> bool {
        self.terminal
            .compare_exchange(
                Self::PENDING,
                Self::WIRE_ACTIVE,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
    }

    fn is_cancelled(&self) -> bool {
        self.terminal.load(Ordering::SeqCst) == Self::CANCELLED
    }

    #[cfg(feature = "secret-service")]
    fn begin_settlement(&self) {
        self.settling.store(true, Ordering::SeqCst);
    }

    fn is_settling(&self) -> bool {
        self.settling.load(Ordering::SeqCst)
    }

    // Clear the transport waiter, then detach the operation waiter for handling outside its registry lock.
    fn take_wakers(&self) -> Vec<Waker> {
        // Clear the transport waiter when the operation becomes terminal.
        {
            let mut t = self
                .transport_wakers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            t.clear();
        }
        let mut wakers = self
            .wakers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *wakers)
    }

    fn register(&self, waker: &Waker) {
        let mut wakers = self
            .wakers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        wakers.clear();
        wakers.push(waker.clone());
    }
}

/// Cloneable handle for cancelling a [`PendingCredentialOp`] from another task.
///
/// Cancelling a completed or dropped operation is a no-op, and retained handles do not retain its task waker.
#[derive(Debug, Clone)]
pub struct CancelHandle {
    flag: Arc<CancelFlag>,
}

impl CancelHandle {
    /// Requests cancellation and returns whether this call won terminal arbitration.
    ///
    /// The pending operation normally reports cancellation on its next poll; a transport may first finish independently bounded protocol settlement.
    pub fn cancel(&self) -> bool {
        self.flag.cancel()
    }

    /// Whether [`Self::cancel`] has been called on this handle or any of its
    /// clones.
    pub fn is_cancelled(&self) -> bool {
        self.flag.is_cancelled()
    }
}

/// A transport view of the same cancellation state used by a pending operation’s handles.
///
/// Awaiting [`Self::cancelled`] lets a backend leave wire waits through a typed cancellation result.
#[derive(Clone)]
pub struct TransportCancel {
    flag: Arc<CancelFlag>,
}

impl fmt::Debug for TransportCancel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TransportCancel")
            .field("cancelled", &self.flag.is_cancelled())
            .finish()
    }
}

impl TransportCancel {
    /// Creates a token that never becomes cancelled.
    pub fn inert() -> Self {
        Self {
            flag: Arc::new(CancelFlag::default()),
        }
    }

    /// Whether the owning operation has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.flag.is_cancelled()
    }

    /// Returns a future that resolves when the owning operation is cancelled.
    ///
    /// Registration is rechecked so a concurrent cancellation cannot be missed.
    pub fn cancelled(&self) -> Cancelled<'_> {
        Cancelled {
            flag: self.flag.as_ref(),
        }
    }

    // Enter bounded protocol settlement so the owner keeps polling after cancellation; use only with an independent deadline.
    #[cfg(feature = "secret-service")]
    fn begin_settlement(&self) {
        self.flag.begin_settlement();
    }

    #[cfg(feature = "secret-service")]
    pub(crate) fn try_enter_wire(&self) -> bool {
        self.flag.try_enter_wire()
    }
}

/// The future returned by [`TransportCancel::cancelled`]. Resolves once, when
/// the operation is cancelled.
pub struct Cancelled<'a> {
    flag: &'a CancelFlag,
}

impl Future for Cancelled<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.flag.is_cancelled() {
            return Poll::Ready(());
        }
        // Register before rechecking so a concurrent cancellation cannot be missed.
        self.flag.register_transport(cx.waker());
        if self.flag.is_cancelled() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// A single-owner future for a credential operation, with cancellation and operation metadata.
pub struct PendingCredentialOp {
    inner: Pin<Box<dyn Future<Output = CredentialOutcome> + Send>>,
    flag: Arc<CancelFlag>,
    operation: CredentialOperation,
    key: CredentialKey,
    finished: bool,
    // Track whether the inner future started: pre-start cancellation must not run it, while a parked transport gets a final cancellation poll and optional bounded settlement.
    inner_polled: bool,
}

impl fmt::Debug for PendingCredentialOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PendingCredentialOp")
            .field("operation", &self.operation)
            .field("key", &self.key)
            .field("cancelled", &self.flag.is_cancelled())
            .field("finished", &self.finished)
            .finish()
    }
}

impl PendingCredentialOp {
    /// Wraps a backend future with its operation and key metadata.
    pub fn new<F>(operation: CredentialOperation, key: CredentialKey, future: F) -> Self
    where
        F: Future<Output = CredentialOutcome> + Send + 'static,
    {
        Self {
            inner: Box::pin(future),
            flag: Arc::new(CancelFlag::default()),
            operation,
            key,
            finished: false,
            inner_polled: false,
        }
    }

    /// Builds an operation whose transport token shares cancellation state with the returned operation’s handles.
    pub fn with_transport_cancel<F, Fut>(
        operation: CredentialOperation,
        key: CredentialKey,
        build: F,
    ) -> Self
    where
        F: FnOnce(TransportCancel) -> Fut,
        Fut: Future<Output = CredentialOutcome> + Send + 'static,
    {
        let flag = Arc::new(CancelFlag::default());
        let cancel = TransportCancel {
            flag: Arc::clone(&flag),
        };
        Self {
            inner: Box::pin(build(cancel)),
            flag,
            operation,
            key,
            finished: false,
            inner_polled: false,
        }
    }

    /// Returns a cloneable cancellation handle for this operation.
    pub fn cancel_handle(&self) -> CancelHandle {
        CancelHandle {
            flag: Arc::clone(&self.flag),
        }
    }

    /// Returns the operation kind represented by this future.
    pub fn operation(&self) -> CredentialOperation {
        self.operation
    }

    /// The key this pending op names.
    pub fn key(&self) -> &CredentialKey {
        &self.key
    }
}

impl Drop for PendingCredentialOp {
    // Dropping retires the shared flag and releases wakers so surviving handles cannot claim cancellation.
    fn drop(&mut self) {
        self.flag.abandon();
    }
}

impl Future for PendingCredentialOp {
    type Output = CredentialOutcome;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Register first so concurrent cancellation wakes this task promptly.
        self.flag.register(cx.waker());
        if self.flag.is_cancelled() && !self.finished {
            if !self.inner_polled {
                // Never start the inner future after cancellation has already won.
                self.finished = true;
                return Poll::Ready(CredentialOutcome::Cancelled);
            }
            // Poll a started transport to observe cancellation; continue only while it performs bounded settlement.
            let inner = self.inner.as_mut().poll(cx);
            if inner.is_pending() && self.flag.is_settling() {
                return Poll::Pending;
            }
            self.finished = true;
            return Poll::Ready(CredentialOutcome::Cancelled);
        }
        self.inner_polled = true;
        match self.inner.as_mut().poll(cx) {
            Poll::Pending => {
                // Recheck cancellation after polling the inner future.
                if self.flag.is_cancelled() {
                    if self.flag.is_settling() {
                        Poll::Pending
                    } else {
                        self.finished = true;
                        Poll::Ready(CredentialOutcome::Cancelled)
                    }
                } else {
                    Poll::Pending
                }
            }
            Poll::Ready(outcome) => {
                self.finished = true;
                // Claim completion against the shared terminal state before publishing the transport outcome.
                if self.flag.complete() {
                    Poll::Ready(outcome)
                } else {
                    Poll::Ready(CredentialOutcome::Cancelled)
                }
            }
        }
    }
}

/// A non-unlocking answer to whether a credential exists.
///
/// Backends that cannot answer cheaply return [`Self::Unknown`] rather than unlocking, blocking, or guessing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CredentialPresence {
    /// The store cheaply determined that a secret is present.
    Present,
    /// The store cheaply determined that no secret is present.
    Absent,
    /// The store cannot answer without expensive or interactive work.
    ///
    /// Callers should treat this as unknown and use `get` only after explicit user action.
    Unknown,
}

impl CredentialPresence {
    /// Whether this is a definite answer ([`Self::Present`] or
    /// [`Self::Absent`]) as opposed to [`Self::Unknown`]. A caller that only
    /// wants to act on certainty gates on this.
    pub fn is_known(self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

/// An object-safe asynchronous credential store.
///
/// Method bodies must not block; waits belong in returned futures. Unavailable stores must not use plaintext-file fallback. Deleting an absent key is successful and returns [`CredentialOutcome::Deleted`].
pub trait CredentialStore: Send + Sync {
    /// Read the value at `key`.
    fn get(&self, key: CredentialKey) -> PendingCredentialOp;

    /// Inserts or overwrites `key`, taking ownership of the non-cloneable secret.
    fn set(&self, key: CredentialKey, secret: Secret) -> PendingCredentialOp;

    /// Remove the value at `key`. Idempotent (see the trait contract).
    fn delete(&self, key: CredentialKey) -> PendingCredentialOp;

    /// Checks for a credential without retrieving or unlocking it.
    ///
    /// This method must remain synchronous and cheap. Implementations requiring I/O, blocking, or user interaction return [`CredentialPresence::Unknown`]; the default does so without calling [`Self::get`].
    fn contains(&self, key: &CredentialKey) -> CredentialPresence {
        let _ = key;
        CredentialPresence::Unknown
    }
}

struct ReadyOnce {
    outcome: Option<CredentialOutcome>,
}

impl ReadyOnce {
    fn new(outcome: CredentialOutcome) -> Self {
        Self {
            outcome: Some(outcome),
        }
    }
}

impl Future for ReadyOnce {
    type Output = CredentialOutcome;
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Ready(
            self.outcome
                .take()
                .expect("ReadyOnce polled after completion"),
        )
    }
}

/// A controllable gate that keeps mock credential operations pending until opened.
#[derive(Default)]
pub struct Gate {
    open: AtomicBool,
    wakers: Mutex<VecDeque<Waker>>,
    #[cfg(test)]
    on_register: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl fmt::Debug for Gate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Gate")
            .field("open", &self.open.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl Gate {
    /// A new, closed gate. Every op stalls until [`open`](Self::open) is
    /// called.
    pub fn new_closed() -> Arc<Self> {
        Arc::new(Self {
            open: AtomicBool::new(false),
            wakers: Mutex::new(VecDeque::new()),
            #[cfg(test)]
            on_register: Mutex::new(None),
        })
    }

    /// A new, open gate. Ops resolve immediately.
    pub fn new_open() -> Arc<Self> {
        Arc::new(Self {
            open: AtomicBool::new(true),
            wakers: Mutex::new(VecDeque::new()),
            #[cfg(test)]
            on_register: Mutex::new(None),
        })
    }

    /// Open the gate and wake every waiter.
    pub fn open(&self) {
        self.open.store(true, Ordering::SeqCst);
        // Never invoke a Waker while holding the registry lock. Wake is an
        // arbitrary executor callback and may synchronously re-enter this gate
        // or poll a future whose lock order starts elsewhere.
        let wakers = {
            let mut wakers = self
                .wakers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *wakers)
        };
        for w in wakers {
            w.wake();
        }
    }

    // Recheck open state while holding the waiter mutex so opening cannot race past registration.
    fn register_if_closed(&self, waker: &Waker) -> bool {
        #[cfg(test)]
        self.fire_register_hook();
        let mut wakers = self
            .wakers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.open.load(Ordering::SeqCst) {
            false
        } else {
            wakers.push_back(waker.clone());
            true
        }
    }

    #[cfg(test)]
    fn fire_register_hook(&self) {
        let hook = self
            .on_register
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(hook) = hook {
            hook();
        }
    }
}

/// In-memory credential storage with an optional operation gate.
///
/// Pending operations share the backing map and may complete after the store
/// handle is dropped. Stored values remain in wipe-on-drop [`Secret`] custody.
pub struct MemoryCredentialStore {
    entries: Arc<Mutex<HashMap<CredentialKey, Secret>>>,
    gate: Arc<Gate>,
    get_calls: Arc<AtomicUsize>,
}

impl Default for MemoryCredentialStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryCredentialStore {
    /// A store with an open gate — ops complete on first poll.
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            gate: Gate::new_open(),
            get_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Creates a memory store whose operations wait for `gate`.
    pub fn with_gate(gate: Arc<Gate>) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            gate,
            get_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Returns the number of `get` calls made through this store.
    pub fn get_call_count(&self) -> usize {
        self.get_calls.load(Ordering::SeqCst)
    }

    /// Returns the number of currently stored entries across all keys.
    pub fn entry_count(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    fn schedule(&self, key: CredentialKey, op: MemoryOp) -> PendingCredentialOp {
        let operation = match op {
            MemoryOp::Get => CredentialOperation::Get,
            MemoryOp::Set(_) => CredentialOperation::Set,
            MemoryOp::Delete => CredentialOperation::Delete,
        };
        let fut = MemoryFuture {
            entries: Arc::clone(&self.entries),
            gate: Arc::clone(&self.gate),
            key: key.clone(),
            op: Some(op),
        };
        PendingCredentialOp::new(operation, key, fut)
    }
}

impl CredentialStore for MemoryCredentialStore {
    fn get(&self, key: CredentialKey) -> PendingCredentialOp {
        self.get_calls.fetch_add(1, Ordering::SeqCst);
        self.schedule(key, MemoryOp::Get)
    }
    fn set(&self, key: CredentialKey, secret: Secret) -> PendingCredentialOp {
        self.schedule(key, MemoryOp::Set(secret))
    }
    fn delete(&self, key: CredentialKey) -> PendingCredentialOp {
        self.schedule(key, MemoryOp::Delete)
    }

    /// Answers from one in-memory lookup without I/O or calling `get`; this store never returns Unknown.
    fn contains(&self, key: &CredentialKey) -> CredentialPresence {
        let entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if entries.contains_key(key) {
            CredentialPresence::Present
        } else {
            CredentialPresence::Absent
        }
    }
}

enum MemoryOp {
    Get,
    Set(Secret),
    Delete,
}

struct MemoryFuture {
    entries: Arc<Mutex<HashMap<CredentialKey, Secret>>>,
    gate: Arc<Gate>,
    key: CredentialKey,
    op: Option<MemoryOp>,
}

impl Future for MemoryFuture {
    type Output = CredentialOutcome;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.gate.register_if_closed(cx.waker()) {
            return Poll::Pending;
        }
        let op = self
            .op
            .take()
            .expect("MemoryFuture polled after completion");
        let mut entries = self
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let outcome = match op {
            MemoryOp::Get => match entries.get(&self.key) {
                Some(secret) => {
                    CredentialOutcome::Retrieved(Secret::new(secret.expose_secret().to_vec()))
                }
                None => CredentialOutcome::NotFound,
            },
            MemoryOp::Set(secret) => {
                entries.insert(self.key.clone(), secret);
                CredentialOutcome::Stored
            }
            MemoryOp::Delete => {
                let _ = entries.remove(&self.key);
                CredentialOutcome::Deleted
            }
        };
        Poll::Ready(outcome)
    }
}

/// A stateless store whose operations always return [`CredentialOutcome::Unavailable`].
#[derive(Debug, Default)]
pub struct UnavailableCredentialStore;

impl CredentialStore for UnavailableCredentialStore {
    fn get(&self, key: CredentialKey) -> PendingCredentialOp {
        PendingCredentialOp::new(
            CredentialOperation::Get,
            key,
            ReadyOnce::new(CredentialOutcome::Unavailable),
        )
    }
    fn set(&self, key: CredentialKey, secret: Secret) -> PendingCredentialOp {
        // Drop the moved secret before returning so this store retains no plaintext.
        drop(secret);
        PendingCredentialOp::new(
            CredentialOperation::Set,
            key,
            ReadyOnce::new(CredentialOutcome::Unavailable),
        )
    }
    fn delete(&self, key: CredentialKey) -> PendingCredentialOp {
        PendingCredentialOp::new(
            CredentialOperation::Delete,
            key,
            ReadyOnce::new(CredentialOutcome::Unavailable),
        )
    }

    /// Returns `Unknown` because an unavailable service cannot establish absence.
    fn contains(&self, key: &CredentialKey) -> CredentialPresence {
        let _ = key;
        CredentialPresence::Unknown
    }
}

/// A stateless store whose operations always return [`CredentialOutcome::PermissionDenied`].
#[derive(Debug, Default)]
pub struct PermissionDeniedCredentialStore;

impl CredentialStore for PermissionDeniedCredentialStore {
    fn get(&self, key: CredentialKey) -> PendingCredentialOp {
        PendingCredentialOp::new(
            CredentialOperation::Get,
            key,
            ReadyOnce::new(CredentialOutcome::PermissionDenied),
        )
    }
    fn set(&self, key: CredentialKey, secret: Secret) -> PendingCredentialOp {
        drop(secret);
        PendingCredentialOp::new(
            CredentialOperation::Set,
            key,
            ReadyOnce::new(CredentialOutcome::PermissionDenied),
        )
    }
    fn delete(&self, key: CredentialKey) -> PendingCredentialOp {
        PendingCredentialOp::new(
            CredentialOperation::Delete,
            key,
            ReadyOnce::new(CredentialOutcome::PermissionDenied),
        )
    }

    /// Returns `Unknown` because policy prevents inspection.
    fn contains(&self, key: &CredentialKey) -> CredentialPresence {
        let _ = key;
        CredentialPresence::Unknown
    }
}

/// A stateless store whose operations always return [`CredentialOutcome::UnlockCancelled`].
#[derive(Debug, Default)]
pub struct UnlockCancelledCredentialStore;

impl CredentialStore for UnlockCancelledCredentialStore {
    fn get(&self, key: CredentialKey) -> PendingCredentialOp {
        PendingCredentialOp::new(
            CredentialOperation::Get,
            key,
            ReadyOnce::new(CredentialOutcome::UnlockCancelled),
        )
    }
    fn set(&self, key: CredentialKey, secret: Secret) -> PendingCredentialOp {
        drop(secret);
        PendingCredentialOp::new(
            CredentialOperation::Set,
            key,
            ReadyOnce::new(CredentialOutcome::UnlockCancelled),
        )
    }
    fn delete(&self, key: CredentialKey) -> PendingCredentialOp {
        PendingCredentialOp::new(
            CredentialOperation::Delete,
            key,
            ReadyOnce::new(CredentialOutcome::UnlockCancelled),
        )
    }

    /// Returns `Unknown` rather than unlocking the keyring to inspect it.
    fn contains(&self, key: &CredentialKey) -> CredentialPresence {
        let _ = key;
        CredentialPresence::Unknown
    }
}

/// A store whose operations return [`CredentialOutcome::Failed`] with a configured diagnostic.
#[derive(Debug)]
pub struct FailingCredentialStore {
    reason: String,
}

impl FailingCredentialStore {
    /// Build a failing store with the given short diagnostic.
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

impl CredentialStore for FailingCredentialStore {
    fn get(&self, key: CredentialKey) -> PendingCredentialOp {
        PendingCredentialOp::new(
            CredentialOperation::Get,
            key,
            ReadyOnce::new(CredentialOutcome::Failed(self.reason.clone())),
        )
    }
    fn set(&self, key: CredentialKey, secret: Secret) -> PendingCredentialOp {
        drop(secret);
        PendingCredentialOp::new(
            CredentialOperation::Set,
            key,
            ReadyOnce::new(CredentialOutcome::Failed(self.reason.clone())),
        )
    }
    fn delete(&self, key: CredentialKey) -> PendingCredentialOp {
        PendingCredentialOp::new(
            CredentialOperation::Delete,
            key,
            ReadyOnce::new(CredentialOutcome::Failed(self.reason.clone())),
        )
    }

    /// Returns `Unknown` because the failed transport cannot inspect the store.
    fn contains(&self, key: &CredentialKey) -> CredentialPresence {
        let _ = key;
        CredentialPresence::Unknown
    }
}

/// A boxed `Send` transport-operation future used by object-safe transport methods.
pub type TransportFuture = Pin<Box<dyn Future<Output = TransportOutcome> + Send>>;

/// A transport-level Secret Service outcome mapped to [`CredentialOutcome`] by [`map_transport_outcome`].
#[derive(Debug)]
pub enum TransportOutcome {
    /// The item was found and its secret decrypted.
    Found(Secret),
    /// The collection was searched and no matching item exists.
    NotFound,
    /// The item was created or its secret overwritten.
    Stored,
    /// The item was deleted or already absent.
    Deleted,
    /// Unlocking was dismissed or suppressed; maps to [`CredentialOutcome::UnlockCancelled`].
    Locked,
    /// The service is present but refused the operation (policy, sandbox).
    PermissionDenied,
    /// No Secret Service is available; maps to [`CredentialOutcome::Unavailable`] without plaintext fallback.
    Unavailable,
    /// The prompt completion signal timed out and maps to a fixed [`CredentialOutcome::Failed`] diagnostic.
    PromptTimeout,
    /// The owning operation was cancelled while awaiting the transport; maps to [`CredentialOutcome::Cancelled`].
    PromptCancelled,
    /// A transport failure with a short diagnostic that must not contain secret bytes.
    Failed(String),
}

impl PartialEq for TransportOutcome {
    fn eq(&self, other: &Self) -> bool {
        use TransportOutcome::*;
        match (self, other) {
            (Found(a), Found(b)) => a.expose_secret() == b.expose_secret(),
            (NotFound, NotFound)
            | (Stored, Stored)
            | (Deleted, Deleted)
            | (Locked, Locked)
            | (PermissionDenied, PermissionDenied)
            | (Unavailable, Unavailable)
            | (PromptTimeout, PromptTimeout)
            | (PromptCancelled, PromptCancelled) => true,
            (Failed(a), Failed(b)) => a == b,
            _ => false,
        }
    }
}

/// Maps every transport outcome to its corresponding application outcome.
///
/// Locked, permission-denied, unavailable, timed-out, and cancelled states remain distinct.
pub fn map_transport_outcome(outcome: TransportOutcome) -> CredentialOutcome {
    match outcome {
        TransportOutcome::Found(secret) => CredentialOutcome::Retrieved(secret),
        TransportOutcome::NotFound => CredentialOutcome::NotFound,
        TransportOutcome::Stored => CredentialOutcome::Stored,
        TransportOutcome::Deleted => CredentialOutcome::Deleted,
        TransportOutcome::Locked => CredentialOutcome::UnlockCancelled,
        TransportOutcome::PermissionDenied => CredentialOutcome::PermissionDenied,
        TransportOutcome::Unavailable => CredentialOutcome::Unavailable,
        TransportOutcome::PromptTimeout => {
            CredentialOutcome::Failed("secret-service prompt timed out".into())
        }
        TransportOutcome::PromptCancelled => CredentialOutcome::Cancelled,
        TransportOutcome::Failed(reason) => CredentialOutcome::Failed(reason),
    }
}

/// An object-safe asynchronous Secret Service transport.
///
/// Methods must not block, diagnostics must exclude secret bytes, and items are keyed by all [`CredentialKey::attributes`].
pub trait SecretTransport: Send + Sync {
    /// Read the secret at `key`.
    fn get(&self, key: &CredentialKey) -> TransportFuture;
    /// Creates or overwrites `key`, taking ownership of a secret the transport must wipe after handoff.
    fn set(&self, key: &CredentialKey, secret: Secret) -> TransportFuture;
    /// Remove the item at `key`. Idempotent.
    fn delete(&self, key: &CredentialKey) -> TransportFuture;

    /// Calls [`Self::get`] with operation cancellation.
    ///
    /// The default ignores the token; wire-waiting transports may override it to observe cancellation directly.
    fn get_cancellable(&self, key: &CredentialKey, cancel: TransportCancel) -> TransportFuture {
        let _ = cancel;
        self.get(key)
    }
    /// Calls [`Self::set`] with operation cancellation; see [`Self::get_cancellable`].
    fn set_cancellable(
        &self,
        key: &CredentialKey,
        secret: Secret,
        cancel: TransportCancel,
    ) -> TransportFuture {
        let _ = cancel;
        self.set(key, secret)
    }
    /// Calls [`Self::delete`] with operation cancellation; see [`Self::get_cancellable`].
    fn delete_cancellable(&self, key: &CredentialKey, cancel: TransportCancel) -> TransportFuture {
        let _ = cancel;
        self.delete(key)
    }

    /// Returns a cheap non-unlocking presence answer, defaulting to [`CredentialPresence::Unknown`].
    fn contains(&self, key: &CredentialKey) -> CredentialPresence {
        let _ = key;
        CredentialPresence::Unknown
    }
}

/// Adapts any [`SecretTransport`] into a [`CredentialStore`].
pub struct TransportCredentialStore {
    transport: Arc<dyn SecretTransport>,
}

impl fmt::Debug for TransportCredentialStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never render the transport's contents: a stored secret must not
        // reach diagnostics through the store's Debug.
        f.debug_struct("TransportCredentialStore")
            .field("transport", &"<dyn SecretTransport>")
            .finish()
    }
}

impl TransportCredentialStore {
    /// Wraps a shared Secret Service transport.
    pub fn new(transport: Arc<dyn SecretTransport>) -> Self {
        Self { transport }
    }
}

impl CredentialStore for TransportCredentialStore {
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
        let transport = Arc::clone(&self.transport);
        let call_key = key.clone();
        PendingCredentialOp::with_transport_cancel(
            CredentialOperation::Set,
            key,
            move |cancel| async move {
                map_transport_outcome(transport.set_cancellable(&call_key, secret, cancel).await)
            },
        )
    }

    fn delete(&self, key: CredentialKey) -> PendingCredentialOp {
        let transport = Arc::clone(&self.transport);
        let call_key = key.clone();
        PendingCredentialOp::with_transport_cancel(
            CredentialOperation::Delete,
            key,
            move |cancel| async move {
                map_transport_outcome(transport.delete_cancellable(&call_key, cancel).await)
            },
        )
    }

    fn contains(&self, key: &CredentialKey) -> CredentialPresence {
        self.transport.contains(key)
    }
}

/// Scripted transport behavior for a [`FakeSecretService`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FakeMode {
    /// The service works: items round-trip and persist.
    Available,
    /// No Secret Service on the bus — every op is [`TransportOutcome::Unavailable`].
    Unavailable,
    /// Policy refuses every op — [`TransportOutcome::PermissionDenied`].
    PermissionDenied,
    /// A locked collection whose dismissed prompt yields [`TransportOutcome::Locked`].
    LockedDismissed,
    /// Transport is broken — every op is [`TransportOutcome::Failed`].
    Failing,
}

struct FakeDaemon {
    mode: FakeMode,
    fail_reason: String,
    // Items are keyed by the full application, owner, and label attribute tuple.
    items: HashMap<Vec<(String, String)>, Secret>,
    gate: Arc<Gate>,
}

impl fmt::Debug for FakeDaemon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Expose only the mode and item count; stored keys and secret bytes remain absent from `Debug`.
        f.debug_struct("FakeDaemon")
            .field("mode", &self.mode)
            .field("item_count", &self.items.len())
            .finish()
    }
}

/// An in-memory Secret Service fake with shared daemon state and scriptable outcomes.
///
/// Fresh transports share stored items across store recreation, and Debug output never includes secret bytes.
pub struct FakeSecretService {
    daemon: Arc<Mutex<FakeDaemon>>,
}

impl fmt::Debug for FakeSecretService {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let daemon = self
            .daemon
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        fmt::Debug::fmt(&*daemon, f)
    }
}

impl Default for FakeSecretService {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeSecretService {
    /// Creates an empty available fake whose operations can run immediately.
    pub fn new() -> Self {
        Self::with_mode(FakeMode::Available)
    }

    /// Creates an empty fake in `mode` with operations enabled.
    pub fn with_mode(mode: FakeMode) -> Self {
        Self {
            daemon: Arc::new(Mutex::new(FakeDaemon {
                mode,
                fail_reason: "fake secret service failure".to_owned(),
                items: HashMap::new(),
                gate: Gate::new_open(),
            })),
        }
    }

    /// Creates an available fake whose operations wait for the returned gate.
    pub fn with_gate(gate: Arc<Gate>) -> Self {
        Self {
            daemon: Arc::new(Mutex::new(FakeDaemon {
                mode: FakeMode::Available,
                fail_reason: "fake secret service failure".to_owned(),
                items: HashMap::new(),
                gate,
            })),
        }
    }

    /// Changes the scripted mode for subsequent operations.
    pub fn set_mode(&self, mode: FakeMode) {
        self.locked_daemon().mode = mode;
    }

    /// Sets the failure diagnostic, which must not contain secret bytes.
    pub fn set_fail_reason(&self, reason: impl Into<String>) {
        self.locked_daemon().fail_reason = reason.into();
    }

    /// Returns the number of items currently held by the fake daemon.
    pub fn item_count(&self) -> usize {
        self.locked_daemon().items.len()
    }

    /// Returns a fresh transport sharing this fake’s persistent daemon state.
    pub fn transport(&self) -> Arc<dyn SecretTransport> {
        Arc::new(FakeTransport {
            daemon: Arc::clone(&self.daemon),
        })
    }

    fn locked_daemon(&self) -> std::sync::MutexGuard<'_, FakeDaemon> {
        self.daemon
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

// Cloneable client transport sharing the fake daemon.
struct FakeTransport {
    daemon: Arc<Mutex<FakeDaemon>>,
}

enum FakeOp {
    Get,
    Set(Secret),
    Delete,
}

struct FakeFuture {
    daemon: Arc<Mutex<FakeDaemon>>,
    attributes: Vec<(String, String)>,
    op: Option<FakeOp>,
}

impl Future for FakeFuture {
    type Output = TransportOutcome;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // FakeFuture holds only Unpin fields; split the borrow so the op can be
        // taken while the daemon guard is live.
        let this = self.get_mut();
        let mut daemon = this
            .daemon
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if daemon.gate.register_if_closed(cx.waker()) {
            return Poll::Pending;
        }
        let op = this.op.take().expect("FakeFuture polled after completion");
        // Reject every operation in nonavailable modes before a Set can mutate daemon state.
        let outcome = match daemon.mode {
            FakeMode::Unavailable => TransportOutcome::Unavailable,
            FakeMode::PermissionDenied => TransportOutcome::PermissionDenied,
            FakeMode::LockedDismissed => TransportOutcome::Locked,
            FakeMode::Failing => TransportOutcome::Failed(daemon.fail_reason.clone()),
            FakeMode::Available => match op {
                FakeOp::Get => match daemon.items.get(&this.attributes) {
                    Some(secret) => {
                        TransportOutcome::Found(Secret::new(secret.expose_secret().to_vec()))
                    }
                    None => TransportOutcome::NotFound,
                },
                FakeOp::Set(secret) => {
                    daemon.items.insert(this.attributes.clone(), secret);
                    TransportOutcome::Stored
                }
                FakeOp::Delete => {
                    let _ = daemon.items.remove(&this.attributes);
                    TransportOutcome::Deleted
                }
            },
        };
        Poll::Ready(outcome)
    }
}

impl FakeTransport {
    fn attributes(key: &CredentialKey) -> Vec<(String, String)> {
        key.attributes()
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn schedule(&self, key: &CredentialKey, op: FakeOp) -> TransportFuture {
        Box::pin(FakeFuture {
            daemon: Arc::clone(&self.daemon),
            attributes: Self::attributes(key),
            op: Some(op),
        })
    }
}

impl SecretTransport for FakeTransport {
    fn get(&self, key: &CredentialKey) -> TransportFuture {
        self.schedule(key, FakeOp::Get)
    }

    fn set(&self, key: &CredentialKey, secret: Secret) -> TransportFuture {
        self.schedule(key, FakeOp::Set(secret))
    }

    fn delete(&self, key: &CredentialKey) -> TransportFuture {
        self.schedule(key, FakeOp::Delete)
    }

    fn contains(&self, key: &CredentialKey) -> CredentialPresence {
        let daemon = self
            .daemon
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Only a working, unlocked service can answer cheaply. Every other
        // mode declines to guess — Unknown, never Absent.
        if daemon.mode != FakeMode::Available {
            return CredentialPresence::Unknown;
        }
        let attributes = Self::attributes(key);
        if daemon.items.contains_key(&attributes) {
            CredentialPresence::Present
        } else {
            CredentialPresence::Absent
        }
    }
}

#[cfg(test)]
mod tests;
