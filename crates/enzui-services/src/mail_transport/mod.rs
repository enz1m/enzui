//! Safe libcurl mail transport options for SMTP and IMAP.
//!
//! Exposes `CURLOPT_USE_SSL`, `CURLOPT_MAIL_FROM`, and `CURLOPT_MAIL_RCPT`
//! through a safe API. Requires the `mail-transport` feature.
//!
//! [`MailTransport`] exclusively borrows the `Easy` handle for its lifetime.
//! `CURLOPT_MAIL_RCPT` stores the slist pointer by reference inside libcurl,
//! so the guard clears or resets the handle before freeing the list on drop.
//! All addresses are refused if they contain ASCII control characters
//! (CR, LF, NUL, TAB, etc.) to prevent SMTP command injection; non-ASCII
//! bytes are permitted for SMTPUTF8 mailboxes.
//! Call [`MailTransport::perform`] to transfer, or [`MailTransport::release`]
//! to detach and reclaim the handle.
//!
//! ```compile_fail
//! use enzui_services::mail_transport::{MailTransport, RecipientList};
//! let mut easy = curl::easy::Easy::new();
//! let guard = MailTransport::new(&mut easy);
//! let _ = easy.url("smtp://localhost"); // ERROR: easy is exclusively borrowed
//! drop(guard);
//! ```

#[allow(unsafe_code, reason = "libcurl is called through FFI")]
mod ffi;

#[cfg(test)]
mod tests;

/// Authenticates an SMTP or IMAP handle with an OAuth 2.0 access token (what
/// Gmail and Outlook require) instead of a password: `CURLOPT_USERNAME` and
/// `CURLOPT_XOAUTH2_BEARER`. libcurl then uses SASL `OAUTHBEARER` or
/// `XOAUTH2`, whichever the server offers. The token comes from
/// `enzui_services::oauth`; refresh it before it expires. Require TLS
/// ([`MailTransport::set_require_ssl`]); the token is a bearer secret.
///
/// # Errors
///
/// A [`MailTransportError`] when the user or token is empty or holds a control character, or
/// libcurl refuses an option.
pub fn set_oauth2_bearer(
    easy: &mut curl::easy::Easy,
    user: &str,
    access_token: &enzui_security::secret::SecretText,
) -> Result<(), MailTransportError> {
    let token = access_token.expose_secret();
    let invalid = |text: &str| text.is_empty() || text.bytes().any(|b| b.is_ascii_control());
    if invalid(user) || invalid(token) {
        return Err(MailTransportError {
            kind: MailTransportErrorKind::InvalidCredential,
        });
    }
    // NUL is a control byte, so both convert.
    let refused = |_| MailTransportError {
        kind: MailTransportErrorKind::InvalidCredential,
    };
    let user = CString::new(user).map_err(refused)?;
    let token = CString::new(token).map_err(refused)?;
    ffi::set_string_option(easy, curl_sys::CURLOPT_USERNAME, "CURLOPT_USERNAME", &user)?;
    let result = ffi::set_string_option(
        easy,
        ffi::CURLOPT_XOAUTH2_BEARER,
        "CURLOPT_XOAUTH2_BEARER",
        &token,
    );
    let mut bytes = token.into_bytes();
    enzui_security::secret::wipe_bytes(&mut bytes);
    result
}

use std::ffi::CString;
use std::fmt;

/// What ended a [`wait_for_activity`] call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Activity {
    /// The connection has bytes to read, or was closed.
    Readable,
    /// The `wake` socket became readable.
    Woken,
    /// `timeout` passed with neither.
    TimedOut,
}

