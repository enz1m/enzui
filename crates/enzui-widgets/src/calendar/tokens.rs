use super::*;

/// Resolved calendar colors; geometry is configured separately.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalendarTokens {
    /// Ordinary day-cell ink.
    pub text: Color,
    /// Ink for days outside the displayed month.
    pub adjacent: Color,
    /// Fallback ink for unavailable days without a state-specific filled surface.
    pub disabled: Color,
    /// Weekday column-header ink.
    pub header: Color,
    /// Month/year title ink.
    pub title: Color,
    /// Fill behind the selected day.
    pub selected_background: Color,
    /// Ink on top of [`Self::selected_background`].
    pub selected_foreground: Color,
    /// Fill behind days inside a selected range.
    pub range_background: Color,
    /// Fill behind a hovered day.
    pub hover_background: Color,
    /// Fill behind today when it is not selected.
    pub today_surface: Color,
    /// Ink on [`Self::today_surface`], chosen by measured contrast.
    pub today_foreground: Color,
    /// Fill behind a day that is both today and selected.
    pub combined_background: Color,
    /// Ink on [`Self::combined_background`], chosen by measured contrast.
    pub combined_foreground: Color,
    /// Focus ring.
    pub focus_ring: Color,
    /// Surface behind the whole calendar.
    pub surface: Color,
    /// Border around the surface.
    pub border: Color,
    /// Default host-data dot ink; filled days use their state-specific numeral ink.
    pub marker_dot: Color,
}

impl CalendarTokens {
    /// Resolve day colours from the runtime palette and its paired control fills.
    ///
    /// Muted date and weekday ink falls back to ordinary text below 4.5:1 on the calendar surface.
    #[must_use]
    pub fn from_palette(palette: &Palette) -> Self {
        let selected = palette.filled_color(palette.accent);
        let combined = palette.filled_color(palette.accent_pressed);
        let selected_background = selected.color;
        Self {
            text: palette.text,
            adjacent: palette.text_muted_on(palette.surface_alt),
            disabled: palette.disabled,
            header: palette.text_muted_on(palette.surface_alt),
            title: palette.text,
            selected_background,
            selected_foreground: selected.text,
            range_background: palette.accent_soft,
            // Composite over calendar chrome; a preset hover color may equal that chrome.
            hover_background: crate::surface_interaction::composite(
                palette.surface_alt,
                palette.overlay_hover(),
            ),
            today_surface: palette.control_border_strong,
            today_foreground: ink_for(palette.control_border_strong, palette),
            combined_background: combined.color,
            combined_foreground: combined.text,
            focus_ring: palette.focus_ring,
            surface: palette.surface_alt,
            border: crate::surface_edge::edge_color(
                palette.surface_alt,
                crate::surface_edge::DEFAULT_EDGE_DEPTH,
            ),
            marker_dot: palette.accent,
        }
    }

    /// Dark-theme tokens.
    #[must_use]
    pub fn dark() -> Self {
        Self::from_palette(&Palette::dark())
    }

    /// Light-theme tokens.
    #[must_use]
    pub fn light() -> Self {
        Self::from_palette(&Palette::light())
    }
}

fn ink_for(background: Color, palette: &Palette) -> Color {
    crate::tokens::ink_for(background, palette)
}

pub(crate) type Resolver = std::rc::Rc<dyn Fn(&iced::Theme) -> CalendarTokens>;

#[derive(Clone)]
pub(crate) enum TokenSource {
    // Boxing keeps resolver-backed variants from carrying the full color payload.
    Fixed(Box<CalendarTokens>),
    Resolved(Resolver),
}

pub(crate) fn resolve_tokens(
    pinned: Option<&TokenSource>,
    dimmed: bool,
    theme: &iced::Theme,
) -> CalendarTokens {
    let base = match pinned {
        Some(TokenSource::Fixed(tokens)) => **tokens,
        Some(TokenSource::Resolved(f)) => f(theme),
        None => crate::theme_lookup::resolve(theme, CalendarTokens::from_palette),
    };
    if dimmed {
        dim_fills(base, crate::theme_lookup::opacity(theme).disabled_surface())
    } else {
        base
    }
}

