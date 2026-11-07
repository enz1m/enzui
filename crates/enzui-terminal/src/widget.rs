//! The terminal view widget.

use std::sync::Arc;

use enzui_core::click::{ClickCount, ClickDetector, SingleClickPolicy};
use enzui_core::collect::SemanticsNode;
use enzui_core::semantics::{Role, Semantics};
use iced::advanced::input_method::{InputMethod, Preedit, Purpose};
use iced::advanced::widget::{Id, Operation, Tree, operation, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, renderer};
use iced::keyboard::{self, Key, Modifiers};
use iced::{Element, Event, Length, Point, Rectangle, Size, window};
use iced_wgpu::primitive::Renderer as _;

use crate::TerminalStyle;
use crate::font::CellMetrics;
use crate::render::TerminalPrimitive;
use crate::screen::{Selection, TerminalScreen};

/// A mouse report for an application that asked for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseAction {
    /// A button went down.
    Press(mouse::Button),
    /// A button went up.
    Release(mouse::Button),
    /// The pointer moved with a button held.
    Drag(mouse::Button),
    /// The pointer moved with no button held.
    Move,
}

/// What the terminal view asks of the app.
#[derive(Debug, Clone, PartialEq)]
pub enum TerminalEvent {
    /// The view fits `cols` by `rows` cells: resize the emulator and the PTY.
    Resize {
        /// Columns.
        cols: u16,
        /// Rows.
        rows: u16,
    },
    /// A key went down while the view had focus.
    Key {
        /// The key, without modifiers applied.
        key: Key,
        /// The key with Shift and other layout modifiers applied.
        modified_key: Key,
        /// The physical key.
        physical_key: keyboard::key::Physical,
        /// Held modifiers.
        modifiers: Modifiers,
        /// The text the key produces, if any.
        text: Option<String>,
    },
    /// Text committed by an input method.
    Text(String),
    /// A mouse press, release or movement over the grid: sent while the
    /// screen's modes ask for mouse reports, and always when the view's own
    /// selection is off ([`EnzTerminal::native_selection`]).
    Mouse {
        /// What happened.
        action: MouseAction,
        /// The cell's column.
        col: u16,
        /// The cell's row.
        row: u16,
        /// The row the pointer is on, not clamped to the grid: negative above
        /// the first row and past the last below it, so a drag that leaves
        /// the grid can keep selecting into scrollback.
        unclamped_row: i32,
        /// Held modifiers.
        modifiers: Modifiers,
    },
    /// The wheel moved by whole lines, positive towards older output.
    Scroll {
        /// Lines; positive scrolls up into history.
        lines: i32,
        /// The pointer's column.
        col: u16,
        /// The pointer's row.
        row: u16,
        /// Held modifiers.
        modifiers: Modifiers,
    },
    /// A selection gesture finished with this text. Offer it as the primary
    /// selection with `iced::clipboard::write_primary`, so a middle click
    /// pastes it in other applications.
    Selected(String),
    /// The middle button asked to paste the primary selection. Read it with
    /// `iced::clipboard::read_primary` and send the text to the terminal, when
    /// `iced::clipboard::primary_paste_enabled` allows it.
    PastePrimary,
    /// The view gained or lost keyboard focus.
    Focus(bool),
}

/// A terminal view over a [`TerminalScreen`].
pub struct EnzTerminal<'a, Message> {
    screen: TerminalScreen,
    style: Arc<TerminalStyle>,
    on_event: Option<Box<dyn Fn(TerminalEvent) -> Message + 'a>>,
    id: Option<Id>,
    label: String,
    width: Length,
    height: Length,
    native_selection: bool,
    keyboard: bool,
    pointer: bool,
    rows_above: u16,
    fade_above: f32,
    focused: Option<bool>,
    interaction: Option<mouse::Interaction>,
}

/// A terminal view over `screen`, filling its space. A screen has one view at
/// a time (see [`TerminalScreen`]).
#[must_use]
pub fn enz_terminal<'a, Message>(screen: &TerminalScreen) -> EnzTerminal<'a, Message> {
    EnzTerminal {
        screen: screen.clone(),
        style: Arc::new(TerminalStyle::default()),
        on_event: None,
        id: None,
        label: "Terminal".to_owned(),
        width: Length::Fill,
        height: Length::Fill,
        native_selection: true,
        keyboard: true,
        pointer: true,
        rows_above: 0,
        fade_above: 0.0,
        focused: None,
        interaction: None,
    }
}

