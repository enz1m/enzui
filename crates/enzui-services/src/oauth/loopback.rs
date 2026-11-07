//! Browser redirect reception on an ephemeral IPv4 loopback port.
//!
//! The listener accepts get requests naming its exact path and Host address.
//! [`LoopbackRedirect::wait_for`] also applies a caller predicate; use
//! [`super::PendingAuthorization::is_callback`] to validate redirect and state.
//! [`LoopbackRedirect::wait`] accepts any otherwise matching request.
//!
//! Run waits off the UI thread. Read readiness is multiplexed across bounded
//! connections, but response writes and oversized-request drains are synchronous.
//! Deadlines and cancellation are checked between operations; custom predicates,
//! response processing, and sustained connection admission can delay them.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use enzui_security::secret::SecretText;

/// Age budget for a request head, checked at outer-loop boundaries; defaults to 2 s.
pub const CONNECTION_READ_BUDGET: Duration = Duration::from_secs(2);

const DEFAULT_PAGE: &str = "<!doctype html><html><head><meta charset=\"utf-8\">\
<title>Signed in</title></head><body style=\"font-family:sans-serif;margin:3em\">\
<p>You are signed in. You can close this tab and return to the app.</p></body></html>";

/// Why the wait ended without a callback.
#[derive(Debug)]
#[non_exhaustive]
pub enum LoopbackError {
    /// Socket setup, accepting, or readiness polling failed.
    Io(io::Error),
    /// The deadline passed.
    TimedOut,
    /// [`LoopbackCancel::cancel`] was called.
    Cancelled,
}

impl fmt::Display for LoopbackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "OAuth redirect listener failed: {error}"),
            Self::TimedOut => f.write_str("no OAuth redirect arrived before the deadline"),
            Self::Cancelled => f.write_str("OAuth redirect wait was cancelled"),
        }
    }
}

impl std::error::Error for LoopbackError {}

impl From<io::Error> for LoopbackError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// A bound redirect listener. Its [`redirect_uri`](Self::redirect_uri) goes
/// into [`ClientConfig::begin`](super::ClientConfig::begin).
pub struct LoopbackRedirect {
    listener: TcpListener,
    address: SocketAddr,
    path: String,
    redirect_uri: String,
    page: String,
    cancelled: Arc<AtomicBool>,
}

impl fmt::Debug for LoopbackRedirect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoopbackRedirect")
            .field("redirect_uri", &self.redirect_uri)
            .finish_non_exhaustive()
    }
}

/// Ends a [`LoopbackRedirect::wait`] from another thread.
#[derive(Debug, Clone)]
pub struct LoopbackCancel {
    address: SocketAddr,
    cancelled: Arc<AtomicBool>,
}

impl LoopbackCancel {
    /// Signal cancellation and attempt to wake the listener. The wake connection
    /// may block for up to 200 ms; the wait observes cancellation between operations.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        // Connecting wakes a listener blocked in poll so it can observe cancellation.
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(200));
    }
}

