//! Keep running without windows: the desktop's `org.freedesktop.portal.Background`.
//!
//! An app that keeps working after its last window closes (syncing mail,
//! finishing a transfer) asks once with [`BackgroundPortal::request`], and
//! keeps a short status line current with [`BackgroundPortal::set_status`]
//! ("Syncing 3 files"). Desktops that implement the portal list the app with
//! that status and a stop control: GNOME Shell's background apps and enzimOS
//! both read it. Start-at-login is requested through the same call
//! ([`BackgroundRequest::autostart`]), never written by the app itself, so the
//! desktop can list and disable it.
//!
//! The D-Bus work runs on a thread started on first use, so an app that never
//! goes into the background never connects, and no call blocks the UI.

use ashpd::desktop::background::{BackgroundProxy, BackgroundRequestOptions, SetStatusOptions};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

/// The longest status the portal accepts, in characters.
pub const MAX_STATUS_CHARS: usize = 96;

/// What the app asks the desktop for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundRequest {
    reason: String,
    autostart: bool,
    command: Vec<String>,
    dbus_activatable: bool,
}

impl BackgroundRequest {
    /// Asks to keep running in the background, telling the user `reason`.
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            autostart: false,
            command: Vec::new(),
            dbus_activatable: false,
        }
    }

    /// Also asks to start at login with `command` (the executable and its
    /// arguments). The desktop writes and owns the autostart entry.
    #[must_use]
    pub fn autostart(mut self, command: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.autostart = true;
        self.command = command.into_iter().map(Into::into).collect();
        self
    }

    /// Says the app is D-Bus activatable, so autostart can activate it.
    #[must_use]
    pub fn dbus_activatable(mut self, activatable: bool) -> Self {
        self.dbus_activatable = activatable;
        self
    }
}

/// Where the app's background request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundState {
    /// Not asked yet.
    NotRequested,
    /// Asked; the desktop has not answered.
    Requested,
    /// The desktop lets the app run in the background; `autostart` says
    /// whether it also starts it at login.
    Granted {
        /// Whether the app will start at login.
        autostart: bool,
    },
    /// The user or the desktop said no.
    Denied,
    /// The desktop has no background portal.
    Unavailable,
}

enum Command {
    Request(BackgroundRequest),
    Status(String),
}

/// The app's background permission and status line.
#[derive(Debug)]
pub struct BackgroundPortal {
    address: Option<String>,
    owner: Option<Sender<Command>>,
    requested: bool,
    status: Option<String>,
    state: Arc<Mutex<BackgroundState>>,
}

impl Default for BackgroundPortal {
    fn default() -> Self {
        Self::new()
    }
}

impl BackgroundPortal {
    /// Uses the session bus. Connects nothing until the first call.
    #[must_use]
    pub fn new() -> Self {
        Self::with_address(None)
    }

    /// Uses the bus at `address`, or the session bus for `None`: for a
    /// qualified proxy or an isolated test.
    #[must_use]
    pub fn with_address(address: Option<String>) -> Self {
        Self {
            address,
            owner: None,
            requested: false,
            status: None,
            state: Arc::new(Mutex::new(BackgroundState::NotRequested)),
        }
    }

    /// Asks the desktop to let the app run without windows. Only the first
    /// call reaches the desktop: ask when the app first goes windowless.
    pub fn request(&mut self, request: BackgroundRequest) {
        if self.requested {
            return;
        }
        self.requested = true;
        self.put(BackgroundState::Requested);
        if !self.send(Command::Request(request)) {
            self.put(BackgroundState::Unavailable);
        }
    }

    /// Shows `status` beside the app in the desktop's background list, or
    /// clears it with `None`. Only a change reaches the desktop; a status
    /// longer than [`MAX_STATUS_CHARS`] is shortened.
    pub fn set_status(&mut self, status: Option<&str>) {
        let status: String = status
            .unwrap_or("")
            .chars()
            .take(MAX_STATUS_CHARS)
            .collect();
        if self.status.as_deref() == Some(status.as_str()) {
            return;
        }
        self.status = Some(status.clone());
        let _ = self.send(Command::Status(status));
    }

    /// Where the request stands.
    #[must_use]
    pub fn state(&self) -> BackgroundState {
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn put(&self, state: BackgroundState) {
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
    }

    fn send(&mut self, command: Command) -> bool {
        if self.owner.is_none() {
            let (owner, commands) = std::sync::mpsc::channel();
            let address = self.address.clone();
            let state = Arc::clone(&self.state);
            let spawned = std::thread::Builder::new()
                .name("enzui-background".into())
                .spawn(move || run(address, &commands, &state))
                .is_ok();
            if !spawned {
                return false;
            }
            self.owner = Some(owner);
        }
        self.owner
            .as_ref()
            .is_some_and(|owner| owner.send(command).is_ok())
    }
}

fn run(address: Option<String>, commands: &Receiver<Command>, state: &Mutex<BackgroundState>) {
    let put = |next: BackgroundState| {
        *state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
    };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        put(BackgroundState::Unavailable);
        return;
    };
    let _entered = runtime.enter();
    let proxy = runtime.block_on(async {
        let connection = match &address {
            Some(address) => {
                ashpd::zbus::connection::Builder::address(address.as_str())?
                    .build()
                    .await?
            }
            None => ashpd::zbus::Connection::session().await?,
        };
        BackgroundProxy::with_connection(connection)
            .await
            .map_err(|error| ashpd::zbus::Error::Failure(error.to_string()))
    });
    let Ok(proxy) = proxy else {
        put(BackgroundState::Unavailable);
        return;
    };
    while let Ok(command) = commands.recv() {
        match command {
            Command::Request(request) => {
                let mut options = BackgroundRequestOptions::default()
                    .set_reason(request.reason.as_str())
                    .set_auto_start(request.autostart)
                    .set_dbus_activatable(request.dbus_activatable);
                if !request.command.is_empty() {
                    options = options.set_command(request.command.iter().map(String::as_str));
                }
                let answer = runtime
                    .block_on(async { proxy.request_background(None, options).await?.response() });
                put(match answer {
                    Ok(granted) if granted.run_in_background() => BackgroundState::Granted {
                        autostart: granted.auto_start(),
                    },
                    Ok(_) | Err(ashpd::Error::Response(_)) => BackgroundState::Denied,
                    Err(_) => BackgroundState::Unavailable,
                });
            }
            Command::Status(message) => {
                let _ = runtime
                    .block_on(proxy.set_status(SetStatusOptions::default().set_message(&message)));
            }
        }
    }
}
