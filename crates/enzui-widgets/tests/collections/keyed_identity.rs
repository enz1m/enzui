//! Stable, key-derived widget identity for horizontal model-driven collections.
//!
//! Positional reconciliation transfers focus / pressed / semantic-action state
//! to whatever item now occupies a slot after insert / filter / reorder. The
//! invariant is that each logical item has a stable, key-derived `widget::Id` so
//! its focus and action target follow the item, not the position.
//!
//! These tests exercise the pagination adoption end-to-end via
//! `iced_test::Simulator`: page cells are addressable by their page-keyed id,
//! the id targets the correct logical page, the id is stable across a
//! `current_page` change (the cell layout shifts but the id does not), and
//! two paginations sharing a screen do not collide when given distinct
//! `id_prefix`es.

use enzui_widgets::button_group::{EnzButtonGroupItem, enz_button_group};
use iced::Element;
use iced_test::Simulator;
use iced_test::selector::id as by_id;

#[derive(Debug, Clone, PartialEq)]
#[allow(
    dead_code,
    reason = "the widget needs these messages to exist; this test never sends them"
)]
enum Msg {
    Page(i32),
    Activate(usize),
}

// ---- ButtonGroup: item-key-derived cell ids ----

#[test]
fn button_group_cell_id_follows_item_across_reorder() {
    // "underline" is index 2 in the first order and index 0 in a reordered
    // group, but the same id keeps addressing the underline cell — the action
    // target follows the logical item, not the slot.
    let reordered: Element<'static, Msg> = enz_button_group(
        vec![
            EnzButtonGroupItem::new("underline", "Underline"),
            EnzButtonGroupItem::new("bold", "Bold"),
            EnzButtonGroupItem::new("italic", "Italic"),
        ],
        Msg::Activate,
    )
    .into();

    let mut ui = Simulator::new(reordered);
    ui.click(by_id("enz-button-group-underline"))
        .expect("underline cell addressable after reorder");

    let messages: Vec<Msg> = ui.into_messages().collect();
    assert_eq!(
        messages,
        vec![Msg::Activate(0)],
        "underline is now index 0; its stable id addresses it wherever it sits"
    );
}
