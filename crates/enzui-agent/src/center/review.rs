//! User review before an external effect (`ui.user_review`).
//!
//! A function that leads to an external effect (sending mail, publishing,
//! purchasing) does not perform it. It prepares the content, shows it in the
//! application's real window and returns a [`ReviewId`] with
//! [`ReviewState::AwaitingUser`]. Only the person, through the app's own
//! control, commits it. The agent observes the outcome with a separate status
//! read; no call waits for a person.
//!
//! [`ReviewBook`] is the ledger behind that contract:
//!
//! - every review belongs to one scope (the source account), and a status
//!   read from another scope finds nothing;
//! - every review is bound to the exact revision of the content it showed.
//!   Content the agent replaces supersedes the earlier review instead of
//!   inheriting its approval;
//! - [`ReviewBook::commit`] succeeds once, for the revision that was on
//!   screen, and is the only path to [`ReviewState::Committed`]. A second
//!   commit, a commit of a superseded revision, or a commit after cancellation
//!   is refused, so a replayed or doubled action cannot send twice;
//! - a repeated call with the same idempotency key is answered by
//!   [`super::CenterHost`] from its replay store, so it does not reach the app
//!   to prepare a second draft.
//!
//! The ledger only records. Committing is the application's job, from the
//! handler of the control a person used; nothing an agent can call reaches it.
//! A review id is not a secret and authorizes nothing: it only names a review
//! for status.

use std::collections::VecDeque;

/// The capability a review-gated function declares in its descriptor.
///
/// The broker refuses external-side-effect functions that do not declare it,
/// unless the owner has explicitly allowed background effects.
pub const USER_REVIEW_CAPABILITY: &str = "ui.user_review";

/// Reviews one ledger retains, including finished ones kept for status.
pub const REVIEW_CAPACITY: usize = 64;

/// Names one review for status reads. Not a credential.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReviewId(String);

impl ReviewId {
    /// The id as the agent sees it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ReviewId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where one review stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewState {
    /// Shown to the person; nothing has happened yet.
    AwaitingUser,
    /// The person committed the revision that was shown.
    Committed,
    /// The person discarded it, or the application closed it unsent.
    Discarded,
    /// The agent prepared different content for the same draft; this review's
    /// content will never be committed.
    Superseded,
}

impl ReviewState {
    /// The state as reported to an agent.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AwaitingUser => "awaiting_user",
            Self::Committed => "committed",
            Self::Discarded => "discarded",
            Self::Superseded => "superseded",
        }
    }

    /// Whether the person may still commit it.
    #[must_use]
    pub fn is_open(self) -> bool {
        self == Self::AwaitingUser
    }
}

/// Why a commit was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitError {
    /// No such review in this scope.
    Unknown,
    /// The review is no longer awaiting the person (already committed,
    /// discarded or superseded).
    NotOpen(ReviewState),
    /// The content on screen is not the revision the review was opened for.
    RevisionChanged,
}

#[derive(Debug)]
struct Entry<S> {
    id: ReviewId,
    scope: S,
    subject: String,
    revision: u64,
    state: ReviewState,
}

/// The ledger of reviews one application has opened.
///
/// `S` is the scope a review belongs to, typically the source account id.
#[derive(Debug)]
pub struct ReviewBook<S> {
    entries: VecDeque<Entry<S>>,
    next: u64,
    salt: u64,
}

