//! Diagnostic scan summaries and opt-in report sinks.
//!
//! Summaries preserve retained, omitted, clipped, and examined counts. A scan that examined
//! no subjects is not clean, and a disabled sink does not invoke its summary closure.

use crate::accessibility_scan::{
    AccessibilityDefect, AccessibilityDefectKind, AccessibilityScanResult,
};
use crate::layout_spill_scan::{LayoutSpillFinding, LayoutSpillScanResult};
use crate::registered_overlap_scan::{RegisteredOverlapFinding, RegisteredOverlapScanResult};
use iced_core::Rectangle;

/// Maximum retained findings expanded into detail lines; additional findings are reported
/// as suppressed.
pub const DETAIL_LINE_CAP: usize = 32;

const DETAIL_PREFIX: &str = "[enzui-diagnostics-detail]";

/// Identifies the audit that produced a summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanKind {
    /// An accessibility audit.
    Accessibility,
    /// A layout-spill audit.
    LayoutSpill,
    /// A registered-overlap audit.
    RegisteredOverlap,
}

impl ScanKind {
    /// A short stable label for reports.
    pub fn label(self) -> &'static str {
        match self {
            ScanKind::Accessibility => "a11y",
            ScanKind::LayoutSpill => "bounds",
            ScanKind::RegisteredOverlap => "overlap",
        }
    }
}

/// A scan verdict with separate retained, omitted, clipped, and examined counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanSummary {
    /// Which audit this came from.
    pub kind: ScanKind,
    /// Findings retained and reportable.
    pub retained: usize,
    /// Findings discovered but omitted after the reporting cap.
    pub overflowed: usize,
    /// Findings discovered but fully outside the viewport.
    pub clipped_out_of_view: usize,
    /// Registered subjects actually examined by the scan.
    pub subjects_examined: usize,
}

impl ScanSummary {
    /// Total defects discovered, including omitted and clipped.
    pub fn total(&self) -> usize {
        self.retained + self.overflowed + self.clipped_out_of_view
    }

    /// Returns true only when at least one subject was examined and no defects were found.
    pub fn is_clean(&self) -> bool {
        self.subjects_examined > 0 && self.total() == 0
    }

    /// Returns whether the scan examined no registered subjects.
    pub fn examined_nothing(&self) -> bool {
        self.subjects_examined == 0
    }

    /// True when the retained list does not tell the whole story.
    pub fn is_truncated(&self) -> bool {
        self.overflowed > 0 || self.clipped_out_of_view > 0
    }

    /// Renders the summary as one JSON object suitable for an NDJSON line.
    pub fn render_ndjson(&self) -> String {
        format!(
            r#"{{"kind":"{}","retained":{},"overflowed":{},"clipped":{},"total":{},"examined":{},"clean":{},"blind":{}}}"#,
            self.kind.label(),
            self.retained,
            self.overflowed,
            self.clipped_out_of_view,
            self.total(),
            self.subjects_examined,
            self.is_clean(),
            self.examined_nothing(),
        )
    }

    /// Returns whether the audit failed; blind scans count as failures.
    pub fn is_failure(&self) -> bool {
        !self.is_clean()
    }

    /// Renders a one-line human report including subject coverage and any truncation.
    pub fn render(&self) -> String {
        if self.examined_nothing() && self.total() == 0 {
            return format!(
                "[enzui-diagnostics] {}: NO REGISTERED SUBJECTS (0 examined) - this walk could not have failed",
                self.kind.label()
            );
        }
        if self.is_clean() {
            return format!(
                "[enzui-diagnostics] {}: clean (complete walk, {} subject(s) examined)",
                self.kind.label(),
                self.subjects_examined,
            );
        }
        if self.is_truncated() {
            format!(
                "[enzui-diagnostics] {}: {} finding(s) - {} shown, {} over cap, {} scrolled out of view ({} subject(s) examined)",
                self.kind.label(),
                self.total(),
                self.retained,
                self.overflowed,
                self.clipped_out_of_view,
                self.subjects_examined,
            )
        } else {
            format!(
                "[enzui-diagnostics] {}: {} finding(s) (complete walk, {} subject(s) examined)",
                self.kind.label(),
                self.total(),
                self.subjects_examined,
            )
        }
    }
}

