//! Queued toast notifications with dwell timing, waiting budgets, and live announcements.
//!
//! Hold a [`ToastHost`] and [`ToastExpirySchedule`] in app state. The host owns
//! no timer subscription: forward timer instants as [`ToastEvent::Tick`] and
//! reconcile the schedule after host updates. `iced::time::every(wait)` is
//! periodic; process its tick and reconcile again to disarm or re-key it.
//! See [`ToastExpirySchedule`] for a compiled lifecycle example.
//!
//! Pushes and ticks initialize newly visible entries with their full dwell.
//! Manual dismissal changes the visible window without initializing newly
//! exposed dwell; call [`ToastHost::tick_at`] afterward before syncing timers.
//! Hover pauses dwell automatically; keyboard focus needs explicit pause and
//! resume events from the app.
//!
//! By default, one live region carries the newest entry presented by a push or
//! tick. Removing its subject removes the region without re-announcing older
//! survivors. [`ToastAnnouncements::Decorative`] omits the region entirely.
//! Individual toast nodes remain readable and silent in either mode.
//! [`ToastHost::view_with`] customizes each entry's presentation while retaining
//! keyed widget state, hover timing, and the shared announcement.
//!
//! [`enz_toast_layer`] keeps the presentation mounted in a stable stack and
//! floats toasts above the outermost presentation, outside modal inert layers.

use crate::motion::TimerWakeFloor;
use crate::toast::{EnzToast, ToastSize, ToastTone, enz_toast};
use enzui_core::live_region::Announcement;
use enzui_core::metrics::FiniteNonNeg;
use iced::widget::container;
use iced::{Element, Length, Padding};
use std::time::{Duration, Instant};

/// Content and optional dwell override for a queued toast.
#[derive(Debug, Clone)]
#[must_use]
pub struct ToastSpec {
    /// The toast message (required).
    pub text: String,
    /// Optional muted second line.
    pub description: String,
    /// Tone (default `Info`).
    pub tone: ToastTone,
    /// Size (default `Md`).
    pub size: ToastSize,
    /// Whether to show the leading status icon.
    pub show_icon: bool,
    /// Optional action button label.
    pub action_label: String,
    /// Dwell override; `None` uses the current host default at initialization.
    pub duration: Option<Duration>,
    /// Optional key for in-place replacement; `None` makes every push distinct.
    pub coalesce_key: Option<String>,
    /// Whether the toast stays until it is dismissed. `None` decides by tone:
    /// an error ([`ToastTone::Error`]) without an explicit
    /// [`duration`](Self::duration) stays, everything else expires.
    pub sticky: Option<bool>,
}

impl ToastSpec {
    /// The total UTF-8 content length of the owned string fields, in bytes.
    ///
    /// Used for waiting-byte budgets; excludes spare string capacity, struct
    /// overhead, and receipt storage.
    pub fn owned_bytes(&self) -> usize {
        self.text.len()
            + self.description.len()
            + self.action_label.len()
            + self
                .coalesce_key
                .as_ref()
                .map_or(0, std::string::String::len)
    }

    /// Create an info toast at medium size, with its icon enabled.
    ///
    /// Description, action, coalescing key, and dwell override are initially empty.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            description: String::new(),
            tone: ToastTone::default(),
            size: ToastSize::default(),
            show_icon: true,
            action_label: String::new(),
            duration: None,
            sticky: None,
            coalesce_key: None,
        }
    }

    /// Set the tone.
    pub fn tone(mut self, tone: ToastTone) -> Self {
        self.tone = tone;
        self
    }

    /// Set the size.
    pub fn size(mut self, size: ToastSize) -> Self {
        self.size = size;
        self
    }

    /// Set whether the leading icon is shown.
    pub fn show_icon(mut self, v: bool) -> Self {
        self.show_icon = v;
        self
    }

    /// Set an optional second-line description.
    pub fn description(mut self, d: impl Into<String>) -> Self {
        self.description = d.into();
        self
    }

    /// Set an action button label.
    pub fn action_label(mut self, label: impl Into<String>) -> Self {
        self.action_label = label.into();
        self
    }

    /// Keep the toast until it is dismissed (`true`), or let it expire even
    /// as an error (`false`). See [`ToastSpec::sticky`](Self#structfield.sticky).
    pub fn sticky(mut self, yes: bool) -> Self {
        self.sticky = Some(yes);
        self
    }

    /// Whether the toast waits for a dismissal instead of expiring.
    #[must_use]
    pub fn is_sticky(&self) -> bool {
        self.sticky
            .unwrap_or_else(|| matches!(self.tone, ToastTone::Error) && self.duration.is_none())
    }

    /// Override dwell duration; zero expires on the next expiry check.
    ///
    /// An unrepresentable instant deadline leaves the toast without automatic expiry.
    pub fn duration(mut self, d: Duration) -> Self {
        self.duration = Some(d);
        self
    }

    /// Set the key for in-place replacement; see [`ToastHost::push_at`].
    pub fn coalesce_key(mut self, key: impl Into<String>) -> Self {
        self.coalesce_key = Some(key.into());
        self
    }
}

