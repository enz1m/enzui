use super::*;
use crate::frame_plan::FrameSeq;
use crate::media::gst_worker::{
    BuiltPipeline, GstWorkerConfig, PreparedSource, build_offline_pipeline, ensure_initialised,
    sample_to_frame_with_limits,
};
use crate::media::{Generation, MediaErrorKind};
use enzui_security::secure_dir::SecureRoot;
use enzui_security::tempstage::{DirectoryStage, unique_temp_file_name};
use gst::prelude::*;
use gstreamer as gst;
use std::ffi::OsStr;
use std::fs::{File, Metadata};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[cfg(test)]
pub(super) mod observations {
    use super::*;
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Default)]
    pub(super) struct Counts {
        pub(super) decoders: AtomicUsize,
        pub(super) audio_sinks: AtomicUsize,
        pub(super) discard_sinks: AtomicUsize,
        pub(super) on_decoder: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    }
    thread_local! { static ACTIVE: RefCell<Option<Arc<Counts>>> = const { RefCell::new(None) }; }
    pub(in super::super) fn attach(pipeline: &gst::Pipeline) {
        let counts = ACTIVE.with(|slot| slot.borrow().clone());
        if let Some(counts) = counts {
            pipeline.connect_deep_element_added(move |_, _, element| {
                if let Some(factory) = element.factory() {
                    if factory.klass().contains("Decoder")
                        && (factory.klass().contains("Video") || factory.klass().contains("Image"))
                    {
                        counts.decoders.fetch_add(1, Ordering::SeqCst);
                        let callback = counts.on_decoder.lock().unwrap().take();
                        if let Some(callback) = callback {
                            callback();
                        }
                    }
                    if factory.klass().contains("Sink/Audio") {
                        counts.audio_sinks.fetch_add(1, Ordering::SeqCst);
                    }
                    if factory.name() == "fakesink" {
                        counts.discard_sinks.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });
        }
    }
}

struct Snapshot {
    file: File,
    _stage: DirectoryStage,
}
fn same_version(a: &Metadata, b: &Metadata) -> bool {
    a.dev() == b.dev()
        && a.ino() == b.ino()
        && a.len() == b.len()
        && a.mtime() == b.mtime()
        && a.mtime_nsec() == b.mtime_nsec()
        && a.ctime() == b.ctime()
        && a.ctime_nsec() == b.ctime_nsec()
}
impl Snapshot {
    fn create(
        request: &OfflineVideoRequest,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Self, OfflineVideoError> {
        let mut source = super::super::open_preview_file(&request.path)?;
        let before = source.metadata()?;
        if !before.is_file() {
            return Err(OfflineVideoError::SourceChanged);
        }
        if before.len() > request.limits.max_file_bytes {
            return Err(OfflineVideoError::FileLimit {
                bytes: before.len(),
                limit: request.limits.max_file_bytes,
            });
        }
        if cancelled() {
            return Err(OfflineVideoError::Cancelled);
        }
        let root = SecureRoot::open(&request.snapshot_root)
            .map_err(|e| OfflineVideoError::Io(format!("{e:?}")))?;
        let stage =
            DirectoryStage::create(&root, &unique_temp_file_name(OsStr::new("offline-video")))?;
        let mut file = stage.root().create_entry_private(OsStr::new("source"))?;
        if rustix::fs::ioctl_ficlone(&file, &source).is_err() {
            if before.len() > request.limits.max_snapshot_copy_bytes {
                return Err(OfflineVideoError::SnapshotUnavailable);
            }
            file.set_len(0)?;
            let mut remaining = before.len();
            let mut buffer = vec![0; 65536];
            while remaining != 0 {
                if cancelled() {
                    return Err(OfflineVideoError::Cancelled);
                }
                let admitted =
                    usize::try_from(remaining).map_or(buffer.len(), |left| left.min(buffer.len()));
                let read = source.read(&mut buffer[..admitted])?;
                if read == 0 {
                    return Err(OfflineVideoError::SourceChanged);
                }
                file.write_all(&buffer[..read])?;
                remaining -= read as u64;
            }
        }
        if !same_version(&before, &source.metadata()?) || file.metadata()?.len() != before.len() {
            return Err(OfflineVideoError::SourceChanged);
        }
        if cancelled() {
            return Err(OfflineVideoError::Cancelled);
        }
        let readonly = stage.root().open_entry_read(OsStr::new("source"))?;
        let written = file.metadata()?;
        let reopened = readonly.metadata()?;
        if written.dev() != reopened.dev() || written.ino() != reopened.ino() {
            return Err(OfflineVideoError::SourceChanged);
        }
        drop(file);
        Ok(Self {
            file: readonly,
            _stage: stage,
        })
    }
    fn path(&self) -> PathBuf {
        format!("/proc/self/fd/{}", self.file.as_raw_fd()).into()
    }
}

type VideoGeometryAndRate = (u32, u32, Option<(u32, u32)>);

#[derive(Default)]
struct Discovery {
    natural: Option<VideoGeometryAndRate>,
    tracks: Vec<OfflineTrack>,
    overflow: bool,
    refusal: Option<OfflineVideoError>,
}
struct Session {
    built: BuiltPipeline,
    initial: gst::Sample,
    probe: VideoProbe,
    _snapshot: Snapshot,
}
impl Drop for Session {
    fn drop(&mut self) {
        self.built.shutdown();
    }
}
fn backend_error(error: crate::media::MediaError) -> OfflineVideoError {
    if error.kind == MediaErrorKind::UnsupportedFormat {
        OfflineVideoError::Unsupported(error.message)
    } else {
        OfflineVideoError::Decode(error.message)
    }
}
fn bus_error(built: &BuiltPipeline) -> Result<(), OfflineVideoError> {
    if let Some(bus) = built.pipeline.bus()
        && let Some(message) = bus.pop_filtered(&[gst::MessageType::Error])
        && let gst::MessageView::Error(error) = message.view()
    {
        let error = error.error();
        return Err(if error.matches(gst::CoreError::MissingPlugin) {
            OfflineVideoError::Unsupported(error.to_string())
        } else {
            OfflineVideoError::Decode(error.to_string())
        });
    }
    Ok(())
}
fn paused(
    built: &BuiltPipeline,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), OfflineVideoError> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or(OfflineVideoError::InvalidRequest)?;
    loop {
        if cancelled() {
            return Err(OfflineVideoError::Cancelled);
        }
        bus_error(built)?;
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or(OfflineVideoError::Timeout)?;
        let (result, state, pending) = built.pipeline.state(gst::ClockTime::from_nseconds(
            u64::try_from(left.min(Duration::from_millis(25)).as_nanos()).unwrap_or(u64::MAX),
        ));
        if result.is_err() {
            bus_error(built)?;
            return Err(OfflineVideoError::Decode("offline preroll failed".into()));
        }
        if state == gst::State::Paused && pending == gst::State::VoidPending {
            return Ok(());
        }
    }
}
fn preroll(
    built: &BuiltPipeline,
    timeout: Duration,
    cancelled: &dyn Fn() -> bool,
) -> Result<gst::Sample, OfflineVideoError> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or(OfflineVideoError::InvalidRequest)?;
    loop {
        if cancelled() {
            return Err(OfflineVideoError::Cancelled);
        }
        bus_error(built)?;
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or(OfflineVideoError::Timeout)?;
        if let Some(sample) = built
            .video_sink
            .try_pull_preroll(gst::ClockTime::from_nseconds(
                u64::try_from(left.min(Duration::from_millis(25)).as_nanos()).unwrap_or(u64::MAX),
            ))
        {
            return Ok(sample);
        }
        if built.video_sink.is_eos() {
            return Err(OfflineVideoError::NoVideo);
        }
    }
}
fn admit(width: u32, height: u32, limits: &DecodeLimits) -> Result<(), OfflineVideoError> {
    if let Some(verdict) = RefusedVerdict::new(limits.check_decoded(width, height)) {
        return Err(OfflineVideoError::FrameLimit(verdict));
    }
    Ok(())
}
impl Session {
    fn open(
        request: &OfflineVideoRequest,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Self, OfflineVideoError> {
        let snapshot = Snapshot::create(request, cancelled)?;
        ensure_initialised().map_err(backend_error)?;
        let built = build_offline_pipeline(
            &PreparedSource::local(snapshot.path()),
            &GstWorkerConfig::default(),
        )
        .map_err(backend_error)?;
        #[cfg(test)]
        observations::attach(&built.pipeline);
        let discovery = Arc::new(Mutex::new(Discovery::default()));
        let captured = Arc::clone(&discovery);
        let decode = built
            .pipeline
            .by_name("enzui-offline-decode")
            .ok_or_else(|| OfflineVideoError::Decode("offline decoder absent".into()))?;
        let header_discovery = Arc::clone(&discovery);
        let frame_limits = request.limits.frames;
        let selection_type =
            gst::glib::Type::from_name("GstAutoplugSelectResult").ok_or_else(|| {
                OfflineVideoError::Unsupported("decoder admission signal is unavailable".into())
            })?;
        let selection_class = gst::glib::EnumClass::with_type(selection_type)
            .filter(|class| class.value(0).is_some() && class.value(2).is_some())
            .ok_or_else(|| {
                OfflineVideoError::Unsupported("decoder admission enum is unavailable".into())
            })?;
        drop(selection_class);
        decode.connect("autoplug-select", false, move |values| {
            let factory = values[3].get::<gst::ElementFactory>().ok();
            let mut decision = 0;
            let caps = values[2].get::<gst::Caps>().ok();
            let raster_caps = caps
                .as_ref()
                .and_then(|caps| caps.structure(0))
                .is_some_and(|structure| {
                    structure.name().starts_with("video/") || structure.name().starts_with("image/")
                });
            if factory.is_some_and(|factory| {
                factory.klass().contains("Decoder")
                    && (factory.klass().contains("Video") || raster_caps)
            }) {
                let geometry = caps.and_then(|caps| {
                    let structure = caps.structure(0)?;
                    Some((
                        structure.get::<i32>("width").ok()?,
                        structure.get::<i32>("height").ok()?,
                    ))
                });
                let admitted = match geometry {
                    Some((width, height)) if width > 0 && height > 0 => {
                        admit(width.unsigned_abs(), height.unsigned_abs(), &frame_limits)
                    }
                    _ => Err(OfflineVideoError::Unsupported(
                        "video decoder requires dimension-bearing encoded caps".into(),
                    )),
                };
                if let Err(error) = admitted {
                    header_discovery
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .refusal = Some(error);
                    decision = 2;
                }
            }
            Some(
                gst::glib::EnumClass::with_type(selection_type)
                    .expect("registered admission enum remains available")
                    .to_value(decision)
                    .expect("validated admission enum value"),
            )
        });
        decode.connect_pad_added(move |_, pad| {
            if let Some(caps) = pad.current_caps() {
                let mut info = captured
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if info.tracks.len() == 64 {
                    info.overflow = true;
                    return;
                }
                info.tracks.push(OfflineTrack {
                    id: pad
                        .stream_id()
                        .map(|x| x.chars().take(512).collect())
                        .unwrap_or_default(),
                    caps: caps.to_string().chars().take(1024).collect(),
                });
                if info.natural.is_none()
                    && let Some(structure) = caps.structure(0)
                    && structure.name().starts_with("video/")
                    && let (Ok(width), Ok(height)) = (
                        structure.get::<i32>("width"),
                        structure.get::<i32>("height"),
                    )
                {
                    let rate = structure
                        .get::<gst::Fraction>("framerate")
                        .ok()
                        .filter(|x| x.numer() > 0 && x.denom() > 0)
                        .map(|x| (x.numer().unsigned_abs(), x.denom().unsigned_abs()));
                    if width > 0 && height > 0 {
                        info.natural = Some((width.unsigned_abs(), height.unsigned_abs(), rate));
                    }
                }
            }
        });
        let completed_discovery = Arc::clone(&discovery);
        decode.connect_no_more_pads(move |_| {
            let mut info = completed_discovery
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if info.natural.is_none() && info.refusal.is_none() {
                info.refusal = Some(OfflineVideoError::NoVideo);
            }
        });
        let opened = (|| {
            built.pipeline.set_state(gst::State::Paused).map_err(|_| {
                OfflineVideoError::Decode("offline pipeline would not pause".into())
            })?;
            let stopped = || {
                cancelled()
                    || discovery
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .refusal
                        .is_some()
            };
            paused(&built, request.limits.timeout, &stopped)?;
            let initial = preroll(&built, request.limits.timeout, cancelled)?;
            let info = discovery
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if info.overflow {
                return Err(OfflineVideoError::InvalidRequest);
            }
            let (width, height, frame_rate) = info.natural.ok_or(OfflineVideoError::NoVideo)?;
            admit(width, height, &request.limits.frames)?;
            let probe = VideoProbe {
                width,
                height,
                duration: built
                    .pipeline
                    .query_duration::<gst::ClockTime>()
                    .map(|x| Duration::from_nanos(x.nseconds())),
                frame_rate,
                timestamp_time_base: (1, 1_000_000_000),
                tracks: info.tracks.clone(),
            };
            Ok((initial, probe))
        })();
        let refusal = discovery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .refusal
            .clone();
        let opened = match refusal {
            Some(error) => Err(error),
            None => opened,
        };
        match opened {
            Ok((initial, probe)) => Ok(Self {
                built,
                initial,
                probe,
                _snapshot: snapshot,
            }),
            Err(error) => {
                built.shutdown();
                Err(error)
            }
        }
    }
    fn target(
        &self,
        size: FrameSize,
        limits: &DecodeLimits,
    ) -> Result<(u32, u32), OfflineVideoError> {
        let (width, height) = match size {
            FrameSize::Source => (self.probe.width, self.probe.height),
            FrameSize::Bounds { width, height } => {
                if width == 0 || height == 0 {
                    return Err(OfflineVideoError::InvalidRequest);
                }
                let numerator = u64::from(width).min(u64::from(self.probe.width));
                let denominator = u64::from(self.probe.width);
                let height_num = u64::from(height).min(u64::from(self.probe.height));
                let height_den = u64::from(self.probe.height);
                let (num, den) = if numerator * height_den <= height_num * denominator {
                    (numerator, denominator)
                } else {
                    (height_num, height_den)
                };
                (
                    u32::try_from((u64::from(self.probe.width) * num / den).max(1))
                        .unwrap_or(u32::MAX),
                    u32::try_from((u64::from(self.probe.height) * num / den).max(1))
                        .unwrap_or(u32::MAX),
                )
            }
        };
        admit(width, height, limits)?;
        Ok((width, height))
    }
    fn frame(
        &self,
        timestamp: Duration,
        policy: FramePolicy,
        size: FrameSize,
        limits: &OfflineVideoLimits,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<OfflineFrame, OfflineVideoError> {
        if cancelled() {
            return Err(OfflineVideoError::Cancelled);
        }
        let (width, height) = self.target(size, &limits.frames)?;
        let caps = gstreamer_video::VideoCapsBuilder::new()
            .format(gstreamer_video::VideoFormat::Rgba)
            .width(i32::try_from(width).unwrap_or(i32::MAX))
            .height(i32::try_from(height).unwrap_or(i32::MAX))
            .build();
        self.built
            .pipeline
            .by_name("enzui-offline-output")
            .ok_or_else(|| OfflineVideoError::Decode("offline output filter absent".into()))?
            .set_property("caps", &caps);
        self.built.video_sink.set_caps(Some(&caps));
        let time =
            gst::ClockTime::try_from(timestamp).map_err(|_| OfflineVideoError::InvalidRequest)?;
        let out_of_range = self
            .probe
            .duration
            .is_some_and(|duration| timestamp >= duration);
        if out_of_range && policy == FramePolicy::Accurate {
            return Err(OfflineVideoError::OutOfRange(timestamp));
        }
        let flags = gst::SeekFlags::FLUSH
            | match policy {
                FramePolicy::Poster => gst::SeekFlags::KEY_UNIT | gst::SeekFlags::SNAP_BEFORE,
                FramePolicy::Accurate => gst::SeekFlags::ACCURATE,
            };
        let seek = if out_of_range {
            false
        } else {
            self.built.pipeline.seek_simple(flags, time).is_ok()
        };
        let (sample, fallback) = if !seek {
            if policy == FramePolicy::Accurate {
                return Err(OfflineVideoError::SeekFailed(timestamp));
            }
            if self
                .built
                .pipeline
                .seek_simple(
                    gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                    gst::ClockTime::ZERO,
                )
                .is_ok()
            {
                paused(&self.built, limits.timeout, cancelled)?;
                (preroll(&self.built, limits.timeout, cancelled)?, true)
            } else {
                (self.initial.clone(), true)
            }
        } else {
            paused(&self.built, limits.timeout, cancelled)?;
            (preroll(&self.built, limits.timeout, cancelled)?, false)
        };
        let actual = sample
            .buffer()
            .and_then(gstreamer::BufferRef::pts)
            .ok_or(OfflineVideoError::MissingTimestamp)?;
        if policy == FramePolicy::Accurate && actual < time {
            return Err(OfflineVideoError::SeekFailed(timestamp));
        }
        let frame =
            sample_to_frame_with_limits(&sample, Generation::FIRST, FrameSeq(1), &limits.frames)
                .map_err(|e| OfflineVideoError::Decode(e.to_string()))?;
        if frame.width() != width || frame.height() != height {
            return Err(OfflineVideoError::Decode(
                "output did not negotiate requested geometry".into(),
            ));
        }
        let raster = RenderedPreview::new(frame.width(), frame.height(), frame.into_pixels())
            .map_err(|e| OfflineVideoError::Decode(e.to_string()))?;
        if cancelled() {
            return Err(OfflineVideoError::Cancelled);
        }
        Ok(OfflineFrame {
            requested: timestamp,
            actual: Duration::from_nanos(actual.nseconds()),
            poster_fallback: fallback,
            raster,
        })
    }
}
pub(super) fn run(
    request: OfflineVideoRequest,
    cancelled: &dyn Fn() -> bool,
) -> Result<OfflineVideoResult, OfflineVideoError> {
    if request.limits.timeout.is_zero() {
        return Err(OfflineVideoError::InvalidRequest);
    }
    let (timestamps, policy, size) = match &request.operation {
        OfflineVideoOperation::Probe => (Vec::new(), FramePolicy::Poster, FrameSize::Source),
        OfflineVideoOperation::Frame {
            timestamp,
            policy,
            size,
        } => (vec![*timestamp], *policy, *size),
        OfflineVideoOperation::Filmstrip {
            timestamps,
            policy,
            size,
        } => {
            if timestamps.is_empty() || timestamps.len() > request.limits.max_frames {
                return Err(OfflineVideoError::InvalidRequest);
            }
            (timestamps.clone(), *policy, *size)
        }
    };
    if !timestamps.is_empty() && request.limits.max_frames == 0 {
        return Err(OfflineVideoError::InvalidRequest);
    }
    for timestamp in &timestamps {
        gst::ClockTime::try_from(*timestamp).map_err(|_| OfflineVideoError::InvalidRequest)?;
    }
    let session = Session::open(&request, cancelled)?;
    if !timestamps.is_empty() {
        let (width, height) = session.target(size, &request.limits.frames)?;
        let total = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|x| x.checked_mul(4))
            .and_then(|x| x.checked_mul(timestamps.len() as u64));
        if total.is_none_or(|bytes| bytes > request.limits.max_retained_rgba_bytes) {
            return Err(OfflineVideoError::InvalidRequest);
        }
    }
    let mut frames = Vec::new();
    let mut bytes = 0u64;
    for timestamp in timestamps {
        let frame = session.frame(timestamp, policy, size, &request.limits, cancelled)?;
        bytes = bytes
            .checked_add(frame.raster.pixels().len() as u64)
            .ok_or(OfflineVideoError::InvalidRequest)?;
        if bytes > request.limits.max_retained_rgba_bytes {
            return Err(OfflineVideoError::InvalidRequest);
        }
        frames.push(frame);
    }
    if cancelled() {
        return Err(OfflineVideoError::Cancelled);
    }
    Ok(OfflineVideoResult {
        generation: request.generation,
        probe: session.probe.clone(),
        frames,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) mod support {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/offline_video.rs"
        ));
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::tests::support;
    use super::*;
    use enzui_test_support::test_scratch::ScratchDir;

    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn offline_video_snapshot_rejects_source_mutation_during_capture() {
        let scratch = ScratchDir::new("offline-snapshot-mutation");
        let source = scratch.path().join("source");
        let root = scratch.path().join("snapshots");
        std::fs::write(&source, vec![73; 200000]).unwrap();
        let request = support::request(&source, &root, OfflineVideoOperation::Probe);
        let checks = AtomicUsize::new(0);
        let result = Snapshot::create(&request, &|| {
            if checks.fetch_add(1, Ordering::SeqCst) == 0 {
                std::fs::write(&source, vec![81; 200000]).unwrap();
            }
            false
        });
        assert!(matches!(result, Err(OfflineVideoError::SourceChanged)));
        assert_eq!(std::fs::read(&source).unwrap(), vec![81; 200000]);
        assert_eq!(std::fs::read_dir(root).unwrap().count(), 0);
    }
}
