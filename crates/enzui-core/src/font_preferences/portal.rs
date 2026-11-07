//! The freedesktop settings-portal source for desktop font preferences.
//!
//! # What is standard here and what is not
//!
//! `org.freedesktop.portal.Settings` standardises the transport: `ReadOne`,
//! `Read` and the `SettingChanged` signal, over a namespace/key pair. It does
//! not standardise a font namespace. The keys this backend reads —
//! `font-name`, `document-font-name`, `monospace-font-name` under
//! `org.gnome.desktop.interface` — are GNOME's own settings surfaced by
//! `xdg-desktop-portal-gnome`. A desktop whose portal backend does not export
//! that namespace answers `org.freedesktop.portal.Error.NotFound`, which this
//! backend reports as [`FontPreferenceRead::Unsupported`] so the caller falls
//! back rather than blanking the catalog.
//!
//! Use [`PortalFontBackend::with_namespace`] to point the same transport at a
//! different desktop's keys.
//!
//! # Example
//!
//! ```no_run
//! use enzui_core::font_preferences::{DesktopFontFeed, portal::PortalFontBackend};
//! use std::sync::Arc;
//!
//! let feed = DesktopFontFeed::with_backend(Arc::new(PortalFontBackend::new()));
//! assert!(feed.is_enabled());
//! ```
//!
//! # Hazards
//!
//! Every bus operation is bounded by [`PORTAL_CALL_TIMEOUT`]: a portal that
//! never answers degrades to "unsupported", it does not hang the feed's worker
//! or the application. The connection is cached and rebuilt after a failure.

use std::sync::Mutex;
use std::time::Duration;

use enzui_theme::font_preferences::DesktopFontPreferences;
use enzui_theme::font_source::{
    FontPreferenceRead, FontPreferenceSignals, FontSignal, SignalCancel,
};
use enzui_theme::{FontRole, RawFontDescription};
use futures_util::StreamExt;
use zbus::zvariant::{OwnedValue, Value};

/// The GNOME namespace whose interface-font keys this backend reads by default.
///
/// A backend extension, not a standardised portal namespace.
pub const GNOME_INTERFACE_NAMESPACE: &str = "org.gnome.desktop.interface";

/// Longest this backend waits for any single bus operation.
pub const PORTAL_CALL_TIMEOUT: Duration = Duration::from_secs(2);

/// The role/key pairs read from [`GNOME_INTERFACE_NAMESPACE`].
pub const GNOME_INTERFACE_KEYS: [(FontRole, &str); 3] = [
    (FontRole::Ui, "font-name"),
    (FontRole::Document, "document-font-name"),
    (FontRole::Monospace, "monospace-font-name"),
];

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

/// Reads desktop font preferences through the freedesktop settings portal.
#[derive(Debug)]
pub struct PortalFontBackend {
    namespace: String,
    keys: Vec<(FontRole, String)>,
    address: Option<String>,
    session: Mutex<Option<Session>>,
}

// A cached runtime plus connection. Reads are rare, but a portal-less desktop
// must not pay a fresh connection attempt on every poll either.
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

impl PortalFontBackend {
    /// The shipped configuration: GNOME's interface keys on the session bus.
    #[must_use]
    pub fn new() -> Self {
        Self::with_namespace(
            GNOME_INTERFACE_NAMESPACE,
            GNOME_INTERFACE_KEYS.iter().copied(),
        )
    }

    /// The same transport pointed at another desktop's namespace and keys.
    #[must_use]
    pub fn with_namespace<'a>(
        namespace: impl Into<String>,
        keys: impl IntoIterator<Item = (FontRole, &'a str)>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            keys: keys
                .into_iter()
                .map(|(role, key)| (role, key.to_owned()))
                .collect(),
            address: None,
            session: Mutex::new(None),
        }
    }

    /// Connects to an explicit bus address instead of the session bus.
    ///
    /// This is how a test drives the real backend against a private bus, with
    /// no host service involved.
    #[must_use]
    pub fn on_bus(mut self, address: impl Into<String>) -> Self {
        self.address = Some(address.into());
        self
    }

    /// The namespace being read.
    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.namespace
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

    // Runs `body` on the cached session, rebuilding it once after a failure so
    // a restarted bus recovers without restarting the application.
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

    /// A connection for the live signal, built on its own runtime.
    fn signal_connection(&self) -> zbus::Result<(tokio::runtime::Runtime, zbus::Connection)> {
        self.connect()
    }
}

