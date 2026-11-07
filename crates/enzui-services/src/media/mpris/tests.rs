use super::*;
use crate::media::{
    Capabilities, MediaEvent, MediaEventKind, MediaSource, MediaState, PlaybackState,
};
use std::time::Duration;

fn caps(seekable: bool) -> Capabilities {
    Capabilities {
        seekable,
        has_audio: true,
        has_video: true,
    }
}

fn loaded(seekable: bool, duration: Option<Duration>) -> (MediaState, Generation) {
    let mut state = MediaState::new();
    let generation = state.replace_source(MediaSource::local("/media/track.flac"));
    let _ = state.apply(&MediaEvent {
        generation,
        kind: MediaEventKind::SourceReady {
            capabilities: caps(seekable),
            duration,
        },
    });
    (state, generation)
}

fn playing_at_30s() -> (MediaState, Generation) {
    let (mut state, generation) = loaded(true, Some(Duration::from_secs(200)));
    let _ = state.apply(&MediaEvent {
        generation,
        kind: MediaEventKind::StateChanged(PlaybackState::Playing),
    });
    let _ = state.apply(&MediaEvent {
        generation,
        kind: MediaEventKind::PositionUpdated(Duration::from_secs(30)),
    });
    (state, generation)
}

fn adapter() -> MprisAdapter {
    MprisAdapter::new("enzui")
}

#[test]
fn a_handled_call_publishes_and_a_refused_one_does_not() {
    let (mut state, _) = playing_at_30s();
    let mut client = MockMprisClient::new();
    let a = adapter();

    assert_eq!(client.publish_count(), 0);

    a.handle_and_publish(&MprisCall::Pause, &mut state, &mut client)
        .expect("pause is possible");
    assert_eq!(
        client.publish_count(),
        1,
        "a handled call publishes the (unchanged) authoritative state"
    );
    assert!(client.refused().is_empty());

    let (mut unseekable, _) = loaded(false, Some(Duration::from_secs(5)));
    let err = a
        .handle_and_publish(
            &MprisCall::Seek {
                offset: 1,
                micros: false,
            },
            &mut unseekable,
            &mut client,
        )
        .unwrap_err();
    assert_eq!(err, MprisRefusal::NotSeekable);
    assert_eq!(
        client.publish_count(),
        1,
        "a refused call must not publish a state change"
    );
    assert_eq!(client.refused().len(), 1, "the refusal must be reported");
    assert_eq!(client.refused()[0].1, MprisRefusal::NotSeekable);
}
