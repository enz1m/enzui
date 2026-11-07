//! Tell the compositor what a window shows: its content type and whether its
//! frames may tear.
//!
//! - [`content_type`]: `Video` or `Game` lets the compositor scan the window
//!   out directly and keep timing steady; `Photo` favours colour accuracy.
//! - [`tearing`]: a game that presents without vsync (`iced::Settings::vsync
//!   = false`) lets frames reach the screen mid-refresh, for the lowest
//!   latency.
//!
//! Each resolves to `HintError::Unsupported` where the compositor lacks the
//! protocol (`wp_content_type_v1`, `wp_tearing_control_v1`), and applies from
//! the window's next frame. The colour space needs no call: the renderer
//! draws sRGB and its Vulkan swapchain describes that to the compositor
//! (`wp_color_management_v1`) through Mesa's WSI.

use iced::Task;

#[cfg(target_os = "linux")]
pub use enzui_wayland::surface_hints::{ContentType, HintError};

#[cfg(target_os = "linux")]
use enzui_wayland::surface_hints;

/// How long a hint waits for the compositor.
#[cfg(target_os = "linux")]
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Sets what `window` shows.
#[cfg(target_os = "linux")]
pub fn content_type(window: iced::window::Id, content: ContentType) -> Task<Result<(), HintError>> {
    apply(window, move |payload| {
        surface_hints::on_window(payload, |connection, surface| {
            surface_hints::set_content_type(connection, surface, content, TIMEOUT)
        })
    })
}

/// Lets `window`'s frames tear (`true`) or asks for vsync again.
#[cfg(target_os = "linux")]
pub fn tearing(window: iced::window::Id, allowed: bool) -> Task<Result<(), HintError>> {
    apply(window, move |payload| {
        surface_hints::on_window(payload, |connection, surface| {
            surface_hints::set_tearing(connection, surface, allowed, TIMEOUT)
        })
    })
}

/// Runs `hint` on the window's own connection and surface, off the UI thread:
/// it waits for the compositor's answer.
#[cfg(target_os = "linux")]
fn apply(
    window: iced::window::Id,
    hint: impl FnOnce(Option<Box<dyn std::any::Any + Send>>) -> Result<(), HintError> + Send + 'static,
) -> Task<Result<(), HintError>> {
    let mut hint = Some(hint);
    iced::window::wayland_parent_export(window).then(move |payload| {
        let payload = std::sync::Mutex::new(hint.take().map(|hint| (payload, hint)));
        Task::future(async move {
            let Some((payload, hint)) = payload.lock().ok().and_then(|mut slot| slot.take()) else {
                return Err(HintError::Disconnected);
            };
            let (sender, receiver) = iced::futures::channel::oneshot::channel();
            let _ = std::thread::Builder::new()
                .name("enzui-surface-hint".into())
                .spawn(move || {
                    let _ = sender.send(hint(payload));
                });
            receiver.await.unwrap_or(Err(HintError::Disconnected))
        })
    })
}

/// Why a hint was not applied.
#[cfg(not(target_os = "linux"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HintError {
    /// Not a Wayland window.
    NotWayland,
}
