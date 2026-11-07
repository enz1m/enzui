//! Runtime-owned Wayland xdg-foreign parent export for enzUI-ICED portals.
//!
//! A Wayland portal `parent_window` token is the reply to an asynchronous
//! [xdg-foreign v2] `export_toplevel` round trip, and the exported object must
//! stay alive for as long as the portal request wants the parent relationship.
//! [`export_toplevel`] performs that round trip on a live window's own
//! [`Connection`](wayland_client::Connection) and
//! [`WlSurface`](wayland_client::protocol::wl_surface::WlSurface) and returns an
//! owned [`ExportLease`] holding the exported object; [`ExportError`] reports
//! each failure as a distinct outcome rather than a silent unparented fallback.
//!
//! Use [`adopt_parent`] for a runtime action payload, or [`ParentExport::open`]
//! with an [`OwnedWaylandWindow`]. The platform supplies its existing connection,
//! surface proxy, and a non-reused surface incarnation; the exporter neither
//! opens another display connection nor reconstructs proxies from raw pointers.
//! Keep the returned export alive until the portal request settles or is
//! cancelled. Parenting does not grant activation or keyboard focus.
//!
//! [`desktop_glass`] asks the compositor for liquid glass over what lies
//! behind a window, where it offers `enzui-material-v1`.
//!
//! [xdg-foreign v2]: https://wayland.app/protocols/xdg-foreign-unstable-v2

#![cfg_attr(docsrs, feature(doc_cfg))]
#[cfg(target_os = "linux")]
mod adopt;
#[cfg(target_os = "linux")]
mod bridge;
#[cfg(target_os = "linux")]
pub mod desktop_glass;
#[cfg(target_os = "linux")]
mod export;
#[cfg(target_os = "linux")]
mod portal_export;
#[cfg(target_os = "linux")]
mod registry;
#[cfg(target_os = "linux")]
pub mod surface_hints;
#[cfg(target_os = "linux")]
pub mod system_bell;
#[cfg(target_os = "linux")]
mod window_owner;
#[cfg(target_os = "linux")]
mod window_source;

#[cfg(target_os = "linux")]
pub use adopt::{ParentAdoption, adopt_parent};
#[cfg(target_os = "linux")]
pub use bridge::{WaylandParentPayload, owned_window_from_payload};
#[cfg(target_os = "linux")]
pub use export::{ExportError, ExportLease, export_toplevel};
#[cfg(target_os = "linux")]
pub use portal_export::{ParentExport, ParentExportError};
#[cfg(target_os = "linux")]
pub use window_owner::{AttachError, SurfaceLifecycle};
#[cfg(target_os = "linux")]
pub use window_source::{OwnedWaylandWindow, WindowGeneration};
