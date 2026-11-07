// motion split production module: generic effects

use super::*;
use iced::advanced::layout::Measure as _;

/// The edge a [`MotionEffect::Slide`] enters from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlideDirection {
    /// Enter from the leading/left edge.
    Left,
    /// Enter from the trailing/right edge.
    Right,
    /// Enter from the top edge.
    Up,
    /// Enter from the bottom edge.
    Down,
}

/// The dimensions changed by a [`MotionEffect::Size`] transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionAxis {
    /// Animate only the reserved width.
    Horizontal,
    /// Animate only the reserved height.
    Vertical,
    /// Animate both reserved dimensions.
    Both,
}

/// A named, allocation-free visual effect for any retained element.
///
/// Effects compose by nesting [`Motion`] wrappers. [`Slide`](Self::Slide),
/// [`Scale`](Self::Scale), and [`Fade`](Self::Fade) are paint-only and keep
/// layout cached; [`Size`](Self::Size) changes layout once, at settlement, or
/// on every moving frame with [`Motion::reflow`]. Fade is a renderer-composited subtree effect, not a wrapper
/// colour adjustment. See the [module documentation](crate::motion).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionEffect {
    /// Translate without changing reserved layout, entering from this edge.
    Slide(SlideDirection),
    /// Animate reserved width, height, or both.
    ///
    /// The element reveals inside the wider of its two endpoints for the
    /// whole travel and its layout changes once, at settlement, so the
    /// travel replays without relayout. Content after it therefore holds its
    /// place while the element folds and moves at the end; with
    /// [`Motion::reflow`] it follows the element on every frame instead. A collapsing
    /// section whose following content must follow it frame by frame (a
    /// sidebar group above the next group) is an
    /// [`enz_expander`](crate::expander::enz_expander), whose body height
    /// reflows on every frame.
    Size(MotionAxis),
    /// Scale around the element centre with a subtle `0.92 → 1` reveal,
    /// fading in with it.
    Scale,
    /// Composite the complete descendant subtree from transparent to opaque.
    Fade,
}

impl MotionEffect {
    /// Whether advancing this effect changes the wrapper's layout node.
    #[must_use]
    pub const fn needs_layout(self) -> bool {
        matches!(self, Self::Size(_))
    }
}

/// Apply a theme-driven named motion effect to any element.
///
/// `visible` is the target state. The child is retained while moving, but once
/// fully hidden its widget tree is removed and all lifecycle delegation stops.
/// While `visible` is true the child is reachable by focus operations and the
/// accessibility collectors at every progress, including zero, so opening a
/// band and focusing into it in the same frame works; the pointer alone waits
/// for geometry it can honestly hit. A `visible == false` wrapper is inert at
/// every progress, however long its exit is still painted.
/// Theme duration, easing, and reduced-motion policy are read through
/// [`PanelMotion`]. Reduced motion jumps directly to the target. An unobserved
/// policy uses the default duration for every effect, so a settled-hidden
/// wrapper (zero-sized, so never drawn) can animate its first reveal; its
/// first drawn frame then latches the real theme. Offscreen wrappers request
/// no animation frames.
pub struct Motion<'a, Message> {
    child: Element<'a, Message>,
    visible: bool,
    effect: MotionEffect,
    on_hidden: Option<std::rc::Rc<dyn Fn() -> Message + 'a>>,
    // Read when the wrapper's state is created: whether it reveals the child
    // rather than starting settled.
    enter_on_mount: Option<std::rc::Rc<Cell<bool>>>,
    expressive: Option<enzui_theme::ExpressiveMotion>,
    spring: Option<enzui_theme::Spring>,
    reflow: bool,
}

/// Publishes the remaining travel as a renderer-owned animation.
///
/// `value` maps an eased progress sample to the track that progress paints, so
/// each effect states its own geometry once and the sampling, the anchoring and
/// the generation bookkeeping are shared by every effect family that reaches the
/// lane -- `Motion`'s `Slide`, `Fade` and `Size` and `CrossFade`'s alpha split alike.
fn publish_curve<Message>(
    shell: &mut Shell<'_, Message>,
    state: &PanelMotion,
    value: impl Fn(f32) -> iced::advanced::renderer::Track,
) {
    publish_curve_channels(shell, state, |bounded, _spatial| value(bounded));
}

/// [`publish_curve`] for a track that reads both channels: each keyframe gets
/// the bounded progress and the spatial one at the same instant, so a slide's
/// translation can overshoot while its clip stays bounded.
fn publish_curve_channels<Message>(
    shell: &mut Shell<'_, Message>,
    state: &PanelMotion,
    value: impl Fn(f32, f32) -> iced::advanced::renderer::Track,
) {
    let Some(curve) = state.track_curve(TRACK_SAMPLES) else {
        return;
    };
    let spatial = |index: usize, bounded: f32| {
        curve
            .spatial_points
            .as_ref()
            .and_then(|points| points.get(index))
            .map_or(bounded, |(_, spatial)| *spatial)
    };

    shell.publish_track_animation(iced::advanced::renderer::TrackAnimation {
        node: value(curve.points[0].1, spatial(0, curve.points[0].1)).node,
        start: curve.start,
        duration: curve.duration,
        keyframes: curve
            .points
            .iter()
            .enumerate()
            .map(|(index, (at, progress))| (*at, value(*progress, spatial(index, *progress))))
            .collect(),
        generation: curve.generation,
    });
}

