//! Fail-closed filesystem primitives for source-tree gates.
//!
//! A source gate must never turn an I/O or classification failure into a
//! smaller, apparently clean population. These helpers panic with the failed
//! operation and exact path instead.

use std::path::{Path, PathBuf};

/// Recursively collect every `.rs` file under `root`, relative to `root`, using
/// `/`-separated paths and deterministic ordering.
///
/// This is deliberately fail closed. A missing or non-directory root, any
/// nested `read_dir` failure, any entry-read or followed-metadata failure, and
/// any root-relative conversion failure panic with the operation and exact
/// path. Root classification deliberately does not follow a symlink: the
/// established helper contract rejects a symlink root as not-a-directory.
/// Metadata for entries discovered below a real root does follow symlinks,
/// preserving the former recursive `Path::is_dir` behavior without swallowing
/// an error while following one.
pub fn walk_rs_files(root: &Path) -> Vec<String> {
    walk_files(root, "rs", "walk_rs_files")
}

/// Recursively collect every file with extension `ext` under `root`, relative
/// to `root`, using `/` separators and deterministic ordering.
///
/// This is the non-Rust twin of [`walk_rs_files`], with the same fail-closed
/// root, directory, entry, metadata, and relative-path guarantees.
pub fn walk_files_with_ext(root: &Path, ext: &str) -> Vec<String> {
    walk_files(root, ext, "walk_files_with_ext")
}

fn walk_files(root: &Path, ext: &str, operation: &str) -> Vec<String> {
    // Preserve the established root contract: unlike discovered children, a
    // symlink root is classified as the symlink itself and rejected below.
    let meta = std::fs::symlink_metadata(root)
        .unwrap_or_else(|e| panic!("{operation}: root missing or unreadable at {root:?}: {e}"));
    if !meta.is_dir() {
        panic!(
            "{operation}: root is not a directory at {:?} (file type: {:?})",
            root,
            meta.file_type()
        );
    }
    let mut out = Vec::new();
    let mut ancestors = vec![directory_identity(root, &meta)];
    walk_inner(root, root, ext, &mut ancestors, &mut out);
    out.sort();
    out
}

/// A directory's `(device, inode)`, which a symlink back to it shares.
fn directory_identity(path: &Path, metadata: &std::fs::Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        let _ = path;
        (metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        panic!("walk_rs_files: directory identity is unix-only, at {path:?}")
    }
}

/// Walk `.rs` files under `root`, canonicalizing a symlink root first.
///
/// The shared walker ([`walk_rs_files`]) deliberately rejects a symlink root
/// (the established walker contract: root classification uses
/// `symlink_metadata`). Three theme-source gates migrated to the shared
/// walker from local `collect_rs`/`walk` implementations that used `read_dir`,
/// which follows symlinks transparently, and whose `Path::is_dir` source-root
/// selection also followed symlinks. This adapter preserves both contracts:
///
/// * The shared walker's symlink-root rejection is not weakened.
/// * Callers whose source roots could be symlinks (a sibling checkout,
///   worktree symlinks) continue to work: the symlink is canonicalized to its real
///   target directory, walked there, and the relative results are unchanged
///   because they are interior to the tree.
///
/// For a non-symlink root this is a zero-cost pass-through to [`walk_rs_files`].
pub fn walk_rs_files_follow_root(root: &Path) -> Vec<String> {
    walk_files_follow_root(root, "rs", "walk_rs_files_follow_root")
}

/// Like [`walk_files_with_ext`] but canonicalizes a symlink root.
pub fn walk_files_with_ext_follow_root(root: &Path, ext: &str) -> Vec<String> {
    walk_files_follow_root(root, ext, "walk_files_with_ext_follow_root")
}

fn walk_files_follow_root(root: &Path, ext: &str, operation: &str) -> Vec<String> {
    let meta = std::fs::symlink_metadata(root)
        .unwrap_or_else(|e| panic!("{operation}: root missing or unreadable at {root:?}: {e}"));
    if meta.is_symlink() {
        let canonical = root.canonicalize().unwrap_or_else(|e| {
            panic!("{operation}: canonicalize failed for symlink root {root:?}: {e}")
        });
        // Validate the symlink target is a directory before walking.
        let target_meta = std::fs::metadata(&canonical).unwrap_or_else(|e| {
            panic!("{operation}: metadata failed for symlink target {canonical:?}: {e}")
        });
        if !target_meta.is_dir() {
            panic!(
                "{operation}: symlink root {:?} does not point to a directory \
                 (target {:?}, file type {:?})",
                root,
                canonical,
                target_meta.file_type()
            );
        }
        // Walk the canonical (real) directory. Relative results are interior
        // paths like "sub/file.rs" — identical regardless of whether the root
        // was reached through a symlink.
        walk_files(&canonical, ext, operation)
    } else {
        // Not a symlink: pass through to the base walker.
        walk_files(root, ext, operation)
    }
}

