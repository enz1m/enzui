//! Horizontal or vertical flex collections that retain compatible child state by key.
//!
//! Stable logical keys preserve state across insertions, removals and reorder.
//! Reusing a key with a different widget tag resets that child. Widget IDs name
//! controls for routing; they do not replace reconciliation keys.
//!
//! [`keyed_row`] uses horizontal flex layout and [`keyed_column`] uses vertical
//! flex layout, with `Fit` dimensions, zero padding and spacing, start alignment,
//! and clipping disabled by default.
//!
//! Duplicate keys panic during diff in debug builds unless
//! [`KeyedRow::allow_duplicate_keys`] is enabled. In release, duplicates pair
//! by occurrence order. Use [`enzui_core::identity::KeyedIds`] to give
//! repeated entries distinct logical keys.
//!
//! Related: [`crate::keyed_diff`], [`crate::grid::KeyedGrid`].

use iced::advanced::layout;
use iced::advanced::layout::Measure as _;
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::Operation;
use iced::advanced::widget::tree::{self, Tree};
use iced::advanced::{Layout, Shell, Widget};
use iced::{Alignment, Element, Event, Length, Padding, Rectangle, Size, Vector};

/// A flex collection whose children retain compatible widget state by logical key.
/// Use [`keyed_row`] or [`keyed_column`] to choose the layout axis.
#[must_use]
pub struct KeyedRow<'a, Message> {
    keys: Vec<String>,
    children: Vec<Element<'a, Message>>,
    vertical: bool,
    spacing: f32,
    padding: Padding,
    align_y: Alignment,
    width: Length,
    height: Length,
    allow_duplicate_keys: bool,
    clip: bool,
    equal_cells: bool,
    equal_cells_below: Option<f32>,
    balance_shrink: bool,
    mirrored: bool,
}

struct KeyedRowState {
    keys: Vec<String>,
}

