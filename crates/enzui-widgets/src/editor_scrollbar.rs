//! Overlay scrollbar for a caller-owned iced text editor.
//!
//! The child must display the supplied `Content`. The adapter passes through
//! layout without translating the child and publishes absolute `Action::ScrollTo`
//! actions; the host applies them through its editor reducer. Vertical chrome
//! is enabled by default, while horizontal chrome requires opt-in and overflow.
//! Pointer loss or window focus loss cancels a drag without publishing an offset.
//! [`enz_text_area`](crate::text_area::enz_text_area) includes this adapter;
//! [`enz_editor_scrollbar`] supports independently composed editor chrome.
//!
//! Chunked hosts retain leading overscan by mapping a global target to a local
//! line and performing [`Action::ScrollToLine`]. Its vertical offset is in pixels
//! within that paragraph; the projection's estimated mean height is for chrome,
//! not for restoring a wrapped logical line.

use iced::advanced::layout::Measure as _;
use iced::advanced::layout::{self, Layout};
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::{Operation, Tree, Widget, tree};
use iced::advanced::{Renderer as _, Shell};
use iced::widget::text_editor::{Action, Content, ScrollMetrics};
use iced::{Element, Event, Length, Rectangle, Size, Vector, mouse};
use std::rc::Rc;

#[derive(Debug, Clone, Copy)]
enum Drag {
    Vertical { grab: f32 },
    Horizontal { grab: f32 },
}

#[derive(Debug, Default)]
struct State {
    drag: Option<Drag>,
    // Event handling has no theme; hit geometry uses the preceding draw's tokens.
    resolved_tokens: std::cell::Cell<enzui_theme::ScrollbarTokens>,
    /// How far the rails' ends keep from the rounded corners around them:
    /// vertical leading and trailing, then horizontal.
    clearance: [f32; 4],
}

type AxisGeometry = iced::advanced::text::editor::scrollbar::Geometry;

fn owned_metrics(
    tokens: enzui_theme::ScrollbarTokens,
) -> iced::advanced::text::editor::scrollbar::Metrics {
    iced::advanced::text::editor::scrollbar::Metrics {
        width: tokens.width,
        hit_width: tokens.hit_width,
        min_thumb_length: tokens.min_thumb_length,
        radius: tokens.border_radius,
    }
}
fn vertical_geometry(
    bounds: Rectangle,
    metrics: ScrollMetrics,
    tokens: enzui_theme::ScrollbarTokens,
    clearance: [f32; 4],
) -> Option<AxisGeometry> {
    use iced::advanced::text::editor::scrollbar;
    scrollbar::vertical(
        scrollbar::cleared(bounds, clearance, true),
        metrics,
        owned_metrics(tokens),
    )
}
fn horizontal_geometry(
    bounds: Rectangle,
    metrics: ScrollMetrics,
    tokens: enzui_theme::ScrollbarTokens,
    inset: f32,
    clearance: [f32; 4],
) -> Option<AxisGeometry> {
    use iced::advanced::text::editor::scrollbar;
    scrollbar::horizontal_inset(
        scrollbar::cleared(bounds, clearance, false),
        metrics,
        owned_metrics(tokens),
        inset,
    )
}

/// How far the editor's rails must keep from the rounded corners around
/// them, for thumbs `tokens` wide along `bounds`' trailing and lower edges.
fn corner_clearance(
    bounds: Rectangle,
    tokens: enzui_theme::ScrollbarTokens,
    horizontal_inset: f32,
) -> [f32; 4] {
    let offset = (tokens.hit_width + tokens.width) / 2.0;
    let x = bounds.x + bounds.width - offset;
    let [top, bottom] = crate::scrollbar_drag::vertical_clearance(bounds, (x, x + tokens.width));
    let y = bounds.y + bounds.height - offset;
    let lower = Rectangle {
        x: bounds.x + horizontal_inset,
        width: (bounds.width - horizontal_inset).max(0.0),
        ..bounds
    };
    let [left, right] = crate::scrollbar_drag::horizontal_clearance(lower, (y, y + tokens.width));
    [top, bottom, left, right]
}

