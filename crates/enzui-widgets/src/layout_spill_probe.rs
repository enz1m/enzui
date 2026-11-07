//! A transparent child wrapper that publishes an independent layout audit region.
//!
//! Pass the collector to the laid-out UI's `operate` method to obtain findings.
//! The default audit region uses the parent's offered maximum at the child's
//! final origin. [`LayoutSpillProbe::reference_bounds`] supplies an independent
//! local rectangle when the intended allocation is smaller than that maximum.
//! Neither choice changes layout or clips paint. Non-finite reference fields
//! produce no finding through [`spill_amount`].
//!
//! Operation payloads carry the renderer's scale, defaulting to one when absent.
//! They retain allocation bounds before scroll translation and viewport clipping;
//! the collector applies those transformations. Before layout, an unset reference
//! falls back to child bounds and cannot report spill. [`crate::bounds_probe()`]
//! supplies the pure edge measurements.

use crate::bounds_probe::bounds_probe;
use enzui_core::layout_spill_scan::LayoutSpillProbeNode;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// A transparent wrapper that publishes a [`LayoutSpillProbeNode`] for its child.
#[must_use]
pub struct LayoutSpillProbe<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    allow_spill: bool,
    id: Option<iced::advanced::widget::Id>,
    reference_bounds: Option<Rectangle>,
    container_size: std::cell::Cell<Option<Size>>,
}

impl<'a, Message, Theme, Renderer> LayoutSpillProbe<'a, Message, Theme, Renderer> {
    /// Wrap a child to report overflow, with no ID or explicit reference.
    pub fn new(child: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        Self {
            child: child.into(),
            allow_spill: false,
            id: None,
            reference_bounds: None,
            container_size: std::cell::Cell::new(None),
        }
    }

    /// Mark overflow intentional; [`spill_amount`] suppresses the resulting finding.
    pub fn allow_spill(mut self, allow: bool) -> Self {
        self.allow_spill = allow;
        self
    }

    /// Attach a stable ID to this probe's operation payload for reporting.
    pub fn id(mut self, id: iced::advanced::widget::Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Audit against a rectangle local to the probe's final origin.
    /// The rectangle is translated during operations after ancestor placement,
    /// independently of the child's measured size. Values are stored without validation.
    pub fn reference_bounds(mut self, bounds: Rectangle) -> Self {
        self.reference_bounds = Some(bounds);
        self
    }

    /// Audit against a same-origin reference size in logical pixels.
    /// Equivalent to [`Self::reference_bounds`] with a zero local offset.
    pub fn reference_size(self, size: Size) -> Self {
        self.reference_bounds(Rectangle::new(iced::Point::ORIGIN, size))
    }
}

/// Wrap a child with the default layout audit policy; see [`LayoutSpillProbe::new`].
pub fn layout_spill_probe<'a, Message, Theme, Renderer>(
    child: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> LayoutSpillProbe<'a, Message, Theme, Renderer> {
    LayoutSpillProbe::new(child)
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for LayoutSpillProbe<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        // Capture only size here; ancestors position the child before operate supplies its origin.
        let max = limits.max();
        let node = self.child.as_widget_mut().measure(tree, renderer, limits);
        self.container_size.set(Some(max));
        node.apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        // Audit allocation, leaving scroll translation and viewport clipping to the collector.
        let child = layout.bounds();
        let container = match self.reference_bounds {
            Some(reference) => Rectangle::new(
                iced::Point::new(child.x + reference.x, child.y + reference.y),
                reference.size(),
            ),
            None => match self.container_size.get() {
                Some(size) => Rectangle::new(child.position(), size),
                None => child,
            },
        };
        let scale = renderer
            .scale()
            .map_or(1.0, iced_core::renderer::Scale::total);
        let mut node = LayoutSpillProbeNode {
            child,
            container,
            allow_spill: self.allow_spill,
            scale,
        };
        operation.custom(self.id.as_ref(), container, &mut node);
        operation.container(self.id.as_ref(), layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            self.child
                .as_widget_mut()
                .operate(tree, layout, viewport, renderer, operation);
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message, Theme, Renderer> From<LayoutSpillProbe<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: LayoutSpillProbe<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}

/// Report the worst edge overflow beyond the scale-adjusted tolerance.
/// Returns `None` for allowed spill, non-finite rectangle fields or an amount
/// within [`crate::bounds_probe::spill_epsilon_for_scale`]. The reported amount
/// stays in logical pixels; scale changes the threshold only.
pub fn spill_amount(
    child: Rectangle,
    container: Rectangle,
    allow_spill: bool,
    scale: f32,
) -> Option<f32> {
    let p = bounds_probe(child, container, allow_spill);
    let epsilon = crate::bounds_probe::spill_epsilon_for_scale(scale);
    (p.spill_amount > epsilon).then_some(p.spill_amount)
}
