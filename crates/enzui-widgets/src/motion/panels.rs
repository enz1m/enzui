// motion split production module: retained panels

use super::*;
use enzui_theme::{ExpressiveMotion, Spring};
use iced::advanced::layout::Measure as _;

mod reveal_widget;

/// Persistent, allocation-free state for a boolean panel transition.
///
/// The shared lifecycle half of sliding sidebars/inspectors/future panels:
/// latches the theme's raw `motion-normal` duration, reduced flag and easing
/// separately; unobserved policy counts as reduced unless the owner opts into
/// the size-reveal fallback; rebuilds `Animation` only on policy change.
/// Wrappers consume [`progress`](PanelMotion::progress) without re-invoking a
/// view closure per frame. Not `Transition`: that widget calls its boxed view
/// closure and builds a new `Element` every frame — a panel owns a retained
/// child tree (per-frame rebuilds allocate and strand hidden panels in
/// focus/a11y traversal).
///
/// # The intra-frame clock
///
/// [`note_frame`](Self::note_frame) latches the `RedrawRequested(now)` iced
/// hands over; every same-frame read — layout, paint, mouse test, operate,
/// overlay — resolves through the `sampled_*` readers and
/// [`set_target_at_frame`](Self::set_target_at_frame), never a second
/// `Instant::now()`. Two reads compute STATE at T1 (layout) and INK at T2
/// (draw), and T2−T1 is the tree's inter-visit cost — the trajectory becomes
/// a function of app business, not frame timestamps. Pre-first-frame the clock
/// is `None`: sampled readers return the pre-flip endpoint and
/// `set_target_at_frame` queues for the first frame rather than starting
/// against a made-up instant. Owners needing animation pre-frame feed
/// [`set_target`](Self::set_target) their own instant (`app_shell` does).
#[derive(Debug, Clone)]
pub struct PanelMotion {
    anim: iced::animation::Animation<bool>,
    target: bool,
    hidden_notified: bool,
    raw_normal_ms: std::cell::Cell<f32>,
    reduced: std::cell::Cell<Option<bool>>,
    easing: std::cell::Cell<MotionEasing>,
    applied_easing: MotionEasing,
    applied_ms: f32,
    layout_at_target: bool,
    animate_if_unobserved: bool,
    /// A queued flip waits for a drawn frame to latch the theme policy, so an
    /// entering wrapper under reduced motion appears at once.
    commit_after_observed: bool,
    /// Frame timestamp latched by [`Self::note_frame`] on `RedrawRequested`.
    /// `None` before any frame; then every sampled read returns the value at
    /// that one instant, so layout and draw within one frame cannot disagree
    frame_at: std::cell::Cell<Option<iced::time::Instant>>,
    /// A queued target flip, recorded as the progress the last frame
    /// presented.
    ///
    /// [`set_target_at_frame`](Self::set_target_at_frame) queues here;
    /// [`note_frame`](Self::note_frame) commits against a real frame timestamp —
    /// the whole anchoring rule, needing no "fresh" clock: the trigger-visible
    /// instant is always the PREVIOUS frame's (arbitrarily old after idle UI —
    /// anchoring there ends the travel on the next frame); wall time breaks
    /// one-frame-one-clock. The trigger-carrying frame's instant is real,
    /// sampled by this frame's `layout`/`draw`, and never stale.
    ///
    /// The stored value is the progress the last frame presented, which
    /// `sampled_progress` reports while the flip is queued so layout does not
    /// jump ahead of paint. The commit does not start from it: it samples the
    /// replaced travel at the committing frame's instant and reverses from
    /// there, so the first reversed frame continues the old travel instead of
    /// repeating its last frame.
    pending_from: Option<f32>,
    /// This transition's stable compositor scene node.
    ///
    /// Minted once with the widget's tree state, living exactly as long —
    /// surviving every `view` rebuild, dying with the widget. The identity a
    /// renderer track addresses: without it a replayed scene cannot say which
    /// subtree moved.
    pub(super) node: iced::advanced::renderer::NodeId,
    // Layout progress must not reinterpret the physical track's translation or clip channels.
    pub(super) progress_node: iced::advanced::renderer::NodeId,
    presented_progress: std::cell::Cell<Option<f32>>,
    /// How far a descendant's ink may reach past the wrapper's bounds,
    /// latched from the theme in [`Self::observe_theme`] so a published
    /// track clips where the immediate draw does.
    ink_margin: std::cell::Cell<iced::Padding>,
    /// The travel offset this node was last recorded at, and whether the
    /// recording is usable.
    ///
    /// A track's translation is relative to the recording scene, not the
    /// settled position — the widget remembers what it painted. `draw` writes;
    /// `update_frame` publishes the difference. A fallback real draw
    /// re-records and resets the baseline: both paths agree with neither
    /// knowing which ran.
    pub(super) recorded_offset: std::cell::Cell<Option<Vector>>,
    /// The alpha this node was last recorded at, for `Fade`.
    pub(super) recorded_progress: std::cell::Cell<Option<f32>>,
    /// The reservation a reflowing effect's layout is PINNED at for the whole
    /// of the current travel.
    ///
    /// `Size`/`Reserve` is the one family whose wrapper changes size — the one
    /// that invalidated layout every frame, disqualifying every frame from the
    /// compositor and running the whole tree per frame. Pinning migrates it:
    /// the reservation latches at the travel's first geometry and holds, so
    /// parent/siblings lay out once; the visible extent animates by clip track
    /// on the retained scene (a compositor op). Settlement releases the pin
    /// and invalidates once — the final commit giving siblings true end
    /// geometry. `None` = not travelling: layout follows live progress
    /// (settled and reduced-motion cases). The pin is a FRACTION, never
    /// absolute: `layout` re-reads the child's full size every pass and
    /// re-applies the fraction — resize/scale/theme remeasure reserves right
    /// without noticing limits moved.
    reservation_pin: std::cell::Cell<Option<f32>>,
    /// Set when a pinned travel has ended and the single final layout commit is
    /// still owed. It survives the pin being cleared, so exactly one
    /// `invalidate_layout` is issued however many frames arrive afterwards.
    final_commit_owed: std::cell::Cell<bool>,
    /// Set when the reservation pin latches on an ARRIVING travel and the one
    /// first-geometry commit is still owed.
    ///
    /// The pin can latch in `layout` (pre-`update`), so `update` cannot detect
    /// it by comparing `is_reservation_pinned()` around its own call.
    /// Recording the debt where incurred, spending it in `update`, is
    /// order-independent.
    first_commit_owed: std::cell::Cell<bool>,
    /// The reservation progress the last layout used, for an effect that
    /// reflows on every frame.
    laid_out_reservation: std::cell::Cell<Option<f32>>,
    /// Bumped every time the travel is retargeted, reversed or settled.
    ///
    /// The render lane keeps presenting its held animation, replacing it on a
    /// generation change (or a re-timed curve, a travel delayed by a stall):
    /// re-publishing the same travel per frame cannot restart or jitter a
    /// running track, while a genuine reversal lands on the next presented
    /// frame.
    generation: u64,
    /// The opted-in expressive or spring travel, which replaces `anim` when
    /// set: see [`Self::set_expressive`] and [`Self::set_spring`]. `None`
    /// keeps the default curve exactly.
    expressive: Option<Travel>,
    /// The spatial value a queued flip will start from: the last presented
    /// one, which may have overshot. Paired with `pending_from`.
    pending_spatial: Option<f32>,
}

/// A new scene node for a widget that animates through the render lane.
///
/// A widget paints with the node (`Renderer::with_motion`) and
/// publishes `Track`s for it; the runtime then replays the recorded paint
/// instead of drawing it again. Every motion driver shares this allocator, so
/// two widgets never move each other's paint. The upper half of the range
/// belongs to automatic layout nodes.
///
/// # Panics
///
/// After 2^63 - 1 nodes, when the lower half of the range is exhausted.
#[must_use]
pub fn next_scene_node() -> iced::advanced::renderer::NodeId {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(1);

    let id = NEXT
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1).filter(|next| *next < (1 << 63))
        })
        .expect("motion scene identity exhausted");
    iced::advanced::renderer::NodeId(id)
}

impl PanelMotion {
    /// Construct a transition settled at its first target.
    ///
    /// An animation answers a change; construction is none. Starting from an
    /// unrelated default then calling [`set_target`](Self::set_target)
    /// manufactures a cold-start transition from an unchosen state. Pre-
    /// [`observe_theme`](Self::observe_theme) policy is unknown, hence reduced:
    /// genuine early edges still jump.
    #[must_use]
    pub fn at_target(visible: bool) -> Self {
        Self::at_target_with_unobserved_fallback(visible, false)
    }

    /// Construct settled state with an opt-in fallback for a culled size node.
    ///
    /// `layout`/`diff`/`update` never see `&Theme`, so none resolves live
    /// motion policy — and a settled-hidden `Size` node is zero-height, so its
    /// parent never calls `draw`, the only theme-observing lifecycle. For that
    /// one effect the default full-motion duration is the only non-invasive
    /// first-reveal mover. Next draw observes the real policy, winning
    /// thereafter; every other owner keeps unknown-is-reduced.
    pub(super) fn at_target_with_unobserved_fallback(
        visible: bool,
        animate_if_unobserved: bool,
    ) -> Self {
        let raw = MotionScale::default().normal_ms;
        let easing = MotionEasing::default();
        Self {
            anim: build_anim(visible, 0.0, easing),
            target: visible,
            // Construction at hidden is already settled; a completion belongs
            // only to a later visible -> hidden edge.
            hidden_notified: !visible,
            raw_normal_ms: std::cell::Cell::new(raw),
            reduced: std::cell::Cell::new(None),
            easing: std::cell::Cell::new(easing),
            applied_easing: easing,
            applied_ms: 0.0,
            layout_at_target: true,
            animate_if_unobserved,
            commit_after_observed: false,
            frame_at: std::cell::Cell::new(None),
            pending_from: None,
            node: next_scene_node(),
            progress_node: next_scene_node(),
            presented_progress: std::cell::Cell::new(None),
            ink_margin: std::cell::Cell::new(iced::Padding::ZERO),
            recorded_offset: std::cell::Cell::new(None),
            recorded_progress: std::cell::Cell::new(None),
            reservation_pin: std::cell::Cell::new(None),
            final_commit_owed: std::cell::Cell::new(false),
            first_commit_owed: std::cell::Cell::new(false),
            laid_out_reservation: std::cell::Cell::new(None),
            generation: 0,
            expressive: None,
            pending_spatial: None,
        }
    }

