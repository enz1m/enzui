//! Vertical overlay scrollbar chrome for a caller-owned content offset.
//!
//! The child receives unchanged layout limits; the trailing thumb reserves no
//! gutter. The caller supplies content extent, viewport extent and offset and
//! applies callback results itself. Wheel callbacks capture and report the raw
//! delta, including when content fits. Drag callbacks publish absolute offsets.
//! [`EnzOffsetScrollbar::backdrop`] resolves ink over the child's actual surface.
//! Pointer or window loss cancels a drag without publishing a replacement offset.
//!
//! Extents, offsets and layout bounds must be finite. This primitive does not
//! sanitise caller geometry. Use [`crate::scroll_view`] when the widget should
//! own scrolling, or [`crate::tail_follow`] for a tail-following policy.

use enzui_theme::ScrollbarTokens;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Background, Border, Color, Element, Event, Length, Rectangle, Size, Vector};

/// Default minimum thumb height in logical pixels, clipped to the track height.
pub const MIN_THUMB_PX: f32 = ScrollbarTokens::DEFAULT.min_thumb_length;

/// Default trailing reveal and grab lane width in logical pixels.
pub const LANE_WIDTH_PX: f32 = ScrollbarTokens::DEFAULT.hit_width;

/// Default painted thumb width in logical pixels.
pub const THUMB_WIDTH_PX: f32 = ScrollbarTokens::DEFAULT.width;

/// Vertical thumb and trailing lane rectangles for a laid-out child.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThumbGeometry {
    /// Thumb rectangle centred horizontally in the lane.
    pub thumb: Rectangle,
    /// Full-height trailing lane, independent of whether the thumb is currently painted.
    pub lane: Rectangle,
    /// Maximum content offset: content extent minus viewport extent.
    pub max_scroll: f32,
    /// Available vertical thumb travel after subtracting thumb height.
    pub travel: f32,
}

/// Compute thumb geometry with [`ScrollbarTokens::DEFAULT`].
/// Returns `None` for non-positive bounds or viewport, or content exceeding the
/// viewport by at most 0.5 logical pixels. Inputs must be finite. Offset progress
/// is clamped to the track; a minimum-height thumb cannot exceed the track.
pub fn thumb_geometry(
    bounds: Rectangle,
    content_len: f32,
    viewport_len: f32,
    offset: f32,
) -> Option<ThumbGeometry> {
    thumb_geometry_with_tokens(
        bounds,
        content_len,
        viewport_len,
        offset,
        ScrollbarTokens::DEFAULT,
    )
}

/// Compute thumb geometry with normalised custom tokens.
/// Uses the same finite-input requirements and overflow threshold as [`thumb_geometry`].
pub fn thumb_geometry_with_tokens(
    bounds: Rectangle,
    content_len: f32,
    viewport_len: f32,
    offset: f32,
    tokens: ScrollbarTokens,
) -> Option<ThumbGeometry> {
    let tokens = tokens.normalized();
    if content_len <= viewport_len + 0.5 || viewport_len <= 0.0 {
        return None;
    }
    if bounds.width <= 0.0 || bounds.height <= 0.0 {
        return None;
    }
    let track_h = bounds.height;
    let frac = (viewport_len / content_len).clamp(0.0, 1.0);
    let thumb_h = (track_h * frac).max(tokens.min_thumb_length).min(track_h);
    let travel = (track_h - thumb_h).max(0.0);
    let max_scroll = (content_len - viewport_len).max(0.0);
    let progress = if max_scroll > 0.0 {
        (offset / max_scroll).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let lane_x = bounds.x + bounds.width - tokens.hit_width;
    Some(ThumbGeometry {
        thumb: Rectangle {
            x: lane_x + (tokens.hit_width - tokens.width) / 2.0,
            y: bounds.y + progress * travel,
            width: tokens.width,
            height: thumb_h,
        },
        lane: Rectangle {
            x: lane_x,
            y: bounds.y,
            width: tokens.hit_width,
            height: track_h,
        },
        max_scroll,
        travel,
    })
}
fn drag_target_offset(geo: &ThumbGeometry, start_scroll: f32, dy: f32) -> f32 {
    if geo.travel <= 0.0 {
        return start_scroll;
    }
    let scroll_delta = dy * geo.max_scroll / geo.travel;
    (start_scroll + scroll_delta).clamp(0.0, geo.max_scroll)
}
fn lane_press_offset(geo: &ThumbGeometry, pointer_y: f32) -> f32 {
    if geo.travel <= 0.0 {
        return 0.0;
    }
    let track_top = geo.lane.y;
    let desired_top = (pointer_y - geo.thumb.height / 2.0).clamp(track_top, track_top + geo.travel);
    let progress = (desired_top - track_top) / geo.travel;
    (progress * geo.max_scroll).clamp(0.0, geo.max_scroll)
}
#[derive(Debug, Default)]
struct ChromeState {
    drag: Option<DragOrigin>,
    painted: Option<bool>,
    /// How far the track's ends keep from the rounded corners around it.
    clearance: [f32; 2],
}

#[derive(Debug, Clone, Copy)]
struct DragOrigin {
    pointer_y: f32,
    scroll_at_press: f32,
}

/// Layout-transparent vertical scrollbar chrome over a child with a caller-owned offset.
pub struct EnzOffsetScrollbar<'a, Message> {
    inner: Element<'a, Message>,
    content_len: f32,
    viewport_len: f32,
    offset: f32,
    backdrop: Option<Color>,
    tokens: ScrollbarTokens,
    on_scroll_to: Option<Box<dyn Fn(f32) -> Message + 'a>>,
    on_wheel: Option<Box<dyn Fn(mouse::ScrollDelta) -> Message + 'a>>,
}

