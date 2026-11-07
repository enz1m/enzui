//! Repeatable performance reports for scripted application workloads.
//!
//! Reports combine paint and layout histograms with optional runtime-observer
//! data. Comparisons reject underpowered, lossy, corrupt, or incompatible runs.
//! Offscreen samples are submitted-only, and visible latency requires one
//! homogeneous presented quality. The `perf-report` feature is intended only
//! for performance measurement.

use core::time::Duration;
use std::fmt::{self, Write as _};
use std::io::Read as _;
use std::time::Instant;

use sha2::{Digest as _, Sha256};

use iced_core::Size;
use iced_runtime::performance::{
    HdrHistogram, HistogramSnapshot, PresentationQuality, Stage, WindowSnapshot,
};

use crate::perf_frame::{
    PerfHarness, PerfSubject, RuntimeRedrawRequest, page_builds, reset_page_builds,
};

/// Minimum measured-frame count required by [`compare`].
pub const MIN_COMPARABLE_FRAMES: u64 = 30;

/// Maximum duration of one [`Step::Wait`]; longer delays must use multiple steps.
pub const MAX_STEP_WAIT: Duration = Duration::from_millis(250);

/// Maximum total [`Step::Wait`] duration in one workload.
pub const MAX_TOTAL_WAIT: Duration = Duration::from_secs(5);

/// Computes a lowercase length-framed SHA-256 digest over ordered byte slices.
///
/// Length framing preserves slice boundaries, so `[b"ab", b"c"]` differs from
/// `[b"a", b"bc"]`.
#[must_use]
pub fn content_sha256(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    format!("{:x}", hasher.finalize())
}

/// The four interaction kinds covered by apps workload report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkloadKind {
    /// Switch the routed page / settings pane repeatedly.
    PageSwitch,
    /// Toggle a sidebar / navigation rail (the animated collapse/expand).
    SidebarToggle,
    /// Scroll a large, busy virtualised list or grid.
    BusyScroll,
    /// Measures the steady-state redraw floor without changing state.
    Idle,
}

impl WorkloadKind {
    /// A stable machine-readable name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PageSwitch => "page_switch",
            Self::SidebarToggle => "sidebar_toggle",
            Self::BusyScroll => "busy_scroll",
            Self::Idle => "idle",
        }
    }
}

/// Identifies an application interaction and the dataset that determines its weight.
/// Reports are comparable only when their workload identities match.
#[derive(Debug, Clone)]
pub struct WorkloadIdentity {
    /// App + interaction, human readable, e.g. `"an app busy sidebar"`.
    pub name: String,
    /// The interaction class.
    pub kind: WorkloadKind,
    /// The dataset / fixture size that gives the workload its weight.
    pub dataset: String,
}

impl WorkloadIdentity {
    /// Build an identity.
    pub fn new(name: impl Into<String>, kind: WorkloadKind, dataset: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind,
            dataset: dataset.into(),
        }
    }
}

/// One step of a workload.
#[derive(Debug, Clone)]
pub enum Step<M> {
    /// Applies a message without measuring a frame.
    Drive(M),
    /// Advances real time between measured frames without charging the delay to a
    /// runtime stage.
    ///
    /// A wait is limited by [`MAX_STEP_WAIT`], all waits by [`MAX_TOTAL_WAIT`], and
    /// [`Workload::validate_waits`] checks the complete script before sleeping. Waits
    /// are outside paint/layout histograms but inside observer frame cadence.
    Wait(Duration),
    /// Optionally apply a message, then measure exactly one paint frame under
    /// `label`.
    Frame {
        /// The message applied immediately before this frame, if any. `None` is
        /// an idle frame (redraw with no state change).
        message: Option<M>,
        /// A human label for the frame (which page, which scroll offset).
        label: String,
    },
    /// Dispatches a wheel event at a surface coordinate and measures the resulting
    /// frame. Any emitted scroll message is applied before the next step.
    Scroll {
        /// Surface-space cursor position; the wheel is only consumed by a
        /// scrollable the cursor is actually over, exactly as at runtime.
        cursor: (f32, f32),
        /// Vertical wheel delta in logical pixels. Positive scrolls toward the
        /// content's end (down); negative toward the start.
        pixels_y: f32,
        /// A human label for the frame (which scroll phase).
        label: String,
    },
}

/// A scripted, bounded interaction over one [`PerfSubject`].
#[derive(Debug, Clone)]
pub struct Workload<M> {
    /// What this workload measures.
    pub identity: WorkloadIdentity,
    /// The scripted steps, applied in order.
    pub steps: Vec<Step<M>>,
}

impl<M> Workload<M> {
    /// Validates every wait and their saturated total before execution begins.
    ///
    /// # Errors
    /// Returns [`PerfError::StepWaitTooLong`] or [`PerfError::TotalWaitTooLong`]
    /// when the corresponding limit is exceeded.
    pub fn validate_waits(&self) -> Result<(), PerfError> {
        let mut total = Duration::ZERO;
        for (step, entry) in self.steps.iter().enumerate() {
            let Step::Wait(duration) = entry else {
                continue;
            };
            if *duration > MAX_STEP_WAIT {
                return Err(PerfError::StepWaitTooLong {
                    step,
                    requested_ns: duration.as_nanos(),
                    max_ns: MAX_STEP_WAIT.as_nanos(),
                });
            }
            total = total.saturating_add(*duration);
            if total > MAX_TOTAL_WAIT {
                return Err(PerfError::TotalWaitTooLong {
                    step,
                    requested_ns: total.as_nanos(),
                    max_ns: MAX_TOTAL_WAIT.as_nanos(),
                });
            }
        }
        Ok(())
    }
}

impl<M: Clone> Workload<M> {
    /// A fully explicit workload.
    #[must_use]
    pub fn scripted(identity: WorkloadIdentity, steps: Vec<Step<M>>) -> Self {
        Self { identity, steps }
    }

    /// `frames` idle frames — the steady redraw floor with no state change.
    #[must_use]
    pub fn idle(name: impl Into<String>, frames: usize) -> Self {
        let steps = (0..frames)
            .map(|i| Step::Frame {
                message: None,
                label: format!("idle {i}"),
            })
            .collect();
        Self::scripted(
            WorkloadIdentity::new(name, WorkloadKind::Idle, format!("{frames} frames")),
            steps,
        )
    }

