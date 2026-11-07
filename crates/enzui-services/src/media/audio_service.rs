//! Audio-only GStreamer playback with generation-tagged commands and events.
//!
//! Loads and commands use nonblocking bounded admission. Remote approval,
//! pipeline construction, and preroll run on the worker; an admitted new load
//! cancels the previous generation's preparation. Old-generation commands are
//! ignored, and [`MediaState`] filters old-generation events.
//!
//! The event queue holds at most [`EVENT_CHANNEL_CAPACITY`] items. At capacity,
//! a position event replaces the newest queued position or is dropped if none
//! exists. Other events evict the oldest position; without one, an overflow
//! error replaces the first non-terminal event, or the oldest terminal event,
//! and closes the channel. The triggering event is lost in that overflow case.
//! [`AudioEventReceiver::subscription`] coalesces wakes and ends after closure.
//!
//! Use [`AudioService::apply_events_no_mpris`] to drain into a reducer; with
//! `mpris`, `AudioService::apply_events` also publishes the resulting state.
//! Its return count includes events rejected by the reducer's generation filter.
//! Confirmed seeks emit [`MediaEventKind::PositionDiscontinuity`] after the
//! pipeline settles and its actual position is queried; normal polls emit
//! [`MediaEventKind::PositionUpdated`].
//!
//! EOS or a fatal bus error retires the live pipeline. Further commands for
//! that generation produce refusal events; recovery requires a new load.
//! A rate event confirms native seek acceptance, not completion of the seek.
//!
//! [`AudioService::begin_shutdown`] signals without joining. Poll
//! [`AudioService::try_join_finished`] for completion, or explicitly block with
//! [`AudioService::shutdown_blocking`]. Dropping transfers a live worker to a
//! process-wide reaper; its reserved slot remains occupied until join. A stuck
//! worker can therefore delay later reaping and exhaust [`AUDIO_REAPER_CAPACITY`].

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use gstreamer as gst;
use gstreamer::prelude::*;

use crate::media::CurlRedirectProbe;
use crate::media::gst_worker::{PreparedSource, ensure_initialised};
use crate::media::lifecycle::WorkerLifecycle;
use crate::media::pinned_proxy::PinnedProxy;
use crate::media::remote_gate::{GateEffects, MediaGate, MonotonicClock, SystemResolver};
use crate::media::{
    Capabilities, CommandRequest, Generation, MediaCommand, MediaError, MediaErrorKind, MediaEvent,
    MediaEventKind, MediaSource, MediaState, PlaybackState, sanitize_rate, sanitize_volume,
};

/// Number of native equalizer bands, from 29 Hz through 15 kHz.
pub const AUDIO_EQUALIZER_BAND_COUNT: usize = 10;
/// Minimum accepted gain for each equalizer band, in decibels.
pub const MIN_EQUALIZER_GAIN_DB: f64 = -12.0;
/// Maximum accepted gain for each equalizer band, in decibels.
pub const MAX_EQUALIZER_GAIN_DB: f64 = 12.0;
/// Minimum independent linear processing gain.
pub const MIN_PROCESSING_GAIN: f64 = 0.0;
/// Maximum independent linear processing gain.
pub const MAX_PROCESSING_GAIN: f64 = 16.0;

/// Validated native audio processing for one source generation.
///
/// The linear gain is independent of user volume and mute. Callers can
/// multiply ReplayGain, fade, and an optional EQ preamp before construction;
/// the combined value must remain in [`MIN_PROCESSING_GAIN`] through
/// [`MAX_PROCESSING_GAIN`]. Source replacement resets processing to
/// [`AudioEffects::FLAT`], so persisted policy must be submitted again with
/// the replacement generation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioEffects {
    equalizer_enabled: bool,
    equalizer_gains_db: [f64; AUDIO_EQUALIZER_BAND_COUNT],
    processing_gain: f64,
}

impl AudioEffects {
    /// Flat equalization and unity processing gain.
    pub const FLAT: Self = Self {
        equalizer_enabled: false,
        equalizer_gains_db: [0.0; AUDIO_EQUALIZER_BAND_COUNT],
        processing_gain: 1.0,
    };

