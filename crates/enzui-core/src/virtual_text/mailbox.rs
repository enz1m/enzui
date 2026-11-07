//! Bounded, cancellable mutation requests between accessibility or IME callers and the application reducer.
//!
//! Admitted requests receive one distinct completion result, and reply locks are never held while reducer code runs.

use super::{
    CompositionId, VirtualDocumentId, VirtualRevision, VirtualSelection, VirtualTextCancellation,
    VirtualTextError,
};
use std::collections::VecDeque;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

/// Maximum pending mutation requests before the mailbox refuses new ones.
pub const DEFAULT_MUTATION_MAILBOX_CAPACITY: usize = 64;
/// Maximum time a synchronous native request may wait for the application
/// reducer. A missing or wedged reducer therefore fails instead of parking an
/// accessibility worker forever.
pub const DEFAULT_MUTATION_REPLY_TIMEOUT: Duration = Duration::from_secs(2);
// AccessKit Unix cancellation has no wake hook, so it must be polled.
const EXTERNAL_CANCELLATION_POLL: Duration = Duration::from_millis(10);

/// The reducer-owned operation carried by a [`MutationRequest`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MutationKind {
    /// Change the semantic selection without changing bytes or revision.
    SetSelection(VirtualSelection),
    /// Replace a complete-document scalar range.
    Replace {
        /// Character range joined to the request's expected revision.
        range: Range<u64>,
        /// Replacement value.
        value: String,
    },
    /// Commit one provider-owned IME composition.
    CommitComposition {
        /// Composition ownership token.
        id: CompositionId,
        /// Character range captured when composition began.
        range: Range<u64>,
        /// Last preedit value.
        value: String,
    },
}

/// A mutation request enqueued by an AT or IME caller. Identity, revision and
/// cancellation travel together to the reducer; pre-enqueue checks are only an
/// optimization and never authorize a drain-time write.
#[derive(Clone, Debug)]
pub struct MutationRequest {
    pub(super) document: VirtualDocumentId,
    pub(super) revision: VirtualRevision,
    pub(super) cancellation: VirtualTextCancellation,
    pub(super) mutation: MutationKind,
}

impl MutationRequest {
    /// Stable document identity this request targeted.
    pub fn document(&self) -> VirtualDocumentId {
        self.document
    }

    /// Revision this request joined against.
    pub fn revision(&self) -> VirtualRevision {
        self.revision
    }

    /// Requested operation.
    pub fn mutation(&self) -> &MutationKind {
        &self.mutation
    }

    /// Returns whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
}

/// The reducer's answer to one [`MutationRequest`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutationReply {
    /// The mutation was applied; here is the new state.
    Accepted {
        /// Revision after the mutation.
        revision: VirtualRevision,
        /// Selection after the mutation.
        selection: VirtualSelection,
    },
    /// Stale revision: the document changed between enqueue and drain.
    Stale {
        /// Requested revision.
        requested: VirtualRevision,
        /// Current revision.
        current: VirtualRevision,
    },
    /// The reducer refused the mutation for application-level policy or materialization.
    Refused,
}

pub(super) type MutationResponse = Result<MutationReply, VirtualTextError>;

// The reply lock is released before reducer work; `admitted` prevents cancellation from winning after ownership transfers.
#[derive(Debug, Default)]
pub(super) struct ReplySlot {
    response: Option<MutationResponse>,
    admitted: bool,
}

#[derive(Debug)]
pub(super) struct ReplyState {
    slot: Mutex<ReplySlot>,
    ready: Condvar,
    deadline: Instant,
}

impl ReplyState {
    fn lock(&self) -> std::sync::MutexGuard<'_, ReplySlot> {
        self.slot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn complete(&self, response: MutationResponse) -> bool {
        let mut slot = self.lock();
        if slot.response.is_some() {
            return false;
        }
        slot.response = Some(response);
        self.ready.notify_all();
        true
    }

    // Cancellation completes only pre-admission requests; after admission, the waiter can still time out.
    pub(super) fn cancel_unless_admitted(&self) -> bool {
        let mut slot = self.lock();
        if slot.response.is_some() || slot.admitted {
            return false;
        }
        slot.response = Some(Err(VirtualTextError::Cancelled));
        self.ready.notify_all();
        true
    }

    pub(super) fn response(&self) -> Option<MutationResponse> {
        self.lock().response
    }
}

/// A waitable handle returned to the request owner.
#[derive(Clone, Debug)]
pub struct MutationTicket {
    reply: Arc<ReplyState>,
    cancellation: VirtualTextCancellation,
}

impl MutationTicket {
    /// Wait until the reducer answers, cancellation wins, or the configured
    /// deadline expires. This method never performs an unbounded condvar wait.
    ///
    /// # Errors
    ///
    /// The reducer's refusal, cancellation, or the deadline passing.
    pub fn wait(&self) -> MutationResponse {
        self.wait_after_lock(|| {})
    }

