//! Bounded handoff from a blocking worker to an asynchronous UI stream.
//!
//! Progress and telemetry each retain only their newest value, while lifecycle
//! events use a bounded, backpressured FIFO. Finishing preserves the latest
//! samples before an optional final lifecycle event and one terminal outcome.
//!
//! Receiver closure wakes blocked producers. Owners that join a worker must
//! arrange for [`WorkerBridgeCloseGuard`] to drop before the join guard.

use iced_futures::futures::Stream;
use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

const FINISH_SLOTS: usize = 4;

struct State<T> {
    lifecycle: VecDeque<T>,
    latest_progress: Option<T>,
    latest_telemetry: Option<T>,
    final_lifecycle: Option<T>,
    terminal: Option<T>,
    prefer_telemetry: bool,
    receiver_open: bool,
    sealed: bool,
    producers: usize,
    waker: Option<Waker>,
}

impl<T> State<T> {
    fn retained(&self) -> usize {
        self.lifecycle.len()
            + usize::from(self.latest_progress.is_some())
            + usize::from(self.latest_telemetry.is_some())
            + usize::from(self.final_lifecycle.is_some())
            + usize::from(self.terminal.is_some())
    }

    fn take_latest(&mut self) -> Option<T> {
        if self.prefer_telemetry {
            if let Some(value) = self.latest_telemetry.take() {
                self.prefer_telemetry = false;
                return Some(value);
            }
            let value = self.latest_progress.take();
            if value.is_some() {
                self.prefer_telemetry = true;
            }
            value
        } else {
            if let Some(value) = self.latest_progress.take() {
                self.prefer_telemetry = true;
                return Some(value);
            }
            let value = self.latest_telemetry.take();
            if value.is_some() {
                self.prefer_telemetry = false;
            }
            value
        }
    }
}

struct Shared<T> {
    state: Mutex<State<T>>,
    room: Condvar,
    lifecycle_capacity: usize,
}

/// A synchronous producer for a bounded worker-to-async bridge.
///
/// Clones are supported when a worker has separate progress and telemetry
/// callbacks. Dropping the last producer ends the stream after retained values
/// drain, even if the worker unwinds before calling [`Self::finish`].
pub struct WorkerSender<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Clone for WorkerSender<T> {
    fn clone(&self) -> Self {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.producers += 1;
        drop(state);
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> Drop for WorkerSender<T> {
    fn drop(&mut self) {
        let wake = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state.producers = state.producers.saturating_sub(1);
            (state.producers == 0).then(|| state.waker.take()).flatten()
        };
        if let Some(waker) = wake {
            waker.wake();
        }
    }
}

impl<T> WorkerSender<T> {
    /// Replace the retained progress value without blocking the worker.
    ///
    /// Returns the value when the receiver is closed or the bridge is already
    /// sealed by [`Self::finish`].
    ///
    /// # Errors
    ///
    /// The value, when the receiver is closed or the bridge is sealed.
    pub fn send_progress(&self, value: T) -> Result<(), T> {
        self.send_latest(value, LatestSlot::Progress)
    }

    /// Replace the retained telemetry value without blocking the worker.
    ///
    /// Telemetry and progress use independent slots. Returns the value when the
    /// receiver is closed or the bridge has been sealed by [`Self::finish`].
    ///
    /// # Errors
    ///
    /// The value, when the receiver is closed or the bridge is sealed.
    pub fn send_telemetry(&self, value: T) -> Result<(), T> {
        self.send_latest(value, LatestSlot::Telemetry)
    }

    fn send_latest(&self, value: T, slot: LatestSlot) -> Result<(), T> {
        let wake = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !state.receiver_open || state.sealed {
                return Err(value);
            }
            match slot {
                LatestSlot::Progress => state.latest_progress = Some(value),
                LatestSlot::Telemetry => state.latest_telemetry = Some(value),
            }
            debug_assert!(
                state.retained() <= self.retained_capacity(),
                "retained results exceed the bridge capacity"
            );
            state.waker.take()
        };
        if let Some(waker) = wake {
            waker.wake();
        }
        Ok(())
    }

