//! Shared width-measured compact presentation for search-like fields.
//!
//! [`crate::search_field::EnzSearchField`] and
//! [`crate::autocomplete_field::EnzAutocompleteField`] deliberately consume
//! this one retained-state wrapper. It owns the usability-width
//! threshold, compact toolbar button, active-query indication, focus routing
//! through the hidden input, and Escape restoration. The consumer supplies the
//! expanded child, so autocomplete keeps its editable-combobox semantics and
//! suggestion overlay without copying any responsive state machine.
//!
//! The widget owns the open/closed bit in its retained tree state. No app
//! message or reducer arm is needed, which lets responsive collapse ship by
//! default for every existing caller.

use enzui_core::collect::SemanticsNode;
use enzui_core::focus::{FocusState, activation_from_key};
use enzui_core::semantics::Semantics;
use enzui_theme::Palette;
use iced::advanced::layout::Measure as _;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Id, Operation, Tree, operation, tree};
use iced::advanced::{Shell, Widget, mouse, overlay, renderer};
use iced::{
    Background, Border, Color, Element, Event, Length, Padding, Point, Rectangle, Size, Vector,
    touch,
};

use super::SearchPresentation;
use crate::text_field::TextFieldSize;

/// Native accessible shortcut carried by the expanded field and compact
/// toolbar button unless a consumer overrides it.
pub(crate) const DEFAULT_SHORTCUT: &str = "Ctrl+F";

/// Minimum useful visible query span before responsive search collapses.
pub(crate) const MIN_VISIBLE_QUERY_CHARACTERS: usize = 6;

const USABLE_QUERY_PROBE: &str = "MMMMMM";

pub(crate) fn icon_size_for(size: crate::text_field::TextFieldSize) -> f32 {
    enzui_theme::text_scale::ui_px(match size {
        crate::text_field::TextFieldSize::Sm => 12.0,
        crate::text_field::TextFieldSize::Md => crate::icon::ICON_SM,
        crate::text_field::TextFieldSize::Lg => 18.0,
    })
}

/// Floating-point tolerance at the measured compact boundary.
const WIDTH_EPSILON: f32 = 0.01;

/// Accessible name of the result surface, at both rungs.
///
/// The collapsed popover and the wide anchored dropdown are the same overlay
/// with the same card chrome; only the anchor and whether the field travels
/// with it differ. Naming them identically is deliberate: a caller (or an AT
/// user) that learns the surface at one rung meets the same object at the
/// other.
pub(crate) const RESULT_SURFACE_LABEL: &str = "Search results";

pub(crate) struct ResultSurface<'a, Message> {
    pub(crate) filters: Option<Element<'a, Message>>,
    pub(crate) results: Option<Element<'a, Message>>,
    /// `None` = the widget's own rule (open while the field is focused, or
    /// while the collapsed popover is open). `Some(_)` = caller-controlled.
    pub(crate) open: Option<bool>,
    /// Publication of the effective open state, so a consumer never has to
    /// re-derive it from a flag the widget does not set.
    pub(crate) on_open: Option<Box<dyn Fn(bool) -> Message + 'a>>,
}

impl<Message> ResultSurface<'_, Message> {
    /// The no-slots case: `enz_autocomplete_field` owns its own popup and
    /// supplies neither slot, so no surface is added to it.
    pub(crate) fn none() -> Self {
        Self {
            filters: None,
            results: None,
            open: None,
            on_open: None,
        }
    }
}

/// Shared intrinsic usability probe: search glyph + six wide query characters
/// + clear slot + the consumer's field padding.
///
/// Placeholder prose is deliberately absent; fitting a placeholder is not the
/// test for whether a query remains editable.
pub(crate) fn usability_probe<'a, Message: 'a>(
    icon_size: f32,
    text_size: f32,
    padding_x: f32,
    height: f32,
) -> Element<'a, Message> {
    debug_assert_eq!(
        MIN_VISIBLE_QUERY_CHARACTERS,
        USABLE_QUERY_PROBE.len(),
        "the probe string is the minimum visible query"
    );
    iced::widget::container(
        iced::widget::Row::new()
            .spacing(6.0)
            .align_y(iced::alignment::Vertical::Center)
            .push(crate::icon::enz_icon("action.search").size(icon_size))
            .push(
                iced::widget::text(USABLE_QUERY_PROBE)
                    .size(text_size)
                    .wrapping(iced::widget::text::Wrapping::None),
            )
            .push(crate::icon::enz_icon("action.close").size(icon_size)),
    )
    .padding(Padding::from([0.0, padding_x]))
    .height(height)
    .width(Length::Shrink)
    .into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PendingFocus {
    #[default]
    None,
    Field,
    Trigger,
}

/// The narrowest slot an inline search expands into; in a narrower one it
/// opens as a popover, so a squeezed titlebar never shows a sliver of a field.
const MIN_INLINE_FIELD_WIDTH: f32 = 120.0;

#[derive(Debug, Default)]
struct State {
    compact: bool,
    /// Compact in a slot too narrow for a usable field: an inline search
    /// opens as a popover instead of expanding into it.
    cramped: bool,
    open: bool,
    pressed: bool,
    pressed_finger: Option<touch::Finger>,
    activate_pending: bool,
    trigger_focus: FocusState,
    pending_focus: PendingFocus,
    retained_caret: Option<crate::focus_retention::RetainedCursor>,
    // Routed focus can precede layout; operations must use the child that was laid out.
    layout_had_field: bool,
    // Escape preserves focus; typing can reopen widget-owned results.
    results_dismissed: bool,
    outside_dismissed: bool,
    observed_results_open: Option<bool>,
    published_results_open: bool,
}

/// Selects the full field or a compact search trigger from the width assigned
/// by the parent. Child 0 is the real field, child 1 the decorative icon, child
/// 2 an intrinsic measurement probe.
pub(crate) struct ResponsiveSearch<'a, Message> {
    children: Vec<Element<'a, Message>>,
    width: Length,
    enabled: bool,
    responsive: bool,
    active_query: bool,
    value: String,
    field_id: Id,
    trigger_id: Id,
    trigger_semantics: Semantics,
    palette: Option<Palette>,
    size: TextFieldSize,
    radius: Option<f32>,
    presentation: SearchPresentation,
    filters_index: Option<usize>,
    results_index: Option<usize>,
    results_open: Option<bool>,
    on_results_open: Option<Box<dyn Fn(bool) -> Message + 'a>>,
}

