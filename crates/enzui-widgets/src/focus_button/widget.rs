//! Focus-button layout, event ownership, semantics and paint.

use super::*;
use iced::advanced::layout::Measure as _;

pub(crate) fn inset_background_bounds(bounds: Rectangle, inset: f32) -> Rectangle {
    let inset = enzui_core::metrics::FiniteNonNeg::new_or(inset, 0.0)
        .get()
        .min(bounds.height / 2.0);
    Rectangle {
        y: bounds.y + inset,
        height: bounds.height - 2.0 * inset,
        ..bounds
    }
}

impl tree::Interactive for State {
    fn cancel_interaction(&mut self) {
        self.is_pressed = false;
        self.pressed_finger = None;
        self.space_held = false;
        self.press_modifiers = keyboard::Modifiers::default();
        self.press_point = None;
        self.activate_pending = false;
    }
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for FocusButton<'a, Message, Theme, Renderer>
where
    Message: 'a + Clone,
    Renderer: 'a + iced::advanced::Renderer,
    Theme: Catalog<Class<'a> = StyleFn<'a, Theme>> + FocusRingTheme,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::interactive(State {
            identity: self.id.clone(),
            ..State::default()
        })
    }
    fn frame_interest(&self, tree: &Tree) -> bool {
        let state = tree.state.downcast_ref::<State>();
        state.frame_owed
            || state.status.is_none()
            || state.fade.is_moving()
            || tree.children.first().is_some_and(Tree::has_frame_interest)
    }

    fn diff(&mut self, tree: &mut Tree) {
        let state = tree.state.downcast_mut::<State>();
        if state.identity != self.id {
            *state = State {
                identity: self.id.clone(),
                ..State::default()
            };
        }

        // Disabling during reconciliation cancels pending ownership before any later re-enable.
        if self.on_press.is_none() {
            let state = tree.state.downcast_mut::<State>();
            state.is_pressed = false;
            state.pressed_finger = None;
            state.space_held = false;
            state.press_point = None;
            state.activate_pending = false;
        }
        if self.relinquish_focus_while_open {
            let state = tree.state.downcast_mut::<State>();
            if state.focus.is_focused() {
                state.focus.unfocus();
            }
        }
        // Only retained, previously updated trees may consume a focus-return record.
        if let Some(id) = self.reclaim_focus_on_return.clone() {
            let state = tree.state.downcast_mut::<State>();
            if state.has_updated && !state.focus.is_focused() {
                let window = state.window_key;
                if crate::focus_return::reclaim(window, &id) {
                    state.focus.focus();
                } else {
                    crate::focus_return::forget(window, &id);
                }
            }
        }
        tree.diff_children(std::slice::from_mut(&mut self.content));
    }

    fn size(&self) -> Size<Length> {
        Size {
            width: self.width,
            height: self.height,
        }
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) {
        let derived_align = |len: Length| {
            if matches!(len, Length::Shrink | Length::Fit) {
                iced::Alignment::Start
            } else {
                iced::Alignment::Center
            }
        };
        let align_x = self
            .content_align_x
            .unwrap_or_else(|| derived_align(self.width));
        let align_y = self
            .content_align_y
            .unwrap_or_else(|| derived_align(self.height));
        layout::padded_aligned(
            limits,
            self.width,
            self.height,
            self.padding,
            [align_x, align_y],
            |limits| {
                let loose = limits.loose();
                let limits = if self.loose_content { &loose } else { limits };
                self.content
                    .as_widget_mut()
                    .measure(&mut tree.children[0], renderer, limits)
            },
        )
        .with_content_viewport_clip(self.clip)
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
        let state = tree.state.downcast_mut::<State>();
        if self.embedded {
            if state.focus.is_focused() {
                state.focus.unfocus();
            }
        } else if self.on_press.is_some() || self.focusable_without_activation {
            operation.focusable(self.id.as_ref(), layout.bounds(), state);
        } else if state.focus.is_focused() {
            state.focus.unfocus();
        }
        if let Some(semantics) = &self.semantics {
            let focused = if self.embedded {
                self.focus_visible
            } else {
                state.focus.is_focused()
            };
            let disabled = self
                .semantics_disabled
                .unwrap_or_else(|| self.on_press.is_none());
            let mut live = semantics.clone().focused(focused).disabled(disabled);
            if self.on_press.is_some() {
                live = live.default_action();
            }
            let mut node = SemanticsNode::new(live);
            if let Some(ct) = &self.content_text {
                node = node.with_content_text(ct.clone());
            }
            operation.custom(self.id.as_ref(), layout.bounds(), &mut node);
        }
        if self.on_press.is_some()
            && let Some(id) = &self.id
        {
            let mut request = crate::activation::ActivationRequest::new(id.clone());
            operation.custom(Some(id), layout.bounds(), &mut request);
            if request.armed() {
                tree.state.downcast_mut::<State>().activate_pending = true;
            }
        }
        if self.on_press.is_some()
            && let Some(alias) = &self.expand_alias
        {
            let mut request = crate::activation::ExpandRequest::new(alias.clone());
            operation.custom(Some(alias), layout.bounds(), &mut request);
            if request.armed() {
                tree.state.downcast_mut::<State>().activate_pending = true;
            }
        }
        operation.container(self.id.as_ref(), layout.bounds(), viewport);
        operation.traverse(&mut |operation| {
            let child_layout = layout
                .children(&tree.children)
                .next()
                .expect("the layout has its one child");
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                child_layout,
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
        renderer: &Renderer,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        if !matches!(event, Event::Window(window::Event::RedrawRequested(_))) {
            tree.state.downcast_mut::<State>().frame_owed = true;
        }
        let window_key = enzui_core::focus::window_key(shell.window());
        {
            let state = tree.state.downcast_mut::<State>();
            state.window_key = window_key;
            state.has_updated = true;
        }
        // Modality is ambient window state and must update even when a sibling captured the
        // event.
        enzui_core::focus::note_input_event_for(event, window_key);

        let child_layout = layout
            .children(&tree.children)
            .next()
            .expect("the layout has its one child");
        let captured_before = shell.is_event_captured();
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        // A control inside this one (a row's trailing button) that took the
        // press owns focus; this one lets go rather than claiming it too.
        let descendant_took_press = !captured_before && shell.is_event_captured();

        let bounds = layout.bounds();
        // Clear sibling focus before the capture guard; the hit control may have captured this
        // press.
        if !self.embedded
            && let Some(verdict) = press_focus_from_cursor(event, cursor, bounds)
        {
            let focusable = self.on_press.is_some() || self.focusable_without_activation;
            let state = tree.state.downcast_mut::<State>();
            let was_focused = state.focus.is_focused();
            let verdict = match verdict {
                PressFocus::Take if descendant_took_press => PressFocus::Drop,
                verdict => verdict,
            };
            match verdict {
                PressFocus::Take if focusable => state.focus.focus(),
                PressFocus::Take | PressFocus::Drop => state.focus.unfocus(),
                PressFocus::Leave => {}
            }
            if state.focus.is_focused() != was_focused {
                shell.request_redraw();
            }
        }
        // Refresh paint caches even on captured frame events to avoid repeated false change
        // detection.
        if let Event::Window(window::Event::RedrawRequested(now)) = event {
            let (status, focus_visible) = {
                let state = tree.state.downcast_ref::<State>();
                (
                    self.status(state, bounds, cursor),
                    self.is_focus_visible(state),
                )
            };
            let state = tree.state.downcast_mut::<State>();
            state.status = Some(status);
            state.focus_visible_painted = Some(focus_visible);
            state.frame_owed = false;
            let moving = state.fade.frame(
                *now,
                self.reduced_motion,
                // A press lands at once; its release fades.
                status == FocusStatus::Pressed,
                |theme: &Theme| self.fill_style(theme, status),
            );
            if moving {
                shell.request_redraw();
            }
        }
        {
            let state = tree.state.downcast_mut::<State>();
            // Consume routed ownership before dispatch; event capture does not suppress AT
            // activation.
            if state.activate_pending {
                state.activate_pending = false;
                let activation = self.on_enter.as_ref().or(self.on_press.as_ref());
                if let Some(a) = activation {
                    a.dispatch(shell, keyboard::Modifiers::default(), None);
                    shell.request_redraw();
                }
            }
        }
        // Every button needs ambient modifier changes, including those captured by another
        // control.
        if let Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) = event {
            tree.state.downcast_mut::<State>().live_modifiers = *modifiers;
        }

        if matches!(event, Event::Window(window::Event::Unfocused)) {
            let state = tree.state.downcast_mut::<State>();
            state.is_pressed = false;
            state.pressed_finger = None;
            state.space_held = false;
            state.press_modifiers = keyboard::Modifiers::default();
            state.press_point = None;
            shell.request_redraw();
        }

        if shell.is_event_captured() {
            return;
        }

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
                if self.on_press.is_some() =>
            {
                let hit_control = cursor.is_over(bounds);
                if hit_control {
                    let state = tree.state.downcast_mut::<State>();
                    state.is_pressed = true;
                    state.press_modifiers = state.live_modifiers;
                    state.press_point = cursor.position();
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            Event::Touch(touch::Event::FingerPressed { id, position })
                if self.on_press.is_some() =>
            {
                let hit_control = bounds.contains(*position);
                if hit_control {
                    let state = tree.state.downcast_mut::<State>();
                    if !state.is_pressed {
                        state.is_pressed = true;
                        state.pressed_finger = Some(*id);
                        state.press_point = Some(*position);
                        shell.capture_event();
                        shell.request_redraw();
                    }
                }
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let state = tree.state.downcast_mut::<State>();
                if state.is_pressed && state.pressed_finger.is_none() {
                    // Release cancels ownership even without a callback; only publication is
                    // gated.
                    let mods = state.press_modifiers;
                    let at = state.press_point;
                    state.is_pressed = false;
                    state.press_modifiers = keyboard::Modifiers::default();
                    state.press_point = None;
                    if let Some(on_press) = &self.on_press {
                        if cursor.is_over(bounds) {
                            on_press.dispatch(shell, mods, at);
                        }
                        shell.capture_event();
                    }
                }
            }
            Event::Touch(touch::Event::FingerLifted { id, position }) => {
                let state = tree.state.downcast_mut::<State>();
                if state.is_pressed && state.pressed_finger == Some(*id) {
                    let at = state.press_point;
                    state.is_pressed = false;
                    state.pressed_finger = None;
                    state.press_point = None;
                    if let Some(on_press) = &self.on_press {
                        if bounds.contains(*position) {
                            on_press.dispatch(shell, keyboard::Modifiers::default(), at);
                        }
                        shell.capture_event();
                    }
                }
            }
            Event::Touch(touch::Event::FingerLost { id, .. }) => {
                let state = tree.state.downcast_mut::<State>();
                if state.pressed_finger == Some(*id) {
                    state.is_pressed = false;
                    state.pressed_finger = None;
                    state.press_point = None;
                    shell.request_redraw();
                }
            }
            Event::Keyboard(kb @ keyboard::Event::KeyPressed { key, .. })
                if self.on_press.is_some() && !self.embedded && activation_from_key(kb) =>
            {
                let state = tree.state.downcast_mut::<State>();
                if state.focus.is_focused() {
                    use keyboard::key::Named;
                    match key {
                        keyboard::Key::Named(Named::Enter) => {
                            let activation = self.on_enter.as_ref().or(self.on_press.as_ref());
                            if let Some(a) = activation {
                                a.dispatch(shell, keyboard::Modifiers::default(), None);
                            }
                            shell.capture_event();
                            shell.request_redraw();
                        }
                        keyboard::Key::Named(Named::Space) => {
                            state.is_pressed = true;
                            state.space_held = true;
                            shell.capture_event();
                            shell.request_redraw();
                        }
                        _ => {}
                    }
                }
            }
            Event::Keyboard(keyboard::Event::KeyReleased { key, .. }) if !self.embedded => {
                let state = tree.state.downcast_mut::<State>();
                if matches!(key, keyboard::Key::Named(keyboard::key::Named::Space))
                    && state.space_held
                {
                    state.space_held = false;
                    state.is_pressed = false;
                    if state.focus.is_focused() {
                        let activation = self.on_space.as_ref().or(self.on_press.as_ref());
                        if let Some(a) = activation {
                            a.dispatch(shell, keyboard::Modifiers::default(), None);
                        }
                    }
                    shell.capture_event();
                    shell.request_redraw();
                }
            }
            _ => {}
        }
        if !matches!(event, Event::Window(window::Event::RedrawRequested(_))) {
            let state = tree.state.downcast_ref::<State>();
            let current_status = self.status(state, bounds, cursor);
            let status_changed = state.status.is_some_and(|status| status != current_status);
            let ring_changed = state
                .focus_visible_painted
                .is_some_and(|was| was != self.is_focus_visible(state));
            if status_changed || ring_changed {
                shell.request_redraw();
            }
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let content_layout = layout
            .children(&tree.children)
            .next()
            .expect("the layout has its one child");
        let state = tree.state.downcast_ref::<State>();
        let status = self.status(state, bounds, cursor);
        if let Some(content_status) = &self.content_status {
            content_status.set(status);
        }
        let focus_visible = self.is_focus_visible(state);
        // Resolve synthetic focus style before live fill style so child tint cells end with
        // live status.
        let ring_border = if self.uses_default_ring() {
            None
        } else if focus_visible && status != FocusStatus::Focused {
            let ring = self.resolve_style(theme, FocusStatus::Focused);
            Some(ring.border)
        } else {
            None
        };
        let mut style = state.fade.paint(theme, self.fill_style(theme, status));
        let default_ring = if focus_visible {
            self.default_ring_border(theme, &style)
        } else {
            None
        };

        // Inset focus rings preserve the selection border.
        if !self.inset_focus_ring
            && let Some(border) = ring_border
            && border.width > 0.0
        {
            style.border = border;
        }

        if (self.inset_focus_ring && focus_visible)
            || default_ring.is_some()
            || self.underline_fn.is_some()
        {
            // Separate ring and underline quads cannot be represented by the cached
            // presentation quad alone.
            layout.reject_presentation_paint(tree, renderer);
        }
        if style.background.is_some() || style.border.width > 0.0 || style.shadow.color.a > 0.0 {
            layout.fill_presentation_quad(
                tree,
                renderer,
                renderer::Quad {
                    bounds: inset_background_bounds(bounds, self.background_inset_y),
                    border: style.border,
                    shadow: style.shadow,
                    snap: style.snap,
                },
                style
                    .background
                    .unwrap_or(Background::Color(Color::TRANSPARENT)),
            );
        }
        if self.inset_focus_ring && focus_visible {
            let ring_b = ring_border
                .unwrap_or_else(|| self.resolve_style(theme, FocusStatus::Focused).border);
            let rw = ring_b.width;
            if rw > 0.0 {
                let outset = style.border.width;
                let inset_bounds = Rectangle {
                    x: bounds.x + outset,
                    y: bounds.y + outset,
                    width: (bounds.width - 2.0 * outset).max(0.0),
                    height: (bounds.height - 2.0 * outset).max(0.0),
                };
                if inset_bounds.width > 0.0 && inset_bounds.height > 0.0 {
                    renderer.fill_quad(
                        renderer::Quad {
                            bounds: inset_bounds,
                            border: ring_b,
                            shadow: iced::Shadow::default(),
                            snap: style.snap,
                        },
                        Background::Color(Color::TRANSPARENT),
                    );
                }
            }
        }

        let content_viewport = if self.clip {
            bounds.intersection(viewport).unwrap_or(*viewport)
        } else {
            *viewport
        };

        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            &renderer::Style {
                text_color: style.text_color,
            },
            content_layout,
            cursor,
            &content_viewport,
        );
        // Use a later layer: backends draw quads before canvas primitives within a single
        // layer.
        if let Some(ring) = default_ring {
            let outset = style.border.width.max(0.0);
            let ring_bounds = Rectangle {
                x: bounds.x + outset,
                y: bounds.y + outset,
                width: (bounds.width - 2.0 * outset).max(0.0),
                height: (bounds.height - 2.0 * outset).max(0.0),
            };
            if ring_bounds.width > 0.0 && ring_bounds.height > 0.0 {
                renderer.with_layer(ring_bounds, |renderer| {
                    renderer.fill_quad(
                        renderer::Quad {
                            bounds: ring_bounds,
                            border: ring,
                            shadow: iced::Shadow::default(),
                            snap: style.snap,
                        },
                        Background::Color(Color::TRANSPARENT),
                    );
                });
            }
        }
        let pointer_over = cursor.is_over(bounds);
        if let Some(underline_fn) = &self.underline_fn
            && underline_fn(status, pointer_over)
        {
            let label_bounds = content_layout
                .children(&tree.children[0].children)
                .next()
                .map_or_else(|| content_layout.bounds(), |l| l.bounds());
            // Generic content exposes bounds, not ragged paragraph widths; each rule spans the
            // label box.
            let line_height = self.underline_line_height;
            let line_count = match line_height {
                Some(lh) if label_bounds.height > lh * 1.5 => {
                    (label_bounds.height / lh).round().max(1.0) as usize
                }
                _ => 1,
            };
            let row_h = match line_height {
                Some(lh) if line_count > 1 => lh,
                _ => label_bounds.height,
            };
            for line in 0..line_count {
                let row_top = label_bounds.y + row_h * line as f32;
                let underline = Rectangle {
                    x: label_bounds.x,
                    y: row_top + row_h - 1.0,
                    width: label_bounds.width,
                    height: 1.0,
                };
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: underline,
                        border: iced::Border::default(),
                        shadow: iced::Shadow::default(),
                        snap: true,
                    },
                    Background::Color(style.text_color),
                );
            }
        }
    }

    fn mouse_interaction(
        &self,
        _tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        _viewport: &Rectangle,
        _renderer: &Renderer,
    ) -> mouse::Interaction {
        if cursor.is_over(layout.bounds()) && self.on_press.is_some() {
            mouse::Interaction::Pointer
        } else {
            mouse::Interaction::default()
        }
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: iced::Vector,
        window: Size,
    ) -> Vec<iced::advanced::overlay::Element<'b, Message, Theme, Renderer>> {
        let child_layout = layout.children(&tree.children).next();
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            child_layout.unwrap_or(layout),
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<FocusButton<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a + Clone,
    Renderer: 'a + iced::advanced::Renderer,
    Theme: 'a + Catalog<Class<'a> = StyleFn<'a, Theme>> + FocusRingTheme,
{
    fn from(button: FocusButton<'a, Message, Theme, Renderer>) -> Self {
        Self::new(button)
    }
}
