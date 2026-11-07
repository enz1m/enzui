//! Turns resolved accessibility actions into the widget operations the pointer
//! and keyboard paths already use, scoped to the window that published them.
//!
//! [`crate::app_setup::run`] applies every action for the application. An
//! application that runs its own loop, or opts out with
//! `AppServices::accessibility_actions(false)`, drains accessibility work in
//! `update` and hands the receipt to [`apply_accessibility_wake`]; ordinary
//! turns use [`apply_pending_accessibility_actions`]. Both return one
//! [`Task`] whose operations run in the exact window that published
//! the target node, so a second window built from the same `view` is untouched.
//!
//! An application that adds its own work for a kind — a message its widgets
//! cannot publish through an operation — iterates the drained actions itself and
//! batches [`apply_accessibility_action`] with that work.
//!
//! Each kind maps to the widget's own path: activation focuses the target, as
//! a press does, and arms its real `on_press` line, value changes hand the widget an intent it clamps itself,
//! and a reveal scrolls the nearest ancestor scrollable. A refused `SetValue`
//! moves focus and changes no value, and an unmapped action performs nothing.
//!
//! Builds without `enzui-core`'s `accesskit` feature keep every call site:
//! no action can be constructed and both drains are empty, so no application
//! needs a `cfg` fork around its accessibility handling.

use enzui_core::app_runtime::{
    self, AccessibilityActionKind, AccessibilityWake, RoutedAccessibilityAction,
};
use iced::Task;

use crate::value_action::{ValueIntent, adjust_value};

/// Applies one resolved action in the window that published its target.
///
/// [`AccessibilityActionKind::Unhandled`] performs no operation; a `SetValue`
/// carrying no payload only moves focus.
pub fn apply_accessibility_action<Message>(routed: &RoutedAccessibilityAction) -> Task<Message>
where
    Message: 'static,
{
    let window = routed.window();
    let id = routed.widget().clone();

    match routed.kind() {
        AccessibilityActionKind::Focus => app_runtime::window_focus_task(window, id),
        // A press focuses the control it lands on and every other control lets
        // go; activation does the same first, so what it opens owns Escape.
        AccessibilityActionKind::Activate => Task::batch([
            app_runtime::window_focus_task(window, id.clone()),
            app_runtime::window_operation_task(window, crate::activation::activate(id)),
        ]),
        AccessibilityActionKind::Increment => adjust_then_focus(window, id, ValueIntent::Increment),
        AccessibilityActionKind::Decrement => adjust_then_focus(window, id, ValueIntent::Decrement),
        // The payload stays unparsed: only the widget knows the value's type and
        // bounds, so parsing here could not clamp a hostile one.
        AccessibilityActionKind::SetValue => match routed.value() {
            Some(value) => adjust_then_focus(window, id, ValueIntent::Set(value.to_owned())),
            None => app_runtime::window_focus_task(window, id),
        },
        // The router refused the payload rather than truncating it, so the
        // control the client addressed takes focus and keeps its value.
        AccessibilityActionKind::SetValueRefused => app_runtime::window_focus_task(window, id),
        AccessibilityActionKind::ShowContextMenu => app_runtime::window_operation_task(
            window,
            crate::context_menu_action::show_context_menu(id),
        ),
        // The separate toggle carrier prevents expand/collapse from selecting a tree row.
        AccessibilityActionKind::Expand | AccessibilityActionKind::Collapse => {
            app_runtime::window_operation_task(window, crate::activation::expand(id))
        }
        AccessibilityActionKind::ScrollIntoView => {
            app_runtime::window_operation_task(window, enzui_core::focus::reveal_id::<()>(id))
        }
        // The input moves its own caret; the next frame publishes it.
        AccessibilityActionKind::SetTextSelection => match routed.text_selection() {
            Some((anchor, focus)) => app_runtime::window_operation_task(
                window,
                iced::advanced::widget::operation::text_input::select_range(id, anchor, focus),
            ),
            None => Task::none(),
        },
        AccessibilityActionKind::Unhandled => Task::none(),
    }
}

/// Applies every action in `actions`, each in its own publishing window.
pub fn apply_accessibility_actions<Message>(
    actions: impl IntoIterator<Item = RoutedAccessibilityAction>,
) -> Task<Message>
where
    Message: 'static,
{
    Task::batch(
        actions
            .into_iter()
            .map(|routed| apply_accessibility_action(&routed)),
    )
}

/// Drains the batch named by `receipt` and applies it.
///
/// A receipt whose batch an earlier turn already drained, or whose final window
/// closed, applies nothing.
pub fn apply_accessibility_wake<Message>(receipt: AccessibilityWake) -> Task<Message>
where
    Message: 'static,
{
    match app_runtime::drain_accessibility_wake(receipt) {
        Some(actions) => apply_accessibility_actions(actions),
        None => Task::none(),
    }
}

/// Drains the actions resolved since the last drain and applies them.
///
/// Call this on ordinary update turns; the wake receipt covers the idle path.
pub fn apply_pending_accessibility_actions<Message>() -> Task<Message>
where
    Message: 'static,
{
    apply_accessibility_actions(app_runtime::drain_accessibility_actions())
}

// The widget applies the intent and publishes through its own change line; the
// follow-up focus leaves the client on the control it just moved.
fn adjust_then_focus<Message>(
    window: iced::window::Id,
    id: iced::advanced::widget::Id,
    intent: ValueIntent,
) -> Task<Message>
where
    Message: 'static,
{
    Task::batch([
        app_runtime::window_operation_task(window, adjust_value(id.clone(), intent)),
        app_runtime::window_focus_task(window, id),
    ])
}

#[cfg(test)]
mod tests;