    /// Append one ordered lifecycle value, blocking while the fixed FIFO is full.
    ///
    /// Returns the value if the receiver or a close handle closes the bridge, or if
    /// [`Self::finish`] seals it before the value is enqueued. Closing before
    /// joining wakes a worker blocked here.
    ///
    /// # Errors
    ///
    /// The value, when the bridge closes or is sealed before it is enqueued.
    pub fn send_lifecycle(&self, value: T) -> Result<(), T> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        while state.receiver_open
            && !state.sealed
            && state.lifecycle.len() == self.shared.lifecycle_capacity
        {
            state = self
                .shared
                .room
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        if !state.receiver_open || state.sealed {
            return Err(value);
        }
        state.lifecycle.push_back(value);
        debug_assert!(
            state.retained() <= self.retained_capacity(),
            "retained results exceed the bridge capacity"
        );
        let wake = state.waker.take();
        drop(state);
        if let Some(waker) = wake {
            waker.wake();
        }
        Ok(())
    }

    /// Seal the bridge with its ordered end-of-attempt records.
    ///
    /// Queued lifecycle values drain first, followed by retained progress and
    /// telemetry, `final_lifecycle`, and then `terminal`. The terminal value is
    /// yielded once before the stream ends; later sends are rejected, and this
    /// method never waits for FIFO capacity.
    ///
    /// # Errors
    ///
    /// Returns `terminal` if the receiver is closed or the bridge is already
    /// sealed. The optional final lifecycle value is not recoverable on error.
    pub fn finish(&self, final_lifecycle: Option<T>, terminal: T) -> Result<(), T> {
        let wake = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !state.receiver_open || state.sealed {
                return Err(terminal);
            }
            state.final_lifecycle = final_lifecycle;
            state.terminal = Some(terminal);
            state.sealed = true;
            debug_assert!(
                state.retained() <= self.retained_capacity(),
                "retained results exceed the bridge capacity"
            );
            state.waker.take()
        };
        self.shared.room.notify_all();
        if let Some(waker) = wake {
            waker.wake();
        }
        Ok(())
    }

    /// Current number of values retained by this bridge.
    #[must_use]
    pub fn retained_len(&self) -> usize {
        self.shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retained()
    }

    /// Maximum number of values this bridge can retain.
    ///
    /// This is the lifecycle FIFO capacity plus independent progress,
    /// telemetry, final-lifecycle, and terminal slots.
    #[must_use]
    pub fn retained_capacity(&self) -> usize {
        self.shared.lifecycle_capacity + FINISH_SLOTS
    }
}

#[derive(Clone, Copy)]
enum LatestSlot {
    Progress,
    Telemetry,
}

/// The asynchronous receiving half of a worker bridge.
///
/// Queued lifecycle values are yielded first. The newest progress and newest
/// telemetry samples follow in fair alternating order based on which kind was
/// delivered previously; neither kind is permanently preferred. Both retained
/// latest samples precede the optional final lifecycle value and terminal
/// outcome. Dropping the receiver closes the bridge and wakes every blocked
/// lifecycle producer.
pub struct WorkerReceiver<T> {
    shared: Arc<Shared<T>>,
}

impl<T> WorkerReceiver<T> {
    /// Obtain a clonable handle that can close this receiver during teardown.
    #[must_use]
    pub fn close_handle(&self) -> WorkerBridgeClose<T> {
        WorkerBridgeClose {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Close the bridge, discard retained values, and wake blocked producers.
    pub fn close(&self) {
        close(&self.shared);
    }
}

impl<T> Drop for WorkerReceiver<T> {
    fn drop(&mut self) {
        self.close();
    }
}

impl<T> Stream for WorkerReceiver<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let value = state
            .lifecycle
            .pop_front()
            .or_else(|| state.take_latest())
            .or_else(|| state.final_lifecycle.take())
            .or_else(|| state.terminal.take());
        if let Some(value) = value {
            self.shared.room.notify_one();
            return Poll::Ready(Some(value));
        }
        if !state.receiver_open || state.sealed || state.producers == 0 {
            return Poll::Ready(None);
        }
        if state
            .waker
            .as_ref()
            .is_none_or(|waker| !waker.will_wake(context.waker()))
        {
            state.waker = Some(context.waker().clone());
        }
        Poll::Pending
    }
}

/// Clonable cancellation handle for a [`WorkerReceiver`].
///
/// Closing is idempotent. It is intentionally separate from the receiver so a
/// drop guard can close a saturated bridge before joining the worker thread.
pub struct WorkerBridgeClose<T> {
    shared: Arc<Shared<T>>,
}

impl<T> Clone for WorkerBridgeClose<T> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T> WorkerBridgeClose<T> {
    /// Close the bridge, discard retained values, and wake blocked producers.
    pub fn close(&self) {
        close(&self.shared);
    }

