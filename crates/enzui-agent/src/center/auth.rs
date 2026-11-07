//! Mutual authentication for the existing v2 and opt-in v3 wire contracts.
//!
//! Client and broker proofs use distinct direction labels and bind the version,
//! fresh hex nonces, role, credential identity and Unix peer UID. Version-3
//! broker proofs also bind the exact ordered capabilities. Verification uses
//! constant-time comparison (`enzui_center_protocol::proof`).
//! [`authenticate`] retains v2 behavior; [`authenticate_v3`] verifies the separate
//! capability-bound extension used by the connector before legacy fallback.

use std::io::{Read, Write};

use crate::center::protocol::{
    Authenticate, AuthenticationResult, AuthenticationResultV3, Challenge, PROTOCOL_VERSION,
    WireRole,
};
use crate::center::wire::{self, WireError};
pub use enzui_center_protocol::proof::{
    COMMIT_REPORT_CAPABILITIES, DIRECTION_BROKER, DIRECTION_CLIENT, ProofContext, valid_nonce,
};

/// Why mutual authentication was refused.
#[derive(Debug)]
pub enum AuthError {
    /// A handshake frame could not be read or written.
    Wire(WireError),
    /// The challenge or acceptance named an unsupported wire version.
    UnsupportedVersion(u32),
    /// The challenge nonce was not 64 lowercase hex characters.
    BadChallengeNonce,
    /// The client supplied an invalid version-3 nonce.
    BadClientNonce,
    /// The version-3 acceptance did not contain the exact ordered capabilities.
    BadCapabilities,
    /// The peer closed without an acceptance frame after version-3 authentication.
    /// A connector may reconnect once with version 2 under its original budget.
    V3Unsupported,
    /// The daemon rejected the credential or proof. The daemon's message is not
    /// retained; a bad proof or wrong role lands here.
    Rejected,
    /// The daemon's acceptance carried a proof that did not verify — a bare
    /// acceptance, a reflected client proof, or a replay is refused here.
    BadBrokerProof,
}

impl core::fmt::Display for AuthError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Wire(error) => write!(f, "handshake io failed: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(f, "daemon offered unsupported wire version {version}")
            }
            Self::BadChallengeNonce => write!(f, "challenge nonce was not 64 lowercase hex"),
            Self::BadClientNonce => write!(f, "client nonce was not 64 lowercase hex"),
            Self::BadCapabilities => write!(f, "version-3 capabilities did not match"),
            Self::V3Unsupported => write!(f, "peer closed version-3 authentication"),
            Self::Rejected => write!(f, "daemon rejected authentication"),
            Self::BadBrokerProof => write!(f, "daemon acceptance proof did not verify"),
        }
    }
}

impl std::error::Error for AuthError {}

impl From<WireError> for AuthError {
    fn from(error: WireError) -> Self {
        Self::Wire(error)
    }
}

/// A completed authentication.
#[derive(Clone, Debug)]
pub struct Authenticated {
    /// The display name the daemon reported for this credential.
    pub display_name: String,
}

/// Drives the v2 mutual handshake over an already-connected stream.
///
/// Reads the challenge, mints a fresh client nonce (supplied by the caller so
/// the entropy source is a single boundary), sends the client proof, then
/// verifies the broker-direction proof before returning. The caller is
/// responsible for the five-second budget via socket read timeouts.
///
/// # Errors
/// Returns the applicable [`AuthError`]; a rejected credential or a bad broker
/// proof both fail closed without exposing secret material.
pub fn authenticate<S: Read + Write>(
    stream: &mut S,
    secret: &[u8],
    role: WireRole,
    credential_id: &str,
    uid: u32,
    client_nonce: &str,
) -> Result<Authenticated, AuthError> {
    let challenge: Challenge = wire::read_message(stream)?;
    if challenge.version != PROTOCOL_VERSION {
        return Err(AuthError::UnsupportedVersion(challenge.version));
    }
    if !valid_nonce(&challenge.nonce) {
        return Err(AuthError::BadChallengeNonce);
    }

    let context = ProofContext {
        server_nonce: &challenge.nonce,
        client_nonce,
        role,
        credential_id,
        uid,
    };
    let client_proof = context.proof(secret, DIRECTION_CLIENT);
    let request = Authenticate {
        version: PROTOCOL_VERSION,
        role,
        credential_id: credential_id.to_owned(),
        client_nonce: client_nonce.to_owned(),
        proof: client_proof.clone(),
    };
    wire::write_message(stream, &request)?;

    match wire::read_message::<_, AuthenticationResult>(stream)? {
        AuthenticationResult::Rejected { .. } => Err(AuthError::Rejected),
        AuthenticationResult::Accepted {
            display_name,
            proof,
        } => {
            if proof == client_proof {
                return Err(AuthError::BadBrokerProof);
            }
            if !context.verify(secret, DIRECTION_BROKER, &proof) {
                return Err(AuthError::BadBrokerProof);
            }
            Ok(Authenticated { display_name })
        }
    }
}

