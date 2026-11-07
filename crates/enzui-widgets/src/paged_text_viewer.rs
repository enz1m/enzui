//! Read-only visible-row viewer for [`enzui_core::paged_text`].
//!
//! The caller owns the state, executes its read operations off-thread and
//! handles navigation messages. The model materializes bounded overscan, but
//! only its visible slice is mounted and announced.
//!
//! Responsive views resolve row capacity during layout. With both dimensions
//! fixed, capacity uses the requested size before layout, even if a parent later
//! constrains it. Capacity is at least one even when no full row fits;
//! document paint is clipped. [`PagedTextViewportState`] retains the last
//! resolved capacity for update-time navigation.
//!
//! Sparse seeks report source bytes separately from absolute-line index
//! coverage. Unknown line numbers use ellipses; index percentage is diagnostic.
//! The overlay scrollbar reports byte progress and emits `SeekByte` navigation;
//! its thumb uses visible byte coverage without reading or indexing extra pages.
//! Wheel and keyboard navigation remain display-row based.
//!
//! [`enz_paged_byte_viewer`] composes fixed-width hexadecimal/ASCII rows from a
//! prepared [`PagedByteState`]. Its [`PagedByteViewport`] receives original byte
//! addresses; status and accessibility describe the original source rather than
//! formatting overhead. Horizontal overflow remains inside the shared scroll view.

use crate::focus_button::focus_button_maybe;
use crate::keyboard_capture::keyboard_capture;
use crate::offset_scrollbar::enz_offset_scrollbar;
use crate::semantics_container::SemanticsContainer;
use crate::text::{EnzText, TextSize, TextTone, enz_text, line_box_px};
use crate::wheel_capture::wheel_capture;
use enzui_core::paged_bytes::{BytePreviewLayout, PagedByteState, PagedByteViewport};
use enzui_core::paged_text::{
    PagedTextFrame, PagedTextNavigation, PagedTextRow, PagedTextState, PagedTextViewport,
};
use enzui_core::semantics::{Role, Semantics};
use enzui_theme::{EnzTheme, FontRole, ScrollbarTokens, SpaceToken, TypographyScale};
use iced::keyboard::{self, key::Named};
use iced::mouse;
use iced::widget::{column, container, row};
use iced::{Element, Length, Padding, Size};
use std::sync::Arc;
const STATUS_INSET_TOKEN: SpaceToken = SpaceToken::Sm;
const ROW_PAD_Y: f32 = 2.0;
// Non-finite responsive heights use a bounded fallback; finite zero remains real.
const UNBOUNDED_FALLBACK_ROWS: usize = 24;

fn document_text_font(text: EnzText, theme: Option<&EnzTheme>) -> EnzText {
    match theme {
        Some(theme) => text.font_role(FontRole::Monospace).font_from_theme(theme),
        None => text,
    }
}

/// Geometry used to materialize one viewer frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PagedTextViewportMetrics {
    /// Resolved size, including status lines; fixed views use their requested size.
    pub size: Size,
    /// Height reserved for document rows above the status lines.
    pub body_height: f32,
    /// Pitch of one document row, including its vertical padding.
    pub row_height: f32,
    /// Materialization capacity, with a minimum of one even when no complete row fits.
    pub visible_rows: usize,
}

/// Retained geometry published during eager building or responsive layout.
/// Values remain unchanged until the next build or layout; keep this in application
/// state and attach it with [`EnzPagedTextViewer::viewport_state`].
#[derive(Debug, Default)]
pub struct PagedTextViewportState(std::cell::Cell<Option<PagedTextViewportMetrics>>);

impl PagedTextViewportState {
    /// Create unresolved state before the viewer first builds or lays out.
    #[must_use]
    pub const fn new() -> Self {
        Self(std::cell::Cell::new(None))
    }

    /// Return the last published geometry, or `None` before the first resolution.
    #[must_use]
    pub fn resolved(&self) -> Option<PagedTextViewportMetrics> {
        self.0.get()
    }

    /// Return the last published row capacity, or `fallback` unchanged if unresolved.
    /// The published value can be stale until the next viewer build or layout.
    #[must_use]
    pub fn visible_rows_or(&self, fallback: usize) -> usize {
        self.resolved().map_or(fallback, |m| m.visible_rows)
    }

    fn publish(&self, metrics: PagedTextViewportMetrics) {
        self.0.set(Some(metrics));
    }
}