impl<S: PartialEq + Clone> Default for ReviewBook<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: PartialEq + Clone> ReviewBook<S> {
    /// An empty ledger. Ids are unique per process and unpredictable across
    /// processes, so a review id from an earlier run never names a new review.
    #[must_use]
    pub fn new() -> Self {
        let salt = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos() as u64)
            ^ u64::from(std::process::id()).rotate_left(32);
        Self {
            entries: VecDeque::new(),
            next: 0,
            salt,
        }
    }

    /// Opens a review of `subject` at `revision` in `scope`.
    ///
    /// `subject` names what is being reviewed inside the app (a draft id); a
    /// newer review of the same subject supersedes every open earlier one.
    ///
    /// When full, the oldest finished review is forgotten; if every retained
    /// review is still open, the oldest open one is discarded first, so the
    /// ledger stays bounded.
    pub fn open(&mut self, scope: S, subject: impl Into<String>, revision: u64) -> ReviewId {
        let subject = subject.into();
        for entry in &mut self.entries {
            if entry.scope == scope && entry.subject == subject && entry.state.is_open() {
                entry.state = ReviewState::Superseded;
            }
        }
        while self.entries.len() >= REVIEW_CAPACITY {
            let victim = self
                .entries
                .iter()
                .position(|entry| !entry.state.is_open())
                .unwrap_or(0);
            self.entries.remove(victim);
        }
        self.next = self.next.wrapping_add(1);
        let id = ReviewId(format!(
            "review-{:016x}",
            mix(self.salt ^ self.next.wrapping_mul(0x9e37_79b9_7f4a_7c15))
        ));
        self.entries.push_back(Entry {
            id: id.clone(),
            scope,
            subject,
            revision,
            state: ReviewState::AwaitingUser,
        });
        id
    }

    /// The state of `id`, when it belongs to `scope`.
    #[must_use]
    pub fn status(&self, scope: &S, id: &str) -> Option<ReviewState> {
        self.find(scope, id).map(|entry| entry.state)
    }

    /// The open review of `subject`, if one is awaiting the person.
    #[must_use]
    pub fn open_for(&self, scope: &S, subject: &str) -> Option<(ReviewId, u64)> {
        self.entries
            .iter()
            .find(|entry| {
                entry.scope == *scope && entry.subject == subject && entry.state.is_open()
            })
            .map(|entry| (entry.id.clone(), entry.revision))
    }

    /// Records that the person committed `subject` while `revision` was on
    /// screen. Call only from the handler of the person's own control.
    ///
    /// # Errors
    /// No open review of that subject, or a different revision on screen. The
    /// application must then not perform the effect on the review's behalf.
    pub fn commit(
        &mut self,
        scope: &S,
        subject: &str,
        revision: u64,
    ) -> Result<ReviewId, CommitError> {
        let entry = self
            .entries
            .iter_mut()
            .rev()
            .find(|entry| entry.scope == *scope && entry.subject == subject)
            .ok_or(CommitError::Unknown)?;
        if !entry.state.is_open() {
            return Err(CommitError::NotOpen(entry.state));
        }
        if entry.revision != revision {
            return Err(CommitError::RevisionChanged);
        }
        entry.state = ReviewState::Committed;
        Ok(entry.id.clone())
    }

    /// Records that the person (or the app on their behalf: closing, discarding
    /// the draft, a cancelled call) ended the open review of `subject` unsent.
    /// Returns the review that ended, if one was open.
    pub fn discard(&mut self, scope: &S, subject: &str) -> Option<ReviewId> {
        let entry = self.entries.iter_mut().find(|entry| {
            entry.scope == *scope && entry.subject == subject && entry.state.is_open()
        })?;
        entry.state = ReviewState::Discarded;
        Some(entry.id.clone())
    }

    /// Records that the open review of `subject` will never be committed
    /// because the agent replaced its content. Returns the review that ended,
    /// if one was open. [`Self::open`] does the same for a review of the same
    /// scope and subject; this is for content that moves to a new subject or
    /// account.
    pub fn supersede(&mut self, scope: &S, subject: &str) -> Option<ReviewId> {
        let entry = self.entries.iter_mut().find(|entry| {
            entry.scope == *scope && entry.subject == subject && entry.state.is_open()
        })?;
        entry.state = ReviewState::Superseded;
        Some(entry.id.clone())
    }

    /// Whether any review is still awaiting a person, which keeps a background
    /// process from exiting under an open window.
    #[must_use]
    pub fn has_open(&self) -> bool {
        self.entries.iter().any(|entry| entry.state.is_open())
    }

    fn find(&self, scope: &S, id: &str) -> Option<&Entry<S>> {
        self.entries
            .iter()
            .find(|entry| entry.scope == *scope && entry.id.0 == id)
    }
}

// SplitMix64 finalizer: a bijection, so distinct counters give distinct ids.
fn mix(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> ReviewBook<&'static str> {
        ReviewBook::new()
    }

    #[test]
    fn a_review_awaits_the_person_and_commits_once_for_the_shown_revision() {
        let mut book = book();
        let id = book.open("work", "draft-1", 7);
        assert_eq!(
            book.status(&"work", id.as_str()),
            Some(ReviewState::AwaitingUser)
        );

        assert_eq!(
            book.commit(&"work", "draft-1", 6),
            Err(CommitError::RevisionChanged)
        );
        assert_eq!(
            book.status(&"work", id.as_str()),
            Some(ReviewState::AwaitingUser)
        );

        assert_eq!(book.commit(&"work", "draft-1", 7), Ok(id.clone()));
        assert_eq!(
            book.status(&"work", id.as_str()),
            Some(ReviewState::Committed)
        );
        assert_eq!(
            book.commit(&"work", "draft-1", 7),
            Err(CommitError::NotOpen(ReviewState::Committed)),
            "a doubled commit must not send twice"
        );
        assert!(!book.has_open());
    }
}
