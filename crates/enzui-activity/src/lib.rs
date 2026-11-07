//! App-scoped background activity publication over the enzui activity v1 Unix socket.
//!
//! Create one [`Publisher`] in the UI process and retain a [`Job`] for each
//! operation. Updates only replace bounded in-memory state; socket I/O and ACKs
//! run on a dedicated thread. The last unfinished job handle removes its
//! status; finishing never performs or cancels producer work. Live records
//! reconnect after broker failure; terminal records are discarded on disconnect.
//! Labels and details must be localized, generic descriptions without paths,
//! names or credentials. A window token must match a native compositor tag;
//! an iced window ID does not qualify.
//!
//! Where the socket cannot be reached (another desktop, or Flatpak, where the
//! activity socket is not exposed), [`Publisher::new`] shows the live jobs as
//! standard `com.canonical.Unity.LauncherEntry` dock progress instead, at most
//! twice a second, and never alongside the socket. Flatpak apps need
//! `--talk-name=com.canonical.Unity` for it.

#![cfg_attr(docsrs, feature(doc_cfg))]
#[cfg(feature = "operation-center")]
mod center;
#[cfg(feature = "operation-center")]
pub use center::CenterJobs;
mod launcher;
mod transport;

use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// Generates a random opaque native window tag, without identifying the app or window.
///
/// Queue it with the native window API before attaching it to work metadata.
/// If entropy or native support is unavailable, omit the token for PID fallback.
///
/// # Errors
/// Returns the operating system's entropy failure.
pub fn new_window_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(std::io::Error::other)?;
    let mut token = String::with_capacity(38);
    token.push_str("enzui-");
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut token, "{byte:02x}");
    }
    Ok(token)
}

/// Maximum locally retained records, including completion linger and pending removal.
pub const MAX_JOBS: usize = 16;

/// A determinate producer measurement. Unknown progress is represented by `None`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Progress {
    /// Units finished so far.
    pub done: u64,
    /// Units in all; positive and at least `done`.
    pub total: u64,
    /// What the units count.
    pub unit: Unit,
}

/// The producer's measurement unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// Bytes transferred or processed.
    Bytes,
    /// Discrete items, such as files.
    Items,
    /// Stages of a multi-step operation.
    Steps,
}

/// Paused work remains live but carries no ETA countdown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// The work is advancing.
    Running,
    /// The work is held; it has no ETA.
    Paused,
}

/// The producer's actual terminal result.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The work finished as intended.
    Succeeded,
    /// The work stopped on an error.
    Failed,
    /// The work was stopped on request.
    Cancelled,
}

/// Whether the shell should also present work while its originating window is active.
///
/// This affects the enzimOS live activity surface. Other desktops' launcher progress
/// does not expose a corresponding visibility policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    /// Hide work when its verified originating window is active.
    #[default]
    WhenUnfocused,
    /// Keep work visible while its originating window is active, for example a timer.
    Always,
}

/// Complete replacement metadata; ETA is producer-estimated milliseconds.
///
/// Labels allow 96 UTF-8 bytes and must contain non-whitespace text; details
/// allow 160 bytes and may be empty. Both reject control characters and bidi
/// embedding/isolate markers. Progress requires a positive total and `done <= total`;
/// ETA is at most seven days and must be absent while paused. Window tags allow
/// 1–128 ASCII bytes from letters, digits, and `._:-`.
///
/// Validation does not detect paths, credentials, or other sensitive content;
/// callers must supply generic localized descriptions.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Status {
    /// What the work is, in at most 96 bytes.
    pub label: String,
    /// A secondary line, in at most 160 bytes; may be empty.
    pub detail: String,
    /// Whether the work is running or paused.
    pub state: State,
    /// How far it has come, or `None` when that is unknown.
    pub progress: Option<Progress>,
    /// The estimated milliseconds left, at most seven days; `None` while paused.
    pub eta_ms: Option<u64>,
    /// The compositor tag of the window the work belongs to, if any.
    pub window_token: Option<String>,
}