impl Default for PortalFontBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl enzui_theme::font_source::DesktopFontBackend for PortalFontBackend {
    fn identity(&self) -> String {
        format!(
            "portal:{}:{}",
            self.address.as_deref().unwrap_or("session"),
            self.namespace
        )
    }

    fn read(&self) -> FontPreferenceRead {
        let namespace = self.namespace.clone();
        let keys = self.keys.clone();
        let outcome = self.with_session(move |runtime, connection| {
            let namespace = namespace.clone();
            let keys = keys.clone();
            runtime.block_on(async move {
                let proxy = SettingsProxy::new(connection).await?;
                let mut preferences = DesktopFontPreferences::new();
                let mut reachable = false;
                for (role, key) in &keys {
                    let value = read_key(&proxy, &namespace, key).await;
                    match value {
                        Ok(Some(text)) => {
                            reachable = true;
                            // Retained verbatim: only the family is applied.
                            preferences.set(*role, Some(RawFontDescription::new(text)));
                        }
                        // A key the backend does not export is a silent role,
                        // not a failure of the whole read.
                        Ok(None) => reachable = true,
                        Err(error) if is_not_found(&error) => reachable = true,
                        Err(error) => return Err(error),
                    }
                }
                Ok((reachable, preferences))
            })
        });

        match outcome {
            Ok((true, preferences)) if !preferences.is_empty() => {
                FontPreferenceRead::Delivered(preferences)
            }
            // Reachable but silent about every role: the portal exists and has
            // nothing to say, which the caller must be able to distinguish
            // from an unreachable bus.
            Ok((true, _)) => FontPreferenceRead::Unsupported,
            Ok((false, _)) => FontPreferenceRead::Unsupported,
            Err(error) if is_unavailable(&error) => FontPreferenceRead::Unsupported,
            Err(error) => FontPreferenceRead::Failed {
                detail: error.to_string(),
            },
        }
    }

    fn subscribe(&self) -> Option<Box<dyn FontPreferenceSignals>> {
        let (runtime, connection) = self.signal_connection().ok()?;
        PortalSignals::start(runtime, connection, self.namespace.clone())
            .map(|signals| Box::new(signals) as Box<dyn FontPreferenceSignals>)
    }
}

// The `SettingChanged` stream, waited on with a bound so the feed's worker
// stays cancellable.
struct PortalSignals {
    runtime: Option<tokio::runtime::Runtime>,
    connection: Option<zbus::Connection>,
    stream: Option<SettingChangedStream>,
    namespace: String,
}

#[cfg(test)]
mod signal_drop_observer {
    use std::cell::RefCell;
    use std::sync::mpsc::SyncSender;

    thread_local! {
        static OBSERVER: RefCell<Option<SyncSender<bool>>> = const { RefCell::new(None) };
    }

    pub(super) fn record(runtime_entered: bool) {
        OBSERVER.with(|slot| {
            if let Some(observer) = slot.borrow_mut().take() {
                let _ = observer.send(runtime_entered);
            }
        });
    }
}

impl PortalSignals {
    fn start(
        runtime: tokio::runtime::Runtime,
        connection: zbus::Connection,
        namespace: String,
    ) -> Option<Self> {
        let mut signals = Self {
            runtime: Some(runtime),
            connection: Some(connection),
            stream: None,
            namespace,
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
        // Same reactor contract as [`Session`]: the signal stream and its
        // connection are registered with the runtime and must be released
        // while it is still entered, or their destructors abort the process.
        let Some(runtime) = self.runtime.take() else {
            return;
        };
        {
            let _guard = runtime.enter();
            drop(self.stream.take());
            drop(self.connection.take());
            #[cfg(test)]
            signal_drop_observer::record(tokio::runtime::Handle::try_current().is_ok());
        }
        drop(runtime);
    }
}

impl FontPreferenceSignals for PortalSignals {
    fn wait_for_change(&mut self, timeout: Duration) -> FontSignal {
        self.wait(timeout, None)
    }