/// Block until a `CONNECT_ONLY` handle's connection has bytes to read, `wake`
/// becomes readable, or `timeout` passes, without waking in between: what a
/// long-lived IMAP `IDLE` needs.
///
/// Read with `Easy::recv` until it reports `EAGAIN` before calling this: TLS
/// can hold decrypted bytes the socket no longer shows as readable. Write a
/// byte to the other end of `wake` to end the wait from another thread.
///
/// # Errors
///
/// [`MailTransportErrorKind::NoActiveSocket`] when the handle holds no
/// connection, [`MailTransportErrorKind::Poll`] when `poll(2)` fails.
#[cfg(unix)]
pub fn wait_for_activity(
    easy: &curl::easy::Easy,
    wake: &std::os::unix::net::UnixStream,
    timeout: std::time::Duration,
) -> Result<Activity, MailTransportError> {
    use std::os::fd::AsRawFd;
    let socket = ffi::active_socket(easy).ok_or(MailTransportError {
        kind: MailTransportErrorKind::NoActiveSocket,
    })?;
    let millis = i32::try_from(timeout.as_millis()).unwrap_or(i32::MAX);
    loop {
        match ffi::poll_readable([socket, wake.as_raw_fd()], millis) {
            Ok([true, _]) => return Ok(Activity::Readable),
            Ok([false, true]) => return Ok(Activity::Woken),
            Ok([false, false]) => return Ok(Activity::TimedOut),
            Err(errno) if errno == libc::EINTR => continue,
            Err(errno) => {
                return Err(MailTransportError {
                    kind: MailTransportErrorKind::Poll(errno),
                });
            }
        }
    }
}

const MAX_RECIPIENTS: usize = 10_000;
const MAX_RECIPIENT_BYTES: usize = 512 * 1024;
// RFC 5321 §4.5.3.1: 256 octets including `<>`, so 254 for the bare address.
const MAX_ADDRESS_LEN: usize = 254;

// ASCII control bytes break a single SMTP command boundary;
// non-ASCII bytes are permitted for SMTPUTF8 mailboxes.
fn check_address(address: &str) -> Result<(), MailTransportError> {
    if address.len() > MAX_ADDRESS_LEN {
        return Err(MailTransportError {
            kind: MailTransportErrorKind::AddressTooLong,
        });
    }
    if address.bytes().any(|b| b.is_ascii_control()) {
        return Err(MailTransportError {
            kind: MailTransportErrorKind::ControlCharInAddress,
        });
    }
    Ok(())
}

std::thread_local! {
    #[cfg(test)]
    static FORCE_CLEAR_FAIL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn try_clear_mail_rcpt(easy: &mut curl::easy::Easy) -> Result<(), MailTransportError> {
    #[cfg(test)]
    {
        let forced = FORCE_CLEAR_FAIL.with(std::cell::Cell::get);
        if forced {
            return Err(MailTransportError {
                kind: MailTransportErrorKind::CurlSetopt {
                    option: "CURLOPT_MAIL_RCPT",
                    curl_code: 99,
                },
            });
        }
    }
    ffi::clear_mail_rcpt(easy)
}

/// A typed error from a mail-transport operation.
///
/// Carries the option name and libcurl code but never an address or
/// credential value.
#[derive(Debug)]
pub struct MailTransportError {
    kind: MailTransportErrorKind,
}

/// The classification of a [`MailTransportError`].
#[derive(Debug)]
pub enum MailTransportErrorKind {
    /// The recipient list was empty.
    EmptyRecipientList,
    /// The count or aggregate address-byte limit of [`RecipientList`] was reached.
    RecipientLimitReached,
    /// The address exceeds 254 bytes (RFC 5321 §4.5.3.1).
    AddressTooLong,
    /// The address contains an ASCII control character (CR, LF, NUL, etc.).
    ControlCharInAddress,
    /// `curl_slist_append` returned null (allocation failure).
    RecipientAppendFailed,
    /// An OAuth user name or token is empty or contains an ASCII control
    /// character (it would break the SASL exchange).
    InvalidCredential,
    /// `curl_easy_setopt` returned a nonzero code.
    CurlSetopt {
        /// The `CURLOPT_*` constant that failed.
        option: &'static str,
        /// The libcurl error code.
        curl_code: u32,
    },
    /// `Easy::perform` returned an error.
    Perform(curl::Error),
    /// The handle holds no open connection to wait on.
    NoActiveSocket,
    /// `poll(2)` failed with this `errno`.
    Poll(i32),
}

impl fmt::Display for MailTransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            MailTransportErrorKind::EmptyRecipientList => {
                write!(f, "mail transport: recipient list is empty")
            }
            MailTransportErrorKind::RecipientLimitReached => {
                write!(f, "mail transport: recipient count or byte limit reached")
            }
            MailTransportErrorKind::AddressTooLong => {
                write!(f, "mail transport: address exceeds 254-byte limit")
            }
            MailTransportErrorKind::ControlCharInAddress => {
                write!(
                    f,
                    "mail transport: address contains ASCII control character"
                )
            }
            MailTransportErrorKind::InvalidCredential => {
                write!(
                    f,
                    "mail transport: OAuth user or token is empty or has a control character"
                )
            }
            MailTransportErrorKind::RecipientAppendFailed => {
                write!(f, "mail transport: curl_slist_append failed (allocation)")
            }
            MailTransportErrorKind::CurlSetopt { option, curl_code } => {
                write!(
                    f,
                    "mail transport: curl_easy_setopt({option}) failed with code {curl_code}"
                )
            }
            MailTransportErrorKind::NoActiveSocket => {
                write!(f, "mail transport: the handle holds no open connection")
            }
            MailTransportErrorKind::Poll(errno) => {
                write!(f, "mail transport: poll failed with errno {errno}")
            }
            MailTransportErrorKind::Perform(e) => {
                write!(f, "mail transport: perform failed: {e}")
            }
        }
    }
}