impl Status {
    /// Starts indeterminate work with no ETA or window association.
    pub fn running(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: String::new(),
            state: State::Running,
            progress: None,
            eta_ms: None,
            window_token: None,
        }
    }

    fn valid(&self) -> bool {
        text_valid(&self.label, 96, false)
            && text_valid(&self.detail, 160, true)
            && self
                .progress
                .as_ref()
                .is_none_or(|p| p.total > 0 && p.done <= p.total)
            && self.eta_ms.is_none_or(|eta| eta <= 604_800_000)
            && (self.state != State::Paused || self.eta_ms.is_none())
            && self.window_token.as_deref().is_none_or(window_token_valid)
    }
}

/// Local admission errors only; service failures are handled asynchronously.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The status failed validation.
    InvalidMetadata,
    /// [`MAX_JOBS`] records are already retained.
    Capacity,
    /// The job already finished.
    Finished,
    /// The publisher has shut down.
    Closed,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "activity publisher: {self:?}")
    }
}

impl std::error::Error for Error {}

fn text_valid(value: &str, max: usize, empty: bool) -> bool {
    value.len() <= max
        && (empty || !value.trim().is_empty())
        && !value.chars().any(|c| {
            c.is_control() || matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
}

fn window_token_valid(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 128
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b))
}

fn identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

struct Shared {
    inner: Mutex<Inner>,
    changed: Notify,
    stopped: Notify,
}

impl Shared {
    /// The job table, as a panicking holder left it: one panic must not
    /// wedge every later report.
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

struct Inner {
    closed: bool,
    next: u64,
    jobs: BTreeMap<String, Entry>,
}

#[derive(Clone)]
struct Entry {
    status: Status,
    visibility: Visibility,
    visible_at: Option<Instant>,
    version: u64,
    terminal: Option<Outcome>,
    removed: bool,
}

/// The broker socket: an absolute `ENZUI_ACTIVITY_SOCKET`, else the
/// `enzui-activity/service.sock` beneath an absolute runtime directory.
fn default_socket(
    explicit: Option<std::ffi::OsString>,
    runtime: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(socket) = explicit
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        return Some(socket);
    }
    runtime
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .map(|runtime| runtime.join("enzui-activity/service.sock"))
}

/// One UI-process connection owner. Drop initiates asynchronous disconnect.
pub struct Publisher {
    shared: Arc<Shared>,
}

