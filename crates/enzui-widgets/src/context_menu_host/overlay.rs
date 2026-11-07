use iced::advanced::layout::Measure as _;

/// Where a submenu level opens: beside the row that opened it, with its
/// own first row level with that row.
struct SubmenuAnchor {
    parent_row: Id,
    first_row: Option<Id>,
}

/// The bounds of the widget with `id`, found by walking a laid-out surface.
struct FindRow<'i> {
    id: &'i Id,
    found: Option<Rectangle>,
}

impl Operation for FindRow<'_> {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        if self.found.is_none() {
            operate(self);
        }
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle, _viewport: &Rectangle) {
        if self.found.is_none() && id == Some(self.id) {
            self.found = Some(bounds);
        }
    }
}

/// The bounds of the row `id` in `surface`, laid out as `node`.
fn row_bounds<Message>(
    surface: &mut Element<'_, Message>,
    tree: &mut Tree,
    node: &layout::Node,
    renderer: &iced::Renderer,
    id: &Id,
) -> Option<Rectangle> {
    let mut find = FindRow { id, found: None };
    let layout = Layout::of(node);
    surface
        .as_widget_mut()
        .operate(tree, layout, &layout.bounds(), renderer, &mut find);
    find.found
}

struct MenuOverlay<'a, 'b, Message> {
    /// One per submenu level, in the order of `surfaces[1..]`.
    anchors: Vec<SubmenuAnchor>,
    surfaces: Vec<Element<'a, Message>>,
    trees: &'b mut [Tree],
    invocation: InvocationKind,
    edge_margin: f32,
    interactive_header: bool,
    /// The root menu fading and scaling in as it opens.
    reveal: crate::anchored_overlay::PopupReveal,
}

impl<Message> crate::overlay_adapter::LegacyOverlay<Message, iced::Theme, iced::Renderer>
    for MenuOverlay<'_, '_, Message>
