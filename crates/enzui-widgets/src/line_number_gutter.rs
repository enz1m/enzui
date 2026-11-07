//! [`enz_line_number_gutter`] — a line-number gutter that stays aligned under
//! word wrapping.
//!
//! # Visual-row alignment invariant
//!
//! A naïve editor gutter numbers logical lines (`1..=n`, one number per
//! `\n`-delimited line) and renders them as a plain `text` widget, so each
//! number occupies exactly one visual row. The text surface beside it
//! wraps long logical lines across several visual rows. The two columns then
//! drift apart by one row per wrap: at a narrow viewport the number `2` ends
//! up beside the *continuation* of logical line 1, and every number below is
//! wrong, the error accumulating downward. The bug is invisible at a wide
//! viewport (nothing wraps) and only appears as the width narrows — precisely
//! the class a widget should make impossible.
//!
//! The standard editor convention is to emit **one gutter entry per
//! visual row**, carrying the logical line number on a line's *first* visual
//! row and a blank on its continuation rows. Getting that right requires
//! knowing where the text wraps, which requires text measurement against the
//! actual font/size/width — framework territory
//! ([`crate::text_measure`] already owns exact shaping).
//!
//! # What it does
//!
//! [`gutter_rows`] is the pure core: given the buffer, the typography
//! ([`MeasureTypography`]), the [`Wrapping`] mode and the text surface's
//! content width, it returns one [`GutterRow`] per visual row. Under
//! [`Wrapping::None`] it degenerates to one row per logical line. Under
//! [`Wrapping::Word`] it measures each logical
//! line's wrapped [`line_count`](crate::text_measure::TextMeasurement::line_count)
//! and emits `count - 1` blank continuation rows after each numbered row.
//!
//! [`enz_line_number_gutter`] wraps that in an iced [`Element`]: a
//! [`Palette`](enzui_theme::Palette)-toned surface ([`surface_alt`] fill, [`text_subtle`] digits)
//! whose fixed width derives from the exact rendered width of the widest line
//! number in the resolved typography (via [`enz_text_measure_width`]), not a
//! pixels-per-char guess. It scrolls in lockstep with the text surface by
//! sharing the caller's scrollable `Id` / offset. Pass the editor's current
//! absolute offset through [`GutterConfig::scroll_offset`] too: if responsive
//! layout reconstructs the gutter's retained scroll state, the replacement is
//! seeded at that viewport instead of briefly (or permanently) painting a
//! windowed gutter's empty leading spacer.
//!
//! A scrolled wrapped editor must also use [`GutterConfig::window`] with the
//! editor's own first logical line and that line's content-space top. Editors
//! may keep a bounded/sparse visual-row index, so their absolute offset is not
//! invertible through a separately shaped whole-document gutter: doing so put
//! logical line 130 beside labels 34, 66, or 77 as the width changed. The
//! window model consumes the editor's logical-line/visual-row mapping directly;
//! it does not infer a logical number from the offset or viewport width.
//!
//! [`surface_alt`]: enzui_theme::Palette::surface_alt
//! [`text_subtle`]: enzui_theme::Palette::text_subtle

use crate::text_measure::{MeasureTypography, enz_text_measure_width, enz_text_measure_with};
use iced::advanced::text::{LineHeight, Wrapping};
use iced::advanced::widget::Id;
use iced::widget::{container, scrollable, text};
use iced::{Element, Length, Padding};

/// One visual row of the gutter.
///
/// `number` is `Some(n)` on the first visual row of logical line `n`
/// (1-based) and `None` on every wrapped continuation row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GutterRow {
    /// The logical line number on this row's *first* visual row, else `None`.
    pub number: Option<usize>,
}

impl GutterRow {
    /// A numbered (first) visual row.
    pub const fn numbered(n: usize) -> Self {
        Self { number: Some(n) }
    }

    /// A blank continuation row (a wrapped tail of the line above).
    pub const fn blank() -> Self {
        Self { number: None }
    }
}