impl<'a, Message: 'a> ResponsiveSearch<'a, Message> {
    #[allow(
        clippy::too_many_arguments,
        reason = "the field's slots and their callbacks"
    )]
    pub(crate) fn new(
        field: Element<'a, Message>,
        icon: Element<'a, Message>,
        probe: Element<'a, Message>,
        width: Length,
        enabled: bool,
        responsive: bool,
        active_query: bool,
        value: String,
        field_id: Id,
        trigger_id: Id,
        trigger_semantics: Semantics,
        palette: Option<Palette>,
        size: TextFieldSize,
        radius: Option<f32>,
        presentation: SearchPresentation,
        surface: ResultSurface<'a, Message>,
    ) -> Self {
        let ResultSurface {
            filters,
            results,
            open: results_open,
            on_open: on_results_open,
        } = surface;
        let mut children = vec![field, icon, probe];
        let filters_index = filters.map(|filters| {
            let index = children.len();
            children.push(filters);
            index
        });
        let results_index = results.map(|results| {
            let index = children.len();
            children.push(results);
            index
        });
        Self {
            children,
            width,
            enabled,
            responsive,
            active_query,
            value,
            field_id,
            trigger_id,
            trigger_semantics,
            palette,
            size,
            radius,
            presentation,
            filters_index,
            results_index,
            results_open,
            on_results_open,
        }
    }

    /// Whether the caller supplied anything for the result surface to carry.
    ///
    /// An empty surface is never published. A dropdown that
    /// always renders would satisfy "results are reachable" while showing the
    /// user an empty card under every search field in apps.
    fn has_result_surface(&self) -> bool {
        self.filters_index.is_some() || self.results_index.is_some()
    }

    /// Does the in-flow field currently hold focus?
    ///
    /// Probed from the field's own widget tree rather than tracked in local
    /// state: focus arrives through routed operations (`focus(id)`, `Tab`,
    /// `focus_next`) that never pass through this widget's `update`, so a
    /// locally cached bit would go stale in exactly the case that matters.
    fn field_focused(
        &mut self,
        tree: &mut Tree,
        field_layout: Layout,
        renderer: &iced::Renderer,
    ) -> bool {
        let mut find = operation::focusable::find_focused();
        let mut boxed: Box<dyn Operation<()>> = Box::new(operation::black_box(&mut find));
        self.children[0].as_widget_mut().operate(
            &mut tree.children[0],
            field_layout,
            &field_layout.bounds(),
            renderer,
            &mut *boxed,
        );
        drop(boxed);
        matches!(find.finish(), operation::Outcome::Some(_))
    }

    /// The effective open state of the result surface, at every rung.
    ///
    /// One function so the collapsed popover and the wide anchored dropdown
    /// cannot disagree, and so the value published to the caller is the value
    /// that decides what is drawn.
    ///
    /// * compact + open + `Popover` — the popover carries the slots; open.
    /// * field in flow (wide, or compact-open `Inline`) — the caller's
    ///   `results_open` when given, else focused-and-not-dismissed.
    /// * compact + closed — nothing: the slots live behind the trigger, and
    ///   presenting them next to a square icon would be a dropdown with no
    ///   visible owner.
    fn result_surface_open(
        &mut self,
        tree: &mut Tree,
        field_layout: Option<Layout>,
        renderer: &iced::Renderer,
    ) -> bool {
        if !self.has_result_surface() || !self.enabled {
            return false;
        }
        let (compact, open, had_field) = {
            let s = tree.state.downcast_ref::<State>();
            (s.compact, s.open, s.layout_had_field)
        };
        if compact && open && self.pops_over(tree.state.downcast_ref::<State>()) {
            return true;
        }
        if !had_field {
            return false;
        }
        if tree.state.downcast_ref::<State>().outside_dismissed {
            let focused =
                field_layout.is_some_and(|layout| self.field_focused(tree, layout, renderer));
            if focused {
                let state = tree.state.downcast_mut::<State>();
                state.outside_dismissed = false;
                state.results_dismissed = false;
            } else {
                return false;
            }
        }
        match self.results_open {
            Some(open) => open,
            None => {
                let focused =
                    field_layout.is_some_and(|layout| self.field_focused(tree, layout, renderer));
                let dismissed = tree.state.downcast_ref::<State>().results_dismissed;
                if !focused && dismissed {
                    // Leaving the field ends the dismissal: coming back to a
                    // search field the user left should offer results again,
                    // not stay silently suppressed by an Escape from an
                    // earlier visit.
                    tree.state.downcast_mut::<State>().results_dismissed = false;
                }
                focused && !dismissed
            }
        }
    }

    /// The presentation in force: an inline search in a slot too narrow for a
    /// usable field opens as a popover.
    fn presentation_in(&self, state: &State) -> SearchPresentation {
        if self.presentation == SearchPresentation::Inline && state.cramped {
            SearchPresentation::Popover
        } else {
            self.presentation
        }
    }

    /// Whether an open compact search shows its field in a popover, either
    /// kind.
    fn pops_over(&self, state: &State) -> bool {
        matches!(
            self.presentation_in(state),
            SearchPresentation::Popover | SearchPresentation::AnchoredPopover
        )
    }

    fn field_in_flow(&self, state: &State) -> bool {
        !state.compact || (state.open && self.presentation_in(state) == SearchPresentation::Inline)
    }

    fn active_index(&self, state: &State) -> usize {
        usize::from(!self.field_in_flow(state))
    }

    // Compact search is an icon-action surface, not a field with its text
    // removed. The titlebar row authority owns its preferred diameter; its
    // resolver applies the independent global hit-target floor afterwards.
    // The field-size ladder still owns inline search/clear glyphs and the
    // usability probe, but a collapsed trigger is a titlebar affordance and
    // therefore uses the shared icon scale.
    pub(crate) fn compact_control_size() -> f32 {
        crate::toolbar_icon::TitlebarAffordanceTokens::DEFAULT.effective_control_size()
    }

    // The responsive widget keeps its assigned width so collapse does not
    // change a titlebar row's allocation. The compact action occupies only a
    // toolbar-sized square at that slot's trailing edge.
    fn compact_bounds(layout: Layout) -> Rectangle {
        let outer = layout.bounds();
        let side = Self::compact_control_size()
            .min(outer.width)
            .min(outer.height);
        Rectangle {
            x: outer.x + outer.width - side,
            y: outer.y + (outer.height - side) / 2.0,
            width: side,
            height: side,
        }
    }

    /// The child slot the last `layout()` pass actually filled.
    /// Read by `overlay`, `draw`, `mouse_interaction` and `update` so those
    /// paths keep matching the layout `iced` handed them even when `operate`
    /// mid-frame flipped `state.open` in response to a routed focus. Using
    /// `active_index(state)` here would panic the child widgets underneath
    /// with `layout.children().next().unwrap()` on a mismatched slot.
    fn layout_index(state: &State) -> usize {
        usize::from(!state.layout_had_field)
    }

    fn operate_field(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation<()>,
    ) {
        let mut op: Box<dyn Operation> = Box::new(operation::black_box(operation));
        self.children[0].as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            &layout.bounds(),
            renderer,
            &mut *op,
        );
    }

    fn open(&mut self, tree: &mut Tree, shell: &mut Shell<'_, Message>) {
        let state = tree.state.downcast_mut::<State>();
        if state.compact && self.enabled {
            state.open = true;
            state.outside_dismissed = false;
            state.pressed = false;
            state.pressed_finger = None;
            state.pending_focus = PendingFocus::Field;
            shell.invalidate_layout();
            shell.request_redraw();
            shell.capture_event();
        }
    }

    fn close(&mut self, tree: &mut Tree, shell: &mut Shell<'_, Message>) {
        let state = tree.state.downcast_mut::<State>();
        state.open = false;
        state.pending_focus = PendingFocus::Trigger;
        shell.invalidate_layout();
        shell.request_redraw();
        shell.capture_event();
    }
}