    pub(super) fn wait_after_lock(&self, mut before_park: impl FnMut()) -> MutationResponse {
        let mut slot = self.reply.lock();
        loop {
            if let Some(response) = slot.response {
                return response;
            }
            // Cancellation loses against a reducer that already admitted this
            // request, but the deadline below never does.
            if !slot.admitted && self.cancellation.is_cancelled() {
                slot.response = Some(Err(VirtualTextError::Cancelled));
                self.reply.ready.notify_all();
                return Err(VirtualTextError::Cancelled);
            }
            let now = Instant::now();
            if now >= self.reply.deadline {
                // Deadlines remain enforceable during reducer work; late replies are rejected.
                slot.response = Some(Err(VirtualTextError::Timeout));
                self.reply.ready.notify_all();
                return Err(VirtualTextError::Timeout);
            }
            let remaining = self.reply.deadline.saturating_duration_since(now);
            let poll = remaining.min(EXTERNAL_CANCELLATION_POLL);
            before_park();
            let (next, _) = self
                .reply
                .ready
                .wait_timeout(slot, poll)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            slot = next;
        }
    }

    /// Checks for a response without blocking.
    pub fn try_recv(&self) -> Option<MutationResponse> {
        self.reply.response()
    }
}

/// Exclusive ownership of one accepted queue entry. Dropping this value
/// without replying resolves its ticket as cancelled, including reducer panic
/// unwinding and early-return paths.
#[must_use = "every drained mutation must be resolved"]
#[derive(Debug)]
pub struct PendingMutation {
    request: MutationRequest,
    reply: Arc<ReplyState>,
}

impl PendingMutation {
    /// Request metadata and operation to validate on the reducer thread.
    pub fn request(&self) -> &MutationRequest {
        &self.request
    }

    /// Returns whether neither cancellation, timeout, nor another completion has won.
    pub fn is_live(&self) -> bool {
        self.reply.response().is_none()
            && !self.request.is_cancelled()
            && Instant::now() < self.reply.deadline
    }

    /// Runs `apply` only after atomically admitting a live request, without holding the reply lock during application work.
    ///
    /// Returns `true` only when the answer wins completion. Cancellation cannot win
    /// after admission, but deadline expiration or mailbox closure can complete first.
    /// A panic drops the guard and marks an otherwise unanswered request cancelled.
    pub fn resolve_with(self, apply: impl FnOnce(&MutationRequest) -> MutationReply) -> bool {
        {
            let mut slot = self.reply.lock();
            if slot.response.is_some() {
                return false;
            }
            if self.request.is_cancelled() {
                slot.response = Some(Err(VirtualTextError::Cancelled));
                self.reply.ready.notify_all();
                return false;
            }
            if Instant::now() >= self.reply.deadline {
                slot.response = Some(Err(VirtualTextError::Timeout));
                self.reply.ready.notify_all();
                return false;
            }
            slot.admitted = true;
        }
        let answer = apply(&self.request);
        self.reply.complete(Ok(answer))
    }
}

impl Drop for PendingMutation {
    fn drop(&mut self) {
        let _ = self.reply.complete(Err(VirtualTextError::Cancelled));
    }
}

#[derive(Debug)]
struct MailboxState {
    closed: bool,
    queue: VecDeque<PendingMutation>,
    // Weakly tracks every unanswered reply, including requests already drained by a reducer.
    active_replies: Vec<Weak<ReplyState>>,
}

/// Bounded, cancellable mutation mailbox between native accessibility or IME callers and the application reducer.
#[derive(Debug)]
pub struct MutationMailbox {
    state: Mutex<MailboxState>,
    capacity: usize,
    reply_timeout: Duration,
    pub(super) enqueued: AtomicU64,
    refused_full: AtomicU64,
    refused_closed: AtomicU64,
    wake: Mutex<Option<Weak<dyn Fn() + Send + Sync>>>,
}

/// Strong lifecycle owner for the mailbox wake callback.
#[must_use = "dropping the registration disconnects the wake"]
pub struct MailboxWakeRegistration {
    _wake: Arc<dyn Fn() + Send + Sync>,
}

impl std::fmt::Debug for MailboxWakeRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailboxWakeRegistration")
            .finish_non_exhaustive()
    }
}

impl MutationMailbox {
    /// Create a new mailbox with the default bounded reply deadline.
    pub fn new(capacity: usize) -> Self {
        Self::with_reply_timeout(capacity, DEFAULT_MUTATION_REPLY_TIMEOUT)
    }

    /// Creates a mailbox with the specified reply deadline.
    pub fn with_reply_timeout(capacity: usize, reply_timeout: Duration) -> Self {
        Self {
            state: Mutex::new(MailboxState {
                closed: false,
                queue: VecDeque::with_capacity(capacity.min(256)),
                active_replies: Vec::with_capacity(capacity.min(256)),
            }),
            capacity,
            reply_timeout,
            enqueued: AtomicU64::new(0),
            refused_full: AtomicU64::new(0),
            refused_closed: AtomicU64::new(0),
            wake: Mutex::new(None),
        }
    }

