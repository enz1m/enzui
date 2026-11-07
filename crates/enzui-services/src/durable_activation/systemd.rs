//! Persistent one-shot activation through the Linux user-systemd manager.
//!
//! Service and timer files are atomically published with matching app, job and
//! generation markers. Publication requires exact current-user ownership and
//! mode 0600 for existing targets. Removal retains inspected descriptors through
//! identity-checked unlink. Ordinary cleanup checks the manager's unit identity
//! and fragment path before stopping a timer; failed-publication rollback uses
//! the changed timer file as its authority instead.
//!
//! Manager commands have separate ten-second deadlines and 8 KiB stdout/stderr
//! limits, use a cleared environment with explicit additions, and do not invoke
//! a shell. A transaction may run several commands and filesystem operations;
//! these per-command limits do not bound its total duration. Installed-artifact
//! enumeration checks files, without verifying manager load or activation state.

use super::{
    ActivationFacility, ActivationPublication, ActivationRemoval, BackendError, BackendErrorKind,
    BackendStage, DesktopApplicationId, DurableActivationScheduler, InstalledActivation,
    JobGeneration, JobId,
};
use crate::cancellation::CancelToken;
use crate::process_transport::{ProcessErrorKind, ProcessRequest, StdinPolicy};
use crate::state_authority::{PublishCustody, PublishError, StateAuthority};
use enzui_security::secure_dir::SecureRoot;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::str;
use std::time::Duration;

const HELPER_DEADLINE: Duration = Duration::from_secs(10);
const HELPER_OUTPUT_LIMIT: usize = 8 * 1024;
const STATE_FILE: &str = "durable-activation/jobs.state";
const UNIT_MARKER_FORMAT: &str = "enzui-durable-activation";

#[derive(Clone, Debug, PartialEq, Eq)]
struct UnitMarker {
    application: DesktopApplicationId,
    job: JobId,
    generation: JobGeneration,
}

#[derive(Debug)]
struct OwnedUnit {
    path: PathBuf,
    marker: UnitMarker,
    file: File,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ManagerTimerState {
    Absent,
    Loaded,
}

#[derive(Debug)]
enum ExistingTarget {
    Absent,
    Managed(Vec<u8>),
}

#[derive(Debug)]
struct PublicationAuthority {
    previous: ExistingTarget,
    state: StateAuthority,
}

#[derive(Debug)]
struct PublicationTargets {
    parent: File,
    service_path: PathBuf,
    service_authority: PublicationAuthority,
    service_written: bool,
    timer_path: PathBuf,
    timer_authority: Option<PublicationAuthority>,
    timer_written: bool,
}

/// Failure to resolve the user's durable state or systemd configuration root.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum SystemdSetupError {
    /// No absolute XDG state or home directory is available.
    NoStateHome,
    /// No absolute XDG config or home directory is available.
    NoConfigHome,
}

impl std::fmt::Display for SystemdSetupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoStateHome => formatter.write_str("no absolute XDG state directory or home"),
            Self::NoConfigHome => formatter.write_str("no absolute XDG config directory or home"),
        }
    }
}

impl std::error::Error for SystemdSetupError {}

/// User-systemd backend with marker-guarded unit ownership.
#[derive(Debug)]
pub struct SystemdUserBackend {
    application: DesktopApplicationId,
    unit_directory: PathBuf,
    systemctl: PathBuf,
    environment: Vec<(OsString, OsString)>,
    boot_id_path: PathBuf,
    removal_recheck_hook: Option<fn(&Path)>,
    publication_hook: Option<fn(&Path)>,
    directory_sync_hook: Option<fn(&Path) -> io::Result<()>>,
}

impl SystemdUserBackend {
    /// Creates a backend with explicit unit, helper, environment, and boot-id paths.
    ///
    /// Child commands receive only `environment`. Existing unit paths must be
    /// regular files with matching bounded ownership metadata; publication also
    /// requires current-user ownership and mode 0600. Unit and boot-id paths are
    /// not inspected here. The executable path is checked but not canonicalized
    /// or retained as a descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`BackendErrorKind::Unavailable`] when `systemctl` is not an
    /// absolute executable, or a typed filesystem error when it cannot be inspected.
    pub fn new(
        application: DesktopApplicationId,
        unit_directory: PathBuf,
        systemctl: PathBuf,
        environment: Vec<(OsString, OsString)>,
        boot_id_path: PathBuf,
    ) -> Result<Self, BackendError> {
        if !systemctl.is_absolute() {
            return Err(BackendError::new(
                BackendErrorKind::Unavailable,
                BackendStage::Reload,
                false,
                true,
                "systemctl path is not absolute",
            ));
        }
        let metadata = std::fs::metadata(&systemctl)
            .map_err(|error| backend_io_error(BackendStage::Reload, false, true, error))?;
        if !super::is_executable_file(&metadata) {
            return Err(BackendError::new(
                BackendErrorKind::Unavailable,
                BackendStage::Reload,
                false,
                true,
                "systemctl path is not a regular executable",
            ));
        }
        Ok(Self {
            application,
            unit_directory,
            systemctl,
            environment,
            boot_id_path,
            removal_recheck_hook: None,
            publication_hook: None,
            directory_sync_hook: None,
        })
    }

    /// Runs `hook` before removal or rollback identity checks, after retaining authority.
    #[doc(hidden)]
    #[must_use]
    pub fn with_removal_recheck_hook(mut self, hook: fn(&Path)) -> Self {
        self.removal_recheck_hook = Some(hook);
        self
    }

    /// Runs `hook` after publication authority is retained and before commit.
    #[doc(hidden)]
    #[must_use]
    pub fn with_publication_hook(mut self, hook: fn(&Path)) -> Self {
        self.publication_hook = Some(hook);
        self
    }

    /// Runs `hook` when cleanup synchronizes the unit directory.
    #[doc(hidden)]
    #[must_use]
    pub fn with_directory_sync_hook(mut self, hook: fn(&Path) -> io::Result<()>) -> Self {
        self.directory_sync_hook = Some(hook);
        self
    }

    /// Returns the directory guarded by ownership-marker checks.
    #[must_use]
    pub fn unit_directory(&self) -> &Path {
        &self.unit_directory
    }

