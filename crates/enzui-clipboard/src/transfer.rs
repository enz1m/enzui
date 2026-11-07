//! Nonblocking descriptor transfers with receive-size and blocked-I/O time limits.
//!
//! Timeout checks occur when a syscall would block. Continuous progress and
//! interrupted syscalls can continue beyond the deadline. Reads enforce
//! `max_bytes`; writes use the supplied slice without applying that size limit.
//! Both paths set close-on-exec and nonblocking flags on the adopted descriptor.

use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::{OFlags, fcntl_setfl};
use rustix::io::{Errno, FdFlags, fcntl_getfd, fcntl_setfd};

use enzui_core::monotonic_ms::{Policy, checked_instant_deadline};

use crate::error::ClipboardError;

/// The resource and deadline limits for clipboard startup and transfers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferLimits {
    /// Largest payload accepted from a peer, in bytes.
    pub max_bytes: usize,

    /// How long a paste waits for the owner to produce data.
    pub receive_timeout: Duration,

    /// How long serving a peer's request may take before the transfer is
    /// abandoned.
    pub send_timeout: Duration,

    /// How many transfers may be in flight at once before further requests are
    /// refused by closing the descriptor.
    pub max_concurrent_transfers: usize,

    /// How long to wait for the compositor to confirm a selection claim.
    ///
    /// The compositor answers a granted claim by delivering the new selection
    /// to the focused client, which is this application. Silence means the
    /// claim was dropped.
    pub claim_timeout: Duration,

    /// How long construction may wait for registry discovery and initial
    /// compositor synchronization.
    pub startup_timeout: Duration,
}

impl Default for TransferLimits {
    /// Use 32 MiB reads, five-second send/receive timeouts, eight transfers,
    /// one-second claim confirmation and two-second startup waits.
    fn default() -> Self {
        Self {
            max_bytes: 32 * 1024 * 1024,
            receive_timeout: Duration::from_secs(5),
            send_timeout: Duration::from_secs(5),
            max_concurrent_transfers: 8,
            claim_timeout: Duration::from_secs(1),
            startup_timeout: Duration::from_secs(2),
        }
    }
}

/// Read until EOF, limiting collected bytes and waits for readable data.
///
/// Checks the deadline when a read would block; it does not interrupt continuous
/// input. Zero or overflowing timeout values expire at the first blocking wait.
/// The descriptor is marked close-on-exec and nonblocking before reading.
///
/// # Errors
///
/// Returns `TransferStalled` on a blocked-read timeout, `TransferTooLarge` above
/// `max_bytes`, or `TransferFailed` for descriptor setup or I/O errors.
pub fn read_bounded(fd: OwnedFd, limits: TransferLimits) -> Result<Vec<u8>, ClipboardError> {
    set_close_on_exec(fd.as_fd())?;
    set_non_blocking(fd.as_fd())?;

    let now = Instant::now();
    let deadline =
        checked_instant_deadline(now, limits.receive_timeout, Policy::Expire).unwrap_or(now);
    let mut collected: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];

    loop {
        match rustix::io::read(fd.as_fd(), &mut chunk[..]) {
            Ok(0) => return Ok(collected),
            Ok(count) => {
                if collected.len() + count > limits.max_bytes {
                    return Err(ClipboardError::TransferTooLarge {
                        limit: limits.max_bytes,
                    });
                }

                collected.extend_from_slice(&chunk[..count]);
            }
            Err(Errno::INTR) => continue,
            Err(Errno::AGAIN) => {
                wait_for(fd.as_fd(), PollFlags::IN, deadline, limits.receive_timeout)?;
            }
            Err(error) => {
                return Err(ClipboardError::TransferFailed {
                    reason: error.to_string(),
                });
            }
        }
    }
}

/// Write a slice with a deadline for waits on a full descriptor.
///
/// Does not enforce `max_bytes`. Continuous progress can exceed the deadline.
/// A closed receiver (`EPIPE`) counts as success even when bytes remain. Sets
/// close-on-exec and nonblocking flags before writing.
///
/// # Errors
///
/// Returns `TransferStalled` on a blocked-write timeout or `TransferFailed` for
/// descriptor setup, zero-byte writes and other I/O errors.
pub fn write_bounded(
    fd: OwnedFd,
    bytes: &[u8],
    limits: TransferLimits,
) -> Result<(), ClipboardError> {
    write_with_control(fd, bytes, limits, None)
}

pub(crate) fn write_sensitive_bounded(
    fd: OwnedFd,
    bytes: &[u8],
    limits: TransferLimits,
    active: &AtomicBool,
    expires: Instant,
) -> Result<(), ClipboardError> {
    if bytes.len() > limits.max_bytes {
        return Err(ClipboardError::TransferTooLarge {
            limit: limits.max_bytes,
        });
    }
    write_with_control(fd, bytes, limits, Some((active, expires)))
}

