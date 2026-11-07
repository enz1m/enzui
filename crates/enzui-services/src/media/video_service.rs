//! Video playback, track selection, and RGBA frames from a worker-owned `playbin3`.
//!
//! Call [`VideoService::poll`] after subscription wakes to take changed status
//! and the newest frame. Wakes and frames coalesce; the shared slot retains one
//! frame, while decoding and pipeline callbacks can hold additional buffers.
//!
//! Use [`VideoService::request_seek`] and [`VideoService::take_seek_outcomes`]
//! for bounded, generation-tagged seek results, including backend positions.
//!
//! Commands use a bounded queue. When full, legacy seek and frame-step commands
//! are dropped; other commands wait for capacity and can block the caller. The
//! worker polls while opening, playing, seeking, or observing disk read-ahead and otherwise waits for
//! commands or bus wakes. Configuration timeouts cover preroll after pipeline creation,
//! not remote approval, pipeline construction, or teardown.
//!
//! Remote sources pass [`MediaGate`] approval before pipeline construction;
//! a pinned proxy supplies the approved HTTP destination. Optional
//! [`DiskReadAhead`] reserves an anonymous unread-byte FIFO; [`VideoBuffering`]
//! configures network buffering duration and watermarks. Consumed bytes are discarded.
//!
//! ```no_run
//! use enzui_services::media::video_service::{DiskReadAhead, VideoService, VideoServiceConfig};
//! let video = VideoService::new(VideoServiceConfig {
//!     disk_read_ahead: Some(DiskReadAhead {
//!         capacity: 64 * 1024 * 1024,
//!         min_free_bytes: 40 * 1024 * 1024 * 1024,
//!     }),
//!     ..VideoServiceConfig::default()
//! })?;
//! # Ok::<(), enzui_services::media::MediaError>(())
//! ```

mod buffering;
mod read_ahead;
pub use buffering::VideoBuffering;
pub use read_ahead::{DiskReadAhead, ReadAheadStatus};
mod pipeline;
mod player;
mod seek;
mod status;
pub use seek::{SeekAdmissionError, SeekOutcome, SeekRejection, SeekRequest, VIDEO_SEEK_CAPACITY};
mod worker;

#[cfg(test)]
mod tests;

pub use status::{SubtitleSize, Track, VideoStatus, language_name};

use crate::media::remote_gate::MediaGate;
use crate::media::rgba::Rgba8Frame;
use crate::media::{Generation, MediaError, MediaErrorKind, MediaSource};
use std::path::PathBuf;
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// Commands buffered between the UI and the worker. Generous: a held arrow
/// key queues one seek per repeat, and the worker coalesces seeks.
pub const VIDEO_COMMAND_CAPACITY: usize = 64;

/// Tuning for a [`VideoService`].
#[derive(Debug, Clone, Copy)]
pub struct VideoServiceConfig {
    /// Worker polling interval while opening or playing; defaults to 200 ms.
    /// Must be nonzero to avoid a busy polling loop.
    pub position_interval: Duration,
    /// Remote preroll timeout after pipeline creation; defaults to 15 s.
    /// Checked at worker ticks, including a start-position seek.
    pub preroll_timeout: Duration,
    /// Local preroll timeout after pipeline creation; defaults to 5 s.
    /// Expiry reports failure without diagnosing the file's condition.
    pub local_preroll_timeout: Duration,
    /// Destination policy for remote sources.
    pub gate: MediaGate,
    /// Optional anonymous disk FIFO for remote sources; defaults to disabled.
    pub disk_read_ahead: Option<DiskReadAhead>,
    /// Network buffering duration and thresholds.
    pub buffering: VideoBuffering,
}

impl Default for VideoServiceConfig {
    fn default() -> Self {
        Self {
            position_interval: Duration::from_millis(200),
            preroll_timeout: Duration::from_secs(15),
            local_preroll_timeout: Duration::from_secs(5),
            gate: MediaGate::DEFAULT,
            disk_read_ahead: None,
            buffering: VideoBuffering::default(),
        }
    }
}

