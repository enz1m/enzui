#![allow(
    clippy::cast_possible_truncation,
    reason = "test fixtures write known small lengths"
)]

use super::*;
use crate::media::MediaSource;
use crate::media::gst_worker::{PreparedSource, configure_remote_source, ensure_initialised};
use crate::media::pinned_proxy::PinnedProxy;
use crate::media::remote_gate::test_support::{FakeCancel, FakeClock, FakeResolver, ScriptedProbe};
use crate::media::remote_gate::{GateEffects, MediaGate};
use crate::url_policy::UrlPolicy;
use enzui_test_support::test_scratch::ScratchDir;
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BODY_BYTES: u64 = 32 * 1024 * 1024;

struct Origin {
    root: ScratchDir,
    child: Child,
    port: u16,
}

impl Origin {
    fn start() -> Self {
        let root = ScratchDir::new_restricted("bounded-https-origin");
        let output = Command::new("openssl")
            .args([
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-days",
                "2",
                "-subj",
                "/CN=media.example.test",
                "-addext",
                "subjectAltName=DNS:media.example.test",
                "-keyout",
            ])
            .arg(root.join("key.pem"))
            .arg("-out")
            .arg(root.join("certificate.pem"))
            .output()
            .expect("OpenSSL is required for the private TLS fixture");
        assert!(
            output.status.success(),
            "private TLS certificate generation failed"
        );
        let error_log = std::fs::File::create(root.join("origin-stderr.log")).unwrap();
        let mut child = Command::new("python3")
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/bounded_https_origin.py"),
            )
            .arg(root.path())
            .arg(BODY_BYTES.to_string())
            .stdout(Stdio::null())
            .stderr(error_log)
            .spawn()
            .expect("Python TLS origin starts");
        let deadline = Instant::now() + Duration::from_secs(5);
        let port = loop {
            if let Ok(text) = std::fs::read_to_string(root.join("ready.json")) {
                let ready: serde_json::Value = serde_json::from_str(&text).unwrap();
                break u16::try_from(ready["port"].as_u64().unwrap()).unwrap();
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "private TLS origin exited before readiness"
            );
            assert!(
                Instant::now() < deadline,
                "private TLS origin readiness timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        Self { root, child, port }
    }

    fn stats(&self) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(self.root.join("origin-stats.json")).unwrap())
            .unwrap()
    }
}

impl Drop for Origin {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Transfer {
    source: gst::Element,
    sink: gst_app::AppSink,
    pipeline: Option<gst::Pipeline>,
    proxy: PinnedProxy,
    read_ahead: Option<u32>,
    anonymous_identity: Option<(u64, u64)>,
}

impl Transfer {
    fn start(origin: &Origin, read_ahead: Option<u32>, trust_fixture: bool) -> Self {
        Self::start_with_file(origin, read_ahead, trust_fixture, |file| file)
    }

    fn start_with_file(
        origin: &Origin,
        read_ahead: Option<u32>,
        trust_fixture: bool,
        backing: impl FnOnce(std::fs::File) -> std::fs::File,
    ) -> Self {
        ensure_initialised().unwrap();
        let gate = MediaGate {
            policy: UrlPolicy {
                allow_private_network: true,
                ..UrlPolicy::DEFAULT
            },
            ..MediaGate::DEFAULT
        };
        let uri = format!("https://media.example.test:{}/payload", origin.port);
        let source = MediaSource::remote(&uri, &gate.policy).unwrap();
        let prepared = PreparedSource::prepare(
            &source,
            &gate,
            &GateEffects {
                resolver: &FakeResolver::always(&["127.0.0.1"]),
                probe: Some(&ScriptedProbe::terminal()),
                clock: &FakeClock::frozen(),
                cancel: &FakeCancel::default(),
            },
        )
        .unwrap();
        let proxy = PinnedProxy::start(prepared.approved().unwrap()).unwrap();
        let pipeline_uri = crate::media::gst_worker::pipeline_uri(&prepared).unwrap();
        let source = gst::Element::make_from_uri(gst::URIType::Src, &pipeline_uri, None).unwrap();
        configure_remote_source(
            &source,
            &proxy.client_configuration(),
            prepared.approved().unwrap().limits(),
            &uri,
        )
        .unwrap();
        if trust_fixture {
            source.set_property(
                "ssl-ca-file",
                origin.root.join("certificate.pem").to_str().unwrap(),
            );
        }
        assert!(source.property::<bool>("ssl-strict"));
        let anonymous_identity = read_ahead.map(|capacity| {
            use std::os::fd::AsRawFd;
            use std::os::unix::fs::MetadataExt;
            assert!(
                source.find_property("read-ahead-capacity").is_some(),
                "the bounded source must support explicit disk read-ahead"
            );
            let file = enzui_security::secure_dir::SecureRoot::open(origin.root.path())
                .unwrap()
                .create_anonymous_file()
                .unwrap();
            rustix::fs::fallocate(
                &file,
                rustix::fs::FallocateFlags::empty(),
                0,
                u64::from(capacity),
            )
            .unwrap();
            let file = backing(file);
            let metadata = file.metadata().unwrap();
            assert_eq!(metadata.nlink(), 0);
            source.set_property("read-ahead-capacity", capacity);
            source.set_property("read-ahead-fd", file.as_raw_fd());
            assert!(source.property::<bool>("read-ahead-ready"));
            (metadata.dev(), metadata.ino())
        });
        let sink = gst_app::AppSink::builder()
            .sync(false)
            .max_buffers(1)
            .drop(false)
            .build();
        let pipeline = gst::Pipeline::new();
        pipeline.add_many([&source, sink.upcast_ref()]).unwrap();
        source.link(&sink).unwrap();
        pipeline.set_state(gst::State::Paused).unwrap();
        Self {
            source,
            sink,
            pipeline: Some(pipeline),
            proxy,
            read_ahead,
            anonymous_identity,
        }
    }

