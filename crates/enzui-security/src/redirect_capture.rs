//! Which navigation is a sign-in callback for the host rather than a page to
//! load.
//!
//! An OAuth provider ends a sign-in by redirecting to the app's registered
//! redirect URI with the code in the query: a private-use scheme
//! (`com.example.app:/oauth2redirect`, RFC 8252 §7.1), a loopback address
//! (`http://127.0.0.1:PORT/cb`, §7.3) or a claimed HTTPS URL (§7.2). In a
//! browser that URL reaches the app. In an embedded view it would be judged as
//! a page: a private scheme fails to load, and a loopback address is refused
//! by the destination policy. Either way the code is lost.
//!
//! A [`RedirectCapture`] names one redirect URI exactly (scheme, authority and
//! path; never a prefix), so the engine can stop that one navigation before it
//! loads and hand its URL to the host. The same type is what
//! `enzui_services::oauth::PendingAuthorization` checks a callback against.

use std::fmt;

/// Schemes a page uses itself; capturing one would capture the web.
const REFUSED_SCHEMES: &[&str] = &[
    "about",
    "blob",
    "data",
    "file",
    "filesystem",
    "ftp",
    "javascript",
    "mailto",
    "tel",
    "ws",
    "wss",
];

/// Why a redirect URI cannot be captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RedirectCaptureError {
    /// Not `scheme:rest`, or it holds a space or a control byte.
    Malformed,
    /// It carries a query or fragment; a redirect URI names a place, and the
    /// provider adds the query.
    QueryOrFragment,
    /// `http`/`https` with no host.
    MissingAuthority,
    /// A scheme pages use themselves (`about`, `data`, `javascript`, …).
    RefusedScheme,
    /// Plain `http://` to a host that is not loopback: anything on the path
    /// could read the code.
    InsecureHttp,
}

impl fmt::Display for RedirectCaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Malformed => "redirect URI is not an absolute URI",
            Self::QueryOrFragment => "redirect URI carries a query or fragment",
            Self::MissingAuthority => "redirect URI has no host",
            Self::RefusedScheme => "redirect URI uses a scheme pages use themselves",
            Self::InsecureHttp => "redirect URI is plain HTTP to a host that is not loopback",
        })
    }
}

impl std::error::Error for RedirectCaptureError {}

/// The kind of redirect URI, per RFC 8252 §7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RedirectKind {
    /// A private-use scheme such as `com.example.app:/oauth2redirect`.
    PrivateScheme,
    /// `http://` to `127.0.0.1`, `[::1]` or `localhost`.
    Loopback,
    /// A claimed `https://` URL.
    Https,
}

/// One redirect URI, matched exactly.
#[derive(Clone, PartialEq, Eq)]
pub struct RedirectCapture {
    kind: RedirectKind,
    scheme: String,
    authority: String,
    path: String,
}

impl fmt::Debug for RedirectCapture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let separator = if self.authority.is_empty() {
            ":"
        } else {
            "://"
        };
        write!(
            f,
            "RedirectCapture({:?} {}{separator}{}{})",
            self.kind, self.scheme, self.authority, self.path
        )
    }
}

/// `scheme`, `authority`, `path`, `rest` (query and fragment, if any) of an
/// absolute URI. Scheme and authority are lowercased.
fn split(uri: &str) -> Result<(String, String, &str, &str), RedirectCaptureError> {
    if uri.bytes().any(|b| b.is_ascii_control() || b == b' ') {
        return Err(RedirectCaptureError::Malformed);
    }
    let (scheme, rest) = uri.split_once(':').ok_or(RedirectCaptureError::Malformed)?;
    let scheme_ok = scheme
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic())
        && scheme
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
    if !scheme_ok {
        return Err(RedirectCaptureError::Malformed);
    }
    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    let (hier, tail) = rest.split_at(end);
    let (authority, path) = match hier.strip_prefix("//") {
        Some(hier) => hier
            .find('/')
            .map_or((hier, ""), |slash| hier.split_at(slash)),
        None => ("", hier),
    };
    Ok((
        scheme.to_ascii_lowercase(),
        authority.to_ascii_lowercase(),
        path,
        tail,
    ))
}

/// The host of a strictly formed authority: `host[:port]` or
/// `[v6-address][:port]`, with no user information, a nonempty host of DNS
/// label characters, and a port of digits within 1..=65535. Anything else,
/// such as `127.0.0.1:x@example.com` or `[::1]junk`, is `None`, so nothing
/// is classified by a prefix that only looks like a host.
fn authority_host(authority: &str) -> Option<&str> {
    let (host, port) = match authority.strip_prefix('[') {
        Some(rest) => {
            let (address, after) = rest.split_once(']')?;
            address.parse::<std::net::Ipv6Addr>().ok()?;
            let port = match after {
                "" => None,
                after => Some(after.strip_prefix(':')?),
            };
            (address, port)
        }
        None => {
            let (host, port) = match authority.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (authority, None),
            };
            let label = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.');
            if host.is_empty() || !host.bytes().all(label) {
                return None;
            }
            (host, port)
        }
    };
    if let Some(port) = port
        && (port.is_empty()
            || !port.bytes().all(|b| b.is_ascii_digit())
            || !port.parse::<u16>().is_ok_and(|port| port != 0))
    {
        return None;
    }
    Some(host)
}

/// Whether an authority (`host[:port]`, lowercase) names this machine. A
/// malformed authority, one with user information included, never does.
#[must_use]
pub fn is_loopback_authority(authority: &str) -> bool {
    authority_host(authority).is_some_and(|host| {
        host == "localhost"
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    })
}

