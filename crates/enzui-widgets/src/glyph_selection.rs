//! Host-controlled text selection over a picture with a [`GlyphText`] layer.
//!
//! Left-button dragging selects between carets; double-click selects a word
//! and triple-click selects a line. A collapsed click or drag publishes clear.
//! The child receives events first, and touch and keyboard selection are not
//! handled here. Glyph coordinates are relative to the content before zoom
//! and rotation: a page drawn turned by [`EnzGlyphSelection::rotation`] keeps
//! its upright glyph layer, reading order and selection indices.
//!
//! Ranges are half-open glyph indices. The host must feed reported ranges back
//! through [`EnzGlyphSelection::selection`] to paint line highlights. Drag finish
//! publishes on release; word and line selections publish on their press.
//! With `app_setup`, an existing selection drags out as a native Copy.
//! The widget does not write the clipboard or publish accessibility semantics.

use enzui_core::click::{ClickCount, ClickDetector, SingleClickPolicy};
use enzui_core::glyph_selection::{GlyphSelection, GlyphText};
use enzui_core::image_transform::QuarterTurn;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};
use std::ops::Range;

/// Wrap content with mouse selection using unscaled, content-relative glyph geometry.
pub fn enz_glyph_selection<'a, Message>(
    content: impl Into<Element<'a, Message>>,
    text: &'a GlyphText,
) -> EnzGlyphSelection<'a, Message> {
    EnzGlyphSelection {
        content: content.into(),
        text,
        selection: None,
        scale: 1.0,
        rotation: QuarterTurn::None,
        on_select: None,
        on_finish: None,
        on_clear: None,
    }
}

/// A host-controlled glyph selection layer over an existing content widget.
pub struct EnzGlyphSelection<'a, Message> {
    content: Element<'a, Message>,
    text: &'a GlyphText,
    selection: Option<Range<usize>>,
    scale: f32,
    rotation: QuarterTurn,
    on_select: Option<Box<dyn Fn(Range<usize>) -> Message + 'a>>,
    on_finish: Option<Box<dyn Fn(Range<usize>) -> Message + 'a>>,
    on_clear: Option<Message>,
}

impl<'a, Message: Clone + 'a> EnzGlyphSelection<'a, Message> {
    /// This selection layer as an [`Element`].
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::new(self)
    }
}

impl<'a, Message> EnzGlyphSelection<'a, Message> {
    /// Paint the supplied half-open glyph range; callbacks do not change this range.
    #[must_use]
    pub fn selection(mut self, selection: Option<Range<usize>>) -> Self {
        self.selection = selection;
        self
    }

    /// Content pixels per glyph unit: the glyphs can stay in the document's
    /// own units (PDF points) while the page is drawn at any zoom.
    /// Defaults to 1; non-finite or nonpositive values retain the previous scale.
    #[must_use]
    pub fn scale(mut self, scale: f32) -> Self {
        if scale.is_finite() && scale > 0.0 {
            self.scale = scale;
        }
        self
    }

    /// The clockwise turn the content draws the page at, as when the host
    /// shows a rotated raster. The widget's bounds are the turned page;
    /// highlights are painted turned with it and pointer positions are turned
    /// back before they reach the glyphs, so the glyph layer stays upright.
    #[must_use]
    pub fn rotation(mut self, rotation: QuarterTurn) -> Self {
        self.rotation = rotation;
        self
    }

    /// Publish caret changes during dragging and the final range when selection completes.
    #[must_use]
    pub fn on_select(mut self, f: impl Fn(Range<usize>) -> Message + 'a) -> Self {
        self.on_select = Some(Box::new(f));
        self
    }

    /// Publish a noncollapsed drag range on release, or a word/line range on its press.
    #[must_use]
    pub fn on_finish(mut self, f: impl Fn(Range<usize>) -> Message + 'a) -> Self {
        self.on_finish = Some(Box::new(f));
        self
    }

    /// Publish on release when a single-click gesture has no noncollapsed selection.
    #[must_use]
    pub fn on_clear(mut self, message: Message) -> Self {
        self.on_clear = Some(message);
        self
    }
}

