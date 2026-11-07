//! The keyboard-and-swipe event wrapper [`CarouselInput`] and its retained
//! [`CarouselState`].
//!
//! `into_element` (in [`super::view`]) composes the visual tree, then wraps it
//! in this [`Widget`] so arrow / Home / End paging and swipe-to-page work over
//! the composed body. The wrapper owns the single [`SwipeGesture`], the
//! roving-focus transfer across page changes, and keys the retained
//! child [`Tree`] by [`PageId`] so a page change never inherits the previous
//! page's widget state.

use super::tokens::SWIPE_ARBITRATION_DEADBAND;
use crate::swipe::{Arbitration, SwipeGesture, SwipePhase, SwipePolicy};
use enzui_core::focus::{PressFocus, press_focus_from_cursor};
use enzui_core::identity::derive_id;
use enzui_core::paging::{PageId, PageRequest};
use enzui_core::placement::Direction;
use iced::advanced::layout::Measure as _;
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::keyboard;
use iced::{Element, Event, Length, Rectangle, Size, touch};

/// Live drag state for swipe-to-page.
///
/// `pub(super)` so the carousel's test module (in the crate root of this
/// widget) can downcast the retained tree state and assert on `swipe` /
/// `active_page`.
#[derive(Debug, Default)]
pub(super) struct CarouselState {
    /// A queued focus target to install on the *next* `operate` pass.
    ///
    /// Set from two sites, consumed once:
    ///
    /// * [`CarouselInput::update`] on an arrow / Home / End that publishes a
    ///   paging message — the new selected dot's id, so the roving tab stop
    ///   follows the selection.
    /// * [`CarouselInput::diff`] on a page change that occurred while the
    ///   strip held keyboard focus — the app's own model update (a `Selected`
    ///   from swipe, a pointer click on a dot, an external
    ///   `focus_button_maybe_with` press) rebuilt the strip with the previous
    ///   focus owner now `.embedded(true)`, which `focus_button::operate` will
    ///   unfocus. Without this transfer, focus is silently lost across
    ///   *every* page change, not just arrow-key ones.
    ///
    /// Consumed by [`CarouselInput::operate`], which walks the child subtree
    /// with [`operation::focusable::focus`]. That operation is destructive
    /// against other focusables *inside this subtree*, which is intended:
    /// the strip is a roving-stop group, so at most one dot
    /// is focused at a time. It is not run against siblings, because
    /// `operate` scopes it to `tree.children[0]`.
    pub(super) pending_focus_target: Option<iced::advanced::widget::Id>,
    /// The swipe in flight, if any.
    ///
    /// Origin and finger ownership both live in here. Keeping separate
    /// `press_origin: Option<Point>` and `pressed_finger: Option<touch::Finger>`
    /// values synchronized across six event sites would duplicate the
    /// finger-ownership rule here and in the sheet. [`SwipeGesture`] answers
    /// "is this input mine"
    /// once, for every widget that swipes.
    pub(super) swipe: SwipeGesture,
    /// Whether the carousel currently holds keyboard focus.
    pub(super) focused: bool,
    /// Which page the retained child subtree was built for.
    ///
    /// The carousel shows exactly one page at a time, so the active page always
    /// occupies child slot 0 -- and `Tree::diff_children` reconciles children by
    /// position. Position alone therefore cannot tell "the same page, rebuilt"
    /// from "a different page in the same slot", so the retained widget state at
    /// slot 0 would survive a page change. For two structurally identical pages
    /// (two `text_input`s, two scrollables) the incoming page would inherit the
    /// outgoing page's cursor, selection and scroll offset -- iced only rebuilds
    /// a subtree when the widget *type* changes, and here the type is identical.
    ///
    /// Selection in this widget is keyed by [`PageId`] everywhere else (see the
    /// module header and the per-dot ids), so the retained tree is keyed the same
    /// way: `diff` compares this against the page now being rendered and discards
    /// the child subtree when they differ, which forces a fresh `Tree` for the
    /// incoming page.
    ///
    /// `None` means nothing has been rendered yet (or the carousel is empty), in
    /// which case there is no retained state to invalidate.
    pub(super) active_page: Option<PageId>,
    /// A newly shown page fading in, and the latest presented frame.
    pub(super) page_fade: crate::motion::Swell,
    pub(super) frame_at: Option<iced::time::Instant>,
}

