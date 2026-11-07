//! The credential store for sandboxed applications, through the Secret portal.
//!
//! Inside Flatpak the Secret Service is out of reach. The
//! `org.freedesktop.portal.Secret` portal instead hands each application one
//! master secret of its own, kept by the host's keyring; the application
//! encrypts its credentials with it in a file of its own data directory
//! (`$XDG_DATA_HOME/<app id>/credentials.keyring`, which Flatpak keeps under
//! `~/.var/app/<app id>/`). This is what libsecret does in a sandbox.
//!
//! The file holds every credential of the application, sealed with
//! ChaCha20-Poly1305 under a key derived (HKDF-SHA256) from the master
//! secret and bound to the application id; each write uses a fresh nonce and
//! replaces the file atomically. A file that does not decrypt is reported as
//! a failure and never overwritten, so a lost master secret cannot quietly
//! erase what was stored. Plaintext buffers are wiped after use, and the
//! work runs off the caller's thread.
//!
//! [`connect_for_app`](super::connect_for_app) chooses this store inside a
//! sandbox and the Secret Service outside one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use enzui_security::secret::{Secret, wipe_bytes};

use super::{CredentialKey, SecretTransport, TransportFuture, TransportOutcome};

/// The keyring file inside the application's data directory.
pub const KEYRING_FILE: &str = "credentials.keyring";

/// The file's leading bytes: format name and version.
const MAGIC: &[u8; 8] = b"ENZKRNG1";

/// The largest keyring file read; a larger one is refused as damaged.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Why the portal store could not be set up.
#[derive(Debug)]
pub enum PortalStoreError {
    /// The application id cannot name a data directory.
    AppId(enzui_core::app_data::AppDataError),
    /// The data directory could not be found or created.
    DataDirectory(enzui_core::app_data::AppDataError),
    /// The Secret portal could not be reached or refused the request.
    Portal(String),
    /// The portal answered with no secret.
    EmptySecret,
}

impl std::fmt::Display for PortalStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AppId(error) => write!(f, "invalid application id: {error}"),
            Self::DataDirectory(error) => write!(f, "no data directory: {error}"),
            Self::Portal(reason) => write!(f, "the Secret portal failed: {reason}"),
            Self::EmptySecret => f.write_str("the Secret portal returned no secret"),
        }
    }
}

impl std::error::Error for PortalStoreError {}

/// A [`SecretTransport`] keeping one application's credentials in a file
/// sealed with the master secret the Secret portal gives it.
#[derive(Clone)]
pub struct PortalSecretTransport {
    inner: Arc<Inner>,
}

struct Inner {
    app_id: String,
    path: PathBuf,
    key: Key,
    /// Serializes this transport's (and its clones') read-modify-write
    /// cycles; [`enzui_core::atomic_state::StateLock`] serializes them with
    /// other transports on the same file.
    file: Mutex<()>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = wipe_bytes(self.key.as_mut_slice());
    }
}

impl std::fmt::Debug for PortalSecretTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortalSecretTransport")
            .field("app_id", &self.inner.app_id)
            .field("path", &self.inner.path)
            .finish_non_exhaustive()
    }
}

impl PortalSecretTransport {
    /// Ask the Secret portal for `app_id`'s master secret and open its
    /// keyring in the application's data directory.
    ///
    /// # Errors
    /// [`PortalStoreError`] when the id is unusable, the data directory
    /// cannot be created, or the portal fails.
    pub async fn connect(app_id: &str) -> Result<Self, PortalStoreError> {
        let connection = zbus::Connection::session()
            .await
            .map_err(|error| PortalStoreError::Portal(error.to_string()))?;
        Self::connect_with(
            app_id,
            connection,
            &enzui_core::app_data::AppDataResolver::from_env(),
        )
        .await
    }

    /// [`Self::connect`] on an existing session-bus connection, with the data
    /// directory found by `resolver` (`AppDataResolver::from_env()` for the
    /// process's own).
    ///
    /// # Errors
    /// As [`Self::connect`].
    pub async fn connect_with(
        app_id: &str,
        connection: zbus::Connection,
        resolver: &enzui_core::app_data::AppDataResolver,
    ) -> Result<Self, PortalStoreError> {
        let app = enzui_core::app_data::AppId::new(app_id).map_err(PortalStoreError::AppId)?;
        let directory = resolver
            .ensure_tightening(&app)
            .map_err(PortalStoreError::DataDirectory)?;
        let master = retrieve_master_secret(connection).await?;
        Ok(Self::with_master_secret(
            app_id,
            directory.path().join(KEYRING_FILE),
            &master,
        ))
    }

