use super::*;
use std::os::unix::ffi::OsStrExt;

/// Typed limits for bounded directory enumeration.
///
/// Both limits are checked before each name allocation or entry retention.
/// Zero values refuse any nonempty directory; an empty directory succeeds.
#[derive(Debug, Clone, Copy)]
pub struct EnumerationLimits {
    /// Maximum number of entries.
    pub max_entries: usize,
    /// Maximum aggregate byte length across all entry names.
    pub max_name_bytes: usize,
}

impl RetainedDir {
    /// Enumerate and retain every entry within bounded resource limits.
    ///
    /// Duplicates the retained directory fd and iterates with `getdents`, never
    /// `std::fs::read_dir`. Returns entries in deterministic raw-byte name
    /// order, each retaining parent, name and inode for direct use with
    /// [`RetainedEntry::rename_noreplace`] and [`RetainedEntry::move_noreplace`].
    ///
    /// Regular files, directories and symlinks are retained without following
    /// links or recursing.
    ///
    /// Names and identities are observed, retained with `O_PATH|NOFOLLOW`, then
    /// observed again. A difference between those observations refuses the
    /// whole result. This is not an atomic snapshot: concurrent mutation can
    /// occur during or after either pass, so callers needing a complete stable
    /// population must provide exclusion.
    ///
    /// # Errors
    ///
    /// - `getdents` iterator failure in either pass.
    /// - Entry name is empty, contains NUL, or contains a path separator.
    /// - Entry count exceeds [`EnumerationLimits::max_entries`].
    /// - Aggregate name bytes exceed [`EnumerationLimits::max_name_bytes`].
    /// - Any entry cannot be retained or changed identity since enumeration.
    /// - The confirmation pass observes a different population than the retained
    ///   descriptors.
    pub fn retain_entries_bounded(
        &self,
        limits: &EnumerationLimits,
    ) -> io::Result<Vec<RetainedEntry>> {
        let observed = self.bounded_getdents(limits)?;

        #[cfg(test)]
        fault::run()?;

        let mut retained = Vec::with_capacity(observed.len());
        for (name, expected_identity) in &observed {
            let entry = self.retain_entry(name).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!("census incomplete: failed to retain {name:?}: {error}"),
                )
            })?;
            if entry.identity() != *expected_identity {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "census incomplete: entry identity changed between enumeration and retention",
                ));
            }
            retained.push(entry);
        }

        // Build the authoritative sorted snapshot from the retained descriptors.
        let retained_snapshot: Vec<(&OsStr, (u64, u64))> =
            retained.iter().map(|e| (e.name(), e.identity())).collect();

        let confirmed = self.bounded_getdents(limits)?;

        if confirmed.len() != retained_snapshot.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "census changed: entry count differs between retention and confirmation",
            ));
        }
        for (confirmed_entry, &(retained_name, retained_id)) in
            confirmed.iter().zip(retained_snapshot.iter())
        {
            if confirmed_entry.0.as_os_str() != retained_name || confirmed_entry.1 != retained_id {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "census changed: entry name or identity differs at confirmation",
                ));
            }
        }

        Ok(retained)
    }

    fn bounded_getdents(
        &self,
        limits: &EnumerationLimits,
    ) -> io::Result<Vec<(OsString, (u64, u64))>> {
        let dirfd = rustix::fs::openat(
            self.fd(),
            ".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;

        let mut dir = rustix::fs::Dir::read_from(dirfd).map_err(io::Error::from)?;
        let mut entries: Vec<(OsString, (u64, u64))> = Vec::new();
        let mut aggregate_bytes: usize = 0;

        while let Some(result) = dir.read() {
            let item = result.map_err(io::Error::from)?;
            let name_bytes = item.file_name().to_bytes();

            if name_bytes == b"." || name_bytes == b".." {
                continue;
            }

            if name_bytes.is_empty() || name_bytes.contains(&0) || name_bytes.contains(&b'/') {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "directory entry name is empty, contains NUL, or contains a path separator",
                ));
            }

            let name_len = name_bytes.len();

            if entries.len() >= limits.max_entries {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "directory contains more than {} entries",
                        limits.max_entries,
                    ),
                ));
            }

            let new_aggregate = aggregate_bytes.checked_add(name_len).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "aggregate name bytes overflow")
            })?;
            if new_aggregate > limits.max_name_bytes {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "aggregate name bytes exceed limit of {}",
                        limits.max_name_bytes,
                    ),
                ));
            }

            let name = OsStr::from_bytes(name_bytes).to_os_string();
            let identity = entry_identity_at(self.fd(), &name)?;

            aggregate_bytes = new_aggregate;
            entries.push((name, identity));
        }

        entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        Ok(entries)
    }
}

#[cfg(test)]
pub(super) mod fault {
    use std::cell::RefCell;
    use std::io;

    type Hook = Box<dyn FnMut() -> io::Result<()> + 'static>;
    thread_local! { static HOOK: RefCell<Option<Hook>> = const { RefCell::new(None) }; }

    pub(crate) fn run() -> io::Result<()> {
        HOOK.with(|slot| match slot.borrow_mut().as_mut() {
            Some(hook) => hook(),
            None => Ok(()),
        })
    }
}
