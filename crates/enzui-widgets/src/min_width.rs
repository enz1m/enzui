//! Raise a child's offered minimum width while preserving its width policy.
//!
//! Floors use logical pixels; non-finite or non-positive values disable the
//! floor. The default clamps it to the parent's maximum. [`MinWidth::overflowing`]
//! permits wider layout by raising that maximum too. Children still control their
//! sizing and must respect the offered limits; fixed sizing can defeat a floor.
//! Painting, input, focus, semantics and overlays are forwarded without clipping.
//!
//! Width intent is forwarded with a lower bound, so fill children compete for
//! space with siblings. Layout preserves height and compression flags.
//!
//! A measured width more than 0.5 below its positive floor is recorded and printed
//! once per approximately distinct pair on the current thread, in every build
//! profile. This includes an overflowing child that ignores its floor. The log
//! does not bound its size or establish a painted or clickable minimum;
//! [`clamped_floors`] and [`reset_clamped_floors`] expose its diagnostic records.

use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};
// Per-thread records prevent a concurrent reset from erasing another layout's evidence.
mod clamped_floor_log {
    use std::cell::RefCell;

    thread_local! {
        pub(super) static CLAMPED_FLOORS: RefCell<Vec<(f32, f32)>> =
            const { RefCell::new(Vec::new()) };
    }
}
const CLAMP_REPORT_EPSILON: f32 = 0.5;

/// Clone this thread's recorded `(requested_floor, measured_width)` pairs.
/// Pairs appear in first-seen order, deduplicated when both values differ by
/// less than 0.01. Only widths more than 0.5 below a positive floor are recorded.
/// Records persist until [`reset_clamped_floors`].
#[must_use]
pub fn clamped_floors() -> Vec<(f32, f32)> {
    clamped_floor_log::CLAMPED_FLOORS.with(|v| v.borrow().clone())
}

/// Clear this thread's clamp records, allowing those pairs to be reported again.
/// Other threads' records are unaffected.
pub fn reset_clamped_floors() {
    clamped_floor_log::CLAMPED_FLOORS.with(|v| v.borrow_mut().clear());
}
// Diagnostics must tolerate thread teardown and a reentrant borrow without affecting layout.
fn report_clamped_floor(floor: f32, granted: f32) {
    let first_time = clamped_floor_log::CLAMPED_FLOORS
        .try_with(|seen| {
            let Ok(mut seen) = seen.try_borrow_mut() else {
                return false;
            };
            if seen
                .iter()
                .any(|(f, g)| (f - floor).abs() < 0.01 && (g - granted).abs() < 0.01)
            {
                return false;
            }
            seen.push((floor, granted));
            true
        })
        .unwrap_or(false);
    if !first_time {
        return;
    }
    log::warn!(
        target: "enzui::layout",
        "enzui layout: child width {granted} px falls below the requested \
         MinWidth floor of {floor} px. Parent limits or child sizing may prevent \
         the floor. Give the layout more room, or use `.overflowing(true)` \
         with a child that respects its offered minimum."
    );
}

/// A transparent wrapper that raises the offered minimum width of its child.
#[must_use]
pub struct MinWidth<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    floor: f32,
    overflowing: bool,
}

impl<'a, Message, Theme, Renderer> MinWidth<'a, Message, Theme, Renderer> {
    /// Allow the requested floor to raise the parent's offered maximum width.
    /// Defaults to false. Callers must provide room or scrolling for overflow; this
    /// permits wider child layout without forcing the child to accept the floor.
    pub fn overflowing(mut self, overflowing: bool) -> Self {
        self.overflowing = overflowing;
        self
    }
}

/// Apply a logical-pixel width floor, clamped to the parent maximum by default.
/// Non-finite or non-positive floors become zero. The child keeps its width
/// policy; use [`MinWidth::overflowing`] to permit overflow.
pub fn min_width<'a, Message, Theme, Renderer>(
    child: impl Into<Element<'a, Message, Theme, Renderer>>,
    floor: f32,
) -> MinWidth<'a, Message, Theme, Renderer> {
    let floor = if floor.is_finite() && floor > 0.0 {
        floor
    } else {
        0.0
    };
    MinWidth {
        child: child.into(),
        floor,
        overflowing: false,
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for MinWidth<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        // Preserve fill participation when the parent allocates space among siblings.
        let child_size = self.child.as_widget().size();
        let width = if self.floor > 0.0 {
            child_size.width.min(self.floor)
        } else {
            child_size.width
        };
        Size {
            width,
            height: child_size.height,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let max = limits.max();
        let min = limits.min();
        let (new_min_w, new_max_w) = if self.overflowing {
            let m = self.floor.max(min.width);
            (m, max.width.max(m))
        } else {
            (self.floor.min(max.width).max(min.width), max.width)
        };
        let floored = layout::Limits::with_compression(
            Size::new(new_min_w, min.height),
            Size::new(new_max_w, max.height),
            limits.compression(),
        );
        let node = self.child.as_widget_mut().measure(tree, renderer, &floored);
        // Inspect actual child geometry: a child can ignore the raised minimum.
        if self.floor > 0.0 && node.size().width < self.floor - CLAMP_REPORT_EPSILON {
            report_clamped_floor(self.floor, node.size().width);
        }
        node.apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_transparent(&self.child, tree)
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
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
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
        crate::frame_routing::forward_frame_transparent(
            &mut self.child,
            tree,
            event,
            layout,
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
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
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
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message, Theme, Renderer> From<MinWidth<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + iced::advanced::Renderer,
{
    fn from(w: MinWidth<'a, Message, Theme, Renderer>) -> Self {
        Element::new(w)
    }
}
