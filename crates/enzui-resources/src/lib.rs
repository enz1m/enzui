#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    reason = "this crate parses untrusted bytes; an `as` that loses data hides a bug"
)]
#![deny(missing_docs)]
//! Application-owned remote resource loading for enzUI-ICED.
//!
//! This crate is the effectful half of [`enzui_core::async_image`]. Core
//! owns policy and the pure lifecycle reducer; this crate owns the bounded
//! worker pool, real transport cancellation, magic-driven decode, single-flight
//! coalescing, and stable decoded-handle reuse. Widgets never spawn work and
//! never perform hidden I/O: the application constructs and owns an
//! [`ImageService`], feeds its [`ServiceEvent`]s into the core reducer, and
//! renders the reducer snapshot.
//!
//! The real HTTP(S) implementation is behind the default `curl-transport`
//! feature. Disable default features to use the service with an injected
//! transport only (tests and sandboxed applications can do this without
//! linking libcurl).
//!
//! HEIC (HEVC-branded HEIF) decode is separately available behind the
//! off-by-default `heif` feature. It runtime-loads system libheif and reports a typed
//! [`DecodeError::Unsupported`] when the feature, library, or decoder plugin is
//! absent; it never fabricates an empty image. Enabling it opts into native
//! decoder attack surface and HEVC's patent/licensing constraints. GIF/APNG
//! posters require the separate `first-frame-previews` feature and explicit
//! `first_frame` API; static and remote defaults still refuse animation.
//! AVIF, TIFF and ICO remain outside the static format set.

mod bounded_drop;
mod cache_identity;
mod decode;
#[cfg(feature = "derived-previews")]
pub mod derived_preview;
#[cfg(feature = "first-frame-previews")]
pub mod first_frame;
mod handle_cache;
pub mod header_limits;
mod http_cache;
#[cfg(feature = "image-ops")]
pub mod image_ops;
#[cfg(feature = "local-images")]
pub mod local_image;
mod service;
pub mod service_download;

#[cfg(feature = "curl-transport")]
mod resolver_gate;

#[cfg(feature = "curl-transport")]
pub mod curl_transport;

#[cfg(feature = "curl-transport")]
pub mod curl_download_transport;

pub use cache_identity::{CacheIdentityDomain, cache_identity};
pub use decode::{DecodeError, DecodedAsset, StaticFormat, decode_static, decode_static_resized};
pub use handle_cache::HandleCache;
// Callers need `CacheKey::new` to keep the secret identity out of the display
// URL accepted by public `HttpCache` methods.
pub use http_cache::{
    CacheKey, CacheUnavailable, CachedResponse, ClearFailure, ClearOutcome, Freshness, HttpCache,
    HttpCacheConfig, VARY_ANY,
};
pub use service::{
    BusyReason, CancelSignal, FlightId, ImageService, RemoteRequest, RequestDisposition,
    ServiceConfig, ServiceError, ServiceEvent, SubscriberId, TransferProgress, Transport,
    TransportError, TransportResponse, Validators,
};