    /// Validates equalizer gains and independent linear processing gain.
    ///
    /// # Errors
    ///
    /// Returns [`AudioEffectsError`] for a non-finite value, a band outside
    /// `-12..=12` dB, or a processing gain outside `0..=16`.
    pub fn new(
        equalizer_enabled: bool,
        equalizer_gains_db: [f64; AUDIO_EQUALIZER_BAND_COUNT],
        processing_gain: f64,
    ) -> Result<Self, AudioEffectsError> {
        for (band, gain_db) in equalizer_gains_db.iter().copied().enumerate() {
            if !gain_db.is_finite() {
                return Err(AudioEffectsError::NonFiniteEqualizerGain { band });
            }
            if !(MIN_EQUALIZER_GAIN_DB..=MAX_EQUALIZER_GAIN_DB).contains(&gain_db) {
                return Err(AudioEffectsError::EqualizerGainOutOfRange { band });
            }
        }
        if !processing_gain.is_finite() {
            return Err(AudioEffectsError::NonFiniteProcessingGain);
        }
        if !(MIN_PROCESSING_GAIN..=MAX_PROCESSING_GAIN).contains(&processing_gain) {
            return Err(AudioEffectsError::ProcessingGainOutOfRange);
        }
        Ok(Self {
            equalizer_enabled,
            equalizer_gains_db,
            processing_gain,
        })
    }

    /// Whether the equalizer bands are applied.
    #[must_use]
    pub const fn equalizer_enabled(self) -> bool {
        self.equalizer_enabled
    }

    /// The ten gains in dB, ordered from the 29 Hz band through 15 kHz.
    #[must_use]
    pub const fn equalizer_gains_db(self) -> [f64; AUDIO_EQUALIZER_BAND_COUNT] {
        self.equalizer_gains_db
    }

    /// The linear gain applied independently of user volume and mute.
    #[must_use]
    pub const fn processing_gain(self) -> f64 {
        self.processing_gain
    }

    fn requests_equalizer(self) -> bool {
        self.equalizer_enabled && self.equalizer_gains_db.iter().any(|gain| *gain != 0.0)
    }
}

impl Default for AudioEffects {
    fn default() -> Self {
        Self::FLAT
    }
}

/// Why audio-effects construction failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioEffectsError {
    /// An equalizer band contains NaN or infinity.
    NonFiniteEqualizerGain {
        /// Zero-based index of the invalid band.
        band: usize,
    },
    /// An equalizer band is outside `-12..=12` dB.
    EqualizerGainOutOfRange {
        /// Zero-based index of the invalid band.
        band: usize,
    },
    /// The independent linear gain contains NaN or infinity.
    NonFiniteProcessingGain,
    /// The independent linear gain is outside `0..=16`.
    ProcessingGainOutOfRange,
}

impl core::fmt::Display for AudioEffectsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NonFiniteEqualizerGain { band } => {
                write!(f, "equalizer band {band} gain is not finite")
            }
            Self::EqualizerGainOutOfRange { band } => {
                write!(f, "equalizer band {band} gain is outside -12..=12 dB")
            }
            Self::NonFiniteProcessingGain => f.write_str("processing gain is not finite"),
            Self::ProcessingGainOutOfRange => f.write_str("processing gain is outside 0..=16"),
        }
    }
}

impl std::error::Error for AudioEffectsError {}

#[cfg(feature = "mpris")]
use crate::media::mpris::{MprisAdapter, MprisConnection};

mod worker;
use worker::*;

/// Tuning knobs for the audio service worker.
#[derive(Debug, Clone, Copy)]
pub struct AudioServiceConfig {
    /// Budget for each remote-configuration and preroll wait; defaults to 5 s.
    /// Excludes gate approval, construction, state-change calls, and teardown.
    pub preroll_timeout: Duration,
    /// Command receive timeout with a live pipeline; defaults to 50 ms.
    /// A zero interval permits busy polling; native operations can delay checks.
    pub bus_poll_interval: Duration,
    /// Minimum interval between position queries; defaults to 250 ms.
    /// Queries run on worker cycles, and changes of at most 10 ms are suppressed.
    pub position_poll_interval: Duration,
    /// Maximum bus messages drained per worker cycle; defaults to 32.
    /// Zero disables bus processing and terminal detection.
    pub max_bus_messages_per_cycle: usize,
    /// Destination policy for remote sources.
    pub gate: MediaGate,
    #[cfg(test)]
    audio_sink_factory: &'static str,
    #[cfg(test)]
    equalizer_factory: &'static str,
}

impl Default for AudioServiceConfig {
    fn default() -> Self {
        Self {
            preroll_timeout: Duration::from_secs(5),
            bus_poll_interval: Duration::from_millis(50),
            position_poll_interval: Duration::from_millis(250),
            max_bus_messages_per_cycle: 32,
            gate: MediaGate::DEFAULT,
            #[cfg(test)]
            audio_sink_factory: "autoaudiosink",
            #[cfg(test)]
            equalizer_factory: "equalizer-10bands",
        }
    }
}

#[derive(Debug)]
struct AudioCommand {
    generation: Generation,
    kind: AudioCommandKind,
}