/// Materialized, renderer-independent description that builds
/// one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PagedTextViewModel {
    /// Visible plus overscan rows, and no other document text.
    pub rows: Vec<PagedTextRow>,
    /// Range of rows mounted and announced; overscan outside it is not mounted.
    pub visible: std::ops::Range<usize>,
    /// Current visible source range. This describes source bytes (and exact
    /// lines when known), never line-index completion.
    pub range_announcement: String,
    /// Optional, distinct explanation of partial absolute-line-number
    /// coverage. `None` means the whole source is indexed.
    pub line_number_status: Option<String>,
    /// Rounded line-index coverage retained for programmatic diagnostics.
    ///
    /// This is intentionally not painted or announced: it is not document
    /// load progress, and sparse end-of-file navigation can legitimately leave
    /// it low.
    pub index_percent: u8,
}

impl PagedTextViewModel {
    /// Materialize a bounded window with the supplied capacity plus policy overscan.
    /// The visible range can contain fewer rows than requested; materialization normalizes
    /// the capacity to at least one. This method does not measure or paint a widget.
    #[must_use]
    pub fn from_state(
        state: &PagedTextState,
        viewport: &PagedTextViewport,
        visible_rows: usize,
    ) -> Self {
        Self::from_frame(viewport.materialize_rows(state, visible_rows), state)
    }

    fn from_frame(frame: PagedTextFrame, state: &PagedTextState) -> Self {
        let visible = frame.visible.clone();
        let rows = frame.rows;
        let announced: &[PagedTextRow] = rows.get(visible.clone()).unwrap_or(&[]);
        let all_visible_lines_known =
            !announced.is_empty() && announced.iter().all(|row| row.line_number.is_some());
        let range_announcement = announced.first().zip(announced.last()).map_or_else(
            || {
                if state.failure().is_some() {
                    enzui_core::localized!("enzui-paged-text-viewer-text-unavailable-source-is-bytes", "bytes" => state.source_len())
                } else if state.source_len() == 0 {
                    enzui_core::localization::text(
                        "enzui-paged-text-viewer-source-is-empty-0-bytes",
                    )
                } else {
                    enzui_core::localized!("enzui-paged-text-viewer-loading-requested-text-from-byte-source", "bytes" => state.source_len())
                }
            },
            |(first, last)| match (first.line_number, last.line_number) {
                (Some(a), Some(b)) => enzui_core::localized!("enzui-paged-text-viewer-lines-bytes-of", "first-line" => a, "last-line" => b, "start" => first.byte_range.start, "end" => last.byte_range.end, "total" => state.source_len()),
                _ => enzui_core::localized!("enzui-paged-text-viewer-bytes-of", "start" => first.byte_range.start, "end" => last.byte_range.end, "total" => state.source_len()),
            },
        );
        let index_percent = (state.index_progress() * 100.0).round() as u8;
        let line_number_status = (!state.line_index_complete()).then(|| {
            if all_visible_lines_known {
                enzui_core::localization::text("enzui-paged-text-partial-index")
            } else {
                enzui_core::localization::text(
                    "enzui-paged-text-viewer-line-numbers-not-indexed-here",
                )
            }
        });
        Self {
            rows,
            visible,
            range_announcement,
            line_number_status,
            index_percent,
        }
    }

    /// Full status spoken for the status node and document value.
    #[must_use]
    pub fn status_announcement(&self) -> String {
        self.line_number_status.as_ref().map_or_else(
            || self.range_announcement.clone(),
            |line_status| enzui_core::localized!("enzui-paged-text-status", "status" => self.range_announcement, "line-status" => line_status),
        )
    }
}

/// Shared large-document viewer builder.
pub struct EnzPagedTextViewer<'a, Message> {
    state: &'a PagedTextState,
    byte_layout: Option<BytePreviewLayout>,
    viewport: PagedTextViewport,
    label: String,
    width: Length,
    height: Length,
    typography: Option<TypographyScale>,
    text_size_px: Option<f32>,
    font_theme: Option<&'a EnzTheme>,
    viewport_state: Option<&'a PagedTextViewportState>,
    scrollbar_tokens: ScrollbarTokens,
    backdrop: Option<iced::Color>,
    on_navigate: Arc<dyn Fn(PagedTextNavigation) -> Message>,
}
// Deriving Clone would unnecessarily require Message: Clone for the Arc callback.
impl<Message> Clone for EnzPagedTextViewer<'_, Message> {
    fn clone(&self) -> Self {
        Self {
            state: self.state,
            byte_layout: self.byte_layout,
            viewport: self.viewport,
            label: self.label.clone(),
            width: self.width,
            height: self.height,
            typography: self.typography,
            text_size_px: self.text_size_px,
            font_theme: self.font_theme,
            viewport_state: self.viewport_state,
            scrollbar_tokens: self.scrollbar_tokens,
            backdrop: self.backdrop,
            on_navigate: Arc::clone(&self.on_navigate),
        }
    }
}

