impl<'a, Message> Widget<Message, iced::Theme, iced::Renderer> for EnzContextMenuHost<'a, Message>
where
    Message: Clone + 'a,
{
    // Opening must preserve the target tree and its scroll/focus state.
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::interactive(State::default())
    }

    fn diff(&mut self, tree: &mut Tree) {
        let mut surfaces = self.surfaces();
        let mut children: Vec<&mut Element<'_, Message>> = vec![&mut self.target];
        children.extend(surfaces.iter_mut());
        // Replacement targets and surfaces can change tags between views.
        tree.diff_children_custom(
            &mut children,
            |state, item| state.diff(item.as_widget_mut()),
            |item| Tree::new(item.as_widget()),
        );
    }

    fn is_void(&self) -> bool {
        self.target.as_widget().is_void()
    }

    fn size(&self) -> Size<Length> {
        self.target.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        self.target
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits);
        iced::advanced::layout::pass_through(tree, 0);
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
        let presenting = self.is_presenting();
        tree.state
            .downcast_mut::<State>()
            .reveal
            .track(event, presenting, shell);
        if self.state.is_open() && !self.is_available() {
            shell.publish((self.on_event)(ContextMenuEvent::Dismissed(
                CloseCause::Host,
            )));
            shell.request_redraw();
        }
        // Menu keys must get first refusal before a focused live-page control.
        if presenting
            && self.live_target
            && let Event::Keyboard(keyboard_event) = event
        {
            match event_for_open_key(self.model, self.state, keyboard_event) {
                Some(MenuEvent::Claimed(decoded)) => {
                    shell.publish((self.on_event)(decoded));
                    shell.capture_event();
                    return;
                }
                Some(MenuEvent::Swallowed) => {
                    shell.capture_event();
                    return;
                }
                None if is_focus_traversal(keyboard_event) => {
                    shell.capture_event();
                    return;
                }
                None => {}
            }
        }
        // A live child must neither act on nor capture the press needed to dismiss this menu.
        let press_belongs_to_dismissal = presenting
            && matches!(
                event,
                Event::Mouse(mouse::Event::ButtonPressed(_))
                    | Event::Touch(iced::touch::Event::FingerPressed { .. })
            );
        // A press the menu took arrives levitating: a control here holding focus lets
        // go, so the pressed menu row is the only focus owner.
        let press_taken_above = press_belongs_to_dismissal && cursor.is_levitating();
        if !presenting || press_taken_above || (self.live_target && !press_belongs_to_dismissal) {
            self.target.as_widget_mut().update(
                &mut tree.children[0],
                event,
                layout,
                cursor,
                renderer,
                shell,
                viewport,
            );
            crate::frame_routing::refresh_opaque_child(&self.target, tree, 0);
            if shell.is_event_captured() {
                return;
            }
        }

        if !self.enabled {
            return;
        }
        {
            let state = tree.state.downcast_mut::<State>();
            // Clear before publishing; child capture above can defer this pending work.
            if state.context_menu_pending {
                state.context_menu_pending = false;
                if self.is_available() {
                    shell.publish((self.on_event)(ContextMenuEvent::Invoked(
                        InvocationKind::Keyboard(bounds_to_viewport(layout.bounds(), Vector::ZERO)),
                    )));
                    shell.request_redraw();
                }
            }
        }

        match event {
            // Missed right-click invocation must fall through to outside dismissal.
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right))
                if self.model.has_focusable()
                    && cursor.position_over(layout.bounds()).is_some() =>
            {
                let Some(position) = cursor.position_over(layout.bounds()) else {
                    return;
                };
                shell.publish((self.on_event)(ContextMenuEvent::Invoked(
                    InvocationKind::Pointer(CorePoint::new(position.x, position.y)),
                )));
                shell.capture_event();
            }
            Event::Mouse(mouse::Event::ButtonPressed(_)) if presenting => {
                let Some(position) = cursor.position() else {
                    return;
                };
                let outcome = dismissal::on_press(
                    context_menu_dismiss_policy(),
                    &dismiss_regions(None),
                    position,
                );
                if outcome.closes() {
                    shell.publish((self.on_event)(ContextMenuEvent::Dismissed(
                        CloseCause::OutsideClick,
                    )));
                }
                if outcome.consumes() {
                    shell.capture_event();
                }
            }

            Event::Touch(iced::touch::Event::FingerPressed { position, .. }) if presenting => {
                let outcome = dismissal::on_press(
                    context_menu_dismiss_policy(),
                    &dismiss_regions(None),
                    *position,
                );
                if outcome.closes() {
                    shell.publish((self.on_event)(ContextMenuEvent::Dismissed(
                        CloseCause::OutsideClick,
                    )));
                }
                if outcome.consumes() {
                    shell.capture_event();
                }
            }

            Event::Keyboard(keyboard_event) => {
                if presenting {
                    match event_for_open_key(self.model, self.state, keyboard_event) {
                        Some(MenuEvent::Claimed(decoded)) => {
                            shell.publish((self.on_event)(decoded));
                            shell.capture_event();
                        }
                        Some(MenuEvent::Swallowed) => shell.capture_event(),
                        None => {
                            if is_focus_traversal(keyboard_event) {
                                shell.capture_event();
                            }
                        }
                    }
                } else if opens_context_menu(keyboard_event) {
                    // Query focus now; pointer focus changes need not run an operation pass.
                    if !self.model.has_focusable()
                        || !self.target_holds_focus(tree, layout, renderer)
                    {
                        return;
                    }
                    shell.publish((self.on_event)(ContextMenuEvent::Invoked(
                        InvocationKind::Keyboard(bounds_to_viewport(layout.bounds(), Vector::ZERO)),
                    )));
                    shell.capture_event();
                }
            }

            _ => {}
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
        if self.is_available()
            && let Some(id) = self.advertised_id()
        {
            let semantics =
                enzui_core::semantics::Semantics::new(enzui_core::semantics::Role::Group)
                    .label_opt(self.label.clone())
                    .context_menu();
            let mut node = enzui_core::collect::SemanticsNode::new(semantics);
            operation.custom(Some(&id), layout.bounds(), &mut node);

            let mut request = crate::context_menu_action::ContextMenuRequest::new(id.clone());
            operation.custom(Some(&id), layout.bounds(), &mut request);
            if request.armed() {
                tree.state.downcast_mut::<State>().context_menu_pending = true;
            }
        }
        // Menu surfaces are operated by their overlay to avoid duplicate semantics.
        self.target.as_widget_mut().operate(
            &mut tree.children[0],
            layout,
            viewport,
            renderer,
            operation,
        );
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        let stale_host = self.state.is_open() && !self.is_available();
        stale_host
            || tree.state.downcast_ref::<State>().reveal.is_revealing()
            || ((!self.is_presenting() || self.live_target)
                && crate::frame_routing::interest_opaque(tree))
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
        let presenting = self.is_presenting();
        tree.state
            .downcast_mut::<State>()
            .reveal
            .track(event, presenting, shell);
        if self.state.is_open() && !self.is_available() {
            shell.publish((self.on_event)(ContextMenuEvent::Dismissed(
                CloseCause::Host,
            )));
            shell.request_redraw();
        }
        if !presenting || self.live_target {
            crate::frame_routing::forward_frame_opaque(
                &mut self.target,
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
        self.target.as_widget().draw(
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
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.target.as_widget().mouse_interaction(
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
        // Both invocation kinds carry host-space geometry until overlay translation is available.
        let invocation = self
            .is_presenting()
            .then(|| self.state.invocation())
            .flatten();
        let edge_margin = self.edge_margin;
        let reveal = tree.state.downcast_ref::<State>().reveal;

        let surfaces = self.surfaces();
        let anchors = self.submenu_anchors();

        let (target_tree, surface_trees) = {
            let (first, rest) = tree.children.split_at_mut(1);
            (&mut first[0], rest)
        };
        let target_overlay = self.target.as_widget_mut().overlay(
            target_tree,
            layout,
            renderer,
            viewport,
            translation,
            window,
        );
        // Submenu placement needs its already-placed parent, so all levels share one overlay.
        let menu_overlay = match invocation {
            Some(invocation) if !surfaces.is_empty() => {
                vec![crate::overlay_adapter::element(
                    MenuOverlay {
                        anchors,
                        surfaces,
                        trees: surface_trees,
                        invocation: translate_invocation(invocation, translation),
                        edge_margin,
                        interactive_header: self.header.is_some(),
                        reveal,
                    },
                    renderer,
                    window,
                )]
            }
            _ => Vec::new(),
        };

        {
            let mut overlays = target_overlay;
            overlays.extend(menu_overlay);
            overlays
        }
    }
}
