//! Package catalogs generated from the application's live typed registry.
//!
//! [`manifest`] serializes the broker's schema-1 wrapper around the exact v2
//! registration, without executing handlers, loading credentials or connecting.
//! Package it as `/usr/share/enzui/agent-apps/APP_ID.json` alongside its matching
//! user unit. Only export a registry that the application's background runtime
//! actually implements; GUI-only handlers do not become headless by exporting.
//!
//! The package owns its unit and app-specific sandbox. This module grants no
//! access and supplies no credentials, login daemon, shell command or auto-start.

use super::{AppFunctionRegistry, AppMetadata, AppRegistration, RegistryError};
use serde::Serialize;
use std::fmt::Write as _;

/// The argument a package's user unit starts the app with.
pub const BACKGROUND_FLAG: &str = "--agent-background";

/// The argument that prints the package catalog and exits, before any
/// credential, connector, display or window is touched.
pub const EXPORT_MANIFEST_FLAG: &str = "--export-agent-manifest";

/// Maximum catalog bytes accepted by the installed-app broker.
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

/// Why a package catalog could not be produced.
#[derive(Debug)]
pub enum ManifestError {
    /// Application metadata or function contracts did not validate.
    Registry(RegistryError),
    /// The complete wrapper exceeded the broker's file limit.
    TooLarge {
        /// Actual serialized bytes.
        bytes: usize,
    },
    /// JSON serialization failed.
    Json(serde_json::Error),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Registry(error) => write!(f, "invalid installed registration: {error}"),
            Self::TooLarge { bytes } => write!(
                f,
                "installed manifest exceeds {MAX_MANIFEST_BYTES} bytes ({bytes})"
            ),
            Self::Json(error) => write!(f, "installed manifest serialization failed: {error}"),
        }
    }
}

impl std::error::Error for ManifestError {}

#[derive(Serialize)]
struct Manifest {
    schema: u32,
    registration: AppRegistration,
    activation_unit: String,
}

/// Serializes an exact live registration with its fixed activation unit.
///
/// Call before opening a GUI or connector for an app's export mode. Output is
/// deterministic for the same ordered registry and metadata, includes no secret,
/// and has no trailing newline; write it to the package staging tree, not user
/// credential state. The package must use `--agent-background` in the matching
/// unit, with `ENZUI_APP_CREDENTIAL` and `ENZUI_CENTER_SOCKET` paths supplied by
/// the user manager. Exporting does not verify that an app implements that mode.
///
/// # Errors
/// Returns invalid registration, serialization, or catalog-size failures.
pub fn manifest<C: 'static>(
    registry: &AppFunctionRegistry<C>,
    app: AppMetadata,
) -> Result<Vec<u8>, ManifestError> {
    let registration = registry
        .registration(app)
        .map_err(ManifestError::Registry)?;
    let activation_unit = format!("enzui-agent-{}.service", registration.app.id);
    let bytes = serde_json::to_vec(&Manifest {
        schema: 1,
        registration,
        activation_unit,
    })
    .map_err(ManifestError::Json)?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestError::TooLarge { bytes: bytes.len() });
    }
    Ok(bytes)
}

/// What the app's own background sandbox allows, beyond the fixed hardening
/// every installed unit gets.
///
/// The broker's confinement is not inherited: a unit started by the user
/// manager runs with ordinary user rights unless this policy narrows them.
/// State only what the background functions need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitSandbox {
    /// Whether the worker may open network sockets. Without it only Unix
    /// sockets work (the broker socket and the session bus).
    pub network: bool,
    /// Whether the home directory stays writable. Without it home is
    /// read-only apart from [`Self::own_directories`] and [`Self::writable`].
    pub writable_home: bool,
    /// The app's own directory name under the XDG state, cache and config
    /// homes. The user manager creates these private (mode 0700) and keeps
    /// them writable, so a first run under a read-only home still persists
    /// its state.
    pub own_directories: Option<String>,
    /// Paths the worker may write even under a read-only home, as systemd
    /// specifiers or absolute paths. Each is written with a `-` prefix, so a
    /// path that does not exist yet does not stop the unit.
    pub writable: Vec<String>,
    /// Whether the unit also runs the app's window: its session D-Bus
    /// service ([`dbus_service`]) is installed, so every desktop launch runs
    /// here. The unit is then unsandboxed, like any other desktop launch:
    /// everything the app opens (a browser, a terminal, `pkexec`) inherits
    /// its execution properties, so kernel hardening, mount options and
    /// address-family limits would break those programs. Agent calls remain
    /// confined by broker consent and the app's own policy. Overrides
    /// [`Self::network`], [`Self::writable_home`] and [`Self::writable`].
    pub desktop: bool,
}

