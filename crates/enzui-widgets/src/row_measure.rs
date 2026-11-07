//! Report intrinsic row heights for caller-owned virtual-list geometry.
//!
//! [`enz_row_measure`] adds an unbounded-height measuring pass before the
//! child's normal layout. It forwards drawing, events, operations and overlays
//! to that child. Each layout performs the extra pass even without a callback.
//!
//! Place the wrapper directly inside the fixed-height slot, around its content.
//! Measuring the slot itself would reproduce the estimate; intermediate
//! shrinking containers can also discard the slot's finite height offer.
//!
//! [`EnzRowMeasure::on_measure`] publishes on `RedrawRequested` when intrinsic
//! height differs from both the assigned slot and the last published height
//! by more than [`EnzRowMeasure::epsilon`] (default 0.5 logical pixels). Dropped
//! reports are not retried while the intrinsic height stays unchanged. Feed
//! [`RowMeasurement`] values into [`crate::virtual_scroll::reconcile_measurements`]
//! and apply the updated table and corrected scroll offset in the caller.
//! [`crate::chat_list::EnzChatList::on_measure`] installs the wrapper for chat rows.
//!
//! [`crate::text_measure`] can estimate shaped text before layout; this wrapper
//! measures complete row content, including padding and captions. Headless
//! callers must deliver a redraw event through `UserInterface::update` to
//! receive reports; a layout or operation alone does not publish them.

use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector, window};

/// Default report tolerance in logical pixels, including equality at the limit.
pub const DEFAULT_EPSILON: f32 = 0.5;

/// One realised row's measured height, ready for
/// [`VariableHeightModel::apply_measurements`](enzui_core::virtual_sequence::VariableHeightModel::apply_measurements).
///
/// `From<RowMeasurement> for (usize, f32)` exists so a batch can be fed
/// straight in without a manual destructure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowMeasurement {
    /// The row's index in the logical sequence.
    pub index: usize,
    /// The height the row's content laid out to with no imposed slot, in
    /// logical pixels.
    pub height: f32,
}

impl From<RowMeasurement> for (usize, f32) {
    fn from(m: RowMeasurement) -> Self {
        (m.index, m.height)
    }
}

#[derive(Debug, Default)]
struct MeasureState {
    intrinsic: f32,
    slot: f32,
    published: Option<f32>,
}

/// A transparent wrapper that reports its child's intrinsic height.
///
/// See the module docs. Layout, drawing, events, semantics and overlays all
/// pass straight through to the child; only the extra measuring pass and the
/// published message are added.
pub struct EnzRowMeasure<'a, Message> {
    index: usize,
    child: Element<'a, Message>,
    on_measure: Option<Box<dyn Fn(RowMeasurement) -> Message + 'a>>,
    epsilon: f32,
}

/// Wrap `child` so its intrinsic height can be reported for row `index`.
pub fn enz_row_measure<'a, Message>(
    index: usize,
    child: impl Into<Element<'a, Message>>,
) -> EnzRowMeasure<'a, Message> {
    EnzRowMeasure {
        index,
        child: child.into(),
        on_measure: None,
        epsilon: DEFAULT_EPSILON,
    }
}

impl<'a, Message> EnzRowMeasure<'a, Message> {
    /// Publish intrinsic height on redraw when it differs from both the slot
    /// and the last published height by more than [`Self::epsilon`].
    ///
    /// The slot is the finite height offered by the parent, not the child's
    /// shrunken layout height. Place this wrapper directly inside the slot;
    /// intermediate shrinking containers can discard that offer.
    #[must_use]
    pub fn on_measure(mut self, f: impl Fn(RowMeasurement) -> Message + 'a) -> Self {
        self.on_measure = Some(Box::new(f));
        self
    }

    /// Tolerance at or below which differences are suppressed. Nonfinite and
    /// negative values fall back to [`DEFAULT_EPSILON`]; `0.0` is expressible
    /// intent (report every difference).
    #[must_use]
    pub fn epsilon(mut self, epsilon: f32) -> Self {
        self.epsilon = if epsilon.is_finite() && epsilon >= 0.0 {
            epsilon
        } else {
            DEFAULT_EPSILON
        };
        self
    }

    /// Finish the builder.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message>
    where
        Message: 'a,
    {
        Element::new(self)
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for EnzRowMeasure<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<MeasureState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(MeasureState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        // Height compression makes Fill children hug content under an unbounded offer.
        let measure_limits = layout::Limits::with_compression(
            Size::new(limits.min().width, 0.0),
            Size::new(limits.max().width, f32::INFINITY),
            Size::new(limits.compression().width, true),
        );
        let probe =
            self.child
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, &measure_limits);
        let intrinsic = probe.size().height;

        let node = self
            .child
            .as_widget_mut()
            .measure(&mut tree.children[0], renderer, limits);

        let state = tree.state.downcast_mut::<MeasureState>();
        // A shrinking child can be shorter than its fixed parent slot even before correction.
        state.slot = if limits.max().height.is_finite() {
            limits.max().height
        } else {
            node.size().height
        };
        state.intrinsic = if intrinsic.is_finite() && intrinsic >= 0.0 {
            intrinsic
        } else {
            0.0
        };

        layout::Node::new(node.size()).apply(tree);
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
        if let (Event::Window(window::Event::RedrawRequested(_)), Some(on_measure)) =
            (event, self.on_measure.as_ref())
        {
            let index = self.index;
            let epsilon = self.epsilon;
            let state = tree.state.downcast_mut::<MeasureState>();
            let intrinsic = state.intrinsic;

            let disagrees_with_slot = (intrinsic - state.slot).abs() > epsilon;
            let is_news = state
                .published
                .is_none_or(|p| (intrinsic - p).abs() > epsilon);

            if disagrees_with_slot && is_news {
                state.published = Some(intrinsic);
                shell.publish(on_measure(RowMeasurement {
                    index,
                    height: intrinsic,
                }));
            }
        }

        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
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
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
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
            layout,
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
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
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
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message> From<EnzRowMeasure<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(w: EnzRowMeasure<'a, Message>) -> Self {
        Element::new(w)
    }
}
