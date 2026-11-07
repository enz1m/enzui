//! The separators of a pane grid, published to the accessibility tree.

use super::*;

type PaneGridTokenResolver<'a, Theme> = dyn Fn(&Theme) -> PaneGridTokens + 'a;

/// Wrap pane content with collected adjustable splitter nodes, pointer focus and keyboard callbacks.
///
/// IDs are `{base}-sep-{index}`; structural changes can assign an ordinal ID to
/// a different split. Resolvable seam nodes precede the child content.
/// The wrapper paints a compact solid handle on hover, drag or visible focus,
/// and a separate focus ring. It does not install native resize callbacks.
pub struct PaneGridSemantics<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    model: &'a SeparatorModel,
    node: &'a iced::widget::pane_grid::Node,
    labels: Vec<Option<String>>,
    base: String,
    on_focus: Option<Box<dyn Fn(usize) -> Message + 'a>>,
    on_resize_finished: Option<Box<dyn Fn() -> Message + 'a>>,
    pointer_resizing: Option<&'a Cell<bool>>,
    focus_visible: Option<&'a Cell<bool>>,
    on_key: Option<Box<dyn Fn(iced::keyboard::Key) -> Message + 'a>>,
    tokens_resolved: Option<Box<PaneGridTokenResolver<'a, Theme>>>,
}

impl<'a, Message, Theme, Renderer> PaneGridSemantics<'a, Message, Theme, Renderer> {
    /// Wrap content with separators computed from a matching model and native tree.
    /// Layout caches split extents for subsequent keyboard and semantic ranges.
    /// Callbacks are unset; IDs default to `enz-pane-grid-sep-{index}`.
    #[must_use]
    pub fn new(
        child: impl Into<Element<'a, Message, Theme, Renderer>>,
        model: &'a SeparatorModel,
        node: &'a iced::widget::pane_grid::Node,
    ) -> Self {
        Self {
            child: child.into(),
            model,
            node,
            labels: Vec::new(),
            base: "enz-pane-grid".to_owned(),
            on_focus: None,
            on_resize_finished: None,
            pointer_resizing: None,
            focus_visible: None,
            on_key: None,
            tokens_resolved: None,
        }
    }

    /// A stable base string for the per-separator ids (`{base}-sep-{i}`).
    ///
    /// Set this per pane grid when a view hosts more than one, so their
    /// separator ids do not collide in the `NodeIdMap`.
    #[must_use]
    pub fn id_base(mut self, base: impl Into<String>) -> Self {
        self.base = base.into();
        self
    }

    /// Accessible labels per separator, in [`SeparatorModel::separators`]
    /// order. A shorter/empty list leaves later separators unlabelled (the
    /// node still publishes with orientation, range and value).
    #[must_use]
    pub fn labels(mut self, labels: impl IntoIterator<Item = Option<String>>) -> Self {
        self.labels = labels.into_iter().collect();
        self
    }

    /// Emit a message when a pointer press focuses a splitter.
    #[must_use]
    pub fn on_focus(mut self, callback: impl Fn(usize) -> Message + 'a) -> Self {
        self.on_focus = Some(Box::new(callback));
        self
    }

    /// Share pointer gesture state and publish completion on left release,
    /// cursor exit or window unfocus. The focus callback must be installed for
    /// a seam press to start that gesture.
    #[must_use]
    pub fn on_resize_finished(
        mut self,
        pointer_resizing: &'a Cell<bool>,
        focus_visible: &'a Cell<bool>,
        callback: impl Fn() -> Message + 'a,
    ) -> Self {
        self.pointer_resizing = Some(pointer_resizing);
        self.focus_visible = Some(focus_visible);
        self.on_resize_finished = Some(Box::new(callback));
        self
    }

    /// Emit a message for resize keys while a splitter is focused.
    #[must_use]
    pub fn on_key(mut self, callback: impl Fn(iced::keyboard::Key) -> Message + 'a) -> Self {
        self.on_key = Some(Box::new(callback));
        self
    }

