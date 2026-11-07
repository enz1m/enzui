//! Surface colour blending and inherited-text dimming for pressable controls.
//!
//! General interaction washes come from [`enzui_theme::Palette::overlay_hover`]
//! and [`enzui_theme::Palette::overlay_pressed`]. [`composite`] preserves the
//! base alpha when mixing RGB, so partially transparent bases do not follow
//! general source-over alpha composition.

use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Color, Element, Event, Length, Rectangle, Size, Vector};

/// Default disabled inherited-text alpha multiplier: 0.55.
pub const DISABLED_ALPHA: f32 = 0.55;

/// Weighted stored RGB sum using 0.299, 0.587 and 0.114.
/// Alpha is ignored and channels are not linearized or validated; this is not
/// WCAG relative luminance.
pub fn luminance(c: Color) -> f32 {
    0.299 * c.r + 0.587 * c.g + 0.114 * c.b
}

/// Mix base RGB toward overlay RGB using overlay alpha, preserving base alpha.
/// If base alpha is at most `f32::EPSILON`, return the overlay unchanged.
/// Inputs are not validated or clamped; partially transparent bases do not use
/// general source-over alpha composition.
pub fn composite(base: Color, overlay: Color) -> Color {
    if base.a <= f32::EPSILON {
        return overlay;
    }
    let a = overlay.a;
    Color {
        r: base.r * (1.0 - a) + overlay.r * a,
        g: base.g * (1.0 - a) + overlay.g * a,
        b: base.b * (1.0 - a) + overlay.b * a,
        a: base.a,
    }
}

/// A transparent child wrapper that multiplies inherited text alpha during draw.
/// Explicit child colours, images and child overlays are unaffected. Factors are
/// used without validation or clamping; input, focus and semantics remain enabled.
pub struct DimContent<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    dim: Box<dyn Fn(&Theme) -> f32 + 'a>,
}

impl<'a, Message, Theme, Renderer> DimContent<'a, Message, Theme, Renderer> {
    /// Multiply inherited text alpha by a fixed factor; 1.0 preserves it.
    pub fn new(child: Element<'a, Message, Theme, Renderer>, dim: f32) -> Self {
        Self::themed(child, move |_| dim)
    }

    /// Resolve the inherited-text alpha multiplier from the live theme on each draw.
    /// The result is used without validation; child overlays retain their own styling.
    pub fn themed(
        child: Element<'a, Message, Theme, Renderer>,
        dim: impl Fn(&Theme) -> f32 + 'a,
    ) -> Self {
        Self {
            child,
            dim: Box::new(dim),
        }
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for DimContent<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> tree::Tag {
        self.child.as_widget().tag()
    }
    fn state(&self) -> tree::State {
        self.child.as_widget().state()
    }
    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }
    // The shared child tree node must retain the child's frame-interest policy.
    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.child, tree)
    }
    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_transparent(
            &mut self.child,
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
        self.child.as_widget().size()
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
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
        let dimmed = renderer::Style {
            text_color: Color {
                a: style.text_color.a * (self.dim)(theme),
                ..style.text_color
            },
        };
        self.child
            .as_widget()
            .draw(tree, renderer, theme, &dimmed, layout, cursor, viewport);
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

impl<'a, Message, Theme, Renderer> From<DimContent<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: DimContent<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}
