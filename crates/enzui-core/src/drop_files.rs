//! Backing storage for virtual files received from another application.
//!
//! A source may remove exported attachment paths after acknowledging a drop.
//! Copy those files on a blocking worker before finishing the receipt. Keep the
//! returned owner while a viewer uses its paths, or preserve a successfully
//! imported document in an application-owned storage directory.

use crate::cancellation::CancelToken;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Private copies whose paths remain usable independently of the source.
/// Clones share ownership; the final owner removes unpreserved copies.
#[derive(Clone, Debug)]
pub struct CopiedFiles {
    paths: Vec<PathBuf>,
    directory: Arc<Mutex<Option<tempfile::TempDir>>>,
    bytes: u64,
}

impl CopiedFiles {
    /// Paths to the copied files, in source order.
    #[must_use]
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    /// Total copied bytes, for the receiving application's storage budget.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Whether this owner backs `path`.
    #[must_use]
    pub fn contains(&self, path: &Path) -> bool {
        self.paths.iter().any(|owned| owned == path)
    }

    /// Keep an accepted import after its owners are dropped.
    /// Use an application-owned `storage` directory when creating persistent imports.
    pub fn preserve(&self) {
        let directory = self
            .directory
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(directory) = directory {
            let _ = directory.keep();
        }
    }
}

/// Copy at most 128 regular files without replacing existing data.
///
/// Call on a blocking worker. `None` uses private session storage; `Some(root)`
/// places the batch inside a private application storage directory. A failed or
/// cancelled batch removes its staged copies. Original files are never changed.
/// Keep sources quiescent when a consistent snapshot is required; size changes
/// are refused, but concurrent edits of the same length cannot be detected.
///
/// # Errors
/// Refuses relative paths, source links/non-files, unsafe storage permissions,
/// cancellation, and batches larger than `max_bytes` (including changing files).
pub fn copy_files(
    paths: &[PathBuf],
    storage: Option<&Path>,
    max_bytes: u64,
    cancel: &CancelToken,
) -> io::Result<CopiedFiles> {
    if paths.is_empty() || paths.len() > 128 || cancel.is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "No live file batch",
        ));
    }
    if let Some(root) = storage {
        if !root.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Storage must be absolute",
            ));
        }
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(root)?;
        let info = fs::symlink_metadata(root)?;
        if !info.is_dir() || fs::canonicalize(root)? != root {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Storage is not a private directory",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if info.uid() != rustix::process::geteuid().as_raw() || info.mode() & 0o077 != 0 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Storage is not private",
                ));
            }
        }
    }
    let mut builder = tempfile::Builder::new();
    builder.prefix("enzui-transfer-");
    let directory = match storage {
        Some(root) => builder.tempdir_in(root)?,
        None => builder.tempdir()?,
    };
    let mut copied = Vec::new();
    let mut total = 0u64;
    let mut buffer = vec![0; 64 * 1024];
    for (index, path) in paths.iter().enumerate() {
        if cancel.is_terminal() {
            return Err(io::Error::from(io::ErrorKind::Interrupted));
        }
        if !path.is_absolute() || fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Expected an absolute regular file",
            ));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let input = options.open(path)?;
        let metadata = input.metadata()?;
        if !metadata.is_file() || metadata.len() > max_bytes.saturating_sub(total) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "File batch exceeds its budget",
            ));
        }
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Missing file name"))?;
        let folder = directory.path().join(index.to_string());
        fs::create_dir(&folder)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&folder, fs::Permissions::from_mode(0o700))?;
        }
        let destination = folder.join(name);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output: File = options.open(&destination)?;
        let mut input = input.take(metadata.len().saturating_add(1));
        let mut written = 0u64;
        loop {
            if cancel.is_terminal() {
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            let read = input.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            written += read as u64;
            if written > metadata.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Source changed while copying",
                ));
            }
            output.write_all(&buffer[..read])?;
        }
        if written != metadata.len() {
            return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
        }
        output.sync_all()?;
        total += written;
        copied.push(destination);
    }
    Ok(CopiedFiles {
        paths: copied,
        directory: Arc::new(Mutex::new(Some(directory))),
        bytes: total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn received_snapshots_survive_source_cleanup_and_release_only_their_own_files() {
        let source = tempfile::tempdir().unwrap();
        let a = source.path().join("a.txt");
        let b = source.path().join("b.txt");
        fs::write(&a, b"attachment A").unwrap();
        fs::write(&b, b"attachment B").unwrap();
        #[cfg(unix)]
        {
            let linked = source.path().join("linked.txt");
            std::os::unix::fs::symlink(&a, &linked).unwrap();
            assert!(copy_files(&[linked], None, 100, &CancelToken::new()).is_err());
        }
        let paths = vec![a, b];
        assert!(copy_files(&paths, None, 1, &CancelToken::new()).is_err());
        let cancelled = CancelToken::new();
        cancelled.cancel(crate::cancellation::CancelCause::Cancelled);
        assert!(copy_files(&paths, None, 100, &cancelled).is_err());
        let copies = copy_files(&paths, None, 100, &CancelToken::new()).unwrap();
        let retained = copies.clone();
        let copy_paths = copies.paths().to_vec();
        drop(source);
        assert_eq!(fs::read(&copy_paths[0]).unwrap(), b"attachment A");
        assert_eq!(fs::read(&copy_paths[1]).unwrap(), b"attachment B");
        drop(copies);
        assert!(copy_paths.iter().all(|path| path.is_file()));
        drop(retained);
        assert!(copy_paths.iter().all(|path| !path.exists()));
        let storage = tempfile::tempdir().unwrap();
        let original = storage.path().join("original.txt");
        fs::write(&original, b"saved document").unwrap();
        let copies = copy_files(
            std::slice::from_ref(&original),
            Some(&storage.path().join("imports")),
            100,
            &CancelToken::new(),
        )
        .unwrap();
        let path = copies.paths()[0].clone();
        copies.preserve();
        drop(copies);
        assert_eq!(fs::read(path).unwrap(), b"saved document");
        assert_eq!(fs::read(original).unwrap(), b"saved document");
    }
}
