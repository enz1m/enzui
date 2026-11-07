//! Inclusive all-day event bands clipped to a 42-day month grid.
//!
//! Spans split at week boundaries and use up to two overlap rows. Excess or
//! off-grid events have no painted band or accessible node. Visible events
//! expose one full-range announcement; only their first visible segment paints
//! a title, and bands shorter than 9 pixels omit text. Titles elide to fit.
//!
//! Segments with either continuation edge have square corners throughout;
//! wholly contained segments have rounded corners. Columns remain left-to-right.

use enzui_core::civil_date::Date;
use enzui_core::collect::SemanticsNode;
use enzui_core::schedule::EventId;
use enzui_core::semantics::{Role, Semantics};
use iced::advanced::graphics::text::Paragraph as GraphicsParagraph;
use iced::advanced::text::Paragraph as _;
use iced::advanced::text::Renderer as _;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Renderer as _, Widget, layout, renderer};
use iced::{Background, Border, Color, Element, Length, Point, Rectangle, Shadow, Size};

/// One logical all-day event over an inclusive date range.
#[derive(Debug, Clone, PartialEq)]
pub struct CalendarEventSpan {
    /// Stable model identity.
    pub id: EventId,
    /// First event date, inclusive.
    pub start: Date,
    /// Last event date, inclusive.
    pub end: Date,
    /// Title painted on the first visible segment when tall enough, and announced in full.
    pub title: String,
    /// Band colour; `None` follows the theme's accent.
    pub color: Option<Color>,
}

impl CalendarEventSpan {
    /// Construct an event span. A reversed range normalizes to one day.
    pub fn new(id: EventId, start: Date, end: Date) -> Self {
        Self {
            id,
            start,
            end: if end < start { start } else { end },
            title: String::new(),
            color: None,
        }
    }

    /// Sets the event title.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Set the band colour.
    #[must_use]
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }
}

/// One week-row piece of a logical event, relative to the 42-cell grid.
#[derive(Debug, Clone, PartialEq)]
pub struct CalendarSpanSegment {
    /// Source span index.
    pub source: usize,
    /// Stable stacking row.
    pub slot: usize,
    /// Painted rectangle.
    pub rect: Rectangle,
    /// The event continues before this rectangle; paint a square cut edge.
    pub continues_before: bool,
    /// The event continues after this rectangle; paint a square cut edge.
    pub continues_after: bool,
    /// Marks the first visible segment, where the title may be painted.
    pub leads: bool,
}

const BAND_GAP: f32 = 1.0;
const MAX_BAND_ROWS: usize = 2;
const BAND_BOTTOM_INSET_RATIO: f32 = 0.10;
const BAND_BOTTOM_INSET_MIN: f32 = 3.0;
const BAND_BOTTOM_INSET_MAX: f32 = 12.0;
const BAND_HEIGHT_MIN: f32 = 5.0;
const BAND_HEIGHT_MAX: f32 = 15.0;
const BAND_HEIGHT_RATIO: f32 = 0.4;
pub(crate) const MIN_TEXT_BAND_HEIGHT: f32 = 9.0;
const BAND_TEXT_PAD_X: f32 = 3.0;

/// Returns 40% of cell height, clamped to 5–15 pixels.
#[must_use]
pub fn band_height(cell_height: f32) -> f32 {
    (cell_height * BAND_HEIGHT_RATIO).clamp(BAND_HEIGHT_MIN, BAND_HEIGHT_MAX)
}

/// Returns bottom clearance in pixels: 10% of height, clamped to 3–12.
///
/// The result is capped by slack after two band rows plus their gaps, so
/// clearance does not change with the number of events and may reach zero.
#[must_use]
pub fn band_bottom_inset(cell_height: f32) -> f32 {
    let slack =
        (cell_height - MAX_BAND_ROWS as f32 * (band_height(cell_height) + BAND_GAP)).max(0.0);
    (cell_height * BAND_BOTTOM_INSET_RATIO)
        .clamp(BAND_BOTTOM_INSET_MIN, BAND_BOTTOM_INSET_MAX)
        .min(slack)
}