impl<'a, Message: 'a> Motion<'a, Message> {
    /// The visible sub-rectangle of a pinned `Size` reservation.
    ///
    /// `bounds` is the HELD reservation (`full * pinned`), so the revealed
    /// fraction of it is `progress / pinned`. This is the single definition of
    /// the reveal geometry: `draw` clips to it so the immediate path paints the
    /// right thing, and the published clip track carries the identical
    /// rectangle so a composited replay is the same picture rather than an
    /// approximation of it.
    fn size_clip(axis: MotionAxis, bounds: Rectangle, progress: f32, pinned: f32) -> Rectangle {
        if pinned <= f32::EPSILON {
            return bounds;
        }
        let fraction = (progress / pinned).clamp(0.0, 1.0);
        match axis {
            MotionAxis::Horizontal => Rectangle {
                width: bounds.width * fraction,
                ..bounds
            },
            MotionAxis::Vertical => Rectangle {
                height: bounds.height * fraction,
                ..bounds
            },
            MotionAxis::Both => Rectangle {
                width: bounds.width * fraction,
                height: bounds.height * fraction,
                ..bounds
            },
        }
    }

    /// The progress a slide's clip is chosen from: a slide still moving past
    /// rest, as a spring overshoots, is cut like a moving one even though its
    /// bounded progress has arrived.
    fn clip_progress(effect: MotionEffect, progress: f32, spatial: f32) -> f32 {
        match effect {
            MotionEffect::Slide(_) if (spatial - 1.0).abs() > 1e-4 => progress.min(0.0),
            _ => progress,
        }
    }

    /// The rectangle this wrapper's layer clips its child to.
    ///
    /// It is the bounds grown by `margin`, the theme's largest shadow, so an
    /// elevated child's shadow is not cut at the wrapper's edge and a settled
    /// wrapper paints its child as it would be painted without it. Two edges
    /// keep the cut because the cut is the effect: a `Slide` still in motion
    /// keeps the bounds along its travel, and a `Size` reveal not yet fully
    /// open keeps its moving edge at [`Self::size_clip`].
    fn ink_clip(
        effect: MotionEffect,
        bounds: Rectangle,
        margin: iced::Padding,
        progress: f32,
        pinned: f32,
    ) -> Rectangle {
        let settled = progress >= 1.0 - f32::EPSILON;
        match effect {
            MotionEffect::Fade | MotionEffect::Scale => bounds.expand(margin),
            MotionEffect::Slide(_) if settled => bounds.expand(margin),
            MotionEffect::Slide(SlideDirection::Left | SlideDirection::Right) => {
                bounds.expand(iced::Padding {
                    left: 0.0,
                    right: 0.0,
                    ..margin
                })
            }
            MotionEffect::Slide(SlideDirection::Up | SlideDirection::Down) => {
                bounds.expand(iced::Padding {
                    top: 0.0,
                    bottom: 0.0,
                    ..margin
                })
            }
            MotionEffect::Size(axis) => {
                let open = pinned <= f32::EPSILON || progress >= pinned - f32::EPSILON;
                let keep = |side: f32| if open { side } else { 0.0 };
                let margin = match axis {
                    MotionAxis::Horizontal => iced::Padding {
                        right: keep(margin.right),
                        ..margin
                    },
                    MotionAxis::Vertical => iced::Padding {
                        bottom: keep(margin.bottom),
                        ..margin
                    },
                    MotionAxis::Both => iced::Padding {
                        right: keep(margin.right),
                        bottom: keep(margin.bottom),
                        ..margin
                    },
                };
                Self::size_clip(axis, bounds, progress, pinned).expand(margin)
            }
        }
    }

    /// The rectangle this wrapper actually SHOWS, for hit testing and
    /// inertness.
    ///
    /// Before the migration a `Size` wrapper reserved exactly the rectangle it
    /// painted, so `layout.bounds()` was both. Pinning breaks that: during a
    /// collapse the reservation is held at the travel's widest extent while the
    /// ink shrinks inside it, so `layout.bounds()` would keep a control
    /// clickable, hoverable and cursor-shaping across a strip that is no longer
    /// painted. Every pointer path resolves through here instead, so what can
    /// be hit is what can be seen.
    fn shown_bounds(&self, tree: &Tree, layout: Layout) -> Rectangle {
        let bounds = layout.bounds();
        let MotionEffect::Size(axis) = self.effect else {
            return bounds;
        };
        let state = tree.state.downcast_ref::<PanelMotion>();
        Self::size_clip(
            axis,
            bounds,
            tree.presented_reveal_progress()
                .unwrap_or_else(|| state.sampled_progress()),
            state.reservation_progress(),
        )
    }

    /// The travel offset a `Slide` paints at, for `progress`.
    ///
    /// Shared by `draw` (which records at it) and `update_frame` (which
    /// publishes a track relative to it) so the two can never drift apart.
    fn slide_offset(direction: SlideDirection, bounds: Rectangle, progress: f32) -> Vector {
        match direction {
            SlideDirection::Left => Vector::new(-bounds.width * (1.0 - progress), 0.0),
            SlideDirection::Right => Vector::new(bounds.width * (1.0 - progress), 0.0),
            SlideDirection::Up => Vector::new(0.0, -bounds.height * (1.0 - progress)),
            SlideDirection::Down => Vector::new(0.0, bounds.height * (1.0 - progress)),
        }
    }

    /// Wrap `child` with `effect`, targeting `visible`.
    #[must_use]
    pub fn new(
        child: impl Into<Element<'a, Message>>,
        visible: bool,
        effect: MotionEffect,
    ) -> Self {
        Self {
            child: crate::viewport_gate::ViewportGate::new(child).into(),
            visible,
            effect,
            on_hidden: None,
            enter_on_mount: None,
            expressive: None,
            spring: None,
            reflow: false,
        }
    }

    /// For [`MotionEffect::Size`]: lay the reservation out at the eased
    /// progress on every frame, so content beside or after the element moves
    /// with it (an inspector beside a list, a panel that pushes its
    /// neighbour), at the cost of a layout and a drawn frame per frame.
    ///
    /// Without it the reservation holds the wider endpoint for the whole
    /// travel and changes once, at settlement: cheaper, and the travel can
    /// play on the render lane, but its neighbours jump by the whole extent
    /// at the start of an opening or the end of a closing.
    #[must_use]
    pub fn reflow(mut self, reflow: bool) -> Self {
        self.reflow = reflow;
        self
    }

    /// Travel on an expressive token instead of the theme's easing, or
    /// `None` (the default) for the ordinary curve.
    ///
    /// A spatial token lets a [`MotionEffect::Slide`] pass its resting place
    /// and settle back; the slide's clip, a [`MotionEffect::Fade`]'s opacity,
    /// a [`MotionEffect::Scale`] and a [`MotionEffect::Size`] reservation read
    /// the bounded value and never overshoot. Reduced motion still snaps. See
    /// [`PanelMotion::set_expressive`].
    #[must_use]
    pub fn expressive(mut self, motion: Option<enzui_theme::ExpressiveMotion>) -> Self {
        self.expressive = motion;
        if motion.is_some() {
            self.spring = None;
        }
        self
    }

    /// Travel on a physical spring instead of the theme's easing, or `None`
    /// (the default) for the ordinary curve. Replaces an expressive token.
    ///
    /// A reversal mid-travel keeps the travel's velocity, so the child turns
    /// around smoothly. An underdamped spring lets a [`MotionEffect::Slide`]
    /// pass its resting place and settle back; every other effect reads the
    /// bounded value. See [`PanelMotion::set_spring`].
    #[must_use]
    pub fn spring(mut self, spring: Option<enzui_theme::Spring>) -> Self {
        self.spring = spring;
        if spring.is_some() {
            self.expressive = None;
        }
        self
    }

    /// Apply this wrapper's chosen travel to `state`.
    fn apply_travel(&self, state: &mut PanelMotion) {
        if self.spring.is_some() {
            state.set_spring(self.spring);
        } else {
            state.set_spring(None);
            state.set_expressive(self.expressive);
        }
    }

    /// Reveal the child when the wrapper first mounts visible, instead of
    /// starting settled.
    ///
    /// For a surface that exists only while shown, such as a dialog layer a
    /// host appends on open: its wrapper is new on the opening frame, so
    /// without this it has no hidden state to travel from. Reduced motion
    /// still shows it at once.
    #[must_use]
    pub fn enter_on_mount(self) -> Self {
        self.enter_on_mount_if(std::rc::Rc::new(Cell::new(true)))
    }

    /// [`Self::enter_on_mount`] decided when the wrapper mounts, by an owner
    /// that sets `entering` while diffing above it.
    #[must_use]
    pub(crate) fn enter_on_mount_if(mut self, entering: std::rc::Rc<Cell<bool>>) -> Self {
        self.enter_on_mount = Some(entering);
        self
    }

    /// Publish once after a visible-to-hidden transition has settled.
    ///
    /// The callback is lazy and is not called for a wrapper constructed
    /// already hidden. Reduced motion settles the edge on its first event.
    #[must_use]
    pub fn on_hidden_with(mut self, on_hidden: impl Fn() -> Message + 'a) -> Self {
        self.on_hidden = Some(std::rc::Rc::new(on_hidden));
        self
    }

    fn progress(tree: &Tree) -> f32 {
        tree.state.downcast_ref::<PanelMotion>().sampled_progress()
    }

    fn child_layout(layout: Layout, tree: &Tree) -> Layout {
        layout
            .children(&tree.children)
            .next()
            .expect("Motion has a child layout while mounted")
    }

    /// Whether the child participates in focus, accessibility, and keyboard.
    ///
    /// Arrival and departure are not symmetric, so this concern and
    /// [`Self::hit_testable`] are separate predicates.
    ///
    /// * Leaving (`!visible`) is inert from the first frame of the exit. An AT
    ///   must not read, and Tab must not land on, a control that is going away
    ///   however long its paint is retained.
    /// * Arriving or present (`visible`) is reachable as soon as it is mounted,
    ///   at `progress == 0`. It *will* be there. One predicate serving both
    ///   concerns made an opening band inert for the whole reveal, so
    ///   `operation::focus` dispatched in the same frame as the open could not
    ///   reach the field and typing went to the document instead.
    ///   Focus and semantics must not wait for paint.
    fn reachable(&self) -> bool {
        self.visible
    }

    /// Whether the child is a usable pointer target yet.
    ///
    /// Unlike focus, a pointer aims at pixels, so this one *does* wait for
    /// paint. [`MotionEffect::Slide`] and [`MotionEffect::Scale`] paint the
    /// child translated or scaled away from the layout rectangle a hit test
    /// would use, so a click mid-reveal would land on a control that is not
    /// under the cursor; they wait for settlement. [`MotionEffect::Size`]
    /// reserves exactly the rectangle it paints, and [`MotionEffect::Fade`]
    /// keeps the child's exact geometry, so any non-empty painted fraction
    /// hit-tests honestly — but a transparent or empty one has nothing to hit.
    fn hit_testable(&self, progress: f32) -> bool {
        self.reachable()
            && (progress >= 1.0 - f32::EPSILON
                || (matches!(self.effect, MotionEffect::Size(_) | MotionEffect::Fade)
                    && progress > f32::EPSILON))
    }
}