impl<'a, Message> EnzTerminal<'a, Message> {
    /// How the terminal looks.
    #[must_use]
    pub fn style(mut self, style: impl Into<Arc<TerminalStyle>>) -> Self {
        self.style = style.into();
        self
    }

    /// Where input, resizes and selections go.
    #[must_use]
    pub fn on_event(mut self, f: impl Fn(TerminalEvent) -> Message + 'a) -> Self {
        self.on_event = Some(Box::new(f));
        self
    }

    /// A stable widget id, for focus operations and accessibility actions.
    #[must_use]
    pub fn id(mut self, id: Id) -> Self {
        self.id = Some(id);
        self
    }

    /// The accessible name; "Terminal" by default.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// The width; fills by default.
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// The height; fills by default.
    #[must_use]
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Whether dragging selects text in the view (on by default). An app
    /// with its own selection model (one that reaches into scrollback, say)
    /// turns it off: presses, drags, releases and moves over the grid then
    /// arrive as [`TerminalEvent::Mouse`], and the app shows its selection
    /// through [`ScreenWriter::set_selection`](crate::ScreenWriter::set_selection)
    /// or the cells' colours.
    #[must_use]
    pub fn native_selection(mut self, enabled: bool) -> Self {
        self.native_selection = enabled;
        self
    }

    /// Whether the view handles the keyboard (on by default). An app that
    /// routes keys itself, through a global listener with its own shortcuts
    /// and focus rules, turns it off: the view then neither publishes nor
    /// captures key and input method events, and enables no input method.
    #[must_use]
    pub fn keyboard(mut self, enabled: bool) -> Self {
        self.keyboard = enabled;
        self
    }

    /// Whether the view handles the pointer (on by default). A view that only
    /// shows output (a preview, a thumbnail of a session) turns it off: presses, drags and the wheel then pass to whatever is
    /// beneath, and the view sets no pointer shape.
    #[must_use]
    pub fn pointer(mut self, enabled: bool) -> Self {
        self.pointer = enabled;
        self
    }

    /// Draws the screen's first `rows` rows above the view's bounds, over
    /// whatever is there (older output continuing under a floating titlebar),
    /// with the rest of the grid in the bounds as before. The grid counts
    /// them: [`TerminalEvent::Resize`] reports them, the cursor and mouse
    /// rows of the bounds start after them, and a drag above the bounds
    /// reaches them. They take no input and are not read to assistive
    /// technology. One primitive draws both, so the rows above cost no more
    /// than rows in the bounds.
    #[must_use]
    pub fn rows_above(mut self, rows: u16) -> Self {
        self.rows_above = rows;
        self
    }

    /// Fades the rows drawn above the bounds into the background towards
    /// their top, by `strength` (0 to 1) there and easing to none at the
    /// bounds, so a floating titlebar over them stays legible. The view draws
    /// the fade itself, with no layer over it.
    #[must_use]
    pub fn fade_rows_above(mut self, strength: f32) -> Self {
        self.fade_above = if strength.is_finite() {
            strength.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self
    }

    /// Whether the terminal is focused, overriding the focus the view takes
    /// on a click. It decides the cursor's look and blinking, and the focus
    /// the view reports to accessibility.
    #[must_use]
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = Some(focused);
        self
    }

