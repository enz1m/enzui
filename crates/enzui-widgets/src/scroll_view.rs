//! EnzScrollView: general single-child scrollable region.
//!
//! The framework-owned scroll container all page/document surfaces prefer over
//! raw `Flickable`s: single [`Element`] children in `iced::widget::Scrollable`s
//! (configurable axes, widths/heights, per-edge content insets).
//!
//! # Why a distinct primitive when `scroll_column` and
//! `horizontal_scroll_view` already exist?
//!
//! * [`enz_scroll_column`](crate::scroll_column::enz_scroll_column): opinionated
//!   `Vec<Element>` top-anchored vertical `Column`s (sidebars/prefs); wrong for
//!   hand-assembled children needing viewports.
//! * [`enz_horizontal_scroll_view`](crate::horizontal_scroll_view::enz_horizontal_scroll_view):
//!   horizontal-only fixed-strip-height.
//! * `EnzScrollView` = the GENERAL case (any single child, axis, width/height):
//!   the primitive `Sidebar`, `EnzMasterDetailShell`, `EnzTable`, other Layer 3
//!   composites build on.
//!
//! # Axis
//!
//! [`Axis::Vertical`] (default) — y-only; x follows parents.
//! [`Axis::Horizontal`] — x-only. [`Axis::Both`] — both.
//!
//! # notes
//!
//! * `scroll-anchor-policy: "pixel"` = iced defaults, knobless.
//! * `reserve-scrollbar-gutter` = iced scrollbar overlays by default; no gutter
//!   reservations here.
//! * `content-width`/`content-height` implicit in iced: children self-measure;
//!   scrollables decide overflows.
//! * `scroll-x`/`scroll-y` I/O unexposed — iced 0.15 owns scroll states
//!   internally (`iced::widget::operation::*` helpers keyed by widget `Id`s).
//!   Programmatic scrollers attach `Id`s (see [`id`](EnzScrollView::id)).

use enzui_core::metrics::FiniteNonNeg;
use iced::advanced::widget::operation::Focusable;
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::widget::scrollable::{AbsoluteOffset, Direction, Scrollable, Scrollbar, Viewport};
use iced::widget::{Id, container};
use iced::{Border, Color, Element, Event, Length, Padding, Rectangle, Size, Vector, keyboard};
use std::cell::Cell;
use std::rc::Rc;
mod scroll_widgets;

// ---------------------------------------------------------------------------
// Axis
// ---------------------------------------------------------------------------

/// Which axis (or both) the scroll view scrolls on.
///
/// Maps 1:1 to `iced::widget::scrollable::Direction`, but uses the public
/// enzUI vocabulary: "vertical scroll view", "horizontal scroll strip", and
/// "canvas".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Axis {
    /// Vertical only. y-axis scrolls, x follows parent.
    #[default]
    Vertical,
    /// Horizontal only. x-axis scrolls, y follows parent.
    Horizontal,
    /// Both axes. x and y scroll — used for canvas/document surfaces
    /// wider than the viewport in both dimensions.
    Both,
}

impl Axis {
    /// Build the iced `Direction`, using `bar` for the visible scrollbar chrome
    /// (pass `Scrollbar::hidden()` for a chromeless flick).
    fn into_direction(self, bar: Scrollbar) -> Direction {
        match self {
            Axis::Vertical => Direction::Vertical(bar),
            Axis::Horizontal => Direction::Horizontal(bar),
            Axis::Both => Direction::Both {
                vertical: bar,
                horizontal: bar,
            },
        }
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Where a reflow puts the vertical offset: see [`EnzScrollView::anchor`].
type ScrollAnchor<'a> = Box<dyn Fn(Size, Size) -> Option<f32> + 'a>;

/// Convenience constructor: `enz_scroll_view(child)`.
pub fn enz_scroll_view<'a, Message>(
    child: impl Into<Element<'a, Message>>,
) -> EnzScrollView<'a, Message>
where
    Message: 'a,
{
    EnzScrollView {
        child: child.into(),
        axis: Axis::default(),
        inset_left: 0.0,
        inset_right: 0.0,
        inset_top: 0.0,
        inset_bottom: 0.0,
        width: Length::Fill,
        height: Length::Fill,
        id: None,
        on_scroll: None,
        hide_scrollbar: false,
        keyboard_scroll: true,
        wheel_chaining: false,
        publish_translation: None,
        backdrop: None,
        // The theme file's scrollbar geometry, published with the theme.
        scrollbar_tokens: enzui_theme::text_defaults::scrollbar(),
        scrollbar_insets: iced::Padding::ZERO,
        initial_offset: None,
        anchor: None,
    }
}

/// Builder returned by [`enz_scroll_view`].
#[must_use]
pub struct EnzScrollView<'a, Message> {
    child: Element<'a, Message>,
    axis: Axis,
    inset_left: f32,
    inset_right: f32,
    inset_top: f32,
    inset_bottom: f32,
    width: Length,
    height: Length,
    id: Option<Id>,
    on_scroll: Option<Box<dyn Fn(Viewport) -> Message + 'a>>,
    hide_scrollbar: bool,
    keyboard_scroll: bool,
    wheel_chaining: bool,
    /// when set, the composed scrollable's live translation is published
    /// into this cell on every pass, readable at build time by the owner.
    publish_translation: Option<ScrollTranslation>,
    /// the colour the app itself paints behind this scroll surface,
    /// when it is not the palette's `surface`. `None` keeps the legacy
    /// palette-relative thumb; `Some` resolves the overlay thumb's ink for
    /// contrast against *this* colour instead. See [`Self::backdrop`].
    backdrop: Option<Color>,
    /// Build-time scrollbar geometry. iced layout does not receive `&Theme`,
    /// so custom dimensional tokens are supplied explicitly by the owner.
    scrollbar_tokens: enzui_theme::ScrollbarTokens,
    scrollbar_insets: iced::Padding,
    /// Seed the internal [`Scrollable`]'s first-layout offset. `None` keeps
    /// iced's default (start at `0`); `Some(offset)` is consumed once, on the
    /// first layout of a fresh scrollable state (see the setter for the
    /// unmount-and-reopen contract this exists to serve).
    initial_offset: Option<AbsoluteOffset>,
    anchor: Option<ScrollAnchor<'a>>,
}