#[derive(Debug)]
enum AudioCommandKind {
    Load {
        source: MediaSource,
        cancel: Arc<AtomicBool>,
        controls: SourceControls,
    },
    Play,
    Pause,
    Stop,
    Seek(Duration),
    SetVolume(f64),
    SetMute(bool),
    // Admission validates the rate before it reaches the worker.
    SetRate(f64),
    SetEffects(AudioEffects),
    #[cfg(test)]
    InspectNative(std::sync::mpsc::SyncSender<NativePlayback>),
    #[cfg(test)]
    ResetNativeAudio,
    Shutdown,
}

#[derive(Debug, Clone, Copy)]
struct SourceControls {
    volume: f64,
    muted: bool,
}

impl Default for SourceControls {
    fn default() -> Self {
        Self {
            volume: 1.0,
            muted: false,
        }
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
struct NativePlayback {
    position: Option<Duration>,
    rate: Option<f64>,
    state: gst::State,
    last_seek: Option<(f64, Duration)>,
    effects: AudioEffects,
    processing_gain: f64,
    user_volume: f64,
    user_muted: bool,
    audio_rms: Option<f64>,
}

/// Hard capacity of the event queue.
pub const EVENT_CHANNEL_CAPACITY: usize = 64;

/// Coalesced wake notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AudioServiceWake {
    /// Wrapping counter advanced after each accepted event.
    pub generation: u64,
}

#[derive(Debug)]
struct EventChannelInner {
    events: VecDeque<MediaEvent>,
    position_coalesced: u64,
    overflow_errors: u64,
    wake_generation: u64,
    observed_generation: u64,
    waker: Option<std::task::Waker>,
    closed: bool,
}

#[derive(Debug, Clone)]
struct EventSender {
    inner: Arc<Mutex<EventChannelInner>>,
}

impl EventSender {
    fn send(&self, event: MediaEvent) {
        let waker = {
            let mut guard = match self.inner.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            if guard.closed {
                return;
            }
            if guard.events.len() >= EVENT_CHANNEL_CAPACITY {
                let is_position = matches!(event.kind, MediaEventKind::PositionUpdated(_));
                if is_position {
                    if let Some(pos) = guard
                        .events
                        .iter_mut()
                        .rev()
                        .find(|e| matches!(e.kind, MediaEventKind::PositionUpdated(_)))
                    {
                        *pos = event;
                    }
                    guard.position_coalesced += 1;
                    Self::collect_waker(&mut guard)
                } else {
                    let evicted = guard
                        .events
                        .iter()
                        .position(|e| matches!(e.kind, MediaEventKind::PositionUpdated(_)));
                    if let Some(idx) = evicted {
                        guard.events.remove(idx);
                        guard.position_coalesced += 1;
                        guard.events.push_back(event);
                        Self::collect_waker(&mut guard)
                    } else {
                        // Report overflow through the same bounded queue before closing it.
                        let overflow_event = MediaEvent {
                            generation: event.generation,
                            kind: MediaEventKind::Error(MediaError {
                                kind: MediaErrorKind::Backend,
                                message: "audio event queue overflow".to_owned(),
                            }),
                        };
                        let replaceable = guard.events.iter().position(|e| {
                            !matches!(
                                e.kind,
                                MediaEventKind::StateChanged(PlaybackState::EndOfStream)
                                    | MediaEventKind::Error(_)
                            )
                        });
                        if let Some(idx) = replaceable {
                            guard.events[idx] = overflow_event;
                        } else {
                            // All events are terminal/error; replace oldest.
                            if let Some(front) = guard.events.front_mut() {
                                *front = overflow_event;
                            }
                        }
                        guard.overflow_errors += 1;
                        guard.closed = true;
                        Self::collect_waker(&mut guard)
                    }
                }
            } else {
                guard.events.push_back(event);
                Self::collect_waker(&mut guard)
            }
        };
        if let Some(w) = waker {
            w.wake();
        }
    }

    fn close(&self) {
        let waker = {
            let mut guard = match self.inner.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.closed = true;
            Self::collect_waker(&mut guard)
        };
        if let Some(w) = waker {
            w.wake();
        }
    }

    fn collect_waker(guard: &mut EventChannelInner) -> Option<std::task::Waker> {
        guard.wake_generation = guard.wake_generation.wrapping_add(1);
        guard.waker.take()
    }
}

/// Receiver half for the app thread.
#[derive(Debug)]
pub struct AudioEventReceiver {
    inner: Arc<Mutex<EventChannelInner>>,
}

impl AudioEventReceiver {
    /// Non-blockingly receives the next event, if any.
    #[must_use]
    pub fn recv(&self) -> Option<MediaEvent> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .events
            .pop_front()
    }

