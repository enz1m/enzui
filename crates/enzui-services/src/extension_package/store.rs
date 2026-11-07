//! The installed-extension store: one private folder per extension, an
//! integrity record of every file, durable enablement and settings, atomic
//! install and update, and tombstones for bundled extensions.

use super::manifest::{Contributions, Host, Incompatible, MANIFEST_NAME, Manifest, WorkerSpec};
use super::package::PackageLimits;
use super::{StoreError, hash, read_regular};
use enzui_security::secure_dir::RetainedDir;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

const STATE_LIMIT: u64 = 64 * 1024;
const ENTRY_LIMIT: usize = 64;

/// One extension's saved state.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    /// Whether the user turned it on.
    pub enabled: bool,
    /// A bundled extension the user removed.
    #[serde(default)]
    pub removed: bool,
    /// Saved setting values.
    #[serde(default)]
    pub settings: BTreeMap<String, Value>,
    /// SHA-256 of every installed file, by package path.
    #[serde(default)]
    pub digests: BTreeMap<String, String>,
}

/// One extension as the store found it.
#[derive(Clone, Debug)]
pub struct Installed<C> {
    /// Its identifier, which is also its folder name.
    pub id: String,
    /// Its manifest, unless that could not be read.
    pub manifest: Option<Manifest<C>>,
    /// Its saved state.
    pub state: State,
    /// Compiled into the app rather than installed.
    pub bundled: bool,
    /// Why it cannot run: damaged, changed on disk, or incompatible.
    pub problem: Option<StoreError>,
}

impl<C> Installed<C> {
    /// The manifest's name, or the id when the manifest is unreadable.
    #[must_use]
    pub fn name(&self) -> &str {
        self.manifest.as_ref().map_or(&self.id, |m| &m.name)
    }

    /// Whether it may run: enabled, readable, intact and compatible.
    #[must_use]
    pub fn active(&self) -> bool {
        self.state.enabled && self.problem.is_none() && self.manifest.is_some()
    }

    /// Every declared setting's current value.
    #[must_use]
    pub fn settings(&self) -> BTreeMap<String, Value> {
        self.manifest.as_ref().map_or_else(BTreeMap::new, |m| {
            super::settings::resolve(&m.settings, &self.state.settings)
        })
    }
}

/// Everything installed, bundled extensions first, then by name.
#[derive(Clone, Debug)]
pub struct Catalog<C> {
    /// The extensions found.
    pub entries: Vec<Installed<C>>,
    /// Why the store as a whole could not be read.
    pub error: Option<StoreError>,
}

impl<C> Catalog<C> {
    /// The extensions that may run.
    pub fn active(&self) -> impl Iterator<Item = &Installed<C>> {
        self.entries.iter().filter(|entry| entry.active())
    }

    /// The extension with `id`.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Installed<C>> {
        self.entries.iter().find(|entry| entry.id == id)
    }
}

/// A package read, validated and ready to review and install.
#[derive(Clone, Debug)]
pub struct Prepared<C> {
    /// Its manifest, for the review screen.
    pub manifest: Manifest<C>,
    files: BTreeMap<String, Vec<u8>>,
}

impl<C: Contributions> Prepared<C> {
    /// Reads and validates the package at `path`.
    ///
    /// # Errors
    ///
    /// The package or its manifest is invalid, or its native worker is
    /// missing.
    pub fn open(path: &Path, host: &Host, limits: &PackageLimits) -> Result<Self, StoreError> {
        let files = super::package::read(path, host, limits).map_err(StoreError::Invalid)?;
        let manifest = Manifest::parse(
            files
                .get(MANIFEST_NAME)
                .ok_or_else(|| StoreError::Invalid("The package has no manifest.json".into()))?,
            host,
        )
        .map_err(StoreError::Invalid)?;
        if let WorkerSpec::Native { program, .. } = &manifest.worker
            && !files.contains_key(program)
        {
            return Err(StoreError::Invalid(
                "The extension worker is missing".into(),
            ));
        }
        Ok(Self { manifest, files })
    }

