//! Peer-pinned GET probes for the remote media gate, without automatic redirects.
//! A first-byte range and early body abort keep probing bounded even when a
//! server ignores the range. GET exposes redirects that HEAD may omit.

use enzui_core::media::remote_gate::*;
use enzui_core::url_policy::{NO_LIMIT, TransportLimits, UrlPolicy};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

/// A libcurl redirect probe that performs one peer-pinned first-byte `GET`
/// per hop, without following redirects or reading a body.
#[derive(Debug, Clone, Copy, Default)]
pub struct CurlRedirectProbe;

impl RedirectProbe for CurlRedirectProbe {
    fn probe(
        &self,
        url: &str,
        endpoint: SocketAddr,
        limits: &TransportLimits,
        budget: &StreamBudget,
        clock: &dyn Clock,
        cancel: &dyn CancelView,
    ) -> Result<HopReport, EffectFailed> {
        use std::cell::Cell;

        let peer = endpoint.ip();
        let port = endpoint.port();
        let elapsed = clock.elapsed_ms();
        if cancel.is_cancelled() {
            return Err(EffectFailed::Cancelled);
        }
        let total_left = remaining_ms(limits.total_timeout_ms, elapsed)?;
        let connect_left = remaining_ms(limits.connect_timeout_ms, elapsed)?;
        let read_left = remaining_ms(limits.read_timeout_ms, 0)?;

        let safe = self.policy_url(url)?;
        let mut easy = curl::easy::Easy::new();
        easy.url(url).map_err(|_| EffectFailed::Unavailable)?;
        let mut request_headers = curl::easy::List::new();
        request_headers
            .append("Range: bytes=0-0")
            .map_err(|_| EffectFailed::Unavailable)?;
        easy.http_headers(request_headers)
            .map_err(|_| EffectFailed::Unavailable)?;
        easy.follow_location(false)
            .map_err(|_| EffectFailed::Unavailable)?;
        easy.unrestricted_auth(false)
            .map_err(|_| EffectFailed::Unavailable)?;
        easy.proxy("").map_err(|_| EffectFailed::Unavailable)?;
        easy.ssl_verify_peer(true)
            .map_err(|_| EffectFailed::Unavailable)?;
        easy.ssl_verify_host(true)
            .map_err(|_| EffectFailed::Unavailable)?;
        easy.progress(true).map_err(|_| EffectFailed::Unavailable)?;

        if let Some(ms) = minimum_timeout(connect_left, total_left) {
            easy.connect_timeout(Duration::from_millis(ms.max(1)))
                .map_err(|_| EffectFailed::Unavailable)?;
        }
        if let Some(ms) = minimum_timeout(total_left, read_left) {
            easy.timeout(Duration::from_millis(ms.max(1)))
                .map_err(|_| EffectFailed::Unavailable)?;
        }

        let mut pinned = curl::easy::List::new();
        let address = match peer {
            IpAddr::V4(ip) => ip.to_string(),
            IpAddr::V6(ip) => format!("[{ip}]"),
        };
        pinned
            .append(&format!("{}:{port}:{address}", safe.host()))
            .map_err(|_| EffectFailed::Unavailable)?;
        easy.resolve(pinned)
            .map_err(|_| EffectFailed::Unavailable)?;

        let header_bytes = Cell::new(0u64);
        // Servers may ignore Range; abort before retaining any body.
        let body_started = Cell::new(false);
        let stopped = Cell::new(None::<EffectFailed>);
        let mut location = None::<String>;
        {
            let mut transfer = easy.transfer();
            transfer
                .header_function(|line| {
                    let next = header_bytes
                        .get()
                        .saturating_add(u64::try_from(line.len()).unwrap_or(u64::MAX));
                    header_bytes.set(next);
                    if budget.probe_bytes != NO_LIMIT && next > budget.probe_bytes {
                        stopped.set(Some(EffectFailed::Unavailable));
                        return false;
                    }
                    if let Ok(line) = std::str::from_utf8(line)
                        && let Some((name, value)) = line.trim_end().split_once(':')
                        && name.eq_ignore_ascii_case("location")
                    {
                        location = Some(value.trim().to_owned());
                    }
                    true
                })
                .map_err(|_| EffectFailed::Unavailable)?;
            transfer
                .write_function(|chunk| {
                    if !chunk.is_empty() {
                        body_started.set(true);
                    }
                    Ok(0)
                })
                .map_err(|_| EffectFailed::Unavailable)?;
            transfer
                .progress_function(|_, _, _, _| {
                    if cancel.is_cancelled() {
                        stopped.set(Some(EffectFailed::Cancelled));
                        return false;
                    }
                    if limits.total_expired(clock.elapsed_ms()) {
                        stopped.set(Some(EffectFailed::Deadline));
                        return false;
                    }
                    true
                })
                .map_err(|_| EffectFailed::Unavailable)?;
            // CURLE_WRITE_ERROR is expected only for our deliberate body abort.
            if let Err(error) = transfer.perform()
                && !(error.is_write_error() && body_started.get() && stopped.get().is_none())
            {
                return Err(stopped.get().unwrap_or(EffectFailed::Unavailable));
            }
        }
        if cancel.is_cancelled() {
            return Err(EffectFailed::Cancelled);
        }
        if limits.total_expired(clock.elapsed_ms()) {
            return Err(EffectFailed::Deadline);
        }
        let actual = easy
            .primary_ip()
            .map_err(|_| EffectFailed::Unavailable)?
            .and_then(|ip| ip.parse::<IpAddr>().ok())
            .ok_or(EffectFailed::Unavailable)?;
        if actual != peer {
            return Err(EffectFailed::Unavailable);
        }
        let status = easy
            .response_code()
            .map_err(|_| EffectFailed::Unavailable)?;
        if (300..400).contains(&status) {
            return location
                .map(HopReport::Redirect)
                .ok_or(EffectFailed::Unavailable);
        }
        Ok(HopReport::Final)
    }
}

