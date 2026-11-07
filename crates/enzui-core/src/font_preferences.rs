//! Desktop font preferences for UI, prose and code text.
//!
//! Sources report user choices; renderer font discovery and embedded-font loading
//! are separate capabilities. Shared precedence resolves application override,
//! platform preference, theme choice, then generic fallback into the font catalog.
//!
//! [`crate::theme_file::integration::ThemeIntegration`] owns the feed for shared
//! theme hosts. With `desktop-font-portal` (enabled by the widgets default features),
//! it follows the Settings portal and falls back to GTK/KDE settings files.
//! File changes use directory watches when available and polling otherwise.
//! Hosts owning their own theme can
//! drive [`DesktopFontFeed`] or inject [`ScriptedFontBackend`] directly.
//!
//! With `desktop-font-portal`, `portal` supplies portal change signals and
//! `portal_with_file_fallback` puts settings files behind it. Portal transport
//! does not define a universal desktop font-key namespace; these sources read
//! desktop-specific extensions. Unsupported sources release the platform layer;
//! failed reads retain the last delivered snapshot. [`FontFeedStatus`] reports the outcome.

pub use enzui_theme::font_preferences::{
    DesktopFontPreferences, FontLayer, FontPreferencePolicy, ResolvedFontRole,
};
pub use enzui_theme::font_source::{
    DEFAULT_FONT_POLL_INTERVAL, DEFAULT_RENDERER_TEXT_SIZE, DesktopFontBackend, DesktopFontFeed,
    FallbackFontBackend, FontBackendHandle, FontFeedStatus, FontPreferenceEvent,
    FontPreferenceRead, FontPreferenceSignals, FontSignal, MAX_SETTINGS_FILE_BYTES,
    ScriptedFontBackend, SettingsFileFontBackend, SettingsFileKind, parse_settings_file,
};

#[cfg(feature = "desktop-font-portal")]
pub mod portal;
pub mod watched_file;

/// The freedesktop settings portal in front of the desktops' settings files.
///
/// The portal answers first; an unreachable bus, a portal without the
/// namespace, or a namespace with no font keys falls through to
/// [`SettingsFileFontBackend`], so a desktop the files answer for keeps being
/// followed when the portal cannot answer.
#[cfg(feature = "desktop-font-portal")]
#[must_use]
pub fn portal_with_file_fallback() -> std::sync::Arc<dyn DesktopFontBackend> {
    std::sync::Arc::new(FallbackFontBackend::new(
        std::sync::Arc::new(portal::PortalFontBackend::new()),
        std::sync::Arc::new(SettingsFileFontBackend::from_env()),
    ))
}

/// Apply desktop families and UI size/weight to a fresh theme whose document
/// tokens have already been applied. Rebuild the base theme for each update. Per-app families and authored typography retain
/// precedence. This is the same resolver used by `ThemeIntegration`; manual
/// desktop hosts can use it before publishing their theme.
pub fn apply_to_theme(
    theme: &mut enzui_theme::EnzTheme,
    document: &crate::theme_file::ThemeDocument,
    mode: enzui_theme::EnzThemeMode,
    fonts: &FontPreferencePolicy,
) {
    let mut effective_fonts = fonts.clone();
    for role in enzui_theme::FontRole::ALL {
        if document.has_application_font(role) && fonts.override_family(role).is_none() {
            let _ =
                effective_fonts.set_override(role, Some(theme.font_catalog.family(role).clone()));
        }
    }
    let _ = effective_fonts.apply(&mut theme.font_catalog);
    if let Some(description) = fonts
        .platform()
        .and_then(|fonts| fonts.parsed(enzui_theme::FontRole::Ui))
    {
        let pixels = description.size.map(|size| match size {
            enzui_theme::FontSize::Points(points) => points * (96.0 / 72.0),
            enzui_theme::FontSize::Pixels(pixels) => pixels,
        });
        if document.get("typography.body_size", mode).is_none()
            && let Some(pixels) = pixels.filter(|pixels| (4.0..=128.0).contains(pixels))
        {
            theme.typography = theme.typography.with_base_body_size(pixels);
        }
        let preferred: i16 = match description
            .weight
            .unwrap_or(iced_core::font::Weight::Normal)
        {
            iced_core::font::Weight::Thin => 100,
            iced_core::font::Weight::ExtraLight => 200,
            iced_core::font::Weight::Light => 300,
            iced_core::font::Weight::Normal => 400,
            iced_core::font::Weight::Medium => 500,
            iced_core::font::Weight::Semibold => 600,
            iced_core::font::Weight::Bold => 700,
            iced_core::font::Weight::ExtraBold => 800,
            iced_core::font::Weight::Black => 900,
        };
        let offset = preferred - 400;
        for (key, weight) in [
            (
                "typography.weight_regular",
                &mut theme.typography.weight_regular,
            ),
            (
                "typography.weight_medium",
                &mut theme.typography.weight_medium,
            ),
            (
                "typography.weight_semibold",
                &mut theme.typography.weight_semibold,
            ),
            ("typography.weight_bold", &mut theme.typography.weight_bold),
            (
                "typography.weight_extrabold",
                &mut theme.typography.weight_extrabold,
            ),
        ] {
            if document.get(key, mode).is_none() {
                *weight = weight.saturating_add_signed(offset).clamp(100, 900);
            }
        }
    }
}
