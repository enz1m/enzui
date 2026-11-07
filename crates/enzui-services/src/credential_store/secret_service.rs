//! Freedesktop Secret Service transport for [`super::CredentialStore`].
//!
//! The transport uses a plain Secret Service session over the local session bus and maps wire outcomes into the parent module’s typed outcome ladder. Missing services and policy denials remain distinct, dismissed unlock prompts are not generic failures, diagnostics exclude secret bytes, and no operation falls back to plaintext files.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use futures_util::future::{Either, select};
use zbus::Connection;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

use super::mutation_owner::{CredentialMutationOwner, CredentialMutationStore};
use super::{
    CredentialKey, CredentialStore, SecretTransport, TransportCancel, TransportCredentialStore,
    TransportFuture, TransportOutcome,
};
use enzui_security::secret::Secret;

const DEFAULT_COLLECTION_ALIAS: &str = "default";
const ITEM_LABEL_PROP: &str = "org.freedesktop.Secret.Item.Label";
const ITEM_ATTRIBUTES_PROP: &str = "org.freedesktop.Secret.Item.Attributes";
const CONTENT_TYPE: &str = "application/octet-stream";

// Secret Service `(session, parameters, value, content_type)` wire shape; `value` is plaintext in a plain session.
type SecretStruct = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

// Wipe the app-owned plaintext `Vec` when this D-Bus payload is dropped, including during unwind or future cancellation.
struct DBusSecretPayload {
    inner: SecretStruct,
}

impl DBusSecretPayload {
    fn new(session: OwnedObjectPath, value: Vec<u8>) -> Self {
        Self {
            inner: (session, Vec::new(), value, CONTENT_TYPE.to_owned()),
        }
    }

    fn as_struct(&self) -> &SecretStruct {
        &self.inner
    }
}

impl Drop for DBusSecretPayload {
    fn drop(&mut self) {
        let wiped = enzui_security::secret::wipe_bytes(&mut self.inner.2);
        #[cfg(test)]
        tests::note_payload_wiped(wiped, self.inner.2.iter().filter(|b| **b == 0).count());
        #[cfg(not(test))]
        let _ = wiped;
    }
}

#[zbus::proxy(
    interface = "org.freedesktop.Secret.Service",
    default_service = "org.freedesktop.secrets",
    default_path = "/org/freedesktop/secrets"
)]
trait Service {
    fn open_session(
        &self,
        algorithm: &str,
        input: &Value<'_>,
    ) -> zbus::Result<(OwnedValue, OwnedObjectPath)>;

    fn search_items(
        &self,
        attributes: HashMap<&str, &str>,
    ) -> zbus::Result<(Vec<OwnedObjectPath>, Vec<OwnedObjectPath>)>;

    fn unlock(
        &self,
        objects: &[ObjectPath<'_>],
    ) -> zbus::Result<(Vec<OwnedObjectPath>, OwnedObjectPath)>;

    fn read_alias(&self, name: &str) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.freedesktop.Secret.Collection",
    default_service = "org.freedesktop.secrets"
)]
trait Collection {
    fn create_item(
        &self,
        properties: HashMap<&str, Value<'_>>,
        secret: &SecretStruct,
        replace: bool,
    ) -> zbus::Result<(OwnedObjectPath, OwnedObjectPath)>;

    #[zbus(property)]
    fn locked(&self) -> zbus::Result<bool>;
}

#[zbus::proxy(
    interface = "org.freedesktop.Secret.Item",
    default_service = "org.freedesktop.secrets"
)]
trait Item {
    fn get_secret(&self, session: &ObjectPath<'_>) -> zbus::Result<SecretStruct>;

    fn delete(&self) -> zbus::Result<OwnedObjectPath>;

    #[zbus(property)]
    fn locked(&self) -> zbus::Result<bool>;
}

#[zbus::proxy(
    interface = "org.freedesktop.Secret.Prompt",
    default_service = "org.freedesktop.secrets"
)]
trait Prompt {
    fn prompt(&self, window_id: &str) -> zbus::Result<()>;