/// Convenience constructor for [`Motion`].
#[must_use]
pub fn motion<'a, Message: 'a>(
    child: impl Into<Element<'a, Message>>,
    visible: bool,
    effect: MotionEffect,
) -> Motion<'a, Message> {
    Motion::new(child, visible, effect)
}

impl<'a, Message: 'a> Widget<Message, iced::Theme, iced::Renderer> for Motion<'a, Message> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<PanelMotion>()
    }

    fn state(&self) -> widget::tree::State {
        // Every effect animates on an unobserved policy. A settled-hidden
        // wrapper has no child, so it lays out at zero size and its parent
        // culls its `draw`: it cannot observe the theme before its first
        // reveal, and a conservative fallback made that reveal a jump (a
        // side pane that slid in only from its second opening on). The first
        // drawn frame latches the real policy, so reduced motion still snaps.
        let entering = self.visible
            && self
                .enter_on_mount
                .as_ref()
                .is_some_and(|entering| entering.get());
        let mut state =
            PanelMotion::at_target_with_unobserved_fallback(self.visible && !entering, true);
        self.apply_travel(&mut state);
        if entering {
            // It starts hidden, and the `diff` that follows its creation
            // queues the flip to visible. The flip starts once a draw has
            // read the theme, so reduced motion is known before it travels.
            widget::tree::State::new(state.commit_after_observed())
        } else {
            widget::tree::State::new(state)
        }
    }

    fn diff(&mut self, tree: &mut Tree) {
        // A target flip must continue from the value the last frame actually
        // painted. Anchoring to wall time here can advance the old trajectory
        // past that value before reversing it, so a second click makes the
        // panel jump in the wrong direction. The frame latch is also the clock
        // used by layout and draw; pre-first-frame flips are queued until the
        // first `RedrawRequested` supplies that clock.
        let state = tree.state.downcast_mut::<PanelMotion>();
        self.apply_travel(state);
        state.set_target_at_frame(self.visible);
        let mounted =
            self.visible || state.sampled_progress() > f32::EPSILON || state.sampled_animating();
        if mounted {
            tree.diff_children(std::slice::from_mut(&mut self.child));
        } else {
            tree.children.clear();
        }
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        if tree.children.is_empty() {
            return layout::void().apply(tree);
        }
        let state = tree.state.downcast_ref::<PanelMotion>();
        if self.effect.needs_layout() && !self.reflow && state.sampled_animating() {
            let _ = state.pin_reservation(true);
        }
        if self.reflow {
            state.note_laid_out_reservation(state.reservation_progress());
        }
        let sizing = self.child.as_widget().size();
        let child =
            self.child
                .as_widget_mut()
                .measure(&mut tree.children[0], renderer, &limits.loose());
        let MotionEffect::Size(axis) = self.effect else {
            return layout::Node::with_children(child.size(), vec![child]).apply(tree);
        };
        let rule = layout::reveal::Rule::Size {
            axes: match axis {
                MotionAxis::Horizontal => [true, false],
                MotionAxis::Vertical => [false, true],
                MotionAxis::Both => [true, true],
            },
            reservation: state.reservation_progress(),
            target: self.visible,
        };
        let progress = state.sampled_progress();
        let driver = state.sampled_animating().then_some(state.progress_node);
        let node = rule
            .finish(child, progress)
            .with_reveal_presentation(*limits, sizing, sizing, rule, progress, driver);
        tree.state
            .downcast_mut::<PanelMotion>()
            .note_layout_at_frame();
        node.apply(tree);
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
        // Latch this frame's timestamp into state *before* any read of
        // progress or animating this frame. Every downstream lifecycle
        // method resolves through the latch, so the geometry `layout`
        // computes and the ink `draw` paints cannot disagree within this
        // frame however costly the rest of the tree is.
        if let Event::Window(iced::window::Event::RedrawRequested(at)) = event {
            tree.state.downcast_mut::<PanelMotion>().note_frame(*at);
        }
        let completed = tree
            .state
            .downcast_mut::<PanelMotion>()
            .take_hidden_completion_at_frame();
        if completed {
            // Settled hidden: the child leaves the tree and the wrapper's
            // allocation collapses, so the siblings are laid out again now
            // rather than whenever some unrelated event next asks.
            shell.invalidate_layout();
            if let Some(on_hidden) = self.on_hidden.as_ref() {
                shell.publish(on_hidden());
            }
        }
        if tree.children.is_empty() {
            return;
        }
        let state = tree.state.downcast_ref::<PanelMotion>();
        let progress = state.sampled_progress();
        let animating = state.sampled_animating();
        // Frame-gate on the child's fully laid-out rectangle rather than the
        // wrapper's animated reservation. A `Size(_)` wrapper
        // legitimately has zero-area bounds at `progress == 0`, and
        // `is_visible` requires *positive* overlap, so an arriving reveal
        // measured by its animated area is judged offscreen: neither the
        // redraw request nor the layout invalidation that would advance the
        // next frame is ever issued, and the animation stalls until some
        // unrelated event happens to produce a frame.
        //
        // A settled-hidden wrapper stays inert without a separate cull here:
        // its `PanelMotion` reports `animating == false` and its children
        // are cleared by `diff`, so neither request line below can fire and
        // `tree.children.is_empty()` returned above. The relaxation only
        // affects arriving/departing wrappers, which really are on-screen
        // and really do owe the runtime a frame.
        let visible =
            crate::viewport_gate::is_visible(Self::child_layout(layout, tree).bounds(), viewport);
        // A node settled since its last recording still offers its settled track,
        // or a replay alongside animating siblings would show it mid-travel.
        let stale = !animating
            && (state
                .recorded_progress
                .get()
                .is_some_and(|recorded| (recorded - progress).abs() > 1e-4)
                || state
                    .recorded_offset
                    .get()
                    .is_some_and(|offset| offset.x.abs() + offset.y.abs() > 1e-3));
        if visible && (animating || stale) {
            let published = shell.tracks().len();
            // Only a recorded node may offer a track: the anchoring frame
            // paints nothing at progress 0 and forgets its recording, and a
            // track then would let the runtime replay a scene without it.
            if self.effect.needs_layout() && !self.reflow && state.recorded_progress.get().is_some()
            {
                let progress_track = |progress| {
                    let mut track = iced::advanced::renderer::Track::identity(state.progress_node);
                    track.translation.x = progress;
                    track
                };
                shell.publish_track(progress_track(progress));
                publish_curve(shell, state, progress_track);
            }

            // Offer this frame to the compositor lane. The offer is
            // only usable when a recording exists to measure against, and the
            // runtime still refuses it if anything else in the frame asked for
            // a rebuild -- including the `invalidate_layout` below, which is
            // exactly why a `Size` effect never reaches the lane.
            match self.effect {
                MotionEffect::Slide(direction) => {
                    // The recording retains the whole child at the offset it
                    // was drawn at (see `draw`), so the lane can move it in as
                    // well as out.
                    if state.recorded_offset.get().is_some() {
                        let bounds = layout.bounds();
                        // Absolute offsets: the recording keeps the offset
                        // its frame was drawn at (`mark_recording_at`), and
                        // a replay moves by the difference. A scene revised
                        // mid-travel therefore cannot pair with a curve
                        // measured against an older one.
                        // The clip is the cut a moving slide is drawn under. A
                        // recording made while settled holds the wider shadow
                        // clip, and replaying it on the way out would let the
                        // panel slide over its neighbour.
                        let margin = state.ink_margin();
                        let clip = |progress, spatial| {
                            Some(Self::ink_clip(
                                self.effect,
                                bounds,
                                margin,
                                Self::clip_progress(self.effect, progress, spatial),
                                state.reservation_progress(),
                            ))
                        };
                        // Translation reads the spatial value, which an
                        // expressive token may carry past rest; the clip
                        // reads the bounded one.
                        shell.publish_track(iced::advanced::renderer::Track {
                            node: state.node,
                            translation: Self::slide_offset(
                                direction,
                                bounds,
                                state.sampled_spatial_progress(),
                            ),
                            opacity: 1.0,
                            scale: 1.0,
                            scale_origin: iced::Point::ORIGIN,
                            rotation: 0.0,
                            clip: clip(progress, state.sampled_spatial_progress()),
                        });

                        // ... and hand the render lane the whole remaining
                        // travel, so it can keep advancing this node from the
                        // presentation clock while this thread is busy.
                        publish_curve_channels(shell, state, |sampled, spatial| {
                            iced::advanced::renderer::Track {
                                node: state.node,
                                translation: Self::slide_offset(direction, bounds, spatial),
                                opacity: 1.0,
                                scale: 1.0,
                                scale_origin: iced::Point::ORIGIN,
                                rotation: 0.0,
                                clip: clip(sampled, spatial),
                            }
                        });
                    }
                }
                MotionEffect::Fade => {
                    if state.recorded_progress.get().is_some() {
                        shell.publish_track(iced::advanced::renderer::Track {
                            node: state.node,
                            translation: Vector::ZERO,
                            // Absolute: a replayed segment's opacity replaces
                            // the alpha it was recorded under.
                            opacity: progress,
                            scale: 1.0,
                            scale_origin: iced::Point::ORIGIN,
                            rotation: 0.0,
                            clip: None,
                        });

                        publish_curve(shell, state, |sampled| iced::advanced::renderer::Track {
                            node: state.node,
                            translation: Vector::ZERO,
                            opacity: sampled,
                            scale: 1.0,
                            scale_origin: iced::Point::ORIGIN,
                            rotation: 0.0,
                            clip: None,
                        });
                    }
                }
                // A `Size` travel now composites. Its reservation is
                // pinned for the travel's duration (see `reservation_pin`), so
                // the reveal is a CLIP inside a rectangle that is not moving --
                // exactly the kind of work a compositor can do without the
                // widget. The recorded paint is the full held reservation; the
                // track narrows it.
                MotionEffect::Size(_) => {
                    // The reservation is pinned to the widest endpoint before
                    // recording, so both shrinking and growing reveals have
                    // every pixel the clip can expose. A reflowing one is laid
                    // out at its progress and drawn every frame.
                    if state.recorded_progress.get().is_some() && !self.reflow {
                        let bounds = layout.bounds();
                        let pinned = state.reservation_progress();

                        let margin = state.ink_margin();
                        let effect = self.effect;

                        shell.publish_track(iced::advanced::renderer::Track {
                            node: state.node,
                            translation: Vector::ZERO,
                            opacity: 1.0,
                            scale: 1.0,
                            scale_origin: iced::Point::ORIGIN,
                            rotation: 0.0,
                            clip: Some(Self::ink_clip(effect, bounds, margin, progress, pinned)),
                        });

                        // The curve is clamped to the recorded extent for the
                        // same reason: the lane must never ask for a wider clip
                        // than the scene it is holding can satisfy.
                        publish_curve(shell, state, move |sampled| {
                            iced::advanced::renderer::Track {
                                node: state.node,
                                translation: Vector::ZERO,
                                opacity: 1.0,
                                scale: 1.0,
                                scale_origin: iced::Point::ORIGIN,
                                rotation: 0.0,
                                clip: Some(Self::ink_clip(effect, bounds, margin, sampled, pinned)),
                            }
                        });
                    }
                }
                MotionEffect::Scale => {
                    if state.recorded_progress.get().is_some() {
                        let bounds = layout.bounds();
                        let current_scale = 0.92 + 0.08 * progress;
                        let origin = iced::Point::new(bounds.center_x(), bounds.center_y());

                        shell.publish_track(iced::advanced::renderer::Track {
                            node: state.node,
                            translation: Vector::ZERO,
                            // Absolute, as for `Fade`: a replayed segment's
                            // opacity replaces the alpha it was recorded under.
                            opacity: progress,
                            // Absolute: the recording contains the unscaled
                            // subtree and its live frame uses this same
                            // compositor path, avoiding a second resample.
                            scale: current_scale,
                            scale_origin: origin,
                            rotation: 0.0,
                            clip: None,
                        });

                        publish_curve(shell, state, move |sampled| {
                            iced::advanced::renderer::Track {
                                node: state.node,
                                translation: Vector::ZERO,
                                opacity: sampled,
                                scale: 0.92 + 0.08 * sampled,
                                scale_origin: origin,
                                rotation: 0.0,
                                clip: None,
                            }
                        });
                    }
                }
            }
            if animating {
                crate::motion::request_motion_frame(shell, shell.tracks().len() > published);
            }
        }
        // A reflowing reservation is laid out again whenever the travel has
        // moved it, every moving frame and the settling one. Its neighbours
        // move with it even while the element itself is still outside the
        // viewport (an opening panel starts at no width, past the edge), so
        // neither the layout nor the frame waits for it to be visible.
        if self.effect.needs_layout() && self.reflow {
            if !state.reservation_laid_out_at(state.reservation_progress()) {
                shell.invalidate_layout();
            }
            if animating {
                shell.request_redraw();
            }
        }
        // The reservation is pinned while it travels, so layout is invalidated
        // once, on the settling frame, which gives the siblings their end
        // geometry.
        if visible && self.effect.needs_layout() && !self.reflow {
            let animating_now = state.sampled_animating();
            let settling_commit = state.pin_reservation(animating_now);
            if state.is_reservation_pinned() {
                // Opening commits the widest first geometry exactly once so
                // the retained scene contains the destination ink. Closing is
                // already at that geometry. After this frame neither direction
                // reflows until settlement.
                //
                // The debt is read from state rather than inferred from a
                // before/after comparison around `pin_reservation`: `layout`
                // pins too, and it runs first, so the comparison always saw an
                // already-pinned travel and never issued this commit.
                if state.take_first_commit_owed() {
                    shell.invalidate_layout();
                }
            } else if settling_commit {
                // The single final layout commit that releases a pinned travel
                // onto its true end geometry.
                shell.invalidate_layout();
            }
        }
        if !animating && progress <= f32::EPSILON && !self.visible {
            // Settled hidden: the child leaves the tree, so the next paint is a
            // fresh recording and any surviving baseline would be a lie.
            state.forget_recording();
            tree.children.clear();
            return;
        }
        // Pointer events need usable geometry; keyboard, IME, clipboard and
        // redraw ticks must reach an arriving child from its first frame, or a
        // just-focused control cannot receive the keys the user is typing.
        let gated = match event {
            Event::Mouse(_) | Event::Touch(_) => !self.hit_testable(progress),
            _ => !self.reachable(),
        };
        if gated {
            return;
        }
        // The SHOWN rectangle, not the pinned reservation. Computed
        // before the child is borrowed mutably.
        let child_cursor =
            crate::inert_clip::cursor_within(cursor, self.shown_bounds(tree, layout));
        let child_layout = Self::child_layout(layout, tree);
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            child_layout,
            child_cursor,
            renderer,
            shell,
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
        if tree.children.is_empty() {
            return mouse::Interaction::default();
        }
        let progress = tree
            .presented_reveal_progress()
            .unwrap_or_else(|| Self::progress(tree));
        if !self.hit_testable(progress) {
            return mouse::Interaction::default();
        }
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            Self::child_layout(layout, tree),
            // The SHOWN rectangle, not the pinned reservation.
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
        let state = tree.state.downcast_ref::<PanelMotion>();
        state.observe_theme(theme);
        if tree.children.is_empty() {
            // Nothing was painted, so there is no recording to measure a track
            // against.
            state.forget_recording();
            return;
        }
        // INK read: identical latched instant to the `layout` GEOMETRY read
        // above, so layout and paint agree by construction.
        let progress = state.sampled_progress();
        let capture_viewport = tree
            .clip_presentation_node()
            .and_then(|node| renderer.layout_reveal_capture_viewport(node));
        let reveal_capture = capture_viewport.is_some();
        let viewport = capture_viewport.as_ref().unwrap_or(viewport);
        if progress <= f32::EPSILON && !reveal_capture {
            state.forget_recording();
            return;
        }
        let bounds = layout.bounds();
        let shown = self.shown_bounds(tree, layout);
        let child_layout = Self::child_layout(layout, tree);
        let draw_child = |renderer: &mut iced::Renderer| {
            self.child.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                child_layout,
                // The SHOWN rectangle, not the pinned reservation.
                crate::inert_clip::cursor_within(cursor, shown),
                viewport,
            );
        };
        use iced::advanced::Renderer as _;
        if reveal_capture
            && renderer.start_layout_reveal(
                tree.clip_presentation_node()
                    .expect("a reveal declaration is present"),
                progress,
            )
        {
            draw_child(renderer);
            renderer.end_layer();
            renderer.mark_recording(iced::advanced::renderer::NodeId::STATIC);
            state.note_recorded(Vector::ZERO, progress);
            return;
        }
        let node = state.node;
        let margin = state.ink_margin();
        let clip = Self::ink_clip(
            self.effect,
            bounds,
            margin,
            Self::clip_progress(self.effect, progress, state.sampled_spatial_progress()),
            state.reservation_progress(),
        );
        // A `Size` reveal cuts in its own retained layer below. Its outer
        // layer keeps the whole reservation, because a plain clip caps what
        // every layer inside it retains, and the lane could then never widen
        // an opening reveal past its first strip.
        let outer = match self.effect {
            MotionEffect::Size(_) => bounds.expand(margin),
            _ => clip,
        };
        // A slide shows the part of its child inside the clip but retains all
        // of it where it was drawn, so the lane can move the recording in as
        // well as out without cutting off the part that had not arrived.
        let slide_offset = match self.effect {
            MotionEffect::Slide(direction) => Some(super::snap_to_device(
                Self::slide_offset(direction, bounds, state.sampled_spatial_progress()),
                renderer,
            )),
            _ => None,
        };
        let paint = |renderer: &mut iced::Renderer| match self.effect {
            MotionEffect::Slide(_) => {
                let offset = slide_offset.unwrap_or(Vector::ZERO);

                // Attribute this subtree's paint to the transition's scene node
                // and remember where it was painted, so a later animation-only
                // frame can move it with a track instead of asking the child to
                // draw again. The frame still paints normally: this is
                // an ordinary draw that is merely addressable afterwards.
                renderer.with_motion(node, offset, draw_child);

                state.note_recorded(offset, progress);
            }
            MotionEffect::Scale => {
                let scale = 0.92 + 0.08 * progress;
                let origin = iced::Point::new(bounds.center_x(), bounds.center_y());
                // Record unscaled content inside the same offscreen compositor
                // operation replay uses. The live control and retained lane now
                // each rasterise once and sample once, so their pixels agree.
                // It fades as it scales, so an exit dissolves rather than
                // shrinking at full strength and then vanishing.
                renderer.with_layer_composite(clip, progress, scale, 0.0, origin, |renderer| {
                    renderer.mark_recording(node);
                    draw_child(renderer);
                    renderer.mark_recording(iced::advanced::renderer::NodeId::STATIC);
                });
                state.note_recorded(Vector::ZERO, progress);
            }
            MotionEffect::Size(_) => {
                // `bounds` is the held reservation, so the reveal is clipped
                // explicitly. The published track uses the same `ink_clip`, so
                // a composited replay paints what this draw paints.
                renderer.with_layer_retained(clip, bounds.expand(margin), |renderer| {
                    renderer.mark_recording(node);
                    draw_child(renderer);
                    renderer.mark_recording(iced::advanced::renderer::NodeId::STATIC);
                });

                // The recording is the FULL held reservation's paint; the track
                // supplies the clip. Recording it already-clipped would make
                // every later frame of the travel unable to reveal more.
                state.note_recorded(Vector::ZERO, progress);
            }
            MotionEffect::Fade => {
                // A replayed segment's opacity REPLACES the alpha it was
                // recorded under (the appended layers are re-tagged with the
                // groups active at replay), so a track carries the absolute
                // alpha and a fade composes in both directions -- not only
                // downwards, which a multiplying ratio would have limited it to.
                renderer.with_layer_alpha(clip, progress, |renderer| {
                    // Marked inside the alpha scope on purpose: the segment's
                    // recorded alpha depth then includes this effect's own
                    // group, so a replay replaces exactly that group with the
                    // track's opacity while any group a descendant pushed
                    // survives.
                    renderer.mark_recording(node);
                    draw_child(renderer);
                    renderer.mark_recording(iced::advanced::renderer::NodeId::STATIC);
                });

                state.note_recorded(Vector::ZERO, progress);
            }
        };
        match slide_offset {
            Some(offset) => {
                let retained = (bounds + offset).expand(margin).union(&clip);
                renderer.with_layer_retained(outer, retained, paint);
            }
            None => renderer.with_layer(outer, paint),
        }
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        if operation.is_native_reconciliation()
            && let Some(progress) = tree.presented_reveal_progress()
        {
            tree.state
                .downcast_ref::<PanelMotion>()
                .note_presented_progress(progress);
        }
        if tree.children.is_empty() {
            return;
        }
        if !self.reachable() && !operation.is_native_reconciliation() {
            return;
        }
        // This wrapper is structurally transparent to operations whenever its
        // child is present. Clipping to the animated reservation is wrong twice
        // over: at rest it changed an open modal's AT tree, because scrollable
        // composites legitimately publish realised descendants beyond the
        // surface viewport and the extra clip dropped the final settings tab
        // plus most command-palette rows; mid-reveal a `Size`
        // rectangle is zero- or few-pixels tall, so every focusable and every
        // semantics node inside an opening band was discarded.
        let child_layout = Self::child_layout(layout, tree);
        self.child.as_widget_mut().operate(
            &mut tree.children[0],
            child_layout,
            viewport,
            renderer,
            operation,
        );
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
        if tree.children.is_empty() {
            return Vec::new();
        }
        // An overlay carries its own focusable content and semantics, and the
        // child is now receiving keyboard events while arriving; suppressing the
        // menu it believes it opened would desynchronise input from paint. It
        // follows reachability, not hit-testability.
        if !self.reachable() {
            return Vec::new();
        }
        let progress = Self::progress(tree);
        let child_layout = Self::child_layout(layout, tree);
        let children = self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            child_layout,
            renderer,
            viewport,
            translation,
            window,
        );
        if self.effect == MotionEffect::Fade && progress < 1.0 - f32::EPSILON {
            children
                .into_iter()
                .map(|content| {
                    overlay::Element::new(Box::new(AlphaOverlay {
                        content,
                        alpha: progress,
                        window,
                    }))
                })
                .collect()
        } else {
            children
        }
    }
}

