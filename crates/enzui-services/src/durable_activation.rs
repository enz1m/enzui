//! Schedules bounded one-shot application activation that survives process exit.
//!
//! Custody is durable before backend publication. An activation is returned only
//! after its generation is durably consumed; stale, cancelled, and duplicate
//! invocations are refused. A crash after consumption can lose delivery; callers
//! must route the returned payload themselves. The systemd backend guards unit
//! publication and removal with ownership markers.
//!
//! Times and catch-up windows are truncated to whole seconds. Claim expiry is
//! the due instant plus the catch-up window and [`DELIVERY_GRACE`], independent
//! of when the process actually starts. Scheduler methods block on filesystem
//! and backend operations; action and payload bounds are not latency bounds.

use crate::atomic_state::{self, StateLoad, StateRefusal};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(target_os = "linux")]
pub mod systemd;

/// Per-application custody limit, including pending cleanup: 128 jobs.
pub const MAX_JOBS: usize = 128;
/// Per-job payload limit: 64 arguments.
pub const MAX_ARGUMENTS: usize = 64;
/// Per-job payload limit across all arguments: 16 KiB of UTF-8.
pub const MAX_ARGUMENT_BYTES: usize = 16 * 1024;
/// Scheduling horizon from the observed wall clock: 366 days.
pub const MAX_HORIZON: Duration = Duration::from_secs(366 * 24 * 60 * 60);
/// Maximum requested reboot catch-up interval: seven days.
pub const MAX_CATCH_UP: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Extra claim-window allowance after the due instant and catch-up interval: 60 seconds.
pub const DELIVERY_GRACE: Duration = Duration::from_secs(60);

const STATE_FORMAT: &str = "durable-activation";
const STATE_VERSION: u32 = 1;
const MAX_STATE_BYTES: u64 = 3 * 1024 * 1024;
const LOCK_WAIT: Duration = Duration::from_secs(2);
const MAX_RECONCILE_ACTIONS: usize = MAX_JOBS * 4;
const MAX_STALE_BACKEND_REPAIRS: usize = 8;
const MAX_DIAGNOSTIC_BYTES: usize = 1024;
const INVOCATION_SWITCH: &str = "--enzui-durable-activation";

/// A bounded reverse-DNS identity shared by custody and the platform backend.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DesktopApplicationId(String);

impl DesktopApplicationId {
    /// Parses a reverse-DNS-style desktop id.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError::DesktopApplicationId`] unless the id has at
    /// least two ASCII segments of 1..=63 bytes and fits the 128-byte bound. Each
    /// segment starts with a letter or digit and otherwise permits those, `_`
    /// and `-`. Input spelling is preserved.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        let valid = value.len() <= 128
            && value.split('.').count() >= 2
            && value.split('.').all(|segment| {
                !segment.is_empty()
                    && segment.len() <= 63
                    && segment
                        .bytes()
                        .next()
                        .is_some_and(|byte| byte.is_ascii_alphanumeric())
                    && segment
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            });
        if valid {
            Ok(Self(value))
        } else {
            Err(ValidationError::DesktopApplicationId)
        }
    }

    /// Borrows the validated id used in durable records and unit markers.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DesktopApplicationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A bounded application-local identity stable across process restarts.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(String);

impl JobId {
    /// Parses a job id that is safe to embed in a user-systemd unit name.
    ///
    /// # Errors
    ///
    /// Returns [`ValidationError::JobId`] for an empty value, a value over 64
    /// bytes, or a byte outside ASCII letters, digits, `_`, and `-`.
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
        if valid {
            Ok(Self(value))
        } else {
            Err(ValidationError::JobId)
        }
    }

    /// Borrows the canonical id used for replacement and cancellation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Nonzero generation allocated from a persisted scheduler-wide counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobGeneration(u64);

impl JobGeneration {
    /// Returns `None` for zero, which is reserved as an invalid generation.
    #[must_use]
    pub fn new(value: u64) -> Option<Self> {
        (value != 0).then_some(Self(value))
    }

    /// Returns the nonzero value carried across process boundaries.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Policy for an instant missed while the user session was unavailable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissedActivationPolicy {
    /// Do not ask the service manager to catch up after the instant.
    ///
    /// A launched process still has [`DELIVERY_GRACE`] to reach [`DurableActivationScheduler::claim`].
    Skip,
    /// Request persistent catch-up with this overdue interval plus startup grace.
    ///
    /// [`DELIVERY_GRACE`] extends the claim window measured from the due instant.
    CatchUpWithin(Duration),
}

impl MissedActivationPolicy {
    fn window_secs(self) -> u64 {
        match self {
            Self::Skip => DELIVERY_GRACE.as_secs(),
            Self::CatchUpWithin(duration) => {
                duration.as_secs().saturating_add(DELIVERY_GRACE.as_secs())
            }
        }
    }

    fn persistent(self) -> bool {
        matches!(self, Self::CatchUpWithin(_))
    }
}

/// A one-shot command whose executable and payload have passed durable-scheduler bounds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivationRequest {
    job: JobId,
    executable: PathBuf,
    arguments: Vec<String>,
    instant: SystemTime,
    missed: MissedActivationPolicy,
}

impl ActivationRequest {
    /// Creates a request after resolving the executable to a canonical path.
    ///
    /// The original payload is stored in custody; backends launch with a
    /// framework invocation that identifies the generation. Executable checks
    /// are a construction-time snapshot, not a retained file identity. Instant
    /// and horizon validation occurs when scheduling.
    ///
    /// # Errors
    ///
    /// Returns an error for an unusable executable, a NUL byte, or an argument or catch-up bound violation.
    pub fn new(
        job: JobId,
        executable: impl AsRef<Path>,
        arguments: Vec<String>,
        instant: SystemTime,
        missed: MissedActivationPolicy,
    ) -> Result<Self, ValidationError> {
        if arguments.len() > MAX_ARGUMENTS {
            return Err(ValidationError::TooManyArguments);
        }
        let argument_bytes = arguments
            .iter()
            .try_fold(0usize, |total, argument| total.checked_add(argument.len()));
        if argument_bytes.is_none_or(|bytes| bytes > MAX_ARGUMENT_BYTES) {
            return Err(ValidationError::ArgumentsTooLarge);
        }
        if arguments.iter().any(|argument| argument.contains('\0')) {
            return Err(ValidationError::ArgumentContainsNul);
        }
        if matches!(missed, MissedActivationPolicy::CatchUpWithin(window) if window > MAX_CATCH_UP)
        {
            return Err(ValidationError::CatchUpTooLong);
        }
        let requested = executable.as_ref();
        if !requested.is_absolute() {
            return Err(ValidationError::ExecutableNotAbsolute);
        }
        let executable = std::fs::canonicalize(requested)
            .map_err(|error| ValidationError::ExecutableUnavailable(error.kind()))?;
        let metadata = std::fs::metadata(&executable)
            .map_err(|error| ValidationError::ExecutableUnavailable(error.kind()))?;
        if !is_executable_file(&metadata) {
            return Err(ValidationError::ExecutableNotRunnable);
        }
        if executable.to_str().is_none() {
            return Err(ValidationError::ExecutableNotUtf8);
        }
        Ok(Self {
            job,
            executable,
            arguments,
            instant,
            missed,
        })
    }

