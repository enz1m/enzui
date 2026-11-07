//! The freedesktop settings-portal source for accessibility preferences.
//!
//! One generated `org.freedesktop.portal.Settings` proxy, two keys, every
//! operation bounded by [`PORTAL_CALL_TIMEOUT`]. No subprocess is spawned and
//! no settings database is read directly: a desktop that does not export a key
//! answers "not found" and this backend reports absence, which is what keeps a
//! material honest on a desktop that has no such preference.
//!
//! # Keys
//!
//! * [`APPEARANCE_NAMESPACE`] / [`CONTRAST_KEY`] — standardised: `0` is no
//!   preference, `1` is higher contrast.
//! * [`GNOME_A11Y_NAMESPACE`] / [`GNOME_HIGH_CONTRAST_KEY`] — GNOME's own
//!   boolean, read only when the standardised key answered nothing, so a
//!   desktop that exports both is read once and consistently.
//! * reduced transparency — no standardised key exists. Supply one with
//!   [`PortalPreferenceBackend::with_transparency_key`]; the default leaves the
//!   dimension unanswered.
//!
//! There is no KDE-specific key here because `xdg-desktop-portal-kde` exports
//! the standardised one: the first read already covers it. A desktop that
//! exports something else is named with
//! [`PortalPreferenceBackend::with_contrast_keys`] rather than by adding a
//! second transport, and no configuration reads a settings database or spawns
//! a helper process.
//!
//! # Hazards
//!
//! A portal that never answers degrades to a failure or to absence; it does not
//! hang the feed's worker. The connection is cached and rebuilt after a
//! failure, so a restarted portal recovers without restarting the application.

use std::sync::Mutex;
use std::time::Duration;

use enzui_theme::material::PreferenceSet;
use enzui_theme::material_source::{
    MaterialPreferenceBackend, MaterialPreferenceRead, MaterialPreferenceSignals, PreferenceSignal,
};
use futures_util::StreamExt;
use zbus::zvariant::{OwnedValue, Value};

/// The standardised appearance namespace.
pub const APPEARANCE_NAMESPACE: &str = "org.freedesktop.appearance";

/// The standardised contrast key: `0` no preference, `1` higher contrast.
pub const CONTRAST_KEY: &str = "contrast";

/// GNOME's accessibility namespace, read as a fallback for contrast.
pub const GNOME_A11Y_NAMESPACE: &str = "org.gnome.desktop.a11y.interface";

/// GNOME's high-contrast boolean.
pub const GNOME_HIGH_CONTRAST_KEY: &str = "high-contrast";

/// Longest this backend waits for any single bus operation.
pub const PORTAL_CALL_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a poll waits between reads when the portal offers no signal.
pub const PORTAL_POLL_INTERVAL: Duration = Duration::from_secs(60);

#[zbus::proxy(
    interface = "org.freedesktop.portal.Settings",
    default_service = "org.freedesktop.portal.Desktop",
    default_path = "/org/freedesktop/portal/desktop"
)]
trait Settings {
    fn read_one(&self, namespace: &str, key: &str) -> zbus::Result<OwnedValue>;

    fn read(&self, namespace: &str, key: &str) -> zbus::Result<OwnedValue>;

    #[zbus(signal)]
    fn setting_changed(&self, namespace: &str, key: &str, value: Value<'_>) -> zbus::Result<()>;
}

/// One namespace/key pair to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreferenceKey {
    /// The portal namespace.
    pub namespace: String,
    /// The key inside it.
    pub key: String,
}

impl PreferenceKey {
    /// Names a key.
    #[must_use]
    pub fn new(namespace: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            key: key.into(),
        }
    }
}

/// Reads accessibility preferences through the freedesktop settings portal.
#[derive(Debug)]
pub struct PortalPreferenceBackend {
    contrast: Vec<PreferenceKey>,
    transparency: Vec<PreferenceKey>,
    address: Option<String>,
    session: Mutex<Option<Session>>,
}

// A cached runtime plus connection. A portal-less desktop must not pay a fresh
// connection attempt on every poll.
struct Session {
    runtime: tokio::runtime::Runtime,
    connection: Option<zbus::Connection>,
}

impl Session {
    fn connection(&self) -> &zbus::Connection {
        self.connection
            .as_ref()
            .expect("the connection outlives every use of the session")
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // zbus's tokio integration reaches for the reactor while its
        // connection is torn down; doing that after the runtime has gone
        // aborts the process from a destructor, so the connection is released
        // inside the runtime it was built on.
        let _guard = self.runtime.enter();
        drop(self.connection.take());
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Session")
    }
}