    /// Applies each message and measures the frame it produces.
    ///
    /// This is the common engine for page switches, sidebar toggles, and busy scrolling.
    #[must_use]
    pub fn from_messages(
        identity: WorkloadIdentity,
        messages: impl IntoIterator<Item = M>,
    ) -> Self {
        let steps = messages
            .into_iter()
            .enumerate()
            .map(|(i, m)| Step::Frame {
                message: Some(m),
                label: format!("{} {i}", identity.kind.name()),
            })
            .collect();
        Self::scripted(identity, steps)
    }

    /// A page/settings switch: each message routes to a different page and the
    /// frame it produces is measured.
    #[must_use]
    pub fn page_switch(
        name: impl Into<String>,
        dataset: impl Into<String>,
        route_messages: impl IntoIterator<Item = M>,
    ) -> Self {
        Self::from_messages(
            WorkloadIdentity::new(name, WorkloadKind::PageSwitch, dataset),
            route_messages,
        )
    }

    /// A sidebar toggle repeated `cycles` times: the animated collapse/expand
    /// the user waits on. Each application of `toggle` is one measured frame.
    #[must_use]
    pub fn sidebar_toggle(
        name: impl Into<String>,
        dataset: impl Into<String>,
        toggle: M,
        cycles: usize,
    ) -> Self {
        Self::from_messages(
            WorkloadIdentity::new(name, WorkloadKind::SidebarToggle, dataset),
            std::iter::repeat_n(toggle, cycles),
        )
    }

    /// A long busy scroll: each message is a scroll delta, and every resulting
    /// frame is measured.
    #[must_use]
    pub fn busy_scroll(
        name: impl Into<String>,
        dataset: impl Into<String>,
        scroll_messages: impl IntoIterator<Item = M>,
    ) -> Self {
        Self::from_messages(
            WorkloadIdentity::new(name, WorkloadKind::BusyScroll, dataset),
            scroll_messages,
        )
    }
}

/// Source and build identity supplied by the capture application.
///
/// Values are not inferred, and missing, empty, or `"unknown"` fields are
/// rejected before measurement.
#[derive(Debug, Clone)]
pub struct CaptureProvenance {
    /// Application/package name.
    pub app_name: String,
    /// Application/package version.
    pub app_version: String,
    /// Application source revision, including a dirty suffix when applicable.
    pub app_source_rev: String,
    /// enzUI framework source revision.
    pub framework_rev: String,
    /// Vendored iced source revision.
    pub vendor_rev: String,
    /// SHA-256 of the app's lock file.
    pub lock_sha256: String,
    /// Named build/runtime configuration.
    pub build_config: String,
    /// Stable, sorted enabled-feature description.
    pub enabled_features: String,
    /// Exact GPU/adapter identity, or `"none-offscreen"` for CPU raster.
    pub gpu: String,
}

impl CaptureProvenance {
    /// Read the complete capture identity exported by the app's runner.
    ///
    /// # Errors
    /// Returns [`PerfError::UnknownProvenance`] when any required value is
    /// absent, empty or explicitly `"unknown"`.
    pub fn from_env() -> Result<Self, PerfError> {
        fn required(name: &'static str) -> Result<String, PerfError> {
            std::env::var(name).map_err(|_| PerfError::UnknownProvenance { field: name })
        }
        let value = Self {
            app_name: required("ENZUI_PERF_APP_NAME")?,
            app_version: required("ENZUI_PERF_APP_VERSION")?,
            app_source_rev: required("ENZUI_PERF_APP_REV")?,
            framework_rev: required("ENZUI_PERF_FRAMEWORK_REV")?,
            vendor_rev: required("ENZUI_PERF_VENDOR_REV")?,
            lock_sha256: required("ENZUI_PERF_LOCK_SHA256")?,
            build_config: required("ENZUI_PERF_CONFIG")?,
            enabled_features: required("ENZUI_PERF_FEATURES")?,
            gpu: required("ENZUI_PERF_GPU")?,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), PerfError> {
        for (field, value) in [
            ("app_name", self.app_name.as_str()),
            ("app_version", self.app_version.as_str()),
            ("app_source_rev", self.app_source_rev.as_str()),
            ("framework_rev", self.framework_rev.as_str()),
            ("vendor_rev", self.vendor_rev.as_str()),
            ("lock_sha256", self.lock_sha256.as_str()),
            ("build_config", self.build_config.as_str()),
            ("enabled_features", self.enabled_features.as_str()),
            ("gpu", self.gpu.as_str()),
        ] {
            if value.trim().is_empty() || value.trim().eq_ignore_ascii_case("unknown") {
                return Err(PerfError::UnknownProvenance { field });
            }
        }
        if self.lock_sha256.len() != 64
            || !self
                .lock_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(PerfError::UnknownProvenance {
                field: "lock_sha256",
            });
        }
        Ok(())
    }
}

/// Source, build, executable, and machine identity recorded with a report.
#[derive(Debug, Clone)]
pub struct Provenance {
    /// Application/package name.
    pub app_name: String,
    /// Application/package version.
    pub app_version: String,
    /// Application source revision.
    pub app_source_rev: String,
    /// enzUI framework source revision.
    pub framework_rev: String,
    /// Vendored iced source revision.
    pub vendor_rev: String,
    /// SHA-256 of the app lock file.
    pub lock_sha256: String,
    /// Named build/runtime configuration.
    pub build_config: String,
    /// Stable enabled-feature description.
    pub enabled_features: String,
    /// The measuring binary's canonical path.
    pub binary: String,
    /// SHA-256 of the exact measuring binary.
    pub binary_sha256: String,
    /// The binary's mtime as Unix seconds.
    pub binary_mtime_unix: u64,
    /// `"release"` or `"debug"`.
    pub profile: &'static str,
    /// The renderer backend actually in use (`"tiny-skia"`, `"wgpu"`).
    pub backend: String,
    /// The GPU/adapter description; use an explicit value such as
    /// `"none-offscreen"` when no GPU participates.
    pub gpu: String,
    /// Logical surface width.
    pub width: f32,
    /// Logical surface height.
    pub height: f32,
    /// Surface scale factor.
    pub scale: f32,
}