/// Split `buffer` into logical lines the way an editor gutter counts them.
///
/// One entry per `\n`-delimited segment. A trailing `\n` does not create a
/// phantom empty final line (matches how an app's `total_lines` counts,
/// and how a text surface renders — the caret after a trailing newline sits on
/// a real line, but the buffer as displayed has `count('\n') + 1` lines only
/// when there is content after the last newline). The empty buffer is one
/// line.
///
/// Concretely, this mirrors `str::lines` semantics: `"a\nb"` → 2, `"a\nb\n"` →
/// 2, `""` → 1, `"\n"` → 2 (an empty first line then an empty second? no — see
/// below).
fn logical_lines(buffer: &str) -> Vec<&str> {
    if buffer.is_empty() {
        return vec![""];
    }
    // `split('\n')` on "a\nb\n" yields ["a","b",""] — a trailing newline
    // produces a trailing empty segment, which is the standard editor view
    // (an empty final line the caret can rest on). We keep it: a gutter for a
    // file ending in a newline shows the blank final line's number.
    buffer.split('\n').collect()
}

/// The pure core: compute the gutter's visual rows for `buffer` under the given
/// typography, wrapping mode and text-surface content width.
///
/// * [`Wrapping::None`] — one row per logical line, numbered `1..=n`, no
///   blanks (today's behaviour; the degenerate case).
/// * [`Wrapping::Word`] / [`Wrapping::Glyph`] / [`Wrapping::WordOrGlyph`] —
///   each logical line is measured at `content_width` and contributes its
///   numbered first row followed by `visual_rows - 1` blank continuation rows,
///   so the gutter stays aligned with the wrapped text.
///
/// The number carried by logical line `n` is always `n`, at every width —
/// only the count of blank rows after it changes as the width narrows. That is
/// the invariant the naïve `1..=n`-as-text gutter violates.
///
/// A non-positive or non-finite `content_width` cannot be measured against, so
/// wrapping is impossible to compute and the function degrades to the
/// [`Wrapping::None`] result (one row per logical line) rather than shaping
/// against garbage.
pub fn gutter_rows(
    buffer: &str,
    typo: MeasureTypography,
    wrapping: Wrapping,
    content_width: f32,
) -> Vec<GutterRow> {
    let lines = logical_lines(buffer);

    let wraps = matches!(
        wrapping,
        Wrapping::Word | Wrapping::Glyph | Wrapping::WordOrGlyph
    ) && content_width.is_finite()
        && content_width > 0.0;

    let mut rows = Vec::with_capacity(lines.len());
    for (i, line) in lines.iter().enumerate() {
        let n = i + 1;
        rows.push(GutterRow::numbered(n));

        if wraps {
            // An empty logical line occupies exactly one visual row (there is
            // nothing to wrap); `enz_text_measure_with` returns line_count 0
            // for empty text, so guard it to 1.
            let visual = if line.is_empty() {
                1
            } else {
                enz_text_measure_with(line, content_width, typo)
                    .line_count
                    .max(1)
            };
            for _ in 1..visual {
                rows.push(GutterRow::blank());
            }
        }
    }
    rows
}

/// The exact rendered width (logical px) a gutter needs to show every number
/// up to `highest`, in `typo`, plus symmetric horizontal padding `pad_x` on
/// each side.
///
/// Derived from the tight advance width of the widest label string (the digit
/// string of `highest`, e.g. `"128"` for a 128-line buffer) measured in the
/// actual typography via [`enz_text_measure_width`] — not a
/// `digits * pixels_per_char + slack` guess, which is correct for exactly one
/// font at one size. A minimum of two digits is reserved so a small file's
/// gutter does not visibly jump wider as it grows from 9 to 10 lines.
pub fn gutter_width(highest: usize, typo: MeasureTypography, pad_x: f32) -> f32 {
    let digits = highest.max(1).to_string().len().max(2);
    // Measure the widest *possible* label at this digit count. '0' is a safe
    // representative monospace digit; every ASCII digit shares one advance in a
    // monospace font, and the gutter renders in monospace.
    let widest = "0".repeat(digits);
    let text_w = enz_text_measure_width(&widest, typo);
    text_w + 2.0 * pad_x.max(0.0)
}

