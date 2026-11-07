//! Absolute schedule layout and hit testing shared by day and week surfaces.

use enzui_core::schedule::{
    BackgroundBand, EventId, NowMarker, ScheduleAxis, ScheduleEvent, ScheduleNow, layout_overlaps,
};
use iced::{Color, Point, Rectangle};

/// Caller-formatted day header and independent today/selection flags.
///
/// The widget owns no date or locale policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleColumn {
    /// Top header line, e.g. `"Mon"`. May be empty.
    pub weekday: String,
    /// Bottom header line, e.g. `"5"`. May be empty.
    pub day_number: String,
    /// Today flag; a today-only column has a disc and wash, while selected today uses numeral weight.
    pub is_today: bool,
    /// Selection flag; adds body and header washes, replacing the today-only disc.
    pub is_selected: bool,
}

impl ScheduleColumn {
    /// A column with both header lines and no today or selection marking.
    pub fn new(weekday: impl Into<String>, day_number: impl Into<String>) -> Self {
        Self {
            weekday: weekday.into(),
            day_number: day_number.into(),
            is_today: false,
            is_selected: false,
        }
    }

    /// Mark this column as today.
    #[must_use]
    pub fn today(mut self, is_today: bool) -> Self {
        self.is_today = is_today;
        self
    }

    /// Set selection independently of today; see [`crate::schedule::ColumnEmphasis`].
    #[must_use]
    pub fn selected(mut self, is_selected: bool) -> Self {
        self.is_selected = is_selected;
        self
    }
}

/// Caller-owned event identity and presentation.
///
/// Event IDs must be unique across the supplied blocks. Colors are inputs to the
/// runtime palette's filled-color resolver; the painted fill may differ.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleBlock {
    /// Stable identity, supplied by the caller.
    pub id: EventId,
    /// First column (0-based).
    pub column: usize,
    /// Inclusive last column; equal to `column` for a single-day event.
    pub end_column: usize,
    /// Start, minutes from the caller's origin (typically midnight).
    pub start_minute: i32,
    /// End, minutes from the caller's origin.
    pub end_minute: i32,
    /// All-day events go in the separate band and get no minute mapping.
    pub all_day: bool,
    /// Primary label.
    pub title: String,
    /// Secondary label (time range, location…). Drawn only when the block is
    /// tall enough for a second line.
    pub subtitle: String,
    /// The event's colour; `None` follows the theme's accent.
    pub color: Option<Color>,
    /// Whether the app considers this block selected.
    pub selected: bool,
}

impl ScheduleBlock {
    /// A timed block.
    pub fn timed(id: EventId, column: usize, start_minute: i32, end_minute: i32) -> Self {
        Self {
            id,
            column,
            end_column: column,
            start_minute,
            end_minute,
            all_day: false,
            title: String::new(),
            subtitle: String::new(),
            color: None,
            selected: false,
        }
    }

    /// All-day block in one column; use [`Self::all_day_span`] for a span.
    pub fn all_day(id: EventId, column: usize) -> Self {
        Self::all_day_span(id, column, column)
    }

    /// All-day event spanning inclusive columns with one accessible identity.
    ///
    /// A reversed endpoint is normalized to the start column. Rows are allocated
    /// independently of caller order, subject to the configured all-day row cap.
    pub fn all_day_span(id: EventId, start_column: usize, end_column: usize) -> Self {
        Self {
            all_day: true,
            end_column: end_column.max(start_column),
            ..Self::timed(id, start_column, 0, 0)
        }
    }

    /// Timed event with midnight-separated segments and one accessible identity.
    ///
    /// First and last segments use the supplied endpoint minutes; intermediate
    /// segments cover a full day. A reversed endpoint becomes the start column.
    /// Supply endpoints within the visible column list: geometry clamps the end
    /// column but still uses `end_minute` on that last retained segment.
    pub fn timed_span(
        id: EventId,
        start_column: usize,
        start_minute: i32,
        end_column: usize,
        end_minute: i32,
    ) -> Self {
        Self {
            end_column: end_column.max(start_column),
            ..Self::timed(id, start_column, start_minute, end_minute)
        }
    }

    /// Set the title.
    #[must_use]
    pub fn title(mut self, t: impl Into<String>) -> Self {
        self.title = t.into();
        self
    }

