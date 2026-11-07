//! App wire types for version 2 and separately negotiated extension acceptance.
//!
//! These mirror the authoritative JSON shapes in the daemon's `src/protocol.rs`
//! for the app connection only: the credential file, the authentication
//! handshake, app registration, and the invocation lifecycle. Agent- and
//! control-role requests are the daemon's concern and are intentionally absent.
//! Security-sensitive envelopes reject unknown fields so a renamed or injected
//! field is refused rather than ignored.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use enzui_security::random::{self, RandomError};

/// Why a credential file is not a usable app credential.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CredentialFormatError(pub &'static str);

impl core::fmt::Display for CredentialFormatError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "credential format: {}", self.0)
    }
}

impl std::error::Error for CredentialFormatError {}

/// The wire protocol version this connector speaks. V2 has no insecure
/// fallback; a daemon offering any other version is refused.
pub const PROTOCOL_VERSION: u32 = 2;

/// The on-disk credential file version, which stays `1` independently of the
/// wire version.
pub const CREDENTIAL_VERSION: u32 = 1;

/// A UUID carried as its canonical hyphenated lowercase string.
///
/// The daemon generates every invocation, caller session, and request id as a
/// UUID string; this newtype validates that shape on the wire without pulling
/// in a UUID dependency. [`Uuid::new_v4`] mints a fresh identity from the OS
/// CSPRNG for the connector's own request ids.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Uuid(String);

impl Uuid {
    /// Mints a random version-4 UUID from the shared entropy boundary.
    ///
    /// # Errors
    /// Returns [`RandomError`] when the OS entropy source is unavailable; the
    /// caller fails closed rather than reusing a predictable id.
    pub fn new_v4() -> Result<Self, RandomError> {
        let mut bytes: [u8; 16] = random::random_array()?;
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let mut text = hex::encode(bytes);
        for at in [8, 13, 18, 23] {
            text.insert(at, '-');
        }
        Ok(Self(text))
    }

    /// The canonical hyphenated string form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn parse(text: &str) -> Result<Self, ()> {
        let bytes = text.as_bytes();
        if bytes.len() != 36 {
            return Err(());
        }
        for (index, byte) in bytes.iter().enumerate() {
            let is_dash_pos = matches!(index, 8 | 13 | 18 | 23);
            if is_dash_pos {
                if *byte != b'-' {
                    return Err(());
                }
            } else if !byte.is_ascii_hexdigit() {
                return Err(());
            }
        }
        Ok(Self(text.to_ascii_lowercase()))
    }
}

impl core::fmt::Display for Uuid {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Uuid {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(|()| serde::de::Error::custom("invalid uuid"))
    }
}

/// The role a credential and connection assume during authentication. The
/// connector always authenticates as [`WireRole::App`].
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireRole {
    /// An enrolled agent that calls app functions.
    Agent,
    /// An app that registers and serves functions.
    App,
    /// The local control client.
    Control,
}

impl WireRole {
    /// The lowercase transcript token bound into the authentication proof.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::App => "app",
            Self::Control => "control",
        }
    }
}

/// The kind field of a credential file, which must match its role.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    /// An agent credential.
    Agent,
    /// An app credential.
    App,
    /// A control credential.
    Control,
}

/// A version-1 credential file as written by `enzui-center enroll`.
///
/// The `secret` is 64 lowercase hex characters decoding to a 32-byte HMAC key.
/// Its [`Debug`] impl redacts the secret; the loaded key is held as a wiped
/// [`enzui_security::secret::Secret`] rather than in this struct.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialFile {
    /// The file format version, [`CREDENTIAL_VERSION`].
    pub version: u32,
    /// The role this credential authenticates.
    pub kind: CredentialKind,
    /// The credential id the daemon issued.
    pub id: String,
    /// The name the daemon shows for this peer.
    pub display_name: String,
    /// The HMAC key, as 64 lowercase hex characters.
    pub secret: String,
}

