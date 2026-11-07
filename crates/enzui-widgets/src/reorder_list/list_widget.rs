//! The reorder list's `Widget` implementation.

use super::*;

impl<'a, Message> Widget<Message, iced::Theme, iced::Renderer> for EnzReorderList<'a, Message>
where
    Message: Clone + 'a,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<ReorderState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(ReorderState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        // Keyed reconciliation. Keyed reorder lists: stable-`ItemId` rows whose
        // identities are the component point. Default `diff_children_custom`s
        // index-pair `self.children[i]` with old-`Tree` index-i, discarding keys:
        // reorders leave child-`Tree` states (focus, editor buffers, overlays,
        // animations) at SLOTS while different items arrive — focused-row drags
        // past peers strand focuses on old-position-taking items (visible bug).
        // iced `keyed::column`s rescue insert/deletes only
        // (`diff_children_custom_with_search`); pure-permutation length-equal
        // paths still positional-zip (same `KeyedGrid` defects). Shared
        // tag-checked duplicate-safe reconciliation reused (`KeyedRow`,
        // `KeyedGrid`): never fourth-spelled algorithms.
        let new_keys: Vec<String> = self.keys.clone();

        // a duplicate reconciliation key is a caller defect that is
        // invisible at runtime — the duplicates reconcile positionally among
        // themselves, which is the bug keying removes. Asserted here (not in the
        // constructor) so a key set that only becomes duplicated on a later
        // frame is still caught. Release behaviour is unchanged and remains
        // defined (occurrence order); see `keyed_diff`.
        debug_assert!(
            self.allow_duplicate_keys || crate::keyed_diff::duplicate_keys(&new_keys).is_empty(),
            "EnzReorderList: duplicate reconciliation key(s) {:?} in {:?} — \
             duplicates reconcile positionally among themselves, which is the \
             bug keying removes. Disambiguate with \
             enzui_core::identity::KeyedIds, or call .allow_duplicate_keys() \
             if this is deliberate.",
            crate::keyed_diff::duplicate_keys(&new_keys),
            new_keys,
        );

        // Split the borrows: `state` out of `tree.state`, the retained child
        // `Tree`s out of `tree.children`, at the same time.
        let Tree {
            state, children, ..
        } = tree;
        let state = state.downcast_mut::<ReorderState>();

        crate::keyed_diff::reconcile_children_by_key(
            children,
            &state.child_keys,
            &new_keys,
            &mut self.children,
            |child_tree, child| child.as_widget_mut().diff(child_tree),
            |child| child.as_widget().tag(),
            |child| Tree::new(child.as_widget()),
        );

        // SELECTION reconciliation, not just child trees. Keyed child fixes
        // cover retained WIDGET states; private id-keyed `ListSelection` stores
        // never hear row removals. Removed ids merely hid in
        // `selected_in_order` (handed-order-filtering) while `len()`/`contains()`
        // kept counting: reinsertions silently resurrected selections; stale
        // range anchors revived as never-set endpoints. `reconcile` = filter-vs-
        // delete-decision deletion halves; models can't distinguish, callers do
        // (`selection_persists_when_filtered()`). Deletion defaults: widget-own
        // drop-route app-model behaviour; defect-found cases.
        if !self.selection_persists_when_filtered && state.child_keys != new_keys {
            let removed = state
                .child_keys
                .iter()
                .any(|old| !new_keys.iter().any(|new| new == old));
            // Only pay for the `ClickModel` when a key actually disappeared; a
            // pure permutation or a pure insert cannot orphan a selected id.
            if removed {
                let model = ClickModel::new(new_keys.iter().map(ItemId::new));
                let _ = state.selection.reconcile(&model);
                // A deferred collapse aimed at a row that no longer exists must
                // not survive: the release that would honour it can no longer
                // refer to anything.
                if let Some(pending) = state.pending_collapse.as_ref()
                    && model.index_of(pending).is_none()
                {
                    state.pending_collapse = None;
                }
            }
        }

        if state.child_keys != new_keys {
            state.child_keys = new_keys;
        }
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        if let Some(columns) = self.columns {
            self.layout_grid(tree, renderer, limits, columns);
            return;
        }
        let axis = match self.orientation {
            Orientation::Vertical => layout::flex::Axis::Vertical,
            Orientation::Horizontal => layout::flex::Axis::Horizontal,
        };
        let flow = if self.orientation == Orientation::Horizontal
            && resolve_direction(self.direction) == Direction::RightToLeft
        {
            layout::flex::SequenceFlow::Reverse
        } else {
            layout::flex::SequenceFlow::Forward
        };
        layout::flex::resolve_sequence(
            axis,
            renderer,
            limits,
            self.width,
            self.height,
            self.spacing,
            flow,
            &mut self.children,
            &mut tree.children,
        )
        .apply(tree);

        // While rows are dragged into this list from another, it grows by
        // their extent so the gap fits inside it and its container re-flows.
        let me = CollectionId::new(&self.collection_id);
        let vertical = self.orientation == Orientation::Vertical;
        let incoming = self.drag_host.as_ref().and_then(|channel| {
            let live = channel.layout_drag()?;
            let destined = live
                .destination
                .as_ref()
                .is_some_and(|(collection, _)| collection == &me);
            (destined && live.source != me)
                .then(|| incoming_extent(&live, vertical))
                .flatten()
        });
        if let Some(extent) = incoming {
            let spacing = if self.children.is_empty() {
                0.0
            } else {
                self.spacing
            };
            let grow = extent + spacing;
            let max = limits.max();
            tree.size = if vertical {
                iced::Size::new(tree.size.width, (tree.size.height + grow).min(max.height))
            } else {
                iced::Size::new((tree.size.width + grow).min(max.width), tree.size.height)
            };
        }
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        // The list is its own Tab stop only when no row is focusable. Focusable
        // rows already drive keyboard reordering (see `handle_keyboard_reorder`),
        // so a second stop in front of them would cost every keyboard user an
        // extra Tab.
        let rows_focusable = {
            let mut probe = FocusProbe::default();
            for (child, (child_layout, child_tree)) in self
                .children
                .iter_mut()
                .zip(layout.iter_mut(&mut tree.children))
            {
                child.as_widget_mut().operate(
                    child_tree,
                    child_layout,
                    viewport,
                    renderer,
                    &mut probe,
                );
            }
            probe.any
        };
        {
            let state = tree.state.downcast_mut::<ReorderState>();
            if self.children.is_empty() || rows_focusable {
                state.keyboard_owner.focus.unfocus();
            }
            if !self.children.is_empty() {
                let mut semantics = Semantics::new(Role::Group).label(
                    enzui_core::localization::text("enzui-reorder-list-reorderable-list"),
                );
                if !rows_focusable {
                    semantics = semantics
                        .focusable()
                        .focused(state.keyboard_owner.focus.is_focused());
                }
                let mut node = SemanticsNode::group(semantics);
                operation.custom(Some(&state.keyboard_owner.id), layout.bounds(), &mut node);
                if !rows_focusable {
                    state.keyboard_owner.focus.operate(
                        operation,
                        Some(&state.keyboard_owner.id),
                        layout.bounds(),
                    );
                }
            }
        }

        // Keyboard-reorder announcements as polite live regions: moved-item new
        // positions heard by screen-reader users. `Role::Status`-seeded
        // `Live::Polite` (`Semantics::new`): dynamically-inserted nodes announce
        // only with politeness (impolite regions add silently). Retained-state-
        // written labels (keyboard-path) survive immutable-widget `operate`
        // passes.
        {
            let state = tree.state.downcast_ref::<ReorderState>();
            if let Some(announcement) = state.announcement.as_ref() {
                let mut node =
                    SemanticsNode::new(Semantics::new(Role::Status).label(announcement.clone()));
                operation.custom(None, layout.bounds(), &mut node);
            }
        }

        for (child, (child_layout, child_tree)) in self
            .children
            .iter_mut()
            .zip(layout.iter_mut(&mut tree.children))
        {
            child
                .as_widget_mut()
                .operate(child_tree, child_layout, viewport, renderer, operation);
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
        self.handle_event(tree, event, layout, cursor, renderer, shell, viewport);
        self.publish_selection(tree, shell);
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
        let state = tree.state.downcast_ref::<ReorderState>();

        // The live drag: the host's while it draws us, else our own.
        let channel = self
            .drag_host
            .as_ref()
            .filter(|channel| channel.is_hosting());
        let live = match channel {
            Some(channel) => channel.live_drag(),
            None => state.session.as_ref().and_then(LiveDrag::of),
        };
        let glide = state.motion.begin(theme, live.is_some());
        if !glide {
            // At rest: every row in its laid-out place, nothing to track.
            state.motion.clear();
            for ((i, child), child_layout) in self
                .children
                .iter()
                .enumerate()
                .zip(layout.children(&tree.children))
            {
                child.as_widget().draw(
                    &tree.children[i],
                    renderer,
                    theme,
                    style,
                    child_layout,
                    cursor,
                    viewport,
                );
            }
            self.draw_focus_ring(state, renderer, theme, layout, tree, viewport, |_| {
                Vector::ZERO
            });
            return;
        }
        let child_layouts: Vec<Layout> = layout.children(&tree.children).collect();
        let rows: Vec<Rectangle> = child_layouts.iter().map(Layout::bounds).collect();
        let me = CollectionId::new(&self.collection_id);
        let vertical = self.orientation == Orientation::Vertical;
        let geometry = feedback::Geometry {
            keys: &self.keys,
            rows: &rows,
            bounds: layout.bounds(),
            vertical,
            reverse: !vertical && resolve_direction(self.direction) == Direction::RightToLeft,
            spacing: self.spacing,
            columns: self.columns,
        };
        let mut plan = live.as_ref().map(|live| {
            let incoming = (live.source != me)
                .then(|| incoming_extent(live, vertical))
                .flatten();
            feedback::plan(
                geometry,
                &me,
                live,
                cursor.position(),
                incoming,
                self.feedback.highlight_border_width,
            )
        });
        // The lifted rows stay where they were grabbed, whatever moved this
        // list's layout since the drag began (#3675).
        match plan.as_mut() {
            Some(plan) => anchor_lift(&state.lift_anchor, &self.keys, &rows, &mut plan.rows),
            None => {
                state.lift_anchor.borrow_mut().take();
            }
        }

        state.motion.retain(&self.keys);
        let shown: Vec<(Vector, bool)> = rows
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let (offset, lifted) = plan
                    .as_ref()
                    .map_or((Vector::ZERO, false), |plan| plan.rows[i]);
                let at = state.motion.place(
                    &self.keys[i],
                    row.position(),
                    row.position() + offset,
                    lifted,
                );
                (at - row.position(), lifted)
            })
            .collect();

        let draw_row = |renderer: &mut iced::Renderer, i: usize| {
            let draw = |renderer: &mut iced::Renderer| {
                self.children[i].as_widget().draw(
                    &tree.children[i],
                    renderer,
                    theme,
                    style,
                    child_layouts[i],
                    cursor,
                    viewport,
                );
            };
            if shown[i].0 == Vector::ZERO {
                draw(renderer);
            } else {
                renderer.with_translation(shown[i].0, draw);
            }
        };
        for (i, (_, lifted)) in shown.iter().enumerate() {
            if !lifted {
                draw_row(renderer, i);
            }
        }

        let palette = theme.palette();
        if let Some(plan) = plan.as_ref() {
            if let Some(bar) = plan.bar.filter(|bar| bar.width > 0.0 && bar.height > 0.0) {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: bar,
                        border: iced::Border {
                            radius: 1.0.into(),
                            ..iced::Border::default()
                        },
                        ..renderer::Quad::default()
                    },
                    palette.primary.strong.color,
                );
            }
            if plan.destination && self.children.is_empty() {
                let mut wash = palette.primary.strong.color;
                wash.a *= self.feedback.area_fill_alpha;
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: layout.bounds(),
                        border: iced::Border {
                            color: palette.primary.strong.color,
                            width: self.feedback.highlight_border_width,
                            radius: self.feedback.area_radius.into(),
                        },
                        ..renderer::Quad::default()
                    },
                    wash,
                );
            }
        }

        // Lifted rows paint above everything in the list, on a raised
        // plate, and may leave the list to follow the pointer.
        if shown.iter().any(|(_, lifted)| *lifted) {
            renderer.with_layer(*viewport, |renderer| {
                for (i, (offset, lifted)) in shown.iter().enumerate() {
                    if !lifted {
                        continue;
                    }
                    // The plate hugs the row's pill, in the raised surface's
                    // own colour: a plate wider than the pill, in the window
                    // colour, read as dark bands either side of it.
                    renderer.fill_quad(
                        renderer::Quad {
                            bounds: self.pill(rows[i]) + *offset,
                            border: iced::Border {
                                radius: self.row_radius.into(),
                                ..iced::Border::default()
                            },
                            shadow: crate::elevation::iced_shadow_for_theme(
                                theme,
                                self.feedback.dragging_elevation,
                            ),
                            snap: true,
                        },
                        crate::theme_lookup::palette(theme).surface_elevated,
                    );
                    draw_row(renderer, i);
                }
            });
        }

        self.draw_focus_ring(state, renderer, theme, layout, tree, viewport, |index| {
            shown.get(index).map_or(Vector::ZERO, |(offset, _)| *offset)
        });
    }

    /// Pointer shape: the row's own children first, then this list's drag
    /// affordance. Trait-default [`None`](mouse::Interaction::None)s would hide in-row
    /// button/link/input cursors (arbitrary application elements) + never
    /// advertise grabbable rows. Highest-first precedence: ACTIVE drags →
    /// [`Grabbing`](mouse::Interaction::Grabbing) (gesture-owned pointers; no mid-drag child
    /// flicker); specific-asking (non-`None`) children → theirs (in-row button
    /// pointers kept); reorderable-row pointers → [`Grab`](mouse::Interaction::Grab)
    /// (grab affordances); otherwise `None`.
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        let dragging = tree
            .state
            .downcast_ref::<ReorderState>()
            .session
            .as_ref()
            .is_some_and(DragSession::is_active);
        if dragging {
            return mouse::Interaction::Grabbing;
        }

        let from_children = self
            .children
            .iter()
            .zip(layout.iter(&tree.children))
            .map(|(child, (child_layout, state))| {
                child
                    .as_widget()
                    .mouse_interaction(state, child_layout, cursor, viewport, renderer)
            })
            .max()
            .unwrap_or_default();

        if from_children != mouse::Interaction::None {
            return from_children;
        }

        let over_a_row = cursor
            .position_over(layout.bounds())
            // Iterate the children directly: this runs after every event while
            // the pointer rests over the list, so it must not allocate.
            .and_then(|position| {
                self.row_at(position, layout.bounds(), layout.children(&tree.children))
            })
            .is_some();

        // With handles, only a handle shows the grab cursor (it sets it itself).
        if over_a_row && !self.drag_handles {
            mouse::Interaction::Grab
        } else {
            mouse::Interaction::None
        }
    }

    /// Forward every row's overlay, in row order. Arbitrary-element rows
    /// (combo boxes, menus, tooltips, context menus, date pickers) mount open
    /// surfaces via `Widget::overlay`; trait-default `None`s never asked
    /// children: retained-state-existing popups never laid out/painted/clicked/
    /// Escape-seen. Standard child-overlay paths
    /// (`iced::advanced::overlay::from_children`) sibling-container-identical
    /// (not second implementations): `children[i]`/`tree.children[i]`/
    /// `layout.children()[i]` pairings — `update`/`draw`/`operate`-same — plus
    /// `translation` threading (scrolled-list-anchored row overlays).
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: iced::Vector,
        window: Size,
    ) -> Vec<iced::advanced::overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        iced::advanced::overlay::from_children(
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

/// Keeps lifted rows where they were grabbed: the first lifted row's layout
/// position when the drag began is remembered in `anchor`, and any move of
/// the layout since (another collection reflowing this one) is taken back
/// out of the lifted rows' offsets (#3675). No lifted row forgets the anchor.
pub(super) fn anchor_lift(
    anchor: &std::cell::RefCell<Option<(String, Point)>>,
    keys: &[String],
    rows: &[Rectangle],
    offsets: &mut [(Vector, bool)],
) {
    let mut anchor = anchor.borrow_mut();
    let Some(first) = offsets.iter().position(|(_, lifted)| *lifted) else {
        *anchor = None;
        return;
    };
    let (key, now) = (&keys[first], rows[first].position());
    let at = match anchor.as_ref() {
        Some((anchored, at)) if anchored == key => *at,
        _ => {
            *anchor = Some((key.clone(), now));
            now
        }
    };
    for (offset, lifted) in offsets.iter_mut() {
        if *lifted {
            *offset += at - now;
        }
    }
}