/// How a newly loaded source starts.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadOptions {
    /// Requested initial position; defaults to zero. Ignored if seeking is
    /// unavailable or the initial seek fails.
    pub start_at: Duration,
    /// Play when ready; defaults to true. False leaves playback paused at the
    /// requested start position when seeking succeeds.
    pub autoplay: bool,
    /// A subtitle file shown over the video, in place of embedded tracks.
    pub subtitle_file: Option<PathBuf>,
    /// Initial slider volume, clamped to `0.0..=1.0`; defaults to 1.0.
    /// Supply a finite value: loading does not sanitize NaN.
    pub volume: f64,
    /// Whether audio starts muted.
    pub muted: bool,
    /// Subtitle text size.
    pub subtitle_size: SubtitleSize,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            start_at: Duration::ZERO,
            autoplay: true,
            subtitle_file: None,
            volume: 1.0,
            muted: false,
            subtitle_size: SubtitleSize::Medium,
        }
    }
}

/// How precisely a seek lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekMode {
    /// Request the nearest keyframe for scrubbing; completion is asynchronous.
    Fast,
    /// Request an accurate seek; achievable precision depends on the source.
    Accurate,
}

/// What changed since the last [`VideoService::poll`].
#[derive(Debug, Default)]
pub struct VideoUpdate {
    /// The new status, when it changed.
    pub status: Option<VideoStatus>,
    /// The newest decoded frame, when one arrived.
    pub frame: Option<Rgba8Frame>,
}

/// A coalesced wake from the worker; poll the service when it arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VideoWake(pub u64);

/// Which replaceable setting a command carries: only its newest value matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingKind {
    Volume,
    Mute,
    SubtitleSize,
    SubtitleDelay,
    FrameLimit,
}

#[derive(Debug)]
pub(crate) enum Command {
    Load(Generation, MediaSource, LoadOptions),
    Unload,
    Play(Generation),
    Pause(Generation),
    Seek(Generation, Duration, SeekMode),
    TrackedSeek(SeekRequest),
    Step(Generation, bool),
    SetVolume(f64),
    SetMute(bool),
    SetRate(Generation, f64),
    SelectAudio(Generation, i32),
    SelectSubtitle(Generation, Option<i32>),
    SubtitleFile(Generation, Option<PathBuf>),
    SubtitleSize(SubtitleSize),
    SubtitleDelay(Duration, bool),
    FrameLimit(Option<(u32, u32)>),
    Bus,
    Shutdown,
}

#[derive(Debug, Default)]
pub(crate) struct Slot {
    pub(crate) status: VideoStatus,
    pub(crate) status_changed: bool,
    pub(crate) frame: Option<Rgba8Frame>,
    seeks: seek::Receipts,
    wakes: u64,
    observed: u64,
    waker: Option<std::task::Waker>,
    pub(crate) closed: bool,
    /// Settings that met a full queue, newest per kind. While one waits, later
    /// settings of its kind wait here too, so none overtakes another; the
    /// worker applies them once it has drained everything sent before.
    pub(crate) settings: Vec<Command>,
}

#[derive(Debug, Default)]
pub(crate) struct Shared(Mutex<Slot>);

