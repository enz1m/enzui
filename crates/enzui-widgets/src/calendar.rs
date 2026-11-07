//! Controlled month calendar with keyboard navigation, date semantics, and event markers.
//!
//! Hosts own [`MonthController`], selection, and today's date; [`CalendarEvent`]
//! reports proposed changes, and [`apply_event`] updates the controller.
//! [`crate::date_picker`] and [`crate::date_range_picker`] compose this surface.
//!
//! The day grid has one Tab stop; header and chooser controls have their own.
//! Pin distinct [`EnzCalendar::id`] values for calendars sharing one screen.
//!
//! [`EnzCalendar::markers`] pairs host-data dots with accessible descriptions.
//! [`EnzCalendar::event_spans`] overlays up to two simultaneous event bands;
//! spans beyond that limit are omitted from paint and accessibility output.
//!
//! # Hazards
//!
//! Day cells smaller than
//! [`DEFAULT_MIN_HIT_TARGET`](crate::accessibility_metrics::DEFAULT_MIN_HIT_TARGET)
//! (24 px), such as sidebar mini-months, miss WCAG 2.2 SC 2.5.8. Cells tile with
//! no dead space, so their hit rects cannot grow without taking a neighbour's
//! clicks. Such calendars declare the deficit instead: each day's hit-target scan
//! probe sets `allow_small` from the shortest painted cell edge (height for
//! fill-width cells, after [`EnzCalendar::cell_spacing`]), and the framework floor
//! itself is unchanged. Keyboard roving still reaches every day through the
//! grid's single Tab stop.

use enzui_core::calendar_nav::{CalendarMove, MonthController, move_for_key};
use enzui_core::calendar_semantics::{
    CalendarLabels, DayMarker, DayState, column_header_semantics, day_cell_semantics, day_state,
    grid_semantics, month_nav_semantics, position_of_index,
};
use enzui_core::civil_date::{DAYS_PER_WEEK, Date, DateRange, DayCell, MONTH_CELLS, RangeRole};
use enzui_core::metrics::FinitePositive;
use enzui_core::placement::{self, Direction};
use enzui_theme::Palette;
use iced::widget::button::Style as ButtonStyle;
use iced::widget::{column, container, row, text};
use iced::{Background, Border, Color, Element, Font, Length, Padding};

use crate::focus_button::{FocusStatus, focus_button_maybe_with};

#[path = "calendar_spans.rs"]
mod spans;
mod tokens;

pub use spans::{
    CalendarCell, CalendarEventSpan, CalendarSpanSegment, band_bottom_inset, band_height, band_ink,
    band_label_fits, calendar_span_overlay, layout_calendar_spans, layout_calendar_spans_in,
};
pub(crate) use tokens::*;
pub use tokens::{CalendarTokens, CellMetrics, DayEmphasis};

/// Default day-cell edge length in pixels: 34.
pub const CELL_SIZE: f32 = 34.0;

/// Month-chooser cell corner radius in pixels; day highlights use cell geometry.
pub const CELL_RADIUS: f32 = 6.0;

/// Corner radius of the calendar surface in pixels.
pub const SURFACE_RADIUS: f32 = 10.0;

/// Header navigation control height in pixels.
pub const NAV_SIZE: f32 = 26.0;

/// Half the header control height, in pixels.
#[must_use]
pub const fn nav_pill_radius() -> f32 {
    NAV_SIZE / 2.0
}

/// Chevron glyph size in pixels.
pub const NAV_ICON_SIZE: f32 = 14.0;

/// Maximum marker-dot diameter in pixels; the enclosing day is the hit target.
pub const MARKER_DOT_SIZE: f32 = 4.0;

/// Scales the marker diameter with the cell edge, clamped to 2–4 pixels.
///
/// Nonpositive or nonfinite scaled values fall back to 4 pixels.
#[must_use]
pub fn marker_dot_diameter(cell_size: f32) -> f32 {
    let scaled = MARKER_DOT_SIZE * (cell_size / CELL_SIZE);
    FinitePositive::new_or(scaled, MARKER_DOT_SIZE)
        .get()
        .clamp(2.0, MARKER_DOT_SIZE)
}

/// Scales the marker diameter with the shortest painted edge known before layout.
#[must_use]
pub fn marker_dot_diameter_for(metrics: CellMetrics) -> f32 {
    marker_dot_diameter(metrics.short_painted_edge())
}

/// Registry name of the previous-month chevron.
pub const NAV_ICON_PREV: &str = "navigation.back";
/// Registry name of the next-month chevron.
pub const NAV_ICON_NEXT: &str = "navigation.forward";

/// Default stable namespace for a calendar without an explicit [`EnzCalendar::id`].
///
/// Multiple calendars sharing a screen must pin distinct ids to avoid collisions.
pub const DEFAULT_GRID_ID_PREFIX: &str = "enz-calendar";

/// Proposed controller or selection changes reported to the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarEvent {
    /// An in-month day was activated, or Enter/Space activated the cursor date.
    DaySelected(Date),
    /// Move the cursor according to the navigation command.
    Moved(CalendarMove),
    /// Focus an explicit date without selecting it, including a clicked padding day.
    Focused(Date),
    /// Page the visible month by the signed offset without moving the cursor.
    Paged(i32),
    /// Toggle the in-surface month/year chooser.
    HeaderPressed,
    /// Browse the chooser to an allowed month in another year, keeping it open.
    YearChanged(enzui_core::civil_date::YearMonth),
    /// Choose a month/year; [`apply_event`] closes the chooser only if the month changes.
    Jumped(enzui_core::civil_date::YearMonth),
}

