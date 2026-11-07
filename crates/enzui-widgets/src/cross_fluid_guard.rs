//! Prevents cross-fluid children from collapsing a compressed row or column.
//!
//! Apply [`hug_cross_fluid`] to each child before composing the flex line.
//! Static children retain their layout shape; fluid children receive a
//! `Length::Shrink` wrapper and use their intrinsic cross-axis size. A guarded
//! child therefore stops stretching to match a taller or wider sibling.
//!
//! [`Axis`] names the line's main axis: horizontal lines guard height, and
//! vertical lines guard width. [`action_bar`](crate::action_bar) and
//! [`flow`](crate::flow) use this guard when composing caller-provided children.

use iced::advanced::widget::Tree;
use iced::{Element, Length};

/// Main axis of the flex line whose children are being guarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// A `Row`. Children are guarded in height.
    Horizontal,
    /// A `Column`. Children are guarded in width.
    Vertical,
}

fn is_cross_fluid<Message>(el: &mut Element<'_, Message>, axis: Axis) -> bool {
    let mut tree = Tree::new(&*el);
    // Diff first: nested fluid children otherwise report `Length::Fit`.
    tree.diff(&mut *el);
    let size = el.as_widget().size();
    let cross = match axis {
        Axis::Horizontal => size.height,
        Axis::Vertical => size.width,
    };
    cross.fill_factor() != 0
}

/// Uses intrinsic cross-axis sizing for a fluid child of a compressed flex line.
///
/// Static children are returned without an extra layout node. Cross-fluid
/// children receive a `Length::Shrink` container and no longer stretch to the
/// cross-axis size of a sibling. Apply this to children before building the line.
///
/// ```
/// use enzui_widgets::cross_fluid_guard::{Axis, hug_cross_fluid};
/// use iced::{Element, Length};
/// use iced::widget::{container, text};
///
/// # #[derive(Debug, Clone)] enum Msg {}
/// let fluid: Element<'_, Msg> = container(text("x")).height(Length::Fill).into();
/// let guarded = hug_cross_fluid(fluid, Axis::Horizontal);
/// assert_eq!(guarded.as_widget().size().height.fill_factor(), 0);
///
/// let plain: Element<'_, Msg> = container(text("x")).into();
/// let same = hug_cross_fluid(plain, Axis::Horizontal);
/// assert_eq!(same.as_widget().size().height.fill_factor(), 0);
/// ```
#[must_use]
pub fn hug_cross_fluid<'a, Message: 'a>(
    mut el: Element<'a, Message>,
    axis: Axis,
) -> Element<'a, Message> {
    if !is_cross_fluid(&mut el, axis) {
        return el;
    }
    match axis {
        Axis::Horizontal => iced::widget::container(el).height(Length::Shrink).into(),
        Axis::Vertical => iced::widget::container(el).width(Length::Shrink).into(),
    }
}
