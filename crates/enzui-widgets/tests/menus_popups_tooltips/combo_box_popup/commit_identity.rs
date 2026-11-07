// `include!`d by `combo_box_popup.rs` — a TEXT move, not a nested `mod`, so
// every test below keeps its original `combo_box_popup::<test>` path. Inner
// doc comments (`//!`) are impossible in an included fragment, so the file
// prose uses `//`.

// ---------------------------------------------------------------------------
// Mutation identity — the COMMITTED value after a live mutation
// ---------------------------------------------------------------------------
//
// The tests above assert where the *highlight* lands. These assert what is
// actually published, which is the user-visible consequence and the thing
// the issue reports: "the user commits a different option than the one they
// saw". A highlight assertion alone would pass against a widget that tracked
// identity for display and then committed `options[active_index]` from a
// pre-mutation index.
//
// The sentinel keeps every option distinguishable, and the mutation is chosen
// so the identity-correct answer and the index-only answer are different
// strings — otherwise the assertion could not fail.

/// The same claim for the pointer path: highlight moves with identity, and
/// a click on the row the user sees publishes that row's value after a
/// mutation has shifted every index.
#[test]
fn clicking_after_a_removal_publishes_the_row_that_was_clicked() {
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
    h.frame(&[key(Named::Enter)]); // open

    // Remove the first option while the popup is open: every remaining option
    // shifts up one slot, so a stale row->index mapping would commit the wrong
    // value. Row 1 of the new list is "Cherry"; under the old indices row 1
    // was "Banana".
    h.options = vec!["Banana".into(), "Cherry".into(), "Date".into()];
    let _ = h.frame(&[]); // let the rebuild land

    let row1 = h.row_center(1).expect("second row center");
    let published = h.click_at(row1);
    assert_eq!(
        published,
        vec![Msg::Pick("Cherry".into())],
        "a click must publish the value of the row actually under the cursor \
         after the option list changed"
    );
}