/// Wrap a child with overlay scrollbar chrome.
/// Extents and offset default to zero, callbacks and backdrop are unset, and
/// geometry uses the current scrollbar text defaults. Supply finite extents and
/// offset; apply callback results in the host before rebuilding the widget.
pub fn enz_offset_scrollbar<'a, Message: 'a>(
    child: impl Into<Element<'a, Message>>,
) -> EnzOffsetScrollbar<'a, Message> {
    EnzOffsetScrollbar {
        inner: child.into(),
        content_len: 0.0,
        viewport_len: 0.0,
        offset: 0.0,
        backdrop: None,
        tokens: enzui_theme::text_defaults::scrollbar(),
        on_scroll_to: None,
        on_wheel: None,
    }
}

impl<'a, Message> EnzOffsetScrollbar<'a, Message> {
    /// Set total content extent in logical pixels; the default is zero.
    /// The value must be finite.
    #[must_use]
    pub fn content_len(mut self, px: f32) -> Self {
        self.content_len = px;
        self
    }

    /// Set viewport extent in logical pixels; the default is zero.
    /// Non-positive values or content exceeding the viewport by at most 0.5 px
    /// disable the lane. The value must be finite.
    #[must_use]
    pub fn viewport_len(mut self, px: f32) -> Self {
        self.viewport_len = px;
        self
    }

    /// Set the caller-owned offset in logical pixels; the default is zero.
    /// Paint clamps its progress to the track, without rewriting this input.
    /// The value must be finite.
    #[must_use]
    pub fn offset(mut self, px: f32) -> Self {
        self.offset = px;
        self
    }

    /// Publish an absolute offset on a lane press or thumb drag, before the child handles it.
    /// Off-thumb presses centre the thumb at the pointer, then establish a drag
    /// origin. Subsequent drag values include the offset at that press; the host
    /// applies them. With positive thumb travel, drag targets clamp to the scroll
    /// range; an immobile thumb retains its press offset. An on-thumb
    /// press initially publishes the supplied offset unchanged.
    #[must_use]
    pub fn on_scroll_to(mut self, f: impl Fn(f32) -> Message + 'a) -> Self {
        self.on_scroll_to = Some(Box::new(f));
        self
    }

    /// Capture and publish raw wheel deltas over the child before it handles them.
    /// Callbacks run even without overflow and do not apply scrolling.
    #[must_use]
    pub fn on_wheel(mut self, f: impl Fn(mouse::ScrollDelta) -> Message + 'a) -> Self {
        self.on_wheel = Some(Box::new(f));
        self
    }

    /// Set normalised geometry and corner-radius tokens for this instance.
    /// Ink alphas resolve through the shared overlay style rather than this token override.
    #[must_use]
    pub fn scrollbar_tokens(mut self, tokens: ScrollbarTokens) -> Self {
        self.tokens = tokens.normalized();
        self
    }

    /// Resolve thumb ink against the supplied child background colour.
    /// Without an override the shared overlay style uses its palette-relative ink.
    #[must_use]
    pub fn backdrop(mut self, color: Color) -> Self {
        self.backdrop = Some(color);
        self
    }

    /// Compute chrome geometry using this instance's extents, offset and geometry tokens.
    /// See [`thumb_geometry`] for finite-input requirements and overflow suppression.
    pub fn geometry(&self, bounds: Rectangle) -> Option<ThumbGeometry> {
        thumb_geometry_with_tokens(
            bounds,
            self.content_len,
            self.viewport_len,
            self.offset,
            self.tokens,
        )
    }

