//! EnzScrollColumn: top-anchored scrollable vertical stack.
//!
//! A vertically scrollable column of children with configurable spacing,
//! per-edge insets, and an optional max-height "grow to fit, then scroll
//! past the cap" behaviour. Used for sidebars, document bodies, preference
//! panes, and section content.
//!
//! iced 0.15 supplies `Scrollable::new(child)` with default vertical
//! direction and top-anchoring; a `Column` composed inside it, apply
//! spacing + padding, and expose a `max_content_height` knob that caps
//! the outer scrollable's height.
//!
//! # Behaviour
//!
//! * Spacing and insets accept the crate-local [`SpaceToken`] for the
//!   semantic path ([`EnzScrollColumn::gap`] / [`EnzScrollColumn::inset_token`])
//!   and keep raw-`f32` setters as an explicit escape hatch.
//! * An unset max height means uncapped (a `<= 0` / non-finite cap degrades to
//!   uncapped). The cap is a maximum,
//!   not an exact height: short content hugs, tall content scrolls — modelled
//!   by the [`crate::max_height::MaxHeight`] limits wrapper, not `Fixed(cap)`.
//! * Scrollbar-gutter clearance is available as an opt-in trailing
//!   clearance, `scrollbar_gutter(true)`
//!   (see [`EnzScrollColumn::scrollbar_gutter`]). It is off by default and
//!   composes as a maximum with the caller's right inset, so
//!   a plain scroll column has identical left and right insets and the
//!   floating bar overlays content rather than displacing it.
//! * Scroll mechanics consolidate through [`crate::scroll_view::EnzScrollView`];
//!   [`EnzScrollColumn::id`] / [`EnzScrollColumn::on_scroll`] expose
//!   `Id`- and `Viewport`-based control for consumers like ChatTranscript.

use crate::max_height::MaxHeight;
use crate::scroll_view::{Axis, ScrollGeometryCell, enz_scroll_view};
use crate::surface_stack::SpaceToken;
use enzui_core::metrics::FiniteNonNeg;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Renderer as _, Shell, Widget, layout, mouse, overlay, renderer};
use iced::widget::Column;
use iced::widget::scrollable::{AbsoluteOffset, Viewport};
use iced::widget::{Id, container};
use iced::{Element, Event, Length, Padding, Rectangle, Size, Vector};

/// Trailing clearance (px) available to callers who opt in to the
/// scrollbar gutter, keeping content clear of the floating bar.
///
/// Derived from `ScrollbarTokens::DEFAULT.content_gutter`, not a copy of
/// its value. It is the compile-time spelling for tests and callers with
/// no theme reference; because it is a `const` *expression* over the token, a
/// retune of the token carries this with it and cannot leave a stale literal
/// behind. `scroll_column::tests::gutter_constant_tracks_the_theme_token` pins
/// the identity, and `..._is_derived_from_the_token_not_a_literal` pins that it
/// is written as a derivation — a value check alone cannot tell a derivation
/// from a coincidence.
///
/// # Opt-in trailing clearance
///
/// The default is `false` ([`EnzScrollColumn::scrollbar_gutter`]), so a plain
/// scroll column is symmetric. When opted in, the gutter composes as a
/// maximum with the caller's right inset rather than adding to it, so a
/// caller whose own padding already clears the bar stays symmetric.
///
/// The elision concern that motivated it is not dropped — it is narrowed to
/// the composites that actually have it. ChatTranscript pushes right-aligned
/// bubbles flush to the trailing edge at `transcript_padding(0)`, so it
/// opts in explicitly; `shortcut_list` end-elides its description column and
/// keeps its own `Scrollbar::spacing` reservation. A generic scroll column has
/// no such content and pays no asymmetry for it.
///
/// The 10 px value follows the thumb's inner edge
/// at `(hit_width + width) / 2` = 10 px from the trailing edge.
///
/// # This is the framework's one scroll-region gutter
///
/// `EnzTable::SCROLLBAR_GUTTER` (the table's thumb *lane*) is a different
/// mechanism with the same source of truth, and is likewise derived from the
/// token rather than re-typed. `shortcut_list::DEFAULT_SCROLLBAR_GUTTER` is
/// not this concept at all despite the name — it is an `iced`
/// `Scrollbar::spacing` value (2 px of breathing room next to a *reserving*
/// bar), and is documented as such where it is declared.
pub const DEFAULT_SCROLLBAR_GUTTER: f32 = enzui_theme::ScrollbarTokens::DEFAULT.content_gutter;

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Convenience constructor: `enz_scroll_column(children)`.
pub fn enz_scroll_column<'a, Message>(
    children: Vec<Element<'a, Message>>,
) -> EnzScrollColumn<'a, Message>
where
    Message: 'a,
{
    EnzScrollColumn {
        children,
        spacing: SpaceToken::Md.px(),
        inset_left: 0.0,
        inset_right: 0.0,
        inset_top: 0.0,
        inset_bottom: 0.0,
        // Off by default. See `DEFAULT_SCROLLBAR_GUTTER`.
        scrollbar_gutter: false,
        max_content_height: None,
        width: Length::Fill,
        height: Length::Fill,
        id: None,
        on_scroll: None,
        initial_offset: None,
        fade_cell: None,
        complete_row_height: None,
    }
}

