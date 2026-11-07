//! Classifies I/O errors into coarse categories suitable for user-facing surfaces.

use std::io;

/// Category of an I/O failure, chosen for what a user can do about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IoFailureCategory {
    /// The OS refused access — `PermissionDenied`, or a read-only filesystem
    /// for a write.
    Permission,
    /// The named file or directory does not exist.
    NotFound,
    /// A path component (or the target itself) is not a directory.
    NotADirectory,
    /// No device space or write quota remains.
    StorageExhausted,
    /// Any I/O failure not covered by another category.
    Unknown,
}

impl IoFailureCategory {
    /// Classify one `io::Error`.
    #[must_use]
    pub fn classify(error: &io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => Self::Permission,
            io::ErrorKind::NotFound => Self::NotFound,
            io::ErrorKind::NotADirectory => Self::NotADirectory,
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => Self::StorageExhausted,
            _ => {
                // Raw OS error 28 is Linux `ENOSPC`.
                #[cfg(target_os = "linux")]
                if error.raw_os_error() == Some(28) {
                    return Self::StorageExhausted;
                }
                Self::Unknown
            }
        }
    }
}