impl Shared {
    pub(crate) fn lock(&self) -> MutexGuard<'_, Slot> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn update(&self, change: impl FnOnce(&mut VideoStatus)) {
        let waker = {
            let mut slot = self.lock();
            let before = slot.status.clone();
            change(&mut slot.status);
            if slot.status == before {
                return;
            }
            slot.status_changed = true;
            slot.wake()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    pub(crate) fn offer_frame(&self, frame: Rgba8Frame) {
        let waker = {
            let mut slot = self.lock();
            if frame.generation() != slot.status.generation {
                return;
            }
            slot.frame = Some(frame);
            slot.wake()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    pub(crate) fn close(&self) {
        self.cancel_seeks(None, SeekRejection::Closed);
        let waker = {
            let mut slot = self.lock();
            slot.closed = true;
            slot.wake()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl Slot {
    fn wake(&mut self) -> Option<std::task::Waker> {
        self.wakes = self.wakes.wrapping_add(1);
        self.waker.take()
    }
}

/// A video player: one GStreamer pipeline on a worker thread.
///
/// Dropping requests shutdown without joining the worker. Pipeline teardown
/// runs on that thread once it observes shutdown; in-flight work can delay it.
#[derive(Debug)]
pub struct VideoService {
    commands: SyncSender<Command>,
    shared: Arc<Shared>,
    generation: Generation,
}

impl VideoService {
    /// Initialize GStreamer on the caller and start the worker thread.
    ///
    /// # Errors
    ///
    /// Returns invalid configuration, missing absolute cache location, GStreamer
    /// initialization or thread-start errors. Storage failures arrive through
    /// playback status when an approved remote source is loaded.
    pub fn new(config: VideoServiceConfig) -> Result<Self, MediaError> {
        let directory = if config.disk_read_ahead.is_some() {
            Some(
                enzui_core::persist::xdg_path(
                    enzui_core::persist::PersistenceKind::Cache,
                    "enzui",
                    "video-read-ahead",
                )
                .map_err(|_| pipeline::refused("the read-ahead cache location is invalid"))?
                .ok_or_else(|| {
                    pipeline::refused("no absolute read-ahead cache location is available")
                })?,
            )
        } else {
            None
        };
        Self::new_inner(config, directory)
    }

    /// Start with a caller-selected private directory for anonymous read-ahead.
    ///
    /// Directory creation and full reservation happen on the worker only for
    /// approved remote loads. Local files do not allocate disk read-ahead.
    ///
    /// # Errors
    /// Returns configuration, GStreamer initialization or thread-start errors.
    /// Filesystem admission errors are reported through playback status on load.
    pub fn new_with_read_ahead_directory(
        config: VideoServiceConfig,
        directory: PathBuf,
    ) -> Result<Self, MediaError> {
        if !directory.is_absolute() {
            return Err(pipeline::refused(
                "the read-ahead directory must be absolute",
            ));
        }
        Self::new_inner(config, Some(directory))
    }

    fn new_inner(
        config: VideoServiceConfig,
        directory: Option<PathBuf>,
    ) -> Result<Self, MediaError> {
        config.buffering.validate()?;
        if let Some(policy) = config.disk_read_ahead {
            policy.validate()?;
        }
        if config.position_interval.is_zero() {
            return Err(pipeline::refused(
                "the video polling interval must be nonzero",
            ));
        }
        crate::media::gst_worker::ensure_initialised()?;
        let (commands, inbox) = std::sync::mpsc::sync_channel(VIDEO_COMMAND_CAPACITY);
        let shared = Arc::new(Shared::default());
        let mut worker = worker::Worker::new(config, Arc::clone(&shared), commands.clone());
        worker.read_ahead_directory = directory;
        std::thread::Builder::new()
            .name("enzui-video".into())
            .spawn(move || worker.run(&inbox))
            .map_err(|_| MediaError {
                kind: MediaErrorKind::Backend,
                message: "the video worker thread could not start".into(),
            })?;
        Ok(Self {
            commands,
            shared,
            generation: Generation::FIRST,
        })
    }

    /// The source generation used to tag playback commands and decoded frames.
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// Queue a replacement source and immediately reset status and the frame
    /// slot for a new generation. Old frames are rejected, but status updates
    /// from an in-flight old pipeline can arrive before replacement finishes.
    pub fn load(&mut self, source: MediaSource, options: LoadOptions) -> Generation {
        self.shared.cancel_seeks(None, SeekRejection::SourceChanged);
        self.generation = self.generation.next();
        let generation = self.generation;
        let display = source.display_name();
        let remote = matches!(source, MediaSource::Remote(_));
        let volume = options.volume;
        let muted = options.muted;
        self.shared.update(|status| {
            *status = VideoStatus::loading(generation, display, remote);
            status.volume = volume.clamp(0.0, 1.0);
            status.muted = muted;
        });
        self.shared.lock().frame = None;
        self.send(Command::Load(generation, source, options));
        generation
    }

    /// Immediately clear status and the frame slot, then queue pipeline release.
    /// In-flight status updates can arrive before the worker handles unloading.
    pub fn unload(&mut self) {
        self.shared.cancel_seeks(None, SeekRejection::SourceChanged);
        self.generation = self.generation.next();
        let generation = self.generation;
        self.shared
            .update(|status| *status = VideoStatus::empty(generation));
        self.shared.lock().frame = None;
        self.send(Command::Unload);
    }

    /// Start or resume playback; at the end, start again from the beginning.
    pub fn play(&self) {
        self.send(Command::Play(self.generation));
    }

    /// Pause on the current frame.
    pub fn pause(&self) {
        self.send(Command::Pause(self.generation));
    }

    /// Request a seek, clamped to the known duration once ready. While opening,
    /// replaces the initial position; ignored during the initial-position seek.
    /// Seeks in flight coalesce to the newest request. Status initially shows the
    /// accepted target; use [`Self::request_seek`] for confirmed outcomes.
    pub fn seek(&self, position: Duration, mode: SeekMode) {
        self.send(Command::Seek(self.generation, position, mode));
    }

    /// Admit a nonblocking, tracked seek on the current source generation.
    /// Every admitted request reserves one outcome until [`Self::take_seek_outcomes`].
    /// Opening sources reject asynchronously; queued seeks coalesce to the newest.
    /// Accurate settlement allows one frame or 100 ms, whichever is larger;
    /// fast settlement reports the actual keyframe without an accuracy guarantee.
    /// In-flight seeks expire after two seconds, checked at worker ticks no more
    /// than 200 ms apart; pipeline calls and teardown can delay those ticks.
    ///
    /// # Errors
    /// Returns [`SeekAdmissionError::Full`] when command or outcome capacity is
    /// exhausted, or [`SeekAdmissionError::Closed`] after worker closure.
    pub fn request_seek(
        &self,
        position: Duration,
        mode: SeekMode,
    ) -> Result<SeekRequest, SeekAdmissionError> {
        let mut slot = self.shared.lock();
        if slot.closed {
            return Err(SeekAdmissionError::Closed);
        }
        let request = slot.seeks.reserve(self.generation, position, mode)?;
        match self.commands.try_send(Command::TrackedSeek(request)) {
            Ok(()) => Ok(request),
            Err(error) => {
                slot.seeks.release(request);
                Err(match error {
                    TrySendError::Full(_) => SeekAdmissionError::Full,
                    TrySendError::Disconnected(_) => SeekAdmissionError::Closed,
                })
            }
        }
    }

    /// Take terminal seek outcomes and release their admission capacity.
    /// Subscription wakes include these outcomes; [`Self::poll`] does not drain them.
    pub fn take_seek_outcomes(&self) -> Vec<SeekOutcome> {
        self.shared.lock().seeks.take()
    }

    /// Request the next or previous frame once ready, pausing first. Backward
    /// stepping requires a known frame rate and seeks back one frame duration.
    pub fn step_frame(&self, forward: bool) {
        self.send(Command::Step(self.generation, forward));
    }

    /// Clamp slider volume to `0.0..=1.0`, treating NaN as zero. The pipeline
    /// receives its cube; perceived loudness depends on the source and output.
    pub fn set_volume(&self, volume: f64) {
        let volume = if volume.is_nan() {
            0.0
        } else {
            volume.clamp(0.0, 1.0)
        };
        self.send(Command::SetVolume(volume));
    }

    /// Mute or unmute without changing the volume.
    pub fn set_mute(&self, muted: bool) {
        self.send(Command::SetMute(muted));
    }

    /// Request a rate change once ready. Nonfinite values and rates outside
    /// [`MIN_RATE`]`..=`[`MAX_RATE`] are ignored. Pitch preservation requires
    /// the optional GStreamer `scaletempo` element.
    pub fn set_rate(&self, rate: f64) {
        if rate.is_finite() && (MIN_RATE..=MAX_RATE).contains(&rate) {
            self.send(Command::SetRate(self.generation, rate));
        }
    }

    /// Play the audio track at `index` in [`VideoStatus::audio_tracks`].
    pub fn select_audio(&self, index: i32) {
        self.send(Command::SelectAudio(self.generation, index));
    }

    /// Show the embedded subtitle track at `index`, or none.
    pub fn select_subtitle(&self, index: Option<i32>) {
        self.send(Command::SelectSubtitle(self.generation, index));
    }

    /// Show a subtitle file instead of the embedded tracks, or drop it. The
    /// source reopens at the current position to take it.
    pub fn set_subtitle_file(&self, path: Option<PathBuf>) {
        self.send(Command::SubtitleFile(self.generation, path));
    }

    /// Change the subtitle text size.
    pub fn set_subtitle_size(&self, size: SubtitleSize) {
        self.send(Command::SubtitleSize(size));
    }

    /// Show subtitles `delay` later (`late`) or earlier than their timing.
    pub fn set_subtitle_delay(&self, delay: Duration, late: bool) {
        self.send(Command::SubtitleDelay(delay, late));
    }

    /// Limit output RGBA frames to physical pixel bounds while preserving
    /// aspect ratio. None or a zero dimension removes this limit; the global
    /// frame edge cap still applies. Scaling occurs after decoding.
    pub fn set_frame_limit(&self, limit: Option<(u32, u32)>) {
        let limit = limit.filter(|(w, h)| *w > 0 && *h > 0);
        self.send(Command::FrameLimit(limit));
    }

    /// The current status.
    #[must_use]
    pub fn status(&self) -> VideoStatus {
        self.shared.lock().status.clone()
    }

    /// Take what changed since the last poll.
    pub fn poll(&self) -> VideoUpdate {
        let mut slot = self.shared.lock();
        slot.observed = slot.wakes;
        let status = std::mem::take(&mut slot.status_changed).then(|| slot.status.clone());
        VideoUpdate {
            status,
            frame: slot.frame.take(),
        }
    }

    /// Coalesced wakes for status, frames and tracked seek outcomes. Take outcomes
    /// separately from polling; polling also acknowledges pending wakes.
    pub fn subscription(&self) -> iced_futures::Subscription<VideoWake> {
        iced_futures::Subscription::run_with(WakeKey(Arc::clone(&self.shared)), |key| {
            let shared = Arc::clone(&key.0);
            iced_futures::futures::stream::unfold(shared, async |shared| {
                let wake = WakeWait(Arc::clone(&shared)).await?;
                Some((wake, shared))
            })
        })
    }

    fn send(&self, command: Command) {
        if let Some(kind) = command.setting_kind() {
            // Replaceable settings never block: a full queue keeps the newest.
            let mut slot = self.shared.lock();
            if let Some(at) = slot
                .settings
                .iter()
                .position(|waiting| waiting.setting_kind() == Some(kind))
            {
                slot.settings[at] = command;
                return;
            }
            if let Err(TrySendError::Full(command)) = self.commands.try_send(command) {
                slot.settings.push(command);
            }
            return;
        }
        match self.commands.try_send(command) {
            Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            Err(TrySendError::Full(command)) => {
                // Preserve control commands under backpressure; scrubbing and stepping may drop.
                if !matches!(command, Command::Seek(..) | Command::Step(..)) {
                    let _ = self.commands.send(command);
                }
            }
        }
    }
}

impl Drop for VideoService {
    fn drop(&mut self) {
        // Approval observes closure; a full queue is drained before the worker checks closure.
        self.shared.lock().closed = true;
        let _ = self.commands.try_send(Command::Shutdown);
    }
}

impl Command {
    /// The setting this command replaces, for generation-independent
    /// settings whose newest value is all that matters.
    pub(crate) fn setting_kind(&self) -> Option<SettingKind> {
        match self {
            Self::SetVolume(_) => Some(SettingKind::Volume),
            Self::SetMute(_) => Some(SettingKind::Mute),
            Self::SubtitleSize(_) => Some(SettingKind::SubtitleSize),
            Self::SubtitleDelay(..) => Some(SettingKind::SubtitleDelay),
            Self::FrameLimit(_) => Some(SettingKind::FrameLimit),
            _ => None,
        }
    }
}

/// The slowest playback rate [`VideoService::set_rate`] accepts.
pub const MIN_RATE: f64 = 0.25;
/// The fastest playback rate [`VideoService::set_rate`] accepts.
pub const MAX_RATE: f64 = 4.0;

#[derive(Clone)]
struct WakeKey(Arc<Shared>);

impl std::hash::Hash for WakeKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

struct WakeWait(Arc<Shared>);

impl std::future::Future for WakeWait {
    type Output = Option<VideoWake>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let mut slot = self.0.lock();
        if slot.wakes != slot.observed {
            slot.observed = slot.wakes;
            return std::task::Poll::Ready(Some(VideoWake(slot.wakes)));
        }
        if slot.closed {
            return std::task::Poll::Ready(None);
        }
        if !slot.waker.as_ref().is_some_and(|w| w.will_wake(cx.waker())) {
            slot.waker = Some(cx.waker().clone());
        }
        std::task::Poll::Pending
    }
}