/// A queue identity allocated by one host, independent of queue position.
///
/// IDs increase until the host counter overflows. Separate or cloned hosts
/// can allocate identical IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ToastId(u64);

impl ToastId {
    /// The raw numeric value (useful for id derivation).
    pub fn raw(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for ToastId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Eviction policy for waiting entries when a push exceeds count or byte limits.
///
/// Visible entries are exempt; the default is [`Self::RejectNew`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverflowPolicy {
    /// Drop the incoming entry if it is waiting, preserving older entries.
    ///
    /// A visible coalescing replacement is retained. Previously accepted waiting
    /// entries can remain over budget after limits are tightened.
    RejectNew,
    /// Evict the oldest waiting entries until the count and byte limits hold.
    DropOldestWaiting,
}

impl Default for OverflowPolicy {
    /// Returns [`OverflowPolicy::RejectNew`].
    fn default() -> Self {
        OverflowPolicy::RejectNew
    }
}

/// Count, UTF-8 content-byte, and monotonic age limits on waiting entries.
///
/// Defaults to unbounded (`None` for every limit). `Some(0)` is a literal
/// zero limit. Age is measured from the latest push or replacement and swept
/// on non-coalescing pushes and ticks; it does not arm an expiry wake.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[must_use]
pub struct WaitingBudget {
    /// Maximum number of entries retained in the waiting region. `Some(0)` =
    /// visible-only, honoured literally; `None` = unbounded.
    pub max_waiting: Option<usize>,
    /// Maximum UTF-8 content bytes across waiting entries; `None` is unbounded.
    pub max_waiting_bytes: Option<usize>,
    /// Maximum monotonic waiting age before a push or tick drops the entry.
    pub max_waiting_age: Option<Duration>,
}

impl WaitingBudget {
    /// An explicitly unbounded budget (all fields `None`). Equivalent to
    /// [`WaitingBudget::default`]; named for call-site clarity.
    pub fn unbounded() -> Self {
        Self::default()
    }

    /// Set the maximum waiting-entry count. `Some(0)` is honoured literally
    /// (visible-only); pass `None` for unbounded.
    pub fn max_waiting(mut self, n: Option<usize>) -> Self {
        self.max_waiting = n;
        self
    }

    /// Set the UTF-8 content-byte limit across waiting entries.
    pub fn max_waiting_bytes(mut self, n: Option<usize>) -> Self {
        self.max_waiting_bytes = n;
        self
    }

    /// Set the staleness cap on waiting entries.
    pub fn max_waiting_age(mut self, d: Option<Duration>) -> Self {
        self.max_waiting_age = d;
        self
    }
}

/// Controls the host live region; individual toast nodes remain silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToastAnnouncements {
    /// Publish a live region for the newest entry presented by a push or tick.
    ///
    /// No region is published after that entry leaves until another is presented.
    #[default]
    Live,
    /// Omit the live region for decorative content.
    Decorative,
}

#[derive(Debug, Clone)]
struct ToastEntry {
    id: ToastId,
    spec: ToastSpec,
    deadline: Option<Instant>,
    paused_remaining: Option<Duration>,
    pushed_at: Instant,
    // Presentation identity is independent of paused or unrepresentable deadlines.
    presented: bool,
}

/// View events and app-supplied timer events handled by [`ToastHost::update`].
#[derive(Debug, Clone)]
pub enum ToastEvent {
    /// A timer tick arrived — the host should check for expired toasts.
    Tick(Instant),
    /// A toast was manually dismissed by the user clicking the dismiss button.
    Dismiss(ToastId),
    /// An action button was clicked. Carries the toast id and the action label.
    Action(ToastId, String),
    /// Pause dwell at [`Instant::now`] when processed by [`ToastHost::update`].
    ///
    /// The view emits this on pointer entry. Apps can send it for keyboard focus;
    /// call [`ToastHost::pause`] for an explicit instant.
    Pause(ToastId),
    /// Resume paused dwell at [`Instant::now`] when processed by [`ToastHost::update`].
    Resume(ToastId),
}

/// Tracks the earliest visible dwell deadline and a stable timer delay.
///
/// Call [`sync`](Self::sync) after host updates and build the app's timer from
/// [`armed`](Self::armed). This object starts and cancels no timers itself.
/// `iced::time::every(wait).with(deadline)` is periodic while mounted: process
/// each tick, sync again, and rebuild the subscription to disarm or re-key it.
/// The delay remains unchanged while the deadline is unchanged; it is at least
/// one millisecond, including for already due deadlines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ToastExpirySchedule {
    armed: Option<ArmedExpiry>,
}

