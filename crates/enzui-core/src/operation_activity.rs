//! Provider-neutral operation activity, aggregation, retirement, paint throttling, and expiry scheduling.
//!
//! Generation checks reject stale updates, terminal rows retire after a caller-driven linger, and mixed or indeterminate units never produce a fabricated aggregate.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Coalesces producer updates before UI messages are emitted.
///
/// [`push`](Self::push) admits at most one ordinary update per interval, while [`flush`](Self::flush) emits the newest pending value before a terminal event.
#[derive(Debug)]
pub struct ActivityPaintThrottle<T> {
    interval: Duration,
    last_emitted: Option<Instant>,
    pending: Option<T>,
}

impl<T> Default for ActivityPaintThrottle<T> {
    fn default() -> Self {
        Self::new(Self::DEFAULT_INTERVAL)
    }
}

impl<T> ActivityPaintThrottle<T> {
    /// Uses a 100 ms default interval; one terminal flush may bypass it.
    pub const DEFAULT_INTERVAL: Duration = Duration::from_millis(100);

    /// Creates a throttle; a zero interval disables throttling.
    #[must_use]
    pub const fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_emitted: None,
            pending: None,
        }
    }

    /// Offers a descriptor and returns it only when an emission is due, retaining only the newest suppressed value.
    ///
    /// All `now` values must use the same monotonic clock.
    pub fn push(&mut self, value: T, now: Instant) -> Option<T> {
        let due = self
            .last_emitted
            .is_none_or(|last| now.saturating_duration_since(last) >= self.interval);
        if due {
            self.record_emission(now);
            self.pending = None;
            Some(value)
        } else {
            self.pending = Some(value);
            None
        }
    }

    /// Emits the newest pending descriptor at most once before a terminal event.
    ///
    /// An out-of-order `now` does not move the next deadline backward.
    pub fn flush(&mut self, now: Instant) -> Option<T> {
        let pending = self.pending.take()?;
        self.record_emission(now);
        Some(pending)
    }

    /// The next monotonic instant at which `push` may emit, or `None` before
    /// the first emission (or when the deadline exceeds [`Instant`]'s range).
    #[must_use]
    pub fn next_emit_at(&self) -> Option<Instant> {
        self.last_emitted
            .and_then(|last| last.checked_add(self.interval))
    }

    fn record_emission(&mut self, now: Instant) {
        self.last_emitted = Some(self.last_emitted.map_or(now, |last| last.max(now)));
    }

    /// Whether a newer descriptor is currently coalesced.
    #[must_use]
    pub const fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
}

/// Identifies a compiled-in provider namespace for activity keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProviderId(pub &'static str);

/// Identifies an operation across attempts by provider and provider-local number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ActivityKey {
    /// Which provider owns this operation.
    pub provider: ProviderId,
    /// The provider's own stable operation number.
    pub op: u64,
}

impl ActivityKey {
    /// A key for `provider`'s operation number `op`.
    #[must_use]
    pub const fn new(provider: &'static str, op: u64) -> Self {
        Self {
            provider: ProviderId(provider),
            op,
        }
    }
}

/// Identifies one attempt; an older generation cannot overwrite a newer one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Generation(pub u32);

impl Generation {
    /// The first attempt.
    pub const FIRST: Generation = Generation(0);

    /// The next attempt, used when retrying.
    #[must_use]
    pub const fn next(self) -> Generation {
        Generation(self.0 + 1)
    }
}

/// Classifies the work represented by an activity row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    /// Copying files.
    Copy,
    /// Moving files within or across volumes.
    Move,
    /// Permanently deleting.
    Delete,
    /// Moving to trash.
    Trash,
    /// Renaming, including bulk rename.
    Rename,
    /// Downloading from the network.
    Download,
    /// Uploading to the network.
    Upload,
    /// Flushing and unmounting a volume.
    Unmount,
    /// Anything else, named by the provider.
    Other(&'static str),
}

impl ActivityKind {
    /// A short present-tense word for this kind.
    #[must_use]
    pub fn word(self) -> String {
        match self {
            ActivityKind::Copy => crate::localization::text("enzui-operation-activity-copying"),
            ActivityKind::Move => crate::localization::text("enzui-operation-activity-moving"),
            ActivityKind::Delete => crate::localization::text("enzui-operation-activity-deleting"),
            ActivityKind::Trash => {
                crate::localization::text("enzui-operation-activity-moving-to-trash")
            }
            ActivityKind::Rename => crate::localization::text("enzui-operation-activity-renaming"),
            ActivityKind::Download => {
                crate::localization::text("enzui-operation-activity-downloading")
            }
            ActivityKind::Upload => crate::localization::text("enzui-operation-activity-uploading"),
            ActivityKind::Unmount => crate::localization::text("enzui-operation-activity-ejecting"),
            ActivityKind::Other(word) => word.to_owned(),
        }
    }
}