    fn dismiss(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn completed(&self, dismissed: bool, result: Value<'_>) -> zbus::Result<()>;
}

/// A cloneable Secret Service transport for credential operations.
#[derive(Clone)]
pub struct SecretServiceTransport {
    connection: Connection,
}

/// Failure to connect Secret Service or start its mutation owner.
#[derive(Debug)]
pub enum CredentialMutationConnectError {
    /// The session-bus connection could not be established.
    Bus(zbus::Error),
    /// The mutation runtime or worker could not start, or reaper capacity was unavailable.
    Worker(std::io::Error),
    /// Inside a sandbox: the Secret portal store could not be opened.
    #[cfg(feature = "secret-portal")]
    Portal(super::portal::PortalStoreError),
}

impl std::fmt::Display for CredentialMutationConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bus(_) => f.write_str("could not connect to the credential service"),
            Self::Worker(_) => f.write_str("could not start the credential mutation owner"),
            #[cfg(feature = "secret-portal")]
            Self::Portal(_) => f.write_str("could not open the Secret portal credential store"),
        }
    }
}

impl std::error::Error for CredentialMutationConnectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bus(error) => Some(error),
            Self::Worker(error) => Some(error),
            #[cfg(feature = "secret-portal")]
            Self::Portal(error) => Some(error),
        }
    }
}

impl From<zbus::Error> for CredentialMutationConnectError {
    fn from(error: zbus::Error) -> Self {
        Self::Bus(error)
    }
}

impl From<std::io::Error> for CredentialMutationConnectError {
    fn from(error: std::io::Error) -> Self {
        Self::Worker(error)
    }
}

impl std::fmt::Debug for SecretServiceTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Keep diagnostics contentless so transport state can never expose a secret.
        f.debug_struct("SecretServiceTransport").finish()
    }
}

impl SecretServiceTransport {
    /// Connects to the session bus.
    ///
    /// Secret Service absence is reported by operations after connection.
    ///
    /// # Errors
    ///
    /// Returns a zbus error when the session-bus connection cannot be established.
    pub async fn connect() -> zbus::Result<Self> {
        Ok(Self {
            connection: Connection::session().await?,
        })
    }

    /// Builds a credential store, returning an unavailable store when the session bus cannot be reached.
    ///
    /// No plaintext file fallback is used.
    pub async fn connect_or_unavailable() -> Arc<dyn CredentialStore> {
        match Self::connect().await {
            Ok(transport) => Arc::new(TransportCredentialStore::new(
                Arc::new(transport) as Arc<dyn SecretTransport>
            )),
            Err(_) => Arc::new(super::UnavailableCredentialStore),
        }
    }

    /// Connects and returns an explicitly retained mutation owner with its cloneable store.
    ///
    /// Applications must adopt and retain this pair before enabling Secret
    /// Service `set` or `delete`. [`Self::connect_or_unavailable`] remains a
    /// compatibility constructor and does not provide mutation ownership.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialMutationConnectError::Bus`] when the session bus
    /// cannot be reached, or [`CredentialMutationConnectError::Worker`] when
    /// the bounded mutation owner cannot reserve reaper capacity or start.
    pub async fn connect_with_mutation_owner()
    -> Result<(CredentialMutationOwner, CredentialMutationStore), CredentialMutationConnectError>
    {
        let transport = Arc::new(Self::connect().await?);
        CredentialMutationOwner::new(transport as Arc<dyn SecretTransport>).map_err(Into::into)
    }
}

// Classify transport errors using D-Bus names only; never include payload bytes in diagnostics.
fn classify_error(err: &zbus::Error) -> TransportOutcome {
    if let zbus::Error::MethodError(name, _, _) = err {
        let name = name.as_str();
        if name.contains("ServiceUnknown")
            || name.contains("NameHasNoOwner")
            || name.contains("NoReply")
        {
            return TransportOutcome::Unavailable;
        }
        if name.contains("AccessDenied")
            || name.contains("AuthFailed")
            || name.contains("NotAuthorized")
            || name.contains(".Security")
        {
            return TransportOutcome::PermissionDenied;
        }
        // Report only the name of an unclassified D-Bus method error.
        return TransportOutcome::Failed(format!("secret-service error: {name}"));
    }
    match err {
        zbus::Error::Address(_) | zbus::Error::InputOutput(_) => TransportOutcome::Unavailable,
        // Category name only — never the secret.
        other => {
            TransportOutcome::Failed(format!("secret-service transport error: {}", short(other)))
        }
    }
}