    /// Follow an expressive token instead of the theme's easing, or `None`
    /// for the default travel.
    ///
    /// The token's spatial curve may pass the target and settle back; only
    /// [`spatial_progress`](Self::spatial_progress) and
    /// [`sampled_spatial_progress`](Self::sampled_spatial_progress) show
    /// that. Every other reader — [`progress`](Self::progress), the
    /// reservation, the sampled readers, completion — stays within `[0, 1]`,
    /// so clip, reservation, fills, input and exit completion remain bounded.
    /// The duration is the token's, scaled as `motion-normal` is by the
    /// theme's speed, and zero under reduced motion. Switching settles the
    /// motion at its target without a travel.
    pub fn set_expressive(&mut self, motion: Option<ExpressiveMotion>) {
        if self.expressive() == motion {
            return;
        }
        self.expressive = motion
            .map(|motion| Travel::Curve(super::ExpressiveTravel::settled(motion, self.target)));
        self.anim = build_anim(self.target, 0.0, self.easing.get());
        self.pending_from = None;
        self.pending_spatial = None;
        self.layout_at_target = true;
        self.note_retarget();
    }

    /// [`Self::set_expressive`] as a builder.
    #[must_use]
    pub fn with_expressive(mut self, motion: Option<ExpressiveMotion>) -> Self {
        self.set_expressive(motion);
        self
    }

    /// The expressive token this motion follows, if any.
    #[must_use]
    pub fn expressive(&self) -> Option<ExpressiveMotion> {
        match self.expressive.as_ref() {
            Some(Travel::Curve(travel)) => Some(travel.motion()),
            _ => None,
        }
    }

    /// Travel on a physical spring instead of the theme's easing, or `None`
    /// for the default travel.
    ///
    /// A spring keeps its velocity through a reversal: flipping the target
    /// mid-travel turns the panel around smoothly instead of starting a new
    /// curve from rest. An underdamped spring's overshoot shows only in
    /// [`spatial_progress`](Self::spatial_progress); every other reader stays
    /// within `[0, 1]`, as with [`Self::set_expressive`]. The spring plays
    /// slower or faster with the theme's `motion-normal` speed and lands at
    /// once under reduced motion. Switching settles the motion at its target
    /// without a travel, and replaces an expressive token.
    pub fn set_spring(&mut self, spring: Option<Spring>) {
        if self.spring() == spring {
            return;
        }
        self.expressive =
            spring.map(|spring| Travel::Spring(super::SpringTravel::settled(spring, self.target)));
        self.anim = build_anim(self.target, 0.0, self.easing.get());
        self.pending_from = None;
        self.pending_spatial = None;
        self.layout_at_target = true;
        self.note_retarget();
    }

    /// [`Self::set_spring`] as a builder.
    #[must_use]
    pub fn with_spring(mut self, spring: Option<Spring>) -> Self {
        self.set_spring(spring);
        self
    }

    /// The spring this motion follows, if any.
    #[must_use]
    pub fn spring(&self) -> Option<Spring> {
        match self.expressive.as_ref() {
            Some(Travel::Spring(travel)) => Some(travel.spring()),
            _ => None,
        }
    }

    /// The policy an expressive travel resolves its duration against: the
    /// observed `motion-normal` speed, and reduced whenever this motion's own
    /// effective duration is zero (reduced motion, or an unobserved policy).
    fn expressive_scale(&self) -> MotionScale {
        MotionScale {
            normal_ms: self.raw_normal_ms.get(),
            reduced_motion: self.effective_ms() <= f32::EPSILON,
            ..MotionScale::default()
        }
    }

    /// Settle the expressive travel, if any, at the current target.
    fn settle_expressive(&mut self) {
        if let Some(travel) = self.expressive.as_mut() {
            travel.settle(self.target);
        }
        self.pending_spatial = None;
    }

    /// The reservation fraction `layout` must use for a reflowing effect
    ///
    ///
    /// Pinned travels hold first geometry (reservation — and every sibling —
    /// still); otherwise the live sampled progress (settled, reduced-motion,
    /// and the travel's first pre-pin frame).
    #[must_use]
    pub fn reservation_progress(&self) -> f32 {
        self.reservation_pin
            .get()
            .unwrap_or_else(|| self.sampled_progress())
    }

    /// Whether a reflowing effect's reservation is currently pinned.
    #[must_use]
    pub fn is_reservation_pinned(&self) -> bool {
        self.reservation_pin.get().is_some()
    }

    /// Takes the owed OPENING first-geometry commit, if the pin latched on an
    /// arriving travel since it was last spent.
    ///
    /// Set where the pin latches -- which may be `layout`, not `update` -- so
    /// the debt survives the phase that discovered it. Spent exactly once.
    pub(super) fn take_first_commit_owed(&self) -> bool {
        self.first_commit_owed.replace(false)
    }

    /// Records the reservation progress a layout used.
    pub(super) fn note_laid_out_reservation(&self, progress: f32) {
        self.laid_out_reservation.set(Some(progress));
    }

    /// Whether the reservation the last layout used is not `progress`.
    pub(super) fn reservation_laid_out_at(&self, progress: f32) -> bool {
        self.laid_out_reservation
            .get()
            .is_some_and(|laid_out| (laid_out - progress).abs() <= f32::EPSILON)
    }

    /// Pin/release the reservation for a reflowing effect, returning whether
    /// this frame owes the single final layout commit.
    ///
    /// `animating` is the travel sampled at this frame's latched instant, once
    /// per frame from `update` (the only state-changing lifecycle). Every Size
    /// travel records the WIDEST endpoint and clips inside it: opening commits
    /// first-geometry once (recording holds destination ink); closing keeps
    /// the already-wide geometry. No reflow either direction; one final commit
    /// at pin release.
    pub(super) fn pin_reservation(&self, animating: bool) -> bool {
        if animating {
            if self.reservation_pin.get().is_none() {
                self.reservation_pin.set(Some(1.0));
                self.final_commit_owed.set(true);
                // Record the owed OPENING commit at pin-latch, never by
                // `update`-side inference: `layout` pins too (else an opening
                // travel leaks one progress-sized node pre-commit) and runs
                // first — by `update`'s look the pin was already latched,
                // `!was_pinned` false, and the one `invalidate_layout` growing
                // the reservation to destination ink never issued: the reveal
                // paints zero-area forever. An owed flag cannot be lost:
                // whoever latches records the debt; only `update` spends it.
                self.first_commit_owed.set(self.target);
            }
            return false;
        }
        // Settled. Release the pin so layout reports the true end geometry,
        // discard any opening debt skipped by a jump straight to the endpoint,
        // and hand out the final commit exactly once.
        self.reservation_pin.set(None);
        self.first_commit_owed.set(false);
        self.final_commit_owed.replace(false)
    }

    /// This transition's stable compositor scene node.
    #[must_use]
    pub fn scene_node(&self) -> iced::advanced::renderer::NodeId {
        self.node
    }

    /// Records what this node was painted at, so a later animation-only frame
    /// can publish a track relative to it.
    pub(super) fn note_recorded(&self, offset: Vector, progress: f32) {
        self.recorded_offset.set(Some(offset));
        self.recorded_progress.set(Some(progress));
    }

    /// Forgets the recording baseline.
    ///
    /// Called whenever the widget stops painting -- settled hidden, culled, or
    /// child dropped -- because the next paint will be a fresh recording and a
    /// track measured against the old one would place the node by an offset
    /// nobody drew.
    pub(super) fn forget_recording(&self) {
        self.recorded_offset.set(None);
        self.recorded_progress.set(None);
    }

    /// Latch the frame timestamp iced handed over on `RedrawRequested(at)`, and
    /// commit any pending target queued pre-frame.
    ///
    /// Non-`update` resolvers (layout, draw, mouse, operate, overlay) must
    /// call this from `Widget::update` on every `RedrawRequested`, then read
    /// `sampled_*` — never re-sample `Instant::now()`. One frame's layout and
    /// draw against two wall-clock instants apart by the tree's inter-visit
    /// cost turns app business into motion judder. Pre-first-frame flips
    /// queued by [`set_target_at_frame`](Self::set_target_at_frame) start here,
    /// anchored to `at` — the earliest real timestamp. No `Instant::now()` on
    /// mount/resize; the transition runs instead of snapping.
    pub fn note_frame(&mut self, at: iced::time::Instant) {
        self.presented_progress.set(None);
        self.frame_at.set(Some(at));
        self.commit_queued_flip(at);
    }

