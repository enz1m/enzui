#[derive(Debug)]
struct Capture {
    bytes: Vec<u8>,
    completeness: CaptureCompleteness,
}

impl Drop for Capture {
    fn drop(&mut self) {
        drop(enzui_security::secret::Secret::new(std::mem::take(
            &mut self.bytes,
        )));
    }
}

impl Capture {
    // Only end of file proves a complete capture; stopped and failed readers retain partial prefixes.
    fn from_read(mut result: ReadResult) -> Self {
        let completeness = match &result.outcome {
            ReadOutcome::Complete => CaptureCompleteness::Complete,
            ReadOutcome::Overflow => CaptureCompleteness::OverflowTruncated,
            ReadOutcome::Stopped | ReadOutcome::Failed(_) => CaptureCompleteness::StoppedPartial,
        };
        Self {
            bytes: std::mem::take(&mut result.bytes),
            completeness,
        }
    }
}

#[derive(Default)]
struct Captures {
    stdout: Option<Capture>,
    stderr: Option<Capture>,
}

impl Captures {
    fn set(&mut self, stream: OutputStream, capture: Capture) {
        match stream {
            OutputStream::Stdout => self.stdout = Some(capture),
            OutputStream::Stderr => self.stderr = Some(capture),
        }
    }

    // A missing reader result is an empty stopped prefix, never a complete empty stream.
    fn into_error_parts(self) -> (Vec<u8>, CaptureCompleteness, Vec<u8>, CaptureCompleteness) {
        let (stdout, stdout_completeness) = Self::parts(self.stdout);
        let (stderr, stderr_completeness) = Self::parts(self.stderr);
        (stdout, stdout_completeness, stderr, stderr_completeness)
    }

    fn parts(capture: Option<Capture>) -> (Vec<u8>, CaptureCompleteness) {
        capture.map_or_else(
            || (Vec::new(), CaptureCompleteness::StoppedPartial),
            |mut capture| (std::mem::take(&mut capture.bytes), capture.completeness),
        )
    }
}

struct DriveState<'a> {
    owner: ProcessOwner,
    receiver: Receiver<WorkerEvent>,
    cancel: &'a CancelToken,
    started: Instant,
    deadline: Duration,
    stdin_complete: bool,
    captures: Captures,
    faults: Faults,
}

impl DriveState<'_> {
    fn fail(
        self,
        kind: ProcessErrorKind,
        diagnostic: Option<String>,
    ) -> Result<ProcessOutput, ProcessError> {
        fail_and_cleanup(self.owner, self.receiver, kind, self.captures, diagnostic)
    }
}

fn drive(mut state: DriveState<'_>) -> Result<ProcessOutput, ProcessError> {
    #[cfg(not(test))]
    let _ = &state.faults;

    loop {
        #[cfg(test)]
        if state.faults.deadline_after_stdin_would_block
            && state
                .faults
                .stdin_probe
                .as_ref()
                .is_some_and(ReaderProbe::has_would_blocked)
        {
            if state.faults.require_reader_would_block
                && let Some(probe) = state.faults.reader_probe.as_ref()
            {
                probe.wait_for_both_readers_would_block(
                    "both output readers must observe real WouldBlock before the injected \
                     deadline",
                );
            }
            return state.fail(ProcessErrorKind::Deadline, None);
        }
        if let Some(cause) = state.cancel.cause() {
            return state.fail(ProcessErrorKind::Cancelled(cause), None);
        }
        let elapsed = state.started.elapsed();
        if elapsed >= state.deadline {
            return state.fail(ProcessErrorKind::Deadline, None);
        }

        if state.captures.stdout.is_some()
            && state.captures.stderr.is_some()
            && state.stdin_complete
        {
            #[cfg(test)]
            if state.faults.fail_wait {
                return state.fail(
                    ProcessErrorKind::Wait,
                    Some("injected wait failure".to_owned()),
                );
            }
            match state.owner.complete_if_exited() {
                Ok(Some(status)) => {
                    let panicked = state.owner.join_all();
                    if let Some(kind) = panicked.first().copied() {
                        return Err(error_from_worker_panic(kind, state.captures));
                    }
                    let (stdout, out_complete, stderr, err_complete) =
                        state.captures.into_error_parts();
                    if out_complete != CaptureCompleteness::Complete
                        || err_complete != CaptureCompleteness::Complete
                    {
                        drop(enzui_security::secret::Secret::new(stdout));
                        drop(enzui_security::secret::Secret::new(stderr));
                        return Err(empty_error(ProcessErrorKind::Setup));
                    }
                    return Ok(ProcessOutput {
                        status,
                        stdout,
                        stderr,
                    });
                }
                Ok(None) => {}
                Err(error) => {
                    return state.fail(error.kind, Some(error.detail));
                }
            }
        }

        let remaining = state.deadline.saturating_sub(elapsed);
        match state.receiver.recv_timeout(POLL_INTERVAL.min(remaining)) {
            Ok(WorkerEvent::Read(stream, result)) => {
                let kind = match result.outcome {
                    ReadOutcome::Complete => None,
                    ReadOutcome::Overflow => Some(ProcessErrorKind::OutputLimitExceeded(stream)),
                    ReadOutcome::Stopped => {
                        let diagnostic = Some(
                            "pipe reader stopped before the owner entered teardown".to_owned(),
                        );
                        state.captures.set(stream, Capture::from_read(result));
                        return state.fail(ProcessErrorKind::Read(stream), diagnostic);
                    }
                    ReadOutcome::Failed(ref detail) => {
                        let diagnostic = Some(detail.clone());
                        state.captures.set(stream, Capture::from_read(result));
                        return state.fail(ProcessErrorKind::Read(stream), diagnostic);
                    }
                };
                state.captures.set(stream, Capture::from_read(result));
                if let Some(kind) = kind {
                    #[cfg(test)]
                    if state.faults.require_reader_would_block
                        && matches!(kind, ProcessErrorKind::OutputLimitExceeded(_))
                        && let Some(probe) = state.faults.reader_probe.as_ref()
                    {
                        probe.wait_for_stream_would_block(
                            stream.sibling(),
                            "the idle sibling reader must observe real WouldBlock before an \
                             overflow ends the run",
                        );
                    }
                    return state.fail(kind, None);
                }
            }
            Ok(WorkerEvent::Stdin(Ok(()))) => state.stdin_complete = true,
            Ok(WorkerEvent::Stdin(Err(detail))) => {
                return state.fail(ProcessErrorKind::StdinWrite, Some(detail));
            }
            Ok(WorkerEvent::Panicked(kind)) => {
                let kind = match kind {
                    WorkerKind::Reader(stream) => ProcessErrorKind::ReaderPanicked(stream),
                    WorkerKind::Stdin => ProcessErrorKind::WorkerPanicked,
                };
                return state.fail(kind, None);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected)
                if state.captures.stdout.is_some()
                    && state.captures.stderr.is_some()
                    && state.stdin_complete =>
            {
                thread::sleep(POLL_INTERVAL.min(remaining));
            }
            Err(RecvTimeoutError::Disconnected) => {
                return state.fail(
                    ProcessErrorKind::Setup,
                    Some("worker event channel closed before every result".to_owned()),
                );
            }
        }
    }
}