impl<'a, Message> EnzScrollView<'a, Message>
where
    Message: 'a,
{
    /// Set the scroll axis (default [`Axis::Vertical`]).
    pub fn axis(mut self, axis: Axis) -> Self {
        self.axis = axis;
        self
    }

    /// Set all four edge insets in pixels (default `0.0`).
    ///
    /// Insets are applied to the child *inside* the scrollable, so the
    /// scrollbar sits against the outer edge and the content has
    /// breathing room.
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

    /// Set per-edge insets in pixels.
    ///
    /// Each edge's non-expressible input falls back to the default inset
    /// (`0.0`) independently; `0.0` is honoured literally.
    pub fn insets(mut self, left: f32, right: f32, top: f32, bottom: f32) -> Self {
        self.inset_left = FiniteNonNeg::new_or(left, 0.0).get();
        self.inset_right = FiniteNonNeg::new_or(right, 0.0).get();
        self.inset_top = FiniteNonNeg::new_or(top, 0.0).get();
        self.inset_bottom = FiniteNonNeg::new_or(bottom, 0.0).get();
        self
    }

    /// Override the outer width (default [`Length::Fill`]).
    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// Override the outer height (default [`Length::Fill`]).
    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }

    /// Attach a stable `iced::widget::Id` so apps can drive scroll
    /// operations (`snap_to`, `scroll_by`, `scroll_to`) via
    /// `iced::widget::operation::*` helpers.
    pub fn id(mut self, id: Id) -> Self {
        self.id = Some(id);
        self
    }

    /// Observe scroll offset changes: the closure receives the
    /// [`Viewport`] on every scroll, so an
    /// app can pin-to-bottom, restore/persist offset, or track position. This
    /// is the Elm-native equivalent of the two-way `scroll-x`/`scroll-y`.
    pub fn on_scroll(mut self, f: impl Fn(Viewport) -> Message + 'a) -> Self {
        self.on_scroll = Some(Box::new(f));
        self
    }

    /// Hide the scrollbar chrome ([`Scrollbar::hidden`]) for chromeless parity
    /// bare-`Flickable` behaviour (no 10px overlay covering strip content).
    /// Scrolling by wheel/touch/keyboard still works. Default `false`.
    pub fn hide_scrollbar(mut self, hide: bool) -> Self {
        self.hide_scrollbar = hide;
        self
    }

    /// Override scrollbar geometry tokens for this scroll surface.
    ///
    /// iced does not provide `&Theme` during layout, so dimensional theme
    /// overrides must be passed at build time (normally from the same
    /// application theme state used to construct `iced::Theme`).
    pub fn scrollbar_tokens(mut self, tokens: enzui_theme::ScrollbarTokens) -> Self {
        self.scrollbar_tokens = tokens.normalized();
        self
    }

    /// Inset scrollbar paint and pointer targets without padding scroll content.
    /// Defaults to zero. Top/bottom affect vertical tracks; left/right affect
    /// horizontal tracks. Values are logical pixels; negative and nonfinite
    /// values become zero. Oversized insets clamp to the available track,
    /// leading edge first; collapsed tracks cannot be grabbed.
    /// The rails already keep clear of the rounded corners of the window and
    /// panes around them, with no insets. Use
    /// [`crate::app_shell::ShellGeometry::scrollbar_safe_area`] in a
    /// geometry-aware primary shell content closure to clear floating chrome
    /// as well; secondary slots use
    /// [`crate::app_shell::ShellGeometry::secondary_scrollbar_safe_area`].
    pub fn scrollbar_insets(mut self, insets: impl Into<iced::Padding>) -> Self {
        self.scrollbar_insets = insets.into();
        self
    }

    /// Seed the composed [`Scrollable`]'s first-layout offset, so a
    /// scrollable whose retained state has just been reconstructed lands at
    /// the caller's remembered position instead of at `0`.
    ///
    /// # When this matters
    ///
    /// Offsets live in widget states surviving ordinary rebuilds (gesture +
    /// message-rebuild offsets need no host help). Structural UNMOUNTS drop
    /// them: parents removing scrollables (hidden-sidebar scrollables,
    /// tree-folded responsive lanes) — distinct from still-visible app-shell
    /// content regions retaining through sidebar disclosures. Unseeded
    /// recreated unmounteds start at `0`. Seeds consume exactly once (first
    /// fresh-state layouts, [`Scrollable::initial_offset`] contracts): later
    /// gestures / [`id`](Self::id)-targeted `scroll_to`s own offsets. Not
    /// controlled offsets: every-build passing never fights widget states;
    /// merely wins reconstructed-state builds. Non-finite/negative components
    /// sanitize per-axis to `0.0`; non-null structural zeroes (`{x: 0.0, y:
    /// 0.0}`) are expressible top-seeding intents, honoured literally.
    pub fn initial_offset(mut self, offset: AbsoluteOffset) -> Self {
        self.initial_offset = Some(sanitize_initial_offset(offset));
        self
    }

    /// Keep a logical position in view while the content reflows: whenever
    /// a layout changes the content's size, `anchor` gets the laid-out
    /// content size and the viewport size and returns the vertical offset
    /// that keeps the position where it was (a document page's top at the
    /// new width), or `None` to leave the offset alone.
    ///
    /// The offset is applied and drawn in the same layout, so a reflow that
    /// runs every frame, such as an app shell's sidebar animating beside a
    /// fit-width document, never paints a frame at a stale offset. Layouts
    /// that keep the content's size leave the offset to the user. Track the
    /// position from [`on_scroll`](Self::on_scroll) and anchor to it.
    pub fn anchor(mut self, anchor: impl Fn(Size, Size) -> Option<f32> + 'a) -> Self {
        self.anchor = Some(Box::new(anchor));
        self
    }

    /// Tell the overlay scrollbar the colour the app is actually painting
    /// behind this surface, so its thumb resolves its ink for contrast against
    /// *that* colour rather than the palette's assumed `surface`.
    ///
    /// # Why this exists
    ///
    /// The colour the app paints behind the scroll region. Thumb ink
    /// otherwise derives from the palette's text colour, which is invisible
    /// over an app-painted page such as a light sepia reading background;
    /// with a backdrop, [`scroller_ink_over`] picks dark or light ink by its
    /// luminance.
    ///
    /// [`scroller_ink_over`]: crate::scroll_view::scroller_ink_over
    pub fn backdrop(mut self, color: Color) -> Self {
        self.backdrop = Some(color);
        self
    }

    /// Enable/disable keyboard scrolling (PageDown / PageUp / Home / End)
    /// for the focused subtree, falling back to the region under the pointer.
    /// Default `true`.
    ///
    /// Turn it off for a viewport whose owner binds those keys itself (a
    /// paginated list, a virtual terminal), so the region does not steal them.
    pub fn keyboard_scroll(mut self, enabled: bool) -> Self {
        self.keyboard_scroll = enabled;
        self
    }

    /// Let wheel events continue to an ancestor at this region's edges.
    /// Defaults to `false`: overflowing regions contain wheel movement on
    /// their scroll axes. Fitting content and unrelated axes still pass it on.
    pub fn wheel_chaining(mut self, enabled: bool) -> Self {
        self.wheel_chaining = enabled;
        self
    }

    /// Publish the live offset for build-time consumers such as virtual lists.
    ///
    /// An update requests a widget rebuild when the offset differs from the
    /// value used to build this view, without publishing an application message.
    /// Operations publish immediately; their following redraw requests the
    /// rebuild. The inner scrollable retains gesture and clamping ownership.
    pub fn publish_translation(mut self, cell: ScrollTranslation) -> Self {
        self.publish_translation = Some(cell);
        self
    }
}

/// Clamp an [`AbsoluteOffset`] to finite, non-negative components.
///
/// Non-finite (`NaN`, `±inf`) and negative components collapse to `0.0` per
/// axis, matching the same-name policy `EnzTreeView::scroll_offset` and
/// `GutterConfig::scroll_offset` apply to their scalar seeds. Scoped to this
/// module because the seed is opt-in and appears in exactly one From impl.
fn sanitize_initial_offset(offset: AbsoluteOffset) -> AbsoluteOffset {
    fn axis(v: f32) -> f32 {
        if v.is_finite() { v.max(0.0) } else { 0.0 }
    }
    AbsoluteOffset {
        x: axis(offset.x),
        y: axis(offset.y),
    }
}

// ---------------------------------------------------------------------------
// Keyboard scrolling
// ---------------------------------------------------------------------------

/// How much of the outgoing page stays on screen after a PageDown/PageUp, in
/// logical pixels.
///
/// A page turn with *no* overlap throws away the reader's place: the line they
/// were reading is gone and there is no anchor to re-acquire. Every document
/// reader keeps a couple of lines of context instead. 40 px is ~2 lines at the
/// theme's 13 px body size with default 1.3 line-height (16.9 px/line), which
/// is the usual convention and lands close to the "~10% of a page" rule of
/// thumb for the ~400 px viewports this primitive is used at.
const PAGE_OVERLAP: f32 = 40.0;

/// The four scrolling keys this primitive understands.
///
/// `pub(crate)` so a modal that owns keyboard input while focus sits outside
/// its scroll region can classify events
/// through the same table rather than growing a second convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScrollKey {
    PageDown,
    PageUp,
    Home,
    End,
}

