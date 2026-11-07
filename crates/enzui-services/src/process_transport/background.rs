//! Best-effort background scheduling for helper processes and in-process work.
//!
//! [`ProcessRequest::background`] requests a transient systemd user scope in
//! `background.slice`, nice level 12 and idle I/O. Slice weights depend on the
//! host's systemd configuration. [`placement`] reports a cached manager probe,
//! not confirmation that an individual helper entered its requested scope.
//! If the manager is unavailable or scopes are disabled, installed `nice` and
//! `ionice` tools provide a fallback; missing tools leave that policy unapplied.
//!
//! Scoped launches disable systemd argument expansion. With `/usr/bin/env` or
//! `/bin/env` available, a cleared helper environment strips manager-injected
//! `XDG_RUNTIME_DIR` and `INVOCATION_ID` unless supplied by the caller. Inherited
//! environments strip `INVOCATION_ID` even when explicitly supplied. Without
//! that tool, these variables can reach the helper. Scope cleanup requests a
//! best-effort kill of descendants, including those outside the process group.
//!
//! [`BackgroundScope::worker`] reads these environment overrides when built:
//!
//! | variable | values | default |
//! |---|---|---|
//! | `ENZUI_BACKGROUND_WORKERS` | exact `off` disables scopes, retaining fallback scheduling | on |
//! | `ENZUI_BACKGROUND_SLICE` | validated unit name ending in `.slice` | `background.slice` |
//! | `ENZUI_BACKGROUND_NICE` | trimmed `0`..=`19` | `12` |
//! | `ENZUI_BACKGROUND_IO` | exact `inherit` omits idle I/O; other values request it | `idle` |
//!
//! Invalid slice and nice values use their defaults. Scoped launches request
//! an absolute nice level; fallback `nice -n` adds the configured adjustment
//! to the inherited level. The policy does not guarantee lower priority than
//! a caller that already runs at a higher nice value.

use super::*;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;

// Scheduling wrappers run before the helper, so ambient PATH must not select them.
const TOOL_DIRECTORIES: [&str; 2] = ["/usr/bin", "/bin"];

const DEFAULT_SLICE: &str = "background.slice";
const DEFAULT_NICE: u8 = 12;
const MAX_NICE: u8 = 19;
const MAX_UNIT_PREFIX: usize = 48;
const PROBE_DEADLINE: Duration = Duration::from_secs(5);
const SCOPE_KILL_DEADLINE: Duration = Duration::from_secs(2);

/// Scheduling policy for helpers, built with [`BackgroundScope::worker`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundScope {
    unit_prefix: String,
    slice: String,
    nice: u8,
    idle_io: bool,
    enabled: bool,
}

impl BackgroundScope {
    /// The policy for one kind of worker, with the administrator's overrides
    /// applied.
    ///
    /// `unit_prefix` names the worker's transient units (`files-copy` gives
    /// `files-copy-<pid>-<n>.scope`). Characters other than ASCII letters,
    /// digits and `-` are replaced by `-`, and it is cut to 48 bytes. An empty
    /// prefix becomes `enzui-worker`. Overrides are read once for this value.
    #[must_use]
    pub fn worker(unit_prefix: &str) -> Self {
        Self::with_overrides(unit_prefix, |name| std::env::var(name).ok())
    }

    pub(super) fn with_overrides(unit_prefix: &str, read: impl Fn(&str) -> Option<String>) -> Self {
        let mut prefix: String = unit_prefix
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '-'
                }
            })
            .take(MAX_UNIT_PREFIX)
            .collect();
        if prefix.is_empty() {
            prefix.push_str("enzui-worker");
        }
        let slice = read("ENZUI_BACKGROUND_SLICE")
            .filter(|slice| valid_slice(slice))
            .unwrap_or_else(|| DEFAULT_SLICE.to_owned());
        let nice = read("ENZUI_BACKGROUND_NICE")
            .and_then(|nice| nice.trim().parse::<u8>().ok())
            .filter(|nice| *nice <= MAX_NICE)
            .unwrap_or(DEFAULT_NICE);
        let idle_io = !matches!(read("ENZUI_BACKGROUND_IO").as_deref(), Some("inherit"));
        let enabled = !matches!(read("ENZUI_BACKGROUND_WORKERS").as_deref(), Some("off"));
        Self {
            unit_prefix: prefix,
            slice,
            nice,
            idle_io,
            enabled,
        }
    }

    /// Requested slice for scoped helpers; ignored by fallback scheduling.
    #[must_use]
    pub fn slice(&self) -> &str {
        &self.slice
    }

    /// Requested absolute nice level for scopes, or additive fallback adjustment.
    #[must_use]
    pub const fn nice(&self) -> u8 {
        self.nice
    }

    /// Whether helper launches request idle I/O when a scheduling wrapper and `ionice` exist.
    #[must_use]
    pub const fn idle_io(&self) -> bool {
        self.idle_io
    }
}

