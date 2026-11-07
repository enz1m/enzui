//! Repository discovery, HEAD labels, and work-tree counts from Git files.
//!
//! This reader runs no repository commands, hooks, or filters. Status compares
//! the index to raw work-tree content; staged changes against HEAD, submodule
//! changes, and skip-worktree entries are excluded. Timestamp/size shortcuts
//! can miss changes that retain those values, and filter-dependent files can
//! appear modified when their raw bytes differ from the recorded blob.
//!
//! Budgets limit untracked entry visits and admit hashing by observed file size.
//! They do not bound total time: tracked scans, file growth during hashing,
//! metadata reads, ignore matching, and filesystem operations can add work.
//! Cancellation is checked between batches or directories, not during each read.
//! Call from a blocking worker rather than the UI update path.
//!
//! Metadata and hashed regular files are opened with nonblocking flags and
//! final-symlink refusal; global excludes may follow a final symlink. Parent
//! symlinks and repository-supplied paths are not confined to the work tree.
//! Index versions 2 through 4 are read without validating extension blocks or
//! the trailing checksum. A matching objectformat=sha256 config line selects
//! SHA-256 stat estimates; other object-format declarations use the SHA-1 width.
//! Unreadable ignore files and directory entries are skipped, so a zero count
//! does not prove that every path was examined.

mod ignore;
mod index;
mod sha1;
mod status;

#[cfg(test)]
mod tests;

pub use status::{StatusBudget, StatusSummary};

use std::path::{Path, PathBuf};

const SMALL_FILE: u64 = 4096;
const CONFIG_FILE: u64 = 1024 * 1024;
const MAX_DEPTH: usize = 256;

/// What kind of checkout a repository is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryKind {
    /// A work tree with its own `.git` directory.
    WorkTree,
    /// A work tree whose `.git` is a file naming the real Git directory: a
    /// linked worktree (`git worktree add`) or a submodule.
    LinkedWorkTree,
    /// A repository without a work tree.
    Bare,
    /// The directory opened is inside a repository's `.git` directory.
    GitDirectory,
}

/// A discovered repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    /// What kind of checkout this is.
    pub kind: RepositoryKind,
    /// The top of the work tree; `None` for a bare repository.
    pub work_tree: Option<PathBuf>,
    /// This checkout's Git directory (`.git`, or a worktree's own directory).
    pub git_dir: PathBuf,
    /// The directory holding refs and objects shared by all worktrees.
    pub common_dir: PathBuf,
    /// The work tree of a repository enclosing this one, when this one is
    /// nested inside it (a vendored checkout, a submodule).
    pub enclosing: Option<PathBuf>,
}

/// What is checked out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadState {
    /// A symbolic HEAD whose named ref exists as a file or packed-ref entry.
    Branch(String),
    /// A symbolic HEAD whose ref was not found; it may be unborn or unreadable.
    Unborn(String),
    /// A hexadecimal HEAD value of at least 40 digits, abbreviated to seven.
    /// Object existence and exact hash length are not validated.
    Detached(String),
}

/// An operation inferred from marker existence, in priority order: rebase,
/// merge, cherry-pick, revert, then bisect. Marker contents are not validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// MERGE_HEAD exists.
    Merging,
    /// rebase-merge or rebase-apply exists.
    Rebasing,
    /// CHERRY_PICK_HEAD exists.
    CherryPicking,
    /// REVERT_HEAD exists.
    Reverting,
    /// BISECT_LOG exists.
    Bisecting,
}

impl Operation {
    /// "Merging", "Rebasing"…
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Operation::Merging => "Merging",
            Operation::Rebasing => "Rebasing",
            Operation::CherryPicking => "Cherry-picking",
            Operation::Reverting => "Reverting",
            Operation::Bisecting => "Bisecting",
        }
    }
}

/// [`Repository::head`]'s answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    /// What is checked out.
    pub state: HeadState,
    /// What is under way, if anything.
    pub operation: Option<Operation>,
}

impl Head {
    /// "main", "detached at 1a2b3c4", "main (no commits)".
    #[must_use]
    pub fn label(&self) -> String {
        match &self.state {
            HeadState::Branch(name) => name.clone(),
            HeadState::Unborn(name) => format!("{name} (no commits)"),
            HeadState::Detached(id) => format!("detached at {id}"),
        }
    }
}

