//! Flatpak `finish-args` validation against a least-privilege allow-list.
//!
//! Unknown grants are rejected. Recognized conditional and permission-bearing
//! metadata forms, plus access suffixes, are normalized into the direct-grant
//! checks.

/// A Flatpak manifest containing the finish arguments relevant to permission review.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlatpakManifest {
    /// Verbatim `finish-args`; an empty list grants no permissions.
    pub finish_args: Vec<String>,
    /// Application ID used to recognize app-private D-Bus names.
    ///
    /// If absent, only the fixed safe namespaces are accepted.
    pub app_id: Option<String>,
}

impl FlatpakManifest {
    /// Builds a manifest from its finish-args.
    #[must_use]
    pub fn from_finish_args<I, S>(args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            finish_args: args.into_iter().map(Into::into).collect(),
            app_id: None,
        }
    }

    /// Sets the application ID, enabling app-private D-Bus name matching.
    #[must_use]
    pub fn with_app_id(mut self, id: impl Into<String>) -> Self {
        self.app_id = Some(id.into());
        self
    }
}

/// A least-privilege violation and its offending `finish-args` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyViolation {
    /// A filesystem grant that reaches the host or user home.
    HostFilesystem(String),
    /// A grant of every device.
    DeviceAll(String),
    /// An unrestricted session- or system-bus socket.
    UnrestrictedBus(String),
    /// A wildcard D-Bus name grant.
    WildcardDBus(String),
    /// A D-Bus grant naming the portal desktop frontend.
    PortalDesktopBusGrant(String),
    /// A grant that does not match any known-safe pattern.
    UnrecognizedGrant(String),
    /// A D-Bus grant naming a service that can escape or subvert the sandbox.
    SandboxEscapeBusGrant(String),
    /// An environment assignment denied for controlling loader, module, interpreter, or runtime startup behavior.
    CodeInjectingEnv(String),
    /// A metadata write to a permission-bearing group that cannot be reclassified precisely.
    PermissionViaMetadata(String),
    /// A persistent path containing a parent-directory component.
    PersistTraversal(String),
    /// A USB grant that is broad, malformed, or hidden in an external list.
    BroadUsbGrant(String),
}

impl PolicyViolation {
    /// The offending `finish-args` string this violation was raised for.
    #[must_use]
    pub fn offending_arg(&self) -> &str {
        match self {
            Self::HostFilesystem(s)
            | Self::DeviceAll(s)
            | Self::UnrestrictedBus(s)
            | Self::WildcardDBus(s)
            | Self::PortalDesktopBusGrant(s)
            | Self::UnrecognizedGrant(s)
            | Self::SandboxEscapeBusGrant(s)
            | Self::CodeInjectingEnv(s)
            | Self::PermissionViaMetadata(s)
            | Self::PersistTraversal(s)
            | Self::BroadUsbGrant(s) => s,
        }
    }
}

const HOST_FILESYSTEM_SCOPES: &[&str] = &["host", "host-os", "host-etc", "host-root", "home", "~"];

const PORTAL_DESKTOP_NAME: &str = "org.freedesktop.portal.Desktop";

// Match sandbox-escape services by D-Bus namespace so child names are blocked too.
const DANGEROUS_DBUS_NAMESPACES: &[&str] = &[
    "org.freedesktop.Flatpak",
    "org.freedesktop.portal.Flatpak",
    "org.freedesktop.portal.Desktop",
    "org.freedesktop.systemd1",
    "org.freedesktop.login1",
    "org.freedesktop.PolicyKit1",
    "org.freedesktop.impl.portal",
    "org.freedesktop.secrets",
];

// Safe D-Bus namespaces use dot-boundary matching; dangerous namespaces are checked first.
const SAFE_DBUS_NAMESPACES: &[&str] = &[
    "org.freedesktop.DBus",
    "org.freedesktop.portal",
    "org.gtk.vfs",
    "ca.desrt.dconf",
    "org.mpris.MediaPlayer2",
    "org.freedesktop.Notifications",
    "org.kde.StatusNotifierWatcher",
    "com.canonical.AppMenu.Registrar",
    "org.a11y.Bus",
    "org.gtk.MountOperationHandler",
];

// These variables alter loader, module, interpreter, or runtime startup behavior.
const CODE_INJECTING_ENV_VARS: &[&str] = &[
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "LD_AUDIT",
    "GIO_MODULE_DIR",
    "GTK_MODULES",
    "GTK_PATH",
    "GST_PLUGIN_PATH",
    "GST_PLUGIN_SYSTEM_PATH",
    "PYTHONPATH",
    "PERL5LIB",
    "NODE_OPTIONS",
    "MALLOC_CONF",
];

