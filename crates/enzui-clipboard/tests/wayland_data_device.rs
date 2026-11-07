//! Cross-process clipboard behavior on a private compositor without data-control globals.
//!
//! Peers have independent Wayland connections, with `DISPLAY` and `XAUTHORITY`
//! removed. The compositor fixture supplies focus and selection events; these
//! cases do not exercise a host compositor or the toolkit's window integration.
//! The separate arboard control must fail on the same socket.

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

use std::time::{Duration, Instant};

use support::compositor::{Config, IconSeen, StartupStall, TestCompositor};
use support::peer::{Peer, to_hex};
use support::runner::{Case, run};
use support::test_scratch::ScratchDir;

const ROLE: &str = "ENZUI_CLIPBOARD_TEST_ROLE";

fn main() {
    if std::env::var(ROLE).as_deref() == Ok("arboard") {
        arboard_probe();
    }

    run(&[
        Case {
            name: "deferred_copy_survives_focus_loss_and_rejects_stale_results",
            run: deferred_copy_survives_focus_loss_and_rejects_stale_results,
        },
        Case {
            name: "a_copy_dies_with_its_owner_and_ownership_is_tracked_for_the_keeper",
            run: a_copy_dies_with_its_owner_and_ownership_is_tracked_for_the_keeper,
        },
        Case {
            name: "a_drag_shows_its_icon_and_carries_a_window",
            run: a_drag_shows_its_icon_and_carries_a_window,
        },
        Case {
            name: "native_drag_ignored_start_release_and_leave_still_resolves",
            run: native_drag_ignored_start_release_and_leave_still_resolves,
        },
        Case {
            name: "native_drag_ignored_start_resolves_and_releases_the_source_slot",
            run: native_drag_ignored_start_resolves_and_releases_the_source_slot,
        },
        Case {
            name: "clipboard_metadata_beyond_drag_limits_remains_truthful",
            run: clipboard_metadata_beyond_drag_limits_remains_truthful,
        },
        Case {
            name: "text_moves_both_ways_between_two_independent_applications",
            run: text_moves_both_ways_between_two_independent_applications,
        },
        Case {
            name: "the_primary_selection_moves_between_applications_apart_from_the_clipboard",
            run: the_primary_selection_moves_between_applications_apart_from_the_clipboard,
        },
        Case {
            name: "without_the_primary_selection_global_primary_requests_are_refused",
            run: without_the_primary_selection_global_primary_requests_are_refused,
        },
        Case {
            name: "an_unfocused_application_gains_no_monitoring_access",
            run: an_unfocused_application_gains_no_monitoring_access,
        },
        Case {
            name: "an_unfocused_application_cannot_claim_the_selection",
            run: an_unfocused_application_cannot_claim_the_selection,
        },
        Case {
            name: "ownership_transfers_and_the_previous_owner_learns_it",
            run: ownership_transfers_and_the_previous_owner_learns_it,
        },
        Case {
            name: "a_discarded_claim_is_reported_rather_than_assumed",
            run: a_discarded_claim_is_reported_rather_than_assumed,
        },
        Case {
            name: "a_claim_outstanding_when_the_compositor_ends_is_answered",
            run: a_claim_outstanding_when_the_compositor_ends_is_answered,
        },
        Case {
            name: "the_selection_disappears_when_its_owner_exits",
            run: the_selection_disappears_when_its_owner_exits,
        },
        Case {
            name: "html_and_file_offers_negotiate_their_type",
            run: html_and_file_offers_negotiate_their_type,
        },
        Case {
            name: "a_large_payload_survives_a_multi_chunk_transfer",
            run: a_large_payload_survives_a_multi_chunk_transfer,
        },
        Case {
            name: "a_stalled_owner_fails_at_its_deadline_and_the_runtime_recovers",
            run: a_stalled_owner_fails_at_its_deadline_and_the_runtime_recovers,
        },
        Case {
            name: "each_seat_carries_its_own_selection",
            run: each_seat_carries_its_own_selection,
        },
        Case {
            name: "the_display_handle_adoption_path_carries_the_same_selection",
            run: the_display_handle_adoption_path_carries_the_same_selection,
        },
        Case {
            name: "a_clipboard_read_by_another_thread_serves_and_pastes",
            run: a_clipboard_read_by_another_thread_serves_and_pastes,
        },
        Case {
            name: "a_stalled_first_startup_round_trip_is_refused_at_its_deadline",
            run: a_stalled_first_startup_round_trip_is_refused_at_its_deadline,
        },
        Case {
            name: "a_stalled_second_startup_round_trip_is_refused_at_its_deadline",
            run: a_stalled_second_startup_round_trip_is_refused_at_its_deadline,
        },
        Case {
            name: "the_data_control_path_fails_where_this_transport_succeeds",
            run: the_data_control_path_fails_where_this_transport_succeeds,
        },
    ])
}

