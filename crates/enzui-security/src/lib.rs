//! Dependency-light security and filesystem primitives shared by enzUI crates.

#![cfg_attr(docsrs, feature(doc_cfg))]
#![warn(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    reason = "this crate parses untrusted bytes; an `as` that loses data hides a bug"
)]

pub mod cancellation;
pub mod decode_limits;
pub mod durable;
pub mod loopback_proxy;
pub mod quarantine;
pub mod random;
pub mod redirect_capture;
pub mod runtime_dir;
pub mod secret;
pub mod secure_dir;
pub mod tempstage;
pub mod url_policy;

#[cfg(test)]
#[path = "../../enzui-test-support/src/test_scratch.rs"]
#[allow(
    unreachable_pub,
    reason = "public API of the test-support crate, private here"
)]
mod test_scratch;

/// Platform constants shared by secure file openers.
pub mod platform {
    /// The platform `O_NOFOLLOW` value used for final-component symlink refusal.
    #[cfg(unix)]
    pub const fn libc_o_nofollow() -> i32 {
        #[cfg(target_os = "linux")]
        {
            0o400_000
        }
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            0x0100
        }
        #[cfg(any(target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd"))]
        {
            0x0100
        }
        #[cfg(target_os = "openbsd")]
        {
            0x0100
        }
        #[cfg(not(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "dragonfly",
            target_os = "netbsd",
            target_os = "openbsd"
        )))]
        {
            0
        }
    }

    /// The platform `O_DIRECTORY` value: `open(2)` fails with `ENOTDIR` unless
    /// the final component is a directory.
    ///
    /// This is a *defense-in-depth early refusal* for an opener that must only
    /// ever hold a directory descriptor (e.g. binding a mode-tightening `chmod`
    /// to a validated directory inode). The authoritative directory
    /// check is still the `fstat` of the returned descriptor: `O_DIRECTORY`
    /// merely refuses a non-directory *before* the caller can chmod through the
    /// fd, closing the window where a same-uid actor swaps a directory for a
    /// file at the pathname between the caller's `lstat` and its `open`.
    ///
    /// Only the Linux value is asserted here (the sole supported target). Every
    /// other platform returns `0` — no flag — because a *wrong* non-zero bit
    /// would be worse than none, and the mandatory `fstat` `is_dir()` check
    /// refuses a non-directory regardless. Kept beside [`libc_o_nofollow`] so
    /// there is one platform table, not a second near-duplicate.
    #[cfg(unix)]
    pub const fn libc_o_directory() -> i32 {
        #[cfg(target_os = "linux")]
        {
            0o200_000
        }
        #[cfg(not(target_os = "linux"))]
        {
            0
        }
    }

    /// The platform `O_NONBLOCK` value used so an `open` can never block.
    ///
    /// A read-only `open(2)` of a FIFO with no writer blocks **inside the
    /// open**, before the caller can ever `fstat` the descriptor and refuse a
    /// non-regular file. Any opener that must not hang on a planted named pipe
    /// therefore needs this flag; it is inert for a regular file. Kept beside
    /// [`libc_o_nofollow`] so there is one platform table, not a second
    /// near-duplicate with a subtly different value.
    ///
    /// `0` on an unknown platform: a wrong non-zero flag bit would be worse
    /// than no flag, and the descriptor check still refuses the FIFO if the
    /// open ever returns.
    #[cfg(unix)]
    pub const fn libc_o_nonblock() -> i32 {
        #[cfg(target_os = "linux")]
        {
            0o4000
        }
        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "dragonfly",
            target_os = "netbsd",
            target_os = "openbsd"
        ))]
        {
            0x0004
        }
        #[cfg(not(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "dragonfly",
            target_os = "netbsd",
            target_os = "openbsd"
        )))]
        {
            0
        }
    }
}
