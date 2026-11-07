//! Graceful stop for an application the OS runs without its window
//! (`application-lifecycle-v1`).
//!
//! An OS background-apps service asks before it stops an app. On the app's
//! own bus name and object path, [`LIFECYCLE_INTERFACE`] answers:
//!
//! - `QueryStop() -> (s revision, a(sss) consequences)`: what stopping now
//!   would interrupt, as the app last published it with
//!   [`StopControl::publish`]. The revision changes whenever the work in that
//!   list does: which pieces of work (their opaque [`StopConsequence::key`]s),
//!   their kinds and their titles, never their progress.
//! - `Stop(s choice, s reviewed_revision) -> s outcome`: `choice` is
//!   [`StopChoice::FINISH_THEN_EXIT`] or [`StopChoice::CANCEL_AND_EXIT`];
//!   there is no force. The outcome is `accepted`, `stale-revision` (the
//!   consequences changed since they were reviewed) or `invalid-choice`. A
//!   list whose work keys are not valid (see [`StopConsequence::key`]) is
//!   never stoppable: `Stop` answers it with an error.
//!
//! - `Work() -> (b known, u active, as kinds, t revision)`: how much work
//!   the app is doing, for a background-apps list. Counts and a fixed
//!   vocabulary of [`WorkKind`]s only: no titles, paths or other content. An
//!   app that never called [`StopControl::publish_work`] answers
//!   `known = false`, never a count of zero. `revision` rises on every
//!   change, so a reader can tell a fresh answer from one it has seen.
//!
//! Only a service the installed OS declares is answered ([`admission`] says
//! how it is recognised, [`CALLERS_DIRECTORY`] where it is declared); anyone
//! else gets `org.freedesktop.DBus.Error.AccessDenied`.
//!
//! An accepted stop reaches the app through [`StopControl::take_stop`] on its
//! existing application wake, carrying the keys of the work that was
//! reviewed. The app checks [`StopControl::is_current`] and compares its live
//! work with those keys before it cancels or exits, so work that started
//! after the review refuses the stop instead of being lost.

pub mod admission;
mod callers;
mod witness;

pub use admission::{CallerPolicy, LifecycleCallers, WitnessCall};
pub use callers::CALLERS_DIRECTORY;

use crate::portal::CompletionSignal;
use std::sync::{Arc, Mutex, MutexGuard};

/// The D-Bus interface served beside `org.freedesktop.Application`.
pub const LIFECYCLE_INTERFACE: &str = "org.enzui.ApplicationLifecycle1";

/// The most consequences one query returns; a longer list ends with a
/// "more" entry counting the rest.
pub const MAX_STOP_CONSEQUENCES: usize = 32;

/// The longest consequence field, in characters.
pub const MAX_CONSEQUENCE_TEXT: usize = 160;

/// The longest work key, in bytes. A longer key makes the list unstoppable
/// rather than being shortened, so two keys never collapse into one.
pub const MAX_WORK_KEY_BYTES: usize = 160;

/// What kind of work an app is doing, in the fixed vocabulary
/// [`StopControl::publish_work`] reports and the OS labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum WorkKind {
    /// Copying or moving files.
    Copy,
    /// Synchronising with a service.
    Sync,
    /// Downloading.
    Download,
    /// Uploading.
    Upload,
    /// Exporting or converting a document or media.
    Export,
    /// Printing.
    Print,
    /// Any other work.
    Other,
}

impl WorkKind {
    /// The wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Copy => "copy",
            Self::Sync => "sync",
            Self::Download => "download",
            Self::Upload => "upload",
            Self::Export => "export",
            Self::Print => "print",
            Self::Other => "other",
        }
    }
}

/// The most kinds one `Work` answer lists. The vocabulary is smaller, so
/// this bounds the wire, not the app.
pub const MAX_WORK_KINDS: usize = 8;

/// How much work the app last published.
#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkState {
    active: u32,
    /// Distinct, in [`WorkKind`] order.
    kinds: Vec<WorkKind>,
}

/// One thing stopping now would interrupt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopConsequence {
    /// The work's opaque identity: stable for its lifetime, never reused,
    /// unique in the list, non-empty, free of control characters and at
    /// most [`MAX_WORK_KEY_BYTES`]. Two transfers with the same title are
    /// still two pieces of work. It is compared exactly, never shortened or
    /// cleaned, and never sent on the wire; it decides the revision and what
    /// was reviewed. A list with any key that breaks these rules cannot be
    /// stopped.
    pub key: String,
    /// A stable category: `transfer`, `download`, `unsaved`…
    pub kind: String,
    /// What it is, for the person: "Copying 3 files".
    pub title: String,
    /// Its state: "2 of 3 done". May be empty.
    pub detail: String,
}