impl RedirectCapture {
    /// Reads a registered redirect URI.
    ///
    /// # Errors
    ///
    /// A [`RedirectCaptureError`] for a query or fragment, a refused or cleartext scheme, or a
    /// missing authority.
    pub fn parse(redirect_uri: &str) -> Result<Self, RedirectCaptureError> {
        let (scheme, authority, path, tail) = split(redirect_uri)?;
        if !tail.is_empty() {
            return Err(RedirectCaptureError::QueryOrFragment);
        }
        if REFUSED_SCHEMES.contains(&scheme.as_str()) {
            return Err(RedirectCaptureError::RefusedScheme);
        }
        let kind = match scheme.as_str() {
            "http" | "https" if authority.is_empty() => {
                return Err(RedirectCaptureError::MissingAuthority);
            }
            "http" | "https" if authority_host(&authority).is_none() => {
                return Err(RedirectCaptureError::Malformed);
            }
            "http" if is_loopback_authority(&authority) => RedirectKind::Loopback,
            "http" => return Err(RedirectCaptureError::InsecureHttp),
            "https" => RedirectKind::Https,
            _ => RedirectKind::PrivateScheme,
        };
        Ok(Self {
            kind,
            scheme,
            authority,
            path: if path.is_empty() {
                "/".into()
            } else {
                path.into()
            },
        })
    }

    /// What kind of redirect URI this is.
    #[must_use]
    pub fn kind(&self) -> RedirectKind {
        self.kind
    }

    /// Whether `url` is this redirect: same scheme, authority (port included)
    /// and path. The query and fragment are what the provider adds.
    #[must_use]
    pub fn matches(&self, url: &str) -> bool {
        split(url).is_ok_and(|(scheme, authority, path, _)| {
            let path = if path.is_empty() { "/" } else { path };
            scheme == self.scheme && authority == self.authority && path == self.path
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_redirect_uri_is_classified_and_unsafe_ones_are_refused() {
        let kind = |uri| RedirectCapture::parse(uri).map(|c| c.kind());
        assert_eq!(
            kind("com.example.app:/oauth2redirect"),
            Ok(RedirectKind::PrivateScheme)
        );
        assert_eq!(
            kind("http://127.0.0.1:49152/cb"),
            Ok(RedirectKind::Loopback)
        );
        assert_eq!(kind("http://[::1]:8080/cb"), Ok(RedirectKind::Loopback));
        assert_eq!(
            kind("https://app.example.test/oauth"),
            Ok(RedirectKind::Https)
        );
        assert_eq!(
            kind("http://app.example.test/cb"),
            Err(RedirectCaptureError::InsecureHttp)
        );
        assert_eq!(
            kind("http://127.0.0.1.example.test/cb"),
            Err(RedirectCaptureError::InsecureHttp)
        );
        assert_eq!(
            kind("javascript:alert(1)"),
            Err(RedirectCaptureError::RefusedScheme)
        );
        assert_eq!(
            kind("about:blank"),
            Err(RedirectCaptureError::RefusedScheme)
        );
        assert_eq!(
            kind("myapp:/cb?x=1"),
            Err(RedirectCaptureError::QueryOrFragment)
        );
        assert_eq!(
            kind("https:///cb"),
            Err(RedirectCaptureError::MissingAuthority)
        );
        assert_eq!(kind("no scheme"), Err(RedirectCaptureError::Malformed));
    }

    #[test]
    fn userinfo_and_ambiguous_authorities_are_never_loopback() {
        let kind = |uri| RedirectCapture::parse(uri).map(|c| c.kind());
        for uri in [
            "http://127.0.0.1:x@evil.example/cb",
            "http://localhost@evil.example/cb",
            "http://user:pw@127.0.0.1/cb",
            "http://[::1]evil.example/cb",
            "http://[::1]:80:81/cb",
            "http://127.0.0.1:99999/cb",
            "http://127.0.0.1:/cb",
            "http://127.0.0.1:0/cb",
            "https://user@login.example/authorize",
            "http://loc\\alhost/cb",
        ] {
            assert_eq!(kind(uri), Err(RedirectCaptureError::Malformed), "{uri}");
        }
        assert!(!is_loopback_authority("127.0.0.1:x@evil.example"));
        assert!(is_loopback_authority("localhost:8080"));
        assert!(is_loopback_authority("[::1]"));
        assert_eq!(kind("http://localhost/cb"), Ok(RedirectKind::Loopback));
    }

    #[test]
    fn only_the_exact_redirect_matches() {
        let capture = RedirectCapture::parse("http://127.0.0.1:49152/cb").expect("capture");
        assert!(capture.matches("http://127.0.0.1:49152/cb?code=c&state=s"));
        assert!(capture.matches("HTTP://127.0.0.1:49152/cb#x"));
        for other in [
            "http://127.0.0.1:49153/cb?code=c",
            "http://127.0.0.1:49152/cb/more?code=c",
            "http://127.0.0.1:49152/c?code=c",
            "https://127.0.0.1:49152/cb?code=c",
            "http://127.0.0.1:49152@evil.test/cb",
        ] {
            assert!(!capture.matches(other), "{other}");
        }
        let scheme = RedirectCapture::parse("com.example.app:/oauth2redirect").expect("capture");
        assert!(scheme.matches("com.example.app:/oauth2redirect?code=c"));
        assert!(!scheme.matches("com.example.app:/oauth2redirect2?code=c"));
        assert!(!scheme.matches("com.example.ap:/oauth2redirect?code=c"));
        let root = RedirectCapture::parse("https://app.example.test").expect("capture");
        assert!(root.matches("https://app.example.test/?code=c"));
    }
}
