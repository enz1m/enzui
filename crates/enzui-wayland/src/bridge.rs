//! Decodes the iced/winit `WaylandParentExport` action payload.
//!
//! Type erasure keeps `wayland-client` out of the runtime action's signature.
//! [`owned_window_from_payload`] uses the platform's existing connection and
//! proxies rather than opening a display or reconstructing raw pointers.

use std::any::Any;

use wayland_client::Connection;
use wayland_client::protocol::wl_surface::WlSurface;

use crate::window_source::{OwnedWaylandWindow, WindowGeneration};

/// The payload the `WaylandParentExport` action delivers: the window's own
/// connection and surface plus an owner-generated incarnation.
///
/// The incarnation is minted by the platform layer from a monotonic counter and
/// is never a reusable Wayland object id, so a replacement surface never aliases
/// the one it replaced while a request is outstanding.
pub type WaylandParentPayload = (Connection, WlSurface, u64);

/// Downcasts a platform payload, returning `None` for an absent or foreign type.
#[must_use]
pub fn owned_window_from_payload(
    payload: Option<Box<dyn Any + Send>>,
) -> Option<OwnedWaylandWindow> {
    let (connection, surface, generation) = *payload?.downcast::<WaylandParentPayload>().ok()?;
    Some(OwnedWaylandWindow::new(
        connection,
        surface,
        WindowGeneration(generation),
    ))
}
