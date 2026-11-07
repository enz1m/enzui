use super::*;

#[test]
fn a_clean_cancel_terminal_remains_pending_until_acknowledged() {
    let mut d = Download::queued(DownloadId(21));
    d.phase = Phase::Active;

    // Positive control: progress is accepted and drained.
    assert!(d.progress(Attempt::FIRST, 256));
    assert!(d.take_pending().is_some(), "calibration: progress posted");

    assert!(d.cancel(), "Active is cancellable");
    assert_eq!(d.phase, Phase::Cancelled);
    let expected = Notification::Terminal {
        attempt: Attempt::FIRST,
        phase: Phase::Cancelled,
    };
    assert_eq!(d.pending(), Some(expected));

    // take_pending must not consume a terminal notification.
    assert_eq!(d.take_pending(), Some(expected));
    assert_eq!(
        d.pending(),
        Some(expected),
        "a terminal notification survives drain"
    );

    // A stale-attempt acknowledge must not clear the slot.
    assert!(
        !d.acknowledge(Attempt(99)),
        "mismatched attempt clears nothing"
    );
    assert_eq!(d.pending(), Some(expected), "still pending");

    assert!(
        d.acknowledge(Attempt::FIRST),
        "the correct attempt clears it"
    );
    assert_eq!(d.pending(), None, "acknowledged and cleared");
    assert!(
        !d.acknowledge(Attempt::FIRST),
        "a second acknowledge is a no-op"
    );
}
