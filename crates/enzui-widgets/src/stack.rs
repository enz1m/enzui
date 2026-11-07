//! EnzVStack / EnzHStack: tokenized stack primitives.
//!
//! Wrappers over iced's `column` / `row` that
//! accept semantic `SpaceToken`s for the gap + (per-edge) insets, a
//! [`StackAlign`] for the main axis (start/center/end/stretch, realized with
//! real Fill-spacer packing), and a [`CrossAlign`] for the cross axis.
//!
//! # Scope
//!
//! - `enz_vstack(children)` — vertical stack (iced `column`).
//! - `enz_hstack(children)` — horizontal stack (iced `row`).
//! - `.gap(SpaceToken)` — spacing between children (defaults to `Md` = 12 px).
//! - `.inset(SpaceToken)` — uniform padding around the group.
//! - `.inset_xy(x, y)` / `.inset_sides(l, r, t, b)` — per-edge insets.
//! - `.align(StackAlign::{Start,Center,End,Stretch})` — main-axis packing.
//! - `.cross(CrossAlign::{Auto,Start,Center,End,Stretch})` — cross-axis
//!   alignment. VStack default `Stretch`, HStack default `Auto`
//!   (resolves to `Center` when natural-height, `Stretch` when `fill`). In iced,
//!   `Stretch` makes the stack accept the full cross extent and lets
//!   children that request `Length::Fill` use it; intrinsic/fixed children
//!   remain intrinsic and are start-aligned.
//! - `.fill(bool)` — grow along the main axis to the parent's assigned extent.
//! - `.hug_content(bool)` — hug the widest child (default) or report a fill
//!   width so a wrapping descendant can’t re-inflate a bounded pane.
//!
//! # Sizing contract
//!
//! A VStack fills width when cross alignment stretches, `fill` is enabled, or
//! `hug_content` is disabled; otherwise it shrinks to its children. Its height
//! fills only with `fill`. An HStack fills width for `fill`, non-Start main-axis
//! packing, or disabled `hug_content`; its height fills for cross-axis stretch
//! or `fill`.
//!
//! [`CrossAlign::Stretch`] offers the available cross extent to children but
//! does not rewrite an intrinsic child's `Length` to `Fill`. Intrinsic/fixed
//! children retain their natural size at the start edge; Fill children expand.
//!
//! # Deferred
//!
//! - the per-child cross-centring quirks — `EnzHCenter`/`EnzVCenter`
//!   already exist in this crate for the row-of-different-heights case.

use crate::surface_stack::{SpaceToken, StackAlign};
use enzui_theme::SpacingScale;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::widget::{Space, container};
use iced::{Alignment, Element, Event, Length, Padding, Rectangle, Size, Vector};

/// Cross-axis alignment inside a stack. Broader than `StackAlign` — includes
/// `Stretch` (the stack accepts the full cross extent and Fill children may
/// use it) and `Auto` (resolve to `Center` for a natural stack, `Stretch`
/// under `fill`).
///
/// VStack defaults to `Stretch`; HStack defaults to `Auto` (resolves
/// to `Center` when natural-height, `Stretch` when `fill`). See
/// [`enz_vstack`] / [`enz_hstack`] for the per-stack default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CrossAlign {
    /// Resolve to `Center` for a natural-extent stack, `Stretch` under `fill`.
    /// This is the HStack default.
    #[default]
    Auto,
    /// Pack children at the start (left for VStack, top for HStack) of the
    /// cross axis.
    Start,
    /// Centre children on the cross axis.
    Center,
    /// Pack children at the end (right for VStack, bottom for HStack) of the
    /// cross axis.
    End,
    /// Accept the full cross-axis extent and start-align children within it.
    /// Children that request `Length::Fill` expand to the
    /// offered extent; intrinsic/fixed children remain intrinsic because iced
    /// does not let a parent rewrite a child's `Length`. VStack default.
    Stretch,
}

