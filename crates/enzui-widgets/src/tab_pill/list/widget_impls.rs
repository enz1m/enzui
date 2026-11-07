//! The `Widget` implementations of the tab list and its narrow viewport.

use super::*;

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for NarrowTabViewport<'_, Message>
where
    Message: Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<NarrowTabViewportState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(NarrowTabViewportState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        tree.children.first().is_none_or(Tree::has_frame_interest)
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let max = limits.max();
        let widened_max = Size::new(max.width.max(self.content_floor), max.height);
        let widened =
            layout::Limits::with_compression(limits.min(), widened_max, limits.compression());
        let mut child =
            self.child
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, &widened);
        let request = self.child.as_widget().size();
        let outer = limits.resolve(request.width, request.height, child.size());
        let active = child.size().width > outer.width + 0.5;
        tree.state.downcast_mut::<NarrowTabViewportState>().0 = active;
        if !active {
            return layout::Node::new(child.size()).apply(tree);
        }

        let min_x = (outer.width - child.size().width).min(0.0);
        // The measured node is shallow; the cells are in the strip's tree,
        // below any wrappers that own their child's tree.
        let mut row = &tree.children[0];
        let mut row_x = 0.0;
        while let [only] = row.children.as_slice() {
            row_x += only.translation.x;
            row = only;
        }
        let offset = row
            .children
            .get(self.selected * self.cell_stride)
            .map_or(0.0, |cell| {
                outer.width / 2.0 - (row_x + cell.translation.x + cell.size.width / 2.0)
            })
            .clamp(min_x, 0.0);
        child = child.move_to(iced::Point::new(offset, 0.0));
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
        let active = tree.state.downcast_ref::<NarrowTabViewportState>().0;
        let child_layout = Self::child_layout(active, layout, tree);
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            child_layout,
            viewport,
            renderer,
            operation,
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
        let active = tree.state.downcast_ref::<NarrowTabViewportState>().0;
        let child_layout = Self::child_layout(active, layout, tree);
        let clipped = if active {
            layout
                .bounds()
                .intersection(viewport)
                .unwrap_or_else(|| layout.bounds())
        } else {
            *viewport
        };
        let cursor = if active {
            crate::inert_clip::cursor_within(cursor, layout.bounds())
        } else {
            cursor
        };
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            &clipped,
        );
        tree.children[0].refresh_frame_interest(self.child.as_widget());
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
        if !tree.children[0].has_frame_interest() {
            return;
        }
        let active = tree.state.downcast_ref::<NarrowTabViewportState>().0;
        let child_layout = Self::child_layout(active, layout, tree);
        let clipped = if active {
            layout
                .bounds()
                .intersection(viewport)
                .unwrap_or_else(|| layout.bounds())
        } else {
            *viewport
        };
        self.child.as_widget_mut().update_frame(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            &clipped,
        );
        tree.children[0].refresh_frame_interest(self.child.as_widget());
        tree.refresh_frame_children();
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
        let active = tree.state.downcast_ref::<NarrowTabViewportState>().0;
        let child_layout = Self::child_layout(active, layout, tree);
        if !active {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                child_layout,
                cursor,
                viewport,
            );
            return;
        }
        let Some(clip) = layout.bounds().intersection(viewport) else {
            return;
        };
        let cursor = crate::inert_clip::cursor_within(cursor, layout.bounds());
        iced::advanced::Renderer::with_layer(renderer, clip, |renderer| {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                child_layout,
                cursor,
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
        let active = tree.state.downcast_ref::<NarrowTabViewportState>().0;
        let child_layout = Self::child_layout(active, layout, tree);
        let cursor = if active {
            crate::inert_clip::cursor_within(cursor, layout.bounds())
        } else {
            cursor
        };
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            child_layout,
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
        let active = tree.state.downcast_ref::<NarrowTabViewportState>().0;
        let child_layout = Self::child_layout(active, layout, tree);
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

impl<'a, Message> Widget<Message, iced::Theme, iced::Renderer> for TabListWidget<'a, Message>
where
    Message: 'a + Clone,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<TabListState>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(TabListState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        // Release stale focus at the rebuild: a strip focused while enabled
        // then rebuilt disabled would ring the selected pill yet publish
        // `focused: true` (the publishing branch runs post-`draw`, AT-only).
        // `self.enabled` is the same gate `operate` uses; selection lives on
        // the widget, so only the modality key persists — untouched here.
        {
            let state = tree.state.downcast_mut::<TabListState>();
            let _ = release_focus_if_inert(&mut state.focus, self.enabled);
        }
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn is_void(&self) -> bool {
        // Defaulted trait method: delegate so a `Void` child stays void and
        // Row/Column/Stack still filter it out.
        self.child.as_widget().is_void()
    }

    /// The strip is on the presented-frame route only while something under
    /// it owes a frame.
    ///
    /// No clock here: state is a focus flag plus the modality key, both
    /// input-moved, so the answer is exactly the [`IndicatorLayer`] branch
    /// below (selection animation plus descendants' interest). Publishing it
    /// keeps the route off every strip cell/label/icon when only something
    /// else animates — the default `update_frame` calls `update`, which
    /// would fan every presented frame over the whole subtree. An
    /// not-yet-diffed tree (no child slot) counts as interested; gated with
    /// [`Widget::update_frame`] by `tab_pill::indicator_frame_routing`.
    fn frame_interest(&self, tree: &Tree) -> bool {
        tree.children.first().is_none_or(Tree::has_frame_interest)
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        let state = tree.state.downcast_mut::<TabListState>();
        // One focus stop for the whole strip (WAI-ARIA tablist) — but only when
        // the strip is live. A disabled strip must not be a dead Tab stop, the
        // same rule `focus_button::operate` applies to a button with no
        // `on_press`; and if it still held focus from an earlier frame, drop it
        // so Tab can move past.
        if self.enabled {
            operation.focusable(self.id.as_ref(), layout.bounds(), state);
        } else if state.focus.is_focused() {
            state.focus.unfocus();
        }
        let focused = tree.state.downcast_ref::<TabListState>().focus.is_focused();

        // Publish the group node, then descend: each cell publishes its own
        // Tab leaf (own bounds) from its wrapping `SemanticsContainer`, so
        // leaves land one level deeper as descendants — batch-publishing here
        // would give every leaf the whole strip's bounds and break
        // duplicate-label suppression. Cells are `embedded` (none publishes
        // `focused`), so the group names the cursored cell as
        // `active_descendant` — the WAI-ARIA answer — using the same index
        // `draw` rings and `nav_target` steps from, so the announced cell is
        // the seen cell by construction.
        let cursor_cell_id = self
            .cursor_index()
            .and_then(|i| self.tab_ids.get(i).cloned())
            .flatten();
        let mut semantics = self.group_semantics(focused);
        if let Some(cell) = cursor_cell_id {
            semantics = semantics.active_descendant(cell);
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
        // Feed this window's input modality from every raw event, so
        // a pointer-last frame suppresses the keyboard ring; keyed per window
        // and stashed for `draw`.
        let window_key = enzui_core::focus::window_key(shell.window());
        tree.state.downcast_mut::<TabListState>().window_key = window_key;
        enzui_core::focus::note_input_event_for(event, window_key);

        // Let the child (mouse_area cells) handle pointer clicks first.
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        // The branch below may have STARTED an animation in response to
        // this very event — a click that arms the selection indicator, a pulse
        // inside a cell. Publish its new interest and reconcile the routing
        // index here, or the cached interest at this level still says "static"
        // and the frame it just asked for is delivered to nobody.
        tree.children[0].refresh_frame_interest(self.child.as_widget());
        tree.refresh_frame_children();

        let bounds = layout.bounds();

        // Pointer focus is EXCLUSIVE. The strip is one composite
        // widget owning the focus for its whole list of pills, so this is a
        // single `press_focus` against the composite's bounds — a press on any
        // pill = Take, a press anywhere else = Drop. Runs before the disabled
        // early-return so a disabled strip still drops focus, and takes no
        // focus of its own when disabled. Selection is untouched here.
        if let Some(decision) = press_focus_from_cursor(event, cursor, bounds) {
            let state = tree.state.downcast_mut::<TabListState>();
            let was_focused = state.focus.is_focused();
            match decision {
                PressFocus::Take if self.enabled => state.focus.focus(),
                PressFocus::Take | PressFocus::Drop => state.focus.unfocus(),
                PressFocus::Leave => {}
            }
            if state.focus.is_focused() != was_focused {
                shell.request_redraw();
            }
        }

        if !self.enabled {
            return;
        }

        // Focus on click is handled by the exclusive press-focus sweep above;
        // only keyboard navigation remains here.
        if let Event::Keyboard(kb) = event {
            let state = tree.state.downcast_ref::<TabListState>();
            if state.focus.is_focused()
                && matches!(
                    kb,
                    iced::keyboard::Event::KeyPressed {
                        key: iced::keyboard::Key::Named(
                            iced::keyboard::key::Named::ArrowLeft
                                | iced::keyboard::key::Named::ArrowRight
                                | iced::keyboard::key::Named::Home
                                | iced::keyboard::key::Named::End
                        ),
                        ..
                    }
                )
                && let Some(nav) =
                    navigation_from_key_in(kb, enzui_core::placement::ambient_direction())
                && let Some(target) = self.nav_target(nav)
                && target != self.selected
                // The second, independent defence on the keyboard route.
                // `self.enabled` above is already `EnzTabList::live()`, so an
                // unwired strip never reaches here -- but this arm publishes
                // through its own call to the callback, separate from the
                // pointer carrier, and a future relaxation of the enabled gate
                // must not be able to turn that into a call into a hole.
                && let Some(on_select) = &self.on_select
            {
                shell.publish(on_select(target));
                shell.request_redraw();
                shell.capture_event();
            }
        }
    }

    /// A presented frame visits only the interested branch.
    ///
    /// Default `update_frame` calls [`Widget::update`], which forwards every
    /// event into the whole strip — the override keeps a presented frame out
    /// of the subtree beneath the outermost node. `RedrawRequested` is the
    /// only actionable input here (press-focus needs a press, keyboard arm a
    /// key); the modality key still stashes for `draw`, un-noted like any
    /// redraw event.
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
        tree.state.downcast_mut::<TabListState>().window_key =
            enzui_core::focus::window_key(shell.window());

        if tree.children[0].has_frame_interest() {
            self.child.as_widget_mut().update_frame(
                &mut tree.children[0],
                event,
                layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
            // Same-frame settle: refresh the branch's interest and the routing
            // index now, so a strip whose animation ended on this frame leaves
            // the route immediately rather than one frame late.
            tree.children[0].refresh_frame_interest(self.child.as_widget());
            tree.refresh_frame_children();
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
        // Draw the pill row.
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );

        // Focus-visible ring around the active pill.
        //
        // Gate on the shared focus-visible policy, not raw focus, so
        // a pointer click focuses the strip without painting the keyboard ring.
        // (Only this composite overlay ring — the per-pill path already routes
        // through `focus_button`.)
        let state = tree.state.downcast_ref::<TabListState>();
        if !enzui_core::focus::focus_ring_visible_for(state.focus.is_focused(), state.window_key)
            || self.count == 0
        {
            return;
        }
        let tokens = resolve_tokens_with(self.pinned, self.geometry, theme);
        // Find the selected pill's bounds by walking the row's child layouts,
        // stepping over the interleaved seams in a segmented strip.
        if let Some(cell) = cell_layouts(layout, tree).nth(self.selected * self.cell_stride) {
            let b = cell.bounds();
            let pad = tokens.focus_ring_outset;
            crate::focus_button::paint_focus_ring(
                renderer,
                b,
                crate::focus_button::FocusRingQuad {
                    ink: tokens.focus_ring,
                    width: tokens.focus_ring_width,
                    radius: (tokens.pill_radius + pad).into(),
                    outset: pad,
                    snap: true,
                },
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
