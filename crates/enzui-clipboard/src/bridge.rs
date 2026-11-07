//! Ordered callbacks over a bounded clipboard-transport worker queue.
//!
//! Accepted jobs run sequentially on the worker. Queue refusal calls the
//! callback synchronously on the submitting thread. Shutdown marks queued jobs
//! stopped, but they cannot run until an in-flight transport call returns.
//! A stalled call or panicking job can prevent remaining callbacks from firing.
//! Drop waits up to 500 ms before detaching; retained sender clones can keep a
//! detached worker and its transport alive.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::error::ClipboardError;
use crate::mime::MimeType;
use crate::payload::{Pasted, Payload};

const DROP_JOIN_BUDGET: Duration = Duration::from_millis(500);
const BRIDGE_QUEUE_CAPACITY: usize = 64;

/// The two operations a runtime bridge needs from the clipboard.
///
/// `WaylandClipboard` satisfies this through direct method calls. A test fake
/// satisfies it with canned results.
pub trait ClipboardTransport: Send + 'static {
    /// Read the focused seat's selection, preferring the given types.
    ///
    /// # Errors
    ///
    /// Return [`ClipboardError`] for selection, negotiation or transfer failure.
    fn paste(&self, preferences: &[MimeType]) -> Result<Pasted, ClipboardError>;

    /// Claim the focused seat's selection.
    ///
    /// # Errors
    ///
    /// Return [`ClipboardError`] when focus, claim confirmation or transport setup fails.
    fn offer(&self, payload: Payload) -> Result<(), ClipboardError>;

    /// Report whether this transport holds keyboard focus on any seat.
    ///
    /// # Errors
    ///
    /// Return [`ClipboardError`] if the current seat state cannot be obtained.
    fn has_focus(&self) -> Result<bool, ClipboardError>;

    /// Returns native drag access when this transport shares the application display.
    fn native_drag(&self) -> Option<crate::drag::NativeDragClient> {
        None
    }

    /// Read the focused seat's primary selection, preferring the given types.
    ///
    /// # Errors
    ///
    /// The default returns [`ClipboardError::NoPrimarySelection`].
    fn paste_primary(&self, preferences: &[MimeType]) -> Result<Pasted, ClipboardError> {
        let _ = preferences;
        Err(ClipboardError::NoPrimarySelection)
    }

    /// Offer `payload` as the focused seat's primary selection.
    ///
    /// # Errors
    ///
    /// The default returns [`ClipboardError::NoPrimarySelection`].
    fn offer_primary(&self, payload: Payload) -> Result<(), ClipboardError> {
        let _ = payload;
        Err(ClipboardError::NoPrimarySelection)
    }
}

/// Operation receiving either transport access or a worker-admission error.
///
/// Callbacks submitted through [`BridgeWorker::read`] or [`BridgeWorker::write`]
/// receive immediate refusal. Raw [`BridgeWorker::sender`] users must handle
/// failed sends and execute the returned job themselves if completion is required.
pub type Job = Box<dyn FnOnce(Result<&dyn ClipboardTransport, ClipboardError>) + Send>;

/// Worker with 64 queued-job slots and sequential transport access.
///
/// [`Self::read`] and [`Self::write`] reject immediately when the queue is full,
/// invoking the callback with [`ClipboardError::Busy`] on the caller's thread.
/// Callback execution itself is not bounded. Accepted callbacks run on the
/// worker; panics terminate it and discard queued work. Drop sets a stop flag
/// and waits up to 500 ms before detaching. Queued callbacks receive `Stopped`
/// only when the worker can resume and process them.
pub struct BridgeWorker {
    sender: Option<SyncSender<Job>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl BridgeWorker {
    /// Starts a worker that owns `transport`.
    ///
    /// # Errors
    ///
    /// Returns [`ClipboardError::Unavailable`] when the OS refuses to spawn
    /// the worker thread (thread-table exhaustion, restrictive `RLIMIT_NPROC`,
    /// low container pid limit).
    pub fn start(transport: impl ClipboardTransport) -> Result<Self, ClipboardError> {
        Self::start_with_gate(transport, || Ok(()))
    }

    fn start_with_gate<T: ClipboardTransport>(
        transport: T,
        gate: impl FnOnce() -> std::io::Result<()>,
    ) -> Result<Self, ClipboardError> {
        let (sender, receiver) = mpsc::sync_channel::<Job>(BRIDGE_QUEUE_CAPACITY);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);

        let thread = gate()
            .and_then(|()| {
                thread::Builder::new()
                    .name(String::from("clipboard-bridge"))
                    .spawn(move || {
                        while let Ok(job) = receiver.recv() {
                            // Shutdown must be checked after dequeue so retained jobs cannot use the transport.
                            if worker_stop.load(Ordering::Acquire) {
                                job(Err(ClipboardError::Stopped));
                                continue;
                            }
                            job(Ok(&transport));
                        }
                    })
            })
            .map_err(|error| ClipboardError::Unavailable {
                reason: format!(
                    "clipboard bridge thread could not be spawned: {}",
                    error.kind()
                ),
            })?;

        Ok(Self {
            sender: Some(sender),
            stop,
            thread: Some(thread),
        })
    }