/// Construct the shared paged-text viewer.
#[must_use]
pub fn enz_paged_text_viewer<'a, Message: Clone + 'static>(
    state: &'a PagedTextState,
    viewport: &PagedTextViewport,
    on_navigate: impl Fn(PagedTextNavigation) -> Message + 'static,
) -> EnzPagedTextViewer<'a, Message> {
    EnzPagedTextViewer {
        state,
        byte_layout: None,
        viewport: *viewport,
        label: enzui_core::localization::text("enzui-paged-text-viewer-large-text-document"),
        width: Length::Fill,
        height: Length::Fill,
        typography: None,
        text_size_px: None,
        font_theme: None,
        viewport_state: None,
        scrollbar_tokens: enzui_theme::text_defaults::scrollbar(),
        backdrop: None,
        on_navigate: Arc::new(on_navigate),
    }
}

/// Read-only byte preview with original byte-address navigation/status and horizontal overflow.
/// Prepare the source and execute page operations off-thread. Pass navigation to
/// [`PagedByteViewport::navigate`]; scrollbar seeks carry original source addresses.
/// Rows in [`EnzPagedTextViewer::model_at`] retain formatted-source byte ranges.
#[must_use]
pub fn enz_paged_byte_viewer<'a, Message: Clone + 'static>(
    state: &'a PagedByteState,
    viewport: &PagedByteViewport,
    on_navigate: impl Fn(PagedTextNavigation) -> Message + 'static,
) -> EnzPagedTextViewer<'a, Message> {
    let layout = state.layout().unwrap_or(BytePreviewLayout::EMPTY);
    let mut viewer = enz_paged_text_viewer(
        state.text_state(),
        viewport.text_viewport(),
        move |movement| {
            let movement = match movement {
                PagedTextNavigation::SeekByte(byte) => {
                    let source_byte = if byte >= layout.display_len().saturating_sub(1) {
                        layout.source_len().saturating_sub(1)
                    } else {
                        layout.source_offset(byte)
                    };
                    PagedTextNavigation::SeekByte(source_byte)
                }
                movement => movement,
            };
            on_navigate(movement)
        },
    );
    viewer.byte_layout = Some(layout);
    viewer.label = enzui_core::localization::text("enzui-byte-preview-label");
    viewer
}

