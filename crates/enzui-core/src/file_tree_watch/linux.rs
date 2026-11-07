//! Linux `inotify` backend.
//!
//! Non-recursive directory watches share one inotify descriptor and reader
//! thread. Subscribers watching the same inode share its kernel watch, while
//! each receives paths composed from the pathname it supplied at installation.
//! Watch installation requires `IN_MASK_CREATE` and refuses recycled watch
//! descriptors, requiring the caller to recreate the backend in that case.
//! Moves are reported as [`Deleted`] and [`Created`], without rename correlation.
//! Moving, deleting, unmounting, or ignoring a watched directory retires its
//! backend subscribers and reports disappearance; the service slot remains
//! installed until the caller cancels it.
//!
//! [`Deleted`]: super::DirectoryChange::Deleted
//! [`Created`]: super::DirectoryChange::Created

use super::backend::{BoxHandle, DirectoryWatchBackend};
use super::sink::EventSink;
use super::types::{DirectoryChange, RawEvent, TokenId, WatchRefusal, WatchToken};
use rustix::event::{EventfdFlags, PollFd, PollFlags, poll};
use rustix::fs::inotify;
use std::ffi::{CString, OsStr, OsString};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const READ_BUF_BYTES: usize = 8 * 1024;
const READER_JOIN_BUDGET: Duration = Duration::from_secs(1);

/// Real Linux backend backed by one shared `inotify` FD.
///
/// Drop requests shutdown through an eventfd wake and checks a one-second
/// reader-exit budget. It joins a reader that published normal exit, otherwise
/// detaches it; dropping from the reader itself detaches immediately. Lock
/// contention and scheduling can extend teardown beyond the retry budget.
///
/// A detached reader retains shared state until its current sink callback
/// returns and it observes shutdown. Surviving watch handles can also retain
/// shared state. A sink panic terminates the reader without publishing its
/// normal-exit signal; the backend does not restart that reader.
pub struct LinuxInotifyBackend {
    shared: Arc<Shared>,
    reader: Option<JoinHandle<()>>,
    reader_exit: Arc<ReaderExit>,
    #[cfg(test)]
    release_probe: Option<Arc<Mutex<Option<ReaderRelease>>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReaderRelease {
    Joined,
    Detached,
    SelfDetached,
}

// Publish normal exit only after reader_loop releases its shared-state ownership.
struct ReaderExit {
    exited: Mutex<bool>,
    changed: Condvar,
}

impl ReaderExit {
    fn new() -> Self {
        Self {
            exited: Mutex::new(false),
            changed: Condvar::new(),
        }
    }

    fn publish(&self) {
        let mut exited = self.exited.lock().unwrap_or_else(PoisonError::into_inner);
        *exited = true;
        drop(exited);
        self.changed.notify_all();
    }

    // Spurious wakes must not restart the exit budget.
    fn wait_for_exit(&self, budget: Duration) -> bool {
        let started = Instant::now();
        let mut exited = self.exited.lock().unwrap_or_else(PoisonError::into_inner);
        while !*exited {
            let Some(remaining) = budget.checked_sub(started.elapsed()) else {
                return false;
            };
            if remaining.is_zero() {
                return false;
            }
            let (guard, _) = self
                .changed
                .wait_timeout(exited, remaining)
                .unwrap_or_else(PoisonError::into_inner);
            exited = guard;
        }
        true
    }
}

struct Shared {
    fd: OwnedFd,
    // Pair shutdown state with a poll wake so idle readers need no timer.
    wake_fd: OwnedFd,
    state: Mutex<State>,
    shutdown: AtomicBool,
    #[cfg(test)]
    test_reader_ready: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    #[cfg(test)]
    test_wake_seen: Mutex<Option<std::sync::mpsc::Sender<()>>>,
    #[cfg(test)]
    test_exit_hold: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    #[cfg(test)]
    test_polls: std::sync::atomic::AtomicUsize,
}

#[derive(Default)]
struct State {
    // Serialize kernel watch changes with subscriber-table changes.
    subs_by_wd: std::collections::HashMap<i32, Vec<Subscriber>>,
    wd_by_token: std::collections::HashMap<TokenId, i32>,
    // Linux allocates new descriptors incrementally. Reuse after wrap can
    // otherwise make unread events target a replacement watch.
    highest_new_wd: Option<i32>,
}

struct Subscriber {
    token_id: TokenId,
    token: WatchToken,
    sink: Arc<dyn EventSink>,
    path: PathBuf,
    alive: Arc<AtomicBool>,
}

#[cfg(test)]
pub(super) struct ReaderControl {
    pub(super) ready: std::sync::mpsc::Receiver<()>,
    pub(super) wake_seen: std::sync::mpsc::Receiver<()>,
    pub(super) release_exit: std::sync::mpsc::Sender<()>,
    release_probe: Arc<Mutex<Option<ReaderRelease>>>,
}

#[cfg(test)]
impl ReaderControl {
    pub(super) fn joined_reader(&self) -> bool {
        self.release() == Some(ReaderRelease::Joined)
    }

