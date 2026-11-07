//! Encode raw X11 handles and request asynchronous Wayland portal parents.
//!
//! [`export_parent`](crate::portal_parent::export_parent) encodes Xlib and Xcb XIDs synchronously. A Wayland surface
//! yields [`ParentExport::NeedsWaylandExport`](crate::portal_parent::ParentExport::NeedsWaylandExport) without retaining its raw pointer;
//! [`export_parent_task`](crate::portal_parent::export_parent_task) obtains owned runtime objects for asynchronous export.
//! Encoding does not validate window liveness or token validity. Resolve requested
//! parent policy with [`enzui_core::portal::parent::PortalParent::resolve`].
//! Unsupported platforms require an explicit caller decision about parenting.

use enzui_core::portal::parent::ParentHandle;
use raw_window_handle::RawWindowHandle;

/// Synchronous encoding outcome without window-liveness validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParentExport {
    /// An encoded X11 parent; the caller must establish that its window is live.
    Ready(ParentHandle),
    /// A Wayland surface requiring asynchronous xdg-foreign export.
    /// No borrowed surface pointer is retained in this outcome.
    NeedsWaylandExport,
    /// A platform without a supported portal-parent encoding.
    /// This does not choose or authorize an unparented request.
    Unsupported,
}

impl ParentExport {
    /// The finished handle, if one was produced synchronously.
    #[must_use]
    pub fn ready(&self) -> Option<&ParentHandle> {
        match self {
            Self::Ready(h) => Some(h),
            _ => None,
        }
    }

    /// Whether this export needs an asynchronous Wayland step to complete.
    #[must_use]
    pub fn needs_wayland_export(&self) -> bool {
        matches!(self, Self::NeedsWaylandExport)
    }
}

/// Encode an Xlib or Xcb XID as a hexadecimal X11 portal parent.
///
/// Wayland returns [`ParentExport::NeedsWaylandExport`]; other platforms return
/// [`ParentExport::Unsupported`]. No compositor exchange or liveness check occurs.
#[must_use]
pub fn export_parent(handle: &RawWindowHandle) -> ParentExport {
    match handle {
        // Xlib uses c_ulong: u32 on some targets and u64 on others.
        #[allow(
            clippy::unnecessary_cast,
            reason = "Xlib's `c_ulong` is u32 on some targets"
        )]
        RawWindowHandle::Xlib(h) => ParentExport::Ready(ParentHandle::x11(h.window as u64)),
        RawWindowHandle::Xcb(h) => {
            ParentExport::Ready(ParentHandle::x11(u64::from(h.window.get())))
        }
        RawWindowHandle::Wayland(_) => ParentExport::NeedsWaylandExport,
        _ => ParentExport::Unsupported,
    }
}

/// Prefix an owned portal-parent value with `wayland:`.
/// The token is copied verbatim without trimming or validity checks.
#[must_use]
pub fn wayland_parent_from_token(export_token: &str) -> ParentHandle {
    ParentHandle::wayland(export_token)
}

/// Request owned Wayland objects from the runtime and export a portal parent.
///
/// The compositor round trip uses `pool`'s bounded workers. Excess requests
/// settle as [`crate::export_pool::PoolError::Busy`]; admitted work retains its
/// occupancy until the closure exits, even after cancellation. A request already
/// terminal when its runtime carrier settles returns `Cancelled` without pool
/// admission. `cancel` and `timeout` also bound the compositor round trip.
///
/// The settlement travels in a single-consumption
/// [`enzui_parent_lease::Handoff`] so cloneable application messages can
/// carry an owned protocol lease. The application must consume that handoff.
#[cfg(target_os = "linux")]
pub fn export_parent_task(
    pool: &crate::export_pool::ExportPoolDispatcher,
    generation: u64,
    window: iced::window::Id,
    cancel: enzui_core::portal::cancel::PortalCancel,
    timeout: std::time::Duration,
) -> iced::Task<
    enzui_parent_lease::Handoff<
        enzui_core::blocking_task::BlockingTaskSettlement<enzui_wayland::ParentAdoption>,
    >,
> {
    use enzui_core::blocking_task::{BlockingTaskError, BlockingTaskSettlement};
    use enzui_parent_lease::Handoff;

    let pool = pool.clone();

    iced::window::wayland_parent_export(window)
        .map(Handoff::new)
        .then(move |carrier| {
            let carrier = carrier.clone();
            let cancel = cancel.clone();
            let pool = pool.clone();
            iced::Task::future(async move {
                // Terminal requests bypass pool occupancy even when the pool is full.
                if cancel.is_terminal() {
                    return Handoff::new(BlockingTaskSettlement::from_error(
                        generation,
                        BlockingTaskError::Cancelled,
                    ));
                }

                let payload = carrier.take().flatten();
                let cancel_for_work = cancel.clone();
                let task = pool.submit(generation, move |_ctx| {
                    enzui_wayland::adopt_parent(payload, &cancel_for_work, timeout)
                });
                Handoff::new(task.await)
            })
        })
}
