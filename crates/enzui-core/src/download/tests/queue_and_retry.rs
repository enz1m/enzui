use super::*;

#[test]
fn a_reply_from_the_previous_attempt_is_rejected() {
    let mut d = Download::queued(DownloadId(1));
    d.retry();
    assert!(
        !d.accepts(Attempt::FIRST),
        "attempt 0 is stale after a retry"
    );
    assert!(d.accepts(Attempt(1)), "attempt 1 is current");
}
