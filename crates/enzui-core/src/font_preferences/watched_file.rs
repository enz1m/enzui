//! The settings-file font backend with a directory watch as its live signal,
//! so an idle app re-reads the file only when something under it changes.

use crate::file_tree_watch::{AncestorWatch, DirectoryWatchBackend, EventSink, RawEvent};
use enzui_theme::font_source::{
    DesktopFontBackend, FontPreferenceRead, FontPreferenceSignals, FontSignal,
    SettingsFileFontBackend, SignalCancel,
};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::task::{Context, Wake, Waker};
use std::time::{Duration, Instant};

/// [`SettingsFileFontBackend`] whose [`subscribe`](DesktopFontBackend::subscribe)
/// watches the directories of its settings files.
#[derive(Debug)]
pub struct WatchedSettingsFontBackend {
    inner: SettingsFileFontBackend,
    watch: WatchBackend,
}

struct WatchBackend(Arc<dyn DirectoryWatchBackend>);

impl std::fmt::Debug for WatchBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DirectoryWatchBackend")
    }
}

impl WatchedSettingsFontBackend {
    /// `inner`, signalled through `watch`.
    #[must_use]
    pub fn new(inner: SettingsFileFontBackend, watch: Arc<dyn DirectoryWatchBackend>) -> Self {
        Self {
            inner,
            watch: WatchBackend(watch),
        }
    }
}

impl DesktopFontBackend for WatchedSettingsFontBackend {
    fn identity(&self) -> String {
        self.inner.identity()
    }

    fn read(&self) -> FontPreferenceRead {
        self.inner.read()
    }

    fn subscribe(&self) -> Option<Box<dyn FontPreferenceSignals>> {
        let state = Arc::new(Signals::default());
        let mut watch = AncestorWatch::new(
            Arc::clone(&self.watch.0),
            self.inner.candidates().iter().map(|(path, _)| path.clone()),
            Arc::new(ChangeSink(Arc::downgrade(&state))),
        );
        if !watch.refresh() {
            return None;
        }
        Some(Box::new(WatchedSignals { watch, state }))
    }

    fn poll_interval(&self) -> Duration {
        self.inner.poll_interval()
    }
}

#[derive(Default)]
struct Signals {
    changed: Mutex<bool>,
    wake: Condvar,
}

impl Wake for Signals {
    fn wake(self: Arc<Self>) {
        self.wake.notify_all();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wake.notify_all();
    }
}

struct ChangeSink(std::sync::Weak<Signals>);

impl EventSink for ChangeSink {
    fn push(&self, _event: RawEvent) {
        if let Some(state) = self.0.upgrade() {
            *state.changed.lock().unwrap_or_else(PoisonError::into_inner) = true;
            state.wake.notify_all();
        }
    }
}

struct WatchedSignals {
    watch: AncestorWatch,
    state: Arc<Signals>,
}

impl WatchedSignals {
    fn wait(&mut self, timeout: Duration, cancel: Option<&SignalCancel>) -> FontSignal {
        if let Some(cancel) = cancel {
            // Registers this thread's condvar as the cancellation waker.
            let waker = Waker::from(Arc::clone(&self.state));
            let mut cancelled = std::pin::pin!(cancel.cancelled());
            if cancelled
                .as_mut()
                .poll(&mut Context::from_waker(&waker))
                .is_ready()
            {
                return FontSignal::Idle;
            }
        }
        let deadline = Instant::now().checked_add(timeout);
        let mut changed = self
            .state
            .changed
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        loop {
            if std::mem::take(&mut *changed) {
                drop(changed);
                // A directory on the path may have appeared: follow it down.
                self.watch.refresh();
                return FontSignal::Changed;
            }
            if cancel.is_some_and(SignalCancel::is_cancelled) {
                return FontSignal::Idle;
            }
            let now = Instant::now();
            let remaining = match deadline {
                Some(deadline) if deadline > now => deadline - now,
                Some(_) => return FontSignal::Idle,
                None => Duration::from_secs(3600),
            };
            changed = self
                .state
                .wake
                .wait_timeout(changed, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl FontPreferenceSignals for WatchedSignals {
    fn wait_for_change(&mut self, timeout: Duration) -> FontSignal {
        self.wait(timeout, None)
    }

    fn wait_for_change_until_cancelled(
        &mut self,
        timeout: Duration,
        _slice: Duration,
        cancel: &SignalCancel,
    ) -> FontSignal {
        self.wait(timeout, Some(cancel))
    }
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;
    use enzui_theme::font_source::SettingsFileKind;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("enzui-watched-fonts-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn signals(settings: PathBuf) -> Box<dyn FontPreferenceSignals> {
        let backend = WatchedSettingsFontBackend::new(
            SettingsFileFontBackend::new(vec![(settings, SettingsFileKind::GtkSettingsIni)]),
            Arc::new(crate::file_tree_watch::linux::LinuxInotifyBackend::new().expect("inotify")),
        );
        backend
            .subscribe()
            .expect("a watch over an existing ancestor")
    }

    /// A write under a not-yet-existing settings directory wakes a long
    /// wait at once, and cancelling releases one that sees no change.
    ///
    /// Sabotage: make `ChangeSink::push` do nothing and the first wait runs
    /// to its timeout; drop the cancellation waker registration and the
    /// second wait ignores `cancel`.
    #[test]
    fn a_settings_write_wakes_the_wait_and_cancel_releases_it() {
        let dir = temp_dir("write");
        let settings = dir.join("gtk-4.0").join("settings.ini");
        let mut signals = signals(settings.clone());
        let cancel = SignalCancel::default();

        let writer = {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                std::fs::create_dir_all(settings.parent().expect("parent")).expect("mkdir");
                std::fs::write(&settings, "[Settings]\ngtk-font-name=Inter 11\n").expect("write");
            })
        };
        let started = Instant::now();
        let signal = signals.wait_for_change_until_cancelled(
            Duration::from_secs(30),
            Duration::from_secs(2),
            &cancel,
        );
        writer.join().expect("writer");
        assert_eq!(signal, FontSignal::Changed);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );

        let canceller = {
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                cancel.cancel();
            })
        };
        // Drain events from the setup before waiting on silence.
        while signals.wait_for_change(Duration::from_millis(20)) == FontSignal::Changed {}
        let started = Instant::now();
        let signal = signals.wait_for_change_until_cancelled(
            Duration::from_secs(30),
            Duration::from_secs(2),
            &cancel,
        );
        canceller.join().expect("canceller");
        assert_eq!(signal, FontSignal::Idle);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