    fn sync_unit_directory(&self) -> io::Result<()> {
        if let Some(hook) = self.directory_sync_hook {
            hook(&self.unit_directory)
        } else {
            sync_directory(&self.unit_directory)
        }
    }

    fn unit_stem(&self, job: &JobId) -> String {
        format!(
            "enzui-activation-a{}-{}-j{}-{}",
            self.application.as_str().len(),
            self.application.as_str(),
            job.as_str().len(),
            job.as_str(),
        )
    }

    fn service_path(&self, job: &JobId, generation: JobGeneration) -> PathBuf {
        self.unit_directory.join(format!(
            "{}.g{}.service",
            self.unit_stem(job),
            generation.get()
        ))
    }

    fn timer_path(&self, job: &JobId, generation: JobGeneration) -> PathBuf {
        self.unit_directory.join(format!(
            "{}.g{}.timer",
            self.unit_stem(job),
            generation.get()
        ))
    }

    fn timer_name(&self, job: &JobId, generation: JobGeneration) -> String {
        format!("{}.g{}.timer", self.unit_stem(job), generation.get())
    }

    fn run_systemctl_output(
        &self,
        stage: BackendStage,
        arguments: impl IntoIterator<Item = OsString>,
        partial: bool,
    ) -> Result<Vec<u8>, BackendError> {
        let request = ProcessRequest::new(
            self.systemctl.as_os_str(),
            StdinPolicy::Null,
            HELPER_OUTPUT_LIMIT,
            HELPER_OUTPUT_LIMIT,
            HELPER_DEADLINE,
        )
        .arg("--user")
        .args(arguments)
        .envs(self.environment.clone());
        match crate::process_transport::run(request, &CancelToken::new()) {
            Ok(output) if output.status.success() => Ok(output.stdout),
            Ok(output) => {
                let detail = String::from_utf8_lossy(if output.stderr.is_empty() {
                    &output.stdout
                } else {
                    &output.stderr
                });
                let kind = classify_diagnostic(&detail);
                Err(BackendError::new(kind, stage, partial, false, detail))
            }
            Err(error) => {
                let kind = match error.kind() {
                    ProcessErrorKind::Deadline => BackendErrorKind::TimedOut,
                    ProcessErrorKind::Spawn | ProcessErrorKind::ExecutableAuthority => {
                        BackendErrorKind::Unavailable
                    }
                    _ => BackendErrorKind::Failed,
                };
                Err(BackendError::new(
                    kind,
                    stage,
                    partial,
                    false,
                    error.to_string(),
                ))
            }
        }
    }

    fn run_systemctl(
        &self,
        stage: BackendStage,
        arguments: impl IntoIterator<Item = OsString>,
        partial: bool,
    ) -> Result<(), BackendError> {
        self.run_systemctl_output(stage, arguments, partial)
            .map(drop)
    }

    fn manager_timer_state(
        &self,
        removal: &ActivationRemoval,
    ) -> Result<ManagerTimerState, BackendError> {
        let timer_name = self.timer_name(&removal.job, removal.generation);
        let output = self.run_systemctl_output(
            BackendStage::Remove,
            [
                OsString::from("show"),
                OsString::from("--no-pager"),
                OsString::from("--property=Id"),
                OsString::from("--property=LoadState"),
                OsString::from("--property=FragmentPath"),
                OsString::from(timer_name.clone()),
            ],
            false,
        )?;
        let output = str::from_utf8(&output).map_err(|_| {
            BackendError::new(
                BackendErrorKind::Failed,
                BackendStage::Remove,
                false,
                false,
                "systemd manager returned non-UTF-8 unit properties",
            )
        })?;
        let mut id = None;
        let mut load_state = None;
        let mut fragment_path = None;
        for line in output.lines() {
            let Some((name, value)) = line.split_once('=') else {
                return Err(manager_state_error("systemd manager property is malformed"));
            };
            let destination = match name {
                "Id" => &mut id,
                "LoadState" => &mut load_state,
                "FragmentPath" => &mut fragment_path,
                _ => continue,
            };
            if destination.replace(value).is_some() {
                return Err(manager_state_error(
                    "systemd manager property is duplicated",
                ));
            }
        }
        let (Some(id), Some(load_state), Some(fragment_path)) = (id, load_state, fragment_path)
        else {
            return Err(manager_state_error(
                "systemd manager properties are incomplete",
            ));
        };
        if id != timer_name {
            return Err(collision_error(
                BackendStage::Remove,
                "systemd manager unit identity does not match the requested timer",
            ));
        }
        if load_state == "not-found" {
            return if fragment_path.is_empty() {
                Ok(ManagerTimerState::Absent)
            } else {
                Err(collision_error(
                    BackendStage::Remove,
                    "absent systemd timer reports a fragment path",
                ))
            };
        }
        if load_state != "loaded" {
            return Err(collision_error(
                BackendStage::Remove,
                "systemd timer load state is neither loaded nor absent",
            ));
        }
        let expected_path = self.timer_path(&removal.job, removal.generation);
        if fragment_path.is_empty() || Path::new(fragment_path) != expected_path.as_path() {
            return Err(collision_error(
                BackendStage::Remove,
                "systemd timer fragment path does not match the generated unit",
            ));
        }
        Ok(ManagerTimerState::Loaded)
    }

