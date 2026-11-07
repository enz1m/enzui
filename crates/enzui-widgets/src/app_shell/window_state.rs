//! Window identity and mirrored state for application-owned chrome commands.
//!
//! [`ShellWindowState::new`] binds the first opened window. Multi-window hosts
//! should use [`ShellWindowState::deferred`] or [`ShellWindowState::for_window`]
//! and bind each shell explicitly. Other windows' events and replies are ignored;
//! an unbound state issues no command. Automatic native resize edges do not
//! require this helper. Hosts own close guards and the control messages.

use iced::{Subscription, Task, window};

/// Which runtime window a [`ShellWindowState`] speaks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellWindowIdentity {
    /// No window, and no event may bind one. Either the state was built
    /// with [`ShellWindowState::deferred`] and is waiting for an explicit
    /// [`bind`](ShellWindowState::bind), or its window was closed.
    Unbound,

    /// No window yet; the first `Opened` observed binds it. Single-window
    /// ergonomics — the only window an app opens is its main one.
    AwaitingFirstOpen,

    /// Bound to this exact runtime window. Only events carrying this id are
    /// applied; only this id is commanded.
    Bound(window::Id),
}

impl ShellWindowIdentity {
    /// The bound id, if any.
    #[must_use]
    pub fn id(self) -> Option<window::Id> {
        match self {
            Self::Bound(id) => Some(id),
            _ => None,
        }
    }
}

/// Identity and compositor replies for one window's application-owned commands.
///
/// Mirrored maximize/fullscreen flags start false; the host supplies resizing
/// policy. Native automatic resize edges consult the runtime directly.
#[derive(Debug, Clone)]
pub struct ShellWindowState {
    identity: ShellWindowIdentity,
    maximized: bool,
    fullscreen: bool,
    resizable: bool,
}

/// Events the helper's subscription produces. Every variant carries the window
/// it is about, so the reducer can filter by exact id.
#[derive(Debug, Clone, PartialEq)]
pub enum ShellWindowEvent {
    /// A window was opened. The only event that can bind an identity, and
    /// only for a state awaiting its first open.
    Opened(window::Id),

    /// Resize, scale, focus or decoration/state changes for this window. Never binds; used
    /// only to re-query compositor state for the already-bound window.
    Observed(window::Id),

    /// A window was closed. Unbinds only when it is the bound window.
    Closed(window::Id),

    /// A `window::is_maximized` reply for this window.
    MaximizedChanged {
        /// The window the query was issued for.
        window: window::Id,
        /// The compositor's answer.
        maximized: bool,
    },

    /// A `window::mode` reply for this window.
    ModeChanged {
        /// The window the query was issued for.
        window: window::Id,
        /// The compositor's answer.
        mode: window::Mode,
    },
}

impl ShellWindowEvent {
    /// Classify a runtime window event for `id`.
    ///
    /// `Opened` is the only lifecycle event that may bind an identity;
    /// Resize, scale, focus and decoration/state changes become [`Observed`], which
    /// never binds. Everything else is not window identity news.
    ///
    /// [`Observed`]: ShellWindowEvent::Observed
    #[must_use]
    pub fn from_window_event(event: &window::Event, id: window::Id) -> Option<Self> {
        match event {
            window::Event::Opened { .. } => Some(Self::Opened(id)),
            window::Event::Closed => Some(Self::Closed(id)),
            window::Event::Resized(_)
            | window::Event::Rescaled(_)
            | window::Event::Focused
            | window::Event::DecorationsChanged(_) => Some(Self::Observed(id)),
            _ => None,
        }
    }

    /// The window this event is about.
    #[must_use]
    pub fn window(&self) -> window::Id {
        match *self {
            Self::Opened(id) | Self::Observed(id) | Self::Closed(id) => id,
            Self::MaximizedChanged { window, .. } | Self::ModeChanged { window, .. } => window,
        }
    }
}

impl Default for ShellWindowState {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellWindowState {
    /// Single-window state: the first `Opened` binds the identity.
    #[must_use]
    pub fn new() -> Self {
        Self {
            identity: ShellWindowIdentity::AwaitingFirstOpen,
            maximized: false,
            fullscreen: false,
            resizable: true,
        }
    }

    /// Multi-window state: no event can bind it; the app must call
    /// [`bind`](Self::bind) with the id of the window it opened as main.
    #[must_use]
    pub fn deferred() -> Self {
        Self {
            identity: ShellWindowIdentity::Unbound,
            ..Self::new()
        }
    }

    /// State already bound to a known window id.
    #[must_use]
    pub fn for_window(id: window::Id) -> Self {
        Self {
            identity: ShellWindowIdentity::Bound(id),
            ..Self::new()
        }
    }

    /// Single-window state for a window whose settings say `resizable: false`.
    #[must_use]
    pub fn fixed_size() -> Self {
        Self {
            resizable: false,
            ..Self::new()
        }
    }

    /// Bind this state explicitly to a host-owned window.
    ///
    /// Rebinding preserves mirrored flags until later replies update them;
    /// it does not issue a compositor query.
    pub fn bind(&mut self, id: window::Id) {
        self.identity = ShellWindowIdentity::Bound(id);
    }

    /// Drop the identity. No event can rebind it afterwards; only
    /// [`bind`](Self::bind) can.
    pub fn unbind(&mut self) {
        self.identity = ShellWindowIdentity::Unbound;
        self.maximized = false;
        self.fullscreen = false;
    }