/// Builds integrated editor numbering with width measured in the editor's typography.
/// Unlike an independent gutter, this consumes the editor's own shaped line heads during drawing.
pub fn editor_gutter(
    highest: usize,
    typo: MeasureTypography,
    pad_x: f32,
) -> iced::advanced::text::editor::gutter::Gutter {
    iced::advanced::text::editor::gutter::Gutter::new(gutter_width(highest, typo, pad_x))
        .padding(pad_x)
}

/// Where a windowed gutter's visible slice sits in the whole column.
///
/// Every field is in the same content coordinates the text surface reports
/// through `text_editor::Content::scroll_metrics`, so a caller passes what the
/// editor measured rather than a guess. That matters with wrapping on, where a
/// logical line covers a variable number of visual rows and the offset of a
/// line cannot be derived from its index.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GutterWindow {
    /// The 0-based logical line the supplied buffer slice starts at.
    pub first_line: usize,
    /// The content-space `y` of that line's top edge.
    pub leading_height: f32,
    /// The full laid-out content height of the text surface, so the column
    /// keeps the same scroll travel as the document it decorates.
    pub total_height: f32,
}

/// Configuration for [`enz_line_number_gutter`].
#[derive(Debug, Clone)]
#[must_use]
pub struct GutterConfig {
    /// Typography of the *text surface* this gutter sits beside — the same
    /// font/size/weight, so wrap points and the derived width match what the
    /// editor actually paints.
    pub typo: MeasureTypography,
    /// The text surface's wrapping mode. [`Wrapping::None`] gives one row per
    /// logical line.
    pub wrapping: Wrapping,
    /// The text surface's content width (the width text wraps against), in
    /// logical px. Non-positive → the gutter falls back to no-wrap numbering.
    pub content_width: f32,
    /// Horizontal padding on each side of the digits (default 8).
    pub pad_x: f32,
    /// Top inset so the first number aligns with the first text row (default 0).
    pub pad_top: f32,
    /// Bottom inset so the gutter's scroll viewport matches the text surface's
    /// When the text surface insets its content on all four
    /// sides (`text_editor().padding(N)`), the gutter must inset its top
    /// and bottom by the same amount or the two viewports differ in height
    /// — the gutter runs out of travel `N` px before the editor does, and the
    /// last few lines drift apart.
    ///
    /// Default 0. Set it to the text surface's bottom padding.
    pub pad_bottom: f32,
    /// The line height (per-row vertical advance) the gutter's digits render
    /// at, in the same units as the text surface it sits beside.
    ///
    /// This is the load-bearing input for row-height parity. The
    /// gutter is a `text` widget and the surface it decorates is (typically) an
    /// iced `text_editor`; both take iced's default
    /// [`LineHeight::Relative`]`(1.3)` when nothing is set, so their per-row
    /// advance matches by default. But that parity is accidental — it
    /// holds only while neither side sets an explicit line height. The moment
    /// the text surface pins its own `line_height`, the gutter (still on the
    /// default) drifts by `(N-1) * delta` on line `N`: zero at the top of the
    /// file, obvious further down. Pin this to the same value the text surface
    /// uses and the two columns advance in lockstep at every line.
    ///
    /// `None` keeps iced's default.
    pub line_height: Option<LineHeight>,
    /// Viewport window: render digits only for the lines the text
    /// surface can currently show, with the rest of the column held open by
    /// two spacers.
    ///
    /// `None` (the default) numbers every line in `buffer`, which is right for
    /// an unscrolled document or a no-wrap editor whose offset shares this
    /// gutter's one-row-per-line coordinate system. A scrolled wrapped editor
    /// must set a window even when its document is small: its own sparse
    /// visual-row index is the authority for `first_line` / `leading_height`,
    /// and independently shaping every preceding line would map the editor's
    /// offset to the wrong logical number. A large document must also window:
    /// the gutter is rebuilt on every frame, so a 40 MB document costs a
    /// full walk plus a multi-megabyte label per frame (measured 5.8 s per
    /// keystroke). With a window set, the caller passes only the visible slice
    /// of the buffer and the geometry that places it.
    pub window: Option<GutterWindow>,
    /// Optional scrollable id, shared with the text surface so the gutter
    /// scrolls in lockstep. `None` leaves the gutter non-scrollable (it then
    /// relies on being clipped to the row's height).
    pub scroll_id: Option<Id>,
    /// The text surface's current absolute vertical scroll offset.
    ///
    /// This seeds a gutter scrollable whose retained widget state has just
    /// been reconstructed (for example, when a responsive split collapses on
    /// resize). iced otherwise starts that replacement state at zero. A
    /// windowed gutter then paints only its leading spacer because its digits
    /// live near this offset, making the gutter appear blank until another
    /// scroll happens. The seed is consumed only by fresh scrollable state, so
    /// it does not fight normal scrolling; [`GutterConfig::scroll_id`] remains
    /// the route for subsequent corrections.
    pub scroll_offset: f32,
    /// Embedded: when `true` the gutter paints **no block of
    /// its own** — the container background is `None`, so the editor's
    /// surface shows through the digit column. Digit ink still resolves to
    /// `text_subtle` from the runtime palette (the tone tracks the theme
    /// regardless).
    ///
    /// The default is `false`, which keeps the hardcoded `surface_alt`
    /// wash a stand-alone gutter paints.
    pub embedded: bool,
}

