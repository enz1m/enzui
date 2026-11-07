//! The video worker thread: one `playbin3` at a time, driven by commands
//! and its own bus.

use super::pipeline::{
    backend, file_uri, missing_element, refused, scale_caps, set_flag, video_sink,
};
use super::player::{Built, Event, Phase, Player, Start, Target};
use super::{
    Command, LoadOptions, SeekOutcome, SeekRejection, Shared, SubtitleSize, VIDEO_COMMAND_CAPACITY,
    VideoServiceConfig,
};
use crate::media::CurlRedirectProbe;
use crate::media::gst_worker::{PayloadCounter, PreparedSource, configure_remote_source_with_disk};
use crate::media::pinned_proxy::PinnedProxy;
use crate::media::remote_gate::{CancelView, GateEffects, MonotonicClock, SystemResolver};
use crate::media::{Generation, MediaError, MediaErrorKind, MediaSource, PlaybackState};
use gstreamer as gst;
use gstreamer::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};

// Superseded approval failures are suppressed rather than shown as playback errors.
const SUPERSEDED: &str = "superseded";

pub(super) struct Worker {
    config: VideoServiceConfig,
    pub(super) read_ahead_directory: Option<PathBuf>,
    shared: Arc<Shared>,
    inbox: SyncSender<Command>,
    player: Option<Player>,
    volume: f64,
    muted: bool,
    subtitle_size: SubtitleSize,
    subtitle_delay_ns: i64,
    frame_limit: Option<(u32, u32)>,
    /// Whether the current generation's stalled pipeline was already reopened.
    reopened: bool,
}

impl Worker {
    pub(super) fn new(
        config: VideoServiceConfig,
        shared: Arc<Shared>,
        inbox: SyncSender<Command>,
    ) -> Self {
        Self {
            config,
            read_ahead_directory: None,
            shared,
            inbox,
            player: None,
            volume: 1.0,
            muted: false,
            subtitle_size: SubtitleSize::Medium,
            subtitle_delay_ns: 0,
            frame_limit: None,
            reopened: false,
        }
    }

    pub(super) fn run(mut self, inbox: &Receiver<Command>) {
        'run: loop {
            let active = self.player.as_ref().is_some_and(Player::is_active);
            let next = if active {
                match inbox.recv_timeout(
                    self.player
                        .as_ref()
                        .expect("active player")
                        .poll_interval(self.config.position_interval),
                ) {
                    Ok(command) => Some(command),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match inbox.recv() {
                    Ok(command) => Some(command),
                    Err(_) => break,
                }
            };
            if let Some(command) = next
                && !self.handle(command)
            {
                break;
            }
            for _ in 0..VIDEO_COMMAND_CAPACITY {
                let Ok(command) = inbox.try_recv() else {
                    break;
                };
                if !self.handle(command) {
                    break 'run;
                }
            }
            // Settings that met a full queue are newer than anything queued
            // before them: apply them once the queue is seen empty, under the
            // lock their senders take, so nothing sent after can slip ahead.
            let waiting = {
                let mut slot = self.shared.lock();
                match inbox.try_recv() {
                    Ok(command) => {
                        drop(slot);
                        if !self.handle(command) {
                            break 'run;
                        }
                        Vec::new()
                    }
                    Err(_) => std::mem::take(&mut slot.settings),
                }
            };
            for setting in waiting {
                let _ = self.handle(setting);
            }
            if self.shared.lock().closed {
                break;
            }
            self.pump_bus();
            self.tick();
        }
        self.shared.cancel_seeks(None, SeekRejection::Closed);
        self.teardown();
        self.shared.close();
    }

