#![allow(
    missing_docs,
    unreachable_pub,
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::panic,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    clippy::print_stdout,
    clippy::string_slice,
    clippy::missing_assert_message,
    reason = "a test crate reports by panicking and printing, and none of it is public API"
)]
#![cfg(target_os = "linux")]

use enzui_services::durable_activation::systemd::SystemdUserBackend;
use enzui_services::durable_activation::{
    ActivationFacility, ActivationInvocation, ActivationPublication, ActivationRemoval,
    ActivationRequest, BackendError, BackendErrorKind, ClaimOutcome, DesktopApplicationId,
    DurableActivationScheduler, InstalledActivation, JobId, JobStatus, MissedActivationPolicy,
    ScheduleOutcome, SchedulerError,
};
use enzui_test_support::test_scratch::ScratchDir;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};

const BASE: u64 = 2_000_000_000;

#[derive(Debug, Default)]
struct BlockGate {
    state: Mutex<BlockGateState>,
    changed: Condvar,
}

#[derive(Debug, Default)]
struct BlockGateState {
    blocked: bool,
    entered: usize,
}

impl BlockGate {
    fn block(&self) {
        let mut state = self.state.lock().expect("lock block gate");
        state.blocked = true;
    }

    fn wait_entered(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut state = self.state.lock().expect("lock block gate");
        while state.entered == 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "backend operation did not enter");
            let waited = self
                .changed
                .wait_timeout(state, remaining)
                .expect("wait for backend operation");
            state = waited.0;
            assert!(!waited.1.timed_out() || state.entered != 0);
        }
    }

    fn release(&self) {
        let mut state = self.state.lock().expect("lock block gate");
        state.blocked = false;
        self.changed.notify_all();
    }

    fn enter(
        &self,
        stage: enzui_services::durable_activation::BackendStage,
    ) -> Result<(), BackendError> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut state = self.state.lock().expect("lock block gate");
        state.entered += 1;
        self.changed.notify_all();
        while state.blocked {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(BackendError::new(
                    BackendErrorKind::TimedOut,
                    stage,
                    false,
                    false,
                    "blocking test backend timed out",
                ));
            }
            let waited = self
                .changed
                .wait_timeout(state, remaining)
                .expect("wait for backend release");
            state = waited.0;
            if waited.1.timed_out() && state.blocked {
                return Err(BackendError::new(
                    BackendErrorKind::TimedOut,
                    stage,
                    false,
                    false,
                    "blocking test backend timed out",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
struct FakePlatform {
    installed: Mutex<
        BTreeMap<(JobId, enzui_services::durable_activation::JobGeneration), InstalledActivation>,
    >,
    boot_id: Mutex<String>,
}

impl Default for FakePlatform {
    fn default() -> Self {
        Self {
            installed: Mutex::new(BTreeMap::new()),
            boot_id: Mutex::new("eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee".into()),
        }
    }
}

#[derive(Clone, Debug)]
struct BlockingFacility {
    platform: Arc<FakePlatform>,
    publish_gate: Option<Arc<BlockGate>>,
    remove_gate: Option<Arc<BlockGate>>,
    fault_after_publish: bool,
    fault_after_remove: bool,
}

impl BlockingFacility {
    fn new(platform: Arc<FakePlatform>) -> Self {
        Self {
            platform,
            publish_gate: None,
            remove_gate: None,
            fault_after_publish: false,
            fault_after_remove: false,
        }
    }

    fn with_publish_gate(mut self, gate: Arc<BlockGate>) -> Self {
        self.publish_gate = Some(gate);
        self
    }
}

impl ActivationFacility for BlockingFacility {
    fn publish(&mut self, publication: &ActivationPublication) -> Result<(), BackendError> {
        if let Some(gate) = &self.publish_gate {
            gate.enter(enzui_services::durable_activation::BackendStage::Start)?;
        }
        let mut installed = self.platform.installed.lock().expect("lock installed map");
        installed.retain(|(job, _), _| job != publication.job());
        installed.insert(
            (publication.job().clone(), publication.generation()),
            InstalledActivation::new(publication.job().clone(), publication.generation(), true),
        );
        drop(installed);
        if self.fault_after_publish {
            enzui_security::tempstage::fault::arm_sync(std::io::ErrorKind::Other);
        }
        Ok(())
    }

    fn remove(&mut self, removal: &ActivationRemoval) -> Result<(), BackendError> {
        if let Some(gate) = &self.remove_gate {
            gate.enter(enzui_services::durable_activation::BackendStage::Remove)?;
        }
        self.platform
            .installed
            .lock()
            .expect("lock installed map")
            .remove(&(removal.job().clone(), removal.generation()));
        if self.fault_after_remove {
            enzui_security::tempstage::fault::arm_sync(std::io::ErrorKind::Other);
        }
        Ok(())
    }

    fn installed(&mut self) -> Result<Vec<InstalledActivation>, BackendError> {
        Ok(self
            .platform
            .installed
            .lock()
            .expect("lock installed map")
            .values()
            .cloned()
            .collect())
    }

    fn boot_id(&mut self) -> Result<String, BackendError> {
        Ok(self.platform.boot_id.lock().expect("lock boot id").clone())
    }
}

struct Fixture {
    _scratch: ScratchDir,
    state: PathBuf,
    units: PathBuf,
    systemctl: PathBuf,
    boot_id: PathBuf,
    executable: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let scratch = ScratchDir::new_restricted(label);
        let state = scratch.join("state/jobs.state");
        let units = scratch.join("config/systemd/user");
        let systemctl = scratch.join("fake-systemctl");
        let boot_id = scratch.join("boot-id");
        let executable = scratch.join("application $(not-shell) % \"");
        std::fs::write(
            &systemctl,
            r#"#!/usr/bin/python3
import os
import sys
root = os.environ["FAKE_SYSTEMCTL_ROOT"]
unit_dir = os.environ["FAKE_SYSTEMD_UNIT_DIR"]
arguments = sys.argv[1:]
with open(os.path.join(root, "systemctl-calls"), "a", encoding="utf-8") as log:
    log.write("\t".join(arguments) + "\n")
for name in os.listdir(root):
    if name.startswith("deny-") and name[5:] in arguments:
        print("Permission denied by isolated user manager", file=sys.stderr)
        sys.exit(1)
    if name.startswith("fail-") and name[5:] in arguments:
        print("injected user manager failure", file=sys.stderr)
        sys.exit(1)
unit = arguments[-1]
record = os.path.join(root, "manager-" + unit)
if "fire" in arguments:
    if os.path.exists(record):
        with open(os.path.join(root, "fired-" + unit), "w", encoding="utf-8") as fired:
            fired.write("fired\n")
elif "show" in arguments:
    print("Id=" + unit)
    if os.path.exists(record):
        with open(record, encoding="utf-8") as state:
            fragment = state.read().rstrip("\n")
        print("LoadState=loaded")
        print("FragmentPath=" + fragment)
    else:
        print("LoadState=not-found")
        print("FragmentPath=")
elif "enable" in arguments:
    with open(record, "w", encoding="utf-8") as state:
        state.write(os.path.join(unit_dir, unit) + "\n")
elif "disable" in arguments:
    service = os.path.join(unit_dir, unit[:-6] + ".service")
    with open(os.path.join(root, "stop-shape-" + unit), "w", encoding="utf-8") as shape:
        shape.write(f"service={int(os.path.exists(service))} timer={int(os.path.exists(os.path.join(unit_dir, unit)))}\n")
    if os.path.exists(record):
        os.remove(record)
"#,
        )
        .expect("write fake systemctl");
        std::fs::write(&executable, "#!/bin/sh\nexit 0\n").expect("write app executable");
        make_executable(&systemctl);
        make_executable(&executable);
        std::fs::write(&boot_id, "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa\n").expect("write boot id");
        Self {
            _scratch: scratch,
            state,
            units,
            systemctl,
            boot_id,
            executable,
        }
    }

    fn backend(&self) -> SystemdUserBackend {
        let app = DesktopApplicationId::new("org.enzui.Calendar").expect("fixture app id");
        SystemdUserBackend::new(
            app,
            self.units.clone(),
            self.systemctl.clone(),
            vec![
                (
                    OsString::from("XDG_RUNTIME_DIR"),
                    self._scratch.as_os_str().to_owned(),
                ),
                (
                    OsString::from("FAKE_SYSTEMCTL_ROOT"),
                    self._scratch.as_os_str().to_owned(),
                ),
                (
                    OsString::from("FAKE_SYSTEMD_UNIT_DIR"),
                    self.units.as_os_str().to_owned(),
                ),
            ],
            self.boot_id.clone(),
        )
        .expect("construct isolated backend")
    }

    fn scheduler(&self) -> DurableActivationScheduler<SystemdUserBackend> {
        DurableActivationScheduler::new(
            DesktopApplicationId::new("org.enzui.Calendar").expect("fixture app id"),
            self.state.clone(),
            self.backend(),
        )
    }

    fn request(
        &self,
        job: &str,
        due: u64,
        arguments: Vec<String>,
        missed: MissedActivationPolicy,
    ) -> ActivationRequest {
        ActivationRequest::new(
            JobId::new(job).expect("fixture job id"),
            &self.executable,
            arguments,
            UNIX_EPOCH + Duration::from_secs(due),
            missed,
        )
        .expect("valid fixture request")
    }

    fn invocation(&self, job: &str, generation: u64) -> ActivationInvocation {
        let generation = generation.to_string();
        ActivationInvocation::parse([
            "--enzui-durable-activation",
            "org.enzui.Calendar",
            job,
            generation.as_str(),
        ])
        .expect("valid invocation")
        .expect("durable invocation")
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .expect("make fixture executable");
}

#[test]
fn durable_activation_schedule_publishes_private_units_and_lists_scheduled() {
    let fixture = Fixture::new("dasched");
    let mut scheduler = fixture.scheduler();
    let dangerous = "$(touch should-not-run); 100% literal".to_owned();
    let outcome = scheduler
        .schedule_at(
            fixture.request(
                "morning-reminder",
                BASE + 60,
                vec![dangerous.clone()],
                MissedActivationPolicy::CatchUpWithin(Duration::from_secs(600)),
            ),
            UNIX_EPOCH + Duration::from_secs(BASE),
        )
        .expect("schedule through isolated user-systemd backend");
    assert_eq!(
        outcome,
        ScheduleOutcome::Scheduled(outcome_generation(outcome))
    );
    assert_eq!(outcome_generation(outcome).get(), 1);

    let listed = scheduler.list().expect("list durable custody");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].status(), JobStatus::Scheduled);
    let service = fixture
        .units
        .join("enzui-activation-a18-org.enzui.Calendar-j16-morning-reminder.g1.service");
    let timer = fixture
        .units
        .join("enzui-activation-a18-org.enzui.Calendar-j16-morning-reminder.g1.timer");
    let service_text = std::fs::read_to_string(&service).expect("read service unit");
    let timer_text = std::fs::read_to_string(&timer).expect("read timer unit");
    let marker = "# enzui-durable-activation format=1 app=org.enzui.Calendar job=morning-reminder generation=1";
    assert_eq!(service_text.lines().next(), Some(marker));
    assert_eq!(timer_text.lines().next(), Some(marker));
    assert!(service_text.contains("ExecStart=:"));
    assert!(!service_text.contains("/bin/sh"));
    assert!(!service_text.contains(&dangerous));
    assert!(service_text.contains("$(not-shell)"));
    assert!(service_text.contains("%%"));
    assert!(service_text.contains("\\\""));
    assert!(timer_text.contains("OnCalendar=@2000000060"));
    assert!(timer_text.contains("Persistent=true"));
    assert!(
        timer_text.contains(
            "Unit=enzui-activation-a18-org.enzui.Calendar-j16-morning-reminder.g1.service"
        )
    );
    let calls = std::fs::read_to_string(fixture._scratch.join("systemctl-calls"))
        .expect("read helper calls");
    assert!(calls.contains("--user\tdaemon-reload"));
    assert!(calls.contains(
        "--user\tenable\t--now\tenzui-activation-a18-org.enzui.Calendar-j16-morning-reminder.g1.timer"
    ));
    assert!(calls.contains(
        "--user\trestart\tenzui-activation-a18-org.enzui.Calendar-j16-morning-reminder.g1.timer"
    ));

    #[cfg(unix)]
    for path in [&fixture.state, &service, &timer] {
        assert_eq!(
            std::fs::metadata(path)
                .expect("published file metadata")
                .mode()
                & 0o777,
            0o600,
            "{} must be owner-only",
            path.display()
        );
    }
}

fn outcome_generation(
    outcome: ScheduleOutcome,
) -> enzui_services::durable_activation::JobGeneration {
    match outcome {
        ScheduleOutcome::Scheduled(generation) | ScheduleOutcome::Replaced(generation) => {
            generation
        }
    }
}
#[path = "durable_activation_systemd/publication.rs"]
mod publication;