/// Build a horizontal collection in the supplied key/child order.
/// Use stable logical keys; dimensions default to [`Length::Fit`].
pub fn keyed_row<'a, Message>(
    keyed_children: Vec<(String, Element<'a, Message>)>,
) -> KeyedRow<'a, Message>
where
    Message: 'a,
{
    let (keys, children): (Vec<String>, Vec<Element<'a, Message>>) =
        keyed_children.into_iter().unzip();
    KeyedRow {
        keys,
        children,
        vertical: false,
        spacing: 0.0,
        padding: Padding::ZERO,
        align_y: Alignment::Start,
        // Fit lets Fill children expand; Shrink would compress them to intrinsic widths.
        width: Length::Fit,
        height: Length::Fit,
        allow_duplicate_keys: false,
        clip: false,
        equal_cells: false,
        equal_cells_below: None,
        balance_shrink: false,
        mirrored: false,
    }
}

/// Build a vertical collection in the supplied key/child order.
/// Defaults and reconciliation match [`keyed_row`], with the main axis reversed.
pub fn keyed_column<'a, Message>(
    keyed_children: Vec<(String, Element<'a, Message>)>,
) -> KeyedRow<'a, Message>
where
    Message: 'a,
{
    let mut c = keyed_row(keyed_children);
    c.vertical = true;
    c
}

impl<'a, Message> KeyedRow<'a, Message>
where
    Message: 'a,
{
    /// Set the main-axis gap in logical pixels; defaults to zero.
    /// Negative and non-finite values become zero.
    pub fn spacing(mut self, px: f32) -> Self {
        self.spacing = if px.is_finite() && px >= 0.0 { px } else { 0.0 };
        self
    }

    /// Set padding around the collection; defaults to zero.
    pub fn padding(mut self, padding: impl Into<Padding>) -> Self {
        self.padding = padding.into();
        self
    }

    /// Set cross-axis alignment; defaults to [`Alignment::Start`].
    /// This aligns vertically for rows and horizontally for columns.
    pub fn align_y(mut self, align: impl Into<Alignment>) -> Self {
        self.align_y = align.into();
        self
    }

    /// Lay a row out from right to left, for a right-to-left locale. Keys,
    /// focus and accessibility order stay logical; a column ignores it.
    pub fn mirrored(mut self, mirrored: bool) -> Self {
        self.mirrored = mirrored;
        self
    }

    /// Axis-neutral alias for [`Self::align_y`].
    pub fn align(self, align: impl Into<Alignment>) -> Self {
        self.align_y(align)
    }

    /// Whether this collection was built with [`keyed_column`].
    pub fn is_vertical(&self) -> bool {
        self.vertical
    }

    /// Narrow child draw viewports to the collection bounds; defaults to `false`.
    /// This does not install a renderer clip layer: children must honor the supplied
    /// viewport. Whole-collection and child viewport culling apply either way.
    pub fn clip(mut self, clip: bool) -> Self {
        self.clip = clip;
        self
    }

    pub(crate) fn equal_cells(mut self, equal: bool) -> Self {
        self.equal_cells = equal;
        self
    }

    pub(crate) fn equal_cells_below(mut self, width: Option<f32>) -> Self {
        self.equal_cells_below = width;
        self
    }

    pub(crate) fn balance_shrink(mut self) -> Self {
        self.balance_shrink = true;
        self
    }

    /// Set the collection width; defaults to [`Length::Fit`].
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Set the collection height; defaults to [`Length::Fit`].
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Allow duplicate keys without the debug-build diff panic.
    /// Duplicates still pair by occurrence order; this changes only the diagnostic.
    /// Prefer [`enzui_core::identity::KeyedIds`] to distinguish repeated entries.
    pub fn allow_duplicate_keys(mut self) -> Self {
        self.allow_duplicate_keys = true;
        self
    }

    /// Return reconciliation keys in child order.
    pub fn keys(&self) -> &[String] {
        &self.keys
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for KeyedRow<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<KeyedRowState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(KeyedRowState {
            keys: self.keys.clone(),
        })
    }

    fn diff(&mut self, tree: &mut Tree) {
        // Check at diff so the builder opt-out and later duplicate keys are honored.
        debug_assert!(
            self.allow_duplicate_keys || crate::keyed_diff::duplicate_keys(&self.keys).is_empty(),
            "KeyedRow: duplicate reconciliation key(s) {:?} in {:?} — duplicates \
             reconcile positionally among themselves, which is the bug keying \
             removes. Disambiguate with enzui_core::identity::KeyedIds, or \
             call .allow_duplicate_keys() if this is deliberate.",
            crate::keyed_diff::duplicate_keys(&self.keys),
            self.keys,
        );

        let state = tree.state.downcast_mut::<KeyedRowState>();

        crate::keyed_diff::reconcile_children_by_key(
            &mut tree.children,
            &state.keys,
            &self.keys,
            &mut self.children,
            |child_tree, child| child.as_widget_mut().diff(child_tree),
            |child| child.as_widget().tag(),
            |child| Tree::new(child.as_widget()),
        );

        if state.keys != self.keys {
            state.keys.clone_from(&self.keys);
        }
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        !tree.frame_children.is_empty()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let equalise = self.equal_cells
            && match self.equal_cells_below {
                // Match ResponsiveSwitch's boundary tolerance.
                Some(threshold) => {
                    let assigned = limits.width(self.width).max().width;
                    assigned.is_finite() && assigned + 0.01 < threshold
                }
                None => true,
            };
        let balanced_widths = if self.balance_shrink && !self.vertical && !self.children.is_empty()
        {
            let max = limits.width(self.width).height(self.height).max();
            let content_w = (max.width - self.padding.left - self.padding.right).max(0.0);
            let content_h = (max.height - self.padding.top - self.padding.bottom).max(0.0);
            let probe = layout::Limits::new(Size::ZERO, Size::new(content_w, content_h));
            let widths: Vec<f32> = self
                .children
                .iter_mut()
                .zip(&mut tree.children)
                .map(|(child, state)| {
                    child
                        .as_widget_mut()
                        .measure(state, renderer, &probe)
                        .size()
                        .width
                })
                .collect();
            let gaps = self.spacing * self.children.len().saturating_sub(1) as f32;
            if max.width.is_finite() && widths.iter().sum::<f32>() + gaps > content_w {
                let mut sorted = widths.clone();
                sorted.sort_by(f32::total_cmp);
                let mut remaining = (content_w - gaps).max(0.0);
                let mut cap = 0.0;
                for (i, width) in sorted.iter().enumerate() {
                    cap = remaining / (sorted.len() - i) as f32;
                    if *width >= cap {
                        break;
                    }
                    remaining -= width;
                }
                Some(
                    widths
                        .into_iter()
                        .map(|width| Some(width.min(cap)))
                        .collect::<Vec<_>>(),
                )
            } else {
                None
            }
        } else {
            None
        };
        if (equalise || balanced_widths.is_some()) && !self.vertical && !self.children.is_empty() {
            let limits = limits.width(self.width).height(self.height);
            let max = limits.max();
            let count = self.children.len();
            let gaps = self.spacing * (count.saturating_sub(1) as f32);
            let content_w = (max.width - self.padding.left - self.padding.right).max(0.0);
            // Fixed-width seams keep their width; remaining tracks share available space.
            let fixed: Vec<Option<f32>> = balanced_widths.unwrap_or_else(|| {
                self.children
                    .iter()
                    .map(|child| match child.as_widget().size().width {
                        Length::Fixed(width) => Some(width),
                        _ => None,
                    })
                    .collect()
            });
            let fixed_total: f32 = fixed.iter().flatten().sum();
            let flexible = fixed.iter().filter(|width| width.is_none()).count();
            let cell_w = if flexible == 0 {
                0.0
            } else {
                ((content_w - gaps - fixed_total) / flexible as f32).max(0.0)
            };
            let content_max_h = (max.height - self.padding.top - self.padding.bottom).max(0.0);
            let mut nodes = Vec::with_capacity(count);
            let mut content_h = 0.0f32;
            for ((child, state), fixed) in
                self.children.iter_mut().zip(&mut tree.children).zip(&fixed)
            {
                let w = fixed.unwrap_or(cell_w);
                let tight = layout::Limits::new(Size::new(w, 0.0), Size::new(w, content_max_h));
                let node = child.as_widget_mut().measure(state, renderer, &tight);
                content_h = content_h.max(node.size().height);
                nodes.push(node);
            }
            let intrinsic = Size::new(
                self.padding.left
                    + self.padding.right
                    + flexible as f32 * cell_w
                    + fixed_total
                    + gaps,
                self.padding.top + self.padding.bottom + content_h,
            );
            let size = limits.resolve(self.width, self.height, intrinsic);
            let mut x = self.padding.left;
            let positioned = nodes
                .into_iter()
                .map(|node| {
                    let y = self.padding.top
                        + match self.align_y {
                            Alignment::Start => 0.0,
                            Alignment::Center => (content_h - node.size().height) / 2.0,
                            Alignment::End => content_h - node.size().height,
                        };
                    let advance = node.size().width + self.spacing;
                    let placed_x = if self.mirrored {
                        size.width - x - node.size().width
                    } else {
                        x
                    };
                    let placed = node.move_to(iced::Point::new(placed_x, y));
                    x += advance;
                    placed
                })
                .collect();
            return layout::Node::with_children(size, positioned).apply(tree);
        }
        // A mirrored row resolves in visual order, then restores the logical
        // order the rest of the widget relies on.
        let mirrored = self.mirrored && !self.vertical;
        if mirrored {
            self.children.reverse();
            tree.children.reverse();
        }
        let node = layout::flex::resolve(
            if self.vertical {
                layout::flex::Axis::Vertical
            } else {
                layout::flex::Axis::Horizontal
            },
            renderer,
            limits,
            self.width,
            self.height,
            self.padding,
            self.spacing,
            self.align_y,
            &mut self.children,
            &mut tree.children,
        );
        if mirrored {
            self.children.reverse();
            tree.children.reverse();
        }
        node.with_reversed_children(mirrored).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
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
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
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
            state.refresh_frame_interest(child.as_widget());
        }
        tree.refresh_frame_children();
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        crate::frame_routing::forward_frame_children(
            &mut self.children,
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
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        if let Some(clipped_viewport) = layout.bounds().intersection(viewport) {
            let viewport = if self.clip {
                &clipped_viewport
            } else {
                viewport
            };

            for (child, (lay, state)) in self
                .children
                .iter()
                .zip(layout.iter(&tree.children))
                .filter(|(_, (lay, _))| lay.bounds().intersects(viewport))
            {
                child
                    .as_widget()
                    .draw(state, renderer, theme, style, lay, cursor, viewport);
            }
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: iced::advanced::mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::advanced::mouse::Interaction {
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
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
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

impl<'a, Message> From<KeyedRow<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(row: KeyedRow<'a, Message>) -> Self {
        Element::new(row)
    }
}