// Return a fixed secret-free category for non-method transport errors.
fn short(err: &zbus::Error) -> &'static str {
    match err {
        zbus::Error::Address(_) => "bus address",
        zbus::Error::InputOutput(_) => "io",
        zbus::Error::Handshake(_) => "handshake",
        zbus::Error::MethodError(..) => "method",
        _ => "protocol",
    }
}

async fn open_plain_session(service: &ServiceProxy<'_>) -> zbus::Result<OwnedObjectPath> {
    let input = Value::new("");
    let (_output, session) = service.open_session("plain", &input).await?;
    Ok(session)
}

async fn build_item(conn: &Connection, path: OwnedObjectPath) -> zbus::Result<ItemProxy<'static>> {
    ItemProxy::builder(conn).path(path)?.build().await
}

async fn build_collection(
    conn: &Connection,
    path: OwnedObjectPath,
) -> zbus::Result<CollectionProxy<'static>> {
    CollectionProxy::builder(conn).path(path)?.build().await
}

// Bound a stalled Secret Service prompt to two minutes.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(120);

// Independently bound Dismiss settlement after prompt timeout or cancellation.
const PROMPT_DISMISS_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PromptResult {
    Completed,
    // Also used when the signal stream ends without a `Completed` signal.
    Dismissed,
    TimedOut,
    Cancelled,
}

// Subscribe before `Prompt`, then race completion against timeout and cancellation;
// the latter outcomes perform independently bounded Dismiss settlement.
async fn resolve_prompt(
    conn: &Connection,
    prompt: &OwnedObjectPath,
    timeout: Duration,
    dismiss_timeout: Duration,
    cancel: &TransportCancel,
) -> Result<PromptResult, zbus::Error> {
    if prompt.as_str() == "/" {
        return Ok(PromptResult::Completed);
    }
    let proxy = PromptProxy::builder(conn)
        .path(prompt.clone())?
        .build()
        .await?;
    let mut completed = proxy.receive_completed().await?;
    proxy.prompt("").await?;

    let signal_fut = Box::pin(completed.next());
    let timeout_fut = Box::pin(tokio::time::sleep(timeout));
    let cancel_fut = Box::pin(cancel.cancelled());

    let bound = select(timeout_fut, cancel_fut);
    // Poll `Completed` first so a simultaneous bound does not send Dismiss after completion.
    match select(signal_fut, Box::pin(bound)).await {
        Either::Left((signal, _)) => Ok(match signal {
            Some(s) => {
                // Propagate malformed `Completed` arguments as transport failure, not dismissal.
                if s.args()?.dismissed {
                    PromptResult::Dismissed
                } else {
                    PromptResult::Completed
                }
            }
            // Stream ended without Completed: treat as dismissed.
            None => PromptResult::Dismissed,
        }),
        Either::Right((Either::Left(_), signal_fut)) => {
            // Timeout owns an explicit Dismiss settlement before returning.
            settle_abandoned_prompt(proxy.dismiss(), signal_fut, dismiss_timeout).await;
            Ok(PromptResult::TimedOut)
        }
        Either::Right((Either::Right(_), signal_fut)) => {
            // Mark bounded settlement before awaiting Dismiss so the app-facing future does not drop this cleanup.
            cancel.begin_settlement();
            settle_abandoned_prompt(proxy.dismiss(), signal_fut, dismiss_timeout).await;
            Ok(PromptResult::Cancelled)
        }
    }
}

