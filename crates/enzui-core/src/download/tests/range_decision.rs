use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
struct DownloadSnapshot {
    phase: Phase,
    attempt: Attempt,
    received: u64,
    total: Option<u64>,
    pending: Option<Notification>,
}

fn snap(q: &DownloadQueue, id: DownloadId) -> DownloadSnapshot {
    let d = q.get(id).expect("tracked");
    DownloadSnapshot {
        phase: d.phase,
        attempt: d.attempt,
        received: d.received,
        total: d.total(),
        pending: d.pending(),
    }
}

#[test]
fn a_paused_download_still_accepts_a_range_response() {
    let mut q = DownloadQueue::new(2);
    q.admit(DownloadId(1));
    q.promote();
    q.get_mut(DownloadId(1)).unwrap().received = 4096;
    q.get_mut(DownloadId(1)).unwrap().total = None;
    assert!(q.apply(DownloadId(1), DownloadAction::Pause));
    assert_eq!(q.get(DownloadId(1)).unwrap().phase, Phase::Paused);
    let before = snap(&q, DownloadId(1));

    let got = q.handle_range_response(
        DownloadId(1),
        Attempt::FIRST,
        &plan_at(4096),
        &ContentCoding::Identity,
        &resp206("bytes 4096-9999/10000", Some(&etag_a())),
    );

    assert!(
        matches!(got, Some(RangeDecision::Resume { .. })),
        "Paused is not terminal, so a resume must still apply: got {got:?}"
    );
    assert_ne!(
        snap(&q, DownloadId(1)),
        before,
        "and it must actually have been applied"
    );
    assert_eq!(
        q.get(DownloadId(1)).unwrap().total(),
        Some(10_000),
        "the resume recorded the announced total over the seeded 777"
    );
    assert_eq!(
        q.get(DownloadId(1)).unwrap().phase,
        Phase::Paused,
        "and a resume decision does not un-pause the download"
    );
}