    /// Queue a paste and call `callback` with the result.
    ///
    /// # Panics
    ///
    /// A callback invoked for immediate refusal can panic on the calling thread.
    pub fn read(
        &self,
        preferences: Vec<MimeType>,
        callback: Box<dyn FnOnce(Result<Pasted, ClipboardError>) + Send>,
    ) {
        self.submit(Box::new(move |transport| {
            callback(transport.and_then(|transport| transport.paste(&preferences)));
        }));
    }

    /// Queue an offer and call `callback` with the result.
    ///
    /// # Panics
    ///
    /// A callback invoked for immediate refusal can panic on the calling thread.
    pub fn write(
        &self,
        payload: Payload,
        callback: Box<dyn FnOnce(Result<(), ClipboardError>) + Send>,
    ) {
        self.submit(Box::new(move |transport| {
            callback(transport.and_then(|transport| transport.offer(payload)));
        }));
    }

    /// Queue a primary-selection read and call `callback` with the result.
    ///
    /// # Panics
    ///
    /// A callback invoked for immediate refusal can panic on the calling thread.
    pub fn read_primary(
        &self,
        preferences: Vec<MimeType>,
        callback: Box<dyn FnOnce(Result<Pasted, ClipboardError>) + Send>,
    ) {
        self.submit(Box::new(move |transport| {
            callback(transport.and_then(|transport| transport.paste_primary(&preferences)));
        }));
    }

    /// Queue a primary-selection offer and call `callback` with the result.
    ///
    /// # Panics
    ///
    /// A callback invoked for immediate refusal can panic on the calling thread.
    pub fn write_primary(
        &self,
        payload: Payload,
        callback: Box<dyn FnOnce(Result<(), ClipboardError>) + Send>,
    ) {
        self.submit(Box::new(move |transport| {
            callback(transport.and_then(|transport| transport.offer_primary(payload)));
        }));
    }

    /// A clone of the sender, so a global handle can submit work to the same
    /// worker without owning or moving the `BridgeWorker` itself.
    ///
    /// Raw sends bypass [`Self::read`]/[`Self::write`] refusal callbacks. A retained
    /// sender keeps the receive loop alive after shutdown until it is dropped.
    pub fn sender(&self) -> Option<SyncSender<Job>> {
        self.sender.clone()
    }

    /// Queue `job`, which receives the transport on the worker, or the
    /// refusal ([`ClipboardError::Busy`] when the queue is full,
    /// [`ClipboardError::Stopped`] after shutdown) on the calling thread. Use
    /// it to do expensive preparation, such as encoding, after admission and
    /// off the caller's thread.
    ///
    /// # Panics
    ///
    /// A job invoked for immediate refusal can panic on the calling thread.
    pub fn submit(&self, job: Job) {
        match self.sender.as_ref() {
            Some(sender) => match sender.try_send(job) {
                Ok(()) => {}
                Err(TrySendError::Full(job)) => job(Err(ClipboardError::Busy)),
                Err(TrySendError::Disconnected(job)) => job(Err(ClipboardError::Stopped)),
            },
            None => job(Err(ClipboardError::Stopped)),
        }
    }
}

impl Drop for BridgeWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        drop(self.sender.take());

        let Some(thread) = self.thread.take() else {
            return;
        };

        let deadline = Instant::now().checked_add(DROP_JOIN_BUDGET);
        while !thread.is_finished() {
            match drop_wait_step(Instant::now(), deadline, DROP_POLL_STEP) {
                DropWait::Detach => {
                    drop(thread);
                    return;
                }
                DropWait::Sleep(dur) => thread::sleep(dur),
            }
        }
        let _ = thread.join();
    }
}

const DROP_POLL_STEP: Duration = Duration::from_millis(1);