impl ScrollKey {
    /// The single source of truth for which named keys this primitive
    /// claims, and the [`ScrollKey`] each maps to.
    ///
    /// Both the production decoder ([`Self::from_event`]) and the decoder
    /// matrix test consume this one table, so a key cannot be claimed in
    /// production without automatically entering the test population. The
    /// matrix pins the table's length, so *adding* a claim reddens the test
    /// until its floors are consciously updated — the claim table is
    /// fail-closed, not a hand-copied list that can drift.
    pub(crate) const CLAIMED: &'static [(keyboard::key::Named, ScrollKey)] = &[
        (keyboard::key::Named::PageDown, ScrollKey::PageDown),
        (keyboard::key::Named::PageUp, ScrollKey::PageUp),
        (keyboard::key::Named::Home, ScrollKey::Home),
        (keyboard::key::Named::End, ScrollKey::End),
    ];

    /// Classify an event as one of the scrolling keys, or `None`.
    ///
    /// Deliberately free classifiers (unit-test-reachable; `update` paths
    /// aren't). Ctrl-/Alt-/Logo-modified presses (Shift-inclusive) return
    /// `None`: events stay `Ignored` for application chord handlers
    /// (application-owned chords, by framework convention).
    /// Shift-alone kept: scroll controls' own modifier (`focus`, `tree_view`,
    /// calendar-matched). Modified paging keys therefore produce zero scroll
    /// tasks/captures on any route (`KeyboardScroll`, `KeyScrollForward`,
    /// sharing virtual list/grids).
    pub(crate) fn from_event(event: &Event) -> Option<Self> {
        let Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event else {
            return None;
        };
        // Ctrl / Alt / Logo mark application chords — let them through.
        if modifiers.control() || modifiers.alt() || modifiers.logo() {
            return None;
        }
        let keyboard::Key::Named(named) = key else {
            return None;
        };
        Self::CLAIMED
            .iter()
            .find_map(|(claimed, scroll_key)| (claimed == named).then_some(*scroll_key))
    }

    /// The scroll amount for this key, in *wheel-pixel* units, given the
    /// viewport `extent` (height for a vertical view, width for a horizontal
    /// one).
    ///
    /// Wheel (not offset) sign conventions: iced `scrollable::update`s turn
    /// `ScrollDelta::Pixels { x, y }` into `-Vector::new(x, y)` — NEGATIVES
    /// here scroll TOWARDS content ends. `Home`/`End` deliberately overshoot
    /// `f32::MAX`: iced `State::scroll`s clamp `0.0..= content - viewport`,
    /// resolving exact tops/bottoms with zero content-size knowledge here.
    /// Degenerate extents (non-finite/`<= 0`) return `None` ("no page to
    /// turn"): callers forward + capture nothing. Currently widget-
    /// unreachable degenerates (`Rectangle::contains` half-opens:
    /// `iced/core/src/rectangle.rs` — zero-height rects contain no
    /// points; `update` cursor gates reject first; measured `Fixed(0.0)` views
    /// return `[Ignored, Ignored]` on PageDown) handled ANYWAY: dividers own
    /// viewport guards, directly testable here (unreachable `update`-buried
    /// branches pin no tests).
    fn delta(self, extent: f32) -> Option<f32> {
        if !extent.is_finite() || extent <= 0.0 {
            return None;
        }
        // A page keeps `PAGE_OVERLAP` px of context — but never less than half
        // a viewport, or a region shorter than 2x the overlap would page
        // *backwards* (extent - overlap < 0) instead of forwards.
        let page = (extent - PAGE_OVERLAP).max(extent * 0.5);
        Some(match self {
            Self::PageDown => -page,
            Self::PageUp => page,
            Self::End => -f32::MAX,
            Self::Home => f32::MAX,
        })
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Global scroll-focus registry
// ---------------------------------------------------------------------------
//
// Whether any scroll region in the window has focus is a whole-tree fact,
// but a region cannot see its siblings during its own `update`, and rows and
// columns dispatch to every child in tree order. So each region publishes
// whether its subtree holds focus (observed on every event and every focus
// operation) into a registry keyed by its bounds, and decisions read the or
// of the registry, independent of order. It is not keyed by window, since the
// test simulator puts every headless tree under one key.
//
// # Why bounds, not an identity
//
// Bounds are per-frame keys, recomputed every pass, not persisted
// identities. Regions in a sane layout have disjoint bounds; two regions that
// overlap exactly are a known blind spot, as overlapping focusables are.
//
// # Runtime-owned, not process/thread-global
//
// `widget_runtime`-lived registries: production event-loop scopes persist
// across frames; framework collection fixtures get fresh owners. Dropped UIs
// never suppress later-same-thread-fixture pointer fallbacks.

/// Whole-tree scroll-focus facts owned by one widget runtime.
///
/// The `epoch` guards against staleness: a scroll region removed from the tree
/// stops re-stamping its entry, so its epoch falls behind and its (possibly
/// `true`) focus flag is ignored within one frame. `last_frame` makes sibling
/// wrappers advance the epoch exactly once for a shared redraw instant.
#[derive(Default)]
struct ScrollFocusRegistry {
    regions: std::collections::HashMap<u64, (u64, bool)>,
    epoch: u64,
    last_frame: Option<std::time::Instant>,
}

/// Advance the frame epoch, but only the first time a given frame's
/// `RedrawRequested` `Instant` is seen. Every sibling wrapper forwards the same
/// redraw, so without the de-dup the epoch would jump by the number of scroll
/// regions per frame and defeat the one-frame liveness tolerance.
fn advance_scroll_focus_epoch(frame: std::time::Instant) {
    enzui_core::widget_runtime::with_state::<ScrollFocusRegistry, _>(|registry| {
        if registry.last_frame == Some(frame) {
            return;
        }
        registry.last_frame = Some(frame);
        registry.epoch = registry.epoch.wrapping_add(1);
    });
}

/// Quantise a rectangle to a stable `u64` key. Rounds each edge to the nearest
/// pixel so sub-pixel jitter between passes maps to one entry, and packs four
/// 16-bit lanes. Non-finite / out-of-range coordinates saturate rather than
/// wrap, so a degenerate layout collapses onto a single key instead of
/// aliasing across distinct regions.
fn bounds_key(bounds: Rectangle) -> u64 {
    fn lane(v: f32) -> u64 {
        if !v.is_finite() {
            return 0;
        }
        (v.round().clamp(0.0, f32::from(u16::MAX)) as u64) & 0xffff
    }
    (lane(bounds.x) << 48)
        | (lane(bounds.y) << 32)
        | (lane(bounds.width) << 16)
        | lane(bounds.height)
}

/// Record whether the region at `bounds` currently holds focus in its subtree,
/// stamping it with the current frame epoch. Called from both `update` (every
/// event) and `operate` (every operation pass), so the registry tracks focus
/// regardless of how it was moved — a keyboard Tab, a pointer click routed to
/// an inner widget, or a programmatic `focus(id)` that never delivers an event
/// to the wrapper at all.
fn register_scroll_focus(bounds: Rectangle, focused: bool) {
    enzui_core::widget_runtime::with_state::<ScrollFocusRegistry, _>(|registry| {
        registry
            .regions
            .insert(bounds_key(bounds), (registry.epoch, focused));
    });
}

/// Whether any scroll region in the active widget runtime currently holds
/// keyboard focus in its subtree — the whole-tree fact a single wrapper cannot
/// compute at its own decision time. `true` suppresses pointer fallbacks:
/// focused regions win regardless of tree order. Live entries only: >1-frame-
/// stale epochs belonged to unlaid-out regions; focus flags ignored (+ pruned)
/// so removed focused regions never pin fallbacks off forever.
fn any_scroll_region_focused() -> bool {
    enzui_core::widget_runtime::with_state::<ScrollFocusRegistry, _>(|registry| {
        // Prune stale entries, then answer from what remains. `wrapping` epoch
        // arithmetic: an entry is live if the epoch advanced by at most 1 since
        // it was stamped.
        let epoch = registry.epoch;
        registry
            .regions
            .retain(|_, &mut (entry_epoch, _)| epoch.wrapping_sub(entry_epoch) <= 1);
        registry.regions.values().any(|&(_, focused)| focused)
    })
}

/// Check whether a widget subtree contains any focused descendant.
///
/// Runs a lightweight [`Operation`] traversal that visits every `Focusable`
/// node and returns as soon as one reports `is_focused() == true`. Called
/// from `KeyboardScroll::update()` to decide which scroll region owns a
/// keyboard paging event.
fn subtree_has_focus<Message, Theme, Renderer>(
    child: &mut Element<'_, Message, Theme, Renderer>,
    tree: &mut Tree,
    layout: Layout,
    renderer: &Renderer,
) -> bool
where
    Renderer: iced::advanced::Renderer,
{
    struct FocusProbe {
        found: bool,
    }
    impl Operation for FocusProbe {
        fn focusable(
            &mut self,
            _id: Option<&iced::advanced::widget::Id>,
            _bounds: Rectangle,
            state: &mut dyn Focusable,
        ) {
            if state.is_focused() {
                self.found = true;
            }
        }

        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            if !self.found {
                operate(self);
            }
        }
    }

    let mut probe = FocusProbe { found: false };
    child
        .as_widget_mut()
        .operate(tree, layout, &layout.bounds(), renderer, &mut probe);
    probe.found
}

