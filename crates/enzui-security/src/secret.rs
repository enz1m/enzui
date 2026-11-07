//! Credential-file loader: securely open credential files
//! once with no-follow, descriptor owner/type/mode checks.
//!
//! # Two stages
//!
//! Stage 1 is the secure *open + verify + read* of a credential file: the file
//! is opened once, with the final path component refusing to follow a symlink,
//! and every subsequent check is made against the file descriptor already held,
//! never against the path.
//!
//! Stage 2 keeps the loaded bytes from outliving their use. [`load_secret`]
//! returns a [`Secret`] rather than a bare `Vec<u8>`: it wipes its buffer on
//! `Drop`, redacts itself in `Debug`/`Display`, and refuses to implement
//! `Clone` (see [`Secret`] for the rationale of each). Timeouts and transport
//! are not this module's concern.
//!
//! # Why the fd, and never the path
//!
//! The whole reason this module exists rather than a plain [`std::fs::read`]
//! call is TOCTOU. A `fs::read` that re-stats the path to check ownership or
//! mode opens a window: an attacker swaps the entry between the stat and the
//! open (or between two stats) and the check describes a different object than
//! the one that is read. So the loader:
//!
//! 1. open with `O_NOFOLLOW | O_NONBLOCK` — the kernel refuses a symlink at
//!    the final component atomically, with no decide-then-open gap, and the
//!    open itself can never wait (see below);
//! 2. `fstat` the *descriptor* via [`File::metadata`] — this describes the
//!    exact object the fd refers to and cannot be swapped out underneath;
//! 3. read from that same descriptor.
//!
//! # Why the open must also be non-blocking
//!
//! Step 2 is what refuses a fifo, socket or device
//! ([`SecretError::NotRegularFile`]) — but it only runs if step 1 *returns*. A
//! read-only `open(2)` of a FIFO with no writer blocks inside the open, so
//! a same-uid actor (or a stale named pipe left where a credential file used to
//! be) could park the loader in the kernel forever and the descriptor check
//! that exists to refuse it would never be reached. That is a hang, not a
//! refusal, and it happens on the thread a caller drives its credential load
//! from. The open therefore also carries `O_NONBLOCK`
//! ([`crate::platform::libc_o_nonblock`]), which is inert for the regular file
//! this loader accepts and turns the planted-FIFO case into the documented
//! [`SecretError::NotRegularFile`]. Because the non-regular refusal happens
//! before the first read, no `EAGAIN`/`WouldBlock` read path is reachable.
//!
//! Neither flag constant is re-derived here. A near-duplicate platform table
//! with a subtly wrong symlink or non-blocking value is the worst possible
//! outcome, because those flags *are* the security-relevant constants, so both
//! come from the single [`crate::platform`] table.

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::Arc;

/// Maximum accepted credential-file size (1 MiB).
///
/// Credential material is read into one fixed allocation sized from the
/// already-open descriptor. A finite cap makes that allocation bounded and
/// prevents a changing or malicious file from forcing unbounded plaintext
/// growth.
pub const MAX_SECRET_BYTES: u64 = 1024 * 1024;