#[derive(Debug)]
struct State {
    copy_lifetime: enzui_core::cancellation::CancelToken,
    copy_owner: Option<(usize, Option<Range<usize>>, u32, QuarterTurn)>,
    clicks: ClickDetector,
    gesture: Option<(GlyphSelection, bool)>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            copy_lifetime: enzui_core::cancellation::CancelToken::new(),
            copy_owner: None,
            clicks: ClickDetector::new(SingleClickPolicy::Immediate),
            gesture: None,
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.copy_lifetime
            .cancel(enzui_core::cancellation::CancelCause::Cancelled);
    }
}

/// Maps between the turned page the widget shows and its upright glyphs.
#[derive(Debug, Clone, Copy)]
struct PageFrame {
    bounds: Rectangle,
    scale: f32,
    rotation: QuarterTurn,
}

impl PageFrame {
    // The upright page in content pixels: a quarter turn swaps the axes.
    fn upright(self) -> Size {
        match self.rotation {
            QuarterTurn::None | QuarterTurn::Clockwise180 => self.bounds.size(),
            QuarterTurn::Clockwise90 | QuarterTurn::Clockwise270 => {
                Size::new(self.bounds.height, self.bounds.width)
            }
        }
    }

    /// A window position in glyph units.
    fn to_glyph(self, p: Point) -> Point {
        let (dx, dy) = (p.x - self.bounds.x, p.y - self.bounds.y);
        let Size { width, height } = self.upright();
        let (x, y) = match self.rotation {
            QuarterTurn::None => (dx, dy),
            QuarterTurn::Clockwise90 => (dy, height - dx),
            QuarterTurn::Clockwise180 => (width - dx, height - dy),
            QuarterTurn::Clockwise270 => (width - dy, dx),
        };
        Point::new(x / self.scale, y / self.scale)
    }

    /// A glyph-unit rectangle in window coordinates.
    fn to_window(self, [x, y, w, h]: [f32; 4]) -> Rectangle {
        let (x, y, w, h) = (
            x * self.scale,
            y * self.scale,
            w * self.scale,
            h * self.scale,
        );
        let Size { width, height } = self.upright();
        let (x, y, w, h) = match self.rotation {
            QuarterTurn::None => (x, y, w, h),
            QuarterTurn::Clockwise90 => (height - (y + h), x, h, w),
            QuarterTurn::Clockwise180 => (width - (x + w), height - (y + h), w, h),
            QuarterTurn::Clockwise270 => (y, width - (x + w), h, w),
        };
        Rectangle {
            x: self.bounds.x + x,
            y: self.bounds.y + y,
            width: w,
            height: h,
        }
    }
}

impl<Message: Clone> EnzGlyphSelection<'_, Message> {
    fn frame(&self, bounds: Rectangle) -> PageFrame {
        PageFrame {
            bounds,
            scale: self.scale,
            rotation: self.rotation,
        }
    }

    fn publish(&self, shell: &mut Shell<'_, Message>, range: Range<usize>, finished: bool) {
        if let Some(on_select) = &self.on_select {
            shell.publish(on_select(range.clone()));
        }
        if finished && let Some(on_finish) = &self.on_finish {
            shell.publish(on_finish(range));
        }
    }

    fn glyph_at(&self, point: Point) -> Option<usize> {
        let glyphs = self.text.glyphs();
        glyphs
            .iter()
            .position(|g| {
                point.x >= g.rect[0]
                    && point.x <= g.rect[0] + g.rect[2]
                    && point.y >= g.rect[1]
                    && point.y <= g.rect[1] + g.rect[3]
            })
            .or_else(|| {
                let caret = self.text.caret_at(point.x, point.y);
                (!glyphs.is_empty()).then(|| caret.min(glyphs.len() - 1))
            })
    }

    fn over_text(&self, point: Point) -> bool {
        self.text
            .range_rects(0..self.text.glyphs().len())
            .iter()
            .any(|[x, y, w, h]| {
                point.x >= *x && point.x <= x + w && point.y >= *y && point.y <= y + h
            })
    }
}

