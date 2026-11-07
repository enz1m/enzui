//! Parent selection and portal handle encoding for interactive operations.
//!
//! Explicitly unparented requests are allowed, while a requested stale window is refused rather than silently downgraded.

use iced_core::window;

/// An explicit parent policy for an interactive portal request.
///
/// [`Self::None`] permits an unparented request; a stale [`Self::Window`] resolves as unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PortalParent {
    /// Parent the request to this window.
    Window(window::Id),
    /// Request an explicitly unparented operation.
    None,
}

/// The result of checking a requested parent against live windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ParentResolution {
    /// Export a parent handle for this live window.
    Parented(window::Id),
    /// Run the operation without a parent.
    Unparented,
    /// Refuse the operation because the requested window is no longer live.
    Unavailable(window::Id),
}

/// An owned XDG portal `parent_window` value.
///
/// X11 handles use `x11:` plus a hexadecimal XID, Wayland handles use `wayland:` plus an xdg-foreign token, and an empty string means explicitly unparented.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ParentHandle(String);

/// Reports the scheme and token length, never the exported token itself.
///
/// A Wayland handle carries an xdg-foreign token that lets its holder parent
/// windows to the exporting surface, so diagnostics must not repeat it.
impl core::fmt::Debug for ParentHandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.is_none() {
            return f.write_str("ParentHandle(None)");
        }

        match self.0.split_once(':') {
            Some((scheme, token)) => {
                write!(f, "ParentHandle({scheme}:[redacted; {}])", token.len())
            }
            None => write!(f, "ParentHandle([redacted; {}])", self.0.len()),
        }
    }
}

impl ParentHandle {
    /// Encodes an X11 XID as lowercase hexadecimal with the `x11:` prefix.
    #[must_use]
    pub fn x11(xid: u64) -> Self {
        Self(format!("x11:{xid:x}"))
    }

    /// Encodes an xdg-foreign export token with the `wayland:` prefix.
    #[must_use]
    pub fn wayland(handle: &str) -> Self {
        Self(format!("wayland:{handle}"))
    }

    /// Returns the empty handle used for an explicitly unparented portal request.
    #[must_use]
    pub fn none() -> Self {
        Self(String::new())
    }

    /// The handle string for the `parent_window` argument.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this is the empty (unparented) handle.
    #[must_use]
    pub fn is_none(&self) -> bool {
        self.0.is_empty()
    }
}

impl ParentResolution {
    /// Returns whether a requested parent window was stale.
    #[must_use]
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable(_))
    }

    /// The window to parent to, if this resolution parents at all.
    #[must_use]
    pub fn parented_window(&self) -> Option<window::Id> {
        match self {
            Self::Parented(id) => Some(*id),
            _ => None,
        }
    }
}

impl PortalParent {
    /// Resolves this policy against current window liveness.
    ///
    /// Explicit none always resolves unparented; a live window resolves parented, and a stale requested window resolves unavailable.
    #[must_use]
    pub fn resolve(&self, is_live: impl Fn(window::Id) -> bool) -> ParentResolution {
        match self {
            Self::None => ParentResolution::Unparented,
            Self::Window(id) => {
                if is_live(*id) {
                    ParentResolution::Parented(*id)
                } else {
                    ParentResolution::Unavailable(*id)
                }
            }
        }
    }

    /// The window this parent names, if any.
    #[must_use]
    pub fn window(&self) -> Option<window::Id> {
        match self {
            Self::Window(id) => Some(*id),
            Self::None => Option::None,
        }
    }
}