/// Pure availability probe: `resolve_request(request)` returns the [`PageId`]
/// that `request` would select, without invoking `on_select`. Shared between
/// the view-time pointer-side factory and the event-time keyboard path
/// so both cannot disagree about the target page.
pub(super) type ResolveRequest<'a> = dyn Fn(&PageRequest) -> Option<PageId> + 'a;

/// The complete typed vocabulary of named keys claimed for carousel paging.
///
/// Both the production decoder and its modifier matrix iterate this table, so a
/// newly claimed key automatically joins the matrix instead of requiring a
/// second hand-maintained subject list.
pub(super) const PAGING_KEY_CLAIMS: &[(keyboard::key::Named, PageRequest)] = &[
    (keyboard::key::Named::ArrowRight, PageRequest::Next),
    (keyboard::key::Named::ArrowLeft, PageRequest::Previous),
    (keyboard::key::Named::Home, PageRequest::First),
    (keyboard::key::Named::End, PageRequest::Last),
];

/// Decode a carousel-owned paging key.
///
/// Ctrl, Alt, and Logo mark application chords and are deliberately declined;
/// Shift remains widget-owned and preserves the bare key's request. Keeping
/// this decision separate from availability is important: a key at a clamped
/// boundary is still a carousel key, even though applying its request publishes
/// nothing.
pub(super) fn paging_request_for_key(
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
) -> Option<PageRequest> {
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }

    let keyboard::Key::Named(named) = key else {
        return None;
    };
    PAGING_KEY_CLAIMS
        .iter()
        .find(|(claimed, _)| claimed == named)
        .map(|(_, request)| request.clone())
}

/// Wraps the composed carousel with arrow-key paging and swipe handling.
///
/// Constructed by [`EnzCarousel::into_element`](super::EnzCarousel) in
/// [`super::view`], so its fields are `pub(super)`.
pub(super) struct CarouselInput<'a, Message> {
    pub(super) content: Element<'a, Message>,
    pub(super) request: Box<dyn Fn(PageRequest) -> Option<Message> + 'a>,
    /// See [`ResolveRequest`].
    pub(super) resolve_request: Box<ResolveRequest<'a>>,
    /// The stable AT-id namespace `enz-carousel-<page-id>`. Kept on the wrapper
    /// so `update` can build the target id for a paging request.
    pub(super) id_prefix: String,
    pub(super) swipe_enabled: bool,
    pub(super) enabled: bool,
    /// The swipe rule: horizontal, either direction, no edge strip, committing
    /// on the token threshold. Built once in [`EnzCarousel::into_element`] from
    /// [`CarouselTokens::threshold`].
    pub(super) policy: SwipePolicy,
    pub(super) width: Length,
    pub(super) height: Length,
    /// The page this `content` was composed for, used to key the retained tree.
    ///
    /// `None` when the carousel resolved to no page at all (empty list).
    pub(super) active_page: Option<PageId>,
    /// The shown page's opacity, which [`PageFade`] reads as it draws.
    pub(super) page_alpha: std::rc::Rc<std::cell::Cell<f32>>,
}

