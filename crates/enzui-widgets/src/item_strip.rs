use iced::advanced::layout::Measure as _;
use iced::advanced::renderer::{LayoutLayer, Renderer as _};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay};
use iced::{Element, Length};
use iced::{Event, Rectangle, Size, Vector};
use std::collections::{BTreeMap, VecDeque};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Key {
    Tab(String, bool),
    List(usize),
    Trailing,
    Prefix(usize),
    Choice(usize),
    Action(String, bool),
    ActionCollector,
}

#[derive(Default)]
struct State {
    keys: Vec<Key>,
}

pub(crate) struct Row<'a, Message> {
    policy: layout::item_strip::Policy,
    keys: Vec<Key>,
    children: Vec<Element<'a, Message>>,
}

impl<'a, Message> Row<'a, Message> {
    pub(crate) fn keyed(
        policy: layout::item_strip::Policy,
        keys: Vec<Key>,
        children: Vec<Element<'a, Message>>,
    ) -> Self {
        Self {
            policy,
            keys,
            children,
        }
    }

    pub(crate) fn prefix(
        mut items: Vec<Element<'a, Message>>,
        indicator: Element<'a, Message>,
        spacing: f32,
    ) -> Self {
        let policy = layout::item_strip::Policy::Prefix {
            items: items.len(),
            spacing,
        };
        items.push(indicator);
        let keys = (0..items.len()).map(Key::Prefix).collect();
        Self::keyed(policy, keys, items)
    }

    pub(crate) fn width_choice(below: f32, children: [Element<'a, Message>; 2]) -> Self {
        Self::keyed(
            layout::item_strip::Policy::WidthChoice { below },
            vec![Key::Choice(0), Key::Choice(1)],
            Vec::from(children),
        )
    }

    fn displayed(&self, layout: Layout, tree: &Tree) -> Vec<usize> {
        match &self.policy {
            layout::item_strip::Policy::Tabs(policy) => policy
                .displayed(&policy.plan(layout.bounds().width))
                .into_iter()
                .map(|(index, _)| index)
                .collect(),
            // A hidden child's slot (or, unwrapped, the child) is empty.
            _ => tree
                .children
                .iter()
                .enumerate()
                .filter_map(|(index, child)| {
                    let size = child
                        .interposed
                        .as_deref()
                        .map_or(child.size, layout::Node::size);
                    (size.width != 0.0 || size.height != 0.0).then_some(index)
                })
                .collect(),
        }
    }

    fn viewport(&self, layout: Layout, viewport: &Rectangle) -> Rectangle {
        if self.policy.clips_to_bounds() {
            layout
                .bounds()
                .intersection(viewport)
                .unwrap_or_else(|| Rectangle {
                    width: 0.0,
                    height: 0.0,
                    ..layout.bounds()
                })
        } else {
            *viewport
        }
    }
}

/// The declared material slot a wrapped strip interposed above a child.
fn material_slot(child: &Tree) -> Option<iced::advanced::renderer::NodeId> {
    child
        .interposed
        .as_deref()
        .and_then(|slot| layout::Placed::of(slot).container_presentation_node())
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for Row<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }
    fn size(&self) -> Size<Length> {
        self.policy.sizing()
    }

