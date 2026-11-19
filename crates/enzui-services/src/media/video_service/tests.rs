//! The service against real GStreamer, on media each test encodes itself.

use super::*;
use crate::media::PlaybackState;
use gstreamer as gst;
use gstreamer::prelude::*;
use std::path::Path;
use std::time::Instant;

// Encoding failure returns None and skips the caller, including errors beyond missing encoders.
fn encode(dir: &Path) -> Option<PathBuf> {
    encode_with(dir, "smpte", 3)
}

// Sparse cues leave gaps in which track switching must not stall playback.
fn encode_with(dir: &Path, pattern: &str, seconds: u32) -> Option<PathBuf> {
    gst::init().ok()?;
    let path = dir.join("clip.mkv");
    let subtitles = dir.join("embedded.srt");
    let stamp = |ms: u32| format!("00:00:{:02},{:03}", ms / 1000, ms % 1000);
    let cues: String = (0..seconds.div_ceil(3))
        .map(|cue| {
            format!(
                "{}\n{} --> {}\nEmbedded\n\n",
                cue + 1,
                stamp(cue * 3000),
                stamp(cue * 3000 + 1500)
            )
        })
        .collect();
    std::fs::write(&subtitles, cues).ok()?;
    let description = format!(
        "matroskamux name=mux ! filesink location={path} \
         filesrc location={subtitles} ! subparse \
           ! taginject tags=\"language-code=en\" ! mux. \
         videotestsrc pattern={pattern} num-buffers={frames} \
           ! video/x-raw,width=320,height=240,framerate=30/1 \
           ! vp8enc deadline=1 ! mux. \
         audiotestsrc num-buffers={audio} ! audioconvert \
           ! taginject tags=\"language-code=en\" ! vorbisenc ! mux. \
         audiotestsrc num-buffers={audio} freq=660 ! audioconvert \
           ! taginject tags=\"language-code=hu\" ! vorbisenc ! mux.",
        path = path.display(),
        subtitles = subtitles.display(),
        frames = seconds * 30,
        audio = seconds * 44 + 2,
    );
    let pipeline = gst::parse::launch(&description).ok()?;
    pipeline.set_state(gst::State::Playing).ok()?;
    let bus = pipeline.bus()?;
    let done = bus.timed_pop_filtered(
        gst::ClockTime::from_seconds(30),
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    let _ = pipeline.set_state(gst::State::Null);
    matches!(done.map(|m| m.type_()), Some(gst::MessageType::Eos)).then_some(path)
}

struct Harness {
    service: VideoService,
    status: VideoStatus,
    frame: Option<Rgba8Frame>,
    frames: usize,
}

impl Harness {
    fn new() -> Self {
        Self {
            service: VideoService::new(VideoServiceConfig::default()).expect("service"),
            status: VideoStatus::default(),
            frame: None,
            frames: 0,
        }
    }

    fn until(&mut self, what: &str, done: impl Fn(&Self) -> bool) {
        let started = Instant::now();
        loop {
            let update = self.service.poll();
            if let Some(status) = update.status {
                self.status = status;
            }
            if let Some(frame) = update.frame {
                self.frames += 1;
                self.frame = Some(frame);
            }
            if done(self) {
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "timed out waiting for {what}; status {:#?}",
                self.status
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn quiet() -> LoadOptions {
    LoadOptions {
        muted: true,
        autoplay: false,
        ..LoadOptions::default()
    }
}

struct Scratch(PathBuf);

impl Scratch {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn clip() -> Option<(Scratch, PathBuf)> {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("enzui-video-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let dir = Scratch(dir);
    let path = encode(dir.path())?;
    Some((dir, path))
}

#[test]
fn a_subtitle_file_reopens_at_the_same_position() {
    let Some((dir, path)) = clip() else {
        return;
    };
    let subtitles = dir.path().join("clip.srt");
    std::fs::write(&subtitles, "1\n00:00:00,000 --> 00:00:03,000\nHello\n").expect("srt");
    let mut h = Harness::new();
    h.service.load(MediaSource::local(&path), quiet());
    h.until("ready", |h| !h.status.loading);
    h.service
        .seek(Duration::from_millis(1000), SeekMode::Accurate);
    h.until("seek", |h| {
        h.status.position.abs_diff(Duration::from_millis(1000)) < Duration::from_millis(60)
    });
    h.service.set_subtitle_file(Some(subtitles.clone()));
    h.until("subtitle", |h| {
        h.status.subtitle_file.as_deref() == Some(subtitles.as_path()) && !h.status.loading
    });
    h.until("position kept", |h| {
        h.status.position.abs_diff(Duration::from_millis(1000)) < Duration::from_millis(100)
    });
    assert_eq!(h.status.playback, PlaybackState::Paused);
}

fn text_pixels(frame: &Rgba8Frame) -> usize {
    frame
        .pixels()
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[..3].iter().all(|channel| *channel > 180))
        .count()
}

#[test]
fn an_audio_switch_while_paused_applies_without_playing() {
    let dir = {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("enzui-video-paused-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir)
    };
    let Some(path) = encode_with(dir.path(), "black", 3) else {
        return;
    };
    let mut h = Harness::new();
    h.service.load(
        MediaSource::local(&path),
        LoadOptions {
            autoplay: true,
            ..quiet()
        },
    );
    h.until("playing", |h| {
        h.status.is_playing() && h.status.audio_tracks.len() == 2
    });
    h.service.pause();
    h.until("paused", |h| h.status.playback == PlaybackState::Paused);
    h.service.seek(Duration::ZERO, SeekMode::Accurate);
    h.until("back at the start", |h| {
        !h.status.seeking && h.status.position < Duration::from_millis(100)
    });
    h.service.select_audio(1);
    h.until("audio switched while paused", |h| {
        h.status.current_audio == Some(1) && !h.status.seeking
    });
    assert_eq!(
        h.status.playback,
        PlaybackState::Paused,
        "the switch keeps it paused"
    );
    assert!(
        h.status.position < Duration::from_millis(200),
        "the switch stays at the paused frame"
    );
}

#[test]
fn track_switches_keep_playing_and_show_or_hide_subtitles() {
    let dir = {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("enzui-video-text-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir)
    };
    let Some(path) = encode_with(dir.path(), "black", 12) else {
        return;
    };
    let mut h = Harness::new();
    h.service.load(
        MediaSource::local(&path),
        LoadOptions {
            autoplay: true,
            ..quiet()
        },
    );
    h.until("playing", |h| {
        h.status.is_playing() && h.status.position > Duration::from_millis(1700)
    });
    let mut seeked = false;
    let mut shown = |h: &mut Harness, visible: bool, what: &str| {
        let started = Instant::now();
        let mut text_frames = 0;
        let mut frames = 0;
        let mut last_frame = Instant::now();
        loop {
            let update = h.service.poll();
            if let Some(status) = update.status {
                seeked |= status.seeking;
                h.status = status;
            }
            if update.frame.is_some() {
                last_frame = Instant::now();
            }
            assert!(
                last_frame.elapsed() < Duration::from_millis(700),
                "{what}: video stalled"
            );
            if let Some(frame) = update.frame
                && started.elapsed() > Duration::from_millis(300)
            {
                frames += 1;
                if text_pixels(&frame) > 20 {
                    text_frames += 1;
                }
            }
            if visible && text_frames > 0 {
                return;
            }
            if !visible && started.elapsed() > Duration::from_millis(1300) {
                assert!(frames > 10, "{what}: playback kept running");
                assert_eq!(text_frames, 0, "{what}: subtitles stayed hidden");
                return;
            }
            assert!(
                started.elapsed() < Duration::from_secs(4),
                "{what}: subtitles never appeared"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    shown(&mut h, false, "before choosing subtitles");
    h.service.select_subtitle(Some(0));
    shown(&mut h, true, "subtitles on");
    assert_eq!(h.status.current_subtitle, Some(0));
    h.service.select_subtitle(None);
    shown(&mut h, false, "subtitles off");
    assert_eq!(h.status.current_subtitle, None);
    h.service.select_subtitle(Some(0));
    shown(&mut h, true, "subtitles on again");
    assert!(!seeked, "subtitle switches must not seek");
    h.service.select_audio(1);
    h.until("audio switched", |h| h.status.current_audio == Some(1));
    let switched = h.status.position;
    h.until("still playing", |h| {
        !h.status.seeking && h.status.position > switched + Duration::from_millis(300)
    });
    let Harness { service, .. } = h;
    assert!(
        service.shutdown(Duration::from_secs(5)),
        "teardown finished before shutdown returned"
    );
}

#[test]
fn a_resize_burst_keeps_frames_flowing_and_lands_on_the_final_size() {
    assert_eq!(
        super::worker::fitted((320, 240), Some((1920, 1080))),
        (320, 240)
    );
    assert_eq!(
        super::worker::fitted((1920, 1080), Some((960, 1080))),
        (960, 540)
    );
    assert_eq!(super::worker::fitted((1920, 800), None), (1920, 800));
    let Some((_dir, path)) = clip() else {
        return;
    };
    let mut h = Harness::new();
    h.service.load(
        MediaSource::local(&path),
        LoadOptions {
            autoplay: true,
            ..quiet()
        },
    );
    h.until("playing", |h| h.status.is_playing() && h.frame.is_some());
    let mut last_frame = Instant::now();
    for step in 0..20u32 {
        h.service
            .set_frame_limit(Some((320 - step * 8, 240 - step * 6)));
        std::thread::sleep(Duration::from_millis(10));
        if h.service.poll().frame.is_some() {
            last_frame = Instant::now();
        }
        assert!(
            last_frame.elapsed() < Duration::from_millis(500),
            "frames stopped during the resize"
        );
    }
    h.until("final size", |h| {
        h.frame
            .as_ref()
            .is_some_and(|frame| (frame.width(), frame.height()) == (168, 126))
    });
    let Harness { service, .. } = h;
    assert!(service.shutdown(Duration::from_secs(5)));
}

mod controls;

mod seek_outcomes;

mod remote_subtitles;
