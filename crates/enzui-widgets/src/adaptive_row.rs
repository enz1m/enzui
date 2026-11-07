//! [`enz_adaptive_row`] — horizontal row with configurable spacing.
//!
//! A horizontal layout for fixed leading content plus one
//! flexible trailing child, with a spacing token, transparent
//! background, shrink-to-content
//! height, and clipping so oversized content can't escape under narrow
//! pressure.
//!
//! ## What this wrapper adds over a bare `Row`
//!
//! It is a thin policy wrapper — its value is baking in the three
//! defaults a bare `Row` gets wrong: the `spacing-lg` token (16 px), clipping,
//! and `width: Fill`. The *sizing* is still the caller's job via native
//! iced lengths: keep leading children intrinsic (`Shrink`) and give the
//! trailing child `Length::Fill` so it absorbs slack and shrinks first under
//! pressure.
//!
//! ## Clipped overflow is inert, not merely invisible
//!
//! A bare `Row::clip(true)` narrows the *draw* viewport and *nothing else*.
//! In `iced/widget/src/row.rs`, `self.clip` is read at exactly one
//! site — `:294`, inside `draw` — while `update` (`:241`), `operate` (`:220`)
//! and `mouse_interaction` (`:263`) forward the *unclipped* `viewport` and the
//! child's own layout bounds to every child. So under a bare clipped `Row`,
//! content that has visually overflowed and been clipped away stays
//! clickable, focusable, and present in the accessibility tree: an
//! ordinary `Button` hit-tests its own bounds and never consults the viewport,
//! so it fires from under the clip. A keyboard user tabs to an invisible
//! control, a screen-reader user is told about a control that is not there, and
//! a mouse user clicks a button they cannot see.
//!
//! This wrapper closes that gap at the *wrapper* level, because it is a shared
//! layout primitive and the caller is the wrong place to fix it. It is a
//! custom [`Widget`] ([`EnzAdaptiveRowWidget`]) that lays its children out with
//! the same `layout::flex::resolve` a `Row` uses, then — when `clip` is on —
//! runs all four paths through one clip filter: `update`,
//! `mouse_interaction`, `operate` and `draw` each skip any child
//! whose layout bounds do not intersect the row's own bounds.
//! The four paths therefore
//! agree on exactly what is visible, so a clipped-away child is inert
//! (no `on_press`, not a tab stop, absent from semantics) as well as
//! unpainted. See [`intersects_clip`] for the single predicate all four share.
//!
//! Partial overlap keeps the control live only through its
//! visible area.
//! Any positive intersection keeps the child in traversal, drawing, and
//! semantics because a partially visible control is still perceivable. Pointer
//! paths additionally levitate the cursor whenever it lies outside the row, so
//! the invisible half of a straddling child cannot hover or click. A zero-width
//! row clips everything away (its bounds intersect nothing), which is the
//! honest reading of "nothing is visible".
//!
//! With `clip(false)` the filter is disabled and every child stays live in all
//! four paths regardless of position — the wrapper is then a plain spacing/width
//! `Row`.

use crate::surface_stack::SpaceToken;
use enzui_core::metrics::FiniteNonNeg;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::alignment::Alignment;
use iced::{Element, Event, Length, Padding, Rectangle, Size, Vector};

/// Does `child_bounds` intersect the clip region `row_bounds`?
///
/// This is the single visibility predicate the wrapper's four
/// paths share when clipping is on. A child is live iff its
/// bounds intersect the row's own bounds; a child entirely
/// outside the row is gated out of `update`, `mouse_interaction`,
/// `operate` and `draw` alike. Any overlap — even a partial one —
/// keeps the child fully live, matching a bare `Row::draw`, which
/// keeps a child that `layout.bounds().intersects(viewport)`.
#[inline]
pub fn intersects_clip(child_bounds: Rectangle, row_bounds: Rectangle) -> bool {
    child_bounds.intersects(&row_bounds)
}

/// Default adaptive-row spacing token (`row-spacing: spacing-lg` = 16 px).
pub const DEFAULT_ADAPTIVE_ROW_SPACING_TOKEN: SpaceToken = SpaceToken::Lg;
/// Default adaptive-row spacing in px (`spacing-lg` = 16 px).
pub const DEFAULT_ADAPTIVE_ROW_SPACING: f32 = 16.0;