impl From<&AccessibilityScanResult> for ScanSummary {
    fn from(r: &AccessibilityScanResult) -> Self {
        Self {
            kind: ScanKind::Accessibility,
            retained: r.findings.len(),
            overflowed: r.overflowed,
            clipped_out_of_view: r.clipped_out_of_view,
            subjects_examined: r.subjects_examined,
        }
    }
}

impl From<&LayoutSpillScanResult> for ScanSummary {
    fn from(r: &LayoutSpillScanResult) -> Self {
        Self {
            kind: ScanKind::LayoutSpill,
            retained: r.findings.len(),
            overflowed: r.overflowed,
            clipped_out_of_view: r.clipped_out_of_view,
            subjects_examined: r.subjects_examined,
        }
    }
}

impl From<&RegisteredOverlapScanResult> for ScanSummary {
    fn from(r: &RegisteredOverlapScanResult) -> Self {
        Self {
            kind: ScanKind::RegisteredOverlap,
            retained: r.findings.len(),
            overflowed: r.overflowed,
            clipped_out_of_view: r.clipped_out_of_view,
            subjects_examined: r.subjects_examined,
        }
    }
}

/// Destination for rendered [`DiagnosticsSink`] report lines.
pub trait SinkWriter: Send + Sync {
    /// Emit one already-rendered report line.
    fn write_line(&self, line: &str);
}

/// Counts opted-in summary closures invoked by [`DiagnosticsSink::report`].
#[doc(hidden)]
pub static SUMMARIES_BUILT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Read [`SUMMARIES_BUILT`].
#[doc(hidden)]
pub fn summaries_built() -> usize {
    SUMMARIES_BUILT.load(std::sync::atomic::Ordering::Relaxed)
}

/// Reset [`SUMMARIES_BUILT`] to zero and return its previous value.
#[doc(hidden)]
pub fn reset_summaries_built() -> usize {
    SUMMARIES_BUILT.swap(0, std::sync::atomic::Ordering::Relaxed)
}

/// Wraps a summary closure so invocation increments [`SUMMARIES_BUILT`].
#[doc(hidden)]
pub fn counted_summary(f: impl FnOnce() -> ScanSummary) -> impl FnOnce() -> ScanSummary {
    move || {
        SUMMARIES_BUILT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        f()
    }
}

/// The real sink target: the `log` facade at info under
/// `enzui::trace::diagnostics`, which the default logger shows.
#[derive(Debug, Default, Clone, Copy)]
pub struct LogWriter;

impl SinkWriter for LogWriter {
    fn write_line(&self, line: &str) {
        log::info!(target: "enzui::trace::diagnostics", "{line}");
    }
}

/// A ring-buffer writer retaining the newest report lines.
///
/// Positive capacities retain at most `cap` lines; a zero capacity retains the latest line.
pub struct BoundedSnapshotWriter {
    lines: std::sync::Mutex<std::collections::VecDeque<String>>,
    cap: usize,
}

impl BoundedSnapshotWriter {
    /// Create a new bounded-snapshot writer with the given capacity.
    pub fn new(cap: usize) -> Self {
        Self {
            lines: std::sync::Mutex::new(std::collections::VecDeque::with_capacity(cap)),
            cap,
        }
    }

    /// Returns retained lines from oldest to newest.
    pub fn lines(&self) -> Vec<String> {
        self.lines
            .lock()
            .map(|lines| lines.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The capacity of this snapshot.
    pub fn cap(&self) -> usize {
        self.cap
    }
}

impl std::fmt::Debug for BoundedSnapshotWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let len = self.lines.lock().map_or(0, |g| g.len());
        f.debug_struct("BoundedSnapshotWriter")
            .field("cap", &self.cap)
            .field("len", &len)
            .finish()
    }
}

impl SinkWriter for BoundedSnapshotWriter {
    fn write_line(&self, line: &str) {
        // Diagnostics are best-effort. A prior writer panic poisons this
        // snapshot; dropping this report is safer than unwinding through the
        // caller (which may be a renderer or platform callback).
        let Ok(mut buf) = self.lines.lock() else {
            return;
        };
        if buf.len() >= self.cap {
            buf.pop_front();
        }
        buf.push_back(line.to_owned());
    }
}