/// Resolved column and row pitch for the 7-column, 6-row grid, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CalendarCell {
    /// Cell width in px (column pitch).
    pub width: f32,
    /// Cell height in px (row pitch).
    pub height: f32,
}

impl CalendarCell {
    /// Creates equal column and row pitch in pixels.
    #[must_use]
    pub fn square(size: f32) -> Self {
        Self {
            width: size,
            height: size,
        }
    }
}

fn band_text_size(bh: f32) -> f32 {
    (bh - 3.0).clamp(6.0, 11.0)
}

/// The label ink for a band painted in `color` as it is, by the shared ink
/// policy ([`Palette::ink_on`](enzui_theme::Palette::ink_on)): white wherever
/// it holds, a tinted dark ink only on genuinely light colours, never black
/// on a saturated blue. The layer itself paints each band through
/// [`Palette::filled_color`](enzui_theme::Palette::filled_color), as the
/// schedule paints its events.
pub fn band_ink(color: Color) -> Color {
    enzui_theme::Palette::ink_on(Color { a: 1.0, ..color })
}

/// Tests whether a nonempty title fits after horizontal padding using paint’s text shaping.
///
/// Returns false when the usable width is nonpositive.
#[must_use]
pub fn band_label_fits(title: &str, band_width: f32, text_size: f32) -> bool {
    let avail = band_width - 2.0 * BAND_TEXT_PAD_X;
    if title.is_empty() || avail <= 0.0 {
        return false;
    }
    let width = GraphicsParagraph::with_text(iced::advanced::text::Text {
        content: title,
        bounds: Size::new(f32::INFINITY, text_size.max(1.0)),
        size: text_size.into(),
        line_height: iced::widget::text::LineHeight::default(),
        font: iced::Font::DEFAULT,
        align_x: iced::advanced::text::Alignment::Left,
        align_y: iced::alignment::Vertical::Center,
        shaping: iced::widget::text::Shaping::Advanced,
        wrapping: iced::widget::text::Wrapping::None,
        ellipsis: iced::widget::text::Ellipsis::None,
        hint_factor: None,
    })
    .min_bounds()
    .width;
    width <= avail
}

/// Lays out visible event segments over square grid cells.
///
/// Orders spans by start date, descending end date, then id; equal sort keys
/// retain input order. At most two overlapping rows are returned.
#[must_use]
pub fn layout_calendar_spans(
    spans: &[CalendarEventSpan],
    first_visible: Date,
    cell_size: f32,
) -> Vec<CalendarSpanSegment> {
    layout_calendar_spans_in(spans, first_visible, CalendarCell::square(cell_size))
}

/// Lays out visible segments over possibly nonsquare grid cells.
///
/// The 42-day window starts at `first_visible`. Spans split at week boundaries
/// and take the first unoccupied interval row; rows beyond the first two are
/// omitted. Horizontal positions use left-to-right columns.
#[must_use]
pub fn layout_calendar_spans_in(
    spans: &[CalendarEventSpan],
    first_visible: Date,
    cell: CalendarCell,
) -> Vec<CalendarSpanSegment> {
    let first = first_visible.to_days();
    let last = first + 41;
    let mut ordered: Vec<_> = spans.iter().enumerate().collect();
    ordered.sort_by_key(|(_, s)| (s.start, std::cmp::Reverse(s.end), s.id));
    let mut row_ends: Vec<i32> = Vec::new();
    let mut out = Vec::new();
    let bh = band_height(cell.height);
    let bottom_inset = band_bottom_inset(cell.height);

    for (source, span) in ordered {
        let event_start = span.start.to_days();
        let event_end = span.end.to_days();
        if event_end < first || event_start > last {
            continue;
        }
        let visible_start = event_start.max(first);
        let visible_end = event_end.min(last);
        let slot = row_ends
            .iter()
            .position(|&end| end < visible_start)
            .unwrap_or_else(|| {
                row_ends.push(i32::MAX);
                row_ends.len() - 1
            });
        row_ends[slot] = visible_end;
        if slot >= MAX_BAND_ROWS {
            continue;
        }

        let mut cursor = visible_start;
        while cursor <= visible_end {
            let index = cursor - first;
            let week_end = first + (index / 7) * 7 + 6;
            let segment_end = visible_end.min(week_end);
            let start_col = (cursor - first).rem_euclid(7) as f32;
            let cells = (segment_end - cursor + 1) as f32;
            let week = (index / 7) as f32;
            out.push(CalendarSpanSegment {
                source,
                slot,
                rect: Rectangle {
                    x: start_col * cell.width,
                    y: week * cell.height + cell.height
                        - bottom_inset
                        - (slot + 1) as f32 * (bh + BAND_GAP),
                    width: cells * cell.width,
                    height: bh,
                },
                continues_before: cursor > event_start,
                continues_after: segment_end < event_end,
                leads: cursor == visible_start,
            });
            cursor = segment_end + 1;
        }
    }
    out
}