impl Provenance {
    fn validate(&self) -> Result<(), PerfError> {
        for (field, value) in [
            ("app_name", self.app_name.as_str()),
            ("app_version", self.app_version.as_str()),
            ("app_source_rev", self.app_source_rev.as_str()),
            ("framework_rev", self.framework_rev.as_str()),
            ("vendor_rev", self.vendor_rev.as_str()),
            ("lock_sha256", self.lock_sha256.as_str()),
            ("build_config", self.build_config.as_str()),
            ("enabled_features", self.enabled_features.as_str()),
            ("binary", self.binary.as_str()),
            ("binary_sha256", self.binary_sha256.as_str()),
            ("backend", self.backend.as_str()),
            ("gpu", self.gpu.as_str()),
        ] {
            if value.trim().is_empty() || value.trim().eq_ignore_ascii_case("unknown") {
                return Err(PerfError::UnknownProvenance { field });
            }
        }
        for (field, hash) in [
            ("lock_sha256", self.lock_sha256.as_str()),
            ("binary_sha256", self.binary_sha256.as_str()),
        ] {
            if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(PerfError::UnknownProvenance { field });
            }
        }
        if self.binary_mtime_unix == 0 {
            return Err(PerfError::UnknownProvenance {
                field: "binary_mtime_unix",
            });
        }
        Ok(())
    }

    fn detect(
        supplied: &CaptureProvenance,
        backend: impl Into<String>,
        size: Size,
        scale: f32,
    ) -> Result<Self, PerfError> {
        supplied.validate()?;
        let backend = backend.into();
        if backend.trim().is_empty() || backend.eq_ignore_ascii_case("unknown") {
            return Err(PerfError::UnknownProvenance { field: "backend" });
        }
        let path = std::env::current_exe()
            .map_err(|_| PerfError::UnknownProvenance { field: "binary" })?;
        let binary = path
            .canonicalize()
            .map_err(|_| PerfError::UnknownProvenance { field: "binary" })?;
        let metadata = std::fs::metadata(&binary)
            .map_err(|_| PerfError::UnknownProvenance { field: "binary" })?;
        let binary_mtime_unix = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .ok_or(PerfError::UnknownProvenance {
                field: "binary_mtime_unix",
            })?;
        let mut file = std::fs::File::open(&binary).map_err(|_| PerfError::UnknownProvenance {
            field: "binary_sha256",
        })?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|_| PerfError::UnknownProvenance {
                    field: "binary_sha256",
                })?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(Self {
            app_name: supplied.app_name.clone(),
            app_version: supplied.app_version.clone(),
            app_source_rev: supplied.app_source_rev.clone(),
            framework_rev: supplied.framework_rev.clone(),
            vendor_rev: supplied.vendor_rev.clone(),
            lock_sha256: supplied.lock_sha256.to_ascii_lowercase(),
            build_config: supplied.build_config.clone(),
            enabled_features: supplied.enabled_features.clone(),
            binary: binary.display().to_string(),
            binary_sha256: format!("{:x}", hasher.finalize()),
            binary_mtime_unix,
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            backend,
            gpu: supplied.gpu.clone(),
            width: size.width,
            height: size.height,
            scale,
        })
    }
}

/// Process resources sampled at report time.
///
/// Fields are optional; RSS and CPU ticks are Linux-only, and CPU raster has no VRAM sample.
#[derive(Debug, Clone, Copy, Default)]
pub struct ResourceUsage {
    /// Resident set size in bytes (`/proc/self/status` `VmRSS`).
    pub rss_bytes: Option<u64>,
    /// Peak resident set size in bytes (`/proc/self/status` `VmHWM`).
    pub peak_rss_bytes: Option<u64>,
    /// Video memory in bytes, if a GPU backend reports it. `None` offscreen.
    pub vram_bytes: Option<u64>,
    /// User + system CPU clock ticks (`/proc/self/stat` fields 14+15).
    pub cpu_ticks: Option<u64>,
}

impl ResourceUsage {
    /// Samples RSS, peak RSS, and CPU ticks from `/proc` when available; VRAM remains `None`.
    #[must_use]
    pub fn sample() -> Self {
        let mut usage = Self::default();
        if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if let Some(rest) = line.strip_prefix("VmRSS:") {
                    usage.rss_bytes = parse_kb(rest);
                } else if let Some(rest) = line.strip_prefix("VmHWM:") {
                    usage.peak_rss_bytes = parse_kb(rest);
                }
            }
        }
        if let Ok(stat) = std::fs::read_to_string("/proc/self/stat") {
            // Fields after the (comm) parenthesis; utime is field 14, stime 15
            // (1-indexed from the start of the line). Split on the last ')'.
            if let Some(after) = stat.rsplit_once(')').map(|(_, rest)| rest) {
                let fields: Vec<&str> = after.split_whitespace().collect();
                // after ')' the first token is state (field 3), so utime (14) is
                // index 11 and stime (15) is index 12.
                if let (Some(u), Some(s)) = (fields.get(11), fields.get(12))
                    && let (Ok(u), Ok(s)) = (u.parse::<u64>(), s.parse::<u64>())
                {
                    usage.cpu_ticks = Some(u.saturating_add(s));
                }
            }
        }
        usage
    }
}

fn parse_kb(rest: &str) -> Option<u64> {
    rest.split_whitespace()
        .next()
        .and_then(|n| n.parse::<u64>().ok())
        .map(|kb| kb.saturating_mul(1024))
}

/// Metadata for an external visible-response capture.
///
/// Cadence must be finite and positive because observer data cannot establish
/// compositor-visible latency by itself.
#[derive(Debug, Clone)]
pub struct ExternalCapture {
    /// The capture tool (`"photodiode"`, `"screen-capture 240fps"`, ...).
    pub tool: String,
    /// Capture cadence in hertz. Finite and strictly positive.
    pub hz: f64,
}

impl ExternalCapture {
    /// Creates an external capture with a non-empty tool and finite positive cadence.
    ///
    /// # Errors
    /// Returns [`PerfError::InvalidExternalCapture`] when validation fails.
    pub fn new(tool: impl Into<String>, hz: f64) -> Result<Self, PerfError> {
        let capture = Self {
            tool: tool.into(),
            hz,
        };
        capture.validate()?;
        Ok(capture)
    }