// Timer delays must stay nonzero even under reduced motion.
const EXPIRY_WAKE_FLOOR: TimerWakeFloor = TimerWakeFloor::of(Duration::from_millis(1));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ArmedExpiry {
    deadline: Instant,
    // Freeze the delay so unrelated updates preserve subscription identity.
    wait: Duration,
}

impl ToastExpirySchedule {
    /// The `(deadline, wait)` currently armed, or `None` when nothing is.
    ///
    /// Arm `iced::time::every(wait).with(deadline)` from this in the app's
    /// `subscription()`. `wait` is stable while the deadline is unchanged, so
    /// the recipe's iced identity is stable across unrelated re-evaluations.
    #[must_use]
    pub const fn armed(&self) -> Option<(Instant, Duration)> {
        match self.armed {
            Some(a) => Some((a.deadline, a.wait)),
            None => None,
        }
    }

    /// The deadline this schedule currently has a wake armed for.
    #[must_use]
    pub const fn armed_deadline(&self) -> Option<Instant> {
        match self.armed {
            Some(a) => Some(a.deadline),
            None => None,
        }
    }

    /// Update the armed deadline, returning a delay only when it changes.
    ///
    /// An absent host deadline clears [`armed`](Self::armed); an unchanged deadline
    /// returns `None` while retaining its previous delay. Timer ownership and
    /// cancellation remain with the app.
    pub fn sync(&mut self, host: &ToastHost, now: Instant) -> Option<Duration> {
        match host.next_expiry() {
            None => {
                self.armed = None;
                None
            }
            Some(deadline) if self.armed_deadline() == Some(deadline) => None,
            Some(deadline) => {
                let wait = EXPIRY_WAKE_FLOOR.wake_after(deadline.saturating_duration_since(now));
                self.armed = Some(ArmedExpiry { deadline, wait });
                Some(wait)
            }
        }
    }
}

const DURATION_MULTIPLIER: u32 = 20;

const DEFAULT_MAX_VISIBLE: usize = 5;

const DEFAULT_MAX_RECEIPTS: usize = 64;

/// A toast queue manager with auto-dismiss timers and stacking.
///
/// See the [module docs](self) for integration instructions.
#[derive(Debug, Clone)]
#[must_use]
pub struct ToastHost {
    queue: Vec<ToastEntry>,
    next_id: u64,
    max_visible: usize,
    default_duration: Duration,
    id_prefix: String,
    waiting_budget: WaitingBudget,
    overflow_policy: OverflowPolicy,
    dropped: Vec<ToastId>,
    dropped_count: u64,
    replaced: Vec<ToastId>,
    replaced_count: u64,
    max_receipts: usize,
    announcements: ToastAnnouncements,
    announced: Option<ToastId>,
}

impl Default for ToastHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ToastHost {
    /// Create an empty host with five visible slots and unbounded waiting.
    ///
    /// Defaults to live announcements, rejection of new waiting overflow, 64 IDs
    /// per receipt buffer, and a 5200 ms dwell derived from default motion tokens.
    pub fn new() -> Self {
        let motion = enzui_theme::MotionScale::default();
        Self {
            queue: Vec::new(),
            next_id: 1,
            max_visible: DEFAULT_MAX_VISIBLE,
            default_duration: Duration::from_millis(
                (enzui_theme::sane_motion_ms(motion.slow_ms) as u64)
                    .saturating_mul(u64::from(DURATION_MULTIPLIER)),
            ),
            id_prefix: "enz-toast-host".into(),
            waiting_budget: WaitingBudget::unbounded(),
            overflow_policy: OverflowPolicy::default(),
            dropped: Vec::new(),
            dropped_count: 0,
            replaced: Vec::new(),
            replaced_count: 0,
            max_receipts: DEFAULT_MAX_RECEIPTS,
            announcements: ToastAnnouncements::Live,
            announced: None,
        }
    }

    /// Set live-region publication; defaults to [`ToastAnnouncements::Live`].
    pub fn announcements(mut self, mode: ToastAnnouncements) -> Self {
        self.announcements = mode;
        self
    }

    /// Set the visible-slot limit, clamped to at least one.
    ///
    /// Changes the visible window immediately; dwell and announcements initialize
    /// on the next push or tick. Existing deadlines and budgets are not reconciled.
    pub fn max_visible(mut self, n: usize) -> Self {
        self.max_visible = n.max(1);
        self
    }

    /// Set waiting limits; defaults to unbounded.
    ///
    /// This setter does not enforce the new limits. Pushes enforce count and byte
    /// limits; non-coalescing pushes and ticks sweep stale entries.
    pub fn waiting_budget(mut self, budget: WaitingBudget) -> Self {
        self.waiting_budget = budget;
        self
    }

    /// Set the overflow policy. See [`OverflowPolicy`]. Default is
    /// [`OverflowPolicy::RejectNew`].
    pub fn overflow_policy(mut self, policy: OverflowPolicy) -> Self {
        self.overflow_policy = policy;
        self
    }