    /// Start a queued [`Self::set_target_at_frame`] flip at a real frame
    /// instant.
    ///
    /// Samples the replaced travel at `at`, this frame's instant, and travels
    /// from that value to the queued target, so the first reversed frame
    /// continues the old travel at the same timestamp instead of holding the
    /// last presented value for a frame. The queued presented value is only
    /// what [`Self::sampled_progress`] reports until this commit.
    fn commit_queued_flip(&mut self, at: iced::time::Instant) {
        if self.commit_after_observed && self.reduced.get().is_none() {
            return;
        }
        let Some(_presented) = self.pending_from.take() else {
            return;
        };
        self.pending_spatial = None;
        // Continue from where the replaced travel is at this frame's instant, which
        // is where this frame would have drawn it.
        let (from, spatial) = match self.expressive.as_ref() {
            Some(travel) => {
                let sample = travel.sample(at);
                (sample.bounded, sample.spatial)
            }
            None => {
                let progress = self.anim.interpolate(0.0, 1.0, at).clamp(0.0, 1.0);
                (progress, progress)
            }
        };
        if self.expressive.is_some() {
            let scale = self.expressive_scale();
            let target = self.target;
            if let Some(travel) = self.expressive.as_mut() {
                travel.retarget_from(spatial, target, at, &scale);
            }
            self.layout_at_target = false;
            self.note_retarget();
            return;
        }

        self.applied_ms = self.effective_ms();
        self.applied_easing = self.easing.get();
        self.anim = anim_holding(from, at, self.applied_ms, self.applied_easing);
        self.anim.go_mut(self.target, at);
        self.layout_at_target = false;
        // The queued flip becomes a real travel here, anchored to this frame
        // instant: a new generation for the lane.
        self.note_retarget();
    }

    /// The frame timestamp last latched by [`Self::note_frame`], if any.
    #[must_use]
    pub fn frame_at(&self) -> Option<iced::time::Instant> {
        self.frame_at.get()
    }

    /// The value frozen while a flip is queued: the last frame's presented
    /// progress, which [`sampled_progress`](Self::sampled_progress) reports
    /// until the flip commits.
    ///
    /// The committed travel does not start here. The commit samples the
    /// replaced travel at the committing frame's instant and reverses from
    /// that value, which differs from this one whenever time passed since the
    /// last presented frame.
    #[must_use]
    pub fn queued_from(&self) -> Option<f32> {
        self.pending_from
    }

    /// [`Self::progress`] resolved at the latched frame instant.
    ///
    /// Before any frame has been noted, returns the settled-at-target value
    /// (`0.0` hidden, `1.0` visible) — the defined pre-first-frame phase; do
    /// not silently fall back to `Instant::now()` here, which would make
    /// layout and paint sample different clocks on mount.
    #[must_use]
    pub fn sampled_progress(&self) -> f32 {
        // A queued flip has not started travelling yet: what is on screen is
        // the value the last frame presented, which is exactly what the queue
        // recorded. Reporting the *new* target's endpoint here would make the
        // flip snap in layout one frame before it animates in paint.
        if let Some(from) = self.pending_from {
            return from;
        }
        if let Some(progress) = self.presented_progress.get() {
            return progress;
        }
        match self.frame_at.get() {
            Some(now) => self.progress(now),
            None => {
                // Pre-first-frame: no frame timestamp has arrived, so the
                // animation has not advanced. Return the exact endpoint value.
                // Never fall back to `Instant::now()` here; doing so would
                // bypass the frame latch.
                if self.target { 1.0 } else { 0.0 }
            }
        }
    }