    /// Drains all pending events into a `Vec`.
    pub fn drain(&self) -> Vec<MediaEvent> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .events
            .drain(..)
            .collect()
    }

    /// Whether the worker has closed the channel.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .closed
    }

    /// Buffered event count.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .events
            .len()
    }

    /// Position events replaced, dropped, or evicted due to capacity pressure.
    #[must_use]
    pub fn position_coalesced(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .position_coalesced
    }

    /// Number of overflow-close events (0 or 1).
    #[must_use]
    pub fn overflow_errors(&self) -> u64 {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .overflow_errors
    }

    /// Builds an iced subscription that wakes the runtime on new events.
    /// After the channel closes the stream emits one final wake then
    /// terminates, preventing an infinite busy-loop.
    pub fn subscription(&self) -> iced_futures::Subscription<AudioServiceWake> {
        iced_futures::Subscription::run_with(
            AudioWakeSubscriptionData {
                inner: Arc::clone(&self.inner),
            },
            |data| {
                let inner = Arc::clone(&data.inner);
                let seen = inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .observed_generation;
                iced_futures::futures::stream::unfold(
                    (inner, seen, false),
                    async move |(inner, seen, close_emitted)| {
                        if close_emitted {
                            return None;
                        }
                        let wake = AudioWakeWait::new(Arc::clone(&inner), seen).await;
                        let closed = inner
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .closed;
                        Some((wake, (inner, wake.generation, closed)))
                    },
                )
            },
        )
    }
}

#[derive(Clone)]
struct AudioWakeSubscriptionData {
    inner: Arc<Mutex<EventChannelInner>>,
}
impl std::hash::Hash for AudioWakeSubscriptionData {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.inner) as usize).hash(state);
    }
}

struct AudioWakeWait {
    inner: Arc<Mutex<EventChannelInner>>,
    seen: u64,
}
impl AudioWakeWait {
    fn new(inner: Arc<Mutex<EventChannelInner>>, seen: u64) -> Self {
        Self { inner, seen }
    }
}
impl std::future::Future for AudioWakeWait {
    type Output = AudioServiceWake;
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        let this = self.get_mut();
        let mut guard = this
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let current = guard.wake_generation;
        if current != this.seen || guard.closed {
            guard.observed_generation = current;
            return std::task::Poll::Ready(AudioServiceWake {
                generation: current,
            });
        }
        match &guard.waker {
            Some(existing) if existing.will_wake(cx.waker()) => {}
            _ => {
                guard.waker = Some(cx.waker().clone());
            }
        }
        std::task::Poll::Pending
    }
}
impl Drop for AudioWakeWait {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.waker = None;
        }
    }
}

fn event_channel() -> (EventSender, AudioEventReceiver) {
    let inner = Arc::new(Mutex::new(EventChannelInner {
        events: VecDeque::with_capacity(EVENT_CHANNEL_CAPACITY),
        position_coalesced: 0,
        overflow_errors: 0,
        wake_generation: 0,
        observed_generation: 0,
        waker: None,
        closed: false,
    }));
    (
        EventSender {
            inner: Arc::clone(&inner),
        },
        AudioEventReceiver { inner },
    )
}

/// Maximum commands buffered.
pub const COMMAND_CHANNEL_CAPACITY: usize = 32;

/// Why a load was not admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadRefused {
    /// The command channel is full.
    ChannelFull,
    /// Admission has closed or the worker has exited.
    Disconnected,
}
impl core::fmt::Display for LoadRefused {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LoadRefused::ChannelFull => f.write_str("audio command channel full"),
            LoadRefused::Disconnected => f.write_str("audio worker exited"),
        }
    }
}
impl std::error::Error for LoadRefused {}

/// Why a command was not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// Admission is closed or the command queue is full or disconnected.
    ChannelUnavailable,
    /// Playlist commands are handled outside this audio service.
    Unsupported,
    /// The rate is nonfinite or outside the supported range.
    InvalidRate,
}
impl core::fmt::Display for SendError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SendError::ChannelUnavailable => f.write_str("audio command channel unavailable"),
            SendError::Unsupported => f.write_str("command not supported by audio service"),
            SendError::InvalidRate => f.write_str("playback rate outside the accepted range"),
        }
    }
}
impl std::error::Error for SendError {}

/// Maximum concurrent [`AudioService`] instances for which the process-wide
/// reaper can retain a worker handle on drop.
pub const AUDIO_REAPER_CAPACITY: usize = 16;

enum ReaperSlotState {
    Free,
    Reserved,
    Stranded { _worker: JoinHandle<()> },
}

struct ReaperShared {
    slots: [Mutex<ReaperSlotState>; AUDIO_REAPER_CAPACITY],
    transfers: std::sync::atomic::AtomicUsize,
    failed: AtomicBool,
}

impl ReaperShared {
    fn new() -> Self {
        Self {
            slots: std::array::from_fn(|_| Mutex::new(ReaperSlotState::Free)),
            transfers: std::sync::atomic::AtomicUsize::new(0),
            failed: AtomicBool::new(false),
        }
    }

