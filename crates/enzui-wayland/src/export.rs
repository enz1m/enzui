//! xdg-foreign v2 export state machine and owned lease (Linux only).

use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use wayland_client::backend::WaylandError;
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, DispatchError, EventQueue, Proxy, QueueHandle};
use wayland_protocols::xdg::foreign::zv2::client::zxdg_exported_v2::{self, ZxdgExportedV2};
use wayland_protocols::xdg::foreign::zv2::client::zxdg_exporter_v2::ZxdgExporterV2;

use enzui_core::portal::CancelReason;
use enzui_core::portal::cancel::PortalCancel;
use enzui_core::portal::parent::ParentHandle;

// Socket waits are sliced so cancellation and the shared deadline remain observable.
const POLL_SLICE: Duration = Duration::from_millis(25);

/// A typed reason an xdg-foreign export could not produce a live lease.
///
/// None of these mean "run unparented": whether an absent parent is acceptable
/// is the caller's [`PortalParent`](enzui_core::portal::parent::PortalParent)
/// decision, not a consequence of a failed export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportError {
    /// The compositor advertises no `zxdg_exporter_v2` global.
    Unsupported,
    /// The surface was already destroyed, or died while the export was in
    /// flight.
    WindowGone,
    /// Cancellation was observed while waiting; carries its recorded reason.
    Cancelled(CancelReason),
    /// The compositor connection dropped during the export.
    Disconnected,
    /// The deadline was observed before a usable export handle was available.
    TimedOut,
    /// The compositor raised a protocol error; the string is the diagnostic.
    Protocol(String),
}

impl core::fmt::Display for ExportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unsupported => {
                f.write_str("compositor does not implement xdg-foreign (zxdg_exporter_v2)")
            }
            Self::WindowGone => {
                f.write_str("the window surface was destroyed before the export completed")
            }
            Self::Cancelled(reason) => write!(f, "export cancelled ({reason:?})"),
            Self::Disconnected => f.write_str("the Wayland connection dropped during export"),
            Self::TimedOut => f.write_str("the compositor did not answer the export in time"),
            Self::Protocol(msg) => write!(f, "xdg-foreign protocol error: {msg}"),
        }
    }
}

impl std::error::Error for ExportError {}

#[derive(Default)]
struct ExportState {
    exporter: Option<ZxdgExporterV2>,
    // After the sync callback, an absent exporter cannot be a pending registry advertisement.
    registry_done: bool,
    token: Option<String>,
}

impl Dispatch<WlRegistry, ()> for ExportState {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        (): &(),
        _conn: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface.as_str() == ZxdgExporterV2::interface().name
            && state.exporter.is_none()
        {
            // xdg-foreign exporter is a version-1 interface.
            let bound = version.min(1);
            state.exporter = Some(registry.bind::<ZxdgExporterV2, (), Self>(name, bound, qh, ()));
        }
    }
}

impl Dispatch<WlCallback, ()> for ExportState {
    fn event(
        state: &mut Self,
        _proxy: &WlCallback,
        event: wl_callback::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.registry_done = true;
        }
    }
}

impl Dispatch<ZxdgExporterV2, ()> for ExportState {
    fn event(
        _state: &mut Self,
        _proxy: &ZxdgExporterV2,
        _event: <ZxdgExporterV2 as Proxy>::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZxdgExportedV2, ()> for ExportState {
    fn event(
        state: &mut Self,
        _proxy: &ZxdgExportedV2,
        event: zxdg_exported_v2::Event,
        (): &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let zxdg_exported_v2::Event::Handle { handle } = event {
            state.token = Some(handle);
        }
    }
}

/// An owned, live xdg-foreign export.
///
/// Retains the exported object until [`Self::release`] or drop queues its
/// destruction. Teardown attempts a nonblocking flush without waiting for
/// compositor acknowledgement. The input surface remains owned by the platform.
pub struct ExportLease {
    conn: Connection,
    // Keep the export's queue alive after handle receipt; it is no longer pumped.
    _queue: EventQueue<ExportState>,
    exported: ZxdgExportedV2,
    token: String,
    released: bool,
}

impl ExportLease {
    /// The raw xdg-foreign export handle token (no `wayland:` prefix).
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The portal `parent_window` handle for this export (`wayland:<token>`).
    #[must_use]
    pub fn parent_handle(&self) -> ParentHandle {
        ParentHandle::wayland(&self.token)
    }

    /// Whether the exported object is still live and unreleased.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        !self.released && self.exported.is_alive()
    }

    /// Queues destruction once and attempts a nonblocking flush.
    ///
    /// Does not await compositor acknowledgement or report flush failures.
    pub fn release(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        if self.exported.is_alive() {
            self.exported.destroy();
            let _ = self.conn.flush();
        }
    }
}

impl Drop for ExportLease {
    fn drop(&mut self) {
        self.release();
    }
}

impl core::fmt::Debug for ExportLease {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ExportLease")
            .field("token", &self.token)
            .field("released", &self.released)
            .field("alive", &self.is_alive())
            .finish()
    }
}

