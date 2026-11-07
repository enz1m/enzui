use super::*;

#[test]
fn snapshots_share_unchanged_text_and_edits_are_exact() {
    let original = "alpha\ncafé\nomega\n".repeat(10_000);
    let mut store = TextStore::from_string(original.clone());
    let before = store.snapshot();
    let start = before.byte_of_line_column(1, 0);
    let end = before.byte_of_line_column(2, 0);
    store.replace(start..end, "BETA\n".to_owned()).unwrap();
    let after = store.snapshot();

    assert!(before.eq_str(&original), "immutable snapshot changed");
    let mut expected = original;
    expected.replace_range(start..end, "BETA\n");
    assert!(after.eq_str(&expected));
    assert!(before.shares_source_with(&after));
    assert_eq!(after.line_count(), before.line_count());
    assert!(after.chunk_count() <= before.chunk_count() + 3);
}
