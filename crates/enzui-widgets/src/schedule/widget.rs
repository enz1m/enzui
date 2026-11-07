//! `EnzSchedule`: the widget shell over [`enzui_core::schedule`].

use enzui_core::collect::SemanticsNode;
use enzui_core::focus::activation_from_key;
use enzui_core::schedule::{
    EventId, EventSemantics, ScheduleDrag, ScheduleDragOutcome, ScheduleGrab, ScheduleNow,
    ScheduleProposal, format_localized_clock,
};
use enzui_core::semantics::{Role, Semantics};
use iced::advanced::text::Renderer as _;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Renderer as _, Shell, Widget, layout, mouse, renderer};
use iced::{Background, Border, Color, Element, Event, Length, Point, Rectangle, Shadow, Size};

use super::geometry::{
    COLUMN_RULE_WIDTH_PX, ScheduleBlock, ScheduleColumn, ScheduleConfig, ScheduleFrame, ScheduleHit,
};
use super::tokens::{ColumnEmphasis, ScheduleTokens, selection_ring_for};
use crate::theme_lookup;

/// Surface reports for the application to apply to its model.
///
/// Scrolling and live gestures are widget-owned; event data and selection remain
/// application-owned.
#[derive(Debug, Clone, PartialEq)]
pub enum ScheduleEvent {
    /// Timed-block activation on release within the deadband or Enter on a selected block.
    BlockActivated {
        /// The event id that was activated.
        id: EventId,
        /// Window-space release point, or `None` for keyboard activation or an unavailable cursor.
        at: Option<Point>,
    },
    /// All-day activation on pointer press or Enter on a selected block.
    AllDayActivated {
        /// The all-day event id that was activated.
        id: EventId,
        /// Window-space press point; `None` for keyboard activation.
        at: Option<Point>,
    },
    /// Empty-grid release within the deadband, with the press minute snapped
    /// to the drag step. Empty slots have no keyboard activation path.
    SlotActivated {
        /// Column index.
        column: usize,
        /// Snapped minute.
        minute: i32,
        /// Window-space release point, or `None` if the cursor is unavailable.
        at: Option<Point>,
    },
    /// A day-column header was clicked (drill into that day).
    HeaderActivated(usize),
    /// A drag committed. The host applies it to its own model, or does not.
    Proposal(ScheduleProposal),
    /// Live proposal from an active drag; creation is also previewed by the widget.
    Previewing(ScheduleProposal),
    /// Right-button press within the surface, carrying its hit and window-space anchor.
    ContextRequested {
        /// What was under the pointer.
        hit: ScheduleHit,
        /// Anchor point, window space.
        at: Point,
    },
    /// Changed vertical scroll offset in logical pixels, clamped to the content range.
    Scrolled(f32),
}

#[derive(Debug, Default)]
struct State {
    drag: Option<ScheduleDrag>,
    scroll: f32,
    initial_scroll_applied: bool,
    /// An event assistive technology activated, and whether it is all-day;
    /// published on the next update.
    routed_activation: Option<(EventId, bool)>,
}

/// Build a fill-sized schedule from caller-owned columns and blocks.
///
/// One column makes a day view and seven make a week. Defaults use a 15-minute
/// drag step and minimum duration, with no current-time marker or initial scroll.
pub fn enz_schedule<'a, Message>(
    columns: Vec<ScheduleColumn>,
    blocks: Vec<ScheduleBlock>,
    on_event: impl Fn(ScheduleEvent) -> Message + 'a,
) -> EnzSchedule<'a, Message> {
    EnzSchedule {
        columns,
        blocks,
        config: ScheduleConfig::default(),
        now: None,
        on_event: Box::new(on_event),
        tokens: None,
        width: Length::Fill,
        height: Length::Fill,
        drag_granularity: 15,
        min_duration: 15,
        deadband: enzui_core::desktop_settings::input().drag_threshold,
        initial_scroll_minute: None,
        hour_label: Box::new(default_hour_label),
        compact: false,
        label: None,
        id_prefix: None,
    }
}

fn default_hour_label(minute: i32) -> String {
    if minute.rem_euclid(60) == 0 {
        compact_hour_label(minute)
    } else {
        format_localized_clock(minute)
    }
}

/// Empty-grid click tolerance in logical pixels.
///
/// Travel at or below 2 pixels remains activation; greater travel starts a range drag.
pub const DEFAULT_CREATE_DRAG_DEADBAND_PX: f32 = 2.0;

/// Vertical gap between an hour rule and the label beneath it.
pub const HOUR_LABEL_GAP_PX: f32 = 2.0;

/// Right inset of an hour label from its gutter's trailing edge.
pub const HOUR_LABEL_INSET_PX: f32 = 6.0;