impl CrossAlign {
    fn resolve(self, fill: bool) -> CrossAlign {
        match self {
            CrossAlign::Auto => {
                if fill {
                    CrossAlign::Stretch
                } else {
                    CrossAlign::Center
                }
            }
            other => other,
        }
    }

    /// Map a resolved (non-`Auto`) cross alignment to iced's cross alignment.
    /// `Stretch` reports `Start`: the stack takes the Fill extent, then each
    /// child independently decides whether to accept Fill or stay intrinsic.
    fn to_iced(self) -> Alignment {
        match self {
            CrossAlign::Start | CrossAlign::Stretch => Alignment::Start,
            CrossAlign::Center | CrossAlign::Auto => Alignment::Center,
            CrossAlign::End => Alignment::End,
        }
    }

    fn stretches(self) -> bool {
        matches!(self, CrossAlign::Stretch)
    }
}

/// Build the per-edge padding from four insets, resolved against the given
/// spacing scale.
fn edge_padding(
    left: SpaceToken,
    right: SpaceToken,
    top: SpaceToken,
    bottom: SpaceToken,
    scale: &SpacingScale,
) -> Padding {
    Padding {
        left: left.resolve(scale),
        right: right.resolve(scale),
        top: top.resolve(scale),
        bottom: bottom.resolve(scale),
    }
}

/// Pack children along the vertical main axis with real Fill spacers,
/// mirroring the `VerticalLayout.alignment`:
///
/// - `Start`  → children packed at the top (no spacers).
/// - `Center` → spacer above and below the group.
/// - `End`    → spacer above the group.
/// - `Stretch`→ a spacer between every adjacent pair (distribute).
///
/// The caller decides whether packing applies (only meaningful when the column
/// is taller than its content, i.e. `fill`).
fn pack_column<'a, Message: 'a>(
    children: Vec<Element<'a, Message>>,
    align: StackAlign,
) -> Vec<Element<'a, Message>> {
    let mut items: Vec<Element<'a, Message>> =
        Vec::with_capacity(packed_len(children.len(), align));
    let spacer = || -> Element<'a, Message> { Space::new().height(Length::Fill).into() };
    match align {
        StackAlign::Start => items.extend(children),
        StackAlign::Center => {
            items.push(spacer());
            items.extend(children);
            items.push(spacer());
        }
        StackAlign::End => {
            items.push(spacer());
            items.extend(children);
        }
        StackAlign::Stretch => {
            let last = children.len().saturating_sub(1);
            for (i, c) in children.into_iter().enumerate() {
                items.push(c);
                if i != last {
                    items.push(spacer());
                }
            }
        }
    }
    items
}

/// Row analog of [`pack_column`] — packs along the horizontal main axis
/// with horizontal Fill spacers.
fn pack_row<'a, Message: 'a>(
    children: Vec<Element<'a, Message>>,
    align: StackAlign,
) -> Vec<Element<'a, Message>> {
    let mut items: Vec<Element<'a, Message>> =
        Vec::with_capacity(packed_len(children.len(), align));
    let spacer = || -> Element<'a, Message> { Space::new().width(Length::Fill).into() };
    match align {
        StackAlign::Start => items.extend(children),
        StackAlign::Center => {
            items.push(spacer());
            items.extend(children);
            items.push(spacer());
        }
        StackAlign::End => {
            items.push(spacer());
            items.extend(children);
        }
        StackAlign::Stretch => {
            let last = children.len().saturating_sub(1);
            for (i, c) in children.into_iter().enumerate() {
                items.push(c);
                if i != last {
                    items.push(spacer());
                }
            }
        }
    }
    items
}