    /// Returns the identity used by replace and cancel operations.
    #[must_use]
    pub fn job(&self) -> &JobId {
        &self.job
    }

    /// Returns the canonical executable captured during validation.
    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Borrows the payload that scheduling will retain in custody.
    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    /// Returns the requested wall-clock instant.
    #[must_use]
    pub fn instant(&self) -> SystemTime {
        self.instant
    }

    /// Returns the policy controlling reboot catch-up and claim expiry.
    #[must_use]
    pub fn missed_policy(&self) -> MissedActivationPolicy {
        self.missed
    }
}

#[cfg(unix)]
fn is_executable_file(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable_file(metadata: &std::fs::Metadata) -> bool {
    metadata.is_file()
}

/// Input validation failure before custody changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ValidationError {
    /// The desktop id is not a bounded reverse-DNS identifier.
    DesktopApplicationId,
    /// The job id is not a bounded unit-safe identifier.
    JobId,
    /// More than [`MAX_ARGUMENTS`] payload arguments were supplied.
    TooManyArguments,
    /// Payload arguments exceed [`MAX_ARGUMENT_BYTES`] combined UTF-8 bytes.
    ArgumentsTooLarge,
    /// A payload argument contains a NUL byte that cannot cross an argv boundary.
    ArgumentContainsNul,
    /// Catch-up exceeds [`MAX_CATCH_UP`].
    CatchUpTooLong,
    /// The executable path is not absolute.
    ExecutableNotAbsolute,
    /// The executable could not be resolved or inspected.
    ExecutableUnavailable(io::ErrorKind),
    /// The resolved path is not a regular executable file.
    ExecutableNotRunnable,
    /// The executable cannot be represented by the UTF-8 systemd unit format.
    ExecutableNotUtf8,
    /// The instant predates the Unix epoch.
    InstantBeforeEpoch,
    /// The instant is already outside its requested catch-up policy.
    InstantMissed,
    /// The instant is more than [`MAX_HORIZON`] in the future.
    HorizonExceeded,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid durable activation request: {self:?}")
    }
}

impl std::error::Error for ValidationError {}

/// Stage of a platform publication or removal transaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BackendStage {
    /// Writing the service unit.
    ServiceFile,
    /// Writing the timer unit.
    TimerFile,
    /// Reloading the user service manager.
    Reload,
    /// Enabling the persistent timer.
    Enable,
    /// Starting or replacing the timer.
    Start,
    /// Stopping or disabling a timer.
    Stop,
    /// Removing generated unit files.
    Remove,
    /// Enumerating generated units.
    List,
    /// Reading the host boot identity.
    BootIdentity,
}

/// Stable class of platform backend failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BackendErrorKind {
    /// The user service facility or its executable is absent.
    Unavailable,
    /// The service manager or filesystem denied the operation.
    PermissionDenied,
    /// A bounded service-manager operation exceeded its deadline.
    TimedOut,
    /// A generated path is occupied by an object not owned by this scheduler.
    Collision,
    /// The backend failed for another reported reason.
    Failed,
}

/// A bounded platform failure that reports whether retry cleanup is complete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendError {
    kind: BackendErrorKind,
    stage: BackendStage,
    partial_publication: bool,
    cleanup_complete: bool,
    detail: String,
}

impl BackendError {
    /// Records a backend failure, truncating its diagnostic to 1 KiB.
    #[must_use]
    pub fn new(
        kind: BackendErrorKind,
        stage: BackendStage,
        partial_publication: bool,
        cleanup_complete: bool,
        detail: impl Into<String>,
    ) -> Self {
        let detail = bounded_diagnostic(detail.into());
        Self {
            kind,
            stage,
            partial_publication,
            cleanup_complete,
            detail,
        }
    }

    /// Returns the class callers use for retry and user-facing policy.
    #[must_use]
    pub fn kind(&self) -> BackendErrorKind {
        self.kind
    }

    /// Identifies the operation that did not complete.
    #[must_use]
    pub fn stage(&self) -> BackendStage {
        self.stage
    }

    /// Reports whether the platform may have observed part of the transaction.
    #[must_use]
    pub fn is_partial_publication(&self) -> bool {
        self.partial_publication
    }

    /// Reports whether the backend declared rollback or cleanup complete.
    #[must_use]
    pub fn cleanup_complete(&self) -> bool {
        self.cleanup_complete
    }

    /// Returns the diagnostic, truncated to 1 KiB at construction.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for BackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "activation backend {:?} at {:?}: {}",
            self.kind, self.stage, self.detail
        )
    }
}

impl std::error::Error for BackendError {}

/// Immutable publication data passed to a platform facility after custody commits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivationPublication {
    application: DesktopApplicationId,
    job: JobId,
    generation: JobGeneration,
    executable: PathBuf,
    arguments: Vec<String>,
    unix_seconds: u64,
    persistent: bool,
}

impl ActivationPublication {
    /// Returns the identity that must appear in platform ownership metadata.
    #[must_use]
    pub fn application(&self) -> &DesktopApplicationId {
        &self.application
    }

    /// Returns the stable replacement and cancellation key.
    #[must_use]
    pub fn job(&self) -> &JobId {
        &self.job
    }

    /// Returns the generation required for delivery admission.
    #[must_use]
    pub fn generation(&self) -> JobGeneration {
        self.generation
    }

    /// Returns the validated executable path; backends must not perform PATH lookup.
    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Returns the exact non-shell argument vector for the launched process.
    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }

    /// Returns the absolute deadline as whole Unix seconds.
    #[must_use]
    pub fn unix_seconds(&self) -> u64 {
        self.unix_seconds
    }

    /// Reports whether the platform timer may fire after downtime.
    #[must_use]
    pub fn persistent(&self) -> bool {
        self.persistent
    }
}

/// Ownership identity that the backend must verify before removing a generation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivationRemoval {
    application: DesktopApplicationId,
    job: JobId,
    generation: JobGeneration,
}

impl ActivationRemoval {
    /// Returns the identity that must appear in platform ownership metadata.
    #[must_use]
    pub fn application(&self) -> &DesktopApplicationId {
        &self.application
    }