// Attempt Dismiss first, then use a late `Completed` only as settlement evidence under the same deadline; never deserialize or publish its result.
async fn settle_abandoned_prompt<D, L>(dismiss: D, late_signal: L, timeout: Duration)
where
    D: std::future::Future,
    L: std::future::Future,
{
    let dismiss = Box::pin(dismiss);
    let deadline = Box::pin(tokio::time::sleep(timeout));
    let late_signal = Box::pin(late_signal);

    // Poll Dismiss first so a queued late signal cannot bypass the retirement attempt.
    let dismiss_or_deadline = Box::pin(select(dismiss, deadline));
    match select(dismiss_or_deadline, late_signal).await {
        Either::Left((Either::Left((_reply, deadline)), late_signal)) => {
            // After Dismiss replies, drain at most one late signal within the remaining deadline.
            let _ = select(late_signal, deadline).await;
        }
        Either::Left((Either::Right((_elapsed, _dismiss)), _late_signal)) => {}
        Either::Right((_late_signal, _dismiss_or_deadline)) => {
            // Consume a late `Completed` without inspecting or publishing its result.
        }
    }
}

async fn ensure_unlocked(
    conn: &Connection,
    service: &ServiceProxy<'_>,
    objects: &[ObjectPath<'_>],
    timeout: Duration,
    cancel: &TransportCancel,
) -> Result<PromptResult, zbus::Error> {
    let (_unlocked, prompt) = service.unlock(objects).await?;
    resolve_prompt(conn, &prompt, timeout, PROMPT_DISMISS_TIMEOUT, cancel).await
}

fn attribute_map(key: &CredentialKey) -> HashMap<&str, &str> {
    key.attributes().into_iter().collect()
}

fn prompt_to_outcome(result: PromptResult) -> Option<TransportOutcome> {
    match result {
        PromptResult::Completed => None,
        PromptResult::Dismissed => Some(TransportOutcome::Locked),
        PromptResult::TimedOut => Some(TransportOutcome::PromptTimeout),
        PromptResult::Cancelled => Some(TransportOutcome::PromptCancelled),
    }
}

fn collection_lock_state(result: zbus::Result<bool>) -> Result<bool, TransportOutcome> {
    result.map_err(|error| classify_error(&error))
}

// Fixed ambiguity diagnostic deliberately omits object paths and credential attributes.
const AMBIGUOUS_SEARCH: &str = "secret-service search returned ambiguous matches";

#[derive(Debug, Clone, PartialEq, Eq)]
enum SearchSelection {
    NotFound,
    Unlocked(OwnedObjectPath),
    Locked(OwnedObjectPath),
    Ambiguous,
}

fn unique_path(paths: &[OwnedObjectPath]) -> Result<Option<OwnedObjectPath>, ()> {
    let Some(first) = paths.first() else {
        return Ok(None);
    };
    if paths.iter().all(|path| path == first) {
        Ok(Some(first.clone()))
    } else {
        Err(())
    }
}

// Reject every reply that assigns any matching item to both unlocked and locked lists.
fn classify_search(unlocked: &[OwnedObjectPath], locked: &[OwnedObjectPath]) -> SearchSelection {
    let Ok(unlocked) = unique_path(unlocked) else {
        return SearchSelection::Ambiguous;
    };
    let Ok(locked) = unique_path(locked) else {
        return SearchSelection::Ambiguous;
    };

    match (unlocked, locked) {
        (None, None) => SearchSelection::NotFound,
        (Some(path), None) => SearchSelection::Unlocked(path),
        (None, Some(path)) => SearchSelection::Locked(path),
        (Some(_), Some(_)) => SearchSelection::Ambiguous,
    }
}

trait GetProtocol {
    fn unlock<'a>(
        &'a self,
        path: &'a OwnedObjectPath,
        cancel: &'a TransportCancel,
    ) -> impl std::future::Future<Output = zbus::Result<PromptResult>> + Send + 'a;

    fn get_secret(
        &self,
        path: OwnedObjectPath,
    ) -> impl std::future::Future<Output = zbus::Result<Vec<u8>>> + Send + '_;
}

struct DbusGetProtocol<'a> {
    conn: &'a Connection,
    service: &'a ServiceProxy<'a>,
    session: &'a OwnedObjectPath,
}

impl GetProtocol for DbusGetProtocol<'_> {
    async fn unlock(
        &self,
        path: &OwnedObjectPath,
        cancel: &TransportCancel,
    ) -> zbus::Result<PromptResult> {
        let paths = [path.as_ref()];
        ensure_unlocked(self.conn, self.service, &paths, PROMPT_TIMEOUT, cancel).await
    }

    async fn get_secret(&self, path: OwnedObjectPath) -> zbus::Result<Vec<u8>> {
        let item = build_item(self.conn, path).await?;
        let (_session, _params, value, _content) = item.get_secret(self.session).await?;
        Ok(value)
    }
}