    /// Package paths and sizes, for the review screen.
    pub fn files(&self) -> impl Iterator<Item = (&str, usize)> {
        self.files
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.len()))
    }

    /// Whether this host can run it.
    ///
    /// # Errors
    ///
    /// See [`Manifest::compatibility`].
    pub fn compatibility(&self, host: &Host) -> Result<(), Incompatible> {
        self.manifest.compatibility(host)
    }
}

/// Reads the store at `root` without locking it. Bundled extensions are
/// listed unless their tombstone says the user removed them.
#[allow(
    clippy::missing_panics_doc,
    reason = "panics only on a broken invariant: each entry's manifest is set before it is read"
)]
#[must_use]
pub fn load<C: Contributions>(root: &Path, host: &Host, bundled: &[Manifest<C>]) -> Catalog<C> {
    let mut catalog = Catalog {
        entries: Vec::new(),
        error: None,
    };
    for manifest in bundled {
        let dir = root.join(&manifest.id);
        let mut entry = Installed {
            id: manifest.id.clone(),
            manifest: Some(manifest.clone()),
            state: State {
                enabled: true,
                ..State::default()
            },
            bundled: true,
            problem: None,
        };
        if dir.exists() {
            match private(&dir).and_then(|()| read_state(&dir)) {
                Ok(state) => entry.state = state,
                Err(error) => {
                    entry.state.enabled = false;
                    entry.problem = Some(error);
                }
            }
        }
        if !entry.state.removed {
            catalog.entries.push(entry);
        }
    }
    if !root.exists() {
        return catalog;
    }
    if let Err(error) = private(root) {
        catalog.error = Some(error);
        return catalog;
    }
    let items = match fs::read_dir(root) {
        Ok(items) => items,
        Err(error) => {
            catalog.error = Some(error.into());
            return catalog;
        }
    };
    let first_installed = catalog.entries.len();
    for item in items.take(512) {
        let Ok(item) = item else {
            catalog.error = Some(StoreError::Invalid(
                "Could not read the extension store".into(),
            ));
            break;
        };
        let Some(id) = item.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !super::identifier(&id) || bundled.iter().any(|m| m.id == id) {
            continue;
        }
        if catalog.entries.len() - first_installed >= ENTRY_LIMIT {
            catalog.error = Some(StoreError::Limit);
            break;
        }
        let dir = item.path();
        let mut entry = Installed {
            id: id.clone(),
            manifest: None,
            state: State::default(),
            bundled: false,
            problem: None,
        };
        let checked = (|| {
            private(&dir)?;
            let manifest: Manifest<C> = Manifest::parse(
                &read_regular(&dir.join(MANIFEST_NAME), STATE_LIMIT)
                    .map_err(StoreError::Damaged)?,
                host,
            )
            .map_err(StoreError::Damaged)?;
            if manifest.id != id {
                return Err(StoreError::Damaged(
                    "The extension's ID does not match its folder".into(),
                ));
            }
            entry.manifest = Some(manifest);
            entry.state = read_state(&dir)?;
            verify(&dir, &entry.state)?;
            let manifest = entry.manifest.as_ref().expect("set above");
            super::settings::check_values(&manifest.settings, &entry.state.settings)
                .map_err(StoreError::Damaged)?;
            manifest
                .compatibility(host)
                .map_err(StoreError::Incompatible)
        })();
        entry.problem = checked.err();
        catalog.entries.push(entry);
    }
    catalog.entries[first_installed..].sort_by(|a, b| a.name().cmp(b.name()));
    catalog
}

/// Checks every installed file against the integrity record.
///
/// # Errors
///
/// A file or folder is missing, not private, a link, or changed.
pub fn verify(dir: &Path, state: &State) -> Result<(), StoreError> {
    if state.digests.is_empty() || state.digests.len() > 256 {
        return Err(StoreError::Damaged(
            "The extension's integrity record is missing".into(),
        ));
    }
    private(dir)?;
    for (name, digest) in &state.digests {
        if !super::plain_relative(Path::new(name)) {
            return Err(StoreError::Damaged(
                "The extension's integrity record is invalid".into(),
            ));
        }
        let mut parent = dir.to_path_buf();
        for component in Path::new(name)
            .parent()
            .into_iter()
            .flat_map(Path::components)
        {
            parent.push(component);
            private(&parent)?;
        }
        let bytes = read_regular(&dir.join(name), 64 << 20).map_err(StoreError::Damaged)?;
        if hash(&bytes) != *digest {
            return Err(StoreError::Damaged(
                "The extension's files have changed; remove and reinstall it".into(),
            ));
        }
    }
    Ok(())
}

