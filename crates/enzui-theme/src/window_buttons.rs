//! Which window controls a window that draws its own frame shows.
//!
//! The theme file's `window.buttons` comes first. A theme that leaves it at
//! [`WindowButtons::Desktop`] follows the desktop's button layout (GNOME's
//! `button-layout`, GTK's `gtk-decoration-layout`), and where the desktop
//! publishes none, only Close shows. Close always shows: a window that draws
//! its own frame must stay closable.

/// The corner radius of a window that draws its own frame, in logical px,
/// unless the theme file's `window.radius` says otherwise.
///
/// Concentric with the close control: a 28 px control sits 8 px from the top
/// and trailing edges of the 44 px titlebar, so its circle's centre is 22 px
/// in from the corner, and a 22 px corner keeps the same 8 px gap all round.
pub const DEFAULT_WINDOW_RADIUS: f32 = 22.0;

/// The window controls a theme asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowButtons {
    /// Follow the desktop's button layout; Close alone without one.
    #[default]
    Desktop,
    /// Close only.
    Close,
    /// Minimize and Close.
    MinimizeClose,
    /// Maximize and Close.
    MaximizeClose,
    /// Minimize, Maximize and Close.
    All,
}

impl WindowButtons {
    /// The theme-file identifiers, in variant order.
    pub const NAMES: &[&str] = &[
        "desktop",
        "close",
        "minimize_close",
        "maximize_close",
        "all",
    ];

    /// The variant a theme-file identifier names.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "desktop" => Self::Desktop,
            "close" => Self::Close,
            "minimize_close" => Self::MinimizeClose,
            "maximize_close" => Self::MaximizeClose,
            "all" => Self::All,
            _ => return None,
        })
    }

    /// The theme-file identifier.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Close => "close",
            Self::MinimizeClose => "minimize_close",
            Self::MaximizeClose => "maximize_close",
            Self::All => "all",
        }
    }

    /// Whether Minimize shows, or `None` when the desktop decides.
    #[must_use]
    pub fn minimize(self) -> Option<bool> {
        match self {
            Self::Desktop => None,
            Self::MinimizeClose | Self::All => Some(true),
            Self::Close | Self::MaximizeClose => Some(false),
        }
    }

    /// Whether Maximize shows, or `None` when the desktop decides.
    #[must_use]
    pub fn maximize(self) -> Option<bool> {
        match self {
            Self::Desktop => None,
            Self::MaximizeClose | Self::All => Some(true),
            Self::Close | Self::MinimizeClose => Some(false),
        }
    }
}
