use super::*;
use std::sync::atomic::{AtomicUsize, Ordering as O};
use std::task::Waker;

fn a_key() -> CredentialKey {
    CredentialKey::new("org.example.remote", "profile:6f2a-…", "password")
}

fn poll_once<F: Future + Unpin>(fut: &mut F) -> Poll<F::Output> {
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    Pin::new(fut).poll(&mut cx)
}

fn drive_to_completion<F: Future + Unpin>(mut fut: F) -> F::Output {
    let attempts = AtomicUsize::new(0);
    loop {
        match poll_once(&mut fut) {
            Poll::Ready(v) => return v,
            Poll::Pending => {
                let n = attempts.fetch_add(1, O::SeqCst);
                assert!(
                    n < 1_000,
                    "drive_to_completion looped 1000 times without progress; a test \
                     forgot to open its Gate or to cancel its handle"
                );
                std::thread::yield_now();
            }
        }
    }
}

#[test]
fn security_gate_nothing_reaches_disk() {
    let store = UnavailableCredentialStore;
    let spy = MemoryCredentialStore::new();
    let key = a_key();

    assert_eq!(spy.entry_count(), 0);

    let outcome = drive_to_completion(store.set(key, Secret::from_string("do-not-store".into())));
    assert_eq!(
        outcome,
        CredentialOutcome::Unavailable,
        "an Unavailable store must return Unavailable — anything else \
         (including a silent success) is the vulnerability this seam \
         exists to prevent",
    );
    assert!(
        outcome.is_unavailable(),
        "is_unavailable() must recognise its own arm — the caller uses \
         this predicate to decide \"tell the user we could not store \
         the password\"; if it returns false the UI silently succeeds",
    );

    assert_eq!(
        spy.entry_count(),
        0,
        "a call through UnavailableCredentialStore must not have touched \
         any other store — the whole point is that no bytes escape",
    );
}
