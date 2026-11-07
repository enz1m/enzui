use iced::advanced::layout::Measure as _;

struct TitlebarLayout<'a, Message> {
    children: Vec<Element<'a, Message>>,
    policy: layout::priority_band::Specification,
    // Probe nodes by slot, limits and scale. Slots are fixed until the next
    // view rebuild creates a new instance; a probe re-shapes every label.
    probe_cache: Vec<(usize, layout::Limits, f32, layout::Node)>,
}

/// The content slot's clip: the band's clip rule reads the declarations
/// below the slot, in its tree.
fn content_slot_clip(content: Layout, tree: &Tree) -> Rectangle {
    layout::priority_band::content_clip_in(content, tree, crate::bounds_probe::SPILL_EPSILON)
}

impl<'a, Message: 'a> TitlebarLayout<'a, Message> {
    #[expect(
        clippy::too_many_arguments,
        reason = "constructor mirrors independently configured titlebar slots and constraints"
    )]
    fn new(
        content: Element<'a, Message>,
        center: Option<Element<'a, Message>>,
        actions: Option<Element<'a, Message>>,
        controls: Option<Element<'a, Message>>,
        spacing: f32,
        has_window_title: bool,
        action_min_width: f32,
        action_reserve_width: f32,
        action_can_render_without_intrinsic_width: bool,
        fluid_content_min_width: Option<f32>,
        modal_navigation: bool,
    ) -> Self {
        // Empty slots preserve child indices across diff, input, and overlay traversal.
        let empty = || -> Element<'a, Message> {
            Space::new()
                .width(Length::Fixed(0.0))
                .height(Length::Fixed(0.0))
                .into()
        };
        let has_controls = controls.is_some();
        let has_center = center.is_some();
        let has_actions = actions.is_some();
        let center = center.unwrap_or_else(empty);
        let actions = actions.unwrap_or_else(empty);
        let controls = controls.unwrap_or_else(empty);
        let ordinary = |child| -> Element<'a, Message> {
            crate::inert::Inert::new(child, modal_navigation)
                .without_paint()
                .into()
        };
        Self {
            probe_cache: Vec::new(),
            children: vec![
                ordinary(content),
                ordinary(center),
                ordinary(actions),
                controls,
            ],
            policy: layout::priority_band::Specification {
                has_controls,
                has_center,
                has_actions,
                action_min_width,
                action_reserve_width,
                action_can_render_without_intrinsic_width,
                fluid_content_min_width,
                has_window_title,
                spacing,
                height: Length::Shrink,
                padding_x: 0.0,
                centring_height: 0.0,
                action_probe_width: MIN_ACTIONS_RESERVE,
                leading_share: LEADING_CHROME_SHARE,
                spill_epsilon: crate::bounds_probe::SPILL_EPSILON,
                mirrored: false,
            },
        }
    }

    fn height(mut self, h: Length) -> Self {
        self.policy.height = h;
        self
    }

    fn padding_x(mut self, p: f32) -> Self {
        self.policy.padding_x = if p.is_finite() && p > 0.0 { p } else { 0.0 };
        self
    }

    fn mirrored(mut self, mirrored: bool) -> Self {
        self.policy.mirrored = mirrored;
        self
    }

    fn centring_height(mut self, h: f32) -> Self {
        self.policy.centring_height = if h.is_finite() && h > 0.0 { h } else { 0.0 };
        self
    }
}

impl<'a, Message> From<TitlebarLayout<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(w: TitlebarLayout<'a, Message>) -> Self {
        Element::new(w)
    }
}

