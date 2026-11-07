//! Opening URIs and files through the desktop's own launcher.
//!
//! An unconfined process should not use the OpenURI portal: the portal asks
//! which application should handle the target, and a process without an
//! application id gets an "open with" dialog instead of the user's default
//! handler (see [`crate::portal::sandbox`]). [`DesktopLauncherBackend`] runs
//! the desktop launcher (`xdg-open`, or `gio open`), which applies the user's
//! `mimeapps.list` defaults, and reports its exit status as the reply.
//! [`host_open_uri_backend`] picks the right route for the running process.
//!
//! Showing a file in its folder asks the file manager over
//! `org.freedesktop.FileManager1.ShowItems` when the `portal-dbus` feature
//! supplies a D-Bus client, and opens the containing directory otherwise.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use super::{OpenUri, OpenUriBackend, OpenUriReply, RetainedDirectory, RetainedFile};
use crate::activation::ActivationContext;
use crate::portal::backend::{BackendError, RequestId};
use crate::portal::parent::ParentHandle;
use crate::portal::sandbox::{SandboxEvidence, is_sandboxed};
use crate::portal::{CancelReason, CompletionSignal};

/// Session variables a desktop launcher needs to find the display, the bus,
/// the user's defaults and the locale. Everything else is withheld.
const INHERITED_ENVIRONMENT: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_MESSAGES",
    "LC_CTYPE",
    "WAYLAND_DISPLAY",
    "DISPLAY",
    "XAUTHORITY",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_CONFIG_DIRS",
    "XDG_DATA_HOME",
    "XDG_DATA_DIRS",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "XDG_SESSION_TYPE",
    "DESKTOP_SESSION",
    "KDE_FULL_SESSION",
    "KDE_SESSION_VERSION",
    "GNOME_DESKTOP_SESSION_ID",
    "BROWSER",
];

/// Which route opens URIs and files for a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenRoute {
    /// The desktop's own launcher, for an unconfined process.
    DesktopLauncher,
    /// The OpenURI portal, the only way out of a sandbox.
    Portal,
}

impl OpenRoute {
    /// The route for a process with this confinement evidence.
    #[must_use]
    pub fn for_evidence(evidence: &SandboxEvidence) -> Self {
        if is_sandboxed(evidence) {
            Self::Portal
        } else {
            Self::DesktopLauncher
        }
    }
}

/// The OpenURI backend this process should use.
///
/// A sandboxed process gets the portal backend (with the `portal-dbus`
/// feature; without it, an unavailable backend). An unconfined process gets
/// [`DesktopLauncherBackend`]. The backend is `Send`, so an application task
/// (`iced::Task::perform`) can hold it while it waits for the request to
/// finish.
#[must_use]
pub fn host_open_uri_backend() -> Box<dyn OpenUriBackend + Send> {
    backend_for_route(OpenRoute::for_evidence(&SandboxEvidence::from_host()))
}

/// The OpenURI backend for an explicit route.
#[must_use]
pub fn backend_for_route(route: OpenRoute) -> Box<dyn OpenUriBackend + Send> {
    match route {
        OpenRoute::DesktopLauncher => Box::new(DesktopLauncherBackend::from_host()),
        OpenRoute::Portal => portal_backend(),
    }
}

#[cfg(feature = "portal-dbus")]
fn portal_backend() -> Box<dyn OpenUriBackend + Send> {
    match crate::portal::dbus_backend::DbusBackend::new() {
        Ok(backend) => Box::new(backend),
        Err(_) => Box::new(super::UnavailableOpenUriBackend),
    }
}

#[cfg(not(feature = "portal-dbus"))]
fn portal_backend() -> Box<dyn OpenUriBackend + Send> {
    Box::new(super::UnavailableOpenUriBackend)
}

/// A launcher program and the arguments placed before the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    program: PathBuf,
    leading_args: Vec<OsString>,
}

impl Launcher {
    /// A launcher that runs `program` with `leading_args`, then the target.
    #[must_use]
    pub fn new(program: impl Into<PathBuf>, leading_args: Vec<OsString>) -> Self {
        Self {
            program: program.into(),
            leading_args,
        }
    }

    /// The first launcher found on `search_path`: `xdg-open`, which defers to
    /// each desktop's own tool, then `gio open`.
    #[must_use]
    pub fn find(search_path: &[PathBuf]) -> Option<Self> {
        let find = |name: &str| {
            search_path
                .iter()
                .filter(|dir| dir.is_absolute())
                .map(|dir| dir.join(name))
                .find(|candidate| is_executable(candidate))
        };
        if let Some(program) = find("xdg-open") {
            return Some(Self::new(program, Vec::new()));
        }
        find("gio").map(|program| Self::new(program, vec![OsString::from("open")]))
    }