    /// [`Self::spatial_progress`] resolved at the latched frame instant: the
    /// value translation reads. Equal to [`Self::sampled_progress`] unless an
    /// expressive token lets the travel overshoot.
    #[must_use]
    pub fn sampled_spatial_progress(&self) -> f32 {
        if self.expressive.is_none() {
            return self.sampled_progress();
        }
        if let Some(from) = self.pending_from {
            return self.pending_spatial.unwrap_or(from);
        }
        match self.frame_at.get() {
            Some(now) => self.spatial_progress(now),
            None => {
                if self.target {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }

    /// [`Self::is_animating`] resolved at the latched frame instant.
    /// A queued pre-first-frame flip counts as animating so the redraw
    /// gate keeps requesting frames until the first `RedrawRequested`
    /// arrives and commits the transition.
    #[must_use]
    pub fn sampled_animating(&self) -> bool {
        if self.pending_from.is_none()
            && let Some(progress) = self.presented_progress.get()
        {
            return if self.target {
                progress < 1.0 - f32::EPSILON
            } else {
                progress > f32::EPSILON
            };
        }
        // A queued flip counts as animating *whether or not* a frame has been
        // latched: the redraw gate is what produces the frame that commits it,
        // so reporting `false` here would leave the travel queued forever on a
        // UI that was otherwise idle -- the snap turned into a freeze.
        self.pending_from.is_some()
            || match self.frame_at.get() {
                Some(now) => self.is_animating(now),
                None => false,
            }
    }

    pub(super) fn note_presented_progress(&self, progress: f32) {
        if progress.is_finite() && (0.0..=1.0).contains(&progress) {
            self.presented_progress.set(Some(progress));
        }
    }

    /// [`Self::needs_layout`] resolved at the latched frame instant.
    #[must_use]
    pub fn sampled_needs_layout(&self) -> bool {
        self.pending_from.is_some()
            || match self.frame_at.get() {
                Some(now) => self.needs_layout(now),
                None => !self.layout_at_target,
            }
    }

    /// Queue a target flip for the frame that will carry it.
    ///
    /// Never started against the latched instant (the PREVIOUS frame's —
    /// arbitrarily old after idle UI: the next frame samples at/past the end)
    /// nor `Instant::now()` (advances the outgoing trajectory past the
    /// last-seen value). The queue records the last frame's PRESENTED progress;
    /// [`note_frame`](Self::note_frame) commits at the frame's real timestamp —
    /// the instant this frame samples. Pre-first-frame is not special: the
    /// queue waits for its first frame. While queued, `sampled_progress` holds
    /// the presented value and `sampled_animating` reports `true`, so the
    /// redraw gate makes the committing frame.
    ///
    /// # Policy is synchronised first, and unconditionally
    ///
    /// Motion policy latches in `draw`; only internal `sync_policy` APPLIES it.
    /// This once early-returned on unchanged targets, so a targetless policy
    /// change never applied: in-flight travels kept old duration/easing after
    /// reduced-motion switched on, and settled-hidden `Size` reveals kept
    /// provisional policy while their target held. Every view rebuild calls
    /// this — synchronising pre-early-return is what carries observed policy
    /// to the default path, not just an edge.
    pub fn set_target_at_frame(&mut self, visible: bool) {
        if visible == self.target {
            // Policy-only resync: no target edge carries it, so before this
            // early return is its only application site. `sync_policy` reports
            // rebuilds; a rebuild asserts "layout for this target committed" —
            // true on an edge (re-opens layout below), false here. An arriving
            // `Size` wrapper at zero progress has reserved no band: swallowing
            // its invalidation paints zero-area and jumps the travel to the
            // endpoint with no intermediate frame. Owed stays owed.
            let layout_still_owed = !self.layout_at_target;

            if self.sync_policy() {
                if layout_still_owed {
                    self.layout_at_target = false;
                }

                // `sync_policy` knows nothing about a QUEUED flip. Under a
                // policy that now forbids motion that queue must not be
                // committed as a travel on the next frame: snap it here
                // instead, so reduced motion takes effect on the frame it was
                // observed rather than one frame later.
                if self.effective_ms() <= f32::EPSILON && self.pending_from.take().is_some() {
                    self.layout_at_target = true;
                    self.anim = build_anim(self.target, 0.0, self.easing.get());
                    self.settle_expressive();
                    // Whatever the lane is presenting is no longer this
                    // widget's travel.
                    self.note_retarget();
                }
            }

            return;
        }
        self.sync_policy();
        // Reduced/unobserved-conservative: no transition — snap now, but
        // leave `hidden_notified` reset toward hidden so a caller-side
        // completion receipt (`Msg::Settled(..)`) still fires next
        // non-frame event. `settle_at` sets it unconditionally: right
        // for a RESPONSIVE re-base, wrong for a USER-VISIBLE reduced-
        // motion dismissal (see the modal_host dismissal test).
        if self.effective_ms() <= f32::EPSILON {
            self.target = visible;
            self.hidden_notified = false;
            self.layout_at_target = true;
            self.anim = build_anim(visible, 0.0, self.easing.get());
            self.pending_from = None;
            self.settle_expressive();
            return;
        }
        // Real transition: queue it at the presented value. If a queue is
        // already pending, its recorded value is still the last thing painted
        // -- keep it rather than overwriting it with a value nobody drew.
        if self.pending_from.is_none() {
            self.pending_spatial = Some(self.sampled_spatial_progress());
            self.pending_from = Some(self.sampled_progress());
        }
        self.note_retarget();
        self.target = visible;
        self.hidden_notified = false;
        self.layout_at_target = false;
        // If the caller queues, then re-queues back to the endpoint the queue
        // was taken at, the transition never has to happen at all.
        // An expressive queue mid-overshoot is not at the endpoint, though
        // its bounded value reads it: only the spatial value can say.
        let at_endpoint = |value: f32, target: bool| {
            if target {
                (value - 1.0).abs() <= f32::EPSILON || (value >= 1.0 && self.expressive.is_none())
            } else {
                value.abs() <= f32::EPSILON || (value <= 0.0 && self.expressive.is_none())
            }
        };
        if let Some(from) = self.pending_from
            && at_endpoint(self.pending_spatial.unwrap_or(from), self.target)
        {
            self.pending_from = None;
            // The screen still shows that endpoint, even if a policy resync
            // rebuilt the animation at the target the queue was heading to.
            self.anim = build_anim(self.target, 0.0, self.easing.get());
            self.settle_expressive();
        }
    }

    /// [`Self::note_layout`] at the latched frame instant. No-op pre-frame.
    pub fn note_layout_at_frame(&mut self) {
        if let Some(now) = self.frame_at.get() {
            self.note_layout(now);
        }
    }

    /// Consume the one completion receipt for a visible-to-hidden edge,
    /// resolved at the latched frame instant.
    ///
    /// Falls through to the sampled endpoint pre-first-frame: a reduced-motion
    /// snap (instant-settled at hidden, no `RedrawRequested` yet) still
    /// publishes its one receipt on the next non-frame event — the modal host
    /// needs it to fire `Msg::Settled(..)` on reduced-motion dismissals.
    pub fn take_hidden_completion_at_frame(&mut self) -> bool {
        if !self.target
            && !self.hidden_notified
            && !self.sampled_animating()
            && self.sampled_progress() <= f32::EPSILON
        {
            self.hidden_notified = true;
            true
        } else {
            false
        }
    }

    /// Construct a transition settled at `visible`.
    ///
    /// This compatibility spelling is equivalent to [`Self::at_target`].
    #[must_use]
    pub fn new(visible: bool) -> Self {
        Self::at_target(visible)
    }

    /// Hold queued flips until [`Self::observe_theme`] has run once.
    pub(super) fn commit_after_observed(mut self) -> Self {
        self.commit_after_observed = true;
        self
    }

    /// Latch the theme policy without applying its lossy reduction.
    pub fn observe_theme(&self, theme: &iced::Theme) {
        let motion = crate::theme_lookup::motion(theme);
        self.raw_normal_ms.set(motion.normal_ms);
        self.reduced.set(Some(motion.reduced_motion));
        self.easing.set(motion.easing_or(None));
        self.ink_margin.set(crate::elevation::ink_margin(
            &crate::theme_lookup::elevation(theme),
        ));
    }

    /// How far a descendant's ink may reach past the wrapper's bounds: the
    /// largest elevation shadow of the theme last observed.
    pub(crate) fn ink_margin(&self) -> iced::Padding {
        self.ink_margin.get()
    }

    fn effective_ms(&self) -> f32 {
        match self.reduced.get() {
            Some(false) => self.raw_normal_ms.get(),
            None if self.animate_if_unobserved => self.raw_normal_ms.get(),
            Some(true) | None => {
                // Resolve through the semantic instant token instead of
                // inventing a second numeric representation of "no motion".
                MotionScale::default().ms(MotionToken::Instant)
            }
        }
    }

    /// Returns whether it actually rebuilt, so a caller that is *not* on a
    /// target edge can tell a real policy change from the no-op that every
    /// frame of a steady theme produces.
    fn sync_policy(&mut self) -> bool {
        let effective = self.effective_ms();
        let easing = self.easing.get();
        if (effective - self.applied_ms).abs() < f32::EPSILON && easing == self.applied_easing {
            return false;
        }
        self.applied_ms = effective;
        self.applied_easing = easing;
        self.anim = build_anim(self.target, effective, easing);
        self.settle_expressive();
        self.layout_at_target = true;
        // Rebuilding REPLACES the owned travel; the lane keeps presenting
        // its held curve until the generation changes. Without this a
        // reduced-motion switch settles the widget while the lane animates the
        // abandoned curve — state and screen disagreeing for the travel. Bump
        // here, not per caller: no policy path can forget it.
        self.note_retarget();

        true
    }

    /// Settle immediately at a target without manufacturing a transition.
    ///
    /// For responsive compositions discovering provisional startup geometry
    /// was not the painted geometry: no visibility change to animate — re-base
    /// at the first real target. Later user/model edges still animate via
    /// [`set_target`](Self::set_target).
    pub fn settle_at(&mut self, visible: bool) {
        self.sync_policy();
        // A snap is a retarget for the lane: whatever it was presenting is no
        // longer the travel this widget owns.
        self.note_retarget();
        self.target = visible;
        self.hidden_notified = !visible;
        self.anim = build_anim(visible, self.applied_ms, self.applied_easing);
        self.layout_at_target = true;
        // A snap clears any queued pre-first-frame flip: there is no
        // transition to commit later.
        self.pending_from = None;
        self.settle_expressive();
    }

    /// Change the target, beginning a `MotionToken::Normal` transition when
    /// the observed policy permits motion.
    ///
    /// # Staggering several transitions on one clock
    ///
    /// `now` is the CALLER's instant: staggering needs no sequencing primitive —
    /// start all items one frame, push each `now` into the future
    /// (`frame + i * step`). A future-dated transition holds origin yet reports
    /// [`is_animating`](Self::is_animating) (frames keep coming until its
    /// turn), and spacing stays exactly `step` — all derived from one instant.
    /// The first-reached alternative, starting `i + 1` where `i` actually
    /// started, quantizes once per item (measured: eight items, 60 ms step, 900 ms
    /// duration: one offset instant leaves a flat −0.0008 residual, no RELATIVE
    /// drift; chained scheduling reached 30.5 ms start error, 0.034 travel by
    /// item eight). [`Motion`] cannot express this (its `visible` interface
    /// owns the latch, offers no per-item offset): owning the [`PanelMotion`]
    /// buys the shared clock.
    pub fn set_target(&mut self, visible: bool, now: iced::time::Instant) {
        self.sync_policy();
        // An explicitly named instant supersedes any queue: this caller has
        // told us which frame the travel belongs to, so commit against it here
        // rather than waiting for a `note_frame` the caller may never make
        // (`app_shell` drives its own clock and never calls one).
        self.commit_queued_flip(now);
        if visible != self.target {
            // Retarget/reversal: the lane must replace the travel it is
            // presenting rather than continue it.
            self.note_retarget();
            self.target = visible;
            self.hidden_notified = false;
            if self.expressive.is_some() {
                let scale = self.expressive_scale();
                if let Some(travel) = self.expressive.as_mut() {
                    travel.set_target(visible, now, &scale);
                }
            } else {
                self.anim.go_mut(visible, now);
            }
            self.layout_at_target = false;
        }
        // The caller named this frame instant: latch it so following
        // `sampled_*` calls this frame resolve against the same instant.
        // Owners like `app_shell::widget` drive `set_target` from their own
        // frame clock; downstream widgets must see neither earlier nor later
        // than the committed one. Driver clock and sampled clock: one clock,
        // not two.
        self.frame_at.set(Some(now));
    }

    /// Retargets from a compositor-confirmed fraction rather than elapsed UI time.
    ///
    /// The caller must obtain `from` from the current presented layout lease and use the
    /// current retarget instant for `at`. Invalid fractions leave all motion state unchanged;
    /// an unchanged target does not restart travel. Reduced motion still settles immediately.
    pub fn set_target_from_presented(
        &mut self,
        visible: bool,
        from: f32,
        at: iced::time::Instant,
    ) -> bool {
        if !from.is_finite() || !(0.0..=1.0).contains(&from) {
            return false;
        }
        self.sync_policy();
        if self.target == visible {
            return true;
        }
        self.pending_from = None;
        self.pending_spatial = None;
        self.applied_ms = self.effective_ms();
        self.applied_easing = self.easing.get();
        self.target = visible;
        if self.expressive.is_some() {
            let scale = self.expressive_scale();
            if let Some(travel) = self.expressive.as_mut() {
                travel.retarget_from(from, visible, at, &scale);
            }
        } else {
            self.anim = anim_holding(from, at, self.applied_ms, self.applied_easing);
            self.anim.go_mut(visible, at);
        }
        self.hidden_notified = false;
        self.layout_at_target = false;
        self.frame_at.set(Some(at));
        self.note_retarget();
        true
    }

    pub(crate) fn can_shift_clock(&self, delay: iced::time::Duration) -> bool {
        (self.expressive.is_some() || self.anim.can_shift_clock(delay))
            && self
                .frame_at
                .get()
                .is_none_or(|at| at.checked_add(delay).is_some())
    }

    pub(crate) fn shift_clock(&mut self, delay: iced::time::Duration) -> bool {
        if !self.can_shift_clock(delay) {
            return false;
        }
        let shifted = match self.expressive.as_mut() {
            Some(travel) => travel.shift(delay),
            None => self.anim.shift_clock(delay),
        };
        if !shifted {
            return false;
        }
        self.frame_at
            .set(self.frame_at.get().and_then(|at| at.checked_add(delay)));
        true
    }

    /// Eased visibility fraction in `0..=1`.
    #[must_use]
    pub fn progress(&self, now: iced::time::Instant) -> f32 {
        match self.expressive.as_ref() {
            Some(travel) => travel.sample(now).bounded,
            None => self.anim.interpolate(0.0, 1.0, now).clamp(0.0, 1.0),
        }
    }

    /// The value a translation reads at `now`: [`Self::progress`], except
    /// that an expressive spatial token may carry it past `1` (or below `0`)
    /// and back before it settles. Never use it for clip, reservation,
    /// opacity, input or completion.
    #[must_use]
    pub fn spatial_progress(&self, now: iced::time::Instant) -> f32 {
        match self.expressive.as_ref() {
            Some(travel) => travel.sample(now).spatial,
            None => self.progress(now),
        }
    }

    /// The duration the animation this motion is currently running was built
    /// with, after motion policy and reduced motion were applied.
    ///
    /// Read by [`capture_hold`](crate::app_shell::capture_hold): an
    /// exact-fraction pin fractions this travel, not a harness-assumed
    /// duration. Valid once [`set_target`](Self::set_target)/[`settle_at`](Self::settle_at)
    /// synchronised policy; zero before the first.
    pub(crate) fn applied_travel(&self) -> iced::time::Duration {
        if let Some(travel) = self.expressive.as_ref() {
            return travel.duration();
        }
        iced::time::Duration::from_secs_f32((self.applied_ms.max(0.0)) / 1000.0)
    }

    /// Whether another frame can change the fraction.
    #[must_use]
    pub fn is_animating(&self, now: iced::time::Instant) -> bool {
        match self.expressive.as_ref() {
            Some(travel) => travel.is_animating(now),
            None => self.anim.is_animating(now),
        }
    }

    /// Record that layout consumed the current fraction.
    pub fn note_layout(&mut self, now: iced::time::Instant) {
        let target = if self.target { 1.0 } else { 0.0 };
        self.layout_at_target = (self.progress(now) - target).abs() < f32::EPSILON;
    }

    /// A late final frame still owes one layout invalidation even after the
    /// animation clock itself reports settled.
    #[must_use]
    pub fn needs_layout(&self, now: iced::time::Instant) -> bool {
        self.is_animating(now) || !self.layout_at_target
    }

    /// The requested end state.
    #[must_use]
    pub fn target(&self) -> bool {
        self.target
    }

    /// This travel's generation, which changes on every retarget.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Marks the travel as retargeted, so the render lane replaces the
    /// animation it is presenting instead of continuing it.
    pub(crate) fn note_retarget(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// How much travel is left at `now`, in either direction.
    ///
    /// `Animation::remaining` interpolates `duration -> 0` TOWARDS `true`: on a
    /// hide it returns the mirror — growing zero→full while the panel finishes.
    /// A collapse must never publish a curve growing ~20 ms/frame: the lane
    /// gets a backwards travel. Bisecting `is_animating` is direction-agnostic,
    /// easing-inverse-free: bracket at the applied duration, doubled for
    /// future-dated (staggered) transitions animating past it; fixed-length
    /// loops, so no pathological curve spins.
    #[must_use]
    pub fn remaining_travel(&self, now: iced::time::Instant) -> iced::time::Duration {
        // Bracketed by the travel actually running: an expressive token's
        // duration is its own, not the theme's `motion-normal`.
        let travel_ms = self.applied_travel().as_secs_f32() * 1000.0;
        remaining_travel_for(now, travel_ms, |at| self.is_animating(at))
    }

    /// Samples the remaining travel into keyframes a render lane can
    /// advance without this widget.
    ///
    /// Cache vs lane: a sampled track is one UI-thread instant (a blocked UI
    /// thread freezes it); these keyframes describe the whole REST of the
    /// journey — the lane continues on the presentation clock. Sampled from
    /// this animation: theme easing, reduced motion, shortened duration,
    /// mid-flight reversal all baked in — the lane never learns easing.
    /// `None` when nothing remains, or pre-first-latched-frame: a lane must
    /// never hold a travel anchored to an unread clock.
    #[must_use]
    pub fn track_curve(&self, samples: usize) -> Option<TrackCurve> {
        self.track_curve_at(samples, self.frame_at.get()?)
    }

    /// [`Self::track_curve`], anchored at the caller's own frame instant
    ///
    ///
    /// Owners driving their own clock (`app_shell`) sample progress at the
    /// instant the prepared frame will paint. Anchoring at the LATCHED instant
    /// lags one frame: the lane starts where the previous frame was — measured
    /// 39.9 px backwards at hand-over, a visible snap in a 260 px drawer. One
    /// clock for sample and curve, or neither is honest. The "no frame read
    /// yet" refusal stays: callers may name the instant, never conjure travel
    /// pre-first-frame.
    #[must_use]
    pub fn track_curve_at(&self, samples: usize, now: iced::time::Instant) -> Option<TrackCurve> {
        let _ = self.frame_at.get()?;
        let remaining = self.remaining_travel(now);

        if remaining <= iced::time::Duration::ZERO || samples < 1 {
            return None;
        }

        // A spring still oscillating needs keyframes at its own rate.
        let samples = match self.expressive.as_ref() {
            Some(Travel::Spring(travel)) => samples.max(travel.track_samples(now)),
            _ => samples,
        };
        let seconds = remaining.as_secs_f32();
        let instant = |index: usize| {
            let at = index as f32 / samples as f32;
            (at, now + iced::time::Duration::from_secs_f32(seconds * at))
        };
        let points = (0..=samples)
            .map(|index| {
                let (at, instant) = instant(index);
                (at, self.progress(instant))
            })
            .collect();
        // The spatial channel is listed only when it can differ.
        let spatial_points = self.expressive.is_some().then(|| {
            (0..=samples)
                .map(|index| {
                    let (at, instant) = instant(index);
                    (at, self.spatial_progress(instant))
                })
                .collect()
        });

        Some(TrackCurve {
            start: now,
            duration: remaining,
            points,
            spatial_points,
            generation: self.generation(),
        })
    }
}

/// The opted-in travel of a [`PanelMotion`]: an expressive curve or a spring.
#[derive(Debug, Clone, PartialEq)]
enum Travel {
    Curve(super::ExpressiveTravel),
    Spring(super::SpringTravel),
}

impl Travel {
    fn settle(&mut self, target: bool) {
        *self = match self {
            Self::Curve(travel) => {
                Self::Curve(super::ExpressiveTravel::settled(travel.motion(), target))
            }
            Self::Spring(travel) => {
                Self::Spring(super::SpringTravel::settled(travel.spring(), target))
            }
        };
    }

    fn set_target(&mut self, target: bool, now: iced::time::Instant, scale: &MotionScale) {
        match self {
            Self::Curve(travel) => travel.set_target(target, now, scale),
            Self::Spring(travel) => travel.set_target(target, now, scale),
        }
    }

    fn retarget_from(
        &mut self,
        from: f32,
        target: bool,
        at: iced::time::Instant,
        scale: &MotionScale,
    ) {
        match self {
            Self::Curve(travel) => travel.retarget_from(from, target, at, scale),
            Self::Spring(travel) => travel.retarget_from(from, target, at, scale),
        }
    }

    fn shift(&mut self, delay: iced::time::Duration) -> bool {
        match self {
            Self::Curve(travel) => travel.shift(delay),
            Self::Spring(travel) => travel.shift(delay),
        }
    }

    fn sample(&self, now: iced::time::Instant) -> super::ExpressiveSample {
        match self {
            Self::Curve(travel) => {
                let mut sample = travel.sample(now);
                // Leaving stops at hidden: past it the panel would only show
                // a blank frame or peek back in.
                if !travel.target() {
                    sample.spatial = sample.spatial.max(0.0);
                }
                sample
            }
            Self::Spring(travel) => travel.sample(now),
        }
    }

    fn is_animating(&self, now: iced::time::Instant) -> bool {
        match self {
            // A leaving curve is done when it reaches hidden, before the part
            // of its curve that would carry it past and back.
            Self::Curve(travel) if !travel.target() => {
                travel.is_animating(now) && travel.sample(now).spatial > 1e-4
            }
            Self::Curve(travel) => travel.is_animating(now),
            Self::Spring(travel) => travel.is_animating(now),
        }
    }

    fn duration(&self) -> iced::time::Duration {
        match self {
            Self::Curve(travel) => travel.duration(),
            Self::Spring(travel) => travel.duration(),
        }
    }
}

/// The remaining travel of a [`PanelMotion`], sampled for a render lane
#[derive(Debug, Clone)]
pub struct TrackCurve {
    /// The frame instant the curve is anchored to.
    pub start: iced::time::Instant,
    /// How much of the travel is left.
    pub duration: iced::time::Duration,
    /// `(normalised time, eased progress)` pairs, ascending, each progress
    /// within `[0, 1]`: what clip, reservation and opacity read.
    pub points: Vec<(f32, f32)>,
    /// The same instants' spatial values, when an expressive token lets them
    /// overshoot; `None` when they equal `points`. What translation reads.
    pub spatial_points: Option<Vec<(f32, f32)>>,
    /// The travel this curve belongs to, incremented on every retarget or
    /// reversal.
    ///
    /// Travels with the curve, never read separately: a publisher may hold a
    /// curve sampled an earlier frame (`PanelReveal` reads a shared cell) —
    /// pairing keeps a stale curve's own stale generation instead of a fresh
    /// one mimicking a lane-restarting retarget.
    pub generation: u64,
}

/// One shared clock for a [`PanelReveal`]: the sampled instant and the
/// remaining travel.
///
/// Overlay reveals and ordinary Reserve reveals carry the remaining travel.
/// Such a Reserve commits its widest first geometry and hands the lane a clip
/// curve, so its sampled width does not force application layout on every
/// animation frame. A reveal marked [`responsive_reserve`](Self::responsive_reserve)
/// deliberately publishes no compositor curve of its own: its sampled width
/// is spent by its owner's local retained layout instead.
#[derive(Debug, Clone)]
pub(crate) struct RevealClock {
    pub(crate) layout_driver: Option<iced::advanced::renderer::NodeId>,
    /// The instant's spatial value, which an expressive travel may carry past
    /// its target; `None` reads the bounded progress.
    pub(crate) spatial: Option<std::rc::Rc<std::cell::Cell<f32>>>,
    pub(crate) progress: std::rc::Rc<std::cell::Cell<f32>>,
    pub(crate) travel: Option<std::rc::Rc<std::cell::RefCell<Option<TrackCurve>>>>,
    /// Whether this `Reserve` reveal is a live resize boundary for the content
    /// beside it.
    ///
    /// Default keeps compositor-pinned size motion: one widest reservation,
    /// then a lane-owned clip travel. Compositions whose adjacent content must
    /// reflow continuously (flexible table columns, collapsing breadcrumbs)
    /// opt in: a destination-pinned reservation cannot give descendants
    /// responsive geometry — only early-revealed endpoint pixels.
    pub(crate) responsive_reserve: bool,
    /// Where a travelling responsive reservation names the scene node that
    /// records its whole panel, for the seam group moving it to unclip it on
    /// the render lane.
    pub(crate) retained_panel:
        Option<std::rc::Rc<std::cell::Cell<Option<iced::advanced::renderer::NodeId>>>>,
    /// The reservation fraction a `Reserve` reveal is holding, or `None` when
    /// it is settled.
    ///
    /// The pinned reservation is the geometry every sibling laid out against —
    /// seam-riding chrome (divider, stable toggle) must know which fraction
    /// its layout describes before compositing the difference. Published here
    /// so riders correct for the ACTUAL reserved pin, never a guessed
    /// "a travel exists".
    pub(crate) pin: Option<std::rc::Rc<std::cell::Cell<Option<f32>>>>,
}

/// How finely a travel is sampled for the render lane.
///
/// 24 points over a 180 ms transition is one sample per 7.5 ms -- finer than a
/// 60 Hz refresh period, so the lane interpolates between neighbours that are
/// never more than one frame apart and the curve it presents is the curve the
/// effect declared.
pub(crate) const TRACK_SAMPLES: usize = 24;

/// The supported public clock for a generic [`PanelReveal`]: the sampled
/// instant and the remaining travel the render lane advances.
///
/// A [`PanelReveal`] owns no cadence — its owner does. A bare progress sample
/// is one UI-thread instant, leaving `lane::Conditions::live_animation` false:
/// the surface never hands over; every reveal frame waits for the UI thread.
/// This handle carries both halves and is the only public supplier of either —
/// the lane-capable form is the default, never a forgotten opt-in (as the
/// crate-internal shell clock carrying both).
///
/// No backend texture, renderer state or scene node exposed: the caller
/// supplies a curve sampled from its own [`PanelMotion`]; the reveal decides
/// what the fraction composites. Driven-from-here means generic NON-responsive
/// [`Reserve`](PanelRevealMode::Reserve) (or [`Overlay`](PanelRevealMode::Overlay)):
/// widest reservation committed once, revealed inside by a compositor-owned
/// retained clip. The responsive docked shell — layout-driven, curve-free — is
/// deliberately unreachable through this type.
///
/// # Cloning
///
/// A clone SHARES the clock: two co-travelling reveals (a split band's two
/// segments) drive from one `advance`. Never a second independent clock — for
/// that, construct another [`PanelRevealClock`] beside its own [`PanelMotion`].
#[derive(Debug, Clone, Default)]
pub struct PanelRevealClock {
    layout_driven: bool,
    target: std::rc::Rc<std::cell::Cell<bool>>,
    progress: std::rc::Rc<std::cell::Cell<f32>>,
    spatial: std::rc::Rc<std::cell::Cell<f32>>,
    travel: std::rc::Rc<std::cell::RefCell<Option<TrackCurve>>>,
}

impl PanelRevealClock {
    /// A clock holding a settled reveal: progress zero, no travel.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserve the current eased extent so adjacent content reflows during travel.
    ///
    /// Defaults to false, which pins the widest reservation until settlement.
    /// Applies only to `Reserve` panels. The owner advances this clock and
    /// rebuilds layout on each animation frame; the reveal adds no redraw cadence.
    /// Departing descendants become inert as soon as the target closes.
    #[must_use]
    pub fn layout_driven(mut self, enabled: bool) -> Self {
        self.layout_driven = enabled;
        self
    }

    /// Publish the eased/spatial fractions, target and remaining travel at `now`.
    ///
    /// Call on each owner frame to follow the motion's theme easing, reduced
    /// motion and reversals.
    pub fn advance(&self, motion: &PanelMotion, now: iced::time::Instant) {
        self.target.set(motion.target());
        self.progress.set(motion.progress(now));
        self.spatial.set(motion.spatial_progress(now));
        self.travel
            .replace(motion.track_curve_at(TRACK_SAMPLES, now));
    }

    /// The spatial value last published: what an overlay's translation
    /// reads. Equal to [`Self::progress`] unless the motion follows an
    /// expressive token.
    #[must_use]
    pub fn spatial_progress(&self) -> f32 {
        self.spatial.get()
    }

    /// The fraction last published, which is what a reveal reading this clock
    /// presents.
    #[must_use]
    pub fn progress(&self) -> f32 {
        self.progress.get()
    }

    fn shared(&self) -> RevealClock {
        RevealClock {
            layout_driver: None,
            spatial: Some(std::rc::Rc::clone(&self.spatial)),
            progress: std::rc::Rc::clone(&self.progress),
            travel: Some(std::rc::Rc::clone(&self.travel)),
            pin: None,
            responsive_reserve: self.layout_driven,
            retained_panel: None,
        }
    }
}

/// Publishes a shared travel curve as a renderer-owned animation.
///
/// Differs from [`publish_curve`] only in curve source: `Motion`/`CrossFade`
/// own their [`PanelMotion`]; a [`PanelReveal`] is handed caller-owned progress
/// and travel. Both end at the same `publish_track_animation`: reveals reach
/// the lane on exactly every other family's terms.
pub(crate) fn publish_travel<Message>(
    shell: &mut Shell<'_, Message>,
    node: iced::advanced::renderer::NodeId,
    curve: &TrackCurve,
    value: impl Fn(f32) -> iced::advanced::renderer::Track,
) {
    publish_travel_channels(shell, node, curve, |bounded, _spatial| value(bounded));
}

/// [`publish_travel`] for a track that reads both channels: each keyframe is
/// built from the bounded progress and the spatial one at the same instant,
/// so translation can overshoot while clip and opacity stay bounded.
pub(crate) fn publish_travel_channels<Message>(
    shell: &mut Shell<'_, Message>,
    node: iced::advanced::renderer::NodeId,
    curve: &TrackCurve,
    value: impl Fn(f32, f32) -> iced::advanced::renderer::Track,
) {
    if curve.points.len() < 2 || curve.duration <= iced::time::Duration::ZERO {
        // `TrackAnimation::is_usable` would refuse it anyway; refusing here
        // keeps an unusable animation out of the published set entirely, so a
        // lane never has to distinguish "malformed" from "settled".
        return;
    }

    shell.publish_track_animation(iced::advanced::renderer::TrackAnimation {
        node,
        start: curve.start,
        duration: curve.duration,
        keyframes: curve
            .points
            .iter()
            .enumerate()
            .map(|(index, (at, progress))| {
                let spatial = curve
                    .spatial_points
                    .as_ref()
                    .and_then(|points| points.get(index))
                    .map_or(*progress, |(_, spatial)| *spatial);
                (*at, value(*progress, spatial))
            })
            .collect(),
        generation: curve.generation,
    });
}

/// How [`PanelReveal`] spends its progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelRevealMode {
    /// The wrapper's extent along its edge's axis grows/shrinks, so its
    /// sibling's geometry changes.
    Reserve,
    /// The wrapper keeps its resting extent while its retained child translates
    /// across the edge it enters from, leaving the pane below unchanged.
    Overlay,
}

/// Which window edge a [`PanelReveal`] is docked against and travels from
///
///
/// Travel direction is STATED, never a hardcoded `-bounds.width * (1 - p)`
/// sign: a left drawer enters from the left; an RTL shell's right drawer from
/// the right — never flying across the window over unrelated content. The edge
/// also declares the travel AXIS: top/bottom bars are the same composition, not
/// a second widget — a vertical edge spends the reveal on panel height
/// (resting extent = height), clipping/translating along y, horizontal keeping
/// width behaviour. [`Leading`](PanelRevealEdge::Leading) is the default:
/// existing consumers keep left-edge travel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelRevealEdge {
    /// Enter from the left edge: the child starts one width to the left of
    /// its resting rectangle and slides right. This is the default.
    #[default]
    Leading,
    /// Enter from the right edge: the child starts one width to the right
    /// of its resting rectangle and slides left. What an RTL drawer needs.
    Trailing,
    /// Enter from the top edge: the child starts one height above its
    /// resting rectangle and slides down. An attached top bar or a workspace
    /// overview that grows downwards out of it.
    Top,
    /// Enter from the bottom edge: the child starts one height below its
    /// resting rectangle and slides up. An attached bottom bar or a dock
    /// panel that grows upwards out of it.
    Bottom,
}

impl PanelRevealEdge {
    /// The signed multiple of the child's travel extent to offset it by at
    /// progress 0.
    ///
    /// A sign, not a draw-site `match`: travel directions cannot drift, and
    /// each mirror is one test-reasonable negation. The extent is child width
    /// on horizontal edges, height on vertical ([`is_vertical`](Self::is_vertical)).
    #[must_use]
    pub fn offset_sign(self) -> f32 {
        match self {
            PanelRevealEdge::Leading | PanelRevealEdge::Top => -1.0,
            PanelRevealEdge::Trailing | PanelRevealEdge::Bottom => 1.0,
        }
    }

    /// Whether this edge's travel runs along y.
    ///
    /// One predicate answers every axis question the reveal asks -- which
    /// dimension the resting extent describes, which one the reservation and
    /// the clip spend, and which component the overlay translation carries --
    /// so no two of them can disagree about what a top panel is.
    #[must_use]
    pub fn is_vertical(self) -> bool {
        matches!(self, PanelRevealEdge::Top | PanelRevealEdge::Bottom)
    }
}

/// A retained-child edge reveal used by panel compositions.
///
/// The dock edge ([`PanelRevealEdge`]) chooses the axis: leading/trailing
/// reveal across width, top/bottom across height. All else — caller-owned
/// clock, pinned reservation, compositor-animated retained clip, hit/operation
/// boundary — is one composition on either axis. The child always measures at
/// resting extent (no reflow from stable width); `app_shell` replays that
/// fixed-width layout node while travelling, so only the O(1) wrapper extent
/// changes. Replacements/ordinary input do one real layout before replay is
/// allowed (the shell's `RetainedResponsiveWidth` correctness boundary). Child
/// overlays suppress at every partial progress: an iced overlay roots outside
/// the wrapper's layer and cannot clip to the animated reservation. At progress
/// zero all lifecycle delegation stops (no update/draw/overlay/focus/a11y-op
/// reaches the hidden child); the parent may remove the wrapper after the
/// final layout receipt.
pub struct PanelReveal<'a, Message> {
    layout_driver: Option<iced::advanced::renderer::NodeId>,
    child: Element<'a, Message>,
    progress: f32,
    live_progress: Option<std::rc::Rc<std::cell::Cell<f32>>>,
    input_target: Option<std::rc::Rc<std::cell::Cell<bool>>>,
    /// The live spatial value, when the clock carries one; see
    /// [`PanelRevealClock`].
    live_spatial: Option<std::rc::Rc<std::cell::Cell<f32>>>,
    /// The REST of the travel, shared with whoever owns the clock.
    ///
    /// `live_progress` is one sampled instant, so an overlay driven only by it
    /// needs the UI thread awake for every frame. This curve lets the render
    /// lane advance the reveal on the presentation clock while the UI thread
    /// is busy.
    live_travel: Option<std::rc::Rc<std::cell::RefCell<Option<TrackCurve>>>>,
    /// Where this reveal publishes the reservation fraction it is holding, so
    /// seam chrome laid out against that reservation can correct for it
    live_pin: Option<std::rc::Rc<std::cell::Cell<Option<f32>>>>,
    /// Where this reveal names its recorded panel while a responsive
    /// reservation travels; see [`RevealClock::retained_panel`].
    live_retained: Option<std::rc::Rc<std::cell::Cell<Option<iced::advanced::renderer::NodeId>>>>,
    /// The settled extent along the edge's axis: a width for a leading or
    /// trailing panel, a height for a top or bottom one.
    resting_extent: Option<f32>,
    mode: PanelRevealMode,
    cache_layout_while_revealing: bool,
    child_laid_out: bool,
    /// The edge the panel is docked against: the axis both modes travel along,
    /// the side an `Overlay` translates in from, and the side a `Reserve` clip
    /// keeps anchored while the opposite seam moves.
    enters_from: PanelRevealEdge,
    /// Spend `Reserve` progress on real layout instead of a retained clip, so
    /// siblings are re-limited on every presented frame. Off by default; see
    /// [`RevealClock::responsive_reserve`].
    responsive_reserve: bool,
    /// How far past its entering edge a travelling child may paint: the gap
    /// to the window's edge, so a rounded pane slides out of the window
    /// instead of being cut straight at its own edge.
    overhang: f32,
}

#[derive(Debug)]
pub(super) struct PanelRevealLayout {
    input_was_enabled: std::cell::Cell<bool>,
    cached: Option<(Size, layout::Node)>,
    // An event dispatched to the child ended with the layout invalidated.
    child_invalidated_layout: std::cell::Cell<bool>,
    /// This reveal's stable compositor scene node, minted with the widget's
    /// tree state so it survives every `view` rebuild.
    node: iced::advanced::renderer::NodeId,
    /// The translation the node was last recorded at, or `None` when the
    /// last paint cannot be replayed (settled, hidden, or reflowing).
    recorded_offset: std::cell::Cell<Option<Vector>>,
    /// The reservation fraction held for the duration of a `Reserve` travel,
    /// or `None` when settled.
    ///
    /// The travel's WIDEST extent — the point: reserved width commits once, up
    /// front, so every sibling reaches end geometry immediately and nothing
    /// reflows until settlement. The reveal is a clip inside a still rectangle
    /// — compositor work, no widget. This is what puts the shipped inline
    /// sidebar (Desktop Text) on the lane: before it, `Reserve` spent progress
    /// on wrapper width — genuine per-frame reflow, lane-never-ownable.
    /// Exposed to the parent `motion` module: the generic-first-frame-pin test
    /// seeds a stale reservation proving responsive reserves ignore it.
    pub(super) reserve_pin: std::cell::Cell<Option<f32>>,
    /// The reservation fraction the last replayable paint was recorded at.
    ///
    /// A clip track NARROWS a recording, so a published clip may never ask for
    /// more ink than this. `None` means the last paint is not a baseline.
    recorded_reserve: std::cell::Cell<Option<f32>>,
    /// Whether an opening travel still owes its single first-geometry commit.
    ///
    /// `layout` may be the phase that latches `reserve_pin`, before
    /// `update_frame` can observe the transition. Keeping this debt beside the
    /// pin makes that ordering irrelevant: the next update spends it once.
    reserve_first_commit_owed: std::cell::Cell<bool>,
    /// Whether the settling frame still owes its single final layout commit.
    reserve_commit_owed: std::cell::Cell<bool>,
}

impl Default for PanelRevealLayout {
    fn default() -> Self {
        Self {
            input_was_enabled: std::cell::Cell::new(false),
            cached: None,
            child_invalidated_layout: std::cell::Cell::new(false),
            node: next_scene_node(),
            recorded_offset: std::cell::Cell::new(None),
            reserve_pin: std::cell::Cell::new(None),
            recorded_reserve: std::cell::Cell::new(None),
            reserve_first_commit_owed: std::cell::Cell::new(false),
            reserve_commit_owed: std::cell::Cell::new(false),
        }
    }
}

impl<'a, Message> PanelReveal<'a, Message> {
    /// Wrap `child` at a resting width and reveal fraction, revealing across
    /// the leading edge.
    ///
    /// `resting_width = None` takes the parent-assigned maximum. Fraction
    /// clamps to `0..=1`. Pair with
    /// [`enters_from`](Self::enters_from)`(`[`Trailing`](PanelRevealEdge::Trailing)`)`
    /// for right-hand panels, [`vertical`](Self::vertical) for top/bottom.
    #[must_use]
    pub fn new(
        child: Element<'a, Message>,
        progress: f32,
        resting_width: Option<f32>,
        mode: PanelRevealMode,
    ) -> Self {
        Self {
            layout_driver: None,
            child,
            progress: progress.clamp(0.0, 1.0),
            live_progress: None,
            input_target: None,
            live_spatial: None,
            live_travel: None,
            live_retained: None,
            live_pin: None,
            resting_extent: resting_width,
            mode,
            cache_layout_while_revealing: false,
            child_laid_out: false,
            enters_from: PanelRevealEdge::Leading,
            responsive_reserve: false,
            overhang: 0.0,
        }
    }

