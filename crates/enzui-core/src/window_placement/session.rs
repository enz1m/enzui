//! The compositor's window session, kept between launches.
//!
//! Wayland apps cannot place their windows; a compositor that offers
//! `xdg_session_manager_v1` places them itself, as they were last time, when
//! the app names the same session and window again. [`WindowSession`] stores
//! the session id beside the placement state, and
//! [`WindowSession::toplevel`] names a window in it for its
//! `window::Settings` (`platform_specific.session`). After a window opens,
//! read `iced::window::wayland_session_id` and [`WindowSession::record`] it.

use std::io;
use std::path::{Path, PathBuf};

use super::PlacementKey;
use crate::atomic_state::{self, StateLoad};

const FORMAT_ID: &str = "window-session";
const FILE_VERSION: u32 = 1;
// A session id is an opaque compositor string; anything longer is refused.
const MAX_ID_BYTES: usize = 1024;

/// The file the session id is stored under, beside `placement.state`.
pub const SESSION_FILE_NAME: &str = "window-session.state";

/// The compositor's window session id, if one was recorded.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowSession {
    id: Option<String>,
    dirty: bool,
}

impl WindowSession {
    /// The session stored at `path`; none when the file is absent, foreign
    /// or malformed.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        let id = match atomic_state::load_bounded(path, FORMAT_ID, FILE_VERSION, 4096).0 {
            StateLoad::Loaded(body) => body
                .lines()
                .find_map(|line| line.strip_prefix("id="))
                .filter(|id| valid_id(id))
                .map(str::to_owned),
            StateLoad::UsedDefaults { .. } => None,
        };
        Self { id, dirty: false }
    }

    /// The stored session id.
    #[must_use]
    pub fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// Records the id the compositor reported for this run; a new value marks
    /// the session for [`Self::save`]. Ids that cannot be stored are ignored.
    pub fn record(&mut self, id: Option<String>) {
        let id = id.filter(|id| valid_id(id));
        if id.is_some() && id != self.id {
            self.id = id;
            self.dirty = true;
        }
    }

    /// Whether [`Self::record`] changed the id since it was loaded or saved.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// The window `key` in this session, for its `window::Settings`
    /// (`platform_specific.session`).
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn toplevel(&self, key: &PlacementKey) -> iced_core::window::settings::SessionToplevel {
        iced_core::window::settings::SessionToplevel {
            session_id: self.id.clone(),
            toplevel: key.as_str().to_owned(),
        }
    }

    /// Writes the id to `path` atomically when it changed.
    ///
    /// # Errors
    /// An I/O error from the atomic write.
    pub fn save(&mut self, path: &Path) -> io::Result<()> {
        let Some(id) = self.id.as_deref().filter(|_| self.dirty) else {
            return Ok(());
        };
        atomic_state::save_atomic(path, FORMAT_ID, FILE_VERSION, &format!("id={id}\n"))?;
        self.dirty = false;
        Ok(())
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID_BYTES && !id.chars().any(char::is_control)
}

/// Resolves `$XDG_STATE_HOME/<app>/window-session.state`.
///
/// Returns `Ok(None)` when there is no usable absolute root.
///
/// # Errors
/// Returns [`crate::persist::XdgPathError`] unless `app` is one ordinary path component.
pub fn default_session_path(app: &str) -> Result<Option<PathBuf>, crate::persist::XdgPathError> {
    atomic_state::xdg_state_path(app, SESSION_FILE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recorded_session_survives_a_restart() {
        let directory = super::super::tests::TempDir::new("session");
        let path = directory.state_file().with_file_name(SESSION_FILE_NAME);
        let mut session = WindowSession::load(&path);
        assert_eq!(session.id(), None, "no file, no session");
        session.record(Some("kwin-7f3a".to_owned()));
        assert!(session.is_dirty());
        session.save(&path).unwrap();
        assert!(!session.is_dirty());

        let mut restarted = WindowSession::load(&path);
        assert_eq!(restarted.id(), Some("kwin-7f3a"));
        restarted.record(Some("kwin-7f3a".to_owned()));
        assert!(!restarted.is_dirty(), "the same id is no change");
        restarted.record(None);
        assert_eq!(
            restarted.id(),
            Some("kwin-7f3a"),
            "no report keeps the stored id"
        );
        restarted.record(Some("bad\nid".to_owned()));
        assert_eq!(
            restarted.id(),
            Some("kwin-7f3a"),
            "an unstorable id is ignored"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_window_joins_the_stored_session_under_its_key() {
        let mut session = WindowSession::default();
        session.record(Some("s-1".to_owned()));
        let toplevel = session.toplevel(&PlacementKey::new("viewer").unwrap());
        assert_eq!(toplevel.session_id.as_deref(), Some("s-1"));
        assert_eq!(toplevel.toplevel, "viewer");
    }
}
