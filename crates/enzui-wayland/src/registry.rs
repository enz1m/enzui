//! One registry round trip on a window's own connection.
//!
//! The window's event loop owns the connection and may be reading its socket
//! on another thread, so [`discover`] dispatches a private queue and polls
//! with a deadline instead of blocking: whichever thread reads queues the
//! events for this one.

use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};

/// The connection dropped, or the compositor did not answer in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Disconnected;

/// The globals the compositor advertised, gathered on a private queue.
#[derive(Debug, Default)]
pub(crate) struct Globals {
    list: Vec<(u32, String, u32)>,
    done: bool,
}

/// The outcome of [`discover`]: the queue and registry to bind on; call
/// [`Discovered::finish`] once bound.
pub(crate) struct Discovered {
    queue: EventQueue<Globals>,
    registry: WlRegistry,
    connection: Connection,
}

impl Discovered {
    /// Ends discovery and returns the queue the bound objects use.
    pub(crate) fn finish(self) -> EventQueue<Globals> {
        // wl_registry has no destructor request; drop its local route.
        let _ = self
            .connection
            .backend()
            .destroy_object(&self.registry.id());
        self.queue
    }
}

/// Lists the compositor's globals on `connection`, waiting at most `timeout`.
pub(crate) fn discover(
    connection: &Connection,
    timeout: Duration,
) -> Result<(Discovered, Globals), Disconnected> {
    let queue = connection.new_event_queue::<Globals>();
    let registry = connection.display().get_registry(&queue.handle(), ());
    let mut discovered = Discovered {
        queue,
        registry,
        connection: connection.clone(),
    };
    let mut globals = Globals::default();
    discovered.roundtrip(&mut globals, timeout)?;
    Ok((discovered, globals))
}

impl Discovered {
    /// Waits, at most `timeout`, until the compositor has answered every
    /// request sent so far on this queue, dispatching its events.
    pub(crate) fn roundtrip(
        &mut self,
        globals: &mut Globals,
        timeout: Duration,
    ) -> Result<(), Disconnected> {
        let connection = &self.connection;
        globals.done = false;
        connection.display().sync(&self.queue.handle(), ());
        let deadline = Instant::now() + timeout;
        loop {
            if self.queue.dispatch_pending(globals).is_err() {
                return Err(Disconnected);
            }
            if globals.done {
                return Ok(());
            }
            if Instant::now() >= deadline || connection.flush().is_err() {
                return Err(Disconnected);
            }
            // Another thread may be reading the socket; then its read queues ours.
            let Some(guard) = connection.prepare_read() else {
                continue;
            };
            let mut fds = [PollFd::from_borrowed_fd(
                guard.connection_fd(),
                PollFlags::IN,
            )];
            let slice = Timespec {
                tv_sec: 0,
                tv_nsec: 25_000_000,
            };
            match poll(&mut fds, Some(&slice)) {
                Ok(0) | Err(rustix::io::Errno::INTR) => drop(guard),
                Ok(_) if fds[0].revents().contains(PollFlags::IN) => {
                    if guard.read().is_err() {
                        return Err(Disconnected);
                    }
                }
                _ => return Err(Disconnected),
            }
        }
    }

    /// The queue objects bound through this discovery use.
    pub(crate) fn handle(&self) -> QueueHandle<Globals> {
        self.queue.handle()
    }
}

impl Globals {
    /// Binds the first global of interface `I` at no more than `max_version`
    /// through `discovered`'s registry and queue.
    pub(crate) fn bind<I>(&self, discovered: &Discovered, max_version: u32) -> Option<I>
    where
        I: Proxy + 'static,
        Self: Dispatch<I, ()>,
    {
        self.bind_with(discovered, max_version, ())
    }

    /// [`Self::bind`] with user data for the bound object's events.
    pub(crate) fn bind_with<I, U>(
        &self,
        discovered: &Discovered,
        max_version: u32,
        data: U,
    ) -> Option<I>
    where
        I: Proxy + 'static,
        U: Send + Sync + 'static,
        Self: Dispatch<I, U>,
    {
        let (name, _, version) = self
            .list
            .iter()
            .find(|(_, interface, _)| interface == I::interface().name)?;
        Some(discovered.registry.bind(
            *name,
            (*version).min(max_version),
            &discovered.queue.handle(),
            data,
        ))
    }
}

impl Dispatch<WlRegistry, ()> for Globals {
    fn event(
        state: &mut Self,
        _: &WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            state.list.push((name, interface, version));
        }
    }
}

impl Dispatch<WlCallback, ()> for Globals {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.done = true;
        }
    }
}
