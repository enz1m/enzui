//! Hold off idle, suspend, logout or user switching: the desktop's
//! `org.freedesktop.portal.Inhibit`.
//!
//! A video player must stop the screen from dimming and locking while it
//! plays; an editor with unsaved work asks the desktop to hold off logout and
//! suspend until it is saved; a long copy holds off suspend. [`Inhibitor`]
//! owns one such request for the [`InhibitKinds`] it was made with: the app
//! states what it wants on every update with [`Inhibitor::set`] (unsaved
//! work or not), and the inhibitor turns only the edges into portal calls.
//! The desktop shows the reason when the user tries to log out or suspend.
//!
//! [`crate::portal::session_monitor`] tells the app when the session is about
//! to end, so it can inhibit in time.
//!
//! The portal holds an inhibition as long as its request object lives; the
//! inhibitor closes it on release and on drop. The D-Bus work happens on a
//! thread the inhibitor starts on first use, so an app that never inhibits
//! never connects, and `set` never blocks the UI.

use ashpd::desktop::inhibit::{InhibitFlags, InhibitOptions, InhibitProxy};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

/// Where an [`IdleInhibitor`]'s inhibition stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InhibitState {
    /// Not inhibiting.
    Released,
    /// Asked for; the portal has not answered yet.
    Requested,
    /// The desktop holds the inhibition.
    Active,
    /// The desktop has no inhibit portal, or refused. The screen may sleep;
    /// there is nothing more the app can do about it.
    Unavailable,
}

/// What an [`Inhibitor`] holds off, combined with `|`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct InhibitKinds(u8);

impl InhibitKinds {
    /// Logging out (and shutting down from the session).
    pub const LOGOUT: Self = Self(1);
    /// Switching to another user.
    pub const USER_SWITCH: Self = Self(2);
    /// Suspending.
    pub const SUSPEND: Self = Self(4);
    /// Going idle: dimming, locking, the screensaver.
    pub const IDLE: Self = Self(8);
    /// Everything an app with unsaved work holds off: logout, user switching
    /// and suspend.
    pub const UNSAVED_WORK: Self = Self(1 | 2 | 4);

    /// Whether `other`'s kinds are all included.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether no kind is included.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    fn flags(self) -> ashpd::enumflags2::BitFlags<InhibitFlags> {
        let mut flags = ashpd::enumflags2::BitFlags::empty();
        for (kind, flag) in [
            (Self::LOGOUT, InhibitFlags::Logout),
            (Self::USER_SWITCH, InhibitFlags::UserSwitch),
            (Self::SUSPEND, InhibitFlags::Suspend),
            (Self::IDLE, InhibitFlags::Idle),
        ] {
            if self.contains(kind) {
                flags |= flag;
            }
        }
        flags
    }
}

impl std::ops::BitOr for InhibitKinds {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// Holds off what its [`InhibitKinds`] name while the app says it should.
#[derive(Debug)]
pub struct Inhibitor {
    kinds: InhibitKinds,
    reason: String,
    wanted: bool,
    owner: Option<Sender<bool>>,
    state: Arc<Mutex<InhibitState>>,
    address: Option<String>,
}

/// The inhibitor a media player or presentation uses: idle only.
pub type IdleInhibitor = Inhibitor;

impl Inhibitor {
    /// An inhibitor of `kinds` that says `reason` ("Unsaved changes in
    /// report.odt") when the desktop lists what holds it off. Starts
    /// released; connects nothing.
    #[must_use]
    pub fn new(kinds: InhibitKinds, reason: impl Into<String>) -> Self {
        Self {
            kinds,
            reason: reason.into(),
            wanted: false,
            owner: None,
            state: Arc::new(Mutex::new(InhibitState::Released)),
            address: None,
        }
    }

    /// An idle inhibitor, for playback and presentations.
    #[must_use]
    pub fn idle(reason: impl Into<String>) -> Self {
        Self::new(InhibitKinds::IDLE, reason)
    }