    fn release(&self) -> Option<ReaderRelease> {
        *self
            .release_probe
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
pub(super) struct SharedProbe(std::sync::Weak<Shared>);

#[cfg(test)]
impl SharedProbe {
    pub(super) fn is_released(&self) -> bool {
        self.0.upgrade().is_none()
    }
}

struct LinuxHandle {
    shared: Arc<Shared>,
    wd: i32,
    token_id: TokenId,
    alive: Arc<AtomicBool>,
}

impl Drop for LinuxHandle {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        // Hold the state lock across removal so an installer cannot interleave a replacement.
        let mut state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.wd_by_token.remove(&self.token_id);
        let now_empty = if let Some(subs) = state.subs_by_wd.get_mut(&self.wd) {
            subs.retain(|s| s.token_id != self.token_id);
            subs.is_empty()
        } else {
            true
        };
        if now_empty {
            state.subs_by_wd.remove(&self.wd);
            let _ = inotify::remove_watch(self.shared.fd.as_fd(), self.wd);
        }
    }
}

impl Drop for LinuxInotifyBackend {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        let _ = rustix::io::write(&self.shared.wake_fd, &1u64.to_ne_bytes());
        let Some(handle) = self.reader.take() else {
            return;
        };
        let release = if handle.thread().id() == std::thread::current().id() {
            // A sink may release the final backend owner on this reader.
            drop(handle);
            ReaderRelease::SelfDetached
        } else if self.reader_exit.wait_for_exit(READER_JOIN_BUDGET) {
            let _ = handle.join();
            ReaderRelease::Joined
        } else {
            // The reader owns the state it still needs after detachment.
            drop(handle);
            ReaderRelease::Detached
        };
        self.record_release(release);
    }
}

impl LinuxInotifyBackend {
    /// Create a new backend and spawn its reader thread.
    ///
    /// # Errors
    ///
    /// [`WatchRefusal::BackendError`] if `inotify_init1`, `eventfd`, or
    /// the reader thread spawn fails.
    pub fn new() -> Result<Self, WatchRefusal> {
        let fd = inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK)
            .map_err(|e| WatchRefusal::BackendError(format!("inotify_init1: {e}")))?;
        let wake_fd = rustix::event::eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)
            .map_err(|e| WatchRefusal::BackendError(format!("eventfd: {e}")))?;
        let shared = Arc::new(Shared {
            fd,
            wake_fd,
            state: Mutex::new(State::default()),
            shutdown: AtomicBool::new(false),
            #[cfg(test)]
            test_reader_ready: Mutex::new(None),
            #[cfg(test)]
            test_wake_seen: Mutex::new(None),
            #[cfg(test)]
            test_exit_hold: Mutex::new(None),
            #[cfg(test)]
            test_polls: std::sync::atomic::AtomicUsize::new(0),
        });
        let (reader, reader_exit) = spawn_reader(&shared)?;
        Ok(Self {
            shared,
            reader: Some(reader),
            reader_exit,
            #[cfg(test)]
            release_probe: None,
        })
    }

    #[cfg(test)]
    pub(super) fn new_observed() -> Result<(Self, ReaderControl), WatchRefusal> {
        let fd = inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK)
            .map_err(|e| WatchRefusal::BackendError(format!("inotify_init1: {e}")))?;
        let wake_fd = rustix::event::eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)
            .map_err(|e| WatchRefusal::BackendError(format!("eventfd: {e}")))?;
        let (ready_tx, ready) = std::sync::mpsc::channel();
        let (wake_seen_tx, wake_seen) = std::sync::mpsc::channel();
        let (release_exit, exit_hold) = std::sync::mpsc::channel();
        let shared = Arc::new(Shared {
            fd,
            wake_fd,
            state: Mutex::new(State::default()),
            shutdown: AtomicBool::new(false),
            test_reader_ready: Mutex::new(Some(ready_tx)),
            test_wake_seen: Mutex::new(Some(wake_seen_tx)),
            test_exit_hold: Mutex::new(Some(exit_hold)),
            test_polls: std::sync::atomic::AtomicUsize::new(0),
        });
        let (reader, reader_exit) = spawn_reader(&shared)?;
        let release_probe = Arc::new(Mutex::new(None));
        Ok((
            Self {
                shared,
                reader: Some(reader),
                reader_exit,
                release_probe: Some(Arc::clone(&release_probe)),
            },
            ReaderControl {
                ready,
                wake_seen,
                release_exit,
                release_probe,
            },
        ))
    }

    #[cfg(test)]
    fn record_release(&self, release: ReaderRelease) {
        if let Some(probe) = &self.release_probe {
            *probe.lock().unwrap_or_else(PoisonError::into_inner) = Some(release);
        }
    }

    #[cfg(not(test))]
    fn record_release(&self, _release: ReaderRelease) {}

    #[cfg(test)]
    pub(super) fn shared_probe(&self) -> SharedProbe {
        SharedProbe(Arc::downgrade(&self.shared))
    }

    /// Number of live subscribers across every kernel watch.
    #[must_use]
    pub fn active_subscriber_count(&self) -> usize {
        let state = self
            .shared
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state
            .subs_by_wd
            .values()
            .map(|v| v.iter().filter(|s| s.alive.load(Ordering::SeqCst)).count())
            .sum()
    }
}