impl StopConsequence {
    /// A consequence of the work `key` (see [`Self::key`]), with no detail.
    pub fn new(key: impl Into<String>, kind: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            kind: kind.into(),
            title: title.into(),
            detail: String::new(),
        }
    }

    /// Adds the detail line.
    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }
}

/// How the person chose to stop the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopChoice {
    /// Let the listed work finish, then exit.
    FinishThenExit,
    /// Cancel the listed work, then exit.
    CancelAndExit,
}

impl StopChoice {
    /// The wire name of [`Self::FinishThenExit`].
    pub const FINISH_THEN_EXIT: &str = "finish-then-exit";
    /// The wire name of [`Self::CancelAndExit`].
    pub const CANCEL_AND_EXIT: &str = "cancel-and-exit";

    fn parse(choice: &str) -> Option<Self> {
        match choice {
            Self::FINISH_THEN_EXIT => Some(Self::FinishThenExit),
            Self::CANCEL_AND_EXIT => Some(Self::CancelAndExit),
            _ => None,
        }
    }
}

/// A stop the OS asked for, against the revision it reviewed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopRequest {
    /// What the person chose.
    pub choice: StopChoice,
    /// The revision the choice was made against.
    pub revision: String,
    /// The keys of every piece of work that revision listed, including any
    /// the wire folded into a "more" entry.
    pub reviewed: Vec<String>,
}

/// What one piece of work contributes to the revision.
type Member = (String, String, String);

#[derive(Debug, Default)]
struct StopState {
    revision: u64,
    /// The whole published list's identities, before the wire bound.
    members: Vec<Member>,
    /// Whether every key in `members` is a valid work key.
    stoppable: bool,
    consequences: Vec<StopConsequence>,
    pending: Option<StopRequest>,
    /// `None` until the app publishes work.
    work: Option<WorkState>,
    work_revision: u64,
}

fn lock(state: &Mutex<StopState>) -> MutexGuard<'_, StopState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// [`StopControl::request_stop`]'s answer for a list whose work keys are
/// not valid. Over D-Bus it is an error reply, not an outcome string.
pub const INVALID_WORK: &str = "invalid-work";

/// Every key non-empty, free of control characters, at most
/// [`MAX_WORK_KEY_BYTES`] and unique.
fn valid_work_keys(members: &[Member]) -> bool {
    let mut seen = std::collections::HashSet::with_capacity(members.len());
    members.iter().all(|(key, _, _)| {
        !key.is_empty()
            && key.len() <= MAX_WORK_KEY_BYTES
            && !key.chars().any(char::is_control)
            && seen.insert(key.as_str())
    })
}

fn bounded(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(MAX_CONSEQUENCE_TEXT)
        .collect()
}

fn bound_list(consequences: Vec<StopConsequence>) -> Vec<StopConsequence> {
    let mut list: Vec<StopConsequence> = consequences
        .iter()
        .map(|item| StopConsequence {
            key: bounded(&item.key),
            kind: bounded(&item.kind),
            title: bounded(&item.title),
            detail: bounded(&item.detail),
        })
        .collect();
    if list.len() > MAX_STOP_CONSEQUENCES {
        let rest = list.len() - (MAX_STOP_CONSEQUENCES - 1);
        list.truncate(MAX_STOP_CONSEQUENCES - 1);
        list.push(StopConsequence::new(
            "",
            "more",
            crate::localized!("enzui-lifecycle-more", "count" => rest),
        ));
    }
    list
}

/// The app's side of graceful stop: publish what stopping would interrupt,
/// and take the stops the OS asks for.
#[derive(Debug, Clone)]
pub struct StopControl {
    state: Arc<Mutex<StopState>>,
    wake: CompletionSignal,
}

impl StopControl {
    pub(crate) fn new(wake: CompletionSignal) -> Self {
        Self {
            state: Arc::new(Mutex::new(StopState {
                revision: 1,
                stoppable: true,
                ..StopState::default()
            })),
            wake,
        }
    }

    /// A control with no bus behind it, for tests and harnesses that play
    /// the OS through [`Self::request_stop`].
    #[must_use]
    pub fn detached() -> Self {
        Self::new(CompletionSignal::new())
    }

    /// Publishes what stopping now would interrupt. The revision advances
    /// only when what would be interrupted changes: a piece of work arrives,
    /// leaves or is replaced (its key), or its kind or title changes. A
    /// detail such as progress updates in place, so a running transfer does
    /// not make every reviewed stop stale. Every item counts, including those
    /// past [`MAX_STOP_CONSEQUENCES`] that the wire folds away.
    pub fn publish(&self, consequences: Vec<StopConsequence>) {
        let members: Vec<Member> = consequences
            .iter()
            .map(|item| (item.key.clone(), bounded(&item.kind), bounded(&item.title)))
            .collect();
        let stoppable = valid_work_keys(&members);
        let list = bound_list(consequences);
        let mut state = lock(&self.state);
        if state.members != members {
            state.revision += 1;
        }
        state.members = members;
        state.stoppable = stoppable;
        state.consequences = list;
    }