impl LoopbackRedirect {
    /// Binds `127.0.0.1` on a port the kernel picks. `path` is the redirect
    /// path, such as `/oauth2/callback`; it must start with `/`, contain only
    /// ASCII graphic bytes, and hold no query or fragment.
    ///
    /// # Errors
    /// Returns [`LoopbackError::Io`] for an invalid path or socket setup failure.
    pub fn bind(path: &str) -> Result<Self, LoopbackError> {
        let path_ok = path.starts_with('/')
            && !path.contains(['?', '#'])
            && path.bytes().all(|b| b.is_ascii_graphic());
        if !path_ok {
            return Err(LoopbackError::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "redirect path must be an absolute path with no query or fragment",
            )));
        }
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        Ok(Self {
            redirect_uri: format!("http://{address}{path}"),
            listener,
            address,
            path: path.to_owned(),
            page: DEFAULT_PAGE.to_owned(),
            cancelled: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Set the HTML returned when the predicate accepts a callback, including
    /// provider-error callbacks. Content and length are not validated.
    #[must_use]
    pub fn with_page(mut self, html: impl Into<String>) -> Self {
        self.page = html.into();
        self
    }

    /// The redirect URI to register and send: `http://127.0.0.1:PORT/path`.
    #[must_use]
    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// A handle that ends the wait.
    #[must_use]
    pub fn canceller(&self) -> LoopbackCancel {
        LoopbackCancel {
            address: self.address,
            cancelled: Arc::clone(&self.cancelled),
        }
    }

    /// Blocks until the browser requests the redirect path, then returns the
    /// full callback URL for
    /// [`PendingAuthorization::complete`](super::PendingAuthorization::complete).
    ///
    /// Prefer [`wait_for`](Self::wait_for) with
    /// [`PendingAuthorization::is_callback`](super::PendingAuthorization::is_callback):
    /// this accepts the first request to the path, so any local process can
    /// end the wait with a forged one.
    ///
    /// # Errors
    /// Returns [`LoopbackError`] on socket failure, deadline expiry, or cancellation.
    pub fn wait(self, deadline: Instant) -> Result<SecretText, LoopbackError> {
        self.wait_for(deadline, |_| true)
    }

    /// Block until `accept` approves a matching get request, returning its URL.
    /// Rejected callbacks receive 400 and the listener keeps waiting. The first
    /// Host header must match this socket address exactly.
    ///
    /// At most [`MAX_OPEN_CONNECTIONS`] request heads are retained. Expired
    /// connections are dropped at outer-loop checks; when full, idle connections
    /// can be evicted, while a new connection is dropped if none is idle.
    /// Oversized incomplete heads receive 431. Replies are attempted before
    /// returning a callback; write failures do not change the successful result.
    ///
    /// # Errors
    /// Returns [`LoopbackError`] on poll/accept failure, observed deadline expiry,
    /// or observed cancellation. A slow predicate or response can delay checks.
    ///
    /// # Panics
    /// Propagates a panic from `accept`.
    pub fn wait_for(
        self,
        deadline: Instant,
        accept: impl Fn(&SecretText) -> bool,
    ) -> Result<SecretText, LoopbackError> {
        let mut open: Vec<Connection> = Vec::new();
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                return Err(LoopbackError::Cancelled);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(LoopbackError::TimedOut);
            }
            open.retain(|connection| connection.deadline > now);
            let wake = open
                .iter()
                .map(|connection| connection.deadline)
                .chain(std::iter::once(deadline))
                .min()
                .unwrap_or(deadline);
            let timeout = rustix::event::Timespec::try_from(wake.saturating_duration_since(now))
                .unwrap_or(rustix::event::Timespec {
                    tv_sec: i64::MAX,
                    tv_nsec: 0,
                });
            let ready: Vec<bool> = {
                let mut fds = Vec::with_capacity(open.len() + 1);
                fds.push(rustix::event::PollFd::new(
                    &self.listener,
                    rustix::event::PollFlags::IN,
                ));
                for connection in &open {
                    fds.push(rustix::event::PollFd::new(
                        &connection.stream,
                        rustix::event::PollFlags::IN,
                    ));
                }
                match rustix::event::poll(&mut fds, Some(&timeout)) {
                    Ok(_) => {}
                    Err(rustix::io::Errno::INTR) => continue,
                    Err(error) => return Err(LoopbackError::Io(error.into())),
                }
                fds.iter().map(|fd| !fd.revents().is_empty()).collect()
            };
            let mut index = open.len();
            while index > 0 {
                index -= 1;
                if !ready[index + 1] {
                    continue;
                }
                match open[index].read() {
                    Progress::Pending => {}
                    Progress::Closed => {
                        open.swap_remove(index);
                    }
                    Progress::Head(head) => {
                        let mut connection = open.swap_remove(index);
                        if let Some(callback) = self.answer(&mut connection.stream, &head, &accept)
                        {
                            return Ok(callback);
                        }
                    }
                    Progress::TooLarge => {
                        let mut connection = open.swap_remove(index);
                        respond(
                            &mut connection.stream,
                            "431 Request Header Fields Too Large",
                            "",
                        );
                        drain_before_close(connection.stream);
                    }
                }
            }
            if ready[0] {
                loop {
                    match self.listener.accept() {
                        Ok((stream, _)) => {
                            if self.cancelled.load(Ordering::Acquire) {
                                return Err(LoopbackError::Cancelled);
                            }
                            if stream.set_nonblocking(true).is_err() {
                                continue;
                            }
                            if open.len() >= MAX_OPEN_CONNECTIONS {
                                // Preserve connections already sending bytes over silent preconnects.
                                match open.iter().position(Connection::is_idle) {
                                    Some(idle) => {
                                        open.remove(idle);
                                    }
                                    None => continue,
                                }
                            }
                            open.push(Connection {
                                stream,
                                head: Vec::new(),
                                deadline: Instant::now() + CONNECTION_READ_BUDGET,
                            });
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) => return Err(LoopbackError::Io(error)),
                    }
                }
            }
        }
    }

    fn answer(
        &self,
        stream: &mut TcpStream,
        head: &[u8],
        accept: &impl Fn(&SecretText) -> bool,
    ) -> Option<SecretText> {
        let Ok(text) = std::str::from_utf8(head) else {
            respond(stream, "400 Bad Request", "");
            return None;
        };
        let mut lines = text.split("\r\n");
        let line = lines.next().unwrap_or_default();
        let mut parts = line.split(' ');
        let (Some(method), Some(target), Some(version), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            respond(stream, "400 Bad Request", "");
            return None;
        };
        if !version.starts_with("HTTP/1.") {
            respond(stream, "400 Bad Request", "");
            return None;
        }
        let host = lines.find_map(|header| {
            let (name, value) = header.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("host")
                .then(|| value.trim().to_owned())
        });
        if host.as_deref() != Some(&self.address.to_string()) {
            respond(stream, "400 Bad Request", "");
            return None;
        }
        let path = target.split(['?', '#']).next().unwrap_or_default();
        if path != self.path {
            respond(stream, "404 Not Found", "");
            return None;
        }
        if method != "GET" {
            respond(stream, "405 Method Not Allowed", "");
            return None;
        }
        let callback = SecretText::new(format!("http://{}{target}", self.address));
        if !accept(&callback) {
            respond(stream, "400 Bad Request", REFUSED_PAGE);
            return None;
        }
        respond(stream, "200 OK", &self.page);
        Some(callback)
    }
}

