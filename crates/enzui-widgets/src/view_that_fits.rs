//! Render the first authored candidate whose intrinsic width fits the assigned width.
//!
//! Candidates are tried in order with a half-pixel fit tolerance. Height is not
//! part of the decision. The last candidate renders unconditionally without a fit
//! measurement; an empty list renders nothing. Put preferred wide layouts first
//! and a gracefully degrading fallback last.
//!
//! Measurement uses unbounded limits with shrink compression, so fluid-width
//! candidates report content width. Candidate factories must be repeatable and
//! free of side effects: a winner can be built for measurement and again for
//! rendering. Repeated layouts at the same width within one widget instance reuse
//! the selected content; a new instance or changed width repeats selection.
//!
//! Only the winner enters the returned widget tree. Losing candidates have no
//! input, focus or accessibility presence. [`EnzFitProbe`] reads the chosen index
//! through an operation; assign a widget ID to target one of several instances.
//! [`crate::page_header`] uses this primitive for adaptive trailing actions.

use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Id, Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};
use std::any::Any;

/// Report the selected candidate through `Operation::custom`.
///
/// The report contains no losing candidate elements or trees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnzFitReport {
    /// The index (in authored order) of the candidate that rendered. `None`
    /// only when the candidate list was empty and nothing rendered.
    pub chosen: Option<usize>,
    /// The number of candidates the widget was built with, so a reader can
    /// distinguish "the fallback (last) won" from "a middle candidate won".
    pub candidate_count: usize,
}

/// Read the first matching [`ViewThatFits`] report from a laid-out widget tree.
///
/// Use [`EnzFitProbe::for_id`] to target a particular instance. Running the operation
/// emits no messages and leaves widget state unchanged.
#[derive(Debug, Default)]
pub struct EnzFitProbe {
    want_id: Option<Id>,
    report: Option<EnzFitReport>,
}

impl EnzFitProbe {
    /// A probe that captures the first view-that-fits it visits.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A probe that captures only the view-that-fits carrying `id`.
    #[must_use]
    pub fn for_id(id: impl Into<Id>) -> Self {
        Self {
            want_id: Some(id.into()),
            report: None,
        }
    }

    /// The captured report, or `None` if no matching widget was visited.
    #[must_use]
    pub fn report(&self) -> Option<EnzFitReport> {
        self.report
    }

    /// The captured chosen index, flattened: `None` if no widget was
    /// visited, or the visited widget rendered nothing (empty candidate
    /// list).
    #[must_use]
    pub fn chosen(&self) -> Option<usize> {
        self.report.and_then(|r| r.chosen)
    }
}

impl<T> Operation<T> for EnzFitProbe {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
        operate(self);
    }

    fn custom(&mut self, id: Option<&Id>, _bounds: Rectangle, state: &mut dyn Any) {
        if self.report.is_some() {
            return;
        }
        if let Some(report) = state.downcast_ref::<EnzFitReport>()
            && (self.want_id.is_none() || self.want_id.as_ref() == id)
        {
            self.report = Some(*report);
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct FitState {
    chosen: Option<usize>,
    candidate_count: usize,
    last_built_width: Option<u32>,
}

/// Build a candidate for measurement or rendering.
///
/// Factories must be side-effect-free and repeatable; a winning candidate may be
/// invoked twice when selection is evaluated.
pub type CandidateFn<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> =
    Box<dyn Fn() -> Element<'a, Message, Theme, Renderer> + 'a>;

/// Create a [`ViewThatFits`] from an ordered list of candidate builders.
///
/// The candidates are tried in authored order; the first whose measured
/// intrinsic width fits the parent-assigned width is rendered, and the
/// last is the unconditional fallback. An empty list renders nothing.
pub fn enz_view_that_fits<'a, Message, Theme, Renderer>(
    candidates: Vec<CandidateFn<'a, Message, Theme, Renderer>>,
) -> ViewThatFits<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    ViewThatFits {
        candidates,
        width: Length::Fill,
        height: Length::Shrink,
        id: None,
        content: Element::new(iced::widget::Space::new()),
        built_this_cycle: false,
    }
}

/// The widget behind [`enz_view_that_fits`]. See the module docs.
pub struct ViewThatFits<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    candidates: Vec<CandidateFn<'a, Message, Theme, Renderer>>,
    width: Length,
    height: Length,
    id: Option<Id>,
    content: Element<'a, Message, Theme, Renderer>,
    built_this_cycle: bool,
}