/// Builder returned by [`enz_scroll_column`].
#[must_use]
pub struct EnzScrollColumn<'a, Message> {
    children: Vec<Element<'a, Message>>,
    spacing: f32,
    inset_left: f32,
    inset_right: f32,
    inset_top: f32,
    inset_bottom: f32,
    /// Reserve a trailing gutter so content clears the scrollbar overlay.
    scrollbar_gutter: bool,
    /// Raise the right inset to at least the gutter when enabled. Opt-in for
    /// composites like ChatTranscript whose content needs scrollbar clearance.
    max_content_height: Option<f32>,
    width: Length,
    height: Length,
    id: Option<Id>,
    on_scroll: Option<Box<dyn Fn(Viewport) -> Message + 'a>>,
    /// First-layout seed forwarded to the composed [`enz_scroll_view`]. See
    /// [`Self::initial_offset`] for the unmount/reopen contract this exists to
    /// serve.
    initial_offset: Option<AbsoluteOffset>,
    /// Vertical edge-fade cell. When `Some`, the scroll column
    /// overlays top/bottom fade gradients on the clipped edges.
    fade_cell: Option<ScrollGeometryCell>,
    /// Optional complete-row viewport policy. The outer widget keeps
    /// its full allocation while the inner scroll viewport uses only the
    /// largest whole-row band; the fractional tail stays quiet.
    complete_row_height: Option<f32>,
}