/// Represents progress as unknown, dimensionless, bytes, or items.
///
/// [`Activity::progress`], [`ActivityCenter::publish`], and [`ActivityCenter::progress`] normalize fractional values; direct enum construction does not.
#[derive(Debug, Clone, Copy)]
pub enum ActivityProgress {
    /// Running with no knowable completion measure.
    Indeterminate,
    /// Dimensionless progress; model ingress clamps finite values to `0.0..=1.0` and converts non-finite values to [`Self::Indeterminate`].
    Fraction(f32),
    /// Byte-denominated progress. `total` is `None` until the size is known.
    Bytes {
        /// Bytes transferred so far.
        done: u64,
        /// Total bytes, once known.
        total: Option<u64>,
    },
    /// Item-denominated progress (files, entries, renames).
    Items {
        /// Items finished so far.
        done: u64,
        /// Total items, once known.
        total: Option<u64>,
    },
}

// Treat all NaN payloads as equal so `Eq` remains reflexive before normalization.
impl PartialEq for ActivityProgress {
    fn eq(&self, other: &Self) -> bool {
        match (*self, *other) {
            (Self::Indeterminate, Self::Indeterminate) => true,
            (Self::Fraction(left), Self::Fraction(right)) => {
                left == right || (left.is_nan() && right.is_nan())
            }
            (
                Self::Bytes {
                    done: left_done,
                    total: left_total,
                },
                Self::Bytes {
                    done: right_done,
                    total: right_total,
                },
            )
            | (
                Self::Items {
                    done: left_done,
                    total: left_total,
                },
                Self::Items {
                    done: right_done,
                    total: right_total,
                },
            ) => left_done == right_done && left_total == right_total,
            _ => false,
        }
    }
}

impl Eq for ActivityProgress {}

impl ActivityProgress {
    /// Returns normalized completion, or `None` when completion is unknown.
    ///
    /// A known zero total is complete; finite values clamp to `0.0..=1.0`.
    #[must_use]
    pub fn fraction(self) -> Option<f32> {
        let (done, total) = match self {
            ActivityProgress::Indeterminate => return None,
            ActivityProgress::Fraction(value) => {
                return value.is_finite().then(|| value.clamp(0.0, 1.0));
            }
            ActivityProgress::Bytes { done, total } | ActivityProgress::Items { done, total } => {
                (done, total?)
            }
        };
        if total == 0 {
            return Some(1.0);
        }
        Some((done.min(total) as f32) / (total as f32))
    }

    /// Clamps finite fractions and converts non-finite fractions to [`Self::Indeterminate`].
    #[must_use]
    pub fn normalized(self) -> Self {
        match self {
            ActivityProgress::Fraction(value) if value.is_finite() => {
                ActivityProgress::Fraction(value.clamp(0.0, 1.0))
            }
            ActivityProgress::Fraction(_) => ActivityProgress::Indeterminate,
            other => other,
        }
    }

    /// Whether this value carries a determinate total or fraction.
    #[must_use]
    pub fn is_determinate(self) -> bool {
        self.fraction().is_some()
    }

    // Unit tag used to reject mixed-unit aggregation.
    #[must_use]
    fn unit(self) -> Unit {
        match self {
            ActivityProgress::Indeterminate => Unit::Unknown,
            ActivityProgress::Fraction(_) => Unit::Fraction,
            ActivityProgress::Bytes { .. } => Unit::Bytes,
            ActivityProgress::Items { .. } => Unit::Items,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Unknown,
    Fraction,
    Bytes,
    Items,
}

/// Lifecycle of one operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityPhase {
    /// Admitted, not yet doing work.
    Queued,
    /// Doing work now.
    Running,
    /// Suspended by the user; resumable under the same generation.
    Paused,
    /// Blocked on the user — a conflict prompt, a passphrase, a retry choice.
    /// Active, but making no progress, and the surface must say so.
    Waiting,
    /// Finished successfully.
    Completed,
    /// Failed. Terminal until a retry publishes a newer generation.
    Failed,
    /// Cancelled by the user.
    Cancelled,
}

impl ActivityPhase {
    /// Whether this phase ends the operation.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            ActivityPhase::Completed | ActivityPhase::Failed | ActivityPhase::Cancelled
        )
    }

    /// Whether this phase counts as live work for the surface's visibility.
    #[must_use]
    pub const fn is_active(self) -> bool {
        !self.is_terminal()
    }

    /// A short word for the phase, for the accessible description.
    #[must_use]
    pub fn word(self) -> String {
        match self {
            ActivityPhase::Queued => crate::localization::text("enzui-operation-activity-queued"),
            ActivityPhase::Running => crate::localization::text("enzui-operation-activity-running"),
            ActivityPhase::Paused => crate::localization::text("enzui-operation-activity-paused"),
            ActivityPhase::Waiting => crate::localization::text("enzui-operation-activity-waiting"),
            ActivityPhase::Completed => {
                crate::localization::text("enzui-operation-activity-completed")
            }
            ActivityPhase::Failed => crate::localization::text("enzui-operation-activity-failed"),
            ActivityPhase::Cancelled => {
                crate::localization::text("enzui-operation-activity-cancelled")
            }
        }
    }
}