    /// The pointer's shape over the grid, overriding the text cursor (or the
    /// arrow while the screen reports the mouse); a pointing hand over a
    /// link, say.
    #[must_use]
    pub fn mouse_interaction(mut self, interaction: mouse::Interaction) -> Self {
        self.interaction = Some(interaction);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Cell,
    Word,
    Line,
}

struct State {
    /// The screen this view last registered its window waker with.
    registered: Option<u64>,
    focused: bool,
    window_focused: bool,
    reported: Option<(u16, u16)>,
    metrics: Option<(crate::TerminalFont, u32, CellMetrics)>,
    clicks: ClickDetector,
    drag: Option<((u16, u16), Unit)>,
    report_button: Option<mouse::Button>,
    scroll: f32,
    modifiers: Modifiers,
    preedit: Option<Preedit>,
    /// Each row's revision, text, and cells up to its last glyph.
    rows: Vec<(u64, String, u16)>,
    /// When the cursor last moved or a key was pressed; blinking restarts
    /// from here.
    activity: iced::time::Instant,
    cursor_at: (u16, u16),
}

impl Default for State {
    fn default() -> Self {
        Self {
            registered: None,
            focused: false,
            window_focused: true,
            reported: None,
            metrics: None,
            clicks: ClickDetector::new(SingleClickPolicy::Immediate),
            drag: None,
            report_button: None,
            scroll: 0.0,
            modifiers: Modifiers::default(),
            preedit: None,
            rows: Vec::new(),
            activity: iced::time::Instant::now(),
            cursor_at: (0, 0),
        }
    }
}

impl operation::Focusable for State {
    fn is_focused(&self) -> bool {
        self.focused
    }

    fn focus(&mut self) {
        self.focused = true;
    }

    fn unfocus(&mut self) {
        self.focused = false;
    }
}

/// How long a cursor blinks after the last input or cursor movement.
const BLINK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Whether the cursor is in the visible half of its blink at `now`, and when
/// that next changes (`None` once blinking has stopped).
fn blink(
    period: Option<std::time::Duration>,
    activity: iced::time::Instant,
    now: iced::time::Instant,
) -> (bool, Option<iced::time::Instant>) {
    let Some(period) = period.filter(|p| !p.is_zero()) else {
        return (true, None);
    };
    let elapsed = now.saturating_duration_since(activity);
    if elapsed >= BLINK_TIMEOUT {
        return (true, None);
    }
    let phase = elapsed.as_nanos() / period.as_nanos();
    let next = activity + period * (phase as u32 + 1);
    (
        phase.is_multiple_of(2),
        Some(next.min(activity + BLINK_TIMEOUT)),
    )
}

fn scale_of(renderer: &iced::Renderer) -> f32 {
    use iced::advanced::Renderer as _;
    renderer
        .scale()
        .map_or(1.0, iced::advanced::renderer::Scale::total)
}

/// The grid's placement: origin and cell size in device pixels.
#[derive(Debug, Clone, Copy)]
struct Grid {
    origin: (f32, f32),
    cell: (f32, f32),
    scale: f32,
    fits: (u16, u16),
}

impl Grid {
    fn new(bounds: Rectangle, style: &TerminalStyle, metrics: CellMetrics, scale: f32) -> Self {
        let padding = (style.padding * scale).round();
        let origin = (
            (bounds.x * scale + padding).round(),
            (bounds.y * scale + padding).round(),
        );
        let cell = (metrics.width as f32, metrics.height as f32);
        let avail = (
            (bounds.width * scale - 2.0 * padding).max(0.0),
            (bounds.height * scale - 2.0 * padding).max(0.0),
        );
        let fits = (
            ((avail.0 / cell.0).floor() as u16).max(1),
            ((avail.1 / cell.1).floor() as u16).max(1),
        );
        Self {
            origin,
            cell,
            scale,
            fits,
        }
    }

    /// The row under a logical point, not clamped to the grid.
    fn row_at(&self, p: Point) -> i32 {
        ((p.y * self.scale - self.origin.1) / self.cell.1).floor() as i32
    }

    /// The cell under a logical point, clamped to `cols` by `rows`.
    fn cell_at(&self, p: Point, cols: u16, rows: u16) -> (u16, u16) {
        let x = ((p.x * self.scale - self.origin.0) / self.cell.0).floor();
        let y = ((p.y * self.scale - self.origin.1) / self.cell.1).floor();
        (
            x.clamp(0.0, f32::from(cols.saturating_sub(1))) as u16,
            y.clamp(0.0, f32::from(rows.saturating_sub(1))) as u16,
        )
    }