    fn diff(&mut self, tree: &mut Tree) {
        let keys = std::mem::take(&mut tree.state.downcast_mut::<State>().keys);
        let previous = std::mem::take(&mut tree.children);
        let mut retained: BTreeMap<Key, VecDeque<Tree>> = BTreeMap::new();
        for (key, child) in keys.into_iter().zip(previous) {
            retained.entry(key).or_default().push_back(child);
        }
        tree.children = self
            .keys
            .iter()
            .zip(self.children.iter_mut())
            .map(|(key, child)| {
                let mut state = retained
                    .get_mut(key)
                    .and_then(VecDeque::pop_front)
                    .unwrap_or_else(|| Tree::new(child.as_widget()));
                state.diff(child.as_widget_mut());
                state
            })
            .collect();
        tree.state
            .downcast_mut::<State>()
            .keys
            .clone_from(&self.keys);
        tree.refresh_frame_children();
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        if tree.children.len() != self.children.len() {
            self.diff(tree);
        }
        if let layout::item_strip::Policy::Actions(policy) = &mut self.policy {
            policy.sizing = self
                .children
                .iter()
                .map(|child| child.as_widget().size())
                .collect();
        }
        let node = layout::item_strip::try_resolve(limits, &self.policy, |slot, limits| {
            Ok::<_, std::convert::Infallible>(self.children[slot].as_widget_mut().measure(
                &mut tree.children[slot],
                renderer,
                limits,
            ))
        })
        .unwrap();
        node.with_item_strip_presentation(
            *limits,
            self.policy.clone(),
            self.children
                .iter()
                .map(|child| child.as_widget().size())
                .collect(),
        )
        .apply(tree);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        tree.children.is_empty() || !tree.frame_children.is_empty()
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let displayed = self.displayed(layout, tree);
        let reconcile = operation.is_native_reconciliation();
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            for (index, (child, (child_layout, state))) in self
                .children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
                .enumerate()
            {
                if reconcile || displayed.contains(&index) {
                    child.as_widget_mut().operate(
                        state,
                        child_layout,
                        viewport,
                        renderer,
                        operation,
                    );
                } else {
                    state.cancel_interactions();
                }
            }
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
        let displayed = self.displayed(layout, tree);
        let visible = self.viewport(layout, viewport);
        for (index, (child, (child_layout, state))) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
            .enumerate()
        {
            if displayed.contains(&index) {
                child.as_widget_mut().update(
                    state,
                    event,
                    child_layout,
                    cursor,
                    renderer,
                    shell,
                    &visible,
                );
                state.refresh_frame_interest(child.as_widget());
            } else {
                state.cancel_interactions();
                // A folded child cannot be reached or seen, so it holds no
                // focus that could return beside another owner when shown.
                child.as_widget_mut().operate(
                    state,
                    child_layout,
                    &visible,
                    renderer,
                    &mut iced::advanced::widget::operation::focusable::unfocus::<()>(),
                );
            }
        }
        tree.refresh_frame_children();
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
        let displayed = self.displayed(layout, tree);
        let visible = self.viewport(layout, viewport);
        for (index, (child, (child_layout, state))) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
            .enumerate()
        {
            if displayed.contains(&index) && state.has_frame_interest() {
                child.as_widget_mut().update_frame(
                    state,
                    event,
                    child_layout,
                    cursor,
                    renderer,
                    shell,
                    &visible,
                );
                state.refresh_frame_interest(child.as_widget());
            } else if !displayed.contains(&index) {
                state.cancel_interactions();
            }
        }
        tree.refresh_frame_children();
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &iced::advanced::renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let displayed = self.displayed(layout, tree);
        let wrapped = self.policy.wrapped();
        let visible = self.viewport(layout, viewport);
        let node = tree.tab_strip_presentation_node();
        let capture = node.and_then(|node| renderer.tab_strip_capture_viewport(node));
        let paint = |renderer: &mut iced::Renderer| {
            for (index, (child, (child_layout, state))) in self
                .children
                .iter()
                .zip(layout.iter(&tree.children))
                .enumerate()
            {
                if capture.is_some() || displayed.contains(&index) {
                    let paint_child = |renderer: &mut iced::Renderer| {
                        let layout = child_layout;
                        let viewport = capture.unwrap_or(visible);
                        if matches!(self.policy, layout::item_strip::Policy::Actions(_)) {
                            layout.draw_culled(state, renderer, &viewport, |renderer, viewport| {
                                child
                                    .as_widget()
                                    .draw(state, renderer, theme, style, layout, cursor, viewport);
                            });
                        } else {
                            child
                                .as_widget()
                                .draw(state, renderer, theme, style, layout, cursor, &viewport);
                        }
                    };
                    if wrapped && capture.is_some() {
                        let slot = material_slot(state).expect("declared material slot");
                        let mask = if displayed.contains(&index) {
                            visible
                        } else {
                            Rectangle {
                                width: 0.0,
                                height: 0.0,
                                ..visible
                            }
                        };
                        renderer.with_layout_layer(
                            LayoutLayer::Material(slot),
                            mask,
                            None,
                            paint_child,
                        );
                    } else {
                        paint_child(renderer);
                    }
                }
            }
        };
        if self.policy.clips_to_bounds() {
            if let Some(node) = node {
                renderer.with_layout_layer(LayoutLayer::TabStrip(node), visible, None, paint);
            } else {
                renderer.with_layer(visible, paint);
            }
        } else {
            paint(renderer);
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
        if !matches!(self.policy, layout::item_strip::Policy::Actions(_))
            && !cursor.is_over(layout.bounds())
        {
            return mouse::Interaction::default();
        }
        let displayed = self.displayed(layout, tree);
        self.children
            .iter()
            .zip(layout.iter(&tree.children))
            .enumerate()
            .filter(|(index, _)| displayed.contains(index))
            .map(|(_, (child, (layout, state)))| {
                child
                    .as_widget()
                    .mouse_interaction(state, layout, cursor, viewport, renderer)
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
        let displayed = self.displayed(layout, tree);
        let mut overlays = self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
            .enumerate()
            .map(|(index, (child, (layout, state)))| {
                if displayed.contains(&index) {
                    child.as_widget_mut().overlay(
                        state,
                        layout,
                        renderer,
                        viewport,
                        translation,
                        window,
                    )
                } else {
                    Vec::new()
                }
            });
        // A prefix strip shows the first child's overlays, all of them: an
        // anchored popup comes with the backdrop that dismisses it.
        if matches!(self.policy, layout::item_strip::Policy::Prefix { .. }) {
            return overlays.find(|child| !child.is_empty()).unwrap_or_default();
        }
        overlays.flatten().collect()
    }
}