async fn get_from_search(
    protocol: &impl GetProtocol,
    unlocked: Vec<OwnedObjectPath>,
    locked: Vec<OwnedObjectPath>,
    cancel: &TransportCancel,
) -> TransportOutcome {
    let item_path = match classify_search(&unlocked, &locked) {
        SearchSelection::NotFound => return TransportOutcome::NotFound,
        SearchSelection::Ambiguous => {
            return TransportOutcome::Failed(AMBIGUOUS_SEARCH.to_owned());
        }
        SearchSelection::Unlocked(path) => path,
        SearchSelection::Locked(path) => match protocol.unlock(&path, cancel).await {
            Ok(result) => {
                if let Some(outcome) = prompt_to_outcome(result) {
                    return outcome;
                }
                path
            }
            Err(error) => return classify_error(&error),
        },
    };

    match protocol.get_secret(item_path).await {
        Ok(value) => TransportOutcome::Found(Secret::new(value)),
        Err(error) => classify_error(&error),
    }
}

// Providers that encrypt attributes cannot discover items in a locked collection.
// Retry an empty search after unlocking the same default collection used by set.
async fn search_with_unlock(
    conn: &Connection,
    service: &ServiceProxy<'_>,
    key: &CredentialKey,
    cancel: &TransportCancel,
) -> Result<(Vec<OwnedObjectPath>, Vec<OwnedObjectPath>), TransportOutcome> {
    let found = service
        .search_items(attribute_map(key))
        .await
        .map_err(|e| classify_error(&e))?;
    if !found.0.is_empty() || !found.1.is_empty() {
        return Ok(found);
    }
    let path = service
        .read_alias(DEFAULT_COLLECTION_ALIAS)
        .await
        .map_err(|e| classify_error(&e))?;
    if path.as_str() == "/" {
        return Ok(found);
    }
    let collection = build_collection(conn, path.clone())
        .await
        .map_err(|e| classify_error(&e))?;
    if !collection_lock_state(collection.locked().await)? {
        return Ok(found);
    }
    let result = ensure_unlocked(conn, service, &[path.as_ref()], PROMPT_TIMEOUT, cancel)
        .await
        .map_err(|e| classify_error(&e))?;
    if let Some(outcome) = prompt_to_outcome(result) {
        return Err(outcome);
    }
    service
        .search_items(attribute_map(key))
        .await
        .map_err(|e| classify_error(&e))
}

async fn get_impl(
    conn: Connection,
    key: CredentialKey,
    cancel: TransportCancel,
) -> TransportOutcome {
    let service = match ServiceProxy::new(&conn).await {
        Ok(s) => s,
        Err(e) => return classify_error(&e),
    };
    let session = match open_plain_session(&service).await {
        Ok(s) => s,
        Err(e) => return classify_error(&e),
    };
    let (unlocked, locked) = match search_with_unlock(&conn, &service, &key, &cancel).await {
        Ok(v) => v,
        Err(outcome) => return outcome,
    };
    let protocol = DbusGetProtocol {
        conn: &conn,
        service: &service,
        session: &session,
    };
    get_from_search(&protocol, unlocked, locked, &cancel).await
}

