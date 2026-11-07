//! Capability authentication shared by enzUI's loopback HTTP proxies.
//!
//! A loopback address is routing, not identity: another local user in the same
//! network namespace can connect to it.  Each listener therefore mints one
//! unguessable bearer capability before binding and requires standard HTTP
//! `Proxy-Authorization: Basic …` credentials on every request. The same
//! value is handed to native clients through client-specific authentication
//! mechanisms.
//!
//! The capability and its client configuration deliberately redact `Debug`
//! and implement no `Display`.  Secret-returning accessors exist only for the
//! private configuration handoff to those two native clients.

use std::fmt;
use std::net::SocketAddr;

use crate::random::{RandomError, random_array};

/// Fixed, non-secret username paired with each freshly minted password.
pub const PROXY_USERNAME: &str = "enzui";

/// Standard challenge sent when a client omits or misstates the capability.
pub const PROXY_AUTHENTICATE_VALUE: &str = "Basic realm=\"enzUI loopback\"";

const TOKEN_BYTES: usize = 32;

/// A freshly minted bearer capability for one loopback proxy listener.
///
/// Clones represent the same listener capability; call [`Self::mint`] for a
/// distinct listener.  Its diagnostic representation never includes secret
/// bytes.
#[derive(Clone)]
pub struct LoopbackProxyCapability {
    password: String,
    authorization: String,
}

impl fmt::Debug for LoopbackProxyCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoopbackProxyCapability")
            .field("credential", &"<redacted>")
            .finish()
    }
}

impl LoopbackProxyCapability {
    /// Mint a new 256-bit capability from the framework's OS entropy boundary.
    ///
    /// # Errors
    ///
    /// Returns [`RandomError`] when fresh OS entropy is unavailable.  There is
    /// no clock, PID, port, counter, or fixed-value fallback.
    pub fn mint() -> Result<Self, RandomError> {
        let bytes = random_array::<TOKEN_BYTES>()?;
        let mut password = String::with_capacity(TOKEN_BYTES * 2);
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in bytes {
            password.push(char::from(HEX[usize::from(byte >> 4)]));
            password.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        let authorization = format!(
            "Basic {}",
            base64_standard(format!("{PROXY_USERNAME}:{password}").as_bytes())
        );
        Ok(Self {
            password,
            authorization,
        })
    }

    /// A credential-free URI suitable for diagnostics and address comparison.
    #[must_use]
    pub fn display_uri(&self, address: SocketAddr) -> String {
        format!("http://{address}")
    }

    /// A proxy URI carrying the capability through standard URI userinfo.
    ///
    /// This is secret material for clients that support URI-userinfo proxy
    /// authentication. It must not be logged, displayed, or retained in public
    /// diagnostics.
    #[must_use]
    pub fn authenticated_uri(&self, address: SocketAddr) -> String {
        format!("http://{PROXY_USERNAME}:{}@{address}", self.password)
    }

    /// The non-secret proxy-authentication username for native client setup.
    #[must_use]
    pub const fn username(&self) -> &'static str {
        PROXY_USERNAME
    }

    /// The secret proxy-authentication password for native client setup.
    ///
    /// This is the minimum private handoff required by GStreamer's
    /// `proxy-pw` property.  Callers must not include it in diagnostics.
    #[must_use]
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Authenticate one complete HTTP/1 request head and remove the credential.
    ///
    /// Exactly one `Proxy-Authorization` field is required.  Missing,
    /// malformed, wrong, or duplicate fields are indistinguishable to callers
    /// except for the safe classification in [`ProxyAuthenticationError`].
    /// The returned head contains no proxy credential and can be parsed or
    /// forwarded safely.
    ///
    /// # Errors
    ///
    /// A [`ProxyAuthenticationError`] when the credential is missing, malformed, duplicated or
    /// wrong.
    pub fn authenticate_and_strip(
        &self,
        head: &[u8],
    ) -> Result<AuthenticatedProxyHead, ProxyAuthenticationError> {
        let body = head
            .strip_suffix(b"\r\n\r\n")
            .ok_or(ProxyAuthenticationError::Malformed)?;
        let first_end = find_crlf(body).unwrap_or(body.len());
        if first_end == 0 {
            return Err(ProxyAuthenticationError::Malformed);
        }

        let mut credential: Option<&[u8]> = None;
        let mut stripped = Vec::with_capacity(head.len());
        stripped.extend_from_slice(&body[..first_end]);
        stripped.extend_from_slice(b"\r\n");
        let mut cursor = if first_end < body.len() {
            first_end + 2
        } else {
            body.len()
        };
        while cursor < body.len() {
            let rest = &body[cursor..];
            let line_end = find_crlf(rest).unwrap_or(rest.len());
            let line = &rest[..line_end];
            let colon = line.iter().position(|byte| *byte == b':');
            let Some(colon) = colon else {
                // Leave general HTTP syntax judgment to the listener's parser,
                // but an unparseable auth-shaped line cannot authenticate.
                if ascii_starts_with_ignore_case(line.trim_ascii_start(), b"proxy-authorization") {
                    return Err(ProxyAuthenticationError::Malformed);
                }
                stripped.extend_from_slice(line);
                stripped.extend_from_slice(b"\r\n");
                if line_end == rest.len() {
                    break;
                }
                cursor += line_end + 2;
                continue;
            };
            let name = &line[..colon];
            let value = line[colon + 1..].trim_ascii();
            if name.eq_ignore_ascii_case(b"proxy-authorization") {
                if credential.replace(value).is_some() {
                    return Err(ProxyAuthenticationError::Duplicate);
                }
            } else {
                stripped.extend_from_slice(line);
                stripped.extend_from_slice(b"\r\n");
            }
            if line_end == rest.len() {
                break;
            }
            cursor += line_end + 2;
        }
        let credential = credential.ok_or(ProxyAuthenticationError::Missing)?;
        if !constant_time_eq(credential, self.authorization.as_bytes()) {
            return Err(ProxyAuthenticationError::Invalid);
        }
        stripped.extend_from_slice(b"\r\n");
        Ok(AuthenticatedProxyHead(stripped))
    }
}

