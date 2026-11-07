//! One shared OS-backed random-byte boundary for every security identity.
//!
//! Handshake nonces, session ids, broker credentials and consent grant keys
//! draw from this boundary, which has two outcomes: fresh OS bytes or a typed
//! failure. There is no fallback to a clock, PID or counter, which would make
//! an identity predictable or repeat it after a restart; a service that cannot
//! obtain entropy refuses to run.
//!
//! # Why `/dev/urandom` and not a dependency
//!
//! This crate keeps a two-dependency, no-`libc`, no-`unsafe`-syscall budget
//! (see [`crate::secret`] for the same discipline). The portable `getrandom`
//! crate or a raw `getrandom(2)` syscall would both breach it. `/dev/urandom`
//! is reachable through plain [`std::fs`], is the kernel CSPRNG on every target
//! this framework runs on, and — unlike `/dev/random` — does not block after
//! boot. The one hazard of reading it as a *path* is that a hostile parent could
//! plant a regular file at `/dev/urandom`, so the opened descriptor (never a
//! re-`stat` of the path) is verified to be a
//! character device before trusting a single byte. That is the same
//! open-once-then-`fstat`-the-fd shape [`crate::secret`] uses against TOCTOU.
//!
//! # The descriptor is close-on-exec
//!
//! Descriptors received over D-Bus `SCM_RIGHTS` need atomic close-on-exec at
//! the transport boundary; the workspace's pinned Linux zbus patch supplies
//! `MSG_CMSG_CLOEXEC` there. This module is not in that class: the source is
//! opened with [`std::fs::File::open`], and std sets `O_CLOEXEC`
//! unconditionally on every descriptor it creates (`library/std/src/sys/fs/
//! unix.rs`: `let flags = libc::O_CLOEXEC | opts.get_access_mode()? | ..`). The
//! file is also dropped before the call returns, so it is never live across a
//! spawn. Nothing here needs received-descriptor `FD_CLOEXEC` normalisation; recorded so
//! a later audit does not have to re-derive it.
//!
//! # What this does not claim
//!
//! It does not `mlock`, and the kernel CSPRNG's quality is the platform's
//! guarantee, not ours. It fails closed: an unreadable or wrong-typed
//! source is [`RandomError`], never a fabricated value.

use std::fmt;
use std::io::{self, Read};
use std::path::Path;

/// The kernel CSPRNG source. Non-blocking after the pool is initialised, which
/// on every supported target it is by the time a GUI process starts.
#[cfg(unix)]
const RANDOM_SOURCE: &str = "/dev/urandom";

/// Why an entropy draw was refused.
///
/// Every variant is a *refusal*, never a substituted value. A caller that
/// cannot proceed without the bytes must propagate the error and disable the
/// dependent capability — that is the whole point of making failure explicit.
#[derive(Debug)]
pub enum RandomError {
    /// The entropy source could not be opened.
    Open(io::Error),
    /// The opened descriptor is not a character device. `/dev/urandom` is a
    /// character device; anything else at that path (a planted regular file, a
    /// fifo, a directory) is refused rather than read, because its contents are
    /// attacker-chosen, not kernel entropy.
    NotCharDevice,
    /// `fstat` on the opened descriptor failed, so its type could not be
    /// verified. Fail closed rather than trust an unverified source.
    Fstat(io::Error),
    /// Reading from the source failed or reached EOF before the request was
    /// filled. The kernel CSPRNG never returns EOF, so a short read means the
    /// source is not what is required.
    Read(io::Error),
    /// Repeated draws collided against the set of already-issued identities more
    /// than [`MAX_COLLISION_RETRIES`] times. Astronomically unlikely for a
    /// full-width identity, so it signals a broken source (e.g. one stuck
    /// returning a constant), and is surfaced rather than looped on forever.
    Exhausted {
        /// How many attempts were made before giving up.
        attempts: u32,
    },
    /// No OS entropy source is reachable on this (non-Unix) target. There is no
    /// safe fallback, so the capability is simply unavailable and every caller
    /// fails closed.
    Unsupported,
}

