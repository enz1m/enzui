use super::*;

#[test]
fn an_invalid_name_is_refused_before_anything_is_sent() {
    for name in ["", "a..b", "white space.com", &"x".repeat(64)] {
        assert_eq!(
            build_query(1, name, TYPE_MX),
            Err(DnsError::InvalidName),
            "{name:?}"
        );
    }
}

/// A UDP server on loopback that answers each query with `respond`.
fn server(
    respond: impl Fn(&[u8]) -> Vec<Vec<u8>> + Send + 'static,
) -> (SocketAddr, std::thread::JoinHandle<()>) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let address = socket.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let mut buf = [0u8; 512];
        while let Ok((n, from)) = socket.recv_from(&mut buf) {
            for datagram in respond(&buf[..n]) {
                let _ = socket.send_to(&datagram, from);
            }
        }
    });
    (address, handle)
}

/// A response header to `query` with `rcode` and no records.
fn reply(query: &[u8], rcode: u8) -> Vec<u8> {
    let mut out = query.to_vec();
    out[2] = 0x81;
    out[3] = 0x80 | rcode;
    out
}

/// `query` answered with one MX record for its own name.
fn mx_reply(query: &[u8]) -> Vec<u8> {
    let mut out = reply(query, 0);
    out[7] = 1; // one answer
    out.extend_from_slice(&[0xC0, 12]); // the question's name
    out.extend_from_slice(&TYPE_MX.to_be_bytes());
    out.extend_from_slice(&CLASS_IN.to_be_bytes());
    out.extend_from_slice(&[0, 0, 0, 60]);
    out.extend_from_slice(&[0, 4, 0, 10, 0xC0, 12]); // preference 10, the same name
    out
}

#[test]
fn a_server_failure_falls_through_to_the_next_server() {
    let (failing, _a) = server(|query| vec![reply(query, 2)]);
    let (working, _b) = server(|query| vec![mx_reply(query)]);
    let mut resolver = Resolver::new(vec![failing, working]);
    resolver.timeout = Duration::from_millis(500);
    let records = lookup_mx(&resolver, "example.test").unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].preference, 10);
    // A name that does not exist is an answer, not a failure to move past.
    let (absent, _c) = server(|query| vec![reply(query, 3)]);
    let resolver = Resolver::new(vec![absent, working]);
    assert_eq!(lookup_mx(&resolver, "missing.test").unwrap(), Vec::new());
}

#[test]
fn stray_datagrams_do_not_stretch_an_attempt() {
    // Answers the query with a datagram for another id every 25 ms, each
    // arriving well inside a per-read timeout.
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let noisy = socket.local_addr().unwrap();
    let _a = std::thread::spawn(move || {
        let mut buf = [0u8; 512];
        let Ok((n, from)) = socket.recv_from(&mut buf) else {
            return;
        };
        let mut stray = reply(&buf[..n], 0);
        stray[0] ^= 0xFF;
        for _ in 0..60 {
            std::thread::sleep(Duration::from_millis(25));
            let _ = socket.send_to(&stray, from);
        }
    });
    let mut resolver = Resolver::new(vec![noisy]);
    resolver.timeout = Duration::from_millis(200);
    resolver.attempts = 1;
    let started = Instant::now();
    assert_eq!(
        lookup_mx(&resolver, "example.test"),
        Err(DnsError::Unreachable)
    );
    assert!(
        started.elapsed() < Duration::from_millis(600),
        "{:?}",
        started.elapsed()
    );
}
