//! Copies of unsaved work that survive a crash, and finding them again.
//!
//! A [`RecoveryStore`] is this process's session directory under
//! `$XDG_STATE_HOME/<app>/recovery`, locked for as long as the process lives.
//! Write a copy of each unsaved document with [`RecoveryStore::save`] (an
//! [`autosave`](crate::autosave) schedule suits it), drop it with
//! [`RecoveryStore::discard`] once the document is saved or closed, and call
//! [`RecoveryStore::finish`] on a clean exit.
//!
//! The kernel releases the lock when a process dies, however it dies. At the
//! next start, [`orphans`] lists the copies of sessions whose lock is free:
//! offer each to the user, then [`Recovered::read`] it or
//! [`Recovered::discard`] it. Sessions of other running instances stay
//! locked and are not listed.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::atomic_state::{self, StateLock};

const SESSION_FILE: &str = "session";
const COPY_SUFFIX: &str = ".copy";

/// This process's recovery copies. See the [module](self) documentation.
#[derive(Debug)]
pub struct RecoveryStore {
    dir: PathBuf,
    _lock: StateLock,
}

/// A copy left behind by a session that ended without finishing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovered {
    key: String,
    path: PathBuf,
    modified: Option<SystemTime>,
}

/// The recovery directory of `app`: `$XDG_STATE_HOME/<app>/recovery`.
///
/// # Errors
/// [`crate::persist::XdgPathError`] unless `app` is one ordinary path component.
pub fn default_root(app: &str) -> Result<Option<PathBuf>, crate::persist::XdgPathError> {
    atomic_state::xdg_state_path(app, "recovery")
}

impl RecoveryStore {
    /// Opens a new session under `root` and locks it.
    ///
    /// # Errors
    /// The directory or its lock cannot be created.
    pub fn open(root: &Path) -> io::Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let dir = root.join(format!("{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let lock = StateLock::try_acquire(&dir.join(SESSION_FILE))?
            .ok_or_else(|| io::Error::other("the recovery session is already locked"))?;
        Ok(Self { dir, _lock: lock })
    }

    /// Writes the copy of document `key` (any string the app knows it by,
    /// such as its path or `untitled-2`), replacing the previous one.
    ///
    /// # Errors
    /// The atomic write failed.
    pub fn save(&self, key: &str, bytes: &[u8]) -> io::Result<()> {
        atomic_state::save_atomic_raw(&self.copy_path(key), bytes)
    }

    /// Removes the copy of `key`, once its document is saved or closed.
    ///
    /// # Errors
    /// The file exists but cannot be removed.
    pub fn discard(&self, key: &str) -> io::Result<()> {
        match std::fs::remove_file(self.copy_path(key)) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    /// Ends the session on a clean exit, removing every copy in it.
    ///
    /// # Errors
    /// The session directory cannot be removed.
    pub fn finish(self) -> io::Result<()> {
        std::fs::remove_dir_all(&self.dir)
    }

    fn copy_path(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{}{COPY_SUFFIX}", encode_key(key)))
    }
}

/// The copies under `root` whose session has ended without finishing,
/// newest first. Empty finished sessions are removed on the way.
///
/// # Errors
/// `root` exists but cannot be read.
pub fn orphans(root: &Path) -> io::Result<Vec<Recovered>> {
    let sessions = match std::fs::read_dir(root) {
        Ok(sessions) => sessions,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut found = Vec::new();
    for session in sessions.flatten() {
        let dir = session.path();
        if !dir.is_dir() {
            continue;
        }
        // A live session holds its lock.
        let Ok(Some(_lock)) = StateLock::try_acquire(&dir.join(SESSION_FILE)) else {
            continue;
        };
        let mut copies = 0;
        for entry in std::fs::read_dir(&dir)?.flatten() {
            let name = entry.file_name();
            let Some(key) = name
                .to_str()
                .and_then(|name| name.strip_suffix(COPY_SUFFIX))
                .and_then(decode_key)
            else {
                continue;
            };
            copies += 1;
            found.push(Recovered {
                key,
                path: entry.path(),
                modified: entry.metadata().and_then(|meta| meta.modified()).ok(),
            });
        }
        if copies == 0 {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
    found.sort_by_key(|copy| std::cmp::Reverse(copy.modified));
    Ok(found)
}

impl Recovered {
    /// The key the document was saved under.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// When the copy was last written.
    #[must_use]
    pub fn modified(&self) -> Option<SystemTime> {
        self.modified
    }

    /// The copy's contents.
    ///
    /// # Errors
    /// The file cannot be read.
    pub fn read(&self) -> io::Result<Vec<u8>> {
        std::fs::read(&self.path)
    }

    /// Deletes the copy, and its session once nothing is left in it.
    ///
    /// # Errors
    /// The file exists but cannot be removed.
    pub fn discard(self) -> io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
            _ => {}
        }
        if let Some(dir) = self.path.parent() {
            let empty = std::fs::read_dir(dir).is_ok_and(|entries| {
                entries.flatten().all(|entry| {
                    !entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.ends_with(COPY_SUFFIX))
                })
            });
            if empty {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
        Ok(())
    }
}

/// A key as a file name: lowercase hex of its UTF-8 bytes.
fn encode_key(key: &str) -> String {
    key.bytes().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_key(name: &str) -> Option<String> {
    if !name.len().is_multiple_of(2) {
        return None;
    }
    let bytes = (0..name.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(name.get(at..at + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Root(PathBuf);

    impl Root {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("enzui-core-recovery-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_crashed_session_leaves_its_copies_for_the_next_start() {
        let root = Root::new("crash");
        let crashed = RecoveryStore::open(&root.0).unwrap();
        crashed
            .save("/home/me/notes.txt", b"unsaved notes")
            .unwrap();
        crashed.save("untitled 2", b"draft").unwrap();
        crashed.discard("untitled 2").unwrap();
        // The process dies: its lock goes with it, and nothing is finished.
        drop(crashed);

        let running = RecoveryStore::open(&root.0).unwrap();
        running.save("mine", b"still being edited").unwrap();

        let found = orphans(&root.0).unwrap();
        assert_eq!(
            found.iter().map(Recovered::key).collect::<Vec<_>>(),
            ["/home/me/notes.txt"],
            "the running session's copy and the discarded one are not listed"
        );
        assert_eq!(found[0].read().unwrap(), b"unsaved notes");
        found.into_iter().next().unwrap().discard().unwrap();
        assert!(orphans(&root.0).unwrap().is_empty());
        running.finish().unwrap();
        assert_eq!(
            std::fs::read_dir(&root.0).unwrap().count(),
            0,
            "both sessions are gone"
        );
    }

    #[test]
    fn a_finished_session_leaves_nothing() {
        let root = Root::new("finish");
        let store = RecoveryStore::open(&root.0).unwrap();
        store.save("doc", b"x").unwrap();
        store.finish().unwrap();
        assert!(orphans(&root.0).unwrap().is_empty());
    }

    #[test]
    fn keys_round_trip_through_file_names() {
        for key in ["a/b c", "ünï", "", "x.copy"] {
            assert_eq!(decode_key(&encode_key(key)).as_deref(), Some(key));
        }
        assert_eq!(decode_key("zz"), None);
    }
}
