//! The `Widget` implementations of the scroll view's wrappers: keyboard
//! scrolling, key forwarding and geometry and translation publishing.

use super::*;

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for KeyboardScroll<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        iced::advanced::widget::tree::Tag::of::<ScrollInputState>()
    }

    fn state(&self) -> iced::advanced::widget::tree::State {
        iced::advanced::widget::tree::State::new(ScrollInputState::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        tree.children
            .first()
            .is_none_or(|child| self.child.as_widget().frame_interest(child))
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        let child = &mut tree.children[0];
        self.child
            .as_widget_mut()
            .update_frame(child, event, layout, cursor, renderer, shell, viewport);
        child.refresh_frame_interest(self.child.as_widget());
        if self.keyboard_scroll {
            if let Event::Window(iced::window::Event::RedrawRequested(frame)) = event {
                advance_scroll_focus_epoch(*frame);
            }
            let focused = subtree_has_focus(&mut self.child, child, layout, renderer);
            register_scroll_focus(layout.bounds(), focused);
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
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
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );

        // publish this region's focus into the global registry on every
        // operation pass. This is the path that catches focus moved *without*
        // an event reaching us: the app-level `focus_next`/`focus_previous`
        // traversal, a host's programmatic `focus(id)`, and the `find_focused`
        // sweep an AT runs all arrive as operations, not events. Because
        // `operate` visits every sibling wrapper in one pass, the registry is
        // fully populated before any later `update` reads it.
        if !self.keyboard_scroll {
            return;
        }
        let focused = subtree_has_focus(&mut self.child, &mut tree.children[0], layout, renderer);
        register_scroll_focus(layout.bounds(), focused);
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
        if let Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) = event {
            tree.state.downcast_mut::<ScrollInputState>().modifiers = *modifiers;
        }
        let modifiers = tree.state.downcast_ref::<ScrollInputState>().modifiers;
        let tree = &mut tree.children[0];

        // The real event goes down first, unchanged. A focused descendant (a
        // text editor, a list with its own paging) therefore gets first
        // refusal on PageDown, and a scroll region that ate its child's key
        // would be a worse bug than the one this fixes.
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);

        if !self.wheel_chaining
            && !shell.is_event_captured()
            && let Event::Mouse(mouse::Event::WheelScrolled { delta }) = event
            && let Some(geometry) = first_scroll_geometry(&mut self.child, tree, layout, renderer)
            && cursor.land().is_over(geometry.bounds)
        {
            let delta = match *delta {
                mouse::ScrollDelta::Lines { x, y }
                    if modifiers.shift() && !cfg!(target_os = "macos") =>
                {
                    Vector::new(y, x)
                }
                mouse::ScrollDelta::Lines { x, y } | mouse::ScrollDelta::Pixels { x, y } => {
                    Vector::new(x, y)
                }
            };
            let contains_x = !matches!(self.axis, Axis::Vertical)
                && geometry.is_overflowing_x()
                && delta.x != 0.0;
            let contains_y = !matches!(self.axis, Axis::Horizontal)
                && geometry.is_overflowing_y()
                && delta.y != 0.0;
            if contains_x || contains_y {
                shell.capture_event();
            }
        }
        if !self.keyboard_scroll {
            return;
        }

        // a new frame's redraw advances the epoch so stale entries from
        // removed scroll regions expire. De-duplicated per frame instant, so
        // the several sibling wrappers that each forward the same redraw do not
        // over-advance it.
        if let Event::Window(iced::window::Event::RedrawRequested(frame)) = event {
            advance_scroll_focus_epoch(*frame);
        }

        let bounds = layout.bounds();

        // keep the global scroll-focus registry current from the event
        // path too, so an event-driven focus change (a Tab or a click routed to
        // an inner focusable) is reflected without waiting for the next
        // operation pass. The child ran first above, so its focus flag is
        // already settled when we probe.
        let has_focused_descendant = subtree_has_focus(&mut self.child, tree, layout, renderer);
        register_scroll_focus(bounds, has_focused_descendant);

        if shell.is_event_captured() {
            return;
        }

        // Not one of our keys: pass it on untouched. Nothing is captured here,
        // so an unhandled key still reaches the app's own bindings.
        let Some(scroll_key) = ScrollKey::from_event(event) else {
            return;
        };

        // Focus ownership: CONTAINING-FOCUSED-WIDGET regions react, never
        // pointer-resting ones. Pointers are irrelevant to keyboard events:
        // carets in region A page A regardless of mouse rests. Pointer
        // fallbacks available only when zero window scroll regions focus — not
        // merely own-subtree-focusless. Siblings invisible during own `update`s;
        // arbitrary dispatch orders let hovered-unfocused first-visited siblings
        // capture ahead of truly-focused regions. `any_scroll_region_focused()`
        // reads whole-window registry facts: focused regions win tree-order-
        // independently. Focusless windows still pointer-fallback (caretless
        // users: mice are the best intent hints).
        let may_fallback_to_pointer = !any_scroll_region_focused();
        let owns_via_focus = has_focused_descendant;
        let owns_via_pointer = may_fallback_to_pointer && cursor.position_over(bounds).is_some();
        if !owns_via_focus && !owns_via_pointer {
            return;
        }

        // The scroll amount is derived from the viewport extent, so a
        // degenerate viewport has no meaningful page to turn. Do nothing at
        // all — no forwarded event, no capture — rather than emitting a
        // zero-length "page turn" that still publishes an `on_scroll`.
        let is_horizontal = matches!(self.axis, Axis::Horizontal);
        let extent = if is_horizontal {
            bounds.width
        } else {
            bounds.height
        };
        let Some(amount) = scroll_key.delta(extent) else {
            return;
        };

        // Handoff invariant: capture only on actually-moved offsets.
        // Bottomed regions swallowing PageDowns starve outer scroll views /
        // applications despite zero scrolling. Direct-`scroll_by` operations
        // (never synthetic wheels): `in_transaction` captures block nested
        // handoffs (inner-forwarded synthetic wheels transaction-captured,
        // outers never see); operations bypass event systems, directly
        // modifying offsets. Sign conventions: `scroll_by` positives scroll
        // TOWARDS ends (down/right); `ScrollKey::delta` PageDown negatives
        // (wheel conventions) — hence negated.
        let scroll_delta = if is_horizontal {
            Vector::new(-amount, 0.0)
        } else {
            Vector::new(0.0, -amount)
        };

        let moved = scroll_first_and_present(
            &mut self.child,
            tree,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
            scroll_delta,
        )
        // If we can't find the scrollable (shouldn't happen), be
        // conservative and capture.
        .unwrap_or(true);
        if moved {
            shell.capture_event();
        }
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
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
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
        renderer: &Renderer,
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
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'b, Message, Theme, Renderer>> {
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

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for KeyScrollForward<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    // Fully transparent to layout/state — the child owns everything visual.
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }
    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }
    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }
    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }
    fn frame_interest(&self, tree: &Tree) -> bool {
        self.child.as_widget().frame_interest(tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget_mut()
            .update_frame(tree, event, layout, cursor, renderer, shell, viewport);
        tree.refresh_frame_interest(self.child.as_widget());
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
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
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
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
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
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
        // Child gets first refusal on every event: an inner `KeyboardScroll`
        // whose region *is* under the cursor, an open combo box, or any other
        // consumer of Page/Home/End still wins here. This is the same
        // ordering `EscapeCapture` uses for Escape.
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
        if shell.is_event_captured() {
            return;
        }

        // Not one of our four keys: passthrough. Escape, Tab, arrows and app
        // bindings all reach whatever binds them after us.
        let Some(scroll_key) = ScrollKey::from_event(event) else {
            return;
        };

        // Locate the first scrollable so we can compute a page delta against
        // its own viewport, not our (possibly larger) surface bounds. Without
        // this, a small scroll region inside a big modal would jump by the
        // modal's height and blow straight past its content in one press.
        let Some(bounds) = first_scrollable_bounds(&mut self.child, tree, layout, renderer) else {
            return;
        };
        // Vertical-only: the modal surfaces this wraps (shortcuts dialog
        // today) list content vertically. If a future caller needs horizontal
        // forwarding, an axis constructor argument is the extension point;
        // deliberately not added here to avoid API surface no one calls yet.
        let Some(amount) = scroll_key.delta(bounds.height) else {
            return;
        };
        let delta = Vector::new(0.0, -amount);

        let Some(moved) = scroll_first_and_present(
            &mut self.child,
            tree,
            layout,
            cursor,
            renderer,
            shell,
            viewport,
            delta,
        ) else {
            // No scrollable in the subtree: nothing to capture. Deliberately
            // not capturing keeps the wrapper safe to compose around any
            // element — a dialog with only static text still lets Page/Home/
            // End fall through to app bindings.
            return;
        };
        if !moved {
            // Already at an edge: leave the key uncaptured so an outer scroll
            // region or app binding can pick it up. This mirrors defect
            // B in `KeyboardScroll::update`.
            return;
        }
        shell.capture_event();
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for PublishGeometry<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }
    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }
    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }
    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }
    fn frame_interest(&self, tree: &Tree) -> bool {
        self.child.as_widget().frame_interest(tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget_mut()
            .update_frame(tree, event, layout, cursor, renderer, shell, viewport);
        tree.refresh_frame_interest(self.child.as_widget());
        let cell = self.cell.clone();
        stamp_first_geometry(&mut self.child, tree, layout, renderer, &cell);
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
        let cell = self.cell.clone();
        stamp_first_geometry(&mut self.child, tree, layout, renderer, &cell);
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
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
        let cell = self.cell.clone();
        stamp_first_geometry(&mut self.child, tree, layout, renderer, &cell);
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
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<iced::advanced::overlay::Element<'b, Message, Theme, Renderer>> {
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for PublishTranslation<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    // Fully transparent: the child owns tree state, so the inner Scrollable's
    // offset survives this wrapper untouched.
    fn tag(&self) -> iced::advanced::widget::tree::Tag {
        self.child.as_widget().tag()
    }
    fn state(&self) -> iced::advanced::widget::tree::State {
        self.child.as_widget().state()
    }
    fn diff(&mut self, tree: &mut Tree) {
        self.child.as_widget_mut().diff(tree);
    }
    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }
    fn frame_interest(&self, tree: &Tree) -> bool {
        self.child.as_widget().frame_interest(tree)
    }

    fn update_frame(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child
            .as_widget_mut()
            .update_frame(tree, event, layout, cursor, renderer, shell, viewport);
        tree.refresh_frame_interest(self.child.as_widget());
        let cell = self.cell.clone();
        if stamp_first_translation(&mut self.child, tree, layout, renderer, &cell)
            && cell.get() != self.built_translation
        {
            shell.invalidate_widgets();
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        self.child.as_widget_mut().layout(tree, renderer, limits);
    }
    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        // The caller's operation runs first (unchanged), then we probe.
        self.child
            .as_widget_mut()
            .operate(tree, layout, viewport, renderer, operation);
        let cell = self.cell.clone();
        stamp_first_translation(&mut self.child, tree, layout, renderer, &cell);
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
        self.child
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, shell, viewport);
        let cell = self.cell.clone();
        // Operations may have stamped the cell already; only a rebuilt view consumes it.
        if stamp_first_translation(&mut self.child, tree, layout, renderer, &cell)
            && cell.get() != self.built_translation
        {
            shell.invalidate_widgets();
        }
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
        self.child
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.child
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
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
        self.child
            .as_widget_mut()
            .overlay(tree, layout, renderer, viewport, translation, window)
    }
}
