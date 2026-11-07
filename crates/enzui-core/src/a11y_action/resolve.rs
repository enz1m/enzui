//! Pure resolution of AccessKit node requests into widget-level actions.

use crate::collect::NodeIdMap;
use accesskit::{Action, ActionData, NodeId};
use iced_core::widget::Id;

/// A routed widget interaction, including explicit refusal and unsupported outcomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessibilityAction {
    /// Moves keyboard focus to the target widget.
    Focus(Id),
    /// Activates the widget through the same path as keyboard or pointer activation.
    Activate(Id),
    /// Increments the widget by its own step and lets the widget clamp the result.
    Increment(Id),
    /// Decrements the widget by its own step.
    Decrement(Id),
    /// Replaces a text or numeric widget value using an unvalidated string payload.
    ///
    /// Text is passed through and numeric AccessKit data is formatted as text; the
    /// receiving widget remains responsible for parsing and clamping.
    SetValue {
        /// Target widget.
        id: Id,
        /// Unvalidated replacement value.
        value: String,
    },
    /// Opens the target context menu when the widget publishes that capability.
    ShowContextMenu(Id),
    /// Expands a capable widget that currently publishes a collapsed state.
    Expand(Id),
    /// Collapses a capable widget that currently publishes an expanded state.
    Collapse(Id),
    /// Requests minimum-motion reveal in the nearest ancestor scrollable.
    ///
    /// Any routable node is accepted; an already visible target or one without a
    /// scrollable ancestor produces no movement.
    ScrollIntoView(Id),
    /// Moves the caret of a text input, or selects text: `anchor` stays
    /// where the selection starts and the caret goes to `focus`.
    SetTextSelection {
        /// Target widget.
        id: Id,
        /// Where the selection starts.
        anchor: iced_core::text::Position,
        /// Where the caret goes.
        focus: iced_core::text::Position,
    },
    /// Reports a routable request that conflicts with the widget contract or has no mapping.
    Unhandled {
        /// Target widget.
        id: Id,
        /// Unsupported AccessKit action.
        action: Action,
    },
    /// Reports a `SetValue` payload refused for exceeding the byte cap.
    ///
    /// The value is never truncated or copied into this outcome.
    SetValueRefused {
        /// Target widget.
        id: Id,
        /// Submitted payload length in bytes.
        len: usize,
        /// Byte cap applied to the payload.
        cap: usize,
    },
}

impl AccessibilityAction {
    /// The widget [`Id`] this resolved action targets, whichever variant it is.
    pub fn id(&self) -> &Id {
        match self {
            AccessibilityAction::Focus(id)
            | AccessibilityAction::Activate(id)
            | AccessibilityAction::Increment(id)
            | AccessibilityAction::Decrement(id)
            | AccessibilityAction::SetValue { id, .. }
            | AccessibilityAction::ShowContextMenu(id)
            | AccessibilityAction::Expand(id)
            | AccessibilityAction::Collapse(id)
            | AccessibilityAction::ScrollIntoView(id)
            | AccessibilityAction::SetTextSelection { id, .. }
            | AccessibilityAction::Unhandled { id, .. }
            | AccessibilityAction::SetValueRefused { id, .. } => id,
        }
    }
}

