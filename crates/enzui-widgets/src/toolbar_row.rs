//! Stable start, centre and end slots for toolbar content.
//!
//! Absolute centring is the default: side zones hug the inset edges, while the
//! centre is capped to the remaining width and clamped away from them. Overflowing
//! side zones can leave a zero-width centre. Flex mode flows start and centre
//! from the leading inset, with a gap only when both are present, and inserts a
//! flexible filler before a present end zone to pin it to the trailing edge.
//!
//! Defaults are stock control height (32 px), Md inset (12 px) and Sm gap (8 px).
//! Forward scales at construction to follow custom spacing or sizing. Explicit
//! positive height wins over sizing. Three retained slot boundaries survive zone
//! insertion and presentation changes; each slot accepts one caller element.

use crate::surface_stack::SpaceToken;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::widget::Space;
use iced::{Alignment, Element, Event, Length, Padding, Point, Rectangle, Size, Vector};

/// Builder for three stable toolbar slots with absolute-centred or flex layout.
#[must_use]
pub struct EnzToolbarRow<'a, Message> {
    start: Option<Element<'a, Message>>,
    center: Option<Element<'a, Message>>,
    end: Option<Element<'a, Message>>,
    height: Option<f32>,
    inset: SpaceToken,
    zone_spacing: SpaceToken,
    center_absolute: bool,
    spacing_scale: Option<enzui_theme::SpacingScale>,
    sizing_scale: Option<enzui_theme::SizingTokens>,
    direction: Option<enzui_core::placement::Direction>,
}

/// Build an empty absolute-centred row with stock 32 px height, Md inset and Sm gap.
pub fn enz_toolbar_row<'a, Message: 'a>() -> EnzToolbarRow<'a, Message> {
    EnzToolbarRow {
        start: None,
        center: None,
        end: None,
        height: None,
        inset: SpaceToken::Md,
        zone_spacing: SpaceToken::Sm,
        center_absolute: true,
        spacing_scale: None,
        sizing_scale: None,
        direction: None,
    }
}