/// Builder returned by [`enz_adaptive_row`].
#[must_use]
pub struct EnzAdaptiveRow<'a, Message> {
    children: Vec<Element<'a, Message>>,
    row_spacing: f32,
    width: Length,
    clip: bool,
    align_y: Alignment,
}

/// Create an EnzAdaptiveRow wrapping a slice of children.
pub fn enz_adaptive_row<'a, Message: 'a>(
    children: Vec<Element<'a, Message>>,
) -> EnzAdaptiveRow<'a, Message> {
    EnzAdaptiveRow {
        children,
        row_spacing: DEFAULT_ADAPTIVE_ROW_SPACING_TOKEN.px(),
        width: Length::Fill,
        clip: true,
        // `Alignment::Start` is the default for callers that do not choose an
        // axis alignment explicitly.
        align_y: Alignment::Start,
    }
}

impl<'a, Message: 'a> EnzAdaptiveRow<'a, Message> {
    /// Override the inter-child spacing from a semantic [`SpaceToken`]
    /// (`row-spacing: spacing-lg`, default [`SpaceToken::Lg`] = 16 px).
    pub fn row_spacing_token(mut self, token: SpaceToken) -> Self {
        self.row_spacing = token.px();
        self
    }

    /// Raw-pixel escape hatch for [`Self::row_spacing_token`].
    ///
    /// Non-expressible input (`NaN`, `±INFINITY`, negative) falls back to the
    /// token default ([`DEFAULT_ADAPTIVE_ROW_SPACING_TOKEN`] = `SpaceToken::Lg`
    /// = 16 px); `0.0` is expressible intent ("no gap") and is honoured
    /// literally. This keeps invalid values from reaching layout or being
    /// mistaken for an explicit zero gap.
    pub fn row_spacing(mut self, px: f32) -> Self {
        self.row_spacing = FiniteNonNeg::new_or(px, DEFAULT_ADAPTIVE_ROW_SPACING_TOKEN.px()).get();
        self
    }

    /// Override the outer width (default `Length::Fill`).
    pub fn width(mut self, w: Length) -> Self {
        self.width = w;
        self
    }

    /// Toggle content clipping (`clip: true`, default `true`). Keeps a
    /// child that overflows under narrow pressure from drawing past the row.
    pub fn clip(mut self, clip: bool) -> Self {
        self.clip = clip;
        self
    }

    /// Cross-axis (vertical) alignment of the children, threaded into
    /// `layout::flex::resolve`. Defaults to [`Alignment::Start`], matching the
    /// literal `Start` used by the layout path when no override is set.
    ///
    /// Named `align_y` to match the sibling horizontal-row spellings
    /// ([`keyed_row::KeyedRow::align_y`](crate::keyed_row::KeyedRow::align_y),
    /// `iced::widget::Row::align_y`) and every other widget in this crate.
    ///
    /// A toolbar of mixed-height children — a fixed-height pill or icon chip
    /// sharing a row with a bare text label — needs `Alignment::Center` to
    /// put them on a common midline; the default `Start` top-aligns each
    /// child within the row's resolved height.
    pub fn align_y(mut self, align: impl Into<Alignment>) -> Self {
        self.align_y = align.into();
        self
    }

    /// Build the element.
    pub fn into_element(self) -> Element<'a, Message> {
        Element::new(EnzAdaptiveRowWidget {
            children: self.children,
            spacing: self.row_spacing,
            width: self.width,
            clip: self.clip,
            align_y: self.align_y,
        })
    }
}

// ---------------------------------------------------------------------------
// Widget: a flex Row whose four interaction paths honour the clip region.
// ---------------------------------------------------------------------------