    fn command(&self, target: &str) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.leading_args).arg(target);
        command
    }

    /// Maps the launcher's exit status to a reply. `xdg-open` exits with 3
    /// when no tool for the desktop exists, which is an absent launcher
    /// rather than a failure of this target.
    fn reply_for(&self, code: Option<i32>) -> OpenUriReply {
        let is_xdg_open = self
            .program
            .file_name()
            .is_some_and(|name| name == "xdg-open");
        match code {
            Some(0) => OpenUriReply::Success,
            Some(3) if is_xdg_open => OpenUriReply::Unavailable,
            _ => OpenUriReply::Failed,
        }
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

#[derive(Default)]
struct Shared {
    replies: HashMap<RequestId, OpenUriReply>,
    /// Requests closed by the caller; a late launcher exit is discarded.
    closed: Vec<RequestId>,
}

/// Opens URIs and files with the desktop launcher and the user's defaults.
pub struct DesktopLauncherBackend {
    launcher: Option<Launcher>,
    next_id: AtomicU64,
    shared: Arc<Mutex<Shared>>,
    signal: CompletionSignal,
    show_items: bool,
}

impl std::fmt::Debug for DesktopLauncherBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DesktopLauncherBackend")
            .field("launcher", &self.launcher)
            .finish()
    }
}

impl DesktopLauncherBackend {
    /// A backend using the launcher found on this process's `PATH`.
    #[must_use]
    pub fn from_host() -> Self {
        let path = std::env::var_os("PATH").unwrap_or_default();
        let search: Vec<PathBuf> = std::env::split_paths(&path).collect();
        Self::with_launcher(Launcher::find(&search))
    }

    /// A backend using an explicit launcher, or none (every request then
    /// reports [`BackendError::Unavailable`]).
    #[must_use]
    pub fn with_launcher(launcher: Option<Launcher>) -> Self {
        Self {
            launcher,
            next_id: AtomicU64::new(1),
            shared: Arc::default(),
            signal: CompletionSignal::new(),
            show_items: cfg!(feature = "portal-dbus"),
        }
    }

    /// Whether "show in folder" asks the file manager over D-Bus before
    /// falling back to opening the directory. On by default with the
    /// `portal-dbus` feature.
    #[must_use]
    pub fn with_file_manager_show_items(mut self, enabled: bool) -> Self {
        self.show_items = enabled && cfg!(feature = "portal-dbus");
        self
    }

    fn allocate(&self) -> Result<RequestId, BackendError> {
        self.next_id
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |id| id.checked_add(1))
            .map(RequestId::new)
            .map_err(|_| BackendError::ResourceExhausted)
    }

    fn launch(
        &mut self,
        target: &str,
        activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        let launcher = self.launcher.clone().ok_or(BackendError::Unavailable)?;
        let mut command = launcher.command(target);
        prepare_environment(&mut command, activation);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = command.spawn().map_err(|_| BackendError::Unavailable)?;
        let id = self.allocate()?;
        let shared = Arc::clone(&self.shared);
        let signal = self.signal.clone();
        std::thread::Builder::new()
            .name("enzui-open-uri".into())
            .spawn(move || {
                let code = child.wait().ok().and_then(|status| status.code());
                finish(&shared, &signal, id, launcher.reply_for(code));
            })
            .map_err(|_| BackendError::Failed)?;
        Ok(id)
    }
}

fn prepare_environment(command: &mut Command, activation: Option<&ActivationContext>) {
    command.env_clear();
    for key in INHERITED_ENVIRONMENT {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    if let Some(context) = activation {
        for (key, value) in context.launch_environment(Instant::now()) {
            command.env(key, value);
        }
    }
}

fn finish(shared: &Mutex<Shared>, signal: &CompletionSignal, id: RequestId, reply: OpenUriReply) {
    let mut state = shared.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(index) = state.closed.iter().position(|closed| *closed == id) {
        state.closed.swap_remove(index);
    } else {
        state.replies.insert(id, reply);
    }
    drop(state);
    signal.notify();
}

/// The path a retained descriptor names right now, provided that path still
/// names the same file. A native launcher can only be given a path; checking
/// identity refuses a path that was renamed over in the meantime.
fn retained_path(file: &RetainedFile) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        descriptor_path(file.raw(), file.identity())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = file;
        None
    }
}

fn retained_directory_path(directory: &RetainedDirectory) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        descriptor_path(directory.raw(), directory.identity())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = directory;
        None
    }
}

/// The path a descriptor was opened by, if it still names the same file or
/// directory: a renamed or replaced target is refused.
#[cfg(target_os = "linux")]
fn descriptor_path(descriptor: &std::fs::File, identity: super::FileIdentity) -> Option<PathBuf> {
    use std::os::fd::AsRawFd;
    let link = std::fs::read_link(format!("/proc/self/fd/{}", descriptor.as_raw_fd())).ok()?;
    let on_disk = std::fs::File::open(&link).ok()?;
    (super::FileIdentity::of(&on_disk) == identity).then_some(link)
}