#[must_use]
pub(crate) fn calendar_surface_style(tokens: &CalendarTokens, flat: bool) -> container::Style {
    if flat {
        container::Style {
            background: None,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: SURFACE_RADIUS.into(),
            },
            ..container::Style::default()
        }
    } else {
        container::Style {
            background: Some(Background::Color(tokens.surface)),
            border: Border {
                color: tokens.border,
                width: 1.0,
                radius: SURFACE_RADIUS.into(),
            },
            ..container::Style::default()
        }
    }
}

#[must_use]
pub(crate) fn dim_fills(mut tokens: CalendarTokens, opacity: f32) -> CalendarTokens {
    for fill in [
        &mut tokens.selected_background,
        &mut tokens.range_background,
        &mut tokens.today_surface,
        &mut tokens.combined_background,
        &mut tokens.marker_dot,
    ] {
        *fill = crate::color::with_opacity(*fill, opacity);
    }
    tokens
}

/// Day emphasis derived from selection, range membership, today, and interaction.
///
/// Today remains semibold when selected. Range-middle backgrounds are painted
/// by the enclosing band; hover may paint a separate cell wash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DayEmphasis {
    /// Neither hovered nor stateful.
    Rest,
    /// Hovered (or pressed) and otherwise plain.
    Hover,
    /// Today, not selected.
    Today,
    /// Selected (including a range endpoint), not today.
    Selected,
    /// Today and selected.
    TodayAndSelected,
    /// Inside a selected range, but not an endpoint.
    RangeMiddle,
}

impl DayEmphasis {
    /// Resolves emphasis with today and selection taking precedence over range and hover.
    ///
    /// Disabled status suppresses hover but preserves value-bearing states.
    #[must_use]
    pub fn resolve(state: DayState, status: FocusStatus) -> Self {
        let selected_fill = state.selected
            || matches!(
                state.range,
                Some(RangeRole::Start | RangeRole::End | RangeRole::Single)
            );
        match (state.today, selected_fill) {
            (true, true) => Self::TodayAndSelected,
            (false, true) => Self::Selected,
            (true, false) => Self::Today,
            (false, false) => {
                if state.range.is_some() {
                    Self::RangeMiddle
                } else if matches!(status, FocusStatus::Hovered | FocusStatus::Pressed) {
                    Self::Hover
                } else {
                    Self::Rest
                }
            }
        }
    }

    /// Returns the fill associated with this emphasis, including the range-band fill.
    #[must_use]
    pub const fn fill(self, t: &CalendarTokens) -> Option<Color> {
        match self {
            Self::Rest => None,
            Self::Hover => Some(t.hover_background),
            Self::Today => Some(t.today_surface),
            Self::Selected => Some(t.selected_background),
            Self::TodayAndSelected => Some(t.combined_background),
            Self::RangeMiddle => Some(t.range_background),
        }
    }

    /// Returns state-specific ink for today and selected surfaces; other states use fallback ink.
    #[must_use]
    pub const fn ink(self, t: &CalendarTokens) -> Option<Color> {
        match self {
            Self::Rest | Self::Hover | Self::RangeMiddle => None,
            Self::Today => Some(t.today_foreground),
            Self::Selected => Some(t.selected_foreground),
            Self::TodayAndSelected => Some(t.combined_foreground),
        }
    }

    /// Reports whether the numeral marks today, including a selected today.
    #[must_use]
    pub const fn numeral_is_emphasised(self) -> bool {
        matches!(self, Self::Today | Self::TodayAndSelected)
    }
}

/// Day-cell dimensions and the gap inset from painted surfaces, in pixels.
///
/// `width = None` means fill-width layout has not resolved the inline edge;
/// pre-layout geometry uses painted height in that case.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellMetrics {
    /// Cell-box width in pixels, or none for unresolved fill-width layout.
    pub width: Option<f32>,
    /// Cell-box height in pixels.
    pub height: f32,
    /// Gap between ordinary day surfaces in pixels; calendar builders default to zero.
    pub spacing: f32,
}