    fn reserve_index(&self) -> std::io::Result<usize> {
        if self.failed.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("audio reaper unavailable"));
        }
        for (index, slot) in self.slots.iter().enumerate() {
            let mut state = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if matches!(&*state, ReaperSlotState::Free) {
                *state = ReaperSlotState::Reserved;
                if self.failed.load(Ordering::SeqCst) {
                    *state = ReaperSlotState::Free;
                    return Err(std::io::Error::other("audio reaper unavailable"));
                }
                return Ok(index);
            }
        }
        Err(std::io::Error::other("audio reaper capacity exhausted"))
    }

    fn release(&self, index: usize) {
        let mut state = self.slots[index]
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(&*state, ReaperSlotState::Reserved) {
            *state = ReaperSlotState::Free;
        }
    }

    fn strand(&self, index: usize, worker: JoinHandle<()>) {
        let mut state = self.slots[index]
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *state = ReaperSlotState::Stranded { _worker: worker };
    }

    fn snapshot_counts(&self) -> ReaperSnapshot {
        let mut reserved = 0;
        let mut stranded = 0;
        for slot in &self.slots {
            let state = slot
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match &*state {
                ReaperSlotState::Free => {}
                ReaperSlotState::Reserved => reserved += 1,
                ReaperSlotState::Stranded { .. } => stranded += 1,
            }
        }
        ReaperSnapshot {
            reserved,
            stranded,
            failed: self.failed.load(Ordering::SeqCst),
        }
    }
}

struct ReapRequest {
    slot: usize,
    worker: JoinHandle<()>,
}

struct ReaperReservation {
    sender: std::sync::mpsc::SyncSender<ReapRequest>,
    shared: Arc<ReaperShared>,
    slot: usize,
    active: bool,
}

impl ReaperReservation {
    fn transfer(mut self, worker: JoinHandle<()>) {
        let request = ReapRequest {
            slot: self.slot,
            worker,
        };
        if self.shared.failed.load(Ordering::SeqCst) {
            self.shared.strand(request.slot, request.worker);
        } else {
            self.shared.transfers.fetch_add(1, Ordering::SeqCst);
            if self.shared.failed.load(Ordering::SeqCst) {
                self.shared.transfers.fetch_sub(1, Ordering::SeqCst);
                self.shared.strand(request.slot, request.worker);
            } else {
                let result = self.sender.try_send(request);
                self.shared.transfers.fetch_sub(1, Ordering::SeqCst);
                if let Err(error) = result {
                    self.shared.failed.store(true, Ordering::SeqCst);
                    let request = match error {
                        std::sync::mpsc::TrySendError::Full(request)
                        | std::sync::mpsc::TrySendError::Disconnected(request) => request,
                    };
                    self.shared.strand(request.slot, request.worker);
                }
            }
        }
        self.active = false;
    }
}

impl Drop for ReaperReservation {
    fn drop(&mut self) {
        if self.active {
            self.shared.release(self.slot);
        }
    }
}

impl std::fmt::Debug for ReaperReservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReaperReservation")
            .field("slot", &self.slot)
            .field("active", &self.active)
            .finish_non_exhaustive()
    }
}

struct AudioReaper {
    sender: std::sync::mpsc::SyncSender<ReapRequest>,
    shared: Arc<ReaperShared>,
    _thread: Mutex<Option<JoinHandle<()>>>,
}

impl AudioReaper {
    fn start() -> std::io::Result<Self> {
        let shared = Arc::new(ReaperShared::new());
        let worker_shared = Arc::clone(&shared);
        let (sender, receiver) =
            std::sync::mpsc::sync_channel::<ReapRequest>(AUDIO_REAPER_CAPACITY);
        let thread = std::thread::Builder::new()
            .name("enzui-audio-reaper".to_owned())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    while let Ok(request) = receiver.recv() {
                        let ReapRequest { slot, worker } = request;
                        let _ = worker.join();
                        worker_shared.release(slot);
                    }
                }));
                worker_shared.failed.store(true, Ordering::SeqCst);
                while worker_shared.transfers.load(Ordering::SeqCst) != 0 {
                    std::thread::yield_now();
                }
                if result.is_err() {
                    while let Ok(request) = receiver.try_recv() {
                        worker_shared.strand(request.slot, request.worker);
                    }
                }
            })?;
        Ok(Self {
            sender,
            shared,
            _thread: Mutex::new(Some(thread)),
        })
    }

    fn reserve(&self) -> std::io::Result<ReaperReservation> {
        let slot = self.shared.reserve_index()?;
        Ok(ReaperReservation {
            sender: self.sender.clone(),
            shared: Arc::clone(&self.shared),
            slot,
            active: true,
        })
    }
}