/// Output format used by [`DiagnosticsSink`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReportFormat {
    /// Human-readable one-line output and the default format.
    #[default]
    Human,
    /// Newline-delimited JSON output.
    Ndjson,
}

/// An opt-in audit reporter that is disabled by default.
pub struct DiagnosticsSink {
    writer: Option<Box<dyn SinkWriter>>,
    format: ReportFormat,
}

impl std::fmt::Debug for DiagnosticsSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiagnosticsSink")
            .field("enabled", &self.is_enabled())
            .finish()
    }
}

impl Default for DiagnosticsSink {
    fn default() -> Self {
        Self::disabled()
    }
}

impl DiagnosticsSink {
    /// Creates a disabled sink that does not invoke report closures.
    pub fn disabled() -> Self {
        Self {
            writer: None,
            format: ReportFormat::Human,
        }
    }

    /// A sink writing rendered lines to the process's stderr.
    pub fn to_stderr() -> Self {
        Self {
            writer: Some(Box::new(LogWriter)),
            format: ReportFormat::Human,
        }
    }

    /// Creates a human-format sink writing to `writer`.
    pub fn to_writer(writer: Box<dyn SinkWriter>) -> Self {
        Self {
            writer: Some(writer),
            format: ReportFormat::Human,
        }
    }

    /// Creates a sink that writes one NDJSON object per line to stderr.
    pub fn to_ndjson_stderr() -> Self {
        Self {
            writer: Some(Box::new(LogWriter)),
            format: ReportFormat::Ndjson,
        }
    }

    /// Creates an NDJSON sink writing to `writer`.
    pub fn to_ndjson_writer(writer: Box<dyn SinkWriter>) -> Self {
        Self {
            writer: Some(writer),
            format: ReportFormat::Ndjson,
        }
    }

    /// Returns whether the sink has an output writer.
    pub fn is_enabled(&self) -> bool {
        self.writer.is_some()
    }

    /// The report format this sink uses.
    pub fn format(&self) -> ReportFormat {
        self.format
    }

    /// Reports a summary when enabled without invoking `summarize` when disabled.
    pub fn report(&self, summarize: impl FnOnce() -> ScanSummary) {
        let Some(writer) = self.writer.as_ref() else {
            return;
        };
        self.write_summary(writer.as_ref(), summarize());
    }

    /// Reports an accessibility summary and bounded finding details.
    pub fn report_a11y(&self, result: &AccessibilityScanResult) {
        let Some(writer) = self.writer.as_ref() else {
            return;
        };
        self.write_summary(writer.as_ref(), ScanSummary::from(result));
        for (index, finding) in result.findings.iter().take(DETAIL_LINE_CAP).enumerate() {
            writer.write_line(&self.render_a11y_detail(index + 1, finding));
        }
        self.write_suppressed(
            writer.as_ref(),
            ScanKind::Accessibility,
            result.findings.len(),
        );
    }

    /// Reports a layout-bounds summary and bounded finding details.
    pub fn report_bounds(&self, result: &LayoutSpillScanResult) {
        let Some(writer) = self.writer.as_ref() else {
            return;
        };
        self.write_summary(writer.as_ref(), ScanSummary::from(result));
        for (index, finding) in result.findings.iter().take(DETAIL_LINE_CAP).enumerate() {
            writer.write_line(&self.render_bounds_detail(index + 1, finding));
        }
        self.write_suppressed(
            writer.as_ref(),
            ScanKind::LayoutSpill,
            result.findings.len(),
        );
    }

    /// Reports an overlap summary and bounded finding details.
    pub fn report_overlap(&self, result: &RegisteredOverlapScanResult) {
        let Some(writer) = self.writer.as_ref() else {
            return;
        };
        self.write_summary(writer.as_ref(), ScanSummary::from(result));
        for (index, finding) in result.findings.iter().take(DETAIL_LINE_CAP).enumerate() {
            writer.write_line(&self.render_overlap_detail(index + 1, finding));
        }
        self.write_suppressed(
            writer.as_ref(),
            ScanKind::RegisteredOverlap,
            result.findings.len(),
        );
    }

    fn write_summary(&self, writer: &dyn SinkWriter, summary: ScanSummary) {
        let line = match self.format {
            ReportFormat::Human => summary.render(),
            ReportFormat::Ndjson => summary.render_ndjson(),
        };
        writer.write_line(&line);
    }

