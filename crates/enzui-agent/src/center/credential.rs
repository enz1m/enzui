//! Secure loading of a version-1 app credential file.
//!
//! The file is opened once with the same descriptor-bound discipline as every
//! other framework secret: no symlink at the final component, regular file,
//! owned by the current uid, and no group/other/special mode bits.
//!
//! The raw file bytes are a wipe-on-drop [`Secret`]. To keep the hex secret
//! from also surviving in a plain `String` that frees without wiping, this
//! loader deserializes through a private shape whose secret field moves its
//! text straight into a [`SecretText`]. Both the file bytes and that text are
//! wiped when the loader returns, on the success path and every error path
//! alike; the decoded 32-byte key is then held as its own wiped [`Secret`].

use std::path::Path;

use serde::Deserialize;
use serde::de::{self, Deserializer, Visitor};

use crate::center::CentralError;
use crate::center::protocol::{CREDENTIAL_VERSION, CredentialKind};
use crate::secret::{Secret, SecretText};

/// An enrolled app credential: its non-secret identity and its wiped key.
pub struct AppCredential {
    /// The app id, which must equal the registration's app id.
    pub id: String,
    /// The daemon-assigned display name.
    pub display_name: String,
    /// The 32-byte HMAC key, wiped on drop.
    pub secret: Secret,
}

impl core::fmt::Debug for AppCredential {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AppCredential")
            .field("id", &self.id)
            .field("display_name", &self.display_name)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

// The credential file as parsed by the loader. It matches the public wire shape
// of `protocol::CredentialFile` field for field, but its secret is a wiped
// `SecretText` so the plaintext never lands in a `String` that frees unwiped.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ParsedCredential {
    version: u32,
    kind: CredentialKind,
    id: String,
    display_name: String,
    secret: HexSecret,
}

// A JSON string deserialized directly into a wipe-on-drop `SecretText`. The
// visitor moves owned text into the `SecretText` (or copies a borrowed slice
// out of the already-wiped file buffer), so no separate plaintext `String`
// outlives this value.
struct HexSecret(SecretText);

impl<'de> Deserialize<'de> for HexSecret {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct HexSecretVisitor;

        impl<'de> Visitor<'de> for HexSecretVisitor {
            type Value = HexSecret;

            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("a hex-encoded credential secret string")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(HexSecret(SecretText::new(value.to_owned())))
            }

            fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(HexSecret(SecretText::new(value)))
            }
        }

        deserializer.deserialize_string(HexSecretVisitor)
    }
}

/// Loads and validates an app credential file from `path`.
///
/// # Errors
/// Returns [`CentralError::Credential`] for a file that fails the secure-open
/// checks, or [`CentralError::CredentialFormat`] for a malformed, non-app, or
/// wrong-version credential.
pub fn load_app_credential(path: &Path) -> Result<AppCredential, CentralError> {
    let file_bytes = crate::secret::load_secret(path).map_err(CentralError::Credential)?;
    let parsed: ParsedCredential = serde_json::from_slice(file_bytes.expose_secret())
        .map_err(|_| CentralError::CredentialFormat("not a credential file"))?;
    // `file_bytes` and `parsed.secret` are wipe-on-drop; `parsed` is dropped
    // when this function returns, so the checks below fail closed with the
    // plaintext wiped.
    if parsed.version != CREDENTIAL_VERSION {
        return Err(CentralError::CredentialFormat("unsupported version"));
    }
    if !matches!(parsed.kind, CredentialKind::App) {
        return Err(CentralError::CredentialFormat("credential is not an app"));
    }
    let key = {
        let hex = parsed.secret.0.expose_secret();
        if hex.len() != 64 {
            return Err(CentralError::CredentialFormat("secret length"));
        }
        hex::decode(hex).map_err(|_| CentralError::CredentialFormat("secret hex"))?
    };
    Ok(AppCredential {
        id: parsed.id,
        display_name: parsed.display_name,
        secret: Secret::new(key),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    fn write_mode_600(path: &Path, contents: &[u8]) {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .expect("create credential");
        file.write_all(contents).expect("write credential");
    }

    #[test]
    fn loads_a_valid_app_credential_and_wipes_its_key() {
        let dir = crate::center::tests_support::scratch_dir("cred-ok");
        let path = dir.join("app.cred");
        let secret_hex = "07".repeat(32);
        write_mode_600(
            &path,
            format!(
                "{{\"version\":1,\"kind\":\"app\",\"id\":\"org.enzui.Example\",\"display_name\":\"Example\",\"secret\":\"{secret_hex}\"}}"
            )
            .as_bytes(),
        );
        let credential = load_app_credential(&path).expect("load");
        assert_eq!(credential.id, "org.enzui.Example");
        assert_eq!(credential.display_name, "Example");
        assert_eq!(credential.secret.expose_secret(), &[7u8; 32]);
        assert!(
            !format!("{credential:?}").contains(&secret_hex),
            "debug must not leak the key"
        );
    }
}
