//! Tests for the streaming curl download transport.

use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A one-shot localhost HTTP server. It accepts exactly one connection, runs
/// `handle` with the accepted stream, and returns. Bound to port 0 so the OS
/// picks a free port; the chosen URL is returned for the client to hit.
fn one_shot_server(
    handle: impl FnOnce(TcpStream) + Send + 'static,
) -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let url = format!("http://127.0.0.1:{}/file", addr.port());
    let handle = std::thread::spawn(move || {
        listener
            .set_nonblocking(true)
            .expect("nonblocking for timed accept");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream
                        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                        .expect("read timeout");
                    handle(stream);
                    return;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() > deadline {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(_) => return,
            }
        }
    });
    (url, handle)
}

/// Read and discard the request line + headers (up to the blank line), and
/// return them so a test can assert what curl sent.
fn drain_request(stream: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    while !buf.ends_with(b"\r\n\r\n") {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => buf.push(byte[0]),
            Err(_) => break,
        }
        if buf.len() > 8192 {
            break;
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

/// Long enough that a held-body fixture, not curl's own deadline, exposes a
/// missing transport-owned abort in cancellation and Drop tests.
fn lifecycle_stall_deadlines() -> DownloadDeadlines {
    DownloadDeadlines {
        connect: std::time::Duration::from_millis(500),
        response_headers: std::time::Duration::from_millis(500),
        body_progress: std::time::Duration::from_secs(5),
        total: std::time::Duration::from_secs(5),
    }
}

/// Emit `count` custom headers, each with name `X-Pad-N` and a value of
/// `value_len` bytes, followed by a normal 200 body.
fn header_flood_response(count: usize, value_len: usize) -> Vec<u8> {
    let mut resp = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n".to_vec();
    let pad_value = "A".repeat(value_len);
    for i in 0..count {
        resp.extend_from_slice(format!("X-Pad-{i}: {pad_value}\r\n").as_bytes());
    }
    resp.extend_from_slice(b"\r\nok");
    resp
}

#[test]
fn download_header_over_count_aborts_before_body() {
    use crate::header_limits::MAX_HEADER_COUNT;

    let over_count = MAX_HEADER_COUNT + 10;
    let (url, server) = one_shot_server(move |mut stream| {
        let _req = drain_request(&mut stream);
        let resp = header_flood_response(over_count, 1);
        let _ = stream.write_all(&resp);
        let _ = stream.flush();
    });

    let transport = CurlDownloadTransport::fixture(DownloadDeadlines::default());
    let cancel = CancelSignal::new();
    let result = transport.start(&url, None, &cancel);
    assert!(
        matches!(&result, Err(TransportError::HeaderLimit(_))),
        "expected HeaderLimit, got {result:?}"
    );
    let _ = server.join();
}

mod drop_bounded;
mod lifecycle_poisoning;
mod resolver_budget;
