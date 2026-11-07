//! The work tree compared with the index, within a budget.

use super::ignore::{RuleSet, Stack};
use super::index::{self, Entry};
use super::sha1::Sha1;
use super::{ReadError, Repository};
use std::collections::HashSet;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// Entry-visit and stat-size limits used by a status scan. These do not
/// impose a wall-clock limit or stop reads if a file grows while being hashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusBudget {
    /// Directory entries admitted while looking for untracked files; defaults
    /// to 50,000. One extra entry is visited to detect truncation.
    pub max_untracked_visits: usize,
    /// Total observed file sizes admitted for hashing; defaults to 64 MiB.
    /// Actual bytes read can exceed this if files grow during a read.
    pub max_hash_bytes: u64,
    /// Largest observed size admitted for a single hash; defaults to 16 MiB.
    /// A larger same-size file with changed stat data counts as approximate modification.
    pub max_file_hash: u64,
}

impl Default for StatusBudget {
    fn default() -> Self {
        Self {
            max_untracked_visits: 50_000,
            max_hash_bytes: 64 * 1024 * 1024,
            max_file_hash: 16 * 1024 * 1024,
        }
    }
}

/// What changed in the work tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatusSummary {
    /// Tracked files with detected or conservatively assumed content/mode changes.
    pub modified: usize,
    /// Tracked paths whose metadata could not be read, including missing paths.
    pub deleted: usize,
    /// Paths with unresolved merge conflicts.
    pub conflicted: usize,
    /// Untracked, unignored entries, a wholly untracked directory counting
    /// once, as `git status` lists it.
    pub untracked: usize,
    /// The untracked walk stopped at its budget: there may be more.
    pub untracked_truncated: bool,
    /// A same-size stat change could not be settled by hashing, including
    /// hash limits, SHA-256 IDs, or read failures. Does not report skipped
    /// directories, ignored-file read failures, or timestamp-shortcut misses.
    pub approximate: bool,
}

impl StatusSummary {
    /// All reported counts are zero; truncation and approximation flags are
    /// not consulted. Skipped or unexamined paths can still have changes.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.modified == 0 && self.deleted == 0 && self.conflicted == 0 && self.untracked == 0
    }

    /// Tracked changes (modified + deleted + conflicted).
    #[must_use]
    pub fn changed(&self) -> usize {
        self.modified + self.deleted + self.conflicted
    }
}

// A present but unreadable config must not silently select the SHA-1 index width.
fn id_len(repository: &Repository) -> Result<usize, ReadError> {
    let config = match super::read_regular(
        &repository.common_dir.join("config"),
        super::CONFIG_FILE,
        false,
    ) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| ReadError::Format("config"))?,
        Err(super::Refusal::Missing) => return Ok(20),
        Err(super::Refusal::Refused) => return Err(ReadError::Io("the config".into())),
    };
    let sha256 = config.lines().map(str::trim).any(|line| {
        line.replace(' ', "")
            .eq_ignore_ascii_case("objectformat=sha256")
    });
    Ok(if sha256 { 32 } else { 20 })
}

pub(super) fn status(
    repository: &Repository,
    budget: StatusBudget,
    cancelled: &dyn Fn() -> bool,
) -> Result<StatusSummary, ReadError> {
    let Some(work_tree) = &repository.work_tree else {
        return Ok(StatusSummary::default());
    };
    let id_len = id_len(repository)?;
    let entries = index::read(&repository.git_dir.join("index"), id_len)?.unwrap_or_default();
    let mut summary = StatusSummary::default();
    let mut hashed: u64 = 0;

    let conflicts: HashSet<&[u8]> = entries
        .iter()
        .filter(|entry| entry.stage != 0)
        .map(|entry| entry.path.as_slice())
        .collect();
    summary.conflicted = conflicts.len();

    for (n, entry) in entries.iter().enumerate() {
        if n % 256 == 0 && cancelled() {
            return Err(ReadError::Cancelled);
        }
        if entry.stage != 0 || entry.skip_worktree || entry.is_gitlink() {
            continue;
        }
        match compare(work_tree, entry, id_len, &budget, &mut hashed) {
            Comparison::Same => {}
            Comparison::Missing => summary.deleted += 1,
            Comparison::Differs => summary.modified += 1,
            Comparison::Unread => {
                summary.modified += 1;
                summary.approximate = true;
            }
        }
    }

    let tracked: HashSet<&[u8]> = entries.iter().map(|entry| entry.path.as_slice()).collect();
    let tracked_dirs: HashSet<&[u8]> = entries
        .iter()
        .flat_map(|entry| {
            entry
                .path
                .iter()
                .enumerate()
                .filter(|(_, byte)| **byte == b'/')
                .map(|(at, _)| &entry.path[..at])
        })
        .collect();
    let base = base_rules(repository);
    let mut walk = Walk {
        tracked: &tracked,
        tracked_dirs: &tracked_dirs,
        visits: 0,
        budget: budget.max_untracked_visits,
        cancelled,
    };
    let root_rules = Stack::new(base).with(RuleSet::from_file(b"", &work_tree.join(".gitignore")));
    let (untracked, truncated) = walk.directory(b"", work_tree, &root_rules, false)?;
    summary.untracked = untracked;
    summary.untracked_truncated = truncated;
    Ok(summary)
}

fn base_rules(repository: &Repository) -> Vec<RuleSet> {
    let mut sets = Vec::new();
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    if let Some(global) =
        config_home.and_then(|dir| RuleSet::from_user_file(b"", &dir.join("git/ignore")))
    {
        sets.push(global);
    }
    if let Some(exclude) = RuleSet::from_file(b"", &repository.common_dir.join("info/exclude")) {
        sets.push(exclude);
    }
    sets
}