struct AlphaOverlay<'a, Message> {
    content: overlay::Element<'a, Message, iced::Theme, iced::Renderer>,
    alpha: f32,
    /// The composited group covers the window: since upstream iced #3479 an
    /// overlay no longer reports its own bounds.
    window: Size,
}

impl<Message> overlay::Overlay<Message, iced::Theme, iced::Renderer> for AlphaOverlay<'_, Message> {
    fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        cursor: mouse::Cursor,
    ) {
        use iced::advanced::Renderer as _;
        renderer.with_layer_alpha(Rectangle::with_size(self.window), self.alpha, |renderer| {
            self.content
                .as_overlay()
                .draw(renderer, theme, style, cursor);
        });
    }

    fn operate(&mut self, renderer: &iced::Renderer, operation: &mut dyn widget::Operation) {
        self.content.as_overlay_mut().operate(renderer, operation);
    }

    fn update(
        &mut self,
        event: &Event,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        shell: &mut Shell<'_, Message>,
    ) {
        self.content
            .as_overlay_mut()
            .update(event, cursor, renderer, shell);
    }

    fn mouse_interaction(
        &self,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content
            .as_overlay()
            .mouse_interaction(cursor, renderer)
    }

    fn overlay<'a>(
        &'a mut self,
        renderer: &iced::Renderer,
    ) -> Vec<overlay::Element<'a, Message, iced::Theme, iced::Renderer>> {
        let (alpha, window) = (self.alpha, self.window);

        self.content
            .as_overlay_mut()
            .overlay(renderer)
            .into_iter()
            .map(|content| {
                overlay::Element::new(Box::new(AlphaOverlay {
                    content,
                    alpha,
                    window,
                }))
            })
            .collect()
    }

    fn index(&self) -> f32 {
        self.content.as_overlay().index()
    }

    fn node(&self) -> Option<&iced::advanced::layout::Node> {
        self.content.as_overlay().node()
    }

    // A fading menu still leaves the window in a popup: the runtime reads
    // these to size, place and grab it.
    fn area(&self) -> Option<Rectangle> {
        self.content.as_overlay().area()
    }

    fn anchor(&self) -> Option<Rectangle> {
        self.content.as_overlay().anchor()
    }

    fn grabs(&self) -> bool {
        self.content.as_overlay().grabs()
    }
}

