//! Why this process was started.
//!
//! A desktop launch runs with a window. A unit start with
//! `--agent-background` runs windowless, and was asked for by the broker, by
//! D-Bus activation for a desktop launch (which then arrives as `Activate`),
//! or by the OS for a Search query. The OS marks a Search start with a record
//! under `$XDG_RUNTIME_DIR/enzui/search-activation/<app_id>.json`, bound to
//! the started unit's invocation ID once the app owns its bus name:
//!
//! ```json
//! {"schema": 1, "origin": "search", "app_id": "org.example.Files",
//!  "unit": "enzui-agent-org.example.Files.service",
//!  "invocation_id": "<32 hex>", "requested_realtime_us": 1760000000000000}
//! ```
//!
//! The record is informational: a Search start gets no agent authority, stays
//! windowless and idle-exits like any background start, and an explicit
//! `Activate` still promotes it to a window. While the OS has not bound the
//! record yet (`invocation_id` is `null`) the origin is
//! [`ActivationOrigin::Unknown`], so read it when a decision needs it, such as
//! on the first search request, rather than first thing in `main`.

use std::fs::Metadata;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use enzui_agent::center::background::LaunchMode;

/// The longest record the reader accepts.
const RECORD_LIMIT: u64 = 4096;

/// Why this process was started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationOrigin {
    /// A desktop launch: the process runs with a window.
    Desktop,
    /// A windowless unit start that no Search record claims: the broker, or
    /// D-Bus activation whose desktop launch arrives as `Activate`.
    Background,
    /// The OS started this unit invocation for a Search query.
    Search,
    /// A Search record exists but does not prove this invocation: still
    /// pending, stale, for another unit or invocation, or not trustworthy.
    Unknown,
}

/// The origin of this process, from its launch mode, `$INVOCATION_ID` and
/// the Search record. Reads the record on every call.
#[must_use]
pub fn activation_origin(app_id: &str, mode: LaunchMode) -> ActivationOrigin {
    if mode != LaunchMode::Background {
        return ActivationOrigin::Desktop;
    }
    let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from) else {
        return ActivationOrigin::Background;
    };
    let invocation = std::env::var("INVOCATION_ID").ok();
    origin_in(&runtime, app_id, invocation.as_deref(), own_uid())
}

/// [`activation_origin`] for a background start, over an explicit runtime
/// directory, invocation ID and user.
fn origin_in(
    runtime: &Path,
    app_id: &str,
    invocation: Option<&str>,
    uid: Option<u32>,
) -> ActivationOrigin {
    let dir = runtime.join("enzui/search-activation");
    let path = dir.join(format!("{app_id}.json"));
    let record = match std::fs::symlink_metadata(&path) {
        Ok(record) => record,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ActivationOrigin::Background;
        }
        Err(_) => return ActivationOrigin::Unknown,
    };
    let Some(uid) = uid else {
        return ActivationOrigin::Unknown;
    };
    // Ours alone: owned by this user and closed to group and others.
    let private = |meta: &Metadata| meta.uid() == uid && meta.permissions().mode() & 0o077 == 0;
    let dir_ok = std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.is_dir() && private(&meta));
    let file_ok =
        record.is_file() && record.nlink() == 1 && record.len() <= RECORD_LIMIT && private(&record);
    if !dir_ok || !file_ok {
        return ActivationOrigin::Unknown;
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
        return ActivationOrigin::Unknown;
    };
    match search_invocation(&text, app_id) {
        Some(bound) if invocation.is_some_and(|own| valid_invocation(own) && own == bound) => {
            ActivationOrigin::Search
        }
        _ => ActivationOrigin::Unknown,
    }
}

/// The invocation a well-formed Search record for `app_id` is bound to.
fn search_invocation(text: &str, app_id: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let unit = format!("enzui-agent-{app_id}.service");
    (value.get("schema")?.as_u64()? == 1
        && value.get("origin")?.as_str()? == "search"
        && value.get("app_id")?.as_str()? == app_id
        && value.get("unit")?.as_str()? == unit)
        .then_some(())?;
    let bound = value.get("invocation_id")?.as_str()?;
    valid_invocation(bound).then(|| bound.to_owned())
}

/// A systemd invocation ID: 32 lowercase hex digits, not all zero.
fn valid_invocation(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && id.bytes().any(|b| b != b'0')
}

/// This process's user: the owner of `/proc/self`.
fn own_uid() -> Option<u32> {
    std::fs::metadata("/proc/self").ok().map(|meta| meta.uid())
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = "org.example.Files";
    const OWN: &str = "0123456789abcdef0123456789abcdef";
    const OTHER: &str = "fedcba9876543210fedcba9876543210";

    struct Runtime(PathBuf);

    impl Runtime {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "enzui-activation-origin-{}-{tag}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("enzui/search-activation")).unwrap();
            std::fs::set_permissions(
                root.join("enzui/search-activation"),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            Self(root)
        }

        fn record(&self, invocation: Option<&str>, mode: u32) {
            let path = self.0.join(format!("enzui/search-activation/{APP}.json"));
            let invocation = invocation.map_or_else(|| "null".to_owned(), |id| format!("\"{id}\""));
            std::fs::write(
                &path,
                format!(
                    "{{\"schema\": 1, \"origin\": \"search\", \"app_id\": \"{APP}\", \
                     \"unit\": \"enzui-agent-{APP}.service\", \"invocation_id\": {invocation}, \
                     \"requested_realtime_us\": 1760000000000000}}"
                ),
            )
            .unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }

        fn origin(&self, invocation: Option<&str>) -> ActivationOrigin {
            origin_in(&self.0, APP, invocation, own_uid())
        }
    }

    impl Drop for Runtime {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_a_record_bound_to_this_invocation_is_a_search_start() {
        let runtime = Runtime::new("bound");
        assert_eq!(
            runtime.origin(Some(OWN)),
            ActivationOrigin::Background,
            "no record"
        );

        runtime.record(Some(OWN), 0o600);
        assert_eq!(runtime.origin(Some(OWN)), ActivationOrigin::Search);
        assert_eq!(
            runtime.origin(Some(OTHER)),
            ActivationOrigin::Unknown,
            "another invocation"
        );
        assert_eq!(
            runtime.origin(None),
            ActivationOrigin::Unknown,
            "no INVOCATION_ID"
        );

        runtime.record(None, 0o600);
        assert_eq!(
            runtime.origin(Some(OWN)),
            ActivationOrigin::Unknown,
            "still pending"
        );

        runtime.record(Some(OWN), 0o644);
        assert_eq!(
            runtime.origin(Some(OWN)),
            ActivationOrigin::Unknown,
            "readable by others"
        );

        assert_eq!(
            origin_in(&runtime.0, APP, Some(OWN), own_uid().map(|uid| uid + 1)),
            ActivationOrigin::Unknown,
            "owned by another user"
        );
    }

    #[test]
    fn a_desktop_launch_is_a_desktop_start_whatever_the_record_says() {
        assert_eq!(
            activation_origin(APP, LaunchMode::Gui),
            ActivationOrigin::Desktop
        );
        assert_eq!(
            search_invocation(
                &format!(
                    "{{\"schema\": 1, \"origin\": \"agent\", \"app_id\": \"{APP}\", \
                     \"unit\": \"enzui-agent-{APP}.service\", \"invocation_id\": \"{OWN}\"}}"
                ),
                APP
            ),
            None,
            "a record for another origin proves nothing"
        );
    }
}