enum Comparison {
    Same,
    Missing,
    Differs,
    Unread,
}

fn path_of(work_tree: &Path, relative: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    work_tree.join(std::ffi::OsStr::from_bytes(relative))
}

fn compare(
    work_tree: &Path,
    entry: &Entry,
    id_len: usize,
    budget: &StatusBudget,
    hashed: &mut u64,
) -> Comparison {
    let path = path_of(work_tree, &entry.path);
    let Ok(meta) = std::fs::symlink_metadata(&path) else {
        return Comparison::Missing;
    };
    if entry.intent_to_add {
        return Comparison::Differs;
    }
    if entry.is_symlink() != meta.file_type().is_symlink() || meta.is_dir() {
        return Comparison::Differs;
    }
    if !entry.is_symlink() && entry.is_executable() != (meta.permissions().mode() & 0o111 != 0) {
        return Comparison::Differs;
    }
    // The index stores the low 32 bits of the size.
    if (meta.len() & 0xffff_ffff) as u32 != entry.size {
        return Comparison::Differs;
    }
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "git's index stores the low 32 bits of each"
    )]
    let mtime = (meta.mtime() as u32, meta.mtime_nsec() as u32);
    if mtime == entry.mtime {
        return Comparison::Same;
    }
    if id_len != 20 || meta.len() > budget.max_file_hash {
        return Comparison::Unread;
    }
    if hashed.saturating_add(meta.len()) > budget.max_hash_bytes {
        return Comparison::Unread;
    }
    *hashed += meta.len();
    match blob_id(&path, &meta, entry.is_symlink()) {
        Some(id) if id.as_slice() == entry.id.as_slice() => Comparison::Same,
        Some(_) => Comparison::Differs,
        None => Comparison::Unread,
    }
}

// Match the opened file's identity to the stat used for hash admission; reject final-path swaps.
fn blob_id(path: &Path, meta: &std::fs::Metadata, symlink: bool) -> Option<[u8; 20]> {
    use std::os::unix::fs::OpenOptionsExt;
    let len = meta.len();
    let mut hasher = Sha1::new();
    if symlink {
        use std::os::unix::ffi::OsStrExt;
        let target = std::fs::read_link(path).ok()?;
        let bytes = target.as_os_str().as_bytes();
        hasher.update(format!("blob {}\0", bytes.len()).as_bytes());
        hasher.update(bytes);
        return Some(hasher.finish());
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_NOCTTY | libc::O_CLOEXEC)
        .open(path)
        .ok()?;
    let opened = file.metadata().ok()?;
    if !opened.is_file() || opened.dev() != meta.dev() || opened.ino() != meta.ino() {
        return None;
    }
    hasher.update(format!("blob {len}\0").as_bytes());
    let mut buffer = vec![0; 64 * 1024];
    let mut read = 0u64;
    loop {
        let n = file.read(&mut buffer).ok()?;
        if n == 0 {
            break;
        }
        read += n as u64;
        hasher.update(&buffer[..n]);
    }
    // A changed length makes the comparison unavailable and conservatively modified.
    (read == len).then(|| hasher.finish())
}

struct Walk<'a> {
    tracked: &'a HashSet<&'a [u8]>,
    tracked_dirs: &'a HashSet<&'a [u8]>,
    visits: usize,
    budget: usize,
    cancelled: &'a dyn Fn() -> bool,
}

impl Walk<'_> {
    fn directory(
        &mut self,
        relative: &[u8],
        dir: &Path,
        rules: &Stack,
        first_only: bool,
    ) -> Result<(usize, bool), ReadError> {
        use std::os::unix::ffi::OsStrExt;
        if (self.cancelled)() {
            return Err(ReadError::Cancelled);
        }
        let Ok(read) = std::fs::read_dir(dir) else {
            return Ok((0, false));
        };
        let mut untracked = 0;
        for entry in read.flatten() {
            self.visits += 1;
            if self.visits > self.budget {
                return Ok((untracked, true));
            }
            let name = entry.file_name();
            let name = name.as_bytes();
            if name == b".git" {
                continue;
            }
            let mut path = relative.to_vec();
            path.extend_from_slice(name);
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let is_dir = kind.is_dir();
            if self.tracked.contains(path.as_slice()) {
                continue;
            }
            if rules.is_ignored(&path, is_dir) {
                continue;
            }
            if !is_dir {
                untracked += 1;
                if first_only {
                    return Ok((untracked, false));
                }
                continue;
            }
            let full = entry.path();
            // A nested repository is one untracked entry, never walked.
            if full.join(".git").exists() {
                untracked += 1;
                if first_only {
                    return Ok((untracked, false));
                }
                continue;
            }
            let mut below = path.clone();
            below.push(b'/');
            let nested = rules.with(RuleSet::from_file(&below, &full.join(".gitignore")));
            let partly_tracked = self.tracked_dirs.contains(path.as_slice());
            // A wholly untracked directory is one entry, if anything in it
            // survives the ignore rules (git lists it once).
            let (count, truncated) =
                self.directory(&below, &full, &nested, first_only || !partly_tracked)?;
            untracked += if partly_tracked {
                count
            } else {
                usize::from(count > 0)
            };
            if truncated {
                return Ok((untracked, true));
            }
            if first_only && untracked > 0 {
                return Ok((untracked, false));
            }
        }
        Ok((untracked, false))
    }
}
