//! Desktop settings that change how widgets behave, read from the desktop.
//!
//! [`DesktopSettings`] holds the values a desktop publishes for its own
//! toolkit and that enzUI widgets follow too. These sources feed it:
//!
//! - GTK's `settings.ini` (`gtk-4.0`, then `gtk-3.0`, under the XDG config
//!   directories), then KDE's `kdeglobals`, read once at startup with no bus;
//! - with the `desktop-settings-portal` feature (on through enzui-widgets'
//!   defaults, and part of `portal-dbus`), the freedesktop Settings portal
//!   (`org.gnome.desktop.*` keys on GNOME, `org.kde.kdeglobals.KDE` on KDE),
//!   read at startup and then followed through its `SettingChanged` signal.
//!   Portal values win.
//!
//! [`spawn_watcher`] runs both on a background thread and [`apply`]s every
//! change; `enzui_widgets::app_setup::run` starts it, so applications need no
//! code. Nothing polls: without a portal, file values are read once.
//!
//! | setting | GTK `settings.ini` | Settings portal | effect |
//! |---|---|---|---|
//! | middle-click paste | `gtk-enable-primary-paste` | `org.gnome.desktop.interface` `gtk-enable-primary-paste` | [`iced_core::clipboard::set_primary_paste_enabled`] |
//! | accent colour | - | `org.freedesktop.appearance` `accent-color` | the theme's accent, below an explicit `palette.accent` in the theme file |
//! | text size | - | `org.gnome.desktop.interface` `text-scaling-factor` | the theme's platform text scale, below the theme file and an in-app choice |
//! | window buttons | `gtk-decoration-layout` | `org.gnome.desktop.wm.preferences` `button-layout` | [`button_layout`]: window control order and side |
//! | scrollbars | `gtk-overlay-scrolling` | `org.gnome.desktop.interface` `overlay-scrolling` | [`overlay_scrollbars`]: hide scrollbars until used, or always show them |
//! | contrast | `gtk-theme-name` `HighContrast*` | `org.freedesktop.appearance` `contrast`, then `org.gnome.desktop.a11y.interface` `high-contrast` | [`DesktopSettings::high_contrast`]: the high-contrast palette and opaque materials |
//! | clock | - | `org.gnome.desktop.interface` `clock-format` | [`clock_24h`]: the hour cycle time formatting and time fields use |
//! | double click | `gtk-double-click-time`, `gtk-double-click-distance`; KDE `DoubleClickInterval` | `org.gnome.desktop.peripherals.mouse` `double-click` | [`input`]: iced click classification and [`crate::click`] |
//! | drag threshold | `gtk-dnd-drag-threshold`; KDE `StartDragDist` | `org.gnome.desktop.peripherals.mouse` `drag-threshold` | [`input`]: drag sources, marquees, pane drags |
//! | caret blink | `gtk-cursor-blink`, `-time`, `-timeout`; KDE `CursorBlinkRate` | `org.gnome.desktop.interface` `cursor-blink`, `-time`, `-timeout` | [`input`]: text inputs and editors; reduced motion also stops it |
//! | caret width | `gtk-cursor-aspect-ratio` (at a 20 px line) | - | [`input`]: text inputs and editors |
//! | glyph hinting | `gtk-xft-hinting`, `gtk-xft-hintstyle`; KDE `XftHintStyle` | `org.gnome.desktop.interface` `font-hinting` | [`input`]: the renderer's glyph rasterizer, else fontconfig's setting |
//! | antialiasing | `gtk-xft-antialias`; KDE `XftAntialias` | `org.gnome.desktop.interface` `font-antialiasing` | [`input`]: grayscale or none (subpixel draws as grayscale) |
//!
//! Theme values (accent, text size) reach the theme through
//! [`subscription`], which `ThemeIntegration` follows; the others apply
//! process-wide and widgets read them while the view is built. The input
//! values live in `iced_core::preferences` ([`input`]), shared with iced's
//! own widgets; `ThemeIntegration` adds the resolved reduced motion.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError, RwLock};
use std::time::Duration;

/// The settings a desktop publishes; `None` means it did not say.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DesktopSettings {
    /// Whether a middle click pastes the primary selection.
    pub primary_paste: Option<bool>,
    /// The user's accent colour.
    pub accent: Option<iced_core::Color>,
    /// The text size factor ("Large Text" is 1.25).
    pub text_scale: Option<f32>,
    /// Which window buttons go where.
    pub button_layout: Option<ButtonLayout>,
    /// Scrollbars hide until used (`true`) or always show (`false`).
    pub overlay_scrolling: Option<bool>,
    /// The clock shows 24 hours (`true`) or 12 with AM/PM (`false`).
    pub clock_24h: Option<bool>,
    /// The longest time between the presses of a double click.
    pub double_click_time: Option<Duration>,
    /// How far the pointer may move between the presses of a double click.
    pub double_click_distance: Option<f32>,
    /// How far the pointer moves with a button held before a drag starts.
    pub drag_threshold: Option<f32>,
    /// Whether the text caret blinks.
    pub cursor_blink: Option<bool>,
    /// One full caret blink cycle.
    pub cursor_blink_time: Option<Duration>,
    /// How long after the last caret activity the caret stops blinking.
    pub cursor_blink_timeout: Option<Duration>,
    /// The text caret's width in logical pixels.
    pub caret_width: Option<f32>,
    /// Whether glyphs are hinted.
    pub font_hinting: Option<bool>,
    /// Whether glyphs are antialiased.
    pub font_antialias: Option<bool>,
    /// The portal's standardised contrast preference: more contrast or none.
    pub contrast: Option<bool>,
    /// The desktop's own high-contrast switch (GNOME's `high-contrast`, a
    /// GTK `HighContrast` theme), below [`Self::contrast`].
    pub desktop_high_contrast: Option<bool>,
}

