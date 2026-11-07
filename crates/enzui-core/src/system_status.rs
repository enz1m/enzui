//! What the system is doing around the app: network connectivity, memory
//! pressure and power-saver mode.
//!
//! Each subscription asks the desktop portal first (`NetworkMonitor`,
//! `MemoryMonitor`, `PowerProfileMonitor`), which works inside a sandbox.
//! Without the portal it reads the system directly: NetworkManager for the
//! network, the kernel's pressure-stall information
//! (`/proc/pressure/memory`) for memory, and power-profiles-daemon for
//! power-saver mode. A subscription whose sources are all missing delivers
//! nothing, and the app keeps its defaults.
//!
//! - [`network_subscription`]: a [`NetworkStatus`] at once and on every change.
//!   Hold off big downloads while [`NetworkStatus::metered`], and show an
//!   offline state when it is not [`NetworkStatus::available`].
//! - [`memory_subscription`]: a [`MemoryPressure`] when the system runs low.
//!   Drop caches (decoded images, undo history beyond a point).
//! - [`power_saver_subscription`]: `true` while power-saver mode is on. Cut
//!   animation and polling.
//!
//! The `run_*` functions are the same loops with the connections and the
//! pressure file passed in.

use iced_futures::futures::StreamExt as _;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;

/// How far the network reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Connectivity {
    /// Only this machine.
    Local,
    /// The local network, not the internet.
    Limited,
    /// A captive portal (a hotel's sign-in page) stands before the internet.
    CaptivePortal,
    /// The internet.
    Full,
}

/// The network as the app sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NetworkStatus {
    /// Whether there is a network to use at all (a default route).
    pub available: bool,
    /// Whether traffic costs the user (a phone hotspot, a capped plan).
    pub metered: bool,
    /// How far the network reaches.
    pub connectivity: Connectivity,
}

/// How short of memory the system is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MemoryPressure {
    /// Free what is cheap to rebuild.
    Low,
    /// Free what the app can do without.
    Medium,
    /// Free everything possible: the system is about to kill processes.
    Critical,
}

impl MemoryPressure {
    /// The portal's warning level (GLib's: 50, 100, 255) as a pressure.
    #[must_use]
    pub const fn from_level(level: u8) -> Self {
        match level {
            0..100 => Self::Low,
            100..255 => Self::Medium,
            255 => Self::Critical,
        }
    }
}

const NM_DEST: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const PPD_DEST: &str = "org.freedesktop.UPower.PowerProfiles";
const PPD_PATH: &str = "/org/freedesktop/UPower/PowerProfiles";
const PORTAL_DEST: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
/// How often the pressure file is read without the portal.
const PRESSURE_POLL: Duration = Duration::from_secs(2);

/// Sends the network status to `events` at once and on each change, until
/// `events` closes: from the portal on `session`, or else NetworkManager on
/// `system`. Returns at once when neither answers.
pub async fn run_network(
    session: Option<zbus::Connection>,
    system: Option<zbus::Connection>,
    events: UnboundedSender<NetworkStatus>,
) {
    if let Some(session) = session
        && let Ok(portal) =
            ashpd::desktop::network_monitor::NetworkMonitor::with_connection(session).await
        && let Ok(changes) = portal.receive_changed().await
        && let Ok(first) = portal.status().await
    {
        let read = |status: ashpd::desktop::network_monitor::NetworkStatus| NetworkStatus {
            available: status.is_available(),
            metered: status.is_metered(),
            connectivity: match status.connectivity() {
                ashpd::desktop::network_monitor::Connectivity::Local => Connectivity::Local,
                ashpd::desktop::network_monitor::Connectivity::Limited => Connectivity::Limited,
                ashpd::desktop::network_monitor::Connectivity::CaptivePortal => {
                    Connectivity::CaptivePortal
                }
                ashpd::desktop::network_monitor::Connectivity::FullNetwork => Connectivity::Full,
            },
        };
        let mut changes = std::pin::pin!(changes);
        let mut shown = read(first);
        if events.send(shown).is_err() {
            return;
        }
        while until_closed(changes.next(), &events).await.is_some() {
            if let Ok(status) = portal.status().await.map(read)
                && status != shown
            {
                shown = status;
                if events.send(status).is_err() {
                    return;
                }
            }
        }
        return;
    }
    let Some(system) = system else {
        return;
    };
    watch_properties(
        &system,
        NM_DEST,
        NM_PATH,
        &events,
        |properties| async move {
            let connectivity: u32 = properties.get(NM_DEST, "Connectivity").await.ok()?;
            let metered: u32 = properties.get(NM_DEST, "Metered").await.ok()?;
            let state: u32 = properties.get(NM_DEST, "State").await.ok()?;
            Some(network_from_manager(connectivity, metered, state))
        },
    )
    .await;
}