impl<Message: Clone> Widget<Message, iced::Theme, iced::Renderer>
    for EnzGlyphSelection<'_, Message>
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        let mut state = State::default();
        state.copy_owner = Some((
            std::ptr::from_ref(self.text) as usize,
            self.selection.clone(),
            self.scale.to_bits(),
            self.rotation,
        ));
        tree::State::new(state)
    }

    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<State>();
        let owner = (
            std::ptr::from_ref(self.text) as usize,
            self.selection.clone(),
            self.scale.to_bits(),
            self.rotation,
        );
        if state.copy_owner.as_ref() != Some(&owner) {
            state
                .copy_lifetime
                .cancel(enzui_core::cancellation::CancelCause::Cancelled);
            state.copy_lifetime = enzui_core::cancellation::CancelToken::new();
            state.copy_owner = Some(owner);
        }
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        layout::pass_through(tree, 0);
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
        if shell.is_event_captured() || self.text.is_empty() {
            return;
        }
        let bounds = layout.bounds();
        let frame = self.frame(bounds);
        let state = tree.state.downcast_mut::<State>();
        let local = |p: Point| frame.to_glyph(p);
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let Some(position) = cursor.position_over(bounds) else {
                    return;
                };
                let point = local(position);
                let click = state.clicks.press(iced::time::Instant::now(), position);
                match click.count {
                    ClickCount::Single => {
                        let caret = self.text.caret_at(point.x, point.y);
                        state.gesture = Some((GlyphSelection::at(caret), false));
                    }
                    ClickCount::Double | ClickCount::Triple => {
                        state.gesture = None;
                        if let Some(index) = self.glyph_at(point) {
                            let range = if click.count == ClickCount::Double {
                                self.text.word_at(index)
                            } else {
                                self.text.line_at(index)
                            };
                            self.publish(shell, range, true);
                        }
                    }
                }
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                if let Some((selection, moved)) = state.gesture.as_mut() {
                    let point = local(*position);
                    let focus = self.text.caret_at(point.x, point.y);
                    if focus != selection.focus {
                        selection.focus = focus;
                        *moved = true;
                        let range = selection.range();
                        self.publish(shell, range, false);
                    }
                    shell.capture_event();
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if let Some((selection, moved)) = state.gesture.take() {
                    if moved && !selection.is_collapsed() {
                        self.publish(shell, selection.range(), true);
                    } else if let Some(clear) = &self.on_clear {
                        shell.publish(clear.clone());
                    }
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
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let bounds = layout.bounds();
        let dragging = tree.state.downcast_ref::<State>().gesture.is_some();
        if let Some(position) = cursor.position_over(bounds)
            && (dragging || self.over_text(self.frame(bounds).to_glyph(position)))
        {
            return mouse::Interaction::Text;
        }
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
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
        let Some(range) = self.selection.clone() else {
            return;
        };
        use iced::advanced::Renderer as _;
        let bounds = layout.bounds();
        let fill = crate::selection_scope::selection_fill(&crate::theme_lookup::palette(theme));
        // A separate layer keeps selection quads above the content image.
        let frame = self.frame(bounds);
        renderer.with_layer(bounds, |renderer| {
            for rect in self.text.range_rects(range) {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: frame.to_window(rect),
                        ..renderer::Quad::default()
                    },
                    fill,
                );
            }
        });
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let frame = self.frame(layout.bounds());
        let state = tree.state.downcast_ref::<State>();
        crate::text_drag::offer_read_only_selection(
            operation,
            None,
            layout.bounds().intersection(viewport).unwrap_or_default(),
            |point| {
                let range = self.selection.clone()?;
                let point = frame.to_glyph(point);
                self.text
                    .range_rects(range.clone())
                    .iter()
                    .any(|[x, y, w, h]| {
                        Rectangle::new(Point::new(*x, *y), Size::new(*w, *h)).contains(point)
                    })
                    .then(|| (self.text.text(range), state.copy_lifetime.share()))
            },
        );
        self.content.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
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

impl<'a, Message: Clone + 'a> From<EnzGlyphSelection<'a, Message>> for Element<'a, Message> {
    fn from(widget: EnzGlyphSelection<'a, Message>) -> Self {
        Element::new(widget)
    }
}