    /// Returns the stable job whose owned artifacts may be removed.
    #[must_use]
    pub fn job(&self) -> &JobId {
        &self.job
    }

    /// Returns the only generation this request authorizes for removal.
    #[must_use]
    pub fn generation(&self) -> JobGeneration {
        self.generation
    }
}

/// Backend-reported artifact identity and completeness used for reconciliation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledActivation {
    job: JobId,
    generation: JobGeneration,
    complete: bool,
}

impl InstalledActivation {
    /// Records a discovered generation and whether all required artifacts exist.
    #[must_use]
    pub fn new(job: JobId, generation: JobGeneration, complete: bool) -> Self {
        Self {
            job,
            generation,
            complete,
        }
    }

    /// Returns the job reported by the backend.
    #[must_use]
    pub fn job(&self) -> &JobId {
        &self.job
    }

    /// Returns the generation found in the ownership marker.
    #[must_use]
    pub fn generation(&self) -> JobGeneration {
        self.generation
    }

    /// Reports backend-defined artifact completeness.
    ///
    /// The systemd backend checks file markers and timer targets, without
    /// checking whether the manager has loaded or armed the timer.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.complete
    }
}

/// Backend boundary for durable publication, guarded removal, and boot identity.
///
/// The scheduler invokes these methods synchronously without its custody lock.
/// Implementations must verify ownership before modifying artifacts and report
/// partial changes so failed operations can be retried.
pub trait ActivationFacility {
    /// Publish one generation while preserving unrelated artifacts.
    ///
    /// # Errors
    ///
    /// Report publication failure and any partial changes through [`BackendError`].
    fn publish(&mut self, publication: &ActivationPublication) -> Result<(), BackendError>;
    /// Stop and remove only artifacts owned by the requested generation.
    ///
    /// # Errors
    ///
    /// Report ownership refusal or cleanup failure through [`BackendError`].
    fn remove(&mut self, removal: &ActivationRemoval) -> Result<(), BackendError>;
    /// List only artifacts whose ownership metadata validates.
    ///
    /// # Errors
    ///
    /// Report enumeration or ownership-validation failure through [`BackendError`].
    fn installed(&mut self) -> Result<Vec<InstalledActivation>, BackendError>;
    /// Return the current boot identity used to trigger restart reconciliation.
    ///
    /// # Errors
    ///
    /// Report an unavailable or invalid boot identity through [`BackendError`].
    fn boot_id(&mut self) -> Result<String, BackendError>;
}

/// Durable state distinguishing acknowledged work from work requiring repair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobStatus {
    /// Custody is durable, but publication has not been acknowledged.
    PublicationPending,
    /// The current generation was acknowledged by the platform.
    Scheduled,
    /// Cancellation bars delivery; platform cleanup remains retryable.
    RemovalPending,
    /// The generation was consumed by delivery or expiry.
    Delivered,
}

impl JobStatus {
    fn may_deliver(self) -> bool {
        matches!(self, Self::PublicationPending | Self::Scheduled)
    }
}

/// A bounded snapshot of one job still in durable custody.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobRecord {
    job: JobId,
    generation: JobGeneration,
    unix_seconds: u64,
    missed: MissedActivationPolicy,
    status: JobStatus,
}

impl JobRecord {
    /// Returns the key accepted by replacement and cancellation.
    #[must_use]
    pub fn job(&self) -> &JobId {
        &self.job
    }

    /// Returns the generation accepted by [`DurableActivationScheduler::claim`].
    #[must_use]
    pub fn generation(&self) -> JobGeneration {
        self.generation
    }

    /// Returns the scheduled instant as whole Unix seconds.
    #[must_use]
    pub fn unix_seconds(&self) -> u64 {
        self.unix_seconds
    }

    /// Returns the policy retained for restart reconciliation and claim expiry.
    #[must_use]
    pub fn missed_policy(&self) -> MissedActivationPolicy {
        self.missed
    }

    /// Returns whether publication or cleanup still needs retrying.
    #[must_use]
    pub fn status(&self) -> JobStatus {
        self.status
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredJob {
    summary: JobRecord,
    executable: PathBuf,
    arguments: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Store {
    boot_id: String,
    last_wall_seconds: u64,
    next_generation: u64,
    jobs: BTreeMap<JobId, StoredJob>,
}

#[derive(Clone, Debug)]
enum ReconcileStep {
    Publish {
        job: StoredJob,
        expected_status: JobStatus,
    },
    Remove {
        job: StoredJob,
        expected_status: JobStatus,
    },
    Expire {
        job: StoredJob,
        expected_status: JobStatus,
    },
    Orphan(InstalledActivation),
}

/// Failure of durable state custody.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum CustodyErrorKind {
    /// Another process retained the state lock beyond the bounded wait.
    Busy,
    /// State was malformed, incompatible, or refused by the bounded loader.
    Corrupt,
    /// Filesystem permissions refused a state operation.
    PermissionDenied,
    /// Another state I/O operation failed.
    Io,
    /// Atomic publication failed after the destination may already have changed.
    CommitUncertain,
}

/// A durable-state failure carrying a diagnostic of at most 1 KiB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustodyError {
    kind: CustodyErrorKind,
    detail: String,
}

impl CustodyError {
    fn io(error: io::Error, commit: bool) -> Self {
        let kind = if error.kind() == io::ErrorKind::PermissionDenied {
            CustodyErrorKind::PermissionDenied
        } else if commit {
            CustodyErrorKind::CommitUncertain
        } else {
            CustodyErrorKind::Io
        };
        Self {
            kind,
            detail: bounded_diagnostic(error.to_string()),
        }
    }

    fn corrupt(detail: impl Into<String>) -> Self {
        Self {
            kind: CustodyErrorKind::Corrupt,
            detail: bounded_diagnostic(detail.into()),
        }
    }

    /// Returns the class callers use to distinguish contention, corruption, and I/O.
    #[must_use]
    pub fn kind(&self) -> CustodyErrorKind {
        self.kind
    }

    /// Returns the diagnostic retained for logging.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for CustodyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "activation custody {:?}: {}",
            self.kind, self.detail
        )
    }
}

impl std::error::Error for CustodyError {}

