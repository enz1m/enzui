//! Exercises [`surface_hints`] against an in-process `wayland-server`
//! compositor that offers content type and tearing control, or neither.
//!
//! It establishes the requests on the wire and one hint object per surface.
//! It renders nothing.
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

use enzui_wayland::surface_hints::{self, ContentType, HintError};
use support::{Offer, Seen, TIMEOUT, compositor, seen, window};

fn hints() -> Offer {
    Offer {
        hints: true,
        ..Offer::default()
    }
}

#[test]
fn content_type_and_tearing_reach_the_surface() {
    let (connection, witness, _server) = compositor(hints());
    let surface = window(&connection);
    surface_hints::set_content_type(&connection, &surface, ContentType::Game, TIMEOUT).unwrap();
    surface_hints::set_tearing(&connection, &surface, true, TIMEOUT).unwrap();
    // A second change reuses the surface's objects: a second one is a
    // protocol error.
    surface_hints::set_content_type(&connection, &surface, ContentType::Video, TIMEOUT).unwrap();
    surface_hints::set_tearing(&connection, &surface, false, TIMEOUT).unwrap();
    // Game = 3, video = 2; async = 1, vsync = 0.
    assert_eq!(
        seen(&witness, 6),
        [
            Seen::Object("content type"),
            Seen::ContentType(3),
            Seen::Object("tearing control"),
            Seen::Tearing(1),
            Seen::ContentType(2),
            Seen::Tearing(0)
        ]
    );
}

#[test]
fn without_the_protocols_every_hint_is_refused() {
    let (connection, witness, _server) = compositor(Offer::default());
    let surface = window(&connection);
    assert_eq!(
        surface_hints::set_content_type(&connection, &surface, ContentType::Photo, TIMEOUT),
        Err(HintError::Unsupported)
    );
    assert_eq!(
        surface_hints::set_tearing(&connection, &surface, true, TIMEOUT),
        Err(HintError::Unsupported)
    );
    assert!(witness.lock().unwrap().seen.is_empty());
}
