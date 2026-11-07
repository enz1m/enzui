//! The audio service's worker thread: its command loop, pipeline build, bus
//! polling and error classification.

use super::*;
use crate::media::gst_worker::PayloadCounter;

pub(super) fn audio_worker_loop(
    cmd_rx: std::sync::mpsc::Receiver<AudioCommand>,
    evt_tx: EventSender,
    stop: Arc<AtomicBool>,
    config: AudioServiceConfig,
) {
    let mut pipeline_state: Option<LivePipeline> = None;
    let mut current_generation = Generation::FIRST;
    let mut last_position_poll = Instant::now();

    loop {
        if stop.load(Ordering::Acquire) {
            // Settle queued commands when shutdown wins before receiving them.
            drain_pending_commands_on_shutdown(&cmd_rx, &evt_tx);
            break;
        }
        // Without a live pipeline, shutdown wakes the blocking receive.
        let received = if pipeline_state.is_some() {
            cmd_rx.recv_timeout(config.bus_poll_interval)
        } else {
            cmd_rx
                .recv()
                .map_err(|_| std::sync::mpsc::RecvTimeoutError::Disconnected)
        };

        match received {
            Ok(cmd) => {
                if stop.load(Ordering::Acquire) {
                    // Shutdown can win after recv dequeues an accepted command.
                    emit_shutdown_refusal_for(&cmd, &evt_tx);
                    drain_pending_commands_on_shutdown(&cmd_rx, &evt_tx);
                    break;
                }
                match cmd.kind {
                    AudioCommandKind::Load {
                        source,
                        cancel,
                        controls,
                    } => {
                        if let Some(live) = pipeline_state.take() {
                            live.reset_stop_debounce();
                            live.shutdown();
                        }
                        current_generation = cmd.generation;
                        let gen_cancel = cancel;
                        let prepared =
                            prepare_source_worker(&source, &config.gate, &stop, &gen_cancel);
                        match prepared {
                            Ok(prepared_src) => {
                                match build_audio_pipeline(
                                    &prepared_src,
                                    &config,
                                    &stop,
                                    &gen_cancel,
                                    controls,
                                ) {
                                    Ok(live) => {
                                        let capabilities = live.query_capabilities();
                                        let duration = live
                                            .pipeline
                                            .query_duration::<gst::ClockTime>()
                                            .map(clock_time_to_duration);
                                        evt_tx.send(MediaEvent {
                                            generation: current_generation,
                                            kind: MediaEventKind::SourceReady {
                                                capabilities,
                                                duration,
                                            },
                                        });
                                        pipeline_state = Some(live);
                                    }
                                    Err(error) => {
                                        evt_tx.send(MediaEvent {
                                            generation: current_generation,
                                            kind: MediaEventKind::Error(error),
                                        });
                                    }
                                }
                            }
                            Err(error) => {
                                evt_tx.send(MediaEvent {
                                    generation: current_generation,
                                    kind: MediaEventKind::Error(error),
                                });
                            }
                        }
                    }
                    AudioCommandKind::Play => {
                        if cmd.generation != current_generation {
                            continue;
                        }
                        match &pipeline_state {
                            Some(live) if !live.is_terminal() => {
                                live.reset_stop_debounce();
                                let silent = live.volume_element.property::<bool>("mute")
                                    || live.volume_element.property::<f64>("volume") == 0.0
                                    || live
                                        .processing_volume_element
                                        .property::<f64>("volume-full-range")
                                        == 0.0;
                                // Silent starts need a clock that advances without sink output.
                                if silent && live.pipeline.current_state() != gst::State::Playing {
                                    live.pipeline.use_clock(Some(&gst::SystemClock::obtain()));
                                }
                                if live.pipeline.set_state(gst::State::Playing).is_err() {
                                    evt_tx.send(MediaEvent {
                                        generation: current_generation,
                                        kind: MediaEventKind::Error(backend_error(
                                            "the audio pipeline could not resume",
                                        )),
                                    });
                                }
                            }
                            _ => refuse_no_live_pipeline(current_generation, "play", &evt_tx),
                        }
                    }
                    AudioCommandKind::Pause => {
                        if cmd.generation != current_generation {
                            continue;
                        }
                        match &pipeline_state {
                            Some(live) if !live.is_terminal() => {
                                live.reset_stop_debounce();
                                if live.pipeline.set_state(gst::State::Paused).is_err() {
                                    evt_tx.send(MediaEvent {
                                        generation: current_generation,
                                        kind: MediaEventKind::Error(backend_error(
                                            "the audio pipeline could not pause",
                                        )),
                                    });
                                }
                            }
                            _ => refuse_no_live_pipeline(current_generation, "pause", &evt_tx),
                        }
                    }
                    AudioCommandKind::Stop => {
                        if cmd.generation != current_generation {
                            continue;
                        }
                        match &pipeline_state {
                            Some(live) if !live.is_terminal() => {
                                run_stop_transaction(live, current_generation, &config, &evt_tx);
                            }
                            _ => refuse_no_live_pipeline(current_generation, "stop", &evt_tx),
                        }
                    }
                    AudioCommandKind::Seek(position) => {
                        if cmd.generation != current_generation {
                            continue;
                        }
                        match &pipeline_state {
                            Some(live) if !live.is_terminal() => {
                                if seek_at_confirmed_rate(live, position).is_err() {
                                    evt_tx.send(MediaEvent {
                                        generation: current_generation,
                                        kind: MediaEventKind::Error(MediaError {
                                            kind: MediaErrorKind::SeekFailed,
                                            message: "the audio pipeline refused the seek".into(),
                                        }),
                                    });
                                } else {
                                    live.pending_seek.store(true, Ordering::Release);
                                }
                            }
                            _ => refuse_no_live_pipeline(current_generation, "seek", &evt_tx),
                        }
                    }
                    AudioCommandKind::SetVolume(v) => {
                        if cmd.generation != current_generation {
                            continue;
                        }
                        match &pipeline_state {
                            Some(live) if !live.is_terminal() => {
                                let clamped = sanitize_volume(v);
                                live.volume_element.set_property("volume", clamped);
                                let confirmed = live.volume_element.property::<f64>("volume");
                                if !native_float_readback_matches(clamped, confirmed) {
                                    evt_tx.send(MediaEvent {
                                        generation: current_generation,
                                        kind: MediaEventKind::Error(backend_error(
                                            "the audio pipeline refused the volume change",
                                        )),
                                    });
                                } else {
                                    evt_tx.send(MediaEvent {
                                        generation: current_generation,
                                        kind: MediaEventKind::VolumeChanged { volume: confirmed },
                                    });
                                }
                            }
                            _ => refuse_no_live_pipeline(current_generation, "set volume", &evt_tx),
                        }
                    }
                    AudioCommandKind::SetMute(m) => {
                        if cmd.generation != current_generation {
                            continue;
                        }
                        match &pipeline_state {
                            Some(live) if !live.is_terminal() => {
                                live.volume_element.set_property("mute", m);
                                let confirmed = live.volume_element.property::<bool>("mute");
                                if confirmed != m {
                                    evt_tx.send(MediaEvent {
                                        generation: current_generation,
                                        kind: MediaEventKind::Error(backend_error(
                                            "the audio pipeline refused the mute change",
                                        )),
                                    });
                                } else {
                                    evt_tx.send(MediaEvent {
                                        generation: current_generation,
                                        kind: MediaEventKind::MuteChanged(confirmed),
                                    });
                                }
                            }
                            _ => refuse_no_live_pipeline(current_generation, "set mute", &evt_tx),
                        }
                    }
                    AudioCommandKind::SetRate(rate) => {
                        if cmd.generation != current_generation {
                            continue;
                        }
                        apply_rate_change(&mut pipeline_state, current_generation, rate, &evt_tx);
                    }
                    AudioCommandKind::SetEffects(effects) => {
                        if cmd.generation != current_generation {
                            continue;
                        }
                        match &mut pipeline_state {
                            Some(live) if !live.is_terminal() => {
                                if let Err(error) = live.apply_effects(effects) {
                                    evt_tx.send(MediaEvent {
                                        generation: current_generation,
                                        kind: MediaEventKind::Error(error),
                                    });
                                }
                            }
                            _ => {
                                refuse_no_live_pipeline(current_generation, "set effects", &evt_tx);
                            }
                        }
                    }
                    #[cfg(test)]
                    AudioCommandKind::InspectNative(reply) => {
                        if cmd.generation != current_generation {
                            continue;
                        }
                        if let Some(live) = pipeline_state.as_ref().filter(|l| !l.is_terminal()) {
                            let _ = reply.try_send(live.inspect_native());
                        }
                    }
                    #[cfg(test)]
                    AudioCommandKind::ResetNativeAudio => {
                        if cmd.generation != current_generation {
                            continue;
                        }
                        if let Some(live) = pipeline_state.as_ref().filter(|l| !l.is_terminal()) {
                            live.reset_audio_measurement();
                        }
                    }
                    AudioCommandKind::Shutdown => {
                        drain_pending_commands_on_shutdown(&cmd_rx, &evt_tx);
                        break;
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }

        let terminal_after_bus = if let Some(live) = &pipeline_state {
            poll_bus(live, current_generation, &evt_tx, &stop, &config);
            let terminal = live.is_terminal();
            if !terminal
                && !stop.load(Ordering::Acquire)
                && last_position_poll.elapsed() >= config.position_poll_interval
            {
                poll_position(live, current_generation, &evt_tx);
                last_position_poll = Instant::now();
            }
            terminal
        } else {
            false
        };
        if terminal_after_bus && let Some(live) = pipeline_state.take() {
            live.shutdown();
        }
    }

    if let Some(live) = pipeline_state.take() {
        live.shutdown();
    }
    evt_tx.close();
}

pub(super) fn drain_pending_commands_on_shutdown(
    cmd_rx: &std::sync::mpsc::Receiver<AudioCommand>,
    evt_tx: &EventSender,
) {
    while let Ok(pending) = cmd_rx.try_recv() {
        emit_shutdown_refusal_for(&pending, evt_tx);
    }
}

pub(super) fn emit_shutdown_refusal_for(cmd: &AudioCommand, evt_tx: &EventSender) {
    if matches!(cmd.kind, AudioCommandKind::Shutdown) {
        return;
    }
    evt_tx.send(MediaEvent {
        generation: cmd.generation,
        kind: MediaEventKind::Error(MediaError {
            kind: MediaErrorKind::Refused,
            message: "the audio service is shutting down".to_owned(),
        }),
    });
}

pub(super) fn refuse_no_live_pipeline(generation: Generation, command: &str, evt_tx: &EventSender) {
    evt_tx.send(MediaEvent {
        generation,
        kind: MediaEventKind::Error(MediaError {
            kind: MediaErrorKind::Refused,
            message: format!("{command}: no live audio pipeline for this generation"),
        }),
    });
}

pub(super) fn run_stop_transaction(
    live: &LivePipeline,
    generation: Generation,
    config: &AudioServiceConfig,
    evt_tx: &EventSender,
) {
    live.reset_stop_debounce();
    // Seek before changing state: a refused seek must not queue a Stopped bus outcome.
    if seek_at_confirmed_rate(live, Duration::ZERO).is_err() {
        evt_tx.send(MediaEvent {
            generation,
            kind: MediaEventKind::Error(MediaError {
                kind: MediaErrorKind::SeekFailed,
                message: "the audio pipeline refused the stop seek".to_owned(),
            }),
        });
        return;
    }
    if live.pipeline.set_state(gst::State::Ready).is_err() {
        evt_tx.send(MediaEvent {
            generation,
            kind: MediaEventKind::Error(backend_error("the audio pipeline could not stop")),
        });
        return;
    }
    live.restore_rate_on_preroll.store(true, Ordering::Release);
    // Suppress delayed bus confirmation even if the explicit Stop probe times out.
    live.arm_stop_debounce();
    let probe_timeout = gst::ClockTime::from_mseconds(
        u64::try_from(config.bus_poll_interval.as_millis().saturating_mul(4)).unwrap_or(200),
    );
    let (result, current, pending) = live.pipeline.state(probe_timeout);
    let established =
        result.is_ok() && current == gst::State::Ready && pending == gst::State::VoidPending;
    if established {
        live.pending_seek.store(false, Ordering::Release);
        evt_tx.send(MediaEvent {
            generation,
            kind: MediaEventKind::StateChanged(PlaybackState::Stopped),
        });
        evt_tx.send(MediaEvent {
            generation,
            kind: MediaEventKind::PositionDiscontinuity(Duration::ZERO),
        });
    } else {
        evt_tx.send(MediaEvent {
            generation,
            kind: MediaEventKind::Error(backend_error("the audio pipeline could not confirm stop")),
        });
    }
}

fn seek_at_confirmed_rate(
    live: &LivePipeline,
    position: Duration,
) -> Result<(), gst::glib::error::BoolError> {
    let target = u64::try_from(position.as_nanos())
        .map_or(gst::ClockTime::MAX, gst::ClockTime::from_nseconds);
    let rate = live.accepted_rate;
    live.pipeline.seek(
        rate,
        gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
        gst::SeekType::Set,
        target,
        gst::SeekType::None,
        gst::ClockTime::NONE,
    )
}

pub(super) fn apply_rate_change(
    pipeline_state: &mut Option<LivePipeline>,
    generation: Generation,
    rate: f64,
    evt_tx: &EventSender,
) {
    let Some(live) = pipeline_state.as_mut().filter(|l| !l.is_terminal()) else {
        evt_tx.send(MediaEvent {
            generation,
            kind: MediaEventKind::Error(refused_error(
                "set rate: no live audio pipeline for this generation",
            )),
        });
        return;
    };
    let start = live
        .pipeline
        .query_position::<gst::ClockTime>()
        .or_else(|| {
            live.last_position
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .and_then(|duration| {
                    u64::try_from(duration.as_nanos())
                        .ok()
                        .map(gst::ClockTime::from_nseconds)
                })
        })
        .unwrap_or(gst::ClockTime::ZERO);
    let outcome = live.pipeline.seek(
        rate,
        gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
        gst::SeekType::Set,
        start,
        gst::SeekType::None,
        gst::ClockTime::NONE,
    );
    if outcome.is_err() {
        evt_tx.send(MediaEvent {
            generation,
            kind: MediaEventKind::Error(MediaError {
                kind: MediaErrorKind::Backend,
                message: "the audio pipeline refused the rate change".into(),
            }),
        });
        return;
    }
    debug_assert!(
        (crate::media::MIN_PLAYBACK_RATE..=crate::media::MAX_PLAYBACK_RATE).contains(&rate)
            && rate.is_finite(),
        "admission must sanitise rate before the worker sees it",
    );
    live.accepted_rate = rate;
    evt_tx.send(MediaEvent {
        generation,
        kind: MediaEventKind::RateChanged { rate },
    });
}

pub(super) fn prepare_source_worker(
    source: &MediaSource,
    gate: &MediaGate,
    stop: &Arc<AtomicBool>,
    gen_cancel: &Arc<AtomicBool>,
) -> Result<PreparedSource, MediaError> {
    match source {
        MediaSource::Local(path) => Ok(PreparedSource::local(path.clone())),
        MediaSource::Remote(remote) => {
            let clock = MonotonicClock::start();
            let redirect_probe = CurlRedirectProbe;
            let combined = CombinedCancel {
                stop: Arc::clone(stop),
                gen_cancel: Arc::clone(gen_cancel),
            };
            let effects = GateEffects {
                resolver: &SystemResolver,
                probe: Some(&redirect_probe),
                clock: &clock,
                cancel: &combined,
            };
            let approved = gate.approve(remote, &effects).map_err(MediaError::from)?;
            Ok(PreparedSource::approved_stream(approved))
        }
    }
}

pub(super) struct CombinedCancel {
    stop: Arc<AtomicBool>,
    gen_cancel: Arc<AtomicBool>,
}

impl crate::media::remote_gate::CancelView for CombinedCancel {
    fn is_cancelled(&self) -> bool {
        self.stop.load(Ordering::Acquire) || self.gen_cancel.load(Ordering::Acquire)
    }
}

pub(super) struct LivePipeline {
    pipeline: gst::Pipeline,
    equalizer_element: Option<gst::Element>,
    processing_volume_element: gst::Element,
    volume_element: gst::Element,
    accepted_effects: AudioEffects,
    _pinned_proxy: Option<PinnedProxy>,
    payload: Option<PayloadCounter>,
    last_position: Mutex<Option<Duration>>,
    accepted_rate: f64,
    pending_seek: AtomicBool,
    restore_rate_on_preroll: AtomicBool,
    #[cfg(test)]
    observed_seek: Arc<Mutex<Option<(f64, Duration)>>>,
    #[cfg(test)]
    audio_measurement: Arc<Mutex<AudioMeasurement>>,
    terminal: AtomicBool,
    // Suppress Stop transition tails until Ready/Null is consumed or a new state command arrives.
    stop_debounce: AtomicBool,
}
impl LivePipeline {
    fn shutdown(&self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
    fn is_terminal(&self) -> bool {
        self.terminal.load(Ordering::Acquire)
    }
    fn mark_terminal(&self) {
        self.terminal.store(true, Ordering::Release);
    }
    fn arm_stop_debounce(&self) {
        self.stop_debounce.store(true, Ordering::Release);
    }
    fn reset_stop_debounce(&self) {
        self.stop_debounce.store(false, Ordering::Release);
    }
    fn is_stop_debounced(&self) -> bool {
        self.stop_debounce.load(Ordering::Acquire)
    }
    fn take_stop_debounce(&self) -> bool {
        self.stop_debounce.swap(false, Ordering::AcqRel)
    }
    fn apply_effects(&mut self, effects: AudioEffects) -> Result<(), MediaError> {
        if effects.requests_equalizer() && self.equalizer_element.is_none() {
            return Err(refused_error(
                "equalizer processing unavailable: GStreamer element `equalizer-10bands` is not installed",
            ));
        }
        if let Some(equalizer) = &self.equalizer_element {
            let applied = if effects.equalizer_enabled() {
                effects.equalizer_gains_db()
            } else {
                [0.0; AUDIO_EQUALIZER_BAND_COUNT]
            };
            for (band, gain_db) in applied.into_iter().enumerate() {
                equalizer.set_property(&format!("band{band}"), gain_db);
                let confirmed = equalizer.property::<f64>(&format!("band{band}"));
                if (confirmed - gain_db).abs() > f64::EPSILON {
                    return Err(backend_error(
                        "the audio pipeline refused an equalizer gain",
                    ));
                }
            }
        }
        self.processing_volume_element
            .set_property("volume-full-range", effects.processing_gain());
        let confirmed = self
            .processing_volume_element
            .property::<f64>("volume-full-range");
        if !native_float_readback_matches(effects.processing_gain(), confirmed) {
            return Err(backend_error(
                "the audio pipeline refused the processing gain",
            ));
        }
        self.accepted_effects = effects;
        Ok(())
    }
    fn query_seekable(&self) -> bool {
        let mut query = gst::query::Seeking::new(gst::Format::Time);
        if self.pipeline.query(&mut query) {
            let (seekable, _, _) = query.result();
            seekable
        } else {
            false
        }
    }
    fn query_capabilities(&self) -> Capabilities {
        Capabilities {
            seekable: self.query_seekable(),
            has_audio: true,
            has_video: false,
        }
    }
    #[cfg(test)]
    fn inspect_native(&self) -> NativePlayback {
        let mut segment = gst::query::Segment::new(gst::Format::Time);
        let rate = self
            .pipeline
            .query(&mut segment)
            .then(|| segment.result().0);
        let (_, state, _) = self.pipeline.state(gst::ClockTime::ZERO);
        NativePlayback {
            position: self
                .pipeline
                .query_position::<gst::ClockTime>()
                .map(clock_time_to_duration),
            rate,
            state,
            last_seek: *self
                .observed_seek
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            effects: self.accepted_effects,
            processing_gain: self
                .processing_volume_element
                .property::<f64>("volume-full-range"),
            user_volume: self.volume_element.property::<f64>("volume"),
            user_muted: self.volume_element.property::<bool>("mute"),
            audio_rms: self
                .audio_measurement
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .rms(),
        }
    }
    #[cfg(test)]
    fn reset_audio_measurement(&self) {
        *self
            .audio_measurement
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = AudioMeasurement::default();
    }
}

#[cfg(test)]
#[derive(Debug, Default)]
struct AudioMeasurement {
    sum_squares: f64,
    sample_count: u64,
}

#[cfg(test)]
impl AudioMeasurement {
    fn rms(&self) -> Option<f64> {
        (self.sample_count != 0).then(|| (self.sum_squares / self.sample_count as f64).sqrt())
    }
}
impl std::fmt::Debug for LivePipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LivePipeline")
            .field("pipeline", &"<gst>")
            .finish()
    }
}

pub(super) fn is_cancelled(stop: &AtomicBool, gen_cancel: &AtomicBool) -> bool {
    stop.load(Ordering::Acquire) || gen_cancel.load(Ordering::Acquire)
}

pub(super) fn build_audio_pipeline(
    source: &PreparedSource,
    config: &AudioServiceConfig,
    stop: &Arc<AtomicBool>,
    gen_cancel: &Arc<AtomicBool>,
    controls: SourceControls,
) -> Result<LivePipeline, MediaError> {
    let uri = crate::media::gst_worker::pipeline_uri(source)?;
    let pinned_proxy = source.approved().map(PinnedProxy::start).transpose()?;
    let remote_configuration = pinned_proxy
        .as_ref()
        .map(|_| Arc::new(Mutex::new(None::<Result<(), MediaError>>)));
    let payload = pinned_proxy.as_ref().map(|_| PayloadCounter::new());
    let pipeline = gst::Pipeline::new();
    let decode = gst::ElementFactory::make("uridecodebin")
        .property("uri", &uri)
        .build()
        .map_err(|_| missing_element("uridecodebin"))?;

    if let Some(approved) = source.approved() {
        crate::media::gst_worker::apply_stream_budget(&decode, approved)?;
        let limits = approved.limits();
        let approved_uri = approved.transport_url().to_owned();
        let proxy = pinned_proxy
            .as_ref()
            .expect("remote creates proxy")
            .client_configuration();
        let configuration = Arc::clone(remote_configuration.as_ref().expect("remote creates cfg"));
        let counted = payload.clone().expect("remote creates a payload counter");
        decode.connect("source-setup", false, move |values| {
            let result = values
                .get(1)
                .and_then(|v| v.get::<gst::Element>().ok())
                .ok_or_else(|| refused_error("uridecodebin did not expose its live source"))
                .and_then(|src| {
                    let r = crate::media::gst_worker::configure_remote_source(
                        &src,
                        &proxy,
                        limits,
                        &approved_uri,
                    );
                    if r.is_err() {
                        let _ = src.set_state(gst::State::Null);
                    } else {
                        counted.observe(&src);
                    }
                    r
                });
            if let Ok(mut slot) = configuration.lock() {
                *slot = Some(result);
            }
            None
        });
    }

    let audioconvert = gst::ElementFactory::make("audioconvert")
        .build()
        .map_err(|_| missing_element("audioconvert"))?;
    #[cfg(not(test))]
    let equalizer_factory = "equalizer-10bands";
    #[cfg(test)]
    let equalizer_factory = config.equalizer_factory;
    let equalizer = gst::ElementFactory::make(equalizer_factory).build().ok();
    if let Some(equalizer) = &equalizer {
        validate_equalizer_schema(equalizer)?;
    }
    let processing_volume = gst::ElementFactory::make("volume")
        .build()
        .map_err(|_| missing_element("volume"))?;
    let volume = gst::ElementFactory::make("volume")
        .build()
        .map_err(|_| missing_element("volume"))?;
    validate_processing_volume_schema(&processing_volume)?;
    validate_volume_schema(&volume)?;
    volume.set_property("volume", controls.volume);
    let confirmed_volume = volume.property::<f64>("volume");
    if !native_float_readback_matches(controls.volume, confirmed_volume) {
        return Err(backend_error(
            "the audio pipeline refused the initial volume",
        ));
    }
    volume.set_property("mute", controls.muted);
    if volume.property::<bool>("mute") != controls.muted {
        return Err(backend_error("the audio pipeline refused the initial mute"));
    }
    #[cfg(not(test))]
    let audio_sink_factory = "autoaudiosink";
    #[cfg(test)]
    let audio_sink_factory = config.audio_sink_factory;
    let audiosink = gst::ElementFactory::make(audio_sink_factory)
        .build()
        .map_err(|_| missing_element(audio_sink_factory))?;
    #[cfg(test)]
    if audio_sink_factory == "fakesink" {
        audiosink.set_property("sync", true);
    }
    #[cfg(test)]
    let observed_seek = Arc::new(Mutex::new(None));
    #[cfg(test)]
    let audio_measurement = Arc::new(Mutex::new(AudioMeasurement::default()));
    #[cfg(test)]
    if audio_sink_factory == "fakesink"
        && let Some(sink_pad) = audiosink.static_pad("sink")
    {
        let observed_seek_for_probe = Arc::clone(&observed_seek);
        let measurement_for_probe = Arc::clone(&audio_measurement);
        let _ = sink_pad.add_probe(
            gst::PadProbeType::EVENT_UPSTREAM | gst::PadProbeType::BUFFER,
            move |pad, info| {
                match &info.data {
                    Some(gst::PadProbeData::Event(event))
                        if matches!(event.view(), gst::EventView::Seek(_)) =>
                    {
                        if let gst::EventView::Seek(seek) = event.view() {
                            let (rate, _, _, start, _, _) = seek.get();
                            if let gst::GenericFormattedValue::Time(Some(start)) = start {
                                *observed_seek_for_probe
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner) =
                                    Some((rate, clock_time_to_duration(start)));
                            }
                        }
                    }
                    Some(gst::PadProbeData::Buffer(buffer)) => {
                        let format = pad.current_caps().and_then(|caps| {
                            caps.structure(0)
                                .and_then(|structure| structure.get::<String>("format").ok())
                        });
                        if let (Some(format), Ok(map)) = (format, buffer.map_readable()) {
                            let mut measurement = measurement_for_probe
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            accumulate_audio_samples(&mut measurement, &format, map.as_slice());
                        }
                    }
                    _ => {}
                }
                gst::PadProbeReturn::Ok
            },
        );
    }
    pipeline
        .add_many([
            &decode,
            &audioconvert,
            &processing_volume,
            &volume,
            &audiosink,
        ])
        .map_err(|_| backend_error("could not assemble the audio pipeline"))?;
    if let Some(equalizer) = &equalizer {
        pipeline
            .add(equalizer)
            .map_err(|_| backend_error("could not add the audio equalizer"))?;
        audioconvert
            .link(equalizer)
            .map_err(|_| backend_error("link audioconvert→equalizer"))?;
        equalizer
            .link(&processing_volume)
            .map_err(|_| backend_error("link equalizer→processing volume"))?;
    } else {
        audioconvert
            .link(&processing_volume)
            .map_err(|_| backend_error("link audioconvert→processing volume"))?;
    }
    processing_volume
        .link(&volume)
        .map_err(|_| backend_error("link processing volume→user volume"))?;
    volume
        .link(&audiosink)
        .map_err(|_| backend_error("link user volume→autoaudiosink"))?;

    let convert_weak = audioconvert.downgrade();
    decode.connect_pad_added(move |_, pad| {
        let Some(convert) = convert_weak.upgrade() else {
            return;
        };
        let Some(mt) = pad
            .current_caps()
            .and_then(|c| c.structure(0).map(|s| s.name().to_string()))
        else {
            return;
        };
        if mt.starts_with("audio/")
            && let Some(sp) = convert.static_pad("sink")
            && !sp.is_linked()
        {
            let _ = pad.link(&sp);
        }
    });

    if pipeline.set_state(gst::State::Paused).is_err() {
        let _ = pipeline.set_state(gst::State::Null);
        return Err(backend_error("the audio pipeline would not start"));
    }

    if let Some(ref cfg_slot) = remote_configuration {
        let deadline = Instant::now() + config.preroll_timeout;
        loop {
            if is_cancelled(stop, gen_cancel) {
                let _ = pipeline.set_state(gst::State::Null);
                return Err(refused_error("audio pipeline setup cancelled"));
            }
            if let Ok(guard) = cfg_slot.lock()
                && let Some(result) = &*guard
            {
                if let Err(e) = result.clone() {
                    let _ = pipeline.set_state(gst::State::Null);
                    return Err(e);
                }
                break;
            }
            if Instant::now() >= deadline {
                let _ = pipeline.set_state(gst::State::Null);
                return Err(refused_error("remote audio source not configured in time"));
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    {
        let deadline = Instant::now() + config.preroll_timeout;
        let bus = pipeline.bus();
        loop {
            if is_cancelled(stop, gen_cancel) {
                let _ = pipeline.set_state(gst::State::Null);
                return Err(refused_error("audio pipeline setup cancelled"));
            }
            let (_, current, pending) = pipeline.state(gst::ClockTime::from_mseconds(50));
            if current == gst::State::Paused && pending == gst::State::VoidPending {
                break;
            }
            if let Some(ref b) = bus
                && let Some(msg) = b.timed_pop(gst::ClockTime::ZERO)
            {
                let src_name = msg.src().map(|s| s.name().to_string());
                if let gst::MessageView::Error(err) = msg.view() {
                    let glib_err = err.error();
                    let received = payload.as_ref().and_then(PayloadCounter::received);
                    let classified = classify_bus_error(&glib_err, src_name.as_deref(), received);
                    let _ = pipeline.set_state(gst::State::Null);
                    return Err(classified);
                }
            }
            if Instant::now() >= deadline {
                let _ = pipeline.set_state(gst::State::Null);
                return Err(backend_error(
                    "audio pipeline did not reach preroll in time",
                ));
            }
        }
    }

    Ok(LivePipeline {
        pipeline,
        equalizer_element: equalizer,
        processing_volume_element: processing_volume,
        volume_element: volume,
        accepted_effects: AudioEffects::FLAT,
        _pinned_proxy: pinned_proxy,
        payload,
        last_position: Mutex::new(None),
        accepted_rate: crate::media::DEFAULT_PLAYBACK_RATE,
        pending_seek: AtomicBool::new(false),
        restore_rate_on_preroll: AtomicBool::new(false),
        #[cfg(test)]
        observed_seek,
        #[cfg(test)]
        audio_measurement,
        terminal: AtomicBool::new(false),
        stop_debounce: AtomicBool::new(false),
    })
}

#[cfg(test)]
fn accumulate_audio_samples(measurement: &mut AudioMeasurement, format: &str, bytes: &[u8]) {
    match format {
        "F32LE" => {
            for sample in bytes.as_chunks::<4>().0 {
                let sample = f64::from(f32::from_le_bytes([
                    sample[0], sample[1], sample[2], sample[3],
                ]));
                measurement.sum_squares += sample * sample;
                measurement.sample_count += 1;
            }
        }
        "F64LE" => {
            for sample in bytes.as_chunks::<8>().0 {
                let sample = f64::from_le_bytes([
                    sample[0], sample[1], sample[2], sample[3], sample[4], sample[5], sample[6],
                    sample[7],
                ]);
                measurement.sum_squares += sample * sample;
                measurement.sample_count += 1;
            }
        }
        "S16LE" => {
            for sample in bytes.as_chunks::<2>().0 {
                let sample =
                    f64::from(i16::from_le_bytes([sample[0], sample[1]])) / f64::from(i16::MAX);
                measurement.sum_squares += sample * sample;
                measurement.sample_count += 1;
            }
        }
        _ => {}
    }
}

pub(super) fn native_float_readback_matches(requested: f64, confirmed: f64) -> bool {
    if !requested.is_finite() || !confirmed.is_finite() {
        return false;
    }
    let scale = requested.abs().max(confirmed.abs()).max(1.0);
    (requested - confirmed).abs() <= f64::from(f32::EPSILON) * scale
}

pub(super) fn validate_equalizer_schema(equalizer: &gst::Element) -> Result<(), MediaError> {
    for band in 0..AUDIO_EQUALIZER_BAND_COUNT {
        require_typed_readwrite_property(equalizer, &format!("band{band}"), gst::glib::Type::F64)?;
    }
    Ok(())
}

pub(super) fn validate_processing_volume_schema(volume: &gst::Element) -> Result<(), MediaError> {
    require_typed_readwrite_property(volume, "volume-full-range", gst::glib::Type::F64)
}

pub(super) fn validate_volume_schema(volume: &gst::Element) -> Result<(), MediaError> {
    require_typed_readwrite_property(volume, "volume", gst::glib::Type::F64)?;
    require_typed_readwrite_property(volume, "mute", gst::glib::Type::BOOL)?;
    Ok(())
}

pub(super) fn require_typed_readwrite_property(
    object: &gst::Element,
    name: &str,
    expected: gst::glib::Type,
) -> Result<(), MediaError> {
    let pspec = object
        .find_property(name)
        .ok_or_else(|| backend_error(&format!("audio element lacks the `{name}` property")))?;
    if pspec.value_type() != expected {
        return Err(backend_error(&format!(
            "audio element `{name}` property has an unexpected type"
        )));
    }
    let flags = pspec.flags();
    if !flags.contains(gst::glib::ParamFlags::READABLE)
        || !flags.contains(gst::glib::ParamFlags::WRITABLE)
    {
        return Err(backend_error(&format!(
            "audio element `{name}` property is not readable and writable"
        )));
    }
    Ok(())
}

pub(super) fn poll_bus(
    live: &LivePipeline,
    generation: Generation,
    evt_tx: &EventSender,
    stop: &AtomicBool,
    config: &AudioServiceConfig,
) {
    let Some(bus) = live.pipeline.bus() else {
        return;
    };
    for _ in 0..config.max_bus_messages_per_cycle {
        if stop.load(Ordering::Acquire) {
            return;
        }
        if live.is_terminal() {
            return;
        }
        let Some(msg) = bus.timed_pop(gst::ClockTime::ZERO) else {
            break;
        };
        let src_name = msg.src().map(|s| s.name().to_string());
        match msg.view() {
            gst::MessageView::Eos(_) => {
                evt_tx.send(MediaEvent {
                    generation,
                    kind: MediaEventKind::StateChanged(PlaybackState::EndOfStream),
                });
                live.mark_terminal();
            }
            gst::MessageView::Error(err) => {
                let glib_err = err.error();
                let received = live.payload.as_ref().and_then(PayloadCounter::received);
                let classified = classify_bus_error(&glib_err, src_name.as_deref(), received);
                evt_tx.send(MediaEvent {
                    generation,
                    kind: MediaEventKind::Error(classified),
                });
                live.mark_terminal();
            }
            gst::MessageView::StateChanged(sc) => {
                let is_current_pipeline = sc
                    .src()
                    .is_some_and(|s| s == live.pipeline.upcast_ref::<gst::Object>());
                if is_current_pipeline {
                    match sc.current() {
                        gst::State::Playing => {
                            evt_tx.send(MediaEvent {
                                generation,
                                kind: MediaEventKind::StateChanged(PlaybackState::Playing),
                            });
                        }
                        gst::State::Paused => {
                            // Ready discards the native segment; restore it after decoding prerolls.
                            if sc.old() == gst::State::Ready
                                && live.restore_rate_on_preroll.swap(false, Ordering::AcqRel)
                                && live.accepted_rate != crate::media::DEFAULT_PLAYBACK_RATE
                                && seek_at_confirmed_rate(live, Duration::ZERO).is_err()
                            {
                                evt_tx.send(MediaEvent {
                                    generation,
                                    kind: MediaEventKind::Error(MediaError {
                                        kind: MediaErrorKind::SeekFailed,
                                        message:
                                            "the audio pipeline could not restore playback speed"
                                                .to_owned(),
                                    }),
                                });
                                live.mark_terminal();
                                return;
                            }
                            // Stop passes through Paused before its confirmed Ready outcome.
                            if !live.is_stop_debounced() {
                                evt_tx.send(MediaEvent {
                                    generation,
                                    kind: MediaEventKind::StateChanged(PlaybackState::Paused),
                                });
                            }
                        }
                        // The explicit Stop probe owns the outcome; absorb its Ready/Null bus tail.
                        gst::State::Ready | gst::State::Null if !live.take_stop_debounce() => {
                            evt_tx.send(MediaEvent {
                                generation,
                                kind: MediaEventKind::StateChanged(PlaybackState::Stopped),
                            });
                        }
                        _ => {}
                    }
                }
            }
            gst::MessageView::DurationChanged(_) => {
                let caps = live.query_capabilities();
                let dur = live
                    .pipeline
                    .query_duration::<gst::ClockTime>()
                    .map(clock_time_to_duration);
                evt_tx.send(MediaEvent {
                    generation,
                    kind: MediaEventKind::SourceReady {
                        capabilities: caps,
                        duration: dur,
                    },
                });
            }
            gst::MessageView::ClockLost(_) => {
                handle_clock_lost(live, generation, evt_tx);
            }
            _ => {}
        }
    }
}

pub(super) fn handle_clock_lost(live: &LivePipeline, generation: Generation, evt_tx: &EventSender) {
    let (_, current, _) = live.pipeline.state(gst::ClockTime::from_mseconds(0));
    match current {
        gst::State::Playing => {
            // Documented recovery: PAUSED then PLAYING re-selects a clock.
            let paused_ok = live.pipeline.set_state(gst::State::Paused).is_ok();
            let playing_ok = paused_ok && live.pipeline.set_state(gst::State::Playing).is_ok();
            if !paused_ok || !playing_ok {
                retire_as_device_lost(live, generation, evt_tx);
                return;
            }
            let (_, settled_current, settled_pending) =
                live.pipeline.state(gst::ClockTime::from_mseconds(200));
            if settled_current != gst::State::Playing || settled_pending != gst::State::VoidPending
            {
                retire_as_device_lost(live, generation, evt_tx);
            }
            // Successful recovery leaves state publication to bus messages.
        }
        gst::State::Paused | gst::State::Ready => {
            let (_, settled_current, settled_pending) =
                live.pipeline.state(gst::ClockTime::from_mseconds(200));
            if settled_current != current || settled_pending != gst::State::VoidPending {
                retire_as_device_lost(live, generation, evt_tx);
            }
        }
        _ => {
            retire_as_device_lost(live, generation, evt_tx);
        }
    }
}

pub(super) fn retire_as_device_lost(
    live: &LivePipeline,
    generation: Generation,
    evt_tx: &EventSender,
) {
    evt_tx.send(MediaEvent {
        generation,
        kind: MediaEventKind::Error(MediaError {
            kind: MediaErrorKind::DeviceLost,
            message: DEVICE_LOST_MESSAGE.to_owned(),
        }),
    });
    live.mark_terminal();
}

pub(super) fn poll_position(live: &LivePipeline, generation: Generation, evt_tx: &EventSender) {
    if live.pending_seek.load(Ordering::Acquire) {
        let (settled, _, pending) = live.pipeline.state(gst::ClockTime::ZERO);
        if !matches!(
            settled,
            Ok(gst::StateChangeSuccess::Success | gst::StateChangeSuccess::NoPreroll)
        ) || pending != gst::State::VoidPending
        {
            return;
        }
    }
    let Some(pos) = live
        .pipeline
        .query_position::<gst::ClockTime>()
        .map(clock_time_to_duration)
    else {
        return;
    };
    if live.pending_seek.load(Ordering::Acquire) {
        live.pending_seek.store(false, Ordering::Release);
        *live
            .last_position
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pos);
        evt_tx.send(MediaEvent {
            generation,
            kind: MediaEventKind::PositionDiscontinuity(pos),
        });
        return;
    }
    let mut last = live
        .last_position
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let changed = match *last {
        Some(prev) => {
            let diff = pos.abs_diff(prev);
            diff > Duration::from_millis(10)
        }
        None => true,
    };
    if changed {
        *last = Some(pos);
        drop(last);
        evt_tx.send(MediaEvent {
            generation,
            kind: MediaEventKind::PositionUpdated(pos),
        });
    }
}

pub(super) fn clock_time_to_duration(ct: gst::ClockTime) -> Duration {
    Duration::from_nanos(ct.nseconds())
}
pub(super) fn missing_element(name: &str) -> MediaError {
    MediaError {
        kind: MediaErrorKind::UnsupportedFormat,
        message: format!("the GStreamer element `{name}` is not installed"),
    }
}
pub(super) fn backend_error(what: &str) -> MediaError {
    MediaError {
        kind: MediaErrorKind::Backend,
        message: what.to_owned(),
    }
}
pub(super) fn refused_error(what: &str) -> MediaError {
    MediaError {
        kind: MediaErrorKind::Refused,
        message: what.to_owned(),
    }
}
pub(super) const DEVICE_LOST_MESSAGE: &str = "the audio output device is no longer available";
pub(super) const SOURCE_NOT_FOUND_MESSAGE: &str = "the audio source could not be opened";
pub(super) const UNSUPPORTED_FORMAT_MESSAGE: &str = "the audio source format is not supported";
pub(super) const EMPTY_STREAM_MESSAGE: &str = "the audio stream ended before any audio arrived";

/// `received` is the payload a remote source delivered, when observed: a
/// stream or core failure after an empty transfer is the transport's.
pub(super) fn classify_bus_error(
    err: &gst::glib::Error,
    src_name: Option<&str>,
    received: Option<u64>,
) -> MediaError {
    let from_audio_sink = src_name.is_some_and(is_audio_sink_element_name);
    if received == Some(0)
        && !from_audio_sink
        && (err.kind::<gst::StreamError>().is_some() || err.kind::<gst::CoreError>().is_some())
    {
        return MediaError {
            kind: MediaErrorKind::SourceNotFound,
            message: EMPTY_STREAM_MESSAGE.to_owned(),
        };
    }
    if err.matches(gst::CoreError::MissingPlugin)
        || err.matches(gst::StreamError::CodecNotFound)
        || err.matches(gst::StreamError::WrongType)
        || err.matches(gst::StreamError::NotImplemented)
        || err.matches(gst::StreamError::TypeNotFound)
    {
        return MediaError {
            kind: MediaErrorKind::UnsupportedFormat,
            message: UNSUPPORTED_FORMAT_MESSAGE.to_owned(),
        };
    }
    if err.matches(gst::ResourceError::NotFound) && !from_audio_sink {
        return MediaError {
            kind: MediaErrorKind::SourceNotFound,
            message: SOURCE_NOT_FOUND_MESSAGE.to_owned(),
        };
    }
    // PulseAudio uses ResourceError::Failed when an established stream disconnects.
    if from_audio_sink
        && (err.matches(gst::ResourceError::Failed)
            || err.matches(gst::ResourceError::OpenWrite)
            || err.matches(gst::ResourceError::OpenReadWrite)
            || err.matches(gst::ResourceError::NotFound)
            || err.matches(gst::ResourceError::NotAuthorized)
            || err.matches(gst::ResourceError::Busy)
            || err.matches(gst::ResourceError::Read)
            || err.matches(gst::ResourceError::Write)
            || err.matches(gst::CoreError::Clock)
            || err.matches(gst::CoreError::StateChange))
    {
        return MediaError {
            kind: MediaErrorKind::DeviceLost,
            message: DEVICE_LOST_MESSAGE.to_owned(),
        };
    }
    if err.matches(gst::CoreError::Clock) {
        return MediaError {
            kind: MediaErrorKind::DeviceLost,
            message: DEVICE_LOST_MESSAGE.to_owned(),
        };
    }
    // Keyword fallback for backends whose domain classification is thin.
    let raw = err.message().to_owned();
    let lower = raw.to_ascii_lowercase();
    if is_unsupported_format_message(&lower) {
        return MediaError {
            kind: MediaErrorKind::UnsupportedFormat,
            message: UNSUPPORTED_FORMAT_MESSAGE.to_owned(),
        };
    }
    if is_device_loss_message(&lower) {
        return MediaError {
            kind: MediaErrorKind::DeviceLost,
            message: DEVICE_LOST_MESSAGE.to_owned(),
        };
    }
    MediaError {
        kind: MediaErrorKind::Backend,
        message: redact_error_message(&raw),
    }
}

pub(super) fn is_unsupported_format_message(lower: &str) -> bool {
    lower.contains("missing plugin")
        || lower.contains("no element")
        || lower.contains("not negotiated")
        || lower.contains("no uri handler")
}

pub(super) fn is_device_loss_message(lower: &str) -> bool {
    const NEEDLES: &[&str] = &[
        "could not open audio device",
        "could not open device",
        "failed to open device",
        "audio device",
        "output device",
        "no such device",
        "device disappeared",
        "device is disconnected",
        "device or resource busy",
        "clock lost",
        "clock loss",
        "lost clock",
        "pipewire disconnected",
        "pulseaudio connection",
        "connection refused by sink",
    ];
    NEEDLES.iter().any(|n| lower.contains(n))
}

pub(super) fn is_audio_sink_element_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("audiosink")
        || lower.starts_with("pulsesink")
        || lower.starts_with("pipewiresink")
        || lower.starts_with("alsasink")
        || lower.starts_with("jackaudiosink")
        || lower.starts_with("wasapisink")
        || lower.starts_with("osxaudiosink")
}

// Only whitespace-delimited tokens beginning with / or containing :// are redacted.
pub(crate) fn redact_error_message(message: &str) -> String {
    let mut out = String::with_capacity(message.len().min(210));
    for token in message.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        if token.starts_with('/') || token.contains("://") {
            out.push_str("<redacted>");
        } else {
            out.push_str(token);
        }
    }
    if out.len() > 200 {
        let mut end = 200;
        while !out.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        out.truncate(end);
        out.push('…');
    }
    out
}