/// Owned credential bytes that are wiped from memory when dropped and never
/// reveal their contents (or length) when formatted.
///
/// [`load_secret`] returns this instead of a bare `Vec<u8>` for three reasons,
/// each of which is a way a plain `Vec<u8>` leaks the credential:
///
/// 1. **`Drop` wipes the buffer.** A `Vec<u8>` dropped normally hands its
///    allocation back to the allocator *without clearing it*, so the credential
///    stays legible in freed heap until something else happens to reuse that
///    memory. [`Secret`]'s [`Drop`] overwrites every byte first (see the wipe
///    section below for why that is harder than a `for` loop).
/// 2. **`Debug`/`Display` redact.** A secret that prints itself into a log line
///    or a panic message has leaked. Both impls emit a fixed `Secret([redacted])`
///    and reveal neither the bytes nor the length — the length alone can be
///    a meaningful oracle (it distinguishes a 16-byte token from a 512-byte key,
///    and narrows a brute force), so it is withheld too. If a caller genuinely
///    needs the length it must ask via [`Secret::len`], which is an explicit,
///    greppable call rather than an accident of a `{:?}` in a format string.
/// 3. **No `Clone`.** [`Secret`] deliberately does not implement [`Clone`].
///    A `Clone` would mint a second copy of the credential with its own
///    lifetime and its own drop site, doubling the window in which the bytes
///    live in memory and making it easy to leave one copy un-wiped. Code that
///    truly needs to move the secret elsewhere moves the whole `Secret` (its
///    `Drop` follows it); code that needs to *read* it borrows [`Secret::as_bytes`].
///
/// # What this does not guarantee
///
/// The wipe is best-effort defence-in-depth, not a proof the secret is
/// unrecoverable:
///
/// - The loader reads directly into one bounded, non-growing allocation that is
///   a `Secret` before the first byte arrives, including on read-error paths.
///   A caller can only borrow that allocation; there is no public growth API.
/// - It does nothing about copies the OS made: pages swapped to disk, a core
///   dump, `ptrace`, or an attacker with read access to this process's memory
///   while it is still live. Locking pages (`mlock`) and disabling core dumps
///   are out of scope and would need the `libc` dependency this crate refuses.
/// - The compiler may keep the value in a register that this wipe never sees.
pub struct Secret {
    bytes: Vec<u8>,
}

/// Immutable UTF-8 secret text with shared, wipe-on-last-drop ownership.
///
/// Cloning this handle clones only an [`Arc`], never the plaintext allocation.
/// The allocation remains readable through an explicit short-lived borrow and
/// is wiped by [`Secret`] when the final handle is dropped. Formatting is
/// always redacted and does not reveal even the byte length.
pub struct SecretText {
    secret: Arc<Secret>,
}

impl SecretText {
    /// Take ownership of a string without copying its plaintext bytes.
    #[must_use]
    pub fn new(value: String) -> Self {
        #[cfg(any(test, feature = "secret-observer"))]
        observer::note_secret_text_allocation();
        Self {
            secret: Arc::new(Secret::from_string(value)),
        }
    }

    /// Borrow the plaintext for the duration of this handle's borrow.
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: a `SecretText` is built from a UTF-8 `String`"
    )]
    #[must_use]
    pub fn expose_secret(&self) -> &str {
        std::str::from_utf8(self.secret.expose_secret())
            .expect("SecretText bytes originate from an owned UTF-8 String")
    }

    /// Whether the protected text is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.secret.is_empty()
    }

    /// Return an allocation identity for lifecycle tests.
    ///
    /// This is intentionally unavailable unless the test-only observer feature
    /// is enabled. The address is never dereferenced and remains valid as an
    /// identity only while this handle is live.
    #[cfg(any(test, feature = "secret-observer"))]
    #[doc(hidden)]
    #[must_use]
    pub fn observed_allocation_id(&self) -> usize {
        Arc::as_ptr(&self.secret).cast::<()>() as usize
    }
}

impl Clone for SecretText {
    fn clone(&self) -> Self {
        Self {
            secret: Arc::clone(&self.secret),
        }
    }
}

impl PartialEq for SecretText {
    fn eq(&self, other: &Self) -> bool {
        self.secret.as_bytes() == other.secret.as_bytes()
    }
}

impl Eq for SecretText {}

impl fmt::Debug for SecretText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretText([redacted])")
    }
}

impl fmt::Display for SecretText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretText([redacted])")
    }
}

impl Secret {
    /// Wrap owned bytes as a [`Secret`]. Takes ownership so there is exactly one
    /// copy whose lifetime the [`Secret`] controls; the caller's binding is
    /// moved, not copied.
    pub fn new(bytes: Vec<u8>) -> Self {
        Secret { bytes }
    }

    /// Borrow the raw credential bytes. Short-lived by construction: the borrow
    /// cannot outlive the [`Secret`], so it cannot survive the wipe.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Alias used by keyring APIs to make plaintext exposure explicit.
    pub fn expose_secret(&self) -> &[u8] {
        self.as_bytes()
    }

