//! Scrollbar chrome for [`EnzTable`](super::EnzTable).
//!
//! `EnzTable` virtualises rows: it must know the scroll offset during layout
//! to decide which rows to realise, so the offset is caller state fed back
//! through [`EnzTable::scroll_y`](super::EnzTable::scroll_y) each frame. The
//! table therefore does not own a `Scrollable`; this wrapper paints a track
//! and a thumb from `(content_height, viewport_height, scroll_y)` — three
//! numbers the table computes anyway — and turns a drag on the thumb back
//! into a new offset, exactly as the wheel does.
//!
//! # Design decisions
//!
//! ## 1. The thumb geometry reflects the real content extent
//!
//! The visible fraction is `viewport_height / content_height`, so the thumb
//! height is `track_height * fraction` (clamped to [`MIN_THUMB_PX`] so a huge
//! list still leaves a grabbable target). The thumb travel maps `scroll_y ∈
//! [0, max_scroll]` linearly onto `thumb_y ∈ [track_top, track_bottom -
//! thumb_h]`, where `max_scroll = content_height - viewport_height`. At
//! offset `0` the thumb sits flush at the top; at `max_scroll` its bottom
//! sits flush at the track bottom.
//!
//! ## 2. No chrome when everything fits; overlay chrome auto-hides otherwise
//!
//! When `content_height <= viewport_height` there is nothing to scroll, so no
//! thumb is painted and there is no grab target. When content overflows, the
//! thumb is still hidden at rest: it is revealed only while the pointer is in
//! the trailing [`HIT_WIDTH_PX`]-wide proximity band or while a drag is
//! active. Hidden paint never removes that band, so the first press can grab
//! a thumb the user has not yet seen. [`TableScrollbar::overflowing`] is the
//! single source that gates both paint and drag.
//!
//! ## 3. Tokenised overlay geometry keeps content clear
//!
//! The invisible reveal/grab lane is [`ScrollbarTokens::hit_width`]
//! (15 px at the shared default), while the painted thumb is
//! [`ScrollbarTokens::width`] (5 px) centred inside it and rounded by
//! [`ScrollbarTokens::border_radius`]. The wrapped body is laid out only
//! [`GUTTER_PX`] (`content_gutter`, 10 px) narrower: as with the standard
//! overlay scroll view, the wider hit band may overlap content while the
//! thumb's inner edge remains outside the cleared content area.
//!
//! ## 4. The drag agrees with the wheel
//!
//! A press inside the lane grabs the thumb; moving the pointer by `dy` px
//! moves the offset by `dy * max_scroll / (track_height - thumb_h)` — the
//! inverse of the thumb-travel map — and the new absolute offset is published
//! through `on_scroll_to`. The caller routes it through the same clamp its
//! wheel path uses, so dragging to the bottom and wheeling to the bottom
//! land on the same offset. If pointer capture or window focus is lost
//! first, the drag is abandoned without publishing a fabricated final offset;
//! a later move stays inert until a fresh press.
//!
//! # Transparency
//!
//! Layout delegates to the inner body (minus the gutter), and every other
//! method — `operate`, `mouse_interaction` off the thumb, `overlay` —
//! delegates straight through, so the chrome cannot move a pixel of the
//! body's geometry or swallow a click that is not on the thumb.

use enzui_theme::ScrollbarTokens;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Background, Border, Color, Element, Event, Length, Rectangle, Size, Vector};

/// Trailing-edge lane width reserved for the scrollbar, in logical px.
///
/// The body is laid out this much narrower and the thumb is painted inside this
/// lane, so the last column ends before the thumb begins.
///
/// Derived from [`ScrollbarTokens::DEFAULT`]`.content_gutter`, not a
/// re-typed literal. The table's lane is a *reserved* lane while a scroll
/// region's gutter is *content clearance* — different mechanisms — but
/// both answer the same question ("how far in from the trailing edge is
/// content safe from the thumb?"), so they share one number by
/// construction.
///
/// [`super::SCROLLBAR_GUTTER`] is the public spelling of this same value;
/// the two are pinned equal by `enz_table::tests`.
pub(super) const GUTTER_PX: f32 = ScrollbarTokens::DEFAULT.content_gutter;

