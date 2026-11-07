//! The app side of the enzUI agent center.
//!
//! The `center` module (Cargo feature `central-connector`) enrolls an app with
//! the external `enzui-center` daemon over its v2 wire protocol: mutual HMAC
//! authentication over an owner-only Unix socket, typed function registration
//! and invocation. The daemon is optional; an app without it runs unchanged.

#![cfg_attr(docsrs, feature(doc_cfg))]
#[cfg(all(unix, feature = "central-connector"))]
pub mod center;

pub use enzui_security::{cancellation, random, runtime_dir, secret};
