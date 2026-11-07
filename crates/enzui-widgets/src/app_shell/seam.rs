// Live seam layout retains content state while width-dependent descendants reflow.

use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

pub(crate) struct LiveSeamPad<'a, Message> {
    child: Element<'a, Message>,
    pad: Pad<'a>,
    trailing: bool,
}

enum Pad<'a> {
    Callback(Box<dyn Fn() -> f32 + 'a>),
    Driven {
        policy: layout::inset::Policy,
        progress: std::rc::Rc<std::cell::Cell<f32>>,
        driver: iced::advanced::renderer::NodeId,
    },
}

impl<'a, Message> LiveSeamPad<'a, Message> {
    pub(crate) fn driven(
        child: impl Into<Element<'a, Message>>,
        policy: layout::inset::Policy,
        progress: std::rc::Rc<std::cell::Cell<f32>>,
        driver: iced::advanced::renderer::NodeId,
    ) -> Self {
        Self {
            child: child.into(),
            pad: Pad::Driven {
                policy,
                progress,
                driver,
            },
            trailing: false,
        }
    }

    pub(crate) fn new(child: impl Into<Element<'a, Message>>, left: impl Fn() -> f32 + 'a) -> Self {
        Self {
            child: child.into(),
            pad: Pad::Callback(Box::new(left)),
            trailing: false,
        }
    }

    pub(crate) fn trailing_edge(mut self, trailing: bool) -> Self {
        self.trailing = trailing;
        self
    }

    fn child_layout(layout: Layout, tree: &Tree) -> Layout {
        layout
            .children(&tree.children)
            .next()
            .expect("LiveSeamPad always owns one child layout")
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for LiveSeamPad<'_, Message> {
    fn size(&self) -> Size<Length> {
        if self.trailing {
            // Right-edge padding spans the parent width to anchor against the sidebar seam.
            Size::new(Length::Fill, Length::Fit)
        } else {
            // Preserve a filling child’s width so sibling window controls keep their allocation.
            let child = self.child.as_widget().size().width;
            let width = if child.is_fill() { child } else { Length::Fit };
            Size::new(width, Length::Fit)
        }
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
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
            Self::child_layout(layout, tree),
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let size = self.size();
        let node = match &self.pad {
            Pad::Driven {
                policy,
                progress,
                driver,
            } => layout::inset::resolve(
                limits,
                size,
                *policy,
                progress.get(),
                self.trailing,
                *driver,
                |limits| {
                    self.child
                        .as_widget_mut()
                        .measure(&mut tree.children[0], renderer, limits)
                },
            ),
            Pad::Callback(left) => {
                let pad = left().max(0.0);
                let padding = if self.trailing {
                    iced::Padding::ZERO.right(pad)
                } else {
                    iced::Padding::ZERO.left(pad)
                };
                let align = if self.trailing {
                    iced::Alignment::End
                } else {
                    iced::Alignment::Start
                };
                // A live callback cannot be replaced with constant owned padding.
                layout::positioned_node(
                    limits,
                    size.width,
                    size.height,
                    padding,
                    |limits| {
                        self.child.as_widget_mut().measure(
                            &mut tree.children[0],
                            renderer,
                            &limits.loose(),
                        )
                    },
                    |child, size| child.align(align, iced::Alignment::Start, size),
                )
            }
        };
        node.apply(tree);
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
        let child_layout = Self::child_layout(layout, tree);
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

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            Self::child_layout(layout, tree),
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
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            Self::child_layout(layout, tree),
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
        let child_layout = Self::child_layout(layout, tree);
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            child_layout,
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
        let child_layout = Self::child_layout(layout, tree);
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

impl<'a, Message: 'a> From<LiveSeamPad<'a, Message>> for Element<'a, Message> {
    fn from(value: LiveSeamPad<'a, Message>) -> Self {
        Element::new(value)
    }
}

pub(crate) struct RetainedResponsiveWidth<'a, Message, Marker = Retained> {
    child: Element<'a, Message>,
    custody: Option<ContentTree>,
    marker: std::marker::PhantomData<Marker>,
}

#[derive(Debug, Default)]
pub(crate) struct Retained;

#[derive(Debug, Default)]
pub(crate) struct RetainedFloatingTitlebar;

#[derive(Debug, Default)]
pub(crate) struct RetainedFloatingTitlebarContent;

// Each role transfers its tree once when a shell rebuild changes ancestry.
#[derive(Clone)]
pub(crate) struct ContentTree(std::rc::Rc<std::cell::RefCell<Option<Tree>>>);

impl ContentTree {
    pub(crate) fn new(tree: Option<Tree>) -> Self {
        Self(std::rc::Rc::new(std::cell::RefCell::new(tree)))
    }

    pub(crate) fn is_consumed(&self) -> bool {
        self.0.borrow().is_none()
    }
}

impl<'a, Message> RetainedResponsiveWidth<'a, Message> {
    pub(crate) fn new(child: Element<'a, Message>) -> Self {
        Self {
            child,
            custody: None,
            marker: std::marker::PhantomData,
        }
    }

    pub(crate) fn floating_titlebar(
        child: Element<'a, Message>,
        custody: &ContentTree,
    ) -> RetainedResponsiveWidth<'a, Message, RetainedFloatingTitlebar> {
        RetainedResponsiveWidth {
            child,
            custody: Some(custody.clone()),
            marker: std::marker::PhantomData,
        }
    }
    pub(crate) fn floating_titlebar_content(
        child: Element<'a, Message>,
        custody: &ContentTree,
    ) -> RetainedResponsiveWidth<'a, Message, RetainedFloatingTitlebarContent> {
        RetainedResponsiveWidth {
            child,
            custody: Some(custody.clone()),
            marker: std::marker::PhantomData,
        }
    }
}

