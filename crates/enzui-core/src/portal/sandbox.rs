//! Whether this process is confined to a sandbox, and therefore to portals.
//!
//! # Purpose
//!
//! A portal is the *only* way out of a Flatpak or Snap confinement, and it is
//! the wrong first choice outside one. `org.freedesktop.portal.OpenURI` asks
//! the desktop which application should handle a file, and a native process
//! that has no application id — every binary in this workspace, run from a
//! shell or a `.desktop` file — gets the portal's "open with" dialog instead of
//! the user's default handler. Presented without a parent window, that dialog
//! can never be answered, so the request never completes and nothing opens.
//!
//! So a native install asks the desktop's own launcher and a confined install
//! asks the portal. Both are correct; picking one for both is not.
//! [`crate::portal::open_uri::desktop_launcher::host_open_uri_backend`] makes
//! that choice from [`SandboxEvidence::from_host`].
//!
//! # Hazards
//!
//! The evidence is deliberately a value, not an ambient read: a test states
//! the confinement it wants instead of mutating process environment other
//! tests share. [`SandboxEvidence::from_host`] performs the reads once.

use std::path::Path;

/// The confinement markers observed on the host.
///
/// Each field is one independent marker. Any of them being present means the
/// process cannot reach the desktop's own launcher and must use portals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SandboxEvidence {
    /// `/.flatpak-info` exists — the file Flatpak mounts inside every sandbox,
    /// and the marker the portal itself uses to identify a confined caller.
    pub flatpak_info: bool,
    /// `FLATPAK_ID` is set and non-empty.
    pub flatpak_id: bool,
    /// `SNAP` is set and non-empty: a Snap confinement's mount root.
    pub snap: bool,
    /// `container` is set and non-empty (`flatpak`, `oci`, …), the generic
    /// marker `systemd-detect-virt --container` reads.
    pub container: bool,
}

impl SandboxEvidence {
    /// Reads the markers from this process's filesystem and environment.
    #[must_use]
    pub fn from_host() -> Self {
        Self {
            flatpak_info: Path::new("/.flatpak-info").exists(),
            flatpak_id: has_value("FLATPAK_ID"),
            snap: has_value("SNAP"),
            container: has_value("container"),
        }
    }
}

fn has_value(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

/// Whether `evidence` shows an application sandbox.
///
/// `true` when any single marker is present, because each one alone confines
/// the process; a portal route is then the only one available.
#[must_use]
pub fn is_sandboxed(evidence: &SandboxEvidence) -> bool {
    evidence.flatpak_info || evidence.flatpak_id || evidence.snap || evidence.container
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every marker must decide the answer on its own: an `&&` of the four
    /// would report an unconfined process inside a real Snap, and a route
    /// choice built on that would call a launcher the sandbox cannot run.
    #[test]
    fn each_marker_alone_reports_a_sandbox() {
        assert!(!is_sandboxed(&SandboxEvidence::default()));
        let markers: [fn(&mut SandboxEvidence); 4] = [
            |evidence| evidence.flatpak_info = true,
            |evidence| evidence.flatpak_id = true,
            |evidence| evidence.snap = true,
            |evidence| evidence.container = true,
        ];
        for (index, set) in markers.iter().enumerate() {
            let mut evidence = SandboxEvidence::default();
            set(&mut evidence);
            assert!(
                is_sandboxed(&evidence),
                "marker {index} alone must report a sandbox: {evidence:?}"
            );
        }
    }
}