/// Read a discovered source file, failing closed with operation and path.
///
/// Use this after discovery instead of `.ok()`, `let Ok(..) = .. else`, or a
/// best-effort `continue`: an unreadable source must stop the gate rather than
/// disappear from a prohibition or inventory check.
pub fn read_source_or_panic(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read_source_or_panic: source read failed at {path:?}: {e}"))
}

/// The file names of `entries`, read from `dir`, failing closed: an entry that
/// cannot be read panics with the operation and the directory rather than
/// dropping out of the listing.
pub fn strict_entry_names(
    dir: &Path,
    entries: impl Iterator<Item = std::io::Result<std::fs::DirEntry>>,
) -> Vec<String> {
    entries
        .map(|entry| {
            entry
                .unwrap_or_else(|e| panic!("entry read failed in {}: {e}", dir.display()))
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// The file names directly under `dir`, unsorted, failing closed on opening
/// the directory and on every entry ([`strict_entry_names`]).
pub fn strict_dir_names(dir: &Path) -> Vec<String> {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("open dir {}: {e}", dir.display()));
    strict_entry_names(dir, entries)
}

/// Return immediate entries under `dir`, sorted and failing closed.
///
/// Both opening the directory and reading each `DirEntry` are required. This
/// is the strict replacement for `read_dir(..).filter_map(Result::ok)` used by
/// flat roster and source-root discovery.
pub fn read_dir_sorted(dir: &Path) -> Vec<PathBuf> {
    let rd = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read_dir_sorted: read_dir failed at {dir:?}: {e}"));
    collect_entries_or_panic(dir, rd.map(|r| r.map(|e| e.path())))
}

/// Return the stems of immediate regular `.rs` files in `dir`, sorted and
/// failing closed.
///
/// This is the common collector for split-module boundary ledgers. It is flat
/// (nested files stay outside the population), classifies the directory entry
/// itself without following symlinks (matching `DirEntry::file_type`), and
/// propagates directory-open, entry-read, and metadata/type failures instead of
/// shrinking the on-disk side of the comparison.
pub fn rust_file_stems(dir: &Path) -> Vec<String> {
    let entries = read_dir_sorted(dir);
    collect_rust_file_stems(entries, |path| {
        std::fs::symlink_metadata(path).map(|metadata| metadata.is_file())
    })
    .unwrap_or_else(|error| {
        panic!("rust_file_stems: classify discovered entry under {dir:?}: {error}")
    })
}

/// Collector seam for [`rust_file_stems`]. Keeping classification injectable
/// gives the shared verdict a deterministic metadata/type failure control;
/// production supplies `symlink_metadata` above.
pub(crate) fn collect_rust_file_stems<I, F>(
    entries: I,
    mut is_file: F,
) -> std::io::Result<Vec<String>>
where
    I: IntoIterator<Item = PathBuf>,
    F: FnMut(&Path) -> std::io::Result<bool>,
{
    let mut out = Vec::new();
    for path in entries {
        let regular = is_file(&path).map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!(
                    "rust_file_stems: metadata/type failed at {}: {error}",
                    path.display()
                ),
            )
        })?;
        if regular
            && path.extension().and_then(|ext| ext.to_str()) == Some("rs")
            && let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
        {
            out.push(stem.to_owned());
        }
    }
    out.sort();
    Ok(out)
}

/// Follow symlinks and classify a required path, failing on every metadata
/// error, including `NotFound`.
///
/// Use this for an entry already returned by [`read_dir_sorted`]. Its type must
/// be checked before probing optional child markers: a regular-file sibling is
/// outside a directory-derived population, while failure to classify the
/// discovered entry must red the gate.
pub fn required_metadata(path: &Path) -> std::fs::Metadata {
    std::fs::metadata(path).unwrap_or_else(|e| {
        panic!(
            "required_metadata: metadata failed at {:?} (kind {:?}): {e}",
            path,
            e.kind()
        )
    })
}

/// Metadata for an optional path. Only `NotFound` is accepted as absence.
///
/// `Path::exists`, `is_file`, and `is_dir` collapse every metadata error to
/// `false`. This helper retains the one legitimate optional case while making
/// permission, loop, and other classification failures panic with the exact
/// path and error kind. Metadata follows symlinks like those predicates.
pub fn optional_metadata(path: &Path) -> Option<std::fs::Metadata> {
    classify_optional_metadata(path, std::fs::metadata(path))
}

/// True when an optional path exists and is a regular file.
pub fn optional_file_exists(path: &Path) -> bool {
    optional_metadata(path).is_some_and(|meta| meta.is_file())
}

/// True when an optional path exists and is a directory.
pub fn optional_dir_exists(path: &Path) -> bool {
    optional_metadata(path).is_some_and(|meta| meta.is_dir())
}