fn native_drag_ignored_start_resolves_and_releases_the_source_slot() {
    ignored_start_releases_the_source_slot(Config::default());
}

fn native_drag_ignored_start_release_and_leave_still_resolves() {
    ignored_start_releases_the_source_slot(Config {
        ignored_drag_release_leave: true,
        ..Config::default()
    });
}

fn a_drag_shows_its_icon_and_carries_a_window() {
    let session = Session::with_config(Config {
        drag_window: true,
        ..Config::default()
    });
    let mut peer = session.peer("icon-drag");
    session.compositor.pointer_grab(0);
    assert_eq!(peer.expect_ok("icon-drag"), "attached Cancelled");
    assert_eq!(
        session.compositor.drag_icon(),
        Some(IconSeen {
            width: 2,
            height: 2,
            offset: (-1, -1),
            scale: 1,
            // RGBA 200, 100, 50 at half alpha, premultiplied, stored B, G, R, A.
            first_pixel: [25, 50, 100, 128],
        }),
        "the icon is committed with its hotspot at the pointer"
    );
    assert_eq!(
        session.compositor.attached_windows(),
        [(String::from("peer"), 12, 6)]
    );
    peer.quit();
}

fn ignored_start_releases_the_source_slot(config: Config) {
    let session = Session::with_config(config);
    let mut peer = session.peer("ignored-drag");
    for _ in 0..2 {
        session.compositor.pointer_grab(0);
        assert_eq!(peer.expect_ok("ignored-drag"), "Cancelled");
    }
    assert_eq!(session.compositor.refused_selections(), 2);
    peer.quit();
}

fn clipboard_metadata_beyond_drag_limits_remains_truthful() {
    let session = Session::start(1);
    let mut source = session.peer("metadata-source");
    let mut destination = session.peer("metadata-destination");
    session.compositor.focus(0, Some(0));
    source.expect_ok("wait-focus 3000");
    source.expect_ok("offer many-mimes");
    assert_eq!(session.compositor.selection_mimes(0).len(), 66);
    session.compositor.focus(0, Some(1));
    destination.expect_ok("wait-offer 3000");
    assert!(
        destination
            .expect_ok("paste mime application/x-64")
            .ends_with("2a")
    );
    let long = format!("application/{}", "x".repeat(1025));
    assert!(
        destination
            .expect_ok(&format!("paste mime {long}"))
            .ends_with("2b")
    );
    destination.quit();
    source.quit();
}

struct Session {
    scratch: ScratchDir,
    compositor: TestCompositor,
    spawned: std::cell::Cell<usize>,
}

impl Session {
    fn start(seats: usize) -> Self {
        Self::with_config(Config {
            seats,
            ..Config::default()
        })
    }

    fn with_config(config: Config) -> Self {
        let scratch = ScratchDir::new_restricted("clip");
        let compositor = TestCompositor::start(scratch.path(), config);

        Self {
            scratch,
            compositor,
            spawned: std::cell::Cell::new(0),
        }
    }

    fn peer(&self, label: &str) -> Peer {
        self.peer_with(label, &[])
    }

    fn peer_with(&self, label: &str, environment: &[(&str, String)]) -> Peer {
        let peer = Peer::spawn(label, &self.compositor, self.scratch.path(), environment);
        self.spawned.set(self.spawned.get() + 1);

        // Connection order decides the compositor's client index, so a peer is
        // fully registered before the next one starts.
        self.compositor
            .wait_for_clients(self.spawned.get(), Duration::from_secs(5));

        peer
    }
}