    fn validate(&self) -> Result<(), PerfError> {
        if self.tool.trim().is_empty() {
            return Err(PerfError::InvalidExternalCapture { field: "tool" });
        }
        if !self.hz.is_finite() || self.hz <= 0.0 {
            return Err(PerfError::InvalidExternalCapture { field: "hz" });
        }
        Ok(())
    }
}

/// Runtime-observer counts and histograms attached to a workload report,
/// including exact milestone-quality buckets and dropped-event counts.
#[derive(Debug, Clone)]
pub struct ObserverSummary {
    /// Redraw requests observed during the measured workload.
    pub requested_frames: u64,
    /// Frames that produced fresh draw primitives.
    pub drawn_frames: u64,
    /// Successful newly-drawn frame milestones.
    pub milestone_frames: u64,
    /// Redraws that did not produce a new milestone.
    pub skipped_redraws: u64,
    /// Input arrival to first newly-drawn milestone.
    pub input_to_milestone: HistogramSnapshot,
    /// Input latency split by exact milestone quality (`Submitted`,
    /// `SurfacePresented`, `CompositorPresented`). This prevents one stronger
    /// sample from relabelling a mixed aggregate as visible.
    pub input_to_milestone_by_quality: [HistogramSnapshot; 3],
    /// Intervals between successive newly drawn milestones: observed frame cadence, not inverse paint time.
    pub frame_interval: HistogramSnapshot,
    /// Per runtime stage (`view_rebuild`, `tree_diff`, `layout`, ...).
    pub stages: Vec<(String, HistogramSnapshot)>,
    /// Trace events evicted by ring wrap.
    pub dropped_trace_events: u64,
    /// Window events refused after the window bound.
    pub dropped_window_events: u64,
    /// Strongest milestone quality the observer saw.
    pub quality: Option<PresentationQuality>,
}

impl ObserverSummary {
    /// Fold a live observer snapshot into the summary the report carries.
    #[must_use]
    pub fn from_snapshot(snapshot: &WindowSnapshot) -> Self {
        Self {
            requested_frames: snapshot.requested_frames,
            drawn_frames: snapshot.drawn_frames,
            milestone_frames: snapshot.milestone_frames,
            skipped_redraws: snapshot.skipped_redraws,
            input_to_milestone: snapshot.input_to_milestone,
            input_to_milestone_by_quality: snapshot.input_to_milestone_by_quality,
            frame_interval: snapshot.frame_interval,
            stages: snapshot
                .stages
                .iter()
                .map(|s| (stage_name(s.stage).to_owned(), s.duration))
                .collect(),
            dropped_trace_events: snapshot.dropped_trace_events,
            dropped_window_events: snapshot.dropped_window_events,
            quality: snapshot.presentation_quality,
        }
    }

    /// Total dropped events — a report with any is not a comparison baseline.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped_trace_events
            .saturating_add(self.dropped_window_events)
    }
}

/// The comparable, provenance-bearing result of one workload.
#[derive(Debug, Clone)]
pub struct WorkloadReport {
    /// What ran.
    pub identity: WorkloadIdentity,
    /// Where it ran.
    pub provenance: Provenance,
    /// Number of measured frames.
    pub frame_count: u64,
    /// Complete measured paint-pass distribution, including build, layout, draw, and rasterization.
    pub paint: HistogramSnapshot,
    /// View-build, diff and layout portion of the same paint passes. The
    /// driver does not run a second throwaway layout tree for this diagnostic.
    pub layout: HistogramSnapshot,
    /// Worst routed-page rebuild count in one measured frame. A directly
    /// rebuilt route is normally `1`; a retained unchanged route is `0`.
    pub max_builds_per_frame: usize,
    /// The strongest milestone the paint distribution represents. Offscreen this
    /// is always [`PresentationQuality::Submitted`].
    pub quality: PresentationQuality,
    /// Live observer data, when a run supplied a snapshot.
    pub observer: Option<ObserverSummary>,
    /// Process resource usage at report time.
    pub resources: ResourceUsage,
    /// External visible-response capture, if one accompanied the run.
    pub external_capture: Option<ExternalCapture>,
}

impl WorkloadReport {
    /// Returns inverse median paint time in frames per second, or zero when unmeasured.
    ///
    /// This is offscreen processing throughput, not observed presentation cadence;
    /// use [`ObserverSummary::frame_interval`] for the latter.
    #[must_use]
    pub fn paint_throughput_p50(&self) -> f64 {
        if self.paint.p50 == 0 {
            0.0
        } else {
            1_000_000_000.0 / self.paint.p50 as f64
        }
    }

    /// Dropped events across the whole run (0 without an observer).
    #[must_use]
    pub fn dropped_events(&self) -> u64 {
        self.observer.as_ref().map_or(0, ObserverSummary::dropped)
    }

    /// Returns input latency only for one homogeneous visible milestone quality.
    ///
    /// # Errors
    /// Returns [`PerfError::SubmittedClaimedVisible`] if no visible sample exists,
    /// [`PerfError::MixedMilestoneQualities`] for mixed qualities, or
    /// [`PerfError::CorruptSamples`] when exact and aggregate counts disagree.
    pub fn visible_input_latency(&self) -> Result<HistogramSnapshot, PerfError> {
        let observer = self
            .observer
            .as_ref()
            .ok_or(PerfError::SubmittedClaimedVisible)?;
        let [submitted, surface, compositor] = observer.input_to_milestone_by_quality;
        let separated_count = submitted
            .count
            .saturating_add(surface.count)
            .saturating_add(compositor.count);
        if separated_count != observer.input_to_milestone.count {
            return Err(PerfError::CorruptSamples {
                claimed: observer.input_to_milestone.count,
                actual: separated_count,
            });
        }
        if (submitted.count > 0 && (surface.count > 0 || compositor.count > 0))
            || (surface.count > 0 && compositor.count > 0)
        {
            return Err(PerfError::MixedMilestoneQualities);
        }
        if surface.count > 0 {
            return Ok(surface);
        }
        if compositor.count > 0 {
            return Ok(compositor);
        }
        Err(PerfError::SubmittedClaimedVisible)
    }

    /// Attaches observer data without promoting the report's own milestone quality.
    #[must_use]
    pub fn with_observer(mut self, summary: ObserverSummary) -> Self {
        self.observer = Some(summary);
        self
    }