    /// Construct from an owned string, transferring its bytes into wipe-on-drop custody.
    pub fn from_string(value: String) -> Self {
        Self::new(value.into_bytes())
    }

    /// The length in bytes. Exposed as an explicit call (never via `Debug`) so a
    /// length disclosure is a decision in the source, not a side effect of
    /// logging.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Whether the credential is empty.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

/// Overwrite every byte of `buf` with zero using writes the optimiser is not
/// permitted to elide, and report how many bytes were wiped.
///
/// Uses volatile writes and compiler fences to retain the wipe. Only this
/// slice is cleared; copies elsewhere are unaffected. Prefer [`Secret`] when
/// owning the whole allocation, and use this helper for transient buffers that
/// must be cleared on every exit path.
pub fn wipe_bytes(buf: &mut [u8]) -> usize {
    let wiped = wipe_volatile(buf);
    #[cfg(any(test, feature = "secret-observer"))]
    observer::note_wiped(wiped, buf.iter().filter(|byte| **byte == 0).count());
    wiped
}

#[allow(
    unsafe_code,
    reason = "volatile writes, so the wipe cannot be optimised away"
)]
fn wipe_volatile(buf: &mut [u8]) -> usize {
    use std::sync::atomic::{Ordering, compiler_fence};

    compiler_fence(Ordering::SeqCst);
    let ptr = buf.as_mut_ptr();
    let len = buf.len();
    for i in 0..len {
        // SAFETY: `ptr` is valid for `len` bytes (it is `buf`'s own storage),
        // `i < len`, and `u8` has no alignment requirement, so `ptr.add(i)` is a
        // valid, aligned, writable location for a single `u8`.
        unsafe {
            std::ptr::write_volatile(ptr.add(i), 0u8);
        }
    }
    compiler_fence(Ordering::SeqCst);
    len
}

impl Drop for Secret {
    fn drop(&mut self) {
        let _ = wipe_bytes(&mut self.bytes);
    }
}

/// `Debug` redacts: no bytes, no length, no prefix. See [`Secret`] point 2 for
/// why the length is withheld too. This is the impl that stops a stray `{:?}`
/// in a log macro or a `#[derive(Debug)]` on an enclosing struct from leaking
/// the credential.
impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

/// `Display` redacts identically, so a secret accidentally interpolated with
/// `{}` (e.g. into a panic message or a user-facing error) is redacted too.
impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

/// Sound, test-only observation of secret lifecycle post-conditions.
///
/// Counts inspect storage immediately after wiping, while it remains live.
/// Tests never retain or dereference a pointer after free. Thread-local counters
/// keep focused tests from racing each other.
#[cfg(any(test, feature = "secret-observer"))]
#[doc(hidden)]
pub mod observer {
    use std::cell::Cell;

    /// A snapshot of lifecycle counters for the current test thread.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Snapshot {
        /// Bytes the wipe primitive reported processing.
        pub wiped_bytes: usize,
        /// Bytes observed as zero after the wipe completed.
        pub zeroed_bytes: usize,
        /// Distinct protected text allocations constructed.
        pub secret_text_allocations: usize,
    }

    thread_local! {
        static WIPED_BYTES: Cell<usize> = const { Cell::new(0) };
        static ZEROED_BYTES: Cell<usize> = const { Cell::new(0) };
        static SECRET_TEXT_ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    }

    pub(crate) fn note_wiped(wiped: usize, zeroed: usize) {
        WIPED_BYTES.with(|count| count.set(count.get() + wiped));
        ZEROED_BYTES.with(|count| count.set(count.get() + zeroed));
    }

    pub(crate) fn note_secret_text_allocation() {
        SECRET_TEXT_ALLOCATIONS.with(|count| count.set(count.get() + 1));
    }

    /// Reset this thread's counters before a focused lifecycle assertion.
    pub fn reset() {
        WIPED_BYTES.with(|count| count.set(0));
        ZEROED_BYTES.with(|count| count.set(0));
        SECRET_TEXT_ALLOCATIONS.with(|count| count.set(0));
    }

    /// Read this thread's counters.
    #[must_use]
    pub fn snapshot() -> Snapshot {
        Snapshot {
            wiped_bytes: WIPED_BYTES.with(Cell::get),
            zeroed_bytes: ZEROED_BYTES.with(Cell::get),
            secret_text_allocations: SECRET_TEXT_ALLOCATIONS.with(Cell::get),
        }
    }
}