impl Publisher {
    /// Uses `$ENZUI_ACTIVITY_SOCKET` when it is an absolute path, else
    /// `$XDG_RUNTIME_DIR/enzui-activity/service.sock`.
    ///
    /// Missing runtime directory, invalid app identity or thread creation fails
    /// locally. A missing broker does not fail construction.
    ///
    /// # Errors
    /// Returns an error for missing runtime configuration, invalid app identity,
    /// runtime creation, or worker-thread creation.
    pub fn new(app_id: impl Into<String>) -> std::io::Result<Self> {
        let socket = default_socket(
            std::env::var_os("ENZUI_ACTIVITY_SOCKET"),
            std::env::var_os("XDG_RUNTIME_DIR"),
        )
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "runtime directory unavailable",
            )
        })?;
        Self::spawn(app_id, socket, launcher::Bus::Session)
    }

    /// Selects a socket explicitly for a qualified proxy or an isolated test.
    /// This grants no sandbox permission and never changes directory access.
    /// App IDs must contain a dot with no empty components and use at most
    /// 128 ASCII bytes from letters, digits, and `._-`. An explicit socket
    /// sends no launcher progress when it is unreachable.
    ///
    /// # Errors
    /// Returns an error for invalid app identity or runtime/worker creation;
    /// connection and protocol failures are retried asynchronously.
    pub fn at_socket(app_id: impl Into<String>, socket: PathBuf) -> std::io::Result<Self> {
        Self::spawn(app_id, socket, launcher::Bus::Disabled)
    }

    /// [`Self::at_socket`], showing launcher progress on the bus at
    /// `bus_address` while the socket is unreachable, as [`Self::new`] does on
    /// the session bus.
    ///
    /// # Errors
    /// As [`Self::at_socket`].
    pub fn at_socket_with_launcher_bus(
        app_id: impl Into<String>,
        socket: PathBuf,
        bus_address: impl Into<String>,
    ) -> std::io::Result<Self> {
        Self::spawn(app_id, socket, launcher::Bus::Address(bus_address.into()))
    }

    fn spawn(
        app_id: impl Into<String>,
        socket: PathBuf,
        bus: launcher::Bus,
    ) -> std::io::Result<Self> {
        let app_id = app_id.into();
        if !identifier(&app_id, 128)
            || !app_id.contains('.')
            || app_id.split('.').any(str::is_empty)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid app identity",
            ));
        }
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                closed: false,
                next: 0,
                jobs: BTreeMap::new(),
            }),
            changed: Notify::new(),
            stopped: Notify::new(),
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let actor = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("activity-publisher".into())
            .spawn(move || {
                runtime.block_on(transport::run(actor, app_id, socket, bus));
            })?;
        Ok(Self { shared })
    }

    /// Admits one status record. Capacity includes completion linger and pending
    /// cleanup; callers must continue their operation even if admission fails.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidMetadata`] when `status` fails validation, [`Error::Closed`] after the
    /// publisher shut down, and [`Error::Capacity`] when [`MAX_JOBS`] records are retained.
    pub fn start(&self, status: Status) -> Result<Job, Error> {
        self.start_with_visibility(status, Visibility::default())
    }

    /// Admits work with an explicit shell visibility policy.
    ///
    /// Existing [`Self::start`] callers hide work while its originating window is active.
    ///
    /// # Errors
    /// Returns the same admission errors as [`Self::start`].
    pub fn start_with_visibility(
        &self,
        status: Status,
        visibility: Visibility,
    ) -> Result<Job, Error> {
        self.start_at(status, visibility, None)
    }

    /// Admits work that should stay quiet unless it lasts at least `delay`.
    ///
    /// Measurements still coalesce while hidden. Finishing or dropping the job
    /// before its reveal deadline removes it without publishing a completion.
    /// This delays presentation only; callers start the actual operation immediately.
    ///
    /// # Errors
    /// Returns the admission errors from [`Self::start`]. A delay that overflows
    /// the monotonic clock returns [`Error::InvalidMetadata`].
    pub fn start_after(
        &self,
        status: Status,
        visibility: Visibility,
        delay: Duration,
    ) -> Result<Job, Error> {
        let visible_at = Instant::now()
            .checked_add(delay)
            .ok_or(Error::InvalidMetadata)?;
        self.start_at(status, visibility, Some(visible_at))
    }

    fn start_at(
        &self,
        status: Status,
        visibility: Visibility,
        visible_at: Option<Instant>,
    ) -> Result<Job, Error> {
        if !status.valid() {
            return Err(Error::InvalidMetadata);
        }
        let mut inner = self.shared.lock();
        if inner.closed {
            return Err(Error::Closed);
        }
        if inner.jobs.len() >= MAX_JOBS {
            return Err(Error::Capacity);
        }
        inner.next = inner.next.checked_add(1).ok_or(Error::Capacity)?;
        let key = format!("job-{}", inner.next);
        inner.jobs.insert(
            key.clone(),
            Entry {
                status,
                visibility,
                visible_at,
                version: 1,
                terminal: None,
                removed: false,
            },
        );
        drop(inner);
        self.shared.changed.notify_one();
        Ok(Job {
            lease: Arc::new(Lease {
                shared: Arc::downgrade(&self.shared),
                key,
            }),
        })
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        self.shared.lock().closed = true;
        self.shared.stopped.notify_one();
    }
}

/// Cloneable producer handle. The last unfinished handle's drop removes status.
/// Completion retains local capacity for four seconds after broker acknowledgement.
/// Terminal records are discarded when the connection ends, even if not acknowledged.
#[derive(Clone)]
pub struct Job {
    lease: Arc<Lease>,
}

struct Lease {
    shared: Weak<Shared>,
    key: String,
}

impl Job {
    /// Replaces desired metadata without socket I/O or an unbounded queue.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidMetadata`] when `status` fails validation; [`Error::Closed`] or
    /// [`Error::Finished`] when the job can no longer change.
    pub fn update(&self, status: Status) -> Result<(), Error> {
        if !status.valid() {
            return Err(Error::InvalidMetadata);
        }
        self.change(|entry| {
            if entry.status != status {
                entry.status = status;
                entry.version += 1;
            }
        })
    }