const CONTENT: usize = layout::priority_band::CONTENT;

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for TitlebarLayout<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::stateless()
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fill,
            height: self.policy.height,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let policy = self.policy;
        let mut used_ambient_deficit = false;
        let mut probes = Vec::new();
        // The band reads each slot's descendant extent, which a measured node
        // is too shallow to hold; each slot's node carries it.
        let node = layout::priority_band::try_resolve(limits, policy, |index, limits, pass| {
            let node = match pass {
                layout::priority_band::Pass::Probe => {
                    let scale = iced::advanced::Renderer::scale(renderer)
                        .map_or(1.0, iced_core::renderer::Scale::total);
                    let cached = self.probe_cache.iter().find(|(slot, at, at_scale, _)| {
                        *slot == index && at == limits && *at_scale == scale
                    });
                    let node = if let Some((_, _, _, node)) = cached {
                        node.clone()
                    } else {
                        let mut scratch = Tree::new(self.children[index].as_widget());
                        self.children[index].as_widget_mut().diff(&mut scratch);
                        let node = self.children[index]
                            .as_widget_mut()
                            .measure(&mut scratch, renderer, limits)
                            .with_measured_extent(layout::priority_band::tree_extent(&scratch));
                        // Bounded against a width that never settles.
                        if self.probe_cache.len() >= 16 {
                            self.probe_cache.remove(0);
                        }
                        self.probe_cache.push((index, *limits, scale, node.clone()));
                        node
                    };
                    probes.push(layout::priority_band::Probe::capture(index, *limits, &node));
                    node
                }
                layout::priority_band::Pass::Layout => self.children[index]
                    .as_widget_mut()
                    .measure(&mut tree.children[index], renderer, limits)
                    .with_measured_extent(layout::priority_band::tree_extent(
                        &tree.children[index],
                    )),
                layout::priority_band::Pass::Renegotiate(deficit) => {
                    used_ambient_deficit = true;
                    with_band_deficit(deficit, || {
                        self.children[index]
                            .as_widget_mut()
                            .measure(&mut tree.children[index], renderer, limits)
                            .with_measured_extent(layout::priority_band::tree_extent(
                                &tree.children[index],
                            ))
                    })
                }
            };
            Ok::<_, std::convert::Infallible>(node)
        })
        .unwrap();
        // Ambient callback-derived geometry has no owned declaration.
        if used_ambient_deficit {
            return node.apply(tree);
        }
        node.with_priority_band_presentation(
            *limits,
            policy,
            self.children
                .iter()
                .map(|child| child.as_widget().size())
                .collect(),
            probes,
        )
        .apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        operation.container(None, layout.bounds(), viewport);
        // Clipped descendants cannot report their hidden deficit through ordinary traversal.
        if let Some(content) = layout.children(&tree.children).nth(CONTENT) {
            let container = content.bounds();
            let child = layout::priority_band::layout_extent_in(content, &tree.children[CONTENT]);
            let spill = (child.x + child.width) - (container.x + container.width);
            if spill > crate::bounds_probe::SPILL_EPSILON {
                let scale = iced::advanced::Renderer::scale(renderer)
                    .map_or(1.0, iced_core::renderer::Scale::total);
                let mut node = enzui_core::layout_spill_scan::LayoutSpillProbeNode {
                    child,
                    container,
                    allow_spill: false,
                    scale,
                };
                operation.custom(Some(&content_oversubscription_id()), container, &mut node);
            }
        }
        operation.traverse(&mut |operation| {
            for (index, (child, (lay, state))) in self
                .children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
                .enumerate()
            {
                if index == CONTENT {
                    let mut clipped = crate::inert_clip::ClipOperation::new(
                        operation,
                        content_slot_clip(lay, state),
                    )
                    .whole_targets(true);
                    child
                        .as_widget_mut()
                        .operate(state, lay, viewport, renderer, &mut clipped);
                } else {
                    child
                        .as_widget_mut()
                        .operate(state, lay, viewport, renderer, operation);
                }
            }
        });
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut iced::advanced::Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        for (index, (child, (lay, state))) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
            .enumerate()
        {
            let cursor = if index == CONTENT {
                crate::inert_clip::cursor_within(cursor, content_slot_clip(lay, state))
            } else {
                cursor
            };
            child
                .as_widget_mut()
                .update(state, event, lay, cursor, renderer, shell, viewport);
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        for (index, (child, (lay, state))) in self
            .children
            .iter()
            .zip(layout.iter(&tree.children))
            .enumerate()
        {
            if index == CONTENT {
                let capture = tree.priority_band_presentation_node().and_then(|node| {
                    iced::advanced::Renderer::layout_capture_viewport(renderer, node)
                });
                let slot = content_slot_clip(lay, state);
                let Some(clip) = slot.intersection(viewport).or_else(|| {
                    capture.map(|_| Rectangle {
                        x: slot.x.max(viewport.x),
                        y: slot.y.max(viewport.y),
                        width: 0.0,
                        height: 0.0,
                    })
                }) else {
                    continue;
                };
                let draw = |renderer: &mut iced::Renderer| {
                    child.as_widget().draw(
                        state,
                        renderer,
                        theme,
                        style,
                        lay,
                        cursor,
                        &capture.unwrap_or(clip),
                    );
                };
                if let Some(node) = tree.priority_band_presentation_node() {
                    iced::advanced::Renderer::with_layout_layer(
                        renderer,
                        renderer::LayoutLayer::BandContent(node),
                        clip,
                        None,
                        draw,
                    );
                } else {
                    iced::advanced::Renderer::with_layer(renderer, clip, draw);
                }
            } else {
                child
                    .as_widget()
                    .draw(state, renderer, theme, style, lay, cursor, viewport);
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
        self.children
            .iter()
            .zip(layout.iter(&tree.children))
            .enumerate()
            .map(|(index, (child, (lay, state)))| {
                let cursor = if index == CONTENT {
                    crate::inert_clip::cursor_within(cursor, content_slot_clip(lay, state))
                } else {
                    cursor
                };
                child
                    .as_widget()
                    .mouse_interaction(state, lay, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
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
        let layouts: Vec<_> = layout.children(&tree.children).collect();
        for ((child, state), lay) in self
            .children
            .iter_mut()
            .zip(tree.children.iter_mut())
            .zip(layouts.iter())
        {
            let overlays =
                child
                    .as_widget_mut()
                    .overlay(state, *lay, renderer, viewport, translation, window);

            if !overlays.is_empty() {
                return overlays;
            }
        }
        Vec::new()
    }
}
