//! Network playback shares the pipeline with an explicitly requested local subtitle source.

use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Origin {
    address: std::net::SocketAddr,
    stopped: Arc<AtomicBool>,
    requests: Arc<AtomicUsize>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Origin {
    fn new(bytes: Vec<u8>) -> Self {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).expect("origin");
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(AtomicUsize::new(0));
        let stop = Arc::clone(&stopped);
        let count = Arc::clone(&requests);
        let worker = std::thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(1)))
                            .unwrap();
                        let _ = serve(&mut stream, &bytes);
                        count.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept: {error}"),
                }
            }
        });
        Self {
            address,
            stopped,
            requests,
            worker: Some(worker),
        }
    }
}

impl Drop for Origin {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}

fn serve(stream: &mut TcpStream, bytes: &[u8]) -> std::io::Result<()> {
    let mut headers = Vec::new();
    while !headers.ends_with(b"\r\n\r\n") && headers.len() < 16_384 {
        let mut byte = [0];
        stream.read_exact(&mut byte)?;
        headers.push(byte[0]);
    }
    let request = String::from_utf8_lossy(&headers);
    let range = request.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case("range").then_some(value.trim())
    });
    let start = range
        .and_then(|r| r.strip_prefix("bytes="))
        .and_then(|r| r.split_once('-'))
        .and_then(|(start, _)| start.parse::<usize>().ok())
        .unwrap_or(0);
    if start >= bytes.len() {
        write!(
            stream,
            "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )?;
        return Ok(());
    }
    let status = if range.is_some() {
        "206 Partial Content"
    } else {
        "200 OK"
    };
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: video/x-matroska\r\nAccept-Ranges: bytes\r\nContent-Length: {}\r\nConnection: close\r\n",
        bytes.len() - start
    )?;
    if range.is_some() {
        write!(
            stream,
            "Content-Range: bytes {start}-{}/{}\r\n",
            bytes.len() - 1,
            bytes.len()
        )?;
    }
    stream.write_all(b"\r\n")?;
    if !request.starts_with("HEAD ") {
        stream.write_all(&bytes[start..])?;
    }
    Ok(())
}

fn video_only(dir: &Scratch) -> PathBuf {
    gst::init().expect("GStreamer");
    let path = dir.path().join("silent.mkv");
    let pipeline = gst::parse::launch(&format!(
        "videotestsrc pattern=black num-buffers=90 ! video/x-raw,width=320,height=240,framerate=30/1 ! vp8enc deadline=1 ! matroskamux ! filesink location={}", path.display()
    )).expect("video fixture encoders");
    pipeline.set_state(gst::State::Playing).expect("encode");
    let done = pipeline.bus().unwrap().timed_pop_filtered(
        gst::ClockTime::from_seconds(10),
        &[gst::MessageType::Eos, gst::MessageType::Error],
    );
    pipeline.set_state(gst::State::Null).expect("stop encoder");
    assert_eq!(
        done.map(|m| m.type_()),
        Some(gst::MessageType::Eos),
        "fixture must encode"
    );
    path
}

