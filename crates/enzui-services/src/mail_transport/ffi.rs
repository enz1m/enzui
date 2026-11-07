//! Unsafe FFI boundary for `CURLOPT_USE_SSL`, `CURLOPT_MAIL_FROM`,
//! `CURLOPT_MAIL_RCPT`, `curl_slist_append`, `curl_slist_free_all`,
//! `CURLINFO_ACTIVESOCKET` and `poll(2)`.
//!
//! `CURLOPT_MAIL_RCPT` stores the slist pointer by reference; the parent
//! guard must clear or reset the handle before freeing the list.
//! `CURLOPT_MAIL_FROM`, `CURLOPT_USERNAME` and `CURLOPT_XOAUTH2_BEARER` are
//! string-copy options (libcurl copies the bytes).

use std::ffi::CString;

use super::{MailTransportError, MailTransportErrorKind, RecipientList};

pub(super) const USE_SSL_VALUE: std::ffi::c_long = curl_sys::CURLUSESSL_ALL as std::ffi::c_long;

pub(super) fn set_use_ssl_all(easy: &mut curl::easy::Easy) -> Result<(), MailTransportError> {
    // SAFETY: long-value option, no pointer stored.
    let rc =
        unsafe { curl_sys::curl_easy_setopt(easy.raw(), curl_sys::CURLOPT_USE_SSL, USE_SSL_VALUE) };
    if rc != curl_sys::CURLE_OK {
        return Err(MailTransportError {
            kind: MailTransportErrorKind::CurlSetopt {
                option: "CURLOPT_USE_SSL",
                curl_code: rc as u32,
            },
        });
    }
    Ok(())
}

pub(super) fn set_mail_from(
    easy: &mut curl::easy::Easy,
    c_address: &CString,
) -> Result<(), MailTransportError> {
    // SAFETY: CURLOPT_MAIL_FROM copies the string; pointer need not outlive the call.
    let rc = unsafe {
        curl_sys::curl_easy_setopt(easy.raw(), curl_sys::CURLOPT_MAIL_FROM, c_address.as_ptr())
    };
    if rc != curl_sys::CURLE_OK {
        return Err(MailTransportError {
            kind: MailTransportErrorKind::CurlSetopt {
                option: "CURLOPT_MAIL_FROM",
                curl_code: rc as u32,
            },
        });
    }
    Ok(())
}

pub(super) fn set_mail_rcpt(
    easy: &mut curl::easy::Easy,
    list: &RecipientList,
) -> Result<(), MailTransportError> {
    // SAFETY: CURLOPT_MAIL_RCPT stores the pointer by reference; the parent
    // guard keeps the slist alive until clear/reset.
    let rc = unsafe {
        curl_sys::curl_easy_setopt(easy.raw(), curl_sys::CURLOPT_MAIL_RCPT, list.as_ptr())
    };
    if rc != curl_sys::CURLE_OK {
        return Err(MailTransportError {
            kind: MailTransportErrorKind::CurlSetopt {
                option: "CURLOPT_MAIL_RCPT",
                curl_code: rc as u32,
            },
        });
    }
    Ok(())
}

pub(super) fn clear_mail_rcpt(easy: &mut curl::easy::Easy) -> Result<(), MailTransportError> {
    // SAFETY: setting CURLOPT_MAIL_RCPT to null detaches the slist without freeing it.
    let rc = unsafe {
        curl_sys::curl_easy_setopt(
            easy.raw(),
            curl_sys::CURLOPT_MAIL_RCPT,
            std::ptr::null::<curl_sys::curl_slist>(),
        )
    };
    if rc != curl_sys::CURLE_OK {
        return Err(MailTransportError {
            kind: MailTransportErrorKind::CurlSetopt {
                option: "CURLOPT_MAIL_RCPT",
                curl_code: rc as u32,
            },
        });
    }
    Ok(())
}

/// `CURLOPT_XOAUTH2_BEARER`, which `curl-sys` leaves commented out:
/// `CURLOPTTYPE_OBJECTPOINT + 220`.
pub(super) const CURLOPT_XOAUTH2_BEARER: curl_sys::CURLoption = 10_000 + 220;

/// Sets a string-copy option (libcurl copies the bytes during the call).
pub(super) fn set_string_option(
    easy: &mut curl::easy::Easy,
    option: curl_sys::CURLoption,
    name: &'static str,
    value: &CString,
) -> Result<(), MailTransportError> {
    // SAFETY: a string-copy option; the pointer need not outlive the call.
    let rc = unsafe { curl_sys::curl_easy_setopt(easy.raw(), option, value.as_ptr()) };
    if rc != curl_sys::CURLE_OK {
        return Err(MailTransportError {
            kind: MailTransportErrorKind::CurlSetopt {
                option: name,
                curl_code: rc as u32,
            },
        });
    }
    Ok(())
}

pub(super) fn slist_append(
    head: *mut curl_sys::curl_slist,
    c_str: &CString,
) -> Result<*mut curl_sys::curl_slist, MailTransportError> {
    // SAFETY: head is null (new list) or valid from a prior append; data is copied.
    // On failure the original list is preserved.
    let new_head = unsafe { curl_sys::curl_slist_append(head, c_str.as_ptr()) };
    if new_head.is_null() {
        return Err(MailTransportError {
            kind: MailTransportErrorKind::RecipientAppendFailed,
        });
    }
    Ok(new_head)
}

pub(super) fn slist_free_all(head: *mut curl_sys::curl_slist) {
    // SAFETY: head is valid and non-null; called exactly once per list.
    unsafe {
        curl_sys::curl_slist_free_all(head);
    }
}

// `CURLINFO_SOCKET + 44` in curl.h; curl-sys does not export it.
const CURLINFO_ACTIVESOCKET: curl_sys::CURLINFO = 0x50_0000 + 44;

/// The socket a `CONNECT_ONLY` handle is connected on.
#[cfg(unix)]
pub(super) fn active_socket(easy: &curl::easy::Easy) -> Option<curl_sys::curl_socket_t> {
    let mut socket: curl_sys::curl_socket_t = curl_sys::CURL_SOCKET_BAD;
    // SAFETY: CURLINFO_ACTIVESOCKET writes one curl_socket_t through the
    // pointer, which outlives the call.
    let rc = unsafe {
        curl_sys::curl_easy_getinfo(easy.raw(), CURLINFO_ACTIVESOCKET, &mut socket as *mut _)
    };
    (rc == curl_sys::CURLE_OK && socket != curl_sys::CURL_SOCKET_BAD).then_some(socket)
}

/// Which of `fds` are readable (or hung up) within `timeout_ms`, or `errno`.
#[cfg(unix)]
pub(super) fn poll_readable(
    fds: [libc::c_int; 2],
    timeout_ms: libc::c_int,
) -> Result<[bool; 2], i32> {
    let mut polled = fds.map(|fd| libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    });
    // SAFETY: `polled` is an array of two initialised pollfd that lives for the
    // whole call, and its length is passed exactly.
    let rc = unsafe { libc::poll(polled.as_mut_ptr(), 2, timeout_ms) };
    if rc < 0 {
        return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
    }
    Ok(polled.map(|fd| fd.revents != 0))
}