/// Invisible reveal/grab band width, in logical px.
pub(super) const HIT_WIDTH_PX: f32 = ScrollbarTokens::DEFAULT.hit_width;

/// Painted thumb width, in logical px.
pub(super) const THUMB_PX: f32 = ScrollbarTokens::DEFAULT.width;

/// Capsule radius for the thumb's rounded ends.
pub(super) const THUMB_RADIUS_PX: f32 = ScrollbarTokens::DEFAULT.border_radius;

/// Minimum painted thumb height, in logical px. Keeps a grabbable target even
/// when the content dwarfs the viewport. Derived from the one shared token.
pub(super) const MIN_THUMB_PX: f32 = ScrollbarTokens::DEFAULT.min_thumb_length;

/// Resolves the thumb colour against the live runtime theme, every draw.
pub(super) type ColorFn<'a> = Box<dyn Fn(&iced::Theme) -> Color + 'a>;

/// The geometry the thumb is drawn from: the vertical rectangle of the thumb
/// itself, and the lane rectangle that is its grab target. Both `None` when the
/// content fits (no chrome).
#[derive(Debug, Clone, Copy)]
pub(super) struct ThumbGeometry {
    /// The painted thumb rectangle.
    pub thumb: Rectangle,
    /// The full-height lane (grab target) along the trailing edge.
    pub lane: Rectangle,
    /// `max_scroll = content_height - viewport_height`, always `> 0` here.
    pub max_scroll: f32,
    /// The vertical travel the thumb has: `track_height - thumb.height`.
    pub travel: f32,
}

/// Persistent per-widget state: the active drag, if any.
#[derive(Debug, Default)]
struct ScrollbarState {
    /// While dragging: the pointer `y` where the drag began, and the
    /// `scroll_y` at that moment. `None` when not dragging.
    drag: Option<DragOrigin>,
    /// Whether the previous rendered frame showed the thumb. This edge cache
    /// requests a frame when the pointer crosses the invisible reveal band.
    painted: Option<bool>,
    /// How far the track's ends keep from the rounded corners around it.
    clearance: [f32; 2],
}

#[derive(Debug, Clone, Copy)]
struct DragOrigin {
    pointer_y: f32,
    scroll_y: f32,
}

/// A transparent single-child wrapper that paints a vertical scrollbar over one
/// table body and turns a thumb drag into a new offset.
pub(super) struct TableScrollbar<'a, Message> {
    inner: Element<'a, Message>,
    /// Total content height on the row axis (`row_geometry.total_height()`).
    content_height: f32,
    /// Effective body viewport height. Fixed-row tables round the allocation
    /// below the header down to complete row slots.
    viewport_height: f32,
    /// Current caller-owned scroll offset, in px.
    scroll_y: f32,
    /// Thumb colour resolver.
    thumb_color: ColorFn<'a>,
    /// Publishes a new absolute scroll offset when the thumb is dragged.
    on_scroll_to: Option<Box<dyn Fn(f32) -> Message + 'a>>,
}

impl<'a, Message> TableScrollbar<'a, Message> {
    /// Wrap a table body.
    pub(super) fn new(
        inner: Element<'a, Message>,
        content_height: f32,
        viewport_height: f32,
        scroll_y: f32,
        thumb_color: ColorFn<'a>,
        on_scroll_to: Option<Box<dyn Fn(f32) -> Message + 'a>>,
    ) -> Self {
        Self {
            inner,
            content_height,
            viewport_height,
            scroll_y,
            thumb_color,
            on_scroll_to,
        }
    }

    /// Whether the content exceeds the viewport (i.e. there is anything to
    /// scroll). The single source of truth that gates both paint and drag.
    fn overflowing(&self) -> bool {
        self.content_height > self.viewport_height + 0.5 && self.viewport_height > 0.0
    }