impl DirectoryWatchBackend for LinuxInotifyBackend {
    fn watch(
        &self,
        path: &Path,
        token: WatchToken,
        sink: Arc<dyn EventSink>,
    ) -> Result<BoxHandle, WatchRefusal> {
        let flags = inotify::WatchFlags::CREATE
            | inotify::WatchFlags::DELETE
            | inotify::WatchFlags::MODIFY
            | inotify::WatchFlags::MOVED_FROM
            | inotify::WatchFlags::MOVED_TO
            | inotify::WatchFlags::ATTRIB
            | inotify::WatchFlags::DELETE_SELF
            | inotify::WatchFlags::MOVE_SELF
            | inotify::WatchFlags::ONLYDIR
            | inotify::WatchFlags::EXCL_UNLINK;
        let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            WatchRefusal::BackendError("path contains an interior NUL byte".to_owned())
        })?;
        let alive = Arc::new(AtomicBool::new(true));
        // Register under the same lock used by handle cancellation.
        let wd = {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let add_error = |e| {
                WatchRefusal::BackendError(format!("inotify_add_watch({}): {e}", path.display()))
            };
            let (wd, created) = match inotify::add_watch(
                self.shared.fd.as_fd(),
                &c_path,
                flags | inotify::WatchFlags::MASK_CREATE,
            ) {
                Ok(wd) => (wd, true),
                Err(rustix::io::Errno::EXIST) => (
                    inotify::add_watch(self.shared.fd.as_fd(), &c_path, flags)
                        .map_err(add_error)?,
                    false,
                ),
                Err(e) => return Err(add_error(e)),
            };
            let known = state.subs_by_wd.contains_key(&wd);
            if created && (known || state.highest_new_wd.is_some_and(|highest| wd <= highest)) {
                let _ = inotify::remove_watch(self.shared.fd.as_fd(), wd);
                return Err(WatchRefusal::BackendError(
                    "inotify watch descriptor was reused; recreate the backend".to_owned(),
                ));
            }
            if !created && !known {
                let _ = inotify::remove_watch(self.shared.fd.as_fd(), wd);
                return Err(WatchRefusal::BackendError(
                    "inotify returned an unowned existing watch".to_owned(),
                ));
            }
            if created {
                state.highest_new_wd = Some(wd);
            }
            state.wd_by_token.insert(token.token_id, wd);
            state.subs_by_wd.entry(wd).or_default().push(Subscriber {
                token_id: token.token_id,
                token: token.clone(),
                sink,
                path: path.to_path_buf(),
                alive: Arc::clone(&alive),
            });
            wd
        };
        Ok(Box::new(LinuxHandle {
            shared: Arc::clone(&self.shared),
            wd,
            token_id: token.token_id,
            alive,
        }))
    }
}

