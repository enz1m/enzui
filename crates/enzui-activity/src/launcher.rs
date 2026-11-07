//! Freedesktop/Unity launcher progress for desktops without the enzimOS broker.
//!
//! When the activity socket cannot be reached (another desktop, or a Flatpak
//! sandbox without access to it), the publisher's live jobs are summarised as
//! one `com.canonical.Unity.LauncherEntry.Update` signal on the session bus:
//! `progress` (0..1) and `progress-visible`. Docks such as GNOME's, KDE's and
//! Plank show it, and enzimOS bridges it into ordinary jobs. While the socket
//! serves the jobs no signal is sent, so the shell never shows them twice.
//! Flatpak apps need `--talk-name=com.canonical.Unity` for the signal to leave
//! the sandbox.

use super::Entry;
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::time::Instant;

/// The longest one launcher signal, with its connection, may take.
#[cfg(feature = "launcher-entry")]
const EMIT_DEADLINE: Duration = Duration::from_millis(500);

/// The shortest gap between two signals: at most 2 Hz, like the socket client.
pub(super) const MIN_INTERVAL: Duration = Duration::from_millis(500);

/// Which session bus the launcher signal goes to.
#[derive(Clone, Debug)]
#[cfg_attr(not(feature = "launcher-entry"), allow(dead_code))]
pub(super) enum Bus {
    /// The user's session bus.
    Session,
    /// An explicit bus address, for a qualified proxy or an isolated test.
    Address(String),
    /// No launcher signal.
    Disabled,
}

/// What the dock should show for the app.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Summary {
    pub visible: bool,
    pub progress: f64,
}

impl Summary {
    pub(crate) const HIDDEN: Self = Self {
        visible: false,
        progress: 0.0,
    };

    /// The mean completed fraction of the live jobs that measure progress, or
    /// an empty bar when none does.
    pub(crate) fn of(jobs: &BTreeMap<String, Entry>) -> Self {
        let live: Vec<&Entry> = jobs
            .values()
            .filter(|entry| {
                !entry.removed
                    && entry.terminal.is_none()
                    && entry
                        .visible_at
                        .is_none_or(|deadline| deadline <= Instant::now().into_std())
            })
            .collect();
        if live.is_empty() {
            return Self::HIDDEN;
        }
        let fractions: Vec<f64> = live
            .iter()
            .filter_map(|entry| entry.status.progress.as_ref())
            .map(|progress| progress.done as f64 / progress.total as f64)
            .collect();
        let progress = if fractions.is_empty() {
            0.0
        } else {
            fractions.iter().sum::<f64>() / fractions.len() as f64
        };
        Self {
            visible: true,
            progress: progress.clamp(0.0, 1.0),
        }
    }
}

pub(super) struct Launcher {
    bus: Bus,
    app_uri: String,
    path: String,
    #[cfg(feature = "launcher-entry")]
    connection: Option<zbus::Connection>,
    shown: Summary,
    sent_at: Option<Instant>,
}

impl Launcher {
    pub(crate) fn new(bus: Bus, app_id: &str) -> Self {
        let segment: String = app_id
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        Self {
            bus,
            app_uri: format!("application://{app_id}.desktop"),
            path: format!("/com/canonical/unity/launcherentry/{segment}"),
            #[cfg(feature = "launcher-entry")]
            connection: None,
            shown: Summary::HIDDEN,
            sent_at: None,
        }
    }

    /// When the next signal may be sent, or `None` if it may be sent now.
    pub(crate) fn ready_at(&self) -> Option<Instant> {
        self.sent_at
            .map(|sent| sent + MIN_INTERVAL)
            .filter(|ready| *ready > Instant::now())
    }

    /// Whether `summary` differs from what the dock shows.
    pub(crate) fn changes(&self, summary: Summary) -> bool {
        cfg!(feature = "launcher-entry")
            && !matches!(self.bus, Bus::Disabled)
            && summary != self.shown
    }

    /// Hides a shown bar at once, whatever the rate: the socket now owns the
    /// jobs, or none is left.
    pub(crate) async fn hide(&mut self) {
        if self.shown.visible {
            self.sent_at = None;
            let _ = self.show(Summary::HIDDEN).await;
        }
    }

    /// Shows `summary`, if it changed and the rate allows. Returns whether a
    /// signal was sent; failures are dropped like any other launcher state.
    pub(crate) async fn show(&mut self, summary: Summary) -> bool {
        if !self.changes(summary) || self.ready_at().is_some() {
            return false;
        }
        // Every attempt counts against the rate, so a missing bus is retried
        // at 2 Hz rather than in a loop. The rate runs from the attempt's end,
        // so a slow bus is not retried back to back either.
        self.sent_at = Some(Instant::now());
        let sent = self.emit(summary).await;
        self.sent_at = Some(Instant::now());
        sent
    }

    #[cfg(not(feature = "launcher-entry"))]
    async fn emit(&mut self, _summary: Summary) -> bool {
        let _ = (&self.app_uri, &self.path);
        false
    }

    /// One attempt, connection included, ends within [`EMIT_DEADLINE`]: a bus
    /// that accepts the connection and then stalls drops it and is retried at
    /// the launcher's rate, rather than holding the activity actor.
    #[cfg(feature = "launcher-entry")]
    async fn emit(&mut self, summary: Summary) -> bool {
        if let Ok(sent) = tokio::time::timeout(EMIT_DEADLINE, self.emit_now(summary)).await {
            sent
        } else {
            self.connection = None;
            false
        }
    }

    #[cfg(feature = "launcher-entry")]
    async fn emit_now(&mut self, summary: Summary) -> bool {
        if self.connection.is_none() {
            self.connection = match &self.bus {
                Bus::Session => zbus::Connection::session().await.ok(),
                Bus::Address(address) => match zbus::connection::Builder::address(address.as_str())
                {
                    Ok(builder) => builder.build().await.ok(),
                    Err(_) => None,
                },
                Bus::Disabled => None,
            };
        }
        let Some(connection) = &self.connection else {
            return false;
        };
        let mut properties: std::collections::HashMap<&str, zbus::zvariant::Value<'_>> =
            std::collections::HashMap::new();
        properties.insert("progress", summary.progress.into());
        properties.insert("progress-visible", summary.visible.into());
        let sent = connection
            .emit_signal(
                None::<&str>,
                self.path.as_str(),
                "com.canonical.Unity.LauncherEntry",
                "Update",
                &(self.app_uri.as_str(), properties),
            )
            .await
            .is_ok();
        if sent {
            self.shown = summary;
        } else {
            self.connection = None;
        }
        sent
    }
}