// Writes to these metadata groups can grant the same permissions as direct arguments.
const PERMISSION_METADATA_GROUPS: &[&str] = &[
    "Context",
    "Session Bus Policy",
    "System Bus Policy",
    "Environment",
];

// X11, unrestricted buses, input capture, and the host SSH agent are intentionally excluded.
const SAFE_SOCKETS: &[&str] = &[
    "wayland",
    "pulseaudio",
    "cups",
    "pcsc",
    "gpg-agent",
    "inherit-wayland-socket",
];

// `input` is excluded because host device access can expose events outside the app.
const SAFE_DEVICES: &[&str] = &["dri", "kvm", "shm", "usb"];

const SAFE_SHARES: &[&str] = &["ipc", "network"];

// `devel` is excluded because it enables ptrace and performance-monitoring access.
const SAFE_ALLOWS: &[&str] = &["bluetooth", "multiarch", "canbus", "per-app-dev-shm"];

// Runtime-directory scopes are excluded because read-only access can still connect to sockets.
const SAFE_FILESYSTEM_PREFIXES: &[&str] = &[
    "xdg-desktop",
    "xdg-documents",
    "xdg-download",
    "xdg-music",
    "xdg-pictures",
    "xdg-public-share",
    "xdg-videos",
    "xdg-templates",
    "xdg-config",
    "xdg-cache",
    "xdg-data",
];

/// Reviews finish arguments in manifest order, returning one violation for each argument not proven safe.
#[must_use]
pub fn review(manifest: &FlatpakManifest) -> Vec<PolicyViolation> {
    let mut out = Vec::new();
    for arg in &manifest.finish_args {
        let arg = arg.trim();
        if let Some(v) = classify(arg, manifest.app_id.as_deref()) {
            out.push(v);
        }
    }
    out
}

/// Whether a manifest is clean (no least-privilege violations).
#[must_use]
pub fn is_least_privilege(manifest: &FlatpakManifest) -> bool {
    review(manifest).is_empty()
}

// Strip access suffixes before classifying filesystem, device, socket, share, and allow values.
fn normalise_value(value: &str) -> &str {
    for suffix in [":ro", ":rw", ":create"] {
        if let Some(base) = value.strip_suffix(suffix) {
            return base;
        }
    }
    value
}

fn strip_trailing_slash(path: &str) -> &str {
    if path == "/" {
        return path;
    }
    path.strip_suffix('/').unwrap_or(path)
}

// Compare path components exactly so names merely containing two dots remain valid.
fn path_contains_parent_traversal(path: &str) -> bool {
    path.split('/').any(|component| component == "..")
}

// Reclassification retains the manifest spelling in the reported violation.
fn rewrite_offending(v: PolicyViolation, original: &str) -> PolicyViolation {
    let s = original.to_owned();
    match v {
        PolicyViolation::HostFilesystem(_) => PolicyViolation::HostFilesystem(s),
        PolicyViolation::DeviceAll(_) => PolicyViolation::DeviceAll(s),
        PolicyViolation::UnrestrictedBus(_) => PolicyViolation::UnrestrictedBus(s),
        PolicyViolation::WildcardDBus(_) => PolicyViolation::WildcardDBus(s),
        PolicyViolation::PortalDesktopBusGrant(_) => PolicyViolation::PortalDesktopBusGrant(s),
        PolicyViolation::UnrecognizedGrant(_) => PolicyViolation::UnrecognizedGrant(s),
        PolicyViolation::SandboxEscapeBusGrant(_) => PolicyViolation::SandboxEscapeBusGrant(s),
        PolicyViolation::CodeInjectingEnv(_) => PolicyViolation::CodeInjectingEnv(s),
        PolicyViolation::PermissionViaMetadata(_) => PolicyViolation::PermissionViaMetadata(s),
        PolicyViolation::PersistTraversal(_) => PolicyViolation::PersistTraversal(s),
        PolicyViolation::BroadUsbGrant(_) => PolicyViolation::BroadUsbGrant(s),
    }
}

// Match D-Bus namespaces only at a dot boundary to exclude similarly prefixed services.
fn dbus_name_in_namespace(name: &str, namespace: &str) -> bool {
    if name == namespace {
        return true;
    }
    match name.strip_prefix(namespace) {
        Some(rest) => rest.starts_with('.'),
        None => false,
    }
}

// Accept only four-digit vendor queries, vendor-qualified devices, or concrete class/subclass queries.
fn usb_query_is_narrow(entry: &str) -> bool {
    fn is_hex4(s: &str) -> bool {
        s.len() == 4 && s.chars().all(|c| c.is_ascii_hexdigit())
    }
    fn is_hex2(s: &str) -> bool {
        s.len() == 2 && s.chars().all(|c| c.is_ascii_hexdigit())
    }

    if let Some(rest) = entry.strip_prefix("vnd:") {
        return match rest.split_once("+dev:") {
            Some((vendor, device)) => is_hex4(vendor) && is_hex4(device),
            None => is_hex4(rest),
        };
    }

    if let Some(rest) = entry.strip_prefix("cls:") {
        return match rest.split_once(':') {
            Some((class, sub)) => is_hex2(class) && is_hex2(sub),
            None => false,
        };
    }

    false
}

