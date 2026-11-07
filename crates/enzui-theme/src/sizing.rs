//! Shared logical-pixel dimensions for controls, targets, surfaces, icons,
//! content widths, and layout switches.
//!
//! `control_height` is not "the button height": buttons use the independent
//! five-preset ladder of `ButtonSizeTokens`.

/// Sizing tokens — dimensions of UI elements.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SizingTokens {
    /// Default composed-row height, 32 logical pixels.
    /// Buttons use their independent `ButtonSizeTokens` ladder.
    pub control_height: f32,
    /// WCAG 2.2 SC 2.5.8 minimum-target token, defaulting to 24 logical pixels.
    /// [`Self::control_min_hit`] is the larger primary-control target.
    pub min_hit_target: f32,
    /// Minimum pointer hit target, in logical pixels.
    pub control_min_hit: f32,
    /// Default list and table row height, in logical pixels.
    pub row_height: f32,
    /// Toolbar height, in logical pixels.
    pub toolbar_height: f32,
    /// Height of a standalone titlebar and the default app-shell titlebar band.
    pub titlebar_height: f32,
    /// Text-field height, in logical pixels.
    pub field_height: f32,
    /// Resting width of an inline fixed-width text field; defaults to 240
    /// logical pixels.
    pub search_field_width: f32,

    /// Default sidebar width, in logical pixels.
    pub sidebar_width: f32,
    /// Sidebar row height, in logical pixels.
    pub sidebar_row_height: f32,
    /// Minimum resizable sidebar width, in logical pixels.
    pub sidebar_min_width: f32,
    /// Maximum resizable sidebar width, in logical pixels.
    pub sidebar_max_width: f32,

    /// Minimum window width, in logical pixels.
    pub window_min_width: f32,
    /// Minimum window height, in logical pixels.
    pub window_min_height: f32,
    /// Default window width, in logical pixels.
    pub window_default_width: f32,
    /// Default window height, in logical pixels.
    pub window_default_height: f32,

    /// Small dialog width, in logical pixels.
    pub dialog_width_sm: f32,
    /// Medium dialog width, in logical pixels.
    pub dialog_width_md: f32,
    /// Large dialog width, in logical pixels.
    pub dialog_width_lg: f32,

    /// Minimum touch target, in logical pixels.
    pub touch_target: f32,

    /// Status-bar height, in logical pixels.
    pub statusbar_height: f32,

    /// Extra-extra-small icon size, in logical pixels.
    pub icon_xxs: f32,
    /// Extra-small icon size, in logical pixels.
    pub icon_xs: f32,
    /// Small icon size, in logical pixels.
    pub icon_sm: f32,
    /// Medium icon size, in logical pixels.
    pub icon_md: f32,
    /// Large icon size, in logical pixels.
    pub icon_lg: f32,
    /// Extra-large icon size, in logical pixels.
    pub icon_xl: f32,
    /// Extra-extra-large icon size, in logical pixels.
    pub icon_xxl: f32,

    /// Status-dot diameter (px) — the presence/state pip next to a label.
    pub status_dot: f32,

    /// Narrow single-column content width; defaults to 480 logical pixels.
    pub container_narrow: f32,
    /// General app-content width; defaults to 720 logical pixels.
    pub container_default: f32,
    /// Mid-density content width; defaults to 960 logical pixels.
    pub container_wide: f32,
    /// Full dashboard/gallery width; defaults to 1280 logical pixels.
    pub container_xwide: f32,

    /// Narrow layout-switch threshold; defaults to 520 logical pixels.
    pub layout_switch_narrow: f32,
    /// Stack layout-switch threshold; defaults to 640 logical pixels and is
    /// independent of [`Self::dialog_width_lg`].
    pub layout_switch_stack: f32,
    /// Compact layout-switch threshold; defaults to 900 logical pixels.
    pub layout_switch_compact: f32,
    /// Wide layout-switch threshold; defaults to 1000 logical pixels.
    pub layout_switch_wide: f32,
}

impl Default for SizingTokens {
    fn default() -> Self {
        Self {
            control_height: 32.0,
            min_hit_target: 24.0,
            control_min_hit: 36.0,
            row_height: 34.0,
            toolbar_height: 48.0,
            titlebar_height: 44.0,
            field_height: 32.0,
            search_field_width: 240.0,

            sidebar_width: 260.0,
            sidebar_row_height: 34.0,
            sidebar_min_width: 200.0,
            sidebar_max_width: 340.0,

            window_min_width: 720.0,
            window_min_height: 480.0,
            window_default_width: 1100.0,
            window_default_height: 760.0,

            dialog_width_sm: 360.0,
            dialog_width_md: 460.0,
            dialog_width_lg: 640.0,

            touch_target: 36.0,
            statusbar_height: 28.0,

            icon_xxs: 10.0,
            icon_xs: 12.0,
            icon_sm: 16.0,
            icon_md: 20.0,
            icon_lg: 24.0,
            icon_xl: 32.0,
            icon_xxl: 48.0,

            status_dot: 8.0,

            container_narrow: 480.0,
            container_default: 720.0,
            container_wide: 960.0,
            container_xwide: 1280.0,

            layout_switch_narrow: 520.0,
            layout_switch_stack: 640.0,
            layout_switch_compact: 900.0,
            layout_switch_wide: 1000.0,
        }
    }
}
