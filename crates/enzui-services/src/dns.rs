//! DNS service and mail-exchanger lookups: SRV and MX records from the
//! system's resolver.
//!
//! The standard library resolves names to addresses and nothing else, so a
//! client that discovers a service by its records (RFC 6186 mail servers,
//! RFC 2782 services, a domain's mail exchangers) has no route to them. This
//! module asks the nameservers listed in `/etc/resolv.conf` directly: one
//! question over UDP, the same question over TCP when the answer was
//! truncated, and a parser that bounds every length and refuses a
//! compression loop.
//!
//! A lookup blocks for at most the configured timeout per server and attempt,
//! so call it from a worker thread, never from `update`.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::time::{Duration, Instant};

/// The resolver configuration file read by [`Resolver::system`].
pub const RESOLV_CONF: &str = "/etc/resolv.conf";

/// The longest answer read over TCP; DNS frames it with a 16-bit length.
const MAX_MESSAGE: usize = 65_535;
/// The longest name DNS allows, in presentation form.
const MAX_NAME: usize = 253;
/// The most compression pointers one name may follow.
const MAX_POINTERS: usize = 32;

const TYPE_MX: u16 = 15;
const TYPE_SRV: u16 = 33;
const CLASS_IN: u16 = 1;

/// The nameservers a lookup asks, and how long it waits for each.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolver {
    /// Asked in order until one answers.
    pub servers: Vec<SocketAddr>,
    /// How long one attempt at one server may take.
    pub timeout: Duration,
    /// How many times each server is asked before moving on.
    pub attempts: usize,
}

impl Resolver {
    /// The resolver for these servers, with a 2 s timeout and two attempts.
    pub fn new(servers: Vec<SocketAddr>) -> Self {
        Self {
            servers,
            timeout: Duration::from_secs(2),
            attempts: 2,
        }
    }

    /// The system resolver: the `nameserver` lines of [`RESOLV_CONF`].
    ///
    /// # Errors
    ///
    /// [`DnsError::NoServers`] when the resolver configuration is unreadable or names no server.
    pub fn system() -> Result<Self, DnsError> {
        let text = std::fs::read_to_string(RESOLV_CONF).map_err(|_| DnsError::NoServers)?;
        let resolver = Self::new(nameservers(&text));
        if resolver.servers.is_empty() {
            return Err(DnsError::NoServers);
        }
        Ok(resolver)
    }
}

/// The `nameserver` addresses in a `resolv.conf` text, on port 53.
pub fn nameservers(resolv_conf: &str) -> Vec<SocketAddr> {
    resolv_conf
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            (words.next()? == "nameserver").then_some(())?;
            // A scoped IPv6 address ("fe80::1%eth0") cannot be parsed as an
            // `IpAddr`; such a server is skipped rather than guessed at.
            let ip: std::net::IpAddr = words.next()?.parse().ok()?;
            Some(SocketAddr::new(ip, 53))
        })
        .collect()
}

/// One service record (RFC 2782).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SrvRecord {
    /// Lower is preferred.
    pub priority: u16,
    /// Relative share among records of equal priority.
    pub weight: u16,
    /// The port the service listens on.
    pub port: u16,
    /// The host offering the service, without the trailing dot. `"."` in
    /// the record means "not offered here" and is reported as an empty name.
    pub target: String,
}

/// One mail-exchanger record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MxRecord {
    /// Lower is preferred.
    pub preference: u16,
    /// The exchanger's host name, without the trailing dot.
    pub exchange: String,
}

/// Why a lookup produced no answer. A name that does not exist is not an
/// error: it is an empty list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DnsError {
    /// No nameserver is configured.
    NoServers,
    /// The name is not a valid DNS name.
    InvalidName,
    /// Every server timed out or refused the connection.
    Unreachable,
    /// A server answered with a failure code (SERVFAIL, REFUSED, ...).
    ServerFailure(u8),
    /// The answer could not be parsed.
    Malformed,
}