    /// Convert this handle into an RAII guard that closes on drop.
    ///
    /// Declare the worker's join guard first and this guard second. Rust's
    /// reverse local drop order will then close the bridge before joining.
    #[must_use]
    pub fn into_guard(self) -> WorkerBridgeCloseGuard<T> {
        WorkerBridgeCloseGuard(self)
    }
}

/// RAII bridge closer for cancellation-safe worker teardown.
///
/// Obtain one with [`WorkerBridgeClose::into_guard`]. Its drop is idempotent
/// with explicit receiver closure.
pub struct WorkerBridgeCloseGuard<T>(WorkerBridgeClose<T>);

impl<T> Drop for WorkerBridgeCloseGuard<T> {
    fn drop(&mut self) {
        self.0.close();
    }
}

fn close<T>(shared: &Shared<T>) {
    let wake = {
        let mut state = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
        if !state.receiver_open {
            return;
        }
        state.receiver_open = false;
        state.lifecycle.clear();
        state.latest_progress = None;
        state.latest_telemetry = None;
        state.final_lifecycle = None;
        state.terminal = None;
        state.waker.take()
    };
    shared.room.notify_all();
    if let Some(waker) = wake {
        waker.wake();
    }
}

/// Create a bounded synchronous-worker to asynchronous-consumer bridge.
///
/// `lifecycle_capacity` bounds the lossless FIFO and must be nonzero. The total
/// retained-value bound is [`WorkerSender::retained_capacity`]. Progress and
/// telemetry sends never block; lifecycle sends backpressure at the supplied
/// capacity.
#[must_use]
pub fn channel<T>(
    lifecycle_capacity: NonZeroUsize,
) -> (WorkerSender<T>, WorkerReceiver<T>, WorkerBridgeClose<T>) {
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            lifecycle: VecDeque::with_capacity(lifecycle_capacity.get()),
            latest_progress: None,
            latest_telemetry: None,
            final_lifecycle: None,
            terminal: None,
            prefer_telemetry: false,
            receiver_open: true,
            sealed: false,
            producers: 1,
            waker: None,
        }),
        room: Condvar::new(),
        lifecycle_capacity: lifecycle_capacity.get(),
    });
    let receiver = WorkerReceiver {
        shared: Arc::clone(&shared),
    };
    let close = receiver.close_handle();
    (WorkerSender { shared }, receiver, close)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced_futures::futures::Stream;
    use std::task::{Context, Poll};

    #[derive(Debug, PartialEq, Eq)]
    enum Event {
        Lifecycle(u32),
        Progress(u32),
        Telemetry(u32),
        Final,
        Terminal,
    }

    fn next(receiver: &mut WorkerReceiver<Event>) -> Poll<Option<Event>> {
        let waker = iced_futures::futures::task::noop_waker();
        let mut context = Context::from_waker(&waker);
        Pin::new(receiver).poll_next(&mut context)
    }

    #[test]
    fn blocked_consumer_retains_latest_samples_and_one_ordered_terminal() {
        const UPDATES: u32 = 10_000;
        let (sender, mut receiver, _) = channel(NonZeroUsize::new(3).unwrap());

        for lifecycle in 1..=3 {
            sender.send_lifecycle(Event::Lifecycle(lifecycle)).unwrap();
        }
        for value in 1..=UPDATES {
            sender.send_progress(Event::Progress(value)).unwrap();
            sender.send_telemetry(Event::Telemetry(value)).unwrap();
            assert!(sender.retained_len() <= sender.retained_capacity());
        }
        sender.finish(Some(Event::Final), Event::Terminal).unwrap();

        assert_eq!(sender.retained_capacity(), 7);
        assert_eq!(sender.retained_len(), 7);
        for lifecycle in 1..=3 {
            assert_eq!(
                next(&mut receiver),
                Poll::Ready(Some(Event::Lifecycle(lifecycle)))
            );
        }
        assert_eq!(
            next(&mut receiver),
            Poll::Ready(Some(Event::Progress(UPDATES)))
        );
        assert_eq!(
            next(&mut receiver),
            Poll::Ready(Some(Event::Telemetry(UPDATES)))
        );
        assert_eq!(next(&mut receiver), Poll::Ready(Some(Event::Final)));
        assert_eq!(next(&mut receiver), Poll::Ready(Some(Event::Terminal)));
        assert_eq!(next(&mut receiver), Poll::Ready(None));

        assert!(sender.send_progress(Event::Progress(UPDATES + 1)).is_err());
        assert_eq!(next(&mut receiver), Poll::Ready(None));
    }
}