fn valid_slice(slice: &str) -> bool {
    slice.len() <= 128
        && slice.ends_with(".slice")
        && slice.len() > ".slice".len()
        && slice
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

/// Proposed helper placement based on the cached user-manager probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    /// The manager probe succeeded; launches will request the policy's slice.
    Scope {
        /// Requested slice, which the probe does not validate.
        slice: String,
    },
    /// An ordinary child with scheduling applied through available fallback tools.
    Fallback(FallbackReason),
}

/// Why a background helper runs as an ordinary child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    /// `ENZUI_BACKGROUND_WORKERS=off`.
    Disabled,
    /// No executable `systemd-run` was found in the fixed tool directories.
    NoSystemdRun,
    /// `XDG_RUNTIME_DIR` is missing or is not an absolute path.
    NoRuntimeDir,
    /// The user manager did not start a probe scope.
    UserManagerUnavailable,
}

impl FallbackReason {
    /// One bounded sentence for logs and diagnostics.
    #[must_use]
    pub const fn summary(self) -> &'static str {
        match self {
            Self::Disabled => "background scopes are disabled by ENZUI_BACKGROUND_WORKERS",
            Self::NoSystemdRun => "systemd-run is not installed",
            Self::NoRuntimeDir => "XDG_RUNTIME_DIR is not set",
            Self::UserManagerUnavailable => "the systemd user manager did not start a scope",
        }
    }
}

/// Report the placement proposed for helpers built with `scope`.
///
/// The first enabled call synchronously probes the user manager; success and
/// failure are cached for the process lifetime. Polling stops after five
/// seconds, but killing and reaping the probe can take longer. Disabled scopes
/// skip the probe. Success does not validate a custom slice or later launches.
#[must_use]
pub fn placement(scope: &BackgroundScope) -> Placement {
    if !scope.enabled {
        return Placement::Fallback(FallbackReason::Disabled);
    }
    match manager() {
        Ok(_) => Placement::Scope {
            slice: scope.slice.clone(),
        },
        Err(reason) => Placement::Fallback(*reason),
    }
}

#[derive(Debug)]
struct Manager {
    systemd_run: PathBuf,
    systemctl: Option<PathBuf>,
    runtime_dir: OsString,
}

fn manager() -> &'static Result<Manager, FallbackReason> {
    static MANAGER: OnceLock<Result<Manager, FallbackReason>> = OnceLock::new();
    MANAGER.get_or_init(probe_manager)
}

fn tool(name: &str) -> Option<PathBuf> {
    TOOL_DIRECTORIES
        .iter()
        .map(|directory| Path::new(directory).join(name))
        .find(|path| {
            path.metadata()
                .is_ok_and(|meta| executable_candidate(&meta))
        })
}

fn probe_manager() -> Result<Manager, FallbackReason> {
    let systemd_run = tool("systemd-run").ok_or(FallbackReason::NoSystemdRun)?;
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|dir| Path::new(dir).is_absolute())
        .ok_or(FallbackReason::NoRuntimeDir)?;
    let true_tool = tool("true").ok_or(FallbackReason::UserManagerUnavailable)?;
    let mut probe = Command::new(&systemd_run);
    probe
        .args(scope_arguments("enzui-scope-probe", DEFAULT_SLICE, 0))
        .arg("--")
        .arg(true_tool)
        .env_clear()
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = probe
        .spawn()
        .map_err(|_| FallbackReason::UserManagerUnavailable)?;
    if !wait_bounded(&mut child, PROBE_DEADLINE).is_some_and(|status| status.success()) {
        return Err(FallbackReason::UserManagerUnavailable);
    }
    Ok(Manager {
        systemd_run,
        systemctl: tool("systemctl"),
        runtime_dir,
    })
}

