//! The GStreamer services' remote effects against a real loopback origin: the
//! curl redirect probe, and a prepared source's approval expiry. Moved here
//! from the core remote gate's tests with the effects themselves.

use enzui_core::media::MediaSource;
use enzui_core::media::remote_gate::test_support::public_effects;
use enzui_core::media::remote_gate::*;
use enzui_core::url_policy::{RemoteSource, UrlPolicy};

fn source(url: &str) -> RemoteSource {
    RemoteSource::checked(url, &UrlPolicy::DEFAULT).expect("the test URL passes the string stage")
}

#[derive(Debug)]
struct FrozenClock(std::sync::atomic::AtomicU64);

impl FrozenClock {
    fn new(now_ms: u64) -> Self {
        FrozenClock(std::sync::atomic::AtomicU64::new(now_ms))
    }
}

impl enzui_core::media::remote_gate::Clock for FrozenClock {
    fn elapsed_ms(&self) -> u64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[test]
fn prepared_source_check_fresh_refuses_a_stale_approval() {
    use crate::media::gst_worker::PreparedSource;
    use crate::media::{MediaError, MediaErrorKind};

    let support = public_effects();
    support.clock.set(5_000);
    let media_source = MediaSource::Remote(source("https://cdn.example.com/movie.mp4"));
    let prepared = PreparedSource::prepare(&media_source, &MediaGate::DEFAULT, &support.effects())
        .expect("the real prepare path must produce an approved remote source");
    let stamp = prepared
        .approved()
        .expect("a prepared remote source carries an approval")
        .approved_at_ms();
    assert!(
        stamp >= 5_000,
        "positive control: the stamp must be non-zero"
    );

    let fresh = FrozenClock::new(stamp);
    prepared
        .check_fresh(&fresh, MediaGate::DEFAULT.approval_ttl_ms)
        .expect("a stamp-time clock must be fresh");

    let at_ttl = FrozenClock::new(stamp + MediaGate::DEFAULT.approval_ttl_ms);
    let err: MediaError = prepared
        .check_fresh(&at_ttl, MediaGate::DEFAULT.approval_ttl_ms)
        .expect_err("at exactly the TTL must be expired");
    assert_eq!(err.kind, MediaErrorKind::Refused);
    assert!(
        err.message.contains("expired"),
        "the refusal message must name expiry, got {:?}",
        err.message
    );

    let rolled_back = FrozenClock::new(stamp - 1);
    prepared
        .check_fresh(&rolled_back, MediaGate::DEFAULT.approval_ttl_ms)
        .expect_err("a rolled-back clock must be refused, not stealth-refreshed");
}
