//! One renderer-free owner for reducer tasks across background and GUI delivery.
//!
//! [`TaskHost`] runs cooperative asynchronous work on one owned thread and emits
//! typed reducer messages through the same queue before and after GUI promotion.
//! It retains at most 64 submitted task streams and a bounded output queue;
//! inner task batches and message payload sizes remain the application's budget.
//! [`TaskHost::has_work`] includes queued tasks and messages not yet consumed.
//!
//! Widget operations and repaint hints have no surface in background mode and
//! are discarded. Font and image actions prepare the renderer the GUI will
//! need, and system actions ask the platform runtime (theme, reduced motion,
//! system information): they are deferred, not refused. The stream stays alive waiting for
//! their result but stops counting as work, and [`TaskHost::take_deferred`]
//! hands them to the GUI runtime when the window opens, which completes them
//! and lets the stream continue. Window, clipboard, backend, exit and
//! synthetic event actions are refused and abort the submitted stream,
//! including chained continuations. Presenting a review window requires
//! an explicit application transition outside this executor. Futures must yield;
//! blocking I/O belongs on the application's bounded work lane.

use super::iced_consumer::{MessageEvents, MessageReceiveError, message_subscription};
use iced_futures::Subscription;
use iced_futures::futures::{
    Stream, StreamExt,
    stream::{BoxStream, SelectAll},
};
use iced_runtime::{Action, Task};
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::task::{Context, Poll};
use std::time::Duration;

/// Maximum submitted task streams retained by one host, including queued work.
pub const TASK_CAPACITY: usize = 64;
const MESSAGE_CAPACITY: usize = 64;

/// A task could not run or attempted an action outside the headless contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskHostError {
    /// All bounded task slots are occupied; the rejected task is dropped.
    Full,
    /// The worker has stopped or the event receiver has closed.
    Stopped,
    /// A GUI subscription already owns event delivery.
    InUse,
    /// A desktop action was refused without being executed.
    Unsupported(&'static str),
    /// The timer runtime or worker thread could not start.
    Spawn(String),
}

impl std::fmt::Display for TaskHostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => f.write_str("task host is full"),
            Self::Stopped => f.write_str("task host stopped"),
            Self::InUse => f.write_str("GUI subscription owns task delivery"),
            Self::Unsupported(kind) => write!(f, "headless task refused {kind} action"),
            Self::Spawn(error) => write!(f, "task host startup failed: {error}"),
        }
    }
}
impl std::error::Error for TaskHostError {}

// A unit of work counted by `has_work`. Dropping it releases the count, wakes
// the worker when it was a delivered message, and signals `work_changed`.
struct Lease(
    Arc<AtomicUsize>,
    Option<Arc<tokio::sync::Notify>>,
    Arc<tokio::sync::Notify>,
);
impl Drop for Lease {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
        if let Some(wake) = &self.1 {
            wake.notify_one();
        }
        self.2.notify_one();
    }
}

