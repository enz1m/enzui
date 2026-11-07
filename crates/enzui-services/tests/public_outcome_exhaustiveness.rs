//! Downstream applications `match` several public outcome enums
//! exhaustively, with no fallback arm — for example, the apps' picker
//! and download routing. Those enums are therefore *not* `#[non_exhaustive]`,
//! and growing one is a source break that no ordinary framework unit test
//! would otherwise expose.
//!
//! This file reproduces representative downstream shapes inside the framework,
//! so a widening lands as a red core gate; it is not a fixed consumer inventory.
//! The session match retains the public enum's compile contract rather than
//! claiming that an app currently routes it. Every match below is
//! deliberately wildcard-free. `FileWatchOutcome::Superseded` is an intentional
//! source break: stale deliveries need a distinct silent result, and this target
//! records the complete downstream migration shape rather than hiding it behind
//! `_`.
//!
//! Sabotage (must actually be tried before trusting this file): add any variant
//! to one of the imported outcome enums, or omit an existing arm below; this
//! test target then fails to compile with E0004 "non-exhaustive patterns".
//! `CaptureCompleteness` was verified that way: a fourth variant reddens
//! `app_style_helper_status_line` with E0004.
//!
//! `CaptureCompleteness` also carries a *behavioural* consumer gate here, not
//! just a compile one: `a_real_consumer_never_shows_a_partial_capture_as_a
//! _complete_line` runs the real transport twice and proves the same status-line
//! consumer prints a genuinely complete diagnostic while refusing to print a
//! teardown-stopped prefix as a finished line.
//!
//! The retryable, transient refusals the bounded portal command queues produce
//! keep their typed identity one layer down — on `BackendError::ResourceExhausted`
//! and `SessionError::ResourceExhausted` — precisely so these enums need not
//! grow.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use enzui_core::cancellation::CancelToken;
use enzui_services::process_transport::{
    self, CaptureCompleteness, OutputStream, ProcessError, ProcessErrorKind, ProcessRequest,
    StdinPolicy,
};
use std::time::Duration;

/// What a real consumer shows a user after a helper failure.
///
/// The shape of a status-bar/diagnostic adopter: it wants the helper's first
/// stderr line, and it must never present bytes that teardown cut short as if
/// the helper had finished saying them. The match over
/// [`CaptureCompleteness`] is deliberately wildcard-free, so a new capture
/// state is a source break here rather than a silently mis-presented line.
fn app_style_helper_status_line(error: &ProcessError) -> String {
    match error.stderr_completeness() {
        CaptureCompleteness::Complete => error.complete_capture(OutputStream::Stderr).map_or_else(
            || INCOMPLETE_DIAGNOSTIC.to_owned(),
            |bytes| {
                let text = String::from_utf8_lossy(bytes);
                match text.lines().next().map(str::trim) {
                    Some(line) if !line.is_empty() => line.to_owned(),
                    _ => "The helper failed without a diagnostic.".to_owned(),
                }
            },
        ),
        // Bounded overflow: every retained byte is real, but the stream ran
        // past the cap, so the tail is explicitly marked rather than shown as
        // the helper's last word.
        CaptureCompleteness::OverflowTruncated => {
            "The helper failed; its diagnostic was too long to keep.".to_owned()
        }
        // Teardown-stopped: an arbitrary prefix, possibly mid-line. Never
        // rendered as helper speech.
        CaptureCompleteness::StoppedPartial => INCOMPLETE_DIAGNOSTIC.to_owned(),
    }
}

const INCOMPLETE_DIAGNOSTIC: &str = "The helper failed; its diagnostic is incomplete.";

/// A whole-looking line the helper writes to stderr before it is parked.
const PREAMBLE: &str = "helper preamble line";

/// The repeating cycle every drain byte comes from, so the retained capture is
/// exactly characterized rather than merely "some bytes".
const DRAIN_CYCLE: &[u8] = b"1234567890";
/// `10 * 2^17 * 2` = 2 621 440 bytes of stderr after the preamble.
const DRAIN_DOUBLINGS: usize = 17;
const DRAIN_REPEATS: usize = 2;
const DRAIN_BYTES: usize = DRAIN_CYCLE.len() * (1 << DRAIN_DOUBLINGS) * DRAIN_REPEATS;
/// A pipe holds at most its capacity, so a completed write of `DRAIN_BYTES`
/// proves at least `DRAIN_BYTES - capacity` bytes were taken *out* by the real
/// reader. A fresh Linux pipe holds 64 KiB and configuration cannot raise a
/// pipe past `fs/pipe-max-size` (1 MiB by default), so this floor holds without
/// assuming the machine's tuning — and it is far above the preamble.
const DRAIN_FLOOR: usize = 1 << 20;
/// Far above everything the handshake writes, so stderr can never overflow:
/// its only possible incompleteness is teardown.
const HANDSHAKE_STDERR_LIMIT: usize = 8 << 20;
const FLOOD_STDOUT_LIMIT: usize = 32;

const _: () = assert!(
    DRAIN_BYTES > DRAIN_FLOOR && DRAIN_BYTES - DRAIN_FLOOR > PREAMBLE.len() + 1,
    "the drain must outrun any possible pipe capacity by more than the preamble"
);

fn helper(
    script: &str,
    stdout_limit: usize,
    stderr_limit: usize,
    deadline: Duration,
) -> ProcessRequest {
    ProcessRequest::new(
        "/bin/sh",
        StdinPolicy::Null,
        stdout_limit,
        stderr_limit,
        deadline,
    )
    .arg("-c")
    .arg(script)
}

