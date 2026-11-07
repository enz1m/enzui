//! Socket trust for the center connection: the daemon's socket and its
//! parent directory must be owner-only, and the connected peer must run as
//! this user.

use std::io::{self, ErrorKind};
use std::path::Path;

/// Why a broker socket is refused before protocol exchange.
#[derive(Debug)]
pub(crate) enum SocketError {
    /// The path did not exist, or could not be `lstat`ed.
    NotFound(io::Error),
    /// The path is a symlink. Refused without following it — the same rule
    /// [`crate::secret`] applies to credential files, for the same reason.
    IsSymlink,
    NotASocket,
    WrongOwner {
        socket_uid: u32,
        current_uid: u32,
    },
    /// This process's uid could not be established, so ownership could not be
    /// verified. Fails closed rather than skipping the check.
    OwnerUnverifiable,
    InsecureMode {
        mode: u32,
    },
    /// The socket's parent directory is not a private, owner-only directory.
    /// Carries the specific directory refusal.
    InsecureParent(ParentRefusal),
    /// The kernel-reported peer uid differed before protocol exchange.
    PeerUidMismatch {
        peer_uid: u32,
        expected_uid: u32,
    },
    PeerCredentialUnavailable(io::Error),
}

impl core::fmt::Display for SocketError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotFound(e) => write!(f, "broker socket not found: {e}"),
            Self::IsSymlink => write!(f, "broker socket path is a symlink"),
            Self::NotASocket => write!(f, "broker socket path is not a socket"),
            Self::WrongOwner {
                socket_uid,
                current_uid,
            } => write!(
                f,
                "broker socket is owned by uid {socket_uid}, not the current uid {current_uid}"
            ),
            Self::OwnerUnverifiable => {
                write!(
                    f,
                    "could not determine current uid to verify socket ownership"
                )
            }
            Self::InsecureMode { mode } => {
                write!(f, "broker socket mode {mode:04o} grants group/other access")
            }
            Self::InsecureParent(reason) => {
                write!(f, "broker socket parent directory is not private: {reason}")
            }
            Self::PeerUidMismatch {
                peer_uid,
                expected_uid,
            } => write!(
                f,
                "peer uid {peer_uid} does not match expected uid {expected_uid}"
            ),
            Self::PeerCredentialUnavailable(e) => {
                write!(f, "could not read peer credentials: {e}")
            }
        }
    }
}

impl std::error::Error for SocketError {}

/// Why a socket's parent directory is not trusted to hold a private socket.
///
/// The socket mode alone cannot carry the trust decision. Where the parent is
/// writable by another principal, the entry at the socket's name can be
/// replaced between validation and `connect`, so the mode observed is not the
/// mode connected to. Where it merely grants read or search access, the socket
/// stays ours but its location is not the owner-only one the daemon serves
/// from. This is the directory half of that decision.
#[derive(Debug)]
pub(crate) enum ParentRefusal {
    /// The parent directory could not be `lstat`ed.
    Unreadable(io::Error),
    /// The parent path is a symlink. Refused without following it, for the
    /// same reason [`SocketError::IsSymlink`] refuses one at the socket.
    IsSymlink,
    /// The parent path exists but is not a directory.
    NotADirectory,
    /// Another uid owns the parent directory.
    WrongOwner {
        /// The directory's owner.
        dir_uid: u32,
        /// This process's uid.
        current_uid: u32,
    },
    /// The parent directory grants group, other, or special access, so its
    /// entries are not exclusively ours to reach or to replace.
    InsecureMode {
        /// The observed permission bits.
        mode: u32,
    },
    /// The socket path has no parent component at all, so no directory could
    /// be checked. Fails closed rather than assuming the current directory.
    NoParent,
}

impl core::fmt::Display for ParentRefusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Unreadable(e) => write!(f, "parent directory could not be read: {e}"),
            Self::IsSymlink => write!(f, "parent directory path is a symlink"),
            Self::NotADirectory => write!(f, "parent path is not a directory"),
            Self::WrongOwner {
                dir_uid,
                current_uid,
            } => write!(
                f,
                "parent directory is owned by uid {dir_uid}, not the current uid {current_uid}"
            ),
            Self::InsecureMode { mode } => write!(
                f,
                "parent directory mode {mode:04o} grants group/other access"
            ),
            Self::NoParent => write!(f, "socket path has no parent directory to check"),
        }
    }
}