/// Pastes the primary selection, retrying while the offer is still on its way.
fn paste_primary(peer: &mut Peer) -> String {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let answer = peer.ask("paste-primary");
        if let Some(rest) = answer.strip_prefix("ok ") {
            let bytes = support::peer::from_hex(rest.split_whitespace().nth(1).expect("bytes"));
            return String::from_utf8(bytes).expect("UTF-8");
        }
        assert!(
            Instant::now() < deadline,
            "paste-primary kept answering {answer:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn the_primary_selection_moves_between_applications_apart_from_the_clipboard() {
    let session = Session::with_config(Config {
        primary: true,
        ..Config::default()
    });
    let mut alpha = session.peer("alpha");
    let mut beta = session.peer("beta");

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    alpha.copy_text("on the clipboard");
    alpha.expect_ok(&format!("offer-primary {}", to_hex(b"selected by alpha")));
    session
        .compositor
        .wait_for_primary_owner(0, Some(0), Duration::from_secs(3));

    session.compositor.focus(0, Some(1));
    beta.expect_ok("wait-offer 3000");
    assert_eq!(paste_primary(&mut beta), "selected by alpha");
    assert_eq!(
        beta.paste_text(),
        "on the clipboard",
        "the primary selection is separate from the clipboard"
    );

    beta.expect_ok(&format!("offer-primary {}", to_hex(b"selected by beta")));
    session
        .compositor
        .wait_for_primary_owner(0, Some(1), Duration::from_secs(3));
    session.compositor.focus(0, Some(0));
    assert_eq!(paste_primary(&mut alpha), "selected by beta");

    beta.quit();
    alpha.quit();
}

fn without_the_primary_selection_global_primary_requests_are_refused() {
    let session = Session::start(1);
    let mut alpha = session.peer("alpha");
    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    let refused = alpha.expect_err(&format!("offer-primary {}", to_hex(b"x")));
    assert!(refused.contains("NoPrimarySelection"), "{refused}");
    let refused = alpha.expect_err("paste-primary");
    assert!(refused.contains("NoPrimarySelection"), "{refused}");
    alpha.quit();
}

fn text_moves_both_ways_between_two_independent_applications() {
    let session = Session::start(1);
    let mut alpha = session.peer("alpha");
    let mut beta = session.peer("beta");

    let environment = alpha.expect_ok("env");
    assert!(
        environment.contains("display=unset") && environment.contains("xauthority=unset"),
        "the peer still has an X11 route: {environment}"
    );

    let globals = alpha.expect_ok("globals");
    assert_eq!(
        globals, "wl_compositor,wl_data_device_manager,wl_seat",
        "the acceptance environment is one ordinary clipboard global and no \
         privileged one"
    );
    assert!(
        !globals.contains("data_control"),
        "the compositor must withhold clipboard-manager access: {globals}"
    );

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    alpha.copy_text("written by alpha");

    session
        .compositor
        .wait_for_selection_owner(0, Some(0), Duration::from_secs(3));

    session.compositor.focus(0, Some(1));
    beta.expect_ok("wait-offer 3000");
    assert_eq!(beta.paste_text(), "written by alpha");

    beta.copy_text("written by beta");

    session
        .compositor
        .wait_for_selection_owner(0, Some(1), Duration::from_secs(3));

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-offer 3000");
    assert_eq!(alpha.paste_text(), "written by beta");

    beta.quit();
    alpha.quit();
}

fn a_copy_dies_with_its_owner_and_ownership_is_tracked_for_the_keeper() {
    let session = Session::start(1);
    let mut alpha = session.peer("alpha");
    let mut beta = session.peer("beta");

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    alpha.copy_text("first from alpha");
    session
        .compositor
        .wait_for_selection_owner(0, Some(0), Duration::from_secs(3));
    assert_eq!(alpha.expect_ok("owned"), "true");

    // Another copy cancels alpha's source: alpha no longer has anything to keep.
    session.compositor.focus(0, Some(1));
    beta.expect_ok("wait-focus 3000");
    beta.copy_text("from beta");
    session
        .compositor
        .wait_for_selection_owner(0, Some(1), Duration::from_secs(3));
    let deadline = Instant::now() + Duration::from_secs(3);
    while alpha.expect_ok("owned") == "true" && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        alpha.expect_ok("owned"),
        "false",
        "a replaced copy is not kept"
    );

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    let secret = "synthetic-credential-value";
    assert!(
        alpha
            .expect_err(&format!(
                "offer expired-sensitive {}",
                to_hex(secret.as_bytes())
            ))
            .contains("Stopped")
    );
    assert_eq!(
        alpha.paste_text(),
        "from beta",
        "an expired request cannot replace the existing clipboard"
    );
    assert!(
        alpha
            .expect_err(&format!(
                "offer-sensitive-primary {}",
                to_hex(secret.as_bytes())
            ))
            .contains("sensitive text")
    );
    assert!(
        alpha
            .expect_err(&format!(
                "offer-sensitive-drag {}",
                to_hex(secret.as_bytes())
            ))
            .contains("sensitive text")
    );
    alpha.expect_ok(&format!(
        "offer sensitive 1500 {}",
        to_hex(secret.as_bytes())
    ));
    assert_eq!(
        alpha.expect_ok("owned"),
        "false",
        "sensitive text never reaches the keeper"
    );
    assert_eq!(alpha.expect_ok("hand-off"), "false");
    session.compositor.focus(0, Some(1));
    beta.expect_ok("wait-focus 3000");
    beta.expect_ok("wait-offer 3000");
    assert_eq!(beta.paste_text(), secret);
    let hint = beta.expect_ok("paste mime x-kde-passwordManagerHint");
    assert!(
        hint.ends_with(&to_hex(b"secret")),
        "password-manager history hint: {hint}"
    );
    session
        .compositor
        .wait_for_selection_owner(0, None, Duration::from_secs(3));
    assert!(beta.expect_err("paste text").contains("NoSelection"));

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    alpha.expect_ok(&format!(
        "offer sensitive 300 {}",
        to_hex(secret.as_bytes())
    ));
    alpha.copy_text("newer ordinary copy");
    session.compositor.settle(Duration::from_millis(400));
    assert_eq!(
        alpha.paste_text(),
        "newer ordinary copy",
        "expiry cannot revoke a newer source of this process"
    );
    assert_eq!(alpha.expect_ok("owned"), "true");

    alpha.expect_ok(&format!(
        "offer sensitive 300 {}",
        to_hex(secret.as_bytes())
    ));
    session.compositor.focus(0, Some(1));
    beta.expect_ok("wait-focus 3000");
    beta.copy_text("newer foreign copy");
    session.compositor.settle(Duration::from_millis(400));
    assert_eq!(
        beta.paste_text(),
        "newer foreign copy",
        "expiry cannot revoke another process's newer source"
    );

    // Without a clipboard manager, a copy is gone once its owner exits.
    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    alpha.copy_text("second from alpha");
    session
        .compositor
        .wait_for_selection_owner(0, Some(0), Duration::from_secs(3));
    alpha.quit();
    session
        .compositor
        .wait_for_selection_owner(0, None, Duration::from_secs(3));
    session.compositor.focus(0, Some(1));
    let refused = beta.expect_err("paste text");
    assert!(
        refused.contains("NoSelection") || refused.contains("Empty"),
        "nothing is left to paste: {refused}"
    );
    beta.quit();
}