    /// Attaches a validated external capture without merging it into observer data.
    ///
    /// # Errors
    /// Returns [`PerfError::InvalidExternalCapture`] when the record is invalid.
    pub fn with_external_capture(mut self, capture: ExternalCapture) -> Result<Self, PerfError> {
        capture.validate()?;
        self.external_capture = Some(capture);
        Ok(self)
    }

    /// Serialise as one JSON object — the comparable wire format.
    #[must_use]
    pub fn to_json(&self) -> String {
        let p = &self.provenance;
        let mut out = String::new();
        use fmt::Write as _;
        let _ = write!(
            out,
            "{{\"workload\":{{\"name\":{name},\"kind\":\"{kind}\",\"dataset\":{dataset}}},\
             \"provenance\":{{\"app_name\":{app_name},\"app_version\":{app_version},\
             \"app_source_rev\":{app_rev},\"framework_rev\":{framework_rev},\
             \"vendor_rev\":{vendor_rev},\"lock_sha256\":{lock_hash},\
             \"build_config\":{config},\"enabled_features\":{features},\
             \"binary\":{bin},\"binary_sha256\":{bin_hash},\"binary_mtime_unix\":{mtime},\
             \"profile\":\"{profile}\",\"backend\":{backend},\"gpu\":{gpu},\
             \"width\":{w},\"height\":{h},\"scale\":{scale}}},\
             \"frame_count\":{frames},\"paint_throughput_p50\":{fps:.2},\
             \"milestone_quality\":\"{quality}\",\"visible\":{visible},\
             \"max_builds_per_frame\":{builds},\
             \"paint_ns\":{paint},\"layout_ns\":{layout}",
            name = json_str(&self.identity.name),
            kind = self.identity.kind.name(),
            dataset = json_str(&self.identity.dataset),
            app_name = json_str(&p.app_name),
            app_version = json_str(&p.app_version),
            app_rev = json_str(&p.app_source_rev),
            framework_rev = json_str(&p.framework_rev),
            vendor_rev = json_str(&p.vendor_rev),
            lock_hash = json_str(&p.lock_sha256),
            config = json_str(&p.build_config),
            features = json_str(&p.enabled_features),
            bin = json_str(&p.binary),
            bin_hash = json_str(&p.binary_sha256),
            mtime = p.binary_mtime_unix,
            profile = p.profile,
            backend = json_str(&p.backend),
            gpu = json_str(&p.gpu),
            w = p.width,
            h = p.height,
            scale = p.scale,
            frames = self.frame_count,
            fps = self.paint_throughput_p50(),
            quality = quality_name(self.quality),
            visible = self.visible_input_latency().is_ok(),
            builds = self.max_builds_per_frame,
            paint = histogram_json(self.paint),
            layout = histogram_json(self.layout),
        );
        let _ = write!(
            out,
            ",\"resources\":{{\"rss_bytes\":{},\"peak_rss_bytes\":{},\"vram_bytes\":{},\"cpu_ticks\":{}}}",
            opt(self.resources.rss_bytes),
            opt(self.resources.peak_rss_bytes),
            opt(self.resources.vram_bytes),
            opt(self.resources.cpu_ticks),
        );
        let _ = write!(out, ",\"dropped_events\":{}", self.dropped_events());
        if let Some(observer) = &self.observer {
            let _ = write!(
                out,
                ",\"observer\":{{\"requested_frames\":{},\"drawn_frames\":{},\"milestone_frames\":{},\"skipped_redraws\":{},\"input_to_milestone\":{},\"input_to_milestone_by_quality\":{{\"Submitted\":{},\"SurfacePresented\":{},\"CompositorPresented\":{}}},\"frame_interval\":{},\"strongest_quality\":{},\"stages\":{{",
                observer.requested_frames,
                observer.drawn_frames,
                observer.milestone_frames,
                observer.skipped_redraws,
                histogram_json(observer.input_to_milestone),
                histogram_json(observer.input_to_milestone_by_quality[0]),
                histogram_json(observer.input_to_milestone_by_quality[1]),
                histogram_json(observer.input_to_milestone_by_quality[2]),
                histogram_json(observer.frame_interval),
                observer
                    .quality
                    .map_or_else(|| "null".into(), |quality| json_str(quality_name(quality)))
            );
            for (i, (stage, hist)) in observer.stages.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let _ = write!(out, "{}:{}", json_str(stage), histogram_json(*hist));
            }
            let _ = write!(
                out,
                "}},\"dropped_trace_events\":{},\"dropped_window_events\":{}}}",
                observer.dropped_trace_events, observer.dropped_window_events
            );
        } else {
            out.push_str(",\"observer\":null");
        }
        if let Some(capture) = &self.external_capture {
            let _ = write!(
                out,
                ",\"external_capture\":{{\"tool\":{},\"hz\":{}}}",
                json_str(&capture.tool),
                capture.hz
            );
        } else {
            out.push_str(",\"external_capture\":null");
        }
        out.push('}');
        out
    }

    /// Serializes ordered per-frame cadence and redraw dispositions while retaining the base [`Self::to_json`] fields.
    ///
    /// # Panics
    ///
    /// When `frames` does not hold exactly one row per counted frame.
    #[must_use]
    pub fn to_json_with_frames(&self, frames: &[FrameOutcome]) -> String {
        assert_eq!(
            frames.len() as u64,
            self.frame_count,
            "per-frame rows must match report.frame_count"
        );
        let mut out = self.to_json();
        let closing = out.pop();
        debug_assert_eq!(closing, Some('}'), "the report must close its JSON object");
        let _ = write!(out, ",\"frames\":{}", frames_json(frames));
        out.push('}');
        out
    }
}

fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::ViewRebuild => "view_rebuild",
        Stage::TreeDiff => "tree_diff",
        Stage::Layout => "layout",
        Stage::DrawPrepare => "draw_prepare",
        Stage::GpuSubmit => "gpu_submit",
    }
}

fn quality_name(quality: PresentationQuality) -> &'static str {
    match quality {
        PresentationQuality::Submitted => "Submitted",
        PresentationQuality::SurfacePresented => "SurfacePresented",
        PresentationQuality::CompositorPresented => "CompositorPresented",
    }
}

