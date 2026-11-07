//! Grid-local leaf rectangles for sizing content before layout.
//!
//! Geometry uses the state's seam, pixel minimums and native rounding. Invalid
//! size components become zero; rectangles are clamped within the resulting
//! extent. A maximized pane receives that whole extent as the only entry.

use iced::{Rectangle, Size};

use super::EnzPaneGridState;

impl<T> EnzPaneGridState<T> {
    /// Predict leaf rectangles in depth-first order, first child before second.
    ///
    /// Supply the grid's allocated size after subtracting surrounding chrome.
    /// Coordinates are grid-local; add the grid offset for window coordinates.
    /// Seam subtraction, native rounding and pixel minimums make these differ
    /// from `ratio * extent`. Invalid size components become zero, and output
    /// is clamped within that extent, including when minimums cannot fit.
    /// A maximized pane is the only entry.
    #[must_use]
    pub fn pane_rects(&self, size: iced::Size) -> Vec<(&T, Rectangle)> {
        use enzui_core::metrics::FiniteNonNeg;
        let size = Size::new(
            FiniteNonNeg::new_or(size.width, 0.0).get(),
            FiniteNonNeg::new_or(size.height, 0.0).get(),
        );
        if let Some(maximized) = self.panes.maximized() {
            return self
                .panes
                .panes
                .get(&maximized)
                .map(|value| (value, Rectangle::with_size(size)))
                .into_iter()
                .collect();
        }
        let node = self.panes.layout();
        // Native allocation may exceed the extent when nested minimums cannot fit.
        let regions = node.pane_regions(self.tokens.seam_thickness, self.tokens.min_pane_px, size);
        let mut order = Vec::with_capacity(self.panes.len());
        collect_pane_order(node, &mut order);
        order
            .into_iter()
            .filter_map(|pane| {
                Some((
                    self.panes.panes.get(&pane)?,
                    clamp_to_bounds(*regions.get(&pane)?, size),
                ))
            })
            .collect()
    }
    /// Find the first laid-out leaf equal to `value` in [`Self::pane_rects`] order.
    ///
    /// Returns `None` for an unknown value or a leaf hidden by maximization.
    /// Size and coordinate rules follow [`Self::pane_rects`].
    ///
    /// ```
    /// use enzui_widgets::{EnzPaneGridState, PaneSplitOrientation, PaneTree};
    ///
    /// let state = EnzPaneGridState::new(PaneTree::split(
    ///     PaneSplitOrientation::Vertical,
    ///     0.5,
    ///     PaneTree::pane("left"),
    ///     PaneTree::pane("right"),
    /// ));
    /// let left = state.pane_rect(iced::Size::new(801.0, 600.0), &"left").unwrap();
    /// assert_eq!(left.x, 0.0);
    /// assert!(state.pane_rect(iced::Size::new(801.0, 600.0), &"missing").is_none());
    /// ```
    #[must_use]
    pub fn pane_rect(&self, size: iced::Size, value: &T) -> Option<Rectangle>
    where
        T: PartialEq,
    {
        self.pane_rects(size)
            .into_iter()
            .find_map(|(candidate, rect)| (candidate == value).then_some(rect))
    }
}
fn clamp_to_bounds(rect: Rectangle, size: Size) -> Rectangle {
    use enzui_core::metrics::FiniteNonNeg;
    let x = FiniteNonNeg::new_or(rect.x, 0.0).get().min(size.width);
    let y = FiniteNonNeg::new_or(rect.y, 0.0).get().min(size.height);
    let width = FiniteNonNeg::new_or(rect.width, 0.0)
        .get()
        .min(size.width - x);
    let height = FiniteNonNeg::new_or(rect.height, 0.0)
        .get()
        .min(size.height - y);
    Rectangle {
        x,
        y,
        width,
        height,
    }
}
fn collect_pane_order(
    node: &iced::widget::pane_grid::Node,
    out: &mut Vec<iced::widget::pane_grid::Pane>,
) {
    match node {
        iced::widget::pane_grid::Node::Split { a, b, .. } => {
            collect_pane_order(a, out);
            collect_pane_order(b, out);
        }
        iced::widget::pane_grid::Node::Pane(pane) => out.push(*pane),
    }
}
