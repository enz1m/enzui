//! A native event loop that must run on the application's own thread (a GLib
//! main context, a toolkit's dispatcher), woken by its descriptors and
//! deadlines instead of a polling timer.
//!
//! The owner thread does the native work; a watcher thread only waits. Each
//! round goes:
//!
//! 1. On the owner thread, ask the native loop what it waits for (GLib:
//!    `g_main_context_prepare` and `g_main_context_query`) and
//!    [`arm`](NativeLoop::arm) with those descriptors and that deadline.
//! 2. The watcher polls them. When one is ready, or the deadline passes, it
//!    sends a single [`NativeReady`] message through
//!    [`subscription`](NativeLoop::subscription), then waits for the next
//!    `arm`: a descriptor that stays readable until dispatched cannot flood
//!    the application.
//! 3. The message reaches `update` on the owner thread, which dispatches the
//!    native work (GLib: `g_main_context_check` and `g_main_context_dispatch`)
//!    and arms again. When the dispatch published nothing the window shows,
//!    the update returns [`quiet`], so no view is rebuilt or redrawn.
//!
//! A host that publishes frames (an external-image source) should take
//! readiness through a [`NativeLoop::dispatcher`] mounted in its window
//! instead of step 3: the watcher wakes that window, which runs the dispatch
//! on the owner thread during its own event handling. No message reaches
//! `update`, so the view is not rebuilt and a published frame is shown with
//! only the image prepared again; the dispatch returns a message only when
//! the application must hear about a change.
//!
//! The watcher holds only descriptor numbers, never a native object, so
//! nothing native is created, used or destroyed off the owner thread.
//! Dropping the [`NativeLoop`] stops the watcher. A descriptor the native
//! loop closes may be invalid or reused by the next poll. The owner must re-arm
//! after changing descriptors; polling never reads, writes or closes them.
//!
//! ```no_run
//! # use enzui_core::native_loop::{Interest, NativeLoop, NativeReady, quiet};
//! # struct Engine;
//! # impl Engine {
//! #     fn query(&mut self) -> Interest { Interest::new() }
//! #     fn dispatch(&mut self) -> bool { false }
//! #     fn use_polling(&mut self) -> iced_runtime::Task<Message> { iced_runtime::Task::none() }
//! # }
//! # #[derive(Debug, Clone)] enum Message { Native(NativeReady) }
//! struct App { engine: Engine, native: NativeLoop }
//!
//! impl App {
//!     fn update(&mut self, message: Message) -> iced_runtime::Task<Message> {
//!         match message {
//!             // No watcher thread: fall back to polling the engine on a timer.
//!             Message::Native(NativeReady::Unavailable) => self.engine.use_polling(),
//!             Message::Native(_) => {
//!                 let published = self.engine.dispatch();
//!                 self.native.arm(self.engine.query());
//!                 if published { iced_runtime::Task::none() } else { quiet() }
//!             }
//!         }
//!     }
//!
//!     fn subscription(&self) -> iced_futures::Subscription<Message> {
//!         self.native.subscription(Message::Native)
//!     }
//! }
//! ```

use iced_futures::futures::channel::mpsc;
use iced_futures::futures::{Stream, StreamExt};
use rustix::event::{EventfdFlags, eventfd};
use std::hash::{Hash, Hasher};
use std::os::fd::{AsFd, AsRawFd, OwnedFd, RawFd};
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

/// What the native loop waits for: descriptors with `poll(2)` event bits,
/// and a deadline.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Interest {
    descriptors: Vec<(RawFd, i16)>,
    deadline: Option<Instant>,
}

impl Interest {
    /// Nothing yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Wake when `fd` is readable.
    #[must_use]
    pub fn readable(self, fd: RawFd) -> Self {
        self.descriptor(fd, libc::POLLIN)
    }

    /// Wake when `fd` reports any of `events`, `poll(2)` bits (GLib's
    /// `GPollFD.events` uses the same values).
    #[must_use]
    pub fn descriptor(mut self, fd: RawFd, events: i16) -> Self {
        self.descriptors.push((fd, events));
        self
    }

