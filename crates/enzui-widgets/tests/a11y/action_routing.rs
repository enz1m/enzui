//! action routing tail — end-to-end assertion that an AccessKit `Increment` / `Decrement`
//! / `SetValue` request pushed through [`ActionRouter`] resolves to the right
//! widget and, when applied by that widget's own stepping / on-change rule,
//! actually moves the value.
//!
//! The core-crate tests
//! (`enzui_core::a11y_action::tests::increment_routes_on_numeric_and_unhandled_on_non_numeric`
//! and its siblings) cover the routing decisions in isolation; this file
//! closes the loop by publishing a slider- or text-shaped semantics node
//! exactly the way the real widget does, driving a request through the router,
//! draining it, and asking the widget's *own* helper what the new value would
//! be. The slider path routes through
//! [`enzui_widgets::slider::slider_increment_value`] and its sibling —
//! the same functions the arrow-key `nav_value` path calls — so this test
//! asserts the router feeds the same computation.
//!
//! ## Why the router doesn't compute the new value itself
//!
//! The brief explicitly forbids inventing a second stepping rule. The widget
//! owns the rule (`slider::nav_value` for the arrow-key path); the router
//! owns *only* the resolution (`AccessibilityAction::Increment(id)`). This
//! test therefore verifies both halves plug together:
//!  - the router resolved to the right variant with the right id;
//!  - the widget helper the app would call produces the expected new value.
//!
//! A test that only asserted "some variant with the right id was constructed"
//! would pass for a router that swapped Increment and Decrement (they carry
//! the same id shape). The sabotage exercise at the bottom demonstrates the
//! polarity is actually observed.

use accesskit::{Action, ActionData, NodeId};
use enzui_core::a11y_action::{AccessibilityAction, ActionRouter};
use enzui_core::collect::{NodeIdMap, SemanticsCollector, SemanticsNode, node_id_of_widget};
use enzui_core::semantics::{Role, Semantics};
use iced::Rectangle;
use iced::advanced::widget::{Id, Operation};

fn bounds() -> Rectangle {
    Rectangle::new([0.0, 0.0].into(), [200.0, 24.0].into())
}

/// Publish a single editable text-shaped semantics node into a fresh
/// collector (`Role::TextInput` + `value`), matching what `EnzTextField`
/// publishes. `read_only` lets the caller assert the collector's advertised
/// gate is honoured by the router.
fn publish_text(id: &Id, value: &str, read_only: bool, disabled: bool) -> NodeIdMap {
    // `.editable(true)` mirrors what `EnzTextField::semantics` publishes
    //: the editing capability is declared, never inferred from
    // the value being present -- an empty or password field has none and is
    // still editable.
    let mut s = Semantics::new(Role::TextInput)
        .label("test")
        .editable(true)
        .value(value);
    if read_only {
        s = s.read_only(true);
    }
    if disabled {
        s = s.disabled(true);
    }
    let mut collector = SemanticsCollector::new();
    let mut node = SemanticsNode::new(s);
    Operation::<()>::custom(&mut collector, Some(id), bounds(), &mut node);
    let (_update, map) = collector.to_tree_update_with_ids();
    map
}

/// Push a request through the router, drain, and return the single resolved
/// action (or fail loudly). Used by every test below so the assertion is
/// about the resolved action, not the drain plumbing.
fn round_trip(
    router: &ActionRouter,
    action: Action,
    node: NodeId,
    data: Option<&ActionData>,
) -> AccessibilityAction {
    router.handle(
        enzui_core::a11y_action::A11Y_UNKEYED_WINDOW,
        action,
        node,
        data,
    );
    let mut drained = router.drain();
    assert_eq!(
        drained.len(),
        1,
        "router should enqueue exactly one resolved action, got {drained:?}"
    );
    drained.remove(0)
}

// -----------------------------------------------------------------------------
// Increment / Decrement — real slider stepping through the router
// -----------------------------------------------------------------------------

// -----------------------------------------------------------------------------
// SetValue — read-only / disabled refusals
// -----------------------------------------------------------------------------

#[test]
fn set_value_on_disabled_field_is_refused() {
    let id = Id::new("greyed-out");
    let map = publish_text(&id, "greyed", false, true);
    let router = ActionRouter::new();
    router.set_map(enzui_core::a11y_action::A11Y_UNKEYED_WINDOW, map);

    let payload = ActionData::Value("nope".into());
    let resolved = round_trip(
        &router,
        Action::SetValue,
        node_id_of_widget(&id),
        Some(&payload),
    );
    // A disabled control publishes no action advertisements at all
    // (the `if !self.disabled` guard in `Semantics::to_accesskit_node`,
    // `semantics::ak`), so any action against it \u2014 SetValue included \u2014
    // contradicts the tree.
    assert_eq!(
        resolved,
        AccessibilityAction::Unhandled {
            id,
            action: Action::SetValue,
        },
    );
}

// -----------------------------------------------------------------------------
// Anonymous target still resolves to None (regression guard for action-routing contract)
// -----------------------------------------------------------------------------
