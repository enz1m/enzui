//! Column resizing: a drag handle on each header's trailing edge, double-click
//! to fit, and the measuring pass that finds a column's natural width.
//!
//! The table keeps no widths of its own: [`ColumnResize`] reports the width a
//! gesture asks for, and the application writes it into its column geometry
//! (and stores it when `finished`). Fitting measures the header and every
//! realised cell of the column at its natural width: each is wrapped in a
//! [`MeasureCell`] that lays its content out at a wide probe width only when
//! a [`FitRunner`] operation asks for that column, so the measuring costs
//! nothing until a double-click. Content that fills its width (the table
//! text and header cells do) measures by its text: the probe width less the
//! space its filling paragraph leaves unused.

use super::*;
use iced::advanced::layout::{Layout, Limits, Measure as _};
use iced::advanced::widget::{self, Operation, Tree, tree};
use iced::advanced::{Shell, Widget, overlay, renderer};
use iced::{Event, Rectangle, Vector};

/// What a resize gesture asks for: column `column` (`key`) at `width`
/// logical pixels, never below the table's minimum column width.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnResize {
    /// The column's index.
    pub column: usize,
    /// The column's stable key, for storing the width.
    pub key: ColumnKey,
    /// The requested width in logical pixels.
    pub width: f32,
    /// `false` while a drag is moving; `true` when it ends, and for a fit or
    /// a keyboard step. Store widths when this is `true`.
    pub finished: bool,
}

/// The narrowest a column resizes to unless
/// [`EnzTable::min_column_width`] says otherwise.
pub const DEFAULT_MIN_COLUMN_WIDTH: f32 = 40.0;

/// How far one keyboard step (<kbd>Alt+Shift+Left</kbd> /
/// <kbd>Alt+Shift+Right</kbd>) resizes the active column.
pub const COLUMN_RESIZE_STEP: f32 = 16.0;

/// The width of the grab strip at a header's trailing edge.
const HANDLE_WIDTH: f32 = 8.0;

/// The width a fit lays a cell out at: wide enough that no text elides.
const FIT_PROBE_WIDTH: f32 = 4096.0;

type TextState = iced::advanced::widget::text::State<
    <iced::Renderer as iced::advanced::text::Renderer>::Paragraph,
>;

/// The most horizontal space a paragraph in `tree` leaves unused: its
/// laid-out width less the width its lines need. That is the filling
/// paragraph's share of the free space; a shrinking neighbour (a count, a
/// unit) leaves none.
fn text_slack(tree: &Tree) -> Option<f32> {
    let own = (tree.tag == tree::Tag::of::<TextState>()).then(|| {
        let paragraph = tree.state.downcast_ref::<TextState>();
        (tree.size.width - paragraph.min_bounds().width).max(0.0)
    });
    tree.children
        .iter()
        .filter_map(text_slack)
        .chain(own)
        .reduce(f32::max)
}

pub(super) type ResizeCallback<'a, Message> = Rc<dyn Fn(ColumnResize) -> Message + 'a>;

/// Offered by a [`ResizeHandle`] holding a double-click: its column, which
/// a [`FitRunner`] takes (setting `taken`).
struct FitRequestProbe {
    request: Option<(usize, ColumnKey)>,
    taken: bool,
}

/// Asked of every [`MeasureCell`]: whether `column` is being fitted, and the
/// widest natural width seen so far.
struct FitProbe {
    column: usize,
    widest: Option<f32>,
}

// ---------------------------------------------------------------------------
// ResizeHandle
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct HandleState {
    /// The pointer x and column width when the drag began.
    drag: Option<(f32, f32)>,
    /// The width the drag last published.
    dragged_to: Option<f32>,
    last_click: Option<iced::advanced::mouse::Click>,
    fit_requested: bool,
    /// Whether the pointer was over the strip at the last event.
    hovered: bool,
}

/// A header cell with a resize strip on its trailing edge.
pub(super) struct ResizeHandle<'a, Message> {
    content: Element<'a, Message>,
    column: usize,
    key: ColumnKey,
    width: f32,
    min_width: f32,
    on_resize: ResizeCallback<'a, Message>,
}

