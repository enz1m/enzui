//! Live icon-theme observations through the Settings portal.
//!
//! The transport is standardized; `org.gnome.desktop.interface/icon-theme` is
//! a GNOME backend extension. Missing keys or a missing portal leave the
//! service's explicit fallback theme intact. Capture the bus address at boot,
//! retain the watch, and include its subscription to wake an idle application.

use super::{DesktopIconService, Inner, loader};
use futures_util::{StreamExt, stream};
use iced_futures::Subscription;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::watch;
use zbus::zvariant::{OwnedValue, Value};

const CALL_TIMEOUT: Duration = Duration::from_secs(2);

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

/// Captured session address and backend-specific icon-theme setting key.
#[derive(Clone)]
#[must_use]
pub struct PortalIconContext {
    address: String,
    namespace: String,
    key: String,
}
impl PortalIconContext {
    /// Use an explicit bus address and the GNOME icon-theme key.
    pub fn new(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
            namespace: "org.gnome.desktop.interface".into(),
            key: "icon-theme".into(),
        }
    }
    /// Capture `DBUS_SESSION_BUS_ADDRESS`; missing or non-UTF-8 addresses return `None`.
    pub fn from_env() -> Option<Self> {
        std::env::var("DBUS_SESSION_BUS_ADDRESS")
            .ok()
            .filter(|s| !s.is_empty())
            .map(Self::new)
    }
    /// Read a different desktop backend's namespace/key instead of GNOME's extension.
    pub fn with_key(mut self, namespace: impl Into<String>, key: impl Into<String>) -> Self {
        self.namespace = namespace.into();
        self.key = key.into();
        self
    }
}

/// Latest host observation; a slow subscriber retains only the newest value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconThemeObservation {
    /// Connection/read startup has not completed.
    Waiting,
    /// A valid selection was applied to the service; resolve visible artwork again.
    Selected {
        /// Backend-provided icon-theme name.
        theme: String,
        /// Service epoch invalidating earlier lookup replies.
        epoch: u64,
    },
    /// The portal/key/transport is unavailable; the existing selection remains intact.
    Unavailable,
}

struct WatchState {
    receiver: watch::Receiver<IconThemeObservation>,
}
#[derive(Clone)]
struct WatchKey(Arc<WatchState>);
impl PartialEq for WatchKey {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}
impl Eq for WatchKey {}
impl Hash for WatchKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (Arc::as_ptr(&self.0) as usize).hash(state);
    }
}

/// Owns one portal worker; dropping it cancels startup or signal waits and joins it.
pub struct PortalIconWatch {
    key: WatchKey,
    cancel: watch::Sender<bool>,
    worker: Option<JoinHandle<()>>,
}
impl PortalIconWatch {
    /// A stable recipe delivering the newest observation without a polling timer.
    pub fn subscription(&self) -> Subscription<IconThemeObservation> {
        Subscription::run_with(self.key.clone(), |key| {
            stream::unfold(
                (key.0.receiver.clone(), true),
                |(mut receiver, first)| async move {
                    if !first && receiver.changed().await.is_err() {
                        return None;
                    }
                    let event = receiver.borrow_and_update().clone();
                    Some((event, (receiver, false)))
                },
            )
        })
    }
    /// The latest retained observation, without waiting or filesystem access.
    pub fn latest(&self) -> IconThemeObservation {
        self.key.0.receiver.borrow().clone()
    }
}
impl Drop for PortalIconWatch {
    fn drop(&mut self) {
        self.cancel.send_replace(true);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl DesktopIconService {
    /// Start a portal worker which applies valid host selections and emits observations.
    ///
    /// Subscribe through [`PortalIconWatch::subscription`] so theme changes wake
    /// the application and trigger new lookups. No global/session environment is changed.
    ///
    /// # Errors
    /// Returns an invalid-input error for an empty/oversized context, or a thread-start error.
    pub fn watch_portal(&self, context: PortalIconContext) -> std::io::Result<PortalIconWatch> {
        if context.address.is_empty()
            || context.address.len() > 16384
            || context.namespace.is_empty()
            || context.namespace.len() > 255
            || context.key.is_empty()
            || context.key.len() > 255
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid icon portal context",
            ));
        }
        let (sender, receiver) = watch::channel(IconThemeObservation::Waiting);
        let (cancel, mut cancellation) = watch::channel(false);
        let inner = Arc::downgrade(&self.inner);
        let worker = std::thread::Builder::new()
            .name("icon-theme-portal".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    sender.send_replace(IconThemeObservation::Unavailable);
                    return;
                };
                runtime.block_on(async {
                    let _ = futures_util::future::select(
                        Box::pin(cancellation.changed()),
                        Box::pin(observe(context, inner, &sender)),
                    )
                    .await;
                });
                runtime.shutdown_timeout(Duration::from_millis(100));
            })?;
        Ok(PortalIconWatch {
            key: WatchKey(Arc::new(WatchState { receiver })),
            cancel,
            worker: Some(worker),
        })
    }
}