    fn wait_for_backpressure(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.source.property::<bool>("receive-paused") {
            assert!(
                self.source.property::<u64>("peak-buffered-bytes") <= RECEIVE_LIMIT,
                "native receive body exceeded the cap before backpressure"
            );
            if let Some(message) = self
                .pipeline
                .as_ref()
                .unwrap()
                .bus()
                .unwrap()
                .pop_filtered(&[gst::MessageType::Error])
            {
                panic!(
                    "TLS transfer failed before backpressure: {:?}",
                    message.type_()
                );
            }
            assert!(
                Instant::now() < deadline,
                "the native source never imposed backpressure"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let peak = self.source.property::<u64>("peak-buffered-bytes");
        assert!(
            peak <= RECEIVE_LIMIT && (self.read_ahead.is_some() || peak > 0),
            "native receive peak {peak} exceeds {RECEIVE_LIMIT}"
        );
        if let Some(capacity) = self.read_ahead {
            let retained = self.source.property::<u64>("read-ahead-buffered-bytes");
            assert!(
                retained > RECEIVE_LIMIT && retained <= u64::from(capacity),
                "disk read-ahead must fill beyond the fixed memory buffer"
            );
            assert_eq!(
                self.source.property::<u64>("read-ahead-file-bytes"),
                u64::from(capacity)
            );
            assert!(anonymous_descriptor_is_open(
                self.anonymous_identity.unwrap()
            ));
        }
    }

    fn stop(&mut self) {
        let Some(pipeline) = self.pipeline.take() else {
            return;
        };
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let owner = std::thread::spawn(move || {
            let result = pipeline.set_state(gst::State::Null);
            let _ = tx.send(result);
        });
        rx.recv_timeout(Duration::from_secs(2))
            .expect("a backpressured transfer must cancel promptly")
            .unwrap();
        owner.join().unwrap();
    }
}

impl Drop for Transfer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn check_sample(sample: &gst::Sample, expected_offset: u64) -> usize {
    let buffer = sample.buffer().unwrap();
    assert_eq!(
        buffer.offset(),
        expected_offset,
        "seeked output must begin at the requested byte"
    );
    let data = buffer.map_readable().unwrap();
    assert!(!data.is_empty());
    assert!(
        data.len() as u64 <= RECEIVE_LIMIT,
        "one output chunk must stay bounded in RAM"
    );
    for (index, &value) in data.iter().enumerate() {
        assert_eq!(
            value,
            expected_byte(expected_offset.wrapping_add(index as u64))
        );
    }
    data.len()
}

fn expected_byte(position: u64) -> u8 {
    ((position * 37) ^ (position >> 8) ^ (position >> 16) ^ (position >> 24)) as u8
}

fn anonymous_descriptor_is_open(identity: (u64, u64)) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::read_dir("/proc/self/fd").unwrap().any(|entry| {
        entry
            .ok()
            .and_then(|entry| entry.path().metadata().ok())
            .is_some_and(|metadata| (metadata.dev(), metadata.ino()) == identity)
    })
}

#[test]
fn bounded_https_backpressure_resumes_seeks_and_cancels() {
    let origin = Origin::start();
    for read_ahead in [None, Some(1024 * 1024 + 137)] {
        let mut transfer = Transfer::start(&origin, read_ahead, true);
        let window = read_ahead.map_or(RECEIVE_LIMIT, u64::from);
        assert!(
            (0..64).any(|index| expected_byte(index) != expected_byte(index + window)),
            "the fixture must distinguish an offset displaced by one FIFO capacity"
        );
        transfer.wait_for_backpressure();
        let preroll = transfer
            .sink
            .try_pull_preroll(gst::ClockTime::from_seconds(5))
            .expect("TLS preroll bytes");
        check_sample(&preroll, 0);
        transfer
            .pipeline
            .as_ref()
            .unwrap()
            .set_state(gst::State::Playing)
            .unwrap();
        let mut received = 0;
        while received < window * 4 {
            let sample = transfer
                .sink
                .try_pull_sample(gst::ClockTime::from_seconds(5))
                .expect("resume bytes beyond the native buffer");
            received += check_sample(&sample, received) as u64;
        }
        transfer
            .pipeline
            .as_ref()
            .unwrap()
            .set_state(gst::State::Paused)
            .unwrap();
        transfer.wait_for_backpressure();
        for target in [BODY_BYTES / 2 + 13, BODY_BYTES / 2 + 13, 13] {
            transfer
                .pipeline
                .as_ref()
                .unwrap()
                .seek_simple(gst::SeekFlags::FLUSH, gst::format::Bytes::from_u64(target))
                .unwrap();
            let sample = transfer
                .sink
                .try_pull_preroll(gst::ClockTime::from_seconds(5))
                .expect("seeked preroll bytes");
            check_sample(&sample, target);
            transfer
                .pipeline
                .as_ref()
                .unwrap()
                .set_state(gst::State::Playing)
                .unwrap();
            let mut received = 0;
            while received < window * 4 {
                let sample = transfer
                    .sink
                    .try_pull_sample(gst::ClockTime::from_seconds(5))
                    .expect("seeked stream must resume beyond the native buffer");
                received += check_sample(&sample, target + received) as u64;
            }
            transfer
                .pipeline
                .as_ref()
                .unwrap()
                .set_state(gst::State::Paused)
                .unwrap();
            transfer.wait_for_backpressure();
        }
        let peak = transfer.source.property::<u64>("peak-buffered-bytes");
        assert!(peak <= RECEIVE_LIMIT && (read_ahead.is_some() || peak > 0));
        transfer
            .pipeline
            .as_ref()
            .unwrap()
            .set_state(gst::State::Paused)
            .unwrap();
        transfer.wait_for_backpressure();
        transfer.stop();
        assert!(
            transfer.proxy.connections() >= 4,
            "seek must create a new authenticated traversal"
        );
        assert!(transfer.proxy.error().is_none());
        let stats = origin.stats();
        assert_eq!(stats["proxy_credentials_seen"], false);
        assert!(stats["requests"].as_array().unwrap().iter().any(|request| {
            request["range"]
                .as_str()
                .is_some_and(|range| range.starts_with(&format!("bytes={}-", BODY_BYTES / 2 + 13)))
        }));
        assert!(stats["requests"].as_array().unwrap().iter().all(|request| {
            request["tls"]
                .as_str()
                .is_some_and(|tls| tls.starts_with("TLSv1."))
                && request["version"] == "HTTP/1.1"
        }));
        let identity = transfer.anonymous_identity;
        drop(transfer);
        if let Some(identity) = identity {
            let deadline = Instant::now() + Duration::from_secs(2);
            while anonymous_descriptor_is_open(identity) {
                assert!(
                    Instant::now() < deadline,
                    "source teardown must release anonymous storage"
                );
                std::thread::yield_now();
            }
        }
    }
}

#[test]
fn bounded_https_disk_refuses_untrusted_tls_and_write_failure() {
    let origin = Origin::start();
    for read_ahead in [None, Some(1024 * 1024 + 137)] {
        let mut transfer = Transfer::start(&origin, read_ahead, false);
        let error = transfer
            .pipeline
            .as_ref()
            .unwrap()
            .bus()
            .unwrap()
            .timed_pop_filtered(gst::ClockTime::from_seconds(5), &[gst::MessageType::Error])
            .expect("untrusted TLS must fail promptly");
        assert!(matches!(error.view(), gst::MessageView::Error(_)));
        assert!(
            transfer
                .sink
                .try_pull_preroll(gst::ClockTime::ZERO)
                .is_none()
        );
        assert_eq!(transfer.source.property::<u64>("peak-buffered-bytes"), 0);
        assert_eq!(transfer.source.property::<u64>("read-ahead-peak-bytes"), 0);
        transfer.stop();
    }
    let capacity = 1024 * 1024 + 137;
    let mut transfer = Transfer::start_with_file(&origin, Some(capacity), true, |_| {
        let file = std::fs::File::from(
            rustix::fs::memfd_create(
                "read-ahead-write-refusal",
                rustix::fs::MemfdFlags::CLOEXEC | rustix::fs::MemfdFlags::ALLOW_SEALING,
            )
            .unwrap(),
        );
        rustix::fs::fchmod(&file, rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR).unwrap();
        file.set_len(u64::from(capacity)).unwrap();
        rustix::fs::fcntl_add_seals(&file, rustix::fs::SealFlags::WRITE).unwrap();
        file
    });
    let error = transfer
        .pipeline
        .as_ref()
        .unwrap()
        .bus()
        .unwrap()
        .timed_pop_filtered(gst::ClockTime::from_seconds(5), &[gst::MessageType::Error])
        .expect("native disk write refusal must fail promptly");
    let gst::MessageView::Error(error) = error.view() else {
        panic!("expected source error")
    };
    assert!(
        error
            .error()
            .message()
            .contains("anonymous read-ahead buffer could not be written")
    );
    assert!(
        transfer
            .sink
            .try_pull_preroll(gst::ClockTime::ZERO)
            .is_none()
    );
    assert_eq!(transfer.source.property::<u64>("read-ahead-peak-bytes"), 0);
    transfer.stop();
}

#[test]
fn bounded_https_disk_refuses_named_wrong_size_and_read_only_descriptors() {
    use std::os::fd::AsRawFd;
    let origin = Origin::start();
    ensure_initialised().unwrap();
    let source = gst::ElementFactory::make(FACTORY).build().unwrap();
    let capacity = 1024u32 * 1024;
    source.set_property("read-ahead-capacity", capacity);
    let root = enzui_security::secure_dir::SecureRoot::open(origin.root.path()).unwrap();
    let named = root
        .create_entry_private(std::ffi::OsStr::new("named-control"))
        .unwrap();
    named.set_len(u64::from(capacity)).unwrap();
    let named = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(origin.root.join("named-control"))
        .unwrap();
    source.set_property("read-ahead-fd", named.as_raw_fd());
    assert!(
        !source.property::<bool>("read-ahead-ready"),
        "named files must be refused"
    );
    let anonymous = root.create_anonymous_file().unwrap();
    anonymous.set_len(u64::from(capacity) - 1).unwrap();
    source.set_property("read-ahead-fd", anonymous.as_raw_fd());
    assert!(
        !source.property::<bool>("read-ahead-ready"),
        "wrong-size files must be refused"
    );
    anonymous.set_len(u64::from(capacity)).unwrap();
    let readonly = std::fs::File::open(format!("/proc/self/fd/{}", anonymous.as_raw_fd())).unwrap();
    source.set_property("read-ahead-fd", readonly.as_raw_fd());
    assert!(
        !source.property::<bool>("read-ahead-ready"),
        "read-only descriptors must be refused"
    );
    source.set_property("read-ahead-fd", anonymous.as_raw_fd());
    assert!(
        source.property::<bool>("read-ahead-ready"),
        "valid anonymous backing is the positive control"
    );
    source.set_property("read-ahead-fd", -1i32);
    assert!(!source.property::<bool>("read-ahead-ready"));
    source.set_property("location", "http://127.0.0.1:9/refused");
    let pipeline = gst::Pipeline::new();
    let sink = gst::ElementFactory::make("fakesink").build().unwrap();
    pipeline.add_many([&source, &sink]).unwrap();
    source.link(&sink).unwrap();
    let _ = pipeline.set_state(gst::State::Paused);
    let error = pipeline
        .bus()
        .unwrap()
        .timed_pop_filtered(gst::ClockTime::from_seconds(5), &[gst::MessageType::Error])
        .expect("partial disk configuration must fail before transport");
    let gst::MessageView::Error(error) = error.view() else {
        panic!("expected settings error")
    };
    assert!(
        error
            .error()
            .message()
            .contains("anonymous read-ahead buffer is not configured")
    );
    pipeline.set_state(gst::State::Null).unwrap();
}