impl PortalPreferenceBackend {
    /// The shipped configuration: the standardised contrast key, then GNOME's
    /// own, and no transparency key.
    #[must_use]
    pub fn new() -> Self {
        Self {
            contrast: vec![
                PreferenceKey::new(APPEARANCE_NAMESPACE, CONTRAST_KEY),
                PreferenceKey::new(GNOME_A11Y_NAMESPACE, GNOME_HIGH_CONTRAST_KEY),
            ],
            transparency: Vec::new(),
            address: None,
            session: Mutex::new(None),
        }
    }

    /// Adds a key to read for reduced transparency.
    ///
    /// Keys are tried in order and the first one that answers wins. There is no
    /// standardised key for this preference, so nothing is read for it until a
    /// host names one.
    #[must_use]
    pub fn with_transparency_key(
        mut self,
        namespace: impl Into<String>,
        key: impl Into<String>,
    ) -> Self {
        self.transparency.push(PreferenceKey::new(namespace, key));
        self
    }

    /// Replaces the contrast keys.
    #[must_use]
    pub fn with_contrast_keys(mut self, keys: Vec<PreferenceKey>) -> Self {
        self.contrast = keys;
        self
    }

    /// Connects to an explicit bus address instead of the session bus.
    ///
    /// This is how a test drives the real backend against a bus it owns, with
    /// no host service involved.
    #[must_use]
    pub fn on_bus(mut self, address: impl Into<String>) -> Self {
        self.address = Some(address.into());
        self
    }

    /// The contrast keys, in the order they are tried.
    #[must_use]
    pub fn contrast_keys(&self) -> &[PreferenceKey] {
        &self.contrast
    }

    /// The transparency keys, in the order they are tried.
    #[must_use]
    pub fn transparency_keys(&self) -> &[PreferenceKey] {
        &self.transparency
    }

    /// Every namespace this backend reads, for the signal filter.
    fn namespaces(&self) -> Vec<String> {
        let mut namespaces: Vec<String> = self
            .contrast
            .iter()
            .chain(self.transparency.iter())
            .map(|key| key.namespace.clone())
            .collect();
        namespaces.sort();
        namespaces.dedup();

        namespaces
    }

    fn connect(&self) -> zbus::Result<(tokio::runtime::Runtime, zbus::Connection)> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| zbus::Error::InputOutput(std::sync::Arc::new(error)))?;
        let address = self.address.clone();
        let connection = runtime.block_on(async move {
            let connect = async {
                match address {
                    Some(address) => {
                        zbus::connection::Builder::address(address.as_str())?
                            .build()
                            .await
                    }
                    None => zbus::Connection::session().await,
                }
            };

            match tokio::time::timeout(PORTAL_CALL_TIMEOUT, connect).await {
                Ok(result) => result,
                Err(_) => Err(zbus::Error::InputOutput(std::sync::Arc::new(
                    std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "connecting to the session bus timed out",
                    ),
                ))),
            }
        })?;

        Ok((runtime, connection))
    }

    // Runs `body` on the cached session, dropping it after a failure so a
    // restarted bus reconnects on the next read.
    fn with_session<T>(
        &self,
        body: impl Fn(&tokio::runtime::Runtime, &zbus::Connection) -> zbus::Result<T>,
    ) -> zbus::Result<T> {
        let mut slot = self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        if slot.is_none() {
            let (runtime, connection) = self.connect()?;
            *slot = Some(Session {
                runtime,
                connection: Some(connection),
            });
        }

        let session = slot.as_ref().expect("session was just installed");

        match body(&session.runtime, session.connection()) {
            Ok(value) => Ok(value),
            Err(error) => {
                *slot = None;
                Err(error)
            }
        }
    }
}

impl Default for PortalPreferenceBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MaterialPreferenceBackend for PortalPreferenceBackend {
    fn identity(&self) -> String {
        format!(
            "a11y-portal:{}:{}",
            self.address.as_deref().unwrap_or("session"),
            self.namespaces().join(",")
        )
    }

