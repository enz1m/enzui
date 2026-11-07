//! Converts a runtime window payload into a portal parent export.
//!
//! [`adopt_parent`] returns a lease or a classified failure without choosing an
//! unparented fallback. Missing and foreign payloads both yield
//! [`ParentAdoption::NotWayland`]; a valid payload can separately report a dead
//! surface, an unsupported compositor, or an export failure.

use std::any::Any;

use enzui_core::portal::cancel::PortalCancel;
use enzui_parent_lease::SurfaceIncarnation;
use std::time::Duration;

use crate::bridge::owned_window_from_payload;
use crate::export::ExportError;
use crate::portal_export::ParentExport;
use crate::window_source::WindowGeneration;

/// The parent lease or refusal obtained from a runtime window payload.
#[derive(Debug)]
pub enum ParentAdoption {
    /// A request-lifetime lease; releasing or dropping it queues export destruction.
    Parented {
        /// The exported parent, whose handle goes on the request.
        export: ParentExport,
        /// The surface incarnation the lease names.
        surface: SurfaceIncarnation,
    },
    /// No payload or a foreign payload type was supplied.
    ///
    /// This cannot distinguish a closed window from a non-Wayland window.
    NotWayland,
    /// The window was gone before it could be exported.
    WindowGone,
    /// The compositor advertises no `zxdg_exporter_v2`.
    Unsupported,
    /// The export round trip failed for a stated reason.
    Failed(ExportError),
}

impl ParentAdoption {
    /// The lease, if one was obtained.
    #[must_use]
    pub fn export(&self) -> Option<&ParentExport> {
        match self {
            Self::Parented { export, .. } => Some(export),
            _ => None,
        }
    }

    /// Whether a parent was obtained.
    #[must_use]
    pub fn is_parented(&self) -> bool {
        matches!(self, Self::Parented { .. })
    }
}

/// Exports a parent for the window the runtime handed over.
///
/// `payload` is the answer to the runtime's parent-export action: the window's
/// own connection, its surface, and the surface identity its owner minted.
/// `cancel` is the request's own cancellation, shared with the export so
/// cancelling the request ends the round trip, and `timeout` bounds a
/// compositor that never answers.
///
/// Run this synchronous round trip off the interface thread. An absent or
/// wrongly typed payload returns [`ParentAdoption::NotWayland`] without export.
pub fn adopt_parent(
    payload: Option<Box<dyn Any + Send>>,
    cancel: &PortalCancel,
    timeout: Duration,
) -> ParentAdoption {
    let Some(window) = owned_window_from_payload(payload) else {
        return ParentAdoption::NotWayland;
    };
    let surface = window.generation();
    match ParentExport::open(&window, surface, cancel, timeout) {
        Ok(export) => ParentAdoption::Parented {
            export,
            surface: incarnation_of(surface),
        },
        Err(error) => classify(error),
    }
}

fn incarnation_of(generation: WindowGeneration) -> SurfaceIncarnation {
    SurfaceIncarnation(generation.0)
}

fn classify(error: crate::portal_export::ParentExportError) -> ParentAdoption {
    use crate::portal_export::ParentExportError;
    match error {
        ParentExportError::WindowChanged { .. } => ParentAdoption::WindowGone,
        ParentExportError::Export(ExportError::Unsupported) => ParentAdoption::Unsupported,
        ParentExportError::Export(ExportError::WindowGone) => ParentAdoption::WindowGone,
        ParentExportError::Export(other) => ParentAdoption::Failed(other),
    }
}
