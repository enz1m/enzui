//! Owned PipeWire remote descriptors for completed screencast sessions.
//!
//! Adoption normalizes `FD_CLOEXEC` in place and fails closed; ownership then follows [`std::os::fd::OwnedFd`] semantics.
#![allow(unsafe_code, reason = "adopts raw descriptors the portal hands over")]

use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};

/// Failure to establish close-on-exec while adopting a descriptor.
///
/// The descriptor is closed instead of being returned.
#[derive(Debug)]
pub struct AdoptError {
    stage: &'static str,
    source: std::io::Error,
}

impl AdoptError {
    /// The `fcntl` step that failed (`"F_GETFD"` or `"F_SETFD"`).
    #[must_use]
    pub fn stage(&self) -> &'static str {
        self.stage
    }
}

impl std::fmt::Display for AdoptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "refusing to adopt a PipeWire remote fd: fcntl({}) failed: {}",
            self.stage, self.source
        )
    }
}

impl std::error::Error for AdoptError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

// Set `FD_CLOEXEC` in place without changing the fd number or other descriptor flags.
fn set_cloexec(raw: RawFd) -> Result<(), AdoptError> {
    // SAFETY: F_GETFD reads the descriptor flags and mutates nothing; any int is
    // a legal argument and an invalid fd is reported as -1/EBADF, never UB.
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFD) };
    if flags == -1 {
        return Err(AdoptError {
            stage: "F_GETFD",
            source: std::io::Error::last_os_error(),
        });
    }
    // SAFETY: `F_SETFD` receives the existing flags with only `FD_CLOEXEC` added.
    let rc = unsafe { libc::fcntl(raw, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    if rc == -1 {
        return Err(AdoptError {
            stage: "F_SETFD",
            source: std::io::Error::last_os_error(),
        });
    }
    Ok(())
}

/// An owned descriptor for a live PipeWire screencast remote.
///
/// The descriptor is close-on-exec and closes when this value is dropped.
#[derive(Debug)]
pub struct PipeWireRemote {
    fd: OwnedFd,
}

impl PipeWireRemote {
    /// Adopts an owned fd and sets `FD_CLOEXEC` without changing its number.
    ///
    /// This normalization does not replace atomic close-on-exec at descriptor receipt.
    ///
    /// # Errors
    ///
    /// Returns [`AdoptError`] and closes the fd if its flags cannot be read or updated.
    pub fn from_owned_fd(fd: OwnedFd) -> Result<Self, AdoptError> {
        set_cloexec(fd.as_raw_fd())?;
        Ok(Self { fd })
    }

    /// Takes ownership of a raw fd and adopts it as a PipeWire remote.
    ///
    /// # Safety
    ///
    /// `raw` must be open and exclusively owned by the caller.
    ///
    /// # Errors
    ///
    /// Returns [`AdoptError`] and closes the fd if close-on-exec normalization fails.
    pub unsafe fn from_raw_fd(raw: RawFd) -> Result<Self, AdoptError> {
        // SAFETY: forwarded to the caller's contract above.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        Self::from_owned_fd(fd)
    }

    /// Returns the raw fd number without transferring ownership.
    ///
    /// The descriptor remains owned by this value and is guaranteed valid while it remains alive.
    #[must_use]
    pub fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Borrows the underlying [`OwnedFd`], which callers may duplicate with `try_clone`.
    #[must_use]
    pub fn as_owned_fd(&self) -> &OwnedFd {
        &self.fd
    }

    /// Transfers ownership as a raw fd without closing it.
    ///
    /// The caller becomes responsible for closing the descriptor.
    #[must_use]
    pub fn into_raw_fd(self) -> RawFd {
        self.fd.into_raw_fd()
    }

    /// Consumes this value and closes its descriptor immediately.
    pub fn close(self) {
        drop(self.fd);
    }
}

/// Test helpers for observing descriptor ownership and closure.
///
/// Use [`ClosureObserver`](crate::portal::pipewire::test_support::ClosureObserver)
/// for release checks; raw fd numbers may be reused after close.
#[cfg(test)]
pub mod test_support {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

    /// Observes closure of a specific pipe write end through EOF on its owned read end.
    ///
    /// The observation tracks the open-file description and is immune to reuse of the original fd number.
    #[derive(Debug)]
    pub struct ClosureObserver {
        read_end: OwnedFd,
    }

    impl ClosureObserver {
        /// Creates a nonblocking private pipe and returns its observer plus owned write-end subject.
        ///
        /// The subject is released after every copy of the write end closes.
        ///
        /// # Panics
        ///
        /// Panics if `pipe2` fails.
        #[must_use]
        pub fn new() -> (Self, OwnedFd) {
            let mut fds = [0 as RawFd; 2];
            // SAFETY: `pipe2` writes two descriptors into the two-element array, and both flag bits are valid.
            let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_NONBLOCK | libc::O_CLOEXEC) };
            assert_eq!(
                rc,
                0,
                "pipe2(2) failed: {}",
                std::io::Error::last_os_error()
            );
            // SAFETY: pipe2 just returned these as fresh fds this process owns.
            let read_end = unsafe { OwnedFd::from_raw_fd(fds[0]) };
            // SAFETY: as above; each descriptor gets exactly one owner.
            let write_end = unsafe { OwnedFd::from_raw_fd(fds[1]) };
            (Self { read_end }, write_end)
        }

        /// Returns `true` after every copy of the subject write end is closed.
        ///
        /// The nonblocking pipe read is repeatable and immune to fd-number reuse.
        #[must_use]
        pub fn subject_released(&self) -> bool {
            let mut byte = [0u8; 1];
            // SAFETY: the one-byte destination and read fd are owned; `read` reports failures via errno.
            let rc = unsafe { libc::read(self.read_end.as_raw_fd(), byte.as_mut_ptr().cast(), 1) };
            if rc == 0 {
                true
            } else if rc < 0 {
                let errno = std::io::Error::last_os_error().raw_os_error();
                debug_assert!(
                    errno == Some(libc::EAGAIN) || errno == Some(libc::EWOULDBLOCK),
                    "reading the observer pipe failed with an errno other than \
                     EAGAIN/EWOULDBLOCK: {errno:?}"
                );
                false
            } else {
                false
            }
        }
    }

    /// Creates a close-on-exec pipe and returns its raw read and write fds.
    ///
    /// The caller owns both descriptors.
    ///
    /// # Panics
    ///
    /// Panics if `pipe2` fails.
    #[must_use]
    pub fn make_pipe() -> (RawFd, RawFd) {
        let mut fds = [0 as RawFd; 2];
        // SAFETY: `fds` is a 2-element array, exactly what pipe2(2) writes
        // into. O_CLOEXEC prevents a concurrent spawn from inheriting and
        // pinning these transient control fds open.
        let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
        assert_eq!(
            rc,
            0,
            "pipe2(2) failed: {}",
            std::io::Error::last_os_error()
        );
        (fds[0], fds[1])
    }

    /// Closes a raw control fd and ignores the syscall result.
    pub fn close_raw(raw: RawFd) {
        // SAFETY: close accepts any int; a bad fd yields EBADF, not UB.
        unsafe {
            libc::close(raw);
        }
    }
}

#[cfg(test)]
mod tests {

    use super::test_support::ClosureObserver;
    use super::*;

    #[test]
    fn success_path_drop_closes_the_fd_exactly_once() {
        let (observer, write_end) = ClosureObserver::new();
        let subject_raw = write_end.as_raw_fd();
        {
            let remote =
                PipeWireRemote::from_owned_fd(write_end).expect("adoption must set FD_CLOEXEC");
            assert_eq!(
                remote.as_raw_fd(),
                subject_raw,
                "as_raw_fd must report the adopted number"
            );
            assert!(
                !observer.subject_released(),
                "the fd must be live while the remote holds it"
            );
        }
        assert!(
            observer.subject_released(),
            "SUCCESS PATH: the fd must be RELEASED after the remote drops"
        );
    }
}
