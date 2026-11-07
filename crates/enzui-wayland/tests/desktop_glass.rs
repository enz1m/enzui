//! Exercises [`DesktopGlass`] against an in-process `wayland-server`
//! compositor that offers `enzui-material-v1` and records what it is sent.
//!
//! It establishes the wire bytes, their order relative to the surface commit,
//! removal, the fallback when the global is absent, and the version-2
//! measurement round trip. It does not render glass.
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

use enzui_wayland::desktop_glass::{
    DesktopGlass, GlassCutout, GlassRegion, GlassShadow, encode_cutouts,
};
use support::{Offer, Seen, TIMEOUT, compositor, seen, send_luminance, window};
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::{Connection, Dispatch, QueueHandle};

/// The window's own queue: whether its `wl_display.sync` came back.
#[derive(Default)]
struct Synced(bool);

impl Dispatch<WlCallback, ()> for Synced {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            state.0 = true;
        }
    }
}

/// One read of the socket by the window's event loop, then its dispatch.
fn window_loop_reads(
    connection: &Connection,
    queue: &mut wayland_client::EventQueue<Synced>,
    state: &mut Synced,
) {
    use rustix::event::{PollFd, PollFlags, poll};
    connection.flush().unwrap();
    if let Some(guard) = connection.prepare_read() {
        let mut fds = [PollFd::from_borrowed_fd(
            guard.connection_fd(),
            PollFlags::IN,
        )];
        let _ = poll(&mut fds, None);
        let _ = guard.read();
    }
    queue.dispatch_pending(state).unwrap();
}

fn panel() -> GlassRegion {
    GlassRegion {
        x: 12.0,
        y: 40.0,
        width: 320.0,
        height: 200.0,
        radius: 18.0,
        tokens: std::array::from_fn(|i| i as f32 + 0.5),
        tint: [0.1, 0.2, 0.3, 1.0],
    }
}

/// Dropping the last handle destroys the material object, removing the glass.
#[test]
fn dropping_the_glass_removes_it() {
    let (connection, witness, _server) = compositor(Offer {
        material: Some(2),
        ..Offer::default()
    });
    let surface = window(&connection);
    let glass = DesktopGlass::bind(&connection, &surface, TIMEOUT).expect("glass");
    glass.set_regions(&[panel()], GlassShadow::default());
    let clone = glass.clone();
    drop(glass);
    drop(clone);
    let seen = seen(&witness, 2);
    assert_eq!(seen.last(), Some(&Seen::Destroyed), "{seen:?}");
}

/// Binding a surface twice yields one glass: the protocol allows one material
/// object per surface, so a second would be a fatal protocol error.
#[test]
fn a_second_binding_shares_the_first() {
    let (connection, witness, _server) = compositor(Offer {
        material: Some(2),
        ..Offer::default()
    });
    let surface = window(&connection);
    let first = DesktopGlass::bind(&connection, &surface, TIMEOUT).expect("glass");
    let second = DesktopGlass::bind(&connection, &surface, TIMEOUT).expect("glass");
    assert!(first.set_regions(&[panel()], GlassShadow::default()));
    let mut wider = panel();
    wider.width = 400.0;
    assert!(second.set_regions(&[wider], GlassShadow::default()));
    surface.commit();
    connection.flush().unwrap();
    let seen = seen(&witness, 4);
    let made = seen
        .iter()
        .filter(|event| **event == Seen::Object("material"))
        .count();
    assert_eq!(made, 1, "{seen:?}");
}

/// Measure-only asked before the glass has regions is applied when the glass
/// is made, ahead of the commit that shows it.
#[test]
fn measure_only_asked_early_applies_when_the_glass_is_made() {
    let (connection, witness, _server) = compositor(Offer {
        material: Some(2),
        ..Offer::default()
    });
    let surface = window(&connection);
    let glass = DesktopGlass::bind(&connection, &surface, TIMEOUT).expect("glass");
    glass.set_measure_only(true);
    assert!(glass.set_regions(&[panel()], GlassShadow::default()));
    surface.commit();
    connection.flush().unwrap();
    let seen = seen(&witness, 4);
    let measure = seen.iter().position(|event| *event == Seen::MeasureOnly(1));
    let commit = seen.iter().position(|event| *event == Seen::Commit);
    assert!(measure.is_some() && measure < commit, "{seen:?}");
}

fn pane() -> GlassCutout {
    GlassCutout {
        x: 20.0,
        y: 52.0,
        width: 300.0,
        height: 180.0,
        radius: 16.0,
    }
}