/// Restricts [`ActivityCenter::finish`] to terminal lifecycle outcomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalOutcome {
    /// Finished successfully.
    Completed,
    /// Failed. Retryable by publishing a newer [`Generation`].
    Failed,
    /// Cancelled by the user.
    Cancelled,
}

impl TerminalOutcome {
    /// The phase this outcome puts the row in.
    #[must_use]
    pub const fn phase(self) -> ActivityPhase {
        match self {
            TerminalOutcome::Completed => ActivityPhase::Completed,
            TerminalOutcome::Failed => ActivityPhase::Failed,
            TerminalOutcome::Cancelled => ActivityPhase::Cancelled,
        }
    }
}

/// Restricts [`ActivityCenter::set_live_phase`] to nonterminal phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivePhase {
    /// Admitted, not yet doing work.
    Queued,
    /// Doing work now.
    Running,
    /// Suspended by the user.
    Paused,
    /// Blocked on the user.
    Waiting,
}

impl LivePhase {
    /// The phase this moves the row to.
    #[must_use]
    pub const fn phase(self) -> ActivityPhase {
        match self {
            LivePhase::Queued => ActivityPhase::Queued,
            LivePhase::Running => ActivityPhase::Running,
            LivePhase::Paused => ActivityPhase::Paused,
            LivePhase::Waiting => ActivityPhase::Waiting,
        }
    }
}

/// An affordance the surface may offer for a row. The provider decides which
/// ones exist; the presentation decides how they look.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActivityControl {
    /// Suspend the operation.
    Pause,
    /// Resume a paused operation.
    Resume,
    /// Cancel the operation.
    Cancel,
    /// Retry a failed operation under a new generation.
    Retry,
    /// Answer whatever the operation is [`ActivityPhase::Waiting`] on.
    Resolve,
    /// Open the provider's own full panel (the Downloads screen, a job list).
    OpenPanel,
}

impl ActivityControl {
    /// The user-facing verb for this control.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            ActivityControl::Pause => crate::localization::text("enzui-operation-activity-pause"),
            ActivityControl::Resume => crate::localization::text("enzui-operation-activity-resume"),
            ActivityControl::Cancel => crate::localization::text("enzui-operation-activity-cancel"),
            ActivityControl::Retry => crate::localization::text("enzui-operation-activity-retry"),
            ActivityControl::Resolve => {
                crate::localization::text("enzui-operation-activity-review")
            }
            ActivityControl::OpenPanel => {
                crate::localization::text("enzui-operation-activity-show-all")
            }
        }
    }
}

/// One row of live work.
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    /// Stable identity.
    pub key: ActivityKey,
    /// Which attempt this row describes.
    pub generation: Generation,
    /// What kind of work it is.
    pub kind: ActivityKind,
    /// The primary line ("Copying 12 items to Documents").
    pub label: String,
    /// An optional second line: the current item, the error, the question.
    pub detail: Option<String>,
    /// How far along, in the unit the provider knows.
    pub progress: ActivityProgress,
    /// Lifecycle state.
    pub phase: ActivityPhase,
    /// Affordances this row offers, in presentation order.
    pub controls: Vec<ActivityControl>,
    // Set by the center when the row becomes terminal; absent while active.
    terminal_at: Option<u64>,
    // When the center first took this row, and whether the surface may show
    // it yet; see `ActivityCenter::with_reveal_delay`.
    started_at: Option<u64>,
    revealed: bool,
}

impl Activity {
    /// Creates a running row with no detail, controls, or determinate progress.
    #[must_use]
    pub fn new(key: ActivityKey, kind: ActivityKind, label: impl Into<String>) -> Self {
        Self {
            key,
            generation: Generation::FIRST,
            kind,
            label: label.into(),
            detail: None,
            progress: ActivityProgress::Indeterminate,
            phase: ActivityPhase::Running,
            controls: Vec::new(),
            terminal_at: None,
            started_at: None,
            revealed: true,
        }
    }