/// Scroll the first `Scrollable` in a widget subtree by a delta, directly
/// via the `Scrollable::scroll_by` operation method. `pub(crate)` (same as
/// [`ScrollKey`]): shortcuts-dialog key-ownership wrappers (`modal_host.rs`)
/// need identical primitives over second implementations. Returns the
/// translation before the scroll ([`scroll_first_and_present`] reads it back
/// after presenting), `None` on scrollableless subtrees. Event-system-bypassing (no
/// `in_transaction` nested-handoff-blocking captures). NO `on_scroll`
/// notifications: scrollable viewport tracking catches changed offsets on next
/// `update()`s, publishing then (keyboard-scroll one-frame delays acceptable:
/// users already see moved content). First-`Scrollable`-subtree bounds (+
/// `None` on none); same-test-use `pub(crate)`s ([`scroll_by_first`]): focus-
/// outside modals owning keys need region viewport extents for page deltas;
/// deltas can't know treelessly.
pub(crate) fn first_scrollable_bounds<Message, Theme, Renderer>(
    child: &mut Element<'_, Message, Theme, Renderer>,
    tree: &mut Tree,
    layout: Layout,
    renderer: &Renderer,
) -> Option<Rectangle>
where
    Renderer: iced::advanced::Renderer,
{
    use iced::advanced::widget::operation::scrollable::Scrollable;
    struct FirstBounds {
        found: Option<Rectangle>,
    }
    impl Operation for FirstBounds {
        fn scrollable(
            &mut self,
            _id: Option<&iced::advanced::widget::Id>,
            bounds: Rectangle,
            _content_bounds: Size,
            _translation: Vector,
            _state: &mut dyn Scrollable,
        ) {
            if self.found.is_none() {
                self.found = Some(bounds);
            }
        }
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            if self.found.is_none() {
                operate(self);
            }
        }
    }
    let mut op = FirstBounds { found: None };
    child
        .as_widget_mut()
        .operate(tree, layout, &layout.bounds(), renderer, &mut op);
    op.found
}

pub(crate) fn scroll_by_first<Message, Theme, Renderer>(
    child: &mut Element<'_, Message, Theme, Renderer>,
    tree: &mut Tree,
    layout: Layout,
    renderer: &Renderer,
    delta: Vector,
) -> Option<Vector>
where
    Renderer: iced::advanced::Renderer,
{
    use iced::advanced::widget::operation::scrollable::{AbsoluteOffset, Scrollable};

    struct ScrollByFirst {
        delta: Vector,
        before: Option<Vector>,
        done: bool,
    }
    impl Operation for ScrollByFirst {
        fn scrollable(
            &mut self,
            _id: Option<&iced::advanced::widget::Id>,
            bounds: Rectangle,
            content_bounds: Size,
            translation: Vector,
            state: &mut dyn Scrollable,
        ) {
            if !self.done {
                self.before = Some(translation);
                state.scroll_by(
                    AbsoluteOffset {
                        x: self.delta.x,
                        y: self.delta.y,
                    },
                    iced::advanced::widget::operation::Animation::Instant,
                    bounds,
                    content_bounds,
                );
                self.done = true;
            }
        }

        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            if !self.done {
                operate(self);
            }
        }
    }

    let mut op = ScrollByFirst {
        delta,
        before: None,
        done: false,
    };
    child
        .as_widget_mut()
        .operate(tree, layout, &layout.bounds(), renderer, &mut op);

    op.before
}

/// The translation the first `Scrollable` in a subtree reports, or `None` if
/// there is none.
fn first_scrollable_translation<Message, Theme, Renderer>(
    child: &mut Element<'_, Message, Theme, Renderer>,
    tree: &mut Tree,
    layout: Layout,
    renderer: &Renderer,
) -> Option<Vector>
where
    Renderer: iced::advanced::Renderer,
{
    use iced::advanced::widget::operation::scrollable::Scrollable;

    struct TranslationProbe {
        translation: Option<Vector>,
    }
    impl Operation for TranslationProbe {
        fn scrollable(
            &mut self,
            _id: Option<&iced::advanced::widget::Id>,
            _bounds: Rectangle,
            _content_bounds: Size,
            translation: Vector,
            _state: &mut dyn Scrollable,
        ) {
            if self.translation.is_none() {
                self.translation = Some(translation);
            }
        }

        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            if self.translation.is_none() {
                operate(self);
            }
        }
    }

    let mut probe = TranslationProbe { translation: None };
    child
        .as_widget_mut()
        .operate(tree, layout, &layout.bounds(), renderer, &mut probe);
    probe.translation
}

/// Scroll the first `Scrollable` in a subtree by `delta` and present the
/// result to it, returning whether its offset moved (`None` if the subtree has
/// no scrollable).
///
/// A scrollable reports the translation of the last frame it presented, and
/// commits a new one only while handling `RedrawRequested`. So the scroll is
/// followed by a synthetic `RedrawRequested` forwarded to the child, before
/// the translation is read back. The same event fires the scrollable's
/// `on_scroll` in this update rather than a frame late; its messages and
/// redraw request are merged into `shell`.
#[allow(
    clippy::too_many_arguments,
    reason = "the widget-event arguments iced passes"
)]
pub(crate) fn scroll_first_and_present<Message, Theme, Renderer>(
    child: &mut Element<'_, Message, Theme, Renderer>,
    tree: &mut Tree,
    layout: Layout,
    cursor: mouse::Cursor,
    renderer: &Renderer,
    shell: &mut Shell<'_, Message>,
    viewport: &Rectangle,
    delta: Vector,
) -> Option<bool>
where
    Renderer: iced::advanced::Renderer,
{
    let before = scroll_by_first(child, tree, layout, renderer, delta)?;

    let mut presented: iced::advanced::shell::Bus<Message> = iced::advanced::shell::Bus::new();
    let mut presented_shell = shell.local(&mut presented);
    child.as_widget_mut().update(
        tree,
        &Event::Window(iced::window::Event::RedrawRequested(
            std::time::Instant::now(),
        )),
        layout,
        cursor,
        renderer,
        &mut presented_shell,
        viewport,
    );
    shell.merge(presented_shell, std::convert::identity);

    let after = first_scrollable_translation(child, tree, layout, renderer)?;
    Some((before.x - after.x).abs() > 0.001 || (before.y - after.y).abs() > 0.001)
}

#[derive(Default)]
struct ScrollInputState {
    modifiers: keyboard::Modifiers,
}

struct KeyboardScroll<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    axis: Axis,
    keyboard_scroll: bool,
    wheel_chaining: bool,
}

