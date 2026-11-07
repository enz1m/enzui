//! The `enzui-center` app wire, shared by the daemon and every app connector.
//!
//! Both ends build on this crate so a frame or proof one side writes is the
//! frame or proof the other reads. It holds data and pure functions only: the
//! length-prefixed JSON [`framing`] over any [`std::io`] stream, the message
//! and credential types in [`protocol`], and the HMAC handshake [`proof`]s
//! with their [`mac`]. Sockets, peer credentials and the handshake's I/O stay
//! with each connector.

#![cfg_attr(docsrs, feature(doc_cfg))]
pub mod framing;
pub mod mac;
pub mod proof;
pub mod protocol;
