//! Nonblocking inspection of actions already ready in an iced reducer task.
//!
//! Widget-operation tasks retain a sender inside the yielded operation, so collecting their stream to completion can deadlock.

use iced_core::widget::Operation;
use iced_core::window;
use iced_futures::futures::Stream;
use iced_runtime::{Action, Task};
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

/// Maximum consecutive ready actions accepted before saturation; exactly this many are allowed.
pub const MAX_READY_ACTIONS: usize = 64;

/// How a [`drain_ready`] pass ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DrainOutcome {
    /// The task stream ended and cannot produce another action.
    Ended,
    /// The stream parked after yielding every currently ready action.
    Parked,
    /// More than `MAX_READY_ACTIONS` were immediately ready, so draining stopped.
    Saturated,
}

impl DrainOutcome {
    /// Returns whether draining ended or parked without saturation.
    #[must_use]
    pub const fn is_complete(self) -> bool {
        matches!(self, Self::Ended | Self::Parked)
    }
}

/// Fan-out widget operations, and window-scoped operations with their
/// target windows: what [`ReadyActions::into_operations_by_scope`] returns.
pub type ScopedOperations = (
    Vec<Box<dyn Operation>>,
    Vec<(window::Id, Box<dyn Operation>)>,
);

/// Actions observed during one bounded nonblocking drain, split by kind.
pub struct ReadyActions<Message> {
    operations: Vec<Box<dyn Operation>>,
    window_operations: Vec<(window::Id, Box<dyn Operation>)>,
    messages: Vec<Message>,
    other_actions: usize,
    outcome: DrainOutcome,
}

impl<Message> std::fmt::Debug for ReadyActions<Message> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReadyActions")
            .field("operations", &self.operations.len())
            .field("window_operations", &self.window_operations.len())
            .field("messages", &self.messages.len())
            .field("other_actions", &self.other_actions)
            .field("outcome", &self.outcome)
            .finish()
    }
}

impl<Message> ReadyActions<Message> {
    /// How the drain ended. See [`DrainOutcome`].
    #[must_use]
    pub const fn outcome(&self) -> DrainOutcome {
        self.outcome
    }

    /// `Action::Widget` operations, in the order the task produced them.
    #[must_use]
    pub fn operations(&self) -> &[Box<dyn Operation>] {
        &self.operations
    }

    /// Take the `Action::Widget` operations so they can be run against a tree.
    #[must_use]
    pub fn into_operations(self) -> Vec<Box<dyn Operation>> {
        self.operations
    }

    /// Takes window-scoped widget operations with their target windows.
    #[must_use]
    pub fn into_window_operations(self) -> Vec<(window::Id, Box<dyn Operation>)> {
        self.window_operations
    }

    /// Takes the fan-out operations and the window-scoped ones together.
    ///
    /// A driver that must run both — every window for the first, one named
    /// window for the second — cannot call the two single-scope takers.
    #[must_use]
    pub fn into_operations_by_scope(self) -> ScopedOperations {
        (self.operations, self.window_operations)
    }

    /// Messages the task produced synchronously (`Action::Output`).
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Take the messages the task produced synchronously.
    #[must_use]
    pub fn into_messages(self) -> Vec<Message> {
        self.messages
    }

    /// Number of ready clipboard, window, font, or other unclassified actions.
    #[must_use]
    pub const fn other_actions(&self) -> usize {
        self.other_actions
    }

    /// Total ready actions observed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.operations.len()
            + self.window_operations.len()
            + self.messages.len()
            + self.other_actions
    }

    /// Returns whether the drain observed no ready actions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Takes a bounded prefix of immediately ready actions without blocking and reports how polling stopped.
pub fn drain_ready<Message>(task: Task<Message>) -> ReadyActions<Message> {
    let mut ready = ReadyActions {
        operations: Vec::new(),
        window_operations: Vec::new(),
        messages: Vec::new(),
        other_actions: 0,
        outcome: DrainOutcome::Ended,
    };

    let Some(mut stream) = iced_runtime::task::into_stream(task) else {
        return ready;
    };

    let mut cx = Context::from_waker(Waker::noop());
    // `MAX_READY_ACTIONS` actions plus the one poll that observes the end or
    // the park: a task carrying exactly the bound still drains cleanly, and
    // saturation means strictly more ready actions than the bound.
    let mut polls_left = MAX_READY_ACTIONS + 1;
    loop {
        if polls_left == 0 {
            ready.outcome = DrainOutcome::Saturated;
            break;
        }
        polls_left -= 1;
        match Pin::new(&mut stream).poll_next(&mut cx) {
            Poll::Ready(Some(Action::Widget(operation))) => ready.operations.push(operation),
            Poll::Ready(Some(Action::WidgetIn { window, operation })) => {
                ready.window_operations.push((window, operation));
            }
            Poll::Ready(Some(Action::Output(message))) => ready.messages.push(message),
            Poll::Ready(Some(_)) => ready.other_actions += 1,
            Poll::Ready(None) => {
                ready.outcome = DrainOutcome::Ended;
                break;
            }
            Poll::Pending => {
                ready.outcome = DrainOutcome::Parked;
                break;
            }
        }
    }

    ready
}

/// Drains and returns fan-out widget operations.
///
/// # Panics
/// Panics if the task produces more than `MAX_READY_ACTIONS` without parking.
#[must_use]
pub fn widget_operations<Message>(task: Task<Message>) -> Vec<Box<dyn Operation>> {
    let ready = drain_ready(task);
    assert!(
        ready.outcome().is_complete(),
        "reducer_task: more than {MAX_READY_ACTIONS} ready actions came out of one task without \
         the stream ending or parking. A synchronous update arm does not produce that many; \
         suspect an unbounded producer rather than a truncated list ({ready:?})"
    );
    ready.into_operations()
}