/// The most connections read at once.
pub const MAX_OPEN_CONNECTIONS: usize = 16;

/// Incomplete request-head threshold in bytes, including request line and headers.
/// Checked after 4096-byte reads, after checking for a terminator: a completed
/// head in the threshold-crossing chunk can be accepted beyond this threshold.
pub const MAX_REQUEST_HEAD_BYTES: usize = 64 * 1024;

const REFUSED_PAGE: &str = "<!doctype html><html><head><meta charset=\"utf-8\">\
<title>Not this sign-in</title></head><body style=\"font-family:sans-serif;margin:3em\">\
<p>This is not the sign-in the app is waiting for. Start the sign-in again from the app.</p>\
</body></html>";

struct Connection {
    stream: TcpStream,
    head: Vec<u8>,
    deadline: Instant,
}

enum Progress {
    Pending,
    Closed,
    Head(Vec<u8>),
    TooLarge,
}

impl Connection {
    fn is_idle(&self) -> bool {
        self.head.is_empty()
            && matches!(
                self.stream.peek(&mut [0u8; 1]),
                Err(ref error) if error.kind() == io::ErrorKind::WouldBlock
            )
    }

    fn read(&mut self) -> Progress {
        let mut chunk = [0u8; 4096];
        loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => return Progress::Closed,
                Ok(n) => {
                    self.head.extend_from_slice(&chunk[..n]);
                    if let Some(end) = self.head.windows(4).position(|w| w == b"\r\n\r\n") {
                        self.head.truncate(end);
                        return Progress::Head(std::mem::take(&mut self.head));
                    }
                    if self.head.len() > MAX_REQUEST_HEAD_BYTES {
                        return Progress::TooLarge;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    return Progress::Pending;
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Progress::Closed,
            }
        }
    }
}

// Half-close before draining: unread input can reset the socket and discard the reply.
fn drain_before_close(mut stream: TcpStream) {
    // Limit draining by both bytes and time so trickling input cannot hold the listener indefinitely.
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let until = Instant::now() + Duration::from_millis(300);
    let mut sink = [0u8; 8192];
    let mut drained = 0usize;
    while drained < 1024 * 1024 {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() || stream.set_read_timeout(Some(left)).is_err() {
            break;
        }
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(n) => drained += n,
        }
    }
}

fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    // Per-write timeouts limit blocking writes; custom page size has no bound.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
}