    /// Set the maximum number of IDs retained in each receipt buffer.
    ///
    /// Enforced immediately: oldest excess removed, oversized allocations
    /// shrunk toward the cap; monotonic lifetime counters unaffected.
    /// Default: 64. `usize::MAX` = unbounded (grows if never drained).
    pub fn max_receipts(mut self, n: usize) -> Self {
        self.max_receipts = n;
        cap_and_shrink_receipt_buffer(&mut self.dropped, n);
        cap_and_shrink_receipt_buffer(&mut self.replaced, n);
        self
    }

    /// Set the default dwell used when initializing future deadlines.
    ///
    /// Existing deadlines and paused remaining durations are unchanged.
    pub fn default_duration(mut self, d: Duration) -> Self {
        self.default_duration = d;
        self
    }

    /// Set the id namespace prefix.
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = prefix.into();
        self
    }

    /// Set future default dwell to normalized `slow_ms` multiplied by 20.
    ///
    /// Reduced motion does not shorten dwell. Existing deadlines and paused
    /// remaining durations are unchanged.
    pub fn sync_motion(&mut self, motion: &enzui_theme::MotionScale) {
        self.default_duration = Duration::from_millis(
            (enzui_theme::sane_motion_ms(motion.slow_ms) as u64)
                .saturating_mul(u64::from(DURATION_MULTIPLIER)),
        );
    }

    /// Enqueue or coalesce a toast at [`Instant::now`].
    ///
    /// Has the same admission, receipt, and overflow contracts as [`Self::push_at`].
    ///
    /// # Panics
    ///
    /// Panics on ID or lifetime-counter overflow when overflow checks are enabled.
    pub fn push(&mut self, spec: ToastSpec) -> ToastId {
        self.push_at(spec, Instant::now())
    }

    /// Enqueue or coalesce a toast at `now`, returning its newly allocated ID.
    ///
    /// A matching coalescing key replaces the queued entry in place and records
    /// its old ID in [`Self::take_replaced`]. A visible replacement restarts dwell
    /// and announcement; a waiting replacement resets its waiting age.
    ///
    /// Newly visible entries receive deadlines at `now`. Waiting count and byte
    /// limits apply before return; visible entries are exempt. Under `RejectNew`,
    /// previously accepted entries can remain over newly tightened limits. The
    /// returned ID may already be dropped; inspect [`Self::take_dropped`]. Only
    /// non-coalescing pushes sweep stale waiting entries.
    ///
    /// # Panics
    ///
    /// Panics on ID or lifetime-counter overflow when overflow checks are enabled.
    pub fn push_at(&mut self, spec: ToastSpec, now: Instant) -> ToastId {
        let id = ToastId(self.next_id);
        self.next_id += 1;

        if let Some(key) = spec.coalesce_key.as_deref()
            && let Some(pos) = self
                .queue
                .iter()
                .position(|e| e.spec.coalesce_key.as_deref() == Some(key))
        {
            let old_id = self.queue[pos].id;
            self.queue[pos] = ToastEntry {
                id,
                spec,
                deadline: None,
                paused_remaining: None,
                pushed_at: now,
                presented: false,
            };
            self.announced = self.announced.filter(|a| *a != old_id);
            self.replaced.push(old_id);
            self.replaced_count += 1;
            cap_receipt_buffer(&mut self.replaced, self.max_receipts);
            self.enforce_waiting_budget(id);
            self.reconcile_announcement();
            self.promote_visibility(now);
            return id;
        }

        self.queue.push(ToastEntry {
            id,
            spec,
            deadline: None,
            paused_remaining: None,
            pushed_at: now,
            presented: false,
        });
        self.sweep_stale_waiting(now);
        self.enforce_waiting_budget(id);
        self.reconcile_announcement();
        self.promote_visibility(now);
        id
    }

    fn waiting_start(&self) -> usize {
        self.max_visible.min(self.queue.len())
    }

    fn record_dropped(&mut self, id: ToastId) {
        self.dropped.push(id);
        self.dropped_count += 1;
        cap_receipt_buffer(&mut self.dropped, self.max_receipts);
    }

    fn sweep_stale_waiting(&mut self, now: Instant) {
        let Some(max_age) = self.waiting_budget.max_waiting_age else {
            return;
        };
        let start = self.waiting_start();
        let mut stale: Vec<ToastId> = Vec::new();
        for entry in self.queue.iter().skip(start) {
            if now.saturating_duration_since(entry.pushed_at) >= max_age {
                stale.push(entry.id);
            }
        }
        for id in &stale {
            self.queue.retain(|e| e.id != *id);
            self.record_dropped(*id);
        }
        self.reconcile_announcement();
    }

    fn enforce_waiting_budget(&mut self, incoming: ToastId) {
        let policy = self.overflow_policy;

        loop {
            let start = self.waiting_start();
            let waiting_len = self.queue.len() - start;

            let count_ok = match self.waiting_budget.max_waiting {
                Some(cap) => waiting_len <= cap,
                None => true,
            };
            let bytes_ok = match self.waiting_budget.max_waiting_bytes {
                Some(cap) => {
                    let used: usize = self.queue[start..]
                        .iter()
                        .map(|e| e.spec.owned_bytes())
                        .sum();
                    used <= cap
                }
                None => true,
            };

            if count_ok && bytes_ok {
                return;
            }
            if waiting_len == 0 {
                return;
            }

            match policy {
                OverflowPolicy::RejectNew => {
                    if let Some(pos) = self.queue.iter().position(|e| e.id == incoming)
                        && pos >= start
                    {
                        self.queue.remove(pos);
                        self.record_dropped(incoming);
                        continue;
                    }
                    return;
                }
                OverflowPolicy::DropOldestWaiting => {
                    let victim = self.queue[start].id;
                    self.queue.remove(start);
                    self.record_dropped(victim);
                    continue;
                }
            }
        }
    }

    /// Drain and return dropped IDs buffered since the last drain.
    ///
    /// The buffer keeps its allocation; the returned `Vec` is fresh (see
    /// [`drain_dropped_into`](Self::drain_dropped_into) for zero-alloc).
    /// At most [`max_receipts`](Self::max_receipts) IDs are retained — under
    /// a burst the oldest drop silently. Compare drained totals against
    /// [`dropped_count`](Self::dropped_count) to detect and quantify loss.
    #[allow(
        clippy::drain_collect,
        reason = "keep the host buffer allocation as promised by the public drain contract"
    )]
    pub fn take_dropped(&mut self) -> Vec<ToastId> {
        self.dropped.drain(..).collect()
    }

    /// Append buffered dropped IDs to `sink` and empty the host buffer.
    ///
    /// Retains the host allocation. No sink allocation is needed when its spare
    /// capacity covers the appended IDs; clear or reserve the sink between drains.
    pub fn drain_dropped_into(&mut self, sink: &mut Vec<ToastId>) {
        sink.append(&mut self.dropped);
    }

    /// Peek at the currently-buffered dropped ids without draining them.
    pub fn dropped(&self) -> &[ToastId] {
        &self.dropped
    }

    /// The lifetime total of ids dropped by budget/staleness enforcement.
    /// Never reset by drain calls.
    pub fn dropped_count(&self) -> u64 {
        self.dropped_count
    }

    /// Drain and return replaced IDs buffered since the last drain.
    ///
    /// Same allocation and loss contract as
    /// [`take_dropped`](Self::take_dropped); compare against
    /// [`replaced_count`](Self::replaced_count).
    #[allow(
        clippy::drain_collect,
        reason = "keep the host buffer allocation as promised by the public drain contract"
    )]
    pub fn take_replaced(&mut self) -> Vec<ToastId> {
        self.replaced.drain(..).collect()
    }

    /// Append buffered replaced IDs to `sink` and empty the host buffer.
    ///
    /// Retains the host allocation. No sink allocation is needed when its spare
    /// capacity covers the appended IDs; clear or reserve the sink between drains.
    pub fn drain_replaced_into(&mut self, sink: &mut Vec<ToastId>) {
        sink.append(&mut self.replaced);
    }

    /// Peek at the currently-buffered replaced ids without draining them.
    pub fn replaced(&self) -> &[ToastId] {
        &self.replaced
    }

    /// The lifetime total of coalescing replacements. Never reset by
    /// drain calls.
    pub fn replaced_count(&self) -> u64 {
        self.replaced_count
    }

    /// The number of entries currently in the waiting region (behind
    /// `max_visible`). `len() - waiting_count()` is the visible count.
    pub fn waiting_count(&self) -> usize {
        self.queue.len() - self.waiting_start()
    }

    /// The total UTF-8 content length of waiting entries, excluding spare capacity.
    pub fn waiting_bytes(&self) -> usize {
        let start = self.waiting_start();
        self.queue[start..]
            .iter()
            .map(|e| e.spec.owned_bytes())
            .sum()
    }

    fn promote_visibility(&mut self, now: Instant) {
        let cap = self.max_visible;
        let mut newly_presented: Option<ToastId> = None;
        for entry in self.queue.iter_mut().take(cap) {
            if !entry.presented {
                entry.presented = true;
                newly_presented = Some(entry.id);
            }
            // A sticky toast (an error, by default) has no deadline: it stays
            // until the user dismisses it.
            if entry.deadline.is_none()
                && entry.paused_remaining.is_none()
                && !entry.spec.is_sticky()
            {
                let duration = entry.spec.duration.unwrap_or(self.default_duration);
                // Unrepresentable dwell disables expiry rather than panicking.
                entry.deadline = enzui_core::monotonic_ms::checked_instant_deadline(
                    now,
                    duration,
                    enzui_core::monotonic_ms::Policy::Clamp,
                );
            }
        }
        if let Some(id) = newly_presented {
            self.announced = Some(id);
        }
    }

    // Surviving older entries must not inherit the removed subject and re-announce.
    fn reconcile_announcement(&mut self) {
        if let Some(id) = self.announced
            && !self.queue.iter().any(|e| e.id == id)
        {
            self.announced = None;
        }
    }

    fn announced_entry(&self) -> Option<&ToastEntry> {
        self.announced
            .and_then(|id| self.queue.iter().find(|e| e.id == id))
    }

    /// Remove a queued toast by ID, returning whether it existed.
    ///
    /// Does not initialize dwell or announcements for entries exposed by removal.
    /// Call [`Self::tick_at`] before syncing timers to initialize those entries.
    pub fn dismiss(&mut self, id: ToastId) -> bool {
        if let Some(pos) = self.queue.iter().position(|e| e.id == id) {
            self.queue.remove(pos);
            self.reconcile_announcement();
            true
        } else {
            false
        }
    }

    /// The number of toasts currently in the queue (visible + waiting).
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// The ids of all toasts currently in the queue, in order.
    pub fn ids(&self) -> Vec<ToastId> {
        self.queue.iter().map(|e| e.id).collect()
    }

    /// The ids of toasts that are currently visible (up to `max_visible`).
    pub fn visible_ids(&self) -> Vec<ToastId> {
        self.queue
            .iter()
            .take(self.max_visible)
            .map(|e| e.id)
            .collect()
    }

    /// The dwell deadline, or `None` for unknown, uninitialized, paused, or
    /// unrepresentable-deadline entries.
    ///
    /// [`Self::ids`] identifies known entries; [`Self::is_paused`] identifies
    /// paused ones. These accessors do not distinguish the other absent states.
    pub fn deadline(&self, id: ToastId) -> Option<Instant> {
        self.queue
            .iter()
            .find(|e| e.id == id)
            .and_then(|e| e.deadline)
    }

    /// The earliest live dwell deadline within the current visible window.
    ///
    /// Returns `None` when no visible entry has a deadline. Waiting-age limits
    /// are excluded; this accessor neither promotes entries nor starts a timer.
    pub fn next_expiry(&self) -> Option<Instant> {
        self.queue
            .iter()
            .take(self.max_visible)
            .filter_map(|e| e.deadline)
            .min()
    }

    /// The remaining paused time for a given toast, or `None` if the toast
    /// is unknown or not paused.
    pub fn paused_remaining(&self, id: ToastId) -> Option<Duration> {
        self.queue
            .iter()
            .find(|e| e.id == id)
            .and_then(|e| e.paused_remaining)
    }

    /// Whether the given toast is currently paused.
    pub fn is_paused(&self, id: ToastId) -> bool {
        self.paused_remaining(id).is_some()
    }

    /// Freeze remaining dwell at `now`, returning whether the entry became paused.
    ///
    /// Unknown, already paused, or deadline-less entries are unchanged. The view
    /// pauses on hover; apps must explicitly wire keyboard focus if needed.
    pub fn pause(&mut self, id: ToastId, now: Instant) -> bool {
        if let Some(entry) = self.queue.iter_mut().find(|e| e.id == id) {
            if entry.paused_remaining.is_some() {
                return false;
            }
            if let Some(deadline) = entry.deadline.take() {
                let remaining = deadline.saturating_duration_since(now);
                entry.paused_remaining = Some(remaining);
                return true;
            }
        }
        false
    }

    /// Restore paused dwell at `now`, returning whether the entry was paused.
    ///
    /// Unknown or unpaused entries are unchanged. If `now + remaining` is
    /// unrepresentable, the entry resumes without an automatic expiry deadline.
    pub fn resume(&mut self, id: ToastId, now: Instant) -> bool {
        if let Some(entry) = self.queue.iter_mut().find(|e| e.id == id)
            && let Some(remaining) = entry.paused_remaining.take()
        {
            entry.deadline = enzui_core::monotonic_ms::checked_instant_deadline(
                now,
                remaining,
                enzui_core::monotonic_ms::Policy::Clamp,
            );
            return true;
        }
        false
    }

    /// Process a view or timer event and return its dismissal or action receipts.
    ///
    /// `Dismiss` returns its ID even if already absent. `Action` removes and returns
    /// a queued ID only when its nonempty label matches exactly; replay is ignored.
    /// `Tick` returns expired IDs. Pause and resume events return no IDs.
    /// Manual dismissal and actions do not initialize newly exposed entries;
    /// call [`Self::tick_at`] before syncing timers for those entries.
    pub fn update(&mut self, event: ToastEvent) -> Vec<ToastId> {
        match event {
            ToastEvent::Tick(now) => self.tick_at(now),
            ToastEvent::Dismiss(id) => {
                self.dismiss(id);
                vec![id]
            }
            ToastEvent::Action(id, label) => {
                let resolves = self
                    .queue
                    .iter()
                    .find(|e| e.id == id)
                    .is_some_and(|e| !label.is_empty() && e.spec.action_label == label);
                if resolves {
                    self.dismiss(id);
                    vec![id]
                } else {
                    Vec::new()
                }
            }
            ToastEvent::Pause(id) => {
                self.pause(id, Instant::now());
                Vec::new()
            }
            ToastEvent::Resume(id) => {
                self.resume(id, Instant::now());
                Vec::new()
            }
        }
    }

    /// Sweep stale waiting entries, initialize visible dwell, and remove expired entries.
    ///
    /// Returns expired IDs; stale waiting IDs are recorded in [`Self::take_dropped`].
    /// Paused entries do not expire. Entries exposed by expiry initialize at `now`
    /// but are not checked again until the next tick, including zero-duration entries.
    pub fn tick_at(&mut self, now: Instant) -> Vec<ToastId> {
        // Sweep before promotion so stale waiting entries are never presented.
        self.sweep_stale_waiting(now);

        self.promote_visibility(now);

        let check_count = self.queue.len().min(self.max_visible);
        let mut dismissed = Vec::new();
        for entry in self.queue.iter().take(check_count) {
            if entry.paused_remaining.is_some() {
                continue;
            }
            if let Some(deadline) = entry.deadline
                && now >= deadline
            {
                dismissed.push(entry.id);
            }
        }

        for id in &dismissed {
            self.queue.retain(|e| e.id != *id);
        }
        self.reconcile_announcement();

        // Entries exposed by expiry start their full dwell at this tick.
        self.promote_visibility(now);

        dismissed
    }

    /// Reconcile the app-owned schedule with this host; equivalent to
    /// [`ToastExpirySchedule::sync`].
    pub fn sync_expiry(
        &self,
        schedule: &mut ToastExpirySchedule,
        now: Instant,
    ) -> Option<Duration> {
        schedule.sync(self, now)
    }

    /// Render the visible queue in order, with hover pause and resume events.
    ///
    /// Widget IDs derive from the host prefix and toast ID. Individual toast nodes
    /// are readable but silent. In live mode, a region with a stable derived ID
    /// carries the latest presented entry still queued, including its description;
    /// it is removed when that subject leaves. Changing the visible limit alone
    /// does not update the announcement subject.
    pub fn view(&self) -> Element<'_, ToastEvent> {
        self.view_with(|toast, _| toast)
    }

    /// Customize each visible toast without taking over queue timing or announcements.
    ///
    /// The callback receives the queued content and its configured toast builder.
    /// Resolve tokens from the current app theme here to customize typography,
    /// width or material independently of [`ToastSpec`]. It runs once per visible
    /// entry on each view build. The host assigns stable IDs and silent child
    /// announcements afterward, and retains hover pause/resume. Preserve the
    /// supplied builder's dismiss/action messages when customizing it.
    /// Keyboard focus pause/resume still requires the app's explicit events.
    pub fn view_with<'a>(
        &'a self,
        mut present: impl FnMut(EnzToast<'a, ToastEvent>, &'a ToastSpec) -> EnzToast<'a, ToastEvent>,
    ) -> Element<'a, ToastEvent> {
        let visible = self.queue.iter().take(self.max_visible);
        let mut children: Vec<(String, Element<'_, ToastEvent>)> = Vec::new();

        for entry in visible {
            let id = entry.id;
            let key = format!("{}", id.raw());
            let id_string = format!("{}-{}", self.id_prefix, key);

            let mut toast = enz_toast(&entry.spec.text, ToastEvent::Dismiss(id))
                .tone(entry.spec.tone)
                .size(entry.spec.size)
                .show_icon(entry.spec.show_icon);

            if !entry.spec.description.is_empty() {
                toast = toast.description(&entry.spec.description);
            }

            if !entry.spec.action_label.is_empty() {
                let label = entry.spec.action_label.clone();
                toast = toast.action(&entry.spec.action_label, ToastEvent::Action(id, label));
            }

            let toast = present(toast, &entry.spec)
                .id(id_string)
                .announcement(Announcement::Silent);
            let hover_aware = iced::widget::mouse_area(toast)
                .on_enter(ToastEvent::Pause(id))
                .on_exit(ToastEvent::Resume(id));

            // A new toast rises into the stack and fades in.
            let entering = crate::motion::animated(hover_aware)
                .enters_from(iced::Vector::new(0.0, TOAST_RISE), 0.0);
            children.push((key, entering.into()));
        }

        let stack = crate::keyed_row::keyed_column(children)
            .spacing(8.0)
            .width(Length::Shrink);

        match (self.announcements, self.announced_entry()) {
            (ToastAnnouncements::Decorative, _) => stack.into(),
            (ToastAnnouncements::Live, Some(entry)) => {
                let semantics = entry
                    .spec
                    .tone
                    .announcement()
                    .region(announcement_message(&entry.spec));
                crate::semantics_container::SemanticsContainer::new(stack, semantics)
                    .id(enzui_core::identity::derive_id(
                        &self.id_prefix,
                        "live-region",
                    ))
                    .into()
            }
            (ToastAnnouncements::Live, None) => stack.into(),
        }
    }
}