pub(crate) enum SpanLayerSizing {
    Square(f32),
    Fill,
}

impl SpanLayerSizing {
    fn cell(&self, bounds: Rectangle) -> CalendarCell {
        match self {
            SpanLayerSizing::Square(edge) => CalendarCell::square(*edge),
            SpanLayerSizing::Fill => CalendarCell {
                width: bounds.width / 7.0,
                height: bounds.height / 6.0,
            },
        }
    }
}

pub(crate) struct CalendarSpanLayer {
    pub spans: Vec<CalendarEventSpan>,
    pub first_visible: Date,
    pub sizing: SpanLayerSizing,
    pub announcements: Vec<String>,
    /// The day grid runs right to left, so the bands do too.
    pub mirrored: bool,
}

impl CalendarSpanLayer {
    /// The segments over `bounds`, mirrored with a right-to-left grid.
    fn geometry(&self, bounds: Rectangle) -> Vec<CalendarSpanSegment> {
        let cell = self.sizing.cell(bounds);
        let mut segments = layout_calendar_spans_in(&self.spans, self.first_visible, cell);
        if self.mirrored {
            let week = 7.0 * cell.width;
            for segment in &mut segments {
                segment.rect.x = week - segment.rect.x - segment.rect.width;
            }
        }
        segments
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for CalendarSpanLayer {
    fn tag(&self) -> tree::Tag {
        tree::Tag::stateless()
    }
    fn state(&self) -> tree::State {
        tree::State::None
    }
    fn size(&self) -> Size<Length> {
        match self.sizing {
            SpanLayerSizing::Square(edge) => {
                Size::new(Length::Fixed(edge * 7.0), Length::Fixed(edge * 6.0))
            }
            SpanLayerSizing::Fill => Size::new(Length::Fill, Length::Fill),
        }
    }
    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) {
        let node = match self.sizing {
            SpanLayerSizing::Square(edge) => layout::Node::new(limits.resolve(
                Length::Fixed(edge * 7.0),
                Length::Fixed(edge * 6.0),
                Size::ZERO,
            )),
            SpanLayerSizing::Fill => layout::Node::new(limits.max()),
        };
        node.apply(tree);
    }
    fn operate(
        &mut self,
        _tree: &mut Tree,
        layout: Layout,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let origin = layout.bounds();
        let geometry = self.geometry(origin);
        operation.traverse(&mut |operation| {
            for (source, span) in self.spans.iter().enumerate() {
                let Some(segment) = geometry.iter().find(|g| g.source == source) else {
                    continue;
                };
                let mut node = SemanticsNode::new(
                    Semantics::new(Role::ListItem).label(self.announcements[source].clone()),
                );
                operation.custom(
                    None,
                    Rectangle {
                        x: origin.x + segment.rect.x,
                        y: origin.y + segment.rect.y,
                        ..segment.rect
                    },
                    &mut node,
                );
                let _ = span;
            }
        });
    }
    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout,
        _cursor: iced::advanced::mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let palette = crate::theme_lookup::palette(theme);
        let origin = layout.bounds();
        for segment in self.geometry(origin) {
            let span = &self.spans[segment.source];
            // Fill and label ink as one pair, the way schedule events and
            // accent controls resolve an authored colour.
            let pair = palette.filled_color(span.color.unwrap_or(palette.accent));
            let bounds = Rectangle {
                x: origin.x + segment.rect.x,
                y: origin.y + segment.rect.y,
                ..segment.rect
            };
            renderer.fill_quad(
                renderer::Quad {
                    bounds,
                    border: Border {
                        radius: if segment.continues_before || segment.continues_after {
                            0.0
                        } else {
                            2.5
                        }
                        .into(),
                        ..Border::default()
                    },
                    shadow: Shadow::default(),
                    snap: true,
                },
                Background::Color(pair.color),
            );

            if !segment.leads || span.title.is_empty() || bounds.height < MIN_TEXT_BAND_HEIGHT {
                continue;
            }
            let text_size = band_text_size(bounds.height);
            let fits = band_label_fits(&span.title, bounds.width, text_size);
            let text_bounds = Rectangle {
                x: bounds.x + BAND_TEXT_PAD_X,
                y: bounds.y,
                width: (bounds.width - 2.0 * BAND_TEXT_PAD_X).max(0.0),
                height: bounds.height,
            };
            if text_bounds.width <= 0.0 {
                continue;
            }
            renderer.fill_text(
                iced::advanced::text::Text {
                    content: span.title.clone(),
                    bounds: text_bounds.size(),
                    size: text_size.into(),
                    line_height: iced::widget::text::LineHeight::default(),
                    font: iced::Font::DEFAULT,
                    // A band reads from its leading edge: the right one in RTL.
                    align_x: if self.mirrored {
                        iced::advanced::text::Alignment::Right
                    } else {
                        iced::advanced::text::Alignment::Left
                    },
                    align_y: iced::alignment::Vertical::Center,
                    shaping: iced::widget::text::Shaping::Advanced,
                    wrapping: iced::widget::text::Wrapping::None,
                    ellipsis: if fits {
                        iced::widget::text::Ellipsis::None
                    } else {
                        iced::widget::text::Ellipsis::End
                    },
                    hint_factor: None,
                },
                Point::new(
                    if self.mirrored {
                        text_bounds.x + text_bounds.width
                    } else {
                        text_bounds.x
                    },
                    text_bounds.center_y(),
                ),
                pair.text,
                text_bounds,
            );
        }
    }
}

