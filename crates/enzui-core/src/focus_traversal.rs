//! Automatic Tab traversal for an ordinary application, owned by the runtime rather than an application reducer.
//!
//! [`Traversing`] wraps an application's own message type so the traversal
//! subscription and the operation that answers it never appear in the
//! application's `Message` enum. `enzui_widgets::app_setup::run` installs
//! both halves around any `iced::Program`; this module is the renderer-neutral
//! policy they share, and is also the seam a host that owns its own event loop
//! composes directly.
//!
//! ```
//! use enzui_core::focus::Traversal;
//! use enzui_core::focus_traversal::{Traversing, TraversalBinding};
//! use iced_core::event::Status;
//! use iced_core::keyboard::{self, Modifiers, key};
//!
//! # #[derive(Debug)] enum Message { Increment }
//! let tab = iced_core::Event::Keyboard(keyboard::Event::KeyPressed {
//!     key: keyboard::Key::Named(key::Named::Tab),
//!     modified_key: keyboard::Key::Named(key::Named::Tab),
//!     physical_key: key::Physical::Code(key::Code::Tab),
//!     location: keyboard::Location::Standard,
//!     modifiers: Modifiers::default(),
//!     text: None,
//!     repeat: false,
//! });
//!
//! // An event the widget tree left alone traverses; a captured one belongs to
//! // the widget that took it.
//! assert_eq!(
//!     TraversalBinding::Tab.decode(&tab, Status::Ignored),
//!     Some(Traversal::Next),
//! );
//! assert_eq!(TraversalBinding::Tab.decode(&tab, Status::Captured), None);
//!
//! // A surface that owns bare Tab itself keeps every key.
//! assert_eq!(TraversalBinding::Disabled.decode(&tab, Status::Ignored), None);
//!
//! let _user: Traversing<Message> = Traversing::User(Message::Increment);
//! ```

use crate::focus::{self, Traversal};
use iced_core::event::Status;
use iced_core::{Event, window};
use iced_futures::Subscription;
use iced_runtime::Task;

/// Decodes a runtime event and its processing status into a traversal direction.
///
/// A decoder must decline an event whose [`Status`] is
/// [`Captured`](Status::Captured), so a widget that owns the key keeps it, and
/// decline every chord it does not claim, so other bindings still fire.
pub type TraversalDecoder = fn(&Event, Status) -> Option<Traversal>;

/// Which input the automatic traversal answers.
///
/// [`Tab`](Self::Tab) is the default an ordinary window wants. A surface whose
/// child owns bare Tab — a terminal or a code editor — names its own chord with
/// [`Chord`](Self::Chord) or takes the whole route back with
/// [`Disabled`](Self::Disabled).
#[derive(Debug, Clone, Copy, Default)]
pub enum TraversalBinding {
    /// Bare Tab moves forward and Shift+Tab moves backward.
    #[default]
    Tab,
    /// An application-supplied chord decode; bare Tab stays with the widget tree.
    Chord(TraversalDecoder),
    /// No automatic traversal: the application owns decode and operation itself.
    Disabled,
}

impl TraversalBinding {
    /// The decoder this binding installs, or `None` when traversal is disabled.
    #[must_use]
    pub const fn decoder(self) -> Option<TraversalDecoder> {
        match self {
            Self::Tab => Some(focus::traversal_from_event),
            Self::Chord(decode) => Some(decode),
            Self::Disabled => None,
        }
    }

    /// The direction this binding reads from one event, if any.
    #[must_use]
    pub fn decode(self, event: &Event, status: Status) -> Option<Traversal> {
        self.decoder().and_then(|decode| decode(event, status))
    }
}

/// An application message, or the traversal the runtime owns on its behalf.
///
/// A program adapter wraps the application's message type in this enum, so
/// [`reduce`] can answer a traversal without an arm in the application's own
/// `update`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Traversing<Message> {
    /// A message the application published.
    User(Message),
    /// Assistive-technology actions are waiting; an adapter that dispatches
    /// them (enzui-widgets' `app_setup::run`) applies the batch. [`reduce`]
    /// does nothing with it.
    Accessibility(crate::app_runtime::AccessibilityWake),
    /// Work for a runtime service the adapter installed (enzui-widgets'
    /// text drag and drop). [`reduce`] does nothing with it.
    Service(crate::app_runtime::ServiceMessage),
    /// Keyboard traversal decoded in the window that produced the input.
    Traverse {
        /// Which way focus moves.
        direction: Traversal,
        /// The window whose input decoded to this traversal, and the only
        /// window whose focus ring [`reduce`] advances for it.
        window: window::Id,
    },
}

impl<Message> Traversing<Message> {
    /// The application message this carries, or `None` for a traversal.
    #[must_use]
    pub fn user(self) -> Option<Message> {
        match self {
            Self::User(message) => Some(message),
            Self::Accessibility(_) | Self::Service(_) | Self::Traverse { .. } => None,
        }
    }
}

// The recipe identity of the traversal subscription. The decoder is part of it
// so a host that swaps bindings at run time re-subscribes instead of keeping
// the previous decode.
#[derive(Hash)]
struct TraversalRecipe(TraversalDecoder);

/// The subscription that decodes traversal input, tagged with the exact window that produced it.
///
/// Returns [`Subscription::none`] for [`TraversalBinding::Disabled`]. Every
/// event reaches the decoder with its processing status, so a widget that
/// captured the key is never overruled.
pub fn traversal_subscription<Message>(
    binding: TraversalBinding,
) -> Subscription<Traversing<Message>>
where
    Message: Send + 'static,
{
    let Some(decode) = binding.decoder() else {
        return Subscription::none();
    };
    iced_futures::subscription::filter_map(TraversalRecipe(decode), move |event| {
        let iced_futures::subscription::Event::Interaction {
            window,
            event,
            status,
        } = event
        else {
            return None;
        };
        decode(&event, status).map(|direction| Traversing::Traverse { direction, window })
    })
}

/// Batches an application's own subscription with the automatic traversal one.
pub fn wrap_subscription<Message>(
    application: Subscription<Message>,
    binding: TraversalBinding,
) -> Subscription<Traversing<Message>>
where
    Message: Send + 'static,
{
    Subscription::batch([
        application.map(Traversing::User),
        traversal_subscription(binding),
    ])
}

/// The wrapping focus operation for one decoded traversal, aimed at `window` alone.
///
/// iced's ordinary widget task walks every open window, which would advance a
/// second window's ring from the first window's keypress; this task runs in the
/// originating window only and reveals focus through every enclosing viewport.
pub fn traversal_task<Message>(
    direction: Traversal,
    window: window::Id,
) -> Task<Traversing<Message>> {
    crate::app_runtime::window_operation_task(window, focus::traversal_operation::<()>(direction))
}

/// Answers a traversal with [`traversal_task`] and forwards every other message to `application`.
///
/// This is the whole default reducer: an application's `update` never sees a
/// traversal message and needs no focus arm of its own.
pub fn reduce<Message, Update>(
    message: Traversing<Message>,
    application: Update,
) -> Task<Traversing<Message>>
where
    Message: Send + 'static,
    Update: FnOnce(Message) -> Task<Message>,
{
    match message {
        Traversing::User(message) => application(message).map(Traversing::User),
        Traversing::Traverse { direction, window } => traversal_task(direction, window),
        Traversing::Accessibility(_) | Traversing::Service(_) => Task::none(),
    }
}
