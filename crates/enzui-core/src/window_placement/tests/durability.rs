use super::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

fn key(name: &str) -> PlacementKey {
    PlacementKey::sanitized(name)
}

#[test]
fn an_injected_write_failure_commits_nothing_and_the_retry_still_writes() {
    let dir = TempDir::new("durability-failure");
    let path = dir.state_file();

    let tracker = Mutex::new(PlacementTracker::new());
    {
        let mut guard = tracker.lock().unwrap();
        guard.record_resize(key("main"), (900.0, 700.0));
    }

    let attempts = AtomicUsize::new(0);
    let err = flush_shared_with(&tracker, &path, |_p, _snapshot| {
        attempts.fetch_add(1, Ordering::SeqCst);
        Err(std::io::Error::other("injected fsync failure"))
    })
    .expect_err("an injected durability failure must be reported, not swallowed");
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(
        err.to_string().contains("injected fsync failure"),
        "the underlying error must reach the caller; got {err}"
    );

    {
        let guard = tracker.lock().unwrap();
        assert!(
            guard.has_pending(),
            "a failed write must leave the pending set intact for the retry"
        );
        assert_eq!(
            guard
                .pending_snapshot()
                .get(&key("main"))
                .map(|p| p.normal_size),
            Some((900.0, 700.0))
        );
    }
    assert!(
        !path.exists(),
        "nothing may be published when the durable write failed"
    );

    flush_shared(&tracker, &path).expect("retry must write");
    let reloaded = match load(&path) {
        LoadOutcome::Loaded(map) => map,
        LoadOutcome::UsedDefaults { note } => panic!("retry must produce a loadable file: {note}"),
    };
    assert_eq!(
        reloaded.get(&key("main")).map(|p| p.normal_size),
        Some((900.0, 700.0)),
        "the retained pending value must be what the retry writes"
    );
}

// Handoff to the child fixture: the child runs the recursive-writer scenario if and only if
// this variable is set, and clears it before doing anything so any further subprocess it would
// ever inherit an empty ambient env for this key rather than fork-bombing.