impl<'a, Message: 'a> From<CalendarSpanLayer> for Element<'a, Message> {
    fn from(layer: CalendarSpanLayer) -> Self {
        Element::new(layer)
    }
}

/// Builds a fill-sized event overlay for a caller-built 6-row, 7-column month grid.
///
/// Stack it over the matching grid. Layout divides resolved bounds into cells;
/// columns remain left-to-right. Supply one announcement per span, in the same
/// order; only events with painted segments publish an accessible node.
///
/// # Panics
///
/// Construction asserts equal span and announcement counts in debug builds.
/// In any build, accessibility traversal panics if a displayed span has no
/// corresponding announcement.
pub fn calendar_span_overlay<'a, Message: 'a>(
    spans: impl IntoIterator<Item = CalendarEventSpan>,
    first_visible: Date,
    announcements: impl IntoIterator<Item = String>,
) -> Element<'a, Message> {
    let spans: Vec<CalendarEventSpan> = spans.into_iter().collect();
    let announcements: Vec<String> = announcements.into_iter().collect();
    debug_assert_eq!(
        announcements.len(),
        spans.len(),
        "one announcement per span"
    );
    CalendarSpanLayer {
        spans,
        first_visible,
        sizing: SpanLayerSizing::Fill,
        announcements,
        mirrored: false,
    }
    .into()
}