/// Authenticates the version-3 commit-report extension on one connected stream.
///
/// The challenge remains exactly version 2. Acceptance must prove version 3
/// and the exact ordered [`COMMIT_REPORT_CAPABILITIES`] before registration.
/// This function never falls back or reconnects. A connector may retry legacy
/// authentication on a new stream after unsupported-auth closure, within its
/// original total budget; proof/shape/capability failures must fail closed.
///
/// # Errors
/// Returns framing, nonce, version, rejection, capability or broker-proof errors.
pub fn authenticate_v3<S: Read + Write>(
    stream: &mut S,
    secret: &[u8],
    role: WireRole,
    credential_id: &str,
    uid: u32,
    client_nonce: &str,
) -> Result<Authenticated, AuthError> {
    if !valid_nonce(client_nonce) {
        return Err(AuthError::BadClientNonce);
    }
    let challenge: Challenge = wire::read_message(stream)?;
    if challenge.version != PROTOCOL_VERSION {
        return Err(AuthError::UnsupportedVersion(challenge.version));
    }
    if !valid_nonce(&challenge.nonce) {
        return Err(AuthError::BadChallengeNonce);
    }
    let context = ProofContext {
        server_nonce: &challenge.nonce,
        client_nonce,
        role,
        credential_id,
        uid,
    };
    wire::write_message(
        stream,
        &Authenticate {
            version: 3,
            role,
            credential_id: credential_id.to_owned(),
            client_nonce: client_nonce.to_owned(),
            proof: context.v3_client_proof(secret),
        },
    )?;
    let mut acceptance = AcceptanceReader {
        stream,
        received: 0,
    };
    let response = wire::read_message::<_, AuthenticationResultV3>(&mut acceptance);
    let response = match response {
        Err(WireError::PeerClosed) if acceptance.received == 0 => {
            return Err(AuthError::V3Unsupported);
        }
        Err(WireError::PeerClosed | WireError::Truncated { .. }) => {
            return Err(WireError::Malformed.into());
        }
        other => other?,
    };
    match response {
        AuthenticationResultV3::Rejected { .. } => Err(AuthError::Rejected),
        AuthenticationResultV3::Accepted {
            display_name,
            proof,
            wire_version,
            capabilities,
        } => {
            if wire_version != 3 {
                return Err(AuthError::UnsupportedVersion(wire_version));
            }
            if !capabilities
                .iter()
                .map(String::as_str)
                .eq(COMMIT_REPORT_CAPABILITIES)
            {
                return Err(AuthError::BadCapabilities);
            }
            if !context.verify_v3_broker(secret, &COMMIT_REPORT_CAPABILITIES, &proof) {
                return Err(AuthError::BadBrokerProof);
            }
            Ok(Authenticated { display_name })
        }
    }
}

// Only an empty acceptance permits legacy fallback; truncated frames fail closed.
struct AcceptanceReader<'a, S> {
    stream: &'a mut S,
    received: usize,
}

impl<S: Read> Read for AcceptanceReader<'_, S> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.stream.read(buffer)?;
        self.received += count;
        Ok(count)
    }
}

#[cfg(test)]
mod v3_tests {
    use super::*;
    use serde_json::{Value, json};
    use std::io::Cursor;

    const BROKER: &str = "00504b612b3258aa5110eea88480de461224790107e1d356193c28c8c2e798be";

    struct Exchange {
        incoming: Cursor<Vec<u8>>,
        outgoing: Vec<u8>,
    }
    impl Read for Exchange {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            self.incoming.read(bytes)
        }
    }
    impl Write for Exchange {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.outgoing.write(bytes)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    fn exchange(challenge: Value, acceptance: Value) -> Exchange {
        let mut frames = Vec::new();
        wire::write_message(&mut frames, &challenge).unwrap();
        wire::write_message(&mut frames, &acceptance).unwrap();
        Exchange {
            incoming: Cursor::new(frames),
            outgoing: Vec::new(),
        }
    }
    fn challenge() -> Value {
        json!({"version": 2, "nonce": "00".repeat(32)})
    }
    fn accepted() -> Value {
        json!({"status":"accepted", "display_name":"Example", "proof":BROKER,
            "wire_version":3,"capabilities":COMMIT_REPORT_CAPABILITIES})
    }
    fn drive(stream: &mut Exchange) -> Result<Authenticated, AuthError> {
        authenticate_v3(
            stream,
            &[7; 32],
            WireRole::App,
            "org.enzui.Example",
            1000,
            &"11".repeat(32),
        )
    }

    #[test]
    fn v3_driver_refuses_invalid_nonces_or_challenges_before_writing_authentication() {
        for response in [
            json!({"version":3,"nonce":"00".repeat(32)}),
            json!({"version":2,"nonce":"0A".repeat(32)}),
            json!({"version":2,"nonce":"0"}),
            json!({"version":2,"nonce":"00".repeat(32),"capabilities":[]}),
        ] {
            let mut stream = exchange(response, accepted());
            assert!(drive(&mut stream).is_err());
            assert!(stream.outgoing.is_empty());
        }
        let mut stream = exchange(challenge(), accepted());
        assert!(matches!(
            authenticate_v3(
                &mut stream,
                &[7; 32],
                WireRole::App,
                "org.enzui.Example",
                1000,
                "invalid"
            ),
            Err(AuthError::BadClientNonce)
        ));
        assert!(stream.outgoing.is_empty());
        assert_eq!(stream.incoming.position(), 0);
    }
}
