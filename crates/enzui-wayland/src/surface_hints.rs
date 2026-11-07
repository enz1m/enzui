//! What a window's content is, for the compositor: its kind
//! (`wp_content_type_v1`) and whether it may tear (`wp_tearing_control_v1`).
//!
//! Each is state of the window's surface, applied on its next commit and kept
//! for as long as the surface lives; setting it again replaces it. A
//! compositor without the protocol refuses with [`HintError::Unsupported`]
//! and the window behaves as before.
//!
//! - [`set_content_type`]: a video or game asks for direct scan-out and
//!   steady timing, a photo for colour accuracy over speed.
//! - [`set_tearing`]: a game that renders without vsync (iced's
//!   `Settings::vsync = false`) lets its frames reach the screen mid-refresh.
//!
//! The colour space is not set here: the Vulkan swapchain describes it
//! (`wp_color_management_v1`, sRGB) through Mesa's WSI, and a surface takes
//! one colour description object only, so a second would be a protocol
//! error.

use std::any::Any;
use std::sync::Mutex;
use std::time::Duration;

use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::content_type::v1::client::wp_content_type_manager_v1::{
    self, WpContentTypeManagerV1,
};
use wayland_protocols::wp::content_type::v1::client::wp_content_type_v1::{
    self, Type, WpContentTypeV1,
};
use wayland_protocols::wp::tearing_control::v1::client::wp_tearing_control_manager_v1::{
    self, WpTearingControlManagerV1,
};
use wayland_protocols::wp::tearing_control::v1::client::wp_tearing_control_v1::{
    self, PresentationHint, WpTearingControlV1,
};

use crate::registry::{self, Discovered, Globals};

/// Why a hint was not applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HintError {
    /// Not a Wayland window, or the payload was not the window's objects.
    NotWayland,
    /// The window's surface is gone.
    WindowGone,
    /// The compositor offers no protocol for this hint.
    Unsupported,
    /// The connection dropped, or the compositor did not answer in time.
    Disconnected,
}

impl core::fmt::Display for HintError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotWayland => f.write_str("not a Wayland window"),
            Self::WindowGone => f.write_str("the window surface is gone"),
            Self::Unsupported => f.write_str("the compositor does not support this"),
            Self::Disconnected => f.write_str("the compositor did not answer"),
        }
    }
}

impl std::error::Error for HintError {}

/// What a window shows, so the compositor can treat it accordingly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContentType {
    /// Ordinary application content.
    #[default]
    None,
    /// Still pictures: colour accuracy matters more than speed.
    Photo,
    /// Moving pictures: steady timing, possibly direct scan-out.
    Video,
    /// Interactive real-time content: low latency first.
    Game,
}

/// Sets what `surface` shows, from its next commit.
///
/// # Errors
/// [`HintError::Unsupported`] without `wp_content_type_manager_v1`, or a
/// window or connection error.
pub fn set_content_type(
    connection: &Connection,
    surface: &WlSurface,
    content: ContentType,
    timeout: Duration,
) -> Result<(), HintError> {
    let kind = match content {
        ContentType::None => Type::None,
        ContentType::Photo => Type::Photo,
        ContentType::Video => Type::Video,
        ContentType::Game => Type::Game,
    };
    with_objects(
        connection,
        surface,
        timeout,
        |objects, discovered, globals| {
            if objects.content.is_none() {
                let manager = globals
                    .bind::<WpContentTypeManagerV1>(discovered, 1)
                    .ok_or(HintError::Unsupported)?;
                objects.content =
                    Some(manager.get_surface_content_type(surface, &discovered.handle(), ()));
                manager.destroy();
            }
            if let Some(object) = &objects.content {
                object.set_content_type(kind);
            }
            Ok(())
        },
    )
}

