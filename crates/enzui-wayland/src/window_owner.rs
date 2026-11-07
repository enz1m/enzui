//! Owner-side surface-incarnation lifecycle for parented portal exports.
//!
//! Application window IDs may outlive a `wl_surface`; protocol object IDs may
//! be reused. [`SurfaceLifecycle`] therefore validates exports against a distinct
//! incarnation minted by the surface owner.
//!
//! Call [`SurfaceLifecycle::install_surface`] on each installation and
//! [`SurfaceLifecycle::close_window`] on closure. These calls retire any held
//! export; this helper does not observe runtime events or mint identities.

use std::collections::HashMap;

use crate::portal_export::ParentExport;
use crate::window_source::WindowGeneration;

/// Why a parent export could not be bound to a window's current surface.
#[derive(Debug, PartialEq, Eq)]
pub enum AttachError {
    /// No surface is installed for the window id (it never opened, or already
    /// closed).
    UnknownWindow(u64),
    /// The offered export names a different incarnation from the installed one.
    StaleIncarnation {
        /// The incarnation the window currently carries.
        current: WindowGeneration,
        /// The incarnation the offered export was taken against.
        offered: WindowGeneration,
    },
}

impl core::fmt::Display for AttachError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::UnknownWindow(id) => write!(f, "no surface installed for window {id:#x}"),
            Self::StaleIncarnation { current, offered } => write!(
                f,
                "export bound to a replaced surface (current {current:?}, offered {offered:?})"
            ),
        }
    }
}

impl std::error::Error for AttachError {}

struct WindowSlot {
    incarnation: WindowGeneration,
    lease: Option<ParentExport>,
}

/// Tracks one surface incarnation and at most one parent export per window.
///
/// The caller supplies non-reused identities and reports surface installation
/// and window closure. [`Self::install_surface`] releases any held export before
/// replacing its slot; no runtime event subscription is installed automatically.
pub struct SurfaceLifecycle {
    windows: HashMap<u64, WindowSlot>,
}

impl SurfaceLifecycle {
    /// An owner tracking no windows yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            windows: HashMap::new(),
        }
    }

    /// Installs an owner-minted incarnation, releasing any export held for `window`.
    ///
    /// Reinstallation retires the export even when `incarnation` is unchanged.
    pub fn install_surface(&mut self, window: u64, incarnation: WindowGeneration) {
        if let Some(slot) = self.windows.get_mut(&window)
            && let Some(mut lease) = slot.lease.take()
        {
            lease.release();
        }
        self.windows.insert(
            window,
            WindowSlot {
                incarnation,
                lease: None,
            },
        );
    }

    /// The surface incarnation `window` currently carries, or `None` if no
    /// surface is installed for it.
    #[must_use]
    pub fn current(&self, window: u64) -> Option<WindowGeneration> {
        self.windows.get(&window).map(|slot| slot.incarnation)
    }

    /// Holds a matching export, releasing any previously attached export.
    ///
    /// # Errors
    ///
    /// Returns [`AttachError::UnknownWindow`] for an untracked window or
    /// [`AttachError::StaleIncarnation`] for any incarnation mismatch. Both
    /// refusals drop the offered export without replacing an existing lease.
    pub fn attach(&mut self, window: u64, export: ParentExport) -> Result<(), AttachError> {
        let slot = self
            .windows
            .get_mut(&window)
            .ok_or(AttachError::UnknownWindow(window))?;
        if !export.matches(slot.incarnation) {
            return Err(AttachError::StaleIncarnation {
                current: slot.incarnation,
                offered: export.generation(),
            });
        }
        if let Some(mut superseded) = slot.lease.replace(export) {
            superseded.release();
        }
        Ok(())
    }

    /// Retires the export held for `window` — the request it parented settled or
    /// was superseded — without changing the surface incarnation. Idempotent.
    pub fn retire(&mut self, window: u64) {
        if let Some(slot) = self.windows.get_mut(&window)
            && let Some(mut lease) = slot.lease.take()
        {
            lease.release();
        }
    }

    /// Whether `window` currently holds a live, unreleased export lease.
    #[must_use]
    pub fn holds_export(&self, window: u64) -> bool {
        self.windows
            .get(&window)
            .and_then(|slot| slot.lease.as_ref())
            .is_some_and(ParentExport::is_alive)
    }

    /// Removes the window's slot and releases its export; untracked IDs are a no-op.
    ///
    /// The caller must prevent identity reuse; closed IDs are not retained here.
    pub fn close_window(&mut self, window: u64) {
        if let Some(mut slot) = self.windows.remove(&window)
            && let Some(mut lease) = slot.lease.take()
        {
            lease.release();
        }
    }
}

impl Default for SurfaceLifecycle {
    fn default() -> Self {
        Self::new()
    }
}
