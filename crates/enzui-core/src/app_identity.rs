//! The application this process is: the id its windows and desktop entry
//! carry, declared once as the application starts.
//!
//! Framework features that follow the application read it from here instead
//! of each asking the application for its id. An application's own theme
//! file is one ([`crate::theme_file::integration`]): an application that
//! declares its id layers `<id>.toml` over the shared theme with no further
//! wiring. `enzui_widgets`' `DesktopApplicationIdentity` declares itself.

use crate::app_data::AppId;
use std::sync::OnceLock;

static DECLARED: OnceLock<AppId> = OnceLock::new();

/// Declare this process's application id. The first valid declaration
/// stands; later ones, and invalid ids, are ignored. Returns whether `id` is
/// the declared application now.
pub fn declare(id: &str) -> bool {
    let Ok(id) = AppId::new(id) else {
        return false;
    };
    DECLARED.get_or_init(|| id.clone()) == &id
}

/// The declared application id, once one is.
#[must_use]
pub fn declared() -> Option<AppId> {
    DECLARED.get().cloned()
}