// Any entry not proven safe by a classification arm is rejected.
fn classify(arg: &str, app_id: Option<&str>) -> Option<PolicyViolation> {
    for prefix in [
        "--nosocket=",
        "--nodevice=",
        "--unshare=",
        "--disallow=",
        "--nofilesystem=",
        "--nousb=",
        "--remove-policy=",
        "--no-talk-name=",
        "--system-no-talk-name=",
    ] {
        if arg.starts_with(prefix) {
            return None;
        }
    }

    // Conditional grants are classified by value because the condition changes when, not what, they grant.
    for (if_prefix, unconditional_prefix) in [
        ("--socket-if=", "--socket="),
        ("--device-if=", "--device="),
        ("--share-if=", "--share="),
        ("--allow-if=", "--allow="),
    ] {
        if let Some(rest) = arg.strip_prefix(if_prefix) {
            // Split at the final colon because a value may itself carry an access suffix.
            let value = match rest.rsplit_once(':') {
                Some((v, _cond)) => v,
                None => rest,
            };
            let reclassified = format!("{unconditional_prefix}{value}");
            // Report the manifest argument rather than the synthesized unconditional spelling.
            return classify(&reclassified, app_id).map(|v| rewrite_offending(v, arg));
        }
    }

    // Only flags whose values cannot grant permissions may bypass value classification.
    for prefix in ["--require-version=", "--command=", "--unset-env="] {
        if arg.starts_with(prefix) {
            return None;
        }
    }
    if arg == "--no-inherit-permissions" {
        return None;
    }

    // Accessibility ownership is outside the session- and system-bus grants screened here.
    if arg.starts_with("--a11y-own-name=") {
        return None;
    }

    // Loader and module-path variables turn `--env` into a code-execution grant.
    if let Some(assignment) = arg.strip_prefix("--env=") {
        let var = assignment.split('=').next().unwrap_or(assignment);
        if CODE_INJECTING_ENV_VARS.contains(&var) {
            return Some(PolicyViolation::CodeInjectingEnv(arg.to_owned()));
        }
        return None;
    }

    // Parent components escape the per-application home used by `--persist`.
    if let Some(path) = arg.strip_prefix("--persist=") {
        if path_contains_parent_traversal(path) {
            return Some(PolicyViolation::PersistTraversal(arg.to_owned()));
        }
        return None;
    }

    // USB list files are rejected because their grants cannot be inspected from the manifest.
    if arg.starts_with("--usb-list-file=") {
        return Some(PolicyViolation::BroadUsbGrant(arg.to_owned()));
    }
    for prefix in ["--usb=", "--usb-list="] {
        if let Some(list) = arg.strip_prefix(prefix) {
            // A leading exclusion only narrows a USB list.
            for entry in list.split(';').filter(|e| !e.is_empty()) {
                let entry = entry.trim();
                if entry.starts_with('!') {
                    continue;
                }
                if !usb_query_is_narrow(entry) {
                    return Some(PolicyViolation::BroadUsbGrant(arg.to_owned()));
                }
            }
            return None;
        }
    }

    // Permission-bearing metadata groups are decoded into equivalent direct grants; unknown keys are rejected.
    if let Some(rest) = arg.strip_prefix("--metadata=") {
        return classify_metadata_write(rest, app_id).map(|v| rewrite_offending(v, arg));
    }

    // `--add-policy` writes a non-permission `[Policy ...]` group.
    if arg.starts_with("--add-policy=") {
        return None;
    }

    if let Some(scope) = arg.strip_prefix("--filesystem=") {
        let base = normalise_value(scope);
        let base = strip_trailing_slash(base);

        // Reject every parent component rather than proving the resolved path remains scoped.
        if path_contains_parent_traversal(base) {
            return Some(PolicyViolation::HostFilesystem(arg.to_owned()));
        }

        if HOST_FILESYSTEM_SCOPES.contains(&base) {
            return Some(PolicyViolation::HostFilesystem(arg.to_owned()));
        }

        if base.starts_with('/') {
            return Some(PolicyViolation::HostFilesystem(arg.to_owned()));
        }

        if base.starts_with("~/") {
            return Some(PolicyViolation::HostFilesystem(arg.to_owned()));
        }

        // Accept an XDG scope only at its exact name or slash boundary.
        for xdg_prefix in SAFE_FILESYSTEM_PREFIXES {
            if let Some(dir) = xdg_prefix.strip_suffix('/') {
                if base.starts_with(&format!("{dir}/")) {
                    return None;
                }
            } else if base == *xdg_prefix || base.starts_with(&format!("{xdg_prefix}/")) {
                return None;
            }
        }

        return Some(PolicyViolation::UnrecognizedGrant(arg.to_owned()));
    }

    if let Some(value) = arg.strip_prefix("--device=") {
        let base = normalise_value(value);
        if base == "all" {
            return Some(PolicyViolation::DeviceAll(arg.to_owned()));
        }
        if SAFE_DEVICES.contains(&base) {
            return None;
        }
        return Some(PolicyViolation::UnrecognizedGrant(arg.to_owned()));
    }

    if let Some(value) = arg.strip_prefix("--socket=") {
        let base = normalise_value(value);
        if base == "system-bus" || base == "session-bus" {
            return Some(PolicyViolation::UnrestrictedBus(arg.to_owned()));
        }
        if SAFE_SOCKETS.contains(&base) {
            return None;
        }
        return Some(PolicyViolation::UnrecognizedGrant(arg.to_owned()));
    }

    if let Some(value) = arg.strip_prefix("--share=") {
        let base = normalise_value(value);
        if SAFE_SHARES.contains(&base) {
            return None;
        }
        return Some(PolicyViolation::UnrecognizedGrant(arg.to_owned()));
    }

    if let Some(value) = arg.strip_prefix("--allow=") {
        let base = normalise_value(value);
        if SAFE_ALLOWS.contains(&base) {
            return None;
        }
        return Some(PolicyViolation::UnrecognizedGrant(arg.to_owned()));
    }

    for prefix in [
        "--talk-name=",
        "--own-name=",
        "--system-talk-name=",
        "--system-own-name=",
    ] {
        if let Some(name) = arg.strip_prefix(prefix) {
            if name == PORTAL_DESKTOP_NAME {
                return Some(PolicyViolation::PortalDesktopBusGrant(arg.to_owned()));
            }
            if name.contains('*') {
                return Some(PolicyViolation::WildcardDBus(arg.to_owned()));
            }
            if DANGEROUS_DBUS_NAMESPACES
                .iter()
                .any(|ns| dbus_name_in_namespace(name, ns))
            {
                return Some(PolicyViolation::SandboxEscapeBusGrant(arg.to_owned()));
            }
            if name.is_empty() {
                return Some(PolicyViolation::UnrecognizedGrant(arg.to_owned()));
            }
            if SAFE_DBUS_NAMESPACES
                .iter()
                .any(|ns| dbus_name_in_namespace(name, ns))
            {
                return None;
            }
            if app_id.is_some_and(|id| dbus_name_in_namespace(name, id)) {
                return None;
            }
            return Some(PolicyViolation::UnrecognizedGrant(arg.to_owned()));
        }
    }

    Some(PolicyViolation::UnrecognizedGrant(arg.to_owned()))
}