impl std::error::Error for MailTransportError {}

impl MailTransportError {
    /// The error classification.
    pub fn kind(&self) -> &MailTransportErrorKind {
        &self.kind
    }
}

/// An owned SMTP recipient list wrapping a `curl_slist` chain.
///
/// Freed exactly once on drop. A failed [`add`](RecipientList::add) preserves
/// the list built so far. Pass to [`MailTransport::set_recipients`].
///
/// Holds at most 10 000 recipients and 512 KiB of aggregate address bytes.
/// Each address is limited to 254 bytes (RFC 5321 §4.5.3.1).
pub struct RecipientList {
    head: *mut curl_sys::curl_slist,
    count: usize,
    total_bytes: usize,
}

impl RecipientList {
    /// Creates an empty recipient list.
    pub fn new() -> Self {
        Self {
            head: std::ptr::null_mut(),
            count: 0,
            total_bytes: 0,
        }
    }

    /// Appends one recipient address.
    ///
    /// # Errors
    ///
    /// Refuses ASCII control characters (including CR/LF/NUL), addresses
    /// longer than 254 bytes, count above 10 000, aggregate bytes above
    /// 512 KiB, or slist allocation failure. On any refusal the prior
    /// list is preserved.
    pub fn add(&mut self, address: &str) -> Result<(), MailTransportError> {
        check_address(address)?;
        if self.count >= MAX_RECIPIENTS {
            return Err(MailTransportError {
                kind: MailTransportErrorKind::RecipientLimitReached,
            });
        }
        let new_total = self
            .total_bytes
            .checked_add(address.len())
            .ok_or(MailTransportError {
                kind: MailTransportErrorKind::RecipientLimitReached,
            })?;
        if new_total > MAX_RECIPIENT_BYTES {
            return Err(MailTransportError {
                kind: MailTransportErrorKind::RecipientLimitReached,
            });
        }
        let c_address = CString::new(address).map_err(|_| MailTransportError {
            kind: MailTransportErrorKind::ControlCharInAddress,
        })?;
        let new_head = ffi::slist_append(self.head, &c_address)?;
        self.head = new_head;
        self.count += 1;
        self.total_bytes = new_total;
        Ok(())
    }