    fn read(&self) -> MaterialPreferenceRead {
        let contrast = self.contrast.clone();
        let transparency = self.transparency.clone();
        let outcome = self.with_session(move |runtime, connection| {
            let contrast = contrast.clone();
            let transparency = transparency.clone();

            runtime.block_on(async move {
                let proxy = SettingsProxy::new(connection).await?;
                let mut reachable = false;
                let mut preferences = PreferenceSet::NOTHING;

                for key in &contrast {
                    match read_key(&proxy, key).await {
                        Ok(Some(value)) => {
                            reachable = true;
                            if let Some(answer) = contrast_answer(key, &value) {
                                preferences.increased_contrast = Some(answer);
                                break;
                            }
                        }
                        // A key that exists and holds a shape this preference
                        // cannot have is a silent key, not a refusal.
                        Ok(None) => reachable = true,
                        Err(error) if is_not_found(&error) => reachable = true,
                        Err(error) => return Err(error),
                    }
                }

                for key in &transparency {
                    match read_key(&proxy, key).await {
                        Ok(Some(value)) => {
                            reachable = true;
                            if let Some(answer) = as_bool(&value) {
                                preferences.reduced_transparency = Some(answer);
                                break;
                            }
                        }
                        Ok(None) => reachable = true,
                        Err(error) if is_not_found(&error) => reachable = true,
                        Err(error) => return Err(error),
                    }
                }

                Ok((reachable, preferences))
            })
        });

        match outcome {
            // Reachable, with or without answers: a portal that exports none
            // of these keys is a delivered "nothing", which the feed must be
            // able to tell from an unreachable bus.
            Ok((true, preferences)) => MaterialPreferenceRead::Delivered(preferences),
            Ok((false, _)) => MaterialPreferenceRead::Unsupported,
            Err(error) if is_unavailable(&error) => MaterialPreferenceRead::Unsupported,
            Err(error) => MaterialPreferenceRead::Failed {
                detail: error.to_string(),
            },
        }
    }

    fn subscribe(&self) -> Option<Box<dyn MaterialPreferenceSignals>> {
        let (runtime, connection) = self.connect().ok()?;

        PortalSignals::start(runtime, connection, self.namespaces())
            .map(|signals| Box::new(signals) as Box<dyn MaterialPreferenceSignals>)
    }

    fn poll_interval(&self) -> Duration {
        PORTAL_POLL_INTERVAL
    }
}

// The `SettingChanged` stream, waited on with a bound so the feed's worker
// stays cancellable.
struct PortalSignals {
    runtime: Option<tokio::runtime::Runtime>,
    connection: Option<zbus::Connection>,
    stream: Option<SettingChangedStream>,
    namespaces: Vec<String>,
}

impl PortalSignals {
    fn start(
        runtime: tokio::runtime::Runtime,
        connection: zbus::Connection,
        namespaces: Vec<String>,
    ) -> Option<Self> {
        let mut signals = Self {
            runtime: Some(runtime),
            connection: Some(connection),
            stream: None,
            namespaces,
        };
        let runtime = signals.runtime.as_ref()?;
        let connection = signals.connection.as_ref()?;
        let stream = runtime
            .block_on(with_timeout(async {
                let proxy = SettingsProxy::new(connection).await?;
                proxy.receive_setting_changed().await
            }))
            .ok()?;
        signals.stream = Some(stream);

        Some(signals)
    }
}

impl Drop for PortalSignals {
    fn drop(&mut self) {
        // Same reactor contract as `Session`: the stream and its connection are
        // registered with the runtime and must be released while it is still
        // entered, or their destructors abort the process.
        let Some(runtime) = self.runtime.take() else {
            return;
        };
        {
            let _guard = runtime.enter();
            drop(self.stream.take());
            drop(self.connection.take());
        }
        drop(runtime);
    }
}

impl MaterialPreferenceSignals for PortalSignals {
    fn wait_for_change(&mut self, timeout: Duration) -> PreferenceSignal {
        let namespaces = self.namespaces.clone();
        let Some(stream) = self.stream.as_mut() else {
            return PreferenceSignal::Closed;
        };
        let runtime = self
            .runtime
            .as_ref()
            .expect("the runtime outlives every wait");

        runtime.block_on(async move {
            match tokio::time::timeout(timeout, stream.next()).await {
                Err(_) => PreferenceSignal::Idle,
                Ok(None) => PreferenceSignal::Closed,
                Ok(Some(signal)) => match signal.args() {
                    // Another namespace's change is not ours to re-read for.
                    Ok(args)
                        if namespaces
                            .iter()
                            .any(|name| name.as_str() == *args.namespace()) =>
                    {
                        PreferenceSignal::Changed
                    }
                    Ok(_) => PreferenceSignal::Idle,
                    // An unparseable signal is still a signal: re-read rather
                    // than assume nothing happened.
                    Err(_) => PreferenceSignal::Changed,
                },
            }
        })
    }
}

