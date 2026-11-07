//! `tree_view`: widget.
//!
//! Owns the private stateful widget assembled by the builder for the tree-view implementation.

use super::*;
use iced::advanced::layout::Measure as _;

thread_local! {
    static FRAME_UPDATES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Number of frame-route updates delivered to a tree view on this thread.
///
/// Test diagnostics use this to prove an unrelated animation prunes a static
/// workspace tree rather than merely making its update cheap.
#[doc(hidden)]
#[must_use]
pub fn frame_updates() -> u64 {
    FRAME_UPDATES.with(std::cell::Cell::get)
}

// ---------------------------------------------------------------------------
// The roving-focus owner
// ---------------------------------------------------------------------------

/// Per-tree state: the single focus stop, the roving cursor, and the last
/// observed viewport translation of the internal scrollable.
///
/// `translation` is refreshed in `update()` from an `Operation::scrollable`
/// probe so that `row_at` and the focus-ring `draw` — which see only
/// document-space rectangles from the layout tree — can shift into
/// viewport-space. It is kept up to date every event pass so a
/// scroll caused by the mouse wheel (which is handled inside the child
/// scrollable before `update` returns) is visible to the next click.
#[derive(Debug, Clone, Default)]
pub(crate) struct TreeViewState {
    pub(crate) focus: FocusState,
    /// The row the arrow keys act on, as a flat index into the current
    /// frame's rows. `None` until the tree is entered or a row is clicked.
    ///
    /// This index is *derived*, not authoritative: it is re-resolved from
    /// [`Self::cursor_key`] against the live row keys on every event and
    /// operate pass, because a model change (insert/remove/expand/
    /// collapse) moves a row to a different slot. Storing only the index is
    /// inserting a row above the cursor must not silently move the
    /// selection to an unrelated item.
    pub(crate) cursor: Option<usize>,
    /// The identity of the cursor row: its stable path-derived key (see
    /// [`row_keys`]). This is the authoritative cursor — [`Self::cursor`] is
    /// re-derived from it. `None` when no row is cursored. Survives model
    /// changes so the cursor follows its row rather than its slot.
    pub(crate) cursor_key: Option<String>,
    /// Live scroll translation of the internal scrollable, cached during
    /// `update()` for use by `draw()` and by the hit-test helpers.
    pub(crate) translation: Vector,
    /// A logical row index that reconciliation moved the cursor onto and which
    /// has not been scrolled into view yet.
    ///
    /// `diff` is where a model change first becomes observable, but it is
    /// handed neither a `Layout` nor a `&Renderer`, so it cannot run the
    /// `Operation` that [`TreeViewWidget::reveal_row`] needs. The index is
    /// therefore parked here by `diff` and consumed by `layout`, which
    /// `UserInterface::build` calls immediately afterwards
    /// (`iced/runtime/src/user_interface.rs`) and which does
    /// have both — so the reveal lands before the first post-rebuild `draw`
    /// with no intervening event or operation.
    ///
    /// Set only when reconciliation actually moved the cursor's index: a
    /// row that kept its slot also kept its document-space `y`, so revealing
    /// it could only yank a viewport the user scrolled by hand. Cleared as
    /// soon as it is consumed, so one rebuild causes at most one reveal.
    pub(crate) pending_reveal: Option<usize>,
    /// Per-window modality key: captured in `update`, read in
    /// `draw`, so a commit in another window does not hide this tree's ring.
    pub(crate) window_key: u64,
}

impl iced::advanced::widget::operation::Focusable for TreeViewState {
    fn is_focused(&self) -> bool {
        self.focus.is_focused()
    }
    fn focus(&mut self) {
        self.focus.focus();
    }
    fn unfocus(&mut self) {
        self.focus.unfocus();
    }
}

/// Owns the tree's single Tab stop, its keyboard contract and its `Role::Tree`
/// group node. The visual rows are the single child.
pub(crate) struct TreeViewWidget<'a, Message> {
    pub(crate) child: Element<'a, Message>,
    /// The logical navigation view: one entry per item in the whole tree,
    /// never windowed. Keyboard navigation, `sibling_position_of` and the
    /// disappeared-row policy all read this, which is what lets the cursor
    /// reach a row the realization window has not built.
    pub(crate) rows: Vec<TreeRow>,
    /// The logical index range realized as `Element`s this frame.
    /// `0..rows.len()` on the eager path. Used to translate between the
    /// layout tree's realized-row rects and logical row indices.
    pub(crate) realized: std::ops::Range<usize>,
    pub(crate) selected_flags: Vec<bool>,
    pub(crate) aria_label: String,
    pub(crate) row_height: f32,
    pub(crate) row_radius: f32,
    pub(crate) on_select: Option<std::rc::Rc<dyn Fn(usize) -> Message + 'a>>,
    pub(crate) on_toggle: Option<std::rc::Rc<dyn Fn(usize) -> Message + 'a>>,
    /// The tree-level secondary-press hook, kept here as well as bound
    /// per row, so the keyboard route (<kbd>Menu</kbd> / <kbd>Shift+F10</kbd>
    /// on the cursor row) can publish the identical message the pointer path
    /// does. One hook, two triggers — not a second convention.
    pub(crate) row_on_context: Option<std::rc::Rc<dyn Fn(usize, iced::Point) -> Message + 'a>>,
    pub(crate) id: Option<iced::advanced::widget::Id>,
    /// The row-id namespace this tree derives its row ids under, kept
    /// so the tree can publish it as its own stable address for
    /// [`crate::tree_view::FocusTreeRow`]. It is the string the host
    /// already passes to [`crate::tree_view::tree_row_id`], so restoring focus
    /// to a row needs no second identity scheme.
    pub(crate) id_prefix: String,
    /// Stable `Id` of the internal scrollable, used to probe its translation
    /// and to drive `scroll_to` for scroll-into-view.
    pub(crate) scroll_id: Id,
    /// Per-row derived ids, used to resolve the cursor’s
    /// `active_descendant` at `operate` time.
    /// `None` for a row with no host-supplied logical key — that row is
    /// published anonymous rather than under a label-derived alias.
    pub(crate) row_ids: Vec<Option<iced::advanced::widget::Id>>,
    /// Per-row reconciliation keys: the same path strings `row_ids` is
    /// derived from, used to re-resolve the keyed cursor to its row after a
    /// model change. `row_keys[i]` is the key of `row_ids[i]`.
    pub(crate) row_keys: Vec<String>,
    /// An explicitly pinned `active_descendant` that wins over the derived
    /// cursor row. Mirrors [`crate::menu::EnzMenu`].
    pub(crate) active_descendant: Option<iced::advanced::widget::Id>,
    /// Reveal signals for realized rows with hover/focus trailing actions.
    pub(crate) row_reveals: Vec<(usize, crate::list_row_surface::TrailingReveal)>,
    /// Right-to-left: Left expands and Right collapses.
    pub(crate) rtl: bool,
}