/// Formats a compact hour with the current localizer, or `"9a"` / `"12p"` when
/// the locale has no compact hour pattern.
///
/// Minute-of-hour information is omitted.
pub fn compact_hour_label(minute: i32) -> String {
    let wrapped = minute.rem_euclid(1440) as u32;
    let localizer = enzui_core::localization::current();
    if let Some(time) = enzui_core::time_of_day::TimeOfDay::hm(wrapped / 60, wrapped % 60)
        && let Ok(label) = localizer.format_compact_hour(time)
    {
        return label;
    }
    let m = minute.rem_euclid(1440);
    let h = m / 60;
    let h12 = match h % 12 {
        0 => 12,
        n => n,
    };
    let ampm = if h < 12 { 'a' } else { 'p' };
    format!("{h12}{ampm}")
}

/// The schedule surface.
pub struct EnzSchedule<'a, Message> {
    columns: Vec<ScheduleColumn>,
    blocks: Vec<ScheduleBlock>,
    config: ScheduleConfig,
    now: Option<ScheduleNow>,
    on_event: Box<dyn Fn(ScheduleEvent) -> Message + 'a>,
    tokens: Option<ScheduleTokens>,
    width: Length,
    height: Length,
    drag_granularity: i32,
    min_duration: i32,
    deadband: f32,
    initial_scroll_minute: Option<i32>,
    hour_label: Box<dyn Fn(i32) -> String + 'a>,
    compact: bool,
    label: Option<String>,
    id_prefix: Option<String>,
}