/// Why a repository could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// A file was missing, unreadable or too large.
    Io(String),
    /// A file is not in a format this reader understands.
    Format(&'static str),
    /// The caller cancelled.
    Cancelled,
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Io(what) => write!(f, "could not read {what}"),
            ReadError::Format(what) => write!(f, "unexpected {what}"),
            ReadError::Cancelled => f.write_str("cancelled"),
        }
    }
}

impl std::error::Error for ReadError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
    Missing,
    Refused,
}

// Check the opened descriptor so a final-path swap to a FIFO cannot block the read.
pub(super) fn read_regular(path: &Path, limit: u64, follow: bool) -> Result<Vec<u8>, Refusal> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let mut flags = libc::O_NONBLOCK | libc::O_NOCTTY | libc::O_CLOEXEC;
    if !follow {
        flags |= libc::O_NOFOLLOW;
    }
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Refusal::Missing);
        }
        Err(_) => return Err(Refusal::Refused),
    };
    let meta = file.metadata().map_err(|_| Refusal::Refused)?;
    if !meta.is_file() || meta.len() > limit {
        return Err(Refusal::Refused);
    }
    let mut bytes = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| Refusal::Refused)?;
    if bytes.len() as u64 > limit {
        return Err(Refusal::Refused);
    }
    Ok(bytes)
}

fn read_small(path: &Path) -> Option<String> {
    String::from_utf8(read_regular(path, SMALL_FILE, false).ok()?).ok()
}

fn read_config(dir: &Path) -> Option<String> {
    String::from_utf8(read_regular(&dir.join("config"), CONFIG_FILE, false).ok()?).ok()
}

fn is_git_dir(dir: &Path) -> bool {
    let has_head = std::fs::symlink_metadata(dir.join("HEAD")).is_ok_and(|meta| meta.is_file());
    if !has_head {
        return false;
    }
    if dir.join("commondir").is_file() {
        return true;
    }
    dir.join("objects").is_dir() && dir.join("refs").is_dir()
}

fn common_dir_of(git_dir: &Path) -> PathBuf {
    match read_small(&git_dir.join("commondir")) {
        Some(text) if !text.trim().is_empty() => {
            let named = PathBuf::from(text.trim());
            if named.is_absolute() {
                named
            } else {
                git_dir.join(named)
            }
        }
        _ => git_dir.to_path_buf(),
    }
}

fn gitdir_from_file(file: &Path) -> Option<PathBuf> {
    let text = read_small(file)?;
    let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
    if target.is_empty() {
        return None;
    }
    let target = PathBuf::from(target);
    let resolved = if target.is_absolute() {
        target
    } else {
        file.parent()?.join(target)
    };
    is_git_dir(&resolved).then_some(resolved)
}

fn looks_bare(dir: &Path) -> bool {
    let named = dir
        .file_name()
        .is_some_and(|name| name.to_string_lossy().ends_with(".git"));
    named
        || read_config(dir).is_some_and(|config| {
            config
                .lines()
                .map(str::trim)
                .any(|line| line.replace(' ', "").eq_ignore_ascii_case("bare=true"))
        })
}

/// Discover a repository while visiting up to 256 lexical ancestors of `start`.
/// Missing or refused metadata can make discovery return None. Paths are not
/// canonicalized, and bare detection uses a name/config heuristic.
#[must_use]
pub fn discover(start: &Path) -> Option<Repository> {
    let mut repository = discover_one(start)?;
    if let Some(work_tree) = &repository.work_tree {
        repository.enclosing = work_tree
            .parent()
            .and_then(discover_one)
            .and_then(|outer| outer.work_tree);
    }
    Some(repository)
}

