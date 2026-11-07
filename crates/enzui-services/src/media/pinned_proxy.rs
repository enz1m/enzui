//! Authenticated loopback proxy that connects GStreamer only to an approved peer.
//!
//! Connection establishment obeys [`TransportLimits`]; established relays remain active until a peer closes or the worker cancels.

use std::io::{Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::remote_gate::ApprovedStream;
use super::{MediaError, MediaErrorKind};
use crate::url_policy::{NO_LIMIT, Scheme, TransportLimits};
use enzui_security::loopback_proxy::{LoopbackProxyCapability, PROXY_AUTHENTICATE_VALUE};

const POLL: Duration = Duration::from_millis(5);
const DROP_JOIN_BUDGET: Duration = Duration::from_millis(100);
const HEADER_LIMIT: usize = 16 * 1024;
// Bound concurrent handshakes so partial unauthenticated requests cannot grow workers or monopolize admission.
const ADMISSION_SLOTS: usize = 4;
const RELAY_BUFFER: usize = 64 * 1024;
// Release an orphaned upstream after this idle period when no read budget exists.
const ORPHAN_IDLE_FALLBACK_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Endpoint {
    Client,
    Upstream,
}

type NonblockingSetup<'a> = &'a (dyn Fn(Endpoint, &TcpStream) -> std::io::Result<()> + Send + Sync);

type SharedNonblockingSetup =
    Arc<dyn Fn(Endpoint, &TcpStream) -> std::io::Result<()> + Send + Sync>;

// Unauthenticated slots may be evicted; authenticated relays are never evicted.
struct Admission {
    started: Instant,
    authenticated: AtomicBool,
    cancel: AtomicBool,
}

impl Admission {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            authenticated: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
}

struct ServeContext {
    peer: SocketAddr,
    expected_host: String,
    expected_port: u16,
    scheme: Scheme,
    limits: TransportLimits,
    capability: LoopbackProxyCapability,
    connections: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<MediaError>>>,
    poisoned: Arc<AtomicBool>,
    set_nonblocking: SharedNonblockingSetup,
}

#[derive(Clone)]
pub(crate) struct PinnedProxyClient {
    uri: String,
    capability: LoopbackProxyCapability,
}

impl std::fmt::Debug for PinnedProxyClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PinnedProxyClient")
            .field("uri", &self.uri)
            .field("credentials", &"<redacted>")
            .finish()
    }
}

impl PinnedProxyClient {
    pub(super) fn new(address: SocketAddr, capability: LoopbackProxyCapability) -> Self {
        Self {
            uri: capability.display_uri(address),
            capability,
        }
    }

    pub(super) fn uri(&self) -> &str {
        &self.uri
    }

    pub(super) fn username(&self) -> &'static str {
        self.capability.username()
    }

    pub(super) fn password(&self) -> &str {
        self.capability.password()
    }
}

#[derive(Debug)]
pub(crate) struct PinnedProxy {
    address: SocketAddr,
    client: PinnedProxyClient,
    stop: Arc<AtomicBool>,
    error: Arc<Mutex<Option<MediaError>>>,
    connections: Arc<AtomicU64>,
    thread: Option<JoinHandle<()>>,
}

impl PinnedProxy {
    pub(crate) fn start(approved: &ApprovedStream) -> Result<Self, MediaError> {
        Self::start_with_nonblocking(approved, |_, stream| stream.set_nonblocking(true))
    }