/// Scheduling, cancellation, listing, claiming, or reconciliation failure.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SchedulerError {
    /// Request validation depends on the operation's current wall clock.
    Validation(ValidationError),
    /// Durable state could not be read or committed safely.
    Custody(CustodyError),
    /// User-session facility failure; custody remains explicit and retryable.
    Backend(BackendError),
    /// The wall clock moved behind the last committed observation.
    ClockChanged {
        /// Last committed Unix timestamp.
        previous_unix_seconds: u64,
        /// Current Unix timestamp.
        current_unix_seconds: u64,
    },
    /// A new stable id would exceed [`MAX_JOBS`].
    Capacity,
    /// The persisted scheduler-wide generation counter cannot advance.
    GenerationExhausted,
    /// A concurrent operation changed custody before backend completion settled.
    Superseded {
        /// Job whose in-flight operation became stale.
        job: JobId,
        /// Generation submitted to the backend.
        generation: JobGeneration,
    },
    /// Concurrent changes exhausted one reconciliation pass's action bound.
    ReconcileLimit,
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(error) => error.fmt(formatter),
            Self::Custody(error) => error.fmt(formatter),
            Self::Backend(error) => error.fmt(formatter),
            Self::ClockChanged {
                previous_unix_seconds,
                current_unix_seconds,
            } => write!(
                formatter,
                "wall clock moved backward from {previous_unix_seconds} to {current_unix_seconds}"
            ),
            Self::Capacity => formatter.write_str("durable activation job bound reached"),
            Self::GenerationExhausted => {
                formatter.write_str("durable activation generation exhausted")
            }
            Self::Superseded { job, generation } => write!(
                formatter,
                "durable activation {job} generation {} was superseded",
                generation.get()
            ),
            Self::ReconcileLimit => {
                formatter.write_str("durable activation reconciliation action bound reached")
            }
        }
    }
}

impl std::error::Error for SchedulerError {}

impl From<CustodyError> for SchedulerError {
    fn from(error: CustodyError) -> Self {
        Self::Custody(error)
    }
}

impl From<BackendError> for SchedulerError {
    fn from(error: BackendError) -> Self {
        Self::Backend(error)
    }
}

/// Whether scheduling created a job or superseded an existing generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduleOutcome {
    /// No prior custody existed for this job id.
    Scheduled(JobGeneration),
    /// The prior generation became stale before backend publication began.
    Replaced(JobGeneration),
}

/// Whether cancellation found work that could still be withdrawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelOutcome {
    /// No generation is currently in custody for this id.
    NotFound,
    /// Delivery is barred and owned platform artifacts were removed.
    Cancelled(JobGeneration),
    /// Delivery was already committed and cannot be withdrawn.
    AlreadyDelivered(JobGeneration),
}

/// A validated process invocation that can be checked against durable custody.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivationInvocation {
    application: DesktopApplicationId,
    job: JobId,
    generation: JobGeneration,
}

impl ActivationInvocation {
    /// Parses the exact four-argument framework prefix generated by the backend.
    ///
    /// Returns `Ok(None)` when the first argument is not the durable-activation
    /// switch, allowing normal application startup to continue.
    ///
    /// # Errors
    ///
    /// Returns [`InvocationError`] when the switch is present but its identity or generation is invalid.
    pub fn parse<'a>(
        arguments: impl IntoIterator<Item = &'a str>,
    ) -> Result<Option<Self>, InvocationError> {
        let mut arguments = arguments.into_iter();
        if arguments.next() != Some(INVOCATION_SWITCH) {
            return Ok(None);
        }
        let application = arguments
            .next()
            .ok_or(InvocationError::MissingApplication)?;
        let job = arguments.next().ok_or(InvocationError::MissingJob)?;
        let generation = arguments.next().ok_or(InvocationError::MissingGeneration)?;
        if arguments.next().is_some() {
            return Err(InvocationError::UnexpectedArgument);
        }
        let application = DesktopApplicationId::new(application)
            .map_err(|_| InvocationError::InvalidApplication)?;
        let job = JobId::new(job).map_err(|_| InvocationError::InvalidJob)?;
        let generation = generation
            .parse::<u64>()
            .ok()
            .filter(|generation| *generation != 0)
            .ok_or(InvocationError::InvalidGeneration)?;
        Ok(Some(Self {
            application,
            job,
            generation: JobGeneration(generation),
        }))
    }

    /// Returns the identity used to reject cross-application delivery.
    #[must_use]
    pub fn application(&self) -> &DesktopApplicationId {
        &self.application
    }

    /// Returns the stable job selected for claiming.
    #[must_use]
    pub fn job(&self) -> &JobId {
        &self.job
    }

    /// Returns the generation that must still own custody.
    #[must_use]
    pub fn generation(&self) -> JobGeneration {
        self.generation
    }
}

/// Why a process argument sequence cannot identify a durable generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InvocationError {
    /// The application id is absent.
    MissingApplication,
    /// The job id is absent.
    MissingJob,
    /// The generation is absent.
    MissingGeneration,
    /// Extra values followed the four-part framework invocation.
    UnexpectedArgument,
    /// The application id is invalid.
    InvalidApplication,
    /// The job id is invalid.
    InvalidJob,
    /// The generation is not a nonzero integer.
    InvalidGeneration,
}

/// Reason an invocation did not become an application activation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimRefusal {
    /// The invocation belongs to another desktop application id.
    WrongApplication,
    /// No durable record exists for the stable job id.
    UnknownJob,
    /// A replacement advanced the job beyond this generation.
    StaleGeneration,
    /// The current generation is pending removal, delivered, or cancelled.
    Inactive,
    /// The service ran before the absolute instant.
    TooEarly,
    /// The allowed delivery or catch-up window passed.
    MissedWindow,
}

/// Reducer payload returned only after durable consumption succeeds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClaimedActivation {
    /// Job whose activation may now be routed.
    pub job: JobId,
    /// Generation already marked delivered before this value was returned.
    pub generation: JobGeneration,
    /// Original payload, recovered from custody rather than process arguments.
    pub arguments: Vec<String>,
}

/// Decision returned before an application dispatches an activation payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// Delivery custody was consumed; the payload may be routed once.
    Deliver(ClaimedActivation),
    /// Delivery was not admitted and must not reach the reducer.
    Refused(ClaimRefusal),
}

/// A repair already completed and committed during reconciliation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconcileAction {
    /// A missing, incomplete, stale, or rebooted platform timer was republished.
    Republished(JobId, JobGeneration),
    /// A stale or terminal platform artifact was removed.
    Removed(JobId),
    /// An overdue generation was consumed without delivery.
    Expired(JobId, JobGeneration),
}

/// Reboot observation and repairs completed by one reconciliation pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Prior boot id when this pass detected a reboot.
    pub previous_boot_id: Option<String>,
    /// Boot id now committed with custody.
    pub current_boot_id: String,
    /// Repairs completed before the method returned.
    pub actions: Vec<ReconcileAction>,
}

/// Coordinates durable custody with a user-session activation facility.
#[derive(Debug)]
pub struct DurableActivationScheduler<B> {
    application: DesktopApplicationId,
    state_path: PathBuf,
    backend: B,
}