    /// The number of recipients in the list.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether the list is empty.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    fn as_ptr(&self) -> *mut curl_sys::curl_slist {
        self.head
    }

    fn into_raw_parts(self) -> (*mut curl_sys::curl_slist, usize) {
        // The caller takes over the list, so it must not be freed here.
        let list = std::mem::ManuallyDrop::new(self);
        (list.head, list.count)
    }
}

impl Default for RecipientList {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RecipientList {
    fn drop(&mut self) {
        if !self.head.is_null() {
            ffi::slist_free_all(self.head);
        }
    }
}

#[cfg(test)]
impl RecipientList {}

impl fmt::Debug for RecipientList {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecipientList")
            .field("count", &self.count)
            .field("total_bytes", &self.total_bytes)
            .finish()
    }
}

/// Lifetime-bound guard for libcurl mail transport options.
///
/// Exclusively borrows `&'a mut Easy` and owns the `curl_slist` installed
/// via `CURLOPT_MAIL_RCPT`. On drop, clears the option (or resets the
/// handle) before freeing the list, so the handle never holds a dangling
/// pointer.
pub struct MailTransport<'a> {
    easy: Option<&'a mut curl::easy::Easy>,
    mail_from: Option<CString>,
    rcpt_head: *mut curl_sys::curl_slist,
    rcpt_count: usize,
    ssl_required: bool,
}

impl<'a> MailTransport<'a> {
    /// Creates a guard bound to the given `Easy` handle.
    pub fn new(easy: &'a mut curl::easy::Easy) -> Self {
        Self {
            easy: Some(easy),
            mail_from: None,
            rcpt_head: std::ptr::null_mut(),
            rcpt_count: 0,
            ssl_required: false,
        }
    }

    fn easy(&mut self) -> &mut curl::easy::Easy {
        self.easy
            .as_deref_mut()
            .expect("MailTransport: handle already released")
    }

    /// Authenticates with an OAuth 2.0 access token instead of a password;
    /// see [`set_oauth2_bearer`].
    ///
    /// # Errors
    ///
    /// As [`set_oauth2_bearer`].
    pub fn set_oauth2_bearer(
        &mut self,
        user: &str,
        access_token: &enzui_security::secret::SecretText,
    ) -> Result<(), MailTransportError> {
        set_oauth2_bearer(self.easy(), user, access_token)
    }

    /// Sets `CURLOPT_USE_SSL` to `CURLUSESSL_ALL`. Idempotent.
    ///
    /// # Errors
    ///
    /// A [`MailTransportError`] when libcurl refuses the option.
    pub fn set_require_ssl(&mut self) -> Result<(), MailTransportError> {
        ffi::set_use_ssl_all(self.easy())?;
        self.ssl_required = true;
        Ok(())
    }

    /// Whether SSL has been required on the handle.
    pub fn ssl_required(&self) -> bool {
        self.ssl_required
    }

    /// Sets `CURLOPT_MAIL_FROM`. libcurl copies the string.
    ///
    /// # Errors
    ///
    /// Refuses ASCII control characters (including CR/LF/NUL) or
    /// addresses longer than 254 bytes (RFC 5321 §4.5.3.1).
    pub fn set_mail_from(&mut self, address: &str) -> Result<(), MailTransportError> {
        check_address(address)?;
        let c_address = CString::new(address).map_err(|_| MailTransportError {
            kind: MailTransportErrorKind::ControlCharInAddress,
        })?;
        ffi::set_mail_from(self.easy(), &c_address)?;
        self.mail_from = Some(c_address);
        Ok(())
    }

    /// Sets `CURLOPT_MAIL_RCPT` from a non-empty [`RecipientList`].
    ///
    /// On success the old list is freed. On failure the old list and
    /// handle option are left intact; the new list is returned in
    /// [`SetRecipientsError`].
    ///
    /// # Errors
    ///
    /// A [`SetRecipientsError`], handing `list` back, when it is empty or libcurl refuses it.
    pub fn set_recipients(&mut self, list: RecipientList) -> Result<(), SetRecipientsError> {
        if list.is_empty() {
            return Err(SetRecipientsError {
                list,
                source: MailTransportError {
                    kind: MailTransportErrorKind::EmptyRecipientList,
                },
            });
        }

        if let Err(e) = ffi::set_mail_rcpt(self.easy(), &list) {
            return Err(SetRecipientsError { list, source: e });
        }

        let (new_head, new_count) = list.into_raw_parts();
        let old_head = self.rcpt_head;
        self.rcpt_head = new_head;
        self.rcpt_count = new_count;
        if !old_head.is_null() {
            ffi::slist_free_all(old_head);
        }
        Ok(())
    }