async fn set_impl(
    conn: Connection,
    key: CredentialKey,
    secret: Secret,
    cancel: TransportCancel,
) -> TransportOutcome {
    let service = match ServiceProxy::new(&conn).await {
        Ok(s) => s,
        Err(e) => {
            drop(secret);
            return classify_error(&e);
        }
    };
    let session = match open_plain_session(&service).await {
        Ok(s) => s,
        Err(e) => {
            drop(secret);
            return classify_error(&e);
        }
    };
    let collection_path = match service.read_alias(DEFAULT_COLLECTION_ALIAS).await {
        Ok(p) if p.as_str() != "/" => p,
        Ok(_) => {
            drop(secret);
            return TransportOutcome::Unavailable;
        }
        Err(e) => {
            drop(secret);
            return classify_error(&e);
        }
    };
    let collection = match build_collection(&conn, collection_path.clone()).await {
        Ok(c) => c,
        Err(e) => {
            drop(secret);
            return classify_error(&e);
        }
    };
    match collection_lock_state(collection.locked().await) {
        Ok(false) => {}
        Ok(true) => {
            let paths = [collection_path.as_ref()];
            match ensure_unlocked(&conn, &service, &paths, PROMPT_TIMEOUT, &cancel).await {
                Ok(r) => {
                    if let Some(o) = prompt_to_outcome(r) {
                        drop(secret);
                        return o;
                    }
                }
                Err(e) => {
                    drop(secret);
                    return classify_error(&e);
                }
            }
        }
        Err(outcome) => {
            drop(secret);
            return outcome;
        }
    }

    let label = format!("{}/{}/{}", key.app_id(), key.owner(), key.label());
    let mut properties: HashMap<&str, Value<'_>> = HashMap::new();
    properties.insert(ITEM_LABEL_PROP, Value::from(label));
    properties.insert(ITEM_ATTRIBUTES_PROP, Value::new(attribute_map(&key)));

    // Put the D-Bus plaintext copy under its wiping guard immediately so every exit path clears it.
    let payload = DBusSecretPayload::new(session, secret.expose_secret().to_vec());
    drop(secret);

    match collection
        .create_item(properties, payload.as_struct(), true)
        .await
    {
        Ok((_item, prompt)) => {
            match resolve_prompt(
                &conn,
                &prompt,
                PROMPT_TIMEOUT,
                PROMPT_DISMISS_TIMEOUT,
                &cancel,
            )
            .await
            {
                Ok(r) => prompt_to_outcome(r).unwrap_or(TransportOutcome::Stored),
                Err(e) => classify_error(&e),
            }
        }
        Err(e) => classify_error(&e),
    }
}

trait DeleteProtocol {
    fn delete<'a>(
        &'a self,
        path: OwnedObjectPath,
        cancel: &'a TransportCancel,
    ) -> impl std::future::Future<Output = zbus::Result<PromptResult>> + Send + 'a;
}

struct DbusDeleteProtocol<'a> {
    conn: &'a Connection,
}

impl DeleteProtocol for DbusDeleteProtocol<'_> {
    async fn delete(
        &self,
        path: OwnedObjectPath,
        cancel: &TransportCancel,
    ) -> zbus::Result<PromptResult> {
        let item = build_item(self.conn, path).await?;
        let prompt = item.delete().await?;
        resolve_prompt(
            self.conn,
            &prompt,
            PROMPT_TIMEOUT,
            PROMPT_DISMISS_TIMEOUT,
            cancel,
        )
        .await
    }
}

async fn delete_from_search(
    protocol: &impl DeleteProtocol,
    unlocked: Vec<OwnedObjectPath>,
    locked: Vec<OwnedObjectPath>,
    cancel: &TransportCancel,
) -> TransportOutcome {
    for path in unlocked.into_iter().chain(locked) {
        match protocol.delete(path, cancel).await {
            Ok(result) => {
                if let Some(outcome) = prompt_to_outcome(result) {
                    return outcome;
                }
            }
            Err(error) => return classify_error(&error),
        }
    }
    TransportOutcome::Deleted
}

async fn delete_impl(
    conn: Connection,
    key: CredentialKey,
    cancel: TransportCancel,
) -> TransportOutcome {
    let service = match ServiceProxy::new(&conn).await {
        Ok(s) => s,
        Err(e) => return classify_error(&e),
    };
    let (unlocked, locked) = match search_with_unlock(&conn, &service, &key, &cancel).await {
        Ok(v) => v,
        Err(outcome) => return outcome,
    };
    let protocol = DbusDeleteProtocol { conn: &conn };
    delete_from_search(&protocol, unlocked, locked, &cancel).await
}

impl SecretTransport for SecretServiceTransport {
    // Plain transport methods use an inert token; `TransportCredentialStore` calls the cancellable overrides with the operation token.
    fn get(&self, key: &CredentialKey) -> TransportFuture {
        self.get_cancellable(key, TransportCancel::inert())
    }