fn editor_horizontal_inset(layout: Layout, children: &[Tree]) -> f32 {
    layout
        .iter(children)
        .next()
        .and_then(|(editor, tree)| editor.editor_content_bounds(tree))
        .map_or(0.0, |content| (content.x - layout.bounds().x).max(0.0))
}

fn offset_from_pointer(
    pointer: f32,
    track_start: f32,
    geometry: AxisGeometry,
    grab: f32,
    current_offset: f32,
) -> f32 {
    if geometry.travel <= 0.0 {
        current_offset.clamp(0.0, geometry.max_offset)
    } else {
        ((pointer - track_start - grab) / geometry.travel).clamp(0.0, 1.0) * geometry.max_offset
    }
}

/// Overlay scrollbar over a caller-owned editor displaying the supplied `Content`.
/// Construct with [`enz_editor_scrollbar`]; the host applies published scroll actions.
#[must_use]
pub struct EnzEditorScrollbar<'a, Message> {
    child: Element<'a, Message>,
    content: &'a Content,
    on_action: Rc<dyn Fn(Action) -> Message + 'a>,
    horizontal: bool,
    metrics_override: Option<ScrollMetrics>,
    projection: Option<iced::advanced::text::editor::scrollbar::Projection>,
}

/// Wrap an editor with overlay scrollbar chrome without translating its layout.
/// The child must display `content`; `on_action` maps absolute `Action::ScrollTo`
/// actions for the host to apply. Defaults to vertical chrome and live editor
/// metrics. Pointer geometry uses the last drawn theme tokens, or defaults before
/// the first draw. Horizontal chrome requires [`EnzEditorScrollbar::horizontal`].
pub fn enz_editor_scrollbar<'a, Message: 'a>(
    editor: impl Into<Element<'a, Message>>,
    content: &'a Content,
    on_action: impl Fn(Action) -> Message + 'a,
) -> EnzEditorScrollbar<'a, Message> {
    EnzEditorScrollbar {
        child: editor.into(),
        content,
        on_action: Rc::new(on_action),
        horizontal: false,
        metrics_override: None,
        projection: None,
    }
}

impl<'a, Message> EnzEditorScrollbar<'a, Message> {
    /// Override scrollbar geometry with full-document metrics.
    /// Clears any projection. Published offsets use these coordinates; the host maps
    /// them into a materialized editor window when its native coordinates differ.
    pub fn metrics(mut self, metrics: ScrollMetrics) -> Self {
        self.metrics_override = Some(metrics);
        self.projection = None;
        self
    }

    /// Project live editor metrics into full-document coordinates.
    /// Clears any metrics override. Published offsets use projected coordinates and
    /// require host mapping when applied to a materialized editor window. An invalid
    /// projection hides chrome and cancels a drag while forwarding events to the child.
    pub fn projection(
        mut self,
        projection: iced::advanced::text::editor::scrollbar::Projection,
    ) -> Self {
        self.projection = Some(projection);
        self.metrics_override = None;
        self
    }

    fn input_metrics(&self) -> Option<ScrollMetrics> {
        let raw = self
            .metrics_override
            .unwrap_or_else(|| self.content.scroll_metrics());
        self.projection
            .map_or(Some(raw), |projection| projection.apply(raw))
    }
    pub(crate) fn with_shared_action(
        child: impl Into<Element<'a, Message>>,
        content: &'a Content,
        on_action: Rc<dyn Fn(Action) -> Message + 'a>,
        horizontal: bool,
    ) -> Self {
        Self {
            child: child.into(),
            content,
            on_action,
            horizontal,
            metrics_override: None,
            projection: None,
        }
    }

