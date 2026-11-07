//! The libcurl redirect probe the GStreamer media services use: one
//! peer-pinned `HEAD` request per hop, no body, no automatic redirects. It
//! implements core's [`RedirectProbe`], so the remote gate itself stays in
//! `enzui-core` and this crate supplies the effect.

use enzui_core::media::remote_gate::*;
use enzui_core::url_policy::{NO_LIMIT, TransportLimits, UrlPolicy};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

/// A libcurl redirect probe that performs one peer-pinned `HEAD` request without following redirects or accepting a body.
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
        easy.nobody(true).map_err(|_| EffectFailed::Unavailable)?;
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
        let body_attempted = Cell::new(false);
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
                    if chunk.is_empty() {
                        return Ok(0);
                    }
                    body_attempted.set(true);
                    stopped.set(Some(EffectFailed::Unavailable));
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
            if transfer.perform().is_err() {
                return Err(stopped.get().unwrap_or(EffectFailed::Unavailable));
            }
        }
        if body_attempted.get() {
            return Err(EffectFailed::Unavailable);
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