fn audio_reaper() -> std::io::Result<&'static AudioReaper> {
    static REAPER: std::sync::OnceLock<Result<AudioReaper, String>> = std::sync::OnceLock::new();
    match REAPER.get_or_init(|| AudioReaper::start().map_err(|e| e.to_string())) {
        Ok(reaper) => Ok(reaper),
        Err(reason) => Err(std::io::Error::other(reason.clone())),
    }
}

/// Observable snapshot of process-wide audio-reaper occupancy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReaperSnapshot {
    /// Occupied reservations, including handles queued for or undergoing reaper join.
    pub reserved: usize,
    /// Slots holding a stranded worker handle after a reaper failure.
    pub stranded: usize,
    /// Whether the reaper thread has exited (once true, further transfers strand).
    pub failed: bool,
}

/// Reads the process-wide audio-reaper occupancy for observability.
///
/// Initializes the singleton reaper on first use. If initialization fails,
/// returns zero occupancy with `failed = true`; subsequent calls retain that failure.
#[must_use]
pub fn audio_reaper_snapshot() -> ReaperSnapshot {
    match audio_reaper() {
        Ok(r) => r.shared.snapshot_counts(),
        Err(_) => ReaperSnapshot {
            reserved: 0,
            stranded: 0,
            failed: true,
        },
    }
}

/// A commandable audio-only playback service backed by GStreamer.
#[derive(Debug)]
pub struct AudioService {
    cmd_tx: std::sync::mpsc::SyncSender<AudioCommand>,
    stop: Arc<AtomicBool>,
    // Cancelled after a new Load is successfully enqueued; also on shutdown.
    generation_cancel: Arc<AtomicBool>,
    generation: Generation,
    thread: Option<JoinHandle<()>>,
    lifecycle: WorkerLifecycle,
    reaper_reservation: Option<ReaperReservation>,
}

impl AudioService {
    /// Creates a new audio service starting at [`Generation::FIRST`].
    ///
    /// # Errors
    ///
    /// Returns a [`MediaError`] if GStreamer cannot init, the reaper cannot
    /// reserve a slot, or the worker thread cannot spawn.
    pub fn new(config: AudioServiceConfig) -> Result<(Self, AudioEventReceiver), MediaError> {
        Self::new_at(config, Generation::FIRST)
    }

    /// Creates a new audio service starting at an explicit generation.
    ///
    /// Sets the service counter used by [`Self::load`]. [`Self::load_into`]
    /// instead derives its next generation from the supplied reducer.
    ///
    /// # Errors
    ///
    /// Returns a [`MediaError`] if GStreamer cannot init, the reaper cannot
    /// reserve a slot ([`AUDIO_REAPER_CAPACITY`]), or the worker thread
    /// cannot spawn.
    pub fn new_at(
        config: AudioServiceConfig,
        start: Generation,
    ) -> Result<(Self, AudioEventReceiver), MediaError> {
        ensure_initialised()?;
        let reaper_reservation =
            audio_reaper()
                .and_then(AudioReaper::reserve)
                .map_err(|e| MediaError {
                    kind: MediaErrorKind::Backend,
                    message: format!("audio reaper reservation unavailable: {e}",),
                })?;
        let (cmd_tx, cmd_rx) = std::sync::mpsc::sync_channel(COMMAND_CHANNEL_CAPACITY);
        let (evt_tx, evt_rx) = event_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("enzui-audio-svc".to_owned())
            .spawn(move || audio_worker_loop(cmd_rx, evt_tx, stop_clone, config))
            .map_err(|_| MediaError {
                kind: MediaErrorKind::Backend,
                message: "could not spawn the audio service thread".to_owned(),
            })?;
        Ok((
            AudioService {
                cmd_tx,
                stop,
                generation_cancel: Arc::new(AtomicBool::new(false)),
                generation: start,
                thread: Some(thread),
                lifecycle: WorkerLifecycle::start(),
                reaper_reservation: Some(reaper_reservation),
            },
            evt_rx,
        ))
    }

    /// Current generation, updated only after an admitted load.
    #[must_use]
    pub fn generation(&self) -> Generation {
        self.generation
    }

