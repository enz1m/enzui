use super::*;
use crate::media::remote_gate::test_support::{FakeCancel, FakeClock, FakeResolver, ScriptedProbe};
use crate::media::remote_gate::{GateEffects, MediaGate};
use crate::url_policy::{RemoteSource, UrlPolicy};

fn approved_local(port: u16) -> ApprovedStream {
    approved_local_with(port, TransportLimits::DEFAULT)
}

fn approved_local_with(port: u16, limits: TransportLimits) -> ApprovedStream {
    approved_local_url(port, limits, "https")
}

fn approved_local_url(port: u16, limits: TransportLimits, scheme: &str) -> ApprovedStream {
    let gate = MediaGate {
        policy: UrlPolicy {
            allow_http: true,
            allow_private_network: true,
            max_redirects: 0,
        },
        limits,
        ..MediaGate::DEFAULT
    };
    let resolver = FakeResolver::always(&["127.0.0.1"]);
    let probe = ScriptedProbe::terminal();
    let clock = FakeClock::frozen();
    let cancel = FakeCancel::default();
    gate.approve(
        &RemoteSource::checked(
            &format!("{scheme}://media.example.test:{port}/movie.mp4"),
            &gate.policy,
        )
        .unwrap(),
        &GateEffects {
            resolver: &resolver,
            probe: Some(&probe),
            clock: &clock,
            cancel: &cancel,
        },
    )
    .unwrap()
}

fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::new();
    for chunk in input.chunks(3) {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        output.push(char::from(ALPHABET[usize::from(a >> 2)]));
        output.push(char::from(ALPHABET[usize::from(((a & 3) << 4) | (b >> 4))]));
        output.push(if chunk.len() > 1 {
            char::from(ALPHABET[usize::from(((b & 15) << 2) | (c >> 6))])
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            char::from(ALPHABET[usize::from(c & 63)])
        } else {
            '='
        });
    }
    output
}

fn authorization_value(proxy: &PinnedProxy) -> String {
    format!(
        "Basic {}",
        base64(format!("{}:{}", proxy.client.username(), proxy.client.password()).as_bytes())
    )
}

fn authorized_request(proxy: &PinnedProxy, request: &str) -> String {
    let (line, rest) = request
        .split_once("\r\n")
        .expect("fixture request has a request line");
    format!(
        "{line}\r\nProxy-Authorization: {}\r\n{rest}",
        authorization_value(proxy)
    )
}

type Observed = Arc<Mutex<Vec<(Endpoint, SocketAddr, SocketAddr)>>>;

fn proxy_failing_nonblocking(port: u16, fail: Option<Endpoint>) -> (PinnedProxy, Observed) {
    let observed: Observed = Arc::new(Mutex::new(Vec::new()));
    let recorder = Arc::clone(&observed);
    let proxy =
        PinnedProxy::start_with_nonblocking(&approved_local(port), move |endpoint, stream| {
            recorder.lock().unwrap().push((
                endpoint,
                stream
                    .local_addr()
                    .expect("a live descriptor has a local address"),
                stream
                    .peer_addr()
                    .expect("a connected descriptor has a peer"),
            ));
            if Some(endpoint) == fail {
                return Err(std::io::Error::other("injected nonblocking failure"));
            }
            stream.set_nonblocking(true)
        })
        .unwrap();
    (proxy, observed)
}

fn refusal_from(proxy: &PinnedProxy) -> MediaError {
    let deadline = Instant::now() + Duration::from_secs(2);
    while proxy.error().is_none() && Instant::now() < deadline {
        thread::sleep(POLL);
    }
    proxy
        .error()
        .expect("a descriptor that cannot be normalized must fail the stream")
}

#[test]
fn an_upstream_that_cannot_go_nonblocking_refuses_the_stream() {
    let origin = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = origin.local_addr().unwrap().port();
    let origin_address = origin.local_addr().unwrap();
    let (proxy, observed) = proxy_failing_nonblocking(port, Some(Endpoint::Upstream));
    let proxy_address = proxy.address;

    let mut client = TcpStream::connect(proxy_address).unwrap();
    let client_address = client.local_addr().unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    client
        .write_all(
            authorized_request(
                &proxy,
                &format!("CONNECT media.example.test:{port} HTTP/1.1\r\n\r\n"),
            )
            .as_bytes(),
        )
        .unwrap();

    let error = refusal_from(&proxy);
    assert_eq!(error.kind, MediaErrorKind::Refused);
    assert_eq!(error.message, "could not configure the pinned media relay");
    assert_eq!(
        proxy.connections(),
        1,
        "the authenticated, parsed and destination-validated request is \
             counted even though later relay setup failed"
    );

    let observed = observed.lock().unwrap().clone();
    assert_eq!(
        observed
            .iter()
            .map(|(endpoint, ..)| *endpoint)
            .collect::<Vec<_>>(),
        vec![Endpoint::Client, Endpoint::Upstream],
    );
    assert_eq!(
        (observed[0].1, observed[0].2),
        (proxy_address, client_address),
        "the Client endpoint must be the source's connection into the proxy"
    );
    assert_eq!(
        observed[1].2, origin_address,
        "the Upstream endpoint must be the socket dialled to the pinned peer"
    );

    let mut tail = [0u8; 64];
    let mut seen = Vec::new();
    loop {
        match client.read(&mut tail) {
            Ok(0) => break,
            Ok(n) => seen.extend_from_slice(&tail[..n]),
            Err(error) => panic!("the refused tunnel must close, got {error}"),
        }
    }
    assert!(
        seen.is_empty() || seen.starts_with(b"HTTP/1.1 200"),
        "unexpected bytes on a refused tunnel: {seen:?}"
    );
}