/// NetworkManager's `Connectivity`, `Metered` and `State` as a status.
fn network_from_manager(connectivity: u32, metered: u32, state: u32) -> NetworkStatus {
    // NM_CONNECTIVITY_*: 0 unknown, 1 none, 2 portal, 3 limited, 4 full.
    let reach = match connectivity {
        4 => Connectivity::Full,
        3 => Connectivity::Limited,
        2 => Connectivity::CaptivePortal,
        1 => Connectivity::Local,
        // Unknown (checking disabled): go by NM_STATE.
        _ => match state {
            70 => Connectivity::Full,
            50 | 60 => Connectivity::Limited,
            _ => Connectivity::Local,
        },
    };
    NetworkStatus {
        // NM_STATE_CONNECTED_LOCAL and up have a route.
        available: state >= 50,
        // NM_METERED_YES or NM_METERED_GUESS_YES.
        metered: matches!(metered, 1 | 3),
        connectivity: reach,
    }
}

/// Sends a pressure to `events` each time memory runs lower, until `events`
/// closes: from the portal on `session`, or else by reading `pressure_file`
/// (`/proc/pressure/memory`) every two seconds. Returns at once when
/// neither is there.
pub async fn run_memory(
    session: Option<zbus::Connection>,
    pressure_file: Option<PathBuf>,
    events: UnboundedSender<MemoryPressure>,
) {
    if let Some(session) = session
        && has_portal(&session, "org.freedesktop.portal.MemoryMonitor").await
        && let Ok(portal) =
            ashpd::desktop::memory_monitor::MemoryMonitor::with_connection(session).await
        // The signal carries a byte; ashpd's own stream decodes an `i32`
        // and drops every warning, so read it from the proxy directly.
        && let Ok(warnings) = portal.receive_signal("LowMemoryWarning").await
    {
        let mut warnings = std::pin::pin!(warnings);
        while let Some(message) = until_closed(warnings.next(), &events).await {
            let Ok(level) = message.body().deserialize::<u8>() else {
                continue;
            };
            if events.send(MemoryPressure::from_level(level)).is_err() {
                return;
            }
        }
        return;
    }
    let Some(path) = pressure_file else {
        return;
    };
    let mut last: Option<MemoryPressure> = None;
    loop {
        let Ok(text) = tokio::fs::read_to_string(&path).await else {
            return;
        };
        let now = pressure_from_stall(&text);
        // Report each rise; stay quiet while it holds or eases.
        if now > last
            && let Some(pressure) = now
            && events.send(pressure).is_err()
        {
            return;
        }
        last = now;
        if until_closed(
            async {
                tokio::time::sleep(PRESSURE_POLL).await;
                Some(())
            },
            &events,
        )
        .await
        .is_none()
        {
            return;
        }
    }
}

/// The pressure in `/proc/pressure/memory`'s text, from its ten-second
/// averages: the share of time some or all tasks waited for memory.
fn pressure_from_stall(text: &str) -> Option<MemoryPressure> {
    let average = |kind: &str| -> f64 {
        text.lines()
            .find(|line| line.starts_with(kind))
            .and_then(|line| {
                line.split_whitespace()
                    .find_map(|field| field.strip_prefix("avg10="))
            })
            .and_then(|value| value.parse().ok())
            .unwrap_or(0.0)
    };
    let (some, full) = (average("some"), average("full"));
    if full >= 10.0 {
        Some(MemoryPressure::Critical)
    } else if some >= 30.0 {
        Some(MemoryPressure::Medium)
    } else if some >= 10.0 {
        Some(MemoryPressure::Low)
    } else {
        None
    }
}

/// Sends whether power-saver mode is on to `events` at once and on each
/// change, until `events` closes: from the portal on `session`, or else
/// power-profiles-daemon on `system`. Returns at once when neither answers.
pub async fn run_power_saver(
    session: Option<zbus::Connection>,
    system: Option<zbus::Connection>,
    events: UnboundedSender<bool>,
) {
    if let Some(session) = session
        && has_portal(&session, "org.freedesktop.portal.PowerProfileMonitor").await
    {
        watch_properties(
            &session,
            PORTAL_DEST,
            PORTAL_PATH,
            &events,
            |properties| async move {
                properties
                    .get(
                        "org.freedesktop.portal.PowerProfileMonitor",
                        "power-saver-enabled",
                    )
                    .await
                    .ok()
            },
        )
        .await;
        return;
    }
    let Some(system) = system else {
        return;
    };
    watch_properties(
        &system,
        PPD_DEST,
        PPD_PATH,
        &events,
        |properties| async move {
            let profile: String = properties.get(PPD_DEST, "ActiveProfile").await.ok()?;
            Some(profile == "power-saver")
        },
    )
    .await;
}