    /// Wake at `deadline` at the latest.
    #[must_use]
    pub fn deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(match self.deadline {
            Some(earlier) => earlier.min(deadline),
            None => deadline,
        });
        self
    }

    /// Wake after `timeout` at the latest (GLib's `query` timeout, when not
    /// negative).
    #[must_use]
    pub fn timeout(self, timeout: Duration) -> Self {
        self.deadline(Instant::now() + timeout)
    }
}

/// Why the watcher woke the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeReady {
    /// A descriptor is ready.
    Descriptors,
    /// The deadline passed.
    Deadline,
    /// The watcher could not start; the host must select another dispatch path.
    Unavailable,
}

#[derive(Debug, Default)]
struct State {
    /// Set by `arm`, taken by the watcher.
    armed: Option<Interest>,
    closed: bool,
    /// The windows a mounted [`Dispatcher`] receives readiness in, by
    /// dispatcher instance.
    routes: Vec<(u64, iced_core::shell::Waker)>,
    /// Readiness waiting for a dispatcher to take it.
    pending: Option<NativeReady>,
}

#[derive(Debug)]
struct Shared {
    id: u64,
    state: Mutex<State>,
    /// Interrupts the watcher's poll when the interest changes or the loop
    /// closes.
    wake: OwnedFd,
}

impl Shared {
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn interrupt(&self) {
        let _ = rustix::io::write(&self.wake, &1u64.to_ne_bytes());
    }
}

/// A native loop's waits, watched off the owner thread; see the
/// [module documentation](self).
#[derive(Debug)]
pub struct NativeLoop {
    shared: Arc<Shared>,
}

impl NativeLoop {
    /// A loop with nothing armed yet.
    ///
    /// # Errors
    /// When the process cannot create the event descriptor the watcher is
    /// interrupted through.
    pub fn new() -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let wake = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?;
        Ok(Self {
            shared: Arc::new(Shared {
                id: NEXT.fetch_add(1, Ordering::Relaxed),
                state: Mutex::new(State::default()),
                wake,
            }),
        })
    }

    /// Watch `interest` until it is ready, then send one [`NativeReady`].
    /// Replaces whatever was armed before. Call it on the owner thread after
    /// each dispatch.
    pub fn arm(&self, interest: Interest) {
        self.shared.state().armed = Some(interest);
        self.shared.interrupt();
    }

    /// One readiness per [`arm`](Self::arm), or [`NativeReady::Unavailable`]
    /// if the watcher cannot start. Select a fallback when startup fails.
    pub fn subscription<Message>(
        &self,
        on_ready: fn(NativeReady) -> Message,
    ) -> iced_futures::Subscription<Message>
    where
        Message: Send + 'static,
    {
        iced_futures::Subscription::run_with(
            (Key(Arc::clone(&self.shared)), on_ready),
            |(key, on_ready)| watch(key).map(*on_ready),
        )
    }
}

impl NativeLoop {
    /// A zero-size widget that takes this loop's readiness on the owner
    /// thread while it is mounted, and runs `dispatch` there, without an
    /// application message.
    ///
    /// The watcher wakes the window the dispatcher is in, and the window
    /// hands the readiness to `dispatch` during its own event handling: no
    /// `update`, no view rebuild and no redraw unless something asks for one.
    /// A frame published through an external-image source is shown with
    /// only the image prepared again. `dispatch` does the native work, arms
    /// again, and returns a message only when the application must hear
    /// about a change (a title, focus, a fallback); that message reaches
    /// `update` as usual.
    ///
    /// The [`subscription`](Self::subscription) still runs the watcher, and
    /// delivers readiness whenever no dispatcher is mounted.
    pub fn dispatcher<'a, Message>(
        &self,
        dispatch: impl Fn(NativeReady) -> Option<Message> + 'a,
    ) -> Dispatcher<'a, Message> {
        Dispatcher {
            shared: Arc::clone(&self.shared),
            dispatch: Box::new(dispatch),
        }
    }
}