/// A window button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowButton {
    /// Minimize.
    Minimize,
    /// Maximize and restore.
    Maximize,
    /// Close.
    Close,
}

/// Which window buttons sit at the start and the end of a title bar, in
/// order, as GTK's `gtk-decoration-layout` / GNOME's `button-layout` say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ButtonLayout {
    /// The buttons before the title, from the left.
    pub leading: Vec<WindowButton>,
    /// The buttons after the title, from the left.
    pub trailing: Vec<WindowButton>,
}

impl Default for ButtonLayout {
    /// Minimize, maximize and close at the end, as GTK ships.
    fn default() -> Self {
        Self {
            leading: Vec::new(),
            trailing: vec![
                WindowButton::Minimize,
                WindowButton::Maximize,
                WindowButton::Close,
            ],
        }
    }
}

impl ButtonLayout {
    /// Parses `"icon:minimize,maximize,close"`: buttons before the colon go
    /// before the title. Unknown names (`appmenu`, `icon`, `spacer`) are
    /// skipped; a value with no colon puts everything at the end.
    #[must_use]
    pub fn parse(value: &str) -> Self {
        let buttons = |part: &str| {
            part.split(',')
                .filter_map(|name| match name.trim() {
                    "minimize" => Some(WindowButton::Minimize),
                    "maximize" => Some(WindowButton::Maximize),
                    "close" => Some(WindowButton::Close),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        match value.split_once(':') {
            Some((leading, trailing)) => Self {
                leading: buttons(leading),
                trailing: buttons(trailing),
            },
            None => Self {
                leading: Vec::new(),
                trailing: buttons(value),
            },
        }
    }

    /// Whether the buttons sit before the title: the layout puts more there.
    #[must_use]
    pub fn is_leading(&self) -> bool {
        self.leading.len() > self.trailing.len()
    }

    /// Every button, in order, at whichever side holds it.
    #[must_use]
    pub fn order(&self) -> Vec<WindowButton> {
        self.leading.iter().chain(&self.trailing).copied().collect()
    }

    /// Whether the layout lists `button` on either side.
    #[must_use]
    pub fn shows(&self, button: WindowButton) -> bool {
        self.leading.contains(&button) || self.trailing.contains(&button)
    }
}

impl DesktopSettings {
    /// This snapshot, with `lower` filling the values it lacks.
    #[must_use]
    pub fn or(self, lower: Self) -> Self {
        Self {
            primary_paste: self.primary_paste.or(lower.primary_paste),
            accent: self.accent.or(lower.accent),
            text_scale: self.text_scale.or(lower.text_scale),
            button_layout: self.button_layout.or(lower.button_layout),
            overlay_scrolling: self.overlay_scrolling.or(lower.overlay_scrolling),
            clock_24h: self.clock_24h.or(lower.clock_24h),
            double_click_time: self.double_click_time.or(lower.double_click_time),
            double_click_distance: self.double_click_distance.or(lower.double_click_distance),
            drag_threshold: self.drag_threshold.or(lower.drag_threshold),
            cursor_blink: self.cursor_blink.or(lower.cursor_blink),
            cursor_blink_time: self.cursor_blink_time.or(lower.cursor_blink_time),
            cursor_blink_timeout: self.cursor_blink_timeout.or(lower.cursor_blink_timeout),
            caret_width: self.caret_width.or(lower.caret_width),
            font_hinting: self.font_hinting.or(lower.font_hinting),
            font_antialias: self.font_antialias.or(lower.font_antialias),
            contrast: self.contrast.or(lower.contrast),
            desktop_high_contrast: self.desktop_high_contrast.or(lower.desktop_high_contrast),
        }
    }

    /// Whether the user asked for more contrast: the standardised preference,
    /// else the desktop's own switch.
    #[must_use]
    pub fn high_contrast(&self) -> Option<bool> {
        self.contrast.or(self.desktop_high_contrast)
    }

    /// The input preferences these settings give, over `base` for what they
    /// do not say.
    #[must_use]
    pub fn input_over(&self, base: InputPreferences) -> InputPreferences {
        InputPreferences {
            double_click_time: self.double_click_time.unwrap_or(base.double_click_time),
            double_click_distance: self
                .double_click_distance
                .unwrap_or(base.double_click_distance),
            drag_threshold: self.drag_threshold.unwrap_or(base.drag_threshold),
            cursor_blink: self.cursor_blink.unwrap_or(base.cursor_blink),
            cursor_blink_time: self.cursor_blink_time.unwrap_or(base.cursor_blink_time),
            cursor_blink_timeout: self
                .cursor_blink_timeout
                .unwrap_or(base.cursor_blink_timeout),
            caret_width: self.caret_width.unwrap_or(base.caret_width),
            reduced_motion: base.reduced_motion,
            text_hinting: self.font_hinting.or(base.text_hinting),
            text_antialias: self.font_antialias.or(base.text_antialias),
        }
    }

    /// Reads one `settings.ini` text (`[Settings]` section).
    #[must_use]
    pub fn from_gtk_ini(text: &str) -> Self {
        let value = |key: &str| ini_value(text, "Settings", key);
        Self {
            primary_paste: value("gtk-enable-primary-paste").and_then(parse_bool),
            button_layout: value("gtk-decoration-layout").map(ButtonLayout::parse),
            overlay_scrolling: value("gtk-overlay-scrolling").and_then(parse_bool),
            double_click_time: value("gtk-double-click-time")
                .and_then(parse_count)
                .map(Duration::from_millis),
            double_click_distance: value("gtk-double-click-distance").and_then(parse_pixels),
            drag_threshold: value("gtk-dnd-drag-threshold").and_then(parse_pixels),
            cursor_blink: value("gtk-cursor-blink").and_then(parse_bool),
            cursor_blink_time: value("gtk-cursor-blink-time")
                .and_then(parse_count)
                .map(Duration::from_millis),
            cursor_blink_timeout: value("gtk-cursor-blink-timeout")
                .and_then(parse_count)
                .map(Duration::from_secs),
            caret_width: value("gtk-cursor-aspect-ratio")
                .and_then(|ratio| ratio.trim().parse::<f32>().ok())
                .and_then(caret_width_from_aspect),
            font_hinting: match (
                value("gtk-xft-hinting").and_then(parse_xft_bool),
                value("gtk-xft-hintstyle"),
            ) {
                (Some(false), _) => Some(false),
                (hinting, Some(style)) => Some(style.trim() != "hintnone").or(hinting),
                (hinting, None) => hinting,
            },
            font_antialias: value("gtk-xft-antialias").and_then(parse_xft_bool),
            desktop_high_contrast: value("gtk-theme-name")
                .map(|theme| theme.trim().starts_with("HighContrast")),
            ..Self::default()
        }
    }

    /// Reads KDE's `kdeglobals` text (`[KDE]` section): double-click
    /// interval, drag distance and caret blink rate (`0` turns blinking off).
    #[must_use]
    pub fn from_kdeglobals(text: &str) -> Self {
        let value = |key: &str| ini_value(text, "KDE", key);
        let general = |key: &str| ini_value(text, "General", key);
        let blink_rate = value("CursorBlinkRate").and_then(parse_count);
        Self {
            font_hinting: general("XftHintStyle").map(|style| style.trim() != "hintnone"),
            font_antialias: general("XftAntialias").and_then(parse_bool),
            double_click_time: value("DoubleClickInterval")
                .and_then(parse_count)
                .map(Duration::from_millis),
            drag_threshold: value("StartDragDist").and_then(parse_pixels),
            cursor_blink: blink_rate.map(|rate| rate > 0),
            cursor_blink_time: blink_rate
                .filter(|rate| *rate > 0)
                .map(Duration::from_millis),
            ..Self::default()
        }
    }

    /// Reads KDE's `kdeglobals` files, the first one that says something
    /// winning for each value.
    #[must_use]
    pub fn from_kdeglobals_files(candidates: &[PathBuf]) -> Self {
        candidates
            .iter()
            .filter_map(|path| read_small(path))
            .map(|text| Self::from_kdeglobals(&text))
            .fold(Self::default(), Self::or)
    }

    /// Reads the GTK settings files of this user, the first one that says
    /// something winning for each value.
    #[must_use]
    pub fn from_files(candidates: &[PathBuf]) -> Self {
        candidates
            .iter()
            .filter_map(|path| read_small(path))
            .map(|text| Self::from_gtk_ini(&text))
            .fold(Self::default(), Self::or)
    }
}

/// The GTK `settings.ini` files for this environment, highest precedence
/// first: `$XDG_CONFIG_HOME` (or `~/.config`), then each `$XDG_CONFIG_DIRS`
/// entry, GTK 4 before GTK 3. Relative roots are ignored.
#[must_use]
pub fn gtk_settings_files(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Vec<PathBuf> {
    config_roots(var)
        .into_iter()
        .flat_map(|root| {
            [
                root.join("gtk-4.0").join("settings.ini"),
                root.join("gtk-3.0").join("settings.ini"),
            ]
        })
        .collect()
}

/// The XDG configuration roots, highest precedence first; relative roots are
/// ignored.
fn config_roots(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Vec<PathBuf> {
    let home = var("HOME").map(PathBuf::from);
    let config_home = var("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.map(|home| home.join(".config")));
    let config_dirs: Vec<PathBuf> = match var("XDG_CONFIG_DIRS") {
        Some(value) if !value.is_empty() => std::env::split_paths(&value).collect(),
        _ => vec![PathBuf::from("/etc/xdg")],
    };
    config_home
        .into_iter()
        .chain(config_dirs)
        .filter(|root| root.is_absolute())
        .collect()
}

/// KDE's `kdeglobals` files for this environment, highest precedence first:
/// `$XDG_CONFIG_HOME` (or `~/.config`), then each `$XDG_CONFIG_DIRS` entry.
#[must_use]
pub fn kdeglobals_files(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Vec<PathBuf> {
    config_roots(var)
        .into_iter()
        .map(|root| root.join("kdeglobals"))
        .collect()
}

/// The desktop's input preferences: double-click time and distance, drag
/// threshold, caret blinking and width, and reduced motion.
#[must_use]
pub fn input() -> InputPreferences {
    iced_core::preferences::get()
}

pub use iced_core::preferences::Preferences as InputPreferences;

/// Applies the settings that act process-wide and publishes the snapshot to
/// [`current`] and every [`subscription`].
pub fn apply(settings: &DesktopSettings) {
    if let Some(enabled) = settings.primary_paste {
        iced_core::clipboard::set_primary_paste_enabled(enabled);
    }
    iced_core::preferences::update(|preferences| {
        *preferences = settings.input_over(InputPreferences {
            reduced_motion: preferences.reduced_motion,
            ..InputPreferences::DEFAULT
        });
    });
    let changed = {
        let mut current = CURRENT.write().unwrap_or_else(PoisonError::into_inner);
        let changed = current.as_ref() != Some(settings);
        *current = Some(settings.clone());
        changed
    };
    if changed {
        let mut listeners = LISTENERS.lock().unwrap_or_else(PoisonError::into_inner);
        listeners.retain(|listener| listener.unbounded_send(settings.clone()).is_ok());
    }
}

static CURRENT: RwLock<Option<DesktopSettings>> = RwLock::new(None);
static LISTENERS: Mutex<
    Vec<iced_futures::futures::channel::mpsc::UnboundedSender<DesktopSettings>>,
> = Mutex::new(Vec::new());

thread_local! {
    static SCOPED: std::cell::RefCell<Option<DesktopSettings>> =
        const { std::cell::RefCell::new(None) };
}

/// Runs `f` with [`current`] answering `settings` on this thread, so a test
/// can lay out widgets under a given desktop without changing the settings
/// other threads see.
pub fn with_settings<R>(settings: DesktopSettings, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<DesktopSettings>);
    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            SCOPED.with(|scoped| *scoped.borrow_mut() = previous);
        }
    }
    let _restore = Restore(SCOPED.with(|scoped| scoped.borrow_mut().replace(settings)));
    f()
}

/// The desktop settings read so far (empty before the reader ran), or the
/// ones [`with_settings`] gives this thread.
#[must_use]
pub fn current() -> DesktopSettings {
    if let Some(scoped) = SCOPED.with(|scoped| scoped.borrow().clone()) {
        return scoped;
    }
    CURRENT
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .unwrap_or_default()
}

/// The window button layout the desktop asks for, or GTK's default.
#[must_use]
pub fn button_layout() -> ButtonLayout {
    current().button_layout.unwrap_or_default()
}

/// Whether scrollbars hide until used; `true` unless the desktop asks for
/// always-visible scrollbars.
#[must_use]
pub fn overlay_scrollbars() -> bool {
    current().overlay_scrolling.unwrap_or(true)
}

/// The clock format the desktop asks for: `Some(true)` for 24 hours,
/// `Some(false)` for 12, `None` to follow the locale.
#[must_use]
pub fn clock_24h() -> Option<bool> {
    current().clock_24h
}

/// The snapshot now and after every change, for an application that rebuilds
/// its view or theme when the desktop changes (`ThemeIntegration` follows it).
pub fn subscription() -> iced_futures::Subscription<DesktopSettings> {
    struct Recipe;
    impl iced_futures::subscription::Recipe for Recipe {
        type Output = DesktopSettings;

        fn hash(&self, state: &mut iced_futures::subscription::Hasher) {
            use std::hash::Hash;
            "enzui-desktop-settings".hash(state);
        }

        fn stream(
            self: Box<Self>,
            _input: iced_futures::subscription::EventStream,
        ) -> iced_futures::BoxStream<Self::Output> {
            let (sender, receiver) = iced_futures::futures::channel::mpsc::unbounded();
            let _ = sender.unbounded_send(current());
            LISTENERS
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(sender);
            Box::pin(receiver)
        }
    }
    iced_futures::subscription::from_recipe(Recipe)
}

/// Starts the background reader: settings files once, then (with
/// `desktop-settings-portal`) the Settings portal and its change signal. Returns `false`
/// when the thread could not be started; settings then keep their defaults.
/// Calling it again does nothing and reports the first outcome.
pub fn spawn_watcher() -> bool {
    static STARTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *STARTED.get_or_init(start_watcher)
}

fn start_watcher() -> bool {
    std::thread::Builder::new()
        .name(String::from("enzui-desktop-settings"))
        .spawn(|| {
            let var = |key: &str| std::env::var_os(key);
            let files = DesktopSettings::from_files(&gtk_settings_files(var)).or(
                DesktopSettings::from_kdeglobals_files(&kdeglobals_files(var)),
            );
            apply(&files);
            #[cfg(feature = "desktop-settings-portal")]
            portal::follow(files);
        })
        .is_ok()
}

const MAX_SETTINGS_BYTES: u64 = 256 * 1024;

fn read_small(path: &Path) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take(MAX_SETTINGS_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// The value of `key` in `[section]` of an ini-style text; the last
/// occurrence wins, as GTK's parser does.
fn ini_value<'a>(text: &'a str, section: &str, key: &str) -> Option<&'a str> {
    let mut current = "";
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.starts_with(';') || line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = name.trim();
            continue;
        }
        if current == section
            && let Some((name, value)) = line.split_once('=')
            && name.trim() == key
        {
            found = Some(value.trim());
        }
    }
    found
}

/// A non-negative whole number, as GTK and KDE write times and distances.
fn parse_count(value: &str) -> Option<u64> {
    value.trim().parse().ok()
}

/// A distance in pixels, at most 1000.
fn parse_pixels(value: &str) -> Option<f32> {
    #[allow(clippy::cast_precision_loss, reason = "at most 1000")]
    parse_count(value)
        .filter(|px| *px <= 1000)
        .map(|px| px as f32)
}

/// GTK draws its caret `height × gtk-cursor-aspect-ratio + 1` pixels wide;
/// at a 20 px line the default ratio (0.04) is one pixel.
fn caret_width_from_aspect(ratio: f32) -> Option<f32> {
    (ratio.is_finite() && (0.0..=1.0).contains(&ratio)).then(|| (20.0 * ratio + 1.0).floor())
}

/// GTK's Xft integers: `1` on, `0` off, `-1` (or anything else) unset.
fn parse_xft_bool(value: &str) -> Option<bool> {
    match value.trim() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

#[cfg(feature = "desktop-settings-portal")]
mod portal {
    //! The Settings portal source: one read per key, then the change signal.

    use futures_util::StreamExt;
    use zbus::zvariant::{OwnedValue, Value};

    use std::time::Duration;

    use super::{ButtonLayout, DesktopSettings, apply, parse_count, parse_pixels};

    /// The `(namespace, key)` pairs this module reads.
    pub(super) const KEYS: [(&str, &str); 18] = [
        ("org.gnome.desktop.interface", "gtk-enable-primary-paste"),
        ("org.gnome.desktop.interface", "text-scaling-factor"),
        ("org.gnome.desktop.interface", "overlay-scrolling"),
        ("org.gnome.desktop.interface", "clock-format"),
        ("org.gnome.desktop.interface", "cursor-blink"),
        ("org.gnome.desktop.a11y.interface", "high-contrast"),
        ("org.freedesktop.appearance", "contrast"),
        ("org.gnome.desktop.interface", "font-hinting"),
        ("org.gnome.desktop.interface", "font-antialiasing"),
        ("org.gnome.desktop.interface", "cursor-blink-time"),
        ("org.gnome.desktop.interface", "cursor-blink-timeout"),
        ("org.gnome.desktop.peripherals.mouse", "double-click"),
        ("org.gnome.desktop.peripherals.mouse", "drag-threshold"),
        ("org.gnome.desktop.wm.preferences", "button-layout"),
        ("org.freedesktop.appearance", "accent-color"),
        (KDE, "DoubleClickInterval"),
        (KDE, "StartDragDist"),
        (KDE, "CursorBlinkRate"),
    ];

    /// KDE's portal namespace for the `[KDE]` group of `kdeglobals`.
    const KDE: &str = "org.kde.kdeglobals.KDE";

    #[zbus::proxy(
        interface = "org.freedesktop.portal.Settings",
        default_service = "org.freedesktop.portal.Desktop",
        default_path = "/org/freedesktop/portal/desktop"
    )]
    trait Settings {
        fn read_one(&self, namespace: &str, key: &str) -> zbus::Result<OwnedValue>;