impl<B: ActivationFacility> DurableActivationScheduler<B> {
    /// Creates a scheduler with explicit storage and platform ownership.
    ///
    /// Performs no I/O or backend validation. Later operations publish state
    /// atomically with owner-only permissions. Linux applications normally use
    /// [`systemd::from_env`] to select the XDG locations. The backend must use the
    /// same application identity and honor [`ActivationFacility`] ownership rules.
    #[must_use]
    pub fn new(application: DesktopApplicationId, state_path: PathBuf, backend: B) -> Self {
        Self {
            application,
            state_path,
            backend,
        }
    }

    /// Returns the identity required on every accepted invocation.
    #[must_use]
    pub fn application(&self) -> &DesktopApplicationId {
        &self.application
    }

    /// Returns the file holding all retained custody, including terminal records.
    #[must_use]
    pub fn state_path(&self) -> &Path {
        &self.state_path
    }

    /// Provides read-only access to backend-specific diagnostics.
    #[must_use]
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Provides mutable access for backend-specific reconciliation hooks.
    #[must_use]
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    fn repair_stale_backend(
        &mut self,
        stale_job: &JobId,
        stale_generation: JobGeneration,
    ) -> Result<(), SchedulerError> {
        for _ in 0..MAX_STALE_BACKEND_REPAIRS {
            let observed = {
                let _lock = acquire_lock(&self.state_path)?;
                load_store(&self.state_path)?.jobs.get(stale_job).cloned()
            };
            match observed.as_ref() {
                Some(job)
                    if matches!(
                        job.summary.status,
                        JobStatus::PublicationPending | JobStatus::Scheduled
                    ) =>
                {
                    self.backend.publish(&publication(&self.application, job))?;
                }
                _ => {
                    self.backend.remove(&removal(
                        &self.application,
                        stale_job,
                        stale_generation,
                    ))?;
                }
            }
            let unchanged = {
                let _lock = acquire_lock(&self.state_path)?;
                let store = load_store(&self.state_path)?;
                match (&observed, store.jobs.get(stale_job)) {
                    (None, None) => true,
                    (Some(before), Some(after)) => {
                        before.summary.generation == after.summary.generation
                            && before.summary.status == after.summary.status
                    }
                    _ => false,
                }
            };
            if unchanged {
                return Ok(());
            }
        }
        Err(SchedulerError::ReconcileLimit)
    }

    /// Schedules or replaces one job using the current wall clock.
    ///
    /// `PublicationPending` is durable before backend work begins, and claims
    /// remain available while publication is in flight.
    ///
    /// # Errors
    ///
    /// Returns [`SchedulerError`] for bounds, clock rollback, custody, backend
    /// failure, or a concurrent operation that supersedes settlement. Backend
    /// failure leaves this generation pending unless concurrent work changes it.
    /// Replacing a job invalidates its prior generation before publication, even
    /// if the new publication fails.
    pub fn schedule(
        &mut self,
        request: ActivationRequest,
    ) -> Result<ScheduleOutcome, SchedulerError> {
        self.schedule_at(request, SystemTime::now())
    }

    /// Schedules or replaces one job using `now` for horizon and rollback checks.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::schedule`].
    pub fn schedule_at(
        &mut self,
        request: ActivationRequest,
        now: SystemTime,
    ) -> Result<ScheduleOutcome, SchedulerError> {
        let now = unix_seconds(now).map_err(SchedulerError::Validation)?;
        let instant = unix_seconds(request.instant).map_err(SchedulerError::Validation)?;
        if instant > now.saturating_add(MAX_HORIZON.as_secs()) {
            return Err(SchedulerError::Validation(ValidationError::HorizonExceeded));
        }
        let catch_up = match request.missed {
            MissedActivationPolicy::Skip => 0,
            MissedActivationPolicy::CatchUpWithin(window) => window.as_secs(),
        };
        if now > instant.saturating_add(catch_up) {
            return Err(SchedulerError::Validation(ValidationError::InstantMissed));
        }
        let (job, replacing) = {
            let _lock = acquire_lock(&self.state_path)?;
            let mut store = load_store(&self.state_path)?;
            reject_clock_rollback(&store, now)?;
            let replacing = store.jobs.contains_key(&request.job);
            if !replacing && store.jobs.len() >= MAX_JOBS {
                return Err(SchedulerError::Capacity);
            }
            store.next_generation = store
                .next_generation
                .checked_add(1)
                .ok_or(SchedulerError::GenerationExhausted)?;
            let generation = JobGeneration(store.next_generation);
            let job = StoredJob {
                summary: JobRecord {
                    job: request.job.clone(),
                    generation,
                    unix_seconds: instant,
                    missed: request.missed,
                    status: JobStatus::PublicationPending,
                },
                executable: request.executable,
                arguments: request.arguments,
            };
            store.last_wall_seconds = now;
            store.jobs.insert(request.job, job.clone());
            save_store(&self.state_path, &store)?;
            (job, replacing)
        };

        let boot_id = self.backend.boot_id()?;
        self.backend
            .publish(&publication(&self.application, &job))?;

        let _lock = acquire_lock(&self.state_path)?;
        let mut store = load_store(&self.state_path)?;
        let matches = store.jobs.get(&job.summary.job).is_some_and(|stored| {
            stored.summary.generation == job.summary.generation
                && stored.summary.status == JobStatus::PublicationPending
        });
        if !matches {
            let stale_job = job.summary.job.clone();
            let stale_generation = job.summary.generation;
            drop(store);
            drop(_lock);
            self.repair_stale_backend(&stale_job, stale_generation)?;
            return Err(SchedulerError::Superseded {
                job: stale_job,
                generation: stale_generation,
            });
        }
        if let Some(stored) = store.jobs.get_mut(&job.summary.job) {
            stored.summary.status = JobStatus::Scheduled;
        }
        // Preserve a differing boot identity so reconciliation still re-arms other jobs.
        if store.boot_id.is_empty() {
            store.boot_id = boot_id;
        }
        store.last_wall_seconds = store.last_wall_seconds.max(now);
        save_store(&self.state_path, &store)?;
        Ok(if replacing {
            ScheduleOutcome::Replaced(job.summary.generation)
        } else {
            ScheduleOutcome::Scheduled(job.summary.generation)
        })
    }

    /// Cancels a stable job using the current wall clock.
    ///
    /// `RemovalPending` is durable before backend work begins, so concurrent
    /// claims are refused without waiting for platform cleanup.
    ///
    /// # Errors
    ///
    /// Returns a typed clock, custody, backend, or superseded-settlement failure.
    /// Failed removal keeps delivery barred unless concurrent work replaces the generation.
    pub fn cancel(&mut self, job: &JobId) -> Result<CancelOutcome, SchedulerError> {
        self.cancel_at(job, SystemTime::now())
    }