    fn start_with_nonblocking(
        approved: &ApprovedStream,
        set_nonblocking: impl Fn(Endpoint, &TcpStream) -> std::io::Result<()> + Send + Sync + 'static,
    ) -> Result<Self, MediaError> {
        // A listener must never exist without its authenticator. There is no
        // weaker fallback when OS entropy is unavailable.
        let capability = LoopbackProxyCapability::mint()
            .map_err(|_| refused("could not mint the pinned media proxy capability"))?;
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .map_err(|_| refused("could not bind the pinned media proxy"))?;
        listener
            .set_nonblocking(true)
            .map_err(|_| refused("could not make the pinned media proxy cancellable"))?;
        let address = listener
            .local_addr()
            .map_err(|_| refused("could not inspect the pinned media proxy"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let error = Arc::new(Mutex::new(None));
        let connections = Arc::new(AtomicU64::new(0));
        let context = Arc::new(ServeContext {
            peer: SocketAddr::new(approved.peer(), approved.port()),
            expected_host: approved.host().to_ascii_lowercase(),
            expected_port: approved.port(),
            scheme: approved.scheme(),
            limits: approved.limits(),
            capability: capability.clone(),
            connections: Arc::clone(&connections),
            stop: Arc::clone(&stop),
            error: Arc::clone(&error),
            poisoned: Arc::new(AtomicBool::new(false)),
            set_nonblocking: Arc::new(set_nonblocking),
        });
        let thread = thread::Builder::new()
            .name("enzui-media-pin".to_owned())
            .spawn(move || accept_loop(&listener, &context))
            .map_err(|_| refused("could not start the pinned media proxy"))?;
        Ok(Self {
            address,
            client: PinnedProxyClient::new(address, capability),
            stop,
            error,
            connections,
            thread: Some(thread),
        })
    }

    pub(super) fn client_configuration(&self) -> PinnedProxyClient {
        self.client.clone()
    }

    // Counts requests only after capability, parsing, and destination validation succeed.
    pub(crate) fn connections(&self) -> u64 {
        self.connections.load(Ordering::Acquire)
    }

    pub(crate) fn error(&self) -> Option<MediaError> {
        match self.error.lock() {
            Ok(error) => error.clone(),
            Err(_) => Some(refused("the pinned media proxy state was poisoned")),
        }
    }
}

impl Drop for PinnedProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect_timeout(&self.address, Duration::from_millis(20));
        if let Some(handle) = self.thread.take() {
            let started = Instant::now();
            while !handle.is_finished() && started.elapsed() < DROP_JOIN_BUDGET {
                thread::sleep(POLL);
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

// Only failures after capability validation poison the intended media stream.
enum ServeFailure {
    PreAuthentication,
    Authenticated(MediaError),
}

impl From<MediaError> for ServeFailure {
    fn from(error: MediaError) -> Self {
        Self::Authenticated(error)
    }
}

// Admit connections concurrently within a fixed bound, evicting only the oldest unauthenticated handshake.
fn accept_loop(listener: &TcpListener, context: &Arc<ServeContext>) {
    let mut admissions: Vec<(Arc<Admission>, JoinHandle<()>)> = Vec::new();
    while !context.stop.load(Ordering::Acquire) && !context.poisoned.load(Ordering::Acquire) {
        reap(&mut admissions);
        if admissions.len() >= ADMISSION_SLOTS {
            evict_oldest_unauthenticated(&admissions);
            thread::sleep(POLL);
            continue;
        }
        match listener.accept() {
            Ok((client, _)) => {
                let admission = Arc::new(Admission::new());
                let handler_admission = Arc::clone(&admission);
                let handler_context = Arc::clone(context);
                match thread::Builder::new()
                    .name("enzui-media-pin-admit".to_owned())
                    .spawn(move || serve_admitted(client, &handler_context, &handler_admission))
                {
                    Ok(handle) => admissions.push((admission, handle)),
                    Err(_) => {
                        // Refusing an admission is a local resource failure, not
                        // an intended-client failure: the accepted socket was
                        // moved into (and dropped with) the failed closure, so
                        // it is already closed; keep serving.
                        thread::sleep(POLL);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(_) => {
                record_error(&context.error, refused("the pinned media proxy failed"));
                break;
            }
        }
    }
    context.stop.store(true, Ordering::Release);
    for (_, handle) in admissions {
        let _ = handle.join();
    }
}

fn reap(admissions: &mut Vec<(Arc<Admission>, JoinHandle<()>)>) {
    let mut index = 0;
    while index < admissions.len() {
        if admissions[index].1.is_finished() {
            let (_, handle) = admissions.swap_remove(index);
            let _ = handle.join();
        } else {
            index += 1;
        }
    }
}

// Cancels the oldest unauthenticated handshake; authenticated relays are ineligible.
fn evict_oldest_unauthenticated(admissions: &[(Arc<Admission>, JoinHandle<()>)]) {
    if let Some((oldest, _)) = admissions
        .iter()
        .filter(|(admission, _)| !admission.authenticated.load(Ordering::Acquire))
        .min_by_key(|(admission, _)| admission.started)
    {
        oldest.cancel.store(true, Ordering::Release);
    }
}

fn serve_admitted(client: TcpStream, context: &ServeContext, admission: &Admission) {
    match serve_one(client, context, admission) {
        Ok(()) | Err(ServeFailure::PreAuthentication) => {}
        Err(ServeFailure::Authenticated(error)) => {
            record_error(&context.error, error);
            context.poisoned.store(true, Ordering::Release);
        }
    }
}

// Before authentication, failures affect only the local client; afterwards they fail the intended stream.
fn serve_one(
    mut client: TcpStream,
    context: &ServeContext,
    admission: &Admission,
) -> Result<(), ServeFailure> {
    let ServeContext {
        peer,
        expected_port,
        scheme,
        limits,
        ..
    } = *context;
    let expected_host = context.expected_host.as_str();
    let capability = &context.capability;
    let connections = context.connections.as_ref();
    let stop = context.stop.as_ref();
    let set_nonblocking: NonblockingSetup<'_> = context.set_nonblocking.as_ref();
    if set_nonblocking(Endpoint::Client, &client).is_err() {
        return Err(ServeFailure::PreAuthentication);
    }
    let mut request = Vec::new();
    let mut scratch = [0u8; 4096];
    // Start each connection with its own establishment budget.
    let establishment = Instant::now();
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        if admission.cancelled() || check_establishment(limits, establishment).is_err() {
            return Err(ServeFailure::PreAuthentication);
        }
        match client.read(&mut scratch) {
            Ok(0) => return Err(ServeFailure::PreAuthentication),
            Ok(n) => {
                request.extend_from_slice(&scratch[..n]);
                if request.len() > HEADER_LIMIT {
                    return Err(ServeFailure::PreAuthentication);
                }
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(POLL),
            Err(_) => return Err(ServeFailure::PreAuthentication),
        }
    }

    let header_end = request
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|at| at + 4)
        .ok_or(ServeFailure::PreAuthentication)?;
    let authenticated = match capability.authenticate_and_strip(&request[..header_end]) {
        Ok(authenticated) => {
            // This slot now belongs to a client that proved the capability; it
            // is no longer evictable.
            admission.authenticated.store(true, Ordering::Release);
            authenticated
        }
        Err(_) => {
            respond_proxy_auth_required(&mut client);
            return Err(ServeFailure::PreAuthentication);
        }
    };
    let header = std::str::from_utf8(authenticated.as_bytes())
        .map_err(|_| refused("the media proxy received non-UTF-8 headers"))?;
    let first = header
        .lines()
        .next()
        .ok_or_else(|| refused("the media proxy received an empty request"))?;
    let mut fields = first.split_whitespace();
    let method = fields.next().unwrap_or_default();
    let target = fields.next().unwrap_or_default();
    let version = fields.next().unwrap_or_default();
    if fields.next().is_some() || !version.starts_with("HTTP/1.") {
        return Err(refused("the media proxy received a malformed request line").into());
    }

    // Validate the requested destination before dialing; a refused request must not open an origin socket.
    let tunnel = method.eq_ignore_ascii_case("CONNECT");
    let forward_path = if tunnel {
        if scheme != Scheme::Https || !authority_matches(target, expected_host, expected_port) {
            return Err(
                refused("the media source requested an unapproved proxy destination").into(),
            );
        }
        None
    } else {
        if scheme != Scheme::Http {
            return Err(refused("the media source bypassed the required HTTPS tunnel").into());
        }
        Some(approved_http_path(target, expected_host, expected_port)?)
    };

    // This is the first traversal observation: authentication, parsing and
    // intended-destination validation have all succeeded. Raw accepts and 407s
    // never advance it.
    connections.fetch_add(1, Ordering::AcqRel);

    let connect_timeout = remaining(limits.connect_timeout_ms, establishment.elapsed())?
        .min(remaining(limits.total_timeout_ms, establishment.elapsed())?);
    let mut upstream = TcpStream::connect_timeout(&peer, connect_timeout)
        .map_err(|_| refused("the pinned media peer did not connect before its deadline"))?;

    match forward_path {
        None => client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .map_err(|_| refused("the media proxy could not establish the tunnel"))?,
        Some(path) => {
            let rewritten = header.replacen(first, &format!("{method} {path} {version}"), 1);
            upstream
                .write_all(rewritten.as_bytes())
                .and_then(|()| upstream.write_all(&request[header_end..]))
                .map_err(|_| refused("the pinned media request could not be forwarded"))?;
        }
    }

    // Descriptor setup ends establishment; a relay ends on peer completion, cancellation, or client-orphan idle expiry.
    relay(client, upstream, limits, stop, set_nonblocking)?;
    Ok(())
}

fn respond_proxy_auth_required(client: &mut TcpStream) {
    let response = format!(
        "HTTP/1.1 407 Proxy Authentication Required\r\n\
         Proxy-Authenticate: {PROXY_AUTHENTICATE_VALUE}\r\n\
         Content-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let _ = client.write_all(response.as_bytes());
    let _ = client.flush();
    let _ = client.shutdown(Shutdown::Both);
}

fn authority_matches(authority: &str, host: &str, port: u16) -> bool {
    let expected = if host.parse::<IpAddr>().is_ok_and(|ip| ip.is_ipv6()) {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    authority.eq_ignore_ascii_case(&expected)
}

fn approved_http_path<'a>(target: &'a str, host: &str, port: u16) -> Result<&'a str, MediaError> {
    let rendered_host = if host.parse::<IpAddr>().is_ok_and(|ip| ip.is_ipv6()) {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    let default = port == 80;
    let authority = if default {
        rendered_host
    } else {
        format!("{rendered_host}:{port}")
    };
    let prefix = format!("http://{authority}");
    let rest = target
        .strip_prefix(&prefix)
        .ok_or_else(|| refused("the media source requested an unapproved proxy destination"))?;
    if rest.is_empty() {
        Ok("/")
    } else if rest.starts_with('/') {
        Ok(rest)
    } else {
        Err(refused(
            "the media source requested an ambiguous proxy destination",
        ))
    }
}

// Relays until cancellation or peer closure; only the upstream descriptor still requires nonblocking normalization.
fn relay(
    mut client: TcpStream,
    mut upstream: TcpStream,
    limits: TransportLimits,
    stop: &AtomicBool,
    set_nonblocking: NonblockingSetup<'_>,
) -> Result<(), MediaError> {
    set_nonblocking(Endpoint::Upstream, &upstream)
        .map_err(|_| refused("could not configure the pinned media relay"))?;
    let mut to_upstream = Vec::new();
    let mut to_client = Vec::new();
    let mut last_io = Instant::now();
    let mut client_eof = false;
    let mut upstream_eof = false;

    loop {
        // Poll cancellation independently of establishment deadlines; clientless tunnels also use orphan idle expiry.
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        // An orphaned tunnel (client gone, upstream silent) is the one case
        // that must not be held forever: nothing is streaming through it.
        if client_eof && to_client.is_empty() && orphan_expired(limits, last_io) {
            return Ok(());
        }
        let mut progressed = false;
        if !client_eof && to_upstream.len() < RELAY_BUFFER {
            let mut buf = [0u8; 8192];
            let room = (RELAY_BUFFER - to_upstream.len()).min(buf.len());
            match client.read(&mut buf[..room]) {
                Ok(0) => {
                    client_eof = true;
                    let _ = upstream.shutdown(Shutdown::Write);
                }
                Ok(n) => {
                    to_upstream.extend_from_slice(&buf[..n]);
                    progressed = true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Ok(()),
            }
        }
        if !upstream_eof && to_client.len() < RELAY_BUFFER {
            let mut buf = [0u8; 8192];
            let room = (RELAY_BUFFER - to_client.len()).min(buf.len());
            match upstream.read(&mut buf[..room]) {
                Ok(0) => {
                    upstream_eof = true;
                    let _ = client.shutdown(Shutdown::Write);
                }
                Ok(n) => {
                    to_client.extend_from_slice(&buf[..n]);
                    progressed = true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Ok(()),
            }
        }
        if !to_upstream.is_empty() {
            match upstream.write(&to_upstream) {
                Ok(n) => {
                    to_upstream.drain(..n);
                    progressed = true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Ok(()),
            }
        }
        if !to_client.is_empty() {
            match client.write(&to_client) {
                Ok(n) => {
                    to_client.drain(..n);
                    progressed = true;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Ok(()),
            }
        }
        if progressed {
            last_io = Instant::now();
        }
        if client_eof && upstream_eof && to_upstream.is_empty() && to_client.is_empty() {
            return Ok(());
        }
        if !progressed {
            thread::sleep(POLL);
        }
    }
}

// Reports whether this connection's request-read and peer-dial budget is exhausted.
fn check_establishment(limits: TransportLimits, establishment: Instant) -> Result<(), MediaError> {
    if expired(limits.total_timeout_ms, establishment.elapsed())
        || expired(limits.connect_timeout_ms, establishment.elapsed())
    {
        return Err(refused(
            "the remote media connection deadline expired before the source sent a request",
        ));
    }
    Ok(())
}

// Reports whether a clientless tunnel exceeded its idle allowance.
fn orphan_expired(limits: TransportLimits, last_io: Instant) -> bool {
    let idle_ms = if limits.read_timeout_ms == NO_LIMIT {
        ORPHAN_IDLE_FALLBACK_MS
    } else {
        limits.read_timeout_ms
    };
    last_io.elapsed() >= Duration::from_millis(idle_ms)
}

fn remaining(limit_ms: u64, elapsed: Duration) -> Result<Duration, MediaError> {
    if limit_ms == NO_LIMIT {
        return Ok(Duration::from_secs(24 * 60 * 60));
    }
    Duration::from_millis(limit_ms)
        .checked_sub(elapsed)
        .filter(|left| !left.is_zero())
        .ok_or_else(|| refused("the remote media connection deadline expired"))
}

fn expired(limit_ms: u64, elapsed: Duration) -> bool {
    limit_ms != NO_LIMIT && elapsed >= Duration::from_millis(limit_ms)
}

fn record_error(slot: &Mutex<Option<MediaError>>, error: MediaError) {
    let mut slot = slot
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if slot.is_none() {
        *slot = Some(error);
    }
}

fn refused(message: &str) -> MediaError {
    MediaError {
        kind: MediaErrorKind::Refused,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests;
