//! Take a screenshot or pick a colour from the screen:
//! `org.freedesktop.portal.Screenshot`.
//!
//! Both go through the desktop, which shows its own capture or picker UI, so
//! they work under Wayland and inside a sandbox. [`take_screenshot`] resolves
//! to the URI of the image file the desktop wrote; [`pick_color`] to the
//! colour under the pointer when the user clicked. Each future runs its
//! D-Bus call on a thread of its own, so it can go straight into
//! `Task::perform`; [`take_screenshot_on`] and [`pick_color_on`] are the same
//! calls on a connection the caller owns.

use super::parent::ParentHandle;
use ashpd::WindowIdentifier;
use ashpd::desktop::screenshot::Screenshot;
use std::future::Future;

/// How a portal request ended.
#[derive(Debug, Clone, PartialEq)]
pub enum PortalOutcome<T> {
    /// The desktop answered.
    Done(T),
    /// The user dismissed the desktop's dialog.
    Cancelled,
    /// The desktop has no such portal, or the call failed; the text says why.
    Unavailable(String),
}

/// A screenshot request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenshotRequest {
    interactive: bool,
    modal: bool,
    parent: ParentHandle,
}

impl Default for ScreenshotRequest {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenshotRequest {
    /// The whole screen, at once, with no dialog.
    #[must_use]
    pub fn new() -> Self {
        Self {
            interactive: false,
            modal: true,
            parent: ParentHandle::none(),
        }
    }

    /// Lets the user choose a window or an area first.
    #[must_use]
    pub const fn interactive(mut self, interactive: bool) -> Self {
        self.interactive = interactive;
        self
    }

    /// Whether the desktop's dialog blocks the parent window (the default).
    #[must_use]
    pub const fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    /// Puts the desktop's dialog over this window.
    #[must_use]
    pub fn parent(mut self, parent: ParentHandle) -> Self {
        self.parent = parent;
        self
    }
}

/// The portal's form of a parent handle, or `None` for no parent.
pub(crate) fn window_identifier(parent: &ParentHandle) -> Option<WindowIdentifier> {
    match parent.as_str().split_once(':')? {
        ("x11", xid) => std::os::raw::c_ulong::from_str_radix(xid, 16)
            .ok()
            .map(WindowIdentifier::from_xid),
        ("wayland", handle) => Some(WindowIdentifier::from_xdg_foreign_exported(
            handle.to_owned(),
        )),
        _ => None,
    }
}

pub(crate) fn outcome<T>(result: Result<T, ashpd::Error>) -> PortalOutcome<T> {
    match result {
        Ok(value) => PortalOutcome::Done(value),
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => {
            PortalOutcome::Cancelled
        }
        Err(error) => PortalOutcome::Unavailable(error.to_string()),
    }
}

/// Takes a screenshot on `connection`; the outcome holds the image's URI.
pub async fn take_screenshot_on(
    connection: zbus::Connection,
    request: ScreenshotRequest,
) -> PortalOutcome<String> {
    let parent = window_identifier(&request.parent);
    outcome(
        async {
            Screenshot::request()
                .connection(Some(connection))
                .identifier(parent)
                .interactive(request.interactive)
                .modal(request.modal)
                .send()
                .await?
                .response()
        }
        .await
        .map(|shot| shot.uri().to_string()),
    )
}

/// Asks the user to pick a colour on `connection`.
pub async fn pick_color_on(
    connection: zbus::Connection,
    parent: ParentHandle,
) -> PortalOutcome<iced_core::Color> {
    let parent = window_identifier(&parent);
    outcome(
        async {
            ashpd::desktop::Color::pick()
                .connection(Some(connection))
                .identifier(parent)
                .send()
                .await?
                .response()
        }
        .await
        .map(|color| {
            #[allow(
                clippy::cast_possible_truncation,
                reason = "colour channels are 0..1 and f32 holds them"
            )]
            let channel = |value: f64| value.clamp(0.0, 1.0) as f32;
            iced_core::Color::from_rgb(
                channel(color.red()),
                channel(color.green()),
                channel(color.blue()),
            )
        }),
    )
}

/// Takes a screenshot on the session bus.
pub fn take_screenshot(
    request: ScreenshotRequest,
) -> impl Future<Output = PortalOutcome<String>> + Send + 'static {
    on_session_bus("enzui-screenshot", move |connection| {
        take_screenshot_on(connection, request)
    })
}

/// Picks a colour on the session bus.
pub fn pick_color(
    parent: ParentHandle,
) -> impl Future<Output = PortalOutcome<iced_core::Color>> + Send + 'static {
    on_session_bus("enzui-pick-color", move |connection| {
        pick_color_on(connection, parent)
    })
}

/// Runs `call` on a thread of its own with a session-bus connection.
pub(crate) fn on_session_bus<T, F>(
    name: &str,
    call: impl FnOnce(zbus::Connection) -> F + Send + 'static,
) -> impl Future<Output = PortalOutcome<T>> + Send + 'static
where
    T: Send + 'static,
    F: Future<Output = PortalOutcome<T>>,
{
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let result = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime.block_on(async move {
                    match zbus::Connection::session().await {
                        Ok(connection) => call(connection).await,
                        Err(error) => PortalOutcome::Unavailable(error.to_string()),
                    }
                }),
                Err(error) => PortalOutcome::Unavailable(error.to_string()),
            };
            let _ = sender.send(result);
        });
    async move {
        if let Err(error) = spawned {
            return PortalOutcome::Unavailable(error.to_string());
        }
        receiver
            .await
            .unwrap_or_else(|_| PortalOutcome::Unavailable("the portal thread ended".into()))
    }
}
