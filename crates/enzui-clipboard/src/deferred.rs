//! Text promised by a focused Copy activation and fulfilled after an asynchronous request.
//!
//! [`DeferredTextSession::prepare`] creates an offer and a one-shot fulfiller.
//! Offer the payload through the ordinary clipboard, await successful native
//! ownership confirmation, then fetch and fulfil the text. Losing focus after
//! confirmation does not require a new selection claim. Dropping the fulfiller
//! cancels an unfinished request. Session invalidation cancels pending requests;
//! text already fulfilled follows its selection lifetime and supersession.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use crate::{ClipboardError, Payload};

/// Maximum deferred UTF-8 payload, independent of the transport's possibly smaller limit.
pub const MAX_DEFERRED_TEXT_BYTES: usize = 32 * 1024 * 1024;
/// Maximum lifetime from preparation to source revocation.
pub const MAX_DEFERRED_TEXT_LIFETIME: Duration = Duration::from_secs(120);
const MAX_PENDING: usize = 8;

/// Byte and lifetime bounds for one deferred selection.
#[derive(Debug, Clone, Copy)]
pub struct DeferredTextLimits {
    /// Positive UTF-8 byte ceiling, at most [`MAX_DEFERRED_TEXT_BYTES`].
    pub max_bytes: usize,
    /// Positive lifetime, at most [`MAX_DEFERRED_TEXT_LIFETIME`], including claim time.
    pub lifetime: Duration,
}

#[derive(Default)]
struct Session {
    generation: u64,
    closed: bool,
    sources: Vec<Weak<Shared>>,
}

#[derive(Default)]
struct Owner(Mutex<Session>);

/// Owns a remote/session generation and at most eight pending deferred copies.
///
/// Dropping or invalidating this owner cancels unfinished copies. It does not
/// revoke text whose asynchronous request completed before invalidation.
#[derive(Default)]
pub struct DeferredTextSession {
    owner: Arc<Owner>,
}

impl fmt::Debug for DeferredTextSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DeferredTextSession")
            .finish_non_exhaustive()
    }
}

impl DeferredTextSession {
    /// Prepare an unclaimed text source for this session generation.
    ///
    /// Native `offer` must succeed before the returned fulfiller accepts text.
    /// Preparation does not change the clipboard or revoke an existing copy.
    ///
    /// # Errors
    /// Refuses invalid limits, closed generations and more than eight pending copies.
    pub fn prepare(
        &self,
        limits: DeferredTextLimits,
    ) -> Result<(Payload, DeferredText), ClipboardError> {
        if limits.max_bytes == 0 || limits.max_bytes > MAX_DEFERRED_TEXT_BYTES {
            return Err(ClipboardError::TransferTooLarge {
                limit: MAX_DEFERRED_TEXT_BYTES,
            });
        }
        if limits.lifetime.is_zero() || limits.lifetime > MAX_DEFERRED_TEXT_LIFETIME {
            return Err(ClipboardError::Unavailable {
                reason: "deferred clipboard lifetime must be positive and at most 120 seconds"
                    .into(),
            });
        }
        let expires = Instant::now()
            .checked_add(limits.lifetime)
            .ok_or(ClipboardError::Stopped)?;
        let mut owner = self
            .owner
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if owner.closed {
            return Err(ClipboardError::Stopped);
        }
        owner
            .sources
            .retain(|source| source.upgrade().is_some_and(|source| source.pending()));
        if owner
            .sources
            .iter()
            .filter_map(Weak::upgrade)
            .filter(|source| source.pending())
            .count()
            >= MAX_PENDING
        {
            return Err(ClipboardError::Busy);
        }
        let source = Arc::new(Shared {
            phase: Mutex::new(Phase::Unclaimed),
            #[cfg(test)]
            waiting: AtomicBool::new(false),
            changed: Condvar::new(),
            wake: Mutex::new(None),
            owner: Arc::downgrade(&self.owner),
            generation: owner.generation,
            limits,
            expires,
        });
        owner.sources.push(Arc::downgrade(&source));
        let payload = Payload::deferred(Arc::clone(&source));
        Ok((
            payload,
            DeferredText {
                guard: Arc::new(Fulfiller { source }),
            },
        ))
    }