fn an_unfocused_application_gains_no_monitoring_access() {
    let session = Session::start(1);
    let mut alpha = session.peer("alpha");
    let mut observer = session.peer("observer");

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    alpha.copy_text("private to alpha");

    // Wait through delayed delivery before asserting the unfocused peer received no offer.
    let waited = observer.expect_err("wait-offer 1500");
    assert_eq!(
        waited, "timeout",
        "an unfocused application must never be handed the selection: {waited}"
    );

    let seats = observer.expect_ok("seats");
    assert!(
        seats.contains("unfocused") && seats.ends_with(':'),
        "an unfocused application must be offered no types at all: {seats}"
    );

    let refusal = observer.expect_err("paste text");
    assert_eq!(refusal, "NotFocused", "unexpected refusal: {refusal}");

    let any_seat = observer.expect_err("paste any");
    assert_eq!(any_seat, "NoSelection", "unexpected refusal: {any_seat}");

    let globals = observer.expect_ok("globals");
    assert!(
        !globals.contains("data_control"),
        "no privileged monitoring protocol may be reachable: {globals}"
    );

    // The same peer must read the selection after gaining focus.
    session.compositor.focus(0, Some(1));
    observer.expect_ok("wait-offer 3000");
    assert_eq!(observer.paste_text(), "private to alpha");

    observer.quit();
    alpha.quit();
}