fn spawn_reader(shared: &Arc<Shared>) -> Result<(JoinHandle<()>, Arc<ReaderExit>), WatchRefusal> {
    let exit = Arc::new(ReaderExit::new());
    let reader_shared = Arc::clone(shared);
    let reader_exit = Arc::clone(&exit);
    let handle = std::thread::Builder::new()
        .name("enzui-file-tree-watch".into())
        .spawn(move || {
            reader_loop(reader_shared);
            reader_exit.publish();
        })
        .map_err(|e| WatchRefusal::BackendError(format!("spawn reader: {e}")))?;
    Ok((handle, exit))
}

fn shutdown_requested(shared: &Shared) -> bool {
    if !shared.shutdown.load(Ordering::Acquire) {
        return false;
    }
    #[cfg(test)]
    {
        let hold = shared
            .test_exit_hold
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(rx) = hold {
            let _ = rx.recv();
        }
    }
    true
}

fn reader_loop(shared: Arc<Shared>) {
    let inotify_fd = shared.fd.as_fd();
    let mut buf = [MaybeUninit::<u8>::uninit(); READ_BUF_BYTES];
    loop {
        if shutdown_requested(&shared) {
            return;
        }
        #[cfg(test)]
        {
            let ready = shared
                .test_reader_ready
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
            if let Some(tx) = ready {
                let _ = tx.send(());
            }
        }
        let wake_borrow = shared.wake_fd.as_fd();
        let mut fds = [
            PollFd::new(&inotify_fd, PollFlags::IN),
            PollFd::new(&wake_borrow, PollFlags::IN),
        ];
        // An eventfd shutdown wake permits an untimed idle poll.
        let polled = poll(&mut fds, None);
        #[cfg(test)]
        shared.test_polls.fetch_add(1, Ordering::Relaxed);
        match polled {
            Ok(0) => continue,
            Err(rustix::io::Errno::INTR) => continue,
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            Ok(_) => {}
        }
        if fds[1].revents().contains(PollFlags::IN) {
            let mut drain = 0u64.to_ne_bytes();
            let _ = rustix::io::read(&shared.wake_fd, &mut drain);
            #[cfg(test)]
            {
                let seen = shared
                    .test_wake_seen
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .take();
                if let Some(tx) = seen {
                    let _ = tx.send(());
                }
            }
            continue;
        }
        if !fds[0].revents().contains(PollFlags::IN) {
            continue;
        }
        let mut reader = inotify::Reader::new(BorrowedFdWrap(inotify_fd), &mut buf);
        loop {
            match reader.next() {
                Err(rustix::io::Errno::WOULDBLOCK) => break,
                Err(_) => break,
                Ok(entry) => {
                    dispatch(&shared, &entry);
                    if shutdown_requested(&shared) {
                        return;
                    }
                }
            }
        }
    }
}

struct BorrowedFdWrap<'a>(BorrowedFd<'a>);
impl<'a> AsFd for BorrowedFdWrap<'a> {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0
    }
}
impl<'a> AsRawFd for BorrowedFdWrap<'a> {
    fn as_raw_fd(&self) -> std::os::fd::RawFd {
        self.0.as_raw_fd()
    }
}

