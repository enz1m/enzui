//! The versioned manifest envelope and host negotiation.

use super::settings::SettingSpec;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ops::RangeInclusive;
use std::path::{Component, Path};

/// The manifest's name inside a package.
pub const MANIFEST_NAME: &str = "manifest.json";
/// The host's per-extension state file; no package may ship one.
pub const STATE_NAME: &str = "state.json";
/// The largest manifest accepted, in bytes.
pub const MANIFEST_LIMIT: usize = 64 * 1024;

/// One extension's manifest: a shared envelope (identity, API, worker,
/// permissions, settings) around the app's own contribution schema `C`,
/// which sits under `contributes` so both stay `deny_unknown_fields`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest<C> {
    /// An editor schema hint; never fetched.
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// The extension API major version the package targets.
    pub api: u32,
    /// Reverse-DNS style identifier: lowercase ASCII, digits, `.`, `_`, `-`.
    pub id: String,
    /// A semantic version; updates must be newer.
    pub version: String,
    /// The name shown to the user.
    pub name: String,
    /// Who made it.
    pub author: String,
    /// What it does, shown on review.
    pub description: String,
    /// What runs when a contribution is invoked.
    pub worker: WorkerSpec,
    /// Host-mediated effects the extension may request, by name.
    #[serde(default)]
    pub permissions: Vec<String>,
    /// User settings, passed to every request.
    #[serde(default)]
    pub settings: Vec<SettingSpec>,
    /// The app's contributions (actions, panels, ...).
    pub contributes: C,
}

/// What runs an extension's requests.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkerSpec {
    /// A program inside the package, built for one `os-arch` target.
    Native {
        /// Its path inside the package.
        program: String,
        /// `linux-x86_64`, `linux-aarch64`, ...
        target: String,
    },
    /// A worker compiled into the host app, named by the host's builtins.
    Builtin {
        /// The builtin's name.
        id: String,
    },
}

/// The app's contribution schema, validated after the envelope.
pub trait Contributions: Serialize + DeserializeOwned + Clone {
    /// Rejects contributions the app does not accept; the message is shown
    /// to the user.
    ///
    /// # Errors
    ///
    /// A contribution is malformed, duplicated or beyond the app's limits.
    fn validate(&self, worker: &WorkerSpec) -> Result<(), String>;
}

/// What the host app offers extensions.
#[derive(Clone, Debug)]
pub struct Host {
    /// The extension API versions this host runs.
    pub api: RangeInclusive<u32>,
    /// Every permission name the host can grant.
    pub permissions: &'static [&'static str],
    /// Every builtin worker the host compiles in.
    pub builtins: &'static [&'static str],
    /// Identifier prefixes reserved for the app's bundled extensions.
    pub reserved_prefixes: &'static [&'static str],
    /// Package file names, besides the manifest and state, the host reserves.
    pub reserved_names: &'static [&'static str],
}

/// Why an otherwise valid extension cannot run on this host.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Incompatible {
    /// It targets an API version this host does not run.
    Api {
        /// What the manifest asks for.
        required: u32,
        /// What the host runs.
        supported: RangeInclusive<u32>,
    },
    /// Its native worker is built for another platform.
    Platform {
        /// The manifest's target.
        built_for: String,
        /// This host's target.
        host: String,
    },
}

impl std::fmt::Display for Incompatible {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api {
                required,
                supported,
            } if supported.start() == supported.end() => write!(
                f,
                "Requires extension API {required}; this app supports API {}",
                supported.start()
            ),
            Self::Api {
                required,
                supported,
            } => write!(
                f,
                "Requires extension API {required}; this app supports API {} to {}",
                supported.start(),
                supported.end()
            ),
            Self::Platform { built_for, host } => {
                write!(f, "Built for {built_for}; install a version for {host}")
            }
        }
    }
}

/// This process's `os-arch` target, as manifests name it.
#[must_use]
pub fn host_target() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}

/// Whether `s` is a valid extension, action or setting identifier.
#[must_use]
pub fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'_' || b == b'-'
        })
}

/// Whether `s` is non-blank display text of at most `max` bytes without
/// control characters.
#[must_use]
pub fn display_text(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}

/// Whether `path` is a non-empty relative path of plain components.
#[must_use]
pub fn plain_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

impl<C: Contributions> Manifest<C> {
    /// Parses and validates a manifest against `host`. An incompatible API
    /// or platform still parses; see [`Self::compatibility`].
    ///
    /// # Errors
    ///
    /// Oversized, malformed, or invalid; the message is shown to the user.
    pub fn parse(bytes: &[u8], host: &Host) -> Result<Self, String> {
        if bytes.len() > MANIFEST_LIMIT {
            return Err("The extension manifest is too large".into());
        }
        let manifest: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("Invalid extension manifest: {error}"))?;
        manifest.validate(host)?;
        Ok(manifest)
    }

    /// Validates the envelope, then the app's contributions.
    ///
    /// # Errors
    ///
    /// The first problem found, as a sentence for the user.
    pub fn validate(&self, host: &Host) -> Result<(), String> {
        if self.api == 0
            || self.schema.as_ref().is_some_and(|s| !display_text(s, 512))
            || !identifier(&self.id)
            || self.version.len() > 128
            || semver::Version::parse(&self.version).is_err()
            || !display_text(&self.name, 80)
            || !display_text(&self.author, 120)
            || !display_text(&self.description, 512)
        {
            return Err("Invalid extension identity, description or semantic version".into());
        }
        match &self.worker {
            WorkerSpec::Native { program, target }
                if plain_relative(Path::new(program))
                    && program.len() <= 240
                    && !program.split('/').any(|part| part.starts_with('.'))
                    && !reserved(program, host)
                    && display_text(target, 64) => {}
            WorkerSpec::Builtin { id } if host.builtins.contains(&id.as_str()) => {}
            _ => return Err("Invalid extension worker".into()),
        }
        let permissions: BTreeSet<_> = self.permissions.iter().collect();
        if permissions.len() != self.permissions.len() {
            return Err("A permission is listed twice".into());
        }
        if let Some(unknown) = self
            .permissions
            .iter()
            .find(|permission| !host.permissions.contains(&permission.as_str()))
        {
            return Err(format!("This app cannot grant the {unknown:?} permission"));
        }
        super::settings::validate(&self.settings)?;
        self.contributes.validate(&self.worker)
    }

    /// Whether this host can run it: `Ok` or why not.
    ///
    /// # Errors
    ///
    /// The API version or native target does not match this host.
    pub fn compatibility(&self, host: &Host) -> Result<(), Incompatible> {
        if !host.api.contains(&self.api) {
            return Err(Incompatible::Api {
                required: self.api,
                supported: host.api.clone(),
            });
        }
        if let WorkerSpec::Native { target, .. } = &self.worker
            && *target != host_target()
        {
            return Err(Incompatible::Platform {
                built_for: target.clone(),
                host: host_target(),
            });
        }
        Ok(())
    }

    /// The parsed semantic version.
    #[must_use]
    pub fn semver(&self) -> Option<semver::Version> {
        semver::Version::parse(&self.version).ok()
    }

    /// Whether `permission` was declared.
    #[must_use]
    pub fn grants(&self, permission: &str) -> bool {
        self.permissions.iter().any(|p| p == permission)
    }
}

/// Whether a package path is the manifest's, the state file's or one the
/// host reserves.
pub(super) fn reserved(name: &str, host: &Host) -> bool {
    name == STATE_NAME || host.reserved_names.contains(&name)
}
