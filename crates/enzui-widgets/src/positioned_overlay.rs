//! Point-anchored surfaces over a live page, with light dismissal.
//!
//! The host supplies a surface and anchor while open, and `None` while closed.
//! Placement flips overflowing axes and clamps the origin to an eight-pixel
//! viewport margin. The anchor receives the host's overlay translation. The
//! surface is measured within that margin; children ignoring limits can overflow.
//!
//! The surface receives events first. Unclaimed Escape and known-position
//! outside presses publish dismissal and are consumed; other unclaimed events
//! can reach the base. No scrim or focus trap is added. Surface operations and
//! nested overlays remain available, and the base retains its own overlays.
//! Use [`crate::context_menu_host::enz_point_context_menu_host`] for menu
//! navigation, focus restoration and menu accessibility semantics.

use enzui_core::menu_model::CloseCause;
use enzui_core::placement::place_at_point;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

/// Clearance kept between the surface and every viewport edge, in logical px.
///
/// Matches [`crate::context_menu_host::EDGE_MARGIN`], so a point-anchored
/// surface raised through this layer keeps the same clearance a hosted context
/// menu does.
pub const EDGE_MARGIN: f32 = 8.0;

/// A layer that places an optional `surface` at an app-supplied point over a
/// live `base`.
///
/// Build with [`enz_positioned_overlay`]. When the surface is `None` the widget is
/// a transparent pass-through: it contributes nothing to events, operations,
/// semantics or the overlay, and the base behaves exactly as if the widget were
/// not there.
pub struct EnzPositionedOverlay<'a, Message> {
    base: Element<'a, Message>,
    surface: Option<(Point, Element<'a, Message>)>,
    on_dismiss: Box<dyn Fn(CloseCause) -> Message + 'a>,
    edge_margin: f32,
    index: f32,
}

/// Place `surface` at a point over `base`, dismissing on an outside press or
/// Escape.
///
/// This compatibility entry point discards the dismissal cause. New consumers
/// should prefer [`enz_positioned_overlay_with_cause`] so outside press and
/// Escape cannot be mistaken for a surface action.
pub fn enz_positioned_overlay<'a, Message: 'a>(
    base: impl Into<Element<'a, Message>>,
    surface: Option<(Point, Element<'a, Message>)>,
    on_dismiss: impl Fn() -> Message + 'a,
) -> EnzPositionedOverlay<'a, Message> {
    enz_positioned_overlay_with_cause(base, surface, move |_| on_dismiss())
}

/// Place `surface` at a point and report why it was dismissed.
///
/// Pass `None` for the surface when it is closed. Unclaimed Escape reports
/// [`CloseCause::Escape`]. An unclaimed press geometrically outside the surface reports
/// [`CloseCause::OutsideClick`] and is consumed, so it does not also
/// activate the control behind it. A press inside is never reported as an
/// outside dismissal. Surface children receive events before the host checks
/// dismissal, including events outside their bounds.
pub fn enz_positioned_overlay_with_cause<'a, Message: 'a>(
    base: impl Into<Element<'a, Message>>,
    surface: Option<(Point, Element<'a, Message>)>,
    on_dismiss: impl Fn(CloseCause) -> Message + 'a,
) -> EnzPositionedOverlay<'a, Message> {
    EnzPositionedOverlay {
        base: base.into(),
        surface,
        on_dismiss: Box::new(on_dismiss),
        edge_margin: EDGE_MARGIN,
        index: enzui_core::overlay_layer::OverlayLayer::Menu.z_index(),
    }
}

impl<'a, Message> EnzPositionedOverlay<'a, Message> {
    /// Clearance kept from each viewport edge. Defaults to [`EDGE_MARGIN`].
    ///
    /// `0` is an expressible intent ("flush to the edge") and is honoured
    /// literally; only non-finite or negative input falls back to the default,
    /// the same rule [`crate::context_menu_host::EnzContextMenuHost::edge_margin`]
    /// applies.
    #[must_use]
    pub fn edge_margin(mut self, margin: f32) -> Self {
        self.edge_margin = if margin.is_finite() && margin >= 0.0 {
            margin
        } else {
            EDGE_MARGIN
        };
        self
    }