/// The custom [`Widget`] behind [`enz_adaptive_row`].
///
/// Layout is identical to a bare `Row` (same `layout::flex::resolve`,
/// horizontal axis, caller-configurable cross-axis alignment via
/// [`EnzAdaptiveRow::align_y`], no padding). The difference is that when
/// `clip` is on, `update`, `mouse_interaction`, `operate` and `draw` all skip
/// any child whose bounds fail [`intersects_clip`] against the row's own
/// bounds — so a clipped-away child is inert as well as unpainted. Pointer
/// paths also levitate positions outside the row, including the invisible half
/// of a partially clipped child. See the module docs.
pub struct EnzAdaptiveRowWidget<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    children: Vec<Element<'a, Message, Theme, Renderer>>,
    spacing: f32,
    width: Length,
    clip: bool,
    align_y: Alignment,
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for EnzAdaptiveRowWidget<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            // Shrink-to-content height (`vertical-stretch: 0`).
            height: Length::Shrink,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        // Exactly what `Row::layout` does: horizontal flex, caller's
        // cross-axis alignment, no padding, our spacing, our width, Shrink
        // height. The `align_y` here is what makes a toolbar of mixed-height
        // children share a midline under `Alignment::Center`; the default
        // `Alignment::Start` keeps top-aligned geometry.
        layout::flex::resolve(
            layout::flex::Axis::Horizontal,
            renderer,
            limits,
            self.width,
            Length::Shrink,
            Padding::ZERO,
            self.spacing,
            self.align_y,
            &mut self.children,
            &mut tree.children,
        )
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
        let row_bounds = layout.bounds();
        let clip = self.clip;
        operation.container(None, row_bounds, viewport);
        operation.traverse(&mut |operation| {
            if clip {
                // Besides dropping wholly invisible descendants, intersect the
                // bounds reported for partial descendants. Focus geometry and
                // AccessKit bounds then describe exactly the painted region.
                let mut clipped = crate::inert_clip::ClipOperation::new(operation, row_bounds);
                self.children
                    .iter_mut()
                    .zip(layout.iter_mut(&mut tree.children))
                    .filter(|(_, (layout, _))| intersects_clip(layout.bounds(), row_bounds))
                    .for_each(|(child, (layout, state))| {
                        child.as_widget_mut().operate(
                            state,
                            layout,
                            viewport,
                            renderer,
                            &mut clipped,
                        );
                    });
            } else {
                self.children
                    .iter_mut()
                    .zip(layout.iter_mut(&mut tree.children))
                    .for_each(|(child, (layout, state))| {
                        child
                            .as_widget_mut()
                            .operate(state, layout, viewport, renderer, operation);
                    });
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
        let row_bounds = layout.bounds();
        let clip = self.clip;
        // A partially visible child remains live, but only through the visible
        // intersection. Forwarding the real cursor at x=220 to a child whose
        // 150..270 bounds straddle a row ending at x=200 made its invisible
        // half clickable. Levitation preserves non-pointer events while
        // making every descendant see the pointer as off-widget.
        let cursor = if clip {
            crate::inert_clip::cursor_within(cursor, row_bounds)
        } else {
            cursor
        };
        for (child, (layout, state)) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
            .filter(|(_, (layout, _))| !clip || intersects_clip(layout.bounds(), row_bounds))
        {
            child
                .as_widget_mut()
                .update(state, event, layout, cursor, renderer, shell, viewport);
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
        let row_bounds = layout.bounds();
        let clip = self.clip;
        let cursor = if clip {
            crate::inert_clip::cursor_within(cursor, row_bounds)
        } else {
            cursor
        };
        self.children
            .iter()
            .zip(layout.iter(&tree.children))
            .filter(|(_, (layout, _))| !clip || intersects_clip(layout.bounds(), row_bounds))
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
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        // Mirror `Row::draw`: intersect the row bounds with the incoming
        // viewport, then (when clipping) draw only children that fall inside.
        if let Some(clipped_viewport) = layout.bounds().intersection(viewport) {
            let draw_viewport = if self.clip {
                &clipped_viewport
            } else {
                viewport
            };
            let row_bounds = layout.bounds();
            let clip = self.clip;
            // Draw-time status must use the same visible pointer region as
            // update/mouse_interaction. Otherwise an invisible cursor location
            // can still paint Hovered on the visible half of a straddling child.
            let cursor = if clip {
                crate::inert_clip::cursor_within(cursor, row_bounds)
            } else {
                cursor
            };
            for (child, (layout, state)) in self
                .children
                .iter()
                .zip(layout.iter(&tree.children))
                .filter(|(_, (layout, _))| !clip || intersects_clip(layout.bounds(), row_bounds))
                .filter(|(_, (layout, _))| layout.bounds().intersects(draw_viewport))
            {
                child.as_widget().draw(
                    state,
                    renderer,
                    theme,
                    style,
                    layout,
                    cursor,
                    draw_viewport,
                );
            }
        }
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

impl<'a, Message: 'a> From<EnzAdaptiveRow<'a, Message>> for Element<'a, Message> {
    fn from(w: EnzAdaptiveRow<'a, Message>) -> Self {
        w.into_element()
    }
}