/// Why a credential load was refused.
///
/// Each variant is distinct on purpose: a caller diagnosing a rejected
/// credential needs to tell "you left it world-readable" (`InsecureMode`) from
/// "that is a symlink" (`Symlink`) from "that file is owned by someone else"
/// (`WrongOwner`), because those want different remediation messages. Do not
/// collapse them.
#[derive(Debug)]
pub enum SecretError {
    /// The final path component was a symlink and `O_NOFOLLOW` refused it, or
    /// the object could not be opened at all. Carries the underlying error kind
    /// for a log (never the path or any secret material).
    Symlink(io::Error),
    /// The descriptor does not refer to a regular file — it is a directory,
    /// fifo, socket, block/char device, etc.
    NotRegularFile,
    /// The file is owned by a uid other than the current process's uid.
    WrongOwner {
        /// The uid that owns the file.
        file_uid: u32,
        /// The current process's uid.
        current_uid: u32,
    },
    /// The current process's uid could not be determined, so ownership could
    /// not be verified. Fail closed rather than skip the check.
    OwnerUnverifiable,
    /// The file's mode has group/other permission bits or a special bit
    /// (setuid/setgid/sticky) set — `mode & 0o7077 != 0`. Either grants access
    /// to someone other than the owner, or (for the special bits) is a sign the
    /// credential file has been tampered with; a credential file must be a
    /// plain owner-only regular file.
    InsecureMode {
        /// The permission and special bits actually present (`mode & 0o7777`).
        mode: u32,
    },
    /// `fstat` on the opened descriptor failed.
    Fstat(io::Error),
    /// The descriptor's size exceeds [`MAX_SECRET_BYTES`].
    TooLarge {
        /// Size reported by the opened descriptor.
        size: u64,
        /// Maximum accepted size.
        max: u64,
    },
    /// The regular file grew after its descriptor metadata was read. The
    /// fixed allocation is wiped and the changing input is refused rather than
    /// truncated or grown.
    LengthChanged {
        /// Size reported before reading began.
        expected: u64,
    },
    /// Reading the file contents failed.
    Read(io::Error),
}

impl fmt::Display for SecretError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SecretError::Symlink(e) => {
                write!(
                    f,
                    "refused to open credential file (symlink or open error): {e}"
                )
            }
            SecretError::NotRegularFile => {
                write!(f, "credential path is not a regular file")
            }
            SecretError::WrongOwner {
                file_uid,
                current_uid,
            } => write!(
                f,
                "credential file is owned by uid {file_uid}, not the current uid {current_uid}"
            ),
            SecretError::OwnerUnverifiable => {
                write!(
                    f,
                    "could not determine current uid to verify credential ownership"
                )
            }
            SecretError::InsecureMode { mode } => write!(
                f,
                "credential file mode {mode:04o} grants group/other or special-bit access (must be 0600)"
            ),
            SecretError::Fstat(e) => write!(f, "fstat on credential descriptor failed: {e}"),
            SecretError::TooLarge { size, max } => write!(
                f,
                "credential file is {size} bytes, exceeding the {max}-byte limit"
            ),
            SecretError::LengthChanged { expected } => write!(
                f,
                "credential file grew while being read (initial size {expected} bytes)"
            ),
            SecretError::Read(e) => write!(f, "reading credential file failed: {e}"),
        }
    }
}

impl std::error::Error for SecretError {}