    /// Publishes the app's live work, one kind per piece of work: a copy of
    /// three files that is also uploading one is `[Copy, Upload]`. Feed it
    /// from the work itself (the activity center, the same rows the activity
    /// publisher shows), never from a timer. Once called, the app's `Work`
    /// answer is known; an empty list means no work. The work revision rises
    /// whenever the count or the kinds change.
    pub fn publish_work(&self, work: impl IntoIterator<Item = WorkKind>) {
        let mut kinds: Vec<WorkKind> = Vec::new();
        let mut active: u32 = 0;
        for kind in work {
            active = active.saturating_add(1);
            kinds.push(kind);
        }
        kinds.sort_unstable();
        kinds.dedup();
        kinds.truncate(MAX_WORK_KINDS);
        let next = Some(WorkState { active, kinds });
        let mut state = lock(&self.state);
        if state.work != next {
            state.work = next;
            state.work_revision += 1;
        }
    }

    /// What `Work` answers: whether the app publishes work, how many pieces
    /// are live, their distinct kinds and the work revision.
    #[must_use]
    pub fn work(&self) -> (bool, u32, Vec<String>, u64) {
        let state = lock(&self.state);
        match &state.work {
            None => (false, 0, Vec::new(), state.work_revision),
            Some(work) => (
                true,
                work.active,
                work.kinds
                    .iter()
                    .map(|kind| kind.as_str().to_owned())
                    .collect(),
                state.work_revision,
            ),
        }
    }

    /// The revision of the last published list.
    #[must_use]
    pub fn revision(&self) -> String {
        lock(&self.state).revision.to_string()
    }

    /// The stop the OS asked for, once.
    pub fn take_stop(&self) -> Option<StopRequest> {
        lock(&self.state).pending.take()
    }

    /// Whether `request` was made against what is published now. Check this
    /// before cancelling anything or exiting.
    #[must_use]
    pub fn is_current(&self, request: &StopRequest) -> bool {
        lock(&self.state).revision.to_string() == request.revision
    }

    fn query(&self) -> (String, Vec<(String, String, String)>) {
        let state = lock(&self.state);
        (
            state.revision.to_string(),
            state
                .consequences
                .iter()
                .map(|item| (item.kind.clone(), item.title.clone(), item.detail.clone()))
                .collect(),
        )
    }

    /// What `Stop` answers once the caller is verified: `accepted`,
    /// `stale-revision`, `invalid-choice`, or [`INVALID_WORK`] for a list
    /// whose work keys break the [`StopConsequence::key`] rules. An accepted
    /// stop is queued for [`Self::take_stop`] and wakes the app.
    pub fn request_stop(&self, choice: &str, reviewed_revision: &str) -> &'static str {
        let Some(choice) = StopChoice::parse(choice) else {
            return "invalid-choice";
        };
        {
            let mut state = lock(&self.state);
            if !state.stoppable {
                return INVALID_WORK;
            }
            if state.revision.to_string() != reviewed_revision {
                return "stale-revision";
            }
            let reviewed = state
                .members
                .iter()
                .map(|(key, _, _)| key.clone())
                .collect();
            state.pending = Some(StopRequest {
                choice,
                revision: reviewed_revision.to_owned(),
                reviewed,
            });
        }
        self.wake.notify();
        "accepted"
    }
}