    fn inspect_target(
        &self,
        path: &Path,
        job: &JobId,
        allowed_generations: &[JobGeneration],
        stage: BackendStage,
    ) -> Result<ExistingTarget, BackendError> {
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(ExistingTarget::Absent),
            Err(error) => Err(collision_error(stage, error.to_string())),
            Ok(metadata) if !metadata.file_type().is_file() => Err(collision_error(
                stage,
                "generated unit path is not a regular file",
            )),
            Ok(_) => {
                let contents = read_managed_unit(path)
                    .map_err(|error| collision_error(stage, error.to_string()))?;
                let marker = parse_unit_marker(&contents)
                    .ok_or_else(|| collision_error(stage, "unit ownership marker is invalid"))?;
                if marker.application != self.application
                    || marker.job != *job
                    || (!allowed_generations.is_empty()
                        && !allowed_generations.contains(&marker.generation))
                {
                    return Err(collision_error(
                        stage,
                        "unit ownership marker does not match this publication",
                    ));
                }
                Ok(ExistingTarget::Managed(contents.into_bytes()))
            }
        }
    }

    fn publication_root(&self) -> Result<(SecureRoot, File), BackendError> {
        use std::os::unix::fs::MetadataExt;

        let root = SecureRoot::open(&self.unit_directory)
            .map_err(|error| collision_error(BackendStage::ServiceFile, error.to_string()))?;
        let parent = File::open(&self.unit_directory)
            .map_err(|error| collision_error(BackendStage::ServiceFile, error.to_string()))?;
        root.revalidate()
            .map_err(|error| collision_error(BackendStage::ServiceFile, error.to_string()))?;
        let metadata = parent
            .metadata()
            .map_err(|error| collision_error(BackendStage::ServiceFile, error.to_string()))?;
        if !metadata.is_dir() || (metadata.dev(), metadata.ino()) != root.identity() {
            return Err(collision_error(
                BackendStage::ServiceFile,
                "unit directory identity changed while retaining publication authority",
            ));
        }
        Ok((root, parent))
    }

    fn retain_publication_target(
        &self,
        root: &SecureRoot,
        parent: &File,
        path: &Path,
        job: &JobId,
        generation: JobGeneration,
        stage: BackendStage,
    ) -> Result<PublicationAuthority, BackendError> {
        use std::os::unix::fs::MetadataExt;

        let name = path
            .file_name()
            .ok_or_else(|| collision_error(stage, "generated unit path has no file name"))?;
        match root.open_entry_read(name) {
            Ok(mut file) => {
                let metadata = file
                    .metadata()
                    .map_err(|error| collision_error(stage, error.to_string()))?;
                if !metadata.is_file() {
                    return Err(collision_error(
                        stage,
                        "generated unit path is not a regular file",
                    ));
                }
                if metadata.uid() != rustix::process::geteuid().as_raw()
                    || metadata.gid() != rustix::process::getegid().as_raw()
                    || metadata.mode() & 0o7777 != 0o600
                {
                    return Err(collision_error(
                        stage,
                        "generated unit is not exact owner-only current-user state",
                    ));
                }
                let contents = read_held_managed_unit(&mut file)
                    .map_err(|error| collision_error(stage, error.to_string()))?;
                let marker = parse_unit_marker(&contents)
                    .ok_or_else(|| collision_error(stage, "unit ownership marker is invalid"))?;
                if marker.application != self.application
                    || marker.job != *job
                    || marker.generation != generation
                {
                    return Err(collision_error(
                        stage,
                        "unit ownership marker does not match this publication",
                    ));
                }
                let state = StateAuthority::open_existing(path)
                    .map_err(|error| collision_error(stage, error.to_string()))?;
                let identity = state.identity().ok_or_else(|| {
                    collision_error(stage, "existing state authority has no identity")
                })?;
                if (identity.device(), identity.inode()) != (metadata.dev(), metadata.ino()) {
                    return Err(collision_error(
                        stage,
                        "state authority does not own the marker-verified unit",
                    ));
                }
                enzui_security::tempstage::verify_path_names_file(parent, &self.unit_directory)
                    .map_err(|error| collision_error(stage, error.to_string()))?;
                Ok(PublicationAuthority {
                    previous: ExistingTarget::Managed(contents.into_bytes()),
                    state,
                })
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let state = StateAuthority::create_new(path)
                    .map_err(|error| collision_error(stage, error.to_string()))?;
                enzui_security::tempstage::verify_path_names_file(parent, &self.unit_directory)
                    .map_err(|error| collision_error(stage, error.to_string()))?;
                Ok(PublicationAuthority {
                    previous: ExistingTarget::Absent,
                    state,
                })
            }
            Err(error) => Err(collision_error(stage, error.to_string())),
        }
    }

    fn publish_authorized_target(
        &self,
        path: &Path,
        authority: &mut PublicationAuthority,
        contents: &[u8],
        stage: BackendStage,
    ) -> Result<(), BackendError> {
        if let Some(hook) = self.publication_hook {
            hook(path);
        }
        authority
            .state
            .publish(contents)
            .map(|_| ())
            .map_err(|error| publication_error(stage, error))
    }

    fn owned_removal_paths(
        &self,
        removal: &ActivationRemoval,
    ) -> Result<Vec<OwnedUnit>, BackendError> {
        if removal.application != self.application {
            return Err(collision_error(
                BackendStage::Remove,
                "removal application does not match this backend",
            ));
        }
        let mut paths = Vec::new();
        let service_path = self.service_path(&removal.job, removal.generation);
        match self.inspect_target(
            &service_path,
            &removal.job,
            &[removal.generation],
            BackendStage::Remove,
        )? {
            ExistingTarget::Absent => {}
            ExistingTarget::Managed(_) => {
                let (mut file, contents) = open_owned_managed_unit(&service_path)
                    .map_err(|error| collision_error(BackendStage::Remove, error.to_string()))?;
                let marker = parse_unit_marker(&contents).ok_or_else(|| {
                    collision_error(BackendStage::Remove, "unit ownership marker is invalid")
                })?;
                if marker.application != self.application
                    || marker.job != removal.job
                    || marker.generation != removal.generation
                {
                    return Err(collision_error(
                        BackendStage::Remove,
                        "service ownership changed before removal",
                    ));
                }
                file.seek(SeekFrom::Start(0))
                    .map_err(|error| collision_error(BackendStage::Remove, error.to_string()))?;
                paths.push(OwnedUnit {
                    path: service_path,
                    marker,
                    file,
                });
            }
        }
        let timer_path = self.timer_path(&removal.job, removal.generation);
        match std::fs::symlink_metadata(&timer_path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(collision_error(BackendStage::Remove, error.to_string())),
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(collision_error(
                    BackendStage::Remove,
                    "generated timer path is not a regular file",
                ));
            }
            Ok(_) => {
                let (mut file, contents) = open_owned_managed_unit(&timer_path)
                    .map_err(|error| collision_error(BackendStage::Remove, error.to_string()))?;
                let marker = parse_unit_marker(&contents).ok_or_else(|| {
                    collision_error(BackendStage::Remove, "unit ownership marker is invalid")
                })?;
                if marker.application != self.application || marker.job != removal.job {
                    return Err(collision_error(
                        BackendStage::Remove,
                        "unit ownership marker does not match this scheduler and job",
                    ));
                }
                if marker.generation != removal.generation {
                    return Err(collision_error(
                        BackendStage::Remove,
                        "timer filename and ownership marker disagree",
                    ));
                }
                let expected = format!(
                    "Unit={}.g{}.service",
                    self.unit_stem(&removal.job),
                    marker.generation.get()
                );
                if !contents.lines().any(|line| line == expected) {
                    return Err(collision_error(
                        BackendStage::Remove,
                        "timer generation marker and service target disagree",
                    ));
                }
                file.seek(SeekFrom::Start(0))
                    .map_err(|error| collision_error(BackendStage::Remove, error.to_string()))?;
                paths.push(OwnedUnit {
                    path: timer_path,
                    marker,
                    file,
                });
            }
        }
        Ok(paths)
    }

    fn remove_owned_paths(&self, paths: &mut [OwnedUnit]) -> Result<(), BackendError> {
        let mut removed_any = false;
        for owned in paths {
            let contents = read_held_managed_unit(&mut owned.file).map_err(|error| {
                cleanup_error(
                    collision_error(BackendStage::Remove, error.to_string()),
                    removed_any,
                )
            })?;
            if parse_unit_marker(&contents).as_ref() != Some(&owned.marker) {
                return Err(cleanup_error(
                    collision_error(
                        BackendStage::Remove,
                        "unit ownership changed before removal",
                    ),
                    removed_any,
                ));
            }
            if let Some(hook) = self.removal_recheck_hook {
                hook(&owned.path);
            }
            if let Err(error) =
                enzui_security::tempstage::remove_if_path_names_file(&owned.path, &owned.file)
            {
                let error = match enzui_security::tempstage::verify_path_names_file(
                    &owned.file,
                    &owned.path,
                ) {
                    Err(identity) if identity.kind() == io::ErrorKind::PermissionDenied => {
                        collision_error(BackendStage::Remove, identity.to_string())
                    }
                    _ => backend_io_error(BackendStage::Remove, false, false, error),
                };
                return Err(cleanup_error(error, removed_any));
            }
            removed_any = true;
        }
        Ok(())
    }

    fn cleanup_owned_generation(
        &self,
        removal: &ActivationRemoval,
        owned: &mut [OwnedUnit],
    ) -> Result<(), BackendError> {
        let manager_state = self.manager_timer_state(removal)?;
        let stopped = manager_state == ManagerTimerState::Loaded;
        if stopped {
            self.run_systemctl(
                BackendStage::Stop,
                [
                    OsString::from("disable"),
                    OsString::from("--now"),
                    OsString::from(self.timer_name(&removal.job, removal.generation)),
                ],
                true,
            )?;
        }
        if let Err(error) = self.remove_owned_paths(owned) {
            let error = cleanup_error(error, stopped);
            if error.is_partial_publication()
                && let Err(sync) = self.sync_unit_directory()
                && error.kind != BackendErrorKind::Collision
            {
                return Err(cleanup_error(
                    backend_io_error(BackendStage::Remove, true, false, sync),
                    true,
                ));
            }
            return Err(error);
        }
        let changed = stopped || !owned.is_empty();
        self.sync_unit_directory().map_err(|error| {
            cleanup_error(
                backend_io_error(BackendStage::Remove, changed, false, error),
                changed,
            )
        })?;
        self.run_systemctl(
            BackendStage::Reload,
            [OsString::from("daemon-reload")],
            changed,
        )
    }

    fn obsolete_unit_paths(
        &self,
        job: &JobId,
        keep: JobGeneration,
    ) -> Result<Vec<OwnedUnit>, BackendError> {
        let entries = match std::fs::read_dir(&self.unit_directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(backend_io_error(BackendStage::Remove, false, false, error));
            }
        };
        let prefix = format!("{}.g", self.unit_stem(job));
        let keep_service = format!("{prefix}{}.service", keep.get());
        let keep_timer = format!("{prefix}{}.timer", keep.get());
        let mut obsolete = Vec::new();
        for entry in entries {
            let entry = entry
                .map_err(|error| backend_io_error(BackendStage::Remove, false, false, error))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if name == keep_service || name == keep_timer || !name.starts_with(&prefix) {
                continue;
            }
            let (generation, is_timer) = name
                .strip_prefix(&prefix)
                .and_then(|name| {
                    name.strip_suffix(".service")
                        .map(|generation| (generation, false))
                        .or_else(|| {
                            name.strip_suffix(".timer")
                                .map(|generation| (generation, true))
                        })
                })
                .and_then(|(generation, is_timer)| {
                    generation
                        .parse::<u64>()
                        .ok()
                        .and_then(JobGeneration::new)
                        .map(|generation| (generation, is_timer))
                })
                .ok_or_else(|| {
                    collision_error(BackendStage::Remove, "obsolete unit name is invalid")
                })?;
            let (mut file, contents) = open_owned_managed_unit(&entry.path())
                .map_err(|error| collision_error(BackendStage::Remove, error.to_string()))?;
            let marker = parse_unit_marker(&contents).ok_or_else(|| {
                collision_error(BackendStage::Remove, "unit ownership marker is invalid")
            })?;
            if marker.application != self.application
                || marker.job != *job
                || marker.generation != generation
            {
                return Err(collision_error(
                    BackendStage::Remove,
                    "obsolete unit filename and ownership marker disagree",
                ));
            }
            if generation > keep {
                continue;
            }
            if is_timer {
                let expected =
                    format!("Unit={}.g{}.service", self.unit_stem(job), generation.get());
                if !contents.lines().any(|line| line == expected) {
                    return Err(collision_error(
                        BackendStage::Remove,
                        "obsolete timer target and ownership marker disagree",
                    ));
                }
            }
            file.seek(SeekFrom::Start(0))
                .map_err(|error| collision_error(BackendStage::Remove, error.to_string()))?;
            obsolete.push(OwnedUnit {
                path: entry.path(),
                marker,
                file,
            });
        }
        Ok(obsolete)
    }

    fn rollback_publication(
        &self,
        publication: &ActivationPublication,
        targets: &mut PublicationTargets,
    ) -> Result<(), BackendError> {
        let cleanup_needed = targets.service_written || targets.timer_written;
        if !cleanup_needed {
            return Ok(());
        }
        let stop_error = if targets.timer_written
            && targets.timer_authority.as_ref().is_some_and(|authority| {
                target_was_changed(&targets.timer_path, &authority.previous, publication)
            }) {
            self.run_systemctl(
                BackendStage::Stop,
                [
                    OsString::from("disable"),
                    OsString::from("--now"),
                    self.timer_name(&publication.job, publication.generation)
                        .into(),
                ],
                true,
            )
            .err()
        } else {
            None
        };
        let service_error = rollback_target(
            &targets.parent,
            &targets.service_path,
            &mut targets.service_authority,
            targets.service_written,
            publication,
            self.removal_recheck_hook,
        )
        .err();
        let parent = &targets.parent;
        let timer_path = &targets.timer_path;
        let timer_written = targets.timer_written;
        let timer_error = targets.timer_authority.as_mut().and_then(|authority| {
            rollback_target(
                parent,
                timer_path,
                authority,
                timer_written,
                publication,
                self.removal_recheck_hook,
            )
            .err()
        });
        let reload_error = self
            .run_systemctl(
                BackendStage::Reload,
                [OsString::from("daemon-reload")],
                true,
            )
            .err();
        let restore_error = match (service_error, timer_error) {
            (Some(_), Some(timer)) if timer.kind == BackendErrorKind::Collision => Some(timer),
            (Some(service), _) => Some(service),
            (None, timer) => timer,
        };
        restore_error
            .or(stop_error)
            .or(reload_error)
            .map_or(Ok(()), Err)
    }

    fn fail_and_rollback(
        &self,
        publication: &ActivationPublication,
        targets: &mut PublicationTargets,
        mut error: BackendError,
    ) -> BackendError {
        error.partial_publication |= targets.service_written || targets.timer_written;
        match self.rollback_publication(publication, targets) {
            Ok(()) if error.kind != BackendErrorKind::Collision => error.cleanup_complete = true,
            Ok(()) => error.cleanup_complete = false,
            Err(mut rollback) if rollback.kind == BackendErrorKind::Collision => {
                rollback.partial_publication = true;
                rollback.cleanup_complete = false;
                return rollback;
            }
            Err(_) => error.cleanup_complete = false,
        }
        error
    }
}