/// Runs a [`NativeLoop`]'s dispatch on the owner thread; see
/// [`NativeLoop::dispatcher`].
pub struct Dispatcher<'a, Message> {
    shared: Arc<Shared>,
    dispatch: Box<dyn Fn(NativeReady) -> Option<Message> + 'a>,
}

impl<Message> std::fmt::Debug for Dispatcher<'_, Message> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dispatcher").finish_non_exhaustive()
    }
}

/// A mounted dispatcher's route; unmounting it removes the route.
struct Route {
    instance: u64,
    shared: Option<Arc<Shared>>,
}

impl Default for Route {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            instance: NEXT.fetch_add(1, Ordering::Relaxed),
            shared: None,
        }
    }
}

impl Route {
    fn leave(&mut self) {
        if let Some(shared) = self.shared.take() {
            let instance = self.instance;
            shared.state().routes.retain(|(id, _)| *id != instance);
            // The watcher hands a readiness nobody took to the subscription.
            shared.interrupt();
        }
    }
}

impl Drop for Route {
    fn drop(&mut self) {
        self.leave();
    }
}

impl<Message, Theme, Renderer> iced_core::Widget<Message, Theme, Renderer>
    for Dispatcher<'_, Message>
where
    Renderer: iced_core::Renderer,
{
    fn tag(&self) -> iced_core::widget::tree::Tag {
        iced_core::widget::tree::Tag::of::<Route>()
    }

    fn state(&self) -> iced_core::widget::tree::State {
        iced_core::widget::tree::State::new(Route::default())
    }

    fn size(&self) -> iced_core::Size<iced_core::Length> {
        iced_core::Size::new(iced_core::Length::Fixed(0.0), iced_core::Length::Fixed(0.0))
    }

    fn layout(
        &mut self,
        tree: &mut iced_core::widget::Tree,
        _renderer: &Renderer,
        limits: &iced_core::layout::Limits,
    ) {
        iced_core::layout::Node::new(limits.resolve(
            iced_core::Length::Fixed(0.0),
            iced_core::Length::Fixed(0.0),
            iced_core::Size::ZERO,
        ))
        .apply(tree);
    }

    fn update(
        &mut self,
        tree: &mut iced_core::widget::Tree,
        _event: &iced_core::Event,
        _layout: iced_core::Layout,
        _cursor: iced_core::mouse::Cursor,
        _renderer: &Renderer,
        shell: &mut iced_core::Shell<'_, Message>,
        _viewport: &iced_core::Rectangle,
    ) {
        let route = tree.state.downcast_mut::<Route>();
        if !route
            .shared
            .as_ref()
            .is_some_and(|shared| Arc::ptr_eq(shared, &self.shared))
        {
            route.leave();
            let mut state = self.shared.state();
            state.routes.push((route.instance, shell.waker().clone()));
            drop(state);
            route.shared = Some(Arc::clone(&self.shared));
        }
        let ready = self.shared.state().pending.take();
        if let Some(message) = ready.and_then(|ready| (self.dispatch)(ready)) {
            shell.publish(message);
        }
    }

    fn draw(
        &self,
        _tree: &iced_core::widget::Tree,
        _renderer: &mut Renderer,
        _theme: &Theme,
        _style: &iced_core::renderer::Style,
        _layout: iced_core::Layout,
        _cursor: iced_core::mouse::Cursor,
        _viewport: &iced_core::Rectangle,
    ) {
    }
}

impl<'a, Message: 'a, Theme, Renderer> From<Dispatcher<'a, Message>>
    for iced_core::Element<'a, Message, Theme, Renderer>
where
    Renderer: iced_core::Renderer,
{
    fn from(dispatcher: Dispatcher<'a, Message>) -> Self {
        Self::new(dispatcher)
    }
}

impl Drop for NativeLoop {
    fn drop(&mut self) {
        self.shared.state().closed = true;
        self.shared.interrupt();
    }
}

/// Returned by an update that dispatched native work but changed nothing the
/// window shows: no view is rebuilt and nothing is redrawn.
pub fn quiet<Message>() -> iced_runtime::Task<Message> {
    iced_runtime::window::invalidate(iced_runtime::window::Invalidation::none())
}