/// Number of elements a packed stack produces for `n` children under `align`
/// (children plus Fill spacers). Pure helper mirroring [`pack_column`] /
/// [`pack_row`] so the spacer placement is unit-testable without reaching into
/// iced's private `Column`/`Row` internals.
///
/// - `Start`   → `n` (no spacers).
/// - `Center`  → `n + 2` (leading + trailing), or `2` for an empty stack.
/// - `End`     → `n + 1` (leading), or `1` for an empty stack.
/// - `Stretch` → `2n - 1` (between each pair), or `0`/`1` for `n <= 1`.
fn packed_len(n: usize, align: StackAlign) -> usize {
    match align {
        StackAlign::Start => n,
        StackAlign::Center => n + 2,
        StackAlign::End => n + 1,
        StackAlign::Stretch => {
            if n <= 1 {
                n
            } else {
                2 * n - 1
            }
        }
    }
}

fn resolve_main_axis(align: StackAlign, fill: bool) -> StackAlign {
    if fill && matches!(align, StackAlign::Start) {
        StackAlign::Stretch
    } else {
        align
    }
}

#[derive(Debug, Clone, Copy)]
enum CollapseAxis {
    Horizontal,
    Vertical,
}

impl CollapseAxis {
    fn flex(self) -> layout::flex::Axis {
        match self {
            Self::Horizontal => layout::flex::Axis::Horizontal,
            Self::Vertical => layout::flex::Axis::Vertical,
        }
    }
}

/// A flex stack whose separator belongs to the two non-empty neighbours.
///
/// iced's stock row/column charges spacing by child count, so a zero-sized
/// retained motion wrapper leaves a phantom gap. This layout charges spacing
/// only between children whose laid-out main-axis extent is non-zero. Two
/// adjacent hidden wrappers therefore contribute no separator at all: after
/// removing them there is still exactly one gap between the surviving
/// neighbours, which is the geometry an actually absent subtree has.
struct CollapsingFlex<'a, Message> {
    axis: CollapseAxis,
    spacing: f32,
    padding: Padding,
    width: Length,
    height: Length,
    align: Alignment,
    children: Vec<Element<'a, Message>>,
}

impl<'a, Message: 'a> CollapsingFlex<'a, Message> {
    fn new(
        axis: CollapseAxis,
        children: Vec<Element<'a, Message>>,
        spacing: f32,
        padding: Padding,
        width: Length,
        height: Length,
        align: Alignment,
    ) -> Self {
        Self {
            axis,
            spacing,
            padding,
            width,
            height,
            align,
            children: children
                .into_iter()
                .filter(|child| !child.as_widget().is_void())
                .collect(),
        }
    }
}

