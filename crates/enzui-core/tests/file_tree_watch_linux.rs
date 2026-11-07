//! Real-filesystem integration tests for [`enzui_core::file_tree_watch::linux`].
//!
//! Gated on Linux and skipped on any host where `inotify_init1` fails (WSL1
//! without inotify, some sandboxed CI). Uses a per-test tempdir under the
//! system temp base; never touches `$HOME`.

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
#![cfg(target_os = "linux")]

use enzui_core::file_tree_watch::linux::LinuxInotifyBackend;
use enzui_core::file_tree_watch::{
    DirectoryChange, DirectoryWatchService, WatchDelivery, WatchOutcome,
};
use iced_futures::futures::FutureExt;
use iced_futures::futures::channel::mpsc;
use iced_futures::futures::stream::{Stream, StreamExt};

// Local single-thread block_on: park the current thread until the
// future is ready. std-only, no dev-dep, safe (uses `pin!`).
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};
    struct ThreadWaker(std::thread::Thread);
    impl Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut pinned = std::pin::pin!(fut);
    loop {
        match pinned.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
    }
}
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

fn scratch_dir(tag: &str) -> PathBuf {
    let base = std::env::temp_dir();
    let name = format!(
        "enzui-file-tree-watch-{}-{}-{}",
        tag,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let dir = base.join(name);
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn tear_down(p: &std::path::Path) {
    let _ = std::fs::remove_dir_all(p);
}

async fn timer(d: Duration) {
    use iced_futures::futures::channel::oneshot;
    let (tx, rx) = oneshot::channel::<()>();
    thread::spawn(move || {
        thread::sleep(d);
        let _ = tx.send(());
    });
    let _ = rx.await;
}

fn subscription_into_stream(
    s: iced_futures::Subscription<Vec<WatchDelivery>>,
) -> impl Stream<Item = Vec<WatchDelivery>> + Send {
    let mut recipes = iced_futures::subscription::into_recipes(s);
    assert_eq!(recipes.len(), 1);
    let recipe = recipes.remove(0);
    let (_tx, rx) = mpsc::unbounded();
    recipe.stream(Box::pin(rx))
}

fn await_matching(
    stream: &mut (impl Stream<Item = Vec<WatchDelivery>> + Unpin),
    timeout: Duration,
    pred: impl Fn(&WatchDelivery) -> bool,
) -> Option<Vec<WatchDelivery>> {
    let deadline = std::time::Instant::now() + timeout;
    block_on(async move {
        loop {
            let now = std::time::Instant::now();
            if now >= deadline {
                return None;
            }
            let remaining = deadline - now;
            let timer_fut = Box::pin(timer(remaining)).fuse();
            let next_fut = stream.next().fuse();
            iced_futures::futures::pin_mut!(timer_fut);
            iced_futures::futures::pin_mut!(next_fut);
            let batch = iced_futures::futures::select_biased! {
                v = next_fut => v,
                () = timer_fut => None,
            };
            match batch {
                None => return None,
                Some(batch) if batch.iter().any(&pred) => return Some(batch),
                Some(_) => continue,
            }
        }
    })
}

fn boot() -> Option<LinuxInotifyBackend> {
    match LinuxInotifyBackend::new() {
        Ok(b) => Some(b),
        Err(_) => {
            eprintln!("linux inotify unavailable on this host; skipping");
            None
        }
    }
}

#[test]
fn a_real_external_create_reaches_the_subscription() {
    let Some(backend) = boot() else { return };
    let dir = scratch_dir("create");
    let svc = DirectoryWatchService::with_config(Arc::new(backend), 8, 128, 32 * 1024);
    let _id = svc.watch(dir.clone()).expect("install real watch");
    let mut stream = Box::pin(subscription_into_stream(svc.subscription()));

    std::fs::write(dir.join("hello.txt"), b"hi").unwrap();
    let batch = await_matching(
        &mut stream,
        Duration::from_secs(3),
        |d| matches!(&d.outcome, WatchOutcome::Changes(c) if c.iter().any(|c| matches!(c, DirectoryChange::Created(p) if p.file_name().is_some_and(|n| n == "hello.txt")))),
    );
    tear_down(&dir);
    assert!(
        batch.is_some(),
        "no delivery arrived for the external create"
    );
}