fn write_with_control(
    fd: OwnedFd,
    bytes: &[u8],
    limits: TransferLimits,
    control: Option<(&AtomicBool, Instant)>,
) -> Result<(), ClipboardError> {
    set_close_on_exec(fd.as_fd())?;
    set_non_blocking(fd.as_fd())?;

    let now = Instant::now();
    let deadline =
        checked_instant_deadline(now, limits.send_timeout, Policy::Expire).unwrap_or(now);
    let deadline = control.map_or(deadline, |(_, expires)| deadline.min(expires));
    let mut written = 0usize;

    while written < bytes.len() {
        if control.is_some_and(|(active, _)| {
            !active.load(Ordering::Acquire) || Instant::now() >= deadline
        }) {
            return Err(ClipboardError::Stopped);
        }
        match rustix::io::write(fd.as_fd(), &bytes[written..]) {
            Ok(0) => {
                return Err(ClipboardError::TransferFailed {
                    reason: String::from("the receiver accepted no bytes"),
                });
            }
            Ok(count) => written += count,
            Err(Errno::INTR) => continue,
            Err(Errno::AGAIN) => {
                let next = if control.is_some() {
                    deadline.min(Instant::now() + Duration::from_millis(50))
                } else {
                    deadline
                };
                match wait_for(fd.as_fd(), PollFlags::OUT, next, limits.send_timeout) {
                    Err(ClipboardError::TransferStalled { .. })
                        if control.is_some() && Instant::now() < deadline => {}
                    result => result?,
                }
            }
            Err(Errno::PIPE) => return Ok(()),
            Err(error) => {
                return Err(ClipboardError::TransferFailed {
                    reason: error.to_string(),
                });
            }
        }
    }

    Ok(())
}

fn wait_for(
    fd: BorrowedFd<'_>,
    flags: PollFlags,
    deadline: Instant,
    budget: Duration,
) -> Result<(), ClipboardError> {
    let remaining = deadline.saturating_duration_since(Instant::now());

    if remaining.is_zero() {
        return Err(ClipboardError::TransferStalled { after: budget });
    }

    let timeout = Timespec {
        tv_sec: remaining.as_secs() as _,
        tv_nsec: remaining.subsec_nanos().into(),
    };

    let mut fds = [PollFd::new(&fd, flags)];

    match poll(&mut fds, Some(&timeout)) {
        Ok(0) => Err(ClipboardError::TransferStalled { after: budget }),
        Ok(_) => Ok(()),
        Err(Errno::INTR) => Ok(()),
        Err(error) => Err(ClipboardError::TransferFailed {
            reason: error.to_string(),
        }),
    }
}

fn set_close_on_exec(fd: BorrowedFd<'_>) -> Result<(), ClipboardError> {
    let flags = fcntl_getfd(fd).map_err(|error| ClipboardError::TransferFailed {
        reason: error.to_string(),
    })?;

    fcntl_setfd(fd, flags | FdFlags::CLOEXEC).map_err(|error| ClipboardError::TransferFailed {
        reason: error.to_string(),
    })
}

pub(crate) fn set_non_blocking(fd: BorrowedFd<'_>) -> Result<(), ClipboardError> {
    let flags = rustix::fs::fcntl_getfl(fd).map_err(|error| ClipboardError::TransferFailed {
        reason: error.to_string(),
    })?;

    fcntl_setfl(fd, flags | OFlags::NONBLOCK).map_err(|error| ClipboardError::TransferFailed {
        reason: error.to_string(),
    })
}

/// Create a close-on-exec pipe, returning its read and write ends.
///
/// # Errors
///
/// Returns `TransferFailed` if the OS cannot create the pipe.
pub fn transfer_pipe() -> Result<(OwnedFd, OwnedFd), ClipboardError> {
    rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).map_err(|error| {
        ClipboardError::TransferFailed {
            reason: error.to_string(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_close_on_exec_normalizes_an_inheritable_descriptor() {
        let (read_end, _write_end) =
            rustix::pipe::pipe_with(rustix::pipe::PipeFlags::empty()).expect("pipe");
        let before = fcntl_getfd(&read_end).expect("read descriptor flags");
        assert!(!before.contains(FdFlags::CLOEXEC));

        set_close_on_exec(read_end.as_fd()).expect("prepare transfer descriptor");

        let after = fcntl_getfd(&read_end).expect("read normalized descriptor flags");
        assert!(after.contains(FdFlags::CLOEXEC));
    }
}
