//! Bounded directory-change subscription for a finite set of visible folders.
//!
//! Complements [`crate::file_watch`], which polls open documents. The service
//! watches directory membership through an injected backend; Linux callers can
//! use [`native_directory_watch`], and [`NoopBackend`] explicitly refuses watches.
//!
//! Default limits bound live slots, queued change count, and queued change-path
//! bytes. Each watch also retains one diagnostic capped separately at the path
//! byte limit. These bounds exclude backend buffers and caller-owned events.
//! Overflow discards queued changes at delivery and requests a fresh listing.
//! Diagnostics and disappearance can accompany changes or overflow; disappearance
//! does not remove the service's slot.
//!
//! Tokens distinguish watches across services and reject events from cancelled
//! watches. The subscription uses event-driven notification without a polling
//! timer. Each admitted sink event calls the waker, including duplicates and
//! events received while overflow is pending; direct batch ingress wakes once.
//! Dropping the service releases its handles and ends a surviving subscription.
//!
//! ```
//! use enzui_core::file_tree_watch::{
//!     DirectoryWatchService, InMemoryDirectoryBackend, DirectoryChange, RawEventKind,
//!     WatchOutcome,
//! };
//!
//! let backend = InMemoryDirectoryBackend::new();
//! let svc = DirectoryWatchService::new(backend.clone());
//! let id = svc.watch("/proj").expect("install watch");
//!
//! backend.emit(
//!     &std::path::PathBuf::from("/proj"),
//!     RawEventKind::Change(DirectoryChange::Created("/proj/new.txt".into())),
//! );
//!
//! let deliveries = svc.take_deliveries();
//! assert!(matches!(deliveries[0].outcome, WatchOutcome::Changes(_)));
//! svc.unwatch(id);
//! ```

mod ancestor_watch;
mod backend;
mod service;
mod sink;
mod types;

#[cfg(target_os = "linux")]
pub mod linux;

pub(crate) use ancestor_watch::AncestorWatch;

/// The process's shared native directory-watch backend, initialized on first use.
///
/// One Linux reader serves its callers. Returns `None` on other platforms or
/// when initialization fails; that failure is cached and is not retried.
pub fn native_directory_watch() -> Option<std::sync::Arc<dyn DirectoryWatchBackend>> {
    #[cfg(target_os = "linux")]
    {
        static NATIVE: std::sync::OnceLock<Option<std::sync::Arc<dyn DirectoryWatchBackend>>> =
            std::sync::OnceLock::new();
        NATIVE
            .get_or_init(|| {
                linux::LinuxInotifyBackend::new().ok().map(|backend| {
                    std::sync::Arc::new(backend) as std::sync::Arc<dyn DirectoryWatchBackend>
                })
            })
            .clone()
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}
pub use backend::{
    BoxHandle, DirectoryWatchBackend, InMemoryDirectoryBackend, NoopBackend, WatchHandle,
};
pub use service::{
    DEFAULT_MAX_PATH_BYTES, DEFAULT_MAX_PENDING_PER_WATCH, DEFAULT_MAX_WATCHES,
    DirectoryWatchService,
};
pub use sink::EventSink;
pub use types::{
    DirectoryChange, IngestReport, RawEvent, RawEventKind, TokenId, WatchDelivery, WatchId,
    WatchOutcome, WatchRefusal, WatchToken,
};

#[cfg(test)]
mod tests;