    /// A transport for `app_id` keeping its keyring at `path`, sealed with
    /// `master`. [`Self::connect`] gets the master secret from the portal;
    /// this constructor is for hosts that obtain it another way, and tests.
    #[must_use]
    pub fn with_master_secret(app_id: &str, path: impl Into<PathBuf>, master: &Secret) -> Self {
        let mut okm = [0u8; 32];
        let hkdf = hkdf::Hkdf::<sha2::Sha256>::new(
            Some(b"enzui credential keyring v1"),
            master.expose_secret(),
        );
        // A 32-byte output is always within HKDF-SHA256's limit.
        let _ = hkdf.expand(app_id.as_bytes(), &mut okm);
        let key = Key::clone_from_slice(&okm);
        let _ = wipe_bytes(&mut okm);
        Self {
            inner: Arc::new(Inner {
                app_id: app_id.to_owned(),
                path: path.into(),
                key,
                file: Mutex::new(()),
            }),
        }
    }

    /// Where the keyring is kept.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// Run `work` on the keyring off the caller's thread.
    fn run(
        &self,
        work: impl FnOnce(&Inner) -> TransportOutcome + Send + 'static,
    ) -> TransportFuture {
        let inner = Arc::clone(&self.inner);
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let spawned = std::thread::Builder::new()
            .name("enzui-keyring".into())
            .spawn(move || {
                let outcome = {
                    let _guard = inner
                        .file
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    // Another transport opened on the same keyring (here or in
                    // another process) takes its turn too, so no update is lost.
                    match enzui_core::atomic_state::StateLock::acquire(&inner.path) {
                        Ok(_file_turn) => work(&inner),
                        Err(error) => TransportOutcome::Failed(format!(
                            "could not lock the keyring: {}",
                            error.kind()
                        )),
                    }
                };
                let _ = sender.send(outcome);
            });
        Box::pin(async move {
            if spawned.is_err() {
                return TransportOutcome::Failed("could not start the keyring worker".into());
            }
            receiver
                .await
                .unwrap_or_else(|_| TransportOutcome::Failed("the keyring worker stopped".into()))
        })
    }
}

impl SecretTransport for PortalSecretTransport {
    fn get(&self, key: &CredentialKey) -> TransportFuture {
        let key = key.clone();
        self.run(move |inner| {
            if key.app_id() != inner.app_id {
                return TransportOutcome::PermissionDenied;
            }
            match inner.load() {
                Ok(mut entries) => entries
                    .remove(&(key.owner().to_owned(), key.label().to_owned()))
                    .map_or(TransportOutcome::NotFound, TransportOutcome::Found),
                Err(failure) => failure,
            }
        })
    }

    fn set(&self, key: &CredentialKey, secret: Secret) -> TransportFuture {
        let key = key.clone();
        self.run(move |inner| {
            if key.app_id() != inner.app_id {
                return TransportOutcome::PermissionDenied;
            }
            let mut entries = match inner.load() {
                Ok(entries) => entries,
                Err(failure) => return failure,
            };
            let _ = entries.insert((key.owner().to_owned(), key.label().to_owned()), secret);
            inner
                .save(&entries)
                .map_or_else(|failure| failure, |()| TransportOutcome::Stored)
        })
    }

    fn delete(&self, key: &CredentialKey) -> TransportFuture {
        let key = key.clone();
        self.run(move |inner| {
            if key.app_id() != inner.app_id {
                return TransportOutcome::PermissionDenied;
            }
            let mut entries = match inner.load() {
                Ok(entries) => entries,
                Err(failure) => return failure,
            };
            if entries
                .remove(&(key.owner().to_owned(), key.label().to_owned()))
                .is_none()
            {
                return TransportOutcome::Deleted;
            }
            inner
                .save(&entries)
                .map_or_else(|failure| failure, |()| TransportOutcome::Deleted)
        })
    }
}

type Entries = BTreeMap<(String, String), Secret>;