/// A subscription's identity: one watcher per loop.
struct Key(Arc<Shared>);

impl Hash for Key {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.id.hash(state);
    }
}

/// Starts the watcher for `key` and returns its messages.
fn watch(key: &Key) -> Readiness {
    watch_with_spawn(key, |shared, sender| {
        std::thread::Builder::new()
            .name("enzui-native-loop".into())
            .spawn(move || watcher(&shared, &sender))
            .map(|_| ())
    })
}

fn watch_with_spawn(
    key: &Key,
    spawn: impl FnOnce(Arc<Shared>, mpsc::UnboundedSender<NativeReady>) -> std::io::Result<()>,
) -> Readiness {
    let shared = Arc::clone(&key.0);
    let (sender, receiver) = mpsc::unbounded();
    let failed = sender.clone();
    if let Err(error) = spawn(Arc::clone(&shared), sender) {
        log::warn!("native loop: the watcher thread could not start: {error}");
        let _ = failed.unbounded_send(NativeReady::Unavailable);
    }
    Readiness { receiver, shared }
}

/// The watcher's messages; dropping it (the subscription went away) stops
/// the watcher.
struct Readiness {
    receiver: mpsc::UnboundedReceiver<NativeReady>,
    shared: Arc<Shared>,
}

impl Stream for Readiness {
    type Item = NativeReady;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<NativeReady>> {
        self.receiver.poll_next_unpin(context)
    }
}

impl Drop for Readiness {
    fn drop(&mut self) {
        self.receiver.close();
        self.shared.interrupt();
    }
}

/// Waits for what is armed, sends one message when it is ready, and waits
/// to be armed again. Returns when the loop closes or nobody listens.
fn watcher(shared: &Shared, sender: &mpsc::UnboundedSender<NativeReady>) {
    let mut current: Option<Interest> = None;
    loop {
        {
            let mut state = shared.state();
            if state.closed || sender.is_closed() {
                return;
            }
            if let Some(fresh) = state.armed.take() {
                current = Some(fresh);
            }
            // The last dispatcher went away before taking its readiness.
            if state.routes.is_empty()
                && let Some(ready) = state.pending.take()
                && sender.unbounded_send(ready).is_err()
            {
                return;
            }
        }
        let woke = match &current {
            None => wait(shared, &[], None),
            Some(interest) => wait(shared, &interest.descriptors, interest.deadline),
        };
        if let Some(ready) = woke {
            current = None;
            // A mounted dispatcher takes it on the owner thread without an
            // application message; otherwise the subscription delivers it.
            let wakers = {
                let mut state = shared.state();
                if state.routes.is_empty() {
                    None
                } else {
                    state.pending = Some(ready);
                    Some(
                        state
                            .routes
                            .iter()
                            .map(|(_, waker)| waker.clone())
                            .collect::<Vec<_>>(),
                    )
                }
            };
            match wakers {
                Some(wakers) => {
                    for waker in wakers {
                        waker.wake();
                    }
                }
                None => {
                    if sender.unbounded_send(ready).is_err() {
                        return;
                    }
                }
            }
        }
    }
}