impl<'a, Message: 'a> From<Motion<'a, Message>> for Element<'a, Message> {
    fn from(value: Motion<'a, Message>) -> Self {
        Element::new(value)
    }
}

/// Theme-driven transition between two retained element subtrees.
///
/// `show_incoming` is the target state. Both subtrees are laid out
/// simultaneously and the wrapper reserves their maximum width and height for
/// its whole lifetime. The stable maximum reservation is deliberate: unlike
/// dropping the outgoing layout at settlement, differently sized content
/// cannot make its parent jump on the last frame. The tradeoff is two retained
/// trees and two layouts; use [`motion`] with [`MotionEffect::Fade`] when only
/// one subtree is needed.
///
/// During motion, the incoming subtree is painted as the backing image and the
/// outgoing subtree is one compositing group at `1 - t`. Thus an opaque overlap
/// has the exact weighted result `outgoing * (1 - t) + incoming * t`, without
/// the brightness dip caused by source-over compositing two translucent sibling
/// groups. At `t == 0` and `t == 1`, only the corresponding endpoint is drawn,
/// so endpoint paint takes the renderer's allocation-free ordinary path.
///
/// Only the target subtree receives input, operations, accessibility, or
/// overlays. Like [`MotionEffect::Fade`], this is paint-only: animation frames
/// request redraws but never invalidate layout.
pub struct CrossFade<'a, Message> {
    outgoing: Element<'a, Message>,
    incoming: Element<'a, Message>,
    show_incoming: bool,
}

