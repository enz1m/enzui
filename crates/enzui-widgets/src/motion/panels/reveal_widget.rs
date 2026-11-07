//! `PanelReveal`'s `Widget` implementation.

use super::*;

impl<Message> Widget<Message, iced::Theme, iced::Renderer> for PanelReveal<'_, Message> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<PanelRevealLayout>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(PanelRevealLayout {
            input_was_enabled: std::cell::Cell::new(self.input_enabled()),
            ..PanelRevealLayout::default()
        })
    }

    fn size(&self) -> Size<Length> {
        let child = self.child.as_widget().size();

        if self.enters_from.is_vertical() {
            Size::new(child.width, Length::Shrink)
        } else {
            Size::new(Length::Shrink, child.height)
        }
    }

    fn diff(&mut self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_mut(&mut self.child));
        self.note_input_state(tree);
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let vertical = self.enters_from.is_vertical();
        let max = limits.max();
        let resting = self
            .resting_extent
            .unwrap_or(if vertical { max.height } else { max.width })
            .max(0.0);
        // A responsive Reserve hands its siblings the live reserved width, so
        // the content beside it reflows every frame; the panel itself keeps
        // its resting width and slides into view behind the seam, clipped to
        // the reservation in `draw`. Its subtree is therefore measured at one
        // width for the whole travel and replays from the cache below, instead
        // of being re-laid out (rows squeezing and ellipsizing) per frame.
        let child_extent = resting;
        let responsive_rule =
            (!vertical && self.mode == PanelRevealMode::Reserve && self.responsive_reserve)
                .then_some(layout::responsive::Rule::Reserve {
                    resting,
                    progress: self.reserve_fraction(tree.state.downcast_ref::<PanelRevealLayout>()),
                    // A panel docked at the left slides in with its trailing
                    // edge on the seam; one docked at the right with its
                    // leading edge on it.
                    align_end: self.enters_from == PanelRevealEdge::Leading,
                });
        let child_limits = responsive_rule.map_or_else(
            || {
                if vertical {
                    limits.height(Length::Fixed(child_extent))
                } else {
                    limits.width(Length::Fixed(child_extent))
                }
            },
            |rule| rule.child_limits(limits),
        );
        let child_max = child_limits.max();
        let progress = self.progress();
        let travelling = progress > f32::EPSILON && progress < 1.0 - f32::EPSILON;
        if self.travelling_reserve() && !self.responsive_reserve {
            // A responsive reservation is exempt: widest-pinning is the
            // endpoint geometry its owner opted out of — siblings would sit at
            // destination width for the whole travel. Layout is the first
            // authority that can commit sibling geometry: latch the widest
            // reservation before computing this frame's node (update_frame
            // would return one progress-sized layout, pinning only next
            // frame). The test is `travelling_reserve`, not this pass's
            // `travelling` sample: layout reads the PREVIOUS frame's progress,
            // so travel-start samples still read the endpoint, and an unpinned
            // clip paints settled width for a whole frame.
            let state = tree.state.downcast_ref::<PanelRevealLayout>();
            if state.reserve_pin.get().is_none() {
                state.reserve_pin.set(Some(1.0));
                state.reserve_first_commit_owed.set(self.growing());
                state.reserve_commit_owed.set(true);
            }
            self.publish_pin(state.reserve_pin.get());
        }
        // Frame interest is not layout change; a child whose layout changes
        // invalidates it, which `note_child_invalidation` records.
        let replay = (self.cache_layout_while_revealing
            && travelling
            && self.child_laid_out
            && !tree
                .state
                .downcast_ref::<PanelRevealLayout>()
                .child_invalidated_layout
                .get())
        .then(|| {
            tree.state
                .downcast_ref::<PanelRevealLayout>()
                .cached
                .as_ref()
                .filter(|(at, _)| *at == child_max)
                .map(|(_, node)| node.clone())
        })
        .flatten();
        let child = match replay {
            Some(node) => {
                crate::app_shell::build_probe::note_panel_layout_replay();
                node
            }
            None => {
                crate::app_shell::build_probe::note_panel_child_layout();
                let node = self.child.as_widget_mut().measure(
                    &mut tree.children[0],
                    renderer,
                    &child_limits,
                );
                self.child_laid_out = true;
                let state = tree.state.downcast_mut::<PanelRevealLayout>();
                state.child_invalidated_layout.set(false);
                state.cached = (self.cache_layout_while_revealing && travelling)
                    .then(|| (child_max, node.clone()));
                node
            }
        };
        // a travelling `Reserve` reserves its travel's WIDEST extent,
        // not its current one. Siblings therefore move exactly once -- when the
        // pin latches -- instead of on every frame, and the reveal inside the
        // held rectangle is a clip the compositor can animate. Settled, this is
        // the live progress and the geometry is exactly what it always was.
        let reserved = match self.mode {
            PanelRevealMode::Reserve => {
                resting * self.reserve_fraction(tree.state.downcast_ref::<PanelRevealLayout>())
            }
            PanelRevealMode::Overlay => resting,
        }
        .min(if vertical { max.height } else { max.width });
        let child_size = child.size();
        let size = if vertical {
            Size::new(child_size.width.min(max.width), reserved)
        } else {
            Size::new(reserved, child_size.height.min(max.height))
        };
        // The capture-hold observation channel describes the docked sidebar's
        // horizontal extent, so a vertical panel publishes no geometry into it
        // rather than reporting a height as a width.
        if vertical {
            crate::app_shell::capture_hold::note_unpublished_reveal();
        } else if self.mode == PanelRevealMode::Reserve && self.responsive_reserve {
            crate::app_shell::capture_hold::note_reserve_layout(progress, size.width, resting);
        } else if self.mode == PanelRevealMode::Overlay {
            crate::app_shell::capture_hold::note_overlay_layout(progress, resting);
        } else {
            crate::app_shell::capture_hold::note_unpublished_reveal();
        }
        // Overlay translation is draw-time so it does not invalidate the
        // unchanged base/content layout every frame.
        // A responsive reservation narrower than its panel is travelling: its
        // draw records the whole panel under its node, which the seam group
        // then publishes unclipped. Named here, before the frame's update.
        if let Some(retained) = self.live_retained.as_ref() {
            let travelling = responsive_rule.is_some() && size.width < resting - 0.5;
            retained.set(travelling.then(|| tree.state.downcast_ref::<PanelRevealLayout>().node));
        }
        let node = if let Some(rule) = responsive_rule {
            rule.finish(limits, child)
                .with_responsive_presentation(
                    *limits,
                    rule,
                    self.child.as_widget().size(),
                    self.layout_driver,
                )
                .with_drawing_cull(false)
        } else {
            layout::Node::with_children(size, vec![child])
        };
        node.apply(tree);
    }

    fn frame_interest(&self, tree: &Tree) -> bool {
        // A MOUNTED `Overlay` reveal stays on the frame route, travelling or
        // not. "Already travelling" is unroutable in production: frame interest
        // reads from a CACHED tree flag recomputed only where the route reaches
        // or during `Tree::diff`. A shell drawer rebuilds exactly once — on the
        // travel-starting message, still settled — so the flag caches `false`,
        // the route never re-enters, and the flip-the-flag track never
        // publishes. Measured live: the shipped shell drawer animated 29 frames
        // with zero tracks/animations — no render lane. Offscreen tests missed
        // it (the harness re-runs `view` per frame, re-diffing the flag — what
        // a real app does not do mid-transition; retained state + targeted
        // routing forbid it). Mounted is honest: the shell mounts `Overlay`
        // only while open/travelling, and a settled shell requests no frames —
        // one extra `update_frame` on others' frames. The subtree still prunes
        // via `forward_frame_opaque`. A MOUNTED reveal stays routed in either
        // mode. `Reserve` falling through to `visible()` is unroutable the same
        // way — worse: a returning docked sidebar starts at progress 0, so
        // `visible()` is false at the flag-caching rebuild; the route never
        // enters, the track never publishes, the reveal sticks to the UI thread
        // for the whole travel (measured: Zero tracks/animations over nine
        // travelling frames). Mounted is honest: mounted only while open/
        // travelling, settled shells request nothing — one extra `update_frame`
        // on others' frames; the child still prunes via `forward_frame_opaque`.
        // The condition is `progress < 1`, not unconditional `true`:
        // unconditional interest walks the whole content subtree per moving
        // frame, defeating targeted routing (measured: 900/900 paragraphs
        // receiving uninterested frame events). Settled-open docks at exactly
        // 1.0 prune as before; settled-closed ones are unmounted.
        self.mode == PanelRevealMode::Overlay
            || (self.mode == PanelRevealMode::Reserve
                && !self.responsive_reserve
                && (self.progress() < 1.0 - f32::EPSILON
                    || self
                        .live_travel
                        .as_ref()
                        .is_some_and(|travel| travel.borrow().is_some())))
            || (self.visible() && crate::frame_routing::interest_opaque(tree))
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
        self.note_input_state(tree);
        if self.mode == PanelRevealMode::Reserve && !self.responsive_reserve {
            let state = tree.state.downcast_ref::<PanelRevealLayout>();
            let travelling = self.travelling_reserve();

            if travelling {
                if state.reserve_pin.get().is_none() {
                    state.reserve_pin.set(Some(1.0));
                    state.reserve_first_commit_owed.set(self.growing());
                    state.reserve_commit_owed.set(true);
                }

                self.publish_pin(state.reserve_pin.get());

                // `layout` is allowed to latch the pin first. In that ordering
                // the old `if pin.is_none()` test silently lost the opening
                // commit and left the retained recording at zero width. Spend
                // the debt independently of which phase created it.
                if state.reserve_first_commit_owed.replace(false) {
                    shell.invalidate_layout();
                }
            } else if state.reserve_pin.get().is_some() {
                // Settled. Release the pin so layout reports the true end
                // geometry, and spend the final commit exactly once.
                state.reserve_pin.set(None);
                state.recorded_reserve.set(None);
                state.reserve_first_commit_owed.set(false);
                self.publish_pin(None);

                if state.reserve_commit_owed.replace(false) {
                    shell.invalidate_layout();
                }
            }

            if travelling
                && state.reserve_pin.get().is_some()
                && let Some(pinned) = state.recorded_reserve.get()
                && pinned > f32::EPSILON
            {
                let bounds = layout.bounds();
                let progress = self.progress();
                let edge = self.enters_from;

                // `PanelReveal` samples a caller-owned clock; it never owns
                // cadence. The public sample-only form has no future curve and
                // must not turn one supplied sample into a free-running redraw
                // loop. The travel-owning `PanelMotion` (or equivalent) owner
                // requests the next frame — retained seam chrome riding the
                // travel publishes from its own retained node, no second
                // cadence. This wrapper publishes only the sampled track (plus
                // the matching curve when supplied) for the frame in progress:
                // a second requester doubles the scene gate's redraw-requester
                // count, breaking its exactly-one-owner proof.
                shell.publish_track(iced::advanced::renderer::Track {
                    node: state.node,
                    translation: Vector::ZERO,
                    opacity: 1.0,
                    scale: 1.0,
                    scale_origin: iced::Point::ORIGIN,
                    rotation: 0.0,
                    clip: Some(Self::reserve_clip(bounds, progress, pinned, edge)),
                });

                // ...and the curve, which is what actually reaches the render
                // lane: a sampled track alone leaves `live_animation` false and
                // the surface is never handed over.
                //
                // Clamped to the recorded extent, because a clip may only
                // narrow the recording -- never ask for ink it does not hold.
                if let Some(travel) = self.live_travel.as_ref() {
                    let curve = travel.borrow();

                    if let Some(curve) = curve.as_ref() {
                        publish_travel(shell, state.node, curve, move |sampled| {
                            iced::advanced::renderer::Track {
                                node: state.node,
                                translation: Vector::ZERO,
                                opacity: 1.0,
                                scale: 1.0,
                                scale_origin: iced::Point::ORIGIN,
                                rotation: 0.0,
                                clip: Some(Self::reserve_clip(
                                    bounds,
                                    sampled.min(pinned),
                                    pinned,
                                    edge,
                                )),
                            }
                        });
                    }
                }
            }
        }

        if self.travelling_overlay() {
            let state = tree.state.downcast_ref::<PanelRevealLayout>();

            // Opening exposes ink that a clipped recording may not contain.
            if !self.growing() && state.recorded_offset.get().is_some() {
                let bounds = layout.bounds();
                let edge = self.enters_from;
                let held = self.paint_bounds(bounds);

                // Replay uses the stamped offset; the enclosing viewport stays fixed.
                shell.publish_track(iced::advanced::renderer::Track {
                    node: state.node,
                    translation: Self::overlay_offset(edge, bounds, self.spatial_progress()),
                    opacity: 1.0,
                    scale: 1.0,
                    scale_origin: iced::Point::ORIGIN,
                    rotation: 0.0,
                    clip: Some(held),
                });

                // ...and the curve, which is what reaches the lane: a sampled
                // track alone never hands the surface over, and every frame
                // would wait for the UI thread.
                if let Some(travel) = self.live_travel.as_ref() {
                    let curve = travel.borrow();

                    if let Some(curve) = curve.as_ref() {
                        publish_travel_channels(shell, state.node, curve, |_, spatial| {
                            iced::advanced::renderer::Track {
                                node: state.node,
                                translation: Self::overlay_offset(edge, bounds, spatial),
                                opacity: 1.0,
                                scale: 1.0,
                                scale_origin: iced::Point::ORIGIN,
                                rotation: 0.0,
                                clip: Some(held),
                            }
                        });
                    }
                }
            }
        }
        if !self.visible()
            || (self.mode == PanelRevealMode::Overlay && self.progress() < 1.0 - f32::EPSILON)
        {
            return;
        }
        let shown = self.shown_bounds(tree, layout);

        crate::frame_routing::forward_frame_opaque(
            &mut self.child,
            tree,
            0,
            event,
            Self::child_layout(layout, tree),
            crate::inert_clip::cursor_within(cursor, shown),
            renderer,
            shell,
            viewport,
        );
        Self::note_child_invalidation(tree, shell);
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
        self.note_input_state(tree);
        if !matches!(
            event,
            Event::Window(iced::window::Event::RedrawRequested(_))
        ) {
            self.child_laid_out = false;
            tree.state.downcast_mut::<PanelRevealLayout>().cached = None;
        }
        if !self.input_enabled()
            || (self.mode == PanelRevealMode::Overlay && self.progress() < 1.0 - f32::EPSILON)
        {
            return;
        }
        let shown = self.shown_bounds(tree, layout);
        let cursor = crate::inert_clip::cursor_within(cursor, shown);
        let child_layout = Self::child_layout(layout, tree);
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            cursor,
            renderer,
            shell,
            viewport,
        );
        crate::frame_routing::refresh_opaque_child(&self.child, tree, 0);
        Self::note_child_invalidation(tree, shell);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if !self.input_enabled()
            || (self.mode == PanelRevealMode::Overlay && self.progress() < 1.0 - f32::EPSILON)
        {
            return mouse::Interaction::default();
        }
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            Self::child_layout(layout, tree),
            crate::inert_clip::cursor_within(cursor, self.shown_bounds(tree, layout)),
            viewport,
            renderer,
        )
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
        let state = tree.state.downcast_ref::<PanelRevealLayout>();

        if !(self.visible() || self.mode == PanelRevealMode::Reserve && self.responsive_reserve) {
            state.recorded_offset.set(None);
            return;
        }
        let bounds = layout.bounds();
        let cursor = crate::inert_clip::cursor_within(cursor, self.shown_bounds(tree, layout));
        if self.mode == PanelRevealMode::Reserve && self.responsive_reserve {
            state.recorded_offset.set(None);
            state.recorded_reserve.set(None);
            // The child retains its resting extent while only its reservation is visible.
            let child_bounds = Self::child_layout(layout, tree).bounds();
            let travelling = if self.enters_from.is_vertical() {
                child_bounds.height > bounds.height + 0.5
            } else {
                child_bounds.width > bounds.width + 0.5
            };
            // The whole panel is painted for the recording, including the part
            // still outside the window.
            let painted = if travelling {
                viewport.union(&child_bounds)
            } else {
                *viewport
            };
            let paint = |renderer: &mut iced::Renderer| {
                // Travelling, the panel is painted whole even while its
                // reservation is still empty, which no viewport cull allows.
                if travelling {
                    self.child.as_widget().draw(
                        &tree.children[0],
                        renderer,
                        theme,
                        style,
                        Self::child_layout(layout, tree),
                        cursor,
                        &painted,
                    );
                    return;
                }
                layout.draw_culled(tree, renderer, &painted, |renderer, viewport| {
                    self.child.as_widget().draw(
                        &tree.children[0],
                        renderer,
                        theme,
                        style,
                        Self::child_layout(layout, tree),
                        cursor,
                        viewport,
                    );
                });
            };
            if travelling {
                use iced::advanced::Renderer as _;
                // The live frame is clipped to the reservation; the recording
                // keeps the whole panel under its own node, for the render lane
                // to show it where a late layout's seam has since moved.
                let visible = self.paint_bounds(bounds);
                renderer.mark_recording(state.node);
                renderer.with_layer_retained(visible, visible.union(&child_bounds), paint);
                renderer.mark_recording(iced::advanced::renderer::NodeId::STATIC);
            } else {
                paint(renderer);
            }
            return;
        }
        // The overlay's offset reads the spatial value; the reservation's
        // clip below stays on the bounded one.
        let progress = self.progress();
        let spatial = self.spatial_progress();
        let overlay = self.mode == PanelRevealMode::Overlay;
        // the sign is the declared travel edge, not a hardcoded "from
        // the left". `Leading` yields `-width * (1 - p)`.
        let offset = if overlay {
            super::snap_to_device(
                Self::overlay_offset(self.enters_from, bounds, spatial),
                renderer,
            )
        } else {
            Vector::ZERO
        };
        let node = state.node;
        // the reveal is a CLIP inside the held reservation. The child is
        // painted at the pinned extent -- the widest the travel reaches -- so
        // the recording always contains at least as much ink as any clip the
        // lane will later ask for. Narrowing a recording is exactly what a
        // clip track does; widening one is impossible, which is why the
        // reservation is pinned wide rather than tracking progress.
        let pinned = self.reserve_fraction(state);
        let reserve_clip =
            (!overlay).then(|| Self::reserve_clip(bounds, progress, pinned, self.enters_from));

        use iced::advanced::Renderer as _;
        let held = if overlay {
            self.paint_bounds(bounds)
        } else {
            bounds
        };
        renderer.with_layer_retained(reserve_clip.unwrap_or(held), held, |renderer| {
            renderer.with_motion(node, offset, |renderer| {
                self.child.as_widget().draw(
                    &tree.children[0],
                    renderer,
                    theme,
                    style,
                    Self::child_layout(layout, tree),
                    cursor,
                    viewport,
                );
            });
        });

        // What a later animation-only frame may measure against. An `Overlay`
        // reveal records where it was painted; a `Reserve` one records the
        // reservation it was painted inside, because a clip track may never ask
        // for a wider rectangle than the recording holds.
        state.recorded_offset.set(overlay.then_some(offset));
        state
            .recorded_reserve
            .set((!overlay && state.reserve_pin.get().is_some()).then_some(pinned));
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        if !self.input_enabled()
            || (self.mode == PanelRevealMode::Overlay && self.progress() < 1.0 - f32::EPSILON)
        {
            return;
        }
        if self.progress() >= 1.0 - f32::EPSILON {
            let child_layout_2 = Self::child_layout(layout, tree);
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                child_layout_2,
                viewport,
                renderer,
                operation,
            );
        } else {
            let shown = self.shown_bounds(tree, layout);
            let mut clipped = crate::inert_clip::ClipOperation::new(operation, shown);
            let child_layout = Self::child_layout(layout, tree);
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                child_layout,
                viewport,
                renderer,
                &mut clipped,
            );
        }
    }

    fn overlay<'a>(
        &'a mut self,
        tree: &'a mut Tree,
        layout: Layout,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
        window: Size,
    ) -> Vec<overlay::Element<'a, Message, iced::Theme, iced::Renderer>> {
        // Overlays are rooted outside this widget's renderer layer, so the
        // reveal rectangle cannot constrain them. Suppress them until fully
        // visible instead of leaving a tooltip/menu at the child's resting
        // position while Reserve mode has already reclaimed that space.
        if !self.input_enabled() || self.progress() < 1.0 - f32::EPSILON {
            return Vec::new();
        }
        let child_layout = Self::child_layout(layout, tree);
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