    fn handle(&mut self, command: Command) -> bool {
        match command {
            Command::Shutdown => return false,
            Command::Bus => {}
            Command::Load(generation, source, options) => self.load(generation, source, options),
            Command::Unload => self.teardown(),
            Command::SetVolume(volume) => {
                self.volume = volume;
                if let Some(player) = &self.player {
                    player.playbin.set_property("volume", volume.powi(3));
                }
                self.shared.update(|status| status.volume = volume);
            }
            Command::SetMute(muted) => {
                self.muted = muted;
                if let Some(player) = &self.player {
                    player.playbin.set_property("mute", muted);
                }
                self.shared.update(|status| status.muted = muted);
            }
            Command::SubtitleSize(size) => {
                self.subtitle_size = size;
                if let Some(player) = &self.player {
                    player
                        .playbin
                        .set_property("subtitle-font-desc", size.font());
                }
                self.shared.update(|status| status.subtitle_size = size);
            }
            Command::SubtitleDelay(delay, late) => {
                let ns = i64::try_from(delay.as_nanos()).unwrap_or(i64::MAX);
                self.subtitle_delay_ns = if late { ns } else { -ns };
                if let Some(player) = &self.player {
                    player
                        .playbin
                        .set_property("text-offset", self.subtitle_delay_ns);
                }
                let ms = self.subtitle_delay_ns / 1_000_000;
                self.shared.update(|status| status.subtitle_delay_ms = ms);
            }
            Command::FrameLimit(limit) => {
                self.frame_limit = limit;
                if let Some(player) = &self.player {
                    player.scale_caps.set_property("caps", scale_caps(limit));
                }
            }
            Command::Play(generation) => self.with_player(generation, Player::play),
            Command::Pause(generation) => self.with_player(generation, Player::pause),
            Command::Seek(generation, position, mode) => {
                self.with_player(generation, |player, shared| {
                    player.seek(position, mode, shared);
                });
            }
            Command::TrackedSeek(request) => {
                if let Some(player) = self
                    .player
                    .as_mut()
                    .filter(|player| player.generation == request.generation)
                {
                    player.tracked_seek(request, &self.shared);
                } else {
                    self.shared.seek_outcome(SeekOutcome {
                        request,
                        target: request.requested,
                        actual: None,
                        result: Err(SeekRejection::NotReady),
                    });
                }
            }
            Command::Step(generation, forward) => {
                self.with_player(generation, |player, shared| player.step(forward, shared));
            }
            Command::SetRate(generation, rate) => {
                self.with_player(generation, |player, _| player.set_rate(rate));
            }
            Command::SelectAudio(generation, index) => {
                self.with_player(generation, |player, shared| {
                    player.select_audio(index, shared);
                });
            }
            Command::SelectSubtitle(generation, index) => {
                self.with_player(generation, |player, shared| {
                    player.select_subtitle(index, shared);
                });
            }
            Command::SubtitleFile(generation, path) => {
                if self
                    .player
                    .as_ref()
                    .is_some_and(|player| player.generation == generation)
                {
                    self.reopen_with_subtitle(path);
                }
            }
        }
        true
    }

    fn with_player(&mut self, generation: Generation, act: impl FnOnce(&mut Player, &Shared)) {
        if let Some(player) = self.player.as_mut()
            && player.generation == generation
        {
            act(player, &self.shared);
        }
    }

    fn load(&mut self, generation: Generation, source: MediaSource, options: LoadOptions) {
        self.teardown();
        self.reopened = false;
        self.volume = options.volume.clamp(0.0, 1.0);
        self.muted = options.muted;
        self.subtitle_size = options.subtitle_size;
        let size = self.subtitle_size;
        self.shared.update(|status| status.subtitle_size = size);
        let start = Start {
            at: options.start_at,
            target: if options.autoplay {
                Target::Playing
            } else {
                Target::Paused
            },
            subtitle_file: options.subtitle_file,
            rate: 1.0,
            audio_choice: None,
            text_choice: None,
        };
        match self.open(generation, &source, start.subtitle_file.as_deref()) {
            Ok(built) => self.player = Some(Player::new(generation, source, built, start)),
            Err(error) => self.fail(generation, error),
        }
    }

