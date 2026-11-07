//! Synchronous startup errors shared by portal D-Bus backends.
//!
//! [`crate::portal::dbus_backend::DbusBackend`] starts owner and watchdog
//! threads synchronously; later initialization failures are reported as
//! [`crate::portal::backend::BackendError::Unavailable`].
//! [`crate::portal::dbus_backend::session::DbusSessionBackend`] constructs
//! its Tokio runtime on the caller thread and reports runtime creation failures
//! through [`RuntimeInitError`].

use tokio::runtime::Runtime;

/// A portal backend could not acquire the OS resource its constructor needs to
/// finish synchronously.
///
/// The variant of failure depends on the caller:
///
/// * [`crate::portal::dbus_backend::DbusBackend::new`] (nonblocking) raises
///   this error only when `std::thread::Builder::spawn` refuses to create the
///   owner thread or the startup-watchdog thread. Every other startup failure
///   — Tokio runtime creation on the owner thread, bus connection, portal
///   activation/readiness — settles into [`crate::portal::backend::BackendError::Unavailable`]
///   without ever passing through this type.
/// * [`crate::portal::dbus_backend::session::DbusSessionBackend::new`] still
///   builds its Tokio runtime synchronously on the caller thread and raises
///   this error when `Runtime::new()` fails.
///
/// The retained [`std::io::Error`] is the exact `io::Error` that the kernel
/// returned to the failed spawn.
#[derive(Debug)]
pub struct RuntimeInitError(std::io::Error);

impl RuntimeInitError {
    pub(crate) fn new(err: std::io::Error) -> Self {
        Self(err)
    }

    /// The underlying OS error that caused the spawn to be refused.
    ///
    /// This is the `io::Error` returned by either `std::thread::Builder::spawn`
    /// (the `DbusBackend` path) or `tokio::runtime::Runtime::new` (the
    /// `DbusSessionBackend` path); see the type docs.
    #[must_use]
    pub fn io_error(&self) -> &std::io::Error {
        &self.0
    }
}

impl core::fmt::Display for RuntimeInitError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "portal D-Bus backend could not spawn required OS resource \
             (owner/watchdog thread or Tokio runtime): {}",
            self.0
        )
    }
}

impl std::error::Error for RuntimeInitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

pub(crate) fn build_runtime() -> Result<Runtime, RuntimeInitError> {
    #[cfg(test)]
    if let Some(injected) = fail::take_injected() {
        return Err(RuntimeInitError(injected));
    }

    Runtime::new().map_err(RuntimeInitError)
}

#[cfg(test)]
pub(crate) mod fail {

    use std::cell::RefCell;

    thread_local! {
        static INJECTED: RefCell<Option<std::io::Error>> = const { RefCell::new(None) };
    }

    pub(crate) fn take_injected() -> Option<std::io::Error> {
        INJECTED.with(|slot| slot.borrow_mut().take())
    }
}
