use crate::{Job, Outcome, Progress, Publisher, State, Status, Unit, Visibility};
use enzui_core::operation_activity::{
    Activity, ActivityCenter, ActivityKey, ActivityPhase, ActivityProgress, Generation,
};
use std::collections::{BTreeMap, BTreeSet};

/// Publishes selected in-app operations without replaying historical completions.
///
/// Call [`Self::sync`] after reducer turns, including the center's scheduled reveal
/// and expiry. No polling is added. Dropping this adapter removes unfinished jobs.
#[derive(Default)]
pub struct CenterJobs {
    jobs: BTreeMap<(ActivityKey, Generation), Job>,
}

impl CenterJobs {
    /// Reconciles live producer state with the shell's transient records.
    ///
    /// `describe` opts each row in with a localized generic label, or excludes it
    /// with `None`. Row labels and details are never copied: they can contain paths,
    /// names or errors. Measurements and lifecycle come from the center, including
    /// its reveal delay. Unknown totals stay indeterminate and no ETA is invented.
    /// A token must have been queued through the native window API successfully.
    ///
    /// Callers continue their work when metadata or service admission fails.
    pub fn sync(
        &mut self,
        publisher: &Publisher,
        center: &ActivityCenter,
        window_token: Option<&str>,
        visibility: Visibility,
        mut describe: impl FnMut(&Activity) -> Option<String>,
    ) {
        let mut retained = BTreeSet::new();
        for row in center.iter() {
            let identity = (row.key, row.generation);
            let Some(label) = describe(row) else {
                continue;
            };
            let terminal = match row.phase {
                ActivityPhase::Completed => Some(Outcome::Succeeded),
                ActivityPhase::Failed => Some(Outcome::Failed),
                ActivityPhase::Cancelled => Some(Outcome::Cancelled),
                _ => None,
            };
            if let Some(outcome) = terminal {
                if let Some(job) = self.jobs.remove(&identity) {
                    let _ = job.finish(outcome);
                }
                continue;
            }
            if !center.is_revealed(row.key) {
                continue;
            }
            let status = Status {
                state: match row.phase {
                    ActivityPhase::Queued | ActivityPhase::Paused | ActivityPhase::Waiting => {
                        State::Paused
                    }
                    _ => State::Running,
                },
                progress: progress(row.progress),
                window_token: window_token.map(str::to_owned),
                ..Status::running(label)
            };
            retained.insert(identity);
            if let Some(job) = self.jobs.get(&identity) {
                let _ = job.update(status);
                let _ = job.set_visibility(visibility);
            } else if let Ok(job) = publisher.start_with_visibility(status, visibility) {
                self.jobs.insert(identity, job);
            }
        }
        self.jobs.retain(|identity, _| retained.contains(identity));
    }
}

fn progress(measurement: ActivityProgress) -> Option<Progress> {
    let (done, total, unit) = match measurement {
        ActivityProgress::Bytes {
            done,
            total: Some(total),
        } => (done, total, Unit::Bytes),
        ActivityProgress::Items {
            done,
            total: Some(total),
        } => (done, total, Unit::Items),
        ActivityProgress::Fraction(fraction) if fraction.is_finite() => (
            (f64::from(fraction.clamp(0.0, 1.0)) * 1_000_000.0).round() as u64,
            1_000_000,
            Unit::Steps,
        ),
        _ => return None,
    };
    (total > 0).then_some(Progress {
        done: done.min(total),
        total,
        unit,
    })
}
