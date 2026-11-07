//! Two owned children reflowing between a row and a column without rebuilding.
//!
//! A positive finite allocated width below the breakpoint stacks the children;
//! equality, zero and non-finite probe widths select the row. Classification uses
//! this component’s width, so a narrow pane can stack inside a wide window.
//! Child tree slots remain stable across the transition.
//!
//! [`PairArm`] supplies padding, spacing, alignment and height for each arrangement.
//! Row-only `leading_width` reserves a fixed gutter, shares space with the trailing
//! child for proportional sizing, or hugs intrinsic content for `Shrink`/`Fit`.
//! The trailing child’s width policy affects proportional sharing. In a stack,
//! both children receive the inner width offer, and finite height is allocated
//! leading-first; the trailing child gets the remaining height after spacing.
//!
//! Fixed or intrinsic gutters can leave no trailing width. Bounded proportional
//! policies retain their floors and ceilings; an internal `Length::Fluid` leading
//! policy is unsupported. Debug builds assert on unsupported fluid policies or
//! proportional allocation that consumes all available trailing space.

use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Alignment, Element, Event, Length, Padding, Rectangle, Size, Vector};

/// Geometry for one of the two arrangements a [`ReflowPair`] can take.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PairArm {
    /// Padding around the pair.
    pub padding: Padding,
    /// Gap between the two children.
    pub spacing: f32,
    /// Cross-axis alignment of the two children.
    pub align: Alignment,
    /// Outer height. `Shrink` lets the pair hug its children, which is what the
    /// stacked arrangement wants; a fixed value pins a single-line row.
    pub height: Length,
    /// Leading-child width policy in the row arrangement; ignored when stacked.
    ///
    /// Fixed widths reserve a gutter capped at the inner box. Proportional
    /// widths share the space left after spacing with the trailing child's
    /// declared width, honoring bounds. `Shrink` and `Fit` measure intrinsically
    /// with fluid descendants compressed. Under unbounded width, proportional
    /// leading children also receive an intrinsic offer.
    pub leading_width: Length,
}

impl Default for PairArm {
    fn default() -> Self {
        Self {
            padding: Padding::ZERO,
            spacing: 0.0,
            align: Alignment::Start,
            height: Length::Shrink,
            leading_width: Length::Shrink,
        }
    }
}

/// Two children that reflow between a row and a column at a width breakpoint.
#[must_use]
pub struct ReflowPair<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    children: Vec<Element<'a, Message, Theme, Renderer>>,
    breakpoint: f32,
    row: PairArm,
    stacked: PairArm,
    width: Length,
}

/// Creates a [`ReflowPair`] from a leading and a trailing child.
///
/// Non-finite or non-positive breakpoints disable stacking. Outer width defaults
/// to [`Length::Fill`].
///
/// # Panics
///
/// During layout, debug builds panic for internal fluid leading widths or a
/// proportional row allocation leaving no trailing space in a finite positive offer.
pub fn reflow_pair<'a, Message, Theme, Renderer>(
    leading: impl Into<Element<'a, Message, Theme, Renderer>>,
    trailing: impl Into<Element<'a, Message, Theme, Renderer>>,
    breakpoint: f32,
    row: PairArm,
    stacked: PairArm,
) -> ReflowPair<'a, Message, Theme, Renderer> {
    ReflowPair {
        children: vec![leading.into(), trailing.into()],
        breakpoint: if breakpoint.is_finite() && breakpoint > 0.0 {
            breakpoint
        } else {
            0.0
        },
        row,
        stacked,
        width: Length::Fill,
    }
}

impl<'a, Message, Theme, Renderer> ReflowPair<'a, Message, Theme, Renderer> {
    /// Overrides the outer width policy (default [`Length::Fill`]).
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Whether `width` px puts this pair in the stacked arrangement.
    ///
    /// Equality, non-finite values and non-positive widths select the row.
    pub fn is_stacked_at(&self, width: f32) -> bool {
        width.is_finite() && width > 0.0 && width < self.breakpoint
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for ReflowPair<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn diff(&mut self, tree: &mut Tree) {
        // The fixed child order preserves retained state across arrangement changes.
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        // Report each arm’s measured height rather than imposing the row height on stacks.
        Size {
            width: self.width,
            height: Length::Shrink,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let arm = |arm: PairArm| layout::pair::Arm {
            padding: arm.padding,
            spacing: arm.spacing,
            align: arm.align,
            height: arm.height,
            leading_width: arm.leading_width,
        };
        let policy = layout::pair::Policy {
            width: self.width,
            breakpoint: self.breakpoint,
            row: arm(self.row),
            stacked: arm(self.stacked),
        };
        let children = std::array::from_fn(|slot| self.children[slot].as_widget().size());
        layout::pair::resolve(limits, policy, children, |slot, limits| {
            self.children[slot]
                .as_widget_mut()
                .measure(&mut tree.children[slot], renderer, limits)
        })
        .apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.container(None, layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            for (child, (lay, state)) in self
                .children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
            {
                child
                    .as_widget_mut()
                    .operate(state, lay, viewport, renderer, operation);
            }
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
        for (child, (lay, state)) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
        {
            child
                .as_widget_mut()
                .update(state, event, lay, cursor, renderer, shell, viewport);
        }
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
        for (child, (lay, state)) in self.children.iter().zip(layout.iter(&tree.children)) {
            child
                .as_widget()
                .draw(state, renderer, theme, style, lay, cursor, viewport);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.children
            .iter()
            .zip(layout.iter(&tree.children))
            .map(|(child, (lay, state))| {
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
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
        overlay::from_children(
            &mut self.children,
            tree,
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<ReflowPair<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn from(pair: ReflowPair<'a, Message, Theme, Renderer>) -> Self {
        Element::new(pair)
    }
}