struct Job<M> {
    stream: Option<BoxStream<'static, Action<M>>>,
    // Released when the stream parks on a deferred renderer action: it waits
    // for the GUI, not for work, so it must not keep a background process up.
    lease: Option<Lease>,
    deferred: Arc<std::sync::Mutex<Vec<Action<Infallible>>>>,
}
impl<M> Stream for Job<M> {
    type Item = Result<M, TaskHostError>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let job = self.get_mut();
        for _ in 0..64 {
            let Some(stream) = job.stream.as_mut() else {
                return Poll::Ready(None);
            };
            let action = match stream.as_mut().poll_next(cx) {
                Poll::Ready(Some(action)) => action,
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            };
            let kind = match action {
                Action::Output(message) => return Poll::Ready(Some(Ok(message))),
                Action::Widget(_)
                | Action::WidgetIn { .. }
                | Action::Tick
                | Action::Reload
                | Action::Invalidate(_) => continue,
                Action::Font(action) => {
                    job.defer(Action::Font(action));
                    continue;
                }
                Action::Image(action) => {
                    job.defer(Action::Image(action));
                    continue;
                }
                Action::System(action) => {
                    job.defer(Action::System(action));
                    continue;
                }
                Action::Window(_) => "window",
                Action::Clipboard(_) => "clipboard",
                Action::Backend(_) => "backend",
                Action::Event { .. } => "synthetic event",
                Action::Exit => "exit",
            };
            job.stream = None;
            return Poll::Ready(Some(Err(TaskHostError::Unsupported(kind))));
        }
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

impl<M> Job<M> {
    fn defer(&mut self, action: Action<Infallible>) {
        self.deferred
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(action);
        self.lease = None;
    }
}

#[derive(Clone)]
struct Envelope<M> {
    value: Result<M, TaskHostError>,
    _lease: Arc<Lease>,
}

/// A cooperative task executor whose queue survives GUI subscription startup.
pub struct TaskHost<M: Clone + Send + 'static> {
    commands: tokio::sync::mpsc::Sender<Job<M>>,
    events: MessageEvents<Envelope<M>>,
    tasks: Arc<AtomicUsize>,
    messages: Arc<AtomicUsize>,
    changed: Arc<tokio::sync::Notify>,
    deferred: Arc<std::sync::Mutex<Vec<Action<Infallible>>>>,
    stop: Arc<AtomicBool>,
    stop_signal: tokio::sync::watch::Sender<bool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl<M: Clone + Send + 'static> TaskHost<M> {
    /// Starts one timer-capable worker without opening a display or renderer.
    ///
    /// # Errors
    /// Returns runtime or thread startup failures.
    pub fn spawn() -> Result<Self, TaskHostError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| TaskHostError::Spawn(error.to_string()))?;
        let (commands, receiver) = tokio::sync::mpsc::channel(TASK_CAPACITY);
        let (sender, events) = mpsc::sync_channel(MESSAGE_CAPACITY);
        let tasks = Arc::new(AtomicUsize::new(0));
        let messages = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let changed = Arc::new(tokio::sync::Notify::new());
        let worker_changed = Arc::clone(&changed);
        let worker_messages = Arc::clone(&messages);
        let worker_stop = Arc::clone(&stop);
        let (stop_signal, stop_wait) = tokio::sync::watch::channel(false);
        let worker = std::thread::Builder::new()
            .name("enzui-app-tasks".into())
            .spawn(move || {
                runtime.block_on(run(
                    receiver,
                    sender,
                    worker_messages,
                    worker_changed,
                    worker_stop,
                    stop_wait,
                ));
            })
            .map_err(|error| TaskHostError::Spawn(error.to_string()))?;
        Ok(Self {
            commands,
            events: MessageEvents::new(events),
            tasks,
            messages,
            changed,
            deferred: Arc::new(std::sync::Mutex::new(Vec::new())),
            stop,
            stop_signal,
            worker: Some(worker),
        })
    }

    /// Submits a reducer task without blocking the caller.
    ///
    /// # Errors
    /// Refuses a full or stopped host. Refusal drops the task and its resources.
    pub fn submit(&self, task: Task<M>) -> Result<(), TaskHostError> {
        let Some(stream) = iced_runtime::task::into_stream(task) else {
            return Ok(());
        };
        if self.stop.load(Ordering::Acquire) {
            return Err(TaskHostError::Stopped);
        }
        self.tasks
            .try_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < TASK_CAPACITY).then_some(count + 1)
            })
            .map_err(|_| TaskHostError::Full)?;
        let job = Job {
            stream: Some(stream),
            lease: Some(Lease(
                Arc::clone(&self.tasks),
                None,
                Arc::clone(&self.changed),
            )),
            deferred: Arc::clone(&self.deferred),
        };
        self.commands.try_send(job).map_err(|error| match error {
            tokio::sync::mpsc::error::TrySendError::Full(_) => TaskHostError::Full,
            tokio::sync::mpsc::error::TrySendError::Closed(_) => TaskHostError::Stopped,
        })
    }

    /// Whether tasks or unconsumed task messages still belong to this host.
    ///
    /// Combine with live center calls, windows and app-owned operations before
    /// idle exit. This does not count detached work created by an inner future.
    #[must_use]
    pub fn has_work(&self) -> bool {
        self.tasks.load(Ordering::Acquire) != 0 || self.messages.load(Ordering::Acquire) != 0
    }

    /// Renderer and platform actions (font and image loads, theme and system
    /// queries) that background work asked for,
    /// as one task for the GUI runtime to run when the window opens.
    ///
    /// The streams that asked are still waiting for these results; running the
    /// returned task completes them, and their messages then arrive through
    /// [`Self::subscription`] as usual. Taken once: a second call returns the
    /// actions deferred since.
    pub fn take_deferred<T: Send + 'static>(&self) -> Task<T> {
        let actions = std::mem::take(
            &mut *self
                .deferred
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        Task::batch(actions.into_iter().map(iced_runtime::task::effect))
    }

    /// Whether renderer actions are waiting for the GUI.
    #[must_use]
    pub fn has_deferred(&self) -> bool {
        !self
            .deferred
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    /// Resolves after [`Self::has_work`] may have changed: a task finished or a
    /// delivered message was released.
    ///
    /// A message reaches the application slightly before its delivery lease is
    /// released, so a caller that decides "idle" from [`Self::has_work`] must
    /// wait on this as well as on messages, or it can block with the host
    /// already idle. One change may be reported once to a later caller.
    pub fn work_changed(&self) -> impl Future<Output = ()> + Send + 'static {
        let changed = Arc::clone(&self.changed);
        async move { changed.notified().await }
    }

    /// Receives a task result while no GUI subscription owns the queue.
    ///
    /// # Errors
    /// Returns refused actions, stopped delivery or an active GUI consumer.
    pub fn poll(&self, timeout: Duration) -> Result<Option<M>, TaskHostError> {
        match self.events.recv_timeout(timeout) {
            Ok(envelope) => envelope.value.map(Some),
            Err(MessageReceiveError::Timeout) => Ok(None),
            Err(MessageReceiveError::Closed) => Err(TaskHostError::Stopped),
            Err(MessageReceiveError::InUse) => Err(TaskHostError::InUse),
        }
    }

    /// Delivers the same queued results after the application opens its GUI.
    ///
    /// Keep offering the same subscription id each turn. Starting it transfers
    /// delivery custody; running tasks and pending messages remain owned here.
    pub fn subscription<Message, F>(&self, id: u64, map: F) -> Subscription<Message>
    where
        Message: Send + 'static,
        F: Fn(Result<M, TaskHostError>) -> Message + Send + 'static,
    {
        message_subscription(&self.events, id, move |envelope| map(envelope.value))
    }
}