/// Decode a key press into a tree navigation intent.
///
/// Ctrl-, Alt-, or Logo-modified presses return `None` so the event stays
/// `Ignored` and reaches application chord handlers. Shift is kept: it is
/// the control's own modifier (consistent with `focus::traversal_from_key`
/// and `focus::navigation_from_key`).
pub(crate) fn tree_key(event: &keyboard::Event) -> Option<TreeKey> {
    let keyboard::Event::KeyPressed { key, modifiers, .. } = event else {
        return None;
    };
    // Ctrl / Alt / Logo mark application chords — let them through.
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    use keyboard::key::Named;
    let keyboard::Key::Named(named) = key else {
        return None;
    };
    Some(match named {
        Named::ArrowDown => TreeKey::Down,
        Named::ArrowUp => TreeKey::Up,
        Named::Home => TreeKey::Home,
        Named::End => TreeKey::End,
        Named::ArrowRight => TreeKey::Right,
        Named::ArrowLeft => TreeKey::Left,
        Named::Enter | Named::Space => TreeKey::Activate,
        _ => return None,
    })
}

impl<Message> TreeViewWidget<'_, Message> {
    /// The cursor to act on: the live one, else whichever row reads as
    /// selected, so the first arrow press continues from the user's selection
    /// rather than jumping to the top.
    pub(crate) fn effective_cursor(&self, state: &TreeViewState) -> Option<usize> {
        state
            .cursor
            .filter(|&i| i < self.rows.len())
            .or_else(|| self.selected_flags.iter().position(|&s| s))
    }

    /// Re-resolve the keyed cursor against this frame's row keys,
    /// updating `state.cursor` in place so every downstream read (keyboard
    /// navigation, focus ring, `active_descendant`) sees the row the cursor's
    /// identity actually names now — not whatever row drifted into its old
    /// slot after a model change.
    ///
    /// Idempotent and cheap: when the model is unchanged the key is found at
    /// its old index and nothing moves. When the cursor's row disappeared, the
    /// disappeared-row policy in [`enzui_core::tree_nav::reconcile_cursor`]
    /// chooses the nearest survivor and `cursor_key` is rewritten to name it,
    /// so the identity stays live rather than dangling.
    ///
    /// Collapse-an-ancestor policy: a collapsed branch rebuilds the list
    /// without the cursor's descendant row, so its key is absent. Before
    /// falling through to the generic disappeared-row policy, the cursor retargets the
    /// cursor onto the nearest surviving ancestor — the row the user can
    /// still see and the one they just collapsed. That ancestor's key is a
    /// prefix of the cursor key up to a `/`, so the deepest present prefix is
    /// the collapsed branch itself.
    fn reconcile_cursor(&self, state: &mut TreeViewState) {
        let Some(old_key) = state.cursor_key.clone() else {
            // No keyed cursor: leave the (possibly selection-derived) index be.
            return;
        };
        let keys: Vec<&str> = self.row_keys.iter().map(String::as_str).collect();

        // Collapse-an-ancestor retarget: if the exact row is gone, try the
        // nearest surviving ancestor prefix before the generic policy. This
        // makes a collapse land on the visible collapsed branch rather than on
        // "the next row", which would be a sibling the user did not collapse.
        let resolved_key = if keys.contains(&old_key.as_str()) {
            Some(old_key.clone())
        } else {
            nearest_present_ancestor(&old_key, &keys)
        };

        let new_index = match resolved_key {
            Some(ref k) => {
                enzui_core::tree_nav::reconcile_cursor(Some(k.as_str()), state.cursor, &keys)
            }
            // No ancestor survived either (whole subtree removed): fall back to
            // the generic nearest-survivor policy on the original key.
            None => {
                enzui_core::tree_nav::reconcile_cursor(Some(old_key.as_str()), state.cursor, &keys)
            }
        };

        state.cursor = new_index;
        // Rewrite the identity to whatever row we actually landed on, so the
        // key never dangles and a subsequent arrow press moves from here.
        state.cursor_key = new_index.and_then(|i| self.row_keys.get(i).cloned());
    }

    /// Place the cursor on `idx`, keeping the keyed identity in sync.
    /// Every cursor write (keyboard, pointer, touch) goes through here so the
    /// index and its key can never disagree.
    fn set_cursor(&self, state: &mut TreeViewState, idx: usize) {
        state.cursor = Some(idx);
        state.cursor_key = self.row_keys.get(idx).cloned();
    }

    /// The anchor point a keyboard-invoked context menu for row `idx`
    /// reports.
    ///
    /// The row's own centre, in viewport space — the space a pointer press
    /// reports, so the two triggers name the same point for the same row.
    ///
    /// [`row_bounds`](Self::row_bounds) returns DOCUMENT-space rects: the
    /// layout tree knows nothing about the internal scrollable's offset. A
    /// right-click, by contrast, arrives already in viewport space — which is
    /// why [`row_at`](Self::row_at) has to *add* the translation to go the
    /// other way. Handing a document-space point to the host would open the
    /// keyboard menu at wherever that row would sit at scroll offset zero: in
    /// a tree scrolled down far enough, above the tree entirely. So shift by
    /// the live [`TreeViewState::translation`] with the same
    /// [`shift_by`] the focus ring uses — the ring and the
    /// keyboard menu must agree about where the cursor row *is*, because the
    /// user is looking at the ring when they press the key.
    ///
    /// Centring (rather than a corner) is the convention
    /// [`crate::context_menu_action`] already fixed for a routed AT request,
    /// which has no cursor either.
    ///
    /// Falls back to the tree's own centre when the cursor row has no layout
    /// node this frame (virtualized off-screen, an empty realization window):
    /// a point inside the surface the menu belongs to is always better than
    /// refusing the keyboard trigger, and the row identity travels in the
    /// index, never in the point. That fallback is already viewport space —
    /// `layout.bounds()` is the tree's own rect, not a scrolled child's — so
    /// it must not be shifted.
    fn row_anchor(
        &self,
        layout: Layout,
        children: &[Tree],
        idx: usize,
        translation: Vector,
    ) -> iced::Point {
        match self.row_bounds(layout, children, idx) {
            Some(row) => shift_by(row, translation).center(),
            None => layout.bounds().center(),
        }
    }

    /// Publish this tree's [`TreeRowFocusRequest`] carrier and apply whatever
    /// a [`FocusTreeRow`] operation claimed through it.
    ///
    /// The resolution lives here, not in the operation, because this
    /// widget owns the row ids and the reconciliation keys: an operation that
    /// carried its own copy of the row table would be a second derivation that
    /// drifts.
    ///
    /// A requested row that is not in this frame's ids is not an error and
    /// never becomes a stale cursor: the tree still takes focus, and the
    /// roving cursor stays where reconciliation put it (the nearest
    /// survivor), which is what the `active_descendant` published moments
    /// later then names.
    fn offer_row_focus(
        &self,
        state: &mut TreeViewState,
        bounds: Rectangle,
        operation: &mut dyn Operation,
    ) {
        let mut request = TreeRowFocusRequest::new(self.id_prefix.clone(), self.id.clone());
        operation.custom(self.id.as_ref(), bounds, &mut request);
        if !(request.claimed() && request.apply()) {
            return;
        }
        // Focus the tree itself: one Tab stop, and the ring the user sees.
        state.focus.focus();
        // Rove the cursor onto the named row, when that row is still here;
        // else onto the deepest surviving ancestor of the path it named.
        let Some(idx) = self.resolve_requested_row(&request) else {
            return;
        };
        let moved = state.cursor != Some(idx);
        self.set_cursor(state, idx);
        // Scroll it back into view only when it actually moved, so a row
        // already under the cursor cannot yank a viewport the user scrolled by
        // hand (the same rule `diff` applies for ).
        if moved {
            state.pending_reveal = Some(idx);
        }
    }

    /// Which row this frame a claimed [`TreeRowFocusRequest`] resolves to.
    ///
    /// 1. The row itself, matched by published id — the normal case.
    /// 2. Its deepest surviving ancestor, when the row is gone and the
    ///    caller supplied the ancestor path: the branch the user can still see
    ///    after `Close tab`/`Delete` removed the row the menu acted on, or
    ///    after collapsing its parent. This is the same
    ///    [`nearest_present_ancestor`] rule the keyed cursor already uses for
    ///    asked here rather than re-derived.
    /// 3. Nothing — the tree still takes focus and keeps whatever live row
    ///    reconciliation left the cursor on. Never the removed row: an id that
    ///    names no row this frame is never written to the cursor, so no stale
    ///    `active_descendant` can be published.
    fn resolve_requested_row(&self, request: &TreeRowFocusRequest) -> Option<usize> {
        if let Some(row) = request.requested_row()
            && let Some(idx) = self
                .row_ids
                .iter()
                .position(|candidate| candidate.as_ref() == Some(row))
        {
            return Some(idx);
        }
        // The row is not here. Its path still tells us where it *was*.
        let path = request.requested_path()?;
        if path.is_empty() {
            return None;
        }
        let wanted = path.join("/");
        let keys: Vec<&str> = self.row_keys.iter().map(String::as_str).collect();
        let ancestor = nearest_present_ancestor(&wanted, &keys)?;
        keys.iter().position(|k| *k == ancestor)
    }

    /// The tree's grouping semantics.
    ///
    /// **No `set_position`**: a `Tree` container is not a member of its own
    /// set, so `(N, N)` would falsely announce it as the last item of a set
    /// it actually *owns*. The item count the AT wants is derived from the
    /// tree's children by the platform — faking it with `SetPosition` here
    /// was a category error (`Role::Tree` in a `SetPosition::index` position
    /// is like a room reporting itself as the last chair in it).
    fn group_semantics(&self, focused: bool) -> Semantics {
        Semantics::new(Role::Tree)
            .label(self.aria_label.clone())
            .focused(focused)
    }
}