impl ActivationFacility for SystemdUserBackend {
    fn publish(&mut self, publication: &ActivationPublication) -> Result<(), BackendError> {
        if publication.application != self.application {
            return Err(BackendError::new(
                BackendErrorKind::Failed,
                BackendStage::ServiceFile,
                false,
                true,
                "publication desktop id does not match backend identity",
            ));
        }
        let service_path = self.service_path(&publication.job, publication.generation);
        let timer_path = self.timer_path(&publication.job, publication.generation);
        let obsolete = self.obsolete_unit_paths(&publication.job, publication.generation)?;
        let (root, parent) = self.publication_root()?;
        let service_authority = self.retain_publication_target(
            &root,
            &parent,
            &service_path,
            &publication.job,
            publication.generation,
            BackendStage::ServiceFile,
        )?;
        let timer_authority = match self.retain_publication_target(
            &root,
            &parent,
            &timer_path,
            &publication.job,
            publication.generation,
            BackendStage::TimerFile,
        ) {
            Ok(authority) => authority,
            Err(error) => {
                let mut targets = PublicationTargets {
                    parent,
                    service_path,
                    service_authority,
                    service_written: false,
                    timer_path,
                    timer_authority: None,
                    timer_written: false,
                };
                return Err(self.fail_and_rollback(publication, &mut targets, error));
            }
        };
        let mut targets = PublicationTargets {
            parent,
            service_path,
            service_authority,
            service_written: false,
            timer_path,
            timer_authority: Some(timer_authority),
            timer_written: false,
        };
        let mut obsolete_by_generation: BTreeMap<JobGeneration, Vec<OwnedUnit>> = BTreeMap::new();
        for unit in obsolete {
            obsolete_by_generation
                .entry(unit.marker.generation)
                .or_default()
                .push(unit);
        }
        let service = render_service(publication);
        let timer = render_timer(
            publication,
            &format!(
                "{}.g{}",
                self.unit_stem(&publication.job),
                publication.generation.get()
            ),
        );
        let service_path = targets.service_path.clone();
        match self.publish_authorized_target(
            &service_path,
            &mut targets.service_authority,
            service.as_bytes(),
            BackendStage::ServiceFile,
        ) {
            Ok(()) => targets.service_written = true,
            Err(error) => {
                targets.service_written = error.is_partial_publication();
                return Err(self.fail_and_rollback(publication, &mut targets, error));
            }
        }
        let timer_path = targets.timer_path.clone();
        let timer_authority = targets
            .timer_authority
            .as_mut()
            .expect("timer authority was retained before publication");
        match self.publish_authorized_target(
            &timer_path,
            timer_authority,
            timer.as_bytes(),
            BackendStage::TimerFile,
        ) {
            Ok(()) => targets.timer_written = true,
            Err(error) => {
                targets.timer_written = error.is_partial_publication();
                return Err(self.fail_and_rollback(publication, &mut targets, error));
            }
        }
        if let Err(error) = self.run_systemctl(
            BackendStage::Reload,
            [OsString::from("daemon-reload")],
            true,
        ) {
            return Err(self.fail_and_rollback(publication, &mut targets, error));
        }
        let timer_name = self.timer_name(&publication.job, publication.generation);
        if let Err(error) = self.run_systemctl(
            BackendStage::Enable,
            [
                OsString::from("enable"),
                OsString::from("--now"),
                OsString::from(timer_name.clone()),
            ],
            true,
        ) {
            return Err(self.fail_and_rollback(publication, &mut targets, error));
        }
        if let Err(error) = self.run_systemctl(
            BackendStage::Start,
            [OsString::from("restart"), OsString::from(timer_name)],
            true,
        ) {
            return Err(self.fail_and_rollback(publication, &mut targets, error));
        }
        for (generation, mut owned) in obsolete_by_generation {
            let removal = ActivationRemoval {
                application: self.application.clone(),
                job: publication.job.clone(),
                generation,
            };
            if let Err(mut error) = self.cleanup_owned_generation(&removal, &mut owned) {
                error.partial_publication = true;
                error.cleanup_complete = false;
                return Err(error);
            }
        }
        Ok(())
    }

