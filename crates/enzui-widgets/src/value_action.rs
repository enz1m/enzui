//! Route value-change intents to the first matching widget carrier by ID.
//!
//! [`AdjustValue`] arms at most one [`ValueRequest`] per operation instance;
//! it does not publish a message, parse the payload or perform the edit.
//! Supporting widgets retain the intent and consume it on a later event pass.
//! The widget decides which intents it accepts, how to parse numeric values,
//! and whether to clamp, wrap, reject or preserve text verbatim.
//!
//! [`ValueIntent::Set`] also serves text fields. The application must resolve
//! the advertised accessibility action to the widget ID and arrange the event
//! pass that consumes it. Stale IDs and widgets without a carrier are no-ops;
//! there is no fallback to [`crate::activation::Activate`]. Matching uses the
//! carrier's ID, rather than the optional ID of the operation callback.

use iced::Rectangle;
use iced::advanced::widget::{Id, Operation, operation::Outcome};

/// An unparsed value change for a supporting widget to interpret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueIntent {
    /// Increase by the widget's own step, when supported.
    Increment,
    /// Decrease by the widget's own step, when supported.
    Decrement,
    /// Replace with raw, unvalidated text. Numeric widgets parse it; text fields
    /// preserve it according to their editing policy.
    Set(String),
}

/// A non-generic carrier for value intents passed through `Operation::custom`.
#[derive(Debug, Clone)]
pub struct ValueRequest {
    id: Id,
    intent: Option<ValueIntent>,
}

impl ValueRequest {
    /// Announce a carrier with no pending intent for this widget ID.
    pub fn new(id: Id) -> Self {
        Self { id, intent: None }
    }

    /// Return the address matched by [`AdjustValue`].
    pub fn id(&self) -> &Id {
        &self.id
    }

    /// Borrow the delivered intent without consuming it.
    pub fn intent(&self) -> Option<&ValueIntent> {
        self.intent.as_ref()
    }

    /// Consume the delivered intent, leaving the carrier empty.
    pub fn take(&mut self) -> Option<ValueIntent> {
        self.intent.take()
    }

    /// Store an intent, replacing any previously delivered intent in this carrier.
    pub fn deliver(&mut self, intent: ValueIntent) {
        self.intent = Some(intent);
    }
}

/// Arm the first matching [`ValueRequest`] encountered, once per operation.
/// The target widget is responsible for consuming the intent and publishing any
/// change. No message or activation fallback leaves this operation.
#[derive(Debug, Clone)]
pub struct AdjustValue {
    target: Id,
    intent: ValueIntent,
    armed: bool,
}

impl AdjustValue {
    /// Create an unarmed operation for a widget ID.
    pub fn new(target: Id, intent: ValueIntent) -> Self {
        Self {
            target,
            intent,
            armed: false,
        }
    }

    /// Return the widget address this operation matches.
    pub fn target(&self) -> &Id {
        &self.target
    }

    /// Borrow the intent this operation delivers.
    pub fn intent(&self) -> &ValueIntent {
        &self.intent
    }

    /// Whether a matching carrier has received the intent.
    /// This records delivery, not acceptance or a committed value change.
    pub fn armed(&self) -> bool {
        self.armed
    }
}

impl<T> Operation<T> for AdjustValue {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn custom(&mut self, _id: Option<&Id>, _bounds: Rectangle, state: &mut dyn std::any::Any) {
        if self.armed {
            return;
        }
        let Some(request) = state.downcast_mut::<ValueRequest>() else {
            return;
        };
        if request.id() != &self.target {
            return;
        }
        self.armed = true;
        request.deliver(self.intent.clone());
    }

    fn finish(&self) -> Outcome<T> {
        Outcome::None
    }
}

/// Build an operation that arms at most the first matching widget carrier.
/// The operation does not interpret the payload or publish a change.
pub fn adjust_value(id: Id, intent: ValueIntent) -> AdjustValue {
    AdjustValue::new(id, intent)
}