impl<'a, Message> Widget<Message, iced::Theme, iced::Renderer> for TreeViewWidget<'a, Message>
where
    Message: 'a + Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<TreeViewState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(TreeViewState::default())
    }

    /// reconcile the keyed cursor here, on the structural change
    /// itself, rather than only through a later `update`/`operate`.
    ///
    /// `diff` is the one lifecycle hook that runs when the host swaps in a
    /// rebuilt row list (`UserInterface::build` calls `Tree::diff` before it
    /// lays anything out, `iced/runtime/src/user_interface.rs`),
    /// and it is the *first* place the new `row_keys` and the surviving
    /// `TreeViewState` are both in scope. Reconciling only in `update` and
    /// `operate` leaves a window in which `draw` — which reads `state.cursor`
    /// directly and is the only method holding a `&Theme` to paint the ring
    /// with — can run against the pre-mutation index and outline the row that
    /// drifted into the old slot. Same lifecycle argument as (`855b448d`):
    /// the state is correct *eventually*, and the first frame is wrong.
    ///
    /// The policy itself is untouched — this calls the same
    /// [`Self::reconcile_cursor`] the other two call sites do, so
    /// identity-first, nearest-ancestor and nearest-survivor behave exactly as
    ///  decided. Those call sites stay: they are idempotent, and `operate`
    /// still needs its pass for the trees a host drives without a rebuild.
    ///
    /// The reveal cannot happen here — `diff` has no `Layout` and no
    /// `&Renderer`, so it cannot run the `Operation` that drives `scroll_to`.
    /// The moved index is parked on [`TreeViewState::pending_reveal`] and
    /// consumed by [`Self::layout`], which is called immediately afterwards by
    /// the same `UserInterface::build` and does have both.
    fn diff(&mut self, tree: &mut Tree) {
        {
            let state = tree.state.downcast_mut::<TreeViewState>();
            let before = state.cursor;
            self.reconcile_cursor(state);
            // Only a row that actually MOVED can have left the viewport. A row
            // that kept its slot kept its document-space `y` too, so revealing
            // it could only yank a viewport the user scrolled by hand — the
            // "do not recenter a visible row" half of, enforced before we
            // even measure.
            if state.cursor != before && state.cursor.is_some() {
                state.pending_reveal = state.cursor;
            }
        }
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    /// Lays the child out and then discharges any reveal `diff` parked.
    ///
    /// This is the earliest hook that holds everything `reveal_row` needs: a
    /// laid-out node to read row rectangles off, a `&Renderer` to run the
    /// `Operation` with, and `&mut Tree` for the scrollable's state. It runs
    /// inside `UserInterface::build`, so the retained row is already revealed
    /// by the time the first post-mutation `draw` happens — with no
    /// intervening event or operation.
    ///
    /// `Layout::new(&node)` places the node at the origin rather than at its
    /// real screen position, which is harmless: `reveal_row` only ever uses
    /// the *difference* `row.y - viewport.y` and the viewport's height, both
    /// invariant under translation.
    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let node = self
            .child
            .as_widget_mut()
            .measure(&mut tree.children[0], renderer, limits);

        // A measurement pass (a `Shrink` ancestor asking "how tall would
        // you like to be?") hands unbounded limits, so the viewport it lays
        // out is not the one the user sees and every row looks visible in it.
        // Revealing against that geometry would consume the pending index and
        // silently skip the real reveal, so leave it parked for the bounded
        // pass that follows.
        let bounded = limits.max().height.is_finite() && limits.max().width.is_finite();
        let pending = if bounded {
            tree.state
                .downcast_mut::<TreeViewState>()
                .pending_reveal
                .take()
        } else {
            None
        };
        if let Some(index) = pending {
            let child_layout = Layout::of(&node);
            // `reveal_row` reasons against the cached translation, so it must
            // be fresh: the scrollable may have been scrolled since our last
            // `update`, and on a first build there is no cached value at all.
            self.refresh_translation(tree, child_layout, renderer);
            // Minimum reveal, and a no-op when the row is already fully
            // visible: `reveal_axis_offset` returns `current` unchanged in
            // that case and `reveal_row` then declines to issue a `scroll_to`.
            self.reveal_row(tree, child_layout, renderer, index);
            // Re-read so `draw`'s focus ring uses the offset we just set
            // rather than the pre-reveal one.
            self.refresh_translation(tree, child_layout, renderer);
        }

        layout::Node::new(node.size()).apply(tree);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_mut::<TreeViewState>();
        // re-resolve the keyed cursor to its row before publishing
        // `active_descendant`, so a screen reader is told the row the cursor
        // actually names now — not the row that drifted into its old slot. The
        // AT half: a keyed cursor keeps `active_descendant`
        // correct across model changes.
        self.reconcile_cursor(state);
        // One focus stop for the whole tree (WAI-ARIA TreeView). The rows are
        // embedded and withdraw themselves from the ring.
        operation.focusable(self.id.as_ref(), layout.bounds(), state);

        // offer the row-focus carrier. Published after `focusable` and
        // before the semantics below, which is the whole ordering contract:
        // `FocusTreeRow`'s applying pass unfocuses every focusable it visits,
        // so the tree's own claim has to land after that unfocus (else the
        // tree would give the focus back up), and the cursor it moves has to
        // land before `active_descendant` is computed (else an AT would be
        // told last frame's row).
        self.offer_row_focus(state, layout.bounds(), operation);

        let focused = state.focus.is_focused();

        // publish `active_descendant` on the tree group, mirroring the
        // precedence in `EnzMenu::into_element`. An explicit pin wins; otherwise
        // the cursor (or selected) row supplies it. `Option::or` gives the
        // no-cursor case for free — no relation emitted rather than a stale
        // one.
        // an unkeyed cursor row contributes no `active_descendant` —
        // `flatten()` drops the inner `None`. Naming a node the tree does not
        // publish would be a dangling relation, which `collect.rs` strips
        // anyway; not emitting it is the same outcome stated once.
        let cursor_row_id = self
            .effective_cursor(state)
            .and_then(|idx| self.row_ids.get(idx).cloned())
            .flatten();
        let mut semantics = self.group_semantics(focused);
        if let Some(row) = self.active_descendant.clone().or(cursor_row_id) {
            semantics = semantics.active_descendant(row);
        }
        let mut group = SemanticsNode::group(semantics);
        operation.custom(self.id.as_ref(), layout.bounds(), &mut group);
        operation.container(self.id.as_ref(), layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                viewport,
                renderer,
                operation,
            );
        });
    }

    /// The tree view has no frame-driven state of its own. Advertising only
    /// the scroll/content branch's interest keeps a static large workspace out
    /// of unrelated shell animation updates.
    fn frame_interest(&self, tree: &Tree) -> bool {
        crate::frame_routing::interest_opaque(tree)
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
        FRAME_UPDATES.with(|count| count.set(count.get().wrapping_add(1)));
        crate::frame_routing::forward_frame_opaque(
            &mut self.child,
            tree,
            0,
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
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
        // feed this window's input modality from every raw event, so
        // a pointer-last frame suppresses the keyboard ring; keyed per window
        //  and stashed for `draw`.
        let window_key = enzui_core::focus::window_key(shell.window());
        tree.state.downcast_mut::<TreeViewState>().window_key = window_key;
        enzui_core::focus::note_input_event_for(event, window_key);

        // re-resolve the keyed cursor to its row before anything reads
        // `state.cursor`. The host may have inserted/removed/expanded/collapsed
        // rows since the last event, so the stored index may name a different
        // row now; reconciliation moves the index back onto the cursor's own
        // row (or the disappeared-row survivor). Idempotent when unchanged.
        {
            let state = tree.state.downcast_mut::<TreeViewState>();
            self.reconcile_cursor(state);
        }

        // Rows first: a click on a row is that row's completed click, and the
        // row publishes the select message itself.
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );

        let bounds = layout.bounds();

        // pointer focus is EXCLUSIVE. A press anywhere in the tree
        // Takes focus; a press outside it Drops focus so a sibling control the
        // user just clicked owns the ring instead. The tree is always a focus
        // target (it has no disabled state), so `enabled` is unconditionally
        // true here. A Drop clears only the focus flag — never `state.cursor`,
        // which is the selected row, a distinct piece of state that must
        // survive an outside press.
        if let Some(decision) = enzui_core::focus::press_focus_from_cursor(event, cursor, bounds) {
            let state = tree.state.downcast_mut::<TreeViewState>();
            let was_focused = state.focus.is_focused();
            match decision {
                PressFocus::Take => state.focus.focus(),
                PressFocus::Drop => state.focus.unfocus(),
                PressFocus::Leave => {}
            }
            if state.focus.is_focused() != was_focused {
                shell.request_redraw();
            }
        }

        // Refresh the cached scroll translation from the child scrollable
        // before running any event that depends on it. The child scrollable
        // has already processed a wheel event by the time our `update` runs
        // (rows-first, above), so its state is authoritative. Hit tests and
        // the focus ring both use `state.translation`.
        self.refresh_translation(tree, layout, renderer);

        match event {
            // Pointer-to-focus: pressing anywhere in the tree moves the cursor
            // to the row under the pointer, so the arrow keys continue from
            // where the user clicked. Focus itself is taken by the exclusive
            // press-focus sweep above.
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let handles_event = cursor.is_over(bounds);
                if handles_event {
                    let translation = tree.state.downcast_ref::<TreeViewState>().translation;
                    let hit = cursor
                        .position()
                        .filter(|_| self.row_height > 0.0)
                        .and_then(|p| self.row_at(layout, &tree.children, p, translation));
                    if let Some(idx) = hit {
                        let state = tree.state.downcast_mut::<TreeViewState>();
                        self.set_cursor(state, idx);
                    }
                }
            }
            // touch carries its own position — use it, not the cursor.
            Event::Touch(iced::touch::Event::FingerPressed { position, .. }) => {
                let handles_event = bounds.contains(*position);
                if handles_event {
                    let translation = tree.state.downcast_ref::<TreeViewState>().translation;
                    let hit = (self.row_height > 0.0)
                        .then_some(*position)
                        .and_then(|p| self.row_at(layout, &tree.children, p, translation));
                    if let Some(idx) = hit {
                        let state = tree.state.downcast_mut::<TreeViewState>();
                        self.set_cursor(state, idx);
                    }
                }
            }
            Event::Keyboard(kb) => {
                let state = tree.state.downcast_ref::<TreeViewState>();
                if !state.focus.is_focused() {
                    return;
                }
                // the KEYBOARD context-menu route. The tree owns the
                // only Tab stop, so no row surface can ever be "the focused
                // widget" a Menu/Shift+F10 press belongs to — without this,
                // `row_on_context` was reachable by pointer and by a routed AT
                // request, and not at all from the keyboard.
                if enzui_core::context_menu::opens_context_menu(kb)
                    && let Some(f) = &self.row_on_context
                    && let Some(idx) = self.effective_cursor(state)
                {
                    let at = self.row_anchor(layout, &tree.children, idx, state.translation);
                    shell.publish(f(idx, at));
                    shell.request_redraw();
                    // Claimed: an ancestor must not also open its own menu for
                    // one press, the same exclusivity `context_menu_host`
                    // gives the focused host.
                    shell.capture_event();
                    return;
                }
                let Some(key) = tree_key(kb) else { return };
                let key = match key {
                    TreeKey::Left if self.rtl => TreeKey::Right,
                    TreeKey::Right if self.rtl => TreeKey::Left,
                    key => key,
                };
                let current = self.effective_cursor(state);
                let Some(action) = tree_key_action(&self.rows, current, key) else {
                    // A key the tree does not use at this position (Down on the
                    // last row) is left uncaptured so an ancestor may have it.
                    return;
                };
                let moved_to = match action {
                    TreeAction::Move(idx) | TreeAction::Activate(idx) => {
                        let state = tree.state.downcast_mut::<TreeViewState>();
                        self.set_cursor(state, idx);
                        if let Some(f) = &self.on_select {
                            shell.publish(f(idx));
                        }
                        Some(idx)
                    }
                    TreeAction::Toggle(idx) => {
                        let state = tree.state.downcast_mut::<TreeViewState>();
                        self.set_cursor(state, idx);
                        if let Some(f) = &self.on_toggle {
                            shell.publish(f(idx));
                        }
                        Some(idx)
                    }
                };
                // Scroll-into-view: move the viewport the
                // minimum distance so the cursor row is fully visible. This is
                // ours to do because iced's scrollable has no keyboard reveal
                // and `Operation::scrollable` on the child can be driven
                // directly from here.
                if let Some(idx) = moved_to {
                    self.reveal_row(tree, layout, renderer, idx);
                }
                shell.request_redraw();
                shell.capture_event();
            }
            _ => {}
        }

        // Re-read after events — a scroll_to we just issued, or a wheel event
        // handled by the child, may have changed the translation. Keeping
        // `state.translation` fresh on every pass is what makes the draw pass
        // consistent without having to run an operation from `draw` (which is
        // `&self`).
        self.refresh_translation(tree, layout, renderer);
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
        let state = tree.state.downcast_ref::<TreeViewState>();
        let keyboard_focused =
            enzui_core::focus::focus_ring_visible_for(state.focus.is_focused(), state.window_key);
        let cursor_row = self.effective_cursor(state);
        // the tree, not an embedded row, owns keyboard focus. Seed each
        // realized row's reveal signal before child drawing; the row surface
        // then preserves this seed while ORing in its own live pointer hover or
        // a focused trailing child.
        for (idx, reveal) in &self.row_reveals {
            reveal.set(keyboard_focused && cursor_row == Some(*idx));
        }

        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );

        // The focus-visible ring is painted here, by the owner, and only while
        // the tree actually holds focus.
        //
        // It cannot live on the row: a row is built during `view`, long before
        // anyone knows whether the tree is focused, so a row that drew its own
        // ring from the builder-time cursor would outline the selected row on
        // a tree nobody has touched. That is exactly what the first screenshot
        // of this widget showed — an accent ring around row 3 at rest.
        // gate on the shared focus-visible policy, not raw focus, so
        // a pointer click focuses the tree without painting the keyboard ring.
        if !keyboard_focused {
            return;
        }
        let Some(index) = self.effective_cursor(state) else {
            return;
        };
        // Row bounds come from the layout tree in DOCUMENT space (the layout
        // tree does not know about scroll translation). Shift into viewport
        // space by the live translation so a ring on row 7 while scrolled
        // paints where row 7 actually is, not where it would be at offset 0.
        //
        let Some(bounds) = self.row_bounds(layout, &tree.children, index) else {
            return;
        };
        let bounds = shift_by(bounds, state.translation);
        // Clip to the scroll viewport so a ring on a scrolled-away row does not
        // paint over the panel border.
        let Some(visible) = bounds.intersection(&layout.bounds()) else {
            return;
        };
        if visible.height < bounds.height - 0.5 && visible.height < 1.0 {
            return;
        }
        // shared ring painter. Zero outset — `visible` is already the
        // scroll-clipped row rect and growing it would paint over the panel
        // border a scrolled-away row must not touch.
        crate::focus_button::paint_focus_ring(
            renderer,
            visible,
            crate::focus_button::FocusRingQuad {
                ink: crate::theme_lookup::palette(theme).focus_ring,
                width: crate::theme_lookup::focus_ring_width(theme),
                radius: self.row_radius.into(),
                outset: 0.0,
                snap: true,
            },
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
            layout,
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
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<Message> TreeViewWidget<'_, Message> {
    /// The laid-out bounds of every row, in DOCUMENT space — the coordinates
    /// the layout tree carries, before the scrollable's translation is
    /// applied. Callers that need screen-space (hit testing, ring painting)
    /// must add [`TreeViewState::translation`] themselves; see [`shift_by`].
    ///
    /// Read off the layout tree rather than computed from `row_height` because
    /// the rows live inside a scroll viewport; document-space arithmetic here
    /// keeps the shift *at one place* rather than scattering it across every
    /// call site.
    ///
    /// The rows sit a few containers deep (panel > scrollable > column), so
    /// this descends to the first layer where every direct child has the
    /// row's fixed height. A count check alone is
    /// unsafe for a one-row tree: `rows.len() == 1` collides with every
    /// intermediate wrapper, which also has exactly one child, and the search
    /// would stop at the wrapper and return its `Fill`-height bounds as the
    /// row's — the focus ring around the whole panel, hit tests answering
    /// row 0 everywhere. The row-height screen is structural: an ancestor
    /// wrapper is `Fill` or `Shrink`, never `Fixed(row_height)`, so it cannot
    /// masquerade. (Regression: `row_layer_finds_the_row_in_a_one_row_tree`.)
    pub(crate) fn row_layer(&self, layout: Layout, children: &[Tree]) -> Option<Vec<Rectangle>> {
        fn walk(
            layout: Layout,
            children: &[Tree],
            want: usize,
            row_height: f32,
            depth: usize,
        ) -> Option<Vec<Rectangle>> {
            if depth > 8 {
                return None;
            }
            let children: Vec<(Layout, &Tree)> = layout.iter(children).collect();
            if children.len() == want
                && !children.is_empty()
                && children
                    .iter()
                    .all(|(c, _)| (c.bounds().height - row_height).abs() < 0.5)
            {
                return Some(children.iter().map(|(c, _)| c.bounds()).collect());
            }
            children
                .into_iter()
                .find_map(|(child, tree)| walk(child, &tree.children, want, row_height, depth + 1))
        }
        if self.rows.is_empty() || self.realized.is_empty() {
            return None;
        }
        // The layer holds the realized rows, not every row. On the eager
        // path `realized` is `0..rows.len()` and this is unchanged.
        walk(layout, children, self.realized.len(), self.row_height, 0)
    }

    /// The laid-out bounds of one logical row, in DOCUMENT space.
    ///
    /// # Why this is not just `row_layer()[index]`
    ///
    /// An unrealized row has no layout node, so indexing the realized layer
    /// would return `None` for exactly the rows scroll-into-view exists to
    /// reach: press `End` on a 1,000-row tree and the cursor's row is 975 rows
    /// below the window, with nothing to scroll *to*. The keyboard cursor is
    /// logical, so its geometry must be logical too.
    ///
    /// The realized rows are laid out contiguously beneath a leading spacer of
    /// exactly `realized.start * row_height`, so every logical row's top edge is
    /// `layer[0].y + (index - realized.start) * row_height` — the same value the
    /// eager path reads off the layout tree, extended past both ends of the
    /// window by the one height the model already assumes. Realized rows still
    /// return their measured rect, so nothing regresses on the eager path.
    ///
    /// `None` when no row is realized at all (an empty tree, or an offset past
    /// the content end): there is then no anchor to extrapolate from.
    fn row_bounds(&self, layout: Layout, children: &[Tree], index: usize) -> Option<Rectangle> {
        let layer = self.row_layer(layout, children)?;
        if let Some(measured) = index
            .checked_sub(self.realized.start)
            .and_then(|off| layer.get(off))
        {
            return Some(*measured);
        }
        if index >= self.rows.len() {
            return None;
        }
        let first = *layer.first()?;
        let delta = index as f32 - self.realized.start as f32;
        Some(Rectangle {
            y: first.y + delta * self.row_height,
            ..first
        })
    }

    /// The row index under a screen-space point.
    ///
    /// Rows come back from the layout tree in document space, so the point
    /// must be moved into that same frame before comparing — else, once the
    /// user has scrolled, a click on a visible row is compared against an
    /// earlier document row and the private cursor lands somewhere the user
    /// never clicked.
    /// Only realized rows can be hit: a row with no layout node is not on
    /// screen, so a pointer cannot be over it. The returned index is logical
    /// (`realized.start + offset`), so every downstream cursor write stays on
    /// the logical scale.
    pub(crate) fn row_at(
        &self,
        layout: Layout,
        children: &[Tree],
        point: iced::Point,
        translation: Vector,
    ) -> Option<usize> {
        let document_point = iced::Point::new(point.x + translation.x, point.y + translation.y);
        self.row_layer(layout, children)?
            .iter()
            .position(|b| b.contains(document_point))
            .map(|off| self.realized.start + off)
    }

    /// Probe the internal scrollable for its live translation and cache it on
    /// [`TreeViewState`]. Called at the top and bottom of each `update()` so
    /// downstream reads — hit tests during this pass, the focus ring on the
    /// next `draw` — see a fresh value.
    fn refresh_translation(&mut self, tree: &mut Tree, layout: Layout, renderer: &iced::Renderer) {
        struct Probe {
            target: Id,
            translation: Vector,
        }
        impl<T> Operation<T> for Probe {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
                operate(self);
            }
            fn scrollable(
                &mut self,
                id: Option<&Id>,
                _bounds: Rectangle,
                _content_bounds: Size,
                translation: Vector,
                _state: &mut dyn Scrollable,
            ) {
                if id == Some(&self.target) {
                    self.translation = translation;
                }
            }
        }
        let mut probe = Probe {
            target: self.scroll_id.clone(),
            translation: Vector::ZERO,
        };
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            &layout.bounds(),
            renderer,
            &mut probe as &mut dyn Operation,
        );
        tree.state.downcast_mut::<TreeViewState>().translation = probe.translation;
    }

    /// Scroll the internal scrollable the minimum distance to bring row
    /// `index` fully into view, then refresh the cached translation.
    ///
    /// Does nothing when the row is already visible; nothing when the row
    /// cannot be located (an empty tree, an off-by-one from a stale layout).
    fn reveal_row(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        index: usize,
    ) {
        // Row bounds and viewport bounds must be in the same frame for
        // `reveal_axis_offset` to reason about them; row_bounds returns
        // document-space, viewport-space is `layout.bounds()`, and the current
        // translation is what closes the gap. We express the row's start in
        // *content* coordinates (document.y − viewport.y at translation 0 is
        // its content-relative start) exactly as `focus::reveal_focused` does
        // for the scroll_to it issues — the target of a `scroll_to` is a
        // content-space offset, not a screen-space one (per iced's `scrollable`).
        let Some(row_doc) = self.row_bounds(layout, &tree.children, index) else {
            return;
        };
        let viewport = layout.bounds();
        let current = tree.state.downcast_ref::<TreeViewState>().translation;

        // Convert row's document-space y to content-space (subtract the
        // viewport's origin). Then reveal_axis_offset gives the new absolute
        // content offset that brings the row fully into view.
        let item_start_y = row_doc.y - viewport.y;
        let new_y = reveal_axis_offset(current.y, viewport.height, item_start_y, row_doc.height);

        let item_start_x = row_doc.x - viewport.x;
        let new_x = reveal_axis_offset(current.x, viewport.width, item_start_x, row_doc.width);

        // No-op when nothing actually moves. `reveal_axis_offset` already
        // returns `current` for the already-visible case, so this catches the
        // common path and avoids firing the scrollable's on_scroll for
        // nothing.
        if (new_y - current.y).abs() < 0.5 && (new_x - current.x).abs() < 0.5 {
            return;
        }

        struct DoScroll {
            target: Id,
            offset: AbsoluteOffset<Option<f32>>,
        }
        impl<T> Operation<T> for DoScroll {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<T>)) {
                operate(self);
            }
            fn scrollable(
                &mut self,
                id: Option<&Id>,
                bounds: Rectangle,
                content_bounds: Size,
                _translation: Vector,
                state: &mut dyn Scrollable,
            ) {
                if id == Some(&self.target) {
                    state.scroll_to(
                        self.offset,
                        iced::advanced::widget::operation::Animation::Instant,
                        bounds,
                        content_bounds,
                    );
                }
            }
        }
        // Compose an inline `scroll_to` for the target: the same operation
        // `iced::widget::operation::scroll_to` produces, dispatched here
        // rather than returned as a `Task`, because this widget is what
        // decided to move.
        let mut op = DoScroll {
            target: self.scroll_id.clone(),
            offset: AbsoluteOffset {
                x: Some(new_x),
                y: Some(new_y),
            },
        };
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            &layout.bounds(),
            renderer,
            &mut op as &mut dyn Operation,
        );
    }
}

/// Shift a document-space rect into viewport-space by subtracting the current
/// scroll translation. Hit-test and focus-ring code share this function so
/// they cannot disagree on the shift.
pub(crate) fn shift_by(r: Rectangle, translation: Vector) -> Rectangle {
    Rectangle {
        x: r.x - translation.x,
        y: r.y - translation.y,
        ..r
    }
}