    fn set(&self, key: &CredentialKey, secret: Secret) -> TransportFuture {
        self.set_cancellable(key, secret, TransportCancel::inert())
    }

    fn delete(&self, key: &CredentialKey) -> TransportFuture {
        self.delete_cancellable(key, TransportCancel::inert())
    }

    fn get_cancellable(&self, key: &CredentialKey, cancel: TransportCancel) -> TransportFuture {
        let conn = self.connection.clone();
        let key = key.clone();
        Box::pin(get_impl(conn, key, cancel))
    }

    fn set_cancellable(
        &self,
        key: &CredentialKey,
        secret: Secret,
        cancel: TransportCancel,
    ) -> TransportFuture {
        let conn = self.connection.clone();
        let key = key.clone();
        Box::pin(set_impl(conn, key, secret, cancel))
    }

    fn delete_cancellable(&self, key: &CredentialKey, cancel: TransportCancel) -> TransportFuture {
        let conn = self.connection.clone();
        let key = key.clone();
        Box::pin(delete_impl(conn, key, cancel))
    }

    // Keep `contains` as `Unknown`; this transport cannot query the bus synchronously without blocking.
}

/// The transport for `app_id` on a host showing `evidence`: with the
/// `secret-portal` feature, the Secret portal store inside a sandbox, where
/// the Secret Service cannot be reached; the Secret Service otherwise. Uses
/// the session bus and the process's data directory.
///
/// # Errors
/// [`CredentialMutationConnectError::Bus`] when the session bus is
/// unreachable, or [`CredentialMutationConnectError::Portal`] when the portal
/// store cannot be opened.
pub async fn system_transport(
    app_id: &str,
    evidence: &enzui_core::portal::sandbox::SandboxEvidence,
) -> Result<Arc<dyn SecretTransport>, CredentialMutationConnectError> {
    let connection = Connection::session().await?;
    system_transport_with(
        app_id,
        evidence,
        connection,
        &enzui_core::app_data::AppDataResolver::from_env(),
    )
    .await
}

/// [`system_transport`] on `connection`, with the data directory `resolver`
/// finds.
///
/// # Errors
/// As [`system_transport`].
pub async fn system_transport_with(
    app_id: &str,
    evidence: &enzui_core::portal::sandbox::SandboxEvidence,
    connection: Connection,
    resolver: &enzui_core::app_data::AppDataResolver,
) -> Result<Arc<dyn SecretTransport>, CredentialMutationConnectError> {
    #[cfg(feature = "secret-portal")]
    if enzui_core::portal::sandbox::is_sandboxed(evidence) {
        let transport =
            super::portal::PortalSecretTransport::connect_with(app_id, connection, resolver)
                .await
                .map_err(CredentialMutationConnectError::Portal)?;
        return Ok(Arc::new(transport));
    }
    let _ = (app_id, evidence, resolver);
    Ok(Arc::new(SecretServiceTransport { connection }))
}

/// The credential store this process should use for `app_id`
/// ([`system_transport`] for this host), or an unavailable store when none
/// can be reached. No plaintext fallback is used.
pub async fn connect_for_app(app_id: &str) -> Arc<dyn CredentialStore> {
    let evidence = enzui_core::portal::sandbox::SandboxEvidence::from_host();
    match system_transport(app_id, &evidence).await {
        Ok(transport) => Arc::new(TransportCredentialStore::new(transport)),
        Err(_) => Arc::new(super::UnavailableCredentialStore),
    }
}

/// [`connect_for_app`] with an explicitly retained mutation owner, as
/// [`SecretServiceTransport::connect_with_mutation_owner`].
///
/// # Errors
/// As [`system_transport`], and [`CredentialMutationConnectError::Worker`]
/// when the mutation owner cannot start.
pub async fn connect_for_app_with_mutation_owner(
    app_id: &str,
) -> Result<(CredentialMutationOwner, CredentialMutationStore), CredentialMutationConnectError> {
    let evidence = enzui_core::portal::sandbox::SandboxEvidence::from_host();
    let transport = system_transport(app_id, &evidence).await?;
    CredentialMutationOwner::new(transport).map_err(Into::into)
}

#[cfg(test)]
mod tests;