    /// Resolve handle and focus-ring paint tokens from the draw-time theme.
    /// The model's stored tokens still control geometry and pointer hit slop.
    /// Nonfinite or negative paint dimensions use stock defaults; zero is preserved.
    #[must_use]
    pub fn tokens_resolved(mut self, resolve: impl Fn(&Theme) -> PaneGridTokens + 'a) -> Self {
        self.tokens_resolved = Some(Box::new(resolve));
        self
    }
    fn separator_id(&self, index: usize) -> iced::advanced::widget::Id {
        iced::advanced::widget::Id::from(format!("{}-sep-{index}", self.base))
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for PaneGridSemantics<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut iced::advanced::widget::Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        renderer: &Renderer,
        limits: &iced::advanced::layout::Limits,
    ) {
        let node = self.child.as_widget_mut().measure(tree, renderer, limits);
        self.model.refresh_extents(self.node, node.bounds().size());
        node.apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        let bounds = self.model.separator_bounds(
            self.node,
            layout.bounds().position(),
            layout.bounds().size(),
        );
        for (index, rect) in bounds.into_iter().enumerate() {
            let Some(rect) = rect else { continue };
            let label = self.labels.get(index).cloned().flatten();
            let Some(sem) = self.model.semantics(index, label) else {
                continue;
            };
            let id = self.separator_id(index);
            let mut node = SemanticsNode::new(sem);
            operation.custom(Some(&id), rect, &mut node);
        }
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            self.child
                .as_widget_mut()
                .operate(tree, layout, viewport, renderer, operation);
        });
    }

    fn update(
        &mut self,
        tree: &mut iced::advanced::widget::Tree,
        event: &iced::Event,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);

        match event {
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)) => {
                if let (Some(position), Some(on_focus)) =
                    (cursor.position(), self.on_focus.as_ref())
                {
                    let seams = self.model.separator_bounds(
                        self.node,
                        layout.bounds().position(),
                        layout.bounds().size(),
                    );
                    let slop = self.model.tokens().handle_slop;
                    if let Some(index) = seams.iter().enumerate().position(|(index, bounds)| {
                        let Some(bounds) = *bounds else {
                            return false;
                        };
                        let Some(orientation) =
                            self.model.get(index).map(|separator| separator.orientation)
                        else {
                            return false;
                        };
                        hit_bounds(bounds, orientation, slop).contains(position)
                    }) {
                        if let Some(active) = self.pointer_resizing {
                            active.set(true);
                        }
                        if let Some(visible) = self.focus_visible {
                            visible.set(false);
                        }
                        shell.publish(on_focus(index));
                    }
                }
            }
            iced::Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
                if self
                    .pointer_resizing
                    .is_some_and(|active| active.replace(false))
                    && let Some(on_finished) = &self.on_resize_finished
                {
                    shell.publish(on_finished());
                    shell.request_redraw();
                }
            }
            iced::Event::Mouse(iced::mouse::Event::CursorLeft)
            | iced::Event::Window(iced::window::Event::Unfocused) => {
                if self
                    .pointer_resizing
                    .is_some_and(|active| active.replace(false))
                    && let Some(on_finished) = &self.on_resize_finished
                {
                    shell.publish(on_finished());
                    shell.request_redraw();
                }
            }
            iced::Event::Keyboard(iced::keyboard::Event::KeyPressed { key, modifiers, .. })
                if self.model.focused().is_some() =>
            {
                if let Some(on_key) = &self.on_key {
                    // Filter modifiers before the callback discards them from its key payload.
                    let relevant = self.model.on_key_with_modifiers(key, *modifiers).is_some();
                    if relevant {
                        if let Some(visible) = self.focus_visible {
                            visible.set(true);
                        }
                        shell.publish(on_key(key.clone()));
                        shell.capture_event();
                    }
                }
            }
            _ => {}
        }
    }

    fn draw(
        &self,
        tree: &iced::advanced::widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &iced::advanced::renderer::Style,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
        let t = self
            .tokens_resolved
            .as_ref()
            .map_or_else(|| self.model.tokens(), |resolve| resolve(theme))
            .sanitized();
        let seams = self.model.separator_bounds(
            self.node,
            layout.bounds().position(),
            layout.bounds().size(),
        );
        let cursor_pos = cursor.position();
        let slop = self.model.tokens().handle_slop;
        let dragging = self.pointer_resizing.is_some_and(std::cell::Cell::get);
        let focused = self.model.focused();
        // Direct wrappers without a visibility cell show any focused separator.
        let focus_visible = self.focus_visible.is_none_or(std::cell::Cell::get);

        iced::advanced::Renderer::with_layer(renderer, layout.bounds(), |renderer| {
            for (index, seam) in seams.iter().enumerate() {
                let Some(seam) = *seam else {
                    continue;
                };
                let Some(orientation) =
                    self.model.get(index).map(|separator| separator.orientation)
                else {
                    continue;
                };
                let is_dragged = dragging && focused == Some(index);
                let is_key_focused = focused == Some(index) && focus_visible;
                let is_hovered = !dragging
                    && cursor_pos.is_some_and(|position| {
                        hit_bounds(seam, orientation, slop).contains(position)
                    });

                let ink = if is_dragged || is_key_focused {
                    Some(t.seam_active)
                } else if is_hovered {
                    Some(t.handle_ink)
                } else {
                    None
                };

                if let Some(ink) = ink {
                    paint_handle_indicator(renderer, seam, orientation, ink);
                }
            }
        });
        let Some(focused) = focused else {
            return;
        };
        if !focus_visible {
            return;
        }
        if t.focus_ring_width <= 0.0 {
            return;
        }
        let Some(Some(seam)) = seams.get(focused).copied() else {
            return;
        };
        let pad = t.focus_ring_width;
        // A separate layer keeps the ring above pane bodies on the wgpu backend.
        iced::advanced::Renderer::with_layer(renderer, layout.bounds(), |renderer| {
            crate::focus_button::paint_focus_ring(
                renderer,
                seam,
                crate::focus_button::FocusRingQuad {
                    ink: t.focus_ring,
                    width: t.focus_ring_width,
                    radius: t.focus_ring_radius.into(),
                    outset: pad,
                    snap: true,
                },
            );
        });
    }

    fn mouse_interaction(
        &self,
        tree: &iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> iced::advanced::mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut iced::advanced::widget::Tree,
        layout: iced::advanced::Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: iced::Vector,
        window: Size,
    ) -> Vec<iced::advanced::overlay::Element<'b, Message, Theme, Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message, Theme, Renderer> From<PaneGridSemantics<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: PaneGridSemantics<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}

/// Construct [`PaneGridSemantics`] for content and a matching model/native tree.
/// Install callbacks to emit focus, keyboard or resize-completion messages.
#[must_use]
pub fn pane_grid_semantics<'a, Message, Theme, Renderer>(
    child: impl Into<Element<'a, Message, Theme, Renderer>>,
    model: &'a SeparatorModel,
    node: &'a iced::widget::pane_grid::Node,
) -> PaneGridSemantics<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    PaneGridSemantics::new(child, model, node)
}
