//! The mutual-authentication proofs both peers compute.
//!
//! Client and broker proofs use distinct direction labels and bind the
//! version, the fresh hex nonces, the role, the credential identity and the
//! Unix peer UID. Version-3 broker proofs also bind the exact ordered
//! capabilities. Verification compares tags in constant time.

use crate::mac::{hmac_sha256, verify_mac};
use crate::protocol::{PROTOCOL_VERSION, WireRole};

/// The transcript label for the client (peer → broker) proof.
pub const DIRECTION_CLIENT: &str = "client";

/// The transcript label for the broker (broker → peer) proof.
pub const DIRECTION_BROKER: &str = "broker";

const DOMAIN: &[u8] = b"enzui-center-v2\0";
const DOMAIN_V3: &[u8] = b"enzui-center-v3\0";

/// Ordered capabilities authenticated by the version-3 broker proof.
pub const COMMIT_REPORT_CAPABILITIES: [&str; 2] =
    ["app_commit_reports_v1", "agent_commit_report_history_v1"];

/// Identity fields bound into mutual authentication proofs.
///
/// Nonces are the lowercase hex text the peers exchanged, not their decoded
/// bytes, matching the daemon.
pub struct ProofContext<'a> {
    /// The daemon's challenge nonce (64 lowercase hex characters).
    pub server_nonce: &'a str,
    /// The peer's own fresh nonce (64 lowercase hex characters).
    pub client_nonce: &'a str,
    /// The role being authenticated.
    pub role: WireRole,
    /// The credential id claimed.
    pub credential_id: &'a str,
    /// The Unix peer uid of the connection.
    pub uid: u32,
}

impl ProofContext<'_> {
    fn transcript(&self, direction: &str) -> Vec<u8> {
        self.transcript_version(direction, DOMAIN, PROTOCOL_VERSION, &[])
    }

    fn transcript_version(
        &self,
        direction: &str,
        domain: &[u8],
        version: u32,
        capabilities: &[&str],
    ) -> Vec<u8> {
        let mut buffer = Vec::with_capacity(96);
        buffer.extend_from_slice(domain);
        buffer.extend_from_slice(&version.to_be_bytes());
        for field in [
            direction,
            self.server_nonce,
            self.client_nonce,
            self.role.as_str(),
            self.credential_id,
        ] {
            buffer.extend_from_slice(&(field.len() as u32).to_be_bytes());
            buffer.extend_from_slice(field.as_bytes());
        }
        buffer.extend_from_slice(&self.uid.to_be_bytes());
        for capability in capabilities {
            buffer.extend_from_slice(&(capability.len() as u32).to_be_bytes());
            buffer.extend_from_slice(capability.as_bytes());
        }
        buffer
    }

    /// Computes a version-3 client proof, without a capability suffix.
    #[must_use]
    pub fn v3_client_proof(&self, secret: &[u8]) -> String {
        let transcript = self.transcript_version(DIRECTION_CLIENT, DOMAIN_V3, 3, &[]);
        hex::encode(hmac_sha256(secret, &[&transcript]))
    }

    /// Computes the version-3 broker proof with ordered capability framing.
    #[must_use]
    pub fn v3_broker_proof(&self, secret: &[u8], capabilities: &[&str]) -> String {
        let transcript = self.transcript_version(DIRECTION_BROKER, DOMAIN_V3, 3, capabilities);
        hex::encode(hmac_sha256(secret, &[&transcript]))
    }

    /// Verifies the capability-bound version-3 broker proof in constant time.
    #[must_use]
    pub fn verify_v3_broker(&self, secret: &[u8], capabilities: &[&str], candidate: &str) -> bool {
        if !valid_nonce(candidate) {
            return false;
        }
        let Ok(candidate) = hex::decode(candidate) else {
            return false;
        };
        let transcript = self.transcript_version(DIRECTION_BROKER, DOMAIN_V3, 3, capabilities);
        let expected = hmac_sha256(secret, &[&transcript]);
        verify_mac(&expected, &candidate)
    }

    /// Computes the lowercase-hex proof for `direction` under `secret`.
    #[must_use]
    pub fn proof(&self, secret: &[u8], direction: &str) -> String {
        let transcript = self.transcript(direction);
        hex::encode(hmac_sha256(secret, &[transcript.as_slice()]))
    }

    /// Verifies a candidate hex proof for `direction` in constant time over the
    /// tag bytes.
    #[must_use]
    pub fn verify(&self, secret: &[u8], direction: &str, candidate: &str) -> bool {
        let Ok(candidate) = hex::decode(candidate) else {
            return false;
        };
        let transcript = self.transcript(direction);
        let expected = hmac_sha256(secret, &[transcript.as_slice()]);
        verify_mac(&expected, &candidate)
    }
}

/// Whether `nonce` is exactly 64 lowercase hexadecimal characters.
#[must_use]
pub fn valid_nonce(nonce: &str) -> bool {
    nonce.len() == 64
        && nonce
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