    /// The current identity.
    #[must_use]
    pub fn identity(&self) -> ShellWindowIdentity {
        self.identity
    }

    /// The bound runtime window id, if identity has resolved.
    #[must_use]
    pub fn window_id(&self) -> Option<window::Id> {
        self.identity.id()
    }

    /// Whether an identity has resolved.
    #[must_use]
    pub fn is_bound(&self) -> bool {
        self.window_id().is_some()
    }

    /// Whether this state speaks for `id`.
    #[must_use]
    pub fn targets(&self, id: window::Id) -> bool {
        self.window_id() == Some(id)
    }

    /// Whether the compositor reports the bound window as maximized.
    #[must_use]
    pub fn maximized(&self) -> bool {
        self.maximized
    }

    /// Whether the compositor reports the bound window as fullscreen.
    #[must_use]
    pub fn fullscreen(&self) -> bool {
        self.fullscreen
    }

    /// Host-supplied resizing policy; defaults to true.
    #[must_use]
    pub fn resizable(&self) -> bool {
        self.resizable
    }

    /// Set host resizing policy without issuing a native window command.
    pub fn set_resizable(&mut self, resizable: bool) {
        self.resizable = resizable;
    }

    /// The subscription that observes window lifecycle events. It is global:
    /// every window's events arrive here, tagged with their id, and
    /// [`update`](Self::update) does the filtering.
    pub fn subscription(&self) -> Subscription<ShellWindowEvent> {
        Self::events()
    }

    /// The global window-event subscription, without a state instance.
    pub fn events() -> Subscription<ShellWindowEvent> {
        iced::event::listen_with(|event, _status, id| match event {
            iced::Event::Window(window_event) => {
                ShellWindowEvent::from_window_event(&window_event, id)
            }
            _ => None,
        })
    }

    /// Apply matching window events and issue tagged maximize/fullscreen queries.
    ///
    /// The first open binds an awaiting state. Other windows cause no change or
    /// task. Closing the bound window clears its identity and mirrored flags.
    pub fn update(&mut self, event: ShellWindowEvent) -> Task<ShellWindowEvent> {
        match event {
            ShellWindowEvent::Opened(id) => {
                if self.identity == ShellWindowIdentity::AwaitingFirstOpen {
                    self.identity = ShellWindowIdentity::Bound(id);
                }
                if self.targets(id) {
                    Self::query(id)
                } else {
                    Task::none()
                }
            }
            ShellWindowEvent::Observed(id) => {
                if self.targets(id) {
                    Self::query(id)
                } else {
                    Task::none()
                }
            }
            ShellWindowEvent::Closed(id) => {
                if self.targets(id) {
                    self.unbind();
                }
                Task::none()
            }
            ShellWindowEvent::MaximizedChanged {
                window: target,
                maximized,
            } => {
                if self.targets(target) {
                    self.maximized = maximized;
                }
                Task::none()
            }
            ShellWindowEvent::ModeChanged {
                window: target,
                mode,
            } => {
                if self.targets(target) {
                    self.fullscreen = mode == window::Mode::Fullscreen;
                }
                Task::none()
            }
        }
    }

    fn query(id: window::Id) -> Task<ShellWindowEvent> {
        Task::batch([
            window::is_maximized(id).map(move |maximized| ShellWindowEvent::MaximizedChanged {
                window: id,
                maximized,
            }),
            window::mode(id).map(move |mode| ShellWindowEvent::ModeChanged { window: id, mode }),
        ])
    }

    /// Request native borderless fullscreen for the bound window, or leave it.
    ///
    /// This does not optimistically change [`Self::fullscreen`]. The compositor's
    /// reply owns presentation. Leaving fullscreen preserves the native maximized
    /// or normal restore state; it does not issue an unmaximize or resize request.
    /// Unbound states issue nothing. Observe [`Self::events`] and pass them to
    /// [`Self::update`] to track user/compositor changes, including same-size ones.
    pub fn set_fullscreen<Message: Send + 'static>(&self, fullscreen: bool) -> Task<Message> {
        self.window_id().map_or_else(Task::none, |id| {
            window::set_mode(
                id,
                if fullscreen {
                    window::Mode::Fullscreen
                } else {
                    window::Mode::Windowed
                },
            )
        })
    }

    /// Toggle the bound window using its current native mode, not a mirrored flag.
    ///
    /// Like [`Self::set_fullscreen`], unbound states issue nothing. Keyboard
    /// shortcuts are application policy: route F11 or a player action here, and
    /// route Escape to `set_fullscreen(false)` only when exiting is appropriate.
    pub fn toggle_fullscreen<Message: Send + 'static>(&self) -> Task<Message> {
        self.window_id().map_or_else(Task::none, |id| {
            window::mode(id).then(move |mode| {
                window::set_mode(
                    id,
                    if mode == window::Mode::Fullscreen {
                        window::Mode::Windowed
                    } else {
                        window::Mode::Fullscreen
                    },
                )
            })
        })
    }

    /// Execute a [`WindowCommand`](crate::title_bar_gesture::WindowCommand)
    /// against the bound window. While unbound this is `Task::none()` — an
    /// unknown identity commands nothing rather than guessing a window.
    pub fn run_command<Message: Send + 'static>(
        &self,
        command: crate::title_bar_gesture::WindowCommand,
    ) -> Task<Message> {
        self.window_id()
            .map_or_else(Task::none, |id| command.into_task(id))
    }
}