impl<'a, Message: 'a> EnzToolbarRow<'a, Message> {
    /// Replace the optional leading slot content.
    pub fn start(mut self, child: Element<'a, Message>) -> Self {
        self.start = Some(child);
        self
    }
    /// Replace the optional centre slot; use center_title for a single-line label.
    pub fn center(mut self, child: Element<'a, Message>) -> Self {
        self.center = Some(child);
        self
    }
    /// Use a single-line label with end elision under the centre width cap.
    pub fn center_title(mut self, title: impl Into<String>) -> Self {
        self.center = Some(crate::text::enz_text(title).no_wrap().into());
        self
    }
    /// Replace the optional trailing slot content.
    pub fn end(mut self, child: Element<'a, Message>) -> Self {
        self.end = Some(child);
        self
    }
    /// Pin height in logical pixels when finite and positive.
    /// Zero, negative and non-finite values retain the previous override; when unset,
    /// forwarded sizing still applies in either call order.
    pub fn height(mut self, h: f32) -> Self {
        self.height = enzui_core::metrics::FinitePositive::new(h)
            .map(enzui_core::metrics::FinitePositive::get)
            .or(self.height);
        self
    }
    /// Resolve inset and zone-gap tokens from this scale at construction.
    /// Nonfinite or negative values use the selected token’s stock spacing. Zero is
    /// accepted, and None tokens resolve the supplied space0, which need not be zero.
    pub fn spacing_scale(mut self, spacing: enzui_theme::SpacingScale) -> Self {
        self.spacing_scale = Some(spacing);
        self
    }
    /// Resolve an unpinned row height from the supplied control_height at construction.
    /// Explicit positive height wins in either call order. Nonfinite or negative scale heights
    /// use the stock control height; zero preserves a collapsed row.
    pub fn sizing_scale(mut self, sizing: enzui_theme::SizingTokens) -> Self {
        self.sizing_scale = Some(sizing);
        self
    }
    /// Return the explicit height, forwarded control_height or stock control_height (32 px).
    pub fn resolved_height(&self) -> f32 {
        self.height.unwrap_or_else(|| {
            let stock = enzui_theme::SizingTokens::default().control_height;
            enzui_core::metrics::FiniteNonNeg::new_or(
                self.sizing_scale.unwrap_or_default().control_height,
                stock,
            )
            .get()
        })
    }
    /// Choose the edge-padding token; defaults to Md (12 px on the stock ladder).
    pub fn inset(mut self, s: SpaceToken) -> Self {
        self.inset = s;
        self
    }
    /// Choose the gap token; defaults to Sm (8 px on the stock ladder).
    /// Absolute mode reserves a safety gap beside each side zone, even when absent.
    /// Flex mode inserts this gap only between a present start and centre.
    pub fn zone_spacing(mut self, s: SpaceToken) -> Self {
        self.zone_spacing = s;
        self
    }
    /// Choose full-row centring with side-zone clamping when true (the default).
    /// False flows start and centre from the leading edge while keeping a present end trailing.
    pub fn center_absolute(mut self, a: bool) -> Self {
        self.center_absolute = a;
        self
    }
    /// Override the writing direction. By default the row follows the
    /// locale; right-to-left puts the start zone on the right and the end
    /// zone on the left.
    pub fn direction(mut self, direction: enzui_core::placement::Direction) -> Self {
        self.direction = Some(direction);
        self
    }
    /// Build three retained slot boundaries, substituting zero-size space for absent content.
    pub fn into_element(self) -> Element<'a, Message> {
        let scale = self.spacing_scale;
        let px = |t: SpaceToken| {
            enzui_core::metrics::FiniteNonNeg::new_or(
                scale.map_or_else(|| t.px(), |s| t.resolve(&s)),
                t.px(),
            )
            .get()
        };
        let row_height = self.resolved_height();
        let inset_px = px(self.inset);
        let gap_px = px(self.zone_spacing);
        let has_start = self.start.is_some();
        let has_center = self.center.is_some();
        let has_end = self.end.is_some();
        let empty = || -> Element<'a, Message> {
            Space::new()
                .width(Length::Fixed(0.0))
                .height(Length::Fixed(0.0))
                .into()
        };

        // Keep logical slots fixed so inserting a zone cannot inherit another zone's state.
        ThreeZoneRow {
            children: [
                ToolbarSlot::new(self.start.unwrap_or_else(empty)).into(),
                ToolbarSlot::new(self.center.unwrap_or_else(empty)).into(),
                ToolbarSlot::new(self.end.unwrap_or_else(empty)).into(),
            ],
            present: [has_start, has_center, has_end],
            center_absolute: self.center_absolute,
            mirrored: enzui_core::placement::resolve_direction(self.direction)
                == enzui_core::placement::Direction::RightToLeft,
            height: row_height,
            inset: inset_px,
            zone_spacing: gap_px,
        }
        .into()
    }
}
struct ToolbarSlotTag;

struct ToolbarSlot<'a, Message> {
    child: Element<'a, Message>,
}

impl<'a, Message> ToolbarSlot<'a, Message> {
    fn new(child: Element<'a, Message>) -> Self {
        Self { child }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for ToolbarSlot<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ToolbarSlotTag>()
    }

