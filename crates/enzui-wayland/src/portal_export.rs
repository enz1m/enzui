//! Portal-request-lifetime ownership of an xdg-foreign parent export.

use std::time::Duration;

use enzui_core::portal::cancel::PortalCancel;
use enzui_core::portal::parent::ParentHandle;

use crate::export::{ExportError, ExportLease, export_toplevel};
use crate::window_source::{OwnedWaylandWindow, WindowGeneration};

/// Why a parent export could not be opened for a portal request.
#[derive(Debug)]
pub enum ParentExportError {
    /// The captured window's incarnation differs from the caller's expectation.
    WindowChanged {
        /// The incarnation the caller expected.
        expected: WindowGeneration,
        /// The incarnation the handle actually carries.
        found: WindowGeneration,
    },
    /// The export itself failed; carries the typed exporter outcome.
    Export(ExportError),
}

impl core::fmt::Display for ParentExportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WindowChanged { expected, found } => write!(
                f,
                "window surface changed (expected {expected:?}, found {found:?})"
            ),
            Self::Export(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for ParentExportError {}

/// An xdg-foreign parent export owned for the lifetime of one portal request.
///
/// Attach [`Self::parent_handle`] to the request and retain this value until
/// settlement. Share the request's [`PortalCancel`] with [`Self::open`] so parent
/// closure can interrupt export. Releasing or dropping this value queues export
/// destruction without waiting for compositor acknowledgement.
pub struct ParentExport {
    lease: ExportLease,
    generation: WindowGeneration,
}

impl core::fmt::Debug for ParentExport {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ParentExport")
            .field("generation", &self.generation)
            .field("alive", &self.lease.is_alive())
            .finish_non_exhaustive()
    }
}

impl ParentExport {
    /// Exports `window` only when its captured incarnation equals `expected`.
    ///
    /// Uses [`export_toplevel`]'s shared deadline and cancellation policy. Pass
    /// the request's `cancel` so parent closure can interrupt the wait.
    ///
    /// # Errors
    ///
    /// Returns [`ParentExportError::WindowChanged`] before export on any
    /// incarnation mismatch, or [`ParentExportError::Export`] on export failure.
    pub fn open(
        window: &OwnedWaylandWindow,
        expected: WindowGeneration,
        cancel: &PortalCancel,
        timeout: Duration,
    ) -> Result<Self, ParentExportError> {
        if !window.is_current(expected) {
            return Err(ParentExportError::WindowChanged {
                expected,
                found: window.generation(),
            });
        }
        let lease = export_toplevel(window.connection(), window.surface(), cancel, timeout)
            .map_err(ParentExportError::Export)?;
        Ok(Self {
            lease,
            generation: expected,
        })
    }

    /// The portal `parent_window` handle to attach to the request.
    #[must_use]
    pub fn parent_handle(&self) -> ParentHandle {
        self.lease.parent_handle()
    }

    /// The surface incarnation this export is bound to.
    #[must_use]
    pub fn generation(&self) -> WindowGeneration {
        self.generation
    }

    /// Whether this export still names surface incarnation `current`.
    ///
    /// Supply the current incarnation from the window owner when accepting a
    /// portal response. Equality alone is checked; this does not query the runtime
    /// or check whether the export is still alive.
    #[must_use]
    pub fn matches(&self, current: WindowGeneration) -> bool {
        self.generation == current
    }

    /// Whether the export is still live and unreleased.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        self.lease.is_alive()
    }

    /// Queues export destruction now rather than on drop.
    pub fn release(&mut self) {
        self.lease.release();
    }
}