impl fmt::Display for RandomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RandomError::Open(e) => write!(f, "could not open OS entropy source: {e}"),
            RandomError::NotCharDevice => {
                write!(f, "OS entropy source is not a character device")
            }
            RandomError::Fstat(e) => write!(f, "fstat on entropy source failed: {e}"),
            RandomError::Read(e) => write!(f, "reading OS entropy source failed: {e}"),
            RandomError::Exhausted { attempts } => write!(
                f,
                "could not mint a collision-free identity in {attempts} attempts \
                 (entropy source may be broken)"
            ),
            RandomError::Unsupported => {
                write!(f, "no OS entropy source is available on this platform")
            }
        }
    }
}

impl std::error::Error for RandomError {}

/// How many colliding draws [`mint_unique`] tolerates before failing closed.
///
/// For a full-width (>= 16-byte) identity a single collision is already beyond
/// astronomically unlikely; a run of them means the source is degenerate, not
/// unlucky. A small bound turns "generate until unique" — which loops forever
/// against a stuck source — into a prompt, typed failure.
pub const MAX_COLLISION_RETRIES: u32 = 16;

/// Fill `buf` entirely with fresh OS entropy, or return a typed refusal.
///
/// Opens `/dev/urandom` once, verifies on the opened descriptor that it is a
/// character device, then reads exactly `buf.len()` bytes. Any shortfall,
/// wrong type, or I/O error is a [`RandomError`]; the function never fabricates
/// bytes and never partially "succeeds" — on error the caller must treat the
/// whole buffer as unusable.
///
/// # Errors
///
/// A [`RandomError`] when the entropy source is not a character device or cannot fill `buf`.
#[cfg(unix)]
pub fn fill_random(buf: &mut [u8]) -> Result<(), RandomError> {
    fill_random_from(Path::new(RANDOM_SOURCE), buf)
}

/// Non-Unix has no reachable OS source within this crate's budget: fail closed.
#[cfg(not(unix))]
pub fn fill_random(_buf: &mut [u8]) -> Result<(), RandomError> {
    Err(RandomError::Unsupported)
}

/// The path-parameterised core, so tests can point the exact same read-and-fill
/// logic at a non-character-device (a regular file, a missing path) and prove
/// the refusal fires — without ever weakening the production entry point, which
/// hard-codes [`RANDOM_SOURCE`].
#[cfg(unix)]
fn fill_random_from(path: &Path, buf: &mut [u8]) -> Result<(), RandomError> {
    use std::os::unix::fs::FileTypeExt;

    if buf.is_empty() {
        return Ok(());
    }

    let mut file = std::fs::File::open(path).map_err(RandomError::Open)?;

    // Verify the OPENED descriptor's type via fstat, never a fresh stat of the
    // path — a character device is what the kernel CSPRNG presents, and a
    // planted regular file (whose bytes an attacker chose) must be refused.
    let meta = file.metadata().map_err(RandomError::Fstat)?;
    if !meta.file_type().is_char_device() {
        return Err(RandomError::NotCharDevice);
    }

    let mut filled = 0usize;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            // EOF from the CSPRNG is impossible; treat it as a broken source.
            Ok(0) => {
                return Err(RandomError::Read(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "entropy source returned EOF before the request was filled",
                )));
            }
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(RandomError::Read(error)),
        }
    }
    Ok(())
}

/// A fresh fixed-width array of OS entropy, or a typed refusal.
///
/// This is the shape every bearer identity wants: `random_array::<16>()` for a
/// session id, `random_array::<32>()` for a handshake nonce. It fills the whole
/// array from [`fill_random`], so a partial or failed draw yields no array at
/// all rather than a half-random one.
///
/// # Errors
///
/// As [`fill_random`].
pub fn random_array<const N: usize>() -> Result<[u8; N], RandomError> {
    let mut out = [0u8; N];
    fill_random(&mut out)?;
    Ok(out)
}

