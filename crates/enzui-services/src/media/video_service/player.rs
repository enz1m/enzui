//! One open source: its `playbin3`, what the user asked of it, and how its
//! bus messages become status.

use super::pipeline::{Stream, classify, video_info};
use super::{SeekMode, SeekOutcome, SeekRejection, SeekRequest, Shared};
use crate::media::gst_worker::PayloadCounter;
use crate::media::pinned_proxy::PinnedProxy;
use crate::media::{Generation, MediaError, MediaSource, PlaybackState};
use gstreamer as gst;
use gstreamer::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub(super) type RemoteConfiguration = Arc<Mutex<Option<Result<(), MediaError>>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    Playing,
    Paused,
}

impl Target {
    fn playback(self) -> PlaybackState {
        match self {
            Target::Playing => PlaybackState::Playing,
            Target::Paused => PlaybackState::Paused,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Opening,
    Starting,
    Ready,
}

pub(super) enum Event {
    Failed(MediaError),
}

pub(super) struct Player {
    pub(super) generation: Generation,
    pub(super) source: MediaSource,
    pub(super) playbin: gst::Element,
    pub(super) scale_caps: gst::Element,
    pub(super) phase: Phase,
    pub(super) target: Target,
    pub(super) opened_at: Instant,
    pub(super) start_at: Duration,
    pub(super) rate: f64,
    seeking: bool,
    // Only the newest seek requested during an in-flight seek runs.
    queued_seek: Option<(Duration, SeekMode, Option<SeekRequest>)>,
    active_seek: Option<(SeekRequest, Duration)>,
    seek_started: Option<Instant>,
    queued_rate: Option<f64>,
    buffering: bool,
    pub(super) ended: bool,
    // A flushing preroll counts as the already displayed frame for stepping.
    flushed: bool,
    pub(super) last_position: Instant,
    frame_duration: Option<Duration>,
    pub(super) subtitle_file: Option<PathBuf>,
    pub(super) bus_wake: Arc<AtomicBool>,
    pub(super) proxy: Option<PinnedProxy>,
    pub(super) remote_configuration: Option<RemoteConfiguration>,
    received: Option<PayloadCounter>,
    pub(super) read_ahead: Option<Arc<super::read_ahead::Backing>>,
    // Stream tags can arrive after the collection message.
    collection: Option<gst::StreamCollection>,
    streams: Vec<Stream>,
    audio_choice: Option<String>,
    text_choice: Option<String>,
}

pub(super) struct Start {
    pub(super) at: Duration,
    pub(super) target: Target,
    pub(super) subtitle_file: Option<PathBuf>,
    pub(super) rate: f64,
    pub(super) audio_choice: Option<String>,
    pub(super) text_choice: Option<String>,
}

pub(super) struct Built {
    pub(super) playbin: gst::Element,
    pub(super) scale_caps: gst::Element,
    pub(super) bus_wake: Arc<AtomicBool>,
    pub(super) proxy: Option<PinnedProxy>,
    pub(super) remote_configuration: Option<RemoteConfiguration>,
    pub(super) received: Option<PayloadCounter>,
    pub(super) read_ahead: Option<Arc<super::read_ahead::Backing>>,
}

impl Player {
    pub(super) fn new(
        generation: Generation,
        source: MediaSource,
        built: Built,
        start: Start,
    ) -> Self {
        Self {
            generation,
            source,
            playbin: built.playbin,
            scale_caps: built.scale_caps,
            phase: Phase::Opening,
            target: start.target,
            opened_at: Instant::now(),
            start_at: start.at,
            rate: start.rate,
            seeking: false,
            queued_seek: None,
            active_seek: None,
            seek_started: None,
            queued_rate: None,
            buffering: false,
            ended: false,
            flushed: true,
            last_position: Instant::now(),
            frame_duration: None,
            subtitle_file: start.subtitle_file,
            bus_wake: built.bus_wake,
            proxy: built.proxy,
            remote_configuration: built.remote_configuration,
            received: built.received,
            read_ahead: built.read_ahead,
            collection: None,
            streams: Vec::new(),
            audio_choice: start.audio_choice,
            text_choice: start.text_choice,
        }
    }

    pub(super) fn choices(&self) -> (Option<String>, Option<String>) {
        (self.audio_choice.clone(), self.text_choice.clone())
    }

    /// Whether the source's streams were read: a stall after this point is
    /// not evidence that the file is damaged.
    pub(super) fn streams_known(&self) -> bool {
        self.collection.is_some()
    }

    /// How a replacement pipeline for this source starts, keeping what the
    /// user asked of this one.
    pub(super) fn restart(&self) -> Start {
        let (audio_choice, text_choice) = self.choices();
        Start {
            at: self.start_at,
            target: self.target,
            subtitle_file: self.subtitle_file.clone(),
            rate: self.rate,
            audio_choice,
            text_choice,
        }
    }

    pub(super) fn shut(self) {
        if let Some(bus) = self.playbin.bus() {
            bus.unset_sync_handler();
        }
        let _ = self.playbin.set_state(gst::State::Null);
    }

    pub(super) fn position(&self) -> Option<Duration> {
        self.playbin
            .query_position::<gst::ClockTime>()
            .map(|position| Duration::from_nanos(position.nseconds()))
    }

    fn duration(&self) -> Option<Duration> {
        self.playbin
            .query_duration::<gst::ClockTime>()
            .map(|duration| Duration::from_nanos(duration.nseconds()))
            .filter(|duration| !duration.is_zero())
    }

    pub(super) fn is_active(&self) -> bool {
        match self.phase {
            Phase::Opening | Phase::Starting => true,
            Phase::Ready => {
                self.read_ahead.is_some()
                    || self.seeking
                    || (self.target == Target::Playing && !self.ended)
            }
        }
    }

    pub(super) fn poll_interval(&self, configured: Duration) -> Duration {
        if self.seeking {
            configured.min(Duration::from_millis(200))
        } else {
            configured
        }
    }

    pub(super) fn publish_position(&mut self, shared: &Shared) {
        self.last_position = Instant::now();
        if self.seeking {
            return;
        }
        let position = self.position();
        let duration = self.duration();
        shared.update(|status| {
            if let Some(position) = position {
                status.position = position;
            }
            if duration.is_some() {
                status.duration = duration;
            }
        });
    }

    pub(super) fn play(&mut self, shared: &Shared) {
        self.target = Target::Playing;
        if self.phase != Phase::Ready {
            return;
        }
        if self.ended {
            self.seek(Duration::ZERO, SeekMode::Accurate, shared);
        }
        if !self.buffering {
            let _ = self.playbin.set_state(gst::State::Playing);
        }
    }

    pub(super) fn pause(&mut self, _shared: &Shared) {
        self.target = Target::Paused;
        if self.phase == Phase::Ready {
            let _ = self.playbin.set_state(gst::State::Paused);
        }
    }

    pub(super) fn seek(&mut self, position: Duration, mode: SeekMode, shared: &Shared) {
        self.seek_request(position, mode, None, shared);
    }

    pub(super) fn tracked_seek(&mut self, request: SeekRequest, shared: &Shared) {
        self.seek_request(request.requested, request.mode, Some(request), shared);
    }

    fn seek_request(
        &mut self,
        position: Duration,
        mode: SeekMode,
        request: Option<SeekRequest>,
        shared: &Shared,
    ) {
        if self.phase != Phase::Ready
            && let Some(request) = request
        {
            shared.seek_outcome(SeekOutcome {
                request,
                target: position,
                actual: self.position(),
                result: Err(SeekRejection::NotReady),
            });
            return;
        }
        match self.phase {
            Phase::Opening => {
                self.start_at = position;
                return;
            }
            Phase::Starting => return,
            Phase::Ready => {}
        }
        if self.seeking {
            if let Some((target, _, Some(previous))) =
                self.queued_seek.replace((position, mode, request))
            {
                shared.seek_outcome(SeekOutcome {
                    request: previous,
                    target,
                    actual: self.position(),
                    result: Err(SeekRejection::Superseded),
                });
            }
            return;
        }
        let position = match self.duration() {
            Some(duration) => position.min(duration),
            None => position,
        };
        let flags = gst::SeekFlags::FLUSH
            | match mode {
                SeekMode::Fast => gst::SeekFlags::KEY_UNIT | gst::SeekFlags::SNAP_NEAREST,
                SeekMode::Accurate => gst::SeekFlags::ACCURATE,
            };
        if !self.do_seek(position, flags) {
            if let Some(request) = request {
                shared.seek_outcome(SeekOutcome {
                    request,
                    target: position,
                    actual: self.position(),
                    result: Err(SeekRejection::BackendRefused),
                });
            }
            return;
        }
        self.seeking = true;
        self.seek_started = Some(Instant::now());
        self.active_seek = request.map(|request| (request, position));
        let resumed = std::mem::take(&mut self.ended).then_some(self.target.playback());
        shared.update(|status| {
            if request.is_none() {
                status.position = position;
            }
            if let Some(playback) = resumed {
                status.playback = playback;
            }
        });
        if resumed == Some(PlaybackState::Playing) {
            let _ = self.playbin.set_state(gst::State::Playing);
        }
    }

    pub(super) fn do_seek(&self, position: Duration, flags: gst::SeekFlags) -> bool {
        let Ok(nanos) = i64::try_from(position.as_nanos()) else {
            return false;
        };
        let position = gst::ClockTime::from_nseconds(nanos.unsigned_abs());
        self.playbin
            .seek(
                self.rate,
                flags,
                gst::SeekType::Set,
                position,
                gst::SeekType::None,
                gst::ClockTime::NONE,
            )
            .is_ok()
    }

    pub(super) fn step(&mut self, forward: bool, shared: &Shared) {
        if self.phase != Phase::Ready {
            return;
        }
        if self.target == Target::Playing {
            self.pause(shared);
        }
        if forward {
            // The first step after a flushing preroll counts the frame
            // already shown; step over it too.
            let amount = if std::mem::take(&mut self.flushed) {
                2
            } else {
                1
            };
            let step =
                gst::event::Step::new(gst::format::Buffers::from_u64(amount), 1.0, true, false);
            self.playbin.send_event(step);
        } else if let Some(frame) = self.frame_duration
            && let Some(position) = self.position()
        {
            self.seek(position.saturating_sub(frame), SeekMode::Accurate, shared);
        }
    }

    pub(super) fn set_rate(&mut self, rate: f64) {
        if self.phase != Phase::Ready {
            return;
        }
        if self.seeking {
            self.queued_rate = Some(rate);
            return;
        }
        if (rate - self.rate).abs() < f64::EPSILON {
            return;
        }
        let Some(position) = self.position() else {
            return;
        };
        let previous = std::mem::replace(&mut self.rate, rate);
        if self.do_seek(position, gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE) {
            self.seeking = true;
            self.seek_started = Some(Instant::now());
        } else {
            self.rate = previous;
        }
    }

    fn audio_streams(&self) -> impl Iterator<Item = &Stream> {
        self.streams
            .iter()
            .filter(|stream| stream.is(gst::StreamType::AUDIO))
    }

    fn embedded_text(&self) -> impl Iterator<Item = &Stream> {
        let origin = self.main_origin();
        self.streams.iter().filter(move |stream| {
            stream.is(gst::StreamType::TEXT)
                && origin.is_none_or(|origin| stream.origin() == origin)
        })
    }

    fn main_origin(&self) -> Option<&str> {
        self.streams
            .iter()
            .find(|stream| !stream.is(gst::StreamType::TEXT))
            .map(Stream::origin)
    }

    pub(super) fn select_audio(&mut self, index: i32, shared: &Shared) {
        let chosen = usize::try_from(index)
            .ok()
            .and_then(|index| self.audio_streams().nth(index))
            .map(|stream| stream.id.clone());
        if chosen.is_some() && chosen != self.audio_choice {
            self.audio_choice = chosen;
            self.apply_selection(shared);
        }
    }

    pub(super) fn select_subtitle(&mut self, index: Option<i32>, shared: &Shared) {
        let chosen = index
            .and_then(|index| usize::try_from(index).ok())
            .and_then(|index| self.embedded_text().nth(index))
            .map(|stream| stream.id.clone());
        if chosen != self.text_choice {
            self.text_choice = chosen;
            self.apply_selection(shared);
        }
    }

    // Re-preroll after selection so buffered data does not delay the change.
    fn apply_selection(&mut self, shared: &Shared) {
        let mut ids: Vec<&str> = Vec::new();
        if let Some(video) = self.streams.iter().find(|s| s.is(gst::StreamType::VIDEO)) {
            ids.push(&video.id);
        }
        if let Some(audio) = &self.audio_choice {
            ids.push(audio);
        }
        if let Some(text) = &self.text_choice {
            ids.push(text);
        }
        if ids.is_empty() {
            return;
        }
        self.playbin
            .send_event(gst::event::SelectStreams::new(&ids));
        if self.phase == Phase::Ready
            && let Some(position) = self.position()
        {
            self.seek(position, SeekMode::Accurate, shared);
        }
    }

    fn on_collection(&mut self, collection: gst::StreamCollection, shared: &Shared) {
        self.collection = Some(collection);
        self.publish_streams(shared);
        let keep = |choice: &Option<String>, streams: &[Stream]| {
            choice
                .as_ref()
                .filter(|id| streams.iter().any(|stream| &stream.id == *id))
                .cloned()
        };
        self.audio_choice = keep(&self.audio_choice, &self.streams)
            .or_else(|| self.audio_streams().next().map(|stream| stream.id.clone()));
        self.text_choice = if self.subtitle_file.is_some() {
            // The file's stream is the one text stream from another origin.
            let origin = self.main_origin().map(str::to_owned);
            self.streams
                .iter()
                .find(|stream| {
                    stream.is(gst::StreamType::TEXT)
                        && origin
                            .as_deref()
                            .is_some_and(|origin| stream.origin() != origin)
                })
                .map(|stream| stream.id.clone())
        } else {
            keep(&self.text_choice, &self.streams)
        };
        self.apply_selection(shared);
    }

    // Republish late tags without resending stream selection.
    fn publish_streams(&mut self, shared: &Shared) {
        let Some(collection) = &self.collection else {
            return;
        };
        self.streams = collection.iter().filter_map(|s| Stream::of(&s)).collect();
        let audio: Vec<_> = self
            .audio_streams()
            .enumerate()
            .map(|(index, stream)| stream.track(index))
            .collect();
        let text: Vec<_> = if self.subtitle_file.is_some() {
            Vec::new()
        } else {
            self.embedded_text()
                .enumerate()
                .map(|(index, stream)| stream.track(index))
                .collect()
        };
        let video = self.streams.iter().find(|s| s.is(gst::StreamType::VIDEO));
        let has_video = video.is_some();
        let codec = video.and_then(|video| {
            video.codec.clone().or_else(|| {
                video
                    .caps
                    .as_ref()
                    .and_then(|caps| caps.structure(0))
                    .map(|structure| structure.name().to_string())
            })
        });
        shared.update(|status| {
            status.has_audio = !audio.is_empty();
            status.has_video = has_video;
            status.audio_tracks = audio;
            status.subtitle_tracks = text;
            status.video_codec = codec;
        });
    }

    fn on_selected(&mut self, selected: &[gst::Stream], shared: &Shared) {
        let ids: Vec<String> = selected
            .iter()
            .filter_map(|stream| stream.stream_id().map(|id| id.to_string()))
            .collect();
        let current_audio = self
            .audio_streams()
            .position(|stream| ids.contains(&stream.id))
            .and_then(|index| i32::try_from(index).ok());
        let current_subtitle = self
            .embedded_text()
            .position(|stream| ids.contains(&stream.id))
            .filter(|_| self.subtitle_file.is_none())
            .and_then(|index| i32::try_from(index).ok());
        shared.update(|status| {
            status.current_audio = current_audio;
            status.current_subtitle = current_subtitle;
        });
    }

    fn publish_video_info(&mut self, shared: &Shared) {
        let video = self
            .streams
            .iter()
            .find(|stream| stream.is(gst::StreamType::VIDEO));
        let quarter_turn = video.is_some_and(|stream| stream.quarter_turn);
        let info = video
            .and_then(|stream| stream.caps.as_ref())
            .and_then(video_info);
        if let Some((size, rate)) = info {
            self.frame_duration = rate
                .filter(|rate| *rate > 0.0)
                .map(|rate| Duration::from_secs_f64(1.0 / rate));
            let size = if quarter_turn { (size.1, size.0) } else { size };
            shared.update(|status| {
                status.natural_size = Some(size);
                status.frame_rate = rate;
            });
        }
    }

    pub(super) fn on_message(&mut self, message: &gst::Message, shared: &Shared) -> Option<Event> {
        use gst::MessageView;
        let from_playbin = message.src() == Some(self.playbin.upcast_ref::<gst::Object>());
        match message.view() {
            MessageView::Error(error) => {
                let received = self.received.as_ref().and_then(PayloadCounter::received);
                return Some(Event::Failed(classify(error.error(), shared, received)));
            }
            MessageView::Eos(_) => {
                self.ended = true;
                let duration = self.duration();
                shared.update(|status| {
                    status.playback = PlaybackState::EndOfStream;
                    if let Some(duration) = duration {
                        status.position = duration;
                    }
                });
            }
            MessageView::AsyncDone(_) if from_playbin => self.on_async_done(shared),
            MessageView::StateChanged(change) if from_playbin => {
                let playback = match change.current() {
                    gst::State::Playing => {
                        self.flushed = false;
                        Some(PlaybackState::Playing)
                    }
                    // A flushing seek and a buffering pause both pass through
                    // PAUSED on the way back to PLAYING; while the user wants
                    // it playing, it still reads as playing.
                    gst::State::Paused if !self.ended && self.phase == Phase::Ready => Some(
                        if (self.buffering || self.seeking) && self.target == Target::Playing {
                            PlaybackState::Playing
                        } else {
                            PlaybackState::Paused
                        },
                    ),
                    _ => None,
                };
                if let Some(playback) = playback {
                    shared.update(|status| status.playback = playback);
                }
            }
            MessageView::Buffering(buffering) => {
                let percent = buffering.percent().clamp(0, 100);
                self.buffering = percent < 100;
                if self.phase == Phase::Ready && self.target == Target::Playing {
                    let state = if self.buffering {
                        gst::State::Paused
                    } else {
                        gst::State::Playing
                    };
                    let _ = self.playbin.set_state(state);
                }
                let shown = u8::try_from(percent).ok().filter(|_| self.buffering);
                shared.update(|status| status.buffering = shown);
            }
            MessageView::Tag(tag) => {
                self.publish_streams(shared);
                // The orientation lives in the stream's tags, which can
                // arrive after its caps did.
                self.publish_video_info(shared);
                // Only the container's own title names the video; a
                // track's title ("Commentary") is that track's name.
                let tags = tag.tags();
                let title = (tags.scope() == gst::TagScope::Global)
                    .then(|| tags.get::<gst::tags::Title>())
                    .flatten()
                    .map(|title| title.get().to_owned());
                if let Some(title) = title {
                    shared.update(|status| status.title = Some(title));
                }
            }
            MessageView::DurationChanged(_) => {
                let duration = self.duration();
                shared.update(|status| status.duration = duration);
            }
            MessageView::StreamCollection(collection) => {
                self.on_collection(collection.stream_collection(), shared);
            }
            MessageView::StreamsSelected(selected) => self.on_selected(&selected.streams(), shared),
            MessageView::StepDone(_) => self.publish_position(shared),
            MessageView::Element(element) => {
                if let Some(structure) = element.structure()
                    && structure.name() == "missing-plugin"
                {
                    let name = structure
                        .get::<String>("name")
                        .ok()
                        .or_else(|| structure.get::<String>("detail").ok())
                        .unwrap_or_else(|| "a decoder".into());
                    shared.update(|status| {
                        if !status.missing_decoders.contains(&name) {
                            status.missing_decoders.push(name);
                        }
                    });
                }
            }
            _ => {}
        }
        None
    }

    fn on_async_done(&mut self, shared: &Shared) {
        self.publish_video_info(shared);
        match self.phase {
            Phase::Opening => {
                let seekable = self.seekable();
                let duration = self.duration();
                if seekable
                    && !self.start_at.is_zero()
                    && self.do_seek(
                        self.start_at,
                        gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                    )
                {
                    self.phase = Phase::Starting;
                    shared.update(|status| {
                        status.seekable = seekable;
                        status.duration = duration;
                    });
                    return;
                }
                self.become_ready(seekable, duration, shared);
            }
            Phase::Starting => {
                let seekable = self.seekable();
                let duration = self.duration();
                self.become_ready(seekable, duration, shared);
            }
            Phase::Ready => {
                self.finish_seek(None, shared);
            }
        }
    }

    pub(super) fn check_seek_timeout(&mut self, shared: &Shared) {
        if self
            .seek_started
            .is_some_and(|started| started.elapsed() >= Duration::from_secs(2))
        {
            self.finish_seek(Some(SeekRejection::TimedOut), shared);
        }
    }

    fn finish_seek(&mut self, rejection: Option<SeekRejection>, shared: &Shared) {
        self.seeking = false;
        self.seek_started = None;
        self.flushed = true;
        let actual = self.position();
        let rate = self.rate;
        self.publish_position(shared);
        shared.update(|status| status.rate = rate);
        if let Some((request, target)) = self.active_seek.take() {
            let tolerance = self
                .frame_duration
                .unwrap_or_default()
                .max(Duration::from_millis(100));
            let result = match (rejection, actual) {
                (Some(reason), _) => Err(reason),
                (None, None) => Err(SeekRejection::PositionUnavailable),
                (None, Some(actual))
                    if request.mode == SeekMode::Accurate
                        && actual.abs_diff(target) > tolerance =>
                {
                    Err(SeekRejection::PositionMismatch)
                }
                _ => Ok(()),
            };
            shared.seek_outcome(SeekOutcome {
                request,
                target,
                actual,
                result,
            });
        }
        if let Some((position, mode, request)) = self.queued_seek.take() {
            self.seek_request(position, mode, request, shared);
        }
        if !self.seeking
            && let Some(rate) = self.queued_rate.take()
        {
            self.set_rate(rate);
        }
    }

    fn become_ready(&mut self, seekable: bool, duration: Option<Duration>, shared: &Shared) {
        self.phase = Phase::Ready;
        let position = self.position().unwrap_or_default();
        let playback = self.target.playback();
        let subtitle_file = self.subtitle_file.clone();
        let rate = self.rate;
        shared.update(|status| {
            status.loading = false;
            status.seekable = seekable;
            status.duration = duration;
            status.position = position;
            status.subtitle_file = subtitle_file;
            status.rate = rate;
            status.playback = playback;
        });
        if self.target == Target::Playing && !self.buffering {
            let _ = self.playbin.set_state(gst::State::Playing);
        }
    }

    fn seekable(&self) -> bool {
        let mut query = gst::query::Seeking::new(gst::Format::Time);
        self.playbin.query(&mut query) && query.result().0
    }
}

#[cfg(test)]
mod seek_tests {
    use super::*;

    fn player() -> Player {
        gst::init().unwrap();
        let playbin = gst::ElementFactory::make("identity").build().unwrap();
        let scale_caps = gst::ElementFactory::make("capsfilter").build().unwrap();
        let mut player = Player::new(
            Generation::FIRST,
            MediaSource::local("fixture.flv"),
            Built {
                playbin,
                scale_caps,
                bus_wake: Arc::new(AtomicBool::new(false)),
                proxy: None,
                remote_configuration: None,
                received: None,
                read_ahead: None,
            },
            Start {
                at: Duration::ZERO,
                target: Target::Paused,
                subtitle_file: None,
                rate: 1.0,
                audio_choice: None,
                text_choice: None,
            },
        );
        player.phase = Phase::Ready;
        player
    }

    fn request(shared: &Shared, at: u64) -> SeekRequest {
        shared
            .lock()
            .seeks
            .reserve(
                Generation::FIRST,
                Duration::from_secs(at),
                SeekMode::Accurate,
            )
            .unwrap()
    }

    #[test]
    fn a_tracked_opening_seek_is_explicitly_rejected_without_changing_start_position() {
        let shared = Shared::default();
        let mut player = player();
        player.phase = Phase::Opening;
        let request = request(&shared, 2);
        player.tracked_seek(request, &shared);
        let outcomes = shared.lock().seeks.take();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].request, request);
        assert_eq!(outcomes[0].result, Err(SeekRejection::NotReady));
        assert_eq!(player.start_at, Duration::ZERO);
        player.shut();
    }
}