/// Export `surface` for xdg-foreign parenting on its own `conn`, returning an
/// owned [`ExportLease`] whose token can parent a portal request.
///
/// Run on the portal request's worker: this pumps the supplied connection
/// synchronously. Registry discovery and handle receipt share one `timeout`
/// deadline; cancellation and expiry are checked between polls configured for
/// 25 ms. Already-dispatched replies are examined before cancellation or expiry.
///
/// # Errors
///
/// - [`ExportError::Unsupported`] — no `zxdg_exporter_v2` global.
/// - [`ExportError::WindowGone`] — the surface is dead before or during export.
/// - [`ExportError::Cancelled`] — `cancel` reached a terminal state.
/// - [`ExportError::Disconnected`] — the connection dropped.
/// - [`ExportError::TimedOut`] — no answer within `timeout`.
/// - [`ExportError::Protocol`] — the compositor reported a protocol error.
#[allow(
    clippy::missing_panics_doc,
    reason = "panics only on a broken invariant: the event loop ends only once the token has \
              arrived"
)]
pub fn export_toplevel(
    conn: &Connection,
    surface: &WlSurface,
    cancel: &PortalCancel,
    timeout: Duration,
) -> Result<ExportLease, ExportError> {
    let start = Instant::now();
    let mut queue = conn.new_event_queue::<ExportState>();
    let qh = queue.handle();
    let mut state = ExportState::default();

    let _registry = conn.display().get_registry(&qh, ());
    let _sync = conn.display().sync(&qh, ());
    conn.flush().map_err(|_| lost(conn))?;

    loop {
        queue
            .dispatch_pending(&mut state)
            .map_err(classify_dispatch)?;
        if state.exporter.is_some() || state.registry_done {
            break;
        }
        if let Some(reason) = cancel.outcome_reason() {
            return Err(ExportError::Cancelled(reason));
        }
        if start.elapsed() >= timeout {
            return Err(ExportError::TimedOut);
        }
        pump_slice(conn)?;
    }

    let exporter = state.exporter.clone().ok_or(ExportError::Unsupported)?;

    if !surface.is_alive() {
        return Err(ExportError::WindowGone);
    }
    if let Some(reason) = cancel.outcome_reason() {
        return Err(ExportError::Cancelled(reason));
    }

    let exported = exporter.export_toplevel(surface, &qh, ());
    conn.flush().map_err(|_| lost(conn))?;

    loop {
        queue
            .dispatch_pending(&mut state)
            .map_err(classify_dispatch)?;
        if state.token.is_some() {
            break;
        }
        if let Some(reason) = cancel.outcome_reason() {
            destroy_pending(conn, &exported);
            return Err(ExportError::Cancelled(reason));
        }
        if !surface.is_alive() {
            destroy_pending(conn, &exported);
            return Err(ExportError::WindowGone);
        }
        if start.elapsed() >= timeout {
            destroy_pending(conn, &exported);
            return Err(ExportError::TimedOut);
        }
        pump_slice(conn)?;
    }

    let token = state.token.take().expect("token present after break");
    Ok(ExportLease {
        conn: conn.clone(),
        _queue: queue,
        exported,
        token,
        released: false,
    })
}

// Format the outer backend error without requiring Display on its inner protocol diagnostic.
fn classify_dispatch(err: DispatchError) -> ExportError {
    match err {
        DispatchError::Backend(backend @ WaylandError::Protocol(_)) => {
            ExportError::Protocol(backend.to_string())
        }
        _ => ExportError::Disconnected,
    }
}

/// Classify a failed connection operation: a compositor-posted protocol
/// error kills the connection too, but it is reported as what it is.
fn lost(conn: &Connection) -> ExportError {
    match conn.protocol_error() {
        Some(error) => ExportError::Protocol(WaylandError::Protocol(error).to_string()),
        None => ExportError::Disconnected,
    }
}

fn destroy_pending(conn: &Connection, exported: &ZxdgExportedV2) {
    if exported.is_alive() {
        exported.destroy();
        let _ = conn.flush();
    }
}

fn pump_slice(conn: &Connection) -> Result<(), ExportError> {
    conn.flush().map_err(|_| lost(conn))?;

    // `prepare_read` yields `None` when another queue already buffered events.
    let Some(guard) = conn.prepare_read() else {
        return Ok(());
    };

    let fd = guard.connection_fd();
    let mut fds = [PollFd::from_borrowed_fd(fd, PollFlags::IN)];
    let ts = Timespec {
        tv_sec: 0,
        tv_nsec: POLL_SLICE.subsec_nanos().into(),
    };

    match poll(&mut fds, Some(&ts)) {
        Ok(0) => {
            drop(guard);
            Ok(())
        }
        Ok(_) => {
            if fds[0].revents().intersects(PollFlags::IN) {
                guard.read().map_err(|_| lost(conn))?;
                Ok(())
            } else {
                drop(guard);
                Err(lost(conn))
            }
        }
        Err(rustix::io::Errno::INTR) => {
            drop(guard);
            Ok(())
        }
        Err(_) => {
            drop(guard);
            Err(lost(conn))
        }
    }
}