impl<'a, Message: Clone + 'static> EnzPagedTextViewer<'a, Message> {
    /// Override normalized overlay scrollbar geometry; no content gutter is reserved.
    #[must_use]
    pub fn scrollbar_tokens(mut self, tokens: ScrollbarTokens) -> Self {
        self.scrollbar_tokens = tokens.normalized();
        self
    }

    /// Resolve scrollbar ink against the document's actual surface colour.
    #[must_use]
    pub fn backdrop(mut self, color: iced::Color) -> Self {
        self.backdrop = Some(color);
        self
    }

    /// Override the accessible document name.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Request a specific width. Defaults to [`Length::Fill`].
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Request a specific height. Defaults to [`Length::Fill`].
    ///
    /// A fixed width and height together keep the deterministic eager path;
    /// any responsive axis resolves the row window from the real allocation.
    #[must_use]
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Set the typography scale for document rows, gutter and status text.
    /// Row capacity follows the Body line height unless an exact text size overrides it.
    #[must_use]
    pub fn typography(mut self, scale: TypographyScale) -> Self {
        self.typography = Some(scale);
        self
    }

    /// Use an exact logical-pixel size for document rows and line numbers.
    ///
    /// This overrides the body typography rung while status text keeps the
    /// active typography scale. Invalid values leave the current setting
    /// unchanged.
    #[must_use]
    pub fn text_size_px(mut self, px: f32) -> Self {
        if px.is_finite() && px > 0.0 {
            self.text_size_px = Some(px);
        }
        self
    }

    /// Resolve document rows and line numbers from the theme's monospace role.
    #[must_use]
    pub fn font_from_theme(mut self, theme: &'a EnzTheme) -> Self {
        self.font_theme = Some(theme);
        self
    }

    /// Publish the allocation used for realization into a retained,
    /// caller-readable state (see [`PagedTextViewportState`]).
    #[must_use]
    pub fn viewport_state(mut self, state: &'a PagedTextViewportState) -> Self {
        self.viewport_state = Some(state);
        self
    }

    /// Return row pitch in logical pixels: exact text line height or the Body
    /// typography line height, plus 4 px of vertical padding.
    #[must_use]
    pub fn row_height_px(&self) -> f32 {
        self.text_size_px.map_or_else(
            || TextSize::Body.resolve_line_height(self.typography.as_ref()),
            line_box_px,
        ) + 2.0 * ROW_PAD_Y
    }
    fn status_height_px(&self) -> f32 {
        let lines = if self.byte_layout.is_some() || self.state.line_index_complete() {
            1.0
        } else {
            2.0
        };
        lines * TextSize::Caption.resolve_line_height(self.typography.as_ref())
    }

    /// Resolve the realised geometry for one concrete allocation.
    #[must_use]
    pub fn metrics_at(&self, allocation: Size) -> PagedTextViewportMetrics {
        let row_height = self.row_height_px();
        let status = self.status_height_px();
        // Preserve finite zero instead of treating it as an unbounded allocation.
        let total_height = if allocation.height.is_finite() && allocation.height >= 0.0 {
            allocation.height
        } else {
            match self.height {
                Length::Fixed(h) => h,
                _ => UNBOUNDED_FALLBACK_ROWS as f32 * row_height + status,
            }
        };
        let body_height = (total_height - status).max(0.0);
        let visible_rows = (body_height / row_height).floor().max(1.0) as usize;
        PagedTextViewportMetrics {
            size: Size::new(allocation.width, total_height),
            body_height,
            row_height,
            visible_rows,
        }
    }

    /// The bounded model this viewer would build at `allocation`.
    #[must_use]
    pub fn model_at(&self, allocation: Size) -> PagedTextViewModel {
        let mut model = PagedTextViewModel::from_state(
            self.state,
            &self.viewport,
            self.metrics_at(allocation).visible_rows,
        );
        if let Some(layout) = self.byte_layout {
            let visible = model.rows.get(model.visible.clone()).unwrap_or(&[]);
            model.range_announcement = match (visible.first(), visible.last()) {
                _ if self.state.identity().is_none() => {
                    enzui_core::localization::text("enzui-paged-text-viewer-no-byte-source")
                }
                _ if self.state.failure().is_some() => {
                    enzui_core::localized!("enzui-paged-text-viewer-bytes-unavailable-source-is-bytes", "bytes" => layout.source_len())
                }
                _ if layout.source_len() == 0 => enzui_core::localization::text(
                    "enzui-paged-text-viewer-byte-source-is-empty-0-bytes",
                ),
                (Some(first), Some(last)) => {
                    let range = layout.source_range(first.byte_range.start..last.byte_range.end);
                    enzui_core::localized!("enzui-paged-text-viewer-bytes-of-read-only", "start" => range.start, "end" => range.end.saturating_sub(1), "total" => layout.source_len())
                }
                _ => {
                    enzui_core::localized!("enzui-paged-text-viewer-loading-byte-preview-from-byte-source", "bytes" => layout.source_len())
                }
            };
            model.line_number_status = None;
            if layout.source_len() == 0 {
                model.rows.clear();
                model.visible = 0..0;
            }
        }
        model
    }

    /// Returns blank rows reserved above a short terminal frame at `allocation`.
    ///
    /// The gap exists only when prior content exists and the terminal frame is
    /// shorter than the allocation. It is owned internally to preserve focus.
    #[must_use]
    pub fn tail_alignment_rows_at(&self, allocation: Size) -> usize {
        let visible_rows = self.metrics_at(allocation).visible_rows;
        let model = PagedTextViewModel::from_state(self.state, &self.viewport, visible_rows);
        self.tail_alignment_rows(&model, visible_rows)
    }

    fn tail_alignment_rows(&self, model: &PagedTextViewModel, visible_rows: usize) -> usize {
        let painted = model.rows.get(model.visible.clone()).unwrap_or(&[]).len();
        if painted == 0 {
            return 0;
        }
        let more_below = self.viewport.can_move_lines(self.state, visible_rows, 1);
        let more_above = self.viewport.can_move_lines(self.state, visible_rows, -1);
        if more_below || !more_above {
            return 0;
        }
        visible_rows.saturating_sub(painted)
    }

    /// Build a focusable document with Arrow/Page/Home/End navigation messages.
    /// Shift is accepted; Alt, Control and Logo suppress key navigation. Wheel input
    /// over the viewer emits movement only when it can move in that direction,
    /// otherwise leaving the event for an enclosing scrollable. Responsive dimensions
    /// use layout allocation; two fixed dimensions use the requested size eagerly.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        let (width, height) = (self.width, self.height);
        if let (Length::Fixed(w), Length::Fixed(h)) = (width, height) {
            return self.into_element_at(Size::new(w, h));
        }
        iced::widget::responsive(move |allocation| self.clone().into_element_at(allocation))
            .width(width)
            .height(height)
            .into()
    }

    fn into_element_at(self, allocation: Size) -> Element<'a, Message> {
        let metrics = self.metrics_at(allocation);
        if let Some(sink) = self.viewport_state {
            sink.publish(metrics);
        }
        let model = self.model_at(allocation);
        let can_wheel_up = self
            .viewport
            .can_move_lines(self.state, metrics.visible_rows, -1);
        let can_wheel_down = self
            .viewport
            .can_move_lines(self.state, metrics.visible_rows, 1);
        let source_len = self.state.source_len();
        let visible = model.rows.get(model.visible.clone()).unwrap_or(&[]);
        let visible_bytes = visible
            .first()
            .zip(visible.last())
            .map_or(0, |(first, last)| {
                last.byte_range.end.saturating_sub(first.byte_range.start)
            });
        let progress = if !can_wheel_down && can_wheel_up {
            1.0
        } else {
            visible
                .first()
                .map_or(self.viewport.anchor_byte, |row| row.byte_range.start) as f64
                / source_len.max(1) as f64
        };
        // Normalized extents avoid losing small thumb travel on very large sources.
        let viewport_extent = if !can_wheel_up && !can_wheel_down && !visible.is_empty() {
            1000.0
        } else {
            (visible_bytes as f64 / source_len.max(1) as f64 * 1000.0).clamp(0.001, 999.0) as f32
        };
        let scroll_range = 1000.0 - viewport_extent;
        let seek_callback = Arc::clone(&self.on_navigate);
        let typography = self.typography;
        let text_size_px = self.text_size_px;
        let font_theme = self.font_theme;
        let mut body = column![].spacing(0).width(if self.byte_layout.is_some() {
            Length::Shrink
        } else {
            Length::Fill
        });
        // Leading overscan must not shift visible tail rows below the body clip.
        let painted = model.rows.get(model.visible.clone()).unwrap_or(&[]);
        let leading_rows = self.tail_alignment_rows(&model, metrics.visible_rows);
        if leading_rows > 0 {
            body = body.push(
                iced::widget::Space::new()
                    .width(Length::Fill)
                    .height(Length::Fixed(leading_rows as f32 * metrics.row_height)),
            );
        }
        for item in painted {
            let number = item.line_number.map_or_else(
                || "…".to_owned(),
                |n| {
                    if item.continuation {
                        "·".to_owned()
                    } else {
                        n.to_string()
                    }
                },
            );
            let mut number_text = enz_text(number)
                .size(TextSize::Caption)
                .tone(TextTone::Muted)
                .no_wrap();
            let mut row_text = enz_text(item.text.clone()).size(TextSize::Body).no_wrap();
            if let Some(scale) = typography {
                number_text = number_text.typography(scale);
                row_text = row_text.typography(scale);
            }
            if let Some(px) = text_size_px {
                number_text = number_text.size_px(px);
                row_text = row_text.size_px(px);
            }
            number_text = document_text_font(number_text, font_theme);
            row_text = document_text_font(row_text, font_theme);
            if self.byte_layout.is_some() {
                body = body.push(
                    container(row_text.font_role(FontRole::Monospace))
                        .width(Length::Shrink)
                        .padding(Padding::from([ROW_PAD_Y, 4.0])),
                );
            } else {
                let line = row![
                    container(number_text)
                        .width(Length::Fixed(64.0))
                        .padding(Padding::from([ROW_PAD_Y, 8.0])),
                    container(row_text)
                        .width(Length::Fill)
                        .padding(Padding::from([ROW_PAD_Y, 4.0])),
                ]
                .width(Length::Fill);
                body = body.push(line);
            }
        }

        let status_inset = STATUS_INSET_TOKEN.px();
        let mut range_text = enz_text(model.range_announcement.clone())
            .size(TextSize::Caption)
            .tone(TextTone::Muted)
            .no_wrap();
        if let Some(scale) = typography {
            range_text = range_text.typography(scale);
        }
        let mut status_lines = column![range_text].spacing(0);
        if let Some(line_number_status) = &model.line_number_status {
            let mut line_text = enz_text(line_number_status.clone())
                .size(TextSize::Caption)
                .tone(TextTone::Muted)
                .no_wrap();
            if let Some(scale) = typography {
                line_text = line_text.typography(scale);
            }
            status_lines = status_lines.push(line_text);
        }
        let status_announcement = model.status_announcement();
        let status = container(SemanticsContainer::leaf(
            status_lines,
            Semantics::new(Role::Status).label(status_announcement.clone()),
        ))
        .padding(Padding::from([0.0, status_inset]));
        let clipped_body: Element<'a, Message> = if self.byte_layout.is_some() {
            // Home/End and paging keys belong to the pager, not the overflow.
            crate::scroll_view::enz_scroll_view(body)
                .axis(crate::scroll_view::Axis::Horizontal)
                .keyboard_scroll(false)
                .width(Length::Fill)
                .height(Length::Fill)
                .inset(0.0)
                .into()
        } else {
            container(body)
                .width(Length::Fill)
                .height(Length::Fill)
                .clip(true)
                .into()
        };
        let mut scroll_body = enz_offset_scrollbar(clipped_body)
            .content_len(if source_len == 0 { 0.0 } else { 1000.0 })
            .viewport_len(viewport_extent)
            .offset(progress as f32 * scroll_range)
            .scrollbar_tokens(self.scrollbar_tokens)
            .on_scroll_to(move |offset| {
                let fraction = (offset / scroll_range).clamp(0.0, 1.0);
                let byte = if fraction >= 1.0 {
                    source_len.saturating_sub(1)
                } else {
                    (f64::from(fraction) * source_len as f64) as u64
                };
                seek_callback(PagedTextNavigation::SeekByte(byte))
            });
        if let Some(backdrop) = self.backdrop {
            scroll_body = scroll_body.backdrop(backdrop);
        }
        let visual = column![scroll_body, status]
            .width(Length::Fill)
            .height(Length::Fill);

        let document_semantics = Semantics::new(Role::Document)
            .label(self.label)
            .description(if self.byte_layout.is_some() {
                enzui_core::localization::text("enzui-byte-preview-description")
            } else {
                enzui_core::localization::text("enzui-paged-text-description")
            })
            .value(status_announcement);
        let focus_host = focus_button_maybe(visual, None)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(0)
            .focusable_without_activation(true)
            .disabled_semantics(false)
            .semantics(document_semantics);

        let keyboard_callback = Arc::clone(&self.on_navigate);
        let keyboard = keyboard_capture(focus_host, move |key, modifiers| {
            navigation_for_key(key, modifiers).map(|movement| keyboard_callback(movement))
        });
        let wheel_callback = Arc::clone(&self.on_navigate);
        wheel_capture(keyboard, move |delta| {
            let lines = match delta {
                mouse::ScrollDelta::Lines { y, .. } => (-y.round() as i32).clamp(-100, 100),
                mouse::ScrollDelta::Pixels { y, .. } => {
                    (-(y / 20.0).round() as i32).clamp(-100, 100)
                }
            };
            let can_move = (lines < 0 && can_wheel_up) || (lines > 0 && can_wheel_down);
            can_move.then(|| wheel_callback(PagedTextNavigation::Wheel(lines)))
        })
    }
}

fn navigation_for_key(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
) -> Option<PagedTextNavigation> {
    if modifiers.alt() || modifiers.control() || modifiers.logo() {
        return None;
    }
    match key.as_ref() {
        keyboard::Key::Named(Named::ArrowUp) => Some(PagedTextNavigation::LineUp),
        keyboard::Key::Named(Named::ArrowDown) => Some(PagedTextNavigation::LineDown),
        keyboard::Key::Named(Named::PageUp) => Some(PagedTextNavigation::PageUp),
        keyboard::Key::Named(Named::PageDown) => Some(PagedTextNavigation::PageDown),
        keyboard::Key::Named(Named::Home) => Some(PagedTextNavigation::Home),
        keyboard::Key::Named(Named::End) => Some(PagedTextNavigation::End),
        _ => None,
    }
}

impl<'a, Message: Clone + 'static> From<EnzPagedTextViewer<'a, Message>> for Element<'a, Message> {
    fn from(value: EnzPagedTextViewer<'a, Message>) -> Self {
        value.into_element()
    }
}