    /// Set the attempt this row describes.
    #[must_use]
    pub fn generation(mut self, generation: Generation) -> Self {
        self.generation = generation;
        self
    }

    /// Set the secondary line.
    #[must_use]
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Set the progress measure, applying [`ActivityProgress::normalized`].
    #[must_use]
    pub fn progress(mut self, progress: ActivityProgress) -> Self {
        self.progress = progress.normalized();
        self
    }

    /// Set the lifecycle phase.
    #[must_use]
    pub fn phase(mut self, phase: ActivityPhase) -> Self {
        self.phase = phase;
        self
    }

    /// Offer these controls, in this order.
    #[must_use]
    pub fn controls(mut self, controls: impl IntoIterator<Item = ActivityControl>) -> Self {
        self.controls = controls.into_iter().collect();
        self
    }

    /// Whether the row is still live work.
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.phase.is_active()
    }

    /// When the row became terminal, if it has.
    #[must_use]
    pub fn terminal_at(&self) -> Option<u64> {
        self.terminal_at
    }

    /// Returns an accessible description containing the label, phase, and a percentage when progress is determinate.
    #[must_use]
    pub fn announcement(&self) -> String {
        match self.progress.fraction() {
            Some(fraction) => {
                crate::localized!("enzui-activity-progress-announcement", "label" => self.label, "phase" => self.phase.word(), "percentage" => (fraction * 100.0).round() as u32)
            }
            None => {
                crate::localized!("enzui-activity-announcement", "label" => self.label, "phase" => self.phase.word())
            }
        }
    }

    fn rank(&self) -> u8 {
        match self.phase {
            ActivityPhase::Failed => 0,
            ActivityPhase::Waiting => 1,
            ActivityPhase::Running => 2,
            ActivityPhase::Paused => 3,
            ActivityPhase::Queued => 4,
            ActivityPhase::Completed | ActivityPhase::Cancelled => 5,
        }
    }
}

/// Why a publish did or did not change the center.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishOutcome {
    /// A new row was added.
    Added,
    /// An existing row was replaced by this newer (or same) generation.
    Replaced,
    /// The operation or generation is absent, stale, terminal, or already retired.
    Stale,
    /// Rejected: the provider is not registered, or is disabled.
    ProviderDisabled,
}

impl PublishOutcome {
    /// Whether the center changed.
    #[must_use]
    pub const fn accepted(self) -> bool {
        matches!(self, PublishOutcome::Added | PublishOutcome::Replaced)
    }
}

/// The aggregate of everything the center currently holds.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ActivitySummary {
    /// Rows queued or running.
    pub running: usize,
    /// Rows paused by the user.
    pub paused: usize,
    /// Rows blocked on the user.
    pub waiting: usize,
    /// Terminal failed rows still lingering.
    pub failed: usize,
    /// Terminal completed rows still lingering.
    pub completed: usize,
    /// Terminal cancelled rows still lingering.
    pub cancelled: usize,
    /// Aggregate completion when every active row is determinate in one unit.
    ///
    /// Byte and item totals are weighted by size; dimensionless fractions use an equal-weight mean. Mixed or unknown progress yields `None`.
    pub fraction: Option<f32>,
}

impl ActivitySummary {
    /// Rows that are live work: queued, running, paused or waiting.
    #[must_use]
    pub const fn active(&self) -> usize {
        self.running + self.paused + self.waiting
    }

    /// Whether anything at all is held.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.active() + self.failed + self.completed + self.cancelled == 0
    }
}

/// Selects the destination that can reveal all rows hidden by the row budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivityOverflowTarget {
    /// Every hidden row belongs to one provider, so open that provider's panel.
    Provider(ProviderId),
    /// Hidden rows belong to multiple providers, so no provider-specific panel
    /// can truthfully reveal all of them.
    AllActivity {
        /// The distinct hidden providers, in their first hidden-row order.
        providers: Vec<ProviderId>,
    },
}

/// A bounded read of the center for one frame.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityView<'a> {
    /// The rows the surface should build, already ordered by attention and
    /// truncated to the center's row budget.
    pub rows: Vec<&'a Activity>,
    /// How many further rows exist beyond `rows`. Non-zero means the surface
    /// shows a "+N more" affordance instead of building them.
    pub overflow: usize,
    /// The route which reveals every hidden row, derived from those rows.
    /// `None` exactly when [`Self::overflow`] is zero.
    pub overflow_target: Option<ActivityOverflowTarget>,
    /// Aggregate over all retained rows, including rows outside the bounded view.
    pub summary: ActivitySummary,
}