where
    Message: Clone,
{
    fn layout(&mut self, renderer: &iced::Renderer, bounds: Size) -> layout::Node {
        // Measure before choosing a side; all surfaces must fit inside both
        // edge margins of the viewport: the window, or, when the menu does
        // not fit there, the room around it where menus may leave the window.
        let margin = self.edge_margin.max(0.0);
        let (invocation, edge_margin) = (self.invocation, self.edge_margin);
        let surfaces = &mut self.surfaces;
        let trees = &mut *self.trees;
        let anchors = &self.anchors;
        let children = crate::anchored_overlay::place_in_window_first(bounds, |room, viewport| {
            let limits = layout::Limits::new(
                Size::ZERO,
                Size::new(
                    (room.width - 2.0 * margin).max(0.0),
                    (room.height - 2.0 * margin).max(0.0),
                ),
            );
            let mut children = Vec::with_capacity(surfaces.len());
            // The surface above, and the row in it that opened this level.
            let mut previous: Option<(Rectangle, Option<Rectangle>)> = None;
            let mut fits = true;

            for (level, (surface, tree)) in surfaces.iter_mut().zip(trees.iter_mut()).enumerate() {
                let node = surface.as_widget_mut().measure(tree, renderer, &limits);
                let size = node.size();
                let anchor = level.checked_sub(1).and_then(|index| anchors.get(index));
                let placed = match previous {
                    None => {
                        let anchor = invocation.anchor();
                        let resolved = enzui_core::placement::place_at_point(
                            Point::new(anchor.x, anchor.y),
                            size,
                            viewport,
                            edge_margin,
                        );
                        Point::new(resolved.bounds.x, resolved.bounds.y)
                    }
                    // Submenus are placed in viewport coordinates.
                    // A submenu opens beside its row, its first row level
                    // with it; without a found row, level with the surface.
                    Some((parent, row)) => {
                        let inset = anchor
                            .and_then(|anchor| anchor.first_row.as_ref())
                            .and_then(|id| row_bounds(surface, tree, &node, renderer, id))
                            .map_or(0.0, |first| first.y - node.bounds().y);
                        let top = row.map_or(parent.y, |row| row.y - inset);
                        let placed = enzui_core::context_menu::place_submenu(
                            CoreRect::new(
                                parent.x - viewport.x,
                                top - viewport.y,
                                parent.width,
                                parent.height,
                            ),
                            (size.width, size.height),
                            (viewport.width, viewport.height),
                            edge_margin,
                        );
                        Point::new(placed.x + viewport.x, placed.y + viewport.y)
                    }
                };
                let rect = Rectangle::new(placed, size);
                fits &= crate::anchored_overlay::fits_within(viewport, rect);
                let node = node.move_to(placed);
                let opener = anchors
                    .get(level)
                    .and_then(|next| row_bounds(surface, tree, &node, renderer, &next.parent_row));
                previous = Some((rect, opener));
                children.push(node);
            }
            (children, fits)
        });
        // Children use absolute viewport positions; a shrink-wrapped parent would offset them twice.
        layout::Node::with_children(bounds, children)
    }

    fn area(&self, node: &layout::Node) -> Option<Rectangle> {
        crate::overlay_adapter::children_area(node)
    }

    fn anchor(&self) -> Option<Rectangle> {
        let anchor = self.invocation.anchor();
        Some(Rectangle::new(
            Point::new(anchor.x, anchor.y),
            Size::new(1.0, 1.0),
        ))
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
        let anchor = self.invocation.anchor();
        let anchor = Rectangle::new(Point::new(anchor.x, anchor.y), Size::ZERO);
        for (level, ((surface, tree), child)) in self
            .surfaces
            .iter()
            .zip(self.trees.iter())
            .zip(layout.children())
            .enumerate()
        {
            let child = child.to_layout();
            let draw = |renderer: &mut iced::Renderer| {
                surface.as_widget().draw(
                    tree,
                    renderer,
                    theme,
                    style,
                    child,
                    cursor,
                    &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
                );
            };
            // Separate layers keep each surface's fill and ink above the preceding menu.
            iced::advanced::Renderer::with_layer(
                renderer,
                crate::overlay_adapter::UNBOUNDED_VIEWPORT,
                |renderer| {
                    if level == 0 {
                        self.reveal
                            .draw(renderer, theme, child.bounds(), anchor, draw);
                    } else {
                        draw(renderer);
                    }
                },
            );
        }
    }

    fn update(
        &mut self,
        event: &Event,
        layout: iced::advanced::layout::Placed<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        if self.interactive_header
            && let Event::Keyboard(keyboard_event) = event
            && is_focus_traversal(keyboard_event)
        {
            let iced::keyboard::Event::KeyPressed { modifiers, .. } = keyboard_event else {
                return;
            };
            enzui_core::focus::note_input_event_for(
                event,
                enzui_core::focus::window_key(shell.window()),
            );
            let direction = if modifiers.shift() {
                enzui_core::focus::Traversal::Previous
            } else {
                enzui_core::focus::Traversal::Next
            };
            let mut operation = enzui_core::focus::traversal_operation::<()>(direction);
            let Some(((surface, tree), child)) = self
                .surfaces
                .iter_mut()
                .zip(self.trees.iter_mut())
                .zip(layout.children())
                .last()
            else {
                return;
            };
            loop {
                surface.as_widget_mut().operate(
                    tree,
                    child.to_layout(),
                    &child.bounds(),
                    renderer,
                    operation.as_mut(),
                );
                match operation.finish() {
                    iced::advanced::widget::operation::Outcome::Chain(next) => operation = next,
                    _ => break,
                }
            }
            shell.capture_event();
            shell.request_redraw();
            return;
        }
        // The deepest surface owns overlapping input, matching paint order.
        // A press it takes still reaches the surfaces beneath, levitating, so a parent
        // row holding focus drops it while nothing beneath activates.
        let press = matches!(event, Event::Mouse(mouse::Event::ButtonPressed(_)));
        let mut cursor = cursor;
        let mut any_over = false;
        let keyboard_depth = if self.interactive_header && matches!(event, Event::Keyboard(_)) {
            self.surfaces.len().checked_sub(1)
        } else {
            None
        };
        for (depth, ((surface, tree), child)) in self
            .surfaces
            .iter_mut()
            .zip(self.trees.iter_mut())
            .zip(layout.children())
            .enumerate()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            if keyboard_depth.is_some_and(|deepest| depth != deepest) {
                continue;
            }
            if cursor.is_over(child.bounds()) {
                any_over = true;
            }
            surface.as_widget_mut().update(
                tree,
                event,
                child.to_layout(),
                cursor,
                renderer,
                shell,
                &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
            );
            if shell.is_event_captured() {
                if !press {
                    return;
                }
                cursor = cursor.levitate();
            }
        }
        // Surface padding must consume mouse presses before the host treats them as outside.
        if matches!(event, Event::Mouse(mouse::Event::ButtonPressed(_))) && any_over {
            shell.capture_event();
        }
    }

    fn operate(
        &mut self,
        layout: iced::advanced::layout::Placed<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        for ((surface, tree), child) in self
            .surfaces
            .iter_mut()
            .zip(self.trees.iter_mut())
            .zip(layout.children())
        {
            surface.as_widget_mut().operate(
                tree,
                child.to_layout(),
                &child.bounds(),
                renderer,
                operation,
            );
        }
    }

    fn mouse_interaction(
        &self,
        layout: iced::advanced::layout::Placed<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        for ((surface, tree), child) in self
            .surfaces
            .iter()
            .zip(self.trees.iter())
            .zip(layout.children())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
        {
            if cursor.is_over(child.bounds()) {
                let inner = surface.as_widget().mouse_interaction(
                    tree,
                    child.to_layout(),
                    cursor,
                    &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
                    renderer,
                );
                // Even non-actionable menu padding must mask underlying hover.
                return if inner == mouse::Interaction::None {
                    mouse::Interaction::Idle
                } else {
                    inner
                };
            }
        }
        mouse::Interaction::default()
    }

    fn index(&self) -> f32 {
        enzui_core::overlay_layer::OverlayLayer::Menu.z_index()
    }

    fn overlay<'c>(
        &'c mut self,
        layout: iced::advanced::layout::Placed<'_>,
        renderer: &iced::Renderer,
        window: iced::Size,
    ) -> Vec<overlay::Element<'c, Message, iced::Theme, iced::Renderer>> {
        let children: Vec<_> = self
            .surfaces
            .iter_mut()
            .zip(self.trees.iter_mut())
            .zip(layout.children())
            .flat_map(|((surface, tree), child)| {
                surface.as_widget_mut().overlay(
                    tree,
                    child.to_layout(),
                    renderer,
                    &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
                    Vector::ZERO,
                    window,
                )
            })
            .collect();
        children
    }
}
