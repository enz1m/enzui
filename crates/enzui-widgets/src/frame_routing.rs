//! Cached frame-event forwarding for custom widget wrappers.
//!
//! Transparent wrappers share the child's tree; opaque wrappers own child slots.
//! Forwarders consult cached interest, so ordinary updates that can start motion
//! must refresh it too. Opaque wrappers use [`refresh_opaque_child`]; child-list
//! owners refresh children and then [`Tree::refresh_frame_children`].
//!
//! Callers supply the child's layout, cursor masking and viewport policy, and
//! handle their own animation separately. These helpers forward events without
//! enforcing inertness, clipping or overlay policy.

use iced::advanced::widget::Tree;
use iced::advanced::{Layout, Shell, mouse};
use iced::{Element, Event, Rectangle};

/// Query a child's current frame interest using its shared tree.
/// This calls the widget rather than reading cached interest.
#[inline]
pub fn interest_transparent<Message, Theme, Renderer>(
    child: &Element<'_, Message, Theme, Renderer>,
    tree: &Tree,
) -> bool
where
    Renderer: iced::advanced::Renderer,
{
    child.as_widget().frame_interest(tree)
}

/// Dispatch a frame to a shared-tree child when its cached interest is set.
/// Refreshes interest after dispatch. Ordinary updates must also refresh the
/// shared tree when they can change interest.
#[inline]
#[expect(
    clippy::too_many_arguments,
    reason = "transparent forwarding mirrors Widget::update_frame plus its child"
)]
pub fn forward_frame_transparent<Message, Theme, Renderer>(
    child: &mut Element<'_, Message, Theme, Renderer>,
    tree: &mut Tree,
    event: &Event,
    layout: Layout,
    cursor: mouse::Cursor,
    renderer: &Renderer,
    shell: &mut Shell<'_, Message>,
    viewport: &Rectangle,
) where
    Renderer: iced::advanced::Renderer,
{
    // An owner's animation can keep the wrapper routed after this child settles.
    if !tree.has_frame_interest() {
        return;
    }
    child
        .as_widget_mut()
        .update_frame(tree, event, layout, cursor, renderer, shell, viewport);
    tree.refresh_frame_interest(child.as_widget());
}

/// Read the first owned child's cached frame interest.
/// Returns false when the wrapper has no child; other child slots are ignored.
#[inline]
pub fn interest_opaque(tree: &Tree) -> bool {
    tree.children.first().is_some_and(Tree::has_frame_interest)
}

/// Refresh an owned child's interest and its membership in the parent's frame route.
/// Call after ordinary events that can change the child's interest. The parent's
/// own cached interest is not recomputed.
///
/// # Panics
///
/// Panics if `index` is outside `tree.children`.
#[inline]
pub fn refresh_opaque_child<Message, Theme, Renderer>(
    child: &Element<'_, Message, Theme, Renderer>,
    tree: &mut Tree,
    index: usize,
) where
    Renderer: iced::advanced::Renderer,
{
    tree.children[index].refresh_frame_interest(child.as_widget());
    tree.refresh_frame_child(index);
}

/// Dispatch a frame to an owned child when its cached interest is set.
/// Refreshes the child and its parent route membership after dispatch; the
/// parent's own cached interest is not recomputed.
///
/// # Panics
///
/// Panics if `index` is outside `tree.children`.
#[inline]
#[expect(
    clippy::too_many_arguments,
    reason = "opaque forwarding mirrors Widget::update_frame plus child ownership"
)]
pub fn forward_frame_opaque<Message, Theme, Renderer>(
    child: &mut Element<'_, Message, Theme, Renderer>,
    tree: &mut Tree,
    index: usize,
    event: &Event,
    layout: Layout,
    cursor: mouse::Cursor,
    renderer: &Renderer,
    shell: &mut Shell<'_, Message>,
    viewport: &Rectangle,
) where
    Renderer: iced::advanced::Renderer,
{
    if !tree.children[index].has_frame_interest() {
        return;
    }
    child.as_widget_mut().update_frame(
        &mut tree.children[index],
        event,
        layout,
        cursor,
        renderer,
        shell,
        viewport,
    );
    refresh_opaque_child(child, tree, index);
}

/// Dispatch a frame to each child listed in the parent's cached frame route.
/// Children and child trees must have the same slot order. Each visited child
/// refreshes its interest, and settled children leave the route. Ordinary event
/// dispatch must refresh child interest and rebuild the route too; this helper
/// does not recompute the parent's own cached interest.
///
/// # Panics
///
/// Panics if a routed index is outside `children` or `tree.children`.
#[inline]
#[expect(
    clippy::too_many_arguments,
    reason = "list forwarding mirrors Widget::update_frame plus child ownership"
)]
pub fn forward_frame_children<Message, Theme, Renderer>(
    children: &mut [Element<'_, Message, Theme, Renderer>],
    tree: &mut Tree,
    event: &Event,
    layout: Layout,
    cursor: mouse::Cursor,
    renderer: &Renderer,
    shell: &mut Shell<'_, Message>,
    viewport: &Rectangle,
) where
    Renderer: iced::advanced::Renderer,
{
    let Tree {
        children: child_trees,
        frame_children,
        ..
    } = tree;
    let mut route = 0;
    while route < frame_children.len() {
        let index = frame_children[route];
        let child = &mut children[index];
        let child_layout = layout.child(child_trees, index);
        let child_tree = &mut child_trees[index];
        child.as_widget_mut().update_frame(
            child_tree,
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        child_tree.refresh_frame_interest(child.as_widget());
        if child_tree.has_frame_interest() {
            route += 1;
        } else {
            let _ = frame_children.remove(route);
        }
    }
}