    /// Resolve the thumb + lane geometry for a body layout rectangle.
    ///
    /// `None` when the content fits (design decision 2).
    pub(super) fn geometry(&self, bounds: Rectangle) -> Option<ThumbGeometry> {
        if !self.overflowing() {
            return None;
        }
        let track_h = bounds.height;
        if track_h <= 0.0 {
            return None;
        }
        // The visible fraction, clamped so a min-height thumb is always
        // grabbable. `content_height > viewport_height` here so `frac < 1`.
        let frac = (self.viewport_height / self.content_height).clamp(0.0, 1.0);
        let thumb_h = (track_h * frac).max(MIN_THUMB_PX).min(track_h);
        let travel = (track_h - thumb_h).max(0.0);

        let max_scroll = (self.content_height - self.viewport_height).max(0.0);
        // Linear map of scroll_y in [0, max_scroll] onto thumb_y in
        // [track_top, track_top + travel]. Clamp so an over-range offset (the
        // caller has not clamped yet) does not push the thumb out of the track.
        let progress = if max_scroll > 0.0 {
            (self.scroll_y / max_scroll).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let thumb_y = bounds.y + progress * travel;

        // The hit/reveal lane hugs the trailing edge at the full token width;
        // the narrower thumb is centred in it.
        let lane_x = bounds.x + bounds.width - HIT_WIDTH_PX;
        let thumb_x = lane_x + (HIT_WIDTH_PX - THUMB_PX) / 2.0;

        Some(ThumbGeometry {
            thumb: Rectangle {
                x: thumb_x,
                y: thumb_y,
                width: THUMB_PX,
                height: thumb_h,
            },
            lane: Rectangle {
                x: lane_x,
                y: bounds.y,
                width: HIT_WIDTH_PX,
                height: track_h,
            },
            max_scroll,
            travel,
        })
    }

    /// Convert a pointer `y` during a drag into a new absolute offset, given the
    /// geometry captured at press time. The inverse of the thumb-travel map.
    fn offset_for_drag(geo: &ThumbGeometry, origin: DragOrigin, pointer_y: f32) -> f32 {
        if geo.travel <= 0.0 {
            return origin.scroll_y;
        }
        let dy = pointer_y - origin.pointer_y;
        let scroll_delta = dy * geo.max_scroll / geo.travel;
        (origin.scroll_y + scroll_delta).clamp(0.0, geo.max_scroll)
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for TableScrollbar<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ScrollbarState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(ScrollbarState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.inner));
    }

    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        // Design decision 3: reserve the trailing gutter so the body's last
        // column ends before the thumb lane. The body is laid out inside a
        // narrower limit; the wrapper keeps the full outer bounds so the lane
        // still has somewhere to paint. `min(GUTTER_PX, width)` guards a
        // degenerately narrow surface.
        let outer = limits.resolve(Length::Shrink, Length::Shrink, Size::ZERO);
        let gutter = GUTTER_PX.min(limits.max().width);
        let child_limits = limits.shrink(Size::new(gutter, 0.0));
        let child =
            self.inner
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, &child_limits);
        let child_size = child.size();
        // Outer node spans the body plus the gutter lane.
        let width = (child_size.width + gutter).max(outer.width);
        let height = child_size.height.max(outer.height);
        layout::Node::with_children(Size::new(width, height), vec![child]).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let child_layout = layout.children(&tree.children).next().unwrap_or(layout);
        self.inner.as_widget_mut().operate(
            &mut tree.children[0],
            child_layout,
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
        let child_layout = layout.children(&tree.children).next().unwrap_or(layout);

        // Auto-hide edge detection. The lane exists independently of paint:
        // entering it must request the first revealed frame, and an active drag
        // keeps the thumb visible even after the pointer leaves the lane.
        let bounds = layout.bounds();
        // The thumb keeps clear of rounded corners around it, measured here
        // and kept for drawing.
        let thumb_x = bounds.x + bounds.width - (HIT_WIDTH_PX + THUMB_PX) / 2.0;
        let clearance =
            crate::scrollbar_drag::vertical_clearance(bounds, (thumb_x, thumb_x + THUMB_PX));
        let state = tree.state.downcast_mut::<ScrollbarState>();
        if state.clearance != clearance {
            state.clearance = clearance;
            shell.request_redraw();
        }
        let geometry = self.geometry(crate::scrollbar_drag::cleared_vertical(bounds, clearance));
        // Pointer/window loss ends the local mouse latch without inventing an
        // offset. Do not return: the wrapped table body observes the loss too.
        if crate::scrollbar_drag::cancels_mouse_drag(event) && state.drag.take().is_some() {
            shell.request_redraw();
        }
        let pointed =
            geometry.is_some_and(|geo| cursor.position().is_some_and(|p| geo.lane.contains(p)));
        let visible = pointed || state.drag.is_some();
        if matches!(
            event,
            Event::Window(iced::window::Event::RedrawRequested(_))
        ) {
            state.painted = Some(visible);
        } else if state.painted.is_some_and(|was| was != visible) {
            shell.request_redraw();
        }

        // Handle scrollbar drag first, before the body sees the event, so a
        // press that lands in the thumb lane grabs the thumb rather than
        // reaching a cell under it.
        if self.on_scroll_to.is_some() {
            if let Some(geo) = geometry {
                match event {
                    Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                        if let Some(p) = cursor.position().filter(|p| geo.lane.contains(*p)) {
                            // If the press is off the thumb but in the lane,
                            // jump so the thumb centres on the pointer, then
                            // drag from there.
                            let start_scroll = if geo.thumb.contains(p) {
                                self.scroll_y
                            } else {
                                page_to(&geo, p.y)
                            };
                            state.drag = Some(DragOrigin {
                                pointer_y: p.y,
                                scroll_y: start_scroll,
                            });
                            if let Some(cb) = &self.on_scroll_to {
                                shell.publish(cb(start_scroll));
                            }
                            shell.capture_event();
                            shell.request_redraw();
                            return;
                        }
                    }
                    Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                        if let (Some(origin), Some(p)) = (state.drag, cursor.position()) {
                            let new_offset = Self::offset_for_drag(&geo, origin, p.y);
                            if let Some(cb) = &self.on_scroll_to {
                                shell.publish(cb(new_offset));
                            }
                            shell.capture_event();
                            shell.request_redraw();
                            return;
                        }
                    }
                    Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                        // `collapsible_match` wants `drag.take()` in a match
                        // guard, but `take()` mutates state; keep it in the body.
                        #[allow(
                            clippy::collapsible_match,
                            reason = "the condition mutates state, so it cannot be a match guard"
                        )]
                        if state.drag.take().is_some() {
                            shell.capture_event();
                            shell.request_redraw();
                            return;
                        }
                    }
                    _ => {}
                }
            } else {
                // No overflow: any stale drag is void.
                state.drag = None;
            }
        }

        self.inner.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
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

        let child_layout = layout.children(&tree.children).next().unwrap_or(layout);
        self.inner.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            child_layout,
            cursor,
            viewport,
        );

        // No overflow means neither paint nor a grab zone. Overflow alone is
        // not enough to reveal: the standard overlay chrome is hidden at rest.
        let state = tree.state.downcast_ref::<ScrollbarState>();
        let Some(geo) = self.geometry(crate::scrollbar_drag::cleared_vertical(
            layout.bounds(),
            state.clearance,
        )) else {
            return;
        };
        let pointed = cursor.position().is_some_and(|p| geo.lane.contains(p));
        let dragging = state.drag.is_some();
        // A desktop that turns overlay scrolling off keeps the thumb visible.
        if !pointed && !dragging && enzui_core::desktop_settings::overlay_scrollbars() {
            return;
        }

        let sb = crate::theme_lookup::scrollbar(theme);
        let mut thumb = (self.thumb_color)(theme);
        thumb.a *= if dragging { sb.alpha_hover } else { sb.alpha };

        // Paint only the tokenised thumb — no permanent track — over the body
        // in a fresh layer so the chrome sits above cells.
        if thumb.a > 0.0 {
            renderer.with_layer(layout.bounds(), |renderer| {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: geo.thumb,
                        border: Border::default().rounded(THUMB_RADIUS_PX),
                        shadow: iced::Shadow::default(),
                        snap: true,
                    },
                    Background::Color(thumb),
                );
            });
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
        let clearance = tree.state.downcast_ref::<ScrollbarState>().clearance;
        if self.on_scroll_to.is_some()
            && self
                .geometry(crate::scrollbar_drag::cleared_vertical(
                    layout.bounds(),
                    clearance,
                ))
                .is_some_and(|geo| cursor.position().is_some_and(|p| geo.lane.contains(p)))
        {
            return mouse::Interaction::Pointer;
        }
        let child_layout = layout.children(&tree.children).next().unwrap_or(layout);
        self.inner.as_widget().mouse_interaction(
            &tree.children[0],
            child_layout,
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
        let child_layout = layout.children(&tree.children).next();
        let child_layout = child_layout.unwrap_or(layout);
        self.inner.as_widget_mut().overlay(
            &mut tree.children[0],
            child_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

/// Jump-to-page: a press in the lane but off the thumb centres the thumb on the
/// pointer. Returns the offset that would place the thumb centre at `pointer_y`.
fn page_to(geo: &ThumbGeometry, pointer_y: f32) -> f32 {
    if geo.travel <= 0.0 {
        return 0.0;
    }
    let track_top = geo.lane.y;
    // Desired thumb top so its centre lands on the pointer.
    let desired_top = (pointer_y - geo.thumb.height / 2.0).clamp(track_top, track_top + geo.travel);
    let progress = (desired_top - track_top) / geo.travel;
    (progress * geo.max_scroll).clamp(0.0, geo.max_scroll)
}

/// Default thumb base colour. [`TableScrollbar::draw`] applies
/// [`ScrollbarTokens::alpha`] or `alpha_hover` for the revealed/dragged state.
pub(super) fn default_thumb_color(theme: &iced::Theme) -> Color {
    crate::theme_lookup::palette(theme).text
}

impl<'a, Message: 'a> From<TableScrollbar<'a, Message>> for Element<'a, Message> {
    fn from(w: TableScrollbar<'a, Message>) -> Self {
        Element::new(w)
    }
}

/// Test seam: build a bare [`TableScrollbar`] wrapping `inner`, with
/// content/viewport heights set so the chrome is active.
///
/// Exposed only so `layout_spill_widget_sweep` can lay `TableScrollbar`
/// out at the root of an `Element`. In its natural home the widget sits
/// several levels inside `EnzTable::into_element`'s composition, whose
/// outer `container(...).width(Fill).height(Fill)` clamps the returned
/// node back to the incoming limits before the sweep can observe it — so
/// a `layout()` defect there would be invisible to an end-to-end sweep.
/// Handing the sweep a tree whose root is the wrapper is the only way to
/// gate the gutter-reserving hunk with the same instrument every other
/// custom `layout()` in this crate is gated by.
///
/// `#[doc(hidden)]` because it is a test seam; the production route is
/// `EnzTable::scrollbar` / `EnzTable::on_scroll_to`.
#[doc(hidden)]
pub fn __test_only_table_scrollbar<'a, Message: 'a>(
    inner: Element<'a, Message>,
) -> Element<'a, Message> {
    // Content twice the viewport so `overflowing()` is true and every path
    // (`geometry`, drag, paint) is on. The exact numbers do not matter to the
    // sweep, which only measures the returned layout node.
    TableScrollbar::new(
        inner,
        200.0, // content_height
        100.0, // viewport_height
        0.0,   // scroll_y
        Box::new(default_thumb_color),
        None,
    )
    .into()
}