/// A request head proven to carry the listener's capability, with that header
/// removed.
pub struct AuthenticatedProxyHead(Vec<u8>);

impl AuthenticatedProxyHead {
    /// Borrow the credential-free HTTP request head as bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for AuthenticatedProxyHead {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthenticatedProxyHead")
            .field("contents", &"<redacted>")
            .finish()
    }
}

/// Safe classification of a refused local proxy client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyAuthenticationError {
    /// No proxy credential was supplied.
    Missing,
    /// The credential field or request framing was malformed.
    Malformed,
    /// More than one credential field was supplied.
    Duplicate,
    /// A single well-formed field did not carry this listener's capability.
    Invalid,
}

fn find_crlf(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|window| window == b"\r\n")
}

fn ascii_starts_with_ignore_case(bytes: &[u8], prefix: &[u8]) -> bool {
    bytes
        .get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0_u8;
    for (&left, &right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

fn base64_standard(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(char::from(ALPHABET[usize::from(first >> 2)]));
        output.push(char::from(
            ALPHABET[usize::from(((first & 0x03) << 4) | (second >> 4))],
        ));
        if chunk.len() > 1 {
            output.push(char::from(
                ALPHABET[usize::from(((second & 0x0f) << 2) | (third >> 6))],
            ));
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(char::from(ALPHABET[usize::from(third & 0x3f)]));
        } else {
            output.push('=');
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn authorization(capability: &LoopbackProxyCapability) -> String {
        format!(
            "Basic {}",
            base64_standard(
                format!("{}:{}", capability.username(), capability.password()).as_bytes()
            )
        )
    }

    #[test]
    fn fresh_capabilities_authenticate_exactly_once_and_strip_the_header() {
        let capability = LoopbackProxyCapability::mint().expect("OS entropy");
        let other = LoopbackProxyCapability::mint().expect("OS entropy");
        assert_ne!(capability.password(), other.password());
        let auth = authorization(&capability);
        let head = format!(
            "GET http://example.test/a HTTP/1.1\r\nHost: example.test\r\n\
             Proxy-Authorization: {auth}\r\nX-Keep: yes\r\n\r\n"
        );
        let stripped = capability
            .authenticate_and_strip(head.as_bytes())
            .expect("exact capability authenticates");
        let stripped = String::from_utf8(stripped.as_bytes().to_vec()).unwrap();
        assert!(stripped.contains("X-Keep: yes"));
        assert!(
            !stripped
                .to_ascii_lowercase()
                .contains("proxy-authorization")
        );
        assert!(!stripped.contains(capability.password()));
        assert_eq!(
            other.authenticate_and_strip(head.as_bytes()).unwrap_err(),
            ProxyAuthenticationError::Invalid
        );
    }

    #[test]
    fn missing_malformed_wrong_and_duplicate_credentials_are_refused() {
        let capability = LoopbackProxyCapability::mint().expect("OS entropy");
        let auth = authorization(&capability);
        let cases = [
            (
                "GET http://example.test/ HTTP/1.1\r\nHost: example.test\r\n\r\n".to_owned(),
                ProxyAuthenticationError::Missing,
            ),
            (
                "GET http://example.test/ HTTP/1.1\r\nProxy-Authorization nope\r\n\r\n".to_owned(),
                ProxyAuthenticationError::Malformed,
            ),
            (
                "GET http://example.test/ HTTP/1.1\r\nProxy-Authorization: Basic wrong\r\n\r\n"
                    .to_owned(),
                ProxyAuthenticationError::Invalid,
            ),
            (
                format!(
                    "GET http://example.test/ HTTP/1.1\r\nProxy-Authorization: {auth}\r\n\
                     Proxy-Authorization: {auth}\r\n\r\n"
                ),
                ProxyAuthenticationError::Duplicate,
            ),
        ];
        for (head, expected) in cases {
            assert_eq!(
                capability
                    .authenticate_and_strip(head.as_bytes())
                    .unwrap_err(),
                expected
            );
        }
    }

    #[test]
    fn every_observable_debug_value_redacts_the_capability() {
        let capability = LoopbackProxyCapability::mint().expect("OS entropy");
        let secret = capability.password().to_owned();
        let auth = authorization(&capability);
        let authenticated = capability
            .authenticate_and_strip(
                format!("CONNECT example.test:443 HTTP/1.1\r\nProxy-Authorization: {auth}\r\n\r\n")
                    .as_bytes(),
            )
            .expect("authenticates");
        for diagnostic in [format!("{capability:?}"), format!("{authenticated:?}")] {
            assert!(!diagnostic.contains(&secret), "secret leaked: {diagnostic}");
            assert!(
                !diagnostic.contains(&auth),
                "authorization leaked: {diagnostic}"
            );
        }
    }
}