    /// Wrap `child` at a resting height and reveal fraction, revealing
    /// downwards from the top edge.
    ///
    /// The attached top/bottom-bar form of [`new`](Self::new): the reveal
    /// spends on height, so `resting_height = None` takes the parent-assigned
    /// maximum height. Add
    /// [`enters_from`](Self::enters_from)`(`[`Bottom`](PanelRevealEdge::Bottom)`)`
    /// for bottom-bar-upwards growth. All else — caller-owned
    /// [`PanelRevealClock`], pinned reservation, retained clip, hit boundary —
    /// exactly as horizontal.
    #[must_use]
    pub fn vertical(
        child: Element<'a, Message>,
        progress: f32,
        resting_height: Option<f32>,
        mode: PanelRevealMode,
    ) -> Self {
        Self::new(child, progress, resting_height, mode).enters_from(PanelRevealEdge::Top)
    }

    /// Dock this reveal against `edge`, which also selects its travel axis.
    ///
    /// Defaults to [`PanelRevealEdge::Leading`], so omitting this call keeps
    /// the standard left-edge travel. A vertical edge reinterprets the resting
    /// extent passed to [`Self::new`] as a height.
    #[must_use]
    pub fn enters_from(mut self, edge: PanelRevealEdge) -> Self {
        self.enters_from = edge;
        self
    }

