//! Headless frame collection and optional widget-tree census helpers.
//!
//! The `test-frame` feature supplies a concrete renderer and runtime UI so callers can measure real layout at an explicit size.

use crate::collect::{AppSubtreeCensus, SemanticsCollector, app_subtree_census};
use iced_core::layout::Measure as _;
use iced_core::{Element, Size};

/// Measurements from one laid-out frame.
///
/// `widget_nodes` counts realized widget-tree nodes, including the root, independently of accessibility-node count.
#[derive(Debug)]
pub struct FrameTreeCensus {
    /// Accessibility observations collected from the measured frame.
    pub collector: SemanticsCollector,
    /// Total realised widget-tree nodes, including the root.
    pub widget_nodes: usize,
}

fn tree_node_count(tree: &iced_core::widget::Tree) -> usize {
    1 + tree.children.iter().map(tree_node_count).sum::<usize>()
}

/// Builds and operates a headless frame at `size`, returning its semantic collector.
///
/// The renderer honors `ICED_TEST_BACKEND`; callers should pass the real top-level view so responsive shell behavior is included.
///
/// # Panics
///
/// Panics if the selected headless renderer cannot be initialized.
pub fn collect_frame<'a, Message, Theme>(
    view: Element<'a, Message, Theme, iced_test::renderer::Renderer>,
    size: Size,
) -> SemanticsCollector
where
    Message: 'a,
    Theme: 'a,
{
    use iced_core::renderer::Headless;
    use iced_core::widget::Operation;

    // Each fixture owns out-of-tree widget hand-offs so state from a dropped UI cannot leak into this frame.
    let _runtime_scope = crate::widget_runtime::WidgetRuntimeScope::new();

    // Keep the renderer binding even when its selected fallback is zero-sized because `UserInterface::build` borrows it mutably.
    let mut renderer = iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as Headless>::new(
            iced_core::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");
    let mut ui = iced_test::runtime::user_interface::UserInterface::build(
        view,
        size,
        iced_test::runtime::user_interface::Cache::default(),
        &mut renderer,
    );
    let mut collector = SemanticsCollector::new();
    ui.operate(&renderer, {
        let op: &mut dyn Operation = &mut collector;
        op
    });
    collector
}

/// Builds, lays out, and operates one frame while counting the realized widget tree.
///
/// The count follows the runtime’s first-cache diff and layout order. It is a widget-node count, not an erased-element or model-inventory count.
///
/// # Panics
///
/// Panics if the selected headless renderer cannot be initialized.
pub fn collect_frame_with_tree_census<'a, Message, Theme>(
    mut view: Element<'a, Message, Theme, iced_test::renderer::Renderer>,
    size: Size,
) -> FrameTreeCensus
where
    Message: 'a,
    Theme: 'a,
{
    use iced_core::layout::{self, Layout};
    use iced_core::renderer::Headless;
    use iced_core::widget::{Operation, Tree};

    // Clear retained focus so the census depends only on this view and size.
    let _runtime_scope = crate::widget_runtime::WidgetRuntimeScope::new();

    let renderer = iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as Headless>::new(
            iced_core::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");

    // Follow `UserInterface::build`’s first-cache ordering.
    let mut tree = Tree::empty();
    tree.diff(view.as_widget_mut());
    let base =
        view.as_widget_mut()
            .measure(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, size));
    let widget_nodes = tree_node_count(&tree);

    let mut collector = SemanticsCollector::new();
    view.as_widget_mut().operate(
        &mut tree,
        Layout::of(&base),
        &iced_core::Rectangle::with_size(size),
        &renderer,
        {
            let operation: &mut dyn Operation = &mut collector;
            operation
        },
    );

    FrameTreeCensus {
        collector,
        widget_nodes,
    }
}

/// Measurements for the whole realized tree and marked app-owned subtrees.
#[derive(Debug)]
pub struct FrameAppSubtreeCensus {
    /// Accessibility observations collected from the measured frame.
    pub collector: SemanticsCollector,
    /// Total realized widget-tree nodes, including the supplied root.
    pub widget_nodes: usize,
    /// The app-owned subtree total and boundary count.
    pub app: AppSubtreeCensus,
}