    /// Cancels a stable job using `now` for rollback detection.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::cancel`].
    pub fn cancel_at(
        &mut self,
        job: &JobId,
        now: SystemTime,
    ) -> Result<CancelOutcome, SchedulerError> {
        let now = unix_seconds(now).map_err(SchedulerError::Validation)?;
        let (generation, delivered) = {
            let _lock = acquire_lock(&self.state_path)?;
            let mut store = load_store(&self.state_path)?;
            reject_clock_rollback(&store, now)?;
            let Some(stored) = store.jobs.get_mut(job) else {
                return Ok(CancelOutcome::NotFound);
            };
            let generation = stored.summary.generation;
            let delivered = stored.summary.status == JobStatus::Delivered;
            stored.summary.status = JobStatus::RemovalPending;
            store.last_wall_seconds = now;
            save_store(&self.state_path, &store)?;
            (generation, delivered)
        };

        self.backend
            .remove(&removal(&self.application, job, generation))?;

        let _lock = acquire_lock(&self.state_path)?;
        let mut store = load_store(&self.state_path)?;
        let matches = store.jobs.get(job).is_some_and(|stored| {
            stored.summary.generation == generation
                && stored.summary.status == JobStatus::RemovalPending
        });
        if !matches {
            return Err(SchedulerError::Superseded {
                job: job.clone(),
                generation,
            });
        }
        store.jobs.remove(job);
        store.last_wall_seconds = store.last_wall_seconds.max(now);
        save_store(&self.state_path, &store)?;
        Ok(if delivered {
            CancelOutcome::AlreadyDelivered(generation)
        } else {
            CancelOutcome::Cancelled(generation)
        })
    }

    /// Lists persisted custody without treating platform files as authoritative.
    ///
    /// # Errors
    ///
    /// Returns a custody error for lock contention, an unreadable or malformed
    /// state file. An absent state file yields an empty list.
    pub fn list(&self) -> Result<Vec<JobRecord>, SchedulerError> {
        let _lock = acquire_lock(&self.state_path)?;
        Ok(load_store(&self.state_path)?
            .jobs
            .into_values()
            .map(|job| job.summary)
            .collect())
    }

    /// Atomically admits one generated invocation at most once.
    ///
    /// A delivered payload is committed before it is returned. If that commit
    /// is uncertain, no payload is returned and retry cannot cause this call to
    /// route twice. Consumption gives at-most-once admission, not guaranteed
    /// reducer delivery. Missed invocations are also marked consumed, and
    /// terminal records remain until cancellation or reconciliation removes them.
    ///
    /// # Errors
    ///
    /// Returns a clock or custody failure without routing a payload.
    pub fn claim(
        &self,
        invocation: &ActivationInvocation,
        now: SystemTime,
    ) -> Result<ClaimOutcome, SchedulerError> {
        if invocation.application != self.application {
            return Ok(ClaimOutcome::Refused(ClaimRefusal::WrongApplication));
        }
        let now = unix_seconds(now).map_err(SchedulerError::Validation)?;
        let _lock = acquire_lock(&self.state_path)?;
        let mut store = load_store(&self.state_path)?;
        reject_clock_rollback(&store, now)?;
        let Some(stored) = store.jobs.get_mut(&invocation.job) else {
            return Ok(ClaimOutcome::Refused(ClaimRefusal::UnknownJob));
        };
        if stored.summary.generation != invocation.generation {
            return Ok(ClaimOutcome::Refused(ClaimRefusal::StaleGeneration));
        }
        if !stored.summary.status.may_deliver() {
            return Ok(ClaimOutcome::Refused(ClaimRefusal::Inactive));
        }
        if now < stored.summary.unix_seconds {
            return Ok(ClaimOutcome::Refused(ClaimRefusal::TooEarly));
        }
        let deadline = stored
            .summary
            .unix_seconds
            .saturating_add(stored.summary.missed.window_secs());
        if now > deadline {
            stored.summary.status = JobStatus::Delivered;
            store.last_wall_seconds = now;
            save_store(&self.state_path, &store)?;
            return Ok(ClaimOutcome::Refused(ClaimRefusal::MissedWindow));
        }
        stored.summary.status = JobStatus::Delivered;
        let claimed = ClaimedActivation {
            job: stored.summary.job.clone(),
            generation: stored.summary.generation,
            arguments: stored.arguments.clone(),
        };
        store.last_wall_seconds = now;
        save_store(&self.state_path, &store)?;
        Ok(ClaimOutcome::Deliver(claimed))
    }

    /// Repairs platform state from durable custody using the current wall clock.
    ///
    /// Each backend operation runs without the custody lock and settles only if
    /// its generation and status still match. A successful pass commits the boot
    /// identity; a failed reboot pass retains the previous identity so eligible
    /// jobs are republished again on retry. Completeness comes from the backend,
    /// rather than independent verification that a timer remains armed. A pass
    /// attempts at most 512 repair steps; earlier repairs persist on failure.
    ///
    /// # Errors
    ///
    /// Returns a typed clock, custody, backend, or action-bound failure.
    pub fn reconcile(&mut self) -> Result<ReconcileReport, SchedulerError> {
        self.reconcile_at(SystemTime::now())
    }