    /// A cell's rectangle in logical pixels.
    fn cell_rect(&self, col: u16, row: u16, span: u16) -> Rectangle {
        Rectangle {
            x: (self.origin.0 + f32::from(col) * self.cell.0) / self.scale,
            y: (self.origin.1 + f32::from(row) * self.cell.1) / self.scale,
            width: self.cell.0 * f32::from(span) / self.scale,
            height: self.cell.1 / self.scale,
        }
    }
}

fn is_word(ch: char) -> bool {
    !(ch.is_whitespace() || ch == '\0' || "'\"()[]{}<>|│,;`".contains(ch))
}

impl<Message> EnzTerminal<'_, Message> {
    /// Where the grid is drawn: `bounds`, reaching up by the rows above it.
    fn grid_bounds(&self, bounds: Rectangle, metrics: CellMetrics, scale: f32) -> Rectangle {
        let above = f32::from(self.rows_above) * metrics.height as f32 / scale;
        Rectangle {
            y: bounds.y - above,
            height: bounds.height + above,
            ..bounds
        }
    }

    /// The focus the terminal draws and reports: the app's, when it set one.
    fn is_focused(&self, state: &State) -> bool {
        self.focused.unwrap_or(state.focused)
    }

    fn metrics(&self, state: &mut State, scale: f32) -> CellMetrics {
        let font = self.style.font.resolved();
        if let Some((known, bits, metrics)) = &state.metrics
            && *known == font
            && *bits == scale.to_bits()
        {
            return *metrics;
        }
        let metrics = CellMetrics::measure(&font, scale);
        state.metrics = Some((font, scale.to_bits(), metrics));
        metrics
    }

    fn publish(&self, shell: &mut Shell<'_, Message>, event: TerminalEvent) {
        if let Some(on_event) = &self.on_event {
            shell.publish(on_event(event));
        }
    }