impl<'a, Message> ResizeHandle<'a, Message> {
    pub(super) fn new(
        content: Element<'a, Message>,
        column: usize,
        key: ColumnKey,
        width: f32,
        min_width: f32,
        on_resize: ResizeCallback<'a, Message>,
    ) -> Self {
        Self {
            content,
            column,
            key,
            width,
            min_width,
            on_resize,
        }
    }

    fn strip(layout: Layout) -> Rectangle {
        let bounds = layout.bounds();
        Rectangle {
            x: bounds.x + bounds.width - HANDLE_WIDTH,
            width: HANDLE_WIDTH,
            ..bounds
        }
    }

    fn resize(&self, width: f32, finished: bool) -> Message {
        (self.on_resize)(ColumnResize {
            column: self.column,
            key: self.key.clone(),
            width,
            finished,
        })
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for ResizeHandle<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<HandleState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(HandleState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_opaque(
            &mut self.content,
            tree,
            0,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &Limits) {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
        // The column edge shows while the strip is hovered or dragged.
        let state = tree.state.downcast_ref::<HandleState>();
        if state.drag.is_some() || cursor.is_over(Self::strip(layout)) {
            use iced::advanced::Renderer as _;
            let bounds = layout.bounds();
            let line = enzui_theme::StrokeToken::Hairline
                .resolve(&crate::theme_lookup::stroke(theme))
                .max(1.0)
                * 2.0;
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        x: bounds.x + bounds.width - line,
                        width: line,
                        ..bounds
                    },
                    ..renderer::Quad::default()
                },
                crate::theme_lookup::palette(theme).accent,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let state = tree.state.downcast_ref::<HandleState>();
        if state.drag.is_some() || cursor.is_over(Self::strip(layout)) {
            return mouse::Interaction::ResizingHorizontally;
        }
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_mut::<HandleState>();
        if state.fit_requested {
            let mut probe = FitRequestProbe {
                request: Some((self.column, self.key.clone())),
                taken: false,
            };
            operation.custom(None, layout.bounds(), &mut probe);
            if probe.taken {
                state.fit_requested = false;
            }
        }
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
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
        viewport: &Rectangle,
    ) {
        let strip = Self::strip(layout);
        let state = tree.state.downcast_mut::<HandleState>();
        // The edge line paints while hovered: repaint when that changes.
        let hovered = cursor.is_over(strip);
        if hovered != state.hovered {
            state.hovered = hovered;
            shell.request_redraw();
        }
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(position) = cursor.position_over(strip) {
                    let click = iced::advanced::mouse::Click::new(
                        position,
                        mouse::Button::Left,
                        state.last_click.take(),
                    );
                    if click.kind() == iced::advanced::mouse::click::Kind::Double {
                        state.fit_requested = true;
                        state.drag = None;
                    } else {
                        state.drag = Some((position.x, self.width));
                        state.dragged_to = None;
                        state.last_click = Some(click);
                    }
                    shell.capture_event();
                    return;
                }
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some((start_x, start_width)) = state.drag {
                    let width = (start_width + position.x - start_x).max(self.min_width);
                    if state.dragged_to != Some(width) && width != self.width {
                        state.dragged_to = Some(width);
                        shell.publish(self.resize(width, false));
                    }
                    shell.capture_event();
                    return;
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
                if state.drag.is_some() =>
            {
                state.drag = None;
                if let Some(width) = state.dragged_to.take() {
                    shell.publish(self.resize(width, true));
                }
                shell.capture_event();
                return;
            }
            _ => {}
        }
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.content, tree, 0);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<ResizeHandle<'a, Message>> for Element<'a, Message> {
    fn from(handle: ResizeHandle<'a, Message>) -> Self {
        Element::new(handle)
    }
}

// ---------------------------------------------------------------------------
// MeasureCell
// ---------------------------------------------------------------------------

/// The limits the content was last laid out with, so a measurement can put
/// the layout back exactly.
#[derive(Debug, Default)]
struct MeasureState {
    limits: Option<Limits>,
}

/// A header or body cell of column `column`, which reports its content's
/// natural width to a fit.
pub(super) struct MeasureCell<'a, Message> {
    content: Element<'a, Message>,
    column: usize,
}

impl<'a, Message> MeasureCell<'a, Message> {
    pub(super) fn new(content: Element<'a, Message>, column: usize) -> Self {
        Self { content, column }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for MeasureCell<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<MeasureState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(MeasureState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_opaque(
            &mut self.content,
            tree,
            0,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &Limits) {
        tree.state.downcast_mut::<MeasureState>().limits = Some(*limits);
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let mut probe = FitProbe {
            column: self.column,
            widest: None,
        };
        operation.custom(None, layout.bounds(), &mut probe);
        // `widest` comes back set only from a fit of this column.
        if probe.widest.is_some() {
            let height = layout.bounds().height;
            let laid_out = self
                .content
                .as_widget_mut()
                .measure(
                    &mut tree.children[0],
                    renderer,
                    &Limits::new(Size::ZERO, Size::new(FIT_PROBE_WIDTH, height.max(1.0))),
                )
                .size()
                .width;
            // Shrinking content is as wide as it laid out; content that
            // filled the probe is as wide as its text needs.
            let natural = if laid_out < FIT_PROBE_WIDTH {
                Some(laid_out)
            } else {
                text_slack(&tree.children[0]).map(|slack| laid_out - slack)
            };
            // Put the content's layout state back the way the frame left it.
            if let Some(limits) = tree.state.downcast_ref::<MeasureState>().limits {
                self.content
                    .as_widget_mut()
                    .layout(&mut tree.children[0], renderer, &limits);
                iced::advanced::layout::pass_through(tree, 0);
            }
            if let Some(natural) = natural {
                let mut sample = FitProbe {
                    column: self.column,
                    widest: Some(natural),
                };
                operation.custom(None, layout.bounds(), &mut sample);
            }
        }
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
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
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.content, tree, 0);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<MeasureCell<'a, Message>> for Element<'a, Message> {
    fn from(cell: MeasureCell<'a, Message>) -> Self {
        Element::new(cell)
    }
}

// ---------------------------------------------------------------------------
// FitRunner
// ---------------------------------------------------------------------------

/// Collects a double-clicked column from the handles.
struct CollectRequest(Option<(usize, ColumnKey)>);

impl Operation for CollectRequest {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn custom(
        &mut self,
        _id: Option<&widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn std::any::Any,
    ) {
        if let Some(probe) = state.downcast_mut::<FitRequestProbe>()
            && self.0.is_none()
        {
            self.0 = probe.request.take();
            probe.taken = true;
        }
    }
}

/// Measures the natural width of one column's cells.
struct Measure {
    column: usize,
    widest: Option<f32>,
}

impl Operation for Measure {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn custom(
        &mut self,
        _id: Option<&widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn std::any::Any,
    ) {
        let Some(probe) = state.downcast_mut::<FitProbe>() else {
            return;
        };
        if probe.column != self.column {
            return;
        }
        match probe.widest {
            // A query: ask for this cell's width.
            None => probe.widest = Some(0.0),
            Some(width) => self.widest = Some(self.widest.map_or(width, |w| w.max(width))),
        }
    }
}

/// Wraps the table: after an event, turns a handle's double-click into a
/// measured [`ColumnResize`].
pub(super) struct FitRunner<'a, Message> {
    content: Element<'a, Message>,
    min_width: f32,
    on_resize: ResizeCallback<'a, Message>,
}

impl<'a, Message> FitRunner<'a, Message> {
    pub(super) fn new(
        content: Element<'a, Message>,
        min_width: f32,
        on_resize: ResizeCallback<'a, Message>,
    ) -> Self {
        Self {
            content,
            min_width,
            on_resize,
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for FitRunner<'_, Message> {
    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        self.content.as_widget_mut().diff(tree);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.content, tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_transparent(
            &mut self.content,
            tree,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &Limits) {
        self.content.as_widget_mut().layout(tree, renderer, limits);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
        if !matches!(
            event,
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
        ) {
            return;
        }
        let mut request = CollectRequest(None);
        self.content
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, &mut request);
        let Some((column, key)) = request.0 else {
            return;
        };
        let mut measure = Measure {
            column,
            widest: None,
        };
        self.content
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, &mut measure);
        if let Some(widest) = measure.widest {
            shell.publish((self.on_resize)(ColumnResize {
                column,
                key,
                width: widest.ceil().max(self.min_width),
                finished: true,
            }));
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.content
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message: 'a> From<FitRunner<'a, Message>> for Element<'a, Message> {
    fn from(runner: FitRunner<'a, Message>) -> Self {
        Element::new(runner)
    }
}
