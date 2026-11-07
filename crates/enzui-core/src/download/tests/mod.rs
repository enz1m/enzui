use super::*;

mod capabilities;
mod completed_history;
mod indexed_queue;
mod persistence;
mod progress_and_states;
mod queue_and_retry;
mod range_decision;
mod range_protocol;
mod transfer;
mod worker;

struct TmpDir(std::path::PathBuf);

impl TmpDir {
    fn new(tag: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "enzui-dl-state-{}-{}-{tag}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("temp dir");
        Self(p)
    }
    fn path(&self, n: &str) -> std::path::PathBuf {
        self.0.join(n)
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn finish_complete(q: &mut DownloadQueue, id: u64) {
    let attempt = q.get(DownloadId(id)).expect("tracked").attempt;
    assert!(
        q.finish(DownloadId(id), attempt, Phase::Complete),
        "finishing {id} to Complete must succeed"
    );
}

fn etag_a() -> Validator {
    Validator::parse_etag(r#""etag-a""#).expect("well-formed strong etag")
}

fn plan_at(offset: u64) -> RangeRequestPlan {
    RangeRequestPlan::conditional(offset, etag_a()).expect("strong validator makes a plan")
}

fn resp206<'a>(content_range: &'a str, validator: Option<&'a Validator>) -> RangeResponse<'a> {
    RangeResponse {
        status: 206,
        content_range: Some(content_range),
        response_validator: validator,
        content_length: None,
        content_encoding: None,
    }
}