    /// Set the stacking index; defaults to the shared menu layer.
    ///
    /// Higher overlays draw above lower ones. The value is used without validation.
    #[must_use]
    pub fn index(mut self, index: f32) -> Self {
        self.index = index;
        self
    }

    /// Whether a surface is currently open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.surface.is_some()
    }

    fn child_count(&self) -> usize {
        1 + usize::from(self.surface.is_some())
    }

    /// Resolve a point-anchored origin using the shared per-axis flip/clamp policy.
    ///
    /// This does not resize the surface. If its size exceeds the margin-inset
    /// viewport, it can still overflow even though its origin is clamped.
    #[must_use]
    pub fn resolve_origin(
        anchor: Point,
        surface_size: Size,
        viewport: Size,
        edge_margin: f32,
    ) -> Point {
        let resolved = place_at_point(
            anchor,
            surface_size,
            Rectangle::new(Point::ORIGIN, viewport),
            edge_margin,
        );
        Point::new(resolved.bounds.x, resolved.bounds.y)
    }
}

impl<'a, Message> Widget<Message, iced::Theme, iced::Renderer>
    for EnzPositionedOverlay<'a, Message>
{
    fn diff(&mut self, tree: &mut Tree) {
        let expected = self.child_count();
        let mut children: Vec<&mut Element<'a, Message>> = match self.surface.as_mut() {
            Some((_, surface)) => vec![&mut self.base, surface],
            None => vec![&mut self.base],
        };
        debug_assert_eq!(
            children.len(),
            expected,
            "the overlay tree has one child per layer"
        );
        // Diff through Tree so changing a child tag recreates its retained state.
        tree.diff_children_custom(
            &mut children,
            |state, item| state.diff(item.as_widget_mut()),
            |item| Tree::new(item.as_widget()),
        );
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::stateless()
    }

    fn size(&self) -> Size<Length> {
        self.base.as_widget().size()
    }

    fn is_void(&self) -> bool {
        self.base.as_widget().is_void()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.base
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
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
        self.base.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
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
        self.base.as_widget().mouse_interaction(
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
        self.base.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        // Surface operations run only through the overlay, avoiding duplicate semantics.
        self.base.as_widget_mut().operate(
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
        let (base_tree, surface_tree) = {
            let (first, rest) = tree.children.split_at_mut(1);
            (&mut first[0], rest.first_mut())
        };

        let base_overlay = self.base.as_widget_mut().overlay(
            base_tree,
            layout,
            renderer,
            viewport,
            translation,
            window,
        );

        let surface_overlay = match (self.surface.as_mut(), surface_tree) {
            (Some((anchor, surface)), Some(surface_tree)) => {
                let anchored = Point::new(anchor.x + translation.x, anchor.y + translation.y);
                vec![crate::overlay_adapter::element(
                    SurfaceOverlay {
                        surface,
                        tree: surface_tree,
                        anchor: anchored,
                        on_dismiss: &self.on_dismiss,
                        edge_margin: self.edge_margin,
                        index: self.index,
                    },
                    renderer,
                    window,
                )]
            }
            _ => Vec::new(),
        };

        {
            let mut overlays = base_overlay;
            overlays.extend(surface_overlay);
            overlays
        }
    }
}

struct SurfaceOverlay<'a, 'b, Message> {
    surface: &'b mut Element<'a, Message>,
    tree: &'b mut Tree,
    anchor: Point,
    on_dismiss: &'b dyn Fn(CloseCause) -> Message,
    edge_margin: f32,
    index: f32,
}

impl<Message> crate::overlay_adapter::LegacyOverlay<Message, iced::Theme, iced::Renderer>
    for SurfaceOverlay<'_, '_, Message>
{
    fn layout(&mut self, renderer: &iced::Renderer, bounds: Size) -> layout::Node {
        let margin = self.edge_margin.max(0.0);
        let placed = crate::anchored_overlay::place_in_window_first(bounds, |room, viewport| {
            let cap = Size::new(
                (room.width - 2.0 * margin).max(0.0),
                (room.height - 2.0 * margin).max(0.0),
            );
            // Measure against the whole inset viewport before choosing a flip direction.
            let limits = layout::Limits::new(Size::ZERO, cap);
            let node = self
                .surface
                .as_widget_mut()
                .measure(self.tree, renderer, &limits);
            let resolved = place_at_point(self.anchor, node.size(), viewport, self.edge_margin);
            let fits = crate::anchored_overlay::fits_within(viewport, resolved.bounds);
            (
                node.move_to(Point::new(resolved.bounds.x, resolved.bounds.y)),
                fits,
            )
        });
        layout::Node::with_children(bounds, vec![placed])
    }

    fn area(&self, node: &layout::Node) -> Option<Rectangle> {
        crate::overlay_adapter::children_area(node)
    }

    fn anchor(&self) -> Option<Rectangle> {
        Some(Rectangle::new(self.anchor, Size::new(1.0, 1.0)))
    }

    fn grabs(&self) -> bool {
        true
    }

    fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: iced::advanced::layout::Placed<'_>,
        cursor: mouse::Cursor,
    ) {
        let Some(surface_layout) = layout.children().next() else {
            return;
        };
        self.surface.as_widget().draw(
            self.tree,
            renderer,
            theme,
            style,
            surface_layout.to_layout(),
            cursor,
            &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
        );
    }

    fn update(
        &mut self,
        event: &Event,
        layout: iced::advanced::layout::Placed<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        let Some(surface_layout) = layout.children().next() else {
            return;
        };
        let surface_bounds = surface_layout.bounds();

        self.surface.as_widget_mut().update(
            self.tree,
            event,
            surface_layout.to_layout(),
            cursor,
            renderer,
            shell,
            &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
        );
        if shell.is_event_captured() {
            return;
        }

        if matches!(
            event,
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                ..
            })
        ) {
            shell.publish((self.on_dismiss)(CloseCause::Escape));
            shell.capture_event();
            return;
        }

        let press_position = match event {
            Event::Mouse(mouse::Event::ButtonPressed(_)) => cursor.position(),
            Event::Touch(iced::touch::Event::FingerPressed { position, .. }) => Some(*position),
            _ => None,
        };
        if press_position.is_some_and(|position| !surface_bounds.contains(position)) {
            shell.publish((self.on_dismiss)(CloseCause::OutsideClick));
            shell.capture_event();
        }
    }

    fn operate(
        &mut self,
        layout: iced::advanced::layout::Placed<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let Some(surface_layout) = layout.children().next() else {
            return;
        };
        self.surface.as_widget_mut().operate(
            self.tree,
            surface_layout.to_layout(),
            &surface_layout.bounds(),
            renderer,
            operation,
        );
    }

    fn mouse_interaction(
        &self,
        layout: iced::advanced::layout::Placed<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let Some(surface_layout) = layout.children().next() else {
            return mouse::Interaction::default();
        };
        if cursor.is_over(surface_layout.bounds()) {
            let inner = self.surface.as_widget().mouse_interaction(
                self.tree,
                surface_layout.to_layout(),
                cursor,
                &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
                renderer,
            );
            if inner == mouse::Interaction::None {
                // A non-None interaction suppresses base hover beneath an otherwise inert surface.
                mouse::Interaction::Idle
            } else {
                inner
            }
        } else {
            mouse::Interaction::default()
        }
    }

    fn overlay<'c>(
        &'c mut self,
        layout: iced::advanced::layout::Placed<'_>,
        renderer: &iced::Renderer,
        window: iced::Size,
    ) -> Vec<overlay::Element<'c, Message, iced::Theme, iced::Renderer>> {
        let Some(surface_layout) = layout.children().next() else {
            return Vec::new();
        };
        self.surface.as_widget_mut().overlay(
            self.tree,
            surface_layout.to_layout(),
            renderer,
            &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
            Vector::ZERO,
            window,
        )
    }

    fn index(&self) -> f32 {
        self.index
    }
}

impl<'a, Message: 'a> EnzPositionedOverlay<'a, Message> {
    /// Consume the builder into an [`Element`] without requiring type inference at the call site.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message: 'a> From<EnzPositionedOverlay<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzPositionedOverlay<'a, Message>) -> Self {
        Element::new(w)
    }
}
