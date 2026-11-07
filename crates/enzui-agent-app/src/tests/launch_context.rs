#[path = "../../../enzui-core/tests/support/activation_child.rs"]
mod activation_child;
#[path = "../../../enzui-core/tests/support/private_bus.rs"]
mod private_bus;

use super::super::*;
use activation_child::{ReapedChild, child_activation_environment};
use enzui_test_support::test_scratch::ScratchDir;
use private_bus::PrivateBus;
use std::process::{Command, Stdio};
use std::time::Duration;

const APP: &str = "org.enzui.ClaimContextFixture";
const ROLE: &str = "ENZUI_CLAIM_CONTEXT_FIXTURE";
const ACK: &str = "ENZUI_CLAIM_CONTEXT_ACK";

fn profile(installed: bool) -> ScratchDir {
    let root = ScratchDir::new_restricted("claim");
    for name in ["home", "config", "state", "data", "system", "runtime"] {
        std::fs::create_dir(root.path().join(name)).unwrap();
    }
    if installed {
        let directory = root.path().join("data/dbus-1/services");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join(format!("{APP}.service")),
            format!(
                "[D-BUS Service]\nName={APP}\nExec={} --agent-background\n",
                std::env::current_exe().unwrap().display(),
            ),
        )
        .unwrap();
    }
    root
}