    pub(super) fn open(
        &self,
        generation: Generation,
        source: &MediaSource,
        subtitle_file: Option<&std::path::Path>,
    ) -> Result<Built, MediaError> {
        let cancel = Superseded {
            shared: &self.shared,
            generation,
        };
        let prepared = match source {
            MediaSource::Local(path) => PreparedSource::local(path.clone()),
            MediaSource::Remote(remote) => {
                let clock = MonotonicClock::start();
                let effects = GateEffects {
                    resolver: &SystemResolver,
                    probe: Some(&CurlRedirectProbe),
                    clock: &clock,
                    cancel: &cancel,
                };
                let approved = self
                    .config
                    .gate
                    .approve(remote, &effects)
                    .map_err(|refusal| refused(&format!("The stream was refused: {refusal}")))?;
                PreparedSource::approved_stream(approved)
            }
        };
        if cancel.is_cancelled() {
            return Err(refused(SUPERSEDED));
        }
        let read_ahead = if prepared.approved().is_some() {
            self.config
                .disk_read_ahead
                .map(|policy| {
                    let directory = self
                        .read_ahead_directory
                        .as_deref()
                        .ok_or_else(|| refused("no disk read-ahead directory was selected"))?;
                    super::read_ahead::Backing::reserve(directory, policy).map(Arc::new)
                })
                .transpose()?
        } else {
            None
        };
        let uri = if read_ahead.is_some() {
            let original = crate::media::gst_worker::source_to_uri(&prepared)?;
            if let Some(rest) = original.strip_prefix("http://") {
                format!("enzuihttp://{rest}")
            } else if let Some(rest) = original.strip_prefix("https://") {
                format!("enzuihttps://{rest}")
            } else {
                return Err(refused("disk read-ahead requires an HTTP source"));
            }
        } else {
            crate::media::gst_worker::pipeline_uri(&prepared)?
        };
        let playbin = gst::ElementFactory::make("playbin3")
            .property("uri", &uri)
            .build()
            .map_err(|_| missing_element("playbin3"))?;
        // Subtitles show by being selected, so the overlay stays enabled.
        set_flag(&playbin, "text", true);
        set_flag(&playbin, "deinterlace", true);
        let subtitle_uri = subtitle_file.map(file_uri).transpose()?;
        if let Some(uri) = &subtitle_uri {
            playbin.set_property("suburi", uri);
        }
        playbin.set_property("volume", self.volume.powi(3));
        playbin.set_property("mute", self.muted);
        playbin.set_property("subtitle-font-desc", self.subtitle_size.font());
        playbin.set_property("text-offset", self.subtitle_delay_ns);
        // Pitch preservation is optional when scaletempo is unavailable.
        if let Ok(tempo) = gst::ElementFactory::make("scaletempo").build() {
            playbin.set_property("audio-filter", &tempo);
        }

        let (video_sink, scale_caps) = video_sink(generation, &self.shared, self.frame_limit)?;
        playbin.set_property("video-sink", &video_sink);

        let mut proxy = None;
        let mut remote_configuration = None;
        let mut received = None;
        if let Some(approved) = prepared.approved() {
            let budget = approved.budget();
            set_flag(&playbin, "buffering", true);
            set_flag(&playbin, "download", false);
            playbin.set_property(
                "buffer-size",
                i32::try_from(budget.buffer_bytes).unwrap_or(i32::MAX),
            );
            playbin.set_property(
                "ring-buffer-max-size",
                if read_ahead.is_some() {
                    0
                } else {
                    budget.ring_buffer_property()
                },
            );
            self.config.buffering.apply(&playbin);
            let started = PinnedProxy::start(approved)?;
            let client = started.client_configuration();
            let limits = approved.limits();
            let approved_uri = approved.transport_url().to_owned();
            let slot = Arc::new(Mutex::new(None::<Result<(), MediaError>>));
            let written = Arc::clone(&slot);
            let payload = PayloadCounter::new();
            let counted = payload.clone();
            let disk = read_ahead.clone();
            playbin.connect("source-setup", false, move |values| {
                let Some(src) = values
                    .get(1)
                    .and_then(|value| value.get::<gst::Element>().ok())
                else {
                    if let Ok(mut slot) = written.lock() {
                        *slot = Some(Err(refused("the stream exposed no source to configure")));
                    }
                    return None;
                };
                // The local subtitle shares playbin's signal but not the approved network transport.
                if requested_subtitle_source(&src, subtitle_uri.as_deref()) {
                    return None;
                }
                let result = configure_remote_source_with_disk(
                    &src,
                    &client,
                    limits,
                    &approved_uri,
                    disk.is_some(),
                )
                .and_then(|()| {
                    disk.as_ref()
                        .map_or(Ok(()), |backing| backing.configure(&src))
                });
                if result.is_err() {
                    let _ = src.set_state(gst::State::Null);
                } else {
                    counted.observe(&src);
                }
                if let Ok(mut slot) = written.lock() {
                    *slot = Some(result);
                }
                None
            });
            proxy = Some(started);
            remote_configuration = Some(slot);
            received = Some(payload);
        }

        // Coalesce bus wakes; the worker drains the bus after draining commands.
        let bus_wake = Arc::new(AtomicBool::new(false));
        let bus = playbin
            .bus()
            .ok_or_else(|| backend("the pipeline has no bus"))?;
        let inbox = self.inbox.clone();
        let pending = Arc::clone(&bus_wake);
        bus.set_sync_handler(move |_, _| {
            if !pending.swap(true, Ordering::AcqRel) {
                let _ = inbox.try_send(Command::Bus);
            }
            gst::BusSyncReply::Pass
        });

        if playbin.set_state(gst::State::Paused).is_err() {
            bus.unset_sync_handler();
            let _ = playbin.set_state(gst::State::Null);
            return Err(MediaError {
                kind: MediaErrorKind::SourceNotFound,
                message: "The video could not be opened".into(),
            });
        }
        Ok(Built {
            playbin,
            scale_caps,
            bus_wake,
            proxy,
            remote_configuration,
            received,
            read_ahead,
        })
    }