/// How far below its place a new toast starts, in logical pixels.
const TOAST_RISE: f32 = 16.0;

fn announcement_message(spec: &ToastSpec) -> String {
    if spec.description.trim().is_empty() {
        spec.text.clone()
    } else {
        format!("{}. {}", spec.text.trim_end_matches('.'), spec.description)
    }
}

const EDGE_GUTTER: f32 = 16.0;

/// Where a mounted [`ToastHost`] sits over an app's presentation.
///
/// A toast floats one gutter (16 px, the margin it keeps from the sides)
/// above the bottom edge, in the trailing (bottom-right) corner by default,
/// where desktop notifications appear and where it covers the least of the
/// content. [`centred`](Self::centred) puts it in the middle instead. An app painting its own chrome there passes
/// that height to [`bottom_inset`](Self::bottom_inset), and the toast floats
/// the same gutter above the chrome, clear of its controls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToastOverlayPlacement {
    bottom_inset: FiniteNonNeg,
    trailing: bool,
}

impl Default for ToastOverlayPlacement {
    /// Returns [`ToastOverlayPlacement::FLUSH`].
    fn default() -> Self {
        Self::FLUSH
    }
}

impl ToastOverlayPlacement {
    /// No bottom chrome: the toast floats one gutter above the window's
    /// bottom edge, in the trailing corner.
    pub const FLUSH: Self = Self {
        bottom_inset: FiniteNonNeg::ZERO,
        trailing: true,
    };

