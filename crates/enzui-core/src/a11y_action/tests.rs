use super::*;
use crate::collect::NodeIdMap;
use crate::collect::{SemanticsCollector, SemanticsNode, node_id_of_widget};
use crate::semantics::{Role, Semantics};
use accesskit::Action;
use iced_core::Rectangle;
use iced_core::widget::Id;
use iced_core::widget::Operation;
use std::sync::Arc;

fn bounds() -> Rectangle {
    Rectangle::new([0.0, 0.0].into(), [80.0, 32.0].into())
}

fn build_map() -> (NodeIdMap, Id, Id) {
    let save = Id::new("save");
    let cancel = Id::new("cancel");
    let mut collector = SemanticsCollector::new();

    let mut n = SemanticsNode::new(Semantics::new(Role::Button).label("Save").default_action());
    Operation::<()>::custom(&mut collector, Some(&save), bounds(), &mut n);

    let mut n = SemanticsNode::new(
        Semantics::new(Role::Button)
            .label("Cancel")
            .default_action(),
    );
    Operation::<()>::custom(&mut collector, Some(&cancel), bounds(), &mut n);

    let mut n = SemanticsNode::new(Semantics::new(Role::Button).label("Anon"));
    Operation::<()>::custom(&mut collector, None, bounds(), &mut n);

    let (_update, map) = collector.to_tree_update_with_ids();
    (map, save, cancel)
}

#[test]
fn queue_recovers_after_a_full_refusal() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (map, save, cancel) = build_map();
    let router = ActionRouter::new();
    let wakes = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&wakes);
    let _wake_registration = router.set_wake(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    });
    router.set_map(A11Y_UNKEYED_WINDOW, map);
    router.set_budget(ActionRouterBudget {
        max_pending: 1,
        max_payload_bytes: 64,
    });

    assert!(
        router
            .handle(
                A11Y_UNKEYED_WINDOW,
                Action::Focus,
                node_id_of_widget(&save),
                None
            )
            .is_some()
    );
    assert_eq!(wakes.load(Ordering::SeqCst), 1);
    assert!(
        router
            .handle(
                A11Y_UNKEYED_WINDOW,
                Action::Click,
                node_id_of_widget(&cancel),
                None
            )
            .is_none()
    );
    assert_eq!(
        wakes.load(Ordering::SeqCst),
        1,
        "a refused enqueue must not wake"
    );
    assert_eq!(router.dropped_full(), 1);
    assert_eq!(router.drain().len(), 1);
    assert!(
        router
            .handle(
                A11Y_UNKEYED_WINDOW,
                Action::Click,
                node_id_of_widget(&cancel),
                None
            )
            .is_some()
    );
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
    assert_eq!(router.drain(), vec![AccessibilityAction::Activate(cancel)]);
}