fn read_state(dir: &Path) -> Result<State, StoreError> {
    let bytes =
        read_regular(&dir.join(super::STATE_NAME), STATE_LIMIT).map_err(StoreError::Damaged)?;
    serde_json::from_slice(&bytes)
        .map_err(|_| StoreError::Damaged("The extension's saved state is invalid".into()))
}

/// A folder only this user can enter, and not a link.
fn private(path: &Path) -> Result<(), StoreError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(StoreError::Damaged(
            "The extension folder is not private".into(),
        ));
    }
    Ok(())
}

fn ensure_private(path: &Path) -> Result<(), StoreError> {
    enzui_security::secure_dir::ensure_private_dir(path)
        .map(|_| ())
        .map_err(|error| StoreError::Invalid(error.to_string()))
}

fn publish_state(path: &Path, state: &State) -> Result<(), StoreError> {
    let bytes =
        serde_json::to_vec(state).map_err(|error| StoreError::Invalid(error.to_string()))?;
    let mut authority =
        enzui_core::state_authority::StateAuthority::open_or_create_tightening(path)?;
    authority
        .publish(&bytes)
        .map(|_| ())
        .map_err(|error| StoreError::Io(std::sync::Arc::new(std::io::Error::other(error))))
}

/// The store, locked for changes by this process.
pub struct Store<'a, C> {
    root: PathBuf,
    host: &'a Host,
    bundled: &'a [Manifest<C>],
    _lock: File,
}

impl<C> std::fmt::Debug for Store<'_, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").field("root", &self.root).finish()
    }
}

