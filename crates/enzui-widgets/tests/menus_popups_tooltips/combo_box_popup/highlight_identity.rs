// `include!`d by `combo_box_popup.rs` — a TEXT move, not a nested `mod`, so
// every test below keeps its original `combo_box_popup::<test>` path. Inner
// doc comments (`//!`) are impossible in an included fragment, so the file
// prose uses `//`.

// ---------------------------------------------------------------------------
// Mutation identity — highlight reconciles by identity across live mutation
// ---------------------------------------------------------------------------
//
// A highlight stored as a flat INDEX would silently retarget to a different
// option when one is inserted or removed above it. The popup stores the
// option's identity and re-resolves the index every frame via
// `tree_nav::reconcile_cursor`, the shared reconciliation policy.
//
// The sentinel keeps every option DISTINGUISHABLE (distinct labels), so a
// wrong read lands on a different observable than a right one.

/// Insertion above the highlight. Highlight "Cherry", then prepend a new
/// option. The highlight must still name "Cherry", which is now one slot lower
/// — not the option that drifted into Cherry's old index.
#[test]
fn inserting_above_the_highlight_keeps_it_on_the_same_option() {
    let mut h = Host::with(
        vec![
            "Apple".into(),
            "Banana".into(),
            "Cherry".into(),
            "Date".into(),
        ],
        None,
    );
    h.focus();
    h.frame(&[key(Named::Enter)]);
    h.frame(&[key(Named::Home)]); // Apple (index 0)
    h.frame(&[key(Named::ArrowDown)]); // Banana
    h.frame(&[key(Named::ArrowDown)]); // Cherry (index 2)
    assert_eq!(
        h.highlighted().as_deref(),
        Some("Cherry"),
        "setup: on Cherry"
    );

    // Prepend an option: Cherry moves from index 2 to index 3.
    h.options = vec![
        "Zero".into(),
        "Apple".into(),
        "Banana".into(),
        "Cherry".into(),
        "Date".into(),
    ];
    // A frame with no key: reconciliation runs in operate/update.
    let _ = h.frame(&[]);

    assert_eq!(
        h.highlighted().as_deref(),
        Some("Cherry"),
        "after inserting an option above it, the highlight must still name \
         Cherry — an index-only highlight would now point at Banana"
    );
}