impl core::fmt::Debug for CredentialFile {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CredentialFile")
            .field("version", &self.version)
            .field("kind", &self.kind)
            .field("id", &self.id)
            .field("display_name", &self.display_name)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

/// Daemon → peer: the first frame, carrying the server nonce to prove against.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Challenge {
    /// The wire protocol version the daemon speaks.
    pub version: u32,
    /// The server nonce, hex-encoded.
    pub nonce: String,
}

/// Peer → daemon: the client nonce and the client-direction proof.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Authenticate {
    /// The wire protocol version the peer speaks.
    pub version: u32,
    /// The role the peer authenticates as.
    pub role: WireRole,
    /// The credential the proof is made with.
    pub credential_id: String,
    /// The peer's nonce, hex-encoded.
    pub client_nonce: String,
    /// The client-direction HMAC proof, hex-encoded.
    pub proof: String,
}

/// Daemon → peer: the authentication outcome, with the broker-direction proof
/// on acceptance.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthenticationResult {
    /// The daemon accepted the proof.
    Accepted {
        /// The name the daemon knows this peer by.
        display_name: String,
        /// The broker-direction HMAC proof, hex-encoded.
        proof: String,
    },
    /// The daemon refused the proof.
    Rejected {
        /// Why it refused.
        message: String,
    },
}

/// Strict acceptance for the separately negotiated commit-report extension.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthenticationResultV3 {
    /// The daemon accepted the proof and the extension.
    Accepted {
        /// The name the daemon knows this peer by.
        display_name: String,
        /// The broker-direction HMAC proof, hex-encoded.
        proof: String,
        /// The wire version the daemon agreed to.
        wire_version: u32,
        /// The extensions the daemon agreed to.
        capabilities: Vec<String>,
    },
    /// The daemon refused the proof.
    Rejected {
        /// Why it refused.
        message: String,
    },
}

/// How dangerous a function's effect is, mirroring the daemon's risk ladder.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FunctionRisk {
    /// Reads state that is not sensitive.
    ReadOnly,
    /// Reads private or sensitive data.
    SensitiveRead,
    /// Changes app state.
    Mutating,
    /// Deletes data or changes it irreversibly.
    Destructive,
    /// Acts outside the app, such as sending a message.
    ExternalSideEffect,
}

/// A consent duration a function is willing to accept. The declared set is an
/// upper bound; the daemon remains the authority on what is granted.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsentScope {
    /// Consent covers one call.
    Once,
    /// Consent lasts for the agent's session.
    Session,
    /// Consent lasts until revoked.
    Always,
}

/// The full descriptor of one registered function.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionDescriptor {
    /// A stable id, unique within the app.
    pub id: String,
    /// A short human-readable name.
    pub title: String,
    /// What the function does, shown when consent is asked.
    pub description: String,
    /// JSON Schema for the arguments.
    pub input_schema: Value,
    /// JSON Schema for the result.
    pub output_schema: Value,
    /// How dangerous the effect is.
    pub risk: FunctionRisk,
    /// The consent scopes the function accepts.
    pub allowed_consent: Vec<ConsentScope>,
    /// Capabilities a caller must hold.
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    /// Whether a running call can be cancelled.
    #[serde(default)]
    pub supports_cancellation: bool,
    /// Whether the function reports progress.
    #[serde(default)]
    pub supports_progress: bool,
    /// The largest result it may return, at most [`MAX_RESULT_BYTES`].
    #[serde(default = "default_result_bytes")]
    pub max_result_bytes: u32,
}

/// The daemon's default per-call result byte budget.
pub const DEFAULT_RESULT_BYTES: u32 = 64 * 1024;

/// The daemon's hard maximum per-call result byte budget.
pub const MAX_RESULT_BYTES: u32 = 512 * 1024;

const fn default_result_bytes() -> u32 {
    DEFAULT_RESULT_BYTES
}