    /// Set the subtitle.
    #[must_use]
    pub fn subtitle(mut self, t: impl Into<String>) -> Self {
        self.subtitle = t.into();
        self
    }

    /// Set the colour.
    #[must_use]
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }

    /// Mark selected.
    #[must_use]
    pub fn selected(mut self, s: bool) -> Self {
        self.selected = s;
        self
    }
}

/// Geometry settings normalized by [`ScheduleFrame::resolve`].
///
/// Invalid pixel scale falls back to [`DEFAULT_PIXELS_PER_MINUTE`]; a nonpositive
/// visible interval becomes a full day. Invalid lengths become zero, and zero
/// header or all-day row height reserves no band. Empty columns resolve to one
/// geometry column. Fields controlling ruler steps and row caps have their own
/// fallbacks below.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleConfig {
    /// First visible minute of the day (default `0` = 00:00).
    pub visible_start_minute: i32,
    /// Last visible minute, exclusive (default `1440` = 24:00).
    pub visible_end_minute: i32,
    /// Major ruler step in minutes; nonpositive values fall back to 60.
    pub granularity_minutes: i32,
    /// Minor ruler step in minutes; absent or nonpositive values draw none.
    pub minor_granularity_minutes: Option<i32>,
    /// Vertical scale. 1.0 => one pixel per minute => 1440px for a full day.
    pub pixels_per_minute: f32,
    /// Working-hours band start; the region *outside* it is tinted.
    pub work_start_minute: i32,
    /// Working-hours band end.
    pub work_end_minute: i32,
    /// Minimum width of a visible hour-label gutter; the widget grows it to fit labels.
    /// Zero or nonfinite values hide the gutter outside compact mode.
    pub gutter_width: f32,
    /// Height of the day-column header row. `0` hides it.
    pub header_height: f32,
    /// Height of one all-day chip row. `0` hides the whole band.
    pub all_day_row_height: f32,
    /// All-day row cap, default three; zero normalizes to one row.
    pub max_all_day_rows: usize,
    /// Signed minute offset of an optional secondary hour ruler. This
    /// is a plain displacement the caller computed; the widget owns no timezone
    /// policy. `None` draws one ruler.
    pub secondary_offset_minutes: Option<i32>,
    /// Secondary gutter header, trimmed and omitted when blank or no gutter is reserved.
    /// Long labels are end-elided within the existing header slice.
    pub secondary_label: Option<String>,
}

/// Default vertical scale: 0.8 px per minute, i.e. 48px per hour.
pub const DEFAULT_PIXELS_PER_MINUTE: f32 = 0.8;

/// Default hour-gutter width. Wide enough for `"12 AM"` at the body size.
pub const DEFAULT_GUTTER_WIDTH: f32 = 56.0;

/// Default height derived from the default weekday line box and today disc.
///
/// Custom drawing text sizes require a matching header height.
pub const DEFAULT_HEADER_HEIGHT: f32 =
    header_height_for(DEFAULT_HEADER_TEXT_SIZE, DEFAULT_DAY_NUMBER_SIZE);

/// Base weekday-label size used by the header geometry.
///
/// Drawing tokens also apply the UI text scale; custom sizes need matching geometry.
pub const DEFAULT_HEADER_TEXT_SIZE: f32 = 11.0;

/// Default day-number text size. Mirrors `ScheduleTokens::day_number_size`.
pub const DEFAULT_DAY_NUMBER_SIZE: f32 = 17.0;

/// Padding above the weekday label inside the header.
pub const HEADER_TOP_PAD_PX: f32 = 6.0;

/// Padding below the today disc inside the header.
pub const HEADER_BOTTOM_PAD_PX: f32 = 4.0;

/// Gap between the weekday label's line box and the day-number disc.
pub const HEADER_LINE_GAP_PX: f32 = 2.0;

/// How much the today disc is inset around the day-number glyph, per side —
/// so the disc's diameter is `day_number_size + 2 * DISC_PAD_PX`.
pub const DISC_PAD_PX: f32 = 4.5;

/// Default relative line-box height, 1.3 times the text size.
#[must_use]
pub const fn line_box(size: f32) -> f32 {
    size * 1.3
}

/// The disc diameter for a day-number of `size`.
#[must_use]
pub const fn disc_diameter(day_number_size: f32) -> f32 {
    day_number_size + 2.0 * DISC_PAD_PX
}

