use crate::operation_activity::*;

const FILES: ProviderId = ProviderId("files");

fn center() -> ActivityCenter {
    let mut center = ActivityCenter::new().with_reveal_delay(std::time::Duration::ZERO);
    center.register_provider(FILES, true);
    center
}

fn copy(op: u64) -> Activity {
    Activity::new(ActivityKey::new("files", op), ActivityKind::Copy, "Copying")
        .controls([ActivityControl::Pause, ActivityControl::Cancel])
}

#[test]
fn progress_after_a_terminal_phase_is_refused() {
    let mut center = center();
    center.publish(copy(1), 0);
    center.finish(
        ActivityKey::new("files", 1),
        Generation::FIRST,
        TerminalOutcome::Completed,
        1,
    );
    assert_eq!(
        center.progress(
            ActivityKey::new("files", 1),
            Generation::FIRST,
            ActivityProgress::Bytes {
                done: 1,
                total: Some(2)
            }
        ),
        PublishOutcome::Stale,
        "a late tick must not resurrect finished work",
    );
    assert_eq!(center.summary().active(), 0);
}