impl CellMetrics {
    /// Creates square cell metrics with zero spacing.
    #[must_use]
    pub const fn square(edge: f32) -> Self {
        Self {
            width: Some(edge),
            height: edge,
            spacing: 0.0,
        }
    }

    /// Returns fixed cell width minus spacing, floored at zero.
    #[must_use]
    pub fn painted_width(self) -> Option<f32> {
        self.width.map(|w| (w - self.spacing).max(0.0))
    }

    /// Returns cell height minus spacing, floored at zero.
    #[must_use]
    pub fn painted_height(self) -> f32 {
        (self.height - self.spacing).max(0.0)
    }

    /// Returns the shortest known painted edge, using height alone for fill-width cells.
    #[must_use]
    pub fn short_painted_edge(self) -> f32 {
        match self.painted_width() {
            Some(w) => w.min(self.painted_height()),
            None => self.painted_height(),
        }
    }

    /// Returns horizontal and vertical insets, in pixels.
    ///
    /// Ordinary cells use half the spacing on each side; selected-range cells
    /// use zero horizontal inset so adjacent band surfaces meet.
    #[must_use]
    pub fn surface_inset(self, state: DayState) -> (f32, f32) {
        let half = (self.spacing / 2.0).max(0.0);
        let inline = if state.range.is_some() { 0.0 } else { half };
        (inline, half)
    }
}

#[must_use]
pub(crate) fn cell_radius(metrics: CellMetrics, _state: DayState) -> f32 {
    metrics.short_painted_edge() / 2.0
}

pub(crate) fn cell_ink(
    tokens: &CalendarTokens,
    cell: DayCell,
    state: DayState,
    available: bool,
) -> Color {
    // Value-specific ink takes precedence over unavailable-day fallback ink.
    if let Some(ink) = DayEmphasis::resolve(state, FocusStatus::Active).ink(tokens) {
        ink
    } else if !available {
        tokens.disabled
    } else if !cell.in_month {
        tokens.adjacent
    } else {
        tokens.text
    }
}

pub(crate) fn marker_dot_ink(tokens: &CalendarTokens, state: DayState, available: bool) -> Color {
    if let Some(ink) = DayEmphasis::resolve(state, FocusStatus::Active).ink(tokens) {
        ink
    } else if !available {
        tokens.disabled
    } else {
        tokens.marker_dot
    }
}

pub(crate) fn cell_style(
    tokens: &CalendarTokens,
    cell: DayCell,
    state: DayState,
    status: FocusStatus,
    metrics: CellMetrics,
) -> ButtonStyle {
    let available = !matches!(status, FocusStatus::Disabled);
    let radius = cell_radius(metrics, state);
    let emphasis = DayEmphasis::resolve(state, status);
    let background = match emphasis {
        DayEmphasis::RangeMiddle
            if matches!(status, FocusStatus::Hovered | FocusStatus::Pressed) =>
        {
            Some(tokens.hover_background)
        }
        DayEmphasis::RangeMiddle => None,
        _ => emphasis.fill(tokens),
    }
    .unwrap_or(Color::TRANSPARENT);

    let border = if matches!(status, FocusStatus::Focused) {
        Border {
            color: tokens.focus_ring,
            width: crate::focus_button::dense_focus_ring_width(),
            radius: radius.into(),
        }
    } else {
        Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: radius.into(),
        }
    };

    ButtonStyle {
        background: Some(Background::Color(background)),
        text_color: cell_ink(tokens, cell, state, available),
        border,
        ..ButtonStyle::default()
    }
}

pub(crate) fn focus_border(tokens: &CalendarTokens, status: FocusStatus, radius: f32) -> Border {
    if matches!(status, FocusStatus::Focused) {
        Border {
            color: tokens.focus_ring,
            width: crate::focus_button::dense_focus_ring_width(),
            radius: radius.into(),
        }
    } else {
        Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: radius.into(),
        }
    }
}