/// Reads an object's state with `read`, sends it, and sends it again each
/// time a `PropertiesChanged` changes it, until `events` closes or the
/// object's service goes away.
async fn watch_properties<T, F>(
    connection: &zbus::Connection,
    destination: &'static str,
    path: &'static str,
    events: &UnboundedSender<T>,
    read: impl Fn(Reader) -> F,
) where
    T: PartialEq + Copy,
    F: std::future::Future<Output = Option<T>>,
{
    let Ok(builder) = zbus::fdo::PropertiesProxy::builder(connection)
        .destination(destination)
        .and_then(|builder| builder.path(path))
    else {
        return;
    };
    let Ok(proxy) = builder
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
    else {
        return;
    };
    let Ok(changes) = proxy.receive_properties_changed().await else {
        return;
    };
    let mut changes = std::pin::pin!(changes);
    let Some(mut shown) = read(Reader(proxy.clone())).await else {
        return;
    };
    if events.send(shown).is_err() {
        return;
    }
    while until_closed(changes.next(), events).await.is_some() {
        if let Some(now) = read(Reader(proxy.clone())).await
            && now != shown
        {
            shown = now;
            if events.send(now).is_err() {
                return;
            }
        }
    }
}

/// Reads one property of the watched object.
struct Reader(zbus::fdo::PropertiesProxy<'static>);

impl Reader {
    async fn get<T>(&self, interface: &str, name: &str) -> zbus::Result<T>
    where
        T: TryFrom<zbus::zvariant::OwnedValue>,
        T::Error: Into<zbus::Error>,
    {
        let interface = zbus::names::InterfaceName::try_from(interface)?;
        let value = self.0.get(interface, name).await?;
        T::try_from(value).map_err(Into::into)
    }
}

/// Whether the desktop portal implements `interface`: its `version` reads.
/// (ashpd's proxies take a missing portal for version 1.)
async fn has_portal(connection: &zbus::Connection, interface: &'static str) -> bool {
    let Ok(builder) = zbus::fdo::PropertiesProxy::builder(connection)
        .destination(PORTAL_DEST)
        .and_then(|builder| builder.path(PORTAL_PATH))
    else {
        return false;
    };
    let Ok(proxy) = builder.build().await else {
        return false;
    };
    Reader(proxy).get::<u32>(interface, "version").await.is_ok()
}

/// `future`'s output, or `None` once `events` closes first.
async fn until_closed<T, E>(
    future: impl std::future::Future<Output = Option<T>>,
    events: &UnboundedSender<E>,
) -> Option<T> {
    use iced_futures::futures::future::{Either, select};
    match select(Box::pin(future), Box::pin(events.closed())).await {
        Either::Left((value, _)) => value,
        Either::Right(_) => None,
    }
}

/// A subscription running `run` on a thread with the session and system
/// buses.
fn bus_subscription<T, F>(
    name: &'static str,
    run: fn(Option<zbus::Connection>, Option<zbus::Connection>, UnboundedSender<T>) -> F,
) -> iced_futures::Subscription<T>
where
    T: Send + 'static,
    F: std::future::Future<Output = ()> + 'static,
{
    struct Recipe<T, F> {
        name: &'static str,
        run: fn(Option<zbus::Connection>, Option<zbus::Connection>, UnboundedSender<T>) -> F,
    }
    impl<T, F> iced_futures::subscription::Recipe for Recipe<T, F>
    where
        T: Send + 'static,
        F: std::future::Future<Output = ()> + 'static,
    {
        type Output = T;

        fn hash(&self, state: &mut iced_futures::subscription::Hasher) {
            use std::hash::Hash;
            self.name.hash(state);
        }

        fn stream(
            self: Box<Self>,
            _input: iced_futures::subscription::EventStream,
        ) -> iced_futures::BoxStream<Self::Output> {
            let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
            let run = self.run;
            let _ = std::thread::Builder::new()
                .name(self.name.into())
                .spawn(move || {
                    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    else {
                        return;
                    };
                    runtime.block_on(async move {
                        let session = zbus::Connection::session().await.ok();
                        let system = zbus::Connection::system().await.ok();
                        run(session, system, sender).await;
                    });
                });
            Box::pin(iced_futures::futures::stream::unfold(
                receiver,
                |mut receiver| async move { receiver.recv().await.map(|event| (event, receiver)) },
            ))
        }
    }
    iced_futures::subscription::from_recipe(Recipe { name, run })
}

/// The network status, at once and on every change; see the module docs.
pub fn network_subscription() -> iced_futures::Subscription<NetworkStatus> {
    bus_subscription("enzui-network-monitor", run_network)
}

/// Memory pressure warnings; see the module docs.
pub fn memory_subscription() -> iced_futures::Subscription<MemoryPressure> {
    async fn run(
        session: Option<zbus::Connection>,
        _system: Option<zbus::Connection>,
        events: UnboundedSender<MemoryPressure>,
    ) {
        run_memory(
            session,
            Some(PathBuf::from("/proc/pressure/memory")),
            events,
        )
        .await;
    }
    bus_subscription("enzui-memory-monitor", run)
}

/// Whether power-saver mode is on, at once and on every change; see the
/// module docs.
pub fn power_saver_subscription() -> iced_futures::Subscription<bool> {
    bus_subscription("enzui-power-saver-monitor", run_power_saver)
}