    // The subtitle URI must be set before starting the replacement pipeline.
    fn reopen_with_subtitle(&mut self, path: Option<PathBuf>) {
        let Some(player) = self.player.take() else {
            return;
        };
        let generation = player.generation;
        let source = player.source.clone();
        let (audio_choice, text_choice) = player.choices();
        let start = Start {
            at: player.position().unwrap_or(player.start_at),
            target: player.target,
            subtitle_file: path.clone(),
            rate: player.rate,
            audio_choice,
            text_choice: if path.is_some() { None } else { text_choice },
        };
        self.shared
            .cancel_seeks(Some(generation), SeekRejection::SourceChanged);
        player.shut();
        self.shared.update(|status| {
            status.subtitle_file = path;
            status.current_subtitle = None;
        });
        match self.open(generation, &source, start.subtitle_file.as_deref()) {
            Ok(built) => self.player = Some(Player::new(generation, source, built, start)),
            Err(error) => self.fail(generation, error),
        }
    }

    /// Replaces a stalled pipeline with a fresh one for the same generation.
    fn reopen(&mut self) {
        let Some(player) = self.player.take() else {
            return;
        };
        self.reopened = true;
        let generation = player.generation;
        let source = player.source.clone();
        let start = player.restart();
        player.shut();
        match self.open(generation, &source, start.subtitle_file.as_deref()) {
            Ok(built) => self.player = Some(Player::new(generation, source, built, start)),
            Err(error) => self.fail(generation, error),
        }
    }

    fn teardown(&mut self) {
        if let Some(player) = self.player.take() {
            self.shared
                .cancel_seeks(Some(player.generation), SeekRejection::SourceChanged);
            player.shut();
        }
    }

    fn fail(&mut self, generation: Generation, error: MediaError) {
        if error.message == SUPERSEDED {
            return;
        }
        self.teardown();
        self.shared.update(|status| {
            if status.generation == generation {
                status.loading = false;
                status.buffering = None;
                status.read_ahead = None;
                status.playback = PlaybackState::Stopped;
                status.error = Some(error);
            }
        });
    }

    fn pump_bus(&mut self) {
        let Some(player) = self.player.as_mut() else {
            return;
        };
        player.bus_wake.store(false, Ordering::Release);
        let Some(bus) = player.playbin.bus() else {
            return;
        };
        while let Some(message) = bus.pop() {
            if let Some(Event::Failed(error)) = player.on_message(&message, &self.shared) {
                let generation = player.generation;
                self.fail(generation, error);
                return;
            }
        }
    }

    fn tick(&mut self) {
        let Some(player) = self.player.as_mut() else {
            return;
        };
        let local = matches!(player.source, MediaSource::Local(_));
        let timeout = if local {
            self.config.local_preroll_timeout
        } else {
            self.config.preroll_timeout
        };
        let stalled = player.phase == Phase::Opening && player.opened_at.elapsed() > timeout;
        // A pipeline that read the source's streams and then stopped is a
        // lifecycle stall, not a damaged file: open it once more.
        if stalled && local && player.streams_known() && !self.reopened {
            self.reopen();
            return;
        }
        let error = player
            .proxy
            .as_ref()
            .and_then(PinnedProxy::error)
            .or_else(|| {
                player
                    .remote_configuration
                    .as_ref()
                    .and_then(|slot| slot.lock().ok().and_then(|slot| slot.clone()))
                    .and_then(Result::err)
            })
            .or_else(|| {
                stalled.then(|| {
                    if player.streams_known() {
                        MediaError {
                            kind: MediaErrorKind::Backend,
                            message: "The video stopped while opening. Try opening it again."
                                .into(),
                        }
                    } else if local {
                        MediaError {
                            kind: MediaErrorKind::UnsupportedFormat,
                            message: "The file could not be read. It may be damaged or incomplete."
                                .into(),
                        }
                    } else {
                        MediaError {
                            kind: MediaErrorKind::SourceNotFound,
                            message: "The stream took too long to start".into(),
                        }
                    }
                })
            });
        if let Some(error) = error {
            let generation = player.generation;
            self.fail(generation, error);
            return;
        }
        let read_ahead = player
            .read_ahead
            .as_ref()
            .and_then(|backing| backing.status());
        self.shared.update(|status| {
            if status.generation == player.generation {
                status.read_ahead = read_ahead;
            }
        });
        player.check_seek_timeout(&self.shared);
        if player.phase == Phase::Ready
            && player.last_position.elapsed() >= self.config.position_interval
        {
            player.publish_position(&self.shared);
        }
    }
}

struct Superseded<'a> {
    shared: &'a Shared,
    generation: Generation,
}

impl CancelView for Superseded<'_> {
    fn is_cancelled(&self) -> bool {
        let slot = self.shared.lock();
        slot.closed || slot.status.generation != self.generation
    }
}

fn requested_subtitle_source(source: &gst::Element, requested_uri: Option<&str>) -> bool {
    let Some(requested_uri) = requested_uri else {
        return false;
    };
    source
        .factory()
        .is_some_and(|factory| factory.name() == "filesrc")
        && source
            .dynamic_cast_ref::<gst::URIHandler>()
            .and_then(gst::prelude::URIHandlerExt::uri)
            .as_deref()
            == Some(requested_uri)
}
