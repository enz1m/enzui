//! Process-wide panic reporting with bounded recent-message context.
//!
//! Reports include panic metadata, a forced backtrace, and recent [`MessageKind`] values, never message payloads. The hook never blocks on the ring, writes reports atomically with owner-only permissions on Unix, and chains to the previously installed hook. [`CrashReporter::new`] uses an explicit destination; [`CrashReporter::for_app`] resolves the XDG state directory at installation time.
//!
//! At the next start, [`unseen_reports`] lists the reports nobody has looked
//! at yet (`enzui_widgets::crash_notice` shows one as a banner); mark each
//! [`SavedReport::mark_seen`] once the user has seen or dismissed it.

use crate::civil_date;
use enzui_security::tempstage::TempStage;
use std::backtrace::Backtrace;
use std::fs;
use std::io::{self, Write};
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock, TryLockError};
use std::time::{SystemTime, UNIX_EPOCH};

/// Default and maximum number of recent message kinds retained in the fixed ring.
pub const DEFAULT_RING_CAP: usize = 32;

struct Ring {
    // Live capacity; zero disables recording and the value never exceeds `DEFAULT_RING_CAP`.
    cap: usize,
    // Oldest live slot, always below `cap` when `len` is nonzero.
    head: usize,
    // Live entry count, never greater than `cap`.
    len: usize,
    // Only the `len` entries from `head` are live; all other slots must be ignored.
    items: [&'static str; DEFAULT_RING_CAP],
}

impl Ring {
    const fn new() -> Self {
        Self {
            cap: DEFAULT_RING_CAP,
            head: 0,
            len: 0,
            items: [""; DEFAULT_RING_CAP],
        }
    }

    fn push(&mut self, kind: &'static str) {
        if self.cap == 0 {
            return;
        }
        let slot = (self.head + self.len) % self.cap;
        self.items[slot] = kind;
        if self.len == self.cap {
            self.head = (self.head + 1) % self.cap;
        } else {
            self.len += 1;
        }
    }

    // Iterate live message kinds from oldest to newest.
    fn iter(&self) -> impl Iterator<Item = &'static str> + '_ {
        let cap = self.cap.max(1);
        (0..self.len).map(move |i| self.items[(self.head + i) % cap])
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    // Clamp capacity and retain the newest fitting entries contiguously from slot zero.
    fn set_cap(&mut self, cap: usize) {
        let cap = cap.min(DEFAULT_RING_CAP);
        let keep = self.len.min(cap);
        let dropped = self.len - keep;
        let mut compacted = [""; DEFAULT_RING_CAP];
        let old_cap = self.cap.max(1);
        for (i, slot) in compacted.iter_mut().enumerate().take(keep) {
            *slot = self.items[(self.head + dropped + i) % old_cap];
        }
        self.items = compacted;
        self.head = 0;
        self.len = keep;
        self.cap = cap;
    }
}

static RING: Mutex<Ring> = Mutex::new(Ring::new());

/// Provides a stable, payload-free message kind for crash context.
pub trait MessageKind {
    /// The payload-free kind of this message, e.g. `"Tick"` or `"SyncDone"`.
    fn message_kind(&self) -> &'static str;
}

/// Records one payload-free message kind in the process-wide crash ring.
///
/// Recording performs no allocation. A poisoned ring is ignored rather than propagated. The `&'static str` type prevents runtime message payloads from being borrowed into the ring.
pub fn record(kind: &'static str) {
    if let Ok(mut ring) = RING.lock() {
        ring.push(kind);
    }
}

/// Records `message.message_kind()` without reading message payload fields.
pub fn record_kind<M: MessageKind>(message: &M) {
    record(message.message_kind());
}

/// Records a reducer message’s payload-free kind.
pub fn observe<M: MessageKind>(message: &M) {
    record_kind(message);
}

struct State {
    app: String,
    version: String,
    report_dir: PathBuf,
}

static STATE: OnceLock<State> = OnceLock::new();

/// Configures and installs the process panic-reporting hook.
///
/// Construct with [`Self::new`] for an explicit directory or [`Self::for_app`] for an XDG state directory, then call [`Self::install`] once.
pub struct CrashReporter {
    app: String,
    version: String,
    report_dir: PathBuf,
    ring_cap: usize,
}

impl CrashReporter {
    /// Creates a reporter for `report_dir` without reading the environment.
    ///
    /// The directory is created when the first report is written.
    pub fn new(
        app: impl Into<String>,
        version: impl Into<String>,
        report_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            app: app.into(),
            version: version.into(),
            report_dir: report_dir.into(),
            ring_cap: DEFAULT_RING_CAP,
        }
    }