#[test]
fn remote_video_with_local_subtitles_decodes_and_replaces_sources() {
    use enzui_core::url_policy::{RemoteSource, UrlPolicy};
    use std::os::fd::AsRawFd;
    let path = std::env::temp_dir().join(format!("enzui-remote-subtitles-{}", std::process::id()));
    std::fs::create_dir(&path).expect("private test directory");
    let dir = Scratch(path);
    let video = video_only(&dir);
    let subtitles = dir.path().join("external.srt");
    std::fs::write(&subtitles, "1\n00:00:00,000 --> 00:00:03,000\nHELLO\n").unwrap();
    let subtitle_fd = std::fs::File::open(&subtitles).unwrap();
    let anonymous = PathBuf::from(format!("/proc/self/fd/{}", subtitle_fd.as_raw_fd()));
    let origin = Origin::new(std::fs::read(&video).unwrap());
    let policy = UrlPolicy {
        allow_http: true,
        allow_private_network: true,
        max_redirects: 0,
    };
    let remote = MediaSource::Remote(
        RemoteSource::checked(&format!("http://{}/silent.mkv", origin.address), &policy).unwrap(),
    );
    let mut h = Harness {
        service: VideoService::new(VideoServiceConfig {
            gate: MediaGate {
                policy,
                ..MediaGate::DEFAULT
            },
            preroll_timeout: Duration::from_secs(3),
            ..VideoServiceConfig::default()
        })
        .unwrap(),
        status: VideoStatus::default(),
        frame: None,
        frames: 0,
    };
    for (source, subtitle) in [
        (MediaSource::local(&video), Some(subtitles.clone())),
        (remote.clone(), None),
        (remote.clone(), Some(subtitles)),
        (remote.clone(), Some(anonymous.clone())),
        (MediaSource::local(&video), None),
    ] {
        if subtitle.as_ref() == Some(&anonymous) {
            std::fs::remove_file(dir.path().join("external.srt")).unwrap();
        }
        h.frames = 0;
        h.frame = None;
        let with_subtitle = subtitle.is_some();
        let generation = h.service.load(
            source,
            LoadOptions {
                muted: true,
                subtitle_file: subtitle,
                ..LoadOptions::default()
            },
        );
        h.until(
            &format!("decoded replacement with subtitle={with_subtitle}"),
            |h| h.status.generation == generation && !h.status.loading && h.frames >= 5,
        );
        assert!(
            h.status.error.is_none(),
            "playback failed: {:?}",
            h.status.error
        );
        let frame = h.frame.as_ref().expect("decoded frame");
        let bright = frame
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[0] > 160 && p[1] > 160 && p[2] > 160)
            .count();
        if with_subtitle {
            assert!(
                bright > 50,
                "external subtitle must reach video pixels: {bright}"
            );
        } else {
            assert_eq!(bright, 0, "plain black-video control");
        }
        h.service.unload();
    }
    h.frames = 0;
    let failed_generation = h.service.load(
        remote,
        LoadOptions {
            muted: true,
            subtitle_file: Some(dir.path().join("missing.srt")),
            ..LoadOptions::default()
        },
    );
    h.until("missing subtitle has a bounded outcome", |h| {
        h.status.generation == failed_generation
            && (h.status.error.is_some() || (!h.status.loading && h.frames >= 5))
    });
    h.frames = 0;
    let replacement = h.service.load(
        MediaSource::local(&video),
        LoadOptions {
            muted: true,
            ..LoadOptions::default()
        },
    );
    h.until("replacement after missing subtitle", |h| {
        h.status.generation == replacement && !h.status.loading && h.frames >= 5
    });
    h.service.unload();
    assert!(
        origin.requests.load(Ordering::Relaxed) >= 6,
        "remote loads must reach the origin"
    );
}

fn anonymous_files(directory: &Path) -> Vec<std::fs::Metadata> {
    use std::os::unix::fs::MetadataExt;
    std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let target = std::fs::read_link(&path).ok()?;
            let metadata = path.metadata().ok()?;
            (target.starts_with(directory) && metadata.is_file() && metadata.nlink() == 0)
                .then_some(metadata)
        })
        .collect()
}

