//! Separator seam geometry: the pointer hit region, the compact
//! resize-handle indicator paint, and the reading-order separator
//! collection that aligns focus order with announced bounds.

use super::*;

/// Length of the compact resize handle along the seam.
pub const HANDLE_INDICATOR_LENGTH: f32 = 24.0;
/// Thickness of the compact resize handle across the seam.
pub const HANDLE_INDICATOR_THICKNESS: f32 = 2.0;
pub(super) fn hit_bounds(seam: Rectangle, orientation: Orientation, slop: f32) -> Rectangle {
    let slop = slop.max(0.0);
    match orientation {
        Orientation::Vertical => Rectangle {
            x: seam.x - slop,
            y: seam.y,
            width: seam.width + 2.0 * slop,
            height: seam.height,
        },
        Orientation::Horizontal => Rectangle {
            x: seam.x,
            y: seam.y - slop,
            width: seam.width,
            height: seam.height + 2.0 * slop,
        },
    }
}

pub(super) fn paint_handle_indicator<Renderer>(
    renderer: &mut Renderer,
    seam: Rectangle,
    orientation: Orientation,
    ink: Color,
) where
    Renderer: iced::advanced::Renderer,
{
    if seam.width <= 0.0 || seam.height <= 0.0 {
        return;
    }
    let thickness = HANDLE_INDICATOR_THICKNESS;
    let bounds = match orientation {
        Orientation::Vertical => {
            let length = HANDLE_INDICATOR_LENGTH.min(seam.height);
            Rectangle {
                x: seam.center_x().round() - thickness / 2.0,
                y: seam.center_y() - length / 2.0,
                width: thickness,
                height: length,
            }
        }
        Orientation::Horizontal => {
            let length = HANDLE_INDICATOR_LENGTH.min(seam.width);
            Rectangle {
                x: seam.center_x() - length / 2.0,
                y: seam.center_y().round() - thickness / 2.0,
                width: length,
                height: thickness,
            }
        }
    };
    renderer.fill_quad(
        iced::advanced::renderer::Quad {
            bounds,
            border: iced::Border {
                radius: (thickness / 2.0).into(),
                ..iced::Border::default()
            },
            ..iced::advanced::renderer::Quad::default()
        },
        iced::Background::Color(ink),
    );
}

impl SeparatorModel {
    /// Compute thin seam rectangles translated by `offset`, in this model's order.
    ///
    /// The supplied size is the grid extent. Native spacing, minimums and rounding
    /// determine seam geometry; these bounds exclude pointer hit slop.
    /// Model and node must describe the same tree: pairing uses reading-order
    /// positions, including for detached separators. Excess model slots are `None`.
    #[must_use]
    pub fn separator_bounds(
        &self,
        node: &iced::widget::pane_grid::Node,
        offset: iced::Point,
        size: iced::Size,
    ) -> Vec<Option<Rectangle>> {
        let spacing = self.tokens.seam_thickness;
        let regions = node.split_regions(spacing, self.tokens.min_pane_px, size);
        let mut seams: Vec<Rectangle> = Vec::new();
        collect_seam_rects(node, &regions, spacing, offset, &mut seams);
        self.separators
            .iter()
            .enumerate()
            .map(|(i, _)| seams.get(i).copied())
            .collect()
    }
}
// Keep the same depth-first order as seam geometry and cached extents.
pub(super) fn collect_splits(node: &iced::widget::pane_grid::Node, out: &mut Vec<Separator>) {
    if let iced::widget::pane_grid::Node::Split {
        id,
        axis,
        ratio,
        a,
        b,
    } = node
    {
        out.push(Separator {
            orientation: match axis {
                iced::widget::pane_grid::Axis::Vertical => Orientation::Vertical,
                iced::widget::pane_grid::Axis::Horizontal => Orientation::Horizontal,
            },
            ratio: *ratio,
            split: Some(*id),
        });
        collect_splits(a, out);
        collect_splits(b, out);
    }
}
pub(super) fn collect_split_ids(
    node: &iced::widget::pane_grid::Node,
    out: &mut Vec<iced::widget::pane_grid::Split>,
) {
    if let iced::widget::pane_grid::Node::Split { id, a, b, .. } = node {
        out.push(*id);
        collect_split_ids(a, out);
        collect_split_ids(b, out);
    }
}
