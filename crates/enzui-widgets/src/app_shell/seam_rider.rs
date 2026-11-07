//! Chrome positioning from a shared seam clock for layout and retained presentation.

use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Rectangle, Size, Vector};

/// A clip no window reaches past: a panel replayed under it is bounded only by
/// its ancestors' clips.
const UNBOUNDED: Rectangle = Rectangle {
    x: -1.0e6,
    y: -1.0e6,
    width: 2.0e6,
    height: 2.0e6,
};

pub(crate) struct SeamRider<'a, Message> {
    child: Element<'a, Message>,
    // Pane and chrome share one motion generation, including mid-travel reversal.
    clock: crate::motion::RevealClock,
    laid_out_at: Box<dyn Fn() -> f32 + 'a>,
    offset_at: Box<dyn Fn(f32) -> f32 + 'a>,
    // Whether a layout-driven rider still publishes its travel: the render
    // lane then presents the travel's frames, each commit at the instant it
    // was laid out for, so the UI thread does not block in present. Only a
    // rider no ancestor already moves may publish one.
    extrapolates: bool,
}

#[derive(Debug)]
struct RiderState {
    node: iced::advanced::renderer::NodeId,
    laid_out_at: std::cell::Cell<Option<f32>>,
    // Layout borrows its node; projected input and operation geometry need an owned copy.
    child_node: std::cell::RefCell<Option<layout::Node>>,
    recorded_offset: std::cell::Cell<Option<f32>>,
}

impl Default for RiderState {
    fn default() -> Self {
        Self {
            node: crate::motion::next_scene_node(),
            laid_out_at: std::cell::Cell::new(None),
            child_node: std::cell::RefCell::new(None),
            recorded_offset: std::cell::Cell::new(None),
        }
    }
}

impl<'a, Message> SeamRider<'a, Message> {
    pub(crate) fn new(
        child: impl Into<Element<'a, Message>>,
        clock: crate::motion::RevealClock,
        laid_out_at: impl Fn() -> f32 + 'a,
        offset_at: impl Fn(f32) -> f32 + 'a,
    ) -> Self {
        Self {
            child: child.into(),
            clock,
            laid_out_at: Box::new(laid_out_at),
            offset_at: Box::new(offset_at),
            extrapolates: false,
        }
    }

    /// Keeps publishing the travel under a layout-driven clock: each frame's
    /// layout places the child, and the published curve, sampled at the
    /// instant that layout was made for, places its recording there too.
    pub(crate) fn extrapolating(mut self) -> Self {
        self.extrapolates = true;
        self
    }

    fn layout_driven(&self) -> bool {
        self.clock.responsive_reserve && !self.extrapolates
    }

    fn progress(&self) -> f32 {
        self.clock.progress.get().clamp(0.0, 1.0)
    }

    fn travelling(&self) -> bool {
        !self.layout_driven()
            && self
                .clock
                .travel
                .as_ref()
                .is_some_and(|travel| travel.borrow().is_some())
    }

    fn offset_for(&self, state: &RiderState, sampled: f32) -> f32 {
        if self.layout_driven() {
            return 0.0;
        }
        let Some(laid_out_at) = state.laid_out_at.get() else {
            return 0.0;
        };

        (self.offset_at)(sampled.clamp(0.0, 1.0)) - (self.offset_at)(laid_out_at)
    }

    // Drawing translates the renderer, so hover resolution needs the cursor in unshifted layout space.
    fn cursor_in_layout_space(cursor: mouse::Cursor, translation: Vector) -> mouse::Cursor {
        if translation.x.abs() <= f32::EPSILON {
            return cursor;
        }

        match cursor {
            mouse::Cursor::Available(point) => mouse::Cursor::Available(point - translation),
            mouse::Cursor::Levitating(point) => mouse::Cursor::Levitating(point - translation),
            mouse::Cursor::Obstructed(point) => mouse::Cursor::Obstructed(point - translation),
            mouse::Cursor::Unavailable => mouse::Cursor::Unavailable,
        }
    }

    fn child_layout(layout: Layout, tree: &Tree) -> Layout {
        layout
            .children(&tree.children)
            .next()
            .expect("SeamRider always owns one child layout")
    }

    fn presentation(&self, tree: &Tree) -> Option<(layout::Node, Vector)> {
        let state = tree.state.downcast_ref::<RiderState>();
        let offset = self.offset_for(state, self.progress());

        if offset.abs() <= f32::EPSILON {
            return None;
        }

        let node = state.child_node.borrow().clone()?;

        Some((node, Vector::new(offset, 0.0)))
    }

    /// Publishes this frame's track and, while travelling, the curve the
    /// render lane samples.
    ///
    /// An extrapolating rider publishes absolute offsets against the baseline
    /// its recording was drawn at, so every commit of one travel publishes the
    /// same curve and each recording, sampled at its own instant, sits where
    /// it was laid out.
    fn publish<M>(&self, tree: &Tree, shell: &mut Shell<'_, M>) {
        let state = tree.state.downcast_ref::<RiderState>();
        let Some(recorded) = state.recorded_offset.get() else {
            return;
        };
        if !self.travelling() {
            return;
        }
        let laid_out_at = state.laid_out_at.get();
        let node = state.node;
        let offset_at: &dyn Fn(f32) -> f32 = self.offset_at.as_ref();
        let track = |translation: f32| iced::advanced::renderer::Track {
            node,
            translation: Vector::new(translation, 0.0),
            opacity: 1.0,
            scale: 1.0,
            scale_origin: iced::Point::ORIGIN,
            rotation: 0.0,
            clip: None,
        };

        if self.extrapolates {
            shell.publish_track(track(offset_at(self.progress())));
            // A panel recorded whole inside this group is shown whole within
            // the window.
            if let Some(panel) = self
                .clock
                .retained_panel
                .as_ref()
                .and_then(|panel| panel.get())
            {
                shell.publish_track(iced::advanced::renderer::Track {
                    node: panel,
                    clip: Some(UNBOUNDED),
                    ..track(0.0)
                });
            }
        } else {
            let now = laid_out_at.map_or(0.0, |_| self.offset_for(state, self.progress()));
            // Tracks are relative to the already-offset recording.
            shell.publish_track(track(now - recorded));
        }

        let Some(curve) = self
            .clock
            .travel
            .as_ref()
            .and_then(|travel| (*travel.borrow()).clone())
        else {
            return;
        };
        let base = if self.extrapolates {
            0.0
        } else {
            laid_out_at.map_or(0.0, offset_at) + recorded
        };
        crate::motion::publish_travel(shell, node, &curve, |sampled| {
            track(offset_at(sampled.clamp(0.0, 1.0)) - base)
        });
    }

