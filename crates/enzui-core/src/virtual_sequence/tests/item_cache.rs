use super::*;

#[test]
fn invalidation_re_fetches_and_returns_the_new_content() {
    struct MutableProvider {
        rows: RefCell<Vec<String>>,
        calls: RefCell<usize>,
    }
    impl ItemProvider<String> for MutableProvider {
        fn len(&self) -> u64 {
            self.rows.borrow().len() as u64
        }
        fn item(&self, index: u64) -> Option<String> {
            *self.calls.borrow_mut() += 1;
            self.rows.borrow().get(index as usize).cloned()
        }
    }
    impl KeyedProvider<String> for MutableProvider {
        fn key(&self, index: u64) -> Option<SequenceKey> {
            if index < self.len() {
                Some(SequenceKey::new(format!("m{index}")))
            } else {
                None
            }
        }
    }

    let p = MutableProvider {
        rows: RefCell::new((0..10).map(|i| format!("v1-{i}")).collect()),
        calls: RefCell::new(0),
    };
    let mut c: ItemCache<String> = ItemCache::new(16);

    assert_eq!(c.items_in(&p, 0..4), vec!["v1-0", "v1-1", "v1-2", "v1-3"]);
    assert_eq!(*p.calls.borrow(), 4);

    *p.rows.borrow_mut() = (0..10).map(|i| format!("v2-{i}")).collect();

    // Stable keys preserve cached values across in-place rewrites until invalidated.
    assert_eq!(
        c.items_in(&p, 0..4),
        vec!["v1-0", "v1-1", "v1-2", "v1-3"],
        "a key-based cache cannot detect a content change by itself"
    );
    assert_eq!(*p.calls.borrow(), 4, "and it did not ask");

    let key1 = SequenceKey::new("m1");
    assert!(
        c.invalidate(&key1),
        "invalidate reports it dropped an entry"
    );
    assert!(
        !c.invalidate(&key1),
        "and reports when there was nothing to drop"
    );
    assert_eq!(
        c.items_in(&p, 0..4),
        vec!["v1-0", "v2-1", "v1-2", "v1-3"],
        "exactly one row refreshed"
    );
    assert_eq!(*p.calls.borrow(), 5, "exactly one re-fetch");

    c.invalidate(&SequenceKey::new("m2"));
    c.invalidate(&SequenceKey::new("m3"));
    assert_eq!(
        c.items_in(&p, 0..4),
        vec!["v1-0", "v2-1", "v2-2", "v2-3"],
        "two more refreshed; row 0 not invalidated so still stale"
    );
    assert_eq!(*p.calls.borrow(), 7);

    c.clear();
    assert_eq!(c.len(), 0);
    assert_eq!(
        c.items_in(&p, 0..4),
        vec!["v2-0", "v2-1", "v2-2", "v2-3"],
        "everything refreshed"
    );
    assert_eq!(*p.calls.borrow(), 11);
}