/// Resolves an AccessKit request against a node map.
///
/// Returns `None` for an unroutable node, `Unhandled` for a routable request
/// outside the widget contract, and a concrete action otherwise.
/// `ScrollIntoView` accepts any routable node.
pub fn resolve_action(
    action: Action,
    target: NodeId,
    data: Option<&ActionData>,
    map: &NodeIdMap,
) -> Option<AccessibilityAction> {
    let id = map.widget_id(target)?.clone();
    let routing = map.routing(target).copied();
    Some(match action {
        Action::Focus => AccessibilityAction::Focus(id),
        // Keep this gate aligned with the collector: only enabled nodes advertising a default action may activate.
        Action::Click => match routing {
            Some(r) if r.supports_default_action && !r.disabled => {
                AccessibilityAction::Activate(id)
            }
            _ => AccessibilityAction::Unhandled {
                id,
                action: Action::Click,
            },
        },
        Action::Increment => match routing {
            // Range actions require the same adjustable, writable, enabled numeric contract the collector advertises.
            Some(r) if r.adjustable && !r.read_only && !r.disabled && r.numeric.is_some() => {
                AccessibilityAction::Increment(id)
            }
            _ => AccessibilityAction::Unhandled {
                id,
                action: Action::Increment,
            },
        },
        Action::Decrement => match routing {
            Some(r) if r.adjustable && !r.read_only && !r.disabled && r.numeric.is_some() => {
                AccessibilityAction::Decrement(id)
            }
            _ => AccessibilityAction::Unhandled {
                id,
                action: Action::Decrement,
            },
        },
        // Accept context-menu requests only when the enabled node advertises that capability.
        Action::ShowContextMenu => match routing {
            Some(r) if r.context_menu && !r.disabled => AccessibilityAction::ShowContextMenu(id),
            _ => AccessibilityAction::Unhandled {
                id,
                action: Action::ShowContextMenu,
            },
        },
        // Expansion requires an enabled toggle-capable node currently advertised as collapsed.
        Action::Expand => match routing {
            Some(r) if r.expanded == Some(false) && r.expandable_action && !r.disabled => {
                AccessibilityAction::Expand(id)
            }
            _ => AccessibilityAction::Unhandled {
                id,
                action: Action::Expand,
            },
        },
        // Collapse requires an enabled toggle-capable node currently advertised as expanded.
        Action::Collapse => match routing {
            Some(r) if r.expanded == Some(true) && r.expandable_action && !r.disabled => {
                AccessibilityAction::Collapse(id)
            }
            _ => AccessibilityAction::Unhandled {
                id,
                action: Action::Collapse,
            },
        },
        // Reveal is a viewport operation, so every routable node is accepted regardless of disabled or read-only state.
        Action::ScrollIntoView => AccessibilityAction::ScrollIntoView(id),
        Action::SetValue => {
            // Accept `SetValue` only for enabled, writable text or numeric-settable contracts advertised by the collector.
            let live = matches!(routing, Some(r) if !r.disabled && !r.read_only);
            let text = live && matches!(routing, Some(r) if r.text_editable);
            let numeric = live && matches!(routing, Some(r) if r.numeric_settable);
            // Text fields accept `Value`; numeric-settable widgets accept `NumericValue` and may also accept textual input.
            match (text, numeric, data) {
                (true, _, Some(ActionData::Value(v))) => AccessibilityAction::SetValue {
                    id,
                    value: v.to_string(),
                },
                (_, true, Some(ActionData::Value(v))) => AccessibilityAction::SetValue {
                    id,
                    value: v.to_string(),
                },
                (_, true, Some(ActionData::NumericValue(n))) if n.is_finite() => {
                    AccessibilityAction::SetValue {
                        id,
                        value: n.to_string(),
                    }
                }
                _ => AccessibilityAction::Unhandled {
                    id,
                    action: Action::SetValue,
                },
            }
        }
        // Selecting is reading, so read-only text accepts it; disabled text does not.
        Action::SetTextSelection => {
            let selection = match data {
                Some(ActionData::SetTextSelection(selection)) => Some(*selection),
                _ => None,
            };
            let positions = selection.and_then(|selection| {
                Some((
                    map.text_position(target, selection.anchor)?,
                    map.text_position(target, selection.focus)?,
                ))
            });
            match positions {
                Some((anchor, focus)) if routing.is_some_and(|r| !r.disabled) => {
                    AccessibilityAction::SetTextSelection { id, anchor, focus }
                }
                _ => AccessibilityAction::Unhandled {
                    id,
                    action: Action::SetTextSelection,
                },
            }
        }
        other => AccessibilityAction::Unhandled { id, action: other },
    })
}
