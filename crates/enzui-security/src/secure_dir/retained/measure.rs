use super::*;
use std::collections::HashSet;
use std::os::unix::ffi::OsStrExt;

/// Whole-tree resource limits for [`RetainedDir::logical_size_bounded`].
#[derive(Debug, Clone, Copy)]
pub struct TreeSizeLimits {
    /// Maximum entries across all visited directories, excluding the root itself.
    pub max_entries: usize,
    /// Maximum aggregate basename bytes across the entire tree.
    pub max_name_bytes: usize,
    /// Maximum child-directory depth; zero permits only root-level non-directories.
    pub max_depth: usize,
}

impl RetainedDir {
    /// Counts regular-file logical bytes through retained descriptors, without following links.
    ///
    /// Sparse files contribute their logical lengths, not allocated blocks. Hard
    /// links count once per directory entry. Directories, symlinks and special
    /// files contribute no bytes, but all entries consume the global budgets.
    /// Root or ancestor pathname replacement cannot redirect the walk. Repeated
    /// directory identities are refused, including bind-mount cycles.
    ///
    /// Each directory uses [`Self::retain_entries_bounded`]'s two-pass census.
    /// This is not an atomic content snapshot; callers needing an exact stable
    /// total must exclude concurrent changes. Limits bound traversal resources,
    /// not kernel I/O latency.
    ///
    /// # Errors
    /// Returns an enumeration, descriptor or metadata error, or `InvalidInput`
    /// when an entry/name/depth budget, repeated directory, or byte sum overflows.
    /// No partial total is returned.
    pub fn logical_size_bounded(&self, limits: &TreeSizeLimits) -> io::Result<u64> {
        let mut remaining = EnumerationLimits {
            max_entries: limits.max_entries,
            max_name_bytes: limits.max_name_bytes,
        };
        let mut visited = HashSet::from([self.identity()]);
        let mut pending = vec![(self.clone(), 0usize)];
        let mut bytes = 0u64;
        while let Some((directory, depth)) = pending.pop() {
            let entries = directory.retain_entries_bounded(&remaining)?;
            for entry in entries {
                remaining.max_entries = remaining.max_entries.checked_sub(1).ok_or_else(budget)?;
                remaining.max_name_bytes = remaining
                    .max_name_bytes
                    .checked_sub(entry.name().as_bytes().len())
                    .ok_or_else(budget)?;
                let metadata = entry.metadata()?;
                if metadata.is_file() {
                    bytes = bytes.checked_add(metadata.len()).ok_or_else(budget)?;
                } else if metadata.is_dir() {
                    let child_depth = depth.checked_add(1).ok_or_else(budget)?;
                    if child_depth > limits.max_depth || !visited.insert(entry.identity()) {
                        return Err(budget());
                    }
                    pending.push((entry.open_directory()?, child_depth));
                }
            }
        }
        Ok(bytes)
    }
}

fn budget() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "directory size walk exceeds its resource limits",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "retained-size-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn limits() -> TreeSizeLimits {
        TreeSizeLimits {
            max_entries: 100,
            max_name_bytes: 4096,
            max_depth: 8,
        }
    }

    #[test]
    fn retained_size_cannot_be_redirected_by_root_path_replacement() {
        let root = Fixture::new();
        let visible = root.0.join("visible");
        fs::create_dir_all(visible.join("child")).unwrap();
        fs::write(visible.join("child/file"), b"original").unwrap();
        let directory = RetainedDir::open(&visible).unwrap();
        fs::rename(&visible, root.0.join("retained")).unwrap();
        fs::create_dir_all(visible.join("child")).unwrap();
        fs::write(visible.join("child/file"), b"substituted data").unwrap();
        assert_eq!(directory.logical_size_bounded(&limits()).unwrap(), 8);
        assert_eq!(
            RetainedDir::open(&visible)
                .unwrap()
                .logical_size_bounded(&limits())
                .unwrap(),
            16
        );
    }

    #[test]
    fn retained_size_charges_entry_and_name_budgets_across_sibling_directories() {
        let root = Fixture::new();
        for name in ["a", "b"] {
            fs::create_dir(root.0.join(name)).unwrap();
            fs::write(root.0.join(name).join("payload"), b"123").unwrap();
        }
        let directory = RetainedDir::open(&root.0).unwrap();
        let exact = TreeSizeLimits {
            max_entries: 4,
            max_name_bytes: 16,
            max_depth: 1,
        };
        assert_eq!(directory.logical_size_bounded(&exact).unwrap(), 6);
        for limited in [
            TreeSizeLimits {
                max_entries: 3,
                ..exact
            },
            TreeSizeLimits {
                max_name_bytes: 15,
                ..exact
            },
            TreeSizeLimits {
                max_depth: 0,
                ..exact
            },
        ] {
            assert_eq!(
                directory.logical_size_bounded(&limited).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[test]
    fn retained_size_handles_empty_zero_budget_and_deep_trees_iteratively() {
        let root = Fixture::new();
        let directory = RetainedDir::open(&root.0).unwrap();
        let zero = TreeSizeLimits {
            max_entries: 0,
            max_name_bytes: 0,
            max_depth: 0,
        };
        assert_eq!(directory.logical_size_bounded(&zero).unwrap(), 0);
        let mut path = root.0.clone();
        for _ in 0..10 {
            path.push("d");
            fs::create_dir(&path).unwrap();
        }
        fs::write(path.join("file"), b"deep").unwrap();
        assert!(directory.logical_size_bounded(&limits()).is_err());
        assert_eq!(
            directory
                .logical_size_bounded(&TreeSizeLimits {
                    max_depth: 10,
                    ..limits()
                })
                .unwrap(),
            4
        );
    }
}