/// A month surface over host-owned navigation and selection state.
pub struct EnzCalendar<'a, Message> {
    controller: &'a MonthController,
    selected: Option<Date>,
    range: Option<DateRange>,
    today: Option<Date>,
    labels: &'a dyn CalendarLabels,
    marker: Option<std::rc::Rc<dyn Fn(Date) -> DayMarker + 'a>>,
    spans: Vec<CalendarEventSpan>,
    on_event: std::rc::Rc<dyn Fn(CalendarEvent) -> Message + 'a>,
    on_day_context: Option<std::rc::Rc<dyn Fn(Date, iced::Point) -> Message + 'a>>,
    tokens: Option<TokenSource>,
    cell_size: f32,
    fill_width: bool,
    round_days: bool,
    cell_spacing: f32,
    flat: bool,
    enabled: bool,
    show_header: bool,
    direction: Option<Direction>,
    id: Option<iced::advanced::widget::Id>,
    focus_on_mount: bool,
}

/// Builds a calendar over a host-owned controller.
///
/// Defaults to labels in the current locale, 34-pixel square cells, zero spacing, a bordered
/// surface, and an enabled header. Selection, range, today, and markers are unset.
pub fn enz_calendar<'a, Message>(
    controller: &'a MonthController,
    on_event: impl Fn(CalendarEvent) -> Message + 'a,
) -> EnzCalendar<'a, Message> {
    EnzCalendar {
        controller,
        selected: None,
        range: None,
        today: None,
        labels: &enzui_core::localization::LocalizedCalendarLabels,
        marker: None,
        spans: Vec::new(),
        on_event: std::rc::Rc::new(on_event),
        on_day_context: None,
        tokens: None,
        cell_size: CELL_SIZE,
        fill_width: false,
        round_days: false,
        cell_spacing: 0.0,
        flat: false,
        enabled: true,
        show_header: true,
        direction: None,
        id: None,
        focus_on_mount: false,
    }
}