/// Header height in logical pixels including weekday line box, gap, disc and padding.
#[must_use]
pub const fn header_height_for(header_text_size: f32, day_number_size: f32) -> f32 {
    HEADER_TOP_PAD_PX
        + line_box(header_text_size)
        + HEADER_LINE_GAP_PX
        + disc_diameter(day_number_size)
        + HEADER_BOTTOM_PAD_PX
}

/// Where the two header elements sit, resolved against one header rectangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeaderLayout {
    /// Top y of the weekday label.
    pub weekday_top: f32,
    /// Bottom y of the weekday label's line box (not its glyphs).
    pub weekday_bottom: f32,
    /// Centre y of the day number and of the today disc.
    pub day_number_center: f32,
    /// The today disc's diameter.
    pub disc_diameter: f32,
}

impl HeaderLayout {
    /// Top y of the today disc.
    #[must_use]
    pub fn disc_top(&self) -> f32 {
        self.day_number_center - self.disc_diameter / 2.0
    }

    /// Bottom y of the today disc.
    #[must_use]
    pub fn disc_bottom(&self) -> f32 {
        self.day_number_center + self.disc_diameter / 2.0
    }

    /// Whether the weekday line box overlaps the today disc for this layout.
    #[must_use]
    pub fn overlaps(&self) -> bool {
        self.weekday_bottom > self.disc_top()
    }
}

/// Default height of one all-day chip row.
pub const DEFAULT_ALL_DAY_ROW_HEIGHT: f32 = 20.0;

/// How close to a block's bottom edge a press counts as a resize grab.
pub const RESIZE_EDGE_PX: f32 = 6.0;

/// Horizontal inset applied to every column's block area, so blocks do not sit
/// flush against the column rule.
pub const COLUMN_INSET_PX: f32 = 2.0;

/// Gap between side-by-side lanes within one column.
pub const LANE_GAP_PX: f32 = 2.0;

/// Lane-width threshold in logical pixels for switching from subdivision to cascade.
///
/// Cascaded chips use this width, capped to the usable column width and floored
/// to one pixel. Later chips overlap earlier ones.
pub const MIN_LANE_WIDTH_PX: f32 = 56.0;

/// Preferred horizontal indent between cascaded lanes, clamped at the column edge.
pub const CASCADE_INDENT_PX: f32 = 12.0;

/// Thickness of a vertical column rule, in pixels. The openers and the closing
/// rule share it, so the grid's right-hand boundary cannot end up a different
/// weight from its interior separators.
pub const COLUMN_RULE_WIDTH_PX: f32 = 1.0;

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            visible_start_minute: 0,
            visible_end_minute: 24 * 60,
            granularity_minutes: 60,
            minor_granularity_minutes: Some(30),
            pixels_per_minute: DEFAULT_PIXELS_PER_MINUTE,
            work_start_minute: 9 * 60,
            work_end_minute: 17 * 60,
            gutter_width: DEFAULT_GUTTER_WIDTH,
            header_height: DEFAULT_HEADER_HEIGHT,
            all_day_row_height: DEFAULT_ALL_DAY_ROW_HEIGHT,
            max_all_day_rows: 3,
            secondary_offset_minutes: None,
            secondary_label: None,
        }
    }
}

/// One column's resolved horizontal geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnGeometry {
    /// Index in the caller's column list.
    pub index: usize,
    /// Absolute x of the column's left edge.
    pub x: f32,
    /// Column width.
    pub width: f32,
    /// Whether this column is today.
    pub is_today: bool,
    /// Whether this column is the caller's selected day.
    pub is_selected: bool,
}

/// Absolute block geometry and lane allocation, with an index into the source block slice.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockGeometry {
    /// The block's identity.
    pub id: EventId,
    /// Its column.
    pub column: usize,
    /// 0-based lane within the column's overlap cluster.
    pub lane: usize,
    /// Lanes this block occupies, starting at [`Self::lane`]. `1` unless the
    /// following lanes stay free for the block's whole interval, in which case
    /// the block widens over them.
    pub lane_span: usize,
    /// Lanes the cluster needed.
    pub lane_count: usize,
    /// Absolute rectangle, already scrolled (timed blocks) or in the all-day
    /// band (all-day chips).
    pub rect: Rectangle,
    /// Index into the caller's block slice, so the caller reads title/colour
    /// back without a map lookup.
    pub source: usize,
}

