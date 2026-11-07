//! The service against real GStreamer, on media each test encodes itself.

use super::*;
use crate::media::PlaybackState;
use gstreamer as gst;
use gstreamer::prelude::*;
use std::path::Path;
use std::time::Instant;

// Encoding failure returns None and skips the caller, including errors beyond missing encoders.
fn encode(dir: &Path) -> Option<PathBuf> {
    gst::init().ok()?;
    let path = dir.join("clip.mkv");
    let subtitles = dir.join("embedded.srt");
    std::fs::write(&subtitles, "1\n00:00:00,000 --> 00:00:03,000\nEmbedded\n").ok()?;
    let description = format!(
        "matroskamux name=mux ! filesink location={path} \
         filesrc location={subtitles} ! subparse \
           ! taginject tags=\"language-code=en\" ! mux. \
         videotestsrc num-buffers=90 ! video/x-raw,width=320,height=240,framerate=30/1 \
           ! vp8enc deadline=1 ! mux. \
         audiotestsrc num-buffers=130 ! audioconvert \
           ! taginject tags=\"language-code=en\" ! vorbisenc ! mux. \
         audiotestsrc num-buffers=130 freq=660 ! audioconvert \
           ! taginject tags=\"language-code=hu\" ! vorbisenc ! mux.",
        path = path.display(),
        subtitles = subtitles.display(),
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

mod seek_outcomes;

mod remote_subtitles;