/// The current process's owning uid, derived without `libc` or `unsafe`.
///
/// Delegates to [`crate::runtime_dir::current_uid`], which prefers a
/// zero-write `/proc/self` stat and falls back to an `O_EXCL` probe file only
/// off Linux. Writing into the world-writable [`std::env::temp_dir`] on every
/// credential load is deliberately avoided: on Linux the probe touches no
/// filesystem at all, and where it must, the `O_EXCL` open refuses a
/// pre-created path rather than reading back an attacker's uid.
///
/// `None` still means owner verification unavailable — the property this
/// module is proudest of. [`load_secret`] maps it to
/// [`SecretError::OwnerUnverifiable`] and fails closed; it never treats it as
/// "the owner check passed".
#[cfg(unix)]
fn current_uid() -> Option<u32> {
    crate::runtime_dir::current_uid()
}

/// Open `path` for reading, refusing to follow a symlink at the final
/// component and never waiting for a peer.
///
/// Mirrors `atomic_state::open_nofollow` but is kept local so the two loaders
/// stay independent; the security-relevant `O_NOFOLLOW` and `O_NONBLOCK` values
/// are *shared* via [`crate::platform`] rather than re-derived.
///
/// `O_NONBLOCK` is load-bearing, not cosmetic: without it a writer-less FIFO
/// planted at `path` blocks inside `open(2)`, so [`load_secret`]'s
/// descriptor-type check never runs and the caller hangs instead of receiving
/// [`SecretError::NotRegularFile`]. It is inert for the regular file this
/// loader accepts, and the non-regular refusal runs before the first read, so
/// no non-blocking read path is reachable.
#[cfg(unix)]
fn open_nofollow(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(crate::platform::libc_o_nofollow() | crate::platform::libc_o_nonblock())
        .open(path)
}

#[cfg(not(unix))]
fn open_nofollow(path: &Path) -> io::Result<File> {
    File::open(path)
}

/// Read an already-verified descriptor into one bounded, non-growing
/// allocation that is under [`Secret`] custody before the first byte arrives.
///
/// One extra byte is allocated as a growth probe. Exact-length input reaches
/// EOF with that byte still zero; a file that grew after `metadata()` fills it
/// and is refused. Every return after allocation — short read, I/O error, or
/// growth — drops `secret`, whose `Drop` wipes the whole allocation.
fn read_secret(reader: &mut impl Read, expected_len: u64) -> Result<Secret, SecretError> {
    if expected_len > MAX_SECRET_BYTES {
        return Err(SecretError::TooLarge {
            size: expected_len,
            max: MAX_SECRET_BYTES,
        });
    }
    let expected = usize::try_from(expected_len).map_err(|_| SecretError::TooLarge {
        size: expected_len,
        max: MAX_SECRET_BYTES,
    })?;
    let allocation_len = expected
        .checked_add(1)
        .expect("MAX_SECRET_BYTES leaves room for a one-byte growth probe");
    let mut secret = Secret::new(vec![0; allocation_len]);
    let mut filled = 0usize;

    while filled < allocation_len {
        match reader.read(&mut secret.bytes[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(SecretError::Read(error)),
        }
    }

    if filled < expected {
        return Err(SecretError::Read(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!(
                "credential file shrank while being read: expected {expected} bytes, read {filled}"
            ),
        )));
    }
    if filled > expected {
        return Err(SecretError::LengthChanged {
            expected: expected_len,
        });
    }

    // The growth probe stayed zero and never held plaintext. Remove it from the
    // logical secret without reallocating; capacity remains fixed.
    secret.bytes.truncate(expected);
    Ok(secret)
}