impl<'a, Message, Theme, Renderer> ViewThatFits<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    /// Override the outer width (default [`Length::Fill`] — the widget fills the
    /// width its parent assigns, and measures candidates against that width).
    #[must_use]
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Override the outer height (default [`Length::Shrink`] — intrinsic).
    #[must_use]
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Attach a stable [`Id`], so an [`EnzFitProbe::for_id`] can read back
    /// which candidate this widget chose when several are present.
    #[must_use]
    pub fn id(mut self, id: impl Into<Id>) -> Self {
        self.id = Some(id.into());
        self
    }

    fn choose(&self, renderer: &Renderer, max_width: f32) -> Option<usize> {
        if self.candidates.is_empty() {
            return None;
        }
        let last = self.candidates.len() - 1;
        for (i, build) in self.candidates.iter().enumerate() {
            if i == last {
                return Some(last);
            }
            if self.candidate_fits(build, renderer, max_width) {
                return Some(i);
            }
        }
        Some(last)
    }

    fn candidate_fits(
        &self,
        build: &CandidateFn<'a, Message, Theme, Renderer>,
        renderer: &Renderer,
        max_width: f32,
    ) -> bool {
        let mut probe = build();
        let mut tree = Tree::new(&probe);
        // Populate child trees before measuring flex descendants.
        tree.diff(&mut probe);
        // Bounded measurement could clamp a wide candidate and falsely report a fit.
        let limits = layout::Limits::new(Size::ZERO, Size::INFINITE)
            .width(Length::Shrink)
            .height(Length::Shrink);
        let node = probe.as_widget_mut().measure(&mut tree, renderer, &limits);
        let measured = node.size().width;
        measured <= max_width + 0.5
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for ViewThatFits<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<FitState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(FitState::default())
    }

    fn diff(&mut self, _tree: &mut Tree) {
        // Reconcile after selection; diffing the placeholder would discard retained focus.
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let limits = limits.width(self.width).height(self.height);
        let max_width = limits.max().width;

        let width_bits = max_width.to_bits();
        let state = tree.state.downcast_ref::<FitState>();
        if self.built_this_cycle && state.last_built_width == Some(width_bits) {
            let node = self.content.as_widget_mut().measure(
                &mut tree.children[0],
                renderer,
                &limits.loose(),
            );
            let size = limits.resolve(self.width, self.height, node.size());
            return layout::Node::with_children(size, vec![node]).apply(tree);
        }

        let chosen = self.choose(renderer, max_width);
        {
            let state = tree.state.downcast_mut::<FitState>();
            state.chosen = chosen;
            state.candidate_count = self.candidates.len();
            state.last_built_width = Some(width_bits);
        }
        self.built_this_cycle = true;

        self.content = match chosen {
            Some(i) => (self.candidates[i])(),
            None => Element::new(iced::widget::Space::new()),
        };
        tree.diff_children(std::slice::from_mut(&mut self.content));

        let node =
            self.content
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, &limits.loose());
        let size = limits.resolve(self.width, self.height, node.size());
        layout::Node::with_children(size, vec![node]).apply(tree);
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
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("the layout has its one child");
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.content, tree, 0);
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
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_opaque(
            &mut self.content,
            tree,
            0,
            event,
            layout
                .children(&tree.children)
                .next()
                .expect("the layout has its one child"),
            cursor,
            renderer,
            shell,
            viewport,
        );
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
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout
                .children(&tree.children)
                .next()
                .expect("the layout has its one child"),
            cursor,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout
                .children(&tree.children)
                .next()
                .expect("the layout has its one child"),
            cursor,
            viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        {
            let state = *tree.state.downcast_ref::<FitState>();
            let mut report = EnzFitReport {
                chosen: state.chosen,
                candidate_count: state.candidate_count,
            };
            operation.custom(self.id.as_ref(), layout.bounds(), &mut report);
        }
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("the layout has its one child");
        self.content.as_widget_mut().operate(
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
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("the layout has its one child");
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            child_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message, Theme, Renderer> ViewThatFits<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    /// Finish the builder and hand back the [`Element`], which is step 3 of
    /// the build pattern every widget in this crate follows (`docs/widgets.md`).
    ///
    /// This forwards to the `From<Self> for Element` impl below, so the two
    /// spellings are one implementation and cannot drift apart. Prefer this
    /// one inside `row![]` / `column![]`, where `.into()` has no target type
    /// to infer.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message, Theme, Renderer> {
        Element::from(self)
    }
}

impl<'a, Message, Theme, Renderer> From<ViewThatFits<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn from(w: ViewThatFits<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}
