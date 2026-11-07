/// Page-navigation policy for a settings dialog; defaults to Automatic.
/// Dialogs with at most one page resolve to None regardless of the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsNav {
    /// Measure the labels against the available width and choose
    /// ([`settings_nav_for`]). The default.
    #[default]
    Automatic,
    /// Suppress page navigation.
    /// Normally resolved for at most one page; forcing this on a multi-page dialog
    /// requires the caller to provide another way to select pages.
    None,
    /// Request a horizontal scrollable tab strip, even when its labels overflow.
    Tabs,
    /// Force the sidebar rail beside the pane, even when tabs would fit.
    Sidebar,
    /// Request vertical navigation above the pane.
    /// An [`EnzSettingsDialog::on_back`] handler and drill state can separate the
    /// page list and pane into two steps.
    Stacked,
}

/// Sidebar rail width in logical pixels: 168; long labels can still elide.
pub const SIDEBAR_RAIL_WIDTH: f32 = 168.0;

/// Sidebar fallback threshold in logical pixels: 464.
/// Applies when the measured tab strip does not fit; a fitting strip still
/// uses Tabs below this threshold.
pub const MIN_SPLIT_WIDTH: f32 = SIDEBAR_RAIL_WIDTH + 280.0 + 16.0;

/// Choose navigation for labels within an available content width in logical pixels.
/// Uses [`SwitcherTokens::ROUNDED_STRIP`]. A fitting strip chooses Tabs; an
/// overflowing strip chooses Sidebar at >=464 px, otherwise Stacked.
/// Nonpositive or non-finite width chooses Stacked.
///
/// Subtract surface content insets before passing `available`. Measurement uses
/// semibold shaped labels and per-cell floors; `icons` reserves icon space for
/// every label. This helper does not apply the dialog's single-page None policy
/// and returns neither Automatic nor None.
#[must_use]
pub fn settings_nav_for(labels: &[&str], available: f32, icons: bool) -> SettingsNav {
    settings_nav_for_with(labels, available, icons, SwitcherTokens::ROUNDED_STRIP)
}

/// Choose navigation with explicit switcher geometry.
/// Uses [`SwitcherTokens::strip_width`] and the same fallback policy as
/// [`settings_nav_for`]; even custom tokens use semibold text measurement.
#[must_use]
pub fn settings_nav_for_with(
    labels: &[&str],
    available: f32,
    icons: bool,
    tokens: SwitcherTokens,
) -> SettingsNav {
    if !available.is_finite() || available <= 0.0 {
        return SettingsNav::Stacked;
    }
    if tokens.strip_width(labels, icons) <= available {
        SettingsNav::Tabs
    } else if available >= MIN_SPLIT_WIDTH {
        SettingsNav::Sidebar
    } else {
        SettingsNav::Stacked
    }
}
