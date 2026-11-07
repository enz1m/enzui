use super::*;

#[test]
fn cancel_through_apply_prunes_the_history() {
    let mut q = DownloadQueue::with_history_limit(5, 1);
    for id in [1_u64, 2, 3] {
        assert!(q.admit(DownloadId(id)));
    }
    finish_complete(&mut q, 1);
    finish_complete(&mut q, 2);
    assert!(q.apply(DownloadId(3), DownloadAction::Cancel));
    let survivors: Vec<u64> = q.iter().map(|d| d.id.0).collect();
    assert_eq!(
        survivors,
        vec![3],
        "the cancel path prunes: only the newest terminal (3) survives"
    );
}