/// Securely load a credential file and return its bytes as a [`Secret`].
///
/// The returned [`Secret`] wipes its buffer on `Drop` and redacts itself when
/// formatted; see that type for the guarantees and their limits.
///
/// Opens the file once with `O_NOFOLLOW`, then verifies the opened
/// descriptor (not the path) against every rule below before reading. Refuses
/// with a distinct [`SecretError`] unless all of:
///
/// - the descriptor refers to a regular file ([`SecretError::NotRegularFile`]);
/// - it is owned by the current uid ([`SecretError::WrongOwner`]);
/// - its mode has no group, other, or special (setuid/setgid/sticky) bits
///   — `mode & 0o7077 == 0` ([`SecretError::InsecureMode`]).
///
/// The mask is `0o7077`, not `0o077`: the special bits live in `0o7000` and a
/// bare `0o077` mask lets `0o4600` (setuid), `0o2600` (setgid) and `0o1600`
/// (sticky) through. Their runtime effect on a non-executable regular file is
/// negligible on Linux, but a credential file carrying setuid is a sign
/// something is wrong regardless of what the kernel does with it, so it is
/// refused. Owner read/write/execute bits are intentionally *not* masked here:
/// `0o000` and `0o200` (and `0o100`) remain accepted — a missing read bit
/// simply fails later at the read, and the security check guards *access*, not
/// *readability*.
///
/// A symlink at the final component is refused by the kernel via `O_NOFOLLOW`
/// and surfaces as [`SecretError::Symlink`]. A FIFO is refused as
/// [`SecretError::NotRegularFile`] rather than blocking the caller, because the
/// open also carries `O_NONBLOCK`; see the [module docs](self).
///
/// # Errors
///
/// A [`SecretError`] when the path is a symlink or not a regular file, its owner cannot be verified
/// or is someone else, or its mode is too open.
#[cfg(unix)]
pub fn load_secret(path: &Path) -> Result<Secret, SecretError> {
    use std::os::unix::fs::MetadataExt;

    // Open once. A symlink at the final component (ELOOP) or any other open
    // failure is refused here, atomically, with no path re-resolution window.
    let mut file = open_nofollow(path).map_err(SecretError::Symlink)?;

    // Every check below is against the OPENED FD via `fstat`, never a fresh
    // `stat` of the path — that is the entire reason this module exists.
    let meta = file.metadata().map_err(SecretError::Fstat)?;

    // 1. Regular file only: not a fifo, device, socket or directory.
    if !meta.is_file() {
        return Err(SecretError::NotRegularFile);
    }

    // 2. Owned by the current uid. Fail closed if we cannot establish our own
    //    uid rather than skipping the check.
    let file_uid = meta.uid();
    let current_uid = current_uid().ok_or(SecretError::OwnerUnverifiable)?;
    if file_uid != current_uid {
        return Err(SecretError::WrongOwner {
            file_uid,
            current_uid,
        });
    }

    // 3. No group, other, or special (setuid/setgid/sticky) permission bits.
    //    The special bits live in 0o7000, so the mask is 0o7077, not 0o077:
    //    0o4600/0o2600/0o1600 would otherwise slip through. Owner r/w/x bits
    //    are intentionally not masked (0o000/0o200/0o100 stay accepted).
    let mode = meta.mode() & 0o7777;
    if mode & 0o7077 != 0 {
        return Err(SecretError::InsecureMode { mode });
    }

    // All checks passed on the fd we hold; read from that same descriptor.
    read_secret(&mut file, meta.len())
}