impl<'a, Message: Clone + 'a> EnzCalendar<'a, Message> {
    /// Sets the single selected date; defaults to none.
    #[must_use]
    pub const fn selected(mut self, selected: Option<Date>) -> Self {
        self.selected = selected;
        self
    }

    /// Sets the selected range independently of [`Self::selected`]; defaults to none.
    #[must_use]
    pub const fn range(mut self, range: Option<DateRange>) -> Self {
        self.range = range;
        self
    }

    /// Sets the date highlighted as today; defaults to none and never reads a clock.
    ///
    /// Today has a semibold numeral, including when it is also selected.
    #[must_use]
    pub const fn today(mut self, today: Option<Date>) -> Self {
        self.today = today;
        self
    }

    /// Locale labels. Defaults follow the current localizer.
    #[must_use]
    pub const fn labels(mut self, labels: &'a dyn CalendarLabels) -> Self {
        self.labels = labels;
        self
    }

    /// Adds host-data markers to day paint and accessible descriptions.
    ///
    /// Called once for each of the 42 grid dates during element construction,
    /// including padding dates and even when the chooser hides the day grid.
    /// Use indexed data for lookup. [`DayMarker::None`] adds no marker.
    #[must_use]
    pub fn markers(mut self, marker: impl Fn(Date) -> DayMarker + 'a) -> Self {
        self.marker = Some(std::rc::Rc::new(marker));
        self
    }

    /// Adds event-presence markers through [`Self::markers`].
    ///
    /// A true result announces event presence without inventing an event count.
    #[must_use]
    pub fn event_days(self, has_events: impl Fn(Date) -> bool + 'a) -> Self {
        self.markers(move |date| DayMarker::present(has_events(date)))
    }

    /// Replaces the inclusive all-day event spans; defaults to none.
    ///
    /// Spans are clipped to the 42-day grid and split at week boundaries. At most
    /// two overlapping rows are painted and announced; excess spans are omitted.
    /// Each displayed logical span has one accessible node, with its full range.
    /// Bands follow the day grid: mirrored with it in right-to-left text.
    #[must_use]
    pub fn event_spans(mut self, spans: impl IntoIterator<Item = CalendarEventSpan>) -> Self {
        self.spans = spans.into_iter().collect();
        self
    }

    /// Pins the calendar token colors, except the grid owner’s runtime-theme focus ring.
    ///
    /// Disabled calendars still apply the runtime theme’s disabled-surface opacity.
    #[must_use]
    pub fn tokens(mut self, tokens: CalendarTokens) -> Self {
        self.tokens = Some(TokenSource::Fixed(Box::new(tokens)));
        self
    }

    /// Resolves calendar token colors from the runtime theme.
    ///
    /// The grid owner’s focus ring uses the ambient palette independently of this resolver.
    #[must_use]
    pub fn tokens_resolved(
        mut self,
        resolver: impl Fn(&iced::Theme) -> CalendarTokens + 'static,
    ) -> Self {
        self.tokens = Some(TokenSource::Resolved(std::rc::Rc::new(resolver)));
        self
    }

    /// Day-cell edge length, or height in fill-width mode, in pixels.
    ///
    /// Defaults to 34; nonpositive or nonfinite inputs fall back to 34. Cells below
    /// the framework hit-target floor miss WCAG 2.2 SC 2.5.8 and are declared as
    /// small targets, as reported by [`Self::cell_size_is_below_the_hit_floor`];
    /// see the module header's hazard section.
    #[must_use]
    pub fn cell_size(mut self, size: f32) -> Self {
        self.cell_size = FinitePositive::new_or(size, CELL_SIZE).get();
        self
    }

    /// Makes each day fill its column; defaults to false.
    ///
    /// [`Self::cell_size`] still controls height, and geometry known before layout
    /// uses that height for marker sizing and small-target declarations.
    #[must_use]
    pub const fn fill_width(mut self, fill: bool) -> Self {
        self.fill_width = fill;
        self
    }

    /// Paints each day of a [`Self::fill_width`] grid as a circle centred in its
    /// column instead of a pill as wide as the column; defaults to false.
    ///
    /// Days inside a selected range still span their column so the range reads
    /// as one band. Without fill width the cells are already square.
    #[must_use]
    pub const fn round_days(mut self, round: bool) -> Self {
        self.round_days = round;
        self
    }

    /// Sets the gap between painted day surfaces in pixels; defaults to zero.
    ///
    /// Nonpositive or nonfinite inputs become zero. Gaps inset surfaces without
    /// changing grid pitch; selected-range cells retain zero horizontal inset.
    #[must_use]
    pub fn cell_spacing(mut self, spacing: f32) -> Self {
        self.cell_spacing = if spacing.is_finite() && spacing > 0.0 {
            spacing
        } else {
            0.0
        };
        self
    }

    /// Removes the outer surface fill and border; defaults to false.
    ///
    /// Day highlights, hover feedback, and focus styling remain active.
    #[must_use]
    pub const fn flat(mut self, flat: bool) -> Self {
        self.flat = flat;
        self
    }

    /// Reports a day and window-space point for a secondary press.
    ///
    /// Available days publish a context-menu action. Menu and Shift+F10 report
    /// the cursor date at its cell center when cell geometry is available; without
    /// a hook, these keys bubble. The host owns the menu contents.
    #[must_use]
    pub fn on_day_context(mut self, f: impl Fn(Date, iced::Point) -> Message + 'a) -> Self {
        self.on_day_context = Some(std::rc::Rc::new(f));
        self
    }

    /// Returns the configured cell dimensions and surface spacing before layout.
    #[must_use]
    pub const fn cell_metrics(&self) -> CellMetrics {
        CellMetrics {
            width: if self.fill_width {
                None
            } else {
                Some(self.cell_size)
            },
            height: self.cell_size,
            spacing: self.cell_spacing,
        }
    }

    /// Tests the shortest known painted edge against the framework hit-target floor.
    ///
    /// Fill-width calendars use painted height because width is unresolved. A true
    /// result declares a small-target exception on the day-cell scan probes.
    #[must_use]
    pub fn cell_size_is_below_the_hit_floor(&self) -> bool {
        self.cell_metrics().short_painted_edge()
            < crate::accessibility_metrics::DEFAULT_MIN_HIT_TARGET
    }

    /// Enables calendar interaction; defaults to true.
    ///
    /// Disabling dims value-bearing fills and hides an open month/year chooser.
    #[must_use]
    pub const fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Shows the month/year header and arrows; defaults to true.
    ///
    /// Hiding the header also hides the chooser, regardless of controller state.
    #[must_use]
    pub const fn show_header(mut self, show: bool) -> Self {
        self.show_header = show;
        self
    }

    /// Pins the day grid and weekday headers to right-to-left or left-to-right layout.
    ///
    /// Horizontal cursor arrows follow the pinned direction; week order is unchanged.
    /// Without an override, the calendar follows the ambient direction.
    #[must_use]
    pub const fn rtl(mut self, rtl: bool) -> Self {
        self.direction = Some(if rtl {
            Direction::RightToLeft
        } else {
            Direction::LeftToRight
        });
        self
    }

    /// Pins the writing direction instead of following the ambient direction.
    #[must_use]
    pub const fn direction(mut self, direction: Direction) -> Self {
        self.direction = Some(direction);
        self
    }

    /// Pins the stable owner identity; defaults to [`DEFAULT_GRID_ID_PREFIX`].
    ///
    /// Day ids derive from dates; header and chooser ids derive from action or month
    /// keys. Use distinct owner ids for calendars sharing a screen.
    #[must_use]
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Focuses the day-grid owner on its first mount; defaults to false.
    ///
    /// Retained widget state prevents repeated focus stealing on rebuild.
    #[must_use]
    pub const fn focus_on_mount(mut self, focus: bool) -> Self {
        self.focus_on_mount = focus;
        self
    }

    // Paint and semantics share this result so a stateful marker callback cannot disagree.
    fn state_of(&self, cell: DayCell) -> DayState {
        let state = day_state(
            cell.date,
            Some(self.controller.focused()),
            self.selected,
            self.range,
            self.today,
            cell.in_month,
        );
        match &self.marker {
            Some(f) => state.with_marker(f(cell.date)),
            None => state,
        }
    }

    fn cell_is_available(&self, cell: DayCell) -> bool {
        self.enabled && cell.selectable
    }

    fn picker_is_open(&self) -> bool {
        self.enabled && self.show_header && self.controller.picker_open()
    }

    /// Resolves the outer surface style from token overrides, enabled state, and flatness.
    #[must_use]
    pub fn resolved_surface_style(&self, theme: &iced::Theme) -> iced::widget::container::Style {
        let tokens = resolve_tokens(self.tokens.as_ref(), !self.enabled, theme);
        calendar_surface_style(&tokens, self.flat)
    }

    /// Reports whether outer surface fill and border are disabled.
    #[must_use]
    pub const fn is_flat(&self) -> bool {
        self.flat
    }

    /// Builds the widget tree and resolves all 42 day markers, including padding dates.
    pub fn into_element(self) -> Element<'a, Message> {
        let pinned = self.tokens.clone();
        let dimmed = !self.enabled;
        let resolve: Resolver = std::rc::Rc::new(move |theme: &iced::Theme| -> CalendarTokens {
            resolve_tokens(pinned.as_ref(), dimmed, theme)
        });

        let on_event = std::rc::Rc::clone(&self.on_event);
        let rtl = matches!(
            placement::resolve_direction(self.direction),
            Direction::RightToLeft
        );
        let enabled_wrapper = self.enabled;
        let focused_date = self.controller.focused();
        let grid_id = self
            .id
            .clone()
            .unwrap_or_else(|| iced::advanced::widget::Id::new(DEFAULT_GRID_ID_PREFIX));
        let id_namespace = format!("{grid_id:?}");
        let focus_on_mount = self.focus_on_mount;

        let grid = self.controller.grid();
        let cell_size = self.cell_size;
        let metrics = self.cell_metrics();
        let allow_small_cells = self.cell_size_is_below_the_hit_floor();
        let on_day_context = self.on_day_context.clone();
        let mut cursor_inset = (0.0_f32, 0.0_f32);
        let mut cursor_round = false;

        let picker_open = self.picker_is_open();
        let cursor_index: Option<usize> = (!picker_open)
            .then(|| {
                grid.cells()
                    .iter()
                    .position(|c| c.date == focused_date && (self.enabled && c.selectable))
            })
            .flatten();
        let has_header = self.show_header;
        let mut rows = Vec::with_capacity(MONTH_CELLS / DAYS_PER_WEEK);

        for row_index in 0..MONTH_CELLS / DAYS_PER_WEEK {
            let mut cells: Vec<Element<'a, Message>> = Vec::with_capacity(DAYS_PER_WEEK);
            for column in 0..DAYS_PER_WEEK {
                let index = row_index * DAYS_PER_WEEK + column;
                let cell = grid.cells()[index];
                let state = self.state_of(cell);
                let available = self.cell_is_available(cell);

                let message: Option<Box<dyn Fn() -> Message + 'a>> = available.then(|| {
                    let on_event = std::rc::Rc::clone(&on_event);
                    let factory: Box<dyn Fn() -> Message + 'a> = Box::new(move || {
                        on_event(if cell.in_month {
                            CalendarEvent::DaySelected(cell.date)
                        } else {
                            CalendarEvent::Focused(cell.date)
                        })
                    });
                    factory
                });

                let label = text(self.labels.day_number(cell.date.day()))
                    .size(enzui_theme::text_scale::ui_px(13.0))
                    .font(Font {
                        weight: if DayEmphasis::resolve(state, FocusStatus::Active)
                            .numeral_is_emphasised()
                        {
                            enzui_theme::text_defaults::weight(enzui_theme::WeightRung::Semibold)
                        } else {
                            enzui_theme::text_defaults::weight(enzui_theme::WeightRung::Regular)
                        },
                        // Day numbers line up in their columns.
                        numeric: iced::font::Numeric::Tabular,
                        ..enzui_theme::text_defaults::font(
                            enzui_theme::FontRole::Ui,
                            enzui_theme::WeightRung::Regular,
                        )
                    })
                    .style({
                        let resolve = std::rc::Rc::clone(&resolve);
                        move |theme: &iced::Theme| {
                            let t = resolve(theme);
                            iced::widget::text::Style {
                                color: Some(cell_ink(&t, cell, state, available)),
                            }
                        }
                    });

                let day_id = enzui_core::identity::derive_id(
                    &id_namespace,
                    &format!("day-{}", cell.date.to_days()),
                );
                let body: Element<'a, Message> = {
                    let dot_size = marker_dot_diameter_for(metrics);
                    let dot_slot: Element<'a, Message> = if state.marker.is_present() {
                        container(iced::widget::Space::new().width(dot_size).height(dot_size))
                            .style({
                                let resolve = std::rc::Rc::clone(&resolve);
                                move |theme: &iced::Theme| container::Style {
                                    background: Some(Background::Color(marker_dot_ink(
                                        &resolve(theme),
                                        state,
                                        available,
                                    ))),
                                    border: Border {
                                        radius: (dot_size / 2.0).into(),
                                        ..Border::default()
                                    },
                                    ..container::Style::default()
                                }
                            })
                            .into()
                    } else {
                        iced::widget::Space::new()
                            .width(dot_size)
                            .height(dot_size)
                            .into()
                    };
                    column![iced::widget::Space::new().height(dot_size), label, dot_slot]
                        .spacing(1.0)
                        .align_x(iced::Alignment::Center)
                        .into()
                };
                let (inset_x, inset_y) = metrics.surface_inset(state);
                let round = self.round_days && metrics.width.is_none() && state.range.is_none();
                let surface_w: Length = match metrics.width {
                    Some(w) => Length::Fixed((w - 2.0 * inset_x).max(0.0)),
                    None if round => Length::Fixed((metrics.height - 2.0 * inset_y).max(0.0)),
                    None => Length::Fill,
                };
                let surface_h = Length::Fixed((metrics.height - 2.0 * inset_y).max(0.0));
                if Some(index) == cursor_index {
                    cursor_inset = (inset_x, inset_y);
                    cursor_round = round;
                }
                let button = focus_button_maybe_with(
                    container(body)
                        .width(surface_w)
                        .height(surface_h)
                        .center_x(surface_w)
                        .center_y(surface_h),
                    message,
                )
                .width(surface_w)
                .height(surface_h)
                .id(day_id)
                .embedded(true)
                .semantics(day_cell_semantics(
                    cell,
                    state,
                    {
                        let mut pos = position_of_index(index);
                        if rtl {
                            pos.column = DAYS_PER_WEEK - 1 - pos.column;
                        }
                        pos
                    },
                    self.labels,
                ))
                .style_focus({
                    let resolve = std::rc::Rc::clone(&resolve);
                    move |theme: &iced::Theme, status: FocusStatus| {
                        cell_style(&resolve(theme), cell, state, status, metrics)
                    }
                });

                let day_surface: Element<'a, Message> = match &on_day_context {
                    Some(f) => {
                        let f = std::rc::Rc::clone(f);
                        let date = cell.date;
                        let context_id = enzui_core::identity::derive_id(
                            &id_namespace,
                            &format!("day-context-{}", cell.date.to_days()),
                        );
                        crate::context_target::enz_context_target(
                            Element::from(button),
                            move |at| f(date, at),
                        )
                        .width(surface_w)
                        .height(surface_h)
                        .enabled(available)
                        .id(context_id)
                        .label(self.labels.full_date(cell.date))
                        .into()
                    }
                    None => Element::from(button),
                };

                let day_surface: Element<'a, Message> = if let Some(role) = state.range {
                    let round_start =
                        column == 0 || matches!(role, RangeRole::Start | RangeRole::Single);
                    let round_end = column == DAYS_PER_WEEK - 1
                        || matches!(role, RangeRole::End | RangeRole::Single);
                    let (left, right) = if rtl {
                        (round_end, round_start)
                    } else {
                        (round_start, round_end)
                    };
                    let radius = metrics.short_painted_edge() / 2.0;
                    container(day_surface)
                        .width(surface_w)
                        .height(surface_h)
                        .style({
                            let resolve = std::rc::Rc::clone(&resolve);
                            move |theme: &iced::Theme| container::Style {
                                background: Some(resolve(theme).range_background.into()),
                                border: Border {
                                    radius: iced::border::Radius {
                                        top_left: if left { radius } else { 0.0 },
                                        bottom_left: if left { radius } else { 0.0 },
                                        top_right: if right { radius } else { 0.0 },
                                        bottom_right: if right { radius } else { 0.0 },
                                    },
                                    ..Default::default()
                                },
                                ..Default::default()
                            }
                        })
                        .into()
                } else {
                    day_surface
                };

                let audited: Element<'a, Message> =
                    crate::accessibility_scan_widgets::HitTargetScanProbe::new(day_surface)
                        .allow_small(allow_small_cells)
                        .into();
                let boxed: Element<'a, Message> = if round {
                    container(audited)
                        .padding(Padding {
                            top: inset_y,
                            bottom: inset_y,
                            left: 0.0,
                            right: 0.0,
                        })
                        .center_x(Length::Fill)
                        .into()
                } else if inset_x > 0.0 || inset_y > 0.0 {
                    container(audited)
                        .padding(Padding {
                            top: inset_y,
                            bottom: inset_y,
                            left: inset_x,
                            right: inset_x,
                        })
                        .into()
                } else {
                    audited
                };
                cells.push(boxed);
            }
            if rtl {
                cells.reverse();
            }
            rows.push(
                crate::semantics_container::SemanticsContainer::new(
                    row(cells),
                    enzui_core::Semantics::new(enzui_core::semantics::Role::GridRow),
                )
                .into(),
            );
        }

        let mut body: Vec<Element<'a, Message>> = Vec::new();

        if self.show_header {
            body.push(self.header(std::rc::Rc::clone(&resolve), rtl, &id_namespace));
        }
        if self.picker_is_open() {
            body.push(self.month_year_chooser(std::rc::Rc::clone(&resolve), &id_namespace));
        } else {
            body.push(self.weekday_header(std::rc::Rc::clone(&resolve), rtl));
            let rows: Element<'a, Message> = column(rows).into();
            if self.spans.is_empty() {
                body.push(rows);
            } else {
                let first_visible = grid.cells()[0].date;
                let announcements = self
                    .spans
                    .iter()
                    .map(|span| {
                        let start = self.labels.full_date(span.start);
                        let end = self.labels.full_date(span.end);
                        if span.start == span.end {
                            enzui_core::localized!("enzui-calendar-all-day", "title" => span.title, "date" => start)
                        } else {
                            enzui_core::localized!("enzui-calendar-all-day-alternate", "title" => span.title, "start" => start, "end" => end)
                        }
                    })
                    .collect();
                let layer: Element<'a, Message> = spans::CalendarSpanLayer {
                    spans: self.spans.clone(),
                    first_visible,
                    sizing: if metrics.width.is_some() {
                        spans::SpanLayerSizing::Square(cell_size)
                    } else {
                        spans::SpanLayerSizing::Fill
                    },
                    announcements,
                    mirrored: rtl,
                }
                .into();
                body.push(iced::widget::stack![rows, layer].into());
            }
        }

        let month = self.controller.visible_month();
        let labels = self.labels;
        let focused_day_id = enzui_core::identity::derive_id(
            &id_namespace,
            &format!("day-{}", focused_date.to_days()),
        );
        let semantics = grid_semantics(month, labels)
            .description(labels.full_date(focused_date))
            .active_descendant(focused_day_id);

        let surface_width = if metrics.width.is_some() {
            Length::Shrink
        } else {
            Length::Fill
        };
        let flat = self.flat;
        let grid_element: Element<'a, Message> =
            container(column(body).spacing(4.0).width(surface_width))
                .width(surface_width)
                .padding(Padding::from(8.0))
                .clip(true)
                .style(move |theme: &iced::Theme| calendar_surface_style(&resolve(theme), flat))
                .into();

        let cursor_bounds = move |layout: iced::advanced::Layout,
                                  tree: &iced::advanced::widget::Tree|
              -> Option<iced::Rectangle> {
            let index = cursor_index?;
            let (r, c) = (index / DAYS_PER_WEEK, index % DAYS_PER_WEEK);
            let visual_c = if rtl { DAYS_PER_WEEK - 1 - c } else { c };
            let (body, body_tree) = layout.iter(&tree.children).next()?;
            let n_before = if has_header { 2 } else { 1 };
            let (rows_col, rows_tree) = body.iter(&body_tree.children).nth(n_before)?;
            let (row_node, row_tree) = rows_col.iter(&rows_tree.children).nth(r)?;
            let cell_node = row_node.children(&row_tree.children).nth(visual_c)?;
            let (ix, iy) = cursor_inset;
            let b = cell_node.bounds();
            if cursor_round {
                let side = (b.height - 2.0 * iy).max(0.0).min(b.width);
                return Some(iced::Rectangle {
                    x: b.x + (b.width - side) / 2.0,
                    y: b.y + iy,
                    width: side,
                    height: side,
                });
            }
            Some(iced::Rectangle {
                x: b.x + ix,
                y: b.y + iy,
                width: (b.width - 2.0 * ix).max(0.0),
                height: (b.height - 2.0 * iy).max(0.0),
            })
        };

        let ring_radius = cell_radius(metrics, DayState::default());
        let ring = move |theme: &iced::Theme| -> crate::keyboard_capture::RovingRing {
            let t = crate::theme_lookup::resolve(theme, CalendarTokens::from_palette);
            crate::keyboard_capture::RovingRing {
                color: t.focus_ring,
                width: crate::focus_button::dense_focus_ring_width(),
                radius: ring_radius,
            }
        };

        let owner_enabled = enabled_wrapper && !picker_open;
        crate::keyboard_capture::roving_grid(
            grid_element,
            owner_enabled,
            crate::keyboard_capture::RovingGridOwner::new(
                grid_id,
                semantics,
                focus_on_mount && !picker_open,
            ),
            cursor_bounds,
            ring,
            move |key, modifiers, cursor_rect| {
                if picker_open {
                    return None;
                }
                if let Some(f) = &on_day_context
                    && enzui_core::context_menu::key_opens_context_menu(key, modifiers)
                {
                    return cursor_rect.map(|rect| f(focused_date, rect.center()));
                }
                event_for_key(key, modifiers, rtl, focused_date).map(|ev| on_event(ev))
            },
        )
    }

    fn header(&self, resolve: Resolver, rtl: bool, id_namespace: &str) -> Element<'a, Message> {
        let month = self.controller.visible_month();
        let bounds = self.controller.bounds();
        let title_available = self.enabled;
        let chooser_open = self.controller.picker_open();
        let title = text(self.labels.month_year(month))
            .size(enzui_theme::text_scale::ui_px(14.0))
            .wrapping(iced::widget::text::Wrapping::None)
            .ellipsis(iced::widget::text::Ellipsis::End)
            .font(enzui_theme::text_defaults::font(
                enzui_theme::FontRole::Ui,
                enzui_theme::WeightRung::Semibold,
            ))
            .style({
                let resolve = std::rc::Rc::clone(&resolve);
                move |theme: &iced::Theme| iced::widget::text::Style {
                    color: Some(if title_available {
                        resolve(theme).title
                    } else {
                        resolve(theme).disabled
                    }),
                }
            });
        let title_message: Option<Box<dyn Fn() -> Message + 'a>> = title_available.then(|| {
            let on_event = std::rc::Rc::clone(&self.on_event);
            let factory: Box<dyn Fn() -> Message + 'a> =
                Box::new(move || on_event(CalendarEvent::HeaderPressed));
            factory
        });
        let title_id = enzui_core::identity::derive_id(id_namespace, "month-year-header");
        let title_semantics = enzui_core::Semantics::new(enzui_core::semantics::Role::Button)
            .label(self.labels.month_year(month))
            .expanded(chooser_open)
            .default_action()
            .disabled(!title_available);
        let title = focus_button_maybe_with(
            container(title)
                .center_x(Length::Fill)
                .center_y(NAV_SIZE)
                .padding(Padding::from([0.0, 6.0])),
            title_message,
        )
        // Expand/Collapse actions require the routed carrier alias to reach activation.
        .expand_alias(title_id.clone())
        .id(title_id)
        .semantics(title_semantics)
        .style_focus({
            let resolve = std::rc::Rc::clone(&resolve);
            move |theme: &iced::Theme, status: FocusStatus| {
                let t = resolve(theme);
                ButtonStyle {
                    background: Some(Background::Color(match status {
                        FocusStatus::Hovered | FocusStatus::Pressed => t.hover_background,
                        _ => Color::TRANSPARENT,
                    })),
                    text_color: if title_available { t.title } else { t.disabled },
                    border: focus_border(&t, status, nav_pill_radius()),
                    ..ButtonStyle::default()
                }
            }
        });

        let arrow = |months: i32, icon: &'static str| -> Element<'a, Message> {
            let resolve = std::rc::Rc::clone(&resolve);
            let on_event = std::rc::Rc::clone(&self.on_event);
            let available = self.enabled && self.controller.can_page_months(months);
            let glyph = crate::icon::enz_icon(icon)
                .size(NAV_ICON_SIZE)
                .tint_resolved({
                    let resolve = std::rc::Rc::clone(&resolve);
                    move |theme: &iced::Theme| {
                        let t = resolve(theme);
                        if available { t.title } else { t.disabled }
                    }
                });
            let message: Option<Box<dyn Fn() -> Message + 'a>> = available.then(|| {
                let on_event = std::rc::Rc::clone(&on_event);
                let factory: Box<dyn Fn() -> Message + 'a> =
                    Box::new(move || on_event(CalendarEvent::Paged(months)));
                factory
            });
            let id = enzui_core::identity::derive_id(
                id_namespace,
                if months < 0 {
                    "month-prev"
                } else {
                    "month-next"
                },
            );
            focus_button_maybe_with(
                container(glyph.into_element::<Message>())
                    .width(NAV_SIZE)
                    .height(NAV_SIZE)
                    .center_x(NAV_SIZE)
                    .center_y(NAV_SIZE),
                message,
            )
            .id(id)
            .semantics(month_nav_semantics(month, months, bounds, self.labels))
            .style_focus(move |theme: &iced::Theme, status: FocusStatus| {
                let t = resolve(theme);
                ButtonStyle {
                    background: Some(Background::Color(match status {
                        FocusStatus::Hovered | FocusStatus::Pressed => t.hover_background,
                        _ => Color::TRANSPARENT,
                    })),
                    text_color: if available { t.title } else { t.disabled },
                    border: focus_border(&t, status, nav_pill_radius()),
                    ..ButtonStyle::default()
                }
            })
            .into()
        };

        let grid_width = self.grid_width();
        let (left_arrow, right_arrow) = if rtl {
            (arrow(1, NAV_ICON_NEXT), arrow(-1, NAV_ICON_PREV))
        } else {
            (arrow(-1, NAV_ICON_PREV), arrow(1, NAV_ICON_NEXT))
        };
        container(
            row![
                left_arrow,
                container(title)
                    .width(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(NAV_SIZE),
                right_arrow,
            ]
            .align_y(iced::Alignment::Center),
        )
        .width(grid_width)
        .into()
    }

    fn month_year_chooser(&self, resolve: Resolver, id_namespace: &str) -> Element<'a, Message> {
        use enzui_core::civil_date::YearMonth;
        let visible = self.controller.visible_month();
        let year = visible.year();
        let grid_width = self.grid_width();

        let year_step = |delta: i32, icon: &'static str| -> Element<'a, Message> {
            let resolve = std::rc::Rc::clone(&resolve);
            let on_event = std::rc::Rc::clone(&self.on_event);
            let target = self.controller.year_step_target(delta);
            let available = self.enabled && target.is_some();
            let glyph = crate::icon::enz_icon(icon)
                .size(NAV_ICON_SIZE)
                .tint_resolved({
                    let resolve = std::rc::Rc::clone(&resolve);
                    move |theme: &iced::Theme| {
                        let t = resolve(theme);
                        if available { t.title } else { t.disabled }
                    }
                });
            let message: Option<Box<dyn Fn() -> Message + 'a>> =
                target.filter(|_| self.enabled).map(|target| {
                    let on_event = std::rc::Rc::clone(&on_event);
                    let factory: Box<dyn Fn() -> Message + 'a> =
                        Box::new(move || on_event(CalendarEvent::YearChanged(target)));
                    factory
                });
            let id = enzui_core::identity::derive_id(
                id_namespace,
                if delta < 0 { "year-prev" } else { "year-next" },
            );
            focus_button_maybe_with(
                container(glyph.into_element::<Message>())
                    .width(NAV_SIZE)
                    .height(NAV_SIZE)
                    .center_x(NAV_SIZE)
                    .center_y(NAV_SIZE),
                message,
            )
            .id(id)
            .semantics(
                enzui_core::Semantics::new(enzui_core::semantics::Role::Button)
                    .label(if delta < 0 {
                        enzui_core::localized!("enzui-calendar-year", "year" => year - 1)
                    } else {
                        enzui_core::localized!("enzui-calendar-year", "year" => year + 1)
                    })
                    .default_action()
                    .disabled(!available),
            )
            .style_focus({
                let resolve = std::rc::Rc::clone(&resolve);
                move |theme: &iced::Theme, status: FocusStatus| {
                    let t = resolve(theme);
                    ButtonStyle {
                        background: Some(Background::Color(match status {
                            FocusStatus::Hovered | FocusStatus::Pressed => t.hover_background,
                            _ => Color::TRANSPARENT,
                        })),
                        text_color: if available { t.title } else { t.disabled },
                        border: focus_border(&t, status, nav_pill_radius()),
                        ..ButtonStyle::default()
                    }
                }
            })
            .into()
        };

        let year_label = text(self.labels.year_number(year))
            .size(enzui_theme::text_scale::ui_px(14.0))
            .font(enzui_theme::text_defaults::font(
                enzui_theme::FontRole::Ui,
                enzui_theme::WeightRung::Semibold,
            ))
            .style({
                let resolve = std::rc::Rc::clone(&resolve);
                move |theme: &iced::Theme| iced::widget::text::Style {
                    color: Some(resolve(theme).title),
                }
            });

        let year_row = container(
            row![
                year_step(-1, NAV_ICON_PREV),
                container(year_label)
                    .width(Length::Fill)
                    .center_x(Length::Fill)
                    .center_y(NAV_SIZE),
                year_step(1, NAV_ICON_NEXT),
            ]
            .align_y(iced::Alignment::Center),
        )
        .width(grid_width);

        const MONTH_GAP: f32 = 2.0;
        let month_cell_w: Length = match grid_width {
            Length::Fixed(w) => Length::Fixed((w - 2.0 * MONTH_GAP) / 3.0),
            _ => Length::Fill,
        };
        let month_cell_h = self.cell_size + 6.0;
        let mut month_rows: Vec<Element<'a, Message>> = Vec::with_capacity(4);
        for r in 0..4u32 {
            let mut cells: Vec<Element<'a, Message>> = Vec::with_capacity(3);
            for c in 0..3u32 {
                let month = r * 3 + c + 1;
                let target = YearMonth::new(year, month);
                let in_bounds = target.is_some_and(|t| self.controller.month_in_bounds(t));
                let available = self.enabled && in_bounds;
                let label_text = self.labels.month_short(month);
                let label = text(label_text.clone())
                    .size(enzui_theme::text_scale::ui_px(13.0))
                    .font(enzui_theme::text_defaults::font(
                        enzui_theme::FontRole::Ui,
                        enzui_theme::WeightRung::Regular,
                    ));
                let message: Option<Box<dyn Fn() -> Message + 'a>> =
                    (available && target.is_some()).then(|| {
                        let on_event = std::rc::Rc::clone(&self.on_event);
                        let target = target.expect("guarded by is_some above");
                        let factory: Box<dyn Fn() -> Message + 'a> =
                            Box::new(move || on_event(CalendarEvent::Jumped(target)));
                        factory
                    });
                let id = enzui_core::identity::derive_id(
                    id_namespace,
                    &format!("chooser-month-{month}"),
                );
                let cell = focus_button_maybe_with(
                    container(label)
                        .width(month_cell_w)
                        .height(month_cell_h)
                        .center_x(month_cell_w)
                        .center_y(month_cell_h),
                    message,
                )
                .width(month_cell_w)
                .height(Length::Fixed(month_cell_h))
                .id(id)
                .semantics(
                    enzui_core::Semantics::new(enzui_core::semantics::Role::Button)
                        .label(self.labels.month_name(month))
                        .selected(false)
                        .default_action()
                        .disabled(!available),
                )
                .style_focus({
                    let resolve = std::rc::Rc::clone(&resolve);
                    move |theme: &iced::Theme, status: FocusStatus| {
                        let t = resolve(theme);
                        let bg = match status {
                            FocusStatus::Hovered | FocusStatus::Pressed => t.hover_background,
                            _ => Color::TRANSPARENT,
                        };
                        ButtonStyle {
                            background: Some(Background::Color(bg)),
                            text_color: if available { t.title } else { t.disabled },
                            border: focus_border(&t, status, CELL_RADIUS),
                            ..ButtonStyle::default()
                        }
                    }
                });
                cells.push(cell.into());
            }
            month_rows.push(row(cells).spacing(MONTH_GAP).into());
        }

        column![year_row, column(month_rows).spacing(MONTH_GAP)]
            .spacing(6.0)
            .width(grid_width)
            .into()
    }

    fn grid_width(&self) -> Length {
        if self.fill_width {
            Length::Fill
        } else {
            Length::Fixed(self.cell_size * DAYS_PER_WEEK as f32)
        }
    }

    fn weekday_header(&self, resolve: Resolver, rtl: bool) -> Element<'a, Message> {
        let cell_size = if self.fill_width {
            Length::Fill
        } else {
            Length::Fixed(self.cell_size)
        };
        let columns: Vec<_> = self
            .controller
            .grid()
            .weekday_header()
            .into_iter()
            .map(|weekday| {
                (
                    self.labels.weekday_short(weekday),
                    self.labels.weekday_narrow(weekday),
                    column_header_semantics(weekday, self.labels),
                )
            })
            .collect();
        container(iced::widget::responsive(
            move |size| -> Element<'a, Message> {
                let font = enzui_theme::text_defaults::font(
                    enzui_theme::FontRole::Ui,
                    enzui_theme::WeightRung::Semibold,
                );
                let size_px = enzui_theme::text_scale::ui_px(11.0);
                let typography =
                    crate::text_measure::MeasureTypography::new(crate::text::TextSize::Caption)
                        .pin_size_px(size_px)
                        .font_family(font.family)
                        .weight(font.weight);
                let compact = columns.iter().any(|(abbreviated, _, _)| {
                    crate::text_measure::enz_text_measure_width(abbreviated, typography)
                        > (size.width / DAYS_PER_WEEK as f32 - 4.0).max(0.0)
                });
                let cells: Vec<Element<'a, Message>> = columns
                    .iter()
                    .map(|(abbreviated, narrow, semantics)| {
                        let resolve = std::rc::Rc::clone(&resolve);
                        let label = text(if compact {
                            narrow.clone()
                        } else {
                            abbreviated.clone()
                        })
                        .size(size_px)
                        .font(font)
                        .width(Length::Fill)
                        .align_x(iced::alignment::Horizontal::Center)
                        .wrapping(iced::widget::text::Wrapping::None)
                        .ellipsis(iced::widget::text::Ellipsis::End)
                        .style(move |theme: &iced::Theme| iced::widget::text::Style {
                            color: Some(resolve(theme).header),
                        });
                        crate::semantics_container::SemanticsContainer::leaf(
                            container(label)
                                .width(cell_size)
                                .center_x(cell_size)
                                .center_y(Length::Fixed(20.0)),
                            semantics.clone(),
                        )
                        .into()
                    })
                    .collect();
                row(if rtl {
                    cells.into_iter().rev().collect()
                } else {
                    cells
                })
                .into()
            },
        ))
        .width(self.grid_width())
        .height(20.0)
        .into()
    }
}