/// The framework-owned activity center: providers publish, the surface reads.
#[derive(Debug, Clone)]
pub struct ActivityCenter {
    rows: Vec<Activity>,
    providers: BTreeMap<ProviderId, bool>,
    // Highest retired generation per key, retained in bounded insertion order.
    retired: Vec<(ActivityKey, Generation)>,
    linger: Duration,
    row_budget: usize,
    reveal_delay: Duration,
}

impl Default for ActivityCenter {
    fn default() -> Self {
        Self::new()
    }
}

impl ActivityCenter {
    /// Default duration for retaining terminal outcomes.
    pub const DEFAULT_LINGER: Duration = Duration::from_secs(6);

    /// Default number of rows shown before overflow is collapsed.
    pub const DEFAULT_ROW_BUDGET: usize = 3;

    /// Default time a running row waits before the surface shows it.
    pub const DEFAULT_REVEAL_DELAY: Duration = Duration::from_millis(800);

    /// Maximum remembered retired keys used to prevent resurrection.
    pub const RETIRED_MEMORY: usize = 256;

    /// An empty center with no providers registered.
    #[must_use]
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            providers: BTreeMap::new(),
            retired: Vec::new(),
            linger: Self::DEFAULT_LINGER,
            row_budget: Self::DEFAULT_ROW_BUDGET,
            reveal_delay: Self::DEFAULT_REVEAL_DELAY,
        }
    }

    /// Set how long a running row waits before the surface shows it.
    ///
    /// Work that finishes sooner never shows as running: its row appears
    /// already finished, so a copy that takes a moment does not flash a
    /// progress row for a few frames. A row is revealed by [`Self::prune`]
    /// at its deadline, which [`Self::next_expiry_ms`] and
    /// [`Self::time_until_prune`] include. Zero shows rows at once.
    #[must_use]
    pub fn with_reveal_delay(mut self, delay: Duration) -> Self {
        self.reveal_delay = delay;
        self
    }

    fn reveal_deadline(&self, started_at_ms: u64) -> u64 {
        use crate::monotonic_ms::{MonotonicMs, Policy};

        let delay = MonotonicMs::from_duration_saturating(self.reveal_delay);
        MonotonicMs(started_at_ms)
            .deadline_from(delay, Policy::Clamp)
            .get()
    }

    /// How long terminal rows remain before [`Self::prune`] removes them.
    #[must_use]
    pub fn linger(&self) -> Duration {
        self.linger
    }

    /// Set the terminal linger.
    #[must_use]
    pub fn with_linger(mut self, linger: Duration) -> Self {
        self.linger = linger;
        self
    }

    /// Sets the row budget, clamping zero to one.
    #[must_use]
    pub fn with_row_budget(mut self, budget: usize) -> Self {
        self.row_budget = budget.max(1);
        self
    }

    /// Registers a provider and its initial enabled state; unregistered providers cannot publish.
    pub fn register_provider(&mut self, provider: ProviderId, enabled: bool) {
        self.providers.insert(provider, enabled);
        if !enabled {
            self.drop_provider_rows(provider);
        }
    }

    /// Registers or changes a provider’s enabled state. Disabling removes and retires its current rows.
    pub fn set_provider_enabled(&mut self, provider: ProviderId, enabled: bool) {
        if let Some(slot) = self.providers.get_mut(&provider) {
            *slot = enabled;
        } else {
            self.providers.insert(provider, enabled);
        }
        if !enabled {
            self.drop_provider_rows(provider);
        }
    }

    fn drop_provider_rows(&mut self, provider: ProviderId) {
        let dropped: Vec<(ActivityKey, Generation)> = self
            .rows
            .iter()
            .filter(|row| row.key.provider == provider)
            .map(|row| (row.key, row.generation))
            .collect();
        self.rows.retain(|row| row.key.provider != provider);
        for (key, generation) in dropped {
            self.retire(key, generation);
        }
    }

    // Record the highest retired generation and evict oldest keys beyond `RETIRED_MEMORY`.
    fn retire(&mut self, key: ActivityKey, generation: Generation) {
        if let Some(slot) = self.retired.iter_mut().find(|(held, _)| *held == key) {
            if slot.1 < generation {
                slot.1 = generation;
            }
            return;
        }
        if self.retired.len() >= Self::RETIRED_MEMORY {
            self.retired.remove(0);
        }
        self.retired.push((key, generation));
    }

    /// Returns whether this generation has been retired; newer generations remain admissible.
    #[must_use]
    pub fn is_retired(&self, key: ActivityKey, generation: Generation) -> bool {
        self.retired
            .iter()
            .any(|(held, retired)| *held == key && *retired >= generation)
    }

    /// Whether `provider` may publish.
    #[must_use]
    pub fn provider_enabled(&self, provider: ProviderId) -> bool {
        self.providers.get(&provider).copied().unwrap_or(false)
    }

    /// Adds or replaces a normalized row and is the only operation that starts an attempt.
    ///
    /// Refuses disabled providers, retired attempts, regressing generations, and same-generation resurrection of terminal rows.
    pub fn publish(&mut self, activity: Activity, now_ms: u64) -> PublishOutcome {
        if !self.provider_enabled(activity.key.provider) {
            return PublishOutcome::ProviderDisabled;
        }
        if self.is_retired(activity.key, activity.generation) {
            return PublishOutcome::Stale;
        }
        let mut activity = activity;
        activity.progress = activity.progress.normalized();
        let terminal = activity.phase.is_terminal();
        activity.terminal_at = terminal.then_some(now_ms);
        let at_once = self.reveal_delay.is_zero();
        match self.rows.iter_mut().find(|row| row.key == activity.key) {
            Some(existing) => {
                let regresses = activity.generation < existing.generation;
                let republishes_finished_work =
                    existing.phase.is_terminal() && activity.generation == existing.generation;
                if regresses || republishes_finished_work {
                    return PublishOutcome::Stale;
                }
                activity.started_at = existing.started_at.or(Some(now_ms));
                activity.revealed = existing.revealed || terminal || at_once;
                *existing = activity;
                PublishOutcome::Replaced
            }
            None => {
                activity.started_at = Some(now_ms);
                activity.revealed = terminal || at_once;
                self.rows.push(activity);
                PublishOutcome::Added
            }
        }
    }

    // Incremental updates require the exact current generation; only `publish` starts a newer attempt.
    fn is_current(row: &Activity, generation: Generation) -> bool {
        row.generation == generation
    }

    /// Updates normalized progress only for an existing live row at its exact generation.
    pub fn progress(
        &mut self,
        key: ActivityKey,
        generation: Generation,
        progress: ActivityProgress,
    ) -> PublishOutcome {
        if !self.provider_enabled(key.provider) {
            return PublishOutcome::ProviderDisabled;
        }
        match self.rows.iter_mut().find(|row| row.key == key) {
            Some(row) if !Self::is_current(row, generation) || row.phase.is_terminal() => {
                PublishOutcome::Stale
            }
            Some(row) => {
                row.progress = progress.normalized();
                PublishOutcome::Replaced
            }
            None => PublishOutcome::Stale,
        }
    }

    /// Changes an existing row to a live phase only at its exact generation.
    pub fn set_live_phase(
        &mut self,
        key: ActivityKey,
        generation: Generation,
        phase: LivePhase,
    ) -> PublishOutcome {
        if !self.provider_enabled(key.provider) {
            return PublishOutcome::ProviderDisabled;
        }
        match self.rows.iter_mut().find(|row| row.key == key) {
            Some(row) if !Self::is_current(row, generation) || row.phase.is_terminal() => {
                PublishOutcome::Stale
            }
            Some(row) => {
                row.phase = phase.phase();
                PublishOutcome::Replaced
            }
            None => PublishOutcome::Stale,
        }
    }

    /// Ends an existing live row at its exact generation.
    ///
    /// Repeated or stale terminal transitions are refused.
    pub fn finish(
        &mut self,
        key: ActivityKey,
        generation: Generation,
        outcome: TerminalOutcome,
        now_ms: u64,
    ) -> PublishOutcome {
        if !self.provider_enabled(key.provider) {
            return PublishOutcome::ProviderDisabled;
        }
        let phase = outcome.phase();
        match self.rows.iter_mut().find(|row| row.key == key) {
            Some(row) if !Self::is_current(row, generation) || row.phase.is_terminal() => {
                PublishOutcome::Stale
            }
            Some(row) => {
                row.phase = phase;
                row.terminal_at = Some(now_ms);
                row.revealed = true;
                if outcome == TerminalOutcome::Completed {
                    row.progress = match row.progress {
                        ActivityProgress::Bytes {
                            total: Some(total), ..
                        } => ActivityProgress::Bytes {
                            done: total,
                            total: Some(total),
                        },
                        ActivityProgress::Items {
                            total: Some(total), ..
                        } => ActivityProgress::Items {
                            done: total,
                            total: Some(total),
                        },
                        ActivityProgress::Fraction(_) => ActivityProgress::Fraction(1.0),
                        other => other,
                    };
                }
                row.controls.retain(|control| {
                    matches!(control, ActivityControl::Retry | ActivityControl::OpenPanel)
                });
                PublishOutcome::Replaced
            }
            None => PublishOutcome::Stale,
        }
    }

    /// Removes a row at any phase and retires its current attempt.
    pub fn dismiss(&mut self, key: ActivityKey) -> bool {
        let Some(row) = self.rows.iter().find(|row| row.key == key) else {
            return false;
        };
        let generation = row.generation;
        self.rows.retain(|row| row.key != key);
        self.retire(key, generation);
        true
    }

    fn terminal_deadline(&self, terminal_at_ms: u64) -> crate::monotonic_ms::MonotonicMs {
        use crate::monotonic_ms::{MonotonicMs, Policy};

        let linger_ms = MonotonicMs::from_duration_saturating(self.linger);
        MonotonicMs(terminal_at_ms).deadline_from(linger_ms, Policy::Clamp)
    }

    /// Returns the earliest terminal-row expiry in the caller’s millisecond clock, if any.
    #[must_use]
    pub fn next_expiry_ms(&self) -> Option<u64> {
        let expiries = self
            .rows
            .iter()
            .filter_map(|row| row.terminal_at)
            .map(|at| self.terminal_deadline(at).get());
        let reveals = self
            .rows
            .iter()
            .filter(|row| !row.revealed)
            .filter_map(|row| row.started_at)
            .map(|at| self.reveal_deadline(at));
        expiries.chain(reveals).min()
    }

    /// Returns the delay until the next inclusive expiry.
    ///
    /// Overdue deadlines yield zero, and clock rollback preserves the full clamped dwell.
    #[must_use]
    pub fn time_until_prune(&self, now_ms: u64) -> Option<Duration> {
        use crate::monotonic_ms::{Boundary, MonotonicMs};

        self.rows
            .iter()
            .filter_map(|row| row.terminal_at)
            .map(|terminal_at| {
                let deadline = self.terminal_deadline(terminal_at);
                let effective_now = MonotonicMs(now_ms.max(terminal_at));
                Duration::from_millis(effective_now.remaining_ms(deadline, Boundary::Inclusive))
            })
            .chain(
                self.rows
                    .iter()
                    .filter(|row| !row.revealed)
                    .filter_map(|row| row.started_at)
                    .map(|started_at| {
                        let deadline = MonotonicMs(self.reveal_deadline(started_at));
                        let effective_now = MonotonicMs(now_ms.max(started_at));
                        Duration::from_millis(
                            effective_now.remaining_ms(deadline, Boundary::Inclusive),
                        )
                    }),
            )
            .min()
    }

    /// Reveals running rows whose delay has elapsed, then removes terminal
    /// rows whose linger has elapsed and retires their attempts.
    pub fn prune(&mut self, now_ms: u64) -> Vec<ActivityKey> {
        use crate::monotonic_ms::{Boundary, MonotonicMs};

        for index in 0..self.rows.len() {
            let row = &self.rows[index];
            if !row.revealed
                && row
                    .started_at
                    .is_some_and(|at| now_ms >= self.reveal_deadline(at))
            {
                self.rows[index].revealed = true;
            }
        }

        let linger_ms = MonotonicMs::from_duration_saturating(self.linger);
        let mut removed = Vec::new();
        self.rows.retain(|row| match row.terminal_at {
            Some(at)
                if MonotonicMs(now_ms).is_expired(
                    MonotonicMs(at).deadline_from(linger_ms, crate::monotonic_ms::Policy::Clamp),
                    Boundary::Inclusive,
                ) =>
            {
                removed.push((row.key, row.generation));
                false
            }
            _ => true,
        });
        for (key, generation) in &removed {
            self.retire(*key, *generation);
        }
        removed.into_iter().map(|(key, _)| key).collect()
    }

    /// Whether the center holds nothing the surface may show — it mounts no
    /// node. Rows still inside their reveal delay do not count.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.rows.iter().any(|row| row.revealed)
    }

    /// Whether `key`'s row is past its reveal delay (or finished).
    #[must_use]
    pub fn is_revealed(&self, key: ActivityKey) -> bool {
        self.get(key).is_some_and(|row| row.revealed)
    }

    /// Total rows held, active and lingering.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Every row, in publish order.
    pub fn iter(&self) -> impl Iterator<Item = &Activity> {
        self.rows.iter()
    }

    /// One row by key.
    #[must_use]
    pub fn get(&self, key: ActivityKey) -> Option<&Activity> {
        self.rows.iter().find(|row| row.key == key)
    }

    /// Aggregates all retained rows.
    #[must_use]
    pub fn summary(&self) -> ActivitySummary {
        self.summary_where(|_| true)
    }

    fn summary_where(&self, keep: impl Fn(&Activity) -> bool) -> ActivitySummary {
        let mut summary = ActivitySummary::default();
        let mut unit = None;
        let mut done = 0u64;
        let mut total = 0u64;
        let mut fraction_sum = 0.0f64;
        let mut fraction_count = 0u64;
        let mut determinate = true;
        for row in self.rows.iter().filter(|row| keep(row)) {
            match row.phase {
                ActivityPhase::Queued | ActivityPhase::Running => summary.running += 1,
                ActivityPhase::Paused => summary.paused += 1,
                ActivityPhase::Waiting => summary.waiting += 1,
                ActivityPhase::Failed => summary.failed += 1,
                ActivityPhase::Completed => summary.completed += 1,
                ActivityPhase::Cancelled => summary.cancelled += 1,
            }
            if !row.is_active() {
                continue;
            }
            match (row.progress, unit) {
                (ActivityProgress::Indeterminate, _) => determinate = false,
                (progress, None) => {
                    unit = Some(progress.unit());
                }
                (progress, Some(seen)) if progress.unit() != seen => determinate = false,
                _ => {}
            }
            match row.progress {
                ActivityProgress::Fraction(value) if value.is_finite() => {
                    fraction_sum += f64::from(value.clamp(0.0, 1.0));
                    fraction_count = fraction_count.saturating_add(1);
                }
                ActivityProgress::Bytes {
                    done: d,
                    total: Some(t),
                }
                | ActivityProgress::Items {
                    done: d,
                    total: Some(t),
                } => {
                    done = done.saturating_add(d.min(t));
                    total = total.saturating_add(t);
                }
                _ => determinate = false,
            }
        }
        summary.fraction = (determinate && summary.active() > 0).then(|| match unit {
            Some(Unit::Fraction) if fraction_count > 0 => {
                (fraction_sum / fraction_count as f64) as f32
            }
            Some(Unit::Bytes | Unit::Items) if total == 0 => {
                // A known zero total is complete, not indeterminate.
                1.0
            }
            Some(Unit::Bytes | Unit::Items) => (done as f32) / (total as f32),
            Some(Unit::Unknown | Unit::Fraction) | None => {
                unreachable!("a determinate non-empty summary must have a supported unit")
            }
        });
        summary
    }

    /// The bounded read the presentation uses for one frame.
    #[must_use]
    pub fn view(&self) -> ActivityView<'_> {
        let mut ordered: Vec<&Activity> = self.rows.iter().filter(|row| row.revealed).collect();
        ordered.sort_by_key(|row| row.rank());

        let visible = ordered.len().min(self.row_budget);
        let hidden = &ordered[visible..];
        let overflow = hidden.len();
        let mut hidden_providers = Vec::new();
        for row in hidden {
            if !hidden_providers.contains(&row.key.provider) {
                hidden_providers.push(row.key.provider);
            }
        }
        let overflow_target = match hidden_providers.as_slice() {
            [] => None,
            [provider] => Some(ActivityOverflowTarget::Provider(*provider)),
            _ => Some(ActivityOverflowTarget::AllActivity {
                providers: hidden_providers,
            }),
        };

        ordered.truncate(visible);
        ActivityView {
            rows: ordered,
            overflow,
            overflow_target,
            summary: self.summary_where(|row| row.revealed),
        }
    }
}

/// Tracks the one pruning deadline an application has armed.
///
/// After center changes, call [`sync`](Self::sync); call [`fired`](Self::fired) when the timer runs before pruning and synchronizing again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ActivitySchedule {
    armed: Option<u64>,
}

impl ActivitySchedule {
    /// The deadline this schedule currently has a timer armed for.
    #[must_use]
    pub const fn armed_deadline(&self) -> Option<u64> {
        self.armed
    }

    /// Clears the armed deadline after its timer fires so [`sync`](Self::sync) may arm another.
    pub fn fired(&mut self) {
        self.armed = None;
    }

    /// Returns a delay when the earliest expiry differs from the armed deadline.
    ///
    /// A moved-later deadline may leave one stale application-owned wakeup; pruning it removes nothing, and the next sync arms the live deadline.
    pub fn sync(&mut self, center: &ActivityCenter, now_ms: u64) -> Option<Duration> {
        match center.next_expiry_ms() {
            None => {
                self.armed = None;
                None
            }
            Some(deadline) if self.armed == Some(deadline) => None,
            Some(deadline) => {
                self.armed = Some(deadline);
                center.time_until_prune(now_ms)
            }
        }
    }
}

pub mod downloads;

#[cfg(test)]
mod tests;