impl std::fmt::Display for DnsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoServers => f.write_str("no DNS server is configured"),
            Self::InvalidName => f.write_str("not a valid domain name"),
            Self::Unreachable => f.write_str("the DNS server did not answer"),
            Self::ServerFailure(code) => write!(f, "the DNS server failed (code {code})"),
            Self::Malformed => f.write_str("the DNS answer could not be read"),
        }
    }
}

impl std::error::Error for DnsError {}

/// The SRV records for `name` (for example `_imaps._tcp.example.com`),
/// sorted by priority, then by descending weight.
///
/// # Errors
///
/// A [`DnsError`] when the query fails or the answer is malformed.
pub fn lookup_srv(resolver: &Resolver, name: &str) -> Result<Vec<SrvRecord>, DnsError> {
    let (message, answers) = lookup(resolver, name, TYPE_SRV)?;
    let mut records: Vec<SrvRecord> = answers
        .iter()
        .filter_map(|rdata| {
            let priority = rdata.u16_at(&message, 0)?;
            let weight = rdata.u16_at(&message, 2)?;
            let port = rdata.u16_at(&message, 4)?;
            let (target, _) = read_name(&message, rdata.offset + 6).ok()?;
            Some(SrvRecord {
                priority,
                weight,
                port,
                target,
            })
        })
        .collect();
    records.sort_by(|a, b| a.priority.cmp(&b.priority).then(b.weight.cmp(&a.weight)));
    Ok(records)
}

/// The MX records for `domain`, most preferred first.
///
/// # Errors
///
/// A [`DnsError`] when the query fails or the answer is malformed.
pub fn lookup_mx(resolver: &Resolver, domain: &str) -> Result<Vec<MxRecord>, DnsError> {
    let (message, answers) = lookup(resolver, domain, TYPE_MX)?;
    let mut records: Vec<MxRecord> = answers
        .iter()
        .filter_map(|rdata| {
            let preference = rdata.u16_at(&message, 0)?;
            let (exchange, _) = read_name(&message, rdata.offset + 2).ok()?;
            Some(MxRecord {
                preference,
                exchange,
            })
        })
        .collect();
    records.sort_by_key(|r| r.preference);
    Ok(records)
}

/// Where one answer's data sits inside its message, so names inside it can
/// follow compression pointers back into the message.
struct Rdata {
    offset: usize,
    len: usize,
}

impl Rdata {
    /// The big-endian `u16` at `at` within this record's data.
    fn u16_at(&self, message: &[u8], at: usize) -> Option<u16> {
        if at + 2 > self.len {
            return None;
        }
        let i = self.offset + at;
        Some(u16::from_be_bytes([*message.get(i)?, *message.get(i + 1)?]))
    }
}

/// A response message and the records in it that answer the question.
type Answers = (Vec<u8>, Vec<Rdata>);

fn lookup(resolver: &Resolver, name: &str, qtype: u16) -> Result<Answers, DnsError> {
    if resolver.servers.is_empty() {
        return Err(DnsError::NoServers);
    }
    let id = query_id();
    let query = build_query(id, name, qtype)?;
    let mut failure = DnsError::Unreachable;
    for server in &resolver.servers {
        for _ in 0..resolver.attempts.max(1) {
            match ask(*server, &query, id, resolver.timeout)
                .and_then(|message| parse_answers(message, id, qtype))
            {
                // Including a name that does not exist: that is an answer.
                Ok(answers) => return Ok(answers),
                // A server that answered, even with a failure, is not asked
                // again; the next one may know better.
                Err(error @ (DnsError::ServerFailure(_) | DnsError::Malformed)) => {
                    failure = error;
                    break;
                }
                Err(error) => failure = error,
            }
        }
    }
    Err(failure)
}

/// The time left before `deadline`, or [`DnsError::Unreachable`] once it has
/// passed: one attempt never outlives its timeout, however its I/O is split.
fn remaining(deadline: Instant) -> Result<Duration, DnsError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or(DnsError::Unreachable)
}