impl GutterConfig {
    /// A gutter config for the given text-surface typography, defaulting to
    /// [`Wrapping::None`] and an unset content width.
    pub fn new(typo: MeasureTypography) -> Self {
        Self {
            typo,
            wrapping: Wrapping::None,
            content_width: 0.0,
            pad_x: 8.0,
            pad_top: 0.0,
            pad_bottom: 0.0,
            line_height: None,
            scroll_id: None,
            scroll_offset: 0.0,
            embedded: false,
            window: None,
        }
    }

    /// Render only the lines the text surface can show.
    ///
    /// The `buffer` handed to [`enz_line_number_gutter`] must then be **just
    /// that slice** of the document; `window.first_line` says which logical
    /// line it starts at, and the two heights place it inside the full column.
    /// For a scrolled wrapped editor these values must come from the editor's
    /// own laid-out viewport mapping, never from `scroll_offset / row_height`
    /// or from independently reshaping the document prefix.
    pub fn window(mut self, window: GutterWindow) -> Self {
        self.window = Some(window);
        self
    }

    /// [`GutterConfig::window`] for a caller that decides per frame whether a
    /// document is large enough to window at all.
    pub fn maybe_window(mut self, window: Option<GutterWindow>) -> Self {
        self.window = window;
        self
    }

    /// Ask the gutter to inherit the surface painted behind it, instead
    /// of laying down its own `surface_alt` wash. Use this when the
    /// gutter sits beside a text editor whose parent already paints a
    /// coherent surface — e.g. An app's embedded editor — so
    /// there is no visible block edge where the gutter meets the text.
    ///
    /// The default is `false`: a stand-alone gutter still paints its own
    /// muted wash so it reads as a separated column beside a raw
    /// `text_editor`.
    pub fn embedded(mut self, embedded: bool) -> Self {
        self.embedded = embedded;
        self
    }

    /// Set the wrapping mode (match the text surface).
    pub fn wrapping(mut self, wrapping: Wrapping) -> Self {
        self.wrapping = wrapping;
        self
    }

    /// Set the content width text wraps against (match the text surface).
    pub fn content_width(mut self, w: f32) -> Self {
        self.content_width = w;
        self
    }

    /// Horizontal padding on each side of the digits.
    pub fn pad_x(mut self, p: f32) -> Self {
        self.pad_x = p;
        self
    }

    /// Top inset aligning the first number with the first text row.
    pub fn pad_top(mut self, p: f32) -> Self {
        self.pad_top = p;
        self
    }

    /// Bottom inset matching the text surface's bottom padding.
    pub fn pad_bottom(mut self, p: f32) -> Self {
        self.pad_bottom = p;
        self
    }

    /// Set both `pad_top` and `pad_bottom` to the text surface's internal
    /// padding in one call.
    ///
    /// This is the convenience path for a gutter that sits beside an
    /// [`EnzTextArea`](crate::text_area::EnzTextArea) or an iced
    /// `text_editor().padding(p)`: the editor insets its content on all four
    /// sides by `p`, and the gutter must match both ends so their scroll
    /// viewports are the same height. Call `.text_surface_padding(p)` instead
    /// of `.pad_top(p).pad_bottom(p)`.
    pub fn text_surface_padding(mut self, p: f32) -> Self {
        self.pad_top = p;
        self.pad_bottom = p;
        self
    }