        #[zbus(signal)]
        fn setting_changed(&self, namespace: &str, key: &str, value: Value<'_>)
        -> zbus::Result<()>;
    }

    /// The value inside any number of variant wrappers.
    fn unwrap<'a>(value: &'a Value<'a>) -> &'a Value<'a> {
        match value {
            Value::Value(inner) => unwrap(inner),
            other => other,
        }
    }

    /// The portal's value for one key this module knows.
    pub(super) fn from_value(namespace: &str, key: &str, value: &Value<'_>) -> DesktopSettings {
        let value = unwrap(value);
        let boolean = || match value {
            Value::Bool(b) => Some(*b),
            _ => None,
        };
        let double = || match value {
            Value::F64(d) => Some(*d),
            _ => None,
        };
        let string = || match value {
            Value::Str(s) => Some(s.as_str().to_owned()),
            _ => None,
        };
        // GSettings sends integers; KDE's portal may send the config text.
        let count = || match value {
            Value::I32(n) => u64::try_from(*n).ok(),
            Value::U32(n) => Some(u64::from(*n)),
            Value::I64(n) => u64::try_from(*n).ok(),
            Value::U64(n) => Some(*n),
            Value::Str(s) => parse_count(s.as_str()),
            _ => None,
        };
        let pixels = || count().and_then(|n| parse_pixels(&n.to_string()));
        let mut settings = DesktopSettings::default();
        match (namespace, key) {
            ("org.gnome.desktop.interface", "gtk-enable-primary-paste") => {
                settings.primary_paste = boolean();
            }
            ("org.gnome.desktop.interface", "text-scaling-factor") => {
                #[allow(clippy::cast_possible_truncation, reason = "a text factor near 1")]
                let factor = double()
                    .filter(|f| f.is_finite() && *f > 0.0)
                    .map(|f| f as f32);
                settings.text_scale = factor;
            }
            ("org.gnome.desktop.interface", "overlay-scrolling") => {
                settings.overlay_scrolling = boolean();
            }
            ("org.gnome.desktop.interface", "clock-format") => {
                settings.clock_24h = string().and_then(|format| match format.as_str() {
                    "24h" => Some(true),
                    "12h" => Some(false),
                    _ => None,
                });
            }
            ("org.freedesktop.appearance", "contrast") => {
                // `1` asks for more contrast, `0` states no preference.
                settings.contrast = count().map(|contrast| contrast == 1);
            }
            ("org.gnome.desktop.a11y.interface", "high-contrast") => {
                settings.desktop_high_contrast = boolean();
            }
            ("org.gnome.desktop.interface", "font-hinting") => {
                settings.font_hinting = string().map(|hinting| hinting != "none");
            }
            ("org.gnome.desktop.interface", "font-antialiasing") => {
                settings.font_antialias = string().map(|antialiasing| antialiasing != "none");
            }
            ("org.gnome.desktop.interface", "cursor-blink") => {
                settings.cursor_blink = boolean();
            }
            ("org.gnome.desktop.interface", "cursor-blink-time") => {
                settings.cursor_blink_time = count().map(Duration::from_millis);
            }
            ("org.gnome.desktop.interface", "cursor-blink-timeout") => {
                settings.cursor_blink_timeout = count().map(Duration::from_secs);
            }
            ("org.gnome.desktop.peripherals.mouse", "double-click")
            | (KDE, "DoubleClickInterval") => {
                settings.double_click_time = count().map(Duration::from_millis);
            }
            ("org.gnome.desktop.peripherals.mouse", "drag-threshold") | (KDE, "StartDragDist") => {
                settings.drag_threshold = pixels();
            }
            (KDE, "CursorBlinkRate") => {
                let rate = count();
                settings.cursor_blink = rate.map(|rate| rate > 0);
                settings.cursor_blink_time =
                    rate.filter(|rate| *rate > 0).map(Duration::from_millis);
            }
            ("org.gnome.desktop.wm.preferences", "button-layout") => {
                settings.button_layout = string().map(|layout| ButtonLayout::parse(&layout));
            }
            ("org.freedesktop.appearance", "accent-color") => {
                settings.accent = accent(value);
            }
            _ => {}
        }
        settings
    }