/// What a point in the surface resolves to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleHit {
    /// The body of a timed block.
    Block {
        /// Which block.
        id: EventId,
    },
    /// Within [`RESIZE_EDGE_PX`] of a timed block's bottom edge.
    BlockResizeEdge {
        /// Which block.
        id: EventId,
    },
    /// An all-day chip.
    AllDayBlock {
        /// Which block.
        id: EventId,
    },
    /// Empty timed grid.
    Slot {
        /// Column index.
        column: usize,
        /// Minute at the pointer, snapped to the drag granularity by the
        /// caller, not here.
        minute: i32,
    },
    /// Empty area of the all-day band.
    AllDaySlot {
        /// Column index.
        column: usize,
    },
    /// A day-column header.
    Header {
        /// Column index.
        column: usize,
    },
    /// The hour gutter or anything else.
    None,
}

/// Everything one paint of the schedule needs, resolved to absolute pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleFrame {
    /// The widget's own bounds.
    pub bounds: Rectangle,
    /// The day-header row (zero-height when `header_height == 0`).
    pub header: Rectangle,
    /// The all-day band (zero-height when there is nothing to show).
    pub all_day_band: Rectangle,
    /// The timed grid viewport — the clipped region blocks are drawn in.
    pub grid: Rectangle,
    /// Absolute x/width of the hour gutter, spanning header→grid.
    pub gutter: Rectangle,
    /// Absolute x/width of the optional secondary hour gutter.
    pub secondary_gutter: Option<Rectangle>,
    /// Trimmed secondary header name, present only when a gutter is
    /// reserved and the caller supplied a nonblank
    /// [`ScheduleConfig::secondary_label`].
    pub secondary_header_label: Option<String>,
    /// Per-column horizontal geometry.
    pub columns: Vec<ColumnGeometry>,
    /// The axis the transform came from; exposed so a caller can convert a
    /// minute without rebuilding it.
    pub axis: ScheduleAxis,
    /// Applied vertical scroll, in pixels, already clamped to
    /// `[0, content_height - grid.height]`.
    pub scroll: f32,
    /// Absolute y of axis pixel zero, shared by paint and hit testing.
    pub origin_y: f32,
    /// Full timed-content height (may exceed `grid.height`).
    pub content_height: f32,
    /// Major rule lines as `(minute, absolute y)`, already scrolled.
    pub rules: Vec<(i32, f32)>,
    /// Secondary-ruler labels as `(labelled minute, absolute y)`.
    pub secondary_rules: Vec<(i32, f32)>,
    /// Minor rule lines as absolute y.
    pub minor_rules: Vec<f32>,
    /// Off-hours tint bands as absolute `(top, bottom)` pairs.
    pub off_hours: Vec<(f32, f32)>,
    /// The current-time marker, absolute y, when visible.
    pub now: Option<NowMarker>,
    /// Timed blocks.
    pub blocks: Vec<BlockGeometry>,
    /// All-day chips.
    pub all_day_chips: Vec<BlockGeometry>,
}

fn sane_scale(px_per_min: f32) -> f32 {
    if px_per_min.is_finite() && px_per_min > 0.0 {
        px_per_min
    } else {
        DEFAULT_PIXELS_PER_MINUTE
    }
}

fn sane_len(px: f32) -> f32 {
    if px.is_finite() && px >= 0.0 { px } else { 0.0 }
}

