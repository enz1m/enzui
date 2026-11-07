//! A bounded, interruptible outbound Unix-socket connect for connector setup.
//!
//! `UnixStream::connect` blocks inside `connect(2)` with no deadline and no way
//! to interrupt it, which a valid private daemon socket whose kernel accept
//! queue is full reaches: the daemon's own bounded accept loop cannot guarantee
//! capacity in the listen backlog. The socket here is non-blocking instead, so
//! `connect(2)` reports one of two pending states that mean different things:
//!
//! - `EINPROGRESS`: the connection is under way and the kernel signals its
//!   completion as writability, so one `poll(2)` covers the socket and the wake.
//! - `EAGAIN`: on a Unix-domain socket this is "the listener's accept queue is
//!   full" (`unix_recvq_full`) and nothing was initiated. An unconnected
//!   `AF_UNIX` socket reports `POLLOUT` at once, so no readiness event can mean
//!   "a slot opened" and the only truthful handling is a later attempt, paused
//!   on the wake so nothing spins.
//!
//! The wake is a self-pipe rather than a condition variable because the
//! `EINPROGRESS` wait must observe socket readiness and the shutdown request in
//! one blocking call.
//!
//! The socket flags, `sockaddr_un` layout and `nfds_t` width here are Linux's;
//! as with `peer::peer_credential`, the wrappers report
//! `Unsupported` elsewhere rather than address a foreign kernel by guess.

use core::ffi::{c_int, c_short, c_ulong};
use std::io::{self, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

// The `sun_path` capacity of Linux's `sockaddr_un`, terminating NUL included.
const MAX_SUN_PATH: usize = 108;

// The first pause after a full accept queue, and the ceiling it doubles toward.
// The total is bounded by the caller's setup deadline, not by these.
const FIRST_RETRY_DELAY: Duration = Duration::from_millis(10);
const MAX_RETRY_DELAY: Duration = Duration::from_millis(100);

// Waits that a signal cut short and this module retried. Counting them lets a
// bound be checked against interruptions that actually happened rather than
// inferred from elapsed time.
static POLL_INTERRUPTIONS: AtomicU64 = AtomicU64::new(0);

// A one-shot, cross-thread wake a bounded wait can observe.
//
// The byte is written once and never drained, so a thread that had not begun
// waiting when shutdown was requested still observes it, and the pipe buffer
// cannot fill.
pub(crate) struct ShutdownWake {
    reader: io::PipeReader,
    writer: io::PipeWriter,
    tripped: AtomicBool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wake {
    Tripped,
    Elapsed,
}

impl ShutdownWake {
    // A connector that cannot build the pipe has no way to interrupt a connect,
    // so the failure is reported rather than downgraded to an un-interruptible
    // setup.
    pub(crate) fn new() -> io::Result<Self> {
        let (reader, writer) = io::pipe()?;
        Ok(Self {
            reader,
            writer,
            tripped: AtomicBool::new(false),
        })
    }

    // Trips the wake permanently, ending any current or future wait.
    pub(crate) fn signal(&self) {
        if !self.tripped.swap(true, Ordering::SeqCst) {
            let mut writer = &self.writer;
            let _ = writer.write(&[1]);
        }
    }

    pub(crate) fn is_tripped(&self) -> bool {
        self.tripped.load(Ordering::SeqCst)
    }

    // Waits up to `timeout`, returning as soon as the wake trips. The bound is
    // absolute from here on, so signals cannot stretch it. A `poll(2)` failure
    // is reported rather than treated as elapsed time, which a caller would
    // otherwise retry into a spin.
    pub(crate) fn wait(&self, timeout: Duration) -> io::Result<Wake> {
        if self.is_tripped() {
            return Ok(Wake::Tripped);
        }
        let ready = poll_ready(None, self.reader.as_fd(), Instant::now() + timeout)?;
        Ok(if ready.wake {
            Wake::Tripped
        } else {
            Wake::Elapsed
        })
    }
}

// Why a bounded connect produced no connected socket. Only `Unaddressable`
// describes a path that can never work; the rest are worth another attempt.
#[derive(Debug)]
pub(crate) enum ConnectError {
    Interrupted,
    TimedOut(Pending),
    Failed(io::Error),
    Unaddressable(&'static str),
}

// What a connect was still waiting for: `EAGAIN` (no queue slot) or
// `EINPROGRESS` (an unfinished connection).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pending {
    AcceptQueueFull,
    Connecting,
}

impl core::fmt::Display for Pending {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::AcceptQueueFull => write!(f, "the listener's accept queue stayed full"),
            Self::Connecting => write!(f, "the connection stayed pending"),
        }
    }
}