    /// `(ddd)` in 0..=1; out-of-range components mean "no accent".
    fn accent(value: &Value<'_>) -> Option<iced_core::Color> {
        let Value::Structure(structure) = value else {
            return None;
        };
        let channels: Vec<f64> = structure
            .fields()
            .iter()
            .filter_map(|field| match unwrap(field) {
                Value::F64(c) => Some(*c),
                _ => None,
            })
            .collect();
        let [r, g, b] = channels[..] else {
            return None;
        };
        if [r, g, b].iter().any(|c| !(0.0..=1.0).contains(c)) {
            return None;
        }
        #[allow(clippy::cast_possible_truncation, reason = "colour channels in 0..=1")]
        Some(iced_core::Color::from_rgb(r as f32, g as f32, b as f32))
    }

    /// Reads the portal, applies it over `files`, then follows changes until
    /// the bus goes away. Returns quietly without a portal.
    pub(super) fn follow(files: DesktopSettings) {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        runtime.block_on(async move {
            let Ok(connection) = zbus::Connection::session().await else {
                return;
            };
            let Ok(proxy) = SettingsProxy::new(&connection).await else {
                return;
            };
            let mut current = files;
            for (namespace, key) in KEYS {
                let read = tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    proxy.read_one(namespace, key),
                )
                .await;
                if let Ok(Ok(value)) = read {
                    current = from_value(namespace, key, &value).or(current);
                }
            }
            apply(&current);
            let Ok(mut changes) = proxy.receive_setting_changed().await else {
                return;
            };
            while let Some(signal) = changes.next().await {
                let Ok(args) = signal.args() else { continue };
                let update = from_value(args.namespace(), args.key(), args.value());
                if update == DesktopSettings::default() {
                    continue;
                }
                current = update.or(current);
                apply(&current);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_settings_decode_gtk_kde_and_portal_preferences_with_precedence() {
        // gtk_ini_values_and_precedence
        {
            let ini = "[Settings]\ngtk-theme-name=Adwaita\ngtk-enable-primary-paste=false\n";
            assert_eq!(
                DesktopSettings::from_gtk_ini(ini).primary_paste,
                Some(false)
            );
            assert_eq!(
                DesktopSettings::from_gtk_ini("[Other]\ngtk-enable-primary-paste=0").primary_paste,
                None
            );
            assert_eq!(
                DesktopSettings::from_gtk_ini("[Settings]\ngtk-enable-primary-paste = 1\n")
                    .primary_paste,
                Some(true)
            );
            let higher = DesktopSettings {
                primary_paste: None,
                ..DesktopSettings::default()
            };
            let lower = DesktopSettings {
                primary_paste: Some(false),
                ..DesktopSettings::default()
            };
            assert_eq!(higher.or(lower).primary_paste, Some(false));
        }

        // input_settings_from_gtk_and_kde_files
        {
            let gtk = DesktopSettings::from_gtk_ini(
                "[Settings]\ngtk-double-click-time=600\ngtk-double-click-distance=8\n\
             gtk-dnd-drag-threshold=12\ngtk-cursor-blink=false\ngtk-cursor-blink-time=800\n\
             gtk-cursor-blink-timeout=4\ngtk-cursor-aspect-ratio=0.1\n",
            );
            let input = gtk.input_over(InputPreferences {
                reduced_motion: true,
                ..InputPreferences::DEFAULT
            });
            assert_eq!(input.double_click_time, Duration::from_millis(600));
            assert_eq!(input.double_click_distance, 8.0);
            assert_eq!(input.drag_threshold, 12.0);
            assert!(!input.cursor_blink);
            assert_eq!(input.cursor_blink_time, Duration::from_millis(800));
            assert_eq!(input.cursor_blink_timeout, Duration::from_secs(4));
            assert_eq!(input.caret_width, 3.0, "20 px x 0.1 + 1");
            assert!(
                input.reduced_motion,
                "reduced motion is not a desktop file value"
            );

            let kde = DesktopSettings::from_kdeglobals(
                "[General]\nDoubleClickInterval=1\n[KDE]\nDoubleClickInterval=500\nStartDragDist=10\n\
             CursorBlinkRate=0\n",
            );
            assert_eq!(kde.double_click_time, Some(Duration::from_millis(500)));
            assert_eq!(kde.drag_threshold, Some(10.0));
            assert_eq!(kde.cursor_blink, Some(false));
            assert_eq!(
                DesktopSettings::default().input_over(InputPreferences::DEFAULT),
                InputPreferences::DEFAULT
            );
            assert_eq!(caret_width_from_aspect(0.04), Some(1.0));
            assert_eq!(
                DesktopSettings::from_gtk_ini("[Settings]\ngtk-theme-name=HighContrastInverse\n")
                    .high_contrast(),
                Some(true)
            );
            assert_eq!(
                DesktopSettings::from_gtk_ini("[Settings]\ngtk-theme-name=Adwaita\n")
                    .high_contrast(),
                Some(false)
            );

            let fonts = |ini: &str| {
                let settings = DesktopSettings::from_gtk_ini(&format!("[Settings]\n{ini}"));
                (settings.font_hinting, settings.font_antialias)
            };
            assert_eq!(
                fonts("gtk-xft-antialias=0\ngtk-xft-hinting=1"),
                (Some(true), Some(false))
            );
            assert_eq!(fonts("gtk-xft-hintstyle=hintnone"), (Some(false), None));
            assert_eq!(
                fonts("gtk-xft-hinting=0\ngtk-xft-hintstyle=hintfull"),
                (Some(false), None)
            );
            assert_eq!(fonts("gtk-xft-antialias=-1"), (None, None));
            let kde_fonts = DesktopSettings::from_kdeglobals(
                "[General]\nXftAntialias=false\nXftHintStyle=hintslight\n",
            );
            assert_eq!(
                (kde_fonts.font_hinting, kde_fonts.font_antialias),
                (Some(true), Some(false))
            );
            let input = DesktopSettings {
                font_antialias: Some(false),
                ..DesktopSettings::default()
            }
            .input_over(InputPreferences::DEFAULT);
            assert_eq!(
                (input.text_hinting, input.text_antialias),
                (None, Some(false))
            );
        }

        // files_are_read_in_precedence_order
        {
            let scratch = enzui_test_support::test_scratch::ScratchDir::new("desktop-settings");
            let user = scratch.path().join("user");
            let system = scratch.path().join("system");
            std::fs::create_dir_all(user.join("gtk-3.0")).unwrap();
            std::fs::create_dir_all(system.join("gtk-4.0")).unwrap();
            std::fs::write(
                user.join("gtk-3.0/settings.ini"),
                "[Settings]\ngtk-enable-primary-paste=false\n",
            )
            .unwrap();
            std::fs::write(
                system.join("gtk-4.0/settings.ini"),
                "[Settings]\ngtk-enable-primary-paste=true\n",
            )
            .unwrap();
            let var = |key: &str| match key {
                "XDG_CONFIG_HOME" => Some(user.clone().into_os_string()),
                "XDG_CONFIG_DIRS" => Some(system.clone().into_os_string()),
                _ => None,
            };
            let files = gtk_settings_files(var);
            assert_eq!(files[1], user.join("gtk-3.0/settings.ini"));
            assert_eq!(
                DesktopSettings::from_files(&files).primary_paste,
                Some(false),
                "the user's file wins"
            );
        }

        // button_layouts_parse_like_gtk
        {
            use WindowButton::{Close, Maximize, Minimize};
            let gnome = ButtonLayout::parse("appmenu:minimize,maximize,close");
            assert_eq!(gnome.trailing, [Minimize, Maximize, Close]);
            assert!(gnome.leading.is_empty() && !gnome.is_leading());
            let mac = ButtonLayout::parse("close,minimize,maximize:");
            assert_eq!(mac.leading, [Close, Minimize, Maximize]);
            assert!(mac.is_leading());
            assert_eq!(mac.order(), [Close, Minimize, Maximize]);
            let bare = ButtonLayout::parse("close");
            assert_eq!(bare.trailing, [Close]);
            let ini = DesktopSettings::from_gtk_ini(
                "[Settings]\ngtk-decoration-layout=close:\ngtk-overlay-scrolling=false\n",
            );
            assert_eq!(
                ini.button_layout.map(|layout| layout.leading),
                Some(vec![Close])
            );
            assert_eq!(ini.overlay_scrolling, Some(false));
        }
    }

    #[cfg(feature = "desktop-settings-portal")]
    #[test]
    fn portal_values_decode_plain_and_wrapped_booleans() {
        use zbus::zvariant::Value;
        assert_eq!(
            portal::from_value(
                "org.gnome.desktop.interface",
                "gtk-enable-primary-paste",
                &Value::Bool(false)
            )
            .primary_paste,
            Some(false)
        );
        let wrapped = Value::Value(Box::new(Value::Bool(true)));
        assert_eq!(
            portal::from_value(
                "org.gnome.desktop.interface",
                "gtk-enable-primary-paste",
                &wrapped
            )
            .primary_paste,
            Some(true)
        );
        assert_eq!(
            portal::from_value("org.gnome.desktop.interface", "other", &Value::Bool(true)),
            DesktopSettings::default()
        );
        let interface = "org.gnome.desktop.interface";
        assert_eq!(
            portal::from_value(interface, "text-scaling-factor", &Value::F64(1.25)).text_scale,
            Some(1.25)
        );
        assert_eq!(
            portal::from_value(interface, "clock-format", &Value::from("12h")).clock_24h,
            Some(false)
        );
        assert_eq!(
            portal::from_value(interface, "overlay-scrolling", &Value::Bool(false))
                .overlay_scrolling,
            Some(false)
        );
        let layout = portal::from_value(
            "org.gnome.desktop.wm.preferences",
            "button-layout",
            &Value::from("close:"),
        );
        assert!(
            layout
                .button_layout
                .is_some_and(|layout| layout.is_leading())
        );
        let accent = Value::Structure(
            zbus::zvariant::StructureBuilder::new()
                .add_field(0.2_f64)
                .add_field(0.4_f64)
                .add_field(0.6_f64)
                .build()
                .unwrap(),
        );
        assert_eq!(
            portal::from_value("org.freedesktop.appearance", "accent-color", &accent).accent,
            Some(iced_core::Color::from_rgb(0.2, 0.4, 0.6))
        );
        let unset = Value::Structure(
            zbus::zvariant::StructureBuilder::new()
                .add_field(-1.0_f64)
                .add_field(-1.0_f64)
                .add_field(-1.0_f64)
                .build()
                .unwrap(),
        );
        assert_eq!(
            portal::from_value("org.freedesktop.appearance", "accent-color", &unset).accent,
            None,
            "out of range means no accent"
        );
    }

    #[cfg(feature = "desktop-settings-portal")]
    #[test]
    fn portal_input_values_from_gnome_and_kde() {
        use zbus::zvariant::Value;
        let mouse = "org.gnome.desktop.peripherals.mouse";
        let interface = "org.gnome.desktop.interface";
        let kde = "org.kde.kdeglobals.KDE";
        assert_eq!(
            portal::from_value(mouse, "double-click", &Value::I32(550)).double_click_time,
            Some(Duration::from_millis(550))
        );
        assert_eq!(
            portal::from_value(mouse, "drag-threshold", &Value::I32(9)).drag_threshold,
            Some(9.0)
        );
        assert_eq!(
            portal::from_value(interface, "cursor-blink", &Value::Bool(false)).cursor_blink,
            Some(false)
        );
        assert_eq!(
            portal::from_value(interface, "cursor-blink-timeout", &Value::I32(3))
                .cursor_blink_timeout,
            Some(Duration::from_secs(3))
        );
        assert_eq!(
            portal::from_value(kde, "DoubleClickInterval", &Value::from("650")).double_click_time,
            Some(Duration::from_millis(650))
        );
        assert_eq!(
            portal::from_value(interface, "font-antialiasing", &Value::from("none")).font_antialias,
            Some(false)
        );
        assert_eq!(
            portal::from_value(interface, "font-antialiasing", &Value::from("rgba")).font_antialias,
            Some(true)
        );
        assert_eq!(
            portal::from_value(interface, "font-hinting", &Value::from("none")).font_hinting,
            Some(false)
        );
        let appearance = "org.freedesktop.appearance";
        let gnome_a11y = "org.gnome.desktop.a11y.interface";
        let more = portal::from_value(appearance, "contrast", &Value::U32(1));
        let none = portal::from_value(appearance, "contrast", &Value::U32(0));
        let gnome = portal::from_value(gnome_a11y, "high-contrast", &Value::Bool(true));
        assert_eq!(more.high_contrast(), Some(true));
        assert_eq!(gnome.high_contrast(), Some(true));
        assert_eq!(
            none.or(gnome).high_contrast(),
            Some(false),
            "the standardised key outranks GNOME's"
        );
        let still = portal::from_value(kde, "CursorBlinkRate", &Value::from("0"));
        assert_eq!(
            (still.cursor_blink, still.cursor_blink_time),
            (Some(false), None)
        );
        assert_eq!(
            portal::from_value(mouse, "double-click", &Value::I32(-5)).double_click_time,
            None
        );
    }
}