    /// Queue a replacement source without blocking. Admission advances the
    /// service generation and signals cancellation of the previous preparation;
    /// refusal leaves the generation and cancellation token unchanged.
    /// Remote approval and pipeline setup run on the worker, where cancellation
    /// is checked between operations.
    ///
    /// # Errors
    ///
    /// Returns [`LoadRefused`] when the channel is full or disconnected.
    pub fn load(&mut self, source: MediaSource) -> Result<Generation, LoadRefused> {
        if self.stop.load(Ordering::Acquire) {
            return Err(LoadRefused::Disconnected);
        }
        let next = self.generation.next();
        let new_cancel = Arc::new(AtomicBool::new(false));
        // Refused admission must leave the prior generation uncancelled.
        match self.cmd_tx.try_send(AudioCommand {
            generation: next,
            kind: AudioCommandKind::Load {
                source,
                cancel: Arc::clone(&new_cancel),
                controls: SourceControls::default(),
            },
        }) {
            Ok(()) => {
                self.generation_cancel.store(true, Ordering::Release);
                self.generation = next;
                self.generation_cancel = new_cancel;
                Ok(next)
            }
            Err(std::sync::mpsc::TrySendError::Full(_)) => Err(LoadRefused::ChannelFull),
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => Err(LoadRefused::Disconnected),
        }
    }

    /// Queue a source using `state.generation().next()`, then update both
    /// service and reducer to that generation after successful admission.
    /// Use this when a reducer owns the source: its generation remains the
    /// authority after clearing or recreating the service. Failure leaves
    /// both counters and the old cancellation token unchanged. The new native
    /// pipeline receives the state's volume and mute before preroll and
    /// `SourceReady`; independent effects reset to flat/unity.
    ///
    /// # Errors
    ///
    /// Returns [`LoadRefused`] when the channel is full or disconnected;
    /// neither the service generation nor the `MediaState` is changed.
    pub fn load_into(
        &mut self,
        source: MediaSource,
        state: &mut MediaState,
    ) -> Result<Generation, LoadRefused> {
        if self.stop.load(Ordering::Acquire) {
            return Err(LoadRefused::Disconnected);
        }
        let target = state.generation().next();
        let snapshot = state.snapshot();
        let controls = SourceControls {
            volume: sanitize_volume(snapshot.volume),
            muted: snapshot.muted,
        };
        let new_cancel = Arc::new(AtomicBool::new(false));
        // Enqueue first; nothing is mutated until admission succeeds.
        match self.cmd_tx.try_send(AudioCommand {
            generation: target,
            kind: AudioCommandKind::Load {
                source: source.clone(),
                cancel: Arc::clone(&new_cancel),
                controls,
            },
        }) {
            Ok(()) => {
                self.generation_cancel.store(true, Ordering::Release);
                self.generation = target;
                self.generation_cancel = new_cancel;
                state.replace_source_at(source, target);
                Ok(target)
            }
            Err(std::sync::mpsc::TrySendError::Full(_)) => Err(LoadRefused::ChannelFull),
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => Err(LoadRefused::Disconnected),
        }
    }

    /// Queue a generation-stamped command without blocking. The worker ignores
    /// stale generations; successful admission does not imply native acceptance.
    ///
    /// # Errors
    ///
    /// Returns [`SendError::Unsupported`] for playlist commands that the
    /// audio service does not handle, [`SendError::InvalidRate`] for a
    /// [`MediaCommand::SetRate`] whose value is non-finite or outside
    /// `[MIN_PLAYBACK_RATE, MAX_PLAYBACK_RATE]`, or
    /// [`SendError::ChannelUnavailable`] when the channel is full or
    /// disconnected. Native refusal, device loss and an unsupported
    /// source/backend are reported asynchronously as typed
    /// [`MediaEventKind::Error`] events on the event channel because
    /// they require the worker to consult the live pipeline.
    pub fn send(&self, request: &CommandRequest) -> Result<(), SendError> {
        if self.stop.load(Ordering::Acquire) {
            return Err(SendError::ChannelUnavailable);
        }
        let kind = match &request.command {
            MediaCommand::Play => AudioCommandKind::Play,
            MediaCommand::Pause => AudioCommandKind::Pause,
            MediaCommand::Stop => AudioCommandKind::Stop,
            MediaCommand::Seek(d) => AudioCommandKind::Seek(*d),
            MediaCommand::SetVolume(v) => AudioCommandKind::SetVolume(*v),
            MediaCommand::SetMute(m) => AudioCommandKind::SetMute(*m),
            MediaCommand::SetRate(r) => {
                let sane = sanitize_rate(*r).ok_or(SendError::InvalidRate)?;
                AudioCommandKind::SetRate(sane)
            }
            MediaCommand::PreviousTrack
            | MediaCommand::NextTrack
            | MediaCommand::ToggleShuffle
            | MediaCommand::CycleRepeat => return Err(SendError::Unsupported),
        };
        self.cmd_tx
            .try_send(AudioCommand {
                generation: request.generation,
                kind,
            })
            .map_err(|_| SendError::ChannelUnavailable)
    }