// A validated `sockaddr_un` pathname: NUL-terminated by its zero padding, with
// `len` excluding that terminator.
pub(crate) struct SocketAddress {
    path: [u8; MAX_SUN_PATH],
    len: usize,
}

impl SocketAddress {
    // Refuses what cannot be a filesystem Unix address before a descriptor
    // exists: an empty path, an interior NUL (which would silently shorten the
    // address the kernel reads), or no room in `sun_path` for the terminator.
    pub(crate) fn for_path(path: &Path) -> Result<Self, &'static str> {
        use std::os::unix::ffi::OsStrExt;

        let bytes = path.as_os_str().as_bytes();
        if bytes.is_empty() {
            return Err("socket path is empty");
        }
        if bytes.contains(&0) {
            return Err("socket path contains a NUL byte");
        }
        if bytes.len() >= MAX_SUN_PATH {
            return Err("socket path is too long for a Unix address");
        }
        let mut buffer = [0u8; MAX_SUN_PATH];
        buffer[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            path: buffer,
            len: bytes.len(),
        })
    }
}

// What one non-blocking `connect(2)` produced. `AcceptQueueFull` carries no
// socket: nothing was initiated, and a retry needs a fresh one.
pub(crate) enum Attempt {
    Connected(UnixStream),
    Connecting(UnixStream),
    AcceptQueueFull,
}

// One attempt, with no waiting and no policy, so a fixture can assert the
// kernel's `EAGAIN` without depending on the deadline and interruption that
// `connect_within` layers on top.
pub(crate) fn begin_connect(address: &SocketAddress) -> io::Result<Attempt> {
    // Linux `EINPROGRESS`. `io::ErrorKind` has no stable variant for it, and
    // `WouldBlock` (`EAGAIN`) means something else entirely here.
    const EINPROGRESS: i32 = 115;

    let stream = create_socket()?;
    match start_connect(&stream, address) {
        Ok(()) => Ok(Attempt::Connected(stream)),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(Attempt::AcceptQueueFull),
        Err(error) if error.raw_os_error() == Some(EINPROGRESS) => Ok(Attempt::Connecting(stream)),
        Err(error) => Err(error),
    }
}

// Connects to `path` before `deadline`, returning as soon as `wake` trips.
//
// The connected socket is blocking: the handshake that follows bounds itself
// with read and write timeouts, which the kernel applies only to a blocking
// socket.
pub(crate) fn connect_within(
    path: &Path,
    deadline: Instant,
    wake: &ShutdownWake,
) -> Result<UnixStream, ConnectError> {
    let address = SocketAddress::for_path(path).map_err(ConnectError::Unaddressable)?;
    let mut delay = FIRST_RETRY_DELAY;
    let mut pending = Pending::Connecting;

    loop {
        if wake.is_tripped() {
            return Err(ConnectError::Interrupted);
        }
        if remaining_until(deadline).is_none() {
            return Err(ConnectError::TimedOut(pending));
        }

        match begin_connect(&address) {
            Ok(Attempt::Connected(stream)) => return into_blocking(stream),
            Ok(Attempt::Connecting(stream)) => {
                pending = Pending::Connecting;
                let ready = poll_ready(Some(stream.as_fd()), wake.reader.as_fd(), deadline)
                    .map_err(ConnectError::Failed)?;
                if ready.wake {
                    return Err(ConnectError::Interrupted);
                }
                if ready.socket && connection_completed(&stream).map_err(ConnectError::Failed)? {
                    return into_blocking(stream);
                }
            }
            Ok(Attempt::AcceptQueueFull) => pending = Pending::AcceptQueueFull,
            // A signal caught inside `connect(2)` established nothing, so the
            // deadline check above governs the next attempt.
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(ConnectError::Failed(error)),
        }

        // Neither pending outcome has a readiness event meaning "there is room
        // now", so the next attempt waits out a bounded, growing pause that
        // shutdown ends immediately.
        let pause = remaining_until(deadline)
            .unwrap_or(Duration::ZERO)
            .min(delay);
        if wake.wait(pause).map_err(ConnectError::Failed)? == Wake::Tripped {
            return Err(ConnectError::Interrupted);
        }
        delay = (delay * 2).min(MAX_RETRY_DELAY);
    }
}

