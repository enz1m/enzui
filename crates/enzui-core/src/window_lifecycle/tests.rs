use super::*;

fn ids<const N: usize>() -> [window::Id; N] {
    std::array::from_fn(|_| window::Id::unique())
}

#[test]
fn reopening_the_primary_repoints_it() {
    let [first, second, preview, old, kept, fresh] = ids();
    let mut ledger = WindowLedger::new();

    // Reuse after a full close: once the ledger has drained to Exit, a new
    // `opened_primary` claims the vacated primary slot and drives the cascade.
    ledger.opened_primary(first);
    assert_eq!(ledger.closed(first), LifecycleAction::Exit);

    ledger.opened_primary(second);
    ledger.opened(preview);
    assert_eq!(ledger.primary(), Some(second));

    let action = ledger.closed(second);
    assert!(
        matches!(action, LifecycleAction::CloseThenExit(ref r) if r == &vec![preview]),
        "the reopened main window must still be the primary, got {action:?}"
    );
    assert_eq!(
        ledger.open_count(),
        0,
        "the reuse scenario drains the ledger"
    );

    // Repoint the primary while the former primary is still open, alongside a
    // preserved secondary. The former primary is demoted to an ordinary open
    // window: it keeps its slot but no longer holds the primary pointer.
    ledger.opened_primary(old);
    ledger.opened(kept);
    ledger.opened_primary(fresh);

    assert_eq!(
        ledger.primary(),
        Some(fresh),
        "the newest primary wins the pointer"
    );
    assert_eq!(
        ledger.open_count(),
        3,
        "repointing keeps the former primary and the secondary open"
    );
    assert!(
        ledger.is_open(old) && ledger.is_open(kept) && ledger.is_open(fresh),
        "the demoted former primary must remain open, not vanish on repoint"
    );

    // Closing the new primary cascades exactly the two survivors once, then
    // exits: the demoted former primary is swept up as an ordinary dependent
    // rather than driving its own exit, and the ledger clears so the
    // dependents' own Closed echoes cannot fire a second exit.
    let action = ledger.closed(fresh);
    assert!(
        action.exits(),
        "closing the primary under CloseEverything must terminate the app"
    );
    let LifecycleAction::CloseThenExit(remaining) = action else {
        panic!("expected CloseThenExit, got {action:?}");
    };
    let mut expected = vec![old, kept];
    expected.sort_unstable();
    assert_eq!(
        remaining, expected,
        "the former primary is cascaded as a dependent beside the secondary"
    );
    assert_eq!(
        ledger.open_count(),
        0,
        "the cascade clears every surviving window exactly once"
    );
}