    /// Enable horizontal chrome when content overflows the viewport width.
    /// Defaults to false; enabling it does not create overflow in wrapped content.
    pub fn horizontal(mut self, yes: bool) -> Self {
        self.horizontal = yes;
        self
    }

    /// Build the editor and scrollbar wrapper element.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message>
    where
        Message: 'a,
    {
        Element::from(self)
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzEditorScrollbar<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::interactive(State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        layout::passthrough(limits, self.child.as_widget().size(), |limits| {
            self.child
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, limits)
        })
        .apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            let child_layout = layout
                .children(&tree.children)
                .next()
                .expect("adapter has one child");
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                child_layout,
                viewport,
                renderer,
                operation,
            );
        });
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
        let bounds = layout.bounds();
        let Some(metrics) = self.input_metrics() else {
            tree.state.downcast_mut::<State>().drag = None;
            let child_layout = layout.child(&tree.children, 0);
            self.child.as_widget_mut().update(
                &mut tree.children[0],
                event,
                child_layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
            crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
            return;
        };
        let horizontal_inset = editor_horizontal_inset(layout, &tree.children);
        let state = tree.state.downcast_mut::<State>();
        let tokens = state.resolved_tokens.get();
        // The thumbs keep clear of rounded corners around them, measured here
        // and kept for drawing.
        let clearance = corner_clearance(bounds, tokens, horizontal_inset);
        if state.clearance != clearance {
            state.clearance = clearance;
            shell.request_redraw();
        }
        let vertical = vertical_geometry(bounds, metrics, tokens, clearance);
        let horizontal = self
            .horizontal
            .then(|| horizontal_geometry(bounds, metrics, tokens, horizontal_inset, clearance))
            .flatten();
        let pointer = cursor.position();
        // Compositor loss may omit release; the child must still receive the loss event.
        if crate::scrollbar_drag::cancels_mouse_drag(event) && state.drag.take().is_some() {
            shell.request_redraw();
        }

        match (event, state.drag, pointer) {
            (Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)), Some(_), _) => {
                state.drag = None;
                shell.capture_event();
                return;
            }
            (
                Event::Mouse(mouse::Event::CursorMoved { .. }),
                Some(Drag::Vertical { grab }),
                Some(p),
            ) => {
                if let Some(geometry) = vertical {
                    let y =
                        offset_from_pointer(p.y, geometry.band.y, geometry, grab, metrics.offset.y);
                    shell.publish((self.on_action)(Action::ScrollTo {
                        offset: Vector::new(metrics.offset.x, y),
                    }));
                    shell.capture_event();
                    return;
                }
            }
            (
                Event::Mouse(mouse::Event::CursorMoved { .. }),
                Some(Drag::Horizontal { grab }),
                Some(p),
            ) => {
                if let Some(geometry) = horizontal {
                    let x =
                        offset_from_pointer(p.x, geometry.band.x, geometry, grab, metrics.offset.x);
                    shell.publish((self.on_action)(Action::ScrollTo {
                        offset: Vector::new(x, metrics.offset.y),
                    }));
                    shell.capture_event();
                    return;
                }
            }
            (Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)), None, Some(p)) => {
                if let Some(geometry) = vertical.filter(|geometry| geometry.band.contains(p)) {
                    let grab = if geometry.thumb.contains(p) {
                        p.y - geometry.thumb.y
                    } else {
                        geometry.thumb.height / 2.0
                    };
                    state.drag = Some(Drag::Vertical { grab });
                    let y =
                        offset_from_pointer(p.y, geometry.band.y, geometry, grab, metrics.offset.y);
                    shell.publish((self.on_action)(Action::ScrollTo {
                        offset: Vector::new(metrics.offset.x, y),
                    }));
                    shell.capture_event();
                    return;
                }
                if let Some(geometry) = horizontal.filter(|geometry| geometry.band.contains(p)) {
                    let grab = if geometry.thumb.contains(p) {
                        p.x - geometry.thumb.x
                    } else {
                        geometry.thumb.width / 2.0
                    };
                    state.drag = Some(Drag::Horizontal { grab });
                    let x =
                        offset_from_pointer(p.x, geometry.band.x, geometry, grab, metrics.offset.x);
                    shell.publish((self.on_action)(Action::ScrollTo {
                        offset: Vector::new(x, metrics.offset.y),
                    }));
                    shell.capture_event();
                    return;
                }
            }
            _ => {}
        }

        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("adapter has one child");
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
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
            &mut self.child,
            tree,
            0,
            event,
            layout
                .children(&tree.children)
                .next()
                .expect("adapter has one child"),
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
        // Query before child draw: a metrics cache update can replace the Arc
        // referenced weakly by the submitted editor primitive.
        let bounds = layout.bounds();
        let metrics = self
            .metrics_override
            .unwrap_or_else(|| self.content.scroll_metrics());
        let tokens = crate::theme_lookup::scrollbar(theme);
        let state = tree.state.downcast_ref::<State>();
        state.resolved_tokens.set(tokens);
        let ink = crate::theme_lookup::palette(theme).text;

        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout
                .children(&tree.children)
                .next()
                .expect("adapter has one child"),
            cursor,
            viewport,
        );

        let metrics = self
            .metrics_override
            .unwrap_or_else(|| self.content.cached_scroll_metrics().unwrap_or(metrics));

        let paint = iced::advanced::text::editor::scrollbar::Paint {
            metrics: owned_metrics(tokens),
            projection: self.projection,
            horizontal: self.horizontal,
            horizontal_inset: editor_horizontal_inset(layout, &tree.children),
            cursor: cursor.position(),
            dragged: [
                matches!(state.drag, Some(Drag::Vertical { .. })),
                matches!(state.drag, Some(Drag::Horizontal { .. })),
            ],
            ink,
            alpha: tokens.alpha,
            alpha_hover: tokens.alpha_hover,
            clearance: state.clearance,
        };
        if self.metrics_override.is_none()
            && let (Some(owner), Some(editor)) = (
                tree.container_presentation_node(),
                tree.children[0].editor_presentation_node(),
            )
        {
            renderer.fill_layout_editor_scrollbars(
                owner,
                editor,
                self.content.presentation_identity(),
                bounds,
                metrics,
                paint,
            );
            return;
        }
        layout.reject_presentation_paint(tree, renderer);
        for (quad, color) in paint.quads(bounds, metrics).into_iter().flatten() {
            renderer.fill_quad(quad, color);
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
        let Some(metrics) = self.input_metrics() else {
            return self.child.as_widget().mouse_interaction(
                &tree.children[0],
                layout.child(&tree.children, 0),
                cursor,
                viewport,
                renderer,
            );
        };
        let state = tree.state.downcast_ref::<State>();
        let tokens = state.resolved_tokens.get();
        let over_bar = cursor.position().is_some_and(|p| {
            vertical_geometry(layout.bounds(), metrics, tokens, state.clearance)
                .is_some_and(|g| g.band.contains(p))
                || (self.horizontal
                    && horizontal_geometry(
                        layout.bounds(),
                        metrics,
                        tokens,
                        editor_horizontal_inset(layout, &tree.children),
                        state.clearance,
                    )
                    .is_some_and(|g| g.band.contains(p)))
        });
        if over_bar || state.drag.is_some() {
            mouse::Interaction::Pointer
        } else {
            self.child.as_widget().mouse_interaction(
                &tree.children[0],
                layout
                    .children(&tree.children)
                    .next()
                    .expect("adapter has one child"),
                cursor,
                viewport,
                renderer,
            )
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
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("adapter has one child");
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            child_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<EnzEditorScrollbar<'a, Message>> for Element<'a, Message> {
    fn from(value: EnzEditorScrollbar<'a, Message>) -> Self {
        Element::new(value)
    }
}

impl tree::Interactive for State {
    fn cancel_interaction(&mut self) {
        self.drag = None;
    }
}
