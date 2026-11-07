//! Service integration tests through the real core reducer.
//!
//! This file holds the shared imports and cross-family helpers (`png`, the
//! transport fakes, `pool`, `config`, `wait_until`, `wait_terminal`, and the
//! worker-thread probes). The individual test families were relocated to keep
//! every source file below the workspace's 2,500-line reviewability ceiling.
//! Per-concern sibling files under `tests/` are pulled back in with the
//! `include!` directives at the foot of this file. `include!` is textual, so
//! every relocated test keeps its exact `service::tests::…` path and shares this
//! module's scope — a pure relocation with no path or behaviour change.

use super::*;
use crate::http_cache::HttpCacheConfig;
use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use std::path::PathBuf;
use std::sync::Condvar;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

fn png(seed: u8) -> Vec<u8> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(
            &[seed, seed.wrapping_add(1), seed.wrapping_add(2), 255],
            1,
            1,
            ExtendedColorType::Rgba8,
        )
        .unwrap();
    out
}

#[derive(Clone)]
struct ImmediateTransport {
    calls: Arc<AtomicUsize>,
}

impl Transport for ImmediateTransport {
    fn fetch(
        &self,
        request: &RemoteRequest,
        _policy: &UrlPolicy,
        _limits: &TransportLimits,
        cancel: &CancelSignal,
        progress: &mut dyn FnMut(TransferProgress),
    ) -> Result<TransportResponse, TransportError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(cause) = cancel.cause() {
            return Err(TransportError::Cancelled(cause));
        }
        let seed = request.transport_url().bytes().fold(0u8, u8::wrapping_add);
        let body = png(seed);
        progress(TransferProgress {
            received: body.len() as u64,
            total: Some(body.len() as u64),
        });
        Ok(TransportResponse {
            status: 200,
            body,
            final_url: request.safe_url().clone(),
            peer_class: AddressClass::Public,
            headers: Vec::new(),
        })
    }
}

struct ControlledTransport {
    calls: Arc<AtomicUsize>,
    aborted: Arc<AtomicUsize>,
    gate: Arc<(Mutex<bool>, Condvar)>,
}

impl Transport for ControlledTransport {
    fn fetch(
        &self,
        request: &RemoteRequest,
        _policy: &UrlPolicy,
        _limits: &TransportLimits,
        cancel: &CancelSignal,
        _progress: &mut dyn FnMut(TransferProgress),
    ) -> Result<TransportResponse, TransportError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let (lock, cv) = &*self.gate;
        let mut released = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        loop {
            if let Some(cause) = cancel.cause() {
                self.aborted.fetch_add(1, Ordering::SeqCst);
                return Err(TransportError::Cancelled(cause));
            }
            if *released {
                break;
            }
            let (next, _) = cv
                .wait_timeout(released, Duration::from_millis(5))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            released = next;
        }
        let body = png(1);
        Ok(TransportResponse {
            status: 200,
            body,
            final_url: request.safe_url().clone(),
            peer_class: AddressClass::Public,
            headers: Vec::new(),
        })
    }
}

/// Build a service whose worker pool is expected to start.
///
/// Construction is fallible because thread creation is an OS resource request,
/// so every test that just wants a working pool goes through here. A test that
/// cares about the *failure* path drives `new_with_spawn_gate` directly.
fn pool(service: Result<ImageService, ServiceError>) -> ImageService {
    service.expect("worker pool must start in the test environment")
}

fn config() -> ServiceConfig {
    ServiceConfig {
        worker_threads: 2,
        decoded_cache_bytes: 32,
        // Disk cache off for service tests. These tests count transport calls,
        // and a real on-disk cache under the developer's `~/.cache` would both
        // pollute a user directory and make those counts depend on entries left
        // by a PREVIOUS test run -- the same URL would be served from disk and
        // the transport never entered. `http_cache` is covered by its own module
        // tests against private temporary directories.
        http_cache: HttpCacheConfig::DISABLED,
        ..ServiceConfig::default()
    }
}

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !condition() {
        assert!(Instant::now() < deadline, "condition timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_terminal(service: &mut ImageService, expected: usize) -> Vec<ServiceEvent> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut events = Vec::new();
    loop {
        events.extend(service.poll());
        let terminal = events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    ServiceEvent::Ready { .. }
                        | ServiceEvent::Failed { .. }
                        | ServiceEvent::Cancelled { .. }
                )
            })
            .count();
        if terminal >= expected {
            return events;
        }
        assert!(Instant::now() < deadline, "service events timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------

include!("tests/pool_lifecycle.rs");
include!("tests/single_flight.rs");
include!("tests/admission.rs");
include!("tests/lru.rs");
include!("tests/progress.rs");
include!("tests/disk_cache.rs");
#[cfg(feature = "curl-transport")]
include!("tests/redirect_validators.rs");