impl<'a, Message> EnzSchedule<'a, Message> {
    /// Namespace for the events' routable accessibility ids
    /// (`<prefix>/event-<id>`). Defaults to one derived from the schedule's
    /// label; give two schedules on screen with the same label distinct
    /// prefixes.
    #[must_use]
    pub fn id_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.id_prefix = Some(prefix.into());
        self
    }

    /// The routable id of event `id`'s accessible node.
    #[must_use]
    pub fn event_widget_id(&self, id: EventId) -> iced::advanced::widget::Id {
        let prefix = self.id_prefix.clone().unwrap_or_else(|| {
            format!(
                "enzui-schedule:{}",
                self.label.as_deref().unwrap_or("schedule")
            )
        });
        enzui_core::identity::derive_id(&prefix, &format!("event-{}", id.0))
    }

    /// Replace geometry settings; visible gutters grow to fit their hour labels.
    #[must_use]
    pub fn config(mut self, config: ScheduleConfig) -> Self {
        self.config = config;
        self
    }

    /// Set the caller-supplied current-time marker; `None` hides it.
    #[must_use]
    pub fn now(mut self, now: Option<ScheduleNow>) -> Self {
        self.now = now;
        self
    }

    /// Pin drawing tokens; event fill and ink still resolve from the runtime palette.
    #[must_use]
    pub fn tokens(mut self, tokens: ScheduleTokens) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Set outer width; defaults to [`Length::Fill`].
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Set outer height; defaults to [`Length::Fill`].
    #[must_use]
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Drag snap step in minutes; defaults to 15, with nonpositive values falling back to 15.
    #[must_use]
    pub fn drag_granularity(mut self, minutes: i32) -> Self {
        self.drag_granularity = if minutes > 0 { minutes } else { 15 };
        self
    }

    /// Minimum created or resized duration in minutes; defaults to 15.
    ///
    /// Negative values become zero. Creation may be shorter when the visible interval
    /// cannot hold the minimum; resize enforcement can extend beyond that interval.
    #[must_use]
    pub fn min_duration(mut self, minutes: i32) -> Self {
        self.min_duration = minutes.max(0);
        self
    }

    /// Scroll toward this minute on the first delivered event after mount.
    ///
    /// The offset is clamped to the content range. Defaults to `None`; later builder
    /// changes do not reapply it to retained widget state.
    #[must_use]
    pub fn initial_scroll_minute(mut self, minute: Option<i32>) -> Self {
        self.initial_scroll_minute = minute;
        self
    }

    /// Replace the hour formatter; compact mode installs its formatter when enabled.
    #[must_use]
    pub fn hour_label(mut self, f: impl Fn(i32) -> String + 'a) -> Self {
        self.hour_label = Box::new(f);
        self
    }

    /// Use a gutter starting at 30 pixels, compact hour labels and no timed-block subtitles.
    ///
    /// Defaults to false. Enabling replaces the hour formatter; disabling restores
    /// the configured minimum gutter and subtitles but leaves that formatter installed.
    /// Visible gutters grow to fit the formatted labels at their effective text size.
    /// A later [`Self::hour_label`] call can override the compact formatter.
    #[must_use]
    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        if compact {
            self.hour_label = Box::new(compact_hour_label);
        }
        self
    }

    pub(crate) fn effective_config(&self) -> ScheduleConfig {
        let mut config = self.config.clone();
        if self.compact {
            config.gutter_width = 30.0;
        }
        if !config.gutter_width.is_finite() || config.gutter_width <= 0.0 {
            return config;
        }
        let font = enzui_theme::text_defaults::font(
            enzui_theme::FontRole::Ui,
            enzui_theme::WeightRung::Regular,
        );
        let typography =
            crate::text_measure::MeasureTypography::new(crate::text::TextSize::Footnote)
                .pin_size_px(self.tokens.unwrap_or_default().hour_label_size)
                .font_family(font.family)
                .weight(font.weight)
                .shaping(iced::widget::text::Shaping::Advanced);
        // Resolve only ruler geometry so measurement shares axis normalization without laying out events twice.
        let ruler = ScheduleFrame::resolve(
            Rectangle::with_size(Size::ZERO),
            &config,
            &[],
            &[],
            None,
            0.0,
        );
        for (minute, _) in ruler.rules.iter().chain(&ruler.secondary_rules) {
            let width = crate::text_measure::enz_text_measure_width(
                &(self.hour_label)(*minute),
                typography,
            );
            config.gutter_width = config
                .gutter_width
                .max(width.ceil() + 2.0 * HOUR_LABEL_INSET_PX);
        }
        config
    }

    /// Accessible group name; defaults to `"Schedule"`.
    ///
    /// The widget publishes event children but registers no Tab focus stop.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    fn resolved_tokens(&self, theme: &iced::Theme) -> ScheduleTokens {
        self.tokens
            .unwrap_or_else(|| ScheduleTokens::from_palette(&theme_lookup::palette(theme)))
    }

    fn frame(&self, bounds: Rectangle, scroll: f32) -> ScheduleFrame {
        let config = self.effective_config();
        ScheduleFrame::resolve(
            bounds,
            &config,
            &self.columns,
            &self.blocks,
            self.now,
            scroll,
        )
    }

    // Ruler spacing and drag snapping use separate steps over the same transform.
    fn drag_axis(&self, frame: &ScheduleFrame) -> enzui_core::schedule::ScheduleAxis {
        enzui_core::schedule::ScheduleAxis::new(
            frame.axis.visible_start(),
            frame.axis.visible_end(),
            self.drag_granularity.max(1),
            frame.axis.height(),
        )
        .unwrap_or(frame.axis)
    }

    fn initial_scroll(&self, bounds: Rectangle) -> f32 {
        let Some(minute) = self.initial_scroll_minute else {
            return 0.0;
        };
        let frame = self.frame(bounds, 0.0);
        (frame.axis.y_of(minute)).clamp(0.0, frame.max_scroll())
    }

    fn grab_for(
        &self,
        frame: &ScheduleFrame,
        point: Point,
    ) -> Option<(ScheduleGrab, usize, i32, i32)> {
        match frame.hit(point) {
            ScheduleHit::Block { id } => {
                let b = self.blocks.iter().find(|b| b.id == id)?;
                Some((
                    ScheduleGrab::MoveEvent { id },
                    b.column,
                    b.start_minute,
                    b.end_minute,
                ))
            }
            ScheduleHit::BlockResizeEdge { id } => {
                let b = self.blocks.iter().find(|b| b.id == id)?;
                Some((
                    ScheduleGrab::ResizeEnd { id },
                    b.column,
                    b.start_minute,
                    b.end_minute,
                ))
            }
            ScheduleHit::Slot { column, .. } => Some((ScheduleGrab::CreateRange, column, 0, 0)),
            _ => None,
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzSchedule<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) {
        // Keep content scrolling inside the offered viewport rather than growing to the full day.
        let size = limits.resolve(self.width, self.height, Size::new(320.0, 400.0));
        layout::Node::new(size).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let frame = self.frame(layout.bounds(), tree.state.downcast_ref::<State>().scroll);
        let mut routed: Option<(EventId, bool)> = None;
        let mut node =
            SemanticsNode::new(
                Semantics::new(Role::Group)
                    .label(self.label.clone().unwrap_or_else(|| {
                        enzui_core::localization::text("enzui-schedule-schedule")
                    }))
                    .value(enzui_core::localized!("enzui-schedule-event-count", "count" => frame.blocks.iter().chain(frame.all_day_chips.iter()).map(|block| block.id).collect::<std::collections::BTreeSet<_>>().len())),
            );
        node.is_group = true;
        operation.custom(None, layout.bounds(), &mut node);
        operation.traverse(&mut |operation| {
            // Several timed segments retain one accessible model identity.
            let mut announced = std::collections::BTreeSet::new();
            for b in frame.blocks.iter().chain(frame.all_day_chips.iter()) {
                if !announced.insert(b.id) {
                    continue;
                }
                let Some(src) = self.blocks.get(b.source) else {
                    continue;
                };
                let first = self
                    .columns
                    .get(src.column)
                    .map(|c| format!("{} {}", c.weekday, c.day_number))
                    .unwrap_or_default();
                let last = self
                    .columns
                    .get(src.end_column)
                    .map(|c| format!("{} {}", c.weekday, c.day_number))
                    .unwrap_or_default();
                let date_label = if src.end_column > src.column {
                    format!("{first}–{last}")
                } else {
                    first
                };
                let sem = EventSemantics {
                    id: b.id,
                    date_label: date_label.clone(),
                    title: src.title.clone(),
                    all_day: src.all_day,
                    start_minute: src.start_minute,
                    end_minute: src.end_minute,
                    focused: src.selected,
                };
                let announcement = if !src.all_day && src.end_column > src.column {
                    // Cross-midnight announcements need dated endpoints rather than a one-day duration.
                    let key = if src.selected { "enzui-schedule-cross-midnight-selected" } else { "enzui-schedule-cross-midnight" };
                    enzui_core::localized!(key,
                        "title" => src.title,
                        "start-date" => self.columns.get(src.column).map(|column| format!("{} {}", column.weekday, column.day_number)).unwrap_or_default(),
                        "start-time" => format_localized_clock(src.start_minute),
                        "end-date" => self.columns.get(src.end_column).map(|column| format!("{} {}", column.weekday, column.day_number)).unwrap_or_default(),
                        "end-time" => format_localized_clock(src.end_minute))
                } else {
                    sem.announcement()
                };
                // A routable id carries the row's activation to assistive
                // technology: one per event, however many segments it spans.
                let id = self.event_widget_id(b.id);
                let mut child = SemanticsNode::new(
                    Semantics::new(Role::ListItem)
                        .label(announcement)
                        .selected(src.selected)
                        .default_action(),
                );
                operation.custom(Some(&id), b.rect, &mut child);
                let mut request = crate::activation::ActivationRequest::new(id.clone());
                operation.custom(Some(&id), b.rect, &mut request);
                if request.armed() {
                    routed = Some((b.id, src.all_day));
                }
            }
        });
        if routed.is_some() {
            tree.state.downcast_mut::<State>().routed_activation = routed;
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<State>();
        // An activation assistive technology routed here, reported like the
        // keyboard's: no pointer position.
        if let Some((id, all_day)) = state.routed_activation.take() {
            shell.publish((self.on_event)(if all_day {
                ScheduleEvent::AllDayActivated { id, at: None }
            } else {
                ScheduleEvent::BlockActivated { id, at: None }
            }));
            shell.request_redraw();
        }
        if !state.initial_scroll_applied {
            state.initial_scroll_applied = true;
            state.scroll = self.initial_scroll(bounds);
        }
        let frame = self.frame(bounds, state.scroll);
        // Adopt the same clamped offset used by paint and hit testing.
        state.scroll = frame.scroll;

        match event {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let Some(p) = cursor.position() else { return };
                if !frame.grid.contains(p) {
                    return;
                }
                let dy = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => y * 40.0,
                    mouse::ScrollDelta::Pixels { y, .. } => *y,
                };
                let next = (state.scroll - dy).clamp(0.0, frame.max_scroll());
                if (next - state.scroll).abs() > f32::EPSILON {
                    state.scroll = next;
                    shell.publish((self.on_event)(ScheduleEvent::Scrolled(next)));
                    shell.request_redraw();
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                let Some(p) = cursor.position() else { return };
                if !bounds.contains(p) {
                    return;
                }
                shell.publish((self.on_event)(ScheduleEvent::ContextRequested {
                    hit: frame.hit(p),
                    at: p,
                }));
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(p) = cursor.position() else { return };
                if !bounds.contains(p) {
                    return;
                }
                match frame.hit(p) {
                    ScheduleHit::Header { column } => {
                        shell.publish((self.on_event)(ScheduleEvent::HeaderActivated(column)));
                        shell.capture_event();
                        return;
                    }
                    ScheduleHit::AllDayBlock { id } => {
                        shell.publish((self.on_event)(ScheduleEvent::AllDayActivated {
                            id,
                            at: Some(p),
                        }));
                        shell.capture_event();
                        return;
                    }
                    _ => {}
                }
                if let Some((grab, column, start, end)) = self.grab_for(&frame, p) {
                    let deadband = if grab == ScheduleGrab::CreateRange {
                        DEFAULT_CREATE_DRAG_DEADBAND_PX
                    } else {
                        self.deadband
                    };
                    let drag = ScheduleDrag::begin(
                        grab,
                        column,
                        start,
                        end,
                        frame.axis_local(p),
                        &self.drag_axis(&frame),
                        frame.columns.first().map_or(f32::INFINITY, |c| c.width),
                        self.min_duration,
                        deadband,
                    )
                    .with_minute_bounds(frame.axis.visible_start(), frame.axis.visible_end());
                    state.drag = Some(drag);
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let Some(p) = cursor.position() else { return };
                let axis = self.drag_axis(&frame);
                let local = frame.axis_local(p);
                let Some(drag) = state.drag.as_mut() else {
                    return;
                };
                if let Some(proposal) = drag.pointer_moved(&axis, local) {
                    shell.publish((self.on_event)(ScheduleEvent::Previewing(proposal)));
                    shell.request_redraw();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let Some(mut drag) = state.drag.take() else {
                    return;
                };
                // An unavailable release cursor supplies no anchor; stale positions are not reused.
                let release_at = cursor.position();
                match drag.release() {
                    Some(ScheduleDragOutcome::Commit(proposal)) => {
                        shell.publish((self.on_event)(ScheduleEvent::Proposal(proposal)));
                    }
                    Some(ScheduleDragOutcome::Activate {
                        event,
                        minute,
                        column,
                    }) => {
                        let msg = match event {
                            Some(id) => ScheduleEvent::BlockActivated { id, at: release_at },
                            None => ScheduleEvent::SlotActivated {
                                column,
                                minute,
                                at: release_at,
                            },
                        };
                        shell.publish((self.on_event)(msg));
                    }
                    Some(ScheduleDragOutcome::Cancelled) | None => {}
                }
                shell.request_redraw();
                shell.capture_event();
            }
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                ..
            }) => {
                if let Some(mut drag) = state.drag.take() {
                    let _ = drag.cancel();
                    shell.request_redraw();
                }
            }
            // Model selection scopes activation; this widget has no retained focus state.
            Event::Keyboard(
                kb @ iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
                    ..
                },
            ) if activation_from_key(kb) => {
                if state.drag.is_some() {
                    return;
                }
                let mut selected_id: Option<EventId> = None;
                let mut selected_all_day = false;
                let mut multiple = false;
                for b in &self.blocks {
                    if b.selected {
                        if selected_id.is_some() {
                            multiple = true;
                            break;
                        }
                        selected_id = Some(b.id);
                        selected_all_day = b.all_day;
                    }
                }
                if multiple {
                    return;
                }
                if let Some(id) = selected_id {
                    let msg = if selected_all_day {
                        ScheduleEvent::AllDayActivated { id, at: None }
                    } else {
                        ScheduleEvent::BlockActivated { id, at: None }
                    };
                    shell.publish((self.on_event)(msg));
                    shell.capture_event();
                }
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let Some(p) = cursor.position() else {
            return mouse::Interaction::default();
        };
        let state = tree.state.downcast_ref::<State>();
        if let Some(drag) = state.drag.as_ref() {
            return match drag.grab() {
                ScheduleGrab::CreateRange => mouse::Interaction::Crosshair,
                ScheduleGrab::ResizeEnd { .. } => mouse::Interaction::ResizingVertically,
                ScheduleGrab::MoveEvent { .. } => mouse::Interaction::Pointer,
            };
        }
        let frame = self.frame(layout.bounds(), state.scroll);
        match frame.hit(p) {
            ScheduleHit::BlockResizeEdge { .. } => mouse::Interaction::ResizingVertically,
            ScheduleHit::Block { .. } | ScheduleHit::AllDayBlock { .. } => {
                mouse::Interaction::Pointer
            }
            ScheduleHit::Slot { .. } => mouse::Interaction::default(),
            ScheduleHit::Header { .. } => mouse::Interaction::Pointer,
            _ => mouse::Interaction::default(),
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return;
        }
        let state = tree.state.downcast_ref::<State>();
        let t = self.resolved_tokens(theme);
        let palette = theme_lookup::palette(theme);
        let frame = self.frame(bounds, state.scroll);

        fill(renderer, bounds, t.background);

        renderer.with_layer(frame.grid, |r| {
            for (top, bottom) in &frame.off_hours {
                let rect = clamp_v(frame.grid, *top, *bottom);
                if rect.height > 0.0 {
                    fill(r, rect, t.off_hours);
                }
            }
            for col in &frame.columns {
                if let Some(wash) =
                    ColumnEmphasis::resolve(col.is_today, col.is_selected).column_wash(&t)
                {
                    fill(
                        r,
                        Rectangle {
                            x: col.x,
                            y: frame.grid.y,
                            width: col.width,
                            height: frame.grid.height,
                        },
                        wash,
                    );
                }
            }
            let inside_off_hours = |y: f32| {
                frame
                    .off_hours
                    .iter()
                    .any(|(top, bottom)| y > *top && y < *bottom)
            };
            for y in &frame.minor_rules {
                let ink = if inside_off_hours(*y) {
                    t.off_hours_minor_rule
                } else {
                    t.minor_rule
                };
                hline(r, frame.grid, *y, 1.0, ink);
            }
            for (_, y) in &frame.rules {
                let ink = if inside_off_hours(*y) {
                    t.off_hours_rule
                } else {
                    t.rule
                };
                hline(r, frame.grid, *y, 1.0, ink);
            }
            for x in frame.column_rule_positions(COLUMN_RULE_WIDTH_PX) {
                fill(
                    r,
                    Rectangle {
                        x,
                        y: frame.grid.y,
                        width: COLUMN_RULE_WIDTH_PX,
                        height: frame.grid.height,
                    },
                    t.column_rule,
                );
            }
            let gutter_labels: [(Rectangle, &Vec<(i32, f32)>); 2] = [
                (frame.gutter, &frame.rules),
                (
                    frame.secondary_gutter.unwrap_or(frame.gutter),
                    &frame.secondary_rules,
                ),
            ];
            for (gutter, rules) in gutter_labels {
                if rules.is_empty() || gutter.width <= 0.0 {
                    continue;
                }
                for (minute, top) in
                    frame.hour_label_positions(rules, t.hour_label_size, HOUR_LABEL_GAP_PX)
                {
                    label(
                        r,
                        (self.hour_label)(minute),
                        Point::new(gutter.x + gutter.width - HOUR_LABEL_INSET_PX, top),
                        t.hour_label_size,
                        t.text_muted,
                        iced::advanced::text::Alignment::Right,
                        iced::alignment::Vertical::Top,
                        frame.grid,
                    );
                }
            }

            // Separate chip layers keep covering quads above covered glyphs and match reverse hit order.
            for b in &frame.blocks {
                let Some(src) = self.blocks.get(b.source) else {
                    continue;
                };
                if b.rect.y > frame.grid.y + frame.grid.height
                    || b.rect.y + b.rect.height < frame.grid.y
                {
                    continue;
                }
                // Snap rounding needs one pixel of slack; intersect with the grid because nested clips are absolute.
                let Some(clip) = b.rect.expand(1.0).intersection(&frame.grid) else {
                    continue;
                };
                if clip.width <= 0.0 || clip.height <= 0.0 {
                    continue;
                }
                let event_fill = palette.filled_color(src.color.unwrap_or(palette.accent));
                let ink = event_fill.text;
                r.with_layer(clip, |r| {
                    r.fill_quad(
                        renderer::Quad {
                            bounds: b.rect,
                            border: Border {
                                color: if src.selected {
                                    selection_ring_for(event_fill.color, &palette)
                                } else {
                                    Color::TRANSPARENT
                                },
                                width: if src.selected {
                                    t.selection_ring_width
                                } else {
                                    0.0
                                },
                                radius: t.block_radius.into(),
                            },
                            shadow: Shadow::default(),
                            snap: true,
                        },
                        Background::Color(
                            palette
                                .filled_color(src.color.unwrap_or(palette.accent))
                                .color,
                        ),
                    );
                    let pad = 4.0;
                    let inner = Rectangle {
                        x: b.rect.x + pad,
                        y: b.rect.y + 2.0,
                        width: (b.rect.width - 2.0 * pad).max(0.0),
                        height: (b.rect.height - 4.0).max(0.0),
                    };
                    if inner.width <= 0.0 || inner.height <= 0.0 {
                        return;
                    }
                    label(
                        r,
                        src.title.clone(),
                        Point::new(inner.x, inner.y),
                        t.block_title_size,
                        ink,
                        iced::advanced::text::Alignment::Left,
                        iced::alignment::Vertical::Top,
                        inner,
                    );
                    let second_line_y = inner.y + t.block_title_size + 3.0;
                    if !self.compact
                        && !src.subtitle.is_empty()
                        && second_line_y + t.block_subtitle_size <= inner.y + inner.height
                    {
                        label(
                            r,
                            src.subtitle.clone(),
                            Point::new(inner.x, second_line_y),
                            t.block_subtitle_size,
                            Color { a: 0.85, ..ink },
                            iced::advanced::text::Alignment::Left,
                            iced::alignment::Vertical::Top,
                            inner,
                        );
                    }
                });
            }

            // Move/resize previews belong to the host; only creation paints an additional block here.
            if let Some(ScheduleProposal::Create {
                start_minute,
                end_minute,
                column,
            }) = state.drag.as_ref().and_then(ScheduleDrag::preview)
                && let Some(rect) = frame.range_preview_rect(column, start_minute, end_minute)
                && let Some(clip) = rect.expand(1.0).intersection(&frame.grid)
            {
                r.with_layer(clip, |r| {
                    r.fill_quad(
                        renderer::Quad {
                            bounds: rect,
                            border: Border {
                                color: t.range_preview_border,
                                width: 1.0,
                                radius: t.block_radius.into(),
                            },
                            shadow: Shadow::default(),
                            snap: true,
                        },
                        Background::Color(t.range_preview),
                    );
                    let pad = 4.0;
                    let inner = Rectangle {
                        x: rect.x + pad,
                        y: rect.y + 2.0,
                        width: (rect.width - 2.0 * pad).max(0.0),
                        height: (rect.height - 4.0).max(0.0),
                    };
                    if inner.width > 0.0 && inner.height >= t.block_title_size {
                        label(
                            r,
                            default_time_range(start_minute, end_minute),
                            Point::new(inner.x, inner.y),
                            t.block_title_size,
                            t.range_preview_ink,
                            iced::advanced::text::Alignment::Left,
                            iced::alignment::Vertical::Top,
                            inner,
                        );
                    }
                });
            }

            // A later layer keeps the now-marker above nested chip layers.
            if let Some(marker) = &frame.now
                && let Some(col) = frame.columns.get(marker.column)
                && marker.y >= frame.grid.y
                && marker.y <= frame.grid.y + frame.grid.height
            {
                r.with_layer(frame.grid, |r| {
                    fill(
                        r,
                        Rectangle {
                            x: col.x,
                            y: marker.y - t.now_line_width / 2.0,
                            width: col.width,
                            height: t.now_line_width,
                        },
                        t.now_line,
                    );
                    r.fill_quad(
                        renderer::Quad {
                            bounds: Rectangle {
                                x: col.x - 3.0,
                                y: marker.y - 3.0,
                                width: 6.0,
                                height: 6.0,
                            },
                            border: Border {
                                radius: 3.0.into(),
                                ..Border::default()
                            },
                            shadow: Shadow::default(),
                            snap: false,
                        },
                        Background::Color(t.now_line),
                    );
                });
            }
        });

        if let Some(thumb) = frame.scrollbar_thumb() {
            let sb = enzui_theme::ScrollbarTokens::DEFAULT;
            renderer.with_layer(frame.grid, |r| {
                r.fill_quad(
                    renderer::Quad {
                        bounds: thumb,
                        border: Border {
                            radius: sb.border_radius.into(),
                            ..Border::default()
                        },
                        shadow: Shadow::default(),
                        snap: true,
                    },
                    Background::Color(Color {
                        a: sb.alpha,
                        ..palette.text
                    }),
                );
            });
        }

        if frame.all_day_band.height > 0.0 {
            fill(renderer, frame.all_day_band, t.all_day_background);
            renderer.with_layer(frame.all_day_band, |r| {
                for chip in &frame.all_day_chips {
                    let Some(src) = self.blocks.get(chip.source) else {
                        continue;
                    };
                    let rect = Rectangle {
                        height: (chip.rect.height - 2.0).max(1.0),
                        ..chip.rect
                    };
                    r.fill_quad(
                        renderer::Quad {
                            bounds: rect,
                            border: Border {
                                radius: t.block_radius.into(),
                                ..Border::default()
                            },
                            shadow: Shadow::default(),
                            snap: true,
                        },
                        Background::Color(
                            palette
                                .filled_color(src.color.unwrap_or(palette.accent))
                                .color,
                        ),
                    );
                    let inner = Rectangle {
                        x: rect.x + 4.0,
                        width: (rect.width - 8.0).max(0.0),
                        ..rect
                    };
                    if inner.width > 0.0 {
                        label(
                            r,
                            src.title.clone(),
                            Point::new(inner.x, inner.center_y()),
                            t.block_subtitle_size,
                            palette
                                .filled_color(src.color.unwrap_or(palette.accent))
                                .text,
                            iced::advanced::text::Alignment::Left,
                            iced::alignment::Vertical::Center,
                            inner,
                        );
                    }
                }
                label(
                    r,
                    if self.compact {
                        "all".to_owned()
                    } else {
                        "all-day".to_owned()
                    },
                    Point::new(
                        frame.gutter.x + frame.gutter.width - 6.0,
                        frame.all_day_band.center_y(),
                    ),
                    t.block_subtitle_size,
                    t.text_muted,
                    iced::advanced::text::Alignment::Right,
                    iced::alignment::Vertical::Center,
                    frame.all_day_band,
                );
            });
            hline(
                renderer,
                bounds,
                frame.all_day_band.y + frame.all_day_band.height,
                1.0,
                t.rule,
            );
        }

        if frame.header.height > 0.0 {
            fill(renderer, frame.header, t.header_background);
            renderer.with_layer(frame.header, |r| {
                let hl = frame.header_layout(t.header_text_size, t.day_number_size);
                for col in &frame.columns {
                    let Some(src) = self.columns.get(col.index) else {
                        continue;
                    };
                    let cell = Rectangle {
                        x: col.x,
                        y: frame.header.y,
                        width: col.width,
                        height: frame.header.height,
                    };
                    let emphasis = ColumnEmphasis::resolve(src.is_today, src.is_selected);
                    if let Some(header_wash) = emphasis.header_wash(&t) {
                        fill(r, cell, header_wash);
                    }
                    label(
                        r,
                        src.weekday.clone(),
                        Point::new(cell.center_x(), hl.weekday_top),
                        t.header_text_size,
                        t.text_muted,
                        iced::advanced::text::Alignment::Center,
                        iced::alignment::Vertical::Top,
                        cell,
                    );
                    if let Some(disc) = emphasis.disc(&t) {
                        let d = hl.disc_diameter;
                        let disc_rect = Rectangle {
                            x: cell.center_x() - d / 2.0,
                            y: hl.disc_top(),
                            width: d,
                            height: d,
                        };
                        r.fill_quad(
                            renderer::Quad {
                                bounds: disc_rect,
                                border: Border {
                                    radius: (d / 2.0).into(),
                                    ..Border::default()
                                },
                                shadow: Shadow::default(),
                                snap: false,
                            },
                            Background::Color(disc),
                        );
                    }
                    label_weighted(
                        r,
                        src.day_number.clone(),
                        Point::new(cell.center_x(), hl.day_number_center),
                        t.day_number_size,
                        emphasis.day_number_ink(&t),
                        iced::advanced::text::Alignment::Center,
                        iced::alignment::Vertical::Center,
                        cell,
                        emphasis.day_number_is_emphasised(),
                    );
                }
                if let (Some(name), Some(secondary_cell)) = (
                    frame.secondary_header_label.as_ref(),
                    frame.secondary_header_cell(),
                ) {
                    label(
                        r,
                        name.clone(),
                        Point::new(
                            secondary_cell.x + secondary_cell.width - HOUR_LABEL_INSET_PX,
                            hl.weekday_top,
                        ),
                        t.header_text_size,
                        t.text_muted,
                        iced::advanced::text::Alignment::Right,
                        iced::alignment::Vertical::Top,
                        secondary_cell,
                    );
                }
            });
            hline(
                renderer,
                bounds,
                frame.header.y + frame.header.height,
                1.0,
                t.rule,
            );
        }
    }
}

fn fill(renderer: &mut iced::Renderer, bounds: Rectangle, color: Color) {
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return;
    }
    renderer.fill_quad(
        renderer::Quad {
            bounds,
            border: Border::default(),
            shadow: Shadow::default(),
            snap: true,
        },
        Background::Color(color),
    );
}