    fn presented<R>(
        presentation: Option<&(layout::Node, Vector)>,
        child: Layout,
        f: impl FnOnce(Layout) -> R,
    ) -> R {
        match presentation {
            Some((_, translation)) => f(child.move_to(child.position() + *translation)),
            None => f(child),
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for SeamRider<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<RiderState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(RiderState::default())
    }

    fn size(&self) -> Size<iced::Length> {
        self.child.as_widget().size()
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let fraction = (self.laid_out_at)().clamp(0.0, 1.0);
        tree.state
            .downcast_ref::<RiderState>()
            .laid_out_at
            .set(Some(fraction));

        // Only layout-driven riders transfer without serializing the offset callback.
        let node = if self.clock.responsive_reserve {
            layout::passthrough(limits, self.child.as_widget().size(), |limits| {
                self.child
                    .as_widget_mut()
                    .measure(&mut tree.children[0], renderer, limits)
            })
        } else {
            let child = self
                .child
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, limits);
            layout::Node::with_children(child.size(), vec![child])
        };
        *tree
            .state
            .downcast_ref::<RiderState>()
            .child_node
            .borrow_mut() = Some(node.children()[0].clone());
        node.apply(tree);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        // Interest is cached when the tree is rebuilt, which for a docked
        // travel happens while it is still settled; an extrapolating rider
        // stays on the frame route so the travel's first frames publish. Its
        // child is still pruned by its own interest.
        self.extrapolates || self.travelling() || crate::frame_routing::interest_opaque(tree)
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
        self.publish(tree, shell);

        let presentation = self.presentation(tree);

        Self::presented(
            presentation.as_ref(),
            Self::child_layout(layout, tree),
            |child_layout| {
                crate::frame_routing::forward_frame_opaque(
                    &mut self.child,
                    tree,
                    0,
                    event,
                    child_layout,
                    cursor,
                    renderer,
                    shell,
                    viewport,
                );
            },
        );
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
        // A redraw can reach this rider through ordinary updates rather than
        // the frame route; an extrapolating rider publishes from either.
        if self.extrapolates
            && matches!(
                event,
                Event::Window(iced::window::Event::RedrawRequested(_))
            )
        {
            self.publish(tree, shell);
        }

        let presentation = self.presentation(tree);

        Self::presented(
            presentation.as_ref(),
            Self::child_layout(layout, tree),
            |child_layout| {
                self.child.as_widget_mut().update(
                    &mut tree.children[0],
                    event,
                    child_layout,
                    cursor,
                    renderer,
                    shell,
                    viewport,
                );
            },
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
        let presentation = self.presentation(tree);

        Self::presented(
            presentation.as_ref(),
            Self::child_layout(layout, tree),
            |child_layout| {
                self.child.as_widget().mouse_interaction(
                    &tree.children[0],
                    child_layout,
                    cursor,
                    viewport,
                    renderer,
                )
            },
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
        let state = tree.state.downcast_ref::<RiderState>();
        let offset = self.offset_for(state, self.progress());
        let translation = Vector::new(offset, 0.0);
        let node = state.node;

        use iced::advanced::Renderer as _;
        // A clipping layer would suppress travel outside pinned geometry; recording alone marks the segment.
        if self.extrapolates {
            // The paint sits at this absolute offset along the travel.
            let baseline = state
                .laid_out_at
                .get()
                .map_or(0.0, |laid_out_at| (self.offset_at)(laid_out_at))
                + offset;
            renderer.mark_recording_at(node, Vector::new(baseline, 0.0));
        } else {
            renderer.mark_recording(node);
        }
        renderer.with_translation(translation, |renderer| {
            // The renderer applies translation; passing projected layout here would translate twice.
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                Self::child_layout(layout, tree),
                Self::cursor_in_layout_space(cursor, translation),
                viewport,
            );
        });
        renderer.mark_recording(iced::advanced::renderer::NodeId::STATIC);

        state.recorded_offset.set(Some(offset));
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let presentation = self.presentation(tree);

        Self::presented(
            presentation.as_ref(),
            Self::child_layout(layout, tree),
            |child_layout| {
                self.child.as_widget_mut().operate(
                    &mut tree.children[0],
                    child_layout,
                    viewport,
                    renderer,
                    operation,
                );
            },
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
        let presented = {
            let state = tree.state.downcast_ref::<RiderState>();

            Vector::new(self.offset_for(state, self.progress()), 0.0)
        };

        let child_layout = Self::child_layout(layout, tree);
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            child_layout,
            renderer,
            viewport,
            translation + presented,
            window,
        )
    }
}

impl<'a, Message: 'a> From<SeamRider<'a, Message>> for Element<'a, Message> {
    fn from(value: SeamRider<'a, Message>) -> Self {
        Element::new(value)
    }
}