/// Polls the interrupt descriptor and `descriptors` until something is
/// ready or `deadline` passes. `None` when interrupted.
fn wait(
    shared: &Shared,
    descriptors: &[(RawFd, i16)],
    deadline: Option<Instant>,
) -> Option<NativeReady> {
    let timeout = deadline.map(|deadline| {
        let left = deadline.saturating_duration_since(Instant::now());
        libc::timespec {
            tv_sec: libc::time_t::try_from(left.as_secs()).unwrap_or(libc::time_t::MAX),
            tv_nsec: libc::c_long::from(left.subsec_nanos()),
        }
    });
    let mut fds = Vec::with_capacity(descriptors.len() + 1);
    fds.push(libc::pollfd {
        fd: shared.wake.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    });
    for &(fd, events) in descriptors {
        fds.push(libc::pollfd {
            fd,
            events,
            revents: 0,
        });
    }
    #[allow(
        unsafe_code,
        reason = "ppoll observes foreign descriptor numbers without claiming their lifetime"
    )]
    // SAFETY: `fds` and the optional timeout outlive the call, and their
    // lengths are what is passed. Descriptor numbers are plain integers to
    // the kernel, not Rust borrows: a closed or reused number reports
    // `POLLNVAL` or early readiness, and a negative one is skipped.
    let result = unsafe {
        libc::ppoll(
            fds.as_mut_ptr(),
            fds.len() as libc::nfds_t,
            timeout
                .as_ref()
                .map_or(std::ptr::null(), std::ptr::from_ref),
            std::ptr::null(),
        )
    };
    match result {
        0 => return deadline.map(|_| NativeReady::Deadline),
        value if value < 0 => {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                return None;
            }
            log::warn!("native loop: poll failed: {error}");
            return Some(NativeReady::Descriptors);
        }
        _ => {}
    }
    // Interrupted: the interest may have been replaced, so what the old one
    // reported is stale. The next pass polls whatever is armed now.
    if fds[0].revents != 0 {
        let mut count = [0u8; 8];
        let _ = rustix::io::read(shared.wake.as_fd(), &mut count);
        return None;
    }
    if fds[1..].iter().any(|fd| fd.revents != 0) {
        Some(NativeReady::Descriptors)
    } else if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        Some(NativeReady::Deadline)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    /// The next message within `within`, or `None`.
    fn next(stream: &mut Readiness, within: Duration) -> Option<NativeReady> {
        let until = Instant::now() + within;
        let waker = std::task::Waker::noop();
        let mut context = Context::from_waker(waker);
        loop {
            if let Poll::Ready(item) = Pin::new(&mut *stream).poll_next(&mut context) {
                return item;
            }
            if Instant::now() >= until {
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn pipe() -> (OwnedFd, OwnedFd) {
        rustix::pipe::pipe().expect("pipe")
    }

    #[test]
    fn native_readiness_rearms_and_follows_dispatcher_ownership() {
        // negative_descriptor_numbers_do_not_kill_the_watcher
        {
            let native = NativeLoop::new().unwrap();
            let mut stream = watch(&Key(Arc::clone(&native.shared)));
            native.arm(
                Interest::new()
                    .readable(-1)
                    .readable(-99)
                    .timeout(Duration::from_millis(10)),
            );
            assert_eq!(
                next(&mut stream, Duration::from_secs(2)),
                Some(NativeReady::Deadline)
            );
        }

        // a_ready_descriptor_wakes_the_owner_once_per_arm
        {
            let native = NativeLoop::new().unwrap();
            let mut stream = watch(&Key(Arc::clone(&native.shared)));
            let (read, write) = pipe();
            native.arm(Interest::new().readable(read.as_raw_fd()));
            assert_eq!(
                next(&mut stream, Duration::from_millis(80)),
                None,
                "nothing is ready, nothing wakes"
            );
            let _ = rustix::io::write(&write, b"x");
            assert_eq!(
                next(&mut stream, Duration::from_secs(2)),
                Some(NativeReady::Descriptors)
            );
            // Still readable (not drained), but not armed again: no flood.
            assert_eq!(next(&mut stream, Duration::from_millis(80)), None);
            native.arm(Interest::new().readable(read.as_raw_fd()));
            assert_eq!(
                next(&mut stream, Duration::from_secs(2)),
                Some(NativeReady::Descriptors),
                "armed again, it wakes again"
            );
        }

        // a_mounted_dispatcher_takes_readiness_without_an_application_message
        {
            let native = NativeLoop::new().unwrap();
            let mut stream = watch(&Key(Arc::clone(&native.shared)));
            let (read, write) = pipe();
            let dispatched = std::cell::Cell::new(0);
            let publish = std::cell::Cell::new(false);
            let mut ui = window(native.dispatcher(|ready| {
                assert_eq!(ready, NativeReady::Descriptors);
                dispatched.set(dispatched.get() + 1);
                publish.get().then_some(7)
            }));
            let (waker, wakes) = counting();
            // Mounting registers the window's route.
            assert!(deliver(&mut ui, &waker, iced_core::Event::Waken).is_empty());

            native.arm(Interest::new().readable(read.as_raw_fd()));
            let _ = rustix::io::write(&write, b"x");
            assert!(
                until(|| wakes.load(Ordering::Relaxed) == 1),
                "the window is woken"
            );
            assert_eq!(
                next(&mut stream, Duration::from_millis(80)),
                None,
                "no message"
            );
            assert!(
                deliver(&mut ui, &waker, iced_core::Event::Waken).is_empty(),
                "nothing published"
            );
            assert_eq!(dispatched.get(), 1, "dispatched on the owner thread");
            assert!(deliver(&mut ui, &waker, iced_core::Event::Waken).is_empty());
            assert_eq!(dispatched.get(), 1, "once per readiness");

            // A dispatch that reports a change publishes its message.
            publish.set(true);
            native.arm(Interest::new().readable(read.as_raw_fd()));
            assert!(until(|| wakes.load(Ordering::Relaxed) == 2));
            assert_eq!(deliver(&mut ui, &waker, iced_core::Event::Waken), vec![7]);
            assert_eq!(dispatched.get(), 2);
        }

        // an_unmounted_dispatcher_returns_readiness_to_the_subscription
        {
            let native = NativeLoop::new().unwrap();
            let mut stream = watch(&Key(Arc::clone(&native.shared)));
            let (read, write) = pipe();
            let mut ui = window(native.dispatcher(|_| None));
            let (waker, wakes) = counting();
            let _ = deliver(&mut ui, &waker, iced_core::Event::Waken);

            native.arm(Interest::new().readable(read.as_raw_fd()));
            let _ = rustix::io::write(&write, b"x");
            assert!(until(|| wakes.load(Ordering::Relaxed) == 1));
            // The window closes before it takes the readiness.
            drop(ui);
            assert_eq!(
                next(&mut stream, Duration::from_secs(2)),
                Some(NativeReady::Descriptors),
                "the pending readiness reaches the subscription"
            );

            native.arm(Interest::new().readable(read.as_raw_fd()));
            assert_eq!(
                next(&mut stream, Duration::from_secs(2)),
                Some(NativeReady::Descriptors),
                "later readiness goes to the subscription"
            );
            assert_eq!(
                wakes.load(Ordering::Relaxed),
                1,
                "the closed window is not woken"
            );
        }
    }

    type Ui<'a> = iced_runtime::UserInterface<'a, u8, iced_core::Theme, ()>;

    /// A window holding only `dispatcher`, as the runtime builds it.
    fn window<'a>(dispatcher: Dispatcher<'a, u8>) -> Ui<'a> {
        let column = iced_core::Element::<'a, u8, iced_core::Theme, ()>::from(dispatcher);
        Ui::build(
            column,
            iced_core::Size::new(100.0, 100.0),
            iced_runtime::user_interface::Cache::default(),
            &mut (),
        )
    }

    /// Delivers `event` to `ui` as the window does, woken through `waker`;
    /// returns the application messages it published.
    fn deliver(
        ui: &mut Ui<'_>,
        waker: &iced_core::shell::Waker,
        event: iced_core::Event,
    ) -> Vec<u8> {
        let mut messages = iced_core::shell::Bus::new();
        let _ = ui.update(
            &iced_core::window::Headless,
            waker,
            &[event],
            iced_core::mouse::Cursor::Unavailable,
            &mut (),
            &mut messages,
        );
        messages.drain_messages().collect()
    }

    /// A waker that counts its wakes.
    fn counting() -> (iced_core::shell::Waker, Arc<AtomicU64>) {
        let wakes = Arc::new(AtomicU64::new(0));
        let count = Arc::clone(&wakes);
        (
            iced_core::shell::Waker::new(move || {
                let _ = count.fetch_add(1, Ordering::Relaxed);
            }),
            wakes,
        )
    }

    fn until(condition: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        condition()
    }
}