/// Builds one frame and counts the complete realized tree plus nodes in outermost marked app-owned subtrees.
///
/// Measurement follows first-cache diff, layout, and operation before a pure tree pass. Nested markers increase the boundary count without double-counting their nodes.
///
/// # Panics
///
/// Panics if the selected headless renderer cannot be initialized.
pub fn collect_frame_with_app_subtree_census<'a, Message, Theme>(
    mut view: Element<'a, Message, Theme, iced_test::renderer::Renderer>,
    size: Size,
) -> FrameAppSubtreeCensus
where
    Message: 'a,
    Theme: 'a,
{
    use iced_core::layout::{self, Layout};
    use iced_core::renderer::Headless;
    use iced_core::widget::{Operation, Tree};

    // Clear retained focus so this measurement is independent of prior fixtures.
    let _runtime_scope = crate::widget_runtime::WidgetRuntimeScope::new();

    let renderer = iced_test::futures::futures::executor::block_on(
        <iced_test::renderer::Renderer as Headless>::new(
            iced_core::renderer::Settings::default(),
            std::env::var("ICED_TEST_BACKEND").ok().as_deref(),
        ),
    )
    .expect("headless renderer");

    // Follow `UserInterface::build`’s first-cache ordering.
    let mut tree = Tree::empty();
    tree.diff(view.as_widget_mut());
    let base =
        view.as_widget_mut()
            .measure(&mut tree, &renderer, &layout::Limits::new(Size::ZERO, size));
    let widget_nodes = tree_node_count(&tree);
    let app = app_subtree_census(&tree);

    let mut collector = SemanticsCollector::new();
    view.as_widget_mut().operate(
        &mut tree,
        Layout::of(&base),
        &iced_core::Rectangle::with_size(size),
        &renderer,
        {
            let operation: &mut dyn Operation = &mut collector;
            operation
        },
    );

    FrameAppSubtreeCensus {
        collector,
        widget_nodes,
        app,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collect::SemanticsNode;
    use crate::semantics::{Role, Semantics};
    use iced_core::layout::{self, Layout};
    use iced_core::renderer::Quad;
    use iced_core::renderer::Style;
    use iced_core::widget::{Id, Operation, Tree, Widget};
    use iced_core::{Color, Event, Length, Rectangle, Renderer as _, Shell, mouse};
    use std::cell::RefCell;
    use std::rc::Rc;

    const PROBE_INK: Color = Color {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };

    #[derive(Default)]
    struct ProbeRecord {
        events: usize,
    }

    struct RecordingProbe {
        rec: Rc<RefCell<ProbeRecord>>,
        id: Id,
    }

    impl Widget<&'static str, iced_core::Theme, iced_test::renderer::Renderer> for RecordingProbe {
        fn size(&self) -> iced_core::Size<Length> {
            iced_core::Size::new(Length::Fixed(120.0), Length::Fixed(40.0))
        }

        fn layout(
            &mut self,
            tree: &mut Tree,
            _renderer: &iced_test::renderer::Renderer,
            _limits: &layout::Limits,
        ) {
            layout::Node::new(iced_core::Size::new(120.0, 40.0)).apply(tree);
        }

        fn draw(
            &self,
            _tree: &Tree,
            renderer: &mut iced_test::renderer::Renderer,
            _theme: &iced_core::Theme,
            _style: &Style,
            layout: Layout,
            _cursor: mouse::Cursor,
            _viewport: &Rectangle,
        ) {
            renderer.fill_quad(
                Quad {
                    bounds: layout.bounds(),
                    ..Quad::default()
                },
                PROBE_INK,
            );
        }

        fn operate(
            &mut self,
            _tree: &mut Tree,
            layout: Layout,
            _viewport: &Rectangle,
            _renderer: &iced_test::renderer::Renderer,
            operation: &mut dyn Operation,
        ) {
            let mut node =
                SemanticsNode::new(Semantics::new(Role::Button).label("Probe").default_action());
            operation.custom(Some(&self.id), layout.bounds(), &mut node);
        }

        fn update(
            &mut self,
            _tree: &mut Tree,
            event: &Event,
            layout: Layout,
            cursor: mouse::Cursor,
            _renderer: &iced_test::renderer::Renderer,
            shell: &mut Shell<'_, &'static str>,
            _viewport: &Rectangle,
        ) {
            if let Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event
                && cursor.is_over(layout.bounds())
            {
                self.rec.borrow_mut().events += 1;
                shell.publish("clicked");
                shell.capture_event();
            }
        }
    }

    impl From<RecordingProbe>
        for Element<'static, &'static str, iced_core::Theme, iced_test::renderer::Renderer>
    {
        fn from(probe: RecordingProbe) -> Self {
            Element::new(probe)
        }
    }

    struct Probe {
        node: SemanticsNode,
    }

    impl Probe {
        fn new(node: SemanticsNode) -> Self {
            Self { node }
        }
    }

    impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer> for Probe
    where
        Renderer: iced_core::Renderer,
    {
        fn size(&self) -> iced_core::Size<Length> {
            iced_core::Size::new(Length::Fixed(24.0), Length::Fixed(24.0))
        }

        fn layout(&mut self, tree: &mut Tree, _renderer: &Renderer, _limits: &layout::Limits) {
            layout::Node::new(iced_core::Size::new(24.0, 24.0)).apply(tree);
        }

        fn draw(
            &self,
            _tree: &Tree,
            _renderer: &mut Renderer,
            _theme: &Theme,
            _style: &Style,
            _layout: Layout,
            _cursor: mouse::Cursor,
            _viewport: &Rectangle,
        ) {
        }

        fn operate(
            &mut self,
            _tree: &mut Tree,
            layout: Layout,
            _viewport: &Rectangle,
            _renderer: &Renderer,
            operation: &mut dyn Operation,
        ) {
            let mut node = self.node.clone();
            operation.custom(None, layout.bounds(), &mut node);
        }
    }

    impl<'a, Message, Theme, Renderer> From<Probe> for Element<'a, Message, Theme, Renderer>
    where
        Renderer: iced_core::Renderer + 'a,
        Message: 'a,
        Theme: 'a,
    {
        fn from(probe: Probe) -> Self {
            Element::new(probe)
        }
    }

    struct Branch<'a, Message, Theme, Renderer> {
        children: Vec<Element<'a, Message, Theme, Renderer>>,
    }

    impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
        for Branch<'a, Message, Theme, Renderer>
    where
        Renderer: iced_core::Renderer,
    {
        fn size(&self) -> iced_core::Size<Length> {
            iced_core::Size::new(Length::Shrink, Length::Shrink)
        }

        fn diff(&mut self, tree: &mut Tree) {
            tree.diff_children(&mut self.children);
        }

        fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
            let children = self
                .children
                .iter_mut()
                .zip(&mut tree.children)
                .map(|(child, child_tree)| {
                    child.as_widget_mut().measure(child_tree, renderer, limits)
                })
                .collect();
            layout::Node::with_children(limits.max(), children).apply(tree);
        }

        fn draw(
            &self,
            _tree: &Tree,
            _renderer: &mut Renderer,
            _theme: &Theme,
            _style: &iced_core::renderer::Style,
            _layout: Layout,
            _cursor: iced_core::mouse::Cursor,
            _viewport: &iced_core::Rectangle,
        ) {
        }

        fn operate(
            &mut self,
            tree: &mut Tree,
            layout: Layout,
            viewport: &Rectangle,
            renderer: &Renderer,
            operation: &mut dyn Operation,
        ) {
            for (child, (child_layout, child_tree)) in self
                .children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
            {
                child.as_widget_mut().operate(
                    child_tree,
                    child_layout,
                    viewport,
                    renderer,
                    operation,
                );
            }
        }
    }

    impl<'a, Message, Theme, Renderer> From<Branch<'a, Message, Theme, Renderer>>
        for Element<'a, Message, Theme, Renderer>
    where
        Renderer: iced_core::Renderer + 'a,
        Message: 'a,
        Theme: 'a,
    {
        fn from(branch: Branch<'a, Message, Theme, Renderer>) -> Self {
            Element::new(branch)
        }
    }

    #[test]
    fn collect_frame_walks_the_view_and_collects_semantics() {
        let view: Element<'_, (), iced_core::Theme, iced_test::renderer::Renderer> = Probe::new(
            SemanticsNode::new(Semantics::new(Role::Button).label("Save")),
        )
        .into();
        let collector = collect_frame(view, Size::new(400.0, 300.0));
        let roles: Vec<Role> = collector
            .collected()
            .iter()
            .map(|n| n.semantics.role)
            .collect();
        assert!(
            roles.contains(&Role::Button),
            "expected the button's semantics in the collected frame, got {roles:?}"
        );
    }
}
