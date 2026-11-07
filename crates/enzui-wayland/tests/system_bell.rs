//! Exercises [`system_bell::ring`] against an in-process `wayland-server`
//! compositor that offers `xdg_system_bell_v1` or not.
//!
//! It establishes the ring request on the wire, for the window's surface, and
//! the refusal where the compositor has no bell. It plays no sound.
#![cfg(target_os = "linux")]
#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

mod support;

use enzui_wayland::system_bell::{self, BellError};
use support::{Offer, Seen, TIMEOUT, compositor, seen, window};
use wayland_client::Proxy as _;

#[test]
fn the_bell_rings_for_the_window() {
    let (connection, witness, _server) = compositor(Offer {
        bell: true,
        ..Offer::default()
    });
    let surface = window(&connection);
    system_bell::ring(&connection, Some(&surface), TIMEOUT).expect("the bell rings");
    system_bell::ring(&connection, None, TIMEOUT).expect("the bell rings again");
    let rings: Vec<Seen> = seen(&witness, 2)
        .into_iter()
        .filter(|seen| matches!(seen, Seen::Bell(_)))
        .collect();
    assert_eq!(
        rings,
        [
            Seen::Bell(Some(surface.id().protocol_id())),
            Seen::Bell(None)
        ]
    );
}

#[test]
fn without_a_bell_the_ring_is_refused() {
    let (connection, witness, _server) = compositor(Offer::default());
    let surface = window(&connection);
    assert_eq!(
        system_bell::ring(&connection, Some(&surface), TIMEOUT),
        Err(BellError::Unsupported)
    );
    assert_eq!(
        system_bell::ring(&connection, Some(&surface), TIMEOUT),
        Err(BellError::Unsupported),
        "the refusal is remembered"
    );
    assert!(witness.lock().unwrap().seen.is_empty());
}