/// Why a user unit could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitError {
    /// The app id cannot name a unit or a credential file.
    InvalidAppId,
    /// The executable is not an absolute path free of whitespace and
    /// specifier characters.
    InvalidExecutable,
    /// A writable path is not absolute or starts with an unknown specifier,
    /// or contains whitespace or a newline.
    InvalidPath(String),
    /// The app directory name is not one plain path component.
    InvalidDirectory(String),
}

impl std::fmt::Display for UnitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAppId => f.write_str("app id cannot name an installed unit"),
            Self::InvalidExecutable => f.write_str("unit executable must be a plain absolute path"),
            Self::InvalidPath(path) => write!(f, "unit writable path {path:?} is not allowed"),
            Self::InvalidDirectory(name) => {
                write!(f, "unit app directory {name:?} is not one path component")
            }
        }
    }
}

impl std::error::Error for UnitError {}

// Path prefixes a writable entry may start with: an absolute path or one of the
// user-manager directory specifiers.
const PATH_PREFIXES: [&str; 6] = ["/", "%h/", "%S/", "%C/", "%E/", "%t/"];

fn plain(text: &str) -> bool {
    !text.is_empty()
        && !text
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\\')
}

/// Renders the package's `enzui-agent-APP_ID.service` user unit.
///
/// The unit starts `executable` with [`BACKGROUND_FLAG`]
/// only on demand: it has no `[Install]` section, so installing it starts
/// nothing at login. The credential and socket paths are the broker's fixed
/// per-user locations. A worker-only unit gets kernel hardening (no new
/// privileges, no namespaces, native system calls only), read-only system
/// directories, a private `/tmp` and the app's own network and home policy.
/// A desktop unit gets none of that (see [`UnitSandbox::desktop`]).
///
/// # Errors
/// An app id, executable or writable path that cannot be expressed safely.
pub fn unit(app_id: &str, executable: &str, sandbox: &UnitSandbox) -> Result<String, UnitError> {
    if !super::registry::valid_id(app_id) {
        return Err(UnitError::InvalidAppId);
    }
    if !executable.starts_with('/') || !plain(executable) || executable.contains('%') {
        return Err(UnitError::InvalidExecutable);
    }
    for path in &sandbox.writable {
        if !plain(path) || !PATH_PREFIXES.iter().any(|prefix| path.starts_with(prefix)) {
            return Err(UnitError::InvalidPath(path.clone()));
        }
    }
    if let Some(name) = &sandbox.own_directories
        && (!super::registry::valid_id(name) || name.starts_with('.') || name.contains('%'))
    {
        return Err(UnitError::InvalidDirectory(name.clone()));
    }

    let mut text = format!(
        "# Generated from the app's live registry by enzui-agent; do not edit.\n\
         [Unit]\n\
         Description=enzUI agent worker for {app_id}\n\
         \n\
         [Service]\n\
         Type=exec\n\
         ExecStart={executable} {BACKGROUND_FLAG}\n\
         Environment=ENZUI_APP_CREDENTIAL=%S/enzui/apps/{app_id}.credential.json\n\
         Environment=ENZUI_CENTER_SOCKET=%t/enzui/center.sock\n\
         Restart=no\n",
    );
    if !sandbox.desktop {
        text.push_str(
            "UMask=0077\n\
             NoNewPrivileges=yes\n\
             LockPersonality=yes\n\
             RestrictRealtime=yes\n\
             RestrictSUIDSGID=yes\n\
             RestrictNamespaces=yes\n\
             SystemCallArchitectures=native\n\
             PrivateTmp=yes\n\
             ProtectSystem=strict\n",
        );
        if sandbox.writable_home {
            text.push_str("ProtectHome=no\n");
        } else {
            text.push_str("ProtectHome=read-only\n");
        }
        // The broker's runtime directory holds the socket; the credential
        // lives in state. Both stay reachable under a strict system and a
        // read-only home.
        text.push_str("ReadWritePaths=-%t/enzui\n");
        for path in &sandbox.writable {
            let _ = writeln!(text, "ReadWritePaths=-{path}");
        }
        if sandbox.network {
            text.push_str("RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6\n");
        } else {
            text.push_str("PrivateNetwork=yes\nRestrictAddressFamilies=AF_UNIX\n");
        }
    }
    if let Some(name) = &sandbox.own_directories {
        for kind in ["State", "Cache", "Configuration"] {
            let _ = write!(text, "{kind}Directory={name}\n{kind}DirectoryMode=0700\n");
        }
    }
    Ok(text)
}