/// Non-Unix fallback: no `O_NOFOLLOW`, ownership or mode model is reachable from
/// `std`, so only the regular-file check is enforced. Credential loading on
/// non-Unix platforms is out of scope for stage 1.
#[cfg(not(unix))]
pub fn load_secret(path: &Path) -> Result<Secret, SecretError> {
    let mut file = open_nofollow(path).map_err(SecretError::Symlink)?;
    let meta = file.metadata().map_err(SecretError::Fstat)?;
    if !meta.is_file() {
        return Err(SecretError::NotRegularFile);
    }
    read_secret(&mut file, meta.len())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    // ---- Drop-observation hook -------------------------------------------
    //
    // We observe the wipe *through an actual `Drop`* without ever reading freed
    // memory (which is UB). `Secret::drop` calls `note_wiped(n)` with the byte
    // count it wiped; a test resets the counter, drops a `Secret` of known
    // length, and asserts the counter advanced by exactly that many bytes. If
    // the `Drop` impl is removed the counter never moves and the test fails.
    //
    // Two counters, and the second is the one with teeth. `WIPED_BYTES` is the
    // length `Drop` says it covered; `ZEROED_BYTES` is how many bytes were
    // *observably zero* afterwards, counted inside `Drop` while the buffer is
    // still owned. Asserting only the former is vacuous — a `Drop` that does
    // nothing but report `self.bytes.len()` satisfies it, which is exactly the
    // sabotage these assertions are split to catch.
    //
    // Honest limitation: neither counter reads the post-free allocation, so
    // neither can catch a hypothetical bug where the write succeeds on a live
    // buffer but the pages are left intact after free — not observable in safe
    // Rust, and reaching for it via a raw pointer to the freed `Vec` is UB.
    fn reset_wiped() {
        observer::reset();
    }

    fn wiped_total() -> usize {
        observer::snapshot().wiped_bytes
    }

    /// Bytes that were actually zero at the end of `Drop`. This is the
    /// post-condition; `wiped_total` is only the claim.
    fn zeroed_total() -> usize {
        observer::snapshot().zeroed_bytes
    }

    /// A unique temp path under `/tmp` (no `tempfile` dev-dependency in this
    /// crate). Each call produces a distinct path via a pid + counter suffix.
    fn unique_path(tag: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("enzui-secret-{tag}-{pid}-{n}"))
    }

    /// Write `bytes` to a fresh file and set its mode to `mode`.
    fn write_with_mode(path: &Path, bytes: &[u8], mode: u32) {
        let mut f = File::create(path).expect("create");
        f.write_all(bytes).expect("write");
        f.flush().expect("flush");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
    }

    // ---- Stage 2: Drop wiping --------------------------------------------

    /// `Drop` runs the wipe over every byte of the buffer, observed through
    /// an actual drop via the byte counter. Removing the `Drop` impl makes the
    /// counter stay at 0, so this test fails.
    /// requires. A `.wipe()`-style explicit call would *not* exercise this,
    /// which is why the observation is bound to the real drop glue here.
    #[test]
    fn drop_wipes_every_byte() {
        reset_wiped();
        let n = 37usize;
        let s = Secret::new(vec![0xABu8; n]);
        assert_eq!(wiped_total(), 0, "counter must not move before the drop");
        drop(s);
        assert_eq!(
            wiped_total(),
            n,
            "Drop must wipe exactly the {n} bytes of the buffer; a missing or \
             partial Drop impl leaves this short"
        );
        // The claim above is satisfiable without writing anything. This is the
        // post-condition: every byte was observably zero when Drop finished.
        assert_eq!(
            zeroed_total(),
            n,
            "all {n} bytes must actually be zero after Drop, not merely reported"
        );
    }

    /// A `Secret` obtained from `load_secret` wipes on drop too — exercises the
    /// real production path, not just a hand-built `Secret`.
    #[test]
    fn loaded_secret_wipes_on_drop() {
        let path = unique_path("wipe");
        write_with_mode(&path, b"correct horse battery staple", 0o600);
        reset_wiped();
        let s = load_secret(&path).expect("0600 file must load");
        let _ = std::fs::remove_file(&path);
        let expected = s.len();
        assert!(expected >= 28, "sanity: known payload length");
        drop(s);
        assert_eq!(
            wiped_total(),
            expected,
            "the Secret returned by load_secret must wipe all {expected} bytes on drop"
        );
        assert_eq!(
            zeroed_total(),
            expected,
            "all {expected} loaded bytes must actually be zero after Drop"
        );
    }

    #[test]
    fn file_growth_is_refused_and_the_probe_byte_is_wiped() {
        reset_wiped();
        let mut reader = io::Cursor::new(b"ABCDE");
        let result = read_secret(&mut reader, 4);
        assert!(
            matches!(result, Err(SecretError::LengthChanged { expected: 4 })),
            "the fifth byte must be detected, not truncated: {result:?}"
        );
        assert_eq!(wiped_total(), 5, "four expected bytes plus probe wiped");
        assert_eq!(
            zeroed_total(),
            5,
            "growth path wipes all five plaintext bytes"
        );
    }

    // ---- Stage 2: redaction ----------------------------------------------
}