    /// Sets ring capacity, clamped to [`DEFAULT_RING_CAP`].
    ///
    /// Zero disables recording; values up to the maximum are used exactly.
    #[must_use]
    pub fn ring_capacity(mut self, cap: usize) -> Self {
        self.ring_cap = cap.min(DEFAULT_RING_CAP);
        self
    }

    /// Resolves the app crash directory from XDG state or HOME and installs the hook.
    ///
    /// Returns `false` without installing when no state root is available or `app` is not one confined namespace component.
    pub fn for_app(app: impl Into<String>, version: impl Into<String>) -> bool {
        let app = app.into();
        // Use the shared XDG resolver so crash reports follow the crate-wide state-directory fallback rules.
        let Ok(Some(dir)) = crate::atomic_state::xdg_state_path(&app, "crash") else {
            return false;
        };
        Self::new(app, version, dir).install();
        true
    }

    /// Installs the hook and chains to the previously installed hook after attempting the report.
    ///
    /// The first installed reporter state wins if called more than once.
    pub fn install(self) {
        if let Ok(mut ring) = RING.lock() {
            ring.set_cap(self.ring_cap);
        }
        let _ = STATE.set(State {
            app: self.app,
            version: self.version,
            report_dir: self.report_dir,
        });

        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            write_report(info);
            previous(info);
        }));
    }
}