fn dispatch(shared: &Arc<Shared>, entry: &inotify::Event<'_>) {
    let flags = entry.events();
    let wd = entry.wd();
    let name = entry
        .file_name()
        .map(|c| OsStr::from_bytes(c.to_bytes()).to_owned());

    if flags.contains(inotify::ReadFlags::QUEUE_OVERFLOW) {
        for sub in snapshot_all_subs(shared) {
            sub.sink.push(RawEvent::overflow(sub.token.clone()));
            if shared.shutdown.load(Ordering::Acquire) {
                return;
            }
        }
        return;
    }

    let terminal = flags.intersects(
        inotify::ReadFlags::IGNORED
            | inotify::ReadFlags::DELETE_SELF
            | inotify::ReadFlags::MOVE_SELF
            | inotify::ReadFlags::UNMOUNT,
    );
    let subs = if terminal {
        retire_subs_for(shared, wd, flags.contains(inotify::ReadFlags::MOVE_SELF))
    } else {
        snapshot_subs_for(shared, wd)
    };
    if subs.is_empty() {
        return;
    }

    if terminal {
        for sub in &subs {
            sub.sink.push(RawEvent::vanished(sub.token.clone()));
            if shared.shutdown.load(Ordering::Acquire) {
                return;
            }
        }
        return;
    }

    let Some(rel_name) = name.as_deref() else {
        return;
    };
    let dchange = if flags.contains(inotify::ReadFlags::MOVED_FROM)
        || flags.contains(inotify::ReadFlags::DELETE)
    {
        Some(DirectoryChange::Deleted as fn(PathBuf) -> DirectoryChange)
    } else if flags.contains(inotify::ReadFlags::MOVED_TO)
        || flags.contains(inotify::ReadFlags::CREATE)
    {
        Some(DirectoryChange::Created as fn(PathBuf) -> DirectoryChange)
    } else if flags.intersects(inotify::ReadFlags::MODIFY | inotify::ReadFlags::ATTRIB) {
        Some(DirectoryChange::Modified as fn(PathBuf) -> DirectoryChange)
    } else {
        None
    };
    let Some(ctor) = dchange else {
        return;
    };
    for sub in &subs {
        let child = compose_child(&sub.path, rel_name);
        sub.sink
            .push(RawEvent::change(sub.token.clone(), ctor(child)));
        if shared.shutdown.load(Ordering::Acquire) {
            return;
        }
    }
}

fn snapshot_subs_for(shared: &Arc<Shared>, wd: i32) -> Vec<Subscriber> {
    let state = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
    state
        .subs_by_wd
        .get(&wd)
        .map(|v| {
            v.iter()
                .filter(|s| s.alive.load(Ordering::SeqCst))
                .map(clone_sub)
                .collect()
        })
        .unwrap_or_default()
}

fn retire_subs_for(shared: &Arc<Shared>, wd: i32, remove_kernel_watch: bool) -> Vec<Subscriber> {
    let mut state = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(subs) = state.subs_by_wd.remove(&wd) else {
        return Vec::new();
    };
    for sub in &subs {
        state.wd_by_token.remove(&sub.token_id);
        sub.alive.store(false, Ordering::SeqCst);
    }
    if remove_kernel_watch {
        let _ = inotify::remove_watch(shared.fd.as_fd(), wd);
    }
    subs
}

fn snapshot_all_subs(shared: &Arc<Shared>) -> Vec<Subscriber> {
    let state = shared.state.lock().unwrap_or_else(PoisonError::into_inner);
    state
        .subs_by_wd
        .values()
        .flatten()
        .filter(|s| s.alive.load(Ordering::SeqCst))
        .map(clone_sub)
        .collect()
}

fn clone_sub(s: &Subscriber) -> Subscriber {
    Subscriber {
        token_id: s.token_id,
        token: s.token.clone(),
        sink: Arc::clone(&s.sink),
        path: s.path.clone(),
        alive: Arc::clone(&s.alive),
    }
}

fn compose_child(parent: &Path, name: &OsStr) -> PathBuf {
    let mut composed = OsString::from(parent.as_os_str());
    if !composed.as_bytes().ends_with(b"/") && !composed.is_empty() {
        composed.push("/");
    }
    composed.push(name);
    PathBuf::from(composed)
}