impl<M: Clone + Send + 'static> Drop for TaskHost<M> {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.stop_signal.send(true);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

async fn run<M: Clone + Send + 'static>(
    mut commands: tokio::sync::mpsc::Receiver<Job<M>>,
    sender: mpsc::SyncSender<Envelope<M>>,
    messages: Arc<AtomicUsize>,
    changed: Arc<tokio::sync::Notify>,
    stop: Arc<AtomicBool>,
    mut stop_wait: tokio::sync::watch::Receiver<bool>,
) {
    let mut jobs = SelectAll::<Job<M>>::new();
    let mut pending = None;
    let available = Arc::new(tokio::sync::Notify::new());
    while !stop.load(Ordering::Acquire) {
        if let Some(envelope) = pending.take() {
            match sender.try_send(envelope) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(envelope)) => pending = Some(envelope),
                Err(mpsc::TrySendError::Disconnected(_)) => break,
            }
        }
        tokio::select! {
            command = commands.recv() => match command {
                Some(job) => jobs.push(job),
                None => break,
            },
            value = jobs.next(), if pending.is_none() && !jobs.is_empty() => {
                if let Some(value) = value {
                    messages.fetch_add(1, Ordering::AcqRel);
                    pending = Some(Envelope { value, _lease: Arc::new(Lease(Arc::clone(&messages), Some(Arc::clone(&available)), Arc::clone(&changed))) });
                }
            },
            () = available.notified(), if pending.is_some() => {},
            _ = stop_wait.changed() => break,
        }
    }
}

#[cfg(test)]
#[path = "task_host_tests.rs"]
mod tests;