#[derive(Debug, PartialEq, Eq)]
enum DropWait {
    Detach,
    Sleep(Duration),
}

fn drop_wait_step(now: Instant, deadline: Option<Instant>, poll_step: Duration) -> DropWait {
    match deadline {
        None => DropWait::Detach,
        Some(d) if now >= d => DropWait::Detach,
        Some(d) => DropWait::Sleep(poll_step.min(d.duration_since(now))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{Receiver, SyncSender, channel, sync_channel};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    struct FakeTransport {
        paste_result: Mutex<Result<Pasted, ClipboardError>>,
        offer_result: Mutex<Result<(), ClipboardError>>,
        paste_calls: AtomicUsize,
        offer_calls: AtomicUsize,
        focused: Mutex<bool>,
    }

    impl FakeTransport {}

    impl ClipboardTransport for Arc<FakeTransport> {
        fn paste(&self, _preferences: &[MimeType]) -> Result<Pasted, ClipboardError> {
            self.paste_calls.fetch_add(1, Ordering::Relaxed);
            self.paste_result
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn offer(&self, _payload: Payload) -> Result<(), ClipboardError> {
            self.offer_calls.fetch_add(1, Ordering::Relaxed);
            self.offer_result
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }

        fn has_focus(&self) -> Result<bool, ClipboardError> {
            Ok(*self
                .focused
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner))
        }
    }

    struct ParkingTransport {
        entered: SyncSender<()>,
        gate: Arc<(Mutex<bool>, Condvar)>,
        calls: AtomicUsize,
    }

    struct ParkedFixture {
        transport: Arc<ParkingTransport>,
        entered: Receiver<()>,
        gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl ParkedFixture {
        fn new() -> Self {
            let (entered_tx, entered_rx) = sync_channel(1);
            let gate = Arc::new((Mutex::new(false), Condvar::new()));
            let transport = Arc::new(ParkingTransport {
                entered: entered_tx,
                gate: Arc::clone(&gate),
                calls: AtomicUsize::new(0),
            });
            Self {
                transport,
                entered: entered_rx,
                gate,
            }
        }
    }

    impl ParkingTransport {
        fn release(gate: &(Mutex<bool>, Condvar)) {
            let (lock, cv) = gate;
            *lock.lock().unwrap() = true;
            cv.notify_all();
        }
    }

    impl ClipboardTransport for Arc<ParkingTransport> {
        fn paste(&self, _preferences: &[MimeType]) -> Result<Pasted, ClipboardError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let _ = self.entered.try_send(());
            let (lock, cv) = &*self.gate;
            let mut released = lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while !*released {
                released = cv
                    .wait(released)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            Ok(Pasted::new(
                MimeType::new("text/plain;charset=utf-8"),
                b"released".to_vec(),
            ))
        }

        fn offer(&self, _payload: Payload) -> Result<(), ClipboardError> {
            Ok(())
        }

        fn has_focus(&self) -> Result<bool, ClipboardError> {
            Ok(true)
        }
    }

    #[test]
    fn a_saturated_bridge_refuses_one_more_job_without_running_it() {
        let ParkedFixture {
            transport,
            entered: entered_rx,
            gate,
        } = ParkedFixture::new();
        let worker = BridgeWorker::start(Arc::clone(&transport)).expect("start");

        worker.read(vec![MimeType::new("text/plain")], Box::new(|_| {}));
        entered_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("first transport call is parked");

        let queued = (0..BRIDGE_QUEUE_CAPACITY)
            .map(|_| {
                let (tx, rx) = channel();
                worker.read(
                    vec![MimeType::new("text/plain")],
                    Box::new(move |result| {
                        let _ = tx.send(result);
                    }),
                );
                rx
            })
            .collect::<Vec<_>>();

        let (refused_tx, refused_rx) = channel();
        worker.read(
            vec![MimeType::new("text/plain")],
            Box::new(move |result| {
                let _ = refused_tx.send(result);
            }),
        );
        assert_eq!(
            refused_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("capacity refusal fires its callback"),
            Err(ClipboardError::Busy),
        );
        assert_eq!(transport.calls.load(Ordering::Relaxed), 1);

        ParkingTransport::release(&gate);
        for callback in queued {
            callback
                .recv_timeout(Duration::from_secs(5))
                .expect("admitted callback fires")
                .expect("admitted paste succeeds");
        }
        assert_eq!(
            transport.calls.load(Ordering::Relaxed),
            BRIDGE_QUEUE_CAPACITY + 1,
        );
    }
}