impl<'a, Message, Marker> RetainedResponsiveWidth<'a, Message, Marker> {
    pub(crate) fn retaining(mut self, custody: &ContentTree) -> Self {
        self.custody = Some(custody.clone());
        self
    }

    fn child_layout(layout: Layout, tree: &Tree) -> Layout {
        layout
            .children(&tree.children)
            .next()
            .expect("RetainedResponsiveWidth always owns one child layout")
    }
}

impl<Message, Marker: Default + std::fmt::Debug + 'static>
    Widget<Message, iced::Theme, iced::Renderer> for RetainedResponsiveWidth<'_, Message, Marker>
{
    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Marker>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(Marker::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        if let Some(retained) = self
            .custody
            .as_ref()
            .and_then(|custody| custody.0.borrow_mut().take())
        {
            *tree = retained;
        }
        tree.diff_children(std::slice::from_mut(&mut self.child));
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
            Self::child_layout(layout, tree),
            cursor,
            renderer,
            shell,
            viewport,
        );
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        if std::any::TypeId::of::<Marker>() != std::any::TypeId::of::<Retained>() {
            self.child
                .as_widget_mut()
                .layout(&mut tree.children[0], renderer, limits);
            iced::advanced::layout::pass_through(tree, 0);
            return;
        }
        crate::app_shell::build_probe::note_responsive_width_layout();
        let sizing = self.child.as_widget().size();
        let rule = layout::responsive::Rule::Content { sizing };
        let child = self.child.as_widget_mut().measure(
            &mut tree.children[0],
            renderer,
            &rule.child_limits(limits),
        );
        rule.finish(limits, child)
            .with_responsive_presentation(*limits, rule, sizing, None)
            .apply(tree);
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
        let child_layout = Self::child_layout(layout, tree);
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

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            Self::child_layout(layout, tree),
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
        let bounds = layout.bounds();
        let child_layout = Self::child_layout(layout, tree);
        let paint = |renderer: &mut iced::Renderer| {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                child_layout,
                cursor,
                viewport,
            );
        };
        // A child’s minimum width can exceed its live content allocation.
        if child_layout.bounds().width > bounds.width + f32::EPSILON {
            use iced::advanced::Renderer as _;
            renderer.with_layer(bounds, paint);
        } else {
            paint(renderer);
        }
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let child_layout = Self::child_layout(layout, tree);
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            child_layout,
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
        let child_layout = Self::child_layout(layout, tree);
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

impl<'a, Message: 'a, Marker: Default + std::fmt::Debug + 'static>
    From<RetainedResponsiveWidth<'a, Message, Marker>> for Element<'a, Message>
{
    fn from(value: RetainedResponsiveWidth<'a, Message, Marker>) -> Self {
        Element::new(value)
    }
}