impl<'a, Message: 'a> Widget<Message, iced::Theme, iced::Renderer>
    for ResponsiveSearch<'a, Message>
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<State>();
        if self.results_open == Some(true) && state.observed_results_open != Some(true) {
            state.outside_dismissed = false;
        }
        state.observed_results_open = self.results_open;
        if !self.enabled {
            let state = tree.state.downcast_mut::<State>();
            state.open = false;
            state.trigger_focus.unfocus();
        }
        tree.diff_children(&mut self.children);
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, Length::Shrink)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        // The threshold is the real intrinsic width of the usability probe —
        // search glyph + six wide query characters + clear slot + this size's
        // padding. It is deliberately not a window-width constant and changes
        // with font metrics, scale, and field size. It deliberately does not
        // change with placeholder length: placeholder ellipsis is a separate
        // concern, and fitting all prose is not the test for whether input is
        // still useful.
        let probe_limits = layout::Limits::new(
            Size::ZERO,
            Size::new(f32::INFINITY, self.size.tokens().height),
        );
        let probe = self.children[2].as_widget_mut().measure(
            &mut tree.children[2],
            renderer,
            &probe_limits,
        );
        let available = limits.width(self.width).max().width;
        // A zero allotment is the most impossible one: it collapses too, so
        // the trigger keeps its square and overhangs where the band can see
        // and clip it, instead of a zero-width field that reports nothing.
        let compact = self.responsive
            && available.is_finite()
            && available + WIDTH_EPSILON < probe.size().width;
        // An unbounded offer is a parent measuring intrinsic width (a band's
        // or action strip's shrink probe), not an allotment. It measures the
        // field and leaves the presentation state alone: letting it record
        // "not compact" closed a popover the user had just opened.
        let measuring = !limits.max().width.is_finite();

        if !measuring {
            let state = tree.state.downcast_mut::<State>();
            if !compact {
                state.open = false;
            }
            state.compact = compact;
            state.cramped = compact && available < MIN_INLINE_FIELD_WIDTH;
        }

        let active = if measuring {
            0
        } else {
            self.active_index(tree.state.downcast_ref::<State>())
        };
        let control_size = Self::compact_control_size();
        let child_limits = if active == 1 {
            // A max-only square lets the SVG honour its own `Fixed(icon_size)`.
            // Pinning both minima to the control size enlarged 12px artwork to
            // the full 26/32px field height.
            layout::Limits::new(Size::ZERO, Size::new(control_size, control_size))
        } else {
            limits.width(self.width).height(Length::Shrink)
        };
        let mut child = self.children[active].as_widget_mut().measure(
            &mut tree.children[active],
            renderer,
            &child_limits,
        );

        // Focus transitions happen after the new presentation has a concrete
        // node and before that frame can be drawn. Opening therefore needs no
        // second click; Escape closes and leaves the icon as the one focus
        // owner, not an empty focus ring.
        let pending = if measuring {
            PendingFocus::None
        } else {
            tree.state.downcast_ref::<State>().pending_focus
        };
        match pending {
            PendingFocus::Field if active == 0 => {
                tree.state.downcast_mut::<State>().pending_focus = PendingFocus::None;
                tree.state.downcast_mut::<State>().trigger_focus.unfocus();
                let mut focus = operation::focusable::focus::<()>(self.field_id.clone());
                self.operate_field(tree, Layout::of(&child), renderer, &mut focus);
                // `focus(id)` moves iced's caret to the end. Collapse must be
                // presentation-only, so put the exact byte index/range
                // captured before hiding back after focus.
                if let Some(caret) = tree.state.downcast_ref::<State>().retained_caret {
                    match caret {
                        crate::focus_retention::RetainedCursor::Index(position) => {
                            let mut cursor = operation::text_input::move_cursor_to::<()>(
                                self.field_id.clone(),
                                crate::focus_retention::line_position(position),
                            );
                            self.operate_field(tree, Layout::of(&child), renderer, &mut cursor);
                        }
                        crate::focus_retention::RetainedCursor::Selection { start, end } => {
                            let mut selection = operation::text_input::select_range::<()>(
                                self.field_id.clone(),
                                crate::focus_retention::line_position(start),
                                crate::focus_retention::line_position(end),
                            );
                            self.operate_field(tree, Layout::of(&child), renderer, &mut selection);
                        }
                    }
                }
            }
            PendingFocus::Trigger if active == 1 => {
                tree.state.downcast_mut::<State>().pending_focus = PendingFocus::None;
                // The hidden field may still carry its local focused bit. It is
                // not traversed while compact, but clear it before the trigger
                // becomes the focus owner so reopening cannot expose two.
                let hidden = self.children[0].as_widget_mut().measure(
                    &mut tree.children[0],
                    renderer,
                    &limits.width(self.width).height(Length::Shrink),
                );
                let mut unfocus = operation::focusable::unfocus::<()>();
                self.operate_field(tree, Layout::of(&hidden), renderer, &mut unfocus);
                tree.state.downcast_mut::<State>().trigger_focus.focus();
            }
            _ => {}
        }

        // Record what this layout actually put in the child slot so
        // `operate` can tell whether `child_layout` matches the field's tree
        // or the trigger's icon. `active_index` reflects the current
        // state.open, which may drift before the next layout runs.
        if !measuring {
            tree.state.downcast_mut::<State>().layout_had_field = active == 0;
        }

        let node = if active == 1 {
            let resolved = limits.resolve(
                self.width,
                Length::Fixed(control_size),
                Size::new(control_size, control_size),
            );
            // The compact presentation is the usable fallback for an already
            // impossible allotment. Preserve the toolbar/min-hit square even
            // when a hostile parent offers less; shrinking it to that offer
            // would recreate a glyph-sized click target while making the
            // layout-spill scan look superficially clean.
            let outer = Size::new(
                resolved.width.max(control_size),
                resolved.height.max(control_size),
            );
            let child_size = child.size();
            child = child.move_to(Point::new(
                (outer.width - control_size).max(0.0)
                    + ((control_size - child_size.width) / 2.0).max(0.0),
                ((outer.height - child_size.height) / 2.0).max(0.0),
            ));
            layout::Node::with_children(outer, vec![child])
        } else if !compact {
            let floor = if self.responsive {
                probe.size().width
            } else {
                0.0
            };
            let node = layout::frame(limits, self.size(), |_| child.clone());
            let node = match node.with_reflow_min_width(floor) {
                Ok(node) => node,
                Err(_) => layout::Node::with_children(child.size(), vec![child.clone()]),
            };
            // Measured, a responsive field reports at least the square it
            // collapses to: it never occupies less, and a strip planning its
            // commands around it must leave that room.
            if measuring && self.responsive && node.size().width < control_size {
                layout::Node::with_children(
                    Size::new(control_size, node.size().height),
                    vec![child],
                )
            } else {
                node
            }
        } else {
            layout::Node::with_children(child.size(), vec![child])
        };
        // Only the active branch is placed; the others stay laid out.
        node.with_placed([active]).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let (compact, open, layout_had_field) = {
            let s = tree.state.downcast_ref::<State>();
            (s.compact, s.open, s.layout_had_field)
        };
        let child_layout = layout.child(
            &tree.children,
            Self::layout_index(tree.state.downcast_ref::<State>()),
        );

        // Fast path: layout put the field in the child slot and current state
        // still wants it there. `child_layout` matches the field's widget tree.
        if layout_had_field && (!compact || open) {
            self.children[0].as_widget_mut().operate(
                &mut tree.children[0],
                child_layout,
                viewport,
                renderer,
                operation,
            );
            return;
        }

        // From here on, `child_layout` is the trigger icon's layout (either
        // because compact-and-closed at layout time, or because a routed focus
        // set `state.open = true` between the last layout and now). Anything
        // that touches the field must use a synthesized hidden layout.
        //
        // Only publish the trigger's semantics/focus when the trigger is still
        // the currently-displayed surface. If `state.open` transitioned to
        // true mid-frame the trigger is going away and its accessible node
        // must not linger in the AT tree.
        if !open {
            let trigger_bounds = Self::compact_bounds(layout);
            let state = tree.state.downcast_mut::<State>();
            if self.enabled {
                state
                    .trigger_focus
                    .operate(operation, Some(&self.trigger_id), trigger_bounds);
            } else {
                state.trigger_focus.unfocus();
            }
            let live = self
                .trigger_semantics
                .clone()
                .focused(state.trigger_focus.is_focused())
                .disabled(!self.enabled)
                .selected(self.active_query);
            let mut node = SemanticsNode::new(live);
            operation.custom(Some(&self.trigger_id), trigger_bounds, &mut node);

            if self.enabled {
                let mut request =
                    crate::activation::ActivationRequest::new(self.trigger_id.clone());
                operation.custom(Some(&self.trigger_id), layout.bounds(), &mut request);
                if request.armed() {
                    state.activate_pending = true;
                }
            }
            operation.container(Some(&self.trigger_id), trigger_bounds, viewport);
            operation.traverse(&mut |operation| {
                self.children[1].as_widget_mut().operate(
                    &mut tree.children[1],
                    child_layout,
                    viewport,
                    renderer,
                    operation,
                );
            });
        }

        // A disabled field publishes no focus target and cannot receive one.
        if !self.enabled {
            return;
        }

        // Once a Popover is open, its overlay is the sole owner of the field's
        // operations and semantics. Publishing the synthesized hidden layout
        // here as well would announce the field at the trigger's in-flow y=0
        // and then announce it again at its painted overlay bounds.
        if open && self.pops_over(tree.state.downcast_ref::<State>()) {
            return;
        }

        // Make the collapsed field addressable through routed focus.
        //
        // At narrow widths the expanded input is replaced by a search icon. If
        // the input node vanished from `operate` with it, a caller's
        // `text_input::focus(id)` / `focus(id)` / `find_focused()` would walk
        // the tree, never see the field, and return `None` — so every app that
        // publishes the field's id (every framework app that consumes it
        // as a routed-focus target) would break silently,
        // because the id no longer names anything a routed focus can land on.
        //
        // The response has two halves and they run in one `operate` pass so
        // the caller's follow-up read (usually `find_focused`, sometimes
        // `SemanticsCollector`) sees a coherent tree:
        //
        //   1. Lay out the field hidden — at the same width the parent
        //      assigned this widget as a whole — and expose only its focusable
        //      / text-input operate hooks through a filtering proxy. Custom
        //      nodes (`TextFieldSemantics`), container/scrollable calls and
        //      arbitrary text stay dropped so an unrelated caller that
        //      enumerates semantics (`SemanticsCollector`, role counts) still
        //      sees only the collapsed icon while the field is not open. The
        //      proxy's inner op is untouched so its own state machine
        //      (`focus(id)`'s target, `find_focused`'s captured id, etc.)
        //      keeps working exactly as if the field had been the only child.
        //
        //   2. Probe the field's own focus state after the proxy pass. If the
        //      inner text input reports focused, the routed operation was
        //      indeed one that pointed here — so open synchronously: flip
        //      `state.open`, drop the trigger's focus bit, and let the next
        //      layout render the expanded field. The already-focused input
        //      keeps its caret, the same rule the focus retention seam pins
        //      for rebuilds.
        //
        // The trigger publish above and this field publish sit under separate
        // ids, so nothing is double-counted in the AT tree — the trigger's
        // Button and the field's TextInput are the two presentations of the
        // same logical widget, exactly one active at a time.
        // The hidden child may be taller than the compact button: an
        // autocomplete can include a visible label, validation line, and a
        // rich two-line suggestion overlay once opened. Give focus-only layout
        // unbounded height so routing to its input cannot collapse the inner
        // field merely because the visible trigger is one square button. The
        // popup remains an overlay owned by the expanded autocomplete child.
        let hidden_limits =
            layout::Limits::new(Size::ZERO, Size::new(f32::INFINITY, f32::INFINITY))
                .width(self.width)
                .height(Length::Shrink);
        let hidden_node = self.children[0].as_widget_mut().measure(
            &mut tree.children[0],
            renderer,
            &hidden_limits,
        );
        let hidden_layout = Layout::of(&hidden_node);

        if open {
            // Compact-open with stale `child_layout`: forward the full
            // operation through the synthesized layout so semantics reads,
            // focus reads, and any other walk see one coherent field surface
            // in the frame between the routed focus and the next layout.
            self.children[0].as_widget_mut().operate(
                &mut tree.children[0],
                hidden_layout,
                viewport,
                renderer,
                operation,
            );
            return;
        }

        // Compact-closed, field currently hidden: forward focus-only ops so
        // routed focus reaches the input while the icon remains the only
        // AT-visible surface.
        {
            let mut proxy = focus_only_proxy::FocusOnly::new(operation);
            self.children[0].as_widget_mut().operate(
                &mut tree.children[0],
                hidden_layout,
                viewport,
                renderer,
                &mut proxy,
            );
        }

        // Probe: has the inner input become focused? A routed `focus(id)` /
        // `text_input::focus(id)` matching the field id will have set its
        // Focusable state to focused inside the proxy pass. `find_focused`
        // returning `Some(_)` is the coincident signal, and its `id` will be
        // the input's probe id — which equals `self.field_id`.
        let mut find = operation::focusable::find_focused();
        let mut boxed: Box<dyn Operation<()>> = Box::new(operation::black_box(&mut find));
        self.children[0].as_widget_mut().operate(
            &mut tree.children[0],
            hidden_layout,
            viewport,
            renderer,
            &mut *boxed,
        );
        drop(boxed);
        if matches!(find.finish(), operation::Outcome::Some(_)) {
            let s = tree.state.downcast_mut::<State>();
            s.open = true;
            s.trigger_focus.unfocus();
            // Deliberately no `PendingFocus::Field`: the input already carries
            // focus and re-issuing `focus(id)` in `layout` would move iced's
            // caret to the end, undoing any exact caret the caller placed.
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
        // A key press that is not Escape re-opens a result surface the
        // user dismissed — typing after Escape must show results again, the
        // way mail's `SearchEdited` re-opens its dropdown. Run before dispatch
        // so the same event that produces the edit also clears the bit.
        if matches!(
            event,
            Event::Keyboard(iced::keyboard::Event::KeyPressed { key, .. })
                if !matches!(key, iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape))
        ) && tree.state.downcast_ref::<State>().results_dismissed
        {
            tree.state.downcast_mut::<State>().results_dismissed = false;
            shell.invalidate_layout();
        }

        self.dispatch_update(tree, event, layout, cursor, renderer, shell, viewport);

        // Publish the effective open state on its edges. Without this every
        // consumer re-derives the surface's state from a flag the widget does
        // not set, which kills Arrow/Enter navigation at the collapsed
        // popover: the caller's `search_open` stays false for the whole
        // interaction, so its reducer's Up/Down/Enter arms are inert while the
        // popover is on screen.
        if self.on_results_open.is_some() {
            let field_layout = tree
                .state
                .downcast_ref::<State>()
                .layout_had_field
                .then(|| {
                    layout.child(
                        &tree.children,
                        Self::layout_index(tree.state.downcast_ref::<State>()),
                    )
                });
            let open = self.result_surface_open(tree, field_layout, renderer);
            let published = tree.state.downcast_ref::<State>().published_results_open;
            if open != published {
                tree.state.downcast_mut::<State>().published_results_open = open;
                if let Some(on_open) = self.on_results_open.as_ref() {
                    shell.publish(on_open(open));
                }
                shell.request_redraw();
            }
        }
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
        self.draw_inner(tree, renderer, theme, style, layout, cursor, viewport);
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        // mouse_interaction reads `layout_index` too.
        let active = Self::layout_index(tree.state.downcast_ref::<State>());
        if active == 1 && self.enabled && cursor.is_over(Self::compact_bounds(layout)) {
            mouse::Interaction::Pointer
        } else {
            let child_layout = layout.child(
                &tree.children,
                Self::layout_index(tree.state.downcast_ref::<State>()),
            );
            self.children[active].as_widget().mouse_interaction(
                &tree.children[active],
                child_layout,
                cursor,
                viewport,
                renderer,
            )
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: iced::Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        // A compact Popover leaves the square trigger in-flow and transports
        // the field plus optional slots through one overlay. The anchor is the
        // painted trigger bounds in viewport space; viewport centring is
        // resolved by core placement, never app arithmetic.
        let popover_open = {
            let state = tree.state.downcast_ref::<State>();
            state.compact && state.open && self.pops_over(state) && !state.layout_had_field
        };
        if popover_open {
            let anchor = Self::compact_bounds(layout) + translation;
            // A cramped inline search opens as the ordinary popover.
            let presentation = self.presentation_in(tree.state.downcast_ref::<State>());
            let Tree {
                state,
                children: tree_children,
                ..
            } = tree;
            return vec![crate::overlay_adapter::element(
                SearchPopoverOverlay {
                    children: &mut self.children,
                    trees: tree_children,
                    state: state.downcast_mut::<State>(),
                    anchor,
                    field_id: self.field_id.clone(),
                    carries_field: true,
                    base_field_layout: None,
                    size: self.size,
                    on_open: self.on_results_open.as_deref(),
                    filters_index: self.filters_index,
                    results_index: self.results_index,
                    palette: self.palette.clone(),
                    radius: self.radius,
                    presentation,
                    index: enzui_core::overlay_layer::OverlayLayer::Popup.z_index(),
                },
                renderer,
                window,
            )];
        }

        // The wide rung. The field stays in flow and the same overlay
        // carries the filter/result slots, anchored under the field — the
        // presentation an app's `anchored_overlay` would otherwise hand-roll.
        // Without this branch the slots are transported by the popover alone,
        // so a caller that passes `.results(..)` to a field wide enough to
        // stay inline gets nothing laid out, drawn or traversed.
        let child_layout = layout.child(
            &tree.children,
            Self::layout_index(tree.state.downcast_ref::<State>()),
        );
        if self.result_surface_open(tree, Some(child_layout), renderer) {
            let anchor = child_layout.bounds() + translation;
            let Tree {
                state,
                children: tree_children,
                ..
            } = tree;
            return vec![crate::overlay_adapter::element(
                SearchPopoverOverlay {
                    children: &mut self.children,
                    trees: tree_children,
                    state: state.downcast_mut::<State>(),
                    anchor,
                    field_id: self.field_id.clone(),
                    carries_field: false,
                    base_field_layout: Some(child_layout),
                    size: self.size,
                    on_open: self.on_results_open.as_deref(),
                    filters_index: self.filters_index,
                    results_index: self.results_index,
                    palette: self.palette.clone(),
                    radius: self.radius,
                    presentation: self.presentation,
                    index: enzui_core::overlay_layer::OverlayLayer::Popup.z_index(),
                },
                renderer,
                window,
            )];
        }

        // Otherwise delegate to the child matching the last layout.
        let active = Self::layout_index(tree.state.downcast_ref::<State>());
        let child_layout = layout.child(
            &tree.children,
            Self::layout_index(tree.state.downcast_ref::<State>()),
        );
        self.children[active].as_widget_mut().overlay(
            &mut tree.children[active],
            child_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> ResponsiveSearch<'a, Message> {
    #[allow(
        clippy::too_many_arguments,
        reason = "the widget-event arguments iced passes"
    )]
    fn dispatch_update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // `update` runs against `layout` — use `layout_index` to
        // dispatch to the child the layout actually filled, even if `operate`
        // flipped `state.open` between the last layout and this event pass.
        let active = Self::layout_index(tree.state.downcast_ref::<State>());
        let child_layout = layout.child(
            &tree.children,
            Self::layout_index(tree.state.downcast_ref::<State>()),
        );

        // Popover keeps the trigger in-flow, so Escape cannot be delegated to
        // the field child as Inline does. Capture it at the retained wrapper;
        // the overlay repeats this guard because iced may dispatch to the
        // overlay before the base tree.
        let popover_open = {
            let state = tree.state.downcast_ref::<State>();
            state.compact && state.open && self.pops_over(state)
        };
        if popover_open
            && matches!(
                event,
                Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                    ..
                })
            )
        {
            tree.state.downcast_mut::<State>().retained_caret =
                crate::focus_retention::focused_retained_cursor(&tree.children[0], &self.value);
            self.close(tree, shell);
            return;
        }

        if active == 0 {
            let compact_open = {
                let state = tree.state.downcast_ref::<State>();
                state.compact && state.open
            };
            if compact_open
                && matches!(
                    event,
                    Event::Keyboard(iced::keyboard::Event::KeyPressed {
                        key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                        ..
                    })
                )
            {
                tree.state.downcast_mut::<State>().retained_caret =
                    crate::focus_retention::focused_retained_cursor(&tree.children[0], &self.value);
                self.close(tree, shell);
                return;
            }
            self.children[0].as_widget_mut().update(
                &mut tree.children[0],
                event,
                child_layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
            return;
        }

        // Pointer focus is exclusive: a press that lands on another control
        // takes the trigger's focus with it, enabled or not.
        if let Some(enzui_core::focus::PressFocus::Drop) =
            enzui_core::focus::press_focus_from_cursor(event, cursor, Self::compact_bounds(layout))
        {
            let state = tree.state.downcast_mut::<State>();
            if state.trigger_focus.is_focused() {
                state.trigger_focus.unfocus();
                shell.request_redraw();
            }
        }

        self.children[1].as_widget_mut().update(
            &mut tree.children[1],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        if !self.enabled {
            return;
        }

        let bounds = Self::compact_bounds(layout);
        let over = cursor.is_over(bounds);
        let activate_pending =
            std::mem::take(&mut tree.state.downcast_mut::<State>().activate_pending);
        if activate_pending {
            self.open(tree, shell);
            return;
        }

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if over => {
                let state = tree.state.downcast_mut::<State>();
                state.pressed = true;
                state.trigger_focus.focus();
                shell.capture_event();
                shell.request_redraw();
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let pressed = std::mem::take(&mut tree.state.downcast_mut::<State>().pressed);
                if pressed && over {
                    self.open(tree, shell);
                }
            }
            Event::Touch(touch::Event::FingerPressed { id, position })
                if bounds.contains(*position) =>
            {
                let state = tree.state.downcast_mut::<State>();
                state.pressed = true;
                state.pressed_finger = Some(*id);
                state.trigger_focus.focus();
                shell.capture_event();
            }
            Event::Touch(touch::Event::FingerLifted { id, position }) => {
                let state = tree.state.downcast_mut::<State>();
                let owns = state.pressed_finger.take() == Some(*id);
                state.pressed = false;
                if owns && bounds.contains(*position) {
                    self.open(tree, shell);
                }
            }
            Event::Keyboard(key)
                if tree
                    .state
                    .downcast_ref::<State>()
                    .trigger_focus
                    .is_focused()
                    && activation_from_key(key) =>
            {
                self.open(tree, shell);
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments, reason = "the draw arguments iced passes")]
    fn draw_inner(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let active = Self::layout_index(tree.state.downcast_ref::<State>());
        let child_layout = layout.child(
            &tree.children,
            Self::layout_index(tree.state.downcast_ref::<State>()),
        );
        use iced::advanced::Renderer as _;
        let mut focus_ring = None;
        if active == 1 {
            let state = tree.state.downcast_ref::<State>();
            let palette = self
                .palette
                .clone()
                .unwrap_or_else(|| crate::theme_lookup::palette(theme));
            let tokens = crate::toolbar_icon::ToolbarIconTokens::from_palette(&palette);
            let bounds = Self::compact_bounds(layout);
            let hovered = cursor.is_over(bounds);
            let background = if self.active_query {
                tokens.active_background
            } else if state.pressed {
                tokens.pressed_background
            } else if hovered {
                tokens.hover_background
            } else {
                Color::TRANSPARENT
            };
            let radius = bounds.width.min(bounds.height) / 2.0;
            if state.trigger_focus.is_focused() {
                focus_ring = Some((
                    bounds,
                    Border {
                        color: tokens.focus_ring,
                        width: tokens.focus_ring_width,
                        radius: radius.into(),
                    },
                ));
            }
            renderer.fill_quad(
                renderer::Quad {
                    bounds,
                    border: Border {
                        radius: radius.into(),
                        ..Border::default()
                    },
                    shadow: iced::Shadow::default(),
                    snap: true,
                },
                Background::Color(background),
            );
        }
        self.children[active].as_widget().draw(
            &tree.children[active],
            renderer,
            theme,
            style,
            child_layout,
            cursor,
            viewport,
        );
        // The focus ring stays above the material and outside its backdrop.
        if let Some((bounds, border)) = focus_ring {
            renderer.fill_quad(
                renderer::Quad {
                    bounds,
                    border,
                    shadow: iced::Shadow::default(),
                    snap: true,
                },
                Background::Color(Color::TRANSPARENT),
            );
        }
    }
}