fn fixture(
    bus: &PrivateBus,
    root: &ScratchDir,
    provisioned: bool,
    expectation: &str,
    wayland: Option<&str>,
    x11: Option<&str>,
) {
    let acknowledged = root.path().join("claim-acknowledged");
    if acknowledged.exists() {
        std::fs::remove_file(&acknowledged).unwrap();
    }
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "tests::launch_context::desktop_claim_fixture",
            "--nocapture",
        ])
        .env(ROLE, expectation)
        .env(ACK, &acknowledged)
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("HOME", root.path().join("home"))
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_DATA_DIRS", root.path().join("system"))
        .env("XDG_RUNTIME_DIR", root.path().join("runtime"))
        .env_remove("ENZUI_APP_CREDENTIAL")
        .env_remove("ENZUI_CENTER_CREDENTIAL")
        .env_remove("ENZUI_CENTER_SOCKET")
        .env_remove("XDG_ACTIVATION_TOKEN")
        .env_remove("DESKTOP_STARTUP_ID")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::null());
    if provisioned {
        let credential = root.path().join("credential.json");
        std::fs::write(&credential, "{}").unwrap();
        command
            .env("ENZUI_APP_CREDENTIAL", credential)
            .env("ENZUI_CENTER_SOCKET", root.path().join("unused.sock"));
    }
    for (name, value) in [
        ("XDG_ACTIVATION_TOKEN", wayland),
        ("DESKTOP_STARTUP_ID", x11),
    ] {
        if let Some(value) = value {
            command.env(name, value);
        }
    }
    let mut child = ReapedChild(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            assert!(status.success(), "desktop claim fixture failed");
            assert_eq!(std::fs::read_to_string(&acknowledged).unwrap(), expectation);
            return;
        }
        assert!(
            Instant::now() < deadline,
            "desktop claim fixture did not exit"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn releasing_application_allows_reopen_while_retirement_completes() {
    let bus = PrivateBus::spawn();
    let service = ApplicationService::connect_to_address(APP, &bus.address).unwrap();
    let tasks = Arc::new(TaskHost::<u32>::spawn().unwrap());
    let (complete, wait) = iced_futures::futures::channel::oneshot::channel();
    tasks
        .submit(Task::perform(async move { wait.await.unwrap() }, |value| {
            value
        }))
        .unwrap();
    let mut process = AgentProcess::windowless(Some(service), Arc::clone(&tasks));
    process.request_present(ApplicationIntent::Activate);
    assert!(ApplicationService::connect_to_address(APP, &bus.address).is_err());
    assert!(process.has_task_work());

    process.release_application();
    assert!(!process.wants_present());
    assert!(process.take_application_calls().is_empty());
    assert!(process.has_task_work());
    let reopened = ApplicationService::connect_to_address(APP, &bus.address)
        .expect("the retired process must release its desktop name");
    complete.send(73).unwrap();
    assert_eq!(
        process
            .tasks()
            .unwrap()
            .poll(Duration::from_secs(5))
            .unwrap(),
        Some(73)
    );
    assert!(process.take_application_calls().is_empty());
    drop(reopened);
}

fn forwarded_context(installed: bool, windowless: bool) {
    let bus = PrivateBus::spawn();
    let root = profile(installed);
    let service = ApplicationService::connect_to_address(APP, &bus.address).unwrap();
    let mut process = if windowless {
        AgentProcess::<()>::windowless(Some(service), Arc::new(TaskHost::spawn().unwrap()))
    } else {
        AgentProcess::<()>::desktop(Some(service))
    };
    for (wayland, x11, expected) in [
        (
            Some("same-opaque-value"),
            None,
            "unset\nsame-opaque-value\n",
        ),
        (
            None,
            Some("same-opaque-value"),
            "same-opaque-value\nunset\n",
        ),
        (
            Some("wayland-field"),
            Some("x11-field"),
            "x11-field\nwayland-field\n",
        ),
    ] {
        fixture(&bus, &root, !installed, "forwarded", wayland, x11);
        let mut calls = process.take_application_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(process.wants_present(), windowless);
        let call = calls.pop().unwrap();
        assert_eq!(call.context_error, None);
        assert_eq!(
            child_activation_environment(call.context.expect("startup context was lost")),
            expected
        );
    }
}

#[test]
fn claim_desktop_installed_forwarding_preserves_both_protocols() {
    forwarded_context(true, false);
}

#[test]
fn claim_desktop_provisioned_forwarding_preserves_both_protocols() {
    forwarded_context(false, false);
}

#[test]
fn desktop_claim_fixture() {
    let Ok(expectation) = std::env::var(ROLE) else {
        return;
    };
    let claim = claim_desktop(APP, ApplicationIntent::Activate);
    let observed = match claim {
        Ok(DesktopClaim::Forwarded) => "forwarded",
        Ok(DesktopClaim::Owner(_)) => "owner",
        Ok(DesktopClaim::Independent) => "independent",
        Err(ApplicationServiceError::InvalidRequest) => "invalid",
        Err(error) => panic!("desktop claim failed: {error}"),
    };
    assert_eq!(observed, expectation);
    if expectation == "owner" {
        assert_eq!(
            std::env::var("DESKTOP_STARTUP_ID").unwrap(),
            "retained-startup"
        );
    }
    std::fs::write(std::env::var_os(ACK).unwrap(), expectation).unwrap();
}

#[test]
fn claim_desktop_owner_retains_environment_and_independent_runs_ignore_unused_context() {
    let bus = PrivateBus::spawn();
    let root = profile(false);
    fixture(&bus, &root, true, "owner", None, Some("retained-startup"));
    std::fs::remove_file(root.path().join("credential.json")).unwrap();
    fixture(
        &bus,
        &root,
        false,
        "independent",
        Some(&"x".repeat(4097)),
        None,
    );
}

/// A package whose activation file names a unit the bus cannot start still
/// opens: the launch becomes the instance itself.
#[test]
fn claim_desktop_installed_without_a_startable_unit_runs_the_window() {
    let bus = PrivateBus::spawn();
    let root = profile(true);
    fixture(&bus, &root, false, "owner", None, Some("retained-startup"));
}

#[test]
fn claim_desktop_windowless_reader_retains_context_for_the_future_window() {
    forwarded_context(true, true);
}

/// A desktop launch that made the bus start a windowless worker is queued
/// before the worker's wake subscription runs; an app that drops that first
/// wake while it starts up never takes the call. The worker still presents
/// instead of waiting for the next launch (enzUI-ICED #3686).
///
/// Sabotage: drop `launch_waiting` from `disposition`.
#[test]
fn a_launch_the_app_never_took_still_presents_a_windowless_worker() {
    let bus = PrivateBus::spawn();
    let service = ApplicationService::connect_to_address(APP, &bus.address).unwrap();
    let process =
        AgentProcess::<()>::windowless(Some(service), Arc::new(TaskHost::spawn().unwrap()));
    assert_eq!(
        disposition::<(), ()>(None, &process, false),
        Disposition::Idle
    );
    ApplicationService::activate_installed_at_address(
        APP,
        &bus.address,
        &ApplicationIntent::Activate,
        None,
    )
    .unwrap();
    assert!(process.launch_waiting());
    assert_eq!(
        disposition::<(), ()>(None, &process, false),
        Disposition::Present
    );
}