fn an_unfocused_application_cannot_claim_the_selection() {
    let session = Session::start(1);
    let mut alpha = session.peer("alpha");
    let mut beta = session.peer("beta");

    // Give beta a valid serial before focus loss to distinguish focus refusal from missing input.
    session.compositor.focus(0, Some(1));
    beta.expect_ok("wait-focus 3000");

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    alpha.copy_text("owned by alpha");

    let refusal = beta.expect_err("offer text 6e6f7065");
    assert_eq!(refusal, "NotFocused", "unexpected refusal: {refusal}");

    // Allow a mistakenly sent request to reach the compositor before checking its counter.
    session.compositor.settle(Duration::from_millis(300));

    assert_eq!(
        session.compositor.selection_owner(0),
        Some(0),
        "the unfocused application must not have taken the selection"
    );
    assert_eq!(
        session.compositor.refused_selections(),
        0,
        "the request must never reach the compositor at all"
    );

    beta.quit();
    alpha.quit();
}

fn ownership_transfers_and_the_previous_owner_learns_it() {
    let session = Session::start(1);
    let mut alpha = session.peer("alpha");
    let mut beta = session.peer("beta");

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    alpha.copy_text("first");

    let owned = alpha.expect_ok("seats");
    assert!(owned.contains(":owner:"), "alpha should own it: {owned}");

    session.compositor.focus(0, Some(1));
    beta.expect_ok("wait-focus 3000");
    beta.copy_text("second");

    alpha.expect_ok("wait-lost-ownership 3000");

    let lost = alpha.expect_ok("seats");
    assert!(
        lost.contains(":guest:"),
        "alpha must know it no longer owns the selection: {lost}"
    );
    session
        .compositor
        .wait_for_selection_owner(0, Some(1), Duration::from_secs(3));

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-offer 3000");
    assert_eq!(alpha.paste_text(), "second");

    beta.quit();
    alpha.quit();
}

fn a_discarded_claim_is_reported_rather_than_assumed() {
    let session = Session::start(1);
    let mut alpha = session.peer("alpha");
    let mut beta = session.peer("beta");

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    alpha.copy_text("alpha wrote this");
    session
        .compositor
        .wait_for_selection_owner(0, Some(0), Duration::from_secs(3));

    let refused_before = session.compositor.refused_selections();
    session.compositor.discard_claims(true);

    session.compositor.focus(0, Some(1));
    beta.expect_ok("wait-focus 3000");

    let refusal = beta.expect_err(&format!("offer text {}", to_hex(b"beta wrote this")));
    assert!(
        refusal.starts_with("ClaimNotHonoured"),
        "a dropped claim must be reported by name: {refusal}"
    );
    assert!(
        session.compositor.refused_selections() > refused_before,
        "the compositor should have seen the claim it dropped"
    );

    session
        .compositor
        .wait_for_selection_owner(0, Some(0), Duration::from_secs(3));

    // Grant a later claim from the same peer to distinguish refusal from a broken connection.
    session.compositor.discard_claims(false);
    beta.copy_text("beta wrote this");
    session
        .compositor
        .wait_for_selection_owner(0, Some(1), Duration::from_secs(3));

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-offer 3000");
    assert_eq!(alpha.paste_text(), "beta wrote this");

    beta.quit();
    alpha.quit();
}

fn a_claim_outstanding_when_the_compositor_ends_is_answered() {
    let mut session = Session::start(1);
    // Keep claim expiry beyond the result wait so disconnect must end the pending request.
    let mut alpha = session.peer_with(
        "alpha",
        &[("ENZUI_CLIPBOARD_CLAIM_TIMEOUT_MS", String::from("30000"))],
    );

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");

    // The compositor keeps the claim: it is neither granted nor refused.
    session.compositor.discard_claims(true);
    alpha.send(&format!("offer text {}", to_hex(b"never granted")));
    std::thread::sleep(Duration::from_millis(250));

    let ended = Instant::now();
    session.compositor.stop();

    let answer = alpha.answer(Duration::from_secs(20));
    let waited = ended.elapsed();

    assert!(
        answer.starts_with("err"),
        "a claim that can never be granted must not be reported as a copy: {answer}"
    );
    assert!(
        answer.contains("Disconnected") || answer.contains("Stopped"),
        "the answer should name the closed connection: {answer}"
    );
    assert!(
        waited < Duration::from_secs(3),
        "the answer should follow the disconnect, not a transfer deadline: {waited:?}"
    );

    alpha.quit();
}