/// Mint an identity that is fresh and distinct from every one already in
/// use, with a bounded number of retries.
///
/// `generate` draws a candidate (typically via [`random_array`]); `is_taken`
/// reports whether a candidate collides with a live identity. The candidate is
/// returned on the first miss. After [`MAX_COLLISION_RETRIES`] consecutive
/// collisions it fails with [`RandomError::Exhausted`] rather than looping
/// forever — a run of collisions on a full-width identity means the source is
/// degenerate, and an infinite retry against a stuck source is itself a denial
/// of service. A `generate` that itself fails propagates that failure
/// unchanged; entropy failure never becomes a silent reuse.
///
/// # Errors
///
/// `generate`'s error, or a [`RandomError`] when every retry collides.
pub fn mint_unique<T, G, P>(mut generate: G, mut is_taken: P) -> Result<T, RandomError>
where
    G: FnMut() -> Result<T, RandomError>,
    P: FnMut(&T) -> bool,
{
    for _ in 0..MAX_COLLISION_RETRIES {
        let candidate = generate()?;
        if !is_taken(&candidate) {
            return Ok(candidate);
        }
    }
    Err(RandomError::Exhausted {
        attempts: MAX_COLLISION_RETRIES,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;

    use crate::test_scratch::ScratchDir;

    // ---- freshness / positive control -----------------------------------

    /// The production source yields a full array and two draws differ. This is
    /// the positive control: it proves the real `/dev/urandom` path works, so a
    /// later fail-closed assertion is not passing merely because everything
    /// errors. A fixed source makes the inequality fail.
    #[test]
    fn production_source_is_fresh_each_draw() {
        let a: [u8; 32] = random_array().expect("/dev/urandom must yield entropy");
        let b: [u8; 32] = random_array().expect("/dev/urandom must yield entropy");
        assert_ne!(
            a, b,
            "two 32-byte draws from the OS CSPRNG must differ; equal means a \
             fixed/stubbed source"
        );
        assert!(
            a.iter().any(|&x| x != 0),
            "a fresh draw must not be all-zero"
        );
    }

    // ---- fail-closed: wrong source type ----------------------------------

    /// A regular file planted where the entropy source should be is refused
    /// as `NotCharDevice`, never read. This is the anti-fabrication guard: the
    /// file's bytes are attacker-chosen, so trusting them would hand an attacker
    /// the nonce. Sabotage: delete the `is_char_device` check and this test
    /// reads the planted bytes and returns Ok, failing here.
    #[test]
    fn regular_file_source_is_refused_as_not_char_device() {
        let scratch = ScratchDir::new("enzui-random-planted");
        let path = scratch.join("source");
        {
            let mut f = std::fs::File::create(&path).expect("create");
            f.write_all(&[0x41u8; 64]).expect("write");
            f.flush().expect("flush");
        }
        let mut buf = [0u8; 32];
        let got = fill_random_from(&path, &mut buf);
        let _ = std::fs::remove_file(&path);
        assert!(
            matches!(got, Err(RandomError::NotCharDevice)),
            "a planted regular file must be refused, got {got:?}"
        );
        assert_eq!(
            buf, [0u8; 32],
            "the buffer must be left untouched when the source is refused"
        );
    }

    /// A missing source is an `Open` refusal, not a fabricated fill.
    #[test]
    fn missing_source_fails_closed() {
        let scratch = ScratchDir::new("enzui-random-gone");
        let missing = scratch.join("missing");
        assert!(!missing.exists());
        let mut buf = [0u8; 16];
        let got = fill_random_from(&missing, &mut buf);
        assert!(
            matches!(got, Err(RandomError::Open(_))),
            "a missing entropy source must fail closed, got {got:?}"
        );
    }

    // ---- bounded collision / exhaustion ----------------------------------
}