    /// The selection from `anchor` to `head`, widened to whole words or lines.
    fn selection(&self, anchor: (u16, u16), head: (u16, u16), unit: Unit) -> Selection {
        let (mut start, mut end) = if (anchor.1, anchor.0) <= (head.1, head.0) {
            (anchor, head)
        } else {
            (head, anchor)
        };
        self.screen.read(|state| match unit {
            Unit::Cell => {}
            Unit::Line => {
                start.0 = 0;
                end.0 = state.cols().saturating_sub(1);
            }
            Unit::Word => {
                let word_at = |(x, y): (u16, u16)| state.cell(x, y).is_some_and(|c| is_word(c.ch));
                if word_at(start) {
                    while start.0 > 0 && word_at((start.0 - 1, start.1)) {
                        start.0 -= 1;
                    }
                }
                if word_at(end) {
                    while end.0 + 1 < state.cols() && word_at((end.0 + 1, end.1)) {
                        end.0 += 1;
                    }
                }
            }
        });
        Selection {
            start,
            end,
            block: false,
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzTerminal<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(&mut self, tree: &mut Tree, _renderer: &iced::Renderer, limits: &layout::Limits) {
        layout::Node::new(limits.resolve(self.width, self.height, Size::ZERO)).apply(tree);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let state = tree.state.downcast_ref::<State>();
        let focused = self.is_focused(state) && state.window_focused;
        let cursor_on = !focused
            || blink(
                self.style.cursor_blink,
                state.activity,
                iced::time::Instant::now(),
            )
            .0;
        let scale = scale_of(renderer);
        let metrics = || {
            let font = self.style.font.resolved();
            match &state.metrics {
                Some((known, bits, metrics)) if *known == font && *bits == scale.to_bits() => {
                    *metrics
                }
                _ => CellMetrics::measure(&font, scale),
            }
        };
        if !renderer.supports_primitives() {
            let metrics = metrics();
            let bounds = self.grid_bounds(layout.bounds(), metrics, scale);
            let grid = Grid::new(bounds, &self.style, metrics, scale);
            crate::software::draw(
                renderer,
                bounds,
                &self.screen,
                &self.style,
                &metrics,
                &crate::software::ViewState {
                    origin: grid.origin,
                    scale,
                    focused,
                    cursor_on,
                    fade_above: (self.rows_above, self.fade_above),
                },
            );
            return;
        }
        let bounds = if self.rows_above > 0 {
            self.grid_bounds(layout.bounds(), metrics(), scale)
        } else {
            layout.bounds()
        };
        renderer.draw_primitive(
            bounds,
            TerminalPrimitive {
                screen: self.screen.clone(),
                style: Arc::clone(&self.style),
                focused,
                cursor_on,
                fade_above: (self.rows_above, self.fade_above),
            },
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<State>();
        if state.registered != Some(self.screen.id()) {
            self.screen.set_window(shell.waker());
            state.registered = Some(self.screen.id());
        }
        if matches!(event, Event::Waken) {
            // Only the grid's content changed, which the primitive reads
            // when it is prepared: the window need not draw again.
            if self.screen.pending() {
                shell.request_content_redraw();
            }
            return;
        }
        let scale = scale_of(renderer);
        let metrics = self.metrics(state, scale);
        let grid = Grid::new(
            self.grid_bounds(bounds, metrics, scale),
            &self.style,
            metrics,
            scale,
        );
        let (cols, rows, modes, cursor_cell) = self.screen.read(|s| {
            let c = s.cursor();
            (s.cols(), s.rows(), s.modes(), (c.col, c.row))
        });

        if state.reported != Some(grid.fits) {
            state.reported = Some(grid.fits);
            self.publish(
                shell,
                TerminalEvent::Resize {
                    cols: grid.fits.0,
                    rows: grid.fits.1,
                },
            );
        }

        // A moving cursor restarts the blink, as typing does. A content-only
        // frame keeps the cursor as last drawn, so one drawn hidden needs a
        // full frame to show again.
        if cursor_cell != state.cursor_at {
            let now = iced::time::Instant::now();
            if !blink(self.style.cursor_blink, state.activity, now).0 {
                shell.request_redraw();
            }
            state.cursor_at = cursor_cell;
            state.activity = now;
        }
        let focused = self.is_focused(state);

        match event {
            Event::Window(window::Event::RedrawRequested(now)) => {
                // A blink changes nothing in this frame: it schedules the
                // frame where it does, which then draws in full.
                if focused
                    && state.window_focused
                    && let (_, Some(next)) = blink(self.style.cursor_blink, state.activity, *now)
                {
                    shell.schedule_redraw_at(next);
                }
                if focused && self.keyboard {
                    let rect = grid.cell_rect(cursor_cell.0, cursor_cell.1, 1);
                    shell.request_input_method(&InputMethod::Enabled {
                        cursor: rect,
                        purpose: Purpose::Terminal,
                        preedit: state.preedit.as_ref().map(Preedit::as_ref),
                    });
                }
            }
            Event::Window(window::Event::Focused) => {
                state.window_focused = true;
                shell.request_redraw();
            }
            Event::Window(window::Event::Unfocused) => {
                state.window_focused = false;
                shell.request_redraw();
            }
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.modifiers = *modifiers;
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key,
                modified_key,
                physical_key,
                modifiers,
                text,
                ..
            }) if focused && self.keyboard => {
                state.activity = iced::time::Instant::now();
                self.publish(
                    shell,
                    TerminalEvent::Key {
                        key: key.clone(),
                        modified_key: modified_key.clone(),
                        physical_key: *physical_key,
                        modifiers: *modifiers,
                        text: text.as_ref().map(ToString::to_string),
                    },
                );
                shell.capture_event();
            }
            Event::InputMethod(iced::advanced::input_method::Event::Preedit(
                content,
                selection,
            )) if focused && self.keyboard => {
                state.preedit = (!content.is_empty()).then(|| Preedit {
                    content: content.clone(),
                    selection: selection.clone(),
                    text_size: Some((metrics.font_px / scale).into()),
                });
                shell.capture_event();
            }
            Event::InputMethod(iced::advanced::input_method::Event::Commit(text))
                if focused && self.keyboard =>
            {
                state.preedit = None;
                self.publish(shell, TerminalEvent::Text(text.clone()));
                shell.capture_event();
            }
            Event::Mouse(_) if !self.pointer => {}
            Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let Some(position) = cursor.position_over(bounds) else {
                    if state.focused && matches!(button, mouse::Button::Left) {
                        state.focused = false;
                        self.publish(shell, TerminalEvent::Focus(false));
                        shell.request_redraw();
                    }
                    return;
                };
                if !state.focused {
                    state.focused = true;
                    self.publish(shell, TerminalEvent::Focus(true));
                    shell.request_redraw();
                }
                let at = grid.cell_at(position, cols, rows);
                let reports =
                    (modes.mouse_reporting && !state.modifiers.shift()) || !self.native_selection;
                if reports {
                    state.report_button = Some(*button);
                    self.publish(
                        shell,
                        TerminalEvent::Mouse {
                            action: MouseAction::Press(*button),
                            col: at.0,
                            row: at.1,
                            unclamped_row: grid.row_at(position),
                            modifiers: state.modifiers,
                        },
                    );
                } else {
                    match button {
                        mouse::Button::Left => {
                            let click = state.clicks.press(iced::time::Instant::now(), position);
                            let unit = match click.count {
                                ClickCount::Single => Unit::Cell,
                                ClickCount::Double => Unit::Word,
                                ClickCount::Triple => Unit::Line,
                            };
                            state.drag = Some((at, unit));
                            let selection =
                                (unit != Unit::Cell).then(|| self.selection(at, at, unit));
                            self.screen.update(|w| w.set_selection(selection));
                        }
                        mouse::Button::Middle => self.publish(shell, TerminalEvent::PastePrimary),
                        _ => {}
                    }
                }
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some(button) = state.report_button {
                    let at = grid.cell_at(*position, cols, rows);
                    self.publish(
                        shell,
                        TerminalEvent::Mouse {
                            action: MouseAction::Drag(button),
                            col: at.0,
                            row: at.1,
                            unclamped_row: grid.row_at(*position),
                            modifiers: state.modifiers,
                        },
                    );
                    shell.capture_event();
                } else if let Some((anchor, unit)) = state.drag {
                    let at = grid.cell_at(*position, cols, rows);
                    let selection = self.selection(anchor, at, unit);
                    let selection = (unit != Unit::Cell || anchor != at).then_some(selection);
                    self.screen.update(|w| w.set_selection(selection));
                    shell.capture_event();
                } else if bounds.contains(*position)
                    && ((modes.mouse_reporting && !state.modifiers.shift())
                        || !self.native_selection)
                {
                    let at = grid.cell_at(*position, cols, rows);
                    self.publish(
                        shell,
                        TerminalEvent::Mouse {
                            action: MouseAction::Move,
                            col: at.0,
                            row: at.1,
                            unclamped_row: grid.row_at(*position),
                            modifiers: state.modifiers,
                        },
                    );
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(button)) => {
                if state.report_button == Some(*button) {
                    state.report_button = None;
                    let at = cursor
                        .position()
                        .map_or((0, 0), |p| grid.cell_at(p, cols, rows));
                    self.publish(
                        shell,
                        TerminalEvent::Mouse {
                            action: MouseAction::Release(*button),
                            col: at.0,
                            row: at.1,
                            unclamped_row: cursor
                                .position()
                                .map_or_else(|| i32::from(at.1), |p| grid.row_at(p)),
                            modifiers: state.modifiers,
                        },
                    );
                    shell.capture_event();
                } else if *button == mouse::Button::Left && state.drag.take().is_some() {
                    if let Some(text) = self.screen.read(super::screen::ScreenState::selection_text)
                        && !text.is_empty()
                    {
                        self.publish(shell, TerminalEvent::Selected(text));
                    }
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let Some(position) = cursor.position_over(bounds) else {
                    return;
                };
                let lines = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y * 3.0,
                    mouse::ScrollDelta::Pixels { y, .. } => *y * scale / grid.cell.1,
                };
                state.scroll += lines;
                let whole = state.scroll.trunc();
                if whole != 0.0 {
                    state.scroll -= whole;
                    let at = grid.cell_at(position, cols, rows);
                    self.publish(
                        shell,
                        TerminalEvent::Scroll {
                            lines: whole as i32,
                            col: at.0,
                            row: at.1,
                            modifiers: state.modifiers,
                        },
                    );
                }
                shell.capture_event();
            }
            _ => {}
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if !self.pointer || !cursor.is_over(layout.bounds()) {
            mouse::Interaction::default()
        } else if let Some(interaction) = self.interaction {
            interaction
        } else if self.screen.read(|s| s.modes().mouse_reporting) {
            mouse::Interaction::default()
        } else {
            mouse::Interaction::Text
        }
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let bounds = layout.bounds();
        let state = tree.state.downcast_mut::<State>();
        operation.focusable(self.id.as_ref(), bounds, state);

        // The screen as text: one line per row (rebuilt only when the row
        // changed), each character placed on its cell, and the caret on the
        // cursor. Screen readers read and follow it like an editor's text;
        // as a polite live region its new output is announced.
        let scale = scale_of(renderer);
        let metrics = self.metrics(state, scale);
        let grid = Grid::new(
            self.grid_bounds(bounds, metrics, scale),
            &self.style,
            metrics,
            scale,
        );
        let screen = self.screen.clone();
        let guard = screen.lock();
        // The rows drawn above the bounds are not read: line `i` is screen
        // row `above + i`.
        let above = self.rows_above.min(guard.rows());
        let shown = guard.rows() - above;
        state
            .rows
            .resize_with(usize::from(shown), || (0, String::new(), 0));
        for i in 0..shown {
            let y = above + i;
            let revision = guard.row_revision(y);
            let slot = &mut state.rows[usize::from(i)];
            if slot.0 != revision || revision == 0 {
                let row = guard.row(y);
                let span = row.iter().rposition(crate::Cell::has_glyph).map_or(0, |x| {
                    x + if row[x].flags.contains(crate::CellFlags::WIDE) {
                        2
                    } else {
                        1
                    }
                });
                *slot = (revision.max(1), guard.row_text(y), span as u16);
            }
        }
        let lines: Vec<enzui_core::semantics::TextLine> = state
            .rows
            .iter()
            .enumerate()
            .map(|(i, (_, text, _))| {
                let y = above + i as u16;
                let mut line = enzui_core::semantics::TextLine::new(text.clone());
                // Each character of the row text is one non-spacer cell.
                let cells = guard
                    .row(y)
                    .iter()
                    .enumerate()
                    .filter(|(_, cell)| !cell.flags.contains(crate::CellFlags::WIDE_SPACER))
                    .take(line.characters.len());
                line.positions = Some(
                    cells
                        .map(|(x, cell)| {
                            let wide = cell.flags.contains(crate::CellFlags::WIDE);
                            let rect = grid.cell_rect(x as u16, y, if wide { 2 } else { 1 });
                            (rect.x - bounds.x, rect.width)
                        })
                        .collect(),
                );
                let row = grid.cell_rect(0, y, guard.cols());
                line.bounds = Some(Rectangle::new(
                    Point::new(row.x - bounds.x, row.y - bounds.y),
                    row.size(),
                ));
                line
            })
            .collect();
        // The text position before cell `col` of screen row `row`: the
        // characters that start before it, within the row's trimmed text.
        let position_at = |row: u16, col: usize| {
            let line_index = usize::from(row.checked_sub(above)?);
            lines.get(line_index).map(|line| {
                let before = guard
                    .row(row)
                    .iter()
                    .take(col)
                    .filter(|cell| !cell.flags.contains(crate::CellFlags::WIDE_SPACER))
                    .count()
                    .min(line.characters.len());
                iced::advanced::text::Position {
                    line: line_index,
                    index: line.byte_of_character(before),
                }
            })
        };
        let cursor = guard.cursor();
        // Assistive technology is told the native selection (a block one as its
        // reading-order run between corners), or else the caret.
        let selection = match guard.selection() {
            Some(selected) => position_at(selected.start.1, usize::from(selected.start.0))
                .zip(position_at(selected.end.1, usize::from(selected.end.0) + 1)),
            None => position_at(cursor.row, usize::from(cursor.col)).map(|caret| (caret, caret)),
        };
        drop(guard);
        let mut semantics = Semantics::new(Role::Terminal)
            .label(self.label.clone())
            .focusable()
            .focused(self.is_focused(state))
            .read_only(true)
            .live(Some(enzui_core::semantics::Live::Polite));
        semantics.text = Some(Box::new((
            bounds,
            enzui_core::semantics::TextLayout { lines, selection },
        )));
        let mut node = SemanticsNode::new(semantics);
        operation.custom(self.id.as_ref(), bounds, &mut node);
        operation.container(self.id.as_ref(), bounds, viewport);
    }
}

impl<'a, Message: 'a> From<EnzTerminal<'a, Message>> for Element<'a, Message> {
    fn from(terminal: EnzTerminal<'a, Message>) -> Self {
        Element::new(terminal)
    }
}