fn frames_json(frames: &[FrameOutcome]) -> String {
    let mut out = String::from("[");
    for (index, frame) in frames.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let interval = frame
            .interval_ns
            .map_or_else(|| "null".into(), |value| value.to_string());
        let deadline = frame
            .redraw
            .deadline_offset_ns()
            .map_or_else(|| "null".into(), |value| value.to_string());
        let remaining = frame
            .redraw
            .remaining_after_paint_ns()
            .map_or_else(|| "null".into(), |value| value.to_string());
        let _ = write!(
            out,
            "{{\"label\":{},\"interval_ns\":{},\"paint_ns\":{},\
             \"redraw\":{{\"kind\":\"{}\",\"deadline_offset_ns\":{},\
             \"remaining_after_paint_ns\":{},\"immediate\":{},\
             \"immediate_chain_settled\":{}}},\"redraw_messages\":{},\
             \"builds\":{}}}",
            json_str(&frame.label),
            interval,
            frame.paint_ns,
            frame.redraw.name(),
            deadline,
            remaining,
            frame.redraw.is_immediate(),
            !frame.redraw.is_immediate(),
            frame.redraw_messages,
            frame.builds,
        );
    }
    out.push(']');
    out
}

fn histogram_json(h: HistogramSnapshot) -> String {
    format!(
        "{{\"count\":{},\"p50_ns\":{},\"p95_ns\":{},\"p99_ns\":{},\"max_ns\":{}}}",
        h.count, h.p50, h.p95, h.p99, h.max
    )
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn opt(v: Option<u64>) -> String {
    v.map_or_else(|| "null".into(), |v| v.to_string())
}

/// Reasons a workload report or comparison is rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PerfError {
    /// The workload contains no measured frames.
    EmptyWorkload,
    /// A submitted-only milestone cannot establish user-visible presentation.
    SubmittedClaimedVisible,
    /// Exact-quality buckets contain multiple milestone qualities, so no single visible-latency aggregate is available.
    MixedMilestoneQualities,
    /// Claimed sample count differs from the histogram's actual count.
    CorruptSamples {
        /// Count claimed by the report or aggregate histogram.
        claimed: u64,
        /// Count found in the measured histogram(s).
        actual: u64,
    },
    /// A required provenance field is empty or unknown.
    UnknownProvenance {
        /// Missing/unusable field.
        field: &'static str,
    },
    /// One [`Step::Wait`] exceeds [`MAX_STEP_WAIT`].
    StepWaitTooLong {
        /// Index of the offending step within `Workload::steps`.
        step: usize,
        /// What that step asked for, in nanoseconds, saturated at `u128::MAX`.
        requested_ns: u128,
        /// [`MAX_STEP_WAIT`] in nanoseconds.
        max_ns: u128,
    },
    /// A workload's [`Step::Wait`]s summed past [`MAX_TOTAL_WAIT`].
    TotalWaitTooLong {
        /// Index of the step at which the running total crossed the ceiling.
        step: usize,
        /// The running total at that point, in nanoseconds (saturating).
        requested_ns: u128,
        /// [`MAX_TOTAL_WAIT`] in nanoseconds.
        max_ns: u128,
    },
    /// A comparison touched a run that dropped trace/window events.
    DroppedTraces,
    /// A comparison has too few frames on one side to rank meaningfully.
    Underpowered {
        /// Frames the offending report actually has.
        frames: u64,
        /// Frames required.
        floor: u64,
    },
    /// A comparison across mismatched backend/resolution/scale.
    Incomparable {
        /// What differs.
        reason: &'static str,
    },
    /// A measured frame produced an empty framebuffer.
    EmptyFrame,
    /// An external capture has an empty tool or a non-finite or non-positive cadence.
    InvalidExternalCapture {
        /// The field that failed (`"tool"` or `"hz"`).
        field: &'static str,
    },
}

impl fmt::Display for PerfError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyWorkload => write!(f, "workload measured no frames"),
            Self::StepWaitTooLong {
                step,
                requested_ns,
                max_ns,
            } => write!(
                f,
                "step {step} asked to wait {requested_ns}ns; a single wait may \
                 not exceed {max_ns}ns"
            ),
            Self::TotalWaitTooLong {
                step,
                requested_ns,
                max_ns,
            } => write!(
                f,
                "waits reached {requested_ns}ns by step {step}; a workload may \
                 not wait more than {max_ns}ns in total"
            ),
            Self::SubmittedClaimedVisible => write!(
                f,
                "milestone quality is Submitted (offscreen submit), not a visible frame; \
                 supply an external capture or a surface/compositor milestone"
            ),
            Self::MixedMilestoneQualities => write!(
                f,
                "input latency contains mixed milestone qualities; compare exact-quality samples"
            ),
            Self::CorruptSamples { claimed, actual } => write!(
                f,
                "corrupt report: claimed {claimed} samples but measured {actual}"
            ),
            Self::UnknownProvenance { field } => {
                write!(f, "required performance provenance is unknown: {field}")
            }
            Self::DroppedTraces => {
                write!(
                    f,
                    "a run dropped trace/window events; its histogram is incomplete"
                )
            }
            Self::Underpowered { frames, floor } => {
                write!(
                    f,
                    "underpowered comparison: {frames} frames < {floor} required"
                )
            }
            Self::Incomparable { reason } => write!(f, "incomparable runs: {reason}"),
            Self::EmptyFrame => write!(f, "workload frame produced an empty framebuffer"),
            Self::InvalidExternalCapture { field } => write!(
                f,
                "external capture is not usable: {field} is empty, NaN, infinite or non-positive"
            ),
        }
    }
}

impl std::error::Error for PerfError {}

/// The signed p50/p95/p99 paint-time deltas of a candidate against a baseline,
/// in nanoseconds. Positive means the candidate is slower.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Comparison {
    /// p50 delta (candidate − baseline), ns.
    pub p50_delta_ns: i64,
    /// p95 delta, ns.
    pub p95_delta_ns: i64,
    /// p99 delta, ns.
    pub p99_delta_ns: i64,
}

