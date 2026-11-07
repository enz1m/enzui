//! Conversion of portal URI results into local paths.
//!
//! File paths are percent-decoded once as bytes. Remote authorities and
//! non-file schemes remain distinct errors.

use std::path::PathBuf;

/// Reason a portal URI cannot be represented as a local path.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UriDecodeError {
    /// A non-`file` scheme, carrying the encountered prefix.
    NotFileScheme(String),
    /// A `file` URI naming a non-local host.
    RemoteHost(String),
    /// A `%` that is not followed by two hexadecimal digits.
    BadEscape,
    /// The URI decoded to an empty path, which no file can have.
    EmptyPath,
    /// The decoded bytes contain an interior NUL, which cannot appear in a path.
    InteriorNul,
    /// Non-UTF-8 path bytes on a platform that cannot represent them losslessly.
    NotUtf8,
}

impl std::fmt::Display for UriDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFileScheme(s) => write!(f, "not a file:// URI (scheme {s:?})"),
            Self::RemoteHost(h) => write!(f, "file:// URI on a remote host ({h:?})"),
            Self::BadEscape => write!(f, "malformed percent-escape"),
            Self::EmptyPath => write!(f, "URI decoded to an empty path"),
            Self::InteriorNul => write!(f, "decoded path contains an interior NUL"),
            Self::NotUtf8 => write!(f, "decoded path is not valid UTF-8"),
        }
    }
}

impl std::error::Error for UriDecodeError {}

const fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Percent-decodes URI text as bytes, preserving `+` literally.
///
/// Returns `BadEscape` for an incomplete or non-hexadecimal escape.
///
/// # Errors
///
/// [`UriDecodeError::BadEscape`] for a malformed `%` escape.
pub fn decode_percent(s: &str) -> Result<Vec<u8>, UriDecodeError> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let (Some(&h), Some(&l)) = (bytes.get(i + 1), bytes.get(i + 2)) else {
                return Err(UriDecodeError::BadEscape);
            };
            let (Some(h), Some(l)) = (hex_val(h), hex_val(l)) else {
                return Err(UriDecodeError::BadEscape);
            };
            out.push(h << 4 | l);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    Ok(out)
}

#[cfg(unix)]
fn path_from_bytes(bytes: Vec<u8>) -> Result<PathBuf, UriDecodeError> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(OsString::from_vec(bytes)))
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: Vec<u8>) -> Result<PathBuf, UriDecodeError> {
    String::from_utf8(bytes)
        .map(PathBuf::from)
        .map_err(|_| UriDecodeError::NotUtf8)
}

/// Converts a local `file` URI to a path.
///
/// Accepts absent, empty, or `localhost` authorities and treats raw `#` as a path byte.
///
/// # Errors
///
/// A [`UriDecodeError`] for a non-`file` scheme, a remote host, a bad escape, an empty path, or an
/// interior NUL.
pub fn file_uri_to_path(uri: &str) -> Result<PathBuf, UriDecodeError> {
    let Some((scheme, rest)) = uri.split_once(':') else {
        return Err(UriDecodeError::NotFileScheme(String::new()));
    };
    if !scheme.eq_ignore_ascii_case("file") {
        return Err(UriDecodeError::NotFileScheme(scheme.to_owned()));
    }

    let after_authority = if let Some(rest) = rest.strip_prefix("//") {
        let (authority, path) = match rest.find('/') {
            Some(i) => rest.split_at(i),
            None => (rest, ""),
        };
        if !authority.is_empty() && !authority.eq_ignore_ascii_case("localhost") {
            return Err(UriDecodeError::RemoteHost(authority.to_owned()));
        }
        path
    } else {
        rest
    };

    let bytes = decode_percent(after_authority)?;
    if bytes.is_empty() {
        return Err(UriDecodeError::EmptyPath);
    }
    if bytes.contains(&0) {
        return Err(UriDecodeError::InteriorNul);
    }
    path_from_bytes(bytes)
}