impl<'a, Message, Theme, Renderer> KeyboardScroll<'a, Message, Theme, Renderer> {
    fn new(child: impl Into<Element<'a, Message, Theme, Renderer>>, axis: Axis) -> Self {
        Self {
            child: child.into(),
            axis,
            keyboard_scroll: true,
            wheel_chaining: false,
        }
    }
}

/// Apply keyboard routing and wheel containment to a framework-owned scroller.
/// Virtual surfaces share [`EnzScrollView`] key classification, page distances,
/// focus ownership and keyboard edge handoff. [`KeyboardScroll`] reads the
/// laid-out viewport instead of taking a caller-supplied page size.
pub(crate) fn keyboard_scroll_of<'a, Message>(
    child: impl Into<Element<'a, Message>>,
    axis: Axis,
) -> Element<'a, Message>
where
    Message: 'a,
{
    Element::new(KeyboardScroll::new(child, axis))
}

// ---------------------------------------------------------------------------
// KeyScrollForward
// ---------------------------------------------------------------------------

/// Wrap a subtree so PageDown/PageUp/Home/End are forwarded to the first
/// inner `Scrollable`, regardless of cursor position or which descendant
/// (if any) currently holds focus.
///
/// # Why this is separate from [`KeyboardScroll`]
///
/// `KeyboardScroll` takes paging keys through two gates: focus inside its
/// subtree, or, failing that, the pointer over its bounds. They matter in a
/// window with several scroll regions, where an ungated PageDown would page
/// all of them. A modal whose focus is outside its scroll region defeats
/// both: a shortcuts dialog focuses its sibling Close button, and a
/// keyboard-only user has no cursor nearby, so without this wrapper they
/// cannot scroll the dialog. As with
/// [`EscapeCapture`](crate::modal_host::EscapeCapture), a modal presentation
/// makes "keyboard over the widget" ill-defined and modal ownership is the
/// honest gate. The presence of a scrollable in the subtree stands in for
/// it: without one, no keys are captured.
///
/// # What it does not do
///
/// * Never fires on child-consumed keys — open combo boxes / event-taking inner
///   `KeyboardScroll`s still win (post-child `shell.is_event_captured()`
///   checks).
/// * Captures only four scroll keys: Escape/Tab/app bindings fall through.
/// * Captures only on actually-moved offsets: bottomed dialogs must not swallow
///   PageDowns from outer scroll views — same `KeyboardScroll` handoff rules.
pub(crate) struct KeyScrollForward<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
}

impl<'a, Message, Theme, Renderer> KeyScrollForward<'a, Message, Theme, Renderer> {
    pub(crate) fn new(child: impl Into<Element<'a, Message, Theme, Renderer>>) -> Self {
        Self {
            child: child.into(),
        }
    }
}

/// Wrap `child` so it forwards Page/Home/End into the first inner scrollable.
/// See [`KeyScrollForward`].
pub(crate) fn key_scroll_forward<'a, Message>(
    child: impl Into<Element<'a, Message>>,
) -> Element<'a, Message>
where
    Message: 'a,
{
    Element::new(KeyScrollForward::new(child))
}

// ---------------------------------------------------------------------------
// Overflow observation — Operation-based probe
// ---------------------------------------------------------------------------

/// Live scroll geometry read from the widget tree via an [`Operation`].
///
/// Produced by running [`QueryScrollGeometry`] against laid-out trees: all
/// three fields come straight from underlying-`Scrollable` [`Operation`] impl
/// `scrollable()` callbacks — ACTUAL last-layout content-vs-viewport
/// measurements, never stored builder fields.
///
/// # Overflow
///
/// Axis-overflow ⟺ content extents exceed viewport extents:
/// [`is_overflowing_x`](ScrollGeometry::is_overflowing_x) /
/// [`is_overflowing_y`](ScrollGeometry::is_overflowing_y) express:
///
/// ```text
/// overflowing_x  ⟺  content_bounds.width  > bounds.width
/// overflowing_y  ⟺  content_bounds.height > bounds.height
/// ```
///
/// Fade-edge indicators, scroll-affordance arrows, "more →" badges need exactly
/// this.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollGeometry {
    /// The viewport rectangle (position + visible size) of the scrollable.
    pub bounds: Rectangle,
    /// The content rectangle (total scrollable area).
    pub content_bounds: Rectangle,
    /// The current scroll translation (offset applied to content).
    pub translation: Vector,
}

impl ScrollGeometry {
    /// Whether the content is wider than the viewport.
    pub fn is_overflowing_x(&self) -> bool {
        self.content_bounds.width > self.bounds.width
    }

    /// Whether the content is taller than the viewport.
    pub fn is_overflowing_y(&self) -> bool {
        self.content_bounds.height > self.bounds.height
    }

    /// Whether the content overflows the viewport on any axis.
    pub fn is_overflowing(&self) -> bool {
        self.is_overflowing_x() || self.is_overflowing_y()
    }

    /// Whether content is clipped off the left edge — i.e. the strip has
    /// been scrolled right and there is hidden content behind the left edge.
    ///
    /// This is the signal a left fade / "scroll back" affordance needs. It is
    /// deliberately *not* the same as
    /// [`is_overflowing_x`](Self::is_overflowing_x): a strip that overflows but
    /// sits at offset 0 hides content on the right only, so painting a left
    /// fade there would be an always-fire indicator — it would claim there is
    /// something to scroll back to when the strip is already at its start.
    ///
    /// `translation.x` is the offset applied to the content, so `x > 0` means
    /// content has moved left out of view.
    ///
    /// The `EPS` guard keeps a sub-pixel residue (the sort a fractional DPI
    /// scale or a momentum-scroll rest position leaves behind) from painting a
    /// fade over nothing.
    pub fn is_clipped_left(&self) -> bool {
        self.is_overflowing_x() && self.translation.x > Self::EPS
    }

    /// Whether content is clipped off the right edge — i.e. there is hidden
    /// content past the right edge still to scroll to.
    ///
    /// Complement of [`is_clipped_left`](Self::is_clipped_left): a strip
    /// scrolled fully to its end hides content on the left only, and must
    /// not paint a right fade.
    ///
    /// Computed as "is there any remaining scrollable distance", i.e.
    /// `content_width - viewport_width - translation.x > 0`.
    pub fn is_clipped_right(&self) -> bool {
        if !self.is_overflowing_x() {
            return false;
        }
        let max_offset = self.content_bounds.width - self.bounds.width;
        (max_offset - self.translation.x) > Self::EPS
    }

    /// Whether content is clipped off the top edge — i.e. the column has
    /// been scrolled down and there is hidden content above the viewport.
    ///
    /// Vertical counterpart of [`is_clipped_left`](Self::is_clipped_left).
    /// `translation.y > 0` means content has moved up out of view.
    pub fn is_clipped_top(&self) -> bool {
        self.is_overflowing_y() && self.translation.y > Self::EPS
    }

    /// Whether content is clipped off the bottom edge — i.e. there is
    /// hidden content below the viewport still to scroll to.
    ///
    /// Vertical counterpart of [`is_clipped_right`](Self::is_clipped_right).
    /// Computed as `content_height - viewport_height - translation.y > 0`.
    pub fn is_clipped_bottom(&self) -> bool {
        if !self.is_overflowing_y() {
            return false;
        }
        let max_offset = self.content_bounds.height - self.bounds.height;
        (max_offset - self.translation.y) > Self::EPS
    }

    /// Sub-pixel tolerance for the clipped-edge predicates.
    ///
    /// Chosen at half a pixel: below this the "hidden" content cannot produce a
    /// visibly different frame, so a fade would be decoration with no referent.
    const EPS: f32 = 0.5;
}

/// An [`Operation`] that reads the live [`ScrollGeometry`] of the scrollable
/// identified by `target`. Built-`UserInterface`s (or `Task`-converted) yield
/// `Some(ScrollGeometry)` on found targets, `None` otherwise.
///
/// # Example (headless / test)
///
/// # Why this is not a stored field
///
/// Stored `is_overflowing`s let builders assign + tests observe without real-
/// layout derivation ("dead field" defects). Live-`Scrollable`-state
/// `bounds`/`content_bounds` reads make measurement-severing (constant-
/// returning) tests fail.
pub struct QueryScrollGeometry {
    target: Id,
    result: Option<ScrollGeometry>,
}