/// Overlay transport for the compact titlebar-search presentation.
///
/// The field remains the same retained child used inline; only its transport
/// changes. Optional slots are laid out in DOM/focus order after it. The card
/// chrome resolves from the same [`crate::card::CardTokens`] as `enz_card`, so
/// find surfaces and titlebar search do not grow a third visual surface.
struct SearchPopoverOverlay<'a, 'b, Message> {
    children: &'b mut [Element<'a, Message>],
    trees: &'b mut [Tree],
    state: &'b mut State,
    anchor: Rectangle,
    field_id: Id,
    carries_field: bool,
    base_field_layout: Option<Layout>,
    on_open: Option<&'b (dyn Fn(bool) -> Message + 'a)>,
    filters_index: Option<usize>,
    results_index: Option<usize>,
    palette: Option<Palette>,
    size: TextFieldSize,
    radius: Option<f32>,
    index: f32,
    presentation: SearchPresentation,
}

impl<Message> SearchPopoverOverlay<'_, '_, Message> {
    fn indices(&self) -> Vec<usize> {
        let mut indices = if self.carries_field {
            vec![0]
        } else {
            Vec::new()
        };
        indices.extend(self.filters_index);
        indices.extend(self.results_index);
        indices
    }
}

impl<Message> crate::overlay_adapter::LegacyOverlay<Message, iced::Theme, iced::Renderer>
    for SearchPopoverOverlay<'_, '_, Message>
{
    fn layout(&mut self, renderer: &iced::Renderer, bounds: Size) -> layout::Node {
        const WIDTH: f32 = 340.0;
        const EDGE_MARGIN: f32 = 8.0;
        const PAD: f32 = 12.0;
        const GAP: f32 = 8.0;

        // The wide rung anchors under a field the user can see, so the
        // surface matches that field's width rather than the popover's fixed
        // card width. A 340 px card under a 600 px field would read as a
        // different object.
        let target = if self.carries_field {
            WIDTH
        } else {
            self.anchor.width.max(0.0)
        };
        let panel_width = target.min((bounds.width - 2.0 * EDGE_MARGIN).max(0.0));
        let inner_width = (panel_width - 2.0 * PAD).max(0.0);
        let max_child_height = (bounds.height - 2.0 * EDGE_MARGIN - 2.0 * PAD).max(0.0);
        let indices = self.indices();
        let mut nodes = Vec::with_capacity(indices.len());
        let mut y = PAD;
        for index in &indices {
            let limits = layout::Limits::new(Size::ZERO, Size::new(inner_width, max_child_height))
                .width(Length::Fixed(inner_width))
                .height(Length::Shrink);
            let node = self.children[*index].as_widget_mut().measure(
                &mut self.trees[*index],
                renderer,
                &limits,
            );
            let height = node.size().height;
            nodes.push(node.move_to(Point::new(PAD, y)));
            y += height + GAP;
        }
        if !nodes.is_empty() {
            y -= GAP;
        }
        let intrinsic = Size::new(panel_width, y + PAD);
        // The legacy popover stays viewport-centred; anchored surfaces follow their trigger.
        let mut placement =
            enzui_core::placement::Placement::new(enzui_core::placement::Side::Bottom)
                .fallbacks([])
                .gap(6.0);
        if self.carries_field && self.presentation == SearchPresentation::Popover {
            placement = placement.viewport_centered();
        }
        let placement = placement
            // A Popover search is the titlebar-safe presentation. Reserve the
            // framework's default titlebar band as well as the ordinary 8 px edge
            // margin, so a vertically-centred 26 px trigger cannot place the card
            // back over the chrome it escaped.
            .insets(Padding {
                top: crate::title_bar::DEFAULT_TITLEBAR_HEIGHT,
                right: EDGE_MARGIN,
                bottom: EDGE_MARGIN,
                left: EDGE_MARGIN,
            });
        let resolved = placement.resolve(self.anchor, intrinsic, Rectangle::with_size(bounds));
        let root = layout::Node::with_children(intrinsic, nodes)
            .move_to(Point::new(resolved.bounds.x, resolved.bounds.y));

        if self.carries_field && self.state.pending_focus == PendingFocus::Field {
            self.state.pending_focus = PendingFocus::None;
            self.state.trigger_focus.unfocus();
            let field_layout = layout::Placed::of(&root).child(0).to_layout();
            let mut focus = operation::focusable::focus::<()>(self.field_id.clone());
            let mut boxed: Box<dyn Operation<()>> = Box::new(operation::black_box(&mut focus));
            self.children[0].as_widget_mut().operate(
                &mut self.trees[0],
                field_layout,
                &field_layout.bounds(),
                renderer,
                &mut *boxed,
            );
        }

        root
    }

    fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: iced::advanced::layout::Placed<'_>,
        cursor: mouse::Cursor,
    ) {
        use iced::advanced::Renderer as _;
        let palette = self
            .palette
            .clone()
            .unwrap_or_else(|| crate::theme_lookup::palette(theme));
        let tokens = crate::card::CardTokens::from_palette(&palette);
        let surface_radius = enzui_core::metrics::FiniteNonNeg::new_or(
            crate::theme_lookup::radius(theme).lg,
            tokens.radius,
        )
        .get();
        let radius = self
            .size
            .resolve_radius(theme, self.radius)
            .max(surface_radius);
        renderer.fill_quad(
            renderer::Quad {
                bounds: layout.bounds(),
                border: Border {
                    color: tokens.border_for_tone(crate::card::CardTone::Default),
                    width: tokens.border_width,
                    radius: radius.into(),
                },
                shadow: iced::Shadow::default(),
                snap: true,
            },
            Background::Color(tokens.surface),
        );
        renderer.with_layer_rounded(layout.bounds(), radius.into(), |renderer| {
            for (index, child_layout) in self.indices().into_iter().zip(layout.children()) {
                self.children[index].as_widget().draw(
                    &self.trees[index],
                    renderer,
                    theme,
                    style,
                    child_layout.to_layout(),
                    cursor,
                    &layout.bounds(),
                );
            }
        });
    }

    fn update(
        &mut self,
        event: &Event,
        layout: iced::advanced::layout::Placed<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        let press = match event {
            Event::Mouse(mouse::Event::ButtonPressed(_)) => cursor.position(),
            Event::Touch(touch::Event::FingerPressed { position, .. }) => Some(*position),
            _ => None,
        };
        if press
            .is_some_and(|point| !layout.bounds().contains(point) && !self.anchor.contains(point))
        {
            let field_layout = if self.carries_field {
                layout
                    .children()
                    .next()
                    .map(iced_core::layout::Placed::to_layout)
            } else {
                self.base_field_layout
            };
            if let Some(field_layout) = field_layout {
                self.children[0].as_widget_mut().operate(
                    &mut self.trees[0],
                    field_layout,
                    &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
                    renderer,
                    &mut operation::focusable::unfocus::<()>(),
                );
            }
            self.state.open = false;
            self.state.pending_focus = PendingFocus::None;
            self.state.trigger_focus.unfocus();
            self.state.outside_dismissed = true;
            self.state.published_results_open = false;
            if let Some(on_open) = self.on_open {
                shell.publish(on_open(false));
            }
            shell.invalidate_layout();
            shell.request_redraw();
            // Outside dismissal must let the same press reach the destination control.
            return;
        }
        if matches!(
            event,
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                ..
            })
        ) {
            if self.carries_field {
                self.state.open = false;
                self.state.pending_focus = PendingFocus::Trigger;
            } else {
                // The field stays in flow and keeps focus; only the
                // surface withdraws, which is what a dropdown's Escape means.
                // Typing re-opens it (see `results_dismissed`).
                self.state.results_dismissed = true;
            }
            if self.state.published_results_open
                && let Some(on_open) = self.on_open
            {
                self.state.published_results_open = false;
                shell.publish(on_open(false));
            }
            shell.invalidate_layout();
            shell.request_redraw();
            shell.capture_event();
            return;
        }
        let viewport = crate::overlay_adapter::UNBOUNDED_VIEWPORT;
        for (index, child_layout) in self.indices().into_iter().zip(layout.children()) {
            self.children[index].as_widget_mut().update(
                &mut self.trees[index],
                event,
                child_layout.to_layout(),
                cursor,
                renderer,
                shell,
                &viewport,
            );
            if shell.is_event_captured() {
                break;
            }
        }
    }

    fn operate(
        &mut self,
        layout: iced::advanced::layout::Placed<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let mut surface = SemanticsNode::group(
            enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Group).label(
                if self.carries_field {
                    enzui_core::localization::text("enzui-search-field-search-popover")
                } else {
                    enzui_core::localization::text("enzui-search-field-results")
                },
            ),
        );
        operation.custom(None, layout.bounds(), &mut surface);
        operation.container(None, layout.bounds(), &layout.bounds());
        operation.traverse(&mut |operation| {
            for (index, child_layout) in self.indices().into_iter().zip(layout.children()) {
                self.children[index].as_widget_mut().operate(
                    &mut self.trees[index],
                    child_layout.to_layout(),
                    &child_layout.bounds(),
                    renderer,
                    operation,
                );
            }
        });
    }

    fn mouse_interaction(
        &self,
        layout: iced::advanced::layout::Placed<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let viewport = crate::overlay_adapter::UNBOUNDED_VIEWPORT;
        self.indices()
            .into_iter()
            .zip(layout.children())
            .map(|(index, child_layout)| {
                self.children[index].as_widget().mouse_interaction(
                    &self.trees[index],
                    child_layout.to_layout(),
                    cursor,
                    &viewport,
                    renderer,
                )
            })
            .max()
            .unwrap_or_else(|| {
                if cursor.is_over(layout.bounds()) {
                    mouse::Interaction::Idle
                } else {
                    mouse::Interaction::None
                }
            })
    }

    fn overlay<'c>(
        &'c mut self,
        layout: iced::advanced::layout::Placed<'_>,
        renderer: &iced::Renderer,
        window: iced::Size,
    ) -> Vec<overlay::Element<'c, Message, iced::Theme, iced::Renderer>> {
        // Forward the field's own nested overlay (context menu, autocomplete
        // host). Filter/result slots are already surface content rather than
        // popup triggers; keeping this delegation singular also preserves the
        // field's retained tree identity.
        //
        // Only the collapsed popover carries the field. At the wide
        // rung the field is still in flow and its own overlay is delegated by
        // `ResponsiveSearch::overlay`'s in-flow path, so forwarding here would
        // ask the field to lay itself out against a result row's bounds.
        if !self.carries_field {
            return Vec::new();
        }
        let Some(field_layout) = layout.children().next() else {
            return Vec::new();
        };
        self.children[0].as_widget_mut().overlay(
            &mut self.trees[0],
            field_layout.to_layout(),
            renderer,
            &crate::overlay_adapter::UNBOUNDED_VIEWPORT,
            Vector::ZERO,
            window,
        )
    }

    fn index(&self) -> f32 {
        self.index
    }
}

