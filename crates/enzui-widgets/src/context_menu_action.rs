//! Arm a live widget's context-menu handler by stable identity.
//!
//! [`show_context_menu`] claims the first matching [`ContextMenuRequest`] in
//! traversal order. It publishes no message, requests no redraw and chains no
//! operation; the host must deliver a subsequent event pass for widget dispatch.
//! Build a fresh operation for each independent request.
//!
//! Context-menu and activation carriers are distinct types, so these operations
//! cannot invoke each other's handlers even when both use one widget ID.
//! A missing carrier is a no-op. The widget chooses the menu anchor;
//! [`crate::context_target`] uses its frame centre in logical window coordinates.
//! Its pointer path invokes the callback directly rather than offering a request.
//! Use [`crate::a11y_actions`] to apply resolved actions in their publishing window.

use iced::Rectangle;
use iced::advanced::widget::{Id, Operation, operation::Outcome};

/// Carrier that lets a widget receive routed context-menu requests without exposing its callback.
/// Carries an owned ID and an initially false armed flag; no message crosses the operation.
#[derive(Debug, Clone)]
pub struct ContextMenuRequest {
    id: Id,
    armed: bool,
}

impl ContextMenuRequest {
    /// A carrier announcing that the widget with `id` offers a context menu.
    pub fn new(id: Id) -> Self {
        Self { id, armed: false }
    }

    /// The id of the widget that built this carrier.
    pub fn id(&self) -> &Id {
        &self.id
    }

    /// Whether a matching carrier was armed.
    pub fn armed(&self) -> bool {
        self.armed
    }

    /// Claim this widget as the context-menu target. Called by
    /// [`ShowContextMenu`], not by widgets.
    pub fn arm(&mut self) {
        self.armed = true;
    }
}

/// Arm the first matching context-menu carrier in traversal order.
/// The widget dispatches on a subsequent event pass. This operation itself
/// produces no message, redraw request or chained operation.
#[derive(Debug, Clone)]
pub struct ShowContextMenu {
    target: Id,
    armed: bool,
}

impl ShowContextMenu {
    /// Target this operation at `target`.
    pub fn new(target: Id) -> Self {
        Self {
            target,
            armed: false,
        }
    }

    /// The id this operation is aimed at.
    pub fn target(&self) -> &Id {
        &self.target
    }

    /// Whether a matching carrier was armed.
    pub fn armed(&self) -> bool {
        self.armed
    }
}

impl<T> Operation<T> for ShowContextMenu {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn custom(&mut self, _id: Option<&Id>, _bounds: Rectangle, state: &mut dyn std::any::Any) {
        if self.armed {
            return;
        }
        let Some(request) = state.downcast_mut::<ContextMenuRequest>() else {
            return;
        };
        if request.id() != &self.target {
            return;
        }
        self.armed = true;
        request.arm();
    }

    fn finish(&self) -> Outcome<T> {
        Outcome::None
    }
}

/// Build a [`ShowContextMenu`] operation targeting `id`.
pub fn show_context_menu(id: Id) -> ShowContextMenu {
    ShowContextMenu::new(id)
}
