use sha2::{Digest, Sha256};

const SHA256_BYTES: usize = 32;

/// Fixed namespaces for reusable remote-resource filename identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CacheIdentityDomain {
    /// Identity of an icon URL.
    IconUrl,
    /// Identity of a screenshot URL.
    ScreenshotUrl,
    /// Identity of a remote host identifier.
    HostId,
}

impl CacheIdentityDomain {
    const fn tag(self) -> &'static [u8] {
        match self {
            Self::IconUrl => b"enzui-cache-identity-v1/icon-url",
            Self::ScreenshotUrl => b"enzui-cache-identity-v1/screenshot-url",
            Self::HostId => b"enzui-cache-identity-v1/host-id",
        }
    }
}

/// Derives a domain-separated SHA-256 filename identity for `bytes`.
///
/// The returned string is exactly 64 lowercase ASCII hex characters and retains
/// all 256 digest bits. Domain tags are fixed by [`CacheIdentityDomain`], and
/// both the tag and exact byte string are length-prefixed before hashing, so a
/// caller cannot choose an empty or ambiguous domain encoding.
///
/// This is an identity key, not authentication or secret storage. Digesting a
/// secret does not make the key safe to disclose, especially when the secret
/// has guessable entropy.
#[must_use]
pub fn cache_identity(domain: CacheIdentityDomain, bytes: &[u8]) -> String {
    let digest = digest_bytes_with_domain(domain.tag(), bytes);
    let mut identity = String::with_capacity(SHA256_BYTES * 2);
    const LOWER_HEX: &[u8; 16] = b"0123456789abcdef";

    for byte in digest {
        identity.push(char::from(LOWER_HEX[usize::from(byte >> 4)]));
        identity.push(char::from(LOWER_HEX[usize::from(byte & 0x0f)]));
    }

    identity
}

pub(crate) fn digest_bytes_with_domain(domain: &[u8], material: &[u8]) -> [u8; SHA256_BYTES] {
    let mut hash = Sha256::new();
    hash.update((domain.len() as u64).to_be_bytes());
    hash.update(domain);
    hash.update((material.len() as u64).to_be_bytes());
    hash.update(material);
    hash.finalize().into()
}
