//! An audio service with nothing loaded sleeps until it is asked for
//! something. Its worker polled every 100 ms, so Music woke ten times a second
//! at rest.

#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]

use enzui_services::media::{AudioService, AudioServiceConfig};
use std::time::Duration;

/// The voluntary context switches of this process's `enzui-audio-svc` threads.
fn audio_worker_switches() -> u64 {
    std::fs::read_dir("/proc/self/task")
        .expect("this process's threads")
        .flatten()
        .filter(|task| {
            std::fs::read_to_string(task.path().join("comm"))
                .is_ok_and(|name| name.trim() == "enzui-audio-svc")
        })
        .map(|task| {
            let status = std::fs::read_to_string(task.path().join("status")).unwrap_or_default();
            status
                .lines()
                .find_map(|line| line.strip_prefix("voluntary_ctxt_switches:"))
                .and_then(|count| count.trim().parse::<u64>().ok())
                .unwrap_or(0)
        })
        .sum()
}

/// Sabotage: give the worker its 100 ms `recv_timeout` back while no
/// pipeline exists and it wakes about ten times here.
#[test]
fn an_idle_audio_service_does_not_wake() {
    let (mut service, _events) = AudioService::new(AudioServiceConfig::default()).expect("service");
    std::thread::sleep(Duration::from_millis(200));

    let before = audio_worker_switches();
    std::thread::sleep(Duration::from_secs(1));
    let woke = audio_worker_switches() - before;

    service.shutdown();
    assert!(
        woke <= 1,
        "the idle audio worker woke {woke} times in a second"
    );
}