    /// Resolve thumb colour through the shared overlay style and current backdrop.
    /// Dragging takes precedence over pointing; resting ink is transparent.
    pub fn thumb_ink(&self, theme: &iced::Theme, dragging: bool, pointed: bool) -> Color {
        thumb_ink(theme, dragging, pointed, self.backdrop)
    }
}
fn thumb_ink(theme: &iced::Theme, dragging: bool, pointed: bool, backdrop: Option<Color>) -> Color {
    use iced::widget::scrollable::Status;
    let status = if dragging {
        Status::Dragged {
            is_horizontal_scrollbar_dragged: false,
            is_vertical_scrollbar_dragged: true,
            is_horizontal_scrollbar_disabled: true,
            is_vertical_scrollbar_disabled: false,
        }
    } else if pointed {
        Status::Hovered {
            is_horizontal_scrollbar_hovered: false,
            is_vertical_scrollbar_hovered: true,
            is_horizontal_scrollbar_disabled: true,
            is_vertical_scrollbar_disabled: false,
        }
    } else {
        Status::Active {
            is_horizontal_scrollbar_disabled: true,
            is_vertical_scrollbar_disabled: false,
        }
    };
    let style = crate::scroll_view::overlay_scrollbar_style_over(theme, status, backdrop);
    match style.vertical_rail.scroller.background {
        Background::Color(c) => c,
        _ => Color::TRANSPARENT,
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzOffsetScrollbar<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ChromeState>()
    }

    fn state(&self) -> tree::State {
        tree::State::interactive(ChromeState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.inner));
    }

    fn size(&self) -> Size<Length> {
        self.inner.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let child = self
            .inner
            .as_widget_mut()
            .measure(&mut tree.children[0], renderer, limits);
        let size = child.size();
        layout::Node::with_children(size, vec![child]).apply(tree);
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
        let bounds = layout.bounds();
        // The thumb keeps clear of rounded corners around it, measured here
        // and kept for drawing.
        let tokens = self.tokens.normalized();
        let thumb_x = bounds.x + bounds.width - (tokens.hit_width + tokens.width) / 2.0;
        let clearance =
            crate::scrollbar_drag::vertical_clearance(bounds, (thumb_x, thumb_x + tokens.width));
        {
            let state = tree.state.downcast_mut::<ChromeState>();
            if state.clearance != clearance {
                state.clearance = clearance;
                shell.request_redraw();
            }
        }
        let geometry = self.geometry(crate::scrollbar_drag::cleared_vertical(bounds, clearance));
        if let Some(on_wheel) = &self.on_wheel
            && let Event::Mouse(mouse::Event::WheelScrolled { delta, .. }) = event
            && cursor.position().is_some_and(|p| bounds.contains(p))
        {
            shell.publish(on_wheel(*delta));
            shell.capture_event();
            shell.request_redraw();
            return;
        }
        let state = tree.state.downcast_mut::<ChromeState>();
        // The child must observe capture loss even when it cancels the scrollbar drag.
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
        // Lane presses must not activate content beneath the overlay.
        if self.on_scroll_to.is_some() {
            if let Some(geo) = geometry {
                match event {
                    Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                        if let Some(p) = cursor.position().filter(|p| geo.lane.contains(*p)) {
                            let start_scroll = if geo.thumb.contains(p) {
                                self.offset
                            } else {
                                lane_press_offset(&geo, p.y)
                            };
                            state.drag = Some(DragOrigin {
                                pointer_y: p.y,
                                scroll_at_press: start_scroll,
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
                            let new_offset = drag_target_offset(
                                &geo,
                                origin.scroll_at_press,
                                p.y - origin.pointer_y,
                            );
                            if let Some(cb) = &self.on_scroll_to {
                                shell.publish(cb(new_offset));
                            }
                            shell.capture_event();
                            shell.request_redraw();
                            return;
                        }
                    }
                    Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
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

        let state = tree.state.downcast_ref::<ChromeState>();
        let Some(geo) = self.geometry(crate::scrollbar_drag::cleared_vertical(
            layout.bounds(),
            state.clearance,
        )) else {
            return;
        };
        let dragging = state.drag.is_some();
        let pointed = cursor.position().is_some_and(|p| geo.lane.contains(p));
        let thumb = thumb_ink(theme, dragging, pointed, self.backdrop);
        if thumb.a <= 0.0 {
            return;
        }
        let radius = self.tokens.border_radius;
        renderer.with_layer(layout.bounds(), |renderer| {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: geo.thumb,
                    border: Border::default().rounded(radius),
                    shadow: iced::Shadow::default(),
                    snap: true,
                },
                Background::Color(thumb),
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
        let clearance = tree.state.downcast_ref::<ChromeState>().clearance;
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
        let child_layout = layout.children(&tree.children).next().unwrap_or(layout);
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

impl<'a, Message: 'a> EnzOffsetScrollbar<'a, Message> {
    /// Build the overlay scrollbar element.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message: 'a> From<EnzOffsetScrollbar<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzOffsetScrollbar<'a, Message>) -> Self {
        Element::new(w)
    }
}

impl tree::Interactive for ChromeState {
    fn cancel_interaction(&mut self) {
        self.drag = None;
    }
}