async fn read_theme(proxy: &SettingsProxy<'_>, context: &PortalIconContext) -> Option<String> {
    let first = tokio::time::timeout(
        CALL_TIMEOUT,
        proxy.read_one(&context.namespace, &context.key),
    )
    .await
    .ok()?;
    let value = match first {
        Ok(value) => value,
        Err(zbus::Error::MethodError(name, _, _))
            if name.as_str() == "org.freedesktop.DBus.Error.UnknownMethod" =>
        {
            tokio::time::timeout(CALL_TIMEOUT, proxy.read(&context.namespace, &context.key))
                .await
                .ok()?
                .ok()?
        }
        Err(_) => return None,
    };
    let mut value: &Value<'_> = &value;
    for _ in 0..4 {
        match value {
            Value::Str(text) if loader::valid_name(text.as_str()) => return Some(text.to_string()),
            Value::Value(inner) => value = inner,
            _ => return None,
        }
    }
    None
}

fn apply(inner: &Weak<Inner>, name: String, sender: &watch::Sender<IconThemeObservation>) -> bool {
    let Some(inner) = inner.upgrade() else {
        return false;
    };
    if inner.set_theme(name).is_err() {
        return true;
    }
    let theme = inner
        .theme
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    sender.send_replace(IconThemeObservation::Selected {
        theme: theme.name,
        epoch: theme.epoch,
    });
    true
}

async fn observe(
    context: PortalIconContext,
    inner: Weak<Inner>,
    sender: &watch::Sender<IconThemeObservation>,
) {
    let connected = async {
        let connection = zbus::connection::Builder::address(context.address.as_str())?
            .max_queued(16)
            .build()
            .await?;
        Ok::<_, zbus::Error>(connection)
    };
    let Ok(Ok(connection)) = tokio::time::timeout(CALL_TIMEOUT, connected).await else {
        sender.send_replace(IconThemeObservation::Unavailable);
        return;
    };
    let Ok(Ok(proxy)) = tokio::time::timeout(CALL_TIMEOUT, SettingsProxy::new(&connection)).await
    else {
        sender.send_replace(IconThemeObservation::Unavailable);
        return;
    };
    let Ok(Ok(mut changes)) =
        tokio::time::timeout(CALL_TIMEOUT, proxy.receive_setting_changed()).await
    else {
        sender.send_replace(IconThemeObservation::Unavailable);
        return;
    };
    if let Some(name) = read_theme(&proxy, &context).await {
        if !apply(&inner, name, sender) {
            return;
        }
    } else {
        sender.send_replace(IconThemeObservation::Unavailable);
    }
    while let Some(change) = changes.next().await {
        let Ok(args) = change.args() else {
            continue;
        };
        if args.namespace != context.namespace || args.key != context.key {
            continue;
        }
        // Queued signals can predate the initial read, so re-read the current selection.
        if let Some(name) = read_theme(&proxy, &context).await {
            if !apply(&inner, name, sender) {
                return;
            }
        } else {
            sender.send_replace(IconThemeObservation::Unavailable);
        }
    }
    sender.send_replace(IconThemeObservation::Unavailable);
}