    fn write_suppressed(&self, writer: &dyn SinkWriter, kind: ScanKind, retained: usize) {
        let suppressed = retained.saturating_sub(DETAIL_LINE_CAP);
        if suppressed == 0 {
            return;
        }
        let line = match self.format {
            ReportFormat::Human => format!(
                "{DETAIL_PREFIX} {}: {suppressed} retained finding detail(s) suppressed by detail cap ({DETAIL_LINE_CAP} shown of {retained} retained)",
                kind.label(),
            ),
            ReportFormat::Ndjson => format!(
                r#"{{"kind":"{}","record":"detail-suppression","suppressed":{},"shown":{},"retained":{}}}"#,
                kind.label(),
                suppressed,
                DETAIL_LINE_CAP,
                retained,
            ),
        };
        writer.write_line(&line);
    }

    fn render_a11y_detail(&self, index: usize, finding: &AccessibilityDefect) -> String {
        let id = finding_id(finding.id.as_ref());
        let bounds = rectangle(finding.bounds);
        let defect = match finding.kind {
            AccessibilityDefectKind::TextElision { clamped_by } => {
                format!("kind=text-elision clamped_by={clamped_by}")
            }
            AccessibilityDefectKind::UndersizedTarget { deficit } => {
                format!("kind=undersized-target deficit={deficit:.3}px")
            }
            AccessibilityDefectKind::LowContrast {
                ratio,
                min_ratio,
                status,
            } => format!(
                "kind=low-contrast ratio={ratio:.3}:1 minimum={min_ratio:.3}:1 status={}",
                status.label()
            ),
            AccessibilityDefectKind::UnresolvedPaint { status } => {
                format!(
                    "kind=unresolved-paint magnitude=unmeasurable status={}",
                    status.label()
                )
            }
            AccessibilityDefectKind::MissingName => {
                "kind=missing-name magnitude=no-explicit-accessible-name".to_owned()
            }
        };
        self.render_detail(
            ScanKind::Accessibility,
            index,
            &format!(
                "{defect} subject={id} bounds={bounds} depth={}",
                finding.depth
            ),
        )
    }

    fn render_bounds_detail(&self, index: usize, finding: &LayoutSpillFinding) -> String {
        self.render_detail(
            ScanKind::LayoutSpill,
            index,
            &format!(
                "kind=layout-spill spill={:.3}px subject={} child_bounds={} container_bounds={} depth={}",
                finding.spill_amount,
                finding_id(finding.id.as_ref()),
                rectangle(finding.child),
                rectangle(finding.container),
                finding.depth,
            ),
        )
    }

    fn render_overlap_detail(&self, index: usize, finding: &RegisteredOverlapFinding) -> String {
        self.render_detail(
            ScanKind::RegisteredOverlap,
            index,
            &format!(
                "kind=overlap intersection={} subject_a={} bounds_a={} subject_b={} bounds_b={} depth={}",
                rectangle(finding.intersection),
                finding_id(finding.id_a.as_ref()),
                rectangle(finding.bounds_a),
                finding_id(finding.id_b.as_ref()),
                rectangle(finding.bounds_b),
                finding.depth,
            ),
        )
    }

    fn render_detail(&self, kind: ScanKind, index: usize, fields: &str) -> String {
        match self.format {
            ReportFormat::Human => {
                format!(
                    "{DETAIL_PREFIX} {}: finding {index}: {fields}",
                    kind.label()
                )
            }
            ReportFormat::Ndjson => format!(
                r#"{{"kind":"{}","record":"finding","index":{},"detail":"{}"}}"#,
                kind.label(),
                index,
                escape_json(fields),
            ),
        }
    }
}

fn finding_id(id: Option<&iced_core::widget::Id>) -> String {
    id.map_or_else(
        || "None (anonymous subject)".to_owned(),
        |id| format!("{id:?}"),
    )
}

fn rectangle(bounds: Rectangle) -> String {
    format!(
        "(x={:.3},y={:.3},width={:.3},height={:.3})",
        bounds.x, bounds.y, bounds.width, bounds.height,
    )
}

fn escape_json(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write;
                let _ = write!(escaped, "\\u{:04x}", character as u32);
            }
            character => escaped.push(character),
        }
    }
    escaped
}