    /// Opt into replaying the child's fixed-width layout while the reveal
    /// travels. Used by `app_shell`, whose app-message rebuild boundary makes
    /// the unchanged-child precondition explicit.
    pub(crate) fn cache_layout_while_revealing(mut self) -> Self {
        self.cache_layout_while_revealing = true;
        self
    }

    /// Read overlay progress from persistent shell state at draw time. This
    /// keeps the content pane's cached layout valid while only the translated
    /// overlay moves.
    pub(crate) fn live_progress(mut self, progress: std::rc::Rc<std::cell::Cell<f32>>) -> Self {
        self.live_progress = Some(progress);
        self
    }

    /// Read progress and travel from a caller-owned clock.
    ///
    /// Reservation is pinned by default; [`PanelRevealClock::layout_driven`]
    /// instead reflows adjacent content on each owner frame. The motion owner
    /// remains responsible for advancing the clock and requesting frames.
    #[must_use]
    pub fn clock(self, clock: &PanelRevealClock) -> Self {
        let mut reveal = self.live_clock(clock.shared());
        reveal.input_target = (clock.layout_driven && reveal.mode == PanelRevealMode::Reserve)
            .then(|| std::rc::Rc::clone(&clock.target));
        reveal
    }

    /// Read both the sampled instant and the remaining travel from one shared
    /// clock.
    ///
    /// Callers use this over [`live_progress`](Self::live_progress) so the two
    /// cannot wire separately: an instant-only reveal is lane-undrivable, an
    /// omission invisible at the call site. One handle carrying both makes the
    /// lane-capable form the default for every shell overlay, not a forgotten
    /// opt-in.
    /// Let the travelling child paint `px` past its entering edge, out to
    /// the window's edge.
    pub(crate) fn overhang(mut self, px: f32) -> Self {
        self.overhang = px.max(0.0);
        self
    }