// Flatpak metadata writes use `GROUP=KEY=VALUE`; permission-bearing groups are reclassified as direct grants.
fn classify_metadata_write(rest: &str, app_id: Option<&str>) -> Option<PolicyViolation> {
    let Some((group, key_value)) = rest.split_once('=') else {
        return Some(PolicyViolation::PermissionViaMetadata(rest.to_owned()));
    };

    let Some(group) = PERMISSION_METADATA_GROUPS.iter().find(|g| **g == group) else {
        // Only the enumerated groups are treated as permission-bearing; `--add-policy` writes a distinct `[Policy ...]` group.
        return None;
    };

    let Some((key, value)) = key_value.split_once('=') else {
        return Some(PolicyViolation::PermissionViaMetadata(rest.to_owned()));
    };

    match *group {
        "Session Bus Policy" | "System Bus Policy" => {
            let system = *group == "System Bus Policy";
            let flag = match (system, value) {
                (false, "own") => "--own-name=",
                (false, _) => "--talk-name=",
                (true, "own") => "--system-own-name=",
                (true, _) => "--system-talk-name=",
            };
            classify(&format!("{flag}{key}"), app_id)
        }
        "Context" => {
            let flag = match key {
                "sockets" => "--socket=",
                "devices" => "--device=",
                "filesystems" => "--filesystem=",
                "shared" => "--share=",
                "features" => "--allow=",
                _ => return Some(PolicyViolation::PermissionViaMetadata(rest.to_owned())),
            };
            for element in value.split(';').filter(|e| !e.is_empty()) {
                if let Some(v) = classify(&format!("{flag}{element}"), app_id) {
                    return Some(v);
                }
            }
            None
        }
        "Environment" => classify(&format!("--env={key}={value}"), app_id),
        _ => Some(PolicyViolation::PermissionViaMetadata(rest.to_owned())),
    }
}

#[cfg(test)]
mod tests;