/// Lets `surface`'s frames reach the screen as soon as they are ready, even
/// mid-refresh (tearing), or asks for vsync again; from its next commit.
/// Tearing only happens when the app presents without vsync.
///
/// # Errors
/// [`HintError::Unsupported`] without `wp_tearing_control_manager_v1`, or a
/// window or connection error.
pub fn set_tearing(
    connection: &Connection,
    surface: &WlSurface,
    allowed: bool,
    timeout: Duration,
) -> Result<(), HintError> {
    let hint = if allowed {
        PresentationHint::Async
    } else {
        PresentationHint::Vsync
    };
    with_objects(
        connection,
        surface,
        timeout,
        |objects, discovered, globals| {
            if objects.tearing.is_none() {
                let manager = globals
                    .bind::<WpTearingControlManagerV1>(discovered, 1)
                    .ok_or(HintError::Unsupported)?;
                objects.tearing =
                    Some(manager.get_tearing_control(surface, &discovered.handle(), ()));
                manager.destroy();
            }
            if let Some(object) = &objects.tearing {
                object.set_presentation_hint(hint);
            }
            Ok(())
        },
    )
}

/// The hint objects of one surface, kept while it lives: a second object of
/// the same kind for a surface is a protocol error.
struct SurfaceObjects {
    connection: Connection,
    surface: WlSurface,
    content: Option<WpContentTypeV1>,
    tearing: Option<WpTearingControlV1>,
}

static SURFACES: Mutex<Vec<SurfaceObjects>> = Mutex::new(Vec::new());

/// Runs `apply` with `surface`'s objects and a fresh discovery of the
/// compositor's globals.
fn with_objects(
    connection: &Connection,
    surface: &WlSurface,
    timeout: Duration,
    apply: impl FnOnce(&mut SurfaceObjects, &mut Discovered, &mut Globals) -> Result<(), HintError>,
) -> Result<(), HintError> {
    if !surface.is_alive() {
        return Err(HintError::WindowGone);
    }
    let mut surfaces = SURFACES.lock().map_err(|_| HintError::Disconnected)?;
    surfaces.retain(|objects| objects.surface.is_alive());
    let index = match surfaces.iter().position(|objects| {
        objects.surface == *surface && objects.connection.backend() == connection.backend()
    }) {
        Some(index) => index,
        None => {
            surfaces.push(SurfaceObjects {
                connection: connection.clone(),
                surface: surface.clone(),
                content: None,
                tearing: None,
            });
            surfaces.len() - 1
        }
    };
    let (mut discovered, mut globals) =
        registry::discover(connection, timeout).map_err(|_| HintError::Disconnected)?;
    let outcome = apply(&mut surfaces[index], &mut discovered, &mut globals);
    drop(discovered.finish());
    connection.flush().map_err(|_| HintError::Disconnected)?;
    outcome
}

/// Runs `hint` with the window's connection and surface from the
/// `(Connection, WlSurface, u64)` payload `iced::window::wayland_parent_export`
/// resolves to.
///
/// # Errors
/// [`HintError::NotWayland`] for a missing or foreign payload, or the hint's
/// own error.
pub fn on_window(
    payload: Option<Box<dyn Any + Send>>,
    hint: impl FnOnce(&Connection, &WlSurface) -> Result<(), HintError>,
) -> Result<(), HintError> {
    let (connection, surface, _) = *payload
        .ok_or(HintError::NotWayland)?
        .downcast::<crate::WaylandParentPayload>()
        .map_err(|_| HintError::NotWayland)?;
    hint(&connection, &surface)
}

impl Dispatch<WpContentTypeManagerV1, ()> for Globals {
    fn event(
        _: &mut Self,
        _: &WpContentTypeManagerV1,
        _: wp_content_type_manager_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpContentTypeV1, ()> for Globals {
    fn event(
        _: &mut Self,
        _: &WpContentTypeV1,
        _: wp_content_type_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpTearingControlManagerV1, ()> for Globals {
    fn event(
        _: &mut Self,
        _: &WpTearingControlManagerV1,
        _: wp_tearing_control_manager_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WpTearingControlV1, ()> for Globals {
    fn event(
        _: &mut Self,
        _: &WpTearingControlV1,
        _: wp_tearing_control_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