    /// `bounds` reaching [`overhang`](Self::overhang) further out across the
    /// entering edge: what a travelling child may paint into.
    fn paint_bounds(&self, bounds: Rectangle) -> Rectangle {
        let reach = self.overhang;
        match self.enters_from {
            PanelRevealEdge::Leading => Rectangle {
                x: bounds.x - reach,
                width: bounds.width + reach,
                ..bounds
            },
            PanelRevealEdge::Trailing => Rectangle {
                width: bounds.width + reach,
                ..bounds
            },
            PanelRevealEdge::Top => Rectangle {
                y: bounds.y - reach,
                height: bounds.height + reach,
                ..bounds
            },
            PanelRevealEdge::Bottom => Rectangle {
                height: bounds.height + reach,
                ..bounds
            },
        }
    }

    pub(crate) fn live_clock(mut self, clock: RevealClock) -> Self {
        self.layout_driver = clock.layout_driver;
        self.responsive_reserve = clock.responsive_reserve;
        self.live_retained = clock.retained_panel;
        self.live_spatial = clock.spatial;

        let reveal = self.live_progress(clock.progress);

        let reveal = match clock.travel {
            Some(travel) => reveal.live_travel(travel),
            None => reveal,
        };

        match clock.pin {
            Some(pin) => reveal.live_pin(pin),
            None => reveal,
        }
    }

    /// Publish the held reservation fraction into shell-owned state.
    pub(crate) fn live_pin(mut self, pin: std::rc::Rc<std::cell::Cell<Option<f32>>>) -> Self {
        self.live_pin = Some(pin);
        self
    }