impl QueryScrollGeometry {
    /// Create a new probe targeting the scrollable with the given [`Id`].
    pub fn new(target: Id) -> Self {
        Self {
            target,
            result: None,
        }
    }

    /// The result after running the operation, or `None` if the target was
    /// not found.
    pub fn result(&self) -> Option<ScrollGeometry> {
        self.result
    }
}

impl Operation for QueryScrollGeometry {
    fn scrollable(
        &mut self,
        id: Option<&iced::advanced::widget::Id>,
        bounds: Rectangle,
        content_bounds: Size,
        translation: Vector,
        _state: &mut dyn iced::advanced::widget::operation::scrollable::Scrollable,
    ) {
        if id == Some(&self.target) {
            self.result = Some(ScrollGeometry {
                bounds,
                // Laid out at the viewport origin; the operation reports its
                // size since upstream iced #3479.
                content_bounds: Rectangle::new(bounds.position(), content_bounds),
                translation,
            });
        }
    }

    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        if self.result.is_none() {
            operate(self);
        }
    }
}

/// A shared, build-time-readable scroll offset in logical pixels.
///
/// Retain the same handle across views and pass a clone to
/// [`EnzScrollView::publish_translation`] or a virtual control's
/// `self_scrolling` method. Publication requests the rebuild needed to consume
/// a changed offset. Fresh handles read [`Vector::ZERO`].
#[derive(Debug, Clone)]
pub struct ScrollTranslation(Rc<Cell<Vector>>);

impl Default for ScrollTranslation {
    fn default() -> Self {
        Self::new()
    }
}

impl ScrollTranslation {
    /// Create a handle reading `Vector::ZERO` (scrolled to the top).
    pub fn new() -> Self {
        ScrollTranslation(Rc::new(Cell::new(Vector::ZERO)))
    }

    /// The live scroll translation as last stamped by the wrapper.
    ///
    /// `x`/`y` are the offset applied to the content, so `y == 40.0` means the
    /// top 40px of content is scrolled out of view above the viewport -- the
    /// same sign as `Operation::scrollable`'s `translation` argument.
    pub fn get(&self) -> Vector {
        self.0.get()
    }

    /// Replace the published value without moving the underlying scrollable.
    ///
    /// Use scrollable operations to move a mounted control; its publisher
    /// overwrites this value with the actual offset on the next pass.
    pub fn set(&self, translation: Vector) {
        self.0.set(translation);
    }
}

/// A build-time-readable handle to a scrollable's full live geometry.
///
/// [`ScrollTranslation`] offsets suffice virtualized lists ("which rows
/// visible"); edge-fade indicators need more: right-edge-clipping decisions
/// need viewport + content widths (remaining distances = `content - viewport -
/// offset`; 200-offsets are ends for some strips, halfway for others).
/// Cloneable cheap `Rc<Cell<..>>`s; same seams + documented one-frame lags as
/// [`ScrollTranslation`]. Fresh-handle `None`s = "nothing clipped yet" (never
/// guessed).
#[derive(Debug, Clone, Default)]
pub struct ScrollGeometryCell(Rc<Cell<Option<ScrollGeometry>>>);

impl ScrollGeometryCell {
    /// A new, unstamped cell. Reads `None` until a pass observes the
    /// scrollable.
    pub fn new() -> Self {
        Self(Rc::new(Cell::new(None)))
    }

    /// The geometry the scrollable reported on its last pass, or `None` if no
    /// pass has stamped it yet.
    pub fn get(&self) -> Option<ScrollGeometry> {
        self.0.get()
    }

    /// Stamp new geometry. Called by the wrapper; also usable directly to seed
    /// a known state in a test.
    pub fn set(&self, geo: ScrollGeometry) {
        self.0.set(Some(geo));
    }
}

/// Wrap `child` so the first inner `Scrollable`'s full geometry is published
/// into `cell` on every `operate`/`update` pass.
///
/// Transparent: every `Widget` method delegates to the child, so the inner
/// `Scrollable` stays the sole owner of the offset and its clamping.
pub fn publish_geometry_of<'a, Message>(
    child: impl Into<Element<'a, Message>>,
    cell: ScrollGeometryCell,
) -> Element<'a, Message>
where
    Message: 'a,
{
    Element::new(PublishGeometry {
        child: child.into(),
        cell,
    })
}

fn first_scroll_geometry<Message, Theme, Renderer>(
    child: &mut Element<'_, Message, Theme, Renderer>,
    tree: &mut Tree,
    layout: Layout,
    renderer: &Renderer,
) -> Option<ScrollGeometry>
where
    Renderer: iced::advanced::Renderer,
{
    use iced::advanced::widget::operation::scrollable::Scrollable;
    struct FirstGeometry {
        found: Option<ScrollGeometry>,
    }
    impl Operation for FirstGeometry {
        fn scrollable(
            &mut self,
            _id: Option<&iced::advanced::widget::Id>,
            bounds: Rectangle,
            content_bounds: Size,
            translation: Vector,
            _state: &mut dyn Scrollable,
        ) {
            if self.found.is_none() {
                self.found = Some(ScrollGeometry {
                    bounds,
                    content_bounds: Rectangle::new(bounds.position(), content_bounds),
                    translation,
                });
            }
        }
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            if self.found.is_none() {
                operate(self);
            }
        }
    }
    let mut op = FirstGeometry { found: None };
    child
        .as_widget_mut()
        .operate(tree, layout, &layout.bounds(), renderer, &mut op);
    op.found
}

fn stamp_first_geometry<Message, Theme, Renderer>(
    child: &mut Element<'_, Message, Theme, Renderer>,
    tree: &mut Tree,
    layout: Layout,
    renderer: &Renderer,
    cell: &ScrollGeometryCell,
) where
    Renderer: iced::advanced::Renderer,
{
    if let Some(geometry) = first_scroll_geometry(child, tree, layout, renderer) {
        cell.set(geometry);
    }
}

/// Transparent wrapper backing [`publish_geometry_of`].
struct PublishGeometry<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    cell: ScrollGeometryCell,
}

struct PublishTranslation<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer> {
    child: Element<'a, Message, Theme, Renderer>,
    cell: ScrollTranslation,
    built_translation: Vector,
}

impl<'a, Message, Theme, Renderer> PublishTranslation<'a, Message, Theme, Renderer> {
    fn new(
        child: impl Into<Element<'a, Message, Theme, Renderer>>,
        cell: ScrollTranslation,
    ) -> Self {
        Self {
            child: child.into(),
            built_translation: cell.get(),
            cell,
        }
    }
}

pub(crate) fn publish_translation_of<'a, Message>(
    child: impl Into<Element<'a, Message>>,
    cell: ScrollTranslation,
) -> Element<'a, Message>
where
    Message: 'a,
{
    Element::new(PublishTranslation::new(child, cell))
}