impl Inner {
    /// The stored credentials; an absent file holds none.
    fn load(&self) -> Result<Entries, TransportOutcome> {
        let sealed = match read_bounded(&self.path) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Ok(Entries::new()),
            Err(reason) => return Err(TransportOutcome::Failed(reason)),
        };
        let Some(body) = sealed.strip_prefix(MAGIC.as_slice()) else {
            return Err(TransportOutcome::Failed(
                "the keyring file is not a keyring".into(),
            ));
        };
        if body.len() < 12 {
            return Err(TransportOutcome::Failed(
                "the keyring file is truncated".into(),
            ));
        }
        let (nonce, ciphertext) = body.split_at(12);
        let mut plain = ChaCha20Poly1305::new(&self.key)
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: MAGIC,
                },
            )
            .map_err(|_| {
                TransportOutcome::Failed(
                    "the keyring does not decrypt with this application's secret".into(),
                )
            })?;
        let entries = decode(&plain);
        let _ = wipe_bytes(&mut plain);
        entries.ok_or_else(|| TransportOutcome::Failed("the keyring contents are damaged".into()))
    }

    /// Seal and atomically replace the file.
    fn save(&self, entries: &Entries) -> Result<(), TransportOutcome> {
        let mut plain = encode(entries);
        let nonce = enzui_security::random::random_array::<12>()
            .map_err(|_| TransportOutcome::Failed("no randomness for the keyring".into()));
        let sealed = nonce.and_then(|nonce| {
            ChaCha20Poly1305::new(&self.key)
                .encrypt(
                    Nonce::from_slice(&nonce),
                    Payload {
                        msg: &plain,
                        aad: MAGIC,
                    },
                )
                .map(|ciphertext| [MAGIC.as_slice(), &nonce, &ciphertext].concat())
                .map_err(|_| TransportOutcome::Failed("could not seal the keyring".into()))
        });
        let _ = wipe_bytes(&mut plain);
        let sealed = sealed?;
        // The reader refuses a larger file, so writing one would lock the
        // application out of every credential; the previous file stays.
        if sealed.len() as u64 > MAX_FILE_BYTES {
            return Err(TransportOutcome::Failed(
                "the keyring would exceed its size limit; nothing was changed".into(),
            ));
        }
        enzui_core::atomic_state::save_atomic_raw(&self.path, &sealed).map_err(|error| {
            TransportOutcome::Failed(format!("could not write the keyring: {}", error.kind()))
        })
    }
}

/// The file's bytes, `None` when it does not exist.
fn read_bounded(path: &Path) -> Result<Option<Vec<u8>>, String> {
    use std::io::Read as _;
    // No-follow and nonblocking, checked to be a regular file on the open
    // descriptor: a planted FIFO or device cannot strand the worker.
    let file = match enzui_core::atomic_state::open_state_file_nofollow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not read the keyring: {}", error.kind())),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read the keyring: {}", error.kind()))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("the keyring file is too large".into());
    }
    Ok(Some(bytes))
}

/// `count, then (owner, label, secret)` each as a length and its bytes.
fn encode(entries: &Entries) -> Vec<u8> {
    let mut out = Vec::new();
    let field = |out: &mut Vec<u8>, bytes: &[u8]| {
        out.extend_from_slice(&u32::try_from(bytes.len()).unwrap_or(u32::MAX).to_le_bytes());
        out.extend_from_slice(bytes);
    };
    out.extend_from_slice(
        &u32::try_from(entries.len())
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    for ((owner, label), secret) in entries {
        field(&mut out, owner.as_bytes());
        field(&mut out, label.as_bytes());
        field(&mut out, secret.expose_secret());
    }
    out
}

fn decode(mut bytes: &[u8]) -> Option<Entries> {
    fn take<'a>(bytes: &mut &'a [u8], len: usize) -> Option<&'a [u8]> {
        if bytes.len() < len {
            return None;
        }
        let (head, rest) = bytes.split_at(len);
        *bytes = rest;
        Some(head)
    }
    fn number(bytes: &mut &[u8]) -> Option<usize> {
        let raw: [u8; 4] = take(bytes, 4)?.try_into().ok()?;
        usize::try_from(u32::from_le_bytes(raw)).ok()
    }
    fn field<'a>(bytes: &mut &'a [u8]) -> Option<&'a [u8]> {
        let len = number(bytes)?;
        take(bytes, len)
    }
    let count = number(&mut bytes)?;
    let mut entries = Entries::new();
    for _ in 0..count {
        let owner = String::from_utf8(field(&mut bytes)?.to_vec()).ok()?;
        let label = String::from_utf8(field(&mut bytes)?.to_vec()).ok()?;
        let secret = Secret::new(field(&mut bytes)?.to_vec());
        let _ = entries.insert((owner, label), secret);
    }
    bytes.is_empty().then_some(entries)
}

/// The application's master secret from `org.freedesktop.portal.Secret`.
async fn retrieve_master_secret(connection: zbus::Connection) -> Result<Secret, PortalStoreError> {
    use std::io::Read as _;
    let portal = ashpd::desktop::secret::Secret::with_connection(connection)
        .await
        .map_err(|error| PortalStoreError::Portal(error.to_string()))?;
    let (mut reader, writer) = std::os::unix::net::UnixStream::pair()
        .map_err(|error| PortalStoreError::Portal(error.to_string()))?;
    portal
        .retrieve(&writer, Default::default())
        .await
        .map_err(|error| PortalStoreError::Portal(error.to_string()))?;
    drop(writer);
    let mut bytes = Vec::new();
    reader
        .read_to_end(&mut bytes)
        .map_err(|error| PortalStoreError::Portal(error.to_string()))?;
    if bytes.is_empty() {
        return Err(PortalStoreError::EmptySecret);
    }
    Ok(Secret::new(bytes))
}