fn hline(renderer: &mut iced::Renderer, within: Rectangle, y: f32, thickness: f32, color: Color) {
    if y < within.y - thickness || y > within.y + within.height {
        return;
    }
    fill(
        renderer,
        Rectangle {
            x: within.x,
            y,
            width: within.width,
            height: thickness,
        },
        color,
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "one label's content, place and style"
)]
fn label(
    renderer: &mut iced::Renderer,
    content: String,
    position: Point,
    size: f32,
    color: Color,
    align_x: iced::advanced::text::Alignment,
    align_y: iced::alignment::Vertical,
    clip: Rectangle,
) {
    label_weighted(
        renderer, content, position, size, color, align_x, align_y, clip, false,
    );
}

#[allow(
    clippy::too_many_arguments,
    reason = "one label's content, place and style"
)]
fn label_weighted(
    renderer: &mut iced::Renderer,
    content: String,
    position: Point,
    size: f32,
    color: Color,
    align_x: iced::advanced::text::Alignment,
    align_y: iced::alignment::Vertical,
    clip: Rectangle,
    emphasised: bool,
) {
    if content.is_empty() || clip.width <= 0.0 {
        return;
    }
    let font = enzui_theme::text_defaults::font(
        enzui_theme::FontRole::Ui,
        if emphasised {
            enzui_theme::WeightRung::Semibold
        } else {
            enzui_theme::WeightRung::Regular
        },
    );
    renderer.fill_text(
        iced::advanced::text::Text {
            content,
            bounds: Size::new(clip.width, clip.height.max(size)),
            size: size.into(),
            line_height: iced::widget::text::LineHeight::default(),
            font,
            align_x,
            align_y,
            shaping: iced::widget::text::Shaping::Advanced,
            wrapping: iced::widget::text::Wrapping::None,
            ellipsis: iced::widget::text::Ellipsis::End,
            hint_factor: None,
        },
        position,
        color,
        clip,
    );
}

fn clamp_v(within: Rectangle, top: f32, bottom: f32) -> Rectangle {
    let t = top.max(within.y);
    let b = bottom.min(within.y + within.height);
    Rectangle {
        x: within.x,
        y: t,
        width: within.width,
        height: (b - t).max(0.0),
    }
}

impl<'a, Message: 'a> EnzSchedule<'a, Message> {
    /// Finish the builder as a schedule element.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message: 'a> From<EnzSchedule<'a, Message>> for Element<'a, Message> {
    fn from(s: EnzSchedule<'a, Message>) -> Self {
        Element::new(s)
    }
}

/// Formats a localized time-range subtitle; each endpoint wraps within a day.
#[must_use]
pub fn default_time_range(start_minute: i32, end_minute: i32) -> String {
    enzui_core::localized!("enzui-time-range", "start" => format_localized_clock(start_minute), "end" => format_localized_clock(end_minute))
}