impl<'a, C: Contributions> Store<'a, C> {
    /// Opens the store at `root`, creating it privately, and takes its
    /// lock.
    ///
    /// # Errors
    ///
    /// [`StoreError::Busy`] when another process holds the lock.
    pub fn open(
        root: impl Into<PathBuf>,
        host: &'a Host,
        bundled: &'a [Manifest<C>],
    ) -> Result<Self, StoreError> {
        let root = root.into();
        ensure_private(&root)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join(".lock"))?;
        lock.try_lock().map_err(|_| StoreError::Busy)?;
        Ok(Self {
            root,
            host,
            bundled,
            _lock: lock,
        })
    }

    /// The catalog as it is now.
    #[must_use]
    pub fn catalog(&self) -> Catalog<C> {
        load(&self.root, self.host, self.bundled)
    }

    fn entry(&self, id: &str) -> Result<Installed<C>, StoreError> {
        if !super::identifier(id) {
            return Err(StoreError::NotInstalled);
        }
        self.catalog()
            .entries
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or(StoreError::NotInstalled)
    }

    fn save(&self, id: &str, state: &State) -> Result<(), StoreError> {
        let dir = self.root.join(id);
        ensure_private(&dir)?;
        publish_state(&dir.join(super::STATE_NAME), state)
    }

    /// Turns an extension on or off. A damaged or incompatible one cannot
    /// be turned on.
    ///
    /// # Errors
    ///
    /// It is not installed, cannot run, or the state could not be saved.
    pub fn enable(&self, id: &str, enabled: bool) -> Result<(), StoreError> {
        let mut entry = self.entry(id)?;
        if enabled && let Some(problem) = entry.problem {
            return Err(problem);
        }
        entry.state.enabled = enabled;
        self.save(id, &entry.state)
    }

    /// Saves a complete set of setting values, all or none.
    ///
    /// # Errors
    ///
    /// A value does not fit its setting, or the state could not be saved.
    pub fn set_settings(
        &self,
        id: &str,
        values: BTreeMap<String, Value>,
    ) -> Result<(), StoreError> {
        let mut entry = self.entry(id)?;
        let manifest = entry.manifest.as_ref().ok_or(StoreError::NotInstalled)?;
        super::settings::check_values(&manifest.settings, &values).map_err(StoreError::Invalid)?;
        entry.state.settings = values;
        self.save(id, &entry.state)
    }

    /// Removes an installed extension, or tombstones a bundled one.
    ///
    /// # Errors
    ///
    /// It is not installed, or the folder could not be removed.
    pub fn remove(&self, id: &str) -> Result<(), StoreError> {
        let mut entry = self.entry(id)?;
        if entry.bundled {
            entry.state = State {
                removed: true,
                ..State::default()
            };
            return self.save(id, &entry.state);
        }
        let dir = self.root.join(id);
        if fs::symlink_metadata(&dir)?.file_type().is_symlink() {
            fs::remove_file(dir)?;
        } else {
            fs::remove_dir_all(dir)?;
        }
        Ok(())
    }

    /// Brings back a removed bundled extension, enabled and with defaults.
    ///
    /// # Errors
    ///
    /// `id` is not bundled, or the state could not be saved.
    pub fn restore(&self, id: &str) -> Result<(), StoreError> {
        if !self.bundled.iter().any(|manifest| manifest.id == id) {
            return Err(StoreError::NotInstalled);
        }
        self.save(
            id,
            &State {
                enabled: true,
                ..State::default()
            },
        )
    }

    /// Installs `package`, or updates the installed copy when the package
    /// is newer, keeping the settings that still fit. The new files are
    /// written and synced in private staging, then swapped in atomically:
    /// a crash leaves either the old extension or the new one, never a mix.
    /// A fresh install starts disabled until the user enables it.
    ///
    /// # Errors
    ///
    /// The package is for a reserved identifier, not newer than the
    /// installed one, beyond the store's limit, or could not be written.
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: a prepared manifest was validated, so its \
                  version parses"
    )]
    pub fn install(&self, package: &Prepared<C>) -> Result<(), StoreError> {
        let manifest = &package.manifest;
        manifest.validate(self.host).map_err(StoreError::Invalid)?;
        let id = &manifest.id;
        if self
            .host
            .reserved_prefixes
            .iter()
            .any(|prefix| id.starts_with(prefix))
            || self.bundled.iter().any(|bundled| bundled.id == *id)
        {
            return Err(StoreError::Invalid(
                "Bundled extensions are updated with the app".into(),
            ));
        }
        let target = self.root.join(id);
        let mut state = State::default();
        if target.exists() {
            let old = self.entry(id)?;
            let installed = old
                .manifest
                .as_ref()
                .and_then(Manifest::semver)
                .ok_or_else(|| {
                    StoreError::Invalid("Remove the damaged extension before reinstalling".into())
                })?;
            let offered = manifest.semver().expect("validated");
            if !offered.cmp_precedence(&installed).is_gt() {
                return Err(StoreError::NotNewer {
                    installed: installed.to_string(),
                    offered: offered.to_string(),
                });
            }
            state.enabled = old.state.enabled;
            state.settings = old
                .state
                .settings
                .into_iter()
                .filter(|(key, value)| {
                    manifest
                        .settings
                        .iter()
                        .any(|spec| spec.id == *key && spec.accepts(value))
                })
                .collect();
        } else if self.catalog().entries.iter().filter(|e| !e.bundled).count() >= ENTRY_LIMIT {
            return Err(StoreError::Limit);
        }

        let mut stage = Stage::create(&self.root)?;
        for (name, bytes) in &package.files {
            let path = stage.path.join(name);
            if let Some(parent) = path.parent() {
                ensure_private(parent)?;
            }
            let program =
                matches!(&manifest.worker, WorkerSpec::Native { program, .. } if program == name);
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(if program { 0o700 } else { 0o600 })
                .custom_flags(libc::O_NOFOLLOW)
                .open(&path)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            state.digests.insert(name.clone(), hash(bytes));
        }
        publish_state(&stage.path.join(super::STATE_NAME), &state)?;
        sync_tree(&stage.path)?;
        stage.publish(OsStr::new(id))?;
        File::open(&self.root)?.sync_all()?;
        Ok(())
    }
}