impl<'a, Message: 'a> CrossFade<'a, Message> {
    /// Create a cross-fade retaining both endpoint subtrees.
    #[must_use]
    pub fn new(
        outgoing: impl Into<Element<'a, Message>>,
        incoming: impl Into<Element<'a, Message>>,
        show_incoming: bool,
    ) -> Self {
        Self {
            outgoing: outgoing.into(),
            incoming: incoming.into(),
            show_incoming,
        }
    }

    fn child_layout(layout: Layout, tree: &Tree, index: usize) -> Layout {
        layout
            .children(&tree.children)
            .nth(index)
            .expect("CrossFade always owns two child layouts")
    }

    fn active_index(&self) -> usize {
        usize::from(self.show_incoming)
    }
}

/// Convenience constructor for [`CrossFade`].
#[must_use]
pub fn cross_fade<'a, Message: 'a>(
    outgoing: impl Into<Element<'a, Message>>,
    incoming: impl Into<Element<'a, Message>>,
    show_incoming: bool,
) -> CrossFade<'a, Message> {
    CrossFade::new(outgoing, incoming, show_incoming)
}

impl<'a, Message: 'a> Widget<Message, iced::Theme, iced::Renderer> for CrossFade<'a, Message> {
    fn tag(&self) -> widget::tree::Tag {
        widget::tree::Tag::of::<PanelMotion>()
    }

    fn state(&self) -> widget::tree::State {
        widget::tree::State::new(PanelMotion::at_target(self.show_incoming))
    }

    fn diff(&mut self, tree: &mut Tree) {
        // The flip starts from the value the last frame presented and is
        // anchored by the frame that carries it, so a cross-fade triggered on
        // an idle UI cannot snap.
        tree.state
            .downcast_mut::<PanelMotion>()
            .set_target_at_frame(self.show_incoming);
        tree.diff_children(&mut [&mut self.outgoing, &mut self.incoming]);
    }

    fn size(&self) -> Size<Length> {
        let outgoing = self.outgoing.as_widget().size();
        let incoming = self.incoming.as_widget().size();
        let combined = |a: Length, b: Length| {
            if a.is_fill() || b.is_fill() {
                Length::Fill
            } else {
                Length::Fit
            }
        };
        Size::new(
            combined(outgoing.width, incoming.width),
            combined(outgoing.height, incoming.height),
        )
    }

    fn layout(&mut self, tree: &mut Tree, renderer: &iced::Renderer, limits: &layout::Limits) {
        let (outgoing_tree, incoming_tree) = tree.children.split_at_mut(1);
        let outgoing =
            self.outgoing
                .as_widget_mut()
                .measure(&mut outgoing_tree[0], renderer, &limits.loose());
        let incoming =
            self.incoming
                .as_widget_mut()
                .measure(&mut incoming_tree[0], renderer, &limits.loose());
        let size = Size::new(
            outgoing.size().width.max(incoming.size().width),
            outgoing.size().height.max(incoming.size().height),
        );
        layout::Node::with_children(size, vec![outgoing, incoming]).apply(tree);
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
        // Latch the frame timestamp so `draw` reads the same instant as any
        // read that happens here in `update`. Without the latch,
        // `is_animating` here and `progress` in `draw` would sample two
        // different wall-clock instants and the tree's cost between them
        // would leak into the cross-fade curve.
        if let Event::Window(iced::window::Event::RedrawRequested(at)) = event {
            tree.state.downcast_mut::<PanelMotion>().note_frame(*at);
        }
        let state = tree.state.downcast_ref::<PanelMotion>();
        let bounds = layout.bounds();
        if state.sampled_animating() && crate::viewport_gate::is_visible(bounds, viewport) {
            // A cross-fade's whole travel is the OUTGOING group's alpha
            // -- the incoming subtree is painted opaque and unchanged
            // underneath it -- so one opacity track on the recorded outgoing
            // node is the entire effect. Only a recording taken from the
            // mid-travel branch can be replayed: the endpoint branches paint a
            // single child, and `draw` drops the baseline there.
            let progress = state.sampled_progress();

            if state.recorded_progress.get().is_some() {
                shell.publish_track(iced::advanced::renderer::Track {
                    node: state.node,
                    translation: Vector::ZERO,
                    // Absolute, and inverted: the recorded group is the
                    // outgoing child, which fades out as progress rises.
                    opacity: 1.0 - progress,
                    scale: 1.0,
                    scale_origin: iced::Point::ORIGIN,
                    rotation: 0.0,
                    clip: None,
                });

                publish_curve(shell, state, |sampled| iced::advanced::renderer::Track {
                    node: state.node,
                    translation: Vector::ZERO,
                    opacity: 1.0 - sampled,
                    scale: 1.0,
                    scale_origin: iced::Point::ORIGIN,
                    rotation: 0.0,
                    clip: None,
                });
            }
            crate::motion::request_motion_frame(shell, state.recorded_progress.get().is_some());
        }

        let index = self.active_index();
        let child = if index == 0 {
            &mut self.outgoing
        } else {
            &mut self.incoming
        };
        let child_layout = Self::child_layout(layout, tree, index);
        child.as_widget_mut().update(
            &mut tree.children[index],
            event,
            child_layout,
            crate::inert_clip::cursor_within(cursor, bounds),
            renderer,
            shell,
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
        let index = self.active_index();
        let child = if index == 0 {
            &self.outgoing
        } else {
            &self.incoming
        };
        child.as_widget().mouse_interaction(
            &tree.children[index],
            Self::child_layout(layout, tree, index),
            crate::inert_clip::cursor_within(cursor, layout.bounds()),
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
        use iced::advanced::Renderer as _;

        let state = tree.state.downcast_ref::<PanelMotion>();
        state.observe_theme(theme);
        // Paint the cross-fade at the frame-latched instant, so the
        // outgoing / incoming alpha split matches whatever `update` last
        // measured on this frame.
        let progress = state.sampled_progress();
        let bounds = layout.bounds();
        let cursor = crate::inert_clip::cursor_within(cursor, bounds);
        let draw = |index: usize, renderer: &mut iced::Renderer| {
            let child = if index == 0 {
                &self.outgoing
            } else {
                &self.incoming
            };
            child.as_widget().draw(
                &tree.children[index],
                renderer,
                theme,
                style,
                Self::child_layout(layout, tree, index),
                cursor,
                viewport,
            );
        };

        let node = state.node;

        renderer.with_layer(bounds, |renderer| {
            if progress <= f32::EPSILON {
                // Settled on the outgoing endpoint: one child, no alpha group.
                // A track measured against this recording would fade a subtree
                // that is not there.
                state.forget_recording();
                draw(0, renderer);
            } else if progress >= 1.0 - f32::EPSILON {
                state.forget_recording();
                draw(1, renderer);
            } else {
                draw(1, renderer);
                renderer.with_layer_alpha(bounds, 1.0 - progress, |renderer| {
                    // Marked inside the alpha scope, as `Fade` is: the recorded
                    // segment's alpha depth then includes this group, so a
                    // replay replaces exactly it.
                    renderer.mark_recording(node);
                    draw(0, renderer);
                    renderer.mark_recording(iced::advanced::renderer::NodeId::STATIC);
                });

                state.note_recorded(Vector::ZERO, progress);
            }
        });
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        let index = self.active_index();
        let child = if index == 0 {
            &mut self.outgoing
        } else {
            &mut self.incoming
        };
        let child_layout = Self::child_layout(layout, tree, index);
        child.as_widget_mut().operate(
            &mut tree.children[index],
            child_layout,
            viewport,
            renderer,
            operation,
        );
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
        let index = usize::from(self.show_incoming);
        let child_layout = Self::child_layout(layout, tree, index);
        let child = if index == 0 {
            &mut self.outgoing
        } else {
            &mut self.incoming
        };
        child.as_widget_mut().overlay(
            &mut tree.children[index],
            child_layout,
            renderer,
            viewport,
            translation,
            window,
        )
    }
}

impl<'a, Message: 'a> From<CrossFade<'a, Message>> for Element<'a, Message> {
    fn from(value: CrossFade<'a, Message>) -> Self {
        Element::new(value)
    }
}