    /// Queues validated native audio processing for the specified generation.
    ///
    /// Successful admission does not imply native acceptance. A missing
    /// `equalizer-10bands` plugin produces an asynchronous
    /// [`MediaErrorKind::Refused`] event when non-flat equalization is
    /// requested. Stale generations are ignored. Processing does not alter
    /// user volume, mute, or their events.
    ///
    /// # Errors
    ///
    /// Returns [`SendError::ChannelUnavailable`] when admission is closed or
    /// the command queue cannot accept the request.
    pub fn set_effects(
        &self,
        generation: Generation,
        effects: AudioEffects,
    ) -> Result<(), SendError> {
        if self.stop.load(Ordering::Acquire) {
            return Err(SendError::ChannelUnavailable);
        }
        self.cmd_tx
            .try_send(AudioCommand {
                generation,
                kind: AudioCommandKind::SetEffects(effects),
            })
            .map_err(|_| SendError::ChannelUnavailable)
    }

    /// Drain events into `state` and publish once via MPRIS if any were drained.
    /// Returns the drained count, including stale events rejected by `state`.
    #[cfg(feature = "mpris")]
    pub fn apply_events(
        &self,
        receiver: &AudioEventReceiver,
        state: &mut MediaState,
        adapter: &MprisAdapter,
        connection: &mut dyn MprisConnection,
    ) -> usize {
        let events = receiver.drain();
        let count = events.len();
        for event in events {
            state.apply(&event);
        }
        if count > 0 {
            adapter.publish_state(state, connection);
        }
        count
    }

    /// Drain events into `state`, returning the count drained, including
    /// stale events rejected by the reducer.
    pub fn apply_events_no_mpris(
        &self,
        receiver: &AudioEventReceiver,
        state: &mut MediaState,
    ) -> usize {
        let events = receiver.drain();
        let count = events.len();
        for event in events {
            state.apply(&event);
        }
        count
    }

    /// Close admission and request shutdown without joining; returns true
    /// only when the lifecycle enters Draining.
    ///
    /// Cancels preparation and attempts a queue wake. The stop flag remains
    /// effective if the queue is full. Queued commands drained during shutdown
    /// attempt refusal events, subject to event-queue overflow and closure.
    /// Observe receiver closure and [`Self::try_join_finished`] for completion;
    /// native calls and teardown can delay it.
    pub fn begin_shutdown(&mut self) -> bool {
        self.stop.store(true, Ordering::Release);
        self.generation_cancel.store(true, Ordering::Release);
        let _ = self.cmd_tx.try_send(AudioCommand {
            generation: self.generation,
            kind: AudioCommandKind::Shutdown,
        });
        self.lifecycle.begin_shutdown()
    }

    /// Joins the worker only if it has already exited; never blocks.
    ///
    /// Returns `true` when the worker was joined by this call (or was
    /// already joined), `false` when the worker is still running. When
    /// this returns `true` the reaper reservation is released back to the
    /// free pool.
    pub fn try_join_finished(&mut self) -> bool {
        let ready = match self.thread.as_ref() {
            Some(handle) => handle.is_finished(),
            None => return true,
        };
        if !ready {
            return false;
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
            self.lifecycle.complete_join();
            self.reaper_reservation.take();
        }
        true
    }

    /// Explicitly blocking shutdown. Must not be called on a reducer or UI
    /// thread; use [`Self::begin_shutdown`] + [`Self::try_join_finished`]
    /// for the app-close path.
    ///
    /// This is [`Self::begin_shutdown`] followed by [`JoinHandle::join`],
    /// whose duration is unbounded by the service's polling intervals.
    /// Repeated calls are idempotent.
    pub fn shutdown_blocking(&mut self) {
        self.begin_shutdown();
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
            self.lifecycle.complete_join();
            self.reaper_reservation.take();
        }
    }

    /// Blocking alias for [`Self::shutdown_blocking`].
    #[doc(hidden)]
    pub fn shutdown(&mut self) {
        self.shutdown_blocking();
    }

    /// Whether the worker thread has finished.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.thread
            .as_ref()
            .is_none_or(std::thread::JoinHandle::is_finished)
    }

    /// Whether the worker thread has been joined.
    #[must_use]
    pub fn is_joined(&self) -> bool {
        self.thread.is_none()
    }

    /// The current shutdown lifecycle phase.
    #[must_use]
    pub fn lifecycle_phase(&self) -> crate::media::lifecycle::Phase {
        self.lifecycle.phase()
    }
}

impl Drop for AudioService {
    fn drop(&mut self) {
        self.begin_shutdown();
        let Some(handle) = self.thread.take() else {
            return;
        };
        if handle.is_finished() {
            let _ = handle.join();
            self.lifecycle.complete_join();
            self.reaper_reservation.take();
            return;
        }
        match self.reaper_reservation.take() {
            Some(reservation) => reservation.transfer(handle),
            None => {
                // A live worker must retain its construction-time reaper reservation.
                debug_assert!(false, "reaper reservation lost with a live worker");
            }
        }
    }
}

#[cfg(test)]
mod tests;