// `None` once `deadline` has passed, so an exhausted budget is a state the
// caller reports rather than another syscall with a zero timeout.
fn remaining_until(deadline: Instant) -> Option<Duration> {
    let now = Instant::now();
    if now >= deadline {
        return None;
    }
    Some(deadline.saturating_duration_since(now))
}

// Whether a socket reported writable finished connecting.
//
// `take_error` is `SO_ERROR`, the kernel's verdict on the attempt; a writable
// socket with no error and no peer is not connected yet.
fn connection_completed(stream: &UnixStream) -> io::Result<bool> {
    if let Some(error) = stream.take_error()? {
        return Err(error);
    }
    match stream.peer_addr() {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotConnected => Ok(false),
        Err(error) => Err(error),
    }
}

fn into_blocking(stream: UnixStream) -> Result<UnixStream, ConnectError> {
    stream
        .set_nonblocking(false)
        .map_err(ConnectError::Failed)?;
    Ok(stream)
}

#[derive(Clone, Copy, Debug)]
struct Readiness {
    // The pending connection is writable, failed, or hung up.
    socket: bool,
    wake: bool,
}

// `socket(2)` for a close-on-exec, non-blocking `AF_UNIX` stream socket. Both
// flags belong to the creating call: a descriptor that is briefly inheritable
// can leak into a concurrent `exec`, and a blocking window is what this module
// exists to remove.
#[cfg(target_os = "linux")]
#[allow(unsafe_code, reason = "`socket(2)` with flags std does not expose")]
fn create_socket() -> io::Result<UnixStream> {
    use std::os::fd::{FromRawFd, OwnedFd};

    const AF_UNIX: c_int = 1;
    const SOCK_STREAM: c_int = 1;
    // Linux gives `SOCK_CLOEXEC` and `SOCK_NONBLOCK` the values of `O_CLOEXEC`
    // and `O_NONBLOCK`.
    const SOCK_CLOEXEC: c_int = 0o2_000_000;
    const SOCK_NONBLOCK: c_int = 0o4_000;

    unsafe extern "C" {
        fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int;
    }

    // SAFETY: `socket` takes three integers by value and returns a descriptor
    // or -1; no pointer crosses the boundary and nothing outlives the call.
    let fd = unsafe { socket(AF_UNIX, SOCK_STREAM | SOCK_CLOEXEC | SOCK_NONBLOCK, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` is a descriptor this call just created, is not -1, and no
    // other owner exists, so `OwnedFd` closes it exactly once.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    Ok(UnixStream::from(owned))
}

// Starts `connect(2)` on a non-blocking socket. The pending outcomes are the
// caller's to classify, so nothing is retried or reinterpreted here.
#[cfg(target_os = "linux")]
#[allow(
    unsafe_code,
    reason = "a non-blocking `connect(2)` std does not expose"
)]
fn start_connect(stream: &UnixStream, address: &SocketAddress) -> io::Result<()> {
    // `struct sockaddr_un` from `<sys/un.h>`: a 16-bit `sa_family_t` followed
    // by `sun_path`. `socklen_t` is 32-bit.
    #[repr(C)]
    struct SockaddrUn {
        sun_family: u16,
        sun_path: [u8; MAX_SUN_PATH],
    }

    const AF_UNIX: u16 = 1;

    unsafe extern "C" {
        fn connect(fd: c_int, address: *const SockaddrUn, length: u32) -> c_int;
    }

    let addr = SockaddrUn {
        sun_family: AF_UNIX,
        sun_path: address.path,
    };
    // The kernel reads the pathname up to `length`, so it covers the family,
    // the path, and its terminating NUL — never the whole zero padding, which
    // would name a different (trailing-NUL) address on some kernels.
    let offset = core::mem::offset_of!(SockaddrUn, sun_path);
    let length = u32::try_from(offset + address.len + 1)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "socket address length"))?;

    // SAFETY: `connect` receives this frame's initialized `SockaddrUn` by
    // const pointer with a length that is at most its size (`address.len` is
    // below `MAX_SUN_PATH`, checked when the address was validated), plus a
    // descriptor borrowed from `stream` for the duration of the call. The
    // pointer does not escape.
    let result = unsafe { connect(stream.as_raw_fd(), &raw const addr, length) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

// Waits for the optional socket's writability or the wake, whichever comes
// first, until `deadline`.
//
// An `EINTR` established nothing, so it is retried rather than reported as a
// broken wait; the retry's timeout is recomputed from `deadline`, so a stream
// of signals cannot hand the wait a fresh budget each time and outlast the
// caller's total. An expired deadline polls with a zero timeout, which reports
// what is already ready and terminates.
#[cfg(target_os = "linux")]
#[allow(unsafe_code, reason = "`poll(2)` over two descriptors")]
fn poll_ready(
    socket: Option<BorrowedFd<'_>>,
    wake: BorrowedFd<'_>,
    deadline: Instant,
) -> io::Result<Readiness> {
    // `poll(2)` event bits, and Linux's `nfds_t` (`unsigned long`).
    const POLLIN: c_short = 0x001;
    const POLLOUT: c_short = 0x004;
    const POLLERR: c_short = 0x008;
    const POLLHUP: c_short = 0x010;

    #[repr(C)]
    struct PollFd {
        fd: c_int,
        events: c_short,
        revents: c_short,
    }

    unsafe extern "C" {
        fn poll(fds: *mut PollFd, count: c_ulong, timeout: c_int) -> c_int;
    }

    let mut fds = [
        PollFd {
            fd: wake.as_raw_fd(),
            events: POLLIN,
            revents: 0,
        },
        PollFd {
            fd: -1,
            events: POLLOUT,
            revents: 0,
        },
    ];
    let count: c_ulong = match socket {
        Some(fd) => {
            fds[1].fd = fd.as_raw_fd();
            2
        }
        None => 1,
    };
    loop {
        let millis = timeout_millis(remaining_until(deadline).unwrap_or(Duration::ZERO));
        // SAFETY: `poll` receives a pointer to `count` initialized `PollFd`
        // entries owned by this frame, where `count` is 1 or 2 and the array
        // holds 2. The descriptors are borrowed for the whole call through
        // `BorrowedFd`, and the pointer does not escape.
        let ready = unsafe { poll(fds.as_mut_ptr(), count, millis) };
        if ready >= 0 {
            return Ok(Readiness {
                wake: fds[0].revents & (POLLIN | POLLERR | POLLHUP) != 0,
                socket: count == 2 && fds[1].revents & (POLLOUT | POLLERR | POLLHUP) != 0,
            });
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
        POLL_INTERRUPTIONS.fetch_add(1, Ordering::SeqCst);
        fds[0].revents = 0;
        fds[1].revents = 0;
    }
}

// `poll(2)` takes whole milliseconds and reads a negative value as "block
// forever", so a sub-millisecond remainder rounds *up* to one (rounding down to
// zero would spin) and an enormous budget clamps instead of wrapping.
#[cfg(target_os = "linux")]
fn timeout_millis(timeout: Duration) -> c_int {
    let ceiling = u128::try_from(c_int::MAX).unwrap_or(u128::MAX);
    let millis = timeout.as_nanos().div_ceil(1_000_000).min(ceiling);
    c_int::try_from(millis).unwrap_or(c_int::MAX)
}

// Non-Linux stub: this module's address layout and socket flags are Linux's.
#[cfg(not(target_os = "linux"))]
fn create_socket() -> io::Result<UnixStream> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "interruptible Unix connect is implemented for Linux only",
    ))
}

// Non-Linux stub: see `create_socket`.
#[cfg(not(target_os = "linux"))]
fn start_connect(_stream: &UnixStream, _address: &SocketAddress) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "interruptible Unix connect is implemented for Linux only",
    ))
}

// Non-Linux stub: see `create_socket`.
#[cfg(not(target_os = "linux"))]
fn poll_ready(
    _socket: Option<BorrowedFd<'_>>,
    _wake: BorrowedFd<'_>,
    _deadline: Instant,
) -> io::Result<Readiness> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "interruptible Unix connect is implemented for Linux only",
    ))
}