    /// Pin the gutter's per-row line height to match the text surface, for
    /// guaranteed row-height parity. Pass the same
    /// `iced::advanced::text::LineHeight` the text surface uses
    /// (an `f32` factor coerces via [`From`], e.g. `.line_height(1.3)`).
    pub fn line_height(mut self, lh: impl Into<LineHeight>) -> Self {
        self.line_height = Some(lh.into());
        self
    }

    /// Share a scrollable id with the text surface for lockstep scrolling.
    pub fn scroll_id(mut self, id: impl Into<Id>) -> Self {
        self.scroll_id = Some(id.into());
        self
    }

    /// Seed a newly reconstructed gutter from the text surface's current
    /// absolute vertical offset.
    ///
    /// This is not a controlled offset: it is applied once per fresh iced
    /// scrollable state, then gestures and [`GutterConfig::scroll_id`]
    /// operations own subsequent movement. Pass the same laid-out offset the
    /// decorated editor reports, so a responsive re-parent cannot reset the
    /// gutter to the top while the editor remains mid-document.
    pub fn scroll_offset(mut self, offset: f32) -> Self {
        self.scroll_offset = if offset.is_finite() {
            offset.max(0.0)
        } else {
            0.0
        };
        self
    }
}

/// Render the gutter string from a list of [`GutterRow`]s, right-aligning the
/// digits to `width_chars` and blanking continuation rows.
pub(crate) fn gutter_text(rows: &[GutterRow], width_chars: usize) -> String {
    rows.iter()
        .map(|r| match r.number {
            Some(n) => format!("{n:>width_chars$}"),
            None => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The gutter rows for `buffer`, renumbered from the window's first line when
/// one is set.
fn gutter_rows_for(buffer: &str, config: &GutterConfig) -> Vec<GutterRow> {
    let rows = gutter_rows(buffer, config.typo, config.wrapping, config.content_width);
    let Some(window) = config.window else {
        return rows;
    };

    rows.into_iter()
        .map(|row| match row.number {
            Some(n) => GutterRow::numbered(n + window.first_line),
            None => row,
        })
        .collect()
}

/// The highest line number the column must be able to show.
///
/// With a window that is the *document's* last line -- estimated from the full
/// content height the caller reports -- not the last line of the visible
/// slice, or the column would narrow and widen as the user scrolls.
fn highest_number(rows: &[GutterRow], config: &GutterConfig) -> usize {
    let in_rows = rows.iter().filter_map(|r| r.number).max().unwrap_or(1);
    match &config.window {
        Some(window) => {
            let rows_total = (window.total_height / row_height_px(config))
                .round()
                .max(1.0);
            (rows_total as usize).max(in_rows)
        }
        None => in_rows,
    }
}

/// The exact text a gutter renders for `buffer` under `config`.
///
/// This is what [`enz_line_number_gutter`] shapes, exposed so a caller (and a
/// test) can read the numbers the column will actually show for a window
/// rather than inferring them.
#[must_use]
pub fn gutter_window_label(buffer: &str, config: &GutterConfig) -> String {
    let rows = gutter_rows_for(buffer, config);
    let width_chars = highest_number(&rows, config).to_string().len().max(2);
    gutter_text(&rows, width_chars)
}

/// The per-row vertical advance a gutter renders at, in logical px.
fn row_height_px(config: &GutterConfig) -> f32 {
    let size = config.typo.size_px();
    match config.line_height {
        Some(line_height) => f32::from(line_height.to_absolute(iced::Pixels(size))),
        None => f32::from(LineHeight::default().to_absolute(iced::Pixels(size))),
    }
    .max(1.0)
}

/// Build a line-number gutter [`Element`] beside a text surface.
///
/// One visual row per gutter row (numbered first row, blank continuation
/// rows), toned from the theme [`Palette`](enzui_theme::Palette)
/// (`surface_alt` fill, `text_subtle` digits), width derived from the highest
/// line number's rendered width, scrolling in lockstep with the caller's text
/// surface via the shared scrollable `Id`.
pub fn enz_line_number_gutter<'a, Message>(
    buffer: &str,
    config: GutterConfig,
) -> Element<'a, Message>
where
    Message: 'a,
{
    let rows = gutter_rows_for(buffer, &config);
    let highest = highest_number(&rows, &config);
    let body = gutter_window_label(buffer, &config);
    let width = gutter_width(highest, config.typo, config.pad_x);

    let size = config.typo.size_px();

    // The painted family must match the descriptor used by `gutter_width`.
    let label_font = config.typo.font();
    let mut label = text(body)
        .size(size)
        .font(label_font)
        .shaping(config.typo.shaping.into())
        .style(|theme: &iced::Theme| text::Style {
            color: Some(crate::theme_lookup::palette(theme).text_subtle),
        });
    // Pin the per-row line height to the text surface's when the caller
    // supplies it, so the digit column advances in lockstep with the editor's
    // lines regardless of what line height the editor uses. Unset => iced's
    // default.
    if let Some(lh) = config.line_height {
        label = label.line_height(lh);
    }

    let inner: Element<'a, Message> = {
        // Population-gate exception (`line_number_gutter`).
        //
        // The gutter does not get the framework overlay policy: it is a gutter,
        // not a pane. There is no thumb to paint here because the gutter never
        // scrolls on its own — it is driven from the editor's scrollable through
        // a shared `scroll_id`, and its digits ride the editor's translation.
        // Painting a 4 px capsule over the line numbers would be a decoration
        // over content that already has no independent scroll semantics.
        //
        // Both the structural width and the painted thumb are therefore zero,
        // so upstream cannot report a hovered-scrollbar or draw a thumb even if
        // the editor's viewport ends up shorter than the gutter's. The overlay
        // policy's `hit_width`/`width` are wrong here — they would introduce a
        // 12 px hit target over the digits.
        //
        // Kept as `scrollable(label).direction(..)` rather than
        // `enz_scrollable(label)` so this exception is loud rather than a bar
        // that happens to be zero-width; the population gate names this file
        // and this specific direction override.
        // A windowed gutter holds the rest of the column open with two
        // spacers, so the scrollable keeps the document's real travel while
        // only the visible digits are laid out and shaped.
        let content: Element<'a, Message> = match &config.window {
            Some(window) => {
                let row_height = row_height_px(&config);
                let leading = window.leading_height.max(0.0);
                let shown = rows.len() as f32 * row_height;
                let trailing = (window.total_height - leading - shown).max(0.0);

                iced::widget::column![
                    iced::widget::Space::new().height(Length::Fixed(leading)),
                    label,
                    iced::widget::Space::new().height(Length::Fixed(trailing)),
                ]
                .into()
            }
            None => label.into(),
        };
        let mut scrollable_body = scrollable(content)
            .direction(scrollable::Direction::Vertical(
                scrollable::Scrollbar::new().width(0).scroller_width(0),
            ))
            // A responsive layout may reconstruct this scrollable while the
            // editor's Content (and therefore its viewport) survives. Seed the
            // replacement from that same authority instead of painting offset
            // zero, where a windowed gutter contains only its leading spacer.
            // iced consumes this only on a state's first layout.
            .initial_offset(scrollable::AbsoluteOffset {
                x: 0.0,
                y: config.scroll_offset,
            });
        if let Some(id) = config.scroll_id {
            scrollable_body = scrollable_body.id(id);
        }
        scrollable_body.into()
    };

    container(inner)
        .width(Length::Fixed(width))
        .height(Length::Fill)
        .padding(Padding {
            top: config.pad_top,
            right: config.pad_x,
            bottom: config.pad_bottom,
            left: config.pad_x,
        })
        .style({
            let embedded = config.embedded;
            move |theme: &iced::Theme| {
                let pal = crate::theme_lookup::palette(theme);
                container::Style {
                    // Embedded: paint nothing so the editor's own
                    // surface reaches the digit column. `text_subtle` still
                    // resolves against the runtime palette regardless.
                    background: if embedded {
                        None
                    } else {
                        Some(iced::Background::Color(pal.surface_alt))
                    },
                    ..container::Style::default()
                }
            }
        })
        .into()
}