/// App identity carried in a registration.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppMetadata {
    /// The app's stable id.
    pub id: String,
    /// The app's display name.
    pub name: String,
    /// What the app is for.
    pub description: String,
    /// The app's version string.
    pub version: String,
}

/// The first post-authentication app message: identity plus its functions.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppRegistration {
    /// Who the app is.
    pub app: AppMetadata,
    /// The functions it offers.
    pub functions: Vec<FunctionDescriptor>,
}

/// App → daemon: registration, then one event per invocation transition.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AppToBroker {
    /// The first message: who the app is and what it offers.
    Register {
        /// The identity and functions.
        registration: AppRegistration,
    },
    /// One invocation transition.
    Event {
        /// The transition.
        event: InvocationEvent,
    },
}

/// Daemon → app: registration outcome, then invocation dispatch and cancel.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrokerToApp {
    /// The registration was accepted.
    Registered,
    /// The registration was refused.
    RegistrationRejected {
        /// Why it was refused.
        message: String,
    },
    /// Run a function.
    Invoke {
        /// The call to run.
        invocation: Invocation,
    },
    /// Stop a running call.
    Cancel {
        /// The call to stop.
        invocation_id: Uuid,
        /// Why it is being stopped.
        reason: String,
    },
}

/// The broker's answer to a commit report. Only [`Self::Persisted`] and
/// [`Self::AlreadyPersisted`] confirm durable metadata; none of them confirms
/// that the original caller received anything.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitReportDisposition {
    /// The report was stored.
    Persisted,
    /// The same report was stored earlier; nothing new was written.
    AlreadyPersisted,
    /// The broker does not know the invocation (never admitted, or pruned).
    UnknownInvocation,
    /// The invocation's retention window ended.
    Expired,
    /// The broker refused the report (an ineligible call or a rate limit).
    Refused,
    /// Storage failed; the report may be retried on the same connection.
    StorageUnavailable,
}

impl CommitReportDisposition {
    /// Whether the broker durably stored the report.
    #[must_use]
    pub const fn is_durable(self) -> bool {
        matches!(self, Self::Persisted | Self::AlreadyPersisted)
    }
}

/// App → daemon frames that only a negotiated version-3 connection carries.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AppToBrokerV3 {
    /// The app reports that the invocation's side effect committed.
    CommitReport {
        /// The call whose effect committed.
        invocation_id: Uuid,
    },
}

/// Daemon → app on a negotiated version-3 connection: every version-2 frame,
/// plus the commit-report acknowledgement. A version-2 connection keeps
/// parsing [`BrokerToApp`], so this frame ends such a session.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrokerToAppV3 {
    /// The registration was accepted.
    Registered,
    /// The registration was refused.
    RegistrationRejected {
        /// Why it was refused.
        message: String,
    },
    /// Run a function.
    Invoke {
        /// The call to run.
        invocation: Invocation,
    },
    /// Stop a running call.
    Cancel {
        /// The call to stop.
        invocation_id: Uuid,
        /// Why it is being stopped.
        reason: String,
    },
    /// The answer to a commit report.
    CommitReportAck {
        /// The reported call.
        invocation_id: Uuid,
        /// What the broker did with the report.
        disposition: CommitReportDisposition,
    },
}

/// The enrolled agent on whose behalf an invocation runs.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCaller {
    /// The agent's credential id.
    pub credential_id: String,
    /// The agent's display name.
    pub display_name: String,
    /// The agent session the call belongs to.
    pub session_id: Uuid,
}

/// One dispatched call. The `invocation_id` is generated by the daemon, so a
/// reused agent request id can never make a late event complete a new call.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Invocation {
    /// The daemon-generated id of this call.
    pub invocation_id: Uuid,
    /// The function to run.
    pub function_id: String,
    /// The arguments, matching the input schema.
    pub arguments: Value,
    /// The agent that asked.
    pub caller: AgentCaller,
    /// When the call must end, in Unix milliseconds.
    pub deadline_unix_ms: u64,
    /// The caller's key for deduplicating retries, if any.
    pub idempotency_key: Option<String>,
}

