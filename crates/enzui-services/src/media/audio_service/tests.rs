//! Audio queue, worker, and production autoaudiosink tests.

use super::*;
use crate::media::{MediaEvent, MediaEventKind, MediaSource, MediaState};
use enzui_test_support::test_scratch::ScratchDir;
use std::time::Duration;

fn generate_test_wav(scratch: &ScratchDir, name: &str, num_buffers: u32) -> std::path::PathBuf {
    generate_sine_wav(scratch, name, num_buffers, 440)
}

fn generate_sine_wav(
    scratch: &ScratchDir,
    name: &str,
    num_buffers: u32,
    frequency_hz: u32,
) -> std::path::PathBuf {
    gst::init().expect("GStreamer init");
    let path = scratch.path().join(format!("{name}.wav"));
    let pipeline_str = format!(
        "audiotestsrc num-buffers={num_buffers} freq={frequency_hz} ! \
         audioconvert ! audio/x-raw,format=F32LE,rate=44100,channels=1 \
         ! wavenc ! filesink location={}",
        path.display()
    );
    let pipeline = gst::parse::launch(&pipeline_str).expect("parse");
    pipeline.set_state(gst::State::Playing).expect("start");
    let bus = pipeline.bus().expect("bus");
    for msg in bus.iter_timed(gst::ClockTime::from_seconds(10)) {
        match msg.view() {
            gst::MessageView::Eos(_) => break,
            gst::MessageView::Error(err) => panic!("WAV gen failed: {}", err.error().message()),
            _ => {}
        }
    }
    let _ = pipeline.set_state(gst::State::Null);
    assert!(path.exists());
    path
}

