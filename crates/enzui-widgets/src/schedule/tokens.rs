//! Palette-derived schedule drawing tokens and column emphasis.

use enzui_theme::Palette;
use iced::Color;

/// Target contrast between off-hours and working-hours surfaces.
///
/// The 1.2:1 target distinguishes background regions; it is not a text or control floor.
pub const OFF_HOURS_MIN_CONTRAST: f32 = 1.2;

/// Colours and weights for one schedule surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScheduleTokens {
    /// The grid's own background.
    pub background: Color,
    /// The header row background.
    pub header_background: Color,
    /// The all-day band background.
    pub all_day_background: Color,
    /// Major (hour) rule colour.
    pub rule: Color,
    /// Minor (half-hour) rule colour.
    pub minor_rule: Color,
    /// Major rule ink derived against the off-hours surface.
    pub off_hours_rule: Color,
    /// Minor (half-hour) rule colour inside the off-hours band.
    pub off_hours_minor_rule: Color,
    /// Vertical column-separator colour.
    pub column_rule: Color,
    /// Fill replacing the off-hours surface.
    pub off_hours: Color,
    /// Tint painted over today's whole column.
    pub today_column: Color,
    /// Selected-column body fill, replacing the today wash.
    pub selected_column: Color,
    /// Header-cell tint for the selected column — the "stronger header" half of
    /// the selection emphasis, painted over [`Self::header_background`].
    pub selected_header: Color,
    /// Fill of the live create-range preview: the rectangle drawn while the
    /// pointer is drawing a new event on empty grid.
    pub range_preview: Color,
    /// Outline of the live create-range preview.
    pub range_preview_border: Color,
    /// Ink of the time-range label inside the create-range preview, measured
    /// against [`Self::range_preview`].
    pub range_preview_ink: Color,
    /// The current-time line.
    pub now_line: Color,
    /// Primary day-number ink; event ink resolves separately from its fill.
    pub text: Color,
    /// Hour, weekday and all-day gutter-label ink; timed subtitles use translucent event ink.
    pub text_muted: Color,
    /// Fill behind today's day number.
    pub today_disc: Color,
    /// Ink on [`Self::today_disc`], chosen by measured contrast.
    pub today_disc_ink: Color,
    /// Legacy ring tone; current block drawing resolves rings from each event fill.
    pub selection_ring: Color,
    /// Selected timed-block ring width in logical pixels; all-day chips have no ring.
    pub selection_ring_width: f32,
    /// Block corner radius.
    pub block_radius: f32,
    /// Weekday header text size.
    pub header_text_size: f32,
    /// Day-number text size.
    pub day_number_size: f32,
    /// Hour-gutter label size.
    pub hour_label_size: f32,
    /// Block title size.
    pub block_title_size: f32,
    /// Block subtitle size.
    pub block_subtitle_size: f32,
    /// Now-line thickness.
    pub now_line_width: f32,
}

fn blend(under: Color, over: Color, alpha: f32) -> Color {
    let a = alpha.clamp(0.0, 1.0);
    Color {
        r: under.r + (over.r - under.r) * a,
        g: under.g + (over.g - under.g) * a,
        b: under.b + (over.b - under.b) * a,
        a: 1.0,
    }
}

fn recessed_band(surface: Color, floor: f32) -> Color {
    // Near-black surfaces may need a lighter band to reach the target.
    let target = if contrast_ratio(surface, Color::BLACK) >= floor {
        Color::BLACK
    } else {
        Color::WHITE
    };
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..16 {
        let mid = (low + high) * 0.5;
        if contrast_ratio(surface, blend(surface, target, mid)) >= floor {
            high = mid;
        } else {
            low = mid;
        }
    }
    blend(surface, target, high)
}

fn rule_matching_contrast(
    plane: Color,
    toward: Color,
    reference_plane: Color,
    reference: Color,
) -> Color {
    // Match measured contrast across planes; reusing a blend fraction changes the rule weight.
    let target = contrast_ratio(reference, reference_plane);
    let strongest = blend(plane, toward, 1.0);
    if contrast_ratio(strongest, plane) <= target {
        return strongest;
    }
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..16 {
        let mid = (low + high) * 0.5;
        if contrast_ratio(blend(plane, toward, mid), plane) >= target {
            high = mid;
        } else {
            low = mid;
        }
    }
    blend(plane, toward, high)
}