/// Cutouts asked before the glass is made go out with it, ahead of the
/// commit that shows it; asking again for the same ones sends nothing.
///
/// Sabotage: drop the `want_cutouts` replay in `set_regions`. No cutouts are
/// sent before the commit.
#[test]
fn cutouts_asked_early_apply_when_the_glass_is_made() {
    let (connection, witness, _server) = compositor(Offer {
        material: Some(3),
        ..Offer::default()
    });
    let surface = window(&connection);
    let glass = DesktopGlass::bind(&connection, &surface, TIMEOUT).expect("glass");
    assert!(glass.cuts());
    glass.set_cutouts(&[pane()]);
    assert!(glass.set_regions(&[panel()], GlassShadow::default()));
    surface.commit();
    connection.flush().unwrap();
    glass.set_cutouts(&[pane()]);
    surface.commit();
    connection.flush().unwrap();
    let seen = seen(&witness, 5);
    let cutouts: Vec<usize> = seen
        .iter()
        .enumerate()
        .filter(|(_, event)| matches!(event, Seen::Cutouts(_)))
        .map(|(index, _)| index)
        .collect();
    let commit = seen.iter().position(|event| *event == Seen::Commit);
    assert_eq!(cutouts.len(), 1, "sent once: {seen:?}");
    assert!(Some(cutouts[0]) < commit, "{seen:?}");
    assert_eq!(seen[cutouts[0]], Seen::Cutouts(encode_cutouts(&[pane()])));
}

/// A version-2 compositor takes no cutouts: none are sent, and the glass
/// still is.
#[test]
fn a_version_2_compositor_is_sent_no_cutouts() {
    let (connection, witness, _server) = compositor(Offer {
        material: Some(2),
        ..Offer::default()
    });
    let surface = window(&connection);
    let glass = DesktopGlass::bind(&connection, &surface, TIMEOUT).expect("glass");
    assert!(!glass.cuts());
    glass.set_cutouts(&[pane()]);
    assert!(glass.set_regions(&[panel()], GlassShadow::default()));
    surface.commit();
    connection.flush().unwrap();
    let seen = seen(&witness, 3);
    assert!(
        seen.iter().any(|event| matches!(event, Seen::Regions(_))),
        "{seen:?}"
    );
    assert!(
        !seen.iter().any(|event| matches!(event, Seen::Cutouts(_))),
        "{seen:?}"
    );
}

/// The glass shares the window's connection, whose event loop reads it: the
/// glass never reads the socket itself. A reader of its own would take the
/// window's events (its frame callbacks among them) off the socket without
/// waking the window's loop, holding its next frame until some other event.
///
/// Sabotage: dispatch the glass queue with `blocking_dispatch` again. The
/// window's sync reply is read off the socket by the glass thread.
#[test]
fn the_glass_leaves_the_windows_events_to_the_windows_loop() {
    let (connection, _witness, _server) = compositor(Offer {
        material: Some(2),
        ..Offer::default()
    });
    let surface = window(&connection);
    let _glass = DesktopGlass::bind(&connection, &surface, TIMEOUT).expect("glass");
    let mut queue = connection.new_event_queue::<Synced>();
    let mut synced = Synced::default();
    let _callback = connection.display().sync(&queue.handle(), ());
    connection.flush().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(
        queue.dispatch_pending(&mut synced).unwrap(),
        0,
        "no one but the window's loop reads the window's events"
    );
    window_loop_reads(&connection, &mut queue, &mut synced);
    assert!(synced.0, "the window's loop reads its own reply");
}

/// Measurements still reach the glass with no reader of its own: the
/// glass object handles them as the window's loop reads them.
///
/// Sabotage: drop the reading in the material object's `luminance` handler.
/// No measurement arrives.
#[test]
fn measurements_arrive_when_the_windows_loop_reads() {
    let (connection, witness, _server) = compositor(Offer {
        material: Some(2),
        ..Offer::default()
    });
    let surface = window(&connection);
    let glass = DesktopGlass::bind(&connection, &surface, TIMEOUT).expect("glass");
    glass.set_measure_only(true);
    assert!(glass.set_regions(&[panel()], GlassShadow::default()));
    surface.commit();
    connection.flush().unwrap();
    let _ = seen(&witness, 4);
    send_luminance(&witness, 0, 0.5);
    let mut queue = connection.new_event_queue::<Synced>();
    let mut synced = Synced::default();
    let deadline = std::time::Instant::now() + TIMEOUT;
    while glass.luminance(0).is_none() && std::time::Instant::now() < deadline {
        // The window's loop keeps reading, as it does while the window lives.
        let _callback = connection.display().sync(&queue.handle(), ());
        window_loop_reads(&connection, &mut queue, &mut synced);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(glass.luminance(0), Some(0.5));
}