impl ScheduleFrame {
    /// Resolve absolute layout, normalized bands and a clamped scroll offset.
    ///
    /// Timed blocks retain scrolled rectangles even outside the grid viewport; drawing
    /// clips them. All-day events beyond the row cap are omitted.
    ///
    /// # Panics
    ///
    /// In debug builds, minute subtraction or secondary-rule offset addition
    /// that overflows `i32` panics.
    #[must_use]
    pub fn resolve(
        bounds: Rectangle,
        config: &ScheduleConfig,
        columns: &[ScheduleColumn],
        blocks: &[ScheduleBlock],
        now: Option<ScheduleNow>,
        scroll: f32,
    ) -> Self {
        let column_count = columns.len().max(1);

        let header_h = sane_len(config.header_height).min(bounds.height);
        let header = Rectangle {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: header_h,
        };

        let all_day_placements = layout_spanning_all_day(blocks, column_count);
        let deepest = all_day_placements
            .iter()
            .map(|p| p.slot + 1)
            .max()
            .unwrap_or(0);
        let row_h = sane_len(config.all_day_row_height);
        let rows = deepest.min(config.max_all_day_rows.max(1));
        let band_h = (rows as f32 * row_h).min((bounds.height - header_h).max(0.0));
        let all_day_band = Rectangle {
            x: bounds.x,
            y: bounds.y + header_h,
            width: bounds.width,
            height: band_h,
        };

        let grid = Rectangle {
            x: bounds.x,
            y: all_day_band.y + band_h,
            width: bounds.width,
            height: (bounds.height - header_h - band_h).max(0.0),
        };

        let gutter_w = sane_len(config.gutter_width).min(bounds.width);
        let secondary_w = if config.secondary_offset_minutes.is_some() {
            sane_len(config.gutter_width).min((bounds.width - gutter_w).max(0.0))
        } else {
            0.0
        };
        let gutter = Rectangle {
            x: bounds.x,
            y: bounds.y,
            width: gutter_w,
            height: bounds.height,
        };
        let secondary_gutter = (secondary_w > 0.0).then_some(Rectangle {
            x: bounds.x + gutter_w,
            y: bounds.y,
            width: secondary_w,
            height: bounds.height,
        });
        let secondary_header_label = secondary_gutter.and_then(|_| {
            config
                .secondary_label
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        });
        let columns_x = bounds.x + gutter_w + secondary_w;
        let columns_w = (bounds.width - gutter_w - secondary_w).max(0.0);
        let column_w = columns_w / column_count as f32;

        let column_geometry: Vec<ColumnGeometry> = (0..column_count)
            .map(|i| ColumnGeometry {
                index: i,
                x: columns_x + i as f32 * column_w,
                width: column_w,
                is_today: columns.get(i).is_some_and(|c| c.is_today),
                is_selected: columns.get(i).is_some_and(|c| c.is_selected),
            })
            .collect();

        let (vs, ve) = if config.visible_end_minute > config.visible_start_minute {
            (config.visible_start_minute, config.visible_end_minute)
        } else {
            (0, 24 * 60)
        };
        let ppm = sane_scale(config.pixels_per_minute);
        let content_height = (ve - vs) as f32 * ppm;
        let granularity = if config.granularity_minutes > 0 {
            config.granularity_minutes
        } else {
            60
        };
        let axis = ScheduleAxis::new(vs, ve, granularity, content_height.max(1.0))
            .unwrap_or_else(|| ScheduleAxis::new(0, 1, 1, 1.0).expect("trivial axis is valid"));

        let max_scroll = (content_height - grid.height).max(0.0);
        let scroll = if scroll.is_finite() {
            scroll.clamp(0.0, max_scroll)
        } else {
            0.0
        };
        let origin_y = grid.y - scroll;

        let rules: Vec<(i32, f32)> = axis
            .rule_positions()
            .into_iter()
            .map(|(m, y)| (m, origin_y + y))
            .collect();
        let secondary_rules: Vec<(i32, f32)> = match config.secondary_offset_minutes {
            Some(off) => axis
                .secondary_rule_positions(off)
                .into_iter()
                .map(|(m, y)| (m, origin_y + y))
                .collect(),
            None => Vec::new(),
        };
        let minor_rules: Vec<f32> = match config.minor_granularity_minutes {
            Some(g) if g > 0 => ScheduleAxis::new(vs, ve, g, content_height.max(1.0))
                .map(|a| {
                    a.rule_positions()
                        .into_iter()
                        .map(|(_, y)| origin_y + y)
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let off_hours: Vec<(f32, f32)> = axis
            .off_hours_bands(config.work_start_minute, config.work_end_minute)
            .into_iter()
            .map(|b: BackgroundBand| (origin_y + b.top, origin_y + b.bottom))
            .collect();
        let now_marker = now.and_then(|n| {
            let m = axis.now_marker(n, n.column)?;
            Some(NowMarker {
                minute: m.minute,
                column: m.column,
                y: origin_y + m.y,
            })
        });

        let timed_events = to_timed_segments(blocks, column_count);
        let laid = layout_overlaps(&timed_events);
        let mut block_geometry = Vec::with_capacity(laid.len());
        for l in &laid {
            let Some(col) = column_geometry.get(l.column) else {
                continue;
            };
            let usable = (col.width - 2.0 * COLUMN_INSET_PX).max(0.0);
            let lane_count = l.lane_count.max(1);
            let even_slot_w = usable / lane_count as f32;

            let (x, w) = if even_slot_w >= MIN_LANE_WIDTH_PX {
                let span = l
                    .lane_span
                    .clamp(1, lane_count - l.lane.min(lane_count - 1));
                let reaches_last_lane = l.lane + span >= lane_count;
                let x = col.x + COLUMN_INSET_PX + l.lane as f32 * even_slot_w;
                let gap = if reaches_last_lane { 0.0 } else { LANE_GAP_PX };
                let w = (span as f32 * even_slot_w - gap).max(1.0);
                (x, w)
            } else {
                // min/max absorbs NaN here; clamp would propagate it into chip geometry.
                #[allow(
                    clippy::manual_clamp,
                    reason = "min/max absorbs NaN here; clamp would propagate it into chip geometry"
                )]
                let chip_w = usable.min(MIN_LANE_WIDTH_PX).max(1.0);
                let right = col.x + col.width - COLUMN_INSET_PX;
                let ideal_x = col.x + COLUMN_INSET_PX + l.lane as f32 * CASCADE_INDENT_PX;
                let max_x = (right - chip_w).max(col.x + COLUMN_INSET_PX);
                let x = ideal_x.min(max_x);
                (x, chip_w)
            };

            let top = origin_y + axis.y_of(l.start_minute);
            let bottom = origin_y + axis.y_of(l.end_minute.max(l.start_minute));
            block_geometry.push(BlockGeometry {
                id: l.id,
                column: l.column,
                lane: l.lane,
                lane_span: l.lane_span.max(1),
                lane_count,
                rect: Rectangle {
                    x,
                    y: top,
                    width: w,
                    height: (bottom - top).max(2.0),
                },
                source: l.payload,
            });
        }

        let mut chips = Vec::with_capacity(all_day_placements.len());
        for p in &all_day_placements {
            if p.slot >= rows {
                continue;
            }
            let (Some(first), Some(last)) = (
                column_geometry.get(p.start_column),
                column_geometry.get(p.end_column),
            ) else {
                continue;
            };
            chips.push(BlockGeometry {
                id: p.id,
                column: p.start_column,
                lane: p.slot,
                lane_span: 1,
                lane_count: deepest,
                rect: Rectangle {
                    x: first.x + COLUMN_INSET_PX,
                    y: all_day_band.y + p.slot as f32 * row_h,
                    width: if p.start_column == p.end_column {
                        (first.width - 2.0 * COLUMN_INSET_PX).max(1.0)
                    } else {
                        (last.x + last.width - COLUMN_INSET_PX - (first.x + COLUMN_INSET_PX))
                            .max(1.0)
                    },
                    height: row_h,
                },
                source: p.source,
            });
        }

        Self {
            bounds,
            header,
            all_day_band,
            grid,
            gutter,
            secondary_gutter,
            secondary_header_label,
            columns: column_geometry,
            axis,
            scroll,
            origin_y,
            content_height,
            rules,
            secondary_rules,
            minor_rules,
            off_hours,
            now: now_marker,
            blocks: block_geometry,
            all_day_chips: chips,
        }
    }

    /// Full usable-column rectangle for a create proposal.
    ///
    /// Reversed minutes are sorted; height is at least two pixels. Missing columns
    /// return `None`. The result may extend outside the grid and needs viewport clipping.
    #[must_use]
    pub fn range_preview_rect(
        &self,
        column: usize,
        start_minute: i32,
        end_minute: i32,
    ) -> Option<Rectangle> {
        let col = self.columns.get(column)?;
        let (lo, hi) = if start_minute <= end_minute {
            (start_minute, end_minute)
        } else {
            (end_minute, start_minute)
        };
        let top = self.origin_y + self.axis.y_of(lo);
        let bottom = self.origin_y + self.axis.y_of(hi);
        Some(Rectangle {
            x: col.x + COLUMN_INSET_PX,
            y: top,
            width: (col.width - 2.0 * COLUMN_INSET_PX).max(1.0),
            height: (bottom - top).max(2.0),
        })
    }

    /// The largest scroll offset this frame allows.
    #[must_use]
    pub fn max_scroll(&self) -> f32 {
        (self.content_height - self.grid.height).max(0.0)
    }

    /// Overlay thumb rectangle using shared scrollbar dimensions.
    ///
    /// Returns `None` without positive overflow and grid height. Length is proportional
    /// to visible content with the shared minimum, capped to the grid height. Position
    /// follows scroll along the remaining track. The widget paints this thumb but
    /// does not handle thumb dragging.
    #[must_use]
    pub fn scrollbar_thumb(&self) -> Option<Rectangle> {
        let max = self.max_scroll();
        if max <= 0.0 || self.content_height <= 0.0 || self.grid.height <= 0.0 {
            return None;
        }
        let sb = enzui_theme::ScrollbarTokens::DEFAULT;

        let ratio = self.grid.height / self.content_height;
        let thumb_h = (self.grid.height * ratio)
            .max(sb.min_thumb_length)
            .min(self.grid.height);

        let track = self.grid.height - thumb_h;
        let frac = if max > 0.0 { self.scroll / max } else { 0.0 };
        let thumb_y = self.grid.y + track * frac;

        let thumb_x = self.grid.x + self.grid.width - (sb.hit_width + sb.width) / 2.0;

        Some(Rectangle {
            x: thumb_x,
            y: thumb_y,
            width: sb.width,
            height: thumb_h,
        })
    }

    /// Secondary gutter's slice of the existing header.
    ///
    /// Returns `None` for an absent gutter, absent label or zero-height header.
    /// Text wrapping and elision are drawing behavior, not properties of this rectangle.
    #[must_use]
    pub fn secondary_header_cell(&self) -> Option<Rectangle> {
        if self.secondary_header_label.is_none() || self.header.height <= 0.0 {
            return None;
        }
        let sec = self.secondary_gutter?;
        Some(Rectangle {
            x: sec.x,
            y: self.header.y,
            width: sec.width,
            height: self.header.height,
        })
    }

    /// Place weekday and disc top-down using the supplied sizes.
    ///
    /// Extra header height remains below the disc; contents are not compressed to fit
    /// an undersized header.
    #[must_use]
    pub fn header_layout(&self, header_text_size: f32, day_number_size: f32) -> HeaderLayout {
        let weekday_top = self.header.y + HEADER_TOP_PAD_PX;
        let weekday_bottom = weekday_top + line_box(header_text_size);
        let d = disc_diameter(day_number_size);
        HeaderLayout {
            weekday_top,
            weekday_bottom,
            day_number_center: weekday_bottom + HEADER_LINE_GAP_PX + d / 2.0,
            disc_diameter: d,
        }
    }

    /// Hour-label top positions whose supplied height fits within the grid.
    ///
    /// Each top is its rule y plus `gap`. This filters geometric bounds using
    /// `label_height`; it does not measure font glyphs or line boxes.
    #[must_use]
    pub fn hour_label_positions(
        &self,
        rules: &[(i32, f32)],
        label_height: f32,
        gap: f32,
    ) -> Vec<(i32, f32)> {
        let bottom = self.grid.y + self.grid.height;
        rules
            .iter()
            .map(|(m, y)| (*m, *y + gap))
            .filter(|(_, top)| *top >= self.grid.y && *top + label_height <= bottom)
            .collect()
    }

    /// Absolute column-opening rules and a final closing rule inset by its width.
    ///
    /// Invalid thickness falls back to [`COLUMN_RULE_WIDTH_PX`]. The closing rule is
    /// omitted if it would overlap the last opener on a narrow grid.
    #[must_use]
    pub fn column_rule_positions(&self, thickness: f32) -> Vec<f32> {
        let t = if thickness.is_finite() && thickness > 0.0 {
            thickness
        } else {
            COLUMN_RULE_WIDTH_PX
        };
        let right = self.grid.x + self.grid.width;
        let mut xs: Vec<f32> = self.columns.iter().map(|c| c.x).collect();
        if let Some(last) = self.columns.last() {
            let close = (last.x + last.width).min(right) - t;
            if close > last.x && close >= self.grid.x {
                xs.push(close);
            }
        }
        xs
    }

    /// The column index containing absolute x, if any.
    #[must_use]
    pub fn column_at(&self, x: f32) -> Option<usize> {
        self.columns
            .iter()
            .find(|c| x >= c.x && x < c.x + c.width)
            .map(|c| c.index)
    }

    /// Unsnapped minute at absolute y, using the same stored origin as block geometry.
    #[must_use]
    pub fn minute_at(&self, y: f32) -> f32 {
        self.axis.minute_of(y - self.origin_y)
    }

    /// Convert an absolute point into the axis-local pixel pair the engine's
    /// [`enzui_core::schedule::ScheduleDrag`] expects.
    #[must_use]
    pub fn axis_local(&self, point: Point) -> (f32, f32) {
        (point.x, point.y - self.origin_y)
    }

    /// Resolve a point to what the user pressed on.
    ///
    /// Order is deliberate: blocks beat the empty slot beneath, and the
    /// resize edge beats the block body — the bottom 6px starts a resize,
    /// not a move. Blocks test in reverse so a later (on-top) block wins the
    /// press, matching paint order.
    #[must_use]
    pub fn hit(&self, point: Point) -> ScheduleHit {
        if !self.bounds.contains(point) {
            return ScheduleHit::None;
        }
        if self.header.height > 0.0 && self.header.contains(point) {
            return match self.column_at(point.x) {
                Some(c) => ScheduleHit::Header { column: c },
                None => ScheduleHit::None,
            };
        }
        if self.all_day_band.height > 0.0 && self.all_day_band.contains(point) {
            for chip in self.all_day_chips.iter().rev() {
                if chip.rect.contains(point) {
                    return ScheduleHit::AllDayBlock { id: chip.id };
                }
            }
            return match self.column_at(point.x) {
                Some(c) => ScheduleHit::AllDaySlot { column: c },
                None => ScheduleHit::None,
            };
        }
        if !self.grid.contains(point) {
            return ScheduleHit::None;
        }
        for b in self.blocks.iter().rev() {
            if b.rect.contains(point) {
                let bottom = b.rect.y + b.rect.height;
                if (bottom - point.y).abs() <= RESIZE_EDGE_PX {
                    return ScheduleHit::BlockResizeEdge { id: b.id };
                }
                return ScheduleHit::Block { id: b.id };
            }
        }
        match self.column_at(point.x) {
            Some(column) => ScheduleHit::Slot {
                column,
                minute: self.minute_at(point.y).floor() as i32,
            },
            None => ScheduleHit::None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct SpanningAllDayPlacement {
    id: EventId,
    start_column: usize,
    end_column: usize,
    slot: usize,
    source: usize,
}

// Stable start/longest-first/ID ordering keeps span rows independent of caller order.
fn layout_spanning_all_day(
    blocks: &[ScheduleBlock],
    column_count: usize,
) -> Vec<SpanningAllDayPlacement> {
    let mut spans: Vec<_> = blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| b.all_day)
        .collect();
    spans.sort_by_key(|(_, b)| (b.column, std::cmp::Reverse(b.end_column), b.id));
    let mut row_ends: Vec<usize> = Vec::new();
    let mut out = Vec::with_capacity(spans.len());
    for (source, b) in spans {
        if b.column >= column_count {
            continue;
        }
        let end = b.end_column.max(b.column).min(column_count - 1);
        let slot = row_ends
            .iter()
            .position(|&prior_end| prior_end < b.column)
            .unwrap_or_else(|| {
                row_ends.push(usize::MAX);
                row_ends.len() - 1
            });
        row_ends[slot] = end;
        out.push(SpanningAllDayPlacement {
            id: b.id,
            start_column: b.column,
            end_column: end,
            slot,
            source,
        });
    }
    out
}

fn to_timed_segments(blocks: &[ScheduleBlock], column_count: usize) -> Vec<ScheduleEvent<usize>> {
    let mut out = Vec::new();
    for (source, b) in blocks.iter().enumerate().filter(|(_, b)| !b.all_day) {
        if b.column >= column_count {
            continue;
        }
        let last = b.end_column.max(b.column).min(column_count - 1);
        for column in b.column..=last {
            let start = if column == b.column {
                b.start_minute
            } else {
                0
            };
            let end = if column == last {
                b.end_minute
            } else {
                24 * 60
            };
            out.push(ScheduleEvent {
                id: b.id,
                column,
                start_minute: start,
                end_minute: end,
                all_day: false,
                payload: source,
            });
        }
    }
    out
}