    /// Cancel pending work when the application's remote/session generation changes.
    pub fn invalidate_pending(&self) {
        let mut owner = self
            .owner
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(next) = owner.generation.checked_add(1) {
            owner.generation = next;
        } else {
            owner.closed = true;
        }
        for source in owner.sources.iter().filter_map(Weak::upgrade) {
            source.cancel_pending();
        }
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        let owner = self
            .0
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for source in owner.sources.iter().filter_map(Weak::upgrade) {
            source.cancel_pending();
        }
    }
}

enum Phase {
    Unclaimed,
    Claimed,
    Ready(Arc<[u8]>),
    Revoked,
}

pub(crate) struct Shared {
    #[cfg(test)]
    pub(crate) waiting: AtomicBool,
    phase: Mutex<Phase>,
    changed: Condvar,
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    owner: Weak<Owner>,
    generation: u64,
    pub(crate) limits: DeferredTextLimits,
    pub(crate) expires: Instant,
}

impl Shared {
    fn notify(&self) {
        self.changed.notify_all();
        if let Some(wake) = self
            .wake
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            wake();
        }
    }

    pub(crate) fn install_wake(
        &self,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), ClipboardError> {
        let mut slot = self
            .wake
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if slot.is_some() {
            return Err(ClipboardError::Unavailable {
                reason: "a deferred source can be offered only once".into(),
            });
        }
        *slot = Some(wake);
        Ok(())
    }

    fn pending(&self) -> bool {
        Instant::now() < self.expires
            && matches!(
                *self
                    .phase
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                Phase::Unclaimed | Phase::Claimed
            )
    }

    pub(crate) fn revoked(&self) -> bool {
        Instant::now() >= self.expires
            || matches!(
                *self
                    .phase
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
                Phase::Revoked
            )
    }

    pub(crate) fn revoke(&self) {
        *self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Phase::Revoked;
        self.notify();
    }

    fn cancel_pending(&self) {
        let mut phase = self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*phase, Phase::Unclaimed | Phase::Claimed) {
            *phase = Phase::Revoked;
        }
        drop(phase);
        self.notify();
    }

    pub(crate) fn claimed(&self) -> Result<(), ClipboardError> {
        let owner = self.owner.upgrade().ok_or(ClipboardError::Stopped)?;
        let owner = owner
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut phase = self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if owner.generation != self.generation
            || owner.closed
            || Instant::now() >= self.expires
            || matches!(*phase, Phase::Revoked)
        {
            return Err(ClipboardError::Stopped);
        }
        if matches!(*phase, Phase::Unclaimed) {
            *phase = Phase::Claimed;
        }
        drop(phase);
        self.changed.notify_all();
        Ok(())
    }

    pub(crate) fn wait(
        &self,
        active: &AtomicBool,
        budget: Duration,
    ) -> Result<(Arc<[u8]>, Instant), ClipboardError> {
        let deadline = Instant::now()
            .checked_add(budget)
            .ok_or(ClipboardError::TransferStalled { after: budget })?
            .min(self.expires);
        let mut phase = self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if !active.load(Ordering::Acquire) || matches!(*phase, Phase::Revoked) {
                return Err(ClipboardError::Stopped);
            }
            if Instant::now() >= deadline {
                return Err(ClipboardError::TransferStalled { after: budget });
            }
            if let Phase::Ready(bytes) = &*phase {
                return Ok((Arc::clone(bytes), deadline));
            }
            #[cfg(test)]
            self.waiting.store(true, Ordering::Release);
            let pause = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(50));
            phase = self
                .changed
                .wait_timeout(phase, pause)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }
}

/// One-shot permission to fulfil a successfully claimed deferred selection.
///
/// Clones share one completion. Debug output is redacted. Dropping the last
/// unfinished handle revokes its source.
#[derive(Clone)]
pub struct DeferredText {
    guard: Arc<Fulfiller>,
}

struct Fulfiller {
    source: Arc<Shared>,
}

impl fmt::Debug for DeferredText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DeferredText([redacted])")
    }
}