// Ignore report-write errors so the previously installed panic hook can still run.
fn write_report(info: &PanicHookInfo<'_>) {
    let Some(state) = STATE.get() else {
        return;
    };

    // Force capture because unattended processes cannot rely on `RUST_BACKTRACE` being enabled.
    let backtrace = Backtrace::force_capture();

    let thread = std::thread::current();
    let thread_name = thread.name().unwrap_or("<unnamed>").to_owned();

    let message = panic_message(info.payload());
    let location = info.location().map_or_else(
        || "<unknown location>".to_owned(),
        |l| format!("{}:{}:{}", l.file(), l.line(), l.column()),
    );

    // Never block on the ring: the panicking thread may already hold its mutex, so render without ring data on contention.
    let ring_section = match RING.try_lock() {
        Ok(guard) => {
            if guard.is_empty() {
                "  (no messages recorded)".to_owned()
            } else {
                guard
                    .iter()
                    .map(|k| format!("  {k}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        Err(TryLockError::WouldBlock) => {
            "  (recent-message ring unavailable: its lock was held when the \
             panic fired)"
                .to_owned()
        }
        Err(TryLockError::Poisoned(poison)) => {
            let guard = poison.into_inner();
            if guard.is_empty() {
                "  (recent-message ring was poisoned; no messages)".to_owned()
            } else {
                let mut out =
                    String::from("  (recent-message ring was poisoned; showing what survived)\n");
                out.push_str(
                    &guard
                        .iter()
                        .map(|k| format!("  {k}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
                out
            }
        }
    };

    let (timestamp, epoch_secs) = utc_now();

    let body = format!(
        "enzUI crash report\n\
         ==================\n\
         app:       {app} {version}\n\
         time:      {timestamp} (epoch {epoch_secs})\n\
         thread:    {thread_name}\n\
         location:  {location}\n\
         panic:     {message}\n\
         \n\
         recent message kinds (oldest first, cap {cap}):\n\
         {ring_section}\n\
         \n\
         backtrace (force_capture):\n\
         {backtrace}\n",
        app = state.app,
        version = state.version,
        cap = ring_cap_snapshot(),
    );

    let path = state.report_dir.join(report_filename());
    let _ = write_atomic(&path, &body);
}

fn ring_cap_snapshot() -> usize {
    match RING.try_lock() {
        Ok(g) => g.cap,
        Err(TryLockError::Poisoned(p)) => p.into_inner().cap,
        Err(TryLockError::WouldBlock) => DEFAULT_RING_CAP,
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic payload>".to_owned()
    }
}

static SEQ: AtomicU64 = AtomicU64::new(0);

fn report_filename() -> String {
    let (utc, _) = utc_now();
    let compact: String = utc.chars().filter(char::is_ascii_alphanumeric).collect();
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("crash-{compact}-{}-{seq}-{nanos}.txt", std::process::id())
}

fn utc_now() -> (String, u64) {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i32;
    let tod = secs % 86_400;
    let (h, m, s) = (tod / 3_600, (tod % 3_600) / 60, tod % 60);
    let date = civil_date::Date::from_days(days);
    (format!("{date}T{h:02}:{m:02}:{s:02}Z"), secs)
}

// Publish through an owner-only temp sibling, file sync, rename, and parent sync; never open the destination, so a final symlink is replaced rather than followed.
fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)?;
    }

    let mut staged = TempStage::create(path)?;
    staged.file_mut().write_all(contents.as_bytes())?;
    staged.commit()
}

/// The crash report directory [`CrashReporter::for_app`] writes to.
///
/// # Errors
/// [`crate::persist::XdgPathError`] unless `app` is one confined component.
pub fn default_dir(app: &str) -> Result<Option<PathBuf>, crate::persist::XdgPathError> {
    crate::atomic_state::xdg_state_path(app, "crash")
}

const SEEN_PREFIX: &str = "seen-";

/// A crash report on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedReport {
    path: PathBuf,
    written: Option<SystemTime>,
    panic: String,
}

/// The reports in `dir` not yet marked seen, newest first.
#[must_use]
pub fn unseen_reports(dir: &Path) -> Vec<SavedReport> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut reports: Vec<SavedReport> = entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("crash-") && name.ends_with(".txt"))
        })
        .map(|entry| {
            let path = entry.path();
            // The panic line, for a one-line summary; reports are small.
            let panic = fs::read_to_string(&path)
                .ok()
                .and_then(|body| {
                    body.lines()
                        .find_map(|line| line.strip_prefix("panic:"))
                        .map(|panic| panic.trim().to_owned())
                })
                .unwrap_or_default();
            SavedReport {
                written: entry.metadata().and_then(|meta| meta.modified()).ok(),
                path,
                panic,
            }
        })
        .collect();
    reports.sort_by_key(|report| std::cmp::Reverse(report.written));
    reports
}

impl SavedReport {
    /// Where the report is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// When it was written.
    #[must_use]
    pub fn written(&self) -> Option<SystemTime> {
        self.written
    }

    /// The panic message, for a one-line summary.
    #[must_use]
    pub fn panic_message(&self) -> &str {
        &self.panic
    }

    /// The whole report.
    ///
    /// # Errors
    /// The file cannot be read.
    pub fn read(&self) -> io::Result<String> {
        fs::read_to_string(&self.path)
    }

    /// Keeps the report but stops listing it: the user has seen it. Returns
    /// where it now is.
    ///
    /// # Errors
    /// The file cannot be renamed.
    pub fn mark_seen(&self) -> io::Result<PathBuf> {
        let name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| io::Error::other("a report without a file name"))?;
        let seen = self.path.with_file_name(format!("{SEEN_PREFIX}{name}"));
        fs::rename(&self.path, &seen)?;
        Ok(seen)
    }
}

#[cfg(test)]
mod tests;