    /// Place the overlay at the bottom-trailing corner (the default); dwell is
    /// unchanged.
    #[must_use]
    pub fn trailing(mut self) -> Self {
        self.trailing = true;
        self
    }

    /// Centre the overlay horizontally instead of the trailing corner.
    #[must_use]
    pub fn centred(mut self) -> Self {
        self.trailing = false;
        self
    }

    /// Whether the toast floats in the trailing corner.
    #[must_use]
    pub fn is_trailing(self) -> bool {
        self.trailing
    }

    /// Reserve `px` logical pixels below the overlay, in addition to its gutter.
    ///
    /// Zero is literal; negative or non-finite inputs normalize to zero. Use the
    /// same chrome height as the presentation to keep spacing consistent.
    #[must_use]
    pub fn bottom_inset(mut self, px: f32) -> Self {
        self.bottom_inset = FiniteNonNeg::new_or(px, 0.0);
        self
    }

    /// The resolved bottom inset in logical pixels, always finite and `>= 0`.
    #[must_use]
    pub fn bottom(self) -> f32 {
        self.bottom_inset.get()
    }
}

/// Float toast content at the bottom of the available presentation space.
///
/// Adds a 16 logical pixel gutter and the placement inset. Stack above the
/// outermost presentation, outside modal inert layers, so toast semantics stay
/// reachable. Prefer [`enz_toast_layer`] for a stable presentation mount.
pub fn enz_toast_overlay<'a, Message: 'a>(
    host_view: impl Into<Element<'a, Message>>,
    placement: ToastOverlayPlacement,
) -> Element<'a, Message> {
    container(host_view.into())
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(if placement.trailing {
            iced::Alignment::End
        } else {
            iced::Alignment::Center
        })
        .align_y(iced::alignment::Vertical::Bottom)
        .padding(Padding {
            top: 0.0,
            right: EDGE_GUTTER,
            bottom: placement.bottom() + EDGE_GUTTER,
            left: EDGE_GUTTER,
        })
        .into()
}

/// Stack optional toasts over a presentation with a stable first-child mount.
///
/// The presentation always occupies the first stack slot. Toast arrival or
/// removal does not re-parent it; compatible widget identities retain state.
/// Pass `None` when no toast overlay is needed.
pub fn enz_toast_layer<'a, Message: 'a>(
    presentation: impl Into<Element<'a, Message>>,
    toasts: Option<Element<'a, Message>>,
    placement: ToastOverlayPlacement,
) -> Element<'a, Message> {
    let overlay = toasts.map(|view| enz_toast_overlay(view, placement));
    iced::widget::Stack::with_children(std::iter::once(presentation.into()).chain(overlay))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn cap_receipt_buffer(buf: &mut Vec<ToastId>, cap: usize) {
    if buf.len() > cap {
        let excess = buf.len() - cap;
        buf.drain(..excess);
    }
}

fn cap_and_shrink_receipt_buffer(buf: &mut Vec<ToastId>, cap: usize) {
    cap_receipt_buffer(buf, cap);
    if buf.capacity() > cap.saturating_mul(2) {
        buf.shrink_to(cap);
    }
}