/// Renders the package's session D-Bus service file, installed as
/// `dbus-1/services/APP_ID.service`.
///
/// It makes the application's own name activatable: a launch addressed to
/// `APP_ID` (a `DBusActivatable` desktop entry, or a direct run of the
/// installed executable) makes the bus start the existing
/// `enzui-agent-APP_ID.service` unit, so a window and a windowless worker are
/// the same unit's main process. `Exec` is the same background start, for a
/// bus without systemd.
///
/// # Errors
/// An app id or executable that cannot be expressed safely.
pub fn dbus_service(app_id: &str, executable: &str) -> Result<String, UnitError> {
    if !super::registry::valid_id(app_id) {
        return Err(UnitError::InvalidAppId);
    }
    if !executable.starts_with('/') || !plain(executable) || executable.contains('%') {
        return Err(UnitError::InvalidExecutable);
    }
    Ok(format!(
        "# Generated by enzui-agent; do not edit.\n\
         [D-BUS Service]\n\
         Name={app_id}\n\
         Exec={executable} {BACKGROUND_FLAG}\n\
         SystemdService=enzui-agent-{app_id}.service\n",
    ))
}

/// The graceful-stop contract an application implements: the lifecycle
/// interface on its own bus name answers a stop query and a reviewed stop.
pub const GRACEFUL_STOP_LIFECYCLE_V1: &str = "application-lifecycle-v1";

/// The longest title or purpose a background declaration carries.
pub const MAX_DECLARATION_TEXT: usize = 160;

/// What may start an application that runs without its window. Each source
/// is reported apart; declaring one is a capability, never a permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActivationSource {
    /// The person started it.
    User,
    /// An agent's authorized call started it.
    Agent,
    /// Login autostart, when the person turns it on.
    Autostart,
    /// Work the person started keeps it running (a transfer, a download).
    OngoingWork,
}

/// An application's background declaration, before the identity the
/// package fixes (app id, unit, executable) is added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundDeclaration {
    /// The application's display name.
    pub title: String,
    /// One sentence on what it does while no window is open.
    pub purpose: String,
    /// What may start or keep it running.
    pub activation_sources: Vec<ActivationSource>,
    /// Whether it can start at login. A capability only: the OS keeps
    /// autostart off until the person enables it.
    pub autostart_supported: bool,
    /// Whether it answers [`GRACEFUL_STOP_LIFECYCLE_V1`]. Without it the OS
    /// cannot ask before stopping it.
    pub graceful_stop: bool,
    /// Whether its lifecycle interface answers `Work` with live work counts
    /// (the app calls `StopControl::publish_work`). The OS asks only apps
    /// that declare it. Requires `graceful_stop`, which serves the interface.
    pub work: bool,
}

/// Why a background declaration could not be produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclarationError {
    /// The app id or executable cannot name the installed unit.
    Unit(UnitError),
    /// The title or purpose is empty, multi-line or longer than
    /// [`MAX_DECLARATION_TEXT`] characters.
    Text(&'static str),
    /// No activation source, or one listed twice.
    ActivationSources,
    /// `work` without `graceful_stop`: nothing would serve the `Work` read.
    WorkWithoutLifecycle,
}

impl std::fmt::Display for DeclarationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unit(error) => error.fmt(f),
            Self::Text(field) => write!(
                f,
                "background {field} must be one line of 1 to {MAX_DECLARATION_TEXT} characters"
            ),
            Self::ActivationSources => {
                f.write_str("background activation sources must be listed once each, at least one")
            }
            Self::WorkWithoutLifecycle => {
                f.write_str("background work observation needs graceful stop, which serves it")
            }
        }
    }
}

impl std::error::Error for DeclarationError {}

#[derive(Serialize)]
struct Declaration<'a> {
    schema: u32,
    app_id: &'a str,
    title: &'a str,
    unit: String,
    executable: &'a str,
    purpose: &'a str,
    activation_sources: &'a [ActivationSource],
    autostart_supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    graceful_stop: Option<&'static str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    work: bool,
}

