use enzui_test_support::test_scratch::ScratchDir;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub(super) struct PrivateBus {
    child: Option<Child>,
    child_pid: u32,
    pub(super) address: String,
    cleanup_complete: Arc<AtomicBool>,
    _root: ScratchDir,
}

#[derive(Debug)]
#[allow(
    dead_code,
    reason = "read only through `Debug`, in the startup panic message"
)]
struct BusStartupFailure {
    child_pid: u32,
    cleanup_complete: Arc<AtomicBool>,
    error: BusStartupError,
}

enum BusStartupError {
    SocketTimeout,
    ChildExited(std::process::ExitStatus),
}

impl std::fmt::Debug for BusStartupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SocketTimeout => formatter.write_str("SocketTimeout"),
            Self::ChildExited(status) => {
                formatter.debug_tuple("ChildExited").field(status).finish()
            }
        }
    }
}

impl PrivateBus {
    pub(super) fn spawn() -> Self {
        Self::with_service(None)
    }

    pub(super) fn with_service(service: Option<&str>) -> Self {
        Self::try_spawn_with(Duration::from_secs(3), service)
            .unwrap_or_else(|failure| panic!("private dbus-daemon startup: {failure:?}"))
    }

    #[allow(
        clippy::unwrap_in_result,
        reason = "fixture setup failures abort the test; readiness failures carry cleanup diagnostics"
    )]
    fn try_spawn_with(wait: Duration, service: Option<&str>) -> Result<Self, BusStartupFailure> {
        let root = ScratchDir::new_restricted("act");
        let home = root.path().join("home");
        let runtime = root.path().join("runtime");
        let config_home = root.path().join("config");
        let data_home = root.path().join("data");
        let cache_home = root.path().join("cache");
        let state_home = root.path().join("state");
        for directory in [
            &home,
            &runtime,
            &config_home,
            &data_home,
            &cache_home,
            &state_home,
        ] {
            std::fs::create_dir(directory).expect("private environment directory");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700))
                .expect("private runtime permissions");
        }

        let socket = runtime.join("bus");
        let address = format!("unix:path={}", socket.display());
        let config = root.path().join("bus.conf");
        let servicedir = match service {
            Some(service) => {
                let directory = root.path().join("services");
                std::fs::create_dir(&directory).expect("private service directory");
                std::fs::write(
                    directory.join(format!(
                        "{}.service",
                        service
                            .lines()
                            .find_map(|line| line.strip_prefix("Name="))
                            .expect("fixture service name")
                    )),
                    service,
                )
                .expect("private service file");
                format!("  <servicedir>{}</servicedir>\n", directory.display())
            }
            None => String::new(),
        };
        std::fs::write(
            &config,
            format!(
                "<busconfig>\n  <type>session</type>\n  <listen>{address}</listen>\n\
                 {servicedir}  \
                 <policy context=\"default\">\n    <allow send_destination=\"*\"/>\n    \
                 <allow receive_sender=\"*\"/>\n    <allow own=\"*\"/>\n  \
                 </policy>\n</busconfig>\n"
            ),
        )
        .expect("private bus config");
        let child = Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .arg("--nofork")
            .arg("--nopidfile")
            .env("HOME", &home)
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_DATA_HOME", &data_home)
            .env("XDG_CACHE_HOME", &cache_home)
            .env("XDG_STATE_HOME", &state_home)
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("private dbus-daemon");
        let child_pid = child.id();
        let cleanup_complete = Arc::new(AtomicBool::new(false));
        let mut bus = Self {
            child: Some(child),
            child_pid,
            address,
            cleanup_complete,
            _root: root,
        };
        bus.wait_for_socket(&socket, wait)?;
        Ok(bus)
    }

    fn wait_for_socket(
        &mut self,
        socket: &std::path::Path,
        wait: Duration,
    ) -> Result<(), BusStartupFailure> {
        let deadline = Instant::now() + wait;
        loop {
            if Instant::now() >= deadline {
                return Err(self.startup_failure(BusStartupError::SocketTimeout));
            }
            if socket.exists() {
                return Ok(());
            }
            if let Some(child) = self.child.as_mut()
                && let Ok(Some(status)) = child.try_wait()
            {
                return Err(self.startup_failure(BusStartupError::ChildExited(status)));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn startup_failure(&self, error: BusStartupError) -> BusStartupFailure {
        BusStartupFailure {
            child_pid: self.child_pid,
            cleanup_complete: Arc::clone(&self.cleanup_complete),
            error,
        }
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = child.kill();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => {
                    self.cleanup_complete.store(true, Ordering::Release);
                    return;
                }
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                result => panic!("private dbus-daemon cleanup failed: {result:?}"),
            }
        }
    }
}