    /// Mirror this reveal's own pin into the shared cell.
    ///
    /// From every latch/release site: a rider never reads a fraction the panel
    /// no longer holds. Two reveals may share one clock (split band's sidebar
    /// segment + pane below travel together) — latching/releasing in lockstep,
    /// writing the same value.
    fn publish_pin(&self, pinned: Option<f32>) {
        if let Some(pin) = self.live_pin.as_ref() {
            pin.set(pinned);
        }
    }

    /// Read the remaining travel from persistent shell state, so the reveal can
    /// be handed to the render lane as an animation rather than a sample
    ///
    ///
    /// The cell refreshes by the clock owner every advancing frame: retargets
    /// and reversals never publish a curve for an unmade journey.
    pub(crate) fn live_travel(
        mut self,
        travel: std::rc::Rc<std::cell::RefCell<Option<TrackCurve>>>,
    ) -> Self {
        self.live_travel = Some(travel);
        self
    }

    fn progress(&self) -> f32 {
        self.live_progress
            .as_ref()
            .map_or(self.progress, |progress| progress.get())
            .clamp(0.0, 1.0)
    }

    /// Where an overlay's content sits: the spatial value, which an
    /// expressive travel may carry past the resting place. Clip, reservation
    /// and visibility read the bounded [`Self::progress`] instead.
    fn spatial_progress(&self) -> f32 {
        self.live_spatial
            .as_ref()
            .map(|spatial| spatial.get())
            .filter(|spatial| spatial.is_finite())
            .unwrap_or_else(|| self.progress())
    }

    fn visible(&self) -> bool {
        self.progress() > f32::EPSILON
    }

    fn input_enabled(&self) -> bool {
        self.visible() && self.input_target.as_ref().is_none_or(|target| target.get())
    }

    fn note_input_state(&self, tree: &mut Tree) {
        if self.input_target.is_some() {
            let state = tree.state.downcast_ref::<PanelRevealLayout>();
            let enabled = self.input_enabled();
            if state.input_was_enabled.replace(enabled) && !enabled {
                tree.children[0].cancel_interactions();
            }
        }
    }

    /// Whether the travel this reveal is on is heading OPEN.
    ///
    /// Read from the published curve, which is the same travel the lane will
    /// sample, so the direction the pin is decided by and the direction the
    /// lane animates cannot disagree. Without a curve there is no travel to
    /// composite and the answer is `false`.
    fn growing(&self) -> bool {
        let Some(travel) = self.live_travel.as_ref() else {
            return false;
        };

        let curve = travel.borrow();

        let Some(curve) = curve.as_ref() else {
            return false;
        };

        let Some((_, last)) = curve.points.last() else {
            return false;
        };

        *last > self.progress() + f32::EPSILON
    }

    /// Whether the curve this reveal was handed still describes travel it has
    /// not spent.
    ///
    /// [`progress`](Self::progress) is one sample — and on a travel's first
    /// frame it still reads the settled endpoint (`layout` precedes the frame's
    /// redraw event reaching `update_frame`: layout sees the previous frame's
    /// value). Pinning the reservation from that sample alone spends frame one
    /// at the endpoint: unpinned reservation, [`reserve_clip`](Self::reserve_clip)
    /// degenerating to full bounds (`progress / pinned == 1`), the settled
    /// width painted although the clock started. Frame two then jumps two
    /// frames' travel — the user-visible pre-animate jump. The curve is honest
    /// (republished from the same motion, same clock, as the sample): it exists
    /// exactly while a travel is anchored and unfinished, so an endpoint sample
    /// with a live curve STARTED, not ended. Curve-less sample-only reveals
    /// unaffected.
    fn travel_in_flight(&self) -> bool {
        let Some(travel) = self.live_travel.as_ref() else {
            return false;
        };

        let curve = travel.borrow();

        let Some(curve) = curve.as_ref() else {
            return false;
        };

        let Some((_, last)) = curve.points.last() else {
            return false;
        };

        (*last - self.progress()).abs() > f32::EPSILON
    }

    /// Whether this is a `Reserve` reveal part-way through its travel -- the
    /// state in which its reservation is pinned and its reveal composites
    ///
    ///
    /// The frame that STARTS a travel counts: see [`Self::travel_in_flight`]
    /// for why an endpoint sample there is a travel already under way.
    fn travelling_reserve(&self) -> bool {
        let progress = self.progress();

        self.mode == PanelRevealMode::Reserve
            && ((progress > f32::EPSILON && progress < 1.0 - f32::EPSILON)
                || self.travel_in_flight())
    }

    /// The reservation fraction to lay out at, and whether it is pinned
    ///
    ///
    /// While travelling this is the pin -- the travel's widest extent -- so the
    /// reserved width holds still and the reveal happens inside it. Settled, it
    /// is the live progress, which is the true end geometry.
    fn reserve_fraction(&self, state: &PanelRevealLayout) -> f32 {
        if self.responsive_reserve {
            return self.progress();
        }

        state
            .reserve_pin
            .get()
            .unwrap_or_else(|| self.progress())
            .clamp(0.0, 1.0)
    }

    /// The rectangle a `Reserve` reveal actually SHOWS inside its held
    /// reservation.
    ///
    /// One definition, three consumers: `draw` clips to it, the published
    /// track carries the identical rectangle, hit testing resolves through it —
    /// a composited replay is the picture, and reserved-but-unpainted strips
    /// stay unclickable. `edge` is the dock edge: an RTL sidebar reveals from
    /// its own edge, never across the window. The fixed far edge makes the
    /// SEAM (pane inner boundary — the divider/toggle's only rideable edge)
    /// the mover in both directions.
    fn reserve_clip(
        bounds: Rectangle,
        progress: f32,
        pinned: f32,
        edge: PanelRevealEdge,
    ) -> Rectangle {
        if pinned <= f32::EPSILON {
            return bounds;
        }

        let fraction = (progress / pinned).clamp(0.0, 1.0);

        if edge.is_vertical() {
            let height = bounds.height * fraction;

            return Rectangle {
                y: match edge {
                    PanelRevealEdge::Top => bounds.y,
                    _ => bounds.y + bounds.height - height,
                },
                height,
                ..bounds
            };
        }

        let width = bounds.width * fraction;

        Rectangle {
            x: match edge {
                PanelRevealEdge::Trailing => bounds.x + bounds.width - width,
                _ => bounds.x,
            },
            width,
            ..bounds
        }
    }

    /// The rectangle this reveal actually SHOWS.
    ///
    /// Pinning breaks "reserved == painted": mid-travel the reservation holds
    /// resting width while ink reveals inside, so `layout.bounds()` would keep
    /// a control clickable/hoverable/cursor-shaping across an unpainted strip.
    /// Every pointer path resolves here instead: hittable == visible.
    fn shown_bounds(&self, tree: &Tree, layout: Layout) -> Rectangle {
        let bounds = layout.bounds();

        if self.mode != PanelRevealMode::Reserve {
            return bounds;
        }

        let state = tree.state.downcast_ref::<PanelRevealLayout>();

        Self::reserve_clip(
            bounds,
            self.progress(),
            self.reserve_fraction(state),
            self.enters_from,
        )
    }

    /// Whether this is an `Overlay` reveal part-way through its travel, the
    /// state in which it owns a compositor track.
    fn travelling_overlay(&self) -> bool {
        let progress = self.progress();

        self.mode == PanelRevealMode::Overlay
            && progress > f32::EPSILON
            && progress < 1.0 - f32::EPSILON
    }

    // Conservative: the shell cannot tell whether the child or an earlier
    // widget raised the invalidation.
    fn note_child_invalidation(tree: &Tree, shell: &Shell<'_, Message>) {
        if matches!(
            shell.invalidation(),
            iced::advanced::shell::Invalidation::Layout(_)
                | iced::advanced::shell::Invalidation::Widgets
        ) {
            tree.state
                .downcast_ref::<PanelRevealLayout>()
                .child_invalidated_layout
                .set(true);
        }
    }

    fn child_layout(layout: Layout, tree: &Tree) -> Layout {
        layout
            .children(&tree.children)
            .next()
            .expect("PanelReveal always owns one child layout")
    }

    /// The translation an `Overlay` reveal paints at.
    ///
    /// Shared by `draw` (which records at it) and `update` (which publishes a
    /// track relative to it) so the painted travel and the composited travel
    /// cannot drift apart.
    fn overlay_offset(edge: PanelRevealEdge, bounds: Rectangle, progress: f32) -> Vector {
        let extent = if edge.is_vertical() {
            bounds.height
        } else {
            bounds.width
        };
        let travel = edge.offset_sign() * extent * (1.0 - progress);

        if edge.is_vertical() {
            Vector::new(0.0, travel)
        } else {
            Vector::new(travel, 0.0)
        }
    }
}

impl<'a, Message: 'a> From<PanelReveal<'a, Message>> for Element<'a, Message> {
    fn from(value: PanelReveal<'a, Message>) -> Self {
        Element::new(value)
    }
}

pub(crate) fn remaining_travel_for(
    now: iced::time::Instant,
    applied_ms: f32,
    is_animating: impl Fn(iced::time::Instant) -> bool,
) -> iced::time::Duration {
    if !is_animating(now) {
        return iced::time::Duration::ZERO;
    }

    let mut high = applied_ms.max(1.0);

    for _ in 0..8 {
        if !is_animating(now + iced::time::Duration::from_secs_f32(high / 1000.0)) {
            break;
        }
        high *= 2.0;
    }

    let mut low = 0.0_f32;

    for _ in 0..24 {
        let mid = 0.5 * (low + high);

        if is_animating(now + iced::time::Duration::from_secs_f32(mid / 1000.0)) {
            low = mid;
        } else {
            high = mid;
        }
    }

    iced::time::Duration::from_secs_f32(high / 1000.0)
}