fn stamp_first_translation<Message, Theme, Renderer>(
    child: &mut Element<'_, Message, Theme, Renderer>,
    tree: &mut Tree,
    layout: Layout,
    renderer: &Renderer,
    cell: &ScrollTranslation,
) -> bool
where
    Renderer: iced::advanced::Renderer,
{
    use iced::advanced::widget::operation::scrollable::Scrollable;
    struct FirstTranslation {
        found: Option<Vector>,
    }
    impl Operation for FirstTranslation {
        fn scrollable(
            &mut self,
            _id: Option<&iced::advanced::widget::Id>,
            _bounds: Rectangle,
            _content_bounds: Size,
            translation: Vector,
            _state: &mut dyn Scrollable,
        ) {
            if self.found.is_none() {
                self.found = Some(translation);
            }
        }
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            if self.found.is_none() {
                operate(self);
            }
        }
    }
    let mut op = FirstTranslation { found: None };
    child
        .as_widget_mut()
        .operate(tree, layout, &layout.bounds(), renderer, &mut op);
    if let Some(t) = op.found {
        cell.set(t);
        true
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Overlay scrollbar style
// ---------------------------------------------------------------------------

/// The overlay [`Scrollbar`] every scroll primitive installs: a wide invisible
/// grab/reveal band ([`hit_width`](enzui_theme::ScrollbarTokens::hit_width))
/// around a narrow painted thumb
/// ([`width`](enzui_theme::ScrollbarTokens::width)). [`min_thumb_length`](enzui_theme::ScrollbarTokens::min_thumb_length)
/// floors long axes; iced clamps short tracks, mapping offsets over remaining
/// travels so both endpoints stay exact. Track-filling thumbs have no movement
/// axes, ignoring drags. Single seams keep dimensions undrifted + uncollapsed:
/// paint-shrunk bands (hidden-paint thumbs with paint-wide grab zones)
/// redden `tests::built_bar_band_is_hit_width_not_the_painted_thumb`.
pub fn overlay_scrollbar() -> iced::widget::scrollable::Scrollbar {
    overlay_scrollbar_with_tokens(enzui_theme::text_defaults::scrollbar())
}

/// Build the framework overlay scrollbar from an explicit token set.
///
/// Use this when a custom theme changes dimensional scrollbar tokens; iced's
/// layout phase does not receive the runtime theme, so geometry must travel as
/// build-time data.
pub fn overlay_scrollbar_with_tokens(
    sb: enzui_theme::ScrollbarTokens,
) -> iced::widget::scrollable::Scrollbar {
    let sb = sb.normalized();
    iced::widget::scrollable::Scrollbar::default()
        .width(sb.hit_width)
        .scroller_width(sb.width)
        .scroller_min_length(sb.min_thumb_length)
}

/// Keeps the wheel inside `scrollable` (one built with [`enz_scrollable`])
/// while it overflows on `axis`, as [`enz_scroll_view`] does by default: a
/// list scrolled to its end does not go on to scroll the page around it.
/// Content that fits, and the other axis, still pass the wheel on.
pub fn contain_wheel<'a, Message: 'a>(
    scrollable: impl Into<Element<'a, Message, iced::Theme, iced::Renderer>>,
    axis: Axis,
) -> Element<'a, Message, iced::Theme, iced::Renderer> {
    let mut wrapper = KeyboardScroll::new(scrollable, axis);
    wrapper.keyboard_scroll = false;
    Element::new(wrapper)
}

/// Overlay-policy [`Scrollable`] shim. Every crate-scrolling widget builds
/// [`Scrollable`]s through here: framework overlay policies sans hand-
/// application (5px painted thumbs in 15px proximity bands, rest-transparent, trailing-edge-revealed,
/// layout-laneless). Pre-shim precedents: two hand-applying adopters
/// ([`virtual_list`](crate::virtual_list), [`virtual_grid`](crate::virtual_grid));
/// seven bare-`iced::widget::scrollable(..)` builders getting 10px always-on
/// defaults (wrong widths/visibilities; one reserved-gutter content-width-
/// changer — `shortcut_list.rs`, now explicit exceptions; population gates in
/// `crates/enzui-widgets/src/scrollable_population.rs`).
///
/// # Overriding the direction
///
/// Vertical [`Direction::Vertical`] + [`overlay_scrollbar()`] installs.
/// Horizontal callers pass their own directions.
///
/// Reuse [`overlay_scrollbar()`] over `Scrollbar::default()`s: defaults widen
/// bars to 10px + lose 12px proximity bands (population gates name violators).
/// Style functions install [`overlay_scrollbar_style`]; backdrop-aware
/// variants ([`overlay_scrollbar_style_over]) get own `.style(..)`s. Hence
/// population gates assert CONSTRUCTION-SITE shim passage, never style-function
/// names.
pub fn enz_scrollable<'a, Message: 'a>(
    content: impl Into<Element<'a, Message, iced::Theme, iced::Renderer>>,
) -> Scrollable<'a, Message, iced::Theme, iced::Renderer> {
    Scrollable::new(content)
        .direction(Direction::Vertical(overlay_scrollbar()))
        .smooth_scroll_policy(smooth_scroll_allowed)
        .style(overlay_scrollbar_style)
}

/// Whether the theme's motion policy lets a scrollable ease a wheel step
/// over a few frames instead of landing it at once.
///
/// Every framework scrollable ([`enz_scroll_view`], [`enz_scrollable`])
/// decides smooth scrolling with this at each draw, so the reduced-motion
/// preference (`MotionScale::reduced_motion`, see
/// `theme_file::integration::ThemeIntegration`) turns it off everywhere at
/// once. Touchpad scrolling is always immediate, and scroll operations
/// that name `Animation::Instant`
/// stay instant.
pub fn smooth_scroll_allowed(theme: &iced::Theme) -> bool {
    !crate::theme_lookup::motion(theme).reduced_motion
}

/// Style function for the overlay scrollbar: exactly two visible
/// appearances plus hidden: Active (cursor outside regions, or inside but
/// off trailing edges) → fully transparent (hidden bars); Hovered with
/// `is_*_scrollbar_hovered` flags → [`width`](enzui_theme::ScrollbarTokens::width) +
/// [`alpha`](enzui_theme::ScrollbarTokens::alpha) thumbs; axis-Dragged → same
/// widths, [`alpha_hover`](enzui_theme::ScrollbarTokens::alpha_hover).
///
/// # Why the reveal now keys off the per-axis flag
///
/// Bars stay hidden until cursors reach bar zones, never on any regional
/// `Status::Hovered`. Strict painted-thumb hit tests unused: upstream
/// `is_vertical_scrollbar_hovered`s are not painted-thumb tests.
/// `Scrollbars::is_mouse_over` → `internals::Scrollbar::is_mouse_over` tests
/// `total_bounds.contains(cursor)`
/// (`iced/widget/src/scrollable.rs`); `total_bounds` =
/// `width.max(scroller_width) + 2 * margin` by FULL track lengths
/// (`iced/widget/src/scrollable.rs`). Below-`From`s set
/// structural widths to [`hit_width`](enzui_theme::ScrollbarTokens::hit_width)s (15px) while
/// closures paint [`width`](enzui_theme::ScrollbarTokens::width)s (5px): flags are already 15px
/// full-height proximity bands around 5px thumbs — requested "very close /
/// above" sans `iced` changes/hand-rolled geometry. Iced anchors
/// `total_scrollbar_bounds` at region trailing edges (`x = bounds.x +
/// bounds.width - total_width`): beyond-paint band pixels land content-facing
/// (never past window edges pointerless rooms). Proximity-band widenings
/// reach into content. Thumbs never hover-widen: widening needs painted ≡
/// structural widths — exactly narrower-bar/grab-target-shrinking couplings.
/// Opacity carries drag feedback instead. `pub(crate)`s share styles with
/// `scroll_column`/`horizontal_scroll_view` sans duplication.
///
/// [`width`](enzui_theme::ScrollbarTokens::width): enzui_theme::ScrollbarTokens::width
/// [`hit_width`](enzui_theme::ScrollbarTokens::hit_width): enzui_theme::ScrollbarTokens::hit_width
/// [`alpha`](enzui_theme::ScrollbarTokens::alpha): enzui_theme::ScrollbarTokens::alpha
/// [`alpha_hover`](enzui_theme::ScrollbarTokens::alpha_hover): enzui_theme::ScrollbarTokens::alpha_hover
///
/// ``: [`overlay_scrollbar_style_over`] takes optional app-painted backdrops;
/// thumb inks resolve vs app surfaces over palette-assumed ones.
pub fn overlay_scrollbar_style(
    theme: &iced::Theme,
    status: iced::widget::scrollable::Status,
) -> iced::widget::scrollable::Style {
    overlay_scrollbar_style_over(theme, status, None)
}