    /// Use the D-Bus bus at `address` instead of the session bus (tests,
    /// nested sessions).
    #[must_use]
    pub fn on_bus(mut self, address: impl Into<String>) -> Self {
        self.address = Some(address.into());
        self
    }

    /// What this inhibitor holds off.
    #[must_use]
    pub fn kinds(&self) -> InhibitKinds {
        self.kinds
    }

    /// Inhibit while `inhibit` is true. Call it freely: only a change
    /// reaches the desktop.
    pub fn set(&mut self, inhibit: bool) {
        if inhibit == self.wanted {
            return;
        }
        self.wanted = inhibit;
        if self.owner.is_none() {
            if !inhibit {
                return;
            }
            self.owner = self.spawn();
        }
        let sent = self
            .owner
            .as_ref()
            .is_some_and(|owner| owner.send(inhibit).is_ok());
        if !sent {
            self.owner = None;
            self.put(InhibitState::Unavailable);
        } else if inhibit {
            self.put(InhibitState::Requested);
        }
    }

    /// Whether the app currently asks for the screen to stay awake.
    #[must_use]
    pub fn is_wanted(&self) -> bool {
        self.wanted
    }

    /// Where the inhibition stands.
    #[must_use]
    pub fn state(&self) -> InhibitState {
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn put(&self, state: InhibitState) {
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
    }

    fn spawn(&self) -> Option<Sender<bool>> {
        let (owner, wants) = std::sync::mpsc::channel();
        let reason = self.reason.clone();
        let state = Arc::clone(&self.state);
        let kinds = self.kinds;
        let address = self.address.clone();
        std::thread::Builder::new()
            .name("enzui-inhibit".into())
            .spawn(move || run(kinds, &reason, address.as_deref(), &wants, &state))
            .ok()?;
        Some(owner)
    }
}

impl Drop for Inhibitor {
    fn drop(&mut self) {
        // Dropping the sender ends the owner, which closes a held request.
        if let Some(owner) = self.owner.take() {
            let _ = owner.send(false);
        }
    }
}

fn run(
    kinds: InhibitKinds,
    reason: &str,
    address: Option<&str>,
    wants: &Receiver<bool>,
    state: &Mutex<InhibitState>,
) {
    let put = |next: InhibitState| {
        *state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
    };
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        put(InhibitState::Unavailable);
        return;
    };
    // zbus spawns onto the ambient runtime outside `block_on` too (a request
    // dropped here), so the whole thread runs inside it.
    let _entered = runtime.enter();
    let proxy = runtime.block_on(async {
        let connection = match address {
            Some(address) => zbus::connection::Builder::address(address)?.build().await?,
            None => zbus::Connection::session().await?,
        };
        InhibitProxy::with_connection(connection)
            .await
            .map_err(|error| zbus::Error::Failure(error.to_string()))
    });
    let Ok(proxy) = proxy else {
        put(InhibitState::Unavailable);
        return;
    };
    let mut held = None;
    while let Ok(mut want) = wants.recv() {
        // Only the newest wish matters after a burst of toggles.
        while let Ok(newer) = wants.try_recv() {
            want = newer;
        }
        match (want, held.take()) {
            (true, None) => {
                let request = runtime.block_on(proxy.inhibit(
                    None,
                    kinds.flags(),
                    InhibitOptions::default().set_reason(reason),
                ));
                match request {
                    Ok(request) => {
                        held = Some(request);
                        put(InhibitState::Active);
                    }
                    Err(_) => put(InhibitState::Unavailable),
                }
            }
            (false, Some(request)) => {
                let _ = runtime.block_on(request.close());
                put(InhibitState::Released);
            }
            (true, Some(request)) => held = Some(request),
            (false, None) => put(InhibitState::Released),
        }
    }
    if let Some(request) = held {
        let _ = runtime.block_on(request.close());
    }
}