/// Serializes the OS background-app declaration, installed as
/// `/usr/share/enzui/background-apps/APP_ID.json`.
///
/// It names the same `enzui-agent-APP_ID.service` unit and executable that
/// [`unit()`] renders, so pass the same `app_id` and `executable`. The OS
/// verifies the installation itself; the declaration claims capabilities and
/// grants nothing. Output is deterministic and has no trailing newline.
///
/// # Errors
/// An app id or executable [`unit()`] would refuse, unbounded or multi-line
/// text, an empty or repeated activation source, or `work` without
/// `graceful_stop`.
pub fn background_declaration(
    app_id: &str,
    executable: &str,
    declaration: &BackgroundDeclaration,
) -> Result<Vec<u8>, DeclarationError> {
    if !super::registry::valid_id(app_id) {
        return Err(DeclarationError::Unit(UnitError::InvalidAppId));
    }
    if !executable.starts_with('/') || !plain(executable) || executable.contains('%') {
        return Err(DeclarationError::Unit(UnitError::InvalidExecutable));
    }
    let one_line = |text: &str| {
        !text.trim().is_empty()
            && text.chars().count() <= MAX_DECLARATION_TEXT
            && !text.chars().any(char::is_control)
    };
    if !one_line(&declaration.title) {
        return Err(DeclarationError::Text("title"));
    }
    if !one_line(&declaration.purpose) {
        return Err(DeclarationError::Text("purpose"));
    }
    let mut sources = declaration.activation_sources.clone();
    sources.sort();
    sources.dedup();
    if sources.is_empty() || sources.len() != declaration.activation_sources.len() {
        return Err(DeclarationError::ActivationSources);
    }
    if declaration.work && !declaration.graceful_stop {
        return Err(DeclarationError::WorkWithoutLifecycle);
    }
    serde_json::to_vec(&Declaration {
        schema: 1,
        app_id,
        title: &declaration.title,
        unit: format!("enzui-agent-{app_id}.service"),
        executable,
        purpose: &declaration.purpose,
        activation_sources: &declaration.activation_sources,
        autostart_supported: declaration.autostart_supported,
        graceful_stop: declaration
            .graceful_stop
            .then_some(GRACEFUL_STOP_LIFECYCLE_V1),
        work: declaration.work,
    })
    .map_err(|_| DeclarationError::Text("declaration"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dbus_service_activates_the_unit_for_the_app_name() {
        let service = dbus_service("org.example.Files", "/usr/bin/example-files").unwrap();
        assert_eq!(
            service,
            "# Generated by enzui-agent; do not edit.\n\
             [D-BUS Service]\n\
             Name=org.example.Files\n\
             Exec=/usr/bin/example-files --agent-background\n\
             SystemdService=enzui-agent-org.example.Files.service\n"
        );
        assert_eq!(
            dbus_service("not an id", "/usr/bin/x"),
            Err(UnitError::InvalidAppId)
        );
        for executable in ["example-files", "/usr/bin/example files", "/usr/bin/%h"] {
            assert_eq!(
                dbus_service("org.example.Files", executable),
                Err(UnitError::InvalidExecutable),
                "{executable}"
            );
        }
    }

    #[test]
    fn a_desktop_unit_passes_no_sandbox_to_what_the_app_opens() {
        // Each of these breaks something a desktop app opens: Flatpak
        // browsers (namespaces, and the uid remap any mount option causes),
        // pkexec and sudo, 32-bit programs, Bluetooth tools, realtime audio.
        let keys = [
            "NoNewPrivileges=",
            "LockPersonality=",
            "RestrictRealtime=",
            "RestrictSUIDSGID=",
            "RestrictNamespaces=",
            "SystemCallArchitectures=",
            "ProtectSystem=",
            "ProtectHome=",
            "PrivateTmp=",
            "PrivateNetwork=",
            "ReadWritePaths=",
            "RestrictAddressFamilies=",
            "UMask=",
        ];
        let sandbox = |desktop| UnitSandbox {
            network: false,
            writable_home: false,
            own_directories: Some("example-files".into()),
            writable: vec!["%h/Documents".into()],
            desktop,
        };
        let desktop = unit(
            "org.example.Files",
            "/usr/bin/example-files",
            &sandbox(true),
        )
        .unwrap();
        for key in keys {
            assert!(
                !desktop.contains(key),
                "desktop unit carries {key}:\n{desktop}"
            );
        }
        assert!(
            desktop.contains("StateDirectory=example-files\n"),
            "{desktop}"
        );

        let worker = unit(
            "org.example.Files",
            "/usr/bin/example-files",
            &sandbox(false),
        )
        .unwrap();
        for key in keys.iter().filter(|key| **key != "ProtectHome=") {
            assert!(worker.contains(key), "worker unit lacks {key}:\n{worker}");
        }
        assert!(worker.contains("ProtectHome=read-only\n"), "{worker}");
    }
}