fn the_selection_disappears_when_its_owner_exits() {
    let session = Session::start(1);
    let mut alpha = session.peer("alpha");
    let mut beta = session.peer("beta");

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    alpha.copy_text("alpha is still running");

    session.compositor.focus(0, Some(1));
    beta.expect_ok("wait-offer 3000");
    assert_eq!(beta.paste_text(), "alpha is still running");

    alpha.quit();

    beta.expect_ok("wait-no-offer 5000");
    let refusal = beta.expect_err("paste text");
    assert_eq!(refusal, "NoSelection", "unexpected refusal: {refusal}");
    session
        .compositor
        .wait_for_selection_owner(0, None, Duration::from_secs(3));

    beta.quit();
}

fn html_and_file_offers_negotiate_their_type() {
    let session = Session::start(1);
    let mut alpha = session.peer("alpha");
    let mut beta = session.peer("beta");

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");

    let html = support::peer::to_hex(b"<b>rich</b>");
    let plain = support::peer::to_hex(b"rich");
    alpha.expect_ok(&format!("offer html {html} {plain}"));

    session.compositor.focus(0, Some(1));
    beta.expect_ok("wait-offer 3000");

    assert!(
        session
            .compositor
            .selection_mimes(0)
            .iter()
            .any(|mime| mime.starts_with("text/html")),
        "the compositor recorded the promised types: {:?}",
        session.compositor.selection_mimes(0)
    );

    let rich = beta.expect_ok("paste html");
    assert!(
        rich.starts_with("text/html"),
        "an HTML reader must get HTML: {rich}"
    );
    assert_eq!(
        String::from_utf8(support::peer::from_hex(
            rich.split_whitespace().nth(1).expect("payload")
        ))
        .expect("utf-8"),
        "<b>rich</b>"
    );

    assert_eq!(beta.paste_text(), "rich");

    let refusal = beta.expect_err("paste image");
    assert!(
        refusal.starts_with("MimeUnavailable") && refusal.contains("text/html"),
        "the refusal must list what was actually offered: {refusal}"
    );

    session.compositor.focus(0, Some(1));
    let uris = support::peer::to_hex(b"/tmp/one.txt\n/tmp/two.txt");
    beta.expect_ok(&format!("offer uris {uris}"));

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-offer 3000");
    let list = String::from_utf8(alpha.paste_bytes("text/uri-list")).expect("utf-8");
    assert_eq!(list, "file:///tmp/one.txt\r\nfile:///tmp/two.txt\r\n");

    beta.quit();
    alpha.quit();
}

fn a_large_payload_survives_a_multi_chunk_transfer() {
    let session = Session::start(1);
    let mut alpha = session.peer("alpha");
    let mut beta = session.peer("beta");

    const SIZE: usize = 512 * 1024;

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    alpha.expect_ok(&format!("offer bytes application/octet-stream {SIZE}"));

    session.compositor.focus(0, Some(1));
    beta.expect_ok("wait-offer 3000");

    let bytes = beta.paste_bytes("application/octet-stream");
    let expected: Vec<u8> = (0..SIZE).map(|index| (index % 251) as u8).collect();

    assert_eq!(bytes.len(), SIZE, "the whole payload must arrive");
    assert_eq!(bytes, expected, "the payload must arrive unchanged");

    beta.quit();
    alpha.quit();
}

fn a_stalled_owner_fails_at_its_deadline_and_the_runtime_recovers() {
    let session = Session::start(1);
    let mut alpha = session.peer("alpha");
    let mut beta = session.peer_with(
        "beta",
        &[("ENZUI_CLIPBOARD_RECEIVE_TIMEOUT_MS", String::from("400"))],
    );

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    alpha.copy_text("eventually readable");

    session.compositor.focus(0, Some(1));
    beta.expect_ok("wait-offer 3000");

    session.compositor.stall_transfers(true);

    let started = std::time::Instant::now();
    let refusal = beta.expect_err("paste text");
    let elapsed = started.elapsed();

    assert!(
        refusal.starts_with("TransferStalled"),
        "a silent owner must be reported as stalled: {refusal}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "the paste must end at its own deadline, not the harness deadline: {elapsed:?}"
    );

    session.compositor.stall_transfers(false);
    assert_eq!(
        beta.paste_text(),
        "eventually readable",
        "the clipboard runtime must survive a stalled transfer"
    );

    beta.quit();
    alpha.quit();
}