/// One exchange with one server: UDP, then TCP when the answer was cut, all
/// within `timeout`.
fn ask(server: SocketAddr, query: &[u8], id: u16, timeout: Duration) -> Result<Vec<u8>, DnsError> {
    let deadline = Instant::now() + timeout;
    let local = if server.is_ipv4() {
        SocketAddr::from((std::net::Ipv4Addr::UNSPECIFIED, 0))
    } else {
        SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, 0))
    };
    let socket = UdpSocket::bind(local).map_err(|_| DnsError::Unreachable)?;
    socket.connect(server).map_err(|_| DnsError::Unreachable)?;
    socket.send(query).map_err(|_| DnsError::Unreachable)?;
    let mut buf = vec![0u8; 4096];
    loop {
        socket
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(|_| DnsError::Unreachable)?;
        let n = socket.recv(&mut buf).map_err(|_| DnsError::Unreachable)?;
        // A stray datagram with another id is not this answer; keep waiting
        // within what is left of the same timeout.
        if n < 12 || u16::from_be_bytes([buf[0], buf[1]]) != id {
            continue;
        }
        let truncated = buf[2] & 0x02 != 0;
        if !truncated {
            buf.truncate(n);
            return Ok(buf);
        }
        return ask_tcp(server, query, deadline);
    }
}

/// The TCP retry of an exchange, finishing by `deadline`: every connect,
/// write and read gets only the time left, so a peer that dribbles bytes
/// cannot stretch it.
fn ask_tcp(server: SocketAddr, query: &[u8], deadline: Instant) -> Result<Vec<u8>, DnsError> {
    let mut stream = TcpStream::connect_timeout(&server, remaining(deadline)?)
        .map_err(|_| DnsError::Unreachable)?;
    let len = u16::try_from(query.len()).map_err(|_| DnsError::InvalidName)?;
    let mut framed = Vec::with_capacity(2 + query.len());
    framed.extend_from_slice(&len.to_be_bytes());
    framed.extend_from_slice(query);
    let mut sent = 0;
    while sent < framed.len() {
        stream
            .set_write_timeout(Some(remaining(deadline)?))
            .map_err(|_| DnsError::Unreachable)?;
        match stream.write(&framed[sent..]) {
            Ok(0) | Err(_) => return Err(DnsError::Unreachable),
            Ok(n) => sent += n,
        }
    }
    let mut prefix = [0u8; 2];
    read_by(&mut stream, &mut prefix, deadline)?;
    let len = usize::from(u16::from_be_bytes(prefix)).min(MAX_MESSAGE);
    let mut message = vec![0u8; len];
    read_by(&mut stream, &mut message, deadline)?;
    Ok(message)
}

/// Fills `buf` from `stream` before `deadline`.
fn read_by(stream: &mut TcpStream, buf: &mut [u8], deadline: Instant) -> Result<(), DnsError> {
    let mut read = 0;
    while read < buf.len() {
        stream
            .set_read_timeout(Some(remaining(deadline)?))
            .map_err(|_| DnsError::Unreachable)?;
        match stream.read(&mut buf[read..]) {
            Ok(0) => return Err(DnsError::Unreachable),
            Ok(n) => read += n,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err(DnsError::Unreachable),
        }
    }
    Ok(())
}

/// A query id that differs between lookups; it only has to match the answer
/// to its own question, not resist an attacker on the path.
#[allow(
    clippy::cast_possible_truncation,
    reason = "folds clock and process entropy into a 16-bit id"
)]
fn query_id() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static NEXT: AtomicU16 = AtomicU16::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    (nanos as u16) ^ (std::process::id() as u16) ^ NEXT.fetch_add(0x9E37, Ordering::Relaxed)
}