fn wait_storage_closed(directory: &Path) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !anonymous_files(directory).is_empty() {
        assert!(
            Instant::now() < deadline,
            "pipeline release must reclaim anonymous storage"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn disk_read_ahead_video_decodes_seeks_replaces_and_releases_storage() {
    use enzui_core::url_policy::UrlPolicy;
    use enzui_test_support::test_scratch::ScratchDir;
    use std::os::unix::fs::MetadataExt;
    let scratch = ScratchDir::new_restricted("disk-read-ahead-video");
    let dir = Scratch(scratch.join("fixture"));
    std::fs::create_dir(dir.path()).unwrap();
    let video = video_only(&dir);
    let origin = Origin::new(std::fs::read(&video).unwrap());
    let policy = UrlPolicy {
        allow_http: true,
        allow_private_network: true,
        max_redirects: 0,
    };
    let remote = MediaSource::remote(
        &format!(
            "http://{}/silent.mkv?opaque=https://example.invalid",
            origin.address
        ),
        &policy,
    )
    .unwrap();
    let capacity = 1024 * 1024 + 137;
    let config = VideoServiceConfig {
        gate: MediaGate {
            policy,
            ..MediaGate::DEFAULT
        },
        disk_read_ahead: Some(DiskReadAhead {
            capacity,
            min_free_bytes: 1024 * 1024,
        }),
        buffering: VideoBuffering {
            duration: Duration::ZERO,
            low_watermark: 0.1,
            high_watermark: 0.75,
        },
        ..VideoServiceConfig::default()
    };
    let storage = scratch.join("storage");
    let root = enzui_security::secure_dir::SecureRoot::open(&storage).unwrap();
    root.create_entry_private(std::ffi::OsStr::new("control"))
        .unwrap();
    let mut h = Harness {
        service: VideoService::new_with_read_ahead_directory(config, storage.clone()).unwrap(),
        status: VideoStatus::default(),
        frame: None,
        frames: 0,
    };
    let generation = h.service.load(remote.clone(), quiet());
    h.until("disk-backed remote preroll", |h| {
        h.status.generation == generation
            && !h.status.loading
            && h.frame.is_some()
            && h.status.read_ahead.is_some()
    });
    assert!(h.status.error.is_none(), "{:?}", h.status.error);
    let observed = h.status.read_ahead.unwrap();
    assert_eq!(observed.capacity, capacity);
    assert_eq!(observed.reserved_bytes, u64::from(capacity));
    assert!(observed.peak_buffered_bytes > 0);
    let files = anonymous_files(&storage);
    assert!(
        !files.is_empty(),
        "storage must actually be owned while playing"
    );
    assert!(files.iter().all(|m| m.len() == u64::from(capacity)
        && m.blocks() * 512 >= u64::from(capacity)
        && m.mode() & 0o777 == 0o600));
    assert_eq!(root.entry_names().unwrap().len(), 1);
    for target in [Duration::from_secs(2), Duration::from_millis(200)] {
        h.frame = None;
        h.service.request_seek(target, SeekMode::Accurate).unwrap();
        let started = Instant::now();
        let outcome = loop {
            h.service.poll();
            if let Some(outcome) = h.service.take_seek_outcomes().into_iter().next() {
                break outcome;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "disk seek must complete"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(outcome.result, Ok(()), "{outcome:?}");
    }
    let local = h.service.load(MediaSource::local(&video), quiet());
    h.frame = None;
    h.until("local playback without disk retention", |h| {
        h.status.generation == local && !h.status.loading && h.frame.is_some()
    });
    wait_storage_closed(&storage);
    assert!(h.status.read_ahead.is_none());
    assert!(anonymous_files(&storage).is_empty());
    h.frame = None;
    let replacement = h.service.load(remote.clone(), quiet());
    h.until("replacement disk source", |h| {
        h.status.generation == replacement
            && !h.status.loading
            && h.frame.is_some()
            && h.status.read_ahead.is_some()
    });
    assert!(!anonymous_files(&storage).is_empty());
    h.service.unload();
    wait_storage_closed(&storage);
    h.frame = None;
    let final_generation = h.service.load(remote, quiet());
    h.until("disk source before shutdown", |h| {
        h.status.generation == final_generation
            && !h.status.loading
            && h.frame.is_some()
            && h.status.read_ahead.is_some()
    });
    assert!(!anonymous_files(&storage).is_empty());
    drop(h);
    wait_storage_closed(&storage);
    assert_eq!(root.entry_names().unwrap().len(), 1);

    let mut rejected = Harness {
        service: VideoService::new_with_read_ahead_directory(
            VideoServiceConfig {
                disk_read_ahead: Some(DiskReadAhead {
                    capacity,
                    min_free_bytes: u64::MAX - u64::from(capacity),
                }),
                ..config
            },
            storage.clone(),
        )
        .unwrap(),
        status: VideoStatus::default(),
        frame: None,
        frames: 0,
    };
    rejected.service.load(
        MediaSource::remote(&format!("http://{}/silent.mkv", origin.address), &policy).unwrap(),
        quiet(),
    );
    rejected.until("free-space refusal", |h| h.status.error.is_some());
    assert!(
        rejected
            .status
            .error
            .unwrap()
            .message
            .contains("insufficient free space")
    );
    assert!(
        rejected.frame.is_none(),
        "reservation refusal cannot fall back to decoding"
    );
    assert!(anonymous_files(&storage).is_empty());
    assert_eq!(root.entry_names().unwrap().len(), 1);
}

#[test]
fn disk_read_ahead_buffering_reaches_live_decoder_queues() {
    crate::media::gst_worker::ensure_initialised().unwrap();
    use enzui_core::url_policy::UrlPolicy;
    use enzui_test_support::test_scratch::ScratchDir;
    let scratch = ScratchDir::new_restricted("disk-buffering");
    let dir = Scratch(scratch.join("fixture"));
    std::fs::create_dir(dir.path()).unwrap();
    let video = video_only(&dir);
    let origin = Origin::new(std::fs::read(&video).unwrap());
    let policy = UrlPolicy {
        allow_http: true,
        allow_private_network: true,
        max_redirects: 0,
    };
    let source =
        MediaSource::remote(&format!("http://{}/silent.mkv", origin.address), &policy).unwrap();
    for duration in [Duration::ZERO, Duration::from_millis(350)] {
        let config = VideoServiceConfig {
            gate: MediaGate {
                policy,
                ..MediaGate::DEFAULT
            },
            buffering: VideoBuffering {
                duration,
                low_watermark: 0.125,
                high_watermark: 0.875,
            },
            disk_read_ahead: Some(DiskReadAhead {
                capacity: 1024 * 1024,
                min_free_bytes: 0,
            }),
            ..VideoServiceConfig::default()
        };
        let shared = Arc::new(Shared::default());
        let (sender, _inbox) = std::sync::mpsc::sync_channel(VIDEO_COMMAND_CAPACITY);
        let mut worker = super::super::worker::Worker::new(config, shared, sender);
        worker.read_ahead_directory = Some(scratch.join("storage"));
        let built = worker.open(Generation::FIRST, &source, None).unwrap();
        let (result, _, _) = built.playbin.state(gst::ClockTime::from_seconds(5));
        assert!(result.is_ok(), "real playbin must preroll");
        assert_eq!(built.playbin.property::<u64>("ring-buffer-max-size"), 0);
        assert_eq!(
            built.playbin.property::<i64>("buffer-duration"),
            i64::try_from(duration.as_nanos()).unwrap()
        );
        let bin = built.playbin.clone().downcast::<gst::Bin>().unwrap();
        let mut queues = 0;
        for element in bin.iterate_recurse().into_iter().map(Result::unwrap) {
            if element
                .factory()
                .is_some_and(|factory| factory.name() == "urisourcebin")
            {
                queues += 1;
                assert_eq!(
                    element.property::<i64>("buffer-duration"),
                    i64::try_from(duration.as_nanos()).unwrap()
                );
                assert_eq!(element.property::<f64>("low-watermark"), 0.125);
                assert_eq!(element.property::<f64>("high-watermark"), 0.875);
                assert_eq!(element.property::<u64>("ring-buffer-max-size"), 0);
                assert!(!element.property::<bool>("download"));
                let source_bin = element.clone().downcast::<gst::Bin>().unwrap();
                let mut network_queues = 0;
                for queue in source_bin.iterate_recurse().into_iter().map(Result::unwrap) {
                    if queue
                        .factory()
                        .is_some_and(|factory| factory.name() == "multiqueue")
                    {
                        network_queues += 1;
                        assert!(
                            queue.property::<bool>("use-buffering"),
                            "private URI source must be recognized as network transport"
                        );
                        assert_eq!(
                            queue.property::<u64>("min-interleave-time"),
                            u64::try_from(duration.as_nanos()).unwrap()
                        );
                        assert_eq!(queue.property::<f64>("low-watermark"), 0.125);
                        assert_eq!(queue.property::<f64>("high-watermark"), 0.875);
                    }
                }
                assert!(
                    network_queues > 0,
                    "inspect actual network multiqueue settings"
                );
            }
        }
        assert!(
            queues > 0,
            "decoder queue inspection must have a positive population"
        );
        built.playbin.set_state(gst::State::Null).unwrap();
        if let Some(bus) = built.playbin.bus() {
            bus.unset_sync_handler();
        }
        drop(bin);
        drop(built);
        wait_storage_closed(&scratch.join("storage"));
    }
}

#[test]
fn disk_read_ahead_crash_leaves_no_named_media() {
    use enzui_core::url_policy::UrlPolicy;
    use enzui_test_support::test_scratch::ScratchDir;
    use std::process::{Command, Stdio};
    let policy = UrlPolicy {
        allow_http: true,
        allow_private_network: true,
        max_redirects: 0,
    };
    if let Some(directory) = std::env::var_os("ENZUI_READ_AHEAD_CRASH_FIXTURE") {
        let directory = PathBuf::from(directory);
        let mut h = Harness {
            service: VideoService::new_with_read_ahead_directory(
                VideoServiceConfig {
                    gate: MediaGate {
                        policy,
                        ..MediaGate::DEFAULT
                    },
                    disk_read_ahead: Some(DiskReadAhead {
                        capacity: 1024 * 1024,
                        min_free_bytes: 0,
                    }),
                    ..VideoServiceConfig::default()
                },
                directory.join("storage"),
            )
            .unwrap(),
            status: VideoStatus::default(),
            frame: None,
            frames: 0,
        };
        h.service.load(
            MediaSource::remote(
                &std::env::var("ENZUI_READ_AHEAD_CRASH_URI").unwrap(),
                &policy,
            )
            .unwrap(),
            quiet(),
        );
        h.until("crash fixture adoption", |h| {
            h.status.read_ahead.is_some() && h.frame.is_some()
        });
        assert!(!anonymous_files(&directory.join("storage")).is_empty());
        std::fs::write(directory.join("ready"), b"ready").unwrap();
        loop {
            std::thread::park_timeout(Duration::from_millis(100));
        }
    }
    let scratch = ScratchDir::new_restricted("disk-read-ahead-crash");
    let dir = Scratch(scratch.join("fixture"));
    std::fs::create_dir(dir.path()).unwrap();
    let video = video_only(&dir);
    let origin = Origin::new(std::fs::read(&video).unwrap());
    let storage = scratch.join("storage");
    let root = enzui_security::secure_dir::SecureRoot::open(&storage).unwrap();
    root.create_entry_private(std::ffi::OsStr::new("control"))
        .unwrap();
    let name = format!(
        "{}::disk_read_ahead_crash_leaves_no_named_media",
        module_path!().strip_prefix("enzui_services::").unwrap()
    );
    struct CrashProcess(std::process::Child);
    impl Drop for CrashProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &name, "--nocapture"])
        .env("ENZUI_READ_AHEAD_CRASH_FIXTURE", scratch.path())
        .env(
            "ENZUI_READ_AHEAD_CRASH_URI",
            format!("http://{}/silent.mkv", origin.address),
        )
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(scratch.join("child-stderr.log")).unwrap())
        .spawn()
        .unwrap();
    let mut child = CrashProcess(child);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !scratch.join("ready").exists() {
        if child.0.try_wait().unwrap().is_some() || Instant::now() >= deadline {
            let _ = child.0.kill();
            let _ = child.0.wait();
            panic!(
                "crash fixture did not adopt storage: {}",
                std::fs::read_to_string(scratch.join("child-stderr.log")).unwrap()
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let descriptors = std::fs::read_dir(format!("/proc/{}/fd", child.0.id()))
        .unwrap()
        .filter_map(Result::ok)
        .filter_map(|entry| std::fs::read_link(entry.path()).ok())
        .filter(|path| path.starts_with(&storage))
        .count();
    assert!(
        descriptors >= 2,
        "Rust reservation and native duplicate must both be live"
    );
    assert_eq!(root.entry_names().unwrap().len(), 1);
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    assert_eq!(
        root.entry_names().unwrap().len(),
        1,
        "abrupt death must leave no named media"
    );
    assert!(storage.join("control").exists());
}