impl DeferredText {
    /// Publish UTF-8 bytes through the existing confirmed selection, without claiming again.
    ///
    /// # Errors
    /// Refuses unconfirmed, superseded, expired or invalidated requests and text over its byte limit.
    pub fn fulfil(self, text: String) -> Result<(), ClipboardError> {
        if text.len() > self.guard.source.limits.max_bytes {
            return Err(ClipboardError::TransferTooLarge {
                limit: self.guard.source.limits.max_bytes,
            });
        }
        let owner = self
            .guard
            .source
            .owner
            .upgrade()
            .ok_or(ClipboardError::Stopped)?;
        let owner = owner
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut phase = self
            .guard
            .source
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if owner.closed
            || owner.generation != self.guard.source.generation
            || Instant::now() >= self.guard.source.expires
            || matches!(*phase, Phase::Revoked)
        {
            return Err(ClipboardError::Stopped);
        }
        if !matches!(*phase, Phase::Claimed) {
            return Err(ClipboardError::Unavailable {
                reason: "deferred clipboard selection has not been confirmed".into(),
            });
        }
        *phase = Phase::Ready(text.into_bytes().into());
        drop(phase);
        self.guard.source.changed.notify_all();
        Ok(())
    }

    /// Revoke this pending source and wake blocked transfer requests.
    pub fn cancel(self) {
        self.guard.source.revoke();
    }
}

impl Drop for Fulfiller {
    fn drop(&mut self) {
        self.source.cancel_pending();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> DeferredTextLimits {
        DeferredTextLimits {
            max_bytes: 128,
            lifetime: Duration::from_secs(5),
        }
    }

    #[test]
    fn deferred_sessions_bound_pending_work_and_release_it_on_invalidation() {
        let session = DeferredTextSession::default();
        let pending: Vec<_> = (0..MAX_PENDING)
            .map(|_| session.prepare(limits()).unwrap())
            .collect();
        assert!(matches!(
            session.prepare(limits()),
            Err(ClipboardError::Busy)
        ));
        session.invalidate_pending();
        for (payload, fulfil) in pending {
            assert!(payload.confirm_deferred().is_err());
            assert!(fulfil.fulfil("stale".into()).is_err());
        }
        assert!(session.prepare(limits()).is_ok());
    }

    #[test]
    fn deferred_clones_share_one_completion_and_last_drop_cancels_pending() {
        let session = DeferredTextSession::default();
        let (payload, fulfil) = session.prepare(limits()).unwrap();
        let other = fulfil.clone();
        drop(fulfil);
        payload.confirm_deferred().unwrap();
        other.fulfil("text".into()).unwrap();
        assert!(
            payload
                .deferred_source()
                .unwrap()
                .wait(&AtomicBool::new(true), Duration::from_secs(1))
                .is_ok()
        );
        let (pending, fulfil) = session.prepare(limits()).unwrap();
        drop(fulfil);
        assert!(pending.confirm_deferred().is_err());
    }

    #[test]
    fn deferred_text_requires_native_confirmation_and_a_live_owner() {
        let session = DeferredTextSession::default();
        let (_, fulfil) = session.prepare(limits()).unwrap();
        assert!(fulfil.fulfil("before claim".into()).is_err());
        let (payload, fulfil) = session.prepare(limits()).unwrap();
        payload.confirm_deferred().unwrap();
        drop(session);
        assert!(fulfil.fulfil("after owner drop".into()).is_err());
        for limits in [
            DeferredTextLimits {
                max_bytes: 0,
                ..limits()
            },
            DeferredTextLimits {
                max_bytes: MAX_DEFERRED_TEXT_BYTES + 1,
                ..limits()
            },
            DeferredTextLimits {
                lifetime: Duration::ZERO,
                ..limits()
            },
            DeferredTextLimits {
                lifetime: MAX_DEFERRED_TEXT_LIFETIME + Duration::from_secs(1),
                ..limits()
            },
        ] {
            assert!(DeferredTextSession::default().prepare(limits).is_err());
        }
    }
    #[test]
    fn deferred_fulfiller_refuses_expiry_without_waiting_for_native_cleanup() {
        let session = DeferredTextSession::default();
        let (payload, fulfil) = session
            .prepare(DeferredTextLimits {
                max_bytes: 128,
                lifetime: Duration::from_millis(20),
            })
            .unwrap();
        payload.confirm_deferred().unwrap();
        std::thread::sleep(Duration::from_millis(40));
        assert!(fulfil.fulfil("too late".into()).is_err());
    }
}