/// One transition an app reports for an invocation. Only `completed`, `failed`,
/// and `cancelled` are terminal.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum InvocationEvent {
    /// The app started the call.
    Accepted {
        /// The call this event is about.
        invocation_id: Uuid,
    },
    /// The call made progress.
    Progress {
        /// The call this event is about.
        invocation_id: Uuid,
        /// The fraction done, from 0 to 1.
        completed: f32,
        /// An optional status line.
        message: Option<String>,
    },
    /// The call succeeded.
    Completed {
        /// The call this event is about.
        invocation_id: Uuid,
        /// The result, matching the output schema.
        result: Value,
    },
    /// The call failed.
    Failed {
        /// The call this event is about.
        invocation_id: Uuid,
        /// A stable failure code.
        code: String,
        /// A human-readable explanation.
        message: String,
    },
    /// The call stopped on request.
    Cancelled {
        /// The call this event is about.
        invocation_id: Uuid,
    },
}

impl InvocationEvent {
    /// The invocation this event belongs to.
    #[must_use]
    pub const fn invocation_id(&self) -> &Uuid {
        match self {
            Self::Accepted { invocation_id }
            | Self::Progress { invocation_id, .. }
            | Self::Completed { invocation_id, .. }
            | Self::Failed { invocation_id, .. }
            | Self::Cancelled { invocation_id } => invocation_id,
        }
    }

    /// Whether this event completes the invocation for the daemon.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Failed { .. } | Self::Cancelled { .. }
        )
    }
}

impl CredentialFile {
    /// Decodes the 64-hex-character secret into its 32 raw key bytes.
    ///
    /// # Errors
    /// Returns [`CredentialFormatError`] when the field is not 64
    /// lowercase hex characters or the version/kind are not app credentials.
    pub fn app_secret_bytes(&self) -> Result<Vec<u8>, CredentialFormatError> {
        if self.version != CREDENTIAL_VERSION {
            return Err(CredentialFormatError("unsupported version"));
        }
        if !matches!(self.kind, CredentialKind::App) {
            return Err(CredentialFormatError("credential is not an app"));
        }
        if self.secret.len() != 64 {
            return Err(CredentialFormatError("secret length"));
        }
        hex::decode(&self.secret).map_err(|_| CredentialFormatError("secret hex"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn uuid_new_v4_has_version_and_variant_bits() {
        let uuid = Uuid::new_v4().expect("entropy");
        let text = uuid.as_str();
        assert_eq!(text.len(), 36);
        assert_eq!(text.as_bytes()[14], b'4', "version nibble must be 4");
        let variant = text.as_bytes()[19];
        assert!(
            matches!(variant, b'8' | b'9' | b'a' | b'b'),
            "variant nibble was {}",
            variant as char
        );
    }

    #[test]
    fn uuid_deserialize_rejects_malformed_and_accepts_canonical() {
        let good: Uuid =
            serde_json::from_value(json!("2f3b1c9e-0a4d-4e7f-8b21-1c2d3e4f5a6b")).expect("valid");
        assert_eq!(good.as_str(), "2f3b1c9e-0a4d-4e7f-8b21-1c2d3e4f5a6b");
        for bad in ["", "not-a-uuid", "2f3b1c9e0a4d4e7f8b211c2d3e4f5a6b"] {
            assert!(
                serde_json::from_value::<Uuid>(json!(bad)).is_err(),
                "must reject {bad:?}"
            );
        }
    }

    #[test]
    fn unknown_field_on_a_security_envelope_is_rejected() {
        let with_extra = json!({
            "version": 2,
            "nonce": "00",
            "smuggled": true
        });
        assert!(
            serde_json::from_value::<Challenge>(with_extra).is_err(),
            "deny_unknown_fields must refuse an injected field"
        );
    }
}
