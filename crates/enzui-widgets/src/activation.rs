//! Routed activation and disclosure-toggle operations for live widget trees.
//!
//! [`activate`] arms the first matching [`ActivationRequest`]; [`expand`] arms
//! the first matching [`ExpandRequest`]. Neither operation publishes a message
//! or requests a redraw itself. The host must deliver a subsequent event pass
//! so the widget can consume its persistent latch and dispatch its callback.
//!
//! [`crate::focus_button::FocusButton`] prefers `on_enter` for routed activation,
//! falling back to `on_press`. Disabled or anonymous buttons offer no activation
//! carrier. Repeated visits by one operation arm at most one matching widget;
//! build a fresh operation for each independent request.
//!
//! Expand and collapse address a toggle, not a desired expanded value. They use
//! a separate carrier to avoid selecting a tree row whose chevron shares its ID.
//! A missing toggle carrier is a no-op; there is no activation fallback.
//! Use [`crate::a11y_actions`] to apply resolved actions in their publishing window.

use iced::Rectangle;
use iced::advanced::widget::{Id, Operation, operation::Outcome};

/// Carrier that lets a widget receive a routed activation without exposing its callback.
/// Carries an owned ID and an initially false armed flag; no message or lifetime
/// bound on the callback crosses `iced::advanced::widget::Operation::custom`.
#[derive(Debug, Clone)]
pub struct ActivationRequest {
    id: Id,
    armed: bool,
}

impl ActivationRequest {
    /// Create an unarmed carrier for `id`.
    pub fn new(id: Id) -> Self {
        Self { id, armed: false }
    }

    /// The identity this carrier answers to.
    pub fn id(&self) -> &Id {
        &self.id
    }

    /// Whether a matching operation claimed this carrier.
    pub fn armed(&self) -> bool {
        self.armed
    }

    /// Mark this carrier as claimed; the matching operation calls this.
    pub fn arm(&mut self) {
        self.armed = true;
    }
}

/// Arm the first matching activation carrier in traversal order.
/// The widget dispatches on a subsequent event pass. This operation itself
/// produces no message, redraw request or chained operation.
#[derive(Debug, Clone)]
pub struct Activate {
    target: Id,
    armed: bool,
}

impl Activate {
    /// Create a fresh, unarmed operation targeting `target`.
    pub fn new(target: Id) -> Self {
        Self {
            target,
            armed: false,
        }
    }

    /// The identity this operation addresses.
    pub fn target(&self) -> &Id {
        &self.target
    }

    /// Whether this operation claimed a matching carrier.
    /// False also covers controls that publish no carrier, such as disabled buttons.
    pub fn armed(&self) -> bool {
        self.armed
    }
}

impl<T> Operation<T> for Activate {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        // Traversal continues after a match; only the first matching carrier is armed.
        operate(self);
    }

    fn custom(&mut self, _id: Option<&Id>, _bounds: Rectangle, state: &mut dyn std::any::Any) {
        if self.armed {
            return;
        }
        let Some(request) = state.downcast_mut::<ActivationRequest>() else {
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

/// Build an [`Activate`] operation targeting `id`.
pub fn activate(id: Id) -> Activate {
    Activate::new(id)
}

/// Carrier for a disclosure toggle, separate from row activation.
/// A tree chevron offers this carrier under its row's ID, so expand/collapse
/// routing reaches the toggle while activation reaches the row action.
#[derive(Debug, Clone)]
pub struct ExpandRequest {
    id: Id,
    armed: bool,
}

impl ExpandRequest {
    /// Create an unarmed carrier for `id`.
    pub fn new(id: Id) -> Self {
        Self { id, armed: false }
    }

    /// The identity this carrier answers to.
    pub fn id(&self) -> &Id {
        &self.id
    }

    /// Whether a matching operation claimed this carrier.
    pub fn armed(&self) -> bool {
        self.armed
    }

    /// Mark this carrier as claimed; the matching operation calls this.
    pub fn arm(&mut self) {
        self.armed = true;
    }
}

/// Arm the first matching toggle carrier in traversal order.
/// Both expand and collapse requests toggle the addressed control; no desired
/// state is carried. A missing toggle carrier does nothing and never falls back
/// to [`Activate`]. The widget dispatches on a subsequent event pass.
#[derive(Debug, Clone)]
pub struct Expand {
    target: Id,
    armed: bool,
}

impl Expand {
    /// Create a fresh, unarmed operation targeting `target`.
    pub fn new(target: Id) -> Self {
        Self {
            target,
            armed: false,
        }
    }

    /// The identity this operation addresses.
    pub fn target(&self) -> &Id {
        &self.target
    }

    /// Whether this operation claimed a matching carrier.
    /// False also covers controls that publish no carrier, such as disabled buttons.
    pub fn armed(&self) -> bool {
        self.armed
    }
}

impl<T> Operation<T> for Expand {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn custom(&mut self, _id: Option<&Id>, _bounds: Rectangle, state: &mut dyn std::any::Any) {
        if self.armed {
            return;
        }
        let Some(request) = state.downcast_mut::<ExpandRequest>() else {
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

/// Build a toggle operation for a routed expand or collapse request.
/// Uses the separate toggle carrier and never falls back to activation.
///
/// ```ignore
/// AccessibilityAction::Expand(id) | AccessibilityAction::Collapse(id) => {
///     tasks.push(iced::advanced::widget::operate(expand(id)));
/// }
/// ```
pub fn expand(id: Id) -> Expand {
    Expand::new(id)
}