    /// Changes measurements without replacing window identity, labels or live phase.
    ///
    /// Unknown measurements use `None`; paused work clears ETA. Invalid counts or
    /// an ETA beyond seven days leave all desired metadata unchanged.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidMetadata`] when the counts or ETA are invalid; [`Error::Closed`] or
    /// [`Error::Finished`] when the job can no longer change.
    pub fn set_progress(
        &self,
        progress: Option<Progress>,
        eta_ms: Option<u64>,
    ) -> Result<(), Error> {
        if progress
            .as_ref()
            .is_some_and(|p| p.total == 0 || p.done > p.total)
            || eta_ms.is_some_and(|eta| eta > 604_800_000)
        {
            return Err(Error::InvalidMetadata);
        }
        self.change(|entry| {
            let eta_ms = if entry.status.state == State::Paused {
                None
            } else {
                eta_ms
            };
            if entry.status.progress != progress || entry.status.eta_ms != eta_ms {
                entry.status.progress = progress;
                entry.status.eta_ms = eta_ms;
                entry.version += 1;
            }
        })
    }

    /// Replaces the window tag without changing producer measurements.
    ///
    /// Only tag syntax is checked; callers must ensure the compositor uses that tag.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidMetadata`] when the tag is malformed; [`Error::Closed`] or
    /// [`Error::Finished`] when the job can no longer change.
    pub fn set_window_token(&self, token: Option<String>) -> Result<(), Error> {
        if token
            .as_deref()
            .is_some_and(|token| !window_token_valid(token))
        {
            return Err(Error::InvalidMetadata);
        }
        self.change(|entry| {
            if entry.status.window_token != token {
                entry.status.window_token = token;
                entry.version += 1;
            }
        })
    }

    /// Changes shell visibility without replacing progress or window identity.
    ///
    /// # Errors
    /// [`Error::Closed`] or [`Error::Finished`] when the job can no longer change.
    pub fn set_visibility(&self, visibility: Visibility) -> Result<(), Error> {
        self.change(|entry| {
            if entry.visibility != visibility {
                entry.visibility = visibility;
                entry.version += 1;
            }
        })
    }

    /// Changes the live phase; pausing clears the producer's ETA.
    ///
    /// # Errors
    ///
    /// [`Error::Closed`] or [`Error::Finished`] when the job can no longer change.
    pub fn set_state(&self, state: State) -> Result<(), Error> {
        self.change(|entry| {
            if entry.status.state == state {
                return;
            }
            entry.status.state = state;
            if state == State::Paused {
                entry.status.eta_ms = None;
            }
            entry.version += 1;
        })
    }

    /// Queues terminal status without performing, cancelling, or joining producer work.
    ///
    /// Success acknowledges local admission only. Terminal status is not replayed
    /// after disconnect and can be lost before the broker receives it.
    ///
    /// # Errors
    ///
    /// [`Error::Closed`] or [`Error::Finished`] when the job can no longer change.
    pub fn finish(&self, outcome: Outcome) -> Result<(), Error> {
        self.change(|entry| {
            entry.terminal = Some(outcome);
            if entry
                .visible_at
                .is_some_and(|deadline| deadline > Instant::now())
            {
                entry.removed = true;
            }
        })
    }

    fn change(&self, change: impl FnOnce(&mut Entry)) -> Result<(), Error> {
        let shared = self.lease.shared.upgrade().ok_or(Error::Closed)?;
        let mut inner = shared.lock();
        if inner.closed {
            return Err(Error::Closed);
        }
        let entry = inner.jobs.get_mut(&self.lease.key).ok_or(Error::Finished)?;
        if entry.terminal.is_some() || entry.removed {
            return Err(Error::Finished);
        }
        let before = (entry.version, entry.terminal);
        change(entry);
        let changed = before != (entry.version, entry.terminal);
        drop(inner);
        if changed {
            shared.changed.notify_one();
        }
        Ok(())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(shared) = self.shared.upgrade() {
            if let Some(entry) = shared.lock().jobs.get_mut(&self.key)
                && entry.terminal.is_none()
            {
                entry.removed = true;
            }
            shared.changed.notify_one();
        }
    }
}

#[cfg(test)]
mod socket_path_tests {
    use super::default_socket;
    use std::path::PathBuf;

    #[test]
    fn relative_paths_are_ignored() {
        assert_eq!(
            default_socket(Some("relative.sock".into()), Some("/run/user/1000".into())),
            Some(PathBuf::from("/run/user/1000/enzui-activity/service.sock"))
        );
        assert_eq!(default_socket(None, Some("run/user".into())), None);
        assert_eq!(default_socket(None, None), None);
    }
}
