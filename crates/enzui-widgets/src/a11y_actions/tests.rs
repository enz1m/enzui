use super::*;

use enzui_core::a11y_action::AccessibilityAction;
use iced::advanced::widget::Id;
use iced::window;
use iced_test::runtime::Action;
use iced_test::runtime::task::into_stream;

#[derive(Debug, Clone)]
enum Message {
    #[allow(
        dead_code,
        reason = "the widget needs these messages to exist; this test never sends them"
    )]
    Unused,
}

const OVERSIZE: usize = 4096;
const CAP: usize = 1024;

/// One resolved action per [`AccessibilityActionKind`]. The match is exhaustive,
/// so a new kind in core cannot reach the applier without a fixture here.
fn action_for(kind: AccessibilityActionKind, id: Id) -> AccessibilityAction {
    match kind {
        AccessibilityActionKind::Focus => AccessibilityAction::Focus(id),
        AccessibilityActionKind::Activate => AccessibilityAction::Activate(id),
        AccessibilityActionKind::Increment => AccessibilityAction::Increment(id),
        AccessibilityActionKind::Decrement => AccessibilityAction::Decrement(id),
        AccessibilityActionKind::SetValue => AccessibilityAction::SetValue {
            id,
            value: "7".to_owned(),
        },
        AccessibilityActionKind::SetValueRefused => AccessibilityAction::SetValueRefused {
            id,
            len: OVERSIZE,
            cap: CAP,
        },
        AccessibilityActionKind::ShowContextMenu => AccessibilityAction::ShowContextMenu(id),
        AccessibilityActionKind::Expand => AccessibilityAction::Expand(id),
        AccessibilityActionKind::Collapse => AccessibilityAction::Collapse(id),
        AccessibilityActionKind::ScrollIntoView => AccessibilityAction::ScrollIntoView(id),
        AccessibilityActionKind::SetTextSelection => AccessibilityAction::SetTextSelection {
            id,
            anchor: iced::advanced::text::Position { line: 0, index: 0 },
            focus: iced::advanced::text::Position { line: 0, index: 1 },
        },
        AccessibilityActionKind::Unhandled => AccessibilityAction::Unhandled {
            id,
            action: accesskit::Action::ReplaceSelectedText,
        },
    }
}

fn routed(
    window: window::Id,
    kind: AccessibilityActionKind,
    id: &'static str,
) -> RoutedAccessibilityAction {
    RoutedAccessibilityAction::new(window, action_for(kind, Id::new(id)))
}

/// The runtime actions a task carries. A widget task's stream never terminates
/// (its chained receiver is closed by running the operation, which no caller
/// does here), so this stops at the first `Pending` instead of collecting.
fn runtime_actions(task: Task<Message>) -> Vec<Action<Message>> {
    let Some(mut stream) = into_stream(task) else {
        return Vec::new();
    };
    let waker = iced::futures::task::noop_waker();
    let mut context = std::task::Context::from_waker(&waker);
    let mut actions = Vec::new();
    loop {
        match iced::futures::StreamExt::poll_next_unpin(&mut stream, &mut context) {
            std::task::Poll::Ready(Some(action)) => actions.push(action),
            _ => return actions,
        }
    }
}

/// A refused payload must not reach the widget: the router declined it rather
/// than truncating it, and applying an intent here would corrupt the value the
/// client was told was rejected. The accepted `SetValue` beside it is the
/// positive control for the same fixture id.
#[test]
fn a_refused_set_value_only_moves_focus() {
    let window = window::Id::unique();

    let refused = runtime_actions(apply_accessibility_action(&routed(
        window,
        AccessibilityActionKind::SetValueRefused,
        "field",
    )));
    let accepted = runtime_actions(apply_accessibility_action(&routed(
        window,
        AccessibilityActionKind::SetValue,
        "field",
    )));

    assert_eq!(refused.len(), 1, "a refusal applies no value intent");
    assert_eq!(
        accepted.len(),
        2,
        "an accepted SetValue adjusts and focuses"
    );
}