impl std::error::Error for ParentRefusal {}

// Read the effective uid from the kernel; parse failure makes ownership unverifiable.
fn current_uid() -> Option<u32> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("Uid:"))?;
    line.split_whitespace().nth(2)?.parse().ok()
}

/// Kernel-reported Unix peer credentials.
///
/// Only `uid` is an authorization boundary; pid and gid are diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PeerCredential {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
}

/// Reads Linux `SO_PEERCRED` from a connected Unix socket.
///
/// # Errors
/// Returns an I/O error when the syscall fails or returns an unexpected
/// credential structure size.
#[cfg(target_os = "linux")]
#[allow(unsafe_code, reason = "`SO_PEERCRED` has no safe std API")]
pub(crate) fn peer_credential(
    fd: &impl std::os::unix::io::AsRawFd,
) -> Result<PeerCredential, io::Error> {
    // Linux `SOL_SOCKET` and `SO_PEERCRED` ABI values.
    const SOL_SOCKET: i32 = 1;
    const SO_PEERCRED: i32 = 17;

    // `struct ucred` from `<sys/socket.h>`. The layout is
    //   { pid_t pid; uid_t uid; gid_t gid; }
    // all three are 32-bit on every Linux architecture.
    #[repr(C)]
    struct Ucred {
        pid: i32,
        uid: u32,
        gid: u32,
    }

    let mut cred = Ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = core::mem::size_of::<Ucred>() as u32;

    unsafe extern "C" {
        fn getsockopt(
            sockfd: i32,
            level: i32,
            optname: i32,
            optval: *mut Ucred,
            optlen: *mut u32,
        ) -> i32;
    }
    // SAFETY: `getsockopt` receives a valid descriptor, an aligned stack output
    // buffer of `size_of::<Ucred>()`, and an initialized length pointer. The
    // returned size is verified below, and no pointer escapes the call.
    let ret = unsafe { getsockopt(fd.as_raw_fd(), SOL_SOCKET, SO_PEERCRED, &mut cred, &mut len) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    if len as usize != core::mem::size_of::<Ucred>() {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            "SO_PEERCRED returned unexpected credential size",
        ));
    }
    Ok(PeerCredential {
        pid: cred.pid,
        uid: cred.uid,
        gid: cred.gid,
    })
}

/// Non-Linux stub: `SO_PEERCRED` is not available.
#[cfg(not(target_os = "linux"))]
pub fn peer_credential(_fd: &impl std::os::unix::io::AsRawFd) -> Result<PeerCredential, io::Error> {
    Err(io::Error::new(
        ErrorKind::Unsupported,
        "SO_PEERCRED is only available on Linux",
    ))
}

/// Verifies the kernel-reported peer uid against `expected_uid`.
///
/// # Errors
/// Returns [`SocketError::PeerCredentialUnavailable`] or
/// [`SocketError::PeerUidMismatch`].
pub(crate) fn verify_peer_uid(
    fd: &impl std::os::unix::io::AsRawFd,
    expected_uid: u32,
) -> Result<PeerCredential, SocketError> {
    let cred = peer_credential(fd).map_err(SocketError::PeerCredentialUnavailable)?;
    if cred.uid != expected_uid {
        return Err(SocketError::PeerUidMismatch {
            peer_uid: cred.uid,
            expected_uid,
        });
    }
    Ok(cred)
}

