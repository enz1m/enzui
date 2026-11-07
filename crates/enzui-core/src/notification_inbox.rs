//! The in-app list of past notifications: what toasts said after they went
//! away, read or unread (a notification centre inside the app).
//!
//! [`Inbox`] keeps the newest entries up to a limit and counts the unread
//! ones for a badge; `enzui_widgets::notification_inbox` draws it.

use std::time::SystemTime;

/// One notification kept in the inbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The app's id for it, to act on or dismiss.
    pub id: u64,
    /// The headline.
    pub title: String,
    /// More detail, if any.
    pub body: Option<String>,
    /// When it arrived.
    pub at: SystemTime,
    /// Whether the user has seen it in the inbox.
    pub read: bool,
}

/// Past notifications, newest first. See the [module](self) documentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbox {
    entries: Vec<Entry>,
    limit: usize,
    next_id: u64,
}

impl Default for Inbox {
    fn default() -> Self {
        Self::new(100)
    }
}

impl Inbox {
    /// An empty inbox keeping at most `limit` entries (at least one).
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self {
            entries: Vec::new(),
            limit: limit.max(1),
            next_id: 1,
        }
    }

    /// Adds an unread notification at `at`; returns its id. The oldest
    /// entries fall off past the limit.
    pub fn push(&mut self, title: impl Into<String>, body: Option<String>, at: SystemTime) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.entries.insert(
            0,
            Entry {
                id,
                title: title.into(),
                body,
                at,
                read: false,
            },
        );
        self.entries.truncate(self.limit);
        id
    }

    /// The entries, newest first.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// How many are unread, for a badge.
    #[must_use]
    pub fn unread(&self) -> usize {
        self.entries.iter().filter(|entry| !entry.read).count()
    }

    /// Marks one entry read.
    pub fn mark_read(&mut self, id: u64) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            entry.read = true;
        }
    }

    /// Marks every entry read, as opening the inbox does.
    pub fn mark_all_read(&mut self) {
        self.entries.iter_mut().for_each(|entry| entry.read = true);
    }

    /// Removes one entry.
    pub fn dismiss(&mut self, id: u64) {
        self.entries.retain(|entry| entry.id != id);
    }

    /// Removes every entry.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
