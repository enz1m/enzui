//! Adapts download-queue state and controls to the shared operation-activity model.
//!
//! Controls round-trip through the queue reducer, and an unregistered or disabled Downloads provider contributes no rows.

use super::{
    Activity, ActivityCenter, ActivityControl, ActivityKey, ActivityKind, ActivityPhase,
    ActivityProgress, Generation, ProviderId, PublishOutcome,
};
use crate::download::{Download, DownloadAction, DownloadId, DownloadQueue, Phase};

/// The provider id the download queue publishes under.
pub const DOWNLOADS: ProviderId = ProviderId("downloads");

/// The activity phase a download phase maps to.
#[must_use]
pub fn phase_of(phase: Phase) -> ActivityPhase {
    match phase {
        Phase::Queued => ActivityPhase::Queued,
        Phase::Active => ActivityPhase::Running,
        Phase::Paused => ActivityPhase::Paused,
        Phase::Complete => ActivityPhase::Completed,
        Phase::Failed => ActivityPhase::Failed,
        Phase::Cancelled => ActivityPhase::Cancelled,
    }
}

/// The activity control a download action maps to.
#[must_use]
pub fn control_of(action: DownloadAction) -> ActivityControl {
    match action {
        DownloadAction::Pause => ActivityControl::Pause,
        DownloadAction::Resume => ActivityControl::Resume,
        DownloadAction::Cancel => ActivityControl::Cancel,
        DownloadAction::Retry => ActivityControl::Retry,
    }
}

/// Maps an activity control to a download action, returning `None` for host-only controls.
#[must_use]
pub fn action_of(control: ActivityControl) -> Option<DownloadAction> {
    match control {
        ActivityControl::Pause => Some(DownloadAction::Pause),
        ActivityControl::Resume => Some(DownloadAction::Resume),
        ActivityControl::Cancel => Some(DownloadAction::Cancel),
        ActivityControl::Retry => Some(DownloadAction::Retry),
        ActivityControl::Resolve | ActivityControl::OpenPanel => None,
    }
}

/// Applies a row control through the download queue.
///
/// Returns `false` for another provider, an unsupported control, or an action refused by the download state.
pub fn apply(queue: &mut DownloadQueue, key: ActivityKey, control: ActivityControl) -> bool {
    if key.provider != DOWNLOADS {
        return false;
    }
    let Some(action) = action_of(control) else {
        return false;
    };
    queue.apply(DownloadId(key.op), action)
}

/// The activity key for a download id.
#[must_use]
pub const fn key_of(id: DownloadId) -> ActivityKey {
    ActivityKey {
        provider: DOWNLOADS,
        op: id.0,
    }
}

/// Converts a download into an activity row using the supplied display name.
#[must_use]
pub fn activity_for(download: &Download, name: &str) -> Activity {
    let progress = ActivityProgress::Bytes {
        done: download.received,
        total: download.total(),
    };
    let mut controls: Vec<ActivityControl> = download
        .actions()
        .into_iter()
        .map(control_of)
        .collect::<Vec<_>>();
    controls.push(ActivityControl::OpenPanel);
    Activity::new(key_of(download.id), ActivityKind::Download, name)
        .generation(Generation(download.attempt.0))
        .progress(progress)
        .phase(phase_of(download.phase))
        .controls(controls)
}

/// Reconciles the center with the complete download queue and returns the number of accepted rows.
///
/// Omitted downloads are dismissed and retired, so filtered views must use [`publish_one`]. Rows seen but refused as stale are not dismissed.
pub fn publish<'a>(
    center: &mut ActivityCenter,
    downloads: impl IntoIterator<Item = (&'a Download, &'a str)>,
    now_ms: u64,
) -> usize {
    let mut published = 0;
    let mut seen: Vec<ActivityKey> = Vec::new();
    for (download, name) in downloads {
        seen.push(key_of(download.id));
        if center
            .publish(activity_for(download, name), now_ms)
            .accepted()
        {
            published += 1;
        }
    }
    let forgotten: Vec<ActivityKey> = center
        .iter()
        .filter(|row| row.key.provider == DOWNLOADS && !seen.contains(&row.key))
        .map(|row| row.key)
        .collect();
    for key in forgotten {
        center.dismiss(key);
    }
    published
}

/// Publishes one download row without reconciling other download rows.
pub fn publish_one(
    center: &mut ActivityCenter,
    download: &Download,
    name: &str,
    now_ms: u64,
) -> PublishOutcome {
    center.publish(activity_for(download, name), now_ms)
}