impl<'a, Message> EnzScrollColumn<'a, Message>
where
    Message: 'a,
{
    /// Set the inter-child spacing from a semantic [`SpaceToken`]
    /// (the named gap: `"xs"`/`"md"`/`"lg"`…). Default [`SpaceToken::Md`].
    pub fn gap(mut self, token: SpaceToken) -> Self {
        self.spacing = token.px();
        self
    }

    /// Set the inter-child spacing in raw pixels (explicit escape hatch).
    ///
    /// Non-expressible input (`NaN`, `±INFINITY`, negative) falls back to the
    /// default gap ([`SpaceToken::Md`] = 12 px); `0.0` is expressible intent
    /// ("no gap") and is honoured literally. The former `s.max(0.0)` let
    /// `+inf` through into `Column` spacing and mapped `NaN` to `0.0`.
    pub fn spacing(mut self, s: f32) -> Self {
        self.spacing = FiniteNonNeg::new_or(s, SpaceToken::Md.px()).get();
        self
    }

    /// Set all four edge insets from a semantic [`SpaceToken`].
    pub fn inset_token(self, token: SpaceToken) -> Self {
        self.inset(token.px())
    }

    /// Set all four edge insets in pixels (default `0.0` = "none").
    ///
    /// Non-expressible input (`NaN`, `±INFINITY`, negative) falls back to the
    /// default inset (`0.0`); `0.0` is honoured literally.
    pub fn inset(self, px: f32) -> Self {
        let px = FiniteNonNeg::new_or(px, 0.0).get();
        self.inset_x(px).inset_y(px)
    }

    /// Set horizontal (left + right) insets in pixels.
    ///
    /// Non-expressible input falls back to the default inset (`0.0`); `0.0`
    /// is honoured literally.
    ///
    /// Left and right come out equal. The opt-in scrollbar gutter, if
    /// enabled, composes as `max(right, gutter)` — so it only ever raises an
    /// inset too small to clear the floating thumb, and never makes a
    /// deliberately-set inset asymmetric.
    pub fn inset_x(mut self, px: f32) -> Self {
        let px = FiniteNonNeg::new_or(px, 0.0).get();
        self.inset_left = px;
        self.inset_right = px;
        self
    }

    /// Set vertical (top + bottom) insets in pixels.
    ///
    /// Non-expressible input falls back to the default inset (`0.0`); `0.0`
    /// is honoured literally.
    pub fn inset_y(mut self, px: f32) -> Self {
        let px = FiniteNonNeg::new_or(px, 0.0).get();
        self.inset_top = px;
        self.inset_bottom = px;
        self
    }

    /// Set the per-edge insets in pixels.
    ///
    /// Each edge's non-expressible input falls back to the default inset
    /// (`0.0`) independently; `0.0` is honoured literally.
    ///
    /// The right inset composes by maximum with the optional scrollbar gutter;
    /// see [`Self::inset_x`].
    pub fn insets(mut self, left: f32, right: f32, top: f32, bottom: f32) -> Self {
        self.inset_left = FiniteNonNeg::new_or(left, 0.0).get();
        self.inset_right = FiniteNonNeg::new_or(right, 0.0).get();
        self.inset_top = FiniteNonNeg::new_or(top, 0.0).get();
        self.inset_bottom = FiniteNonNeg::new_or(bottom, 0.0).get();
        self
    }

    /// Opt in to the trailing scrollbar gutter (default `false`).
    ///
    /// When enabled, the right inset is raised to at least
    /// [`DEFAULT_SCROLLBAR_GUTTER`] px so content clears the floating thumb:
    /// `.inset(20.0).scrollbar_gutter(true)` still yields a right edge of 20,
    /// while `.inset(0.0).scrollbar_gutter(true)` yields 8.
    ///
    /// The gutter is never disabled merely because an explicit inset exists:
    /// `max(4, 8)` still clears the bar. It is also never added to ample
    /// padding: `max(20, 8)` remains 20.
    ///
    /// Reach for this only when content is laid flush to the trailing edge and
    /// would otherwise be overpainted — right-aligned bubbles, end-elided
    /// single lines. Ordinary padded content does not need it.
    pub fn scrollbar_gutter(mut self, on: bool) -> Self {
        self.scrollbar_gutter = on;
        self
    }

    /// Effective padding handed to the inner Column after applying the
    /// scrollbar-gutter policy.
    fn resolved_padding(&self) -> Padding {
        let right = composed_right_inset(self.inset_right, self.scrollbar_gutter);
        Padding {
            top: self.inset_top,
            right,
            bottom: self.inset_bottom,
            left: self.inset_left,
        }
    }

    /// Attach a stable [`Id`] for `Id`-driven scroll control
    /// (`scroll_to`/`scroll_by`/`snap_to`) — e.g. ChatTranscript scroll-to-bottom.
    pub fn id(mut self, id: Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Observe scroll offset changes (delivers the [`Viewport`]).
    pub fn on_scroll(mut self, f: impl Fn(Viewport) -> Message + 'a) -> Self {
        self.on_scroll = Some(Box::new(f));
        self
    }

    /// Seed the internal scrollable's first-layout offset.
    ///
    /// A pure passthrough to
    /// [`EnzScrollView::initial_offset`](crate::scroll_view::EnzScrollView::initial_offset).
    /// Reach for it when the scroll column lives inside a subtree that fully
    /// unmounts (a docked sidebar the shell drops on collapse, a responsive
    /// pane a media query folds away) and the app has already persisted the
    /// offset via [`Self::on_scroll`]. The seed is consumed once per fresh
    /// scrollable state; ordinary gestures and `Id`-driven `scroll_to`
    /// operations own the offset the rest of the time, so passing this every
    /// build is safe.
    pub fn initial_offset(mut self, offset: AbsoluteOffset) -> Self {
        self.initial_offset = Some(offset);
        self
    }

    /// Enable top/bottom edge fade indicators.
    ///
    /// When set, the scroll column overlays short gradients on the edges where
    /// content is clipped, fading the region's background colour over the
    /// content so the user knows there is more to scroll. The cell is read for
    /// the previous frame's geometry and written with the current frame's, so
    /// the fade tracks the live scroll position.
    ///
    /// Only meaningful with a vertically scrolling column (the default axis);
    /// horizontal clipping is not observed.
    pub fn fade_edges(mut self, cell: ScrollGeometryCell) -> Self {
        self.fade_cell = Some(cell);
        self
    }

    /// Cap the outer scroll region height in pixels. Above this cap the
    /// column scrolls; below it, the column hugs its content.
    ///
    /// A `max-content-height` of 0 px means "uncapped"; the equivalent
    /// here is not calling this method (or passing `None` via
    /// [`EnzScrollColumn::max_content_height_opt`]).
    ///
    /// An invalid cap — non-finite (`NaN`, `±INFINITY`) or `<= 0` — is
    /// normalised to `None` at the setter and is indistinguishable from
    /// `None` at every observation point. Zero is *not* expressible
    /// intent here (unlike `spacing(0)`): a cap of `0` would mean
    /// "content may be zero pixels tall", which no caller wants; the doc
    /// comment already calls it uncapped. Rejecting all non-finite values at
    /// the setter keeps them from forcing the viewport to `Length::Shrink`
    /// before the wrapper can filter them out.
    pub fn max_content_height(mut self, cap: f32) -> Self {
        self.max_content_height = sanitize_cap(cap);
        self
    }

    /// Opt-in `Option` variant of [`Self::max_content_height`] for
    /// programmatic use (`None` = uncapped). Same sanitation as
    /// [`Self::max_content_height`].
    pub fn max_content_height_opt(mut self, cap: Option<f32>) -> Self {
        self.max_content_height = cap.and_then(sanitize_cap);
        self
    }

    /// Override the outer width (default `Length::Fill`).
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Override the outer height (default `Length::Fill`). When a
    /// `max_content_height` cap is set, that cap wins over
    /// `Length::Fill`.
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Keep the outer allocation but clip this scroll region to the largest
    /// whole number of fixed-height rows that fits it.
    ///
    /// Use this only when the content has a stable vertical row/line pitch.
    /// The viewport is derived during layout from the space the parent
    /// actually assigns, so callers must not subtract headers, footers, or
    /// shell chrome themselves. Any fractional remainder is a quiet band
    /// after the scroll viewport. Scrolling itself remains pixel-precise: this
    /// policy does not snap wheel deltas or offsets.
    ///
    /// Invalid (`<= 0` or non-finite) values disable the policy, preserving
    /// the normal free-form viewport.
    pub fn complete_rows(mut self, row_height: f32) -> Self {
        self.complete_row_height = sanitize_cap(row_height);
        self
    }

    /// Disable complete-row viewport sizing and use every allocated pixel.
    /// This is the default and is appropriate for variable/free-form content.
    pub fn free_form_viewport(mut self) -> Self {
        self.complete_row_height = None;
        self
    }
}

/// Normalise a requested cap to `Some(finite positive)` or `None` (uncapped).
///
/// Mirrors `max_height::sanitize_cap` — the two must agree, because
/// `MaxHeight` re-runs the same check on whatever it is passed. Sanitising
/// once here means the field is a true "resolved cap" everywhere it is
/// read, so `into_element`'s two consumers (the scroll view's `height` and
/// the wrapper application) cannot disagree on what "has a cap" means.
fn sanitize_cap(cap: f32) -> Option<f32> {
    if cap.is_finite() && cap > 0.0 {
        Some(cap)
    } else {
        None
    }
}

/// Compose the caller's right inset with the opt-in scrollbar gutter.
///
/// Maximum, not sum: when the gutter is on, the right inset is
/// raised to at least [`DEFAULT_SCROLLBAR_GUTTER`]; when it is off (the
/// default) the caller's inset is returned verbatim.
///
/// A maximum keeps a 4 px inset clear of the bar (`max(4, 8) = 8`) without
/// inflating ample padding: a caller with 20 px still gets 20 px on each side.
/// It clears the bar exactly where needed and is the identity everywhere else.
///
/// Extracted so a unit test can pin the composition rule directly, decoupled
/// from layout.
fn composed_right_inset(inset_right: f32, scrollbar_gutter: bool) -> f32 {
    if scrollbar_gutter {
        inset_right.max(DEFAULT_SCROLLBAR_GUTTER)
    } else {
        inset_right
    }
}

impl<'a, Message> EnzScrollColumn<'a, Message>
where
    Message: 'a,
{
    /// Finish the builder and hand back the [`Element`], which is step 3 of
    /// the build pattern every widget in this crate follows (`docs/widgets.md`).
    ///
    /// This forwards to the `From<Self> for Element` impl below, so the two
    /// spellings are one implementation and cannot drift apart. Prefer this
    /// one inside `row![]` / `column![]`, where `.into()` has no target type
    /// to infer.
    #[must_use]
    pub fn into_element(self) -> Element<'a, Message> {
        Element::from(self)
    }
}