// `ReadOne` is the v2 method; `Read` is the v1 one that wraps the value in a
// second variant. Try the new one first and fall back, so this works against
// both portal versions.
async fn read_key(
    proxy: &SettingsProxy<'_>,
    key: &PreferenceKey,
) -> zbus::Result<Option<OwnedValue>> {
    let value = match with_timeout(proxy.read_one(&key.namespace, &key.key)).await {
        Ok(value) => value,
        Err(error) if is_unknown_method(&error) => {
            with_timeout(proxy.read(&key.namespace, &key.key)).await?
        }
        Err(error) => return Err(error),
    };

    Ok(Some(value))
}

async fn with_timeout<T>(
    call: impl std::future::Future<Output = zbus::Result<T>>,
) -> zbus::Result<T> {
    match tokio::time::timeout(PORTAL_CALL_TIMEOUT, call).await {
        Ok(result) => result,
        Err(_) => Err(zbus::Error::InputOutput(std::sync::Arc::new(
            std::io::Error::new(std::io::ErrorKind::TimedOut, "portal call timed out"),
        ))),
    }
}

/// Decode an implementation-specific boolean or numeric preference.
///
/// Accepts booleans and `u8`, `u32` or `i32` integers. Positive integers are true;
/// zero and negative integers are false. Nested variants are unwrapped; other
/// shapes return `None`. The backend decodes standardized contrast separately.
#[must_use]
pub fn as_bool(value: &OwnedValue) -> Option<bool> {
    fn inner(value: &Value<'_>) -> Option<bool> {
        match value {
            Value::Bool(answer) => Some(*answer),
            Value::U32(level) => Some(*level != 0),
            Value::U8(level) => Some(*level != 0),
            Value::I32(level) => Some(*level > 0),
            Value::Value(nested) => inner(nested),
            _ => None,
        }
    }

    inner(value)
}

fn contrast_answer(key: &PreferenceKey, value: &OwnedValue) -> Option<bool> {
    if key.namespace != APPEARANCE_NAMESPACE || key.key != CONTRAST_KEY {
        return as_bool(value);
    }
    fn standard(value: &Value<'_>) -> Option<bool> {
        match value {
            Value::U32(level) => Some(*level == 1),
            Value::Value(nested) => standard(nested),
            _ => None,
        }
    }
    standard(value)
}

fn error_name(error: &zbus::Error) -> Option<String> {
    match error {
        zbus::Error::MethodError(name, _, _) => Some(name.as_str().to_owned()),
        _ => None,
    }
}

/// Whether a D-Bus error name means "this key or namespace is not exported".
///
/// The key is absent, not the source: the read continues with the next key and
/// the dimension stays unanswered.
#[must_use]
pub fn is_not_found_name(name: &str) -> bool {
    matches!(
        name,
        "org.freedesktop.portal.Error.NotFound"
            | "org.freedesktop.DBus.Error.InvalidArgs"
            | "org.freedesktop.DBus.Error.Failed"
    )
}

/// Whether a D-Bus error name means "this portal is too old for `ReadOne`".
#[must_use]
pub fn is_unknown_method_name(name: &str) -> bool {
    matches!(
        name,
        "org.freedesktop.DBus.Error.UnknownMethod" | "org.freedesktop.DBus.Error.NotSupported"
    )
}

/// Whether a D-Bus error name means "there is no portal here at all".
///
/// This is the only error class that *releases* the system layer: the source
/// does not exist, so a preference read from it must not outlive it.
#[must_use]
pub fn is_unavailable_name(name: &str) -> bool {
    matches!(
        name,
        "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner"
            | "org.freedesktop.DBus.Error.UnknownInterface"
            | "org.freedesktop.DBus.Error.UnknownObject"
    )
}

fn is_not_found(error: &zbus::Error) -> bool {
    error_name(error).is_some_and(|name| is_not_found_name(&name))
}

fn is_unknown_method(error: &zbus::Error) -> bool {
    error_name(error).is_some_and(|name| is_unknown_method_name(&name))
}

fn is_unavailable(error: &zbus::Error) -> bool {
    match error {
        zbus::Error::Address(_) => true,
        // A call timeout on an established connection is a transient failure,
        // not an absent source: Failed retains the last delivery while
        // Unsupported would release it.
        zbus::Error::InputOutput(io) if io.kind() != std::io::ErrorKind::TimedOut => true,
        _ => error_name(error).is_some_and(|name| is_unavailable_name(&name)),
    }
}
