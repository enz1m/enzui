use super::*;

#[test]
fn cancel_produces_exactly_one_terminal_transition() {
    let mut d = Download::queued(DownloadId(21));
    d.phase = Phase::Active;
    assert!(d.cancel());
    assert_eq!(d.phase, Phase::Cancelled);
    assert_eq!(
        d.pending(),
        Some(Notification::Terminal {
            attempt: Attempt::FIRST,
            phase: Phase::Cancelled,
        })
    );
    assert!(!d.cancel(), "cancelling twice must be refused");
    assert!(
        !d.apply(DownloadAction::Cancel),
        "and refused through `apply` too"
    );
    assert_eq!(d.phase, Phase::Cancelled);
}