    /// Null-clears `CURLOPT_MAIL_RCPT` on the handle, then frees the slist.
    ///
    /// # Errors
    ///
    /// A [`MailTransportError`] when libcurl refuses to clear the list.
    pub fn clear_recipients(&mut self) -> Result<(), MailTransportError> {
        try_clear_mail_rcpt(self.easy())?;
        let old_head = self.rcpt_head;
        self.rcpt_head = std::ptr::null_mut();
        self.rcpt_count = 0;
        if !old_head.is_null() {
            ffi::slist_free_all(old_head);
        }
        Ok(())
    }

    /// The number of currently configured recipients.
    pub fn recipient_count(&self) -> usize {
        self.rcpt_count
    }

    /// Calls `Easy::perform` while the slist stays alive.
    ///
    /// # Errors
    ///
    /// A [`MailTransportError`] carrying libcurl's failure.
    pub fn perform(&mut self) -> Result<(), MailTransportError> {
        self.easy().perform().map_err(|e| MailTransportError {
            kind: MailTransportErrorKind::Perform(e),
        })
    }

    /// Detaches recipients, frees the slist, and returns the handle.
    ///
    /// The returned handle never holds a dangling pointer. If a full
    /// `Easy::reset()` was required, [`ReleasedEasy::was_reset`] is true
    /// and all previously set options have been cleared.
    #[allow(
        clippy::missing_panics_doc,
        reason = "panics only on a broken invariant: the handle leaves only through `release`, \
                  which consumes the transport"
    )]
    pub fn release(mut self) -> ReleasedEasy<'a> {
        let reset_needed = self.ensure_no_dangling_rcpt();
        let easy = self
            .easy
            .take()
            .expect("MailTransport: handle already released");
        ReleasedEasy {
            easy,
            was_reset: reset_needed,
        }
    }

    fn ensure_no_dangling_rcpt(&mut self) -> bool {
        if self.rcpt_head.is_null() {
            return false;
        }
        let easy = self
            .easy
            .as_deref_mut()
            .expect("MailTransport: handle already released");
        let did_reset = if try_clear_mail_rcpt(easy).is_err() {
            easy.reset();
            true
        } else {
            false
        };
        ffi::slist_free_all(self.rcpt_head);
        self.rcpt_head = std::ptr::null_mut();
        self.rcpt_count = 0;
        did_reset
    }
}

impl<'a> Drop for MailTransport<'a> {
    fn drop(&mut self) {
        if !self.rcpt_head.is_null() && self.easy.is_some() {
            self.ensure_no_dangling_rcpt();
        }
    }
}

/// Result of [`MailTransport::release`].
///
/// The handle never holds a dangling pointer. If `was_reset` is true,
/// `Easy::reset()` cleared all options and the caller must reconfigure.
pub struct ReleasedEasy<'a> {
    /// The released handle.
    pub easy: &'a mut curl::easy::Easy,
    /// `true` if `Easy::reset()` was called as a fallback.
    pub was_reset: bool,
}

/// Error from [`MailTransport::set_recipients`].
///
/// Contains the unconsumed [`RecipientList`]; the old list on the handle
/// is left intact.
#[derive(Debug)]
pub struct SetRecipientsError {
    /// The list that was not installed.
    pub list: RecipientList,
    /// The underlying error.
    pub source: MailTransportError,
}

impl fmt::Display for SetRecipientsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "set recipients failed: {}", self.source)
    }
}

impl std::error::Error for SetRecipientsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}