impl<'a, Message> From<EnzScrollColumn<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(w: EnzScrollColumn<'a, Message>) -> Self {
        // Scrollbar gutter: opt-in, and composed as a MAXIMUM with the
        // caller's right inset. See `composed_right_inset`.
        let padding = w.resolved_padding();
        let content = Column::with_children(w.children)
            .spacing(w.spacing)
            .padding(padding)
            .width(Length::Fill);

        // Resolve the cap *once* into a single value both consumers below
        // read. The setters already sanitise, but re-checking here is
        // the invariant: `into_element` can never see a Some(NaN) /
        // Some(+inf) / Some(<=0) even if a future setter forgets to filter.
        let cap: Option<f32> = w.max_content_height.and_then(sanitize_cap);

        // Preserve the caller's explicit `.height(...)` verbatim, even when a
        // cap is set. The frame height and scroll-region cap are distinct. A
        // fixed frame with a max-content cap is not a
        // contradiction: the frame is the *widget's* outer height, the cap
        // is the ceiling on the *scroll region* the MaxHeight wrapper
        // imposes on the child's max-height limit. MaxHeight's contract
        // handles all three cases coherently:
        //   * Shrink: hugs content ≤ cap, clamps at cap when tall
        //   * Fill: resolves to `min(parent_max, cap)` under compression
        //   * Fixed(n): honoured verbatim (MaxHeight's documented opt-out
        //     via `Limits::resolve_height` matching Fixed before compression)
        // `MaxHeight` handles the Fill case via its compression flag. We do
        // not introduce a fresh
        // `Fill` here.
        let mut sv = enz_scroll_view(content)
            .axis(Axis::Vertical)
            .width(w.width)
            .height(w.height);
        if let Some(id) = w.id {
            sv = sv.id(id);
        }
        if let Some(cb) = w.on_scroll {
            sv = sv.on_scroll(cb);
        }
        if let Some(offset) = w.initial_offset {
            sv = sv.initial_offset(offset);
        }
        let sv: Element<'a, Message> = sv.into();