impl<'a, Message: 'a> Widget<Message, iced::Theme, iced::Renderer> for CollapsingFlex<'a, Message> {
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        layout::flex::resolve_collapsing(
            self.axis.flex(),
            renderer,
            limits,
            self.width,
            self.height,
            self.padding,
            self.spacing,
            self.align,
            &mut self.children,
            &mut tree.children,
        )
        .with_drawing_cull(false)
        .with_child_drawing_culls()
        .apply(tree);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        !tree.frame_children.is_empty()
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
            for (child, (layout, state)) in self
                .children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
            {
                child
                    .as_widget_mut()
                    .operate(state, layout, viewport, renderer, operation);
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
        for (child, (layout, state)) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
        {
            child
                .as_widget_mut()
                .update(state, event, layout, cursor, renderer, shell, viewport);
            state.refresh_frame_interest(child.as_widget());
        }
        tree.refresh_frame_children();
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
        let Tree {
            children,
            frame_children,
            ..
        } = tree;
        let mut route = 0;
        while route < frame_children.len() {
            let index = frame_children[route];
            let child = &mut self.children[index];
            let child_layout = layout.child(children, index);
            let child_tree = &mut children[index];
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
            .map(|(child, (layout, state))| {
                child
                    .as_widget()
                    .mouse_interaction(state, layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default()
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
        layout.draw_culled(tree, renderer, viewport, |renderer, viewport| {
            for (child, (layout, state)) in self.children.iter().zip(layout.iter(&tree.children)) {
                layout.draw_culled(state, renderer, viewport, |renderer, viewport| {
                    child
                        .as_widget()
                        .draw(state, renderer, theme, style, layout, cursor, viewport);
                });
            }
        });
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

pub(crate) fn collapsing_column<'a, Message: 'a>(
    children: Vec<Element<'a, Message>>,
    spacing: f32,
    padding: Padding,
    width: Length,
    height: Length,
    align: Alignment,
) -> Element<'a, Message> {
    Element::new(CollapsingFlex::new(
        CollapseAxis::Vertical,
        children,
        spacing,
        padding,
        width,
        height,
        align,
    ))
}

fn collapsing_row<'a, Message: 'a>(
    children: Vec<Element<'a, Message>>,
    spacing: f32,
    padding: Padding,
    width: Length,
    height: Length,
    align: Alignment,
) -> Element<'a, Message> {
    Element::new(CollapsingFlex::new(
        CollapseAxis::Horizontal,
        children,
        spacing,
        padding,
        width,
        height,
        align,
    ))
}

// ---------------------------------------------------------------------------
// EnzVStack
// ---------------------------------------------------------------------------

/// Vertical-stack builder.
#[must_use]
pub struct EnzVStack<'a, Message> {
    children: Vec<Element<'a, Message>>,
    gap: SpaceToken,
    inset_left: SpaceToken,
    inset_right: SpaceToken,
    inset_top: SpaceToken,
    inset_bottom: SpaceToken,
    align: StackAlign,
    cross: CrossAlign,
    fill: bool,
    hug_content: bool,
    /// Spacing scale for resolving `SpaceToken`s. Defaults to
    /// `SpacingScale::default()`; override via [`Self::spacing_scale`] to use
    /// the active theme’s custom density.
    spacing_scale: SpacingScale,
}

/// Construct a vertical stack. Cross-axis default is [`CrossAlign::Stretch`]
/// (`EnzVStack` `cross-align: "stretch"`). This fills the stack width;
/// only children that themselves request `Length::Fill` fill that width.
pub fn enz_vstack<'a, Message: 'a>(children: Vec<Element<'a, Message>>) -> EnzVStack<'a, Message> {
    EnzVStack {
        children,
        gap: SpaceToken::default(),
        inset_left: SpaceToken::None,
        inset_right: SpaceToken::None,
        inset_top: SpaceToken::None,
        inset_bottom: SpaceToken::None,
        align: StackAlign::default(),
        cross: CrossAlign::Stretch,
        fill: false,
        hug_content: true,
        spacing_scale: SpacingScale::default(),
    }
}

impl<'a, Message: 'a> EnzVStack<'a, Message> {
    /// Vertical gap between children as a spacing token. Defaults to
    /// [`SpaceToken::Md`] (12 logical pixels with the default scale).
    pub fn gap(mut self, g: SpaceToken) -> Self {
        self.gap = g;
        self
    }
    /// Uniform inset on all four edges.
    pub fn inset(mut self, i: SpaceToken) -> Self {
        self.inset_left = i;
        self.inset_right = i;
        self.inset_top = i;
        self.inset_bottom = i;
        self
    }
    /// Independent horizontal / vertical insets (`inset-x` / `inset-y`).
    pub fn inset_xy(mut self, x: SpaceToken, y: SpaceToken) -> Self {
        self.inset_left = x;
        self.inset_right = x;
        self.inset_top = y;
        self.inset_bottom = y;
        self
    }
    /// Per-edge insets .
    pub fn inset_sides(
        mut self,
        left: SpaceToken,
        right: SpaceToken,
        top: SpaceToken,
        bottom: SpaceToken,
    ) -> Self {
        self.inset_left = left;
        self.inset_right = right;
        self.inset_top = top;
        self.inset_bottom = bottom;
        self
    }
    /// Main-axis (vertical) packing of children. Defaults to the
    /// [`StackAlign`] default.
    pub fn align(mut self, a: StackAlign) -> Self {
        self.align = a;
        self
    }
    /// Cross-axis (horizontal) alignment. Defaults to [`CrossAlign::Stretch`]
    /// for a VStack.
    pub fn cross(mut self, c: CrossAlign) -> Self {
        self.cross = c;
        self
    }
    /// Fill the available main-axis extent instead of hugging content. Defaults
    /// to `false`; also resolves `CrossAlign::Auto` to `Stretch`.
    pub fn fill(mut self, f: bool) -> Self {
        self.fill = f;
        self
    }
    /// Hug the widest child (default) or report a fill width so a wrapping
    /// descendant can't re-inflate a bounded pane .
    pub fn hug_content(mut self, hug: bool) -> Self {
        self.hug_content = hug;
        self
    }
    /// Override the spacing scale used to resolve `SpaceToken`s (gap + insets).
    ///
    /// Defaults to [`SpacingScale::default`]. Pass the active theme’s scale
    /// via [`crate::theme_lookup::spacing`] so a custom density flows through.
    pub fn spacing_scale(mut self, scale: SpacingScale) -> Self {
        self.spacing_scale = scale;
        self
    }
    /// Append one child (accumulates). No children by default.
    pub fn push(mut self, child: impl Into<Element<'a, Message>>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        let cross = self.cross.resolve(self.fill);
        let stretches = cross.stretches();
        let col_width = if stretches || self.fill || !self.hug_content {
            Length::Fill
        } else {
            Length::Shrink
        };
        // `fill` grows the main axis (height).
        let col_height = if self.fill {
            Length::Fill
        } else {
            Length::Shrink
        };

        let scale = &self.spacing_scale;
        let padding = edge_padding(
            self.inset_left,
            self.inset_right,
            self.inset_top,
            self.inset_bottom,
            scale,
        );
        let main = resolve_main_axis(self.align, self.fill);
        let col = collapsing_column(
            pack_column(self.children, main),
            self.gap.resolve(scale),
            padding,
            col_width,
            col_height,
            cross.to_iced(),
        );

        if stretches {
            // Constrain the inner column to the cross extent so children that
            // accept Fill grow to it.
            container(col).width(Length::Fill).into()
        } else {
            col
        }
    }
}

impl<'a, Message: 'a> From<EnzVStack<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzVStack<'a, Message>) -> Self {
        w.into_element()
    }
}