impl<Message> CarouselInput<'_, Message> {
    /// Turn a resolved swipe into a published paging message.
    ///
    /// Only [`SwipePhase::Committed`] publishes: `Declined` means the input was
    /// someone else's, and `Cancelled` means the gesture ended without meeting
    /// the rule. Both are silent, which is the property that keeps a lost
    /// pointer from paging the carousel.
    fn resolve(&self, phase: SwipePhase, shell: &mut Shell<'_, Message>) {
        let SwipePhase::Committed { distance, .. } = phase else {
            return;
        };
        if let Some(request) = PageRequest::for_swipe(distance)
            && let Some(message) = (self.request)(request)
        {
            shell.publish(message);
            shell.capture_event();
        }
    }
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for CarouselInput<'_, Message>
where
    Message: Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<CarouselState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(CarouselState {
            page_fade: crate::motion::Swell::snap(true),
            ..CarouselState::default()
        })
    }

    fn diff(&mut self, tree: &mut Tree) {
        // Key the retained subtree by PageId, not by slot.
        //
        // `diff_children` zips children by position, and the active page is
        // always position 0, so without this the incoming page inherits the
        // outgoing page's retained state whenever the two are the same widget
        // type. Dropping the children forces `diff_children` to build a fresh
        // `Tree` for the new page instead of reconciling into the old one.
        let state = tree.state.downcast_mut::<CarouselState>();
        if state.active_page != self.active_page {
            // focus transfer. If the strip held keyboard focus at
            // the moment the page changed, queue a focus operation onto the
            // new selected dot's id — otherwise the outgoing selected
            // indicator (`.focusable_without_activation(true)`) is rebuilt
            // as `.embedded(true)`, and `focus_button::operate` will silently
            // unfocus it (the `embedded` arm of `focus_button::widget`'s
            // `operate` drops focus). The transfer must
            // happen *before* `tree.children.clear()` (which drops the state
            // we are probing) and be consumed on the next `operate` pass
            // (which happens on the same frame this diff completes).
            //
            // Detection: any `focus_button::State` inside our subtree that
            // reports `is_focused()`. Reuses the crate primitive rather than
            // re-inventing a fifth focus-holder walk: every widget that
            // needs one shares this shape.
            //
            // Rule for deterministic transfer: focus moves to the
            // new selected page's dot — the roving tab stop follows the
            // selection, which is the property "roving" means. That single
            // rule covers every trigger of a page change (arrow, Home/End,
            // swipe, pointer click on another dot, external model update),
            // so an AT user's keyboard focus never lands nowhere. If the
            // strip *did not* hold focus we do not synthesise a focus — a
            // page change from an unfocused input (a swipe, an app-driven
            // model change) must not yank focus from wherever the user is.
            let strip_had_focus = tree
                .children
                .first()
                .is_some_and(crate::focus_button::any_focused_in);
            if let (true, Some(active)) = (strip_had_focus, self.active_page.as_ref()) {
                state.pending_focus_target = Some(derive_id(&self.id_prefix, active.as_str()));
            }
            // Ordering: cancel the in-flight swipe before the
            // new page is adopted. `page_changed` is read from the CURRENT
            // state, so the cancel is gated on the real transition; do the
            // cancel first, then the mutation. Swapping the two statements
            // (adopt the page, then decide whether to cancel) always reads
            // "unchanged" and leaves the gesture pointing at a page that no
            // longer exists — `an_active_page_change_cancels_an_in_flight_swipe`
            // fails under that swap.
            state.swipe.clear();
            // A page replacing another fades in; the first page is simply there.
            if state.active_page.is_some() {
                state.page_fade = crate::motion::Swell::snap(false);
            }
            state.active_page.clone_from(&self.active_page);
            tree.children.clear();
        }
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn is_void(&self) -> bool {
        // Defaulted trait method: delegate so a `Void` child stays void and
        // Row/Column/Stack still filter it out.
        self.content.as_widget().is_void()
    }

    fn size(&self) -> Size<Length> {
        Size::new(self.width, self.height)
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let limits = limits.width(self.width).height(self.height);
        let content =
            self.content
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, &limits);
        let size = limits.resolve(self.width, self.height, content.size());
        layout::Node::with_children(size, vec![content]).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        // Consume a queued focus transfer before delegating to the
        // caller's operation. This runs before the caller sees the tree, so a
        // `find_focused` or `count()` pass from the runtime observes the
        // just-installed focus rather than the stale (empty) one — the
        // property that keeps Tab traversal ordered when the user pages while
        // focused. The focus operation walks only OUR subtree
        // (`tree.children[0]`), so its destructive `_ =>` arm cannot unfocus
        // a widget outside the carousel.
        let target = tree
            .state
            .downcast_mut::<CarouselState>()
            .pending_focus_target
            .take();
        if let (Some(target), Some(child)) = (target, layout.children(&tree.children).next()) {
            let mut focus_op = iced::advanced::widget::operation::focusable::focus::<()>(target);
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                child,
                viewport,
                renderer,
                &mut focus_op as &mut dyn Operation,
            );
        }
        if let Some(child) = layout.children(&tree.children).next() {
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                child,
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
        {
            let CarouselState {
                page_fade,
                frame_at,
                ..
            } = tree.state.downcast_mut::<CarouselState>();
            page_fade.track(frame_at, event, true, PAGE_FADE_MS, shell);
        }
        // Content first: an indicator or chrome click belongs to that control,
        // and only an unclaimed press on the page body can begin a swipe.
        if let Some(child) = layout.children(&tree.children).next() {
            self.content.as_widget_mut().update(
                &mut tree.children[0],
                event,
                child,
                cursor,
                renderer,
                shell,
                viewport,
            );
        }

        if !self.enabled {
            return;
        }

        let bounds = layout.bounds();
        // Probe whether any focus_button descendant — the roving tab
        // stop, in practice the selected indicator — currently holds
        // keyboard focus. This is the input to the arrow/Home/End gate
        // below, and to `state.pending_focus_target`, so it must be sampled
        // before the mutable state borrow (they alias `tree`).
        let strip_has_focus = tree
            .children
            .first()
            .is_some_and(crate::focus_button::any_focused_in);
        let state = tree.state.downcast_mut::<CarouselState>();

        // Pointer focus is EXCLUSIVE. Every focusable decides, from
        // the one press position, whether it was the control hit — at most one
        // was, so the press that focuses this carousel is the same press that
        // unfocuses its siblings.
        //
        // Three things about the placement are deliberate:
        //
        // 1. It sits before the `is_event_captured()` bail-out below. The hit
        //    control captures the press, so a sibling reached afterwards sees a
        //    captured shell — a focus drop written below the guard would skip
        //    exactly the controls that need clearing. This is the ordering
        //    `focus_button::widget`'s `update` establishes (the focus drop sits
        //    above the `is_event_captured` bail-out) and
        //    `focus_is_dropped_even_when_an_earlier_sibling_captured_the_press`
        //    pins.
        // 2. It is not gated on `swipe_enabled`. Focus is not a swipe concern:
        //    arrow-key paging is gated on `state.focused` and works whether or
        //    not swipe is on, so a swipe-disabled carousel must still do its
        //    focus bookkeeping. Only `press_origin` — which is genuinely swipe
        //    state — stays behind that gate.
        // 3. `Leave` (a press with no resolvable position) changes nothing.
        //    Sweeping focus there would regress touch-only devices, which
        //    report `Cursor::Unavailable`.
        if let Some(decision) = press_focus_from_cursor(event, cursor, bounds) {
            let was_focused = state.focused;
            match decision {
                PressFocus::Take => state.focused = true,
                PressFocus::Drop => state.focused = false,
                PressFocus::Leave => {}
            }
            if state.focused != was_focused {
                // The focus flag changed on a control that may be nowhere near
                // the pointer, so the frame has to be repainted.
                shell.request_redraw();
            }
        }

        match event {
            // ----- Swipe -------------------------------------------------
            //
            // Mouse and touch differ only in *who* the source is: the mouse
            // passes `None` (there is one of it, so it has no identity) and a
            // finger passes its id. Everything else — the second finger that
            // must not steal a gesture in flight, the foreign lift that must
            // not resolve one, the raw (unclamped) release position, the
            // cancel-is-never-a-commit rule — is `SwipeGesture`'s, not this
            // widget's.
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if shell.is_event_captured() || !self.swipe_enabled {
                    return;
                }
                if let Some(position) = cursor.position() {
                    // Focus was already taken above by the exclusive sweep;
                    // only the swipe origin is recorded here. `accepts_press`
                    // does the bounds test, so a press outside is declined
                    // rather than silently starting a gesture.
                    //
                    // The mouse press is claimed immediately: a desktop
                    // `Scrollable` scrolls by wheel or scrollbar drag, not by a
                    // left-button body drag, so there is no cross-axis rival to
                    // arbitrate against. The carousel has no edge strip, so the
                    // direction argument is inert here.
                    let _ = state.swipe.press(
                        &self.policy,
                        position,
                        None,
                        bounds,
                        Direction::LeftToRight,
                    );
                }
            }

            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                // Keep the mouse gesture's velocity sample fresh so a mouse
                // flick can carry speed. Distance-only carousel ignores it, but
                // the sample keeps the state machine honest for a policy that
                // adds velocity later. No `dt` source on the mouse path, so
                // velocity stays 0 here.
                let _ = state.swipe.moved(&self.policy, *position, None, None);
            }

            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                // Raw position, not `position_over`: a swipe that ends outside
                // the carousel is still a swipe, and clamping the release to
                // the bounds would shorten it into a cancel.
                let Some(position) = cursor.position() else {
                    return;
                };
                // A press owned by a finger stays held: the mouse button
                // coming up says nothing about that finger, and `release`
                // declines it because the ids differ.
                self.resolve(
                    state.swipe.release(&self.policy, position, None, None),
                    shell,
                );
            }

            Event::Mouse(mouse::Event::CursorLeft) => {
                // Losing the pointer cancels rather than committing whatever
                // travel happened to have accumulated — but only if the mouse
                // owns this swipe (a finger's gesture declines this).
                let _ = state.swipe.cancel(None);
            }

            Event::Touch(touch::Event::FingerPressed { id, position }) => {
                if !self.swipe_enabled {
                    return;
                }
                // A descendant `Scrollable` captures the touch
                // press (vendored iced's `scrollable::update`), so gating on
                // `is_event_captured()` here would make a carousel with a
                // scrollable page body unswipeable in the middle — exactly where
                // users touch first. Instead the press begins PROVISIONAL: it
                // is tracked but claims nothing and captures nothing until
                // `arbitrate` (on the first decisive move below) decides the
                // dominant axis. Neither the swipe nor the scroll wins on tree
                // order; the axis decides.
                //
                // The FINGER's coordinates, not the cursor's: on a touch-only
                // device the cursor is unavailable or stale.
                let _ = state.swipe.press_provisional(
                    &self.policy,
                    *position,
                    Some(*id),
                    bounds,
                    Direction::LeftToRight,
                );
            }

            Event::Touch(touch::Event::FingerMoved { id, position }) => {
                // Feed the sample for velocity/distance tracking first —
                // `moved` declines a foreign finger, so this is a no-op unless
                // this finger owns the gesture.
                let _ = state.swipe.moved(&self.policy, *position, Some(*id), None);
                // Then arbitrate the contested axis. On `Claimed` the swipe is
                // ours and we capture, which stops the descendant scrollable
                // from continuing to scroll under the same finger. On `Yielded`
                // the gesture is already cleared and the scroll keeps it.
                if let Arbitration::Claimed = state.swipe.arbitrate(
                    &self.policy,
                    *position,
                    Some(*id),
                    SWIPE_ARBITRATION_DEADBAND,
                ) {
                    shell.capture_event();
                }
            }

            Event::Touch(touch::Event::FingerLifted { id, position }) => {
                self.resolve(
                    state
                        .swipe
                        .release(&self.policy, *position, Some(*id), None),
                    shell,
                );
            }

            // A CANCELLED touch is not a commit. `FingerLost` means the
            // sequence was aborted — the gesture was stolen by the compositor,
            // or the finger left the surface — so the swipe is abandoned and
            // nothing is published. Same split as the `FingerLost` arm of
            // `EnzStepper`'s `update` and
            // upstream iced's own button (`iced_widget`'s `button` module).
            Event::Touch(touch::Event::FingerLost { id, .. }) => {
                let _ = state.swipe.cancel(Some(*id));
            }

            // A swipe in flight when the window loses focus
            // must be abandoned, not left to resume against a page (or a
            // configuration) that may have changed while the window was away.
            // This is the same policy the sheet applies
            // (`sheet.rs` `Window::Unfocused -> Cancelled`) and keeps the three
            // gesture subsystems consistent. `cancel_any` ignores ownership on
            // purpose: unfocus names no finger and ends everything.
            Event::Window(iced::window::Event::Unfocused) => {
                let _ = state.swipe.cancel_any();
            }

            Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
                // The arrow / Home / End gate accepts either the
                // pointer-focused whole-carousel flag or a focused indicator
                // descendant. Gating on `state.focused` alone would accept
                // only what [`press_focus`] above sets — so a user who
                // reached the strip by Tab (never touched the pointer) got no
                // keyboard paging at all. `strip_has_focus` closes that gap
                // by asking the crate's one focus-holder walk (`focus_button::
                // any_focused_in`) directly. The two flags are ORed, not
                // ANDed: a pointer-focused carousel with no focused indicator
                // with no focused indicator is still keyboard-pageable.
                if !state.focused && !strip_has_focus {
                    return;
                }
                // Decode before probing availability. Application chords must
                // remain unclaimed even when the corresponding bare request
                // could move; Shift deliberately keeps the widget action.
                let Some(request) = paging_request_for_key(key, *modifiers) else {
                    return;
                };
                // Resolve the target dot id before consuming
                // `request`. `resolve_request` is pure and side-effect free
                // (it never runs `on_select`), so calling it here first is
                // strictly cheaper than any alternative that clones the
                // request enum. The target is the id the new selected dot
                // will publish after the app processes the paging message
                // and rebuilds. We queue it here (not only in `diff`)
                // because a keyboard user's focus should follow the arrow
                // regardless of whether the strip currently holds focus —
                // the ergonomic property tests observe is "press Right, next
                // dot is focused".
                let target_id = (self.resolve_request)(&request)
                    .map(|pid| derive_id(&self.id_prefix, pid.as_str()));
                if let Some(message) = (self.request)(request) {
                    if let Some(target_id) = target_id {
                        state.pending_focus_target = Some(target_id);
                    }
                    shell.publish(message);
                    shell.capture_event();
                }
            }

            _ => {}
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
        let state = tree.state.downcast_ref::<CarouselState>();
        self.page_alpha
            .set(state.page_fade.paint(state.frame_at, PAGE_FADE_MS, theme));
        if let Some(child) = layout.children(&tree.children).next() {
            self.content.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                child,
                cursor,
                viewport,
            );
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
        layout
            .children(&tree.children)
            .next()
            .map(|child| {
                self.content.as_widget().mouse_interaction(
                    &tree.children[0],
                    child,
                    cursor,
                    viewport,
                    renderer,
                )
            })
            .unwrap_or_default()
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
        let Some(child) = layout.children(&tree.children).next() else {
            return Vec::new();
        };
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            child,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: Clone + 'a> From<CarouselInput<'a, Message>> for Element<'a, Message> {
    fn from(c: CarouselInput<'a, Message>) -> Self {
        Element::new(c)
    }
}

/// How long a newly shown page takes to fade in, in milliseconds.
const PAGE_FADE_MS: f32 = 180.0;

/// The shown page, drawn at the opacity its carousel sets as it fades in.
pub(super) struct PageFade<'a, Message> {
    pub(super) child: Element<'a, Message>,
    pub(super) alpha: std::rc::Rc<std::cell::Cell<f32>>,
}

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for PageFade<'_, Message> {
    fn tag(&self) -> tree::Tag {
        self.child.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.child.as_widget().state()
    }

    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        self.child.as_widget().frame_interest(tree)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
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
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
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
        use iced::advanced::Renderer as _;
        let alpha = self.alpha.get();
        let draw = |renderer: &mut iced::Renderer| {
            self.child
                .as_widget()
                .draw(tree, renderer, theme, style, layout, cursor, viewport);
        };
        if alpha >= 1.0 {
            draw(renderer);
        } else {
            renderer.with_layer_alpha(layout.bounds(), alpha, draw);
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
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
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
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<'a, Message: 'a> From<PageFade<'a, Message>> for Element<'a, Message> {
    fn from(fade: PageFade<'a, Message>) -> Self {
        Element::new(fade)
    }
}