    /// Reconciles durable custody using `now` for expiry and rollback checks.
    ///
    /// # Errors
    ///
    /// Returns the same failures as [`Self::reconcile`].
    pub fn reconcile_at(&mut self, now: SystemTime) -> Result<ReconcileReport, SchedulerError> {
        let now = unix_seconds(now).map_err(SchedulerError::Validation)?;
        let current_boot_id = self.backend.boot_id()?;
        let previous_boot_id = {
            let _lock = acquire_lock(&self.state_path)?;
            let store = load_store(&self.state_path)?;
            reject_clock_rollback(&store, now)?;
            (!store.boot_id.is_empty() && store.boot_id != current_boot_id)
                .then(|| store.boot_id.clone())
        };
        let rebooted = previous_boot_id.is_some();
        let mut attempted_publications = BTreeSet::new();
        let mut actions = Vec::new();

        for _ in 0..MAX_RECONCILE_ACTIONS {
            let installed = self.backend.installed()?;
            let step = {
                let _lock = acquire_lock(&self.state_path)?;
                let store = load_store(&self.state_path)?;
                reject_clock_rollback(&store, now)?;
                next_reconcile_step(&store, &installed, rebooted, &attempted_publications, now)
            };
            let Some(step) = step else {
                let _lock = acquire_lock(&self.state_path)?;
                let mut store = load_store(&self.state_path)?;
                reject_clock_rollback(&store, now)?;
                store.boot_id.clone_from(&current_boot_id);
                store.last_wall_seconds = store.last_wall_seconds.max(now);
                save_store(&self.state_path, &store)?;
                return Ok(ReconcileReport {
                    previous_boot_id,
                    current_boot_id,
                    actions,
                });
            };

            match step {
                ReconcileStep::Publish {
                    job,
                    expected_status,
                } => {
                    attempted_publications
                        .insert((job.summary.job.clone(), job.summary.generation));
                    self.backend
                        .publish(&publication(&self.application, &job))?;
                    let _lock = acquire_lock(&self.state_path)?;
                    let mut store = load_store(&self.state_path)?;
                    if job_matches(&store, &job, expected_status) {
                        if let Some(stored) = store.jobs.get_mut(&job.summary.job) {
                            stored.summary.status = JobStatus::Scheduled;
                        }
                        store.last_wall_seconds = store.last_wall_seconds.max(now);
                        save_store(&self.state_path, &store)?;
                        actions.push(ReconcileAction::Republished(
                            job.summary.job,
                            job.summary.generation,
                        ));
                    }
                }
                ReconcileStep::Remove {
                    job,
                    expected_status,
                } => {
                    self.backend.remove(&removal(
                        &self.application,
                        &job.summary.job,
                        job.summary.generation,
                    ))?;
                    let _lock = acquire_lock(&self.state_path)?;
                    let mut store = load_store(&self.state_path)?;
                    if job_matches(&store, &job, expected_status) {
                        store.jobs.remove(&job.summary.job);
                        store.last_wall_seconds = store.last_wall_seconds.max(now);
                        save_store(&self.state_path, &store)?;
                        actions.push(ReconcileAction::Removed(job.summary.job));
                    }
                }
                ReconcileStep::Expire {
                    job,
                    expected_status,
                } => {
                    self.backend.remove(&removal(
                        &self.application,
                        &job.summary.job,
                        job.summary.generation,
                    ))?;
                    let _lock = acquire_lock(&self.state_path)?;
                    let mut store = load_store(&self.state_path)?;
                    if job_matches(&store, &job, expected_status) {
                        if let Some(stored) = store.jobs.get_mut(&job.summary.job) {
                            stored.summary.status = JobStatus::Delivered;
                        }
                        store.last_wall_seconds = store.last_wall_seconds.max(now);
                        save_store(&self.state_path, &store)?;
                        actions.push(ReconcileAction::Expired(
                            job.summary.job,
                            job.summary.generation,
                        ));
                    }
                }
                ReconcileStep::Orphan(installed) => {
                    self.backend.remove(&ActivationRemoval {
                        application: self.application.clone(),
                        job: installed.job.clone(),
                        generation: installed.generation,
                    })?;
                    actions.push(ReconcileAction::Removed(installed.job));
                }
            }
        }
        Err(SchedulerError::ReconcileLimit)
    }
}

fn next_reconcile_step(
    store: &Store,
    installed: &[InstalledActivation],
    rebooted: bool,
    attempted_publications: &BTreeSet<(JobId, JobGeneration)>,
    now: u64,
) -> Option<ReconcileStep> {
    if let Some(orphan) = installed.iter().find(|installed| {
        store
            .jobs
            .get(&installed.job)
            .is_none_or(|stored| stored.summary.generation != installed.generation)
    }) {
        return Some(ReconcileStep::Orphan(orphan.clone()));
    }
    for job in store.jobs.values() {
        let status = job.summary.status;
        if matches!(status, JobStatus::RemovalPending | JobStatus::Delivered) {
            return Some(ReconcileStep::Remove {
                job: job.clone(),
                expected_status: status,
            });
        }
        let deadline = job
            .summary
            .unix_seconds
            .saturating_add(job.summary.missed.window_secs());
        if now > deadline {
            return Some(ReconcileStep::Expire {
                job: job.clone(),
                expected_status: status,
            });
        }
        let complete = installed.iter().any(|installed| {
            installed.job == job.summary.job
                && installed.generation == job.summary.generation
                && installed.complete
        });
        let attempted =
            attempted_publications.contains(&(job.summary.job.clone(), job.summary.generation));
        if status == JobStatus::PublicationPending || !complete || (rebooted && !attempted) {
            return Some(ReconcileStep::Publish {
                job: job.clone(),
                expected_status: status,
            });
        }
    }
    None
}

fn job_matches(store: &Store, job: &StoredJob, expected_status: JobStatus) -> bool {
    store.jobs.get(&job.summary.job).is_some_and(|stored| {
        stored.summary.generation == job.summary.generation
            && stored.summary.status == expected_status
    })
}

fn removal(
    application: &DesktopApplicationId,
    job: &JobId,
    generation: JobGeneration,
) -> ActivationRemoval {
    ActivationRemoval {
        application: application.clone(),
        job: job.clone(),
        generation,
    }
}

fn publication(application: &DesktopApplicationId, job: &StoredJob) -> ActivationPublication {
    let arguments = vec![
        INVOCATION_SWITCH.to_owned(),
        application.as_str().to_owned(),
        job.summary.job.as_str().to_owned(),
        job.summary.generation.0.to_string(),
    ];
    ActivationPublication {
        application: application.clone(),
        job: job.summary.job.clone(),
        generation: job.summary.generation,
        executable: job.executable.clone(),
        arguments,
        unix_seconds: job.summary.unix_seconds,
        persistent: job.summary.missed.persistent(),
    }
}

fn unix_seconds(time: SystemTime) -> Result<u64, ValidationError> {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| ValidationError::InstantBeforeEpoch)
}

fn reject_clock_rollback(store: &Store, now: u64) -> Result<(), SchedulerError> {
    if store.last_wall_seconds != 0 && now < store.last_wall_seconds {
        Err(SchedulerError::ClockChanged {
            previous_unix_seconds: store.last_wall_seconds,
            current_unix_seconds: now,
        })
    } else {
        Ok(())
    }
}

fn acquire_lock(path: &Path) -> Result<atomic_state::StateLock, CustodyError> {
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        match atomic_state::StateLock::try_acquire(path) {
            Ok(Some(lock)) => return Ok(lock),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                return Err(CustodyError {
                    kind: CustodyErrorKind::Busy,
                    detail: "state lock wait exceeded two seconds".into(),
                });
            }
            Err(error) => return Err(CustodyError::io(error, false)),
        }
    }
}

fn load_store(path: &Path) -> Result<Store, CustodyError> {
    let (loaded, refusal) =
        atomic_state::load_bounded(path, STATE_FORMAT, STATE_VERSION, MAX_STATE_BYTES);
    match (loaded, refusal) {
        (StateLoad::Loaded(body), None) => parse_store(&body),
        (StateLoad::UsedDefaults { .. }, Some(StateRefusal::Absent)) => Ok(Store::default()),
        (StateLoad::UsedDefaults { note }, Some(_)) => Err(CustodyError::corrupt(note)),
        (StateLoad::UsedDefaults { note }, None) => Err(CustodyError::corrupt(note)),
        (StateLoad::Loaded(_), Some(reason)) => Err(CustodyError::corrupt(format!(
            "loader returned state with refusal {reason:?}"
        ))),
    }
}

