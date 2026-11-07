//! Row-level hover surface for [`EnzTable`](super::EnzTable).
//!
//! `EnzTable` emits exactly one element per row (`group_by_row` →
//! `Row::with_children`), so the row is a real element with a real layout box.
//! This wrapper puts the hover there, once, instead of in every consumer.
//!
//! # Design decisions
//!
//! ## 1. The wash paints over the row, not behind it
//!
//! [`Palette::overlay_hover`](enzui_theme::Palette::overlay_hover) is
//! defined as `text` at alpha 0.07, composited over whatever is behind rather
//! than replacing it. Painting it on top of the drawn row — in a fresh
//! renderer layer, so it lands above the cells' own quads and text — makes
//! every one of the interactions below work with no extra branch:
//!
//! * Selection.
//!   [`EnzTable::row_selection`](super::EnzTable::row_selection) fills the
//!   assembled row once with `surface_selected`. A tint painted behind that
//!   opaque band would be completely hidden, so a selected row would stop
//!   responding to the pointer. Painted over, the selected fill still
//!   dominates (the wash is 7% alpha) and the row visibly lifts under the
//!   cursor. Neither state erases the other.
//! * Striped grid. The cells rest transparent so the stripe shows through. A
//!   7%-alpha wash composites over the stripe; it cannot replace it, because
//!   it never paints an opaque quad.
//! * Active-cell highlight (`ACTIVE_CELL_HIGHLIGHT_COLOR`, a solid 2 px
//!   accent border) stays visible for the same reason — a 7% wash over a
//!   solid border leaves 93% of it.
//!
//! ## 2. Hover is bounded by the row rectangle
//!
//! The pointer must be inside both the row bounds and the enclosing viewport.
//! Testing only the vertical band leaks hover into adjacent panes whose
//! content happens to share the same y coordinate. Frozen prefixes are
//! separate row rectangles and therefore light only the section actually
//! under the pointer rather than tinting unrelated content.
//!
//! ## 3. On by default
//!
//! A row-level hover is what a data grid is expected to do, so it is the
//! default; [`EnzTable::row_hover`](super::EnzTable::row_hover) exists to
//! turn it off for a table that is a static read-only display.
//!
//! ## 4. The row owns body-hover paint
//!
//! While this wrapper paints a row wash it draws descendants with an
//! unavailable paint cursor. Events and `mouse_interaction` still receive
//! the real cursor, so embedded pressables remain clickable and keep the
//! pointer cursor, but they cannot stack a second cell-sized hover rectangle
//! over the row. Selection/focus/disabled paint is unaffected because only
//! hover hit-testing changes. A table whose cells are independently
//! actionable opts out with [`EnzTable::row_hover(false)`]; then no wrapper
//! is installed and each cell receives the real draw cursor.
//!
//! Layout, events, focus operations, `mouse_interaction` and overlays
//! otherwise delegate straight through, so adding the wash cannot move
//! geometry or swallow a click.
//!
//! # Redraw contract
//!
//! `draw` reads the live cursor. `update` caches which band was painted and
//! requests a frame only when the hovered-ness of this row actually changes,
//! so a pointer moving within one row (or entirely outside the table) does
//! not spin the compositor. The wash then fades in or out over the theme's
//! `motion-fast` duration (at once under reduced motion), asking for frames
//! only while it moves.

use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Background, Border, Color, Element, Event, Length, Rectangle, Size, Vector, window};

/// Resolves the hover wash against the live runtime theme, every draw.
///
/// A pinned [`Color`] would silently stop tracking a light/dark switch, so the
/// tint is a closure over `&Theme` exactly as every other themed surface in
/// this crate resolves its colours.
pub(super) type TintFn<'a> = Box<dyn Fn(&iced::Theme) -> Color + 'a>;

/// Persistent per-row state: whether this row painted the wash on the last
/// frame. The edge detector behind the redraw rule.
#[derive(Debug, Default)]
struct RowHoverState {
    /// `None` before the first painted frame.
    painted: Option<bool>,
    /// The wash fading in and out.
    wash: crate::motion::StateFade<Color>,
}

/// A transparent single-child wrapper that washes one table row with the
/// hover overlay while the pointer is inside that row's rectangle.
pub(super) struct RowHover<'a, Message> {
    inner: Element<'a, Message>,
    tint: TintFn<'a>,
    radius: f32,
    /// Whether the row draws a hairline divider along its bottom edge.
    rule: bool,
}