    fn wait_for_change_until_cancelled(
        &mut self,
        timeout: Duration,
        _slice: Duration,
        cancel: &SignalCancel,
    ) -> FontSignal {
        self.wait(timeout, Some(cancel))
    }
}

impl PortalSignals {
    fn wait(&mut self, timeout: Duration, cancel: Option<&SignalCancel>) -> FontSignal {
        let namespace = self.namespace.clone();
        let Some(stream) = self.stream.as_mut() else {
            return FontSignal::Closed;
        };
        let runtime = self
            .runtime
            .as_ref()
            .expect("the runtime outlives every wait");
        runtime.block_on(async move {
            let next = tokio::time::timeout(timeout, stream.next());
            let next = match cancel {
                None => next.await,
                Some(cancel) => {
                    let mut next = std::pin::pin!(next);
                    let mut cancelled = std::pin::pin!(cancel.cancelled());
                    let raced = std::future::poll_fn(|context| {
                        if cancelled.as_mut().poll(context).is_ready() {
                            return std::task::Poll::Ready(None);
                        }
                        next.as_mut().poll(context).map(Some)
                    })
                    .await;
                    match raced {
                        Some(next) => next,
                        None => return FontSignal::Idle,
                    }
                }
            };
            match next {
                Err(_) => FontSignal::Idle,
                Ok(None) => FontSignal::Closed,
                Ok(Some(signal)) => match signal.args() {
                    // Another namespace's change is not ours to re-read for.
                    Ok(args) if *args.namespace() == namespace.as_str() => FontSignal::Changed,
                    Ok(_) => FontSignal::Idle,
                    Err(_) => FontSignal::Changed,
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
    namespace: &str,
    key: &str,
) -> zbus::Result<Option<String>> {
    let value = match with_timeout(proxy.read_one(namespace, key)).await {
        Ok(value) => value,
        Err(error) if is_unknown_method(&error) => with_timeout(proxy.read(namespace, key)).await?,
        Err(error) => return Err(error),
    };
    Ok(unwrap_string(&value))
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

/// Unwraps nested variants down to a string, the only shape a font key has.
///
/// Returns `None` for any other type, so a backend answering with a number or
/// an array is a silent role rather than a panic.
#[must_use]
pub fn unwrap_string(value: &OwnedValue) -> Option<String> {
    fn inner(value: &Value<'_>) -> Option<String> {
        match value {
            Value::Str(text) => Some(text.to_string()),
            Value::Value(nested) => inner(nested),
            _ => None,
        }
    }
    inner(value)
}

fn error_name(error: &zbus::Error) -> Option<String> {
    match error {
        zbus::Error::MethodError(name, _, _) => Some(name.as_str().to_owned()),
        _ => None,
    }
}

fn is_not_found(error: &zbus::Error) -> bool {
    error_name(error).is_some_and(|name| {
        name == "org.freedesktop.portal.Error.NotFound"
            || name == "org.freedesktop.DBus.Error.InvalidArgs"
            || name == "org.freedesktop.DBus.Error.Failed"
    })
}

fn is_unknown_method(error: &zbus::Error) -> bool {
    error_name(error).is_some_and(|name| {
        name == "org.freedesktop.DBus.Error.UnknownMethod"
            || name == "org.freedesktop.DBus.Error.NotSupported"
    })
}

fn is_unavailable(error: &zbus::Error) -> bool {
    match error {
        zbus::Error::Address(_) => true,
        // A call timeout on an established connection is a transient failure,
        // not an absent source: Failed retains the last delivery while
        // Unsupported would release it.
        zbus::Error::InputOutput(io) if io.kind() != std::io::ErrorKind::TimedOut => true,
        _ => error_name(error).is_some_and(|name| {
            name == "org.freedesktop.DBus.Error.ServiceUnknown"
                || name == "org.freedesktop.DBus.Error.NameHasNoOwner"
                || name == "org.freedesktop.DBus.Error.UnknownInterface"
                || name == "org.freedesktop.DBus.Error.UnknownObject"
        }),
    }
}

#[cfg(test)]
mod tests;