    fn remove(&mut self, removal: &ActivationRemoval) -> Result<(), BackendError> {
        let mut owned = self.owned_removal_paths(removal)?;
        self.cleanup_owned_generation(removal, &mut owned)
    }

    fn installed(&mut self) -> Result<Vec<InstalledActivation>, BackendError> {
        let entries = match std::fs::read_dir(&self.unit_directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(backend_io_error(BackendStage::List, false, true, error));
            }
        };
        let prefix = format!(
            "enzui-activation-a{}-{}-j",
            self.application.as_str().len(),
            self.application.as_str(),
        );
        let mut discovered: BTreeMap<(JobId, JobGeneration), bool> = BTreeMap::new();
        for entry in entries {
            let entry =
                entry.map_err(|error| backend_io_error(BackendStage::List, false, true, error))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if let Some((job, generation)) =
                service_identity(name, &prefix, &entry.path(), &self.application)?
            {
                let expected_unit = format!(
                    "Unit={}.g{}.service",
                    self.unit_stem(&job),
                    generation.get()
                );
                let timer_path = self.timer_path(&job, generation);
                let complete = regular_file_without_symlink(&timer_path)
                    && read_managed_unit(&timer_path)
                        .is_ok_and(|timer| timer.lines().any(|line| line == expected_unit));
                discovered.insert((job, generation), complete);
                continue;
            }
            if let Some((job, generation)) =
                timer_identity(name, &prefix, &entry.path(), &self.application)?
            {
                let service_path = self.service_path(&job, generation);
                let complete = regular_file_without_symlink(&service_path)
                    && read_managed_unit(&service_path)
                        .ok()
                        .and_then(|service| parse_unit_marker(&service))
                        .is_some_and(|marker| {
                            marker.application == self.application
                                && marker.job == job
                                && marker.generation == generation
                        });
                discovered
                    .entry((job, generation))
                    .and_modify(|present| *present |= complete)
                    .or_insert(complete);
            }
        }
        Ok(discovered
            .into_iter()
            .map(|((job, generation), complete)| {
                InstalledActivation::new(job, generation, complete)
            })
            .collect())
    }

    fn boot_id(&mut self) -> Result<String, BackendError> {
        let id = std::fs::read_to_string(&self.boot_id_path)
            .map_err(|error| backend_io_error(BackendStage::BootIdentity, false, true, error))?;
        let id = id.trim();
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
        {
            return Err(BackendError::new(
                BackendErrorKind::Failed,
                BackendStage::BootIdentity,
                false,
                true,
                "boot id has invalid syntax",
            ));
        }
        Ok(id.to_owned())
    }
}