impl<'a, Message> RowHover<'a, Message> {
    /// Wrap one row element.
    pub(super) fn new(inner: Element<'a, Message>, tint: TintFn<'a>, radius: f32) -> Self {
        Self {
            inner,
            tint,
            radius,
            rule: false,
        }
    }

    /// Draw a hairline divider in the theme's `border` colour along the
    /// row's bottom edge, inside its bounds, so rows read as rows without
    /// moving any geometry.
    pub(super) fn rule(mut self, on: bool) -> Self {
        self.rule = on;
        self
    }

    /// The wash a row paints: the tint while hovered, clear otherwise.
    fn wash(tint: Color, hovered: bool) -> Color {
        if hovered {
            tint
        } else {
            Color { a: 0.0, ..tint }
        }
    }

    /// Whether this row is hovered.
    ///
    /// Rectangle test (design decision 2): the pointer must be inside both
    /// this row's own bounds and the enclosing draw viewport. Testing only
    /// the vertical extent would leak the wash into a neighbouring pane
    /// that happens to share the row's `y`; a frozen prefix and the
    /// scrolling remainder are separate row rectangles, so each lights
    /// only where the pointer actually is.
    fn hovered(bounds: Rectangle, cursor: mouse::Cursor, viewport: &Rectangle) -> bool {
        let Some(p) = cursor.position() else {
            return false;
        };
        if bounds.height <= 0.0 {
            return false;
        }
        viewport.contains(p) && bounds.contains(p)
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for RowHover<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<RowHoverState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(RowHoverState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.inner));
    }

    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.inner
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.inner.as_widget_mut().operate(
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
        // The redraw half (see the module docs). A frame refreshes
        // the cache; anything else compares against it and asks for a frame
        // only on a real transition.
        let now_hovered = Self::hovered(layout.bounds(), cursor, viewport);
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let state = tree.state.downcast_mut::<RowHoverState>();
            state.painted = Some(now_hovered);
            let tint = &self.tint;
            if state.wash.frame(*now, None, false, |theme: &iced::Theme| {
                Self::wash(tint(theme), now_hovered)
            }) {
                shell.request_redraw();
            }
        } else if tree
            .state
            .downcast_ref::<RowHoverState>()
            .painted
            .is_some_and(|was| was != now_hovered)
        {
            shell.request_redraw();
        }

        self.inner.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
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
        use iced::advanced::Renderer as _;

        let hovered = Self::hovered(layout.bounds(), cursor, viewport);
        // Descendants receive an unavailable cursor at draw time so their
        // own hover paint is suppressed and the row owns the hover wash.
        // `update` and `mouse_interaction` still see the real cursor, so
        // clicks and pointer feedback keep working.
        let child_cursor = if hovered {
            mouse::Cursor::Unavailable
        } else {
            cursor
        };
        self.inner.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            child_cursor,
            viewport,
        );

        if self.rule {
            let bounds = layout.bounds();
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        y: bounds.y + bounds.height - 1.0,
                        height: 1.0,
                        ..bounds
                    },
                    border: Border::default(),
                    shadow: iced::Shadow::default(),
                    snap: true,
                },
                Background::Color(crate::theme_lookup::palette(theme).border),
            );
        }

        let tint = tree
            .state
            .downcast_ref::<RowHoverState>()
            .wash
            .paint(theme, Self::wash((self.tint)(theme), hovered));
        if tint.a <= 0.0 {
            return;
        }

        // Pushing a fresh renderer layer is load-bearing. Within one layer
        // the backends draw every quad before any text (`wgpu/src/layer.rs`
        // `start`/`end`: quads are sublayer 1, text is sublayer 5), so a
        // quad recorded after the cells would still land underneath their
        // glyphs and underneath a selected cell's fill. The layer puts the
        // wash in a strictly later presentation pass.
        let bounds = layout.bounds();
        renderer.with_layer(bounds, |renderer| {
            renderer.fill_quad(
                renderer::Quad {
                    bounds,
                    border: Border {
                        radius: self.radius.into(),
                        ..Border::default()
                    },
                    shadow: iced::Shadow::default(),
                    snap: true,
                },
                Background::Color(tint),
            );
        });
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.inner.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
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
        self.inner.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<RowHover<'a, Message>> for Element<'a, Message> {
    fn from(w: RowHover<'a, Message>) -> Self {
        Element::new(w)
    }
}
