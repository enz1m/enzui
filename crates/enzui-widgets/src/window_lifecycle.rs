//! Translate multi-window exit decisions into iced runtime tasks.
//!
//! [`WindowLedger`](crate::window_lifecycle::WindowLedger) owns the close policy;
//! [`lifecycle_task`](crate::window_lifecycle::lifecycle_task) emits its runtime
//! commands. Placement types are re-exported for persistent application-role
//! keys. The application owns loading, flushing and handling persistence errors.
//! Close commands precede exit, but do not wait for close-event acknowledgments
//! or compositor completion. Finish any required state flush before emitting
//! the exit task.

pub use enzui_core::window_lifecycle::{LifecycleAction, PrimaryClosePolicy, WindowLedger};

/// Placement tracking keyed by persistent application roles.
pub use enzui_core::window_placement::{
    LoadOutcome, MAX_SIZE, MIN_SIZE, Placement, PlacementKey, PlacementKeyed, PlacementTracker,
    QueryToken, UNKNOWN_OUTPUT_BOUNDS,
};

/// Emit runtime commands for a close-policy decision.
/// `Continue` emits nothing, `Exit` exits, and `CloseThenExit` batches close
/// commands before exit. Command ordering does not await window-close
/// acknowledgments or save state; invoke this after any required flush completes.
pub fn lifecycle_task<Message: Send + 'static>(action: LifecycleAction) -> iced::Task<Message> {
    match action {
        LifecycleAction::Continue => iced::Task::none(),
        LifecycleAction::Exit => iced::exit(),
        LifecycleAction::CloseThenExit(windows) => {
            let closes: Vec<iced::Task<Message>> = windows
                .into_iter()
                .map(iced::window::close::<Message>)
                .collect();
            iced::Task::batch(closes).chain(iced::exit())
        }
    }
}
