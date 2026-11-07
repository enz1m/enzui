use super::{CallQueue, MprisQueueRefusal, SharedProperties};
use crate::media::mpris::{MprisCall, retained_track_id, track_id};
use crate::media::{MAX_PLAYBACK_RATE, MIN_PLAYBACK_RATE};

pub(super) struct PlayerInterface {
    pub(super) properties: SharedProperties,
    pub(super) calls: CallQueue,
}

impl PlayerInterface {
    fn queue(&self, call: MprisCall) -> zbus::fdo::Result<()> {
        self.calls.push(call).map_err(|refusal| match refusal {
            MprisQueueRefusal::Full { .. } => zbus::fdo::Error::LimitsExceeded(refusal.to_string()),
            // An ended generation is a state failure rather than a limit: the
            // player will not accept this call later either.
            MprisQueueRefusal::PlayerGone { .. } | MprisQueueRefusal::NoSubscriber => {
                zbus::fdo::Error::Failed(refusal.to_string())
            }
        })
    }
}

// Serial dispatch preserves D-Bus call order before calls enter the bounded application queue.
#[zbus::interface(name = "org.mpris.MediaPlayer2.Player", spawn = false)]
impl PlayerInterface {
    fn play(&self) -> zbus::fdo::Result<()> {
        self.queue(MprisCall::Play)
    }

    fn pause(&self) -> zbus::fdo::Result<()> {
        self.queue(MprisCall::Pause)
    }

    fn play_pause(&self) -> zbus::fdo::Result<()> {
        self.queue(MprisCall::PlayPause)
    }

    fn stop(&self) -> zbus::fdo::Result<()> {
        self.queue(MprisCall::Stop)
    }

    // MPRIS `Seek` offsets are relative microseconds.
    fn seek(&self, offset: i64) -> zbus::fdo::Result<()> {
        self.queue(MprisCall::Seek {
            offset,
            micros: true,
        })
    }

    // The `TrackId` is carried to the adapter unparsed; staleness is decided
    // against the snapshot at translation, not against these queued properties.
    fn set_position(
        &self,
        track_id: zbus::zvariant::ObjectPath<'_>,
        position: i64,
    ) -> zbus::fdo::Result<()> {
        self.queue(MprisCall::SetPosition {
            track: retained_track_id(track_id.as_str()),
            micros: position,
        })
    }

    fn next(&self) -> zbus::fdo::Result<()> {
        self.queue(MprisCall::Next)
    }

    fn previous(&self) -> zbus::fdo::Result<()> {
        self.queue(MprisCall::Previous)
    }

    /// Announces a confirmed discontinuous position in microseconds.
    #[zbus(signal)]
    pub(super) async fn seeked(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        position: i64,
    ) -> zbus::Result<()>;

    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.properties.get().playback_status.as_str().to_owned()
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> i64 {
        self.properties.get().position_micros
    }

    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.properties.get().volume
    }

    #[zbus(property)]
    fn set_volume(&self, volume: f64) -> zbus::fdo::Result<()> {
        self.queue(MprisCall::SetVolume(volume))
    }

    #[zbus(property)]
    fn rate(&self) -> f64 {
        self.properties.get().rate
    }

    #[zbus(property)]
    fn set_rate(&self, rate: f64) -> zbus::fdo::Result<()> {
        self.queue(MprisCall::SetRate(rate))
    }

    // The advertised window is the reducer's admitted range, so a client cannot
    // read a bound the audio backend would then refuse.
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        MIN_PLAYBACK_RATE
    }

    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        MAX_PLAYBACK_RATE
    }

    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.properties.get().can_seek
    }

    #[zbus(property)]
    fn can_play(&self) -> bool {
        self.properties.get().can_control
    }

    #[zbus(property)]
    fn can_pause(&self) -> bool {
        self.properties.get().can_control
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn can_control(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.properties.get().can_go_next
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.properties.get().can_go_previous
    }

    // The adapter must supply an already-redacted title; this transport publishes it unchanged to the session bus.
    #[zbus(property)]
    fn metadata(&self) -> std::collections::HashMap<String, zbus::zvariant::OwnedValue> {
        use zbus::zvariant::Value;
        let props = self.properties.get();
        let mut map = std::collections::HashMap::new();
        if !props.can_control {
            return map;
        }

        // A track id is required by the spec for `Metadata` to be useful; this
        // one is stable per generation and carries no content information.
        if let Ok(path) = zbus::zvariant::ObjectPath::try_from(track_id(props.generation))
            && let Ok(v) = Value::from(path).try_to_owned()
        {
            let _ = map.insert("mpris:trackid".to_owned(), v);
        }

        if let Some(length) = props.length_micros
            && let Ok(v) = Value::from(length).try_to_owned()
        {
            let _ = map.insert("mpris:length".to_owned(), v);
        }

        if let Some(title) = props.title.as_ref()
            && let Ok(v) = Value::from(title.clone()).try_to_owned()
        {
            let _ = map.insert("xesam:title".to_owned(), v);
        }

        map
    }
}