impl OpenUriBackend for DesktopLauncherBackend {
    fn start_open_uri(
        &mut self,
        uri: &OpenUri,
        parent: &ParentHandle,
    ) -> Result<RequestId, BackendError> {
        self.start_open_uri_with_activation(uri, parent, None)
    }

    fn start_open_uri_with_activation(
        &mut self,
        uri: &OpenUri,
        _parent: &ParentHandle,
        activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        self.launch(uri.as_str(), activation)
    }

    fn start_open_file(
        &mut self,
        file: &RetainedFile,
        _parent: &ParentHandle,
        activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        let path = retained_path(file).ok_or(BackendError::Failed)?;
        let uri = OpenUri::from_file_path(&path).map_err(|_| BackendError::Failed)?;
        self.launch(uri.as_str(), activation)
    }

    fn start_open_directory(
        &mut self,
        directory: &RetainedDirectory,
        _parent: &ParentHandle,
        activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        let path = retained_directory_path(directory).ok_or(BackendError::Failed)?;
        let uri = OpenUri::from_file_path(&path).map_err(|_| BackendError::Failed)?;
        self.launch(uri.as_str(), activation)
    }

    fn start_open_containing_folder(
        &mut self,
        file: &RetainedFile,
        _parent: &ParentHandle,
        activation: Option<&ActivationContext>,
    ) -> Result<RequestId, BackendError> {
        let path = retained_path(file).ok_or(BackendError::Failed)?;
        let file_uri = OpenUri::from_file_path(&path).map_err(|_| BackendError::Failed)?;
        let directory = path.parent().ok_or(BackendError::Failed)?;
        let directory_uri = OpenUri::from_file_path(directory).map_err(|_| BackendError::Failed)?;
        if self.show_items {
            let id = self.allocate()?;
            let startup_id = activation
                .map(|context| context.launch_environment(Instant::now()))
                .and_then(|pairs| pairs.into_iter().next().map(|(_, value)| value))
                .unwrap_or_default();
            let shared = Arc::clone(&self.shared);
            let signal = self.signal.clone();
            let launcher = self.launcher.clone();
            let directory_target = directory_uri.as_str().to_owned();
            std::thread::Builder::new()
                .name("enzui-show-items".into())
                .spawn(move || {
                    let reply = if show_items(file_uri.as_str(), &startup_id) {
                        OpenUriReply::Success
                    } else {
                        open_with(launcher.as_ref(), &directory_target)
                    };
                    finish(&shared, &signal, id, reply);
                })
                .map_err(|_| BackendError::Failed)?;
            return Ok(id);
        }
        self.launch(directory_uri.as_str(), activation)
    }

    fn poll_open_uri(&mut self, id: RequestId) -> Option<OpenUriReply> {
        self.shared
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replies
            .remove(&id)
    }

    fn close_open_uri(&mut self, id: RequestId, _reason: CancelReason) {
        // A launched handler cannot be recalled; closing only discards the
        // launcher's answer.
        let mut state = self.shared.lock().unwrap_or_else(PoisonError::into_inner);
        if state.replies.remove(&id).is_none() && !state.closed.contains(&id) {
            state.closed.push(id);
        }
    }

    fn open_uri_completion_signal(&self) -> Option<CompletionSignal> {
        Some(self.signal.clone())
    }

    fn describe_open_uri(&self) -> String {
        match &self.launcher {
            Some(launcher) => format!("desktop launcher {}", launcher.program.display()),
            None => "desktop launcher unavailable (no xdg-open or gio on PATH)".into(),
        }
    }
}

/// Runs the launcher on a worker thread's behalf and waits for its answer.
fn open_with(launcher: Option<&Launcher>, target: &str) -> OpenUriReply {
    let Some(launcher) = launcher else {
        return OpenUriReply::Unavailable;
    };
    let mut command = launcher.command(target);
    prepare_environment(&mut command, None);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match command.status() {
        Ok(status) => launcher.reply_for(status.code()),
        Err(_) => OpenUriReply::Unavailable,
    }
}

/// Asks the session's file manager to show `file_uri` selected in its folder.
#[cfg(feature = "portal-dbus")]
fn show_items(file_uri: &str, startup_id: &str) -> bool {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return false;
    };
    runtime.block_on(async {
        let Ok(connection) = zbus::Connection::session().await else {
            return false;
        };
        let body = (vec![file_uri], startup_id);
        let call = connection.call_method(
            Some("org.freedesktop.FileManager1"),
            "/org/freedesktop/FileManager1",
            Some("org.freedesktop.FileManager1"),
            "ShowItems",
            &body,
        );
        matches!(
            tokio::time::timeout(std::time::Duration::from_secs(5), call).await,
            Ok(Ok(_))
        )
    })
}

#[cfg(not(feature = "portal-dbus"))]
fn show_items(_file_uri: &str, _startup_id: &str) -> bool {
    false
}

#[cfg(test)]
mod tests;
