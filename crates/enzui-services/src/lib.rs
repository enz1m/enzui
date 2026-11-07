//! Platform and app-domain services for enzUI applications.
//!
//! `enzui-core` holds the UI architecture: actions, focus, semantics,
//! persistence, portals, window placement, and the models widgets render
//! (media state, downloads, paged text, annotations). This crate holds the
//! services an application drives from outside the view: media backends,
//! subprocess and process-activation transports, the desktop launcher, the
//! credential store, document preview, Git status, OAuth sign-in, DNS lookup
//! and the libcurl mail transport.
//!
//! Each heavy backend is behind its own feature, off by default, so an app
//! compiles only what it uses: `gstreamer`, `mpris`, `pdf-preview`,
//! `secret-service`, `oauth`, `mail-transport`, `archive` and
//! `extension-package`.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    reason = "this crate parses untrusted bytes; an `as` that loses data hides a bug"
)]
#![deny(missing_docs)]

// The core modules the services build on, named at the crate root so their
// `crate::` paths read the same as they did inside core.
#[allow(
    unused_imports,
    reason = "some of these are named only by tests or by optional features"
)]
use enzui_core::{
    atomic_state, blocking_task, cancellation, decode_limits, frame_plan, monotonic_ms,
    state_authority, url_policy, user_file,
};

#[cfg(feature = "archive")]
pub mod archive;
pub mod credential_store;
#[cfg(feature = "desktop-icons")]
pub mod desktop_icons;
pub mod detached_launch;
pub mod dns;
pub mod document_preview;
pub mod durable_activation;
#[cfg(feature = "gstreamer")]
pub mod event_sound;
#[cfg(feature = "extension-package")]
pub mod extension_package;
#[cfg(feature = "mail-transport")]
pub mod mail_transport;
pub mod media;
pub mod mime_apps;
#[cfg(feature = "oauth")]
pub mod oauth;
pub mod process_transport;
#[cfg(feature = "spell-check")]
pub mod spell_check;
pub mod vcs;