    fn increment(counter: &AtomicU64) {
        let _ = counter.try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
            Some(value.saturating_add(1))
        });
    }

    /// Install a wake callback invoked after each successful enqueue.
    pub fn set_wake<F>(&self, wake: F) -> MailboxWakeRegistration
    where
        F: Fn() + Send + Sync + 'static,
    {
        let strong = Arc::new(wake) as Arc<dyn Fn() + Send + Sync>;
        let weak = Arc::downgrade(&strong);
        if let Ok(mut slot) = self.wake.lock() {
            *slot = Some(weak);
        }
        MailboxWakeRegistration { _wake: strong }
    }

    /// Enqueue a mutation request. Admission and close are serialized by one
    /// lock, so close cannot drain and then have an older enqueue appear.
    ///
    /// # Errors
    ///
    /// A [`VirtualTextError`] on cancellation, a closed or full mailbox, or an unavailable reducer.
    pub fn enqueue(&self, request: MutationRequest) -> Result<MutationTicket, VirtualTextError> {
        if request.is_cancelled() {
            return Err(VirtualTextError::Cancelled);
        }
        let deadline = Instant::now()
            .checked_add(self.reply_timeout)
            .unwrap_or_else(Instant::now);
        let reply = Arc::new(ReplyState {
            slot: Mutex::new(ReplySlot::default()),
            ready: Condvar::new(),
            deadline,
        });
        let ticket = MutationTicket {
            reply: Arc::clone(&reply),
            cancellation: request.cancellation.clone(),
        };
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| VirtualTextError::Unavailable)?;
            if state.closed {
                Self::increment(&self.refused_closed);
                return Err(VirtualTextError::MailboxClosed);
            }
            if request.is_cancelled() {
                return Err(VirtualTextError::Cancelled);
            }
            if state.queue.len() >= self.capacity {
                Self::increment(&self.refused_full);
                return Err(VirtualTextError::MailboxFull);
            }
            state.active_replies.retain(|active| {
                active
                    .upgrade()
                    .is_some_and(|reply| reply.response().is_none())
            });
            state.active_replies.push(Arc::downgrade(&reply));
            state.queue.push_back(PendingMutation { request, reply });
            Self::increment(&self.enqueued);
        }
        ticket.cancellation.register(&ticket.reply);
        // Wake only after releasing the queue lock: a callback may drain
        // synchronously without re-entering the admission lock.
        let wake = self
            .wake
            .lock()
            .ok()
            .and_then(|slot| slot.as_ref().and_then(Weak::upgrade));
        if let Some(wake) = wake {
            wake();
        }
        Ok(ticket)
    }

    /// Drain all pending requests on the reducer thread. Every returned guard
    /// replies exactly once or cancels its waiter from `Drop`.
    pub fn drain(&self) -> Vec<PendingMutation> {
        self.state
            .lock()
            .map(|mut state| state.queue.drain(..).collect())
            .unwrap_or_default()
    }

    /// Close permanently and release every request still owned by the queue.
    pub fn close(&self) {
        let (pending, active_replies) = match self.state.lock() {
            Ok(mut state) => {
                state.closed = true;
                (
                    state.queue.drain(..).collect::<Vec<_>>(),
                    std::mem::take(&mut state.active_replies),
                )
            }
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.closed = true;
                (
                    state.queue.drain(..).collect::<Vec<_>>(),
                    std::mem::take(&mut state.active_replies),
                )
            }
        };
        // Completion happens after releasing the mailbox lock. This includes
        // requests a reducer already drained but has not yet resolved.
        for reply in active_replies
            .into_iter()
            .filter_map(|reply| reply.upgrade())
        {
            let _ = reply.complete(Err(VirtualTextError::MailboxClosed));
        }
        drop(pending);
    }

    /// Returns whether the mailbox is closed, treating a poisoned state lock as closed.
    pub fn is_closed(&self) -> bool {
        self.state.lock().map_or(true, |state| state.closed)
    }

    /// Returns the queued depth, or zero if the state lock is poisoned.
    pub fn pending(&self) -> usize {
        self.state.lock().map_or(0, |state| state.queue.len())
    }

    /// Total successful enqueues, saturating rather than wrapping.
    pub fn enqueued(&self) -> u64 {
        self.enqueued.load(Ordering::Relaxed)
    }

    /// Total enqueues refused because the queue was full.
    pub fn refused_full(&self) -> u64 {
        self.refused_full.load(Ordering::Relaxed)
    }

    /// Total enqueues refused because the mailbox was closed.
    pub fn refused_closed(&self) -> u64 {
        self.refused_closed.load(Ordering::Relaxed)
    }
}

impl Drop for MutationMailbox {
    fn drop(&mut self) {
        self.close();
    }
}