fn each_seat_carries_its_own_selection() {
    let session = Session::start(2);
    let mut alpha = session.peer("alpha");
    let mut beta = session.peer("beta");

    session.compositor.focus(0, Some(0));
    session.compositor.focus(1, Some(1));

    alpha.expect_ok("wait-focus 3000");
    beta.expect_ok("wait-focus 3000");

    alpha.copy_text("seat zero content");
    beta.copy_text("seat one content");

    session
        .compositor
        .wait_for_selection_owner(0, Some(0), Duration::from_secs(3));
    session
        .compositor
        .wait_for_selection_owner(1, Some(1), Duration::from_secs(3));

    assert_eq!(alpha.paste_text(), "seat zero content");
    assert_eq!(beta.paste_text(), "seat one content");

    let across = beta.expect_err("paste seat seat0");
    assert_eq!(
        across, "NoSelection",
        "a client focused on one seat must not read another seat's selection: {across}"
    );

    beta.quit();
    alpha.quit();
}

fn the_display_handle_adoption_path_carries_the_same_selection() {
    adopted_peers_carry_the_selection("foreign-display");
}

/// A clipboard that leaves the socket to another reader still serves the
/// paste it owns and receives the offer it pastes: the events that reader
/// routes to the clipboard's queue wake the worker.
///
/// Sabotage: leave no waker on the queue -- the owner never sees the paste
/// request and the paste fails at its deadline.
fn a_clipboard_read_by_another_thread_serves_and_pastes() {
    adopted_peers_carry_the_selection("read-elsewhere");
}

fn adopted_peers_carry_the_selection(mode: &str) {
    let session = Session::start(1);
    let adopt = [("ENZUI_CLIPBOARD_ADOPT", String::from(mode))];
    let mut alpha = session.peer_with("alpha", &adopt);
    let mut beta = session.peer_with("beta", &adopt);

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    alpha.copy_text("through the display handle");

    session
        .compositor
        .wait_for_selection_owner(0, Some(0), Duration::from_secs(3));

    session.compositor.focus(0, Some(1));
    beta.expect_ok("wait-offer 3000");
    assert_eq!(beta.paste_text(), "through the display handle");

    beta.quit();
    alpha.quit();
}

fn assert_startup_round_trip_timeout(stall: StartupStall) {
    let scratch = ScratchDir::new_restricted("clip-startup");
    let compositor = TestCompositor::start(
        scratch.path(),
        Config {
            startup_stall: Some(stall),
            ..Config::default()
        },
    );
    let environment = [
        ("ENZUI_CLIPBOARD_ADOPT", String::from("foreign-display")),
        ("ENZUI_CLIPBOARD_STARTUP_TIMEOUT_MS", String::from("100")),
    ];
    let mut peer = Peer::spawn_starting("startup-stall", &compositor, scratch.path(), &environment);

    compositor.wait_for_startup_stall(stall, Duration::from_secs(5));
    let refusal = peer.startup_announcement(Duration::from_secs(2));
    assert_eq!(
        refusal, "fatal clipboard: StartupStalled { after: 100ms }",
        "startup silence must be reported as a bounded refusal"
    );
    let status = peer.wait_for_exit(Duration::from_secs(2));
    assert_eq!(
        status.code(),
        Some(2),
        "the refused peer must exit normally"
    );
    compositor.resume_startup();
}

fn a_stalled_first_startup_round_trip_is_refused_at_its_deadline() {
    assert_startup_round_trip_timeout(StartupStall::FirstRoundTrip);
}

fn a_stalled_second_startup_round_trip_is_refused_at_its_deadline() {
    assert_startup_round_trip_timeout(StartupStall::SecondRoundTrip);
}