        // Vertical edge fades. Read the geometry the *previous* pass
        // stamped to decide which edges clip, then publish this pass's
        // geometry for the next build.
        let sv: Element<'a, Message> = match w.fade_cell {
            None => sv,
            Some(cell) => {
                let edges = cell
                    .get()
                    .map_or(crate::scroll_fade_edge::ScrollFadeEdges::NONE, |g| {
                        crate::scroll_fade_edge::ScrollFadeEdges::from_geometry(&g)
                    });
                let published = crate::scroll_view::publish_geometry_of(sv, cell);
                crate::scroll_fade_edge::scroll_fade_edges(published, edges).into()
            }
        };
        let sv: Element<'a, Message> = match w.complete_row_height.and_then(sanitize_cap) {
            Some(row_height) => CompleteRowViewport::new(sv, row_height).into(),
            None => sv,
        };

        match cap {
            Some(cap) => {
                // `cap` is already guaranteed finite & > 0 by `sanitize_cap`.
                container(MaxHeight::new(sv, cap)).width(w.width).into()
            }
            None => sv,
        }
    }
}

// ---------------------------------------------------------------------------
// Allocation-derived complete-row viewport
// ---------------------------------------------------------------------------

/// Keep the parent-owned allocation while giving the scrollable child only a
/// whole-row viewport. The unused tail is intentionally outside the child's
/// layout bounds, so paint and pointer input cannot leak into it.
struct CompleteRowViewport<'a, Message> {
    child: Element<'a, Message>,
    row_height: f32,
}