/// Builds a scheduler for the current user's XDG state and systemd directories.
///
/// Absolute `XDG_STATE_HOME` and `XDG_CONFIG_HOME` take precedence over the
/// respective `$HOME/.local/state` and `$HOME/.config` fallbacks. Captures
/// `XDG_RUNTIME_DIR` and `DBUS_SESSION_BUS_ADDRESS` for helper environments.
/// Performs no filesystem or manager probe; scheduling retains custody before
/// it reports backend unavailability.
///
/// # Errors
///
/// Returns [`SystemdSetupError`] when either required XDG root cannot be resolved.
pub fn from_env(
    application: DesktopApplicationId,
) -> Result<DurableActivationScheduler<SystemdUserBackend>, SystemdSetupError> {
    let home = absolute_env("HOME");
    let state_root = absolute_env("XDG_STATE_HOME")
        .or_else(|| home.as_ref().map(|home| home.join(".local/state")))
        .ok_or(SystemdSetupError::NoStateHome)?;
    let config_root = absolute_env("XDG_CONFIG_HOME")
        .or_else(|| home.as_ref().map(|home| home.join(".config")))
        .ok_or(SystemdSetupError::NoConfigHome)?;
    let state_path = state_root.join(application.as_str()).join(STATE_FILE);
    let unit_directory = config_root.join("systemd/user");
    let systemctl = [
        PathBuf::from("/usr/bin/systemctl"),
        PathBuf::from("/bin/systemctl"),
    ]
    .into_iter()
    .find(|path| std::fs::metadata(path).is_ok_and(|metadata| super::is_executable_file(&metadata)))
    .unwrap_or_else(|| PathBuf::from("/usr/bin/systemctl"));
    let environment = ["XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"]
        .into_iter()
        .filter_map(|key| std::env::var_os(key).map(|value| (OsString::from(key), value)))
        .collect();
    let backend = SystemdUserBackend {
        application: application.clone(),
        unit_directory,
        systemctl,
        environment,
        boot_id_path: PathBuf::from("/proc/sys/kernel/random/boot_id"),
        removal_recheck_hook: None,
        publication_hook: None,
        directory_sync_hook: None,
    };
    Ok(DurableActivationScheduler::new(
        application,
        state_path,
        backend,
    ))
}