// ---------------------------------------------------------------------------
// EnzHStack
// ---------------------------------------------------------------------------

/// Horizontal-stack builder.
#[must_use]
pub struct EnzHStack<'a, Message> {
    children: Vec<Element<'a, Message>>,
    gap: SpaceToken,
    inset_left: SpaceToken,
    inset_right: SpaceToken,
    inset_top: SpaceToken,
    inset_bottom: SpaceToken,
    align: StackAlign,
    cross: CrossAlign,
    fill: bool,
    hug_content: bool,
    /// Spacing scale for resolving `SpaceToken`s.
    spacing_scale: SpacingScale,
}

/// Construct a horizontal stack. Cross-axis default is [`CrossAlign::Auto`]
/// (`EnzHStack` `cross-align: "auto"` → `center`/`stretch`).
pub fn enz_hstack<'a, Message: 'a>(children: Vec<Element<'a, Message>>) -> EnzHStack<'a, Message> {
    EnzHStack {
        children,
        gap: SpaceToken::default(),
        inset_left: SpaceToken::None,
        inset_right: SpaceToken::None,
        inset_top: SpaceToken::None,
        inset_bottom: SpaceToken::None,
        align: StackAlign::default(),
        cross: CrossAlign::Auto,
        fill: false,
        hug_content: true,
        spacing_scale: SpacingScale::default(),
    }
}

impl<'a, Message: 'a> EnzHStack<'a, Message> {
    /// Horizontal gap between children as a spacing token. Defaults to
    /// [`SpaceToken::Md`] (12 logical pixels with the default scale).
    pub fn gap(mut self, g: SpaceToken) -> Self {
        self.gap = g;
        self
    }
    /// Uniform inset on all four edges.
    pub fn inset(mut self, i: SpaceToken) -> Self {
        self.inset_left = i;
        self.inset_right = i;
        self.inset_top = i;
        self.inset_bottom = i;
        self
    }
    /// Independent horizontal / vertical insets (`inset-x` / `inset-y`).
    pub fn inset_xy(mut self, x: SpaceToken, y: SpaceToken) -> Self {
        self.inset_left = x;
        self.inset_right = x;
        self.inset_top = y;
        self.inset_bottom = y;
        self
    }
    /// Per-edge insets .
    pub fn inset_sides(
        mut self,
        left: SpaceToken,
        right: SpaceToken,
        top: SpaceToken,
        bottom: SpaceToken,
    ) -> Self {
        self.inset_left = left;
        self.inset_right = right;
        self.inset_top = top;
        self.inset_bottom = bottom;
        self
    }
    /// Main-axis (horizontal) packing of children. Defaults to the
    /// [`StackAlign`] default.
    pub fn align(mut self, a: StackAlign) -> Self {
        self.align = a;
        self
    }
    /// Cross-axis (vertical) alignment. Defaults to [`CrossAlign::Auto`] for an
    /// HStack (Center when natural-extent, Stretch under `fill`).
    pub fn cross(mut self, c: CrossAlign) -> Self {
        self.cross = c;
        self
    }
    /// Fill the available main-axis extent instead of hugging content. Defaults
    /// to `false`; also resolves `CrossAlign::Auto` to `Stretch`.
    pub fn fill(mut self, f: bool) -> Self {
        self.fill = f;
        self
    }
    /// Hug the widest child (default) or report a fill width so a wrapping
    /// descendant can't re-inflate a bounded pane .
    pub fn hug_content(mut self, hug: bool) -> Self {
        self.hug_content = hug;
        self
    }
    /// Override the spacing scale used to resolve `SpaceToken`s (gap + insets).
    ///
    /// Defaults to [`SpacingScale::default`]. Pass the active theme’s scale
    /// via [`crate::theme_lookup::spacing`] so a custom density flows through.
    pub fn spacing_scale(mut self, scale: SpacingScale) -> Self {
        self.spacing_scale = scale;
        self
    }
    /// Append one child (accumulates). No children by default.
    pub fn push(mut self, child: impl Into<Element<'a, Message>>) -> Self {
        self.children.push(child.into());
        self
    }

    /// Consume the builder and produce the widget tree.
    pub fn into_element(self) -> Element<'a, Message> {
        let cross = self.cross.resolve(self.fill);
        let stretches = cross.stretches();
        let needs_main_space =
            self.fill || !matches!(resolve_main_axis(self.align, self.fill), StackAlign::Start);
        let row_width = if needs_main_space || !self.hug_content {
            Length::Fill
        } else {
            Length::Shrink
        };
        // Cross axis is the height: fill it when stretching children, else hug.
        let row_height = if stretches || self.fill {
            Length::Fill
        } else {
            Length::Shrink
        };

        let scale = &self.spacing_scale;
        let padding = edge_padding(
            self.inset_left,
            self.inset_right,
            self.inset_top,
            self.inset_bottom,
            scale,
        );
        let main = resolve_main_axis(self.align, self.fill);
        let row_widget = collapsing_row(
            pack_row(self.children, main),
            self.gap.resolve(scale),
            padding,
            row_width,
            row_height,
            cross.to_iced(),
        );

        if stretches {
            container(row_widget).height(Length::Fill).into()
        } else {
            row_widget
        }
    }
}

impl<'a, Message: 'a> From<EnzHStack<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzHStack<'a, Message>) -> Self {
        w.into_element()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