/// Compares candidate paint percentiles against a compatible baseline.
///
/// # Errors
/// Returns [`PerfError::UnknownProvenance`] for invalid provenance,
/// [`PerfError::Underpowered`] for too few frames, [`PerfError::CorruptSamples`]
/// when either paint or layout counts disagree with `frame_count`,
/// [`PerfError::DroppedTraces`] for event loss, or [`PerfError::Incomparable`]
/// when a required workload or environment dimension differs.
pub fn compare(
    baseline: &WorkloadReport,
    candidate: &WorkloadReport,
) -> Result<Comparison, PerfError> {
    for report in [baseline, candidate] {
        report.provenance.validate()?;
        if report.frame_count < MIN_COMPARABLE_FRAMES {
            return Err(PerfError::Underpowered {
                frames: report.frame_count,
                floor: MIN_COMPARABLE_FRAMES,
            });
        }
        for histogram in [report.paint, report.layout] {
            if histogram.count != report.frame_count {
                return Err(PerfError::CorruptSamples {
                    claimed: report.frame_count,
                    actual: histogram.count,
                });
            }
        }
        if report.dropped_events() > 0 {
            return Err(PerfError::DroppedTraces);
        }
    }
    // Workload and app/config identity must match. Source and binary revisions
    // deliberately may differ: that difference is normally what is measured.
    if baseline.identity.kind != candidate.identity.kind {
        return Err(PerfError::Incomparable {
            reason: "workload kind",
        });
    }
    if baseline.identity.name != candidate.identity.name {
        return Err(PerfError::Incomparable {
            reason: "workload name",
        });
    }
    if baseline.identity.dataset != candidate.identity.dataset {
        return Err(PerfError::Incomparable { reason: "dataset" });
    }
    if baseline.quality != candidate.quality {
        return Err(PerfError::Incomparable {
            reason: "presentation quality",
        });
    }
    if baseline.provenance.app_name != candidate.provenance.app_name {
        return Err(PerfError::Incomparable { reason: "app" });
    }
    if baseline.provenance.build_config != candidate.provenance.build_config {
        return Err(PerfError::Incomparable {
            reason: "build config",
        });
    }
    if baseline.provenance.enabled_features != candidate.provenance.enabled_features {
        return Err(PerfError::Incomparable { reason: "features" });
    }
    if baseline.provenance.lock_sha256 != candidate.provenance.lock_sha256 {
        return Err(PerfError::Incomparable {
            reason: "lock file",
        });
    }
    if baseline.provenance.backend != candidate.provenance.backend {
        return Err(PerfError::Incomparable { reason: "backend" });
    }
    if baseline.provenance.profile != candidate.provenance.profile {
        return Err(PerfError::Incomparable { reason: "profile" });
    }
    if baseline.provenance.gpu != candidate.provenance.gpu {
        return Err(PerfError::Incomparable { reason: "gpu" });
    }
    if (baseline.provenance.width - candidate.provenance.width).abs() > f32::EPSILON
        || (baseline.provenance.height - candidate.provenance.height).abs() > f32::EPSILON
    {
        return Err(PerfError::Incomparable {
            reason: "resolution",
        });
    }
    if (baseline.provenance.scale - candidate.provenance.scale).abs() > f32::EPSILON {
        return Err(PerfError::Incomparable { reason: "scale" });
    }
    let d = |b: u64, c: u64| c as i64 - b as i64;
    Ok(Comparison {
        p50_delta_ns: d(baseline.paint.p50, candidate.paint.p50),
        p95_delta_ns: d(baseline.paint.p95, candidate.paint.p95),
        p99_delta_ns: d(baseline.paint.p99, candidate.paint.p99),
    })
}

/// Runs a scripted [`Workload`] over one [`PerfSubject`] and records measured-frame reports.
pub struct WorkloadDriver<S: PerfSubject> {
    harness: PerfHarness<S>,
    scale: f32,
    size: Size,
    capture: CaptureProvenance,
    frames: Vec<FrameOutcome>,
}

/// Cadence, cost, redraw disposition, messages, and build count for one measured frame.
#[derive(Debug, Clone)]
pub struct FrameOutcome {
    /// The measured `Step::Frame` or `Step::Scroll` label.
    pub label: String,
    /// Wall time from the previous measured frame's start to this one's,
    /// including any [`Step::Wait`] between them. `None` for the first frame.
    pub interval_ns: Option<u64>,
    /// Measured build + layout + draw + rasterise for this frame.
    pub paint_ns: u64,
    /// Exact redraw disposition returned by this frame's runtime update.
    pub redraw: RuntimeRedrawRequest,
    /// Number of messages emitted by this frame and applied before the next step.
    pub redraw_messages: usize,
    /// Routed-page builds observed during this frame.
    pub builds: usize,
}

impl<S: PerfSubject> WorkloadDriver<S> {
    /// Build a driver at `size` (logical) with `scale` and explicit source /
    /// build identity. [`Self::run`] rejects missing or `"unknown"` fields.
    #[must_use]
    pub fn new(subject: S, size: Size, scale: f32, capture: CaptureProvenance) -> Self {
        Self {
            harness: PerfHarness::new(subject, size),
            scale,
            size,
            capture,
            frames: Vec::new(),
        }
    }

    /// Per-frame cadence and animation state for the most recent run.
    #[must_use]
    pub fn frames(&self) -> &[FrameOutcome] {
        &self.frames
    }

    /// Immutable access to the subject.
    pub fn subject(&self) -> &S {
        self.harness.subject()
    }

    /// Executes a workload and returns an offscreen processing-time report.
    ///
    /// The harness warms before measurement, each measured step uses one tree for
    /// layout and paint, and emitted messages are applied before the next step.
    ///
    /// # Errors
    /// Returns a wait-limit error, [`PerfError::EmptyWorkload`], or
    /// [`PerfError::EmptyFrame`] when the corresponding validation fails.
    pub fn run(&mut self, workload: &Workload<S::Message>) -> Result<WorkloadReport, PerfError> {
        self.capture.validate()?;
        workload.validate_waits()?;
        self.harness.warm();
        self.run_inner(workload, None)
    }

