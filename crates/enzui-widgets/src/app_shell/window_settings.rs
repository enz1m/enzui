//! Window-decoration settings and desktop application identity.

use std::fmt;
use std::str::FromStr;

use super::{ShellLayout, WindowDecorations};

/// A validated desktop-entry basename used as the Wayland
/// [`xdg_toplevel` application ID].
///
/// Use the basename without `.desktop`: an entry installed as
/// `$XDG_DATA_HOME/applications/org.example.Reader.desktop` is identified by
/// `org.example.Reader`. The entry's `Icon=` key remains the icon lookup name;
/// changing a window title does not affect either value.
///
/// Accepted basenames contain ASCII letters, digits, `.`, `-`, and `_`.
/// Reverse-DNS naming is recommended by the [Desktop Entry specification];
/// simple basenames are also accepted. This type neither installs a desktop
/// entry nor registers a D-Bus name. `DBusActivatable=true` requires a valid
/// D-Bus name and an application that implements activation under that name.
/// A Wayland application ID is metadata, not authentication.
///
/// On Linux, these methods set the platform-specific `application_id` field.
/// On other targets, they preserve the supplied settings. The free settings
/// functions leave the default empty application ID unchanged.
///
/// [`xdg_toplevel` application ID]: https://wayland.app/protocols/xdg-shell#xdg_toplevel:request:set_app_id
/// [Desktop Entry specification]: https://specifications.freedesktop.org/desktop-entry-spec/latest/file-naming.html
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DesktopApplicationIdentity(String);

impl DesktopApplicationIdentity {
    /// Validates a desktop-entry basename without its `.desktop` suffix.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty value, a `.desktop` suffix, a NUL byte,
    /// either path separator, or a character outside the portable basename
    /// alphabet.
    ///
    /// The first identity a process makes is the application it declares
    /// itself ([`enzui_core::app_identity`]): its own theme file, among
    /// others, follows from that.
    pub fn new(basename: impl Into<String>) -> Result<Self, DesktopApplicationIdentityError> {
        let basename = basename.into();
        validate_desktop_application_identity(&basename)?;
        let _ = enzui_core::app_identity::declare(&basename);
        Ok(Self(basename))
    }

    /// The identity of `package`, whose desktop entry, metainfo and service
    /// file carry the same id ([`enzui_core::packaging`]).
    #[must_use]
    pub fn for_package(package: &enzui_core::packaging::AppPackage) -> Self {
        // A packaging id is a valid basename: reverse-DNS elements of ASCII
        // letters, digits, `_` and `-`.
        let _ = enzui_core::app_identity::declare(package.id());
        Self(package.id().to_owned())
    }

    /// Returns the desktop-entry basename and Wayland application ID.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Applies this identity without changing any other window setting.
    #[must_use]
    pub fn apply_to_window_settings(
        &self,
        settings: iced::window::Settings,
    ) -> iced::window::Settings {
        apply_platform_identity(settings, self.as_str())
    }

    /// Builds settings for one shell layout with this identity.
    #[must_use]
    pub fn window_settings(&self, layout: ShellLayout) -> iced::window::Settings {
        self.apply_to_window_settings(enz_window_settings(layout))
    }

    /// Builds settings for a shell that can render any of `layouts`.
    ///
    /// # Panics
    ///
    /// If `layouts` is empty, matching [`enz_window_settings_for`].
    #[must_use]
    pub fn window_settings_for(&self, layouts: &[ShellLayout]) -> iced::window::Settings {
        self.apply_to_window_settings(enz_window_settings_for(layouts))
    }

    /// Builds server-decorated secondary-window settings with this identity.
    #[must_use]
    pub fn secondary_window_settings(&self) -> iced::window::Settings {
        self.apply_to_window_settings(enz_secondary_window_settings())
    }

    /// Builds dialog-window settings for `parent` with this identity; see
    /// [`enz_dialog_window_settings`].
    #[must_use]
    pub fn dialog_window_settings(
        &self,
        parent: iced::window::Id,
        modal: bool,
        size: iced::Size,
    ) -> iced::window::Settings {
        self.apply_to_window_settings(enz_dialog_window_settings(parent, modal, size))
    }

    /// Builds undecorated content-window settings with this identity.
    #[must_use]
    pub fn content_window_settings(&self) -> iced::window::Settings {
        self.apply_to_window_settings(enz_content_window_settings())
    }
}