impl<'a, Message> CompleteRowViewport<'a, Message> {
    fn new(child: Element<'a, Message>, row_height: f32) -> Self {
        Self { child, row_height }
    }
}

fn complete_row_viewport_height(allocated: f32, row_height: f32) -> f32 {
    if !allocated.is_finite() || allocated <= 0.0 {
        return 0.0;
    }
    if !row_height.is_finite() || row_height <= 0.0 {
        return allocated;
    }
    let rows = (allocated / row_height).floor();
    (rows * row_height).min(allocated)
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for CompleteRowViewport<'_, Message> {
    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Fill)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let outer = limits.resolve(Length::Fill, Length::Fill, Size::ZERO);
        let inner_height = complete_row_viewport_height(outer.height, self.row_height);
        let child_limits = layout::Limits::new(
            Size::new(outer.width, inner_height),
            Size::new(outer.width, inner_height),
        );
        let child =
            self.child
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, &child_limits);
        layout::Node::with_children(outer, vec![child]).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        if let Some(child_layout) = layout.children(&tree.children).next() {
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                child_layout,
                viewport,
                renderer,
                operation,
            );
        }
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
        let Some(child_layout) = layout.children(&tree.children).next() else {
            return;
        };
        let bounds = child_layout.bounds();
        let clip = bounds
            .intersection(viewport)
            .unwrap_or_else(|| Rectangle::new(bounds.position(), Size::ZERO));
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            crate::inert_clip::cursor_within(cursor, clip),
            renderer,
            shell,
            &clip,
        );
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
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
        let Some(child_layout) = layout.children(&tree.children).next() else {
            return;
        };
        let Some(clip) = child_layout.bounds().intersection(viewport) else {
            return;
        };
        renderer.with_layer(clip, |renderer| {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                child_layout,
                crate::inert_clip::cursor_within(cursor, clip),
                &clip,
            );
        });
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let Some(child_layout) = layout.children(&tree.children).next() else {
            return mouse::Interaction::default();
        };
        let bounds = child_layout.bounds();
        let clip = bounds
            .intersection(viewport)
            .unwrap_or_else(|| Rectangle::new(bounds.position(), Size::ZERO));
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            child_layout,
            crate::inert_clip::cursor_within(cursor, clip),
            &clip,
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
        let Some(child_layout) = layout.children(&tree.children).next() else {
            return Vec::new();
        };
        let Some(clip) = child_layout.bounds().intersection(viewport) else {
            return Vec::new();
        };
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            child_layout,
            renderer,
            &clip,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<CompleteRowViewport<'a, Message>> for Element<'a, Message> {
    fn from(value: CompleteRowViewport<'a, Message>) -> Self {
        Element::new(value)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
