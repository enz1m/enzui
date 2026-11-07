//! Per-window diagnostic scan scheduling and finding confirmation.
//!
//! Layout invalidations are coalesced until [`SETTLE_FRAMES`] quiet ticks. Findings are
//! confirmed only after two consecutive scans, and callers must pair a due scan with
//! [`DiagnosticsHost::record_scan`].

use crate::diagnostics_sink::DiagnosticsSink;
use iced_core::widget::Id;
use std::collections::HashSet;

// Global so bounds and accessibility collectors contribute to one scan-cost total.

/// Counts bounds and accessibility collector node visits.
#[doc(hidden)]
pub static NODE_VISITS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Read [`NODE_VISITS`].
#[doc(hidden)]
pub fn node_visits() -> usize {
    NODE_VISITS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Reset [`NODE_VISITS`] to zero and return the previous value.
#[doc(hidden)]
pub fn reset_node_visits() -> usize {
    NODE_VISITS.swap(0, std::sync::atomic::Ordering::Relaxed)
}

/// A type-erased identity for matching the same finding across consecutive scans.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FindingKey {
    id: Option<Id>,
    discriminant: &'static str,
}

impl FindingKey {
    /// Creates a key; `discriminant` must be stable for the same finding kind across scans.
    pub fn new(id: Option<Id>, discriminant: &'static str) -> Self {
        Self { id, discriminant }
    }
}

/// Confirmed findings together with the current scan’s subject and finding counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfirmedFindings {
    confirmed: HashSet<FindingKey>,
    subjects_examined: usize,
    scan_findings: usize,
}

impl ConfirmedFindings {
    /// The confirmed keys: present in both the previous and the current scan.
    pub fn keys(&self) -> &HashSet<FindingKey> {
        &self.confirmed
    }

    /// Consume `self` and return the confirmed keys.
    pub fn into_keys(self) -> HashSet<FindingKey> {
        self.confirmed
    }

    /// Number of confirmed findings.
    pub fn len(&self) -> usize {
        self.confirmed.len()
    }

    /// Returns whether no finding was confirmed; this does not imply [`Self::is_clean`].
    pub fn is_empty(&self) -> bool {
        self.confirmed.is_empty()
    }

    /// Whether a specific key was confirmed.
    pub fn contains(&self, key: &FindingKey) -> bool {
        self.confirmed.contains(key)
    }

    /// Iterate the confirmed keys.
    pub fn iter(&self) -> impl Iterator<Item = &FindingKey> {
        self.confirmed.iter()
    }

    /// How many subjects the scan behind this confirmation examined.
    pub fn subjects_examined(&self) -> usize {
        self.subjects_examined
    }

    /// How many findings the current scan produced, before confirmation.
    pub fn scan_findings(&self) -> usize {
        self.scan_findings
    }

    /// Returns whether the scan examined zero subjects.
    pub fn examined_nothing(&self) -> bool {
        self.subjects_examined == 0
    }

    /// Returns true only when subjects were examined and the scan found nothing.
    pub fn is_clean(&self) -> bool {
        self.subjects_examined > 0 && self.scan_findings == 0 && self.confirmed.is_empty()
    }

    /// Renders the confirmation state and subject coverage as one line.
    pub fn render(&self) -> String {
        if self.examined_nothing() {
            return "NO SUBJECTS SCANNED (0 examined) - this confirmation could not have failed"
                .to_owned();
        }
        if self.is_clean() {
            return format!("clean ({} subject(s) examined)", self.subjects_examined);
        }
        if self.confirmed.is_empty() {
            return format!(
                "baseline: {} unconfirmed finding(s) ({} subject(s) examined)",
                self.scan_findings, self.subjects_examined,
            );
        }
        format!(
            "{} confirmed finding(s) of {} ({} subject(s) examined)",
            self.confirmed.len(),
            self.scan_findings,
            self.subjects_examined,
        )
    }
}

/// How many consecutive ticks with no layout change before a scan fires.
/// Effectively a debounce: resize drags produce many intermediate layouts,
/// and scanning each one is wasteful and noisy.
pub const SETTLE_FRAMES: u32 = 2;

/// Per-window owner of a diagnostic sink and scan-coalescing state.
pub struct DiagnosticsHost {
    sink: DiagnosticsSink,
    quiet_ticks: u32,
    dirty: bool,
    scan_pending: bool,
    scans_completed: u32,
    prev_scan_keys: HashSet<FindingKey>,
}

impl std::fmt::Debug for DiagnosticsHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiagnosticsHost")
            .field("enabled", &self.sink.is_enabled())
            .field("dirty", &self.dirty)
            .field("quiet_ticks", &self.quiet_ticks)
            .field("scans_completed", &self.scans_completed)
            .finish()
    }
}

impl DiagnosticsHost {
    /// Create a new host with the given sink.
    pub fn new(sink: DiagnosticsSink) -> Self {
        Self {
            sink,
            quiet_ticks: 0,
            dirty: false,
            scan_pending: false,
            scans_completed: 0,
            prev_scan_keys: HashSet::new(),
        }
    }

    /// Create a disabled host (no sink, no scanning).
    pub fn disabled() -> Self {
        Self::new(DiagnosticsSink::disabled())
    }

    /// Whether the host is enabled (has a real sink).
    pub fn is_enabled(&self) -> bool {
        self.sink.is_enabled()
    }

    /// Returns the underlying diagnostics sink.
    pub fn sink(&self) -> &DiagnosticsSink {
        &self.sink
    }

    /// Records a layout invalidation and restarts the settle timer.
    pub fn notify_layout_changed(&mut self) {
        if !self.sink.is_enabled() {
            return;
        }
        self.dirty = true;
        self.quiet_ticks = 0;
    }

    /// Advances the settle timer once and returns true when a scan becomes due. Call once per
    /// frame or redraw.
    pub fn tick(&mut self) -> bool {
        if !self.sink.is_enabled() || !self.dirty {
            return false;
        }
        self.quiet_ticks = self.quiet_ticks.saturating_add(1);
        if self.quiet_ticks >= SETTLE_FRAMES && !self.scan_pending {
            self.scan_pending = true;
            return true;
        }
        false
    }

    /// Returns whether a scan is pending; remains true until [`Self::record_scan`] is called.
    pub fn should_scan(&self) -> bool {
        self.scan_pending
    }

    /// Confirms keys present in both the previous and current scan.
    ///
    /// `subjects_examined` must report actual coverage so an empty result can be distinguished
    /// from a blind scan.
    pub fn confirm_findings(
        &mut self,
        current_keys: HashSet<FindingKey>,
        subjects_examined: usize,
    ) -> ConfirmedFindings {
        let confirmed: HashSet<FindingKey> = current_keys
            .intersection(&self.prev_scan_keys)
            .cloned()
            .collect();
        let scan_findings = current_keys.len();
        self.prev_scan_keys = current_keys;
        ConfirmedFindings {
            confirmed,
            subjects_examined,
            scan_findings,
        }
    }

    /// Marks the pending scan complete and clears its invalidation state.
    pub fn record_scan(&mut self) {
        self.scan_pending = false;
        self.dirty = false;
        self.quiet_ticks = 0;
        self.scans_completed += 1;
    }

    /// Total number of scans completed since construction.
    pub fn scans_completed(&self) -> u32 {
        self.scans_completed
    }

    /// Returns mutable access to the underlying sink.
    pub fn sink_mut(&mut self) -> &mut DiagnosticsSink {
        &mut self.sink
    }
}
