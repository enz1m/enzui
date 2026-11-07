/// Size preset for responsive settings-dialog geometry.
/// Width and height caps vary with the viewport rungs; Medium is the default.
/// Minimum available dimensions can exceed tiny viewports; see
/// [`settings_dialog_geometry_sized`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DialogSize {
    /// Narrow desktop preset with a 460 px width ceiling in the wide rung.
    Compact,
    /// Small desktop preset with a 560 px width ceiling in the wide rung.
    Small,
    /// Default desktop preset with an 880 px width ceiling in the wide rung.
    #[default]
    Medium,
    /// Large desktop preset with a 1080 px width ceiling in the wide rung.
    Large,
}

/// Choose a centered dialog, bottom sheet, or viewport-based selection.
/// Defaults to Adaptive ([`enz_settings_presentation_for`]): a centred dialog,
/// and a sheet from the bottom edge in a small window, as every app should.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EnzSettingsPresentation {
    /// A centered modal dialog, independent of viewport-based presentation selection.
    Dialog,
    /// A bottom sheet with the settings content, navigation and footer.
    Sheet,
    /// Choose Sheet when either viewport axis meets its small-window threshold.
    #[default]
    Adaptive,
}

/// Adaptive sheet width threshold in logical pixels: 600, inclusive.
pub const SHEET_ADAPTIVE_WIDTH_THRESHOLD: f32 = 600.0;

/// Adaptive sheet height threshold in logical pixels: 500, inclusive.
pub const SHEET_ADAPTIVE_HEIGHT_THRESHOLD: f32 = 500.0;

/// Preferred sheet height as a fraction of viewport height: 0.85.
pub const SHEET_HEIGHT_FRACTION: f32 = 0.85;

const SHEET_MIN_HEIGHT: f32 = 200.0;

/// Choose Sheet at width <= 600 or height <= 500; otherwise choose Dialog.
/// Dimensions are in logical pixels. This returns a presentation choice without
/// constructing or laying out a surface.
#[must_use]
pub fn enz_settings_presentation_for(window: Size) -> EnzSettingsPresentation {
    if window.width <= SHEET_ADAPTIVE_WIDTH_THRESHOLD
        || window.height <= SHEET_ADAPTIVE_HEIGHT_THRESHOLD
    {
        EnzSettingsPresentation::Sheet
    } else {
        EnzSettingsPresentation::Dialog
    }
}

/// Resolve preferred sheet height from 85% of viewport height.
/// For finite nonnegative height, aim for at least 200 logical pixels while
/// never exceeding the supplied viewport height. A zero-height viewport yields zero.
/// Inputs are used directly rather than normalized.
#[must_use]
pub fn enz_settings_sheet_height(window: Size) -> f32 {
    (window.height * SHEET_HEIGHT_FRACTION)
        .max(SHEET_MIN_HEIGHT)
        .min(window.height)
}

impl DialogSize {
    const fn width_caps(self) -> (f32, f32, f32) {
        match self {
            Self::Compact => (360.0, 460.0, 460.0),
            Self::Small => (360.0, 520.0, 560.0),
            Self::Medium => (360.0, 560.0, 880.0),
            Self::Large => (360.0, 620.0, 1080.0),
        }
    }

    const fn height_caps(self) -> (f32, f32, f32) {
        match self {
            Self::Compact => (780.0, 560.0, 560.0),
            Self::Small => (780.0, 560.0, 560.0),
            Self::Medium => (780.0, 620.0, 680.0),
            Self::Large => (780.0, 680.0, 780.0),
        }
    }

    const fn height_caps_by_height(self) -> (f32, f32, f32) {
        match self {
            Self::Compact => (780.0, 560.0, 780.0),
            Self::Small => (780.0, 560.0, 780.0),
            Self::Medium => (780.0, 620.0, 800.0),
            Self::Large => (780.0, 680.0, 900.0),
        }
    }
}

/// Resolve `(gutter, surface width, surface height)` using [`DialogSize::Medium`].
/// See [`settings_dialog_geometry_sized`] for floors and viewport rungs.
#[must_use]
pub fn settings_dialog_geometry(window: Size) -> (f32, f32, f32) {
    settings_dialog_geometry_sized(window, DialogSize::default())
}

/// Resolve `(gutter, surface width, surface height)` in logical pixels.
/// Width rungs are <600, <960, and >=960, with gutters of 8, 24, and 32 px.
/// The height cap is the larger of the width- and height-based preset caps;
/// height rungs are <600, <=960, and >960.
///
/// Caps are limited by viewport-minus-two-gutters, floored at 240 px wide and
/// 280 px high. Tiny viewports can therefore receive a larger surface; the
/// caller must constrain or clip it. Width includes chrome, so account for
/// content insets before deciding whether navigation labels fit. Inputs are
/// used directly rather than normalized.
#[must_use]
pub fn settings_dialog_geometry_sized(window: Size, size: DialogSize) -> (f32, f32, f32) {
    let (wn, wm, ww) = size.width_caps();
    let (hn_w, hm_w, hw_w) = size.height_caps();
    let (gutter, cap_w, cap_h_by_width): (f32, f32, f32) = if window.width < 600.0 {
        (8.0, wn, hn_w)
    } else if window.width < 960.0 {
        (24.0, wm, hm_w)
    } else {
        (32.0, ww, hw_w)
    };
    let (hn_h, hm_h, ht_h) = size.height_caps_by_height();
    let cap_h_by_height: f32 = if window.height < 600.0 {
        hn_h
    } else if window.height <= 960.0 {
        hm_h
    } else {
        ht_h
    };
    // Honor room on either axis so a portrait viewport is not restricted to a desktop height cap.
    let cap_h = cap_h_by_width.max(cap_h_by_height);
    let avail_w = (window.width - 2.0 * gutter).max(240.0);
    let avail_h = (window.height - 2.0 * gutter).max(280.0);
    (gutter, cap_w.min(avail_w), cap_h.min(avail_h))
}