fn relative_luminance(c: Color) -> f32 {
    fn chan(v: f32) -> f32 {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * chan(c.r) + 0.7152 * chan(c.g) + 0.0722 * chan(c.b)
}

/// WCAG contrast ratio between two opaque colours.
pub fn contrast_ratio(a: Color, b: Color) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Resolve text ink for an event fill using the shared normal-text contrast floor.
///
/// Returns the resolver's best candidate on failure, so invalid or translucent
/// fills do not carry a guaranteed contrast result.
pub fn ink_for(fill: Color, palette: &Palette) -> Color {
    crate::tokens::ink_for_with_floor(fill, palette, enzui_theme::ContrastFloor::Text)
        .unwrap_or_else(crate::tokens::InkContrastFailure::best)
}

/// Target non-text contrast between a selected block ring and its fill.
pub const SELECTION_RING_MIN_CONTRAST: f32 = 3.0;

const SELECTION_RING_STRENGTHS: [f32; 4] = [0.55, 0.7, 0.85, 1.0];

/// Resolve a selected timed-block ring from its fill and text ink.
///
/// Prefers the weakest supported blend that reaches 3:1; falls back to the text
/// ink if none does. Blended candidates are opaque; invalid fill inputs or a
/// failing ink resolver do not guarantee the target.
pub fn selection_ring_for(fill: Color, palette: &Palette) -> Color {
    let ink = ink_for(fill, palette);
    for strength in SELECTION_RING_STRENGTHS {
        let candidate = blend(fill, ink, strength);
        if contrast_ratio(candidate, fill) >= SELECTION_RING_MIN_CONTRAST {
            return candidate;
        }
    }
    ink
}

impl ScheduleTokens {
    /// Derive colors and default dimensions from the palette.
    ///
    /// Explicitly pinned tokens are not normalized or contrast-checked by the widget.
    #[must_use]
    pub fn from_palette(p: &Palette) -> Self {
        let rule = blend(p.surface, p.border, 0.85);
        let minor_rule = blend(p.surface, p.border, 0.40);
        let off_hours_fill = recessed_band(p.surface, OFF_HOURS_MIN_CONTRAST);
        Self {
            background: p.surface,
            header_background: p.surface_alt,
            all_day_background: p.surface_alt,
            rule,
            minor_rule,
            column_rule: blend(p.surface, p.border, 0.95),
            off_hours: off_hours_fill,
            off_hours_rule: rule_matching_contrast(off_hours_fill, p.border, p.surface, rule),
            off_hours_minor_rule: rule_matching_contrast(
                off_hours_fill,
                p.border,
                p.surface,
                minor_rule,
            ),
            today_column: blend(p.surface, p.accent, 0.07),
            selected_column: blend(p.surface, p.accent, 0.16),
            selected_header: blend(p.surface_alt, p.accent, 0.18),
            range_preview: blend(p.surface, p.accent, 0.32),
            range_preview_border: blend(p.surface, p.accent, 0.9),
            range_preview_ink: ink_for(blend(p.surface, p.accent, 0.32), p),
            now_line: p.danger,
            text: p.text,
            text_muted: p.text_muted,
            // The accent, as the month grid, mini month and year view mark
            // today; the red is kept for the now line alone.
            today_disc: p.filled_color(p.accent).color,
            today_disc_ink: p.filled_color(p.accent).text,
            selection_ring: p.focus_ring,
            selection_ring_width: 1.5,
            block_radius: 4.0,
            header_text_size: enzui_theme::text_scale::ui_px(11.0),
            day_number_size: 17.0,
            hour_label_size: enzui_theme::text_scale::ui_px(11.0),
            block_title_size: enzui_theme::text_scale::ui_px(11.0),
            block_subtitle_size: enzui_theme::text_scale::ui_px(10.0),
            now_line_width: 1.5,
        }
    }
}

impl Default for ScheduleTokens {
    fn default() -> Self {
        Self::from_palette(&Palette::dark())
    }
}

/// Column treatment for independent today and selection flags.
///
/// Selection replaces the today body wash and adds a header wash. A today-only
/// column has a disc; a selected column has none. Today remains emphasized by
/// numeral weight in either state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnEmphasis {
    /// Neither today nor selected: no wash, plain day-number ink.
    None,
    /// Today only.
    Today,
    /// Selected only.
    Selected,
    /// Today and selected — the compose-both case.
    TodayAndSelected,
}

impl ColumnEmphasis {
    /// Resolve the emphasis from a column's two independent inputs.
    #[must_use]
    pub const fn resolve(is_today: bool, is_selected: bool) -> Self {
        match (is_today, is_selected) {
            (false, false) => Self::None,
            (true, false) => Self::Today,
            (false, true) => Self::Selected,
            (true, true) => Self::TodayAndSelected,
        }
    }

    /// Whether this column is selected in any way (selected, or today+selected).
    #[must_use]
    pub const fn is_selected(self) -> bool {
        matches!(self, Self::Selected | Self::TodayAndSelected)
    }

    /// Body wash, with selection replacing the today wash.
    #[must_use]
    pub const fn column_wash(self, t: &ScheduleTokens) -> Option<Color> {
        match self {
            Self::None => Option::None,
            Self::Today => Some(t.today_column),
            Self::Selected | Self::TodayAndSelected => Some(t.selected_column),
        }
    }

    /// The stronger header-cell tint for a selected column, if any.
    #[must_use]
    pub const fn header_wash(self, t: &ScheduleTokens) -> Option<Color> {
        match self {
            Self::Selected | Self::TodayAndSelected => Some(t.selected_header),
            _ => Option::None,
        }
    }

    /// Today disc fill; present only for a today-only column.
    #[must_use]
    pub const fn disc(self, t: &ScheduleTokens) -> Option<Color> {
        match self {
            Self::Today => Some(t.today_disc),
            Self::None | Self::Selected | Self::TodayAndSelected => Option::None,
        }
    }

    /// Whether the day numeral uses the today emphasis weight.
    #[must_use]
    pub const fn day_number_is_emphasised(self) -> bool {
        matches!(self, Self::Today | Self::TodayAndSelected)
    }

    /// Numeral ink: today-disc ink for today only, primary text otherwise.
    ///
    /// Header washes use the supplied primary text without a fresh contrast check.
    pub const fn day_number_ink(self, t: &ScheduleTokens) -> Color {
        match self {
            Self::None | Self::Selected | Self::TodayAndSelected => t.text,
            Self::Today => t.today_disc_ink,
        }
    }
}