impl AsRef<str> for DesktopApplicationIdentity {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for DesktopApplicationIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for DesktopApplicationIdentity {
    type Err = DesktopApplicationIdentityError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

/// Why a desktop application identity was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DesktopApplicationIdentityError {
    /// The basename is empty.
    Empty,
    /// The caller included the `.desktop` filename suffix.
    DesktopSuffix,
    /// The basename contains a NUL byte.
    NulByte,
    /// The basename contains `/` or `\\` instead of one filename component.
    PathSeparator,
    /// The basename contains a character outside `A-Z`, `a-z`, `0-9`, `.`,
    /// `-`, and `_`.
    InvalidCharacter(char),
}

impl fmt::Display for DesktopApplicationIdentityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("desktop application identity is empty"),
            Self::DesktopSuffix => formatter
                .write_str("desktop application identity must omit the .desktop filename suffix"),
            Self::NulByte => formatter.write_str("desktop application identity contains NUL"),
            Self::PathSeparator => formatter.write_str(
                "desktop application identity must be one basename without path separators",
            ),
            Self::InvalidCharacter(character) => write!(
                formatter,
                "desktop application identity contains unsupported character {character:?}",
            ),
        }
    }
}

impl std::error::Error for DesktopApplicationIdentityError {}

fn validate_desktop_application_identity(
    basename: &str,
) -> Result<(), DesktopApplicationIdentityError> {
    if basename.is_empty() {
        return Err(DesktopApplicationIdentityError::Empty);
    }
    if basename.contains('\0') {
        return Err(DesktopApplicationIdentityError::NulByte);
    }
    if basename.contains('/') || basename.contains('\\') {
        return Err(DesktopApplicationIdentityError::PathSeparator);
    }
    if basename.ends_with(".desktop") {
        return Err(DesktopApplicationIdentityError::DesktopSuffix);
    }
    if let Some(character) = basename.chars().find(|character| {
        !character.is_ascii_alphanumeric() && !matches!(character, '.' | '-' | '_')
    }) {
        return Err(DesktopApplicationIdentityError::InvalidCharacter(character));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply_platform_identity(
    mut settings: iced::window::Settings,
    identity: &str,
) -> iced::window::Settings {
    identity.clone_into(&mut settings.platform_specific.application_id);
    settings
}

#[cfg(not(target_os = "linux"))]
fn apply_platform_identity(
    settings: iced::window::Settings,
    _identity: &str,
) -> iced::window::Settings {
    settings
}

/// Who draws the titlebar of a shell window with a titlebar layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShellFrame {
    /// The shell, on every desktop: its own titlebar, window controls and
    /// resize edges, and no compositor or window manager titlebar.
    #[default]
    Own,
    /// The desktop where it decorates windows (KDE, sway); the shell draws
    /// its own only where the compositor declines (GNOME). The shell then
    /// leaves out its window controls under the desktop's titlebar.
    Desktop,
}

/// The [`ShellFrame`] windows get by default: [`ShellFrame::Own`], or
/// [`ShellFrame::Desktop`] when the environment sets `ENZUI_SSD=1`.
#[must_use]
pub fn default_shell_frame() -> ShellFrame {
    static FRAME: std::sync::OnceLock<ShellFrame> = std::sync::OnceLock::new();
    *FRAME.get_or_init(|| {
        if std::env::var_os("ENZUI_SSD").is_some_and(|value| value == "1") {
            ShellFrame::Desktop
        } else {
            ShellFrame::Own
        }
    })
}

/// Builds initial window settings with decorations derived from the shell layout.
///
/// Every layout draws its own frame, window controls and resize edges, with
/// no compositor or window manager titlebar, on every desktop
/// ([`ShellFrame::Own`], the [`default_shell_frame`]); a titlebar-less layout
/// floats its window controls at the top trailing corner. [`enz_window_settings_with_frame`] with
/// [`ShellFrame::Desktop`] lets the desktop decorate the window where it does
/// (KDE, sway), and the shell draw its own only where it declines (GNOME); the
/// shell reads the outcome from [`iced::window::decorations`].
/// All other fields use `iced::window::Settings::default`. Override individual
/// fields with struct-update syntax, including `size`, which is a request the
/// compositor may ignore.
///
/// ```
/// use enzui_widgets::app_shell::{ShellLayout, enz_window_settings};
///
/// let csd = enz_window_settings(ShellLayout::TitlebarSidebarContent);
/// assert!(!csd.decorations);
/// // Transparent, so its rounded corners show the desktop behind them.
/// assert!(csd.transparent);
///
/// let content_only = enz_window_settings(ShellLayout::ContentOnly);
/// assert!(!content_only.decorations);
/// ```
#[must_use]
pub fn enz_window_settings(layout: ShellLayout) -> iced::window::Settings {
    enz_window_settings_for(&[layout])
}

/// Builds initial window settings for a shell that can use several layouts.
///
/// If any layout uses client-side decorations, the shell draws its own frame
/// whenever the compositor does not (see [`enz_window_settings`]).
/// All other fields use `iced::window::Settings::default`.
/// These settings apply when opening a window; changing layouts does not update
/// an existing window's decorations.
///
/// # Panics
///
/// Panics if `layouts` is empty.
#[must_use]
pub fn enz_window_settings_for(layouts: &[ShellLayout]) -> iced::window::Settings {
    enz_window_settings_with_frame(layouts, default_shell_frame())
}

/// [`enz_window_settings_for`] with an explicit [`ShellFrame`].
///
/// ```
/// use enzui_widgets::app_shell::{ShellFrame, ShellLayout, enz_window_settings_with_frame};
///
/// let desktop =
///     enz_window_settings_with_frame(&[ShellLayout::TitlebarContent], ShellFrame::Desktop);
/// # #[cfg(target_os = "linux")]
/// assert!(desktop.decorations && desktop.platform_specific.app_drawn_decorations);
/// // Transparent, so rounded corners show the desktop behind them.
/// # #[cfg(target_os = "linux")]
/// assert!(desktop.transparent);
/// ```
///
/// # Panics
///
/// Panics if `layouts` is empty.
#[must_use]
pub fn enz_window_settings_with_frame(
    layouts: &[ShellLayout],
    frame: ShellFrame,
) -> iced::window::Settings {
    assert!(
        !layouts.is_empty(),
        "enz_window_settings_for: declare at least one ShellLayout \
         — the window's decoration posture is derived from the layouts the app \
         can actually render",
    );
    let any_draws_own_titlebar = layouts
        .iter()
        .any(|layout| layout.window_decorations() == WindowDecorations::ClientSide);
    let mut settings = iced::window::Settings {
        decorations: !any_draws_own_titlebar,
        // A shell drawing its own frame paints its background inside rounded
        // corners; the corners show the desktop only when the window is
        // transparent, and such a window then clears its frames to transparent.
        transparent: any_draws_own_titlebar,
        ..iced::window::Settings::default()
    };
    #[cfg(target_os = "linux")]
    {
        if any_draws_own_titlebar && frame == ShellFrame::Desktop {
            settings.decorations = true;
            settings.platform_specific.app_drawn_decorations = true;
        }
        // The platform casts it only while the frame really is self-drawn.
        if any_draws_own_titlebar {
            settings.platform_specific.shadow = crate::window_frame_clip::platform_shadow();
        }
    }
    settings
}

/// Builds server-decorated settings for a secondary window without a shell.
///
/// The server frame supplies the window's close, drag, and resize controls.
/// All other fields use `iced::window::Settings::default`, including
/// `exit_on_close_request: true`. Override that field when closing requires
/// application cleanup. Override `size` or use
/// [`enzui_core::window_placement`] to restore placement.
///
/// Use [`enz_window_settings`] for a secondary window that hosts a shell, or
/// [`enz_content_window_settings`] for one that supplies its own controls.
#[must_use]
pub fn enz_secondary_window_settings() -> iced::window::Settings {
    iced::window::Settings {
        decorations: true,
        ..iced::window::Settings::default()
    }
}

/// Builds server-decorated settings for a dialog window of `parent`.
///
/// The desktop keeps the dialog above `parent` and groups the two as one
/// application window (`xdg_toplevel.set_parent` on Wayland,
/// `WM_TRANSIENT_FOR` on X11). A `modal` dialog also blocks its parent: the
/// parent receives no input while the dialog is open and a click on it
/// raises the dialog; on Wayland the compositor is told through
/// `xdg_wm_dialog_v1` where it offers that. Open it with
/// `iced::window::open` from an `iced::daemon` app; closing the dialog closes
/// only the dialog.
#[must_use]
pub fn enz_dialog_window_settings(
    parent: iced::window::Id,
    modal: bool,
    size: iced::Size,
) -> iced::window::Settings {
    iced::window::Settings {
        size,
        parent: Some(parent),
        modal,
        minimizable: false,
        ..enz_secondary_window_settings()
    }
}

/// Builds undecorated settings for a content window with its own window controls.
///
/// Supply a visible close control, a
/// [`titlebar_drag_region`](crate::titlebar_drag_region::titlebar_drag_region),
/// and a [`window_resize_frame`](crate::window_resize_frame::window_resize_frame).
/// Without them, the window has no corresponding client or server affordances.
/// All other fields use `iced::window::Settings::default`; override `size`
/// or use [`enzui_core::window_placement`] to restore placement.
#[must_use]
pub fn enz_content_window_settings() -> iced::window::Settings {
    iced::window::Settings {
        decorations: false,
        ..iced::window::Settings::default()
    }
}