    fn state(&self) -> tree::State {
        tree::State::None
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let child = self
            .child
            .as_widget_mut()
            .measure(&mut tree.children[0], renderer, limits);
        layout::Node::with_children(child.size(), vec![child]).apply(tree);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        operation.traverse(&mut |operation| {
            let child_layout = layout
                .children(&tree.children)
                .next()
                .expect("toolbar slot child layout");
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                child_layout,
                viewport,
                renderer,
                operation,
            );
        });
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
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("toolbar slot child layout");
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
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
        crate::frame_routing::forward_frame_opaque(
            &mut self.child,
            tree,
            0,
            event,
            layout
                .children(&tree.children)
                .next()
                .expect("toolbar slot child layout"),
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
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout
                .children(&tree.children)
                .next()
                .expect("toolbar slot child layout"),
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
            layout
                .children(&tree.children)
                .next()
                .expect("toolbar slot child layout"),
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
        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("toolbar slot child layout");
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            child_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<ToolbarSlot<'a, Message>> for Element<'a, Message> {
    fn from(slot: ToolbarSlot<'a, Message>) -> Self {
        Element::new(slot)
    }
}
// Layout adapters borrow real zone trees and never enter the retained event/draw tree.
struct FlexLayoutProxy<'borrow, 'element, Message> {
    child: &'borrow mut Element<'element, Message>,
    tree: &'borrow mut Tree,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for FlexLayoutProxy<'_, '_, Message> {
    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child
            .as_widget_mut()
            .layout(self.tree, renderer, limits);
        tree.size = self.tree.size;
    }

    fn draw(
        &self,
        _tree: &Tree,
        _renderer: &mut iced::Renderer,
        _theme: &iced::Theme,
        _style: &renderer::Style,
        _layout: Layout,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
    }
}
struct FlexLayoutConfig {
    present: [bool; 3],
    height: f32,
    inset: f32,
    gap: f32,
}

fn flex_layout<Message>(
    children: &mut [Element<'_, Message>; 3],
    trees: &mut [Tree],
    config: FlexLayoutConfig,
    renderer: &iced::Renderer,
    limits: &layout::Limits,
) -> layout::Node {
    let FlexLayoutConfig {
        present,
        height,
        inset,
        gap,
    } = config;
    let (start, rest) = children.split_at_mut(1);
    let (center, end) = rest.split_at_mut(1);
    let (start_tree, rest_tree) = trees.split_at_mut(1);
    let (center_tree, end_tree) = rest_tree.split_at_mut(1);

    let mut flex_children: Vec<Element<'_, Message>> = Vec::with_capacity(5);
    flex_children.push(Element::new(FlexLayoutProxy {
        child: &mut start[0],
        tree: &mut start_tree[0],
    }));

    if present[ZONE_START] && present[ZONE_CENTER] && gap > 0.0 {
        flex_children.push(Space::new().width(Length::Fixed(gap)).into());
    }

    let center_index = flex_children.len();
    flex_children.push(Element::new(FlexLayoutProxy {
        child: &mut center[0],
        tree: &mut center_tree[0],
    }));

    if present[ZONE_END] {
        flex_children.push(Space::new().width(Length::Fill).into());
    }

    let end_index = flex_children.len();
    flex_children.push(Element::new(FlexLayoutProxy {
        child: &mut end[0],
        tree: &mut end_tree[0],
    }));
    // Helper trees belong to gaps/adapters; proxies lay out against the retained zone trees.
    let mut layout_trees: Vec<Tree> = (0..flex_children.len()).map(|_| Tree::empty()).collect();
    let stock = layout::flex::resolve(
        layout::flex::Axis::Horizontal,
        renderer,
        limits,
        Length::Fill,
        Length::Fixed(height),
        Padding::from([0.0, inset]),
        0.0,
        Alignment::Center,
        &mut flex_children,
        &mut layout_trees,
    );

    let zone_nodes = vec![
        stock.children()[0].clone(),
        stock.children()[center_index].clone(),
        stock.children()[end_index].clone(),
    ];
    layout::Node::with_children(stock.size(), zone_nodes)
}
const ZONE_START: usize = 0;
const ZONE_CENTER: usize = 1;
const ZONE_END: usize = 2;
/// Three-slot layout primitive built by EnzToolbarRow.
/// Absolute mode centres against full row width and clamps the centre within its
/// remaining width; flex mode uses the stock horizontal allocator. Slots retain
/// their tree positions across presence and presentation changes.
pub struct ThreeZoneRow<'a, Message> {
    children: [Element<'a, Message>; 3],
    present: [bool; 3],
    center_absolute: bool,
    mirrored: bool,
    height: f32,
    inset: f32,
    zone_spacing: f32,
}

/// `node` with its children reflected across its width, for a right-to-left
/// row whose children were placed left to right.
fn mirror_children(node: layout::Node) -> layout::Node {
    let width = node.size().width;
    let children = node
        .children()
        .iter()
        .map(|child| {
            let bounds = child.bounds();
            child
                .clone()
                .move_to(Point::new(width - bounds.x - bounds.width, bounds.y))
        })
        .collect();
    layout::Node::with_children(node.size(), children)
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for ThreeZoneRow<'_, Message> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::stateless()
    }

    fn state(&self) -> tree::State {
        tree::State::None
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: Length::Fill,
            height: Length::Fixed(self.height),
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let mirrored = self.mirrored;
        let place = move |node: layout::Node| {
            if mirrored {
                mirror_children(node)
            } else {
                node
            }
        };
        if !self.center_absolute {
            return place(flex_layout(
                &mut self.children,
                &mut tree.children,
                FlexLayoutConfig {
                    present: self.present,
                    height: self.height,
                    inset: self.inset,
                    gap: self.zone_spacing,
                },
                renderer,
                limits,
            ))
            .apply(tree);
        }

        let limits = limits
            .width(Length::Fill)
            .height(Length::Fixed(self.height));
        let max = limits.max();
        let row_h = max.height;

        let i = self.inset;
        let g = self.zone_spacing;
        let side_avail = sub_clamped(max.width, 2.0 * i);
        // Compress side widths without compressing height, so Fill sides cannot swallow the row.
        let side_limits =
            layout::Limits::with_compression(Size::ZERO, Size::new(side_avail, row_h), SHRINK_MAIN);

        let (start_tree, rest) = tree.children.split_at_mut(1);
        let (center_tree, end_tree) = rest.split_at_mut(1);

        let start_node = self.children[ZONE_START].as_widget_mut().measure(
            &mut start_tree[0],
            renderer,
            &side_limits,
        );
        let end_node = self.children[ZONE_END].as_widget_mut().measure(
            &mut end_tree[0],
            renderer,
            &side_limits,
        );

        let s = start_node.size().width;
        let e = end_node.size().width;
        let center_cap = center_leftover(max.width, s, e, i, g);
        let mut center_node = self.children[ZONE_CENTER].as_widget_mut().measure(
            &mut center_tree[0],
            renderer,
            &center_limits(center_cap, row_h),
        );
        let intrinsic_w = s + e + center_node.size().width + 2.0 * i + 2.0 * g;
        let mut row_w = limits.resolve_width(Length::Fill, intrinsic_w);
        if !row_w.is_finite() {
            row_w = intrinsic_w;
        }
        let center_cap = center_leftover(row_w, s, e, i, g);
        // A compressing parent can resolve narrower than the first measurement limits.
        if center_node.size().width > center_cap {
            center_node = self.children[ZONE_CENTER].as_widget_mut().measure(
                &mut center_tree[0],
                renderer,
                &center_limits(center_cap, row_h),
            );
        }
        let c = center_node.size().width;

        let start_x = i;
        let end_x = row_w - e - i;
        let natural_x = (row_w - c) / 2.0;
        let min_x = start_x + s + g;
        let max_x = end_x - c - g;
        let center_x = if max_x < min_x {
            min_x
        } else {
            natural_x.max(min_x).min(max_x)
        };

        let center_y = |h: f32| ((row_h - h) / 2.0).max(0.0);
        let start_y = center_y(start_node.size().height);
        let end_y = center_y(end_node.size().height);
        let c_y = center_y(center_node.size().height);

        place(layout::Node::with_children(
            Size::new(row_w, row_h),
            vec![
                start_node.move_to(Point::new(start_x, start_y)),
                center_node.move_to(Point::new(center_x, c_y)),
                end_node.move_to(Point::new(end_x, end_y)),
            ],
        ))
        .apply(tree);
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
        cursor: mouse::Cursor,
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

    fn frame_interest(&self, tree: &Tree) -> bool {
        !tree.frame_children.is_empty()
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
        renderer: &iced::Renderer,
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
const SHRINK_MAIN: Size<bool> = Size::new(true, false);
fn center_limits(cap: f32, row_h: f32) -> layout::Limits {
    // Restore main-axis compression for an infinite cap so a Fill centre cannot resolve to infinity.
    let compression = Size::new(!cap.is_finite(), false);
    layout::Limits::with_compression(Size::ZERO, Size::new(cap, row_h), compression)
}
fn sub_clamped(a: f32, b: f32) -> f32 {
    if a.is_infinite() { a } else { (a - b).max(0.0) }
}
fn center_leftover(row_w: f32, start_w: f32, end_w: f32, inset: f32, gap: f32) -> f32 {
    sub_clamped(row_w, start_w + end_w + 2.0 * inset + 2.0 * gap)
}

impl<'a, Message: 'a> From<ThreeZoneRow<'a, Message>> for Element<'a, Message> {
    fn from(w: ThreeZoneRow<'a, Message>) -> Self {
        Element::new(w)
    }
}

impl<'a, Message: 'a> From<EnzToolbarRow<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzToolbarRow<'a, Message>) -> Self {
        w.into_element()
    }
}