fn fail_and_cleanup(
    mut owner: ProcessOwner,
    receiver: Receiver<WorkerEvent>,
    mut kind: ProcessErrorKind,
    mut captures: Captures,
    mut diagnostic: Option<String>,
) -> Result<ProcessOutput, ProcessError> {
    if let Err(teardown) = owner.terminate_and_reap() {
        kind = teardown.kind;
        diagnostic = Some(teardown.detail);
    }
    let panicked = owner.join_all();
    for event in receiver.try_iter() {
        match event {
            // Teardown preserves each reader’s outcome: prior end of file remains complete, while interruption is partial.
            WorkerEvent::Read(stream, result) => {
                captures.set(stream, Capture::from_read(result));
            }
            WorkerEvent::Panicked(worker) if panicked.is_empty() => {
                kind = match worker {
                    WorkerKind::Reader(stream) => ProcessErrorKind::ReaderPanicked(stream),
                    WorkerKind::Stdin => ProcessErrorKind::WorkerPanicked,
                };
            }
            WorkerEvent::Stdin(_) | WorkerEvent::Panicked(_) => {}
        }
    }
    if let Some(worker) = panicked.first().copied() {
        kind = match worker {
            WorkerKind::Reader(stream) => ProcessErrorKind::ReaderPanicked(stream),
            WorkerKind::Stdin => ProcessErrorKind::WorkerPanicked,
        };
    }
    let (stdout, stdout_completeness, stderr, stderr_completeness) = captures.into_error_parts();
    Err(ProcessError {
        kind,
        stdout,
        stdout_completeness,
        stderr,
        stderr_completeness,
        diagnostic,
    })
}

fn error_from_worker_panic(worker: WorkerKind, captures: Captures) -> ProcessError {
    let (stdout, stdout_completeness, stderr, stderr_completeness) = captures.into_error_parts();
    ProcessError {
        kind: match worker {
            WorkerKind::Reader(stream) => ProcessErrorKind::ReaderPanicked(stream),
            WorkerKind::Stdin => ProcessErrorKind::WorkerPanicked,
        },
        stdout,
        stdout_completeness,
        stderr,
        stderr_completeness,
        diagnostic: None,
    }
}

// Setup failures have no completed capture, so both streams are empty stopped prefixes.
fn uncaptured_error(kind: ProcessErrorKind, diagnostic: String) -> ProcessError {
    let mut error = empty_error(kind);
    error.diagnostic = Some(diagnostic);
    error
}

fn empty_error(kind: ProcessErrorKind) -> ProcessError {
    ProcessError {
        kind,
        stdout: Vec::new(),
        stdout_completeness: CaptureCompleteness::StoppedPartial,
        stderr: Vec::new(),
        stderr_completeness: CaptureCompleteness::StoppedPartial,
        diagnostic: None,
    }
}

fn bounded_diagnostic(mut diagnostic: String) -> String {
    if diagnostic.len() <= DIAGNOSTIC_LIMIT {
        return diagnostic;
    }
    let mut end = DIAGNOSTIC_LIMIT;
    while !diagnostic.is_char_boundary(end) {
        end -= 1;
    }
    diagnostic.truncate(end);
    diagnostic
}
