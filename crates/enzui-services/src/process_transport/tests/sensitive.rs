use super::*;
use enzui_security::secret::observer;

fn wiped_at_least(minimum: usize) {
    let actual = observer::snapshot();
    assert!(
        actual.wiped_bytes >= minimum,
        "missing wipe custody: {actual:?}, expected at least {minimum}"
    );
    assert_eq!(
        actual.zeroed_bytes, actual.wiped_bytes,
        "observed nonzero bytes after wipe"
    );
}

#[test]
fn sensitive_requests_wipe_input_arguments_and_environment_even_before_spawn() {
    for execute in [false, true] {
        let request = ProcessRequest::new(
            "/bin/cat",
            StdinPolicy::Bytes(b"input-secret".to_vec()),
            64,
            64,
            Duration::from_secs(1),
        )
        .arg("argument-secret")
        .env("SYNTHETIC_TOKEN", "environment-secret");
        assert!(
            !format!("{request:?}").contains("secret"),
            "request Debug must redact values"
        );
        observer::reset();
        if execute {
            let cancel = CancelToken::new();
            cancel.cancel(CancelCause::Cancelled);
            assert!(matches!(
                run_sensitive(request, &cancel).unwrap_err().kind(),
                ProcessErrorKind::Cancelled(_)
            ));
        } else {
            drop(request);
        }
        wiped_at_least(b"input-secretargument-secretenvironment-secret".len());
    }
}

#[test]
fn sensitive_success_and_nonzero_exit_keep_both_streams_in_wiping_custody() {
    for code in [0, 7] {
        let request = ProcessRequest::new(
            "/bin/sh",
            StdinPolicy::Bytes(b"input-secret".to_vec()),
            64,
            64,
            Duration::from_secs(2),
        )
        .args([
            "-c",
            &format!("/bin/cat; printf '%s' \"$SYNTHETIC_TOKEN\" >&2; exit {code}"),
        ])
        .env("SYNTHETIC_TOKEN", "environment-secret");
        let output = run_sensitive(request, &CancelToken::new()).expect("complete output");
        assert_eq!(output.status.code(), Some(code));
        assert_eq!(output.stdout.expose_secret(), b"input-secret");
        assert_eq!(output.stderr.expose_secret(), b"environment-secret");
        assert!(
            !format!("{output:?}").contains("secret"),
            "output Debug must redact values"
        );
        observer::reset();
        drop(output);
        wiped_at_least(b"input-secretenvironment-secret".len());
    }
}

#[test]
fn sensitive_failure_captures_wipe_after_overflow_deadline_and_cancellation() {
    for scenario in ["overflow", "deadline", "cancel"] {
        let token = CancelToken::new();
        let request = match scenario {
            "overflow" => sh("printf 'partial-secret'", 3, 64, Duration::from_secs(2)),
            _ => sh(
                "printf 'partial-secret'; printf 'error-secret' >&2; exec /bin/sleep 5",
                64,
                64,
                Duration::from_secs(1),
            ),
        };
        let (sender, receiver) = mpsc::sync_channel(8);
        let trigger = (scenario != "overflow").then(|| {
            let token = token.clone();
            thread::spawn(move || {
                loop {
                    let chunk: ProcessOutputChunk = receiver
                        .recv_timeout(Duration::from_secs(2))
                        .expect("reader must publish actual bytes");
                    if chunk.stream == OutputStream::Stdout && !chunk.bytes.is_empty() {
                        if scenario == "cancel" {
                            token.cancel(CancelCause::Cancelled);
                        }
                        break;
                    }
                }
            })
        });
        let error = if scenario == "overflow" {
            run_sensitive(request, &token).expect_err("bounded overflow")
        } else {
            run_inner_observed(request, &token, Faults::default(), Some(sender))
                .expect_err("bounded cancellation or deadline")
        };
        match scenario {
            "overflow" => assert!(matches!(
                error.kind(),
                ProcessErrorKind::OutputLimitExceeded(OutputStream::Stdout)
            )),
            "deadline" => assert!(matches!(error.kind(), ProcessErrorKind::Deadline)),
            _ => assert!(matches!(error.kind(), ProcessErrorKind::Cancelled(_))),
        }
        if let Some(trigger) = trigger {
            trigger.join().unwrap();
        }
        assert!(
            !error.stdout().is_empty(),
            "positive control: retained partial capture"
        );
        let captured = error.stdout().len() + error.stderr().len();
        assert!(
            !format!("{error:?} {error}").contains("secret"),
            "diagnostics must redact captures"
        );
        observer::reset();
        drop(error);
        wiped_at_least(captured);
    }
}

