//! Deterministic localhost tests for the real libcurl transport.

use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

fn local_policy() -> UrlPolicy {
    UrlPolicy {
        allow_http: true,
        allow_private_network: true,
        max_redirects: 5,
    }
}

fn request(url: &str, policy: UrlPolicy) -> RemoteRequest {
    RemoteRequest::checked(url, &policy).expect("test URL")
}

fn read_request(stream: &mut TcpStream) {
    let mut data = Vec::new();
    let mut byte = [0u8; 1];
    while data.len() < 16 * 1024 {
        if stream.read_exact(&mut byte).is_err() {
            break;
        }
        data.push(byte[0]);
        if data.ends_with(b"\r\n\r\n") {
            break;
        }
    }
}

#[test]
fn cancellation_is_observed_by_libcurl_and_closes_the_server_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (streaming_tx, streaming_rx) = mpsc::channel();
    let server_saw_close = Arc::new(AtomicBool::new(false));
    let server_closed = Arc::clone(&server_saw_close);
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        read_request(&mut stream);
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8388608\r\n\r\n")
            .unwrap();
        let chunk = vec![0x55; 16 * 1024];
        for index in 0..512 {
            if stream.write_all(&chunk).is_err() {
                server_closed.store(true, Ordering::SeqCst);
                break;
            }
            let _ = stream.flush();
            if index == 0 {
                streaming_tx.send(()).unwrap();
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    });

    let policy = local_policy();
    let cancel = CancelSignal::new();
    let worker_cancel = cancel.clone();
    let url = format!("http://127.0.0.1:{port}/slow.png");
    let worker = std::thread::spawn(move || {
        CurlTransport::new().fetch(
            &request(&url, policy),
            &policy,
            &TransportLimits::DEFAULT,
            &worker_cancel,
            &mut |_| {},
        )
    });

    streaming_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("server began body");
    assert!(cancel.cancel(CancelCause::Cancelled));
    let result = worker.join().unwrap();
    assert_eq!(
        result.expect_err("transport must abort"),
        TransportError::Cancelled(CancelCause::Cancelled),
        "cancellation must be observed inside libcurl, not only by the caller"
    );
    server.join().unwrap();
    assert!(
        server_saw_close.load(Ordering::SeqCst),
        "server must observe the underlying connection close after transport cancellation"
    );
}

// ==== Conditional requests / HTTP 304 ========================================

/// HTTPS-only, public-only, with a chosen redirect bound: the default policy,
/// which is what the screening under test is for. `allow_private_network` is
/// deliberately false — flipping it on would disable the very check.
fn screening_policy(max_redirects: u32) -> UrlPolicy {
    UrlPolicy {
        max_redirects,
        ..UrlPolicy::DEFAULT
    }
}

mod transfers;