fn wait_for_event(
    rx: &AudioEventReceiver,
    timeout: Duration,
    pred: impl Fn(&MediaEvent) -> bool,
) -> Option<MediaEvent> {
    let deadline = Instant::now() + timeout;
    loop {
        for evt in rx.drain() {
            if pred(&evt) {
                return Some(evt);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

fn test_config() -> AudioServiceConfig {
    AudioServiceConfig {
        audio_sink_factory: "fakesink",
        bus_poll_interval: Duration::from_millis(10),
        position_poll_interval: Duration::from_millis(10),
        ..AudioServiceConfig::default()
    }
}

struct NativeFixture {
    service: AudioService,
    events: AudioEventReceiver,
    state: MediaState,
    generation: Generation,
}

impl NativeFixture {
    fn load(wav: &std::path::Path) -> Self {
        let (mut service, events) = AudioService::new(test_config()).expect("service");
        let mut state = MediaState::new();
        let generation = service
            .load_into(MediaSource::local(wav), &mut state)
            .expect("load source");
        wait_for_event(&events, TEST_TIMEOUT, |event| {
            event.generation == generation
                && matches!(event.kind, MediaEventKind::SourceReady { .. })
        })
        .expect("source ready");
        Self {
            service,
            events,
            state,
            generation,
        }
    }

    fn send(&mut self, command: MediaCommand) {
        let request = self.state.dispatch(&command);
        self.service.send(&request).expect("command admission");
    }

    fn wait_for_rate(&self, expected: f64) {
        wait_for_event(&self.events, TEST_TIMEOUT, |event| {
            event.generation == self.generation
                && matches!(
                    event.kind,
                    MediaEventKind::RateChanged { rate } if (rate - expected).abs() < f64::EPSILON
                )
        })
        .expect("rate confirmation");
    }

    fn wait_for_state(&self, expected: PlaybackState) {
        wait_for_event(&self.events, TEST_TIMEOUT, |event| {
            event.generation == self.generation
                && matches!(event.kind, MediaEventKind::StateChanged(state) if state == expected)
        })
        .expect("state confirmation");
    }

    fn inspect(&self) -> NativePlayback {
        let (reply, result) = std::sync::mpsc::sync_channel(1);
        self.service
            .cmd_tx
            .try_send(AudioCommand {
                generation: self.generation,
                kind: AudioCommandKind::InspectNative(reply),
            })
            .expect("inspection admission");
        result
            .recv_timeout(TEST_TIMEOUT)
            .expect("native inspection")
    }

    fn reset_audio_measurement(&self) {
        self.service
            .cmd_tx
            .try_send(AudioCommand {
                generation: self.generation,
                kind: AudioCommandKind::ResetNativeAudio,
            })
            .expect("measurement reset admission");
        let _ = self.inspect();
    }

    fn play_and_capture_rms(&mut self) -> (f64, NativePlayback) {
        self.reset_audio_measurement();
        self.send(MediaCommand::Play);
        self.wait_for_state(PlaybackState::Playing);
        let deadline = Instant::now() + TEST_TIMEOUT;
        loop {
            let native = self.inspect();
            if let Some(rms) = native.audio_rms {
                return (rms, native);
            }
            assert!(Instant::now() < deadline, "native PCM was not collected");
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for NativeFixture {
    fn drop(&mut self) {
        self.service.shutdown();
    }
}

fn assert_seek(fixture: &NativeFixture, rate: f64, target: Duration, state: gst::State) {
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        let native = fixture.inspect();
        let accepted = native.last_seek == Some((rate, target));
        let segment_matches = native
            .rate
            .is_some_and(|value| (value - rate).abs() < f64::EPSILON);
        let position_matches = native
            .position
            .is_some_and(|position| position.abs_diff(target) < Duration::from_secs(2));
        if accepted && segment_matches && position_matches && native.state == state {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "native seek did not settle: {native:?}, expected rate {rate}, target {target:?}, state {state:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn capture_rms(
    wav: &std::path::Path,
    effects: AudioEffects,
    user_volume: f64,
    muted: bool,
) -> (f64, NativePlayback) {
    let mut fixture = NativeFixture::load(wav);
    fixture
        .service
        .set_effects(fixture.generation, effects)
        .expect("effects admission");
    if user_volume != 1.0 {
        fixture.send(MediaCommand::SetVolume(user_volume));
        wait_for_event(&fixture.events, TEST_TIMEOUT, |event| {
            matches!(event.kind, MediaEventKind::VolumeChanged { volume } if (volume - user_volume).abs() < 1.0e-5)
        })
        .expect("volume confirmation");
    }
    if muted {
        fixture.send(MediaCommand::SetMute(true));
        wait_for_event(&fixture.events, TEST_TIMEOUT, |event| {
            matches!(event.kind, MediaEventKind::MuteChanged(true))
        })
        .expect("mute confirmation");
    }
    fixture.play_and_capture_rms()
}

#[test]
fn native_pcm_proves_frequency_selective_equalizer() {
    let scratch = ScratchDir::new("audio-effects-frequency");
    let low = generate_sine_wav(&scratch, "low", 120, 59);
    let high = generate_sine_wav(&scratch, "high", 120, 7523);
    let flat =
        AudioEffects::new(true, [0.0; AUDIO_EQUALIZER_BAND_COUNT], 0.25).expect("flat effects");
    let mut low_boost = [0.0; AUDIO_EQUALIZER_BAND_COUNT];
    low_boost[1] = 12.0;
    let boosted = AudioEffects::new(true, low_boost, 0.25).expect("boosted effects");

    let (flat_low, _) = capture_rms(&low, flat, 1.0, false);
    let (boosted_low, _) = capture_rms(&low, boosted, 1.0, false);
    let (flat_high, _) = capture_rms(&high, flat, 1.0, false);
    let (boosted_high, _) = capture_rms(&high, boosted, 1.0, false);
    let low_ratio = boosted_low / flat_low;
    let high_ratio = boosted_high / flat_high;
    assert!(low_ratio > 2.0, "low-frequency gain ratio {low_ratio}");
    assert!(
        low_ratio > high_ratio * 1.5,
        "equalizer was not selective: low ratio {low_ratio}, high ratio {high_ratio}"
    );
}

#[test]
fn effects_leave_user_volume_and_mute_independent() {
    let scratch = ScratchDir::new("audio-effects-user-controls");
    let wav = generate_sine_wav(&scratch, "user_controls", 120, 947);
    let processing_gain = 10.0_f64.powf(-6.0 / 20.0);
    let effects = AudioEffects::new(false, [0.0; AUDIO_EQUALIZER_BAND_COUNT], processing_gain)
        .expect("effects");
    let mut fixture = NativeFixture::load(&wav);
    fixture
        .service
        .set_effects(fixture.generation, effects)
        .expect("effects admission");
    fixture.send(MediaCommand::SetVolume(0.8));
    let confirmed_volume = wait_for_event(&fixture.events, TEST_TIMEOUT, |event| {
        matches!(event.kind, MediaEventKind::VolumeChanged { .. })
    })
    .and_then(|event| match event.kind {
        MediaEventKind::VolumeChanged { volume } => Some(volume),
        _ => None,
    })
    .expect("volume confirmation");
    let native = fixture.inspect();
    assert_eq!(native.effects, effects);
    assert!(native_float_readback_matches(
        processing_gain,
        native.processing_gain
    ));
    assert_eq!(confirmed_volume, native.user_volume);
    assert!(native_float_readback_matches(0.8, confirmed_volume));
    assert!(!native.user_muted);

    let mut fixture = NativeFixture::load(&wav);
    fixture.send(MediaCommand::SetMute(true));
    wait_for_event(&fixture.events, TEST_TIMEOUT, |event| {
        matches!(event.kind, MediaEventKind::MuteChanged(true))
    })
    .expect("mute confirmation");
    fixture
        .service
        .set_effects(fixture.generation, effects)
        .expect("effects admission");
    let native = fixture.inspect();
    assert!(native.user_muted);
    assert_eq!(native.user_volume, 1.0);
}

#[test]
fn seek_preserves_rate_for_playing_source() {
    let scratch = ScratchDir::new("audio-rate-seek-playing");
    let wav = generate_test_wav(&scratch, "rate_seek_playing", 5_000);
    let mut fixture = NativeFixture::load(&wav);
    fixture.send(MediaCommand::SetRate(1.5));
    fixture.wait_for_rate(1.5);
    fixture.send(MediaCommand::Play);
    fixture.wait_for_state(PlaybackState::Playing);

    let target = Duration::from_secs(13);
    fixture.send(MediaCommand::Seek(target));
    assert_seek(&fixture, 1.5, target, gst::State::Playing);
}

#[test]
fn load_into_after_clear_source_drift() {
    let scratch = ScratchDir::new("audio-svc-drift-clear");
    let wav = generate_test_wav(&scratch, "drift_clear", 100);
    let (mut svc, rx) = AudioService::new(AudioServiceConfig::default()).expect("svc");
    let mut state = MediaState::new();
    state.clear_source();
    state.clear_source();
    let cleared_gn = state.clear_source();
    assert_eq!(cleared_gn.raw(), 3, "state must be at gen 3");
    assert_eq!(svc.generation().raw(), 0, "service must still be at gen 0");

    let gn = svc
        .load_into(MediaSource::local(&wav), &mut state)
        .expect("load_into after clear");
    assert_eq!(
        gn.raw(),
        4,
        "generation must be state's next (4), not service's next (1)"
    );
    assert_eq!(svc.generation(), gn);
    assert_eq!(state.generation(), gn);

    let ready = wait_for_event(&rx, TEST_TIMEOUT, |e| {
        e.generation == gn
            && matches!(
                e.kind,
                MediaEventKind::SourceReady { .. } | MediaEventKind::Error(_)
            )
    })
    .expect("preroll after drift");
    assert!(
        matches!(ready.kind, MediaEventKind::SourceReady { .. }),
        "SourceReady required, got: {:?}",
        ready.kind
    );
    assert!(
        state.apply(&ready),
        "SourceReady must be accepted after clear_source + load_into"
    );
    svc.shutdown();
}

#[cfg(feature = "mpris")]
#[test]
fn mpris_seek_backend_default_drain_preserves_confirmed_discontinuities() {
    use crate::media::mpris::{MprisCall, MprisConnection, MprisProperties, MprisRefusal};
    #[derive(Default)]
    struct Published(Vec<(i64, Option<crate::media::PositionDiscontinuity>)>);
    impl MprisConnection for Published {
        fn publish(&mut self, properties: &MprisProperties) {
            self.0.push((properties.position_micros, None));
        }
        fn publish_with_discontinuity(
            &mut self,
            properties: &MprisProperties,
            marker: Option<crate::media::PositionDiscontinuity>,
        ) {
            self.0.push((properties.position_micros, marker));
        }
        fn refuse(&mut self, _: &MprisCall, _: MprisRefusal) {
            panic!("unexpected refusal");
        }
    }
    let scratch = ScratchDir::new("mpris-native-seek");
    let wav = generate_test_wav(&scratch, "seek", 5_000);
    let mut fixture = NativeFixture::load(&wav);
    let adapter = MprisAdapter::new("NativeSeek");
    let mut published = Published::default();
    assert!(fixture.state.last_position_discontinuity().is_none());
    fixture.send(MediaCommand::Seek(Duration::from_secs(13)));
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        fixture.service.apply_events(
            &fixture.events,
            &mut fixture.state,
            &adapter,
            &mut published,
        );
        if fixture.state.last_position_discontinuity().is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "confirmed native seek was not published"
        );
        thread::sleep(Duration::from_millis(5));
    }
    let native = fixture.inspect();
    assert_eq!(native.state, gst::State::Paused);
    assert!(
        native
            .position
            .unwrap()
            .abs_diff(fixture.state.snapshot().position)
            < Duration::from_millis(1)
    );
    assert!(
        fixture
            .state
            .snapshot()
            .position
            .abs_diff(Duration::from_secs(13))
            < Duration::from_secs(1)
    );
    let marker = fixture.state.last_position_discontinuity().unwrap();
    assert_eq!(published.0.last().unwrap().1, Some(marker));
    fixture.send(MediaCommand::Play);
    let deadline = Instant::now() + TEST_TIMEOUT;
    let position = fixture.state.snapshot().position;
    loop {
        fixture.service.apply_events(
            &fixture.events,
            &mut fixture.state,
            &adapter,
            &mut published,
        );
        if fixture.state.snapshot().position > position + Duration::from_millis(40) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "ordinary playback did not progress"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(fixture.state.last_position_discontinuity(), Some(marker));
    assert_eq!(published.0.last().unwrap().1, Some(marker));
    fixture.send(MediaCommand::Pause);
    fixture.wait_for_state(PlaybackState::Paused);
    fixture.send(MediaCommand::Seek(Duration::from_secs(20)));
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        fixture.service.apply_events(
            &fixture.events,
            &mut fixture.state,
            &adapter,
            &mut published,
        );
        if fixture
            .state
            .last_position_discontinuity()
            .is_some_and(|next| next != marker)
        {
            break;
        }
        assert!(Instant::now() < deadline, "second seek identity was lost");
        thread::sleep(Duration::from_millis(5));
    }
    assert!(
        fixture
            .state
            .snapshot()
            .position
            .abs_diff(Duration::from_secs(20))
            < Duration::from_secs(1)
    );
    let second = fixture.state.last_position_discontinuity().unwrap();
    fixture.send(MediaCommand::Stop);
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        fixture.service.apply_events(
            &fixture.events,
            &mut fixture.state,
            &adapter,
            &mut published,
        );
        if fixture.state.snapshot().playback == PlaybackState::Stopped
            && fixture.state.snapshot().position == Duration::ZERO
            && fixture
                .state
                .last_position_discontinuity()
                .is_some_and(|next| next != second)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "confirmed stop rewind was not published"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