fn save_store(path: &Path, store: &Store) -> Result<(), CustodyError> {
    atomic_state::save_atomic(path, STATE_FORMAT, STATE_VERSION, &encode_store(store))
        .map_err(|error| CustodyError::io(error, true))
}

fn encode_store(store: &Store) -> String {
    let mut body = format!(
        "meta\t{}\t{}\t{}\n",
        hex_encode(store.boot_id.as_bytes()),
        store.last_wall_seconds,
        store.next_generation,
    );
    for job in store.jobs.values() {
        use std::fmt::Write as _;
        let policy = match job.summary.missed {
            MissedActivationPolicy::Skip => "s".to_owned(),
            MissedActivationPolicy::CatchUpWithin(window) => format!("c{}", window.as_secs()),
        };
        let status = match job.summary.status {
            JobStatus::PublicationPending => "p",
            JobStatus::Scheduled => "s",
            JobStatus::RemovalPending => "r",
            JobStatus::Delivered => "d",
        };
        let _ = write!(
            body,
            "job\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            hex_encode(job.summary.job.as_str().as_bytes()),
            job.summary.generation.0,
            job.summary.unix_seconds,
            policy,
            status,
            hex_encode(job.executable.to_string_lossy().as_bytes()),
            job.arguments.len(),
        );
        for argument in &job.arguments {
            let _ = write!(body, "\t{}", hex_encode(argument.as_bytes()));
        }
        body.push('\n');
    }
    body
}

fn parse_store(body: &str) -> Result<Store, CustodyError> {
    if body.len() as u64 > MAX_STATE_BYTES {
        return Err(CustodyError::corrupt("state body exceeds activation bound"));
    }
    let mut lines = body.lines();
    let Some(meta) = lines.next() else {
        return Err(CustodyError::corrupt("state metadata is absent"));
    };
    let meta: Vec<_> = meta.split('\t').collect();
    if meta.len() != 4 || meta[0] != "meta" {
        return Err(CustodyError::corrupt("invalid state metadata"));
    }
    let boot_id = String::from_utf8(hex_decode(meta[1])?)
        .map_err(|_| CustodyError::corrupt("boot id is not UTF-8"))?;
    let last_wall_seconds = parse_u64(meta[2], "last wall timestamp")?;
    let next_generation = parse_u64(meta[3], "next generation")?;
    let mut jobs = BTreeMap::new();
    for line in lines {
        if jobs.len() >= MAX_JOBS {
            return Err(CustodyError::corrupt("state exceeds job bound"));
        }
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() < 8 || fields[0] != "job" {
            return Err(CustodyError::corrupt("invalid job record"));
        }
        let job_text = String::from_utf8(hex_decode(fields[1])?)
            .map_err(|_| CustodyError::corrupt("job id is not UTF-8"))?;
        let job =
            JobId::new(job_text).map_err(|_| CustodyError::corrupt("invalid stored job id"))?;
        let generation = JobGeneration(parse_u64(fields[2], "generation")?);
        if generation.0 == 0 {
            return Err(CustodyError::corrupt("zero generation"));
        }
        let unix_seconds = parse_u64(fields[3], "instant")?;
        let missed = if fields[4] == "s" {
            MissedActivationPolicy::Skip
        } else if let Some(window) = fields[4].strip_prefix('c') {
            let window = Duration::from_secs(parse_u64(window, "catch-up window")?);
            if window > MAX_CATCH_UP {
                return Err(CustodyError::corrupt("catch-up window exceeds bound"));
            }
            MissedActivationPolicy::CatchUpWithin(window)
        } else {
            return Err(CustodyError::corrupt("invalid missed-instant policy"));
        };
        let status = match fields[5] {
            "p" => JobStatus::PublicationPending,
            "s" => JobStatus::Scheduled,
            "r" => JobStatus::RemovalPending,
            "d" => JobStatus::Delivered,
            _ => return Err(CustodyError::corrupt("invalid job status")),
        };
        let executable = String::from_utf8(hex_decode(fields[6])?)
            .map_err(|_| CustodyError::corrupt("executable is not UTF-8"))?;
        let argument_count = fields[7]
            .parse::<usize>()
            .map_err(|_| CustodyError::corrupt("invalid argument count"))?;
        if argument_count > MAX_ARGUMENTS || fields.len() != 8 + argument_count {
            return Err(CustodyError::corrupt("argument count violates bound"));
        }
        let arguments: Vec<String> = fields[8..]
            .iter()
            .map(|field| {
                String::from_utf8(hex_decode(field)?)
                    .map_err(|_| CustodyError::corrupt("argument is not UTF-8"))
            })
            .collect::<Result<_, _>>()?;
        if arguments.iter().map(String::len).sum::<usize>() > MAX_ARGUMENT_BYTES {
            return Err(CustodyError::corrupt("argument bytes exceed bound"));
        }
        let stored = StoredJob {
            summary: JobRecord {
                job: job.clone(),
                generation,
                unix_seconds,
                missed,
                status,
            },
            executable: PathBuf::from(executable),
            arguments,
        };
        if jobs.insert(job, stored).is_some() {
            return Err(CustodyError::corrupt("duplicate job id"));
        }
    }
    if jobs
        .values()
        .any(|job| job.summary.generation.0 > next_generation)
    {
        return Err(CustodyError::corrupt(
            "job generation exceeds the global generation",
        ));
    }
    Ok(Store {
        boot_id,
        last_wall_seconds,
        next_generation,
        jobs,
    })
}

fn parse_u64(value: &str, field: &str) -> Result<u64, CustodyError> {
    value
        .parse()
        .map_err(|_| CustodyError::corrupt(format!("invalid {field}")))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn hex_decode(value: &str) -> Result<Vec<u8>, CustodyError> {
    if !value.len().is_multiple_of(2) {
        return Err(CustodyError::corrupt("odd-length hexadecimal field"));
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let high = hex_nibble(pair[0])
                .ok_or_else(|| CustodyError::corrupt("invalid hexadecimal field"))?;
            let low = hex_nibble(pair[1])
                .ok_or_else(|| CustodyError::corrupt("invalid hexadecimal field"))?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn bounded_diagnostic(mut value: String) -> String {
    if value.len() <= MAX_DIAGNOSTIC_BYTES {
        return value;
    }
    let mut end = MAX_DIAGNOSTIC_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value
}

#[cfg(test)]
mod diagnostic_bound_tests {}