#[test]
fn sensitive_stdin_workers_wipe_on_success_closed_pipe_and_blocked_teardown() {
    for scenario in ["success", "closed", "blocked"] {
        let input_len = if scenario == "success" {
            1024
        } else {
            2 * 1024 * 1024
        };
        let script = match scenario {
            "success" => "exec /bin/cat",
            "closed" => "exec 0<&-; exec /bin/sleep 0.05",
            _ => "exec /bin/sleep 5",
        };
        let request = ProcessRequest::new(
            "/bin/sh",
            StdinPolicy::Bytes(vec![b'S'; input_len]),
            input_len,
            0,
            Duration::from_secs(2),
        )
        .args(["-c", script]);
        let probe = ReaderProbe::default();
        let result = run_inner(
            request,
            &CancelToken::new(),
            Faults {
                stdin_probe: Some(probe.clone()),
                deadline_after_stdin_would_block: scenario == "blocked",
                ..Faults::default()
            },
        );
        match scenario {
            "success" => assert_eq!(result.unwrap().stdout.len(), input_len),
            "closed" => assert!(matches!(
                result.unwrap_err().kind(),
                ProcessErrorKind::StdinWrite
            )),
            _ => assert!(matches!(
                result.unwrap_err().kind(),
                ProcessErrorKind::Deadline
            )),
        }
        let snapshot = probe
            .wipes
            .lock()
            .unwrap()
            .expect("joined stdin worker must report its actual wipe");
        assert_eq!(snapshot.wiped_bytes, input_len);
        assert_eq!(snapshot.zeroed_bytes, input_len, "stdin remained nonzero");
        assert_eq!(probe.active.load(Ordering::Relaxed), 0, "writer must exit");
        assert_eq!(probe.joined.load(Ordering::Relaxed), 1, "writer must join");
    }
}

struct FailingReader(std::io::Cursor<Vec<u8>>, bool);
impl Read for FailingReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.0.read(bytes)?;
        if count > 0 {
            return Ok(count);
        }
        if self.1 {
            panic!("injected read panic");
        }
        Err(io::Error::other("injected read failure"))
    }
}

#[test]
fn sensitive_pipe_scratch_and_abandoned_prefixes_wipe_on_every_reader_exit() {
    for scenario in [
        "complete",
        "overflow",
        "stop",
        "read-error",
        "panic",
        "growth",
    ] {
        let size = if scenario == "growth" { 20_000 } else { 80 };
        let stop = WorkerStop::default();
        let input = std::io::Cursor::new(vec![b'Q'; size]);
        let reader: Box<dyn Read> = match scenario {
            "read-error" | "panic" => Box::new(FailingReader(input, scenario == "panic")),
            _ => Box::new(input),
        };
        let limit = if scenario == "overflow" { 31 } else { size + 1 };
        observer::reset();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let result = capture::read_bounded(
                reader,
                limit,
                &stop,
                |_| {
                    if scenario == "stop" {
                        stop.request();
                    }
                },
                || {},
            );
            match scenario {
                "complete" | "growth" => assert!(matches!(result.outcome, ReadOutcome::Complete)),
                "overflow" => assert!(matches!(result.outcome, ReadOutcome::Overflow)),
                "stop" => assert!(matches!(result.outcome, ReadOutcome::Stopped)),
                "read-error" => assert!(matches!(result.outcome, ReadOutcome::Failed(_))),
                _ => panic!("reader did not panic"),
            }
            assert_eq!(result.bytes.len(), size.min(limit + 1));
            drop(result);
        }));
        assert_eq!(result.is_err(), scenario == "panic");
        let prefix = size.min(limit + 1);
        let growth = if scenario == "growth" {
            8192 + 16_384
        } else {
            0
        };
        wiped_at_least(8192 + prefix + growth);
    }
}

#[test]
fn sensitive_collected_captures_wipe_when_replaced_and_abandoned() {
    let mut captures = Captures::default();
    captures.set(
        OutputStream::Stdout,
        Capture::from_read(ReadResult {
            bytes: vec![b'A'; 80],
            outcome: ReadOutcome::Complete,
        }),
    );
    observer::reset();
    captures.set(
        OutputStream::Stdout,
        Capture::from_read(ReadResult {
            bytes: vec![b'B'; 160],
            outcome: ReadOutcome::Stopped,
        }),
    );
    wiped_at_least(80);
    drop(captures);
    wiped_at_least(240);
}

#[test]
fn sensitive_setup_failure_wipes_input_before_a_stdin_worker_exists() {
    let request = ProcessRequest::new(
        "/bin/cat",
        StdinPolicy::Bytes(vec![b'S'; 4096]),
        64,
        64,
        Duration::from_secs(2),
    );
    let probe = ReaderProbe::default();
    observer::reset();
    let error = run_inner(
        request,
        &CancelToken::new(),
        Faults {
            fail_reaper_start: true,
            stdin_probe: Some(probe.clone()),
            ..Faults::default()
        },
    )
    .expect_err("injected pre-spawn setup failure");
    assert!(matches!(error.kind(), ProcessErrorKind::Setup));
    assert_eq!(
        probe.started.load(Ordering::Relaxed),
        0,
        "no writer took input custody"
    );
    wiped_at_least(4096);
}