fn absolute_env(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn render_service(publication: &ActivationPublication) -> String {
    let mut command = String::from(":");
    command.push_str(&escape_systemd_argument(publication.executable.as_os_str()));
    for argument in &publication.arguments {
        command.push(' ');
        command.push_str(&escape_systemd_argument(OsStr::new(argument)));
    }
    format!(
        "{}\n[Unit]\nDescription=enzUI durable activation {} / {}\n\n[Service]\nType=oneshot\nExecStart={}\n",
        render_unit_marker(publication),
        publication.application.as_str(),
        publication.job.as_str(),
        command,
    )
}

fn render_timer(publication: &ActivationPublication, unit_stem: &str) -> String {
    format!(
        "{}\n[Unit]\nDescription=enzUI durable activation timer {} / {}\n\n[Timer]\nOnCalendar=@{}\nAccuracySec=1s\nPersistent={}\nUnit={}.service\n\n[Install]\nWantedBy=timers.target\n",
        render_unit_marker(publication),
        publication.application.as_str(),
        publication.job.as_str(),
        publication.unix_seconds,
        if publication.persistent {
            "true"
        } else {
            "false"
        },
        unit_stem,
    )
}

fn render_unit_marker(publication: &ActivationPublication) -> String {
    format!(
        "# {UNIT_MARKER_FORMAT} format=1 app={} job={} generation={}",
        publication.application.as_str(),
        publication.job.as_str(),
        publication.generation.get(),
    )
}

fn parse_unit_marker(contents: &str) -> Option<UnitMarker> {
    let fields: Vec<_> = contents.lines().next()?.split_ascii_whitespace().collect();
    if fields.len() != 6
        || fields[0] != "#"
        || fields[1] != UNIT_MARKER_FORMAT
        || fields[2] != "format=1"
    {
        return None;
    }
    let application = DesktopApplicationId::new(fields[3].strip_prefix("app=")?).ok()?;
    let job = JobId::new(fields[4].strip_prefix("job=")?).ok()?;
    let generation = fields[5]
        .strip_prefix("generation=")?
        .parse::<u64>()
        .ok()
        .and_then(JobGeneration::new)?;
    Some(UnitMarker {
        application,
        job,
        generation,
    })
}

fn marker_matches(publication: &ActivationPublication, marker: &UnitMarker) -> bool {
    marker.application == publication.application
        && marker.job == publication.job
        && marker.generation == publication.generation
}

fn target_was_changed(
    path: &Path,
    previous: &ExistingTarget,
    publication: &ActivationPublication,
) -> bool {
    let Ok(current) = read_managed_unit(path) else {
        return false;
    };
    let differs = match previous {
        ExistingTarget::Absent => true,
        ExistingTarget::Managed(previous) => previous.as_slice() != current.as_bytes(),
    };
    differs
        && parse_unit_marker(&current)
            .as_ref()
            .is_some_and(|marker| marker_matches(publication, marker))
}

fn rollback_target(
    parent: &File,
    path: &Path,
    authority: &mut PublicationAuthority,
    written: bool,
    publication: &ActivationPublication,
    recheck_hook: Option<fn(&Path)>,
) -> Result<(), BackendError> {
    if !written {
        return Ok(());
    }
    let parent_path = path.parent().ok_or_else(|| {
        collision_error(BackendStage::Remove, "generated unit path has no parent")
    })?;
    enzui_security::tempstage::verify_path_names_file(parent, parent_path)
        .map_err(|error| collision_error(BackendStage::Remove, error.to_string()))?;
    match &authority.previous {
        ExistingTarget::Managed(previous) => {
            if let Some(hook) = recheck_hook {
                hook(path);
            }
            authority
                .state
                .publish(previous)
                .map(|_| ())
                .map_err(|error| publication_error(BackendStage::Remove, error))
        }
        ExistingTarget::Absent => {
            restore_target(path, &authority.previous, publication, recheck_hook)
        }
    }
}

fn restore_target(
    path: &Path,
    previous: &ExistingTarget,
    publication: &ActivationPublication,
    recheck_hook: Option<fn(&Path)>,
) -> Result<(), BackendError> {
    let current = match open_owned_managed_unit(path) {
        Ok(current) => Some(current),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(collision_error(BackendStage::Remove, error.to_string()));
        }
    };
    match (previous, current) {
        (ExistingTarget::Absent, None) => Ok(()),
        (ExistingTarget::Managed(previous), Some((_, current)))
            if previous.as_slice() == current.as_bytes() =>
        {
            Ok(())
        }
        (_, Some((mut file, current)))
            if parse_unit_marker(&current)
                .as_ref()
                .is_some_and(|marker| marker_matches(publication, marker)) =>
        {
            if let Some(hook) = recheck_hook {
                hook(path);
            }
            match previous {
                ExistingTarget::Absent => unlink_held_unit(path, &file).map_err(|error| {
                    if enzui_security::tempstage::verify_path_names_file(&file, path).is_err() {
                        collision_error(BackendStage::Remove, error.to_string())
                    } else {
                        backend_io_error(BackendStage::Remove, true, false, error)
                    }
                })?,
                ExistingTarget::Managed(previous) => {
                    enzui_security::tempstage::verify_path_names_file(&file, path).map_err(
                        |error| collision_error(BackendStage::Remove, error.to_string()),
                    )?;
                    file.set_len(0)
                        .and_then(|()| {
                            file.seek(SeekFrom::Start(0))?;
                            file.write_all(previous)?;
                            file.sync_all()
                        })
                        .map_err(|error| {
                            backend_io_error(BackendStage::Remove, true, false, error)
                        })?;
                    enzui_security::tempstage::verify_path_names_file(&file, path).map_err(
                        |error| collision_error(BackendStage::Remove, error.to_string()),
                    )?;
                }
            }
            if let Some(parent) = path.parent() {
                sync_directory(parent)
                    .map_err(|error| backend_io_error(BackendStage::Remove, true, false, error))?;
            }
            Ok(())
        }
        _ => Err(collision_error(
            BackendStage::Remove,
            "unit ownership changed before rollback",
        )),
    }
}

fn collision_error(stage: BackendStage, detail: impl Into<String>) -> BackendError {
    BackendError::new(BackendErrorKind::Collision, stage, false, false, detail)
}

fn manager_state_error(detail: impl Into<String>) -> BackendError {
    BackendError::new(
        BackendErrorKind::Failed,
        BackendStage::Remove,
        false,
        false,
        detail,
    )
}

fn cleanup_error(mut error: BackendError, partial: bool) -> BackendError {
    error.partial_publication |= partial;
    error.cleanup_complete = false;
    error
}

fn escape_systemd_argument(argument: &OsStr) -> String {
    let value = argument.to_string_lossy();
    if value.is_empty() {
        return "\"\"".into();
    }
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for byte in value.bytes() {
        match byte {
            b'%' => escaped.push_str("%%"),
            b'\\' => escaped.push_str("\\\\"),
            b'"' => escaped.push_str("\\\""),
            b'\n' => escaped.push_str("\\n"),
            b'\r' => escaped.push_str("\\r"),
            b'\t' => escaped.push_str("\\t"),
            0x20..=0x7e => escaped.push(char::from(byte)),
            _ => {
                use std::fmt::Write as _;
                let _ = write!(escaped, "\\x{byte:02x}");
            }
        }
    }
    escaped.push('"');
    escaped
}

