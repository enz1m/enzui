//! Reader and bounded-stdin worker threads for the process transport.

use super::*;

pub(super) fn spawn_reader<R>(
    owner: &mut ProcessOwner,
    sender: Sender<WorkerEvent>,
    stream: OutputStream,
    reader: R,
    limit: usize,
    faults: Faults,
    output: Option<SyncSender<ProcessOutputChunk>>,
) -> Result<(), ProcessError>
where
    R: Read + Send + NonblockingPipe + 'static,
{
    reader.make_nonblocking().map_err(|error| {
        uncaptured_error(
            ProcessErrorKind::Setup,
            bounded_diagnostic(format!("make {stream:?} pipe nonblocking: {error}")),
        )
    })?;
    let stop = owner.worker_stop();
    #[cfg(test)]
    let joined = faults
        .reader_probe
        .as_ref()
        .map(|probe| std::sync::Arc::clone(&probe.joined));
    #[cfg(not(test))]
    let joined = None;
    #[cfg(not(test))]
    let _ = &faults;
    let handle = thread::Builder::new()
        .name(
            match stream {
                OutputStream::Stdout => "enzui-helper-stdout",
                OutputStream::Stderr => "enzui-helper-stderr",
            }
            .to_owned(),
        )
        .spawn(move || {
            #[cfg(test)]
            let _activity = faults.reader_probe.clone().map(WorkerActivity::enter);
            let work = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                #[cfg(test)]
                if faults.panic_reader == Some(stream) {
                    thread::sleep(Duration::from_millis(50));
                    panic!("injected reader panic");
                }
                #[cfg(test)]
                if faults.fail_reader == Some(stream) {
                    thread::sleep(Duration::from_millis(50));
                    return ReadResult {
                        bytes: Vec::new(),
                        outcome: ReadOutcome::Failed("injected read failure".to_owned()),
                    };
                }
                let result = capture::read_bounded(
                    reader,
                    limit,
                    &stop,
                    |chunk| {
                        #[cfg(test)]
                        if let Some(probe) = faults.reader_probe.as_ref() {
                            probe.observed_stream_bytes(stream, chunk.len());
                        }
                        if let Some(output) = &output {
                            let _ = output.try_send(ProcessOutputChunk {
                                stream,
                                bytes: chunk.to_vec(),
                            });
                        }
                    },
                    || {
                        #[cfg(test)]
                        if let Some(probe) = faults.reader_probe.as_ref() {
                            probe.observed_stream_would_block(stream);
                        }
                    },
                );
                #[cfg(test)]
                if matches!(result.outcome, ReadOutcome::Complete)
                    && let Some(probe) = faults.reader_probe.as_ref()
                {
                    probe.observed_stream_eof(stream);
                }
                result
            }));
            match work {
                Ok(result) => {
                    let _ = sender.send(WorkerEvent::Read(stream, result));
                }
                Err(payload) => {
                    let _ = sender.send(WorkerEvent::Panicked(WorkerKind::Reader(stream)));
                    std::panic::resume_unwind(payload);
                }
            }
        })
        .map_err(|error| {
            uncaptured_error(
                ProcessErrorKind::Setup,
                bounded_diagnostic(format!("start pipe reader: {error}")),
            )
        })?;
    owner.push_worker(WorkerKind::Reader(stream), handle, joined);
    Ok(())
}

pub(super) fn spawn_stdin(
    owner: &mut ProcessOwner,
    sender: Sender<WorkerEvent>,
    mut stdin: ChildStdin,
    bytes: enzui_security::secret::Secret,
    faults: Faults,
) -> Result<(), ProcessError> {
    stdin.make_nonblocking().map_err(|error| {
        uncaptured_error(
            ProcessErrorKind::Setup,
            bounded_diagnostic(format!("make stdin pipe nonblocking: {error}")),
        )
    })?;
    let stop = owner.worker_stop();
    #[cfg(test)]
    let joined = faults
        .stdin_probe
        .as_ref()
        .map(|probe| std::sync::Arc::clone(&probe.joined));
    #[cfg(not(test))]
    let joined = None;
    let handle = thread::Builder::new()
        .name("enzui-helper-stdin".to_owned())
        .spawn(move || {
            #[cfg(test)]
            let _activity = faults.stdin_probe.clone().map(WorkerActivity::enter);
            #[cfg(not(test))]
            let _ = &faults;
            let work = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                write_stdin(&mut stdin, bytes.expose_secret(), &stop, || {
                    #[cfg(test)]
                    if let Some(probe) = faults.stdin_probe.as_ref() {
                        probe.observed_would_block();
                    }
                })
            }));
            drop(bytes);
            #[cfg(test)]
            if let Some(probe) = &faults.stdin_probe {
                *probe.wipes.lock().expect("stdin wipe observation") =
                    Some(enzui_security::secret::observer::snapshot());
            }
            match work {
                Ok(result) => {
                    let _ = sender.send(WorkerEvent::Stdin(result));
                }
                Err(payload) => {
                    let _ = sender.send(WorkerEvent::Panicked(WorkerKind::Stdin));
                    std::panic::resume_unwind(payload);
                }
            }
        })
        .map_err(|error| {
            uncaptured_error(
                ProcessErrorKind::Setup,
                bounded_diagnostic(format!("start stdin writer: {error}")),
            )
        })?;
    owner.push_worker(WorkerKind::Stdin, handle, joined);
    Ok(())
}

fn write_stdin(
    stdin: &mut ChildStdin,
    bytes: &[u8],
    stop: &WorkerStop,
    mut observed_would_block: impl FnMut(),
) -> Result<(), String> {
    let mut written = 0;
    while written < bytes.len() {
        if stop.requested() {
            return Ok(());
        }
        match stdin.write(&bytes[written..]) {
            Ok(0) => {
                return Err(bounded_diagnostic(
                    "write stdin: pipe accepted zero bytes".to_owned(),
                ));
            }
            Ok(count) => written += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                observed_would_block();
                thread::sleep(POLL_INTERVAL);
            }
            Err(error) => {
                return Err(bounded_diagnostic(format!("write stdin: {error}")));
            }
        }
    }
    stdin
        .flush()
        .map_err(|error| bounded_diagnostic(format!("flush stdin: {error}")))
}
