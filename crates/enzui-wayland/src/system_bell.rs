//! The system bell (`xdg_system_bell_v1`): the desktop's own alert for "that
//! did nothing", such as Backspace in an empty field or a refused shortcut.
//!
//! The compositor plays it as the user configured it (a sound, a screen flash,
//! nothing), so prefer it to an app-made beep. [`ring`] binds the bell on the
//! window's own connection once and rings it for the window's surface; where
//! the compositor has no bell it returns [`BellError::Unsupported`], and the
//! caller may play the sound theme's `bell` event instead.

use std::any::Any;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::xdg::system_bell::v1::client::xdg_system_bell_v1::{self, XdgSystemBellV1};

use crate::registry::{self, Globals};

/// Why the bell did not ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BellError {
    /// Not a Wayland window, or the payload was not the window's objects.
    NotWayland,
    /// The compositor offers no `xdg_system_bell_v1`.
    Unsupported,
    /// The connection dropped, or the compositor did not answer in time.
    Disconnected,
}

impl core::fmt::Display for BellError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::NotWayland => "not a Wayland window",
            Self::Unsupported => "the compositor has no system bell (xdg_system_bell_v1)",
            Self::Disconnected => "the compositor did not answer",
        })
    }
}

impl std::error::Error for BellError {}

/// Rings the bell for `surface` (the compositor may flash that window), or
/// for no window in particular. The first ring on a connection makes one
/// registry round trip, waiting at most `timeout`; run it off the draw path.
///
/// # Errors
///
/// [`BellError::Unsupported`] when the compositor has no bell, or
/// [`BellError::Disconnected`].
pub fn ring(
    connection: &Connection,
    surface: Option<&WlSurface>,
    timeout: Duration,
) -> Result<(), BellError> {
    let bell = shared_bell(connection, timeout)?;
    bell.ring(surface.filter(|surface| surface.is_alive()));
    connection.flush().map_err(|_| BellError::Disconnected)
}

/// [`ring`] from the `(Connection, WlSurface, u64)` payload
/// `iced::window::wayland_parent_export` resolves to.
///
/// # Errors
///
/// [`BellError::NotWayland`] for a missing or foreign payload, or any error
/// of [`ring`].
pub fn ring_from_payload(
    payload: Option<Box<dyn Any + Send>>,
    timeout: Duration,
) -> Result<(), BellError> {
    let (connection, surface, _) = *payload
        .ok_or(BellError::NotWayland)?
        .downcast::<crate::WaylandParentPayload>()
        .map_err(|_| BellError::NotWayland)?;
    ring(&connection, Some(&surface), timeout)
}

// One bell per connection, `None` where the compositor has none, so a refusal
// costs no further round trips.
fn shared_bell(connection: &Connection, timeout: Duration) -> Result<XdgSystemBellV1, BellError> {
    type Known = (Connection, Option<XdgSystemBellV1>);
    static SHARED: OnceLock<Mutex<Vec<Known>>> = OnceLock::new();
    let shared = SHARED.get_or_init(Mutex::default);
    let mut shared = shared.lock().map_err(|_| BellError::Disconnected)?;
    shared.retain(|(_, bell)| bell.as_ref().is_none_or(Proxy::is_alive));
    if let Some((_, bell)) = shared
        .iter()
        .find(|(known, _)| known.backend() == connection.backend())
    {
        return bell.clone().ok_or(BellError::Unsupported);
    }
    let (discovered, globals) =
        registry::discover(connection, timeout).map_err(|_| BellError::Disconnected)?;
    let bell = globals.bind::<XdgSystemBellV1>(&discovered, 1);
    // The bell sends no events, so its queue is never dispatched.
    drop(discovered.finish());
    shared.push((connection.clone(), bell.clone()));
    bell.ok_or(BellError::Unsupported)
}

impl Dispatch<XdgSystemBellV1, ()> for Globals {
    fn event(
        _: &mut Self,
        _: &XdgSystemBellV1,
        _: xdg_system_bell_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