fn classify_diagnostic(detail: &str) -> BackendErrorKind {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("permission denied")
        || lower.contains("access denied")
        || lower.contains("authentication is required")
    {
        BackendErrorKind::PermissionDenied
    } else if lower.contains("failed to connect to bus")
        || lower.contains("no medium found")
        || lower.contains("not been booted with systemd")
    {
        BackendErrorKind::Unavailable
    } else {
        BackendErrorKind::Failed
    }
}

fn publication_error(stage: BackendStage, error: PublishError) -> BackendError {
    let kind = if error.io_error().kind() == io::ErrorKind::AlreadyExists
        || matches!(
            error.operation(),
            crate::user_file::SaveOperation::OpenParent
                | crate::user_file::SaveOperation::InspectDestination
                | crate::user_file::SaveOperation::Revalidate
        ) {
        BackendErrorKind::Collision
    } else {
        match error.io_error().kind() {
            io::ErrorKind::PermissionDenied => BackendErrorKind::PermissionDenied,
            io::ErrorKind::TimedOut => BackendErrorKind::TimedOut,
            _ => BackendErrorKind::Failed,
        }
    };
    BackendError::new(
        kind,
        stage,
        error.was_published() || error.custody() != PublishCustody::Unchanged,
        false,
        error.to_string(),
    )
}

fn backend_io_error(
    stage: BackendStage,
    partial: bool,
    cleanup_complete: bool,
    error: io::Error,
) -> BackendError {
    let kind = match error.kind() {
        io::ErrorKind::NotFound => BackendErrorKind::Unavailable,
        io::ErrorKind::PermissionDenied => BackendErrorKind::PermissionDenied,
        io::ErrorKind::TimedOut => BackendErrorKind::TimedOut,
        _ => BackendErrorKind::Failed,
    };
    BackendError::new(kind, stage, partial, cleanup_complete, error.to_string())
}

fn parse_job_component(value: &str) -> Option<JobId> {
    let (length, job) = value.split_once('-')?;
    let length = length.parse::<usize>().ok()?;
    (job.len() == length)
        .then(|| JobId::new(job).ok())
        .flatten()
}

fn service_identity(
    name: &str,
    prefix: &str,
    path: &Path,
    application: &DesktopApplicationId,
) -> Result<Option<(JobId, JobGeneration)>, BackendError> {
    let Some((job_text, name_generation)) = name
        .strip_prefix(prefix)
        .and_then(|name| name.strip_suffix(".service"))
        .and_then(|name| name.rsplit_once(".g"))
    else {
        return Ok(None);
    };
    let (Some(job), Some(name_generation)) = (
        parse_job_component(job_text),
        name_generation
            .parse::<u64>()
            .ok()
            .and_then(JobGeneration::new),
    ) else {
        return Ok(None);
    };
    let service = read_managed_unit(path)
        .map_err(|error| collision_error(BackendStage::List, error.to_string()))?;
    let marker = parse_unit_marker(&service)
        .ok_or_else(|| collision_error(BackendStage::List, "unit ownership marker is invalid"))?;
    if marker.application != *application
        || marker.job != job
        || marker.generation != name_generation
    {
        return Err(collision_error(
            BackendStage::List,
            "service filename and ownership marker disagree",
        ));
    }
    Ok(Some((job, name_generation)))
}

fn timer_identity(
    name: &str,
    prefix: &str,
    path: &Path,
    application: &DesktopApplicationId,
) -> Result<Option<(JobId, JobGeneration)>, BackendError> {
    let Some((job_text, name_generation)) = name
        .strip_prefix(prefix)
        .and_then(|name| name.strip_suffix(".timer"))
        .and_then(|name| name.rsplit_once(".g"))
    else {
        return Ok(None);
    };
    let (Some(job), Some(name_generation)) = (
        parse_job_component(job_text),
        name_generation
            .parse::<u64>()
            .ok()
            .and_then(JobGeneration::new),
    ) else {
        return Ok(None);
    };
    let timer = read_managed_unit(path)
        .map_err(|error| collision_error(BackendStage::List, error.to_string()))?;
    let marker = parse_unit_marker(&timer)
        .ok_or_else(|| collision_error(BackendStage::List, "unit ownership marker is invalid"))?;
    if marker.application != *application
        || marker.job != job
        || marker.generation != name_generation
    {
        return Err(collision_error(
            BackendStage::List,
            "timer filename and ownership marker disagree",
        ));
    }
    let service_name = format!("Unit={prefix}{job_text}.g{}.service", name_generation.get());
    if !timer.lines().any(|line| line == service_name) {
        return Err(collision_error(
            BackendStage::List,
            "timer generation marker and service target disagree",
        ));
    }
    Ok(Some((job, marker.generation)))
}

fn read_managed_unit(path: &Path) -> io::Result<String> {
    let mut file = crate::atomic_state::open_state_file_nofollow(path)?;
    read_held_managed_unit(&mut file)
}

fn open_owned_managed_unit(path: &Path) -> io::Result<(File, String)> {
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(enzui_security::platform::libc_o_nofollow())
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "generated unit path is not a regular file",
        ));
    }
    let contents = read_held_managed_unit(&mut file)?;
    Ok((file, contents))
}

fn read_held_managed_unit(file: &mut File) -> io::Result<String> {
    const MAX_UNIT_BYTES: u64 = 64 * 1024;
    let metadata = file.metadata()?;
    if metadata.len() > MAX_UNIT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "generated unit exceeds the read bound",
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut contents = String::new();
    (&mut *file)
        .take(MAX_UNIT_BYTES.saturating_add(1))
        .read_to_string(&mut contents)?;
    if contents.len() as u64 > MAX_UNIT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "generated unit grew beyond the read bound",
        ));
    }
    Ok(contents)
}

fn unlink_held_unit(path: &Path, file: &File) -> io::Result<()> {
    enzui_security::tempstage::remove_if_path_names_file(path, file)
}

fn regular_file_without_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

fn sync_directory(path: &Path) -> io::Result<()> {
    match std::fs::File::open(path) {
        Ok(directory) => directory.sync_all(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