/// The metadata of an optional path: `None` only when it does not exist; any
/// other I/O failure panics.
pub fn classify_optional_metadata(
    path: &Path,
    result: std::io::Result<std::fs::Metadata>,
) -> Option<std::fs::Metadata> {
    match result {
        Ok(meta) => Some(meta),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => panic!(
            "optional_metadata: metadata failed at {:?} (kind {:?}): {e} — an \
             optional-path probe may treat NotFound as absence, never an \
             unreadable path",
            path,
            e.kind()
        ),
    }
}

/// The metadata of an entry a directory walk discovered; failing to read it
/// panics rather than dropping the entry.
pub fn discovered_entry_metadata_or_panic(
    path: &Path,
    result: std::io::Result<std::fs::Metadata>,
) -> std::fs::Metadata {
    result.unwrap_or_else(|e| {
        panic!(
            "walk_inner: metadata failed at {:?} (kind {:?}): {e}",
            path,
            e.kind()
        )
    })
}

/// Every entry of a directory read, panicking on any entry that failed.
pub fn collect_entries_or_panic<I>(dir: &Path, entries: I) -> Vec<PathBuf>
where
    I: IntoIterator<Item = std::io::Result<PathBuf>>,
{
    let mut out = Vec::new();
    for entry in entries {
        let path = entry.unwrap_or_else(|e| {
            panic!("walk_rs_files: directory entry read failed under {dir:?}: {e}")
        });
        out.push(path);
    }
    out.sort();
    out
}

/// Interior directory links are followed, so `ancestors` holds the identity of
/// every directory on the current path: a link back to one of them is a cycle,
/// and fails closed rather than recursing until a path-length error.
fn walk_inner(
    root: &Path,
    dir: &Path,
    ext: &str,
    ancestors: &mut Vec<(u64, u64)>,
    out: &mut Vec<String>,
) {
    let rd = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("walk_rs_files: read_dir failed at {dir:?}: {e}"));
    let entries = collect_entries_or_panic(dir, rd.map(|r| r.map(|e| e.path())));
    for path in entries {
        let metadata = discovered_entry_metadata_or_panic(&path, std::fs::metadata(&path));
        if metadata.is_dir() {
            let identity = directory_identity(&path, &metadata);
            assert!(
                !ancestors.contains(&identity),
                "walk_rs_files: directory link cycle at {path:?}: it leads back to a directory \
                 already being walked"
            );
            ancestors.push(identity);
            walk_inner(root, &path, ext, ancestors, out);
            ancestors.pop();
        } else if path.extension().and_then(|e| e.to_str()) == Some(ext) {
            out.push(relative_path_or_panic(root, &path));
        }
    }
}

/// `path` relative to `root`, panicking when it is not below it.
pub fn relative_path_or_panic(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or_else(|e| {
        panic!("walk_rs_files: strip_prefix failed for {path:?} against root {root:?}: {e}")
    });
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

// Direct-path test consumers make this module private, so their partial use of
// its public API would otherwise multiply `dead_code` diagnostics across
// Clippy subjects. Keep the whole shared API type-checked and reachable in test
// builds without adding runtime work or suppressing the lint.
#[cfg(test)]
const _: () = {
    let _: fn(&Path) -> Vec<String> = walk_rs_files;
    let _: fn(&Path, &str) -> Vec<String> = walk_files_with_ext;
    let _: fn(&Path) -> Vec<String> = walk_rs_files_follow_root;
    let _: fn(&Path, &str) -> Vec<String> = walk_files_with_ext_follow_root;
    let _: fn(&Path) -> String = read_source_or_panic;
    let _: fn(&Path) -> Vec<PathBuf> = read_dir_sorted;
    let _: fn(&Path) -> Vec<String> = rust_file_stems;
    let _: fn(&Path) -> std::fs::Metadata = required_metadata;
    let _: fn(&Path) -> Option<std::fs::Metadata> = optional_metadata;
    let _: fn(&Path) -> bool = optional_file_exists;
    let _: fn(&Path) -> bool = optional_dir_exists;
};

#[cfg(test)]
mod cycle_tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("enzui-strict-fs-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/inner")).expect("scratch tree");
        std::fs::write(root.join("src/inner/lib.rs"), "").expect("source file");
        root
    }

    #[test]
    fn a_shared_linked_directory_is_walked_but_a_cycle_fails_closed() {
        let root = scratch("links");
        // The same directory reached twice is fine.
        std::os::unix::fs::symlink(root.join("src/inner"), root.join("src/again")).expect("link");
        assert_eq!(
            walk_rs_files(&root),
            ["src/again/lib.rs", "src/inner/lib.rs"]
        );

        // A link back to an ancestor is a cycle.
        std::os::unix::fs::symlink(root.join("src"), root.join("src/inner/up")).expect("link");
        let walked = std::panic::catch_unwind(|| walk_rs_files(&root));
        let message = walked
            .expect_err("the cycle fails closed")
            .downcast::<String>()
            .map(|message| *message)
            .unwrap_or_default();
        assert!(message.contains("cycle"), "{message}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