// The polling budget excludes the blocking reap after termination.
fn wait_bounded(child: &mut Child, deadline: Duration) -> Option<ExitStatus> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if started.elapsed() < deadline => thread::sleep(POLL_INTERVAL),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn scope_arguments(prefix: &str, slice: &str, nice: u8) -> Vec<OsString> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let unit = format!(
        "{prefix}-{}-{}.scope",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    [
        "--user".to_owned(),
        "--scope".to_owned(),
        "--quiet".to_owned(),
        "--collect".to_owned(),
        "--expand-environment=no".to_owned(),
        format!("--unit={unit}"),
        format!("--slice={slice}"),
        format!("--nice={nice}"),
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

pub(super) struct Launch {
    pub(super) command: Command,
    pub(super) scope: Option<ScopeUnit>,
    // Applied after environment clearing so systemd-run can reach the cached manager.
    pub(super) manager_env: Option<(OsString, OsString)>,
}

pub(super) fn launch(
    scope: &BackgroundScope,
    executable: &Path,
    argv: &[OsString],
    env_policy: &EnvPolicy,
    env: &[(OsString, OsString)],
) -> Launch {
    let ionice = scope.idle_io.then(|| tool("ionice")).flatten();
    let nice_tool = tool("nice");
    let env_tool = tool("env");
    let placed = scope
        .enabled
        .then(manager)
        .and_then(|manager| manager.as_ref().ok());
    let mut words: Vec<OsString> = Vec::new();
    let mut unit = None;
    let program = if let Some(manager) = placed {
        let arguments = scope_arguments(&scope.unit_prefix, &scope.slice, scope.nice);
        unit = arguments
            .iter()
            .find_map(|argument| argument.to_str()?.strip_prefix("--unit="))
            .map(str::to_owned);
        words.extend(arguments);
        words.push("--".into());
        manager.systemd_run.clone().into_os_string()
    } else if let Some(nice_tool) = nice_tool {
        words.extend(["-n".into(), scope.nice.to_string().into(), "--".into()]);
        nice_tool.into_os_string()
    } else {
        executable.as_os_str().to_owned()
    };
    let wrapped = program.as_os_str() != executable.as_os_str();
    if wrapped && let Some(ionice) = ionice {
        words.extend([
            ionice.into_os_string(),
            "-c".into(),
            "3".into(),
            "--".into(),
        ]);
    }
    // Keep explicitly supplied cleared-environment values; inherited invocation IDs are stripped.
    let caller_sets = |key: &str| env.iter().any(|(name, _)| name == key);
    let strip: Vec<&str> = match (placed, env_policy) {
        (Some(_), EnvPolicy::Cleared) => ["XDG_RUNTIME_DIR", "INVOCATION_ID"]
            .into_iter()
            .filter(|key| !caller_sets(key))
            .collect(),
        (Some(_), EnvPolicy::Inherit) => vec!["INVOCATION_ID"],
        _ => Vec::new(),
    };
    if let (false, Some(env_tool)) = (strip.is_empty(), env_tool.as_ref()) {
        words.push(env_tool.clone().into_os_string());
        for key in &strip {
            words.extend(["-u".into(), (*key).into()]);
        }
        words.push("--".into());
    }
    if wrapped {
        words.push(executable.as_os_str().to_owned());
    }
    words.extend(argv.iter().cloned());

    let mut command = Command::new(program);
    command.args(words);
    let manager_env = match (placed, env_policy) {
        (Some(manager), EnvPolicy::Cleared) if !caller_sets("XDG_RUNTIME_DIR") => {
            Some(("XDG_RUNTIME_DIR".into(), manager.runtime_dir.clone()))
        }
        _ => None,
    };
    Launch {
        command,
        manager_env,
        scope: unit.zip(placed).map(|(unit, manager)| ScopeUnit {
            unit,
            systemctl: manager.systemctl.clone(),
            runtime_dir: manager.runtime_dir.clone(),
        }),
    }
}

#[derive(Debug)]
pub(super) struct ScopeUnit {
    unit: String,
    systemctl: Option<PathBuf>,
    runtime_dir: OsString,
}

impl ScopeUnit {
    // Descendants can escape the process group; scope cleanup remains best effort.
    pub(super) fn kill_remaining(&self) {
        let Some(systemctl) = &self.systemctl else {
            return;
        };
        let spawned = Command::new(systemctl)
            .args([
                "--user",
                "kill",
                "--kill-whom=all",
                "--signal=SIGKILL",
                "--",
            ])
            .arg(&self.unit)
            .env_clear()
            .env("XDG_RUNTIME_DIR", &self.runtime_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = spawned {
            let _ = wait_bounded(&mut child, SCOPE_KILL_DEADLINE);
        }
    }
}

/// Run `work` on a fresh thread, attempting the policy's absolute nice level.
///
/// Blocks until completion and allows borrowed data. On Linux, setting the
/// thread's nice level is best effort; errors are ignored. The thread remains
/// in the caller's cgroup and does not request idle I/O. Scope enablement and
/// slice settings do not affect this function. The caller keeps its priority.
/// If thread creation fails, `work` runs on the calling thread.
///
/// # Panics
///
/// A panic in `work` is propagated to the caller.
pub fn run_on_lowered_thread<T: Send>(
    scope: &BackgroundScope,
    work: impl FnOnce() -> T + Send,
) -> T {
    let nice = i32::from(scope.nice);
    let mut work = Some(work);
    let lowered = thread::scope(|threads| {
        let work = &mut work;
        thread::Builder::new()
            .name("enzui-background".to_owned())
            .spawn_scoped(threads, move || {
                // `PRIO_PROCESS` with no pid is the calling thread on Linux.
                #[cfg(target_os = "linux")]
                let _ = rustix::process::setpriority_process(None, nice);
                work.take().map(|work| work())
            })
            .ok()
            .map(thread::ScopedJoinHandle::join)
    });
    match lowered {
        Some(Ok(Some(value))) => value,
        Some(Err(panic)) => std::panic::resume_unwind(panic),
        Some(Ok(None)) | None => (work.take().expect("work not yet run"))(),
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    fn with(pairs: &[(&str, &str)]) -> BackgroundScope {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        BackgroundScope::with_overrides("files copy/$HOME", move |name| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        })
    }

    #[test]
    fn defaults_and_administrator_overrides() {
        let stock = with(&[]);
        assert_eq!(stock.unit_prefix, "files-copy--HOME");
        assert_eq!(
            (stock.slice(), stock.nice(), stock.idle_io(), stock.enabled),
            ("background.slice", 12, true, true)
        );
        let custom = with(&[
            ("ENZUI_BACKGROUND_SLICE", "batch.slice"),
            ("ENZUI_BACKGROUND_NICE", "19"),
            ("ENZUI_BACKGROUND_IO", "inherit"),
            ("ENZUI_BACKGROUND_WORKERS", "off"),
        ]);
        assert_eq!(
            (
                custom.slice(),
                custom.nice(),
                custom.idle_io(),
                custom.enabled
            ),
            ("batch.slice", 19, false, false)
        );
        assert_eq!(
            placement(&custom),
            Placement::Fallback(FallbackReason::Disabled)
        );
        for (key, bad) in [
            ("ENZUI_BACKGROUND_SLICE", "../evil.slice"),
            ("ENZUI_BACKGROUND_SLICE", "background.service"),
            ("ENZUI_BACKGROUND_SLICE", ".slice"),
            ("ENZUI_BACKGROUND_NICE", "-5"),
            ("ENZUI_BACKGROUND_NICE", "20"),
            ("ENZUI_BACKGROUND_NICE", "high"),
        ] {
            let scope = with(&[(key, bad)]);
            assert_eq!(
                (scope.slice(), scope.nice()),
                ("background.slice", 12),
                "{key}={bad}"
            );
        }
        assert_eq!(
            BackgroundScope::with_overrides("", |_| None).unit_prefix,
            "enzui-worker"
        );
    }
}