impl CurlRedirectProbe {
    fn policy_url(self, raw: &str) -> Result<crate::url_policy::SafeUrl, EffectFailed> {
        // The gate already checked this URL. Re-parse only to obtain the host
        // used by CURLOPT_RESOLVE, and fail closed if invariants drift.
        UrlPolicy {
            allow_http: true,
            ..UrlPolicy::DEFAULT
        }
        .check_url(raw)
        .map_err(|_| EffectFailed::Unavailable)
    }
}

fn remaining_ms(deadline_ms: u64, elapsed_ms: u64) -> Result<Option<u64>, EffectFailed> {
    if deadline_ms == NO_LIMIT {
        return Ok(None);
    }
    let now = crate::monotonic_ms::MonotonicMs(elapsed_ms);
    let deadline = crate::monotonic_ms::MonotonicMs(deadline_ms);
    if now.is_expired(deadline, crate::monotonic_ms::Boundary::Inclusive) {
        Err(EffectFailed::Deadline)
    } else {
        Ok(Some(now.remaining_ms(
            deadline,
            crate::monotonic_ms::Boundary::Inclusive,
        )))
    }
}

fn minimum_timeout(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use enzui_core::media::remote_gate::test_support::{FakeCancel, FakeClock};
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    fn redirector() -> (SocketAddr, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (seen, requests) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                request.push_str(&line);
                line.clear();
            }
            let _ = seen.send(request.clone());
            let mut stream = stream;
            let response = if request.starts_with("HEAD ") {
                "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n".to_owned()
            } else {
                let body = "<html><body>Moved</body></html>".repeat(30);
                format!(
                    "HTTP/1.1 302 Found\r\nLocation: http://media.example.test/session/1/audio.mp3\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            };
            let _ = stream.write_all(response.as_bytes());
        });
        (address, requests)
    }

    fn limits() -> TransportLimits {
        TransportLimits {
            max_bytes: 1024 * 1024,
            connect_timeout_ms: 2_000,
            read_timeout_ms: 2_000,
            total_timeout_ms: 5_000,
        }
    }

    fn budget() -> StreamBudget {
        StreamBudget {
            buffer_bytes: 1024 * 1024,
            ring_buffer_bytes: 1024 * 1024,
            probe_bytes: 16 * 1024,
        }
    }

    #[test]
    fn a_redirect_given_only_to_a_real_get_is_seen() {
        let (address, requests) = redirector();
        let url = format!("http://media.example.test:{}/audio.mp3", address.port());
        let hop = CurlRedirectProbe
            .probe(
                &url,
                address,
                &limits(),
                &budget(),
                &FakeClock::frozen(),
                &FakeCancel::default(),
            )
            .expect("the hop resolves");
        assert_eq!(
            hop,
            HopReport::Redirect("http://media.example.test/session/1/audio.mp3".to_owned())
        );
        let request = requests.recv().unwrap();
        assert!(
            request.starts_with("GET "),
            "the probe asks as the media request does: {request}"
        );
        assert!(
            request.contains("Range: bytes=0-0"),
            "for the first byte only: {request}"
        );
    }
}
