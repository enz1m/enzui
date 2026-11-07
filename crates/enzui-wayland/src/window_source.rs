//! The ownership seam between a live iced/winit window and the exporter.

use wayland_client::Connection;
use wayland_client::protocol::wl_surface::WlSurface;

/// An owner-generated identity of a window's current surface incarnation.
///
/// The platform must mint a non-reused value for each surface. Reusable Wayland
/// object IDs are unsuitable because an outstanding lease must not alias a
/// replacement surface. This wrapper compares values but does not allocate them.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct WindowGeneration(pub u64);

/// A live window's owned Wayland [`Connection`] and [`WlSurface`], handed over by
/// the platform layer that already holds them.
///
/// Supply the window's existing connection and proxy, not a second display
/// connection or a proxy reconstructed from a borrowed raw pointer. Exporting
/// uses the surface but does not transfer responsibility for destroying it.
pub struct OwnedWaylandWindow {
    connection: Connection,
    surface: WlSurface,
    generation: WindowGeneration,
}

impl OwnedWaylandWindow {
    /// Wraps the platform's owned connection, surface, and current generation.
    #[must_use]
    pub fn new(connection: Connection, surface: WlSurface, generation: WindowGeneration) -> Self {
        Self {
            connection,
            surface,
            generation,
        }
    }

    /// The window's Wayland connection.
    #[must_use]
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// The window's surface.
    #[must_use]
    pub fn surface(&self) -> &WlSurface {
        &self.surface
    }

    /// The surface incarnation this handle was captured against.
    #[must_use]
    pub fn generation(&self) -> WindowGeneration {
        self.generation
    }

    /// Compares the captured incarnation with `expected`, without checking surface liveness.
    #[must_use]
    pub fn is_current(&self, expected: WindowGeneration) -> bool {
        self.generation == expected
    }
}