/// Validates ownership, type, and permissions of a broker socket path.
///
/// This path check has an unavoidable race before `connect`; peer credentials
/// and the mutual handshake remain authoritative after connection.
///
/// # Errors
/// Returns the applicable [`SocketError`] refusal.
pub(crate) fn validate_socket_path(path: &Path) -> Result<(), SocketError> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    // `symlink_metadata` exposes rather than follows a final-component symlink.
    let meta = std::fs::symlink_metadata(path).map_err(SocketError::NotFound)?;
    let ft = meta.file_type();
    if ft.is_symlink() {
        return Err(SocketError::IsSymlink);
    }
    if !ft.is_socket() {
        return Err(SocketError::NotASocket);
    }
    let socket_uid = meta.uid();
    let current = current_uid().ok_or(SocketError::OwnerUnverifiable)?;
    if socket_uid != current {
        return Err(SocketError::WrongOwner {
            socket_uid,
            current_uid: current,
        });
    }
    // Refuse group, other, and special permission bits.
    let mode = meta.mode() & 0o7777;
    if mode & 0o7077 != 0 {
        return Err(SocketError::InsecureMode { mode });
    }
    Ok(())
}

/// Validates that the socket's parent directory is private to this uid.
///
/// The directory must be a real directory (not a symlink), owned by this uid,
/// and free of group, other, and special permission bits — the same mask
/// [`validate_socket_path`] applies to the socket.
///
/// The parent's mode carries two distinct guarantees. Write access decides
/// substitution: where another principal may write, a mode-0600 socket can be
/// unlinked and replaced at the same name, so the inode
/// [`validate_socket_path`] validated need not be the inode `connect` reaches.
/// Read and search access decide disclosure: a mode-0755 parent is writable by
/// nobody else and its 0600 socket is connectable by nobody else, but it still
/// publishes the socket's name, and the daemon serves only from a directory
/// closed to everyone but its owner. A path outside that contract is not the
/// one the daemon laid out.
///
/// Only the immediate parent is checked. Ancestors above it are the runtime
/// directory's contract ([`crate::runtime_dir`] resolves a 0700 per-user
/// directory and refuses an unsafe `$XDG_RUNTIME_DIR`), not this function's.
///
/// # Errors
/// Returns [`SocketError::InsecureParent`] carrying the [`ParentRefusal`], or
/// [`SocketError::OwnerUnverifiable`] when this process's uid is unknown, so
/// ownership fails closed rather than being skipped.
pub(crate) fn validate_socket_parent(path: &Path) -> Result<(), SocketError> {
    use std::os::unix::fs::MetadataExt;

    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        // A bare relative name such as `center.sock` has an empty parent. The
        // directory that would be checked is the process's current directory,
        // which is not a path the caller named, so refuse instead of guessing.
        _ => return Err(SocketError::InsecureParent(ParentRefusal::NoParent)),
    };
    // `symlink_metadata` exposes rather than follows a symlinked parent.
    let meta = std::fs::symlink_metadata(parent)
        .map_err(|error| SocketError::InsecureParent(ParentRefusal::Unreadable(error)))?;
    let file_type = meta.file_type();
    if file_type.is_symlink() {
        return Err(SocketError::InsecureParent(ParentRefusal::IsSymlink));
    }
    if !file_type.is_dir() {
        return Err(SocketError::InsecureParent(ParentRefusal::NotADirectory));
    }
    let current_uid = current_uid().ok_or(SocketError::OwnerUnverifiable)?;
    let dir_uid = meta.uid();
    if dir_uid != current_uid {
        return Err(SocketError::InsecureParent(ParentRefusal::WrongOwner {
            dir_uid,
            current_uid,
        }));
    }
    let mode = meta.mode() & 0o7777;
    if mode & 0o7077 != 0 {
        return Err(SocketError::InsecureParent(ParentRefusal::InsecureMode {
            mode,
        }));
    }
    Ok(())
}

/// Validates both halves of a socket's trust contract: the socket inode
/// ([`validate_socket_path`]) and its parent directory
/// ([`validate_socket_parent`]).
///
/// This is the check to run before writing the first authentication byte to a
/// daemon socket. Either half alone is insufficient: a private socket outside a
/// private directory is not the daemon's socket to begin with, and a private
/// directory does not make a group-readable socket acceptable.
///
/// # Errors
/// The first applicable [`SocketError`], socket half first, so the diagnostic
/// names the closest reason to the path the caller supplied.
pub(crate) fn validate_socket_trust(path: &Path) -> Result<(), SocketError> {
    validate_socket_path(path)?;
    validate_socket_parent(path)
}

#[cfg(test)]
#[path = "peer_tests.rs"]
mod tests;