impl<'a, Message: 'a> From<ResponsiveSearch<'a, Message>> for Element<'a, Message> {
    fn from(widget: ResponsiveSearch<'a, Message>) -> Self {
        Element::new(widget)
    }
}

/// Operation proxy that forwards only the focus / text-input methods.
///
/// The responsive wrapper needs to expose the hidden field to `focus(id)` and
/// `find_focused` without also republishing its accessibility node, its
/// container scope, or any scrollable state — those would leak the collapsed
/// field into `SemanticsCollector` walks (a `Role::TextInput` count of one
/// where the caller can only see an icon) and confuse AT/click routing that
/// scopes by container id.
///
/// `traverse` re-wraps recursively so a chained sub-op the inner operation
/// hands back to the widget tree also sees the same filter; if the wrapping
/// were shallow, one layer of children would be filtered and the next layer
/// would revert to full exposure.
mod focus_only_proxy {
    use iced::advanced::widget::Id;
    use iced::advanced::widget::operation::{Focusable, Operation, Outcome, TextInput};
    use iced::{Rectangle, Vector};
    use std::any::Any;

    pub(super) struct FocusOnly<'a> {
        inner: &'a mut dyn Operation<()>,
    }

    impl<'a> FocusOnly<'a> {
        pub(super) fn new(inner: &'a mut dyn Operation<()>) -> Self {
            Self { inner }
        }
    }

    impl Operation<()> for FocusOnly<'_> {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<()>)) {
            self.inner.traverse(&mut |op| {
                let mut wrapped = FocusOnly { inner: op };
                operate(&mut wrapped);
            });
        }

        fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn Focusable) {
            self.inner.focusable(id, bounds, state);
        }

        fn text_input(&mut self, id: Option<&Id>, bounds: Rectangle, state: &mut dyn TextInput) {
            self.inner.text_input(id, bounds, state);
        }

        // Deliberately dropped: `container`, `scrollable`, `custom`, `text`.
        // Those are how the collapsed field would leak into semantics / AT
        // enumeration walks, and the whole point of the proxy is to keep the
        // collapsed presentation the only visible one until a routed focus
        // actually lands here.
        fn container(&mut self, _id: Option<&Id>, _bounds: Rectangle, _viewport: &Rectangle) {}

        fn scrollable(
            &mut self,
            _id: Option<&Id>,
            _bounds: Rectangle,
            _content_bounds: iced::Size,
            _translation: Vector,
            _state: &mut dyn iced::advanced::widget::operation::Scrollable,
        ) {
        }

        fn custom(&mut self, _id: Option<&Id>, _bounds: Rectangle, _state: &mut dyn Any) {}

        fn text(&mut self, _id: Option<&Id>, _bounds: Rectangle, _text: &str) {}

        fn finish(&self) -> Outcome<()> {
            Outcome::None
        }
    }
}