/// A recursive query for one name and type.
pub(crate) fn build_query(id: u16, name: &str, qtype: u16) -> Result<Vec<u8>, DnsError> {
    let name = name.trim_end_matches('.');
    if name.is_empty() || name.len() > MAX_NAME {
        return Err(DnsError::InvalidName);
    }
    let mut out = Vec::with_capacity(18 + name.len());
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&[0x01, 0x00]); // recursion desired
    out.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]); // one question
    for label in name.split('.') {
        let bytes = label.as_bytes();
        if bytes.is_empty() || bytes.len() > 63 || !bytes.iter().all(u8::is_ascii_graphic) {
            return Err(DnsError::InvalidName);
        }
        out.push(u8::try_from(bytes.len()).map_err(|_| DnsError::InvalidName)?);
        out.extend_from_slice(bytes);
    }
    out.push(0);
    out.extend_from_slice(&qtype.to_be_bytes());
    out.extend_from_slice(&CLASS_IN.to_be_bytes());
    Ok(out)
}

/// The answers of type `qtype` in a response to query `id`. A name that does
/// not exist (NXDOMAIN) is an empty list.
fn parse_answers(message: Vec<u8>, id: u16, qtype: u16) -> Result<Answers, DnsError> {
    if message.len() < 12 || u16::from_be_bytes([message[0], message[1]]) != id {
        return Err(DnsError::Malformed);
    }
    if message[2] & 0x80 == 0 {
        return Err(DnsError::Malformed); // not a response
    }
    match message[3] & 0x0F {
        0 => {}
        3 => return Ok((message, Vec::new())),
        code => return Err(DnsError::ServerFailure(code)),
    }
    let questions = u16::from_be_bytes([message[4], message[5]]);
    let answers = u16::from_be_bytes([message[6], message[7]]);
    let mut at = 12;
    for _ in 0..questions {
        let (_, next) = read_name(&message, at)?;
        at = next + 4;
    }
    let mut out = Vec::new();
    for _ in 0..answers {
        let (_, next) = read_name(&message, at)?;
        let header = message.get(next..next + 10).ok_or(DnsError::Malformed)?;
        let rtype = u16::from_be_bytes([header[0], header[1]]);
        let class = u16::from_be_bytes([header[2], header[3]]);
        let len = usize::from(u16::from_be_bytes([header[8], header[9]]));
        let offset = next + 10;
        if offset + len > message.len() {
            return Err(DnsError::Malformed);
        }
        // A CNAME on the way to the record is skipped; only the asked type
        // is an answer.
        if rtype == qtype && class == CLASS_IN {
            out.push(Rdata { offset, len });
        }
        at = offset + len;
    }
    Ok((message, out))
}

/// The name starting at `at`, and the offset just past it in the record
/// (not past any pointer target).
pub(crate) fn read_name(message: &[u8], at: usize) -> Result<(String, usize), DnsError> {
    let mut name = String::new();
    let mut cursor = at;
    let mut end = None;
    let mut pointers = 0;
    loop {
        let len = *message.get(cursor).ok_or(DnsError::Malformed)?;
        match len {
            0 => {
                return Ok((name, end.unwrap_or(cursor + 1)));
            }
            l if l & 0xC0 == 0xC0 => {
                let low = *message.get(cursor + 1).ok_or(DnsError::Malformed)?;
                pointers += 1;
                if pointers > MAX_POINTERS {
                    return Err(DnsError::Malformed);
                }
                end.get_or_insert(cursor + 2);
                cursor = (usize::from(l & 0x3F) << 8) | usize::from(low);
            }
            l if l & 0xC0 == 0 => {
                let label = message
                    .get(cursor + 1..cursor + 1 + usize::from(l))
                    .ok_or(DnsError::Malformed)?;
                if !name.is_empty() {
                    name.push('.');
                }
                name.push_str(&String::from_utf8_lossy(label));
                if name.len() > MAX_NAME {
                    return Err(DnsError::Malformed);
                }
                cursor += 1 + usize::from(l);
            }
            _ => return Err(DnsError::Malformed),
        }
    }
}

#[cfg(test)]
#[path = "dns/tests.rs"]
mod tests;