    /// Executes a workload with runtime observation enabled for measured frames.
    ///
    /// The report includes frame cadence, runtime-stage histograms, redraw counts,
    /// and submitted-only offscreen milestones. Observation is disabled before every
    /// return, including errors.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::run`].
    pub fn run_observed(
        &mut self,
        workload: &Workload<S::Message>,
    ) -> Result<WorkloadReport, PerfError> {
        use iced_runtime::performance::{self, Config, WindowId};

        self.capture.validate()?;
        // Before `warm()`, before the observer is enabled, and before any
        // sleep: a refusal must not cost a warm-up, a trace buffer or a wait.
        workload.validate_waits()?;
        self.harness.warm();
        let measured = workload
            .steps
            .iter()
            .filter(|step| matches!(step, Step::Frame { .. } | Step::Scroll { .. }))
            .count();
        let window = WindowId(0x2151);
        // The runtime observer is process-global, so two observed runs on
        // parallel threads (a test binary's default) would count each other's
        // frames. One observed run at a time; a panicking run must not poison
        // the next.
        static OBSERVED: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _observed = OBSERVED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        performance::enable(Config {
            // Reserve twelve events per measured frame for five stages and frame lifecycle events, plus a fixed margin.
            trace_capacity: measured.saturating_mul(12).saturating_add(64),
            max_windows: 1,
        });
        let result = self.run_inner(workload, Some(window));
        let snapshot = performance::snapshot(window);
        performance::disable();
        result.map(|report| match snapshot {
            Some(snapshot) => report.with_observer(ObserverSummary::from_snapshot(&snapshot)),
            None => report,
        })
    }

    fn run_inner(
        &mut self,
        workload: &Workload<S::Message>,
        observed_window: Option<iced_runtime::performance::WindowId>,
    ) -> Result<WorkloadReport, PerfError> {
        use iced_runtime::performance;

        let mut paint = HdrHistogram::default();
        let mut layout = HdrHistogram::default();
        let mut max_builds = 0usize;
        let mut frames = 0u64;
        // The start of the previous measured frame, so each frame's
        // recorded interval includes the `Step::Wait` cadence that preceded it.
        let mut last_frame_start: Option<Instant> = None;
        self.frames.clear();

        for step in &workload.steps {
            match step {
                Step::Drive(message) => self.harness.apply(message.clone()),
                // Bounded by `Workload::validate_waits`, which both public
                // entry points run over the whole script before reaching here.
                Step::Wait(duration) => {
                    debug_assert!(*duration <= MAX_STEP_WAIT, "unvalidated wait");
                    std::thread::sleep(*duration);
                }
                Step::Frame { message, label } => {
                    let frame_start = Instant::now();
                    let interval_ns = last_frame_start
                        .map(|previous| duration_ns(frame_start.duration_since(previous)));
                    last_frame_start = Some(frame_start);
                    if let Some(window) = observed_window
                        && message.is_some()
                    {
                        performance::input_arrived(window, 1);
                        performance::input_start(window, 1);
                    }
                    if let Some(message) = message {
                        self.harness.apply(message.clone());
                    }
                    if let Some(window) = observed_window {
                        if message.is_some() {
                            performance::input_end(window, true);
                        }
                        performance::invalidate(window, 1);
                    }

                    reset_page_builds();
                    let t = Instant::now();
                    let (pixels, emitted, layout_elapsed) = if let Some(window) = observed_window {
                        performance::frame_requested(window);
                        let measured = self.harness.paint_pass_observed(window);
                        performance::frame_drawn(window);
                        performance::milestone(window, true, PresentationQuality::Submitted);
                        measured
                    } else {
                        self.harness.paint_pass_measured()
                    };
                    let elapsed = t.elapsed();
                    if pixels.is_empty() {
                        return Err(PerfError::EmptyFrame);
                    }
                    paint.record(duration_ns(elapsed));
                    layout.record(duration_ns(layout_elapsed));
                    let builds = page_builds();
                    max_builds = max_builds.max(builds);
                    // Record before applying emitted messages, so the row
                    // describes exactly this runtime update and paint.
                    self.frames.push(FrameOutcome {
                        label: label.clone(),
                        interval_ns,
                        paint_ns: duration_ns(elapsed),
                        redraw: self.harness.redraw_request(),
                        redraw_messages: emitted.len(),
                        builds,
                    });
                    for message in emitted {
                        self.harness.apply(message);
                    }
                    frames += 1;
                }
                Step::Scroll {
                    cursor,
                    pixels_y,
                    label,
                } => {
                    let frame_start = Instant::now();
                    let interval_ns = last_frame_start
                        .map(|previous| duration_ns(frame_start.duration_since(previous)));
                    last_frame_start = Some(frame_start);
                    // Instrument wheel delivery as input so observer latency includes any resulting invalidation.
                    let cursor = iced_core::Point::new(cursor.0, cursor.1);
                    if let Some(window) = observed_window {
                        performance::input_arrived(window, 1);
                        performance::input_start(window, 1);
                        performance::input_end(window, true);
                        performance::invalidate(window, 1);
                    }

                    reset_page_builds();
                    let t = Instant::now();
                    let (pixels, emitted, layout_elapsed) = if let Some(window) = observed_window {
                        performance::frame_requested(window);
                        let measured = self.harness.scroll_pass_observed(cursor, *pixels_y, window);
                        performance::frame_drawn(window);
                        performance::milestone(window, true, PresentationQuality::Submitted);
                        measured
                    } else {
                        self.harness.scroll_pass_measured(cursor, *pixels_y)
                    };
                    let elapsed = t.elapsed();
                    if pixels.is_empty() {
                        return Err(PerfError::EmptyFrame);
                    }
                    paint.record(duration_ns(elapsed));
                    layout.record(duration_ns(layout_elapsed));
                    let builds = page_builds();
                    max_builds = max_builds.max(builds);
                    self.frames.push(FrameOutcome {
                        label: label.clone(),
                        interval_ns,
                        paint_ns: duration_ns(elapsed),
                        redraw: self.harness.redraw_request(),
                        redraw_messages: emitted.len(),
                        builds,
                    });
                    // Apply emitted scroll messages so model state receives the widget-owned offset before the next step.
                    for message in emitted {
                        self.harness.apply(message);
                    }
                    frames += 1;
                }
            }
        }

        if frames == 0 {
            return Err(PerfError::EmptyWorkload);
        }

        let provenance = Provenance::detect(
            &self.capture,
            self.harness.renderer_name(),
            self.size,
            self.scale,
        )?;
        Ok(WorkloadReport {
            identity: workload.identity.clone(),
            provenance,
            frame_count: frames,
            paint: paint.snapshot(),
            layout: layout.snapshot(),
            max_builds_per_frame: max_builds,
            // Headless texture submission establishes no surface or compositor presentation.
            quality: PresentationQuality::Submitted,
            observer: None,
            resources: ResourceUsage::sample(),
            external_capture: None,
        })
    }
}

fn duration_ns(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}