impl<'a, Message: Clone + 'a> From<EnzCalendar<'a, Message>> for Element<'a, Message> {
    fn from(calendar: EnzCalendar<'a, Message>) -> Self {
        calendar.into_element()
    }
}

/// Decodes activation and navigation keys without applying controller bounds.
///
/// Enter/Space selects with bare or Shift modifiers; Ctrl, Alt, or Logo activation
/// bubbles. Unclaimed keys, including Tab and Escape, return none.
#[must_use]
pub fn event_for_key(
    key: &iced::keyboard::Key,
    modifiers: iced::keyboard::Modifiers,
    rtl: bool,
    focused: Date,
) -> Option<CalendarEvent> {
    use iced::keyboard::key;
    if let iced::keyboard::Key::Named(key::Named::Enter | key::Named::Space) = key {
        if modifiers.control() || modifiers.alt() || modifiers.logo() {
            return None;
        }
        return Some(CalendarEvent::DaySelected(focused));
    }
    move_for_key(key, modifiers, rtl).map(CalendarEvent::Moved)
}

/// Applies a proposed event to the host controller and returns a date to select.
///
/// Only a successful `DaySelected` returns a date; hosts must store selection
/// separately. Focus and paging never select. Year browsing keeps the chooser
/// open; a successful month-changing jump closes it, while a no-op jump does not.
pub fn apply_event(controller: &mut MonthController, event: CalendarEvent) -> Option<Date> {
    match event {
        CalendarEvent::DaySelected(date) => controller.focus_date(date).ok(),
        CalendarEvent::Moved(movement) => {
            let _ = controller.apply(movement);
            None
        }
        CalendarEvent::Focused(date) => {
            let _ = controller.focus_date(date);
            None
        }
        CalendarEvent::Paged(months) => {
            controller.page_months(months);
            None
        }
        CalendarEvent::HeaderPressed => {
            controller.toggle_picker();
            None
        }
        CalendarEvent::YearChanged(target) => {
            controller.jump_to_month(target);
            None
        }
        CalendarEvent::Jumped(target) => {
            // Current-month choices and rejected jumps leave the chooser open.
            if controller.jump_to_month(target) {
                controller.set_picker_open(false);
            }
            None
        }
    }
}

/// Semantics for a date field trigger, re-exported so a picker does not have to
/// reach into core for the one function it needs.
pub use enzui_core::calendar_semantics::date_field_semantics as field_semantics;