/// A real failure whose stderr capture is a synchronized, known, non-empty
/// teardown-stopped prefix.
///
/// The synchronization is a handshake with the transport's own stderr reader,
/// not a delay: the helper writes the preamble and then `DRAIN_BYTES` more,
/// which is larger than any pipe can hold, so its `write(2)` cannot return
/// until the reader has taken at least `DRAIN_FLOOR` bytes out of that pipe.
/// Only after that write returns does the helper flood stdout, so the preamble
/// is provably already in the reader's buffer when the overflow ends the run —
/// and stderr is still open, so the capture is a stopped prefix, never an EOF.
///
/// If that release never happens (no reader, or a stalled one) the helper stays
/// blocked, the flood never starts, and the request's deadline ends the run:
/// the failure mode is a bounded red, never a hang and never an empty capture
/// that passes as "withheld".
fn overflow_beside_a_drained_sibling() -> ProcessError {
    let script = format!(
        "c={cycle}; i=0; while [ $i -lt {DRAIN_DOUBLINGS} ]; do c=\"$c$c\"; i=$((i+1)); done; \
         printf '{PREAMBLE}\\n' >&2; \
         i=0; while [ $i -lt {DRAIN_REPEATS} ]; do printf %s \"$c\" >&2; i=$((i+1)); done; \
         while :; do printf %s \"$c\"; done",
        cycle = String::from_utf8_lossy(DRAIN_CYCLE),
    );
    let error = process_transport::run(
        helper(
            &script,
            FLOOD_STDOUT_LIMIT,
            HANDSHAKE_STDERR_LIMIT,
            Duration::from_secs(10),
        ),
        &CancelToken::new(),
    )
    .expect_err("a stdout overflow is never a complete response");
    // The handshake is checked here, so *every* caller of this fixture depends
    // on it: no test downstream can pass on an unsynchronized, possibly empty
    // sibling capture that a scheduler happened to fill.
    assert_eq!(
        error.kind(),
        &ProcessErrorKind::OutputLimitExceeded(OutputStream::Stdout),
        "the flood only starts after the stderr handshake releases the helper"
    );
    assert!(
        error
            .stderr()
            .starts_with(format!("{PREAMBLE}\n").as_bytes()),
        "the stopped sibling must open with the known preamble"
    );
    assert!(
        error.stderr().len() >= DRAIN_FLOOR,
        "the reader must have drained at least {DRAIN_FLOOR} bytes for the helper's write to \
         return; got {}",
        error.stderr().len()
    );
    error
}

/// Through the real transport: a stdout overflow stops the live stderr reader,
/// and the consumer above must not print what it managed to read as a finished
/// line — while the same consumer still prints a genuinely complete one.
///
/// Both halves are arranged by construction, not by timing: the partial case
/// uses the drain handshake above, and the complete case uses a helper that
/// closes both pipes before parking, so its readers reach end of file and the
/// request's own deadline is the failure. Every claim here is about the exact
/// bytes the real [`ProcessError`] carries.
#[test]
fn a_real_consumer_never_shows_a_partial_capture_as_a_complete_line() {
    let partial = overflow_beside_a_drained_sibling();
    assert_eq!(
        partial.stderr_completeness(),
        CaptureCompleteness::StoppedPartial,
        "the sibling reader was stopped before end of file, so its capture is partial"
    );

    // The exact retained bytes, taken from the error itself, must not reach the
    // user as helper speech.
    let retained = String::from_utf8_lossy(partial.stderr()).into_owned();
    let retained_line = retained
        .lines()
        .next()
        .expect("the retained capture holds a line")
        .to_owned();
    assert_eq!(
        retained_line, PREAMBLE,
        "the withholding claim is about the known preamble the reader really holds"
    );
    let shown = app_style_helper_status_line(&partial);
    assert_eq!(
        shown, INCOMPLETE_DIAGNOSTIC,
        "a teardown-stopped capture must be presented as incomplete"
    );
    assert!(
        !shown.contains(&retained_line),
        "the retained partial bytes {retained_line:?} must never reach the user as a finished \
         line: {shown:?}"
    );
    assert!(
        !retained
            .as_bytes()
            .windows(shown.len().max(1))
            .any(|window| window == shown.as_bytes()),
        "the shown line must be the framework's own prose, not a slice of the helper's bytes"
    );

    // Positive control: the same consumer, a real error whose stderr genuinely
    // reached end of file. The helper closes both pipes before parking, so both
    // readers finish on the kernel's EOF and the deadline is what fails — a
    // failure kind that carries complete captures.
    let complete = process_transport::run(
        helper(
            &format!("printf '{PREAMBLE}\\n' >&2; exec 1>&- 2>&-; while :; do sleep 1; done"),
            4096,
            4096,
            Duration::from_millis(750),
        ),
        &CancelToken::new(),
    )
    .expect_err("the helper never exits, so the deadline ends the run");
    assert_eq!(complete.kind(), &ProcessErrorKind::Deadline);
    assert_eq!(
        complete.stderr_completeness(),
        CaptureCompleteness::Complete,
        "a stderr the reader drained to end of file is complete even on a failure"
    );
    assert_eq!(
        complete.complete_capture(OutputStream::Stderr),
        Some(format!("{PREAMBLE}\n").as_bytes()),
        "the complete capture is exactly what the helper wrote"
    );
    assert_eq!(
        app_style_helper_status_line(&complete),
        PREAMBLE,
        "positive control: a complete diagnostic is still shown to the user"
    );
}