/// [`overlay_scrollbar_style`] with an optional app-painted `backdrop`.
/// `None` = legacy palette-relative (`palette.text`) thumbs. `Some(bg)` =
/// [`scroller_ink_over`]-resolved inks vs `bg`: legible capsules over palette-
/// unknown surfaces. Colour-only changes: hidden/revealed/dragged states, grab
/// bands, capsule radii identical to `None` paths.
pub fn overlay_scrollbar_style_over(
    theme: &iced::Theme,
    status: iced::widget::scrollable::Status,
    backdrop: Option<Color>,
) -> iced::widget::scrollable::Style {
    use iced::widget::scrollable::{Rail, Scroller, Status, Style};

    let sb = crate::theme_lookup::scrollbar(theme);
    let palette = crate::theme_lookup::palette(theme);

    // The scroller colour is derived from the text colour so it works on
    // both dark and light backgrounds -- unless the app named the backdrop it
    // painted, in which case the ink is resolved against that instead.
    let scroller_base = match backdrop {
        Some(bg) => scroller_ink_over(bg),
        None => palette.text,
    };

    // A fully transparent rail: no visible track, and no thumb either. This is
    // the hidden state, and it is also the per-axis "this axis is not being
    // pointed at" state.
    let hidden_rail = Rail {
        background: None,
        border: Border::default(),
        scroller: Scroller {
            background: Color::TRANSPARENT.into(),
            border: Border::default().rounded(sb.border_radius),
        },
    };

    // A revealed thumb at `alpha`. Note there is no visible rail behind it.
    let revealed_rail = |alpha: f32| Rail {
        background: None,
        border: Border::default(),
        scroller: Scroller {
            background: scroller_base.scale_alpha(alpha).into(),
            border: Border::default().rounded(sb.border_radius),
        },
    };

    // Auto-scroll overlay appearance (unchanged from upstream default).
    let auto_scroll = iced::widget::scrollable::AutoScroll {
        background: palette.surface.scale_alpha(0.9).into(),
        border: Border::default()
            .rounded(u32::MAX)
            .width(1)
            .color(palette.text.scale_alpha(0.8)),
        shadow: iced::Shadow {
            color: Color {
                a: 0.7,
                ..palette.shadow
            },
            offset: Vector::ZERO,
            blur_radius: 2.0,
        },
        icon: palette.text.scale_alpha(0.8),
    };

    // Which axes are being pointed at / dragged, and at what opacity they
    // should paint. `Active` means the cursor is not in the region at all, so
    // neither axis reveals.
    // A desktop that turns overlay scrolling off keeps the bars visible.
    let always = !enzui_core::desktop_settings::overlay_scrollbars();
    let (show_vertical, show_horizontal, alpha) = match status {
        Status::Active { .. } => (always, always, sb.alpha),
        // the per-axis flag, not the bare `Hovered`. `Hovered` alone is
        // "somewhere in the region", which is the bug this replaces.
        Status::Hovered {
            is_vertical_scrollbar_hovered,
            is_horizontal_scrollbar_hovered,
            ..
        } => (
            always || is_vertical_scrollbar_hovered,
            always || is_horizontal_scrollbar_hovered,
            sb.alpha,
        ),
        Status::Dragged {
            is_vertical_scrollbar_dragged,
            is_horizontal_scrollbar_dragged,
            ..
        } => (
            is_vertical_scrollbar_dragged,
            is_horizontal_scrollbar_dragged,
            sb.alpha_hover,
        ),
    };

    Style {
        container: iced::widget::container::Style::default(),
        vertical_rail: if show_vertical {
            revealed_rail(alpha)
        } else {
            hidden_rail
        },
        horizontal_rail: if show_horizontal {
            revealed_rail(alpha)
        } else {
            hidden_rail
        },
        gap: None,
        auto_scroll,
    }
}

/// The overlay thumb's ink, resolved for contrast against the colour the
/// app actually painted behind the surface. Default `palette.text` inks assume
/// palette-`surface` undersides. Self-painted canvases (Reader reading
/// backgrounds, terminal backgrounds) collapse thumbs to ~1.0:1 (invisible BY
/// ARITHMETIC, not accident). Actual backdrops pick inks by own relative
/// luminance: near-black on light pages, near-white on dark. Caller-applied
/// revealed/dragged alphas keep translucent capsules — now legible over app
/// surfaces over palette-assumed ones. Deliberately NEUTRAL (never tinted
/// palette colours): arbitrary-hue backdrops meet maximally-opposite-
/// luminance neutrals robustly (all eight reading backgrounds + terminals).
pub fn scroller_ink_over(backdrop: Color) -> Color {
    // WCAG relative luminance of the backdrop.
    let lin = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let l = 0.2126 * lin(backdrop.r) + 0.7152 * lin(backdrop.g) + 0.0722 * lin(backdrop.b);
    // Opposite-luminance neutral. The 0.5 split matches `Palette::is_dark`.
    if l < 0.5 {
        // Dark backdrop -> light thumb.
        Color::from_rgb(0.92, 0.92, 0.92)
    } else {
        // Light backdrop -> dark thumb.
        Color::from_rgb(0.08, 0.08, 0.08)
    }
}

impl<'a, Message> EnzScrollView<'a, Message>
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

impl<'a, Message> From<EnzScrollView<'a, Message>> for Element<'a, Message>
where
    Message: 'a,
{
    fn from(w: EnzScrollView<'a, Message>) -> Self {
        let padding = Padding {
            top: w.inset_top,
            right: w.inset_right,
            bottom: w.inset_bottom,
            left: w.inset_left,
        };
        let padded: Element<'a, Message> = if padding == Padding::ZERO {
            w.child
        } else {
            container(w.child).padding(padding).into()
        };
        let bar = if w.hide_scrollbar {
            Scrollbar::hidden()
        } else {
            // The wider hit band must remain independent of the painted thumb width.
            overlay_scrollbar_with_tokens(w.scrollbar_tokens)
        };
        let mut sv = Scrollable::new(padded)
            .direction(w.axis.into_direction(bar))
            .scrollbar_insets(w.scrollbar_insets)
            .smooth_scroll_policy(smooth_scroll_allowed)
            .width(w.width)
            .height(w.height);
        // when the app named the colour it paints behind this surface,
        // resolve the thumb ink against that rather than the palette's assumed
        // surface, so the overlay capsule stays legible over an app-painted
        // canvas (an app's reading backgrounds, a terminal). Otherwise the
        // plain function pointer keeps the legacy palette-relative thumb.
        sv = match w.backdrop {
            Some(bg) => sv.style(move |theme: &iced::Theme, status| {
                overlay_scrollbar_style_over(theme, status, Some(bg))
            }),
            None => sv.style(overlay_scrollbar_style),
        };
        if let Some(id) = w.id {
            sv = sv.id(id);
        }
        if let Some(cb) = w.on_scroll {
            sv = sv.on_scroll(move |scroll: iced::widget::scrollable::Scroll| cb(scroll.viewport));
        }
        if let Some(anchor) = w.anchor {
            sv = sv.anchor(anchor);
        }
        if let Some(offset) = w.initial_offset {
            sv = sv.initial_offset(offset);
        }
        // if a translation cell was attached, wrap the scrollable so its
        // live offset is stamped into the cell every pass. `PublishTranslation`
        // is transparent, so this composes cleanly under `KeyboardScroll`; the
        // probe still finds the inner `Scrollable` by walking the subtree.
        let inner: Element<'a, Message> = match w.publish_translation {
            Some(cell) => Element::new(PublishTranslation::new(sv, cell)),
            None => sv.into(),
        };
        Element::new(KeyboardScroll {
            child: inner,
            axis: w.axis,
            keyboard_scroll: w.keyboard_scroll,
            wheel_chaining: w.wheel_chaining,
        })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
