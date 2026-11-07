use super::*;

#[test]
fn the_ownership_verification_rejects_a_foreign_owner() {
    assert!(
        owns_name(Some(":1.7"), Some(":1.7")),
        "positive control: matching unique names must count as owned, or every \
         rejection below is vacuous"
    );

    assert!(
        !owns_name(Some(":1.9"), Some(":1.7")),
        "a name owned by another connection must NOT count as ours"
    );
    assert!(
        !owns_name(None, Some(":1.7")),
        "an unknown owner must fail closed"
    );
    assert!(
        !owns_name(Some(":1.7"), None),
        "having no unique name must fail closed"
    );
    assert!(!owns_name(None, None), "knowing nothing must fail closed");
}

#[test]
fn mpris_protocol_coalescing_keeps_the_latest_position_without_losing_seek_identity() {
    use crate::media::{MediaEvent, MediaEventKind, MediaSource, MediaState};
    let mut state = MediaState::new();
    let generation = state.replace_source(MediaSource::local("/private/track.wav"));
    let properties = SharedProperties::empty();
    let live = Arc::new(Liveness::new());
    let (wakes, _receiver) = call_wake_channel(Arc::clone(&live));
    let (sender, mut receiver) = mpsc::channel(MPRIS_SIGNAL_QUEUE_CAPACITY);
    let queue = SignalQueue::new(sender);
    assert!(state.apply(&MediaEvent {
        generation,
        kind: MediaEventKind::PositionDiscontinuity(Duration::from_secs(40))
    }));
    properties.set_with_discontinuity(
        &MprisProperties::from_snapshot(&state.snapshot()),
        state.last_position_discontinuity(),
    );
    queue.publish(&live, &wakes);
    assert!(state.apply(&MediaEvent {
        generation,
        kind: MediaEventKind::PositionUpdated(Duration::from_secs(41))
    }));
    properties.set_with_discontinuity(
        &MprisProperties::from_snapshot(&state.snapshot()),
        state.last_position_discontinuity(),
    );
    queue.publish(&live, &wakes);
    assert_eq!(queue.stats().pending, MPRIS_SIGNAL_QUEUE_CAPACITY);
    assert_eq!(queue.stats().coalesced, 1);
    assert!(matches!(
        receiver.try_recv(),
        Ok(SignalRequest::PropertiesChanged)
    ));
    assert_eq!(properties.take_seeked(), Some(41_000_000));
    assert_eq!(properties.take_seeked(), None);
    properties.set_with_discontinuity(
        &MprisProperties::from_snapshot(&state.snapshot()),
        state.last_position_discontinuity(),
    );
    assert_eq!(properties.take_seeked(), None);
    assert!(state.apply(&MediaEvent {
        generation,
        kind: MediaEventKind::PositionDiscontinuity(Duration::from_secs(50))
    }));
    properties.set_with_discontinuity(
        &MprisProperties::from_snapshot(&state.snapshot()),
        state.last_position_discontinuity(),
    );
    state.clear_source();
    properties.set(&MprisProperties::from_snapshot(&state.snapshot()));
    assert_eq!(properties.take_seeked(), None);
}