/// The live rows of an activity center as stop consequences: each file
/// operation or transfer still queued, running, paused or waiting, with its
/// progress as the detail.
#[must_use]
pub fn activity_consequences(
    center: &crate::operation_activity::ActivityCenter,
) -> Vec<StopConsequence> {
    use crate::operation_activity::{ActivityKind, ActivityPhase, ActivityProgress};
    center
        .iter()
        .filter(|row| row.is_active())
        .map(|row| {
            let kind = match row.kind {
                ActivityKind::Copy => "copy",
                ActivityKind::Move => "move",
                ActivityKind::Delete => "delete",
                ActivityKind::Trash => "trash",
                ActivityKind::Rename => "rename",
                ActivityKind::Download => "download",
                ActivityKind::Upload => "upload",
                ActivityKind::Unmount => "unmount",
                ActivityKind::Other(_) => "work",
            };
            let progress = match row.progress {
                ActivityProgress::Items {
                    done,
                    total: Some(total),
                } => crate::localized!("enzui-lifecycle-item-progress", "done" => done, "total" => total),
                ActivityProgress::Items { done, total: None } => crate::localized!("enzui-lifecycle-items-done", "done" => done),
                progress => progress
                    .fraction()
                    .map(|fraction| format!("{}%", (fraction * 100.0).round() as u32))
                    .unwrap_or_default(),
            };
            let phase = crate::localization::text(match row.phase {
                ActivityPhase::Queued => "enzui-phase-queued",
                ActivityPhase::Running => "enzui-phase-running",
                ActivityPhase::Paused => "enzui-phase-paused",
                ActivityPhase::Waiting => "enzui-phase-waiting",
                ActivityPhase::Completed => "enzui-phase-completed",
                ActivityPhase::Failed => "enzui-phase-failed",
                ActivityPhase::Cancelled => "enzui-phase-cancelled",
            });
            let detail = match row.phase {
                ActivityPhase::Running => progress,
                _ if progress.is_empty() => phase,
                _ => crate::localized!("enzui-lifecycle-detail", "phase" => phase, "progress" => progress),
            };
            let key = format!("{}:{}", row.key.provider.0, row.key.op);
            StopConsequence::new(key, kind, row.label.clone()).detail(detail)
        })
        .collect()
}

/// The live rows of an activity center as work for
/// [`StopControl::publish_work`]: each queued, running, paused or waiting
/// row, by kind. Moves count as copies; deletes, renames and the like as
/// [`WorkKind::Other`].
#[must_use]
pub fn activity_work(center: &crate::operation_activity::ActivityCenter) -> Vec<WorkKind> {
    use crate::operation_activity::ActivityKind;
    center
        .iter()
        .filter(|row| row.is_active())
        .map(|row| match row.kind {
            ActivityKind::Copy | ActivityKind::Move => WorkKind::Copy,
            ActivityKind::Download => WorkKind::Download,
            ActivityKind::Upload => WorkKind::Upload,
            _ => WorkKind::Other,
        })
        .collect()
}

/// The served object: [`StopControl`] behind a caller check.
pub(crate) struct LifecycleObject {
    pub(crate) control: StopControl,
    pub(crate) admission: admission::Admission,
}

impl LifecycleObject {
    async fn authorize(
        &self,
        connection: &zbus::Connection,
        header: &zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<()> {
        // The message names the check that failed, so the OS service's log
        // says why it was refused; it carries none of the values compared.
        let denied = |why: admission::Refusal| {
            zbus::fdo::Error::AccessDenied(format!("not an admitted lifecycle caller: {why}"))
        };
        let sender = header
            .sender()
            .ok_or_else(|| denied(admission::Refusal::UnknownCaller))?;
        admission::admit(&self.admission, connection, sender)
            .await
            .map_err(denied)
    }
}

#[zbus::interface(name = "org.enzui.ApplicationLifecycle1", spawn = false)]
impl LifecycleObject {
    #[zbus(name = "QueryStop")]
    async fn query_stop(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<(String, Vec<(String, String, String)>)> {
        self.authorize(connection, &header).await?;
        Ok(self.control.query())
    }

    #[zbus(name = "Work")]
    async fn work(
        &self,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<(bool, u32, Vec<String>, u64)> {
        self.authorize(connection, &header).await?;
        Ok(self.control.work())
    }

    #[zbus(name = "Stop")]
    async fn stop(
        &self,
        choice: String,
        reviewed_revision: String,
        #[zbus(connection)] connection: &zbus::Connection,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<String> {
        self.authorize(connection, &header).await?;
        match self.control.request_stop(&choice, &reviewed_revision) {
            INVALID_WORK => Err(zbus::fdo::Error::Failed(
                "the app's work cannot be identified, so it cannot be stopped gracefully".into(),
            )),
            outcome => Ok(outcome.to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn control() -> StopControl {
        StopControl::new(CompletionSignal::new())
    }

    #[test]
    fn the_revision_follows_the_work_not_its_title() {
        let job = |key: &str| StopConsequence::new(key, "copy", "Copying 1 item");
        let stop = control();
        stop.publish(vec![job("files:1"), job("files:2")]);
        let both = stop.revision();

        stop.publish(vec![job("files:1"), job("files:3")]);
        let replaced = stop.revision();
        assert_ne!(replaced, both, "a same-title replacement is new work");

        stop.publish(vec![job("files:1"), job("files:3"), job("files:4")]);
        let added = stop.revision();
        assert_ne!(added, replaced, "added work");

        stop.publish(vec![job("files:1"), job("files:4")]);
        assert_ne!(stop.revision(), added, "removed work");

        let before = stop.revision();
        stop.publish(vec![
            job("files:1").detail("50%"),
            job("files:4").detail("1%"),
        ]);
        assert_eq!(stop.revision(), before, "progress alone keeps the revision");
    }
}