fn discover_one(start: &Path) -> Option<Repository> {
    for dir in start.ancestors().take(MAX_DEPTH) {
        let dot_git = dir.join(".git");
        if let Ok(meta) = std::fs::symlink_metadata(&dot_git) {
            if meta.is_dir() && is_git_dir(&dot_git) {
                return Some(Repository {
                    kind: RepositoryKind::WorkTree,
                    work_tree: Some(dir.to_path_buf()),
                    common_dir: common_dir_of(&dot_git),
                    git_dir: dot_git,
                    enclosing: None,
                });
            }
            if meta.is_file()
                && let Some(git_dir) = gitdir_from_file(&dot_git)
            {
                return Some(Repository {
                    kind: RepositoryKind::LinkedWorkTree,
                    work_tree: Some(dir.to_path_buf()),
                    common_dir: common_dir_of(&git_dir),
                    git_dir,
                    enclosing: None,
                });
            }
        }
        if is_git_dir(dir) {
            let inside_dot_git = dir.file_name().is_some_and(|name| name == ".git");
            return Some(Repository {
                kind: if inside_dot_git || !looks_bare(dir) {
                    RepositoryKind::GitDirectory
                } else {
                    RepositoryKind::Bare
                },
                work_tree: None,
                common_dir: common_dir_of(dir),
                git_dir: dir.to_path_buf(),
                enclosing: None,
            });
        }
    }
    None
}

impl Repository {
    /// What is checked out, and what is under way.
    ///
    /// # Errors
    ///
    /// Returns an I/O error for missing, refused, oversized, or non-UTF-8 HEAD
    /// data, or a format error for unrecognized non-symbolic content. Symbolic
    /// ref syntax and commit existence are not validated.
    pub fn head(&self) -> Result<Head, ReadError> {
        let text =
            read_small(&self.git_dir.join("HEAD")).ok_or_else(|| ReadError::Io("HEAD".into()))?;
        let text = text.trim();
        let state = if let Some(reference) = text.strip_prefix("ref:") {
            let reference = reference.trim();
            let name = reference
                .strip_prefix("refs/heads/")
                .unwrap_or(reference)
                .to_owned();
            if self.reference_exists(reference) {
                HeadState::Branch(name)
            } else {
                HeadState::Unborn(name)
            }
        } else if text.len() >= 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            HeadState::Detached(text.chars().take(7).collect())
        } else {
            return Err(ReadError::Format("HEAD"));
        };
        Ok(Head {
            state,
            operation: self.operation(),
        })
    }

    fn reference_exists(&self, reference: &str) -> bool {
        if reference
            .split('/')
            .any(|part| part == ".." || part.is_empty())
        {
            return false;
        }
        if self.common_dir.join(reference).is_file() || self.git_dir.join(reference).is_file() {
            return true;
        }
        // Bound packed-ref reads; an unavailable file leaves the symbolic ref unresolved.
        let path = self.common_dir.join("packed-refs");
        read_regular(&path, 32 * 1024 * 1024, false).is_ok_and(|packed| {
            String::from_utf8_lossy(&packed)
                .lines()
                .filter_map(|line| line.split_once(' '))
                .any(|(_, name)| name == reference)
        })
    }

    fn operation(&self) -> Option<Operation> {
        let has = |name: &str| self.git_dir.join(name).exists();
        if has("rebase-merge") || has("rebase-apply") {
            Some(Operation::Rebasing)
        } else if has("MERGE_HEAD") {
            Some(Operation::Merging)
        } else if has("CHERRY_PICK_HEAD") {
            Some(Operation::CherryPicking)
        } else if has("REVERT_HEAD") {
            Some(Operation::Reverting)
        } else if has("BISECT_LOG") {
            Some(Operation::Bisecting)
        } else {
            None
        }
    }

    /// Strip the work-tree prefix and join remaining components with `/`.
    /// This is lexical: parent components remain and non-UTF-8 names are lossy;
    /// the result does not establish filesystem containment.
    #[must_use]
    pub fn relative_path(&self, path: &Path) -> Option<String> {
        let relative = path.strip_prefix(self.work_tree.as_ref()?).ok()?;
        let parts: Vec<String> = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect();
        Some(parts.join("/"))
    }

    /// Compare the index with raw work-tree content using `budget`.
    /// Cancellation is checked every 256 tracked entries and on directory
    /// entry, after config/index loading. Filesystem reads may delay checks.
    ///
    /// # Errors
    ///
    /// Returns errors for refused or malformed config/index data and observed
    /// cancellation. Without a work tree, returns zero counts without checking
    /// cancellation. A missing index still permits untracked-file scanning.
    /// Unreadable directories and ignore files do not produce errors.
    ///
    /// # Panics
    /// Propagates a panic from `cancelled`.
    pub fn status(
        &self,
        budget: StatusBudget,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<StatusSummary, ReadError> {
        status::status(self, budget, cancelled)
    }
}