fn sync_tree(dir: &Path) -> Result<(), StoreError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_tree(&entry.path())?;
        }
    }
    File::open(dir)?.sync_all()?;
    Ok(())
}

/// Private staging in the store, removed when dropped.
///
/// The store root and the stage are held by descriptor. Publication renames
/// the stage only while its name still denotes it, and the guard then removes
/// only what it owns at the stage name: the stage itself before publication,
/// the replaced extension after an exchange, nothing after a fresh install.
struct Stage {
    path: PathBuf,
    parent: RetainedDir,
    name: std::ffi::OsString,
    /// What the guard removes from the stage name when dropped.
    owned: Option<(u64, u64)>,
}

impl Stage {
    fn create(root: &Path) -> Result<Self, StoreError> {
        let parent = RetainedDir::open(root)?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let name = std::ffi::OsString::from(format!(".install-{}-{nanos}", std::process::id()));
        let stage = parent.create_subroot(&name)?;
        Ok(Self {
            path: root.join(&name),
            owned: Some(stage.identity()),
            parent,
            name,
        })
    }

    /// Renames the stage to `id`, exchanging it with an installed version.
    fn publish(&mut self, id: &OsStr) -> Result<(), StoreError> {
        let stage = self.parent.retain_entry(&self.name)?;
        if Some(stage.identity()) != self.owned {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the install stage was replaced",
            )
            .into());
        }
        let replaced = match self.parent.entry_identity(id) {
            Ok(identity) => Some(identity),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let flags = if replaced.is_some() {
            rustix::fs::RenameFlags::EXCHANGE
        } else {
            rustix::fs::RenameFlags::NOREPLACE
        };
        stage.verify()?;
        rustix::fs::renameat_with(&self.parent, &self.name, &self.parent, id, flags)
            .map_err(|errno| StoreError::from(std::io::Error::from(errno)))?;
        // After an exchange the replaced extension sits at the stage name.
        self.owned = replaced;
        Ok(())
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        if let Some(identity) = self.owned {
            let _ = self.parent.remove_tree_if_identity(&self.name, identity);
        }
    }
}

#[cfg(test)]
mod stage_tests {
    use super::*;
    use enzui_test_support::test_scratch::ScratchDir;

    /// Moves whatever sits at `stage`'s name away and plants a foreign
    /// folder, holding `keep`, there instead.
    fn plant(stage: &Stage) -> PathBuf {
        fs::rename(&stage.path, stage.path.with_extension("moved")).unwrap();
        fs::create_dir(&stage.path).unwrap();
        fs::write(stage.path.join("keep"), b"foreign").unwrap();
        stage.path.join("keep")
    }

    #[test]
    fn an_exchange_hands_the_replaced_version_to_the_guard_and_nothing_else() {
        let root = ScratchDir::new("extension-stage");
        fs::create_dir(root.path().join("org.example.x")).unwrap();
        fs::write(root.path().join("org.example.x").join("old"), b"1").unwrap();

        let mut stage = Stage::create(root.path()).unwrap();
        fs::write(stage.path.join("new"), b"2").unwrap();
        stage.publish(OsStr::new("org.example.x")).unwrap();
        let path = stage.path.clone();
        drop(stage);
        assert!(
            root.path().join("org.example.x").join("new").exists(),
            "the new version is installed"
        );
        assert!(!path.exists(), "the replaced version is removed");

        let mut stage = Stage::create(root.path()).unwrap();
        stage.publish(OsStr::new("org.example.x")).unwrap();
        let keep = plant(&stage);
        drop(stage);
        assert_eq!(
            fs::read(keep).unwrap(),
            b"foreign",
            "a folder planted where the replaced version sat survives"
        );
    }

    #[test]
    fn publication_refuses_a_folder_planted_at_the_stage_name() {
        let root = ScratchDir::new("extension-stage");
        let mut stage = Stage::create(root.path()).unwrap();
        let _ = plant(&stage);
        assert!(stage.publish(OsStr::new("org.example.y")).is_err());
        assert!(!root.path().join("org.example.y").exists());
    }
}