fn the_data_control_path_fails_where_this_transport_succeeds() {
    let session = Session::start(1);
    let mut alpha = session.peer("alpha");
    let mut beta = session.peer("beta");

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    alpha.copy_text("reachable without data control");

    session.compositor.focus(0, Some(1));
    beta.expect_ok("wait-offer 3000");
    assert_eq!(beta.paste_text(), "reachable without data control");

    // Use the same socket and X11-free environment for the arboard control.
    let probe = std::process::Command::new(std::env::current_exe().expect("test executable"))
        .env(ROLE, "arboard")
        .env("WAYLAND_DISPLAY", session.compositor.socket())
        .env("XDG_RUNTIME_DIR", session.scratch.path())
        .env_remove("DISPLAY")
        .env_remove("XAUTHORITY")
        .env_remove("WAYLAND_SOCKET")
        .output()
        .expect("the calibration control runs");

    let report = String::from_utf8_lossy(&probe.stdout).trim().to_owned();

    eprintln!("calibration: the data-control clipboard reported: {report}");

    assert!(
        report.starts_with("arboard-unavailable"),
        "arboard's data-control path must fail here; it reported: {report}"
    );

    beta.quit();
    alpha.quit();
}

fn arboard_probe() -> ! {
    match arboard::Clipboard::new() {
        Ok(mut clipboard) => match clipboard.get_text() {
            Ok(text) => {
                println!("arboard-connected read {} bytes", text.len());
                std::process::exit(0);
            }
            Err(error) => {
                println!("arboard-connected but-unreadable {error}");
                std::process::exit(0);
            }
        },
        Err(error) => {
            println!("arboard-unavailable {error}");
            std::process::exit(0);
        }
    }
}

fn deferred_copy_survives_focus_loss_and_rejects_stale_results() {
    let session = Session::start(1);
    let mut alpha = session.peer("deferred owner");
    let mut beta = session.peer("paste target");
    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    alpha.copy_text("previous value");
    alpha.expect_ok("defer 5000 128");
    session.compositor.focus(0, Some(1));
    beta.expect_ok("wait-offer 3000");
    beta.send("paste text");
    alpha.expect_ok(&format!("fulfil {}", to_hex(b"ready after focus loss")));
    let received = beta.answer(Duration::from_secs(3));
    assert!(
        received.starts_with("ok "),
        "deferred paste failed: {received}"
    );
    assert_eq!(
        received.split_whitespace().last(),
        Some(to_hex(b"ready after focus loss").as_str())
    );

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    alpha.expect_ok("defer 5000 128");
    session.compositor.focus(0, Some(1));
    beta.expect_ok("wait-focus 3000");
    beta.copy_text("new owner value");
    alpha.expect_ok("wait-lost-ownership 3000");
    alpha.expect_err(&format!("fulfil {}", to_hex(b"stale value")));
    assert_eq!(beta.paste_text(), "new owner value");
    assert!(alpha.expect_err("defer 5000 128").contains("NotFocused"));
    assert_eq!(
        beta.paste_text(),
        "new owner value",
        "a refused reservation preserves the newer selection"
    );

    session.compositor.focus(0, Some(0));
    alpha.expect_ok("wait-focus 3000");
    alpha.expect_ok("defer 5000 128");
    alpha.expect_ok("invalidate-deferred");
    alpha.expect_err(&format!("fulfil {}", to_hex(b"wrong generation")));
    session
        .compositor
        .wait_for_selection_owner(0, None, Duration::from_secs(3));

    alpha.expect_ok("defer 100 128");
    session
        .compositor
        .wait_for_selection_owner(0, None, Duration::from_secs(3));
    alpha.expect_err(&format!("fulfil {}", to_hex(b"expired")));
    alpha.expect_ok("defer 5000 4");
    assert!(
        alpha
            .expect_err(&format!("fulfil {}", to_hex(b"too large")))
            .contains("TransferTooLarge")
    );
    session
        .compositor
        .wait_for_selection_owner(0, None, Duration::from_secs(3));

    alpha.expect_ok("defer 5000 128");
    alpha.expect_ok("cancel-deferred");
    session
        .compositor
        .wait_for_selection_owner(0, None, Duration::from_secs(3));
    session.compositor.discard_claims(true);
    assert!(
        alpha
            .expect_err("defer 5000 128")
            .contains("ClaimNotHonoured")
    );
    alpha.expect_err(&format!("fulfil {}", to_hex(b"unclaimed")));
    session.compositor.discard_claims(false);
    alpha.copy_text("recovered");
    session.compositor.focus(0, Some(1));
    beta.expect_ok("wait-offer 3000");
    assert_eq!(beta.paste_text(), "recovered");
    alpha.quit();
    beta.quit();
}
