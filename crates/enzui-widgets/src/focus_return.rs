//! Window-scoped return records for transient surfaces and their triggers.
//!
//! [`arm`] replaces the pending trigger for a window and refreshes its timestamp.
//! [`reclaim`] consumes a matching record within [`RETENTION_WINDOW`] when that
//! window’s tracked modality is keyboard. Unseen modality keys default to keyboard;
//! shortcut chords do not necessarily change modality. A failed reclaim leaves
//! the record pending, so callers use [`forget`] to discard an unwanted return.
//!
//! These functions signal a return; callers own the trigger’s focus state and the
//! actual focus change. Records live in the current thread’s active
//! [`WidgetRuntimeScope`](enzui_core::widget_runtime::WidgetRuntimeScope).
//! Use the same window key as the modality tracker, including
//! [`KEY_UNKEYED`](enzui_core::focus::KEY_UNKEYED) when no window key is available.

use iced::advanced::widget::Id;

/// Maximum age of a reclaimable return record: 400 ms.
pub const RETENTION_WINDOW: std::time::Duration = std::time::Duration::from_millis(400);

#[derive(Debug, Clone)]
struct Record {
    window: u64,
    id: Id,
    at: std::time::Instant,
}

#[derive(Default)]
struct ReturnSlot(Vec<Record>);

/// Replace the pending trigger for this window and refresh its timestamp.
/// The open surface’s focus owner calls this while it holds focus.
pub fn arm(window: u64, id: Id) {
    enzui_core::widget_runtime::with_state::<ReturnSlot, _>(|slot| {
        slot.0.retain(|r| r.window != window);
        slot.0.push(Record {
            window,
            id,
            at: std::time::Instant::now(),
        });
    });
}

/// Consume a matching record no older than [`RETENTION_WINDOW`] in keyboard modality.
/// Return false without consuming on a mismatch, expiry or pointer modality.
/// The caller must apply the focus change and check whether the trigger is already focused.
#[must_use]
pub fn reclaim(window: u64, id: &Id) -> bool {
    if !enzui_core::focus::current_modality_for(window).focus_ring_visible() {
        return false;
    }
    enzui_core::widget_runtime::with_state::<ReturnSlot, _>(|slot| {
        let hit = slot
            .0
            .iter()
            .position(|r| r.window == window && &r.id == id && r.at.elapsed() <= RETENTION_WINDOW);
        if let Some(index) = hit {
            slot.0.remove(index);
            true
        } else {
            false
        }
    })
}

/// Discard this window’s pending record only if it names this trigger.
/// Use after an unwanted or expired return to prevent later reclamation.
pub fn forget(window: u64, id: &Id) {
    enzui_core::widget_runtime::with_state::<ReturnSlot, _>(|slot| {
        slot.0.retain(|r| !(r.window == window && &r.id == id));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use enzui_core::widget_runtime::WidgetRuntimeScope;

    fn id(s: &'static str) -> Id {
        Id::new(s)
    }

    // Keyed modality is process-global: each test owns distinct window tokens.
    fn key_modality(window: u64) {
        enzui_core::focus::note_input_event_for(
            &iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                modified_key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                physical_key: iced::keyboard::key::Physical::Code(
                    iced::keyboard::key::Code::Escape,
                ),
                location: iced::keyboard::Location::Standard,
                modifiers: iced::keyboard::Modifiers::empty(),
                text: None,
                repeat: false,
            }),
            window,
        );
    }

    #[test]
    fn forget_drops_only_the_named_record_in_its_window() {
        const WIN: u64 = 0x104;
        let _scope = WidgetRuntimeScope::new();
        key_modality(WIN);
        arm(WIN, id("trigger-a"));
        forget(WIN, &id("trigger-b"));
        assert!(
            reclaim(WIN, &id("trigger-a")),
            "forgetting another id leaves ours"
        );

        arm(WIN, id("trigger-a"));
        forget(WIN, &id("trigger-a"));
        assert!(
            !reclaim(WIN, &id("trigger-a")),
            "forget drops our record so a pointer dismissal cannot later fire it"
        );
    }
}
