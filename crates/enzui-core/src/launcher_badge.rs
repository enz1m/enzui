//! A count and an urgent mark on the app's dock or taskbar launcher:
//! `com.canonical.Unity.LauncherEntry`.
//!
//! [`LauncherBadge::new`] names the app by its ID (the `.desktop` file's
//! name). Call [`LauncherBadge::set_count`] with the unread count, or `None`
//! to hide it, and [`LauncherBadge::set_urgent`] while the app needs the user;
//! only changes are sent. KDE's task manager, Ubuntu's and Dash to Dock's
//! docks, Plank and Latte show them. Job progress on the same launcher comes
//! from `enzui-activity`, and the two do not disturb each other: every update
//! names only the properties it changes. Flatpak apps need
//! `--talk-name=com.canonical.Unity`.

use std::collections::HashMap;
use std::sync::Arc;
use zbus::zvariant::Value;

/// What the launcher shows besides progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct BadgeState {
    /// The number on the launcher, or `None` for no number.
    pub count: Option<i64>,
    /// Whether the launcher asks for attention.
    pub urgent: bool,
}

/// The app's launcher badge; see the module docs.
#[derive(Debug)]
pub struct LauncherBadge {
    shown: BadgeState,
    updates: tokio::sync::mpsc::UnboundedSender<BadgeState>,
}

impl LauncherBadge {
    /// The badge of the launcher for `app_id`, on the session bus.
    #[must_use]
    pub fn new(app_id: &str) -> Self {
        Self::start(app_id, None)
    }

    /// The badge of the launcher for `app_id`, on the bus at `address`.
    #[must_use]
    pub fn on_bus(app_id: &str, address: impl Into<String>) -> Self {
        Self::start(app_id, Some(address.into()))
    }

    fn start(app_id: &str, address: Option<String>) -> Self {
        let (updates, mut receiver) = tokio::sync::mpsc::unbounded_channel::<BadgeState>();
        let app_uri = format!("application://{app_id}.desktop");
        let segment: String = app_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let path: Arc<str> = format!("/com/canonical/unity/launcherentry/{segment}").into();
        let _ = std::thread::Builder::new()
            .name("enzui-launcher-badge".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                runtime.block_on(async move {
                    let mut connection = None;
                    let mut shown = BadgeState::default();
                    while let Some(mut wanted) = receiver.recv().await {
                        // Only the newest of a burst matters.
                        while let Ok(newer) = receiver.try_recv() {
                            wanted = newer;
                        }
                        if connection.is_none() {
                            connection = connect(address.as_deref()).await;
                        }
                        let Some(bus) = &connection else {
                            continue;
                        };
                        if send(bus, &path, &app_uri, shown, wanted).await {
                            shown = wanted;
                        } else {
                            connection = None;
                        }
                    }
                    // Leave the launcher as it was without the app.
                    if shown != BadgeState::default()
                        && let Some(bus) = &connection
                    {
                        let _ = send(bus, &path, &app_uri, shown, BadgeState::default()).await;
                    }
                });
            });
        Self {
            shown: BadgeState::default(),
            updates,
        }
    }

    /// Shows `count` on the launcher, or no number for `None`.
    pub fn set_count(&mut self, count: Option<i64>) {
        self.set(BadgeState {
            count,
            ..self.shown
        });
    }

    /// Asks for attention while `urgent`.
    pub fn set_urgent(&mut self, urgent: bool) {
        self.set(BadgeState {
            urgent,
            ..self.shown
        });
    }

    /// Shows `state`, if it differs from what is shown.
    pub fn set(&mut self, state: BadgeState) {
        if state != self.shown {
            self.shown = state;
            let _ = self.updates.send(state);
        }
    }

    /// The badge as last set.
    #[must_use]
    pub const fn state(&self) -> BadgeState {
        self.shown
    }
}

async fn connect(address: Option<&str>) -> Option<zbus::Connection> {
    match address {
        Some(address) => zbus::connection::Builder::address(address)
            .ok()?
            .build()
            .await
            .ok(),
        None => zbus::Connection::session().await.ok(),
    }
}

/// Sends the properties that differ between `shown` and `wanted`.
async fn send(
    connection: &zbus::Connection,
    path: &str,
    app_uri: &str,
    shown: BadgeState,
    wanted: BadgeState,
) -> bool {
    let mut properties: HashMap<&str, Value<'_>> = HashMap::new();
    if wanted.count != shown.count {
        let _ = properties.insert("count", wanted.count.unwrap_or(0).into());
        let _ = properties.insert("count-visible", wanted.count.is_some().into());
    }
    if wanted.urgent != shown.urgent {
        let _ = properties.insert("urgent", wanted.urgent.into());
    }
    if properties.is_empty() {
        return true;
    }
    connection
        .emit_signal(
            None::<&str>,
            path,
            "com.canonical.Unity.LauncherEntry",
            "Update",
            &(app_uri, properties),
        )
        .await
        .is_ok()
}
