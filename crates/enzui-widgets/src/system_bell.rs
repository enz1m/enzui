//! The system bell: the desktop's own "that did nothing" alert.
//!
//! [`ring`] asks the compositor to ring it for a window (`xdg_system_bell_v1`),
//! played as the user set it up: a sound, a flash of the window, or nothing.
//! It resolves to [`BellError::Unsupported`] where the compositor has no bell;
//! an app that must be heard then plays the sound theme's `bell` event
//! (`enzui_services::event_sound::play("bell")`).

use iced::Task;

#[cfg(target_os = "linux")]
pub use enzui_wayland::system_bell::BellError;

/// Why the bell did not ring.
#[cfg(not(target_os = "linux"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BellError {
    /// Not a Wayland window.
    NotWayland,
}

/// Rings the system bell for `window`.
#[cfg(target_os = "linux")]
pub fn ring(window: iced::window::Id) -> Task<Result<(), BellError>> {
    iced::window::wayland_parent_export(window).then(|payload| {
        let payload = std::sync::Mutex::new(payload);
        Task::future(async move {
            let payload = payload.lock().ok().and_then(|mut slot| slot.take());
            let (sender, receiver) = iced::futures::channel::oneshot::channel();
            // The first ring on a connection waits for a registry round trip.
            let _ = std::thread::Builder::new()
                .name("enzui-system-bell".into())
                .spawn(move || {
                    let rung = enzui_wayland::system_bell::ring_from_payload(
                        payload,
                        std::time::Duration::from_secs(2),
                    );
                    let _ = sender.send(rung);
                });
            receiver.await.unwrap_or(Err(BellError::Disconnected))
        })
    })
}

/// Rings the system bell for `window`: off Linux there is none.
#[cfg(not(target_os = "linux"))]
pub fn ring(_window: iced::window::Id) -> Task<Result<(), BellError>> {
    Task::done(Err(BellError::NotWayland))
}
