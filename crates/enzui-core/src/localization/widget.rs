use super::Localizer;
use iced_core::widget::{Operation, Tree, tree};
use iced_core::{
    Element, Event, Layout, Length, Rectangle, Shell, Size, Vector, Widget, layout, mouse, overlay,
    renderer,
};

impl Localizer {
    /// Builds a view and carries its locale through layout, events, rendering, accessibility and nested overlays.
    ///
    /// Each view retains a clone of this localizer, allowing independent window
    /// languages without an event-loop scope. Rebuild the view after selection
    /// changes so construction-time strings follow the new language.
    pub fn view<'a, Message: 'a, Theme: 'a, Renderer: renderer::Renderer + 'a>(
        &self,
        build: impl FnOnce() -> Element<'a, Message, Theme, Renderer>,
    ) -> Element<'a, Message, Theme, Renderer> {
        Element::new(LocalizedWidget {
            localizer: self.clone(),
            content: self.scope(build),
        })
    }
}

struct LocalizedWidget<'a, Message, Theme, Renderer> {
    localizer: Localizer,
    content: Element<'a, Message, Theme, Renderer>,
}

impl<Message, Theme, Renderer: renderer::Renderer> Widget<Message, Theme, Renderer>
    for LocalizedWidget<'_, Message, Theme, Renderer>
{
    fn tag(&self) -> tree::Tag {
        self.localizer.scope(|| self.content.as_widget().tag())
    }
    fn state(&self) -> tree::State {
        self.localizer.scope(|| self.content.as_widget().state())
    }
    fn diff(&mut self, tree: &mut Tree) {
        self.localizer
            .scope(|| self.content.as_widget_mut().diff(tree));
    }
    fn size(&self) -> Size<Length> {
        self.localizer.scope(|| self.content.as_widget().size())
    }
    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.localizer
            .scope(|| self.content.as_widget_mut().layout(tree, renderer, limits));
    }
    fn frame_interest(&self, tree: &Tree) -> bool {
        self.localizer
            .scope(|| self.content.as_widget().frame_interest(tree))
    }
    fn is_void(&self) -> bool {
        self.localizer.scope(|| self.content.as_widget().is_void())
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
        self.localizer.scope(|| {
            self.content
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
        });
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.localizer.scope(|| {
            self.content
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
        self.localizer.scope(|| {
            self.content
                .as_widget_mut()
                .update(tree, event, layout, cursor, renderer, shell, viewport);
        });
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
        self.localizer.scope(|| {
            self.content
                .as_widget_mut()
                .update_frame(tree, event, layout, cursor, renderer, shell, viewport);
        });
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.localizer.scope(|| {
            self.content
                .as_widget()
                .mouse_interaction(tree, layout, cursor, viewport, renderer)
        })
    }
    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'a, Message, Theme, Renderer>> {
        let localizer = self.localizer.clone();
        let overlays = localizer.scope(|| {
            self.content.as_widget_mut().overlay(
                tree,
                layout,
                renderer,
                viewport,
                translation,
                window,
            )
        });
        wrap_overlays(&localizer, overlays)
    }
}

struct LocalizedOverlay<'a, Message, Theme, Renderer> {
    localizer: Localizer,
    content: overlay::Element<'a, Message, Theme, Renderer>,
}

fn wrap_overlays<'a, Message: 'a, Theme: 'a, Renderer: renderer::Renderer + 'a>(
    localizer: &Localizer,
    overlays: Vec<overlay::Element<'a, Message, Theme, Renderer>>,
) -> Vec<overlay::Element<'a, Message, Theme, Renderer>> {
    overlays
        .into_iter()
        .map(|content| {
            overlay::Element::new(Box::new(LocalizedOverlay {
                localizer: localizer.clone(),
                content,
            }))
        })
        .collect()
}

impl<Message, Theme, Renderer: renderer::Renderer> overlay::Overlay<Message, Theme, Renderer>
    for LocalizedOverlay<'_, Message, Theme, Renderer>
{
    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        cursor: mouse::Cursor,
    ) {
        self.localizer.scope(|| {
            self.content
                .as_overlay()
                .draw(renderer, theme, style, cursor);
        });
    }
    fn operate(&mut self, renderer: &Renderer, operation: &mut dyn Operation) {
        self.localizer
            .scope(|| self.content.as_overlay_mut().operate(renderer, operation));
    }
    fn update(
        &mut self,
        event: &Event,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        self.localizer.scope(|| {
            self.content
                .as_overlay_mut()
                .update(event, cursor, renderer, shell);
        });
    }
    fn mouse_interaction(&self, cursor: mouse::Cursor, renderer: &Renderer) -> mouse::Interaction {
        self.localizer.scope(|| {
            self.content
                .as_overlay()
                .mouse_interaction(cursor, renderer)
        })
    }
    fn overlay<'a>(
        &'a mut self,
        renderer: &Renderer,
    ) -> Vec<overlay::Element<'a, Message, Theme, Renderer>> {
        let localizer = self.localizer.clone();
        let overlays = localizer.scope(|| self.content.as_overlay_mut().overlay(renderer));
        wrap_overlays(&localizer, overlays)
    }
    fn index(&self) -> f32 {
        self.localizer.scope(|| self.content.as_overlay().index())
    }
    fn node(&self) -> Option<&layout::Node> {
        self.localizer.scope(|| self.content.as_overlay().node())
    }
    // The runtime reads these to show a menu that leaves the window in a
    // popup; without them it sees one window-sized overlay and clips it.
    fn area(&self) -> Option<iced_core::Rectangle> {
        self.localizer.scope(|| self.content.as_overlay().area())
    }
    fn anchor(&self) -> Option<iced_core::Rectangle> {
        self.localizer.scope(|| self.content.as_overlay().anchor())
    }
    fn grabs(&self) -> bool {
        self.localizer.scope(|| self.content.as_overlay().grabs())
    }
}
