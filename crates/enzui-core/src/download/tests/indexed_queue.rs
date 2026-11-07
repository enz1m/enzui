use super::*;

fn assert_index_matches(q: &DownloadQueue, expected_order: &[DownloadId], universe: &[DownloadId]) {
    assert_eq!(
        q.iter().map(|download| download.id).collect::<Vec<_>>(),
        expected_order,
        "the Vec remains in model admission order"
    );
    assert_eq!(
        q.indices.len(),
        expected_order.len(),
        "the index has exactly one entry per tracked download"
    );

    for (expected_index, id) in expected_order.iter().copied().enumerate() {
        assert_eq!(
            q.indices.get(&id),
            Some(&expected_index),
            "id {id:?} maps to its current Vec slot"
        );
        assert_eq!(
            q.get(id).map(|download| download.id),
            Some(id),
            "indexed public lookup returns the requested id, never an alias"
        );
    }
    for id in universe {
        if !expected_order.contains(id) {
            assert!(
                q.get(*id).is_none(),
                "evicted/absent id {id:?} is not stale"
            );
            assert!(
                !q.indices.contains_key(id),
                "evicted/absent id {id:?} has no stale index entry"
            );
        }
    }
}

#[test]
fn indexed_admit_restore_duplicate_get_and_mutation_paths_agree() {
    let mut q = DownloadQueue::new(2);
    assert!(q.admit(DownloadId(41)));
    assert!(q.admit(DownloadId(7)));
    assert!(!q.admit(DownloadId(41)), "duplicate admit is refused");

    let mut restored = Download::queued(DownloadId(90));
    restored.attempt = Attempt(3);
    restored.phase = Phase::Paused;
    restored.received = 700;
    assert!(q.restore(restored.clone()));
    assert!(!q.restore(restored), "duplicate restore is refused");

    assert!(q.set_total(DownloadId(7), Attempt::FIRST, Some(1_000)));
    assert!(q.progress(DownloadId(7), Attempt::FIRST, 250));
    assert_eq!(q.get(DownloadId(7)).and_then(Download::total), Some(1_000));
    assert_eq!(q.get(DownloadId(7)).map(|d| d.received), Some(250));
    assert!(matches!(
        q.take_pending(DownloadId(7)),
        Some(Notification::Progress { received: 250, .. })
    ));
    assert_eq!(q.take_pending(DownloadId(7)), None);

    assert!(q.finish(DownloadId(41), Attempt::FIRST, Phase::Complete));
    assert!(matches!(
        q.take_pending(DownloadId(41)),
        Some(Notification::Terminal {
            phase: Phase::Complete,
            ..
        })
    ));
    assert!(q.acknowledge(DownloadId(41), Attempt::FIRST));
    assert_eq!(q.take_pending(DownloadId(41)), None);

    assert_index_matches(
        &q,
        &[DownloadId(41), DownloadId(7), DownloadId(90)],
        &[
            DownloadId(7),
            DownloadId(41),
            DownloadId(90),
            DownloadId(999),
        ],
    );
}
