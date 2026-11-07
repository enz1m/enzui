//! Noticing when a settings file changes outside this process: another
//! instance of the app, a sync tool or the user's editor (what GSettings'
//! `changed` signal gives GTK apps).
//!
//! [`StoreWatch`] watches the directory of a [`persist::FileBackend`] file
//! through the native directory watch. Map [`StoreWatch::subscription`] into
//! the app's messages and ask [`StoreWatch::is_external_change`] about each
//! batch: `true` means reload the store. Call [`StoreWatch::note_saved`]
//! after every save of this process, so its own writes are not mistaken for
//! someone else's.
//!
//! [`persist::FileBackend`]: crate::persist::FileBackend

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use iced_futures::Subscription;

use crate::file_tree_watch::{
    DirectoryChange, DirectoryWatchBackend, DirectoryWatchService, WatchDelivery, WatchOutcome,
    WatchRefusal,
};
use crate::file_watch::FileIdentity;

/// Watches one store file. See the [module](self) documentation.
pub struct StoreWatch {
    service: DirectoryWatchService,
    file: PathBuf,
    /// The file as this process last wrote or read it.
    known: Mutex<Option<FileIdentity>>,
}

impl std::fmt::Debug for StoreWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoreWatch")
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

impl StoreWatch {
    /// Watches `file` with the native directory watch (inotify on Linux),
    /// taking its current state as known.
    ///
    /// # Errors
    /// [`WatchRefusal::Unsupported`] without a native watch, or the watch
    /// could not be installed.
    pub fn new(file: impl Into<PathBuf>) -> Result<Self, WatchRefusal> {
        let backend = crate::file_tree_watch::native_directory_watch()
            .ok_or(WatchRefusal::Unsupported("no native directory watch"))?;
        Self::with_backend(file, backend)
    }

    /// [`Self::new`] with an explicit backend.
    ///
    /// # Errors
    /// The watch could not be installed.
    pub fn with_backend(
        file: impl Into<PathBuf>,
        backend: std::sync::Arc<dyn DirectoryWatchBackend>,
    ) -> Result<Self, WatchRefusal> {
        let file = file.into();
        let directory = file
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        // One directory, a few events: the file is replaced by renames.
        let service = DirectoryWatchService::with_config(backend, 1, 64, 64 * 1024);
        service.watch(directory)?;
        let watch = Self {
            service,
            file,
            known: Mutex::new(None),
        };
        watch.note_saved();
        Ok(watch)
    }

    /// Change batches for the watched directory.
    pub fn subscription(&self) -> Subscription<Vec<WatchDelivery>> {
        self.service.subscription()
    }

    /// Records the file as it is now as this process's own: call after every
    /// save or load.
    pub fn note_saved(&self) {
        if let Ok(mut known) = self.known.lock() {
            *known = self.identity();
        }
    }

    /// Whether `deliveries` include a change to the file that this process
    /// did not make. Answering `true` takes the new state as known, so the
    /// same change is reported once.
    pub fn is_external_change(&self, deliveries: &[WatchDelivery]) -> bool {
        let touched = deliveries.iter().any(|delivery| match &delivery.outcome {
            WatchOutcome::Changes(changes) => changes.iter().any(|change| self.concerns(change)),
            // Events were dropped or the directory went away: look at the file.
            WatchOutcome::Overflow | WatchOutcome::Vanished => true,
            WatchOutcome::ProbeFailed(_) => false,
        });
        if !touched {
            return false;
        }
        let now = self.identity();
        let Ok(mut known) = self.known.lock() else {
            return false;
        };
        if *known == now {
            return false;
        }
        *known = now;
        true
    }

    fn concerns(&self, change: &DirectoryChange) -> bool {
        match change {
            DirectoryChange::Created(path)
            | DirectoryChange::Deleted(path)
            | DirectoryChange::Modified(path) => self.is_file(path),
            DirectoryChange::Renamed { from, to } => self.is_file(from) || self.is_file(to),
        }
    }

    fn is_file(&self, path: &Path) -> bool {
        path.file_name() == self.file.file_name()
    }

    fn identity(&self) -> Option<FileIdentity> {
        std::fs::metadata(&self.file)
            .ok()
            .map(|metadata| FileIdentity::from_metadata(&metadata))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_tree_watch::{InMemoryDirectoryBackend, RawEventKind};

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch() -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "enzui-store-watch-{}-{}",
            std::process::id(),
            std::thread::current()
                .name()
                .unwrap_or("main")
                .replace("::", "-")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    #[test]
    fn only_changes_made_elsewhere_are_reported() {
        let dir = scratch();
        let file = dir.0.join("settings.state");
        std::fs::write(&file, "theme=dark\n").unwrap();
        let backend = InMemoryDirectoryBackend::new();
        let watch = StoreWatch::with_backend(&file, std::sync::Arc::new(backend.clone())).unwrap();
        let event = |path: &Path| {
            backend.emit(
                &dir.0,
                RawEventKind::Change(DirectoryChange::Modified(path.to_path_buf())),
            );
            watch.service.take_deliveries()
        };

        // This process saves: its own write is not a change.
        std::fs::write(&file, "theme=light\n").unwrap();
        watch.note_saved();
        assert!(!watch.is_external_change(&event(&file)));

        // Another process writes the file: reported once.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&file, "theme=dark, from the other instance\n").unwrap();
        let batch = event(&file);
        assert!(watch.is_external_change(&batch));
        assert!(!watch.is_external_change(&batch), "the same change once");

        // An event for a neighbour file is not this store, even while the
        // store has changed unseen; the store's own event then reports it.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&file, "theme=light, third write\n").unwrap();
        std::fs::write(dir.0.join("other.state"), "x").unwrap();
        assert!(!watch.is_external_change(&event(&dir.0.join("other.state"))));
        assert!(watch.is_external_change(&event(&file)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_native_watch_sees_a_file_replaced_by_rename() {
        let dir = scratch();
        let file = dir.0.join("native.state");
        std::fs::write(&file, "v1").unwrap();
        let watch = StoreWatch::new(&file).unwrap();
        // Stores write a temporary sibling and rename it over the file.
        crate::atomic_state::save_atomic_raw(&file, b"v2 from elsewhere").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut seen = false;
        while !seen && std::time::Instant::now() < deadline {
            seen = watch.is_external_change(&watch.service.take_deliveries());
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(seen, "inotify reported the replaced file");
    }
}
