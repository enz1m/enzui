//! Exact-frame transition capture with geometry, presentation, transition-identity, and one-shot release checks.
use std::fmt;
use std::time::Instant;

/// How far inside the travel a [`Checkpoint::Mid`] frame's progress must sit.
///
/// A tenth of the travel at each end excludes endpoint-adjacent frames that
/// cannot visually establish a midpoint.
pub const MID_MARGIN: f32 = 0.1;

/// How close to an endpoint a [`Checkpoint::Start`] or [`Checkpoint::End`]
/// frame's progress must sit.
pub const ENDPOINT_MARGIN: f32 = 0.02;

/// Returns five percent of `resting_width`, with a two-logical-pixel floor, for endpoint and midpoint geometry checks.
#[must_use]
pub fn geometry_margin(resting_width: f32) -> f32 {
    (resting_width * 0.05).max(2.0)
}

/// Returns the allowed deviation from `progress * resting_width`: two percent of the resting width with a one-logical-pixel floor.
#[must_use]
pub fn geometry_tolerance(resting_width: f32) -> f32 {
    (resting_width * 0.02).max(1.0)
}

/// Selects which presentation milestone may qualify a capture.
///
/// The default requires compositor confirmation. Surface-only presentation is diagnostic, and the session exposes the selected policy for evidence reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PresentationPolicy {
    /// Only a frame the compositor reported as presented may be captured.
    #[default]
    CompositorConfirmed,
    /// A frame whose surface presentation returned may be captured, with no
    /// compositor confirmation. Diagnostic only.
    SurfacePresented,
}

impl PresentationPolicy {
    /// The lowercase name used in evidence records.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::CompositorConfirmed => "compositor-confirmed",
            Self::SurfacePresented => "surface-presented",
        }
    }

    /// Whether `evidence` satisfies this policy's presentation requirement.
    #[must_use]
    pub fn accepts(self, evidence: &FrameEvidence) -> bool {
        match self {
            Self::CompositorConfirmed => evidence.presentation.is_some_and(|presented| {
                presented.submitted_sequence == evidence.submitted_sequence
            }),
            Self::SurfacePresented => true,
        }
    }
}

/// A nonzero capture-run identity used to reject evidence bearing a different run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(u64);

impl RunId {
    /// Names a run explicitly. `None` for zero.
    #[must_use]
    pub fn new(value: u64) -> Option<Self> {
        (value != 0).then_some(Self(value))
    }

    /// Creates a nonzero run identifier derived from the process ID and wall-clock time.
    #[must_use]
    pub fn fresh() -> Self {
        let pid = u64::from(std::process::id());
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos() as u64);
        Self((pid << 40) ^ (nanos | 1))
    }

    /// The raw value, for reporting.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Which way the panel is travelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Visible to hidden: progress travels `1 -> 0`.
    Close,
    /// Hidden to visible: progress travels `0 -> 1`.
    Open,
}

impl Direction {
    /// The progress this direction starts from.
    #[must_use]
    pub fn origin(self) -> f32 {
        match self {
            Self::Close => 1.0,
            Self::Open => 0.0,
        }
    }

    /// The progress this direction settles at.
    #[must_use]
    pub fn destination(self) -> f32 {
        1.0 - self.origin()
    }

    /// The lowercase name used in evidence records.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Close => "close",
            Self::Open => "open",
        }
    }

    /// The direction that can start from the endpoint `evidence` is settled at.
    ///
    /// Returns `None` for inconsistent geometry or an intermediate frame.
    /// Initial capture direction must be chosen after responsive layout settles.
    ///
    /// ```
    /// use enzui_core::transition_capture::{Direction, FrameEvidence};
    ///
    /// let settled_closed = FrameEvidence {
    ///     transition: None,
    ///     observation: 1,
    ///     submitted_sequence: 1,
    ///     presented_frames: 1,
    ///     presentation: None,
    ///     progress: 0.0,
    ///     reserved_width: 0.0,
    ///     resting_width: 240.0,
    /// };
    /// let travelling = FrameEvidence {
    ///     progress: 0.5,
    ///     reserved_width: 120.0,
    ///     ..settled_closed
    /// };
    /// assert_eq!(Direction::from_settled(&settled_closed), Some(Direction::Open));
    /// assert_eq!(Direction::from_settled(&travelling), None);
    /// ```
    #[must_use]
    pub fn from_settled(evidence: &FrameEvidence) -> Option<Self> {
        if !geometry_is_consistent(evidence) {
            return None;
        }
        [Self::Close, Self::Open]
            .into_iter()
            .find(|direction| is_settled_at(evidence, direction.origin()))
    }
}

/// Identity of one transition edge within a run; reversals receive a new identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransitionId {
    run: RunId,
    index: u64,
    direction: Direction,
}

impl TransitionId {
    /// The capture run this transition belongs to.
    #[must_use]
    pub fn run(self) -> RunId {
        self.run
    }

    /// The 1-based edge index inside the run.
    #[must_use]
    pub fn index(self) -> u64 {
        self.index
    }

    /// Which way it travels.
    #[must_use]
    pub fn direction(self) -> Direction {
        self.direction
    }
}

impl fmt::Display for TransitionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}",
            self.run.get(),
            self.index,
            self.direction.name()
        )
    }
}

/// Which picture of a transition is being captured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Checkpoint {
    /// The settled state the transition starts from.
    Start,
    /// A strictly intermediate state.
    Mid,
    /// The settled state it arrives at.
    End,
}

impl Checkpoint {
    /// The lowercase name used in evidence records and file names.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Mid => "mid",
            Self::End => "end",
        }
    }

    /// The settled progress this checkpoint photographs for `direction`.
    ///
    /// [`Checkpoint::Start`] rests at the direction's origin and
    /// [`Checkpoint::End`] at its destination; [`Checkpoint::Mid`] has no
    /// endpoint and returns `None`.
    #[must_use]
    pub fn endpoint(self, direction: Direction) -> Option<f32> {
        match self {
            Self::Start => Some(direction.origin()),
            Self::End => Some(direction.destination()),
            Self::Mid => None,
        }
    }
}

/// The renderer that committed a presented frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PresentationOwner {
    /// The application UI thread committed the frame.
    Ui,
    /// The retained animation lane committed the frame.
    Lane,
}

impl PresentationOwner {
    /// The trace spelling used by iced.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Ui => "Ui",
            Self::Lane => "Lane",
        }
    }
}

/// Identity copied from an exact compositor presentation callback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PresentationIdentity {
    /// Iced window identity.
    pub window: u64,
    /// Surface generation at submission time.
    pub surface_generation: u64,
    /// UI/lane ownership generation at submission time.
    pub ownership_generation: u64,
    /// The renderer that committed the frame.
    pub owner: PresentationOwner,
    /// Retained scene generation sampled by this frame.
    pub scene_generation: u64,
    /// Sequence allocated immediately before the surface commit.
    pub submitted_sequence: u64,
}

/// A presented frame with measured transition geometry, runtime counters, and its exact compositor callback identity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameEvidence {
    /// The transition this frame belongs to, or `None` when the shell was not
    /// travelling — which is itself a refusal, not a wait.
    pub transition: Option<TransitionId>,
    /// Monotonic count of moving-frame layouts published by the shell.
    pub observation: u64,
    /// The highest frame sequence the runtime has sealed against a real frame
    /// key.
    pub submitted_sequence: u64,
    /// Frames the runtime has reported as presentation milestones.
    pub presented_frames: u64,
    /// The exact compositor callback joined to this submitted frame.
    ///
    /// `None` means only submission/surface presentation is known. In strict
    /// sessions that can only wait; it must never be inferred from an
    /// unrelated later callback.
    pub presentation: Option<PresentationIdentity>,
    /// The eased travel fraction this frame was laid out at.
    pub progress: f32,
    /// The panel width this frame actually reserved, in logical pixels.
    pub reserved_width: f32,
    /// The panel's settled width, in logical pixels.
    pub resting_width: f32,
}

/// Counter values captured when a checkpoint is requested.
///
/// A qualifying frame must advance layout, commit, and presentation beyond these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Floors {
    /// Moving-frame layouts published before the request.
    pub observation: u64,
    /// Highest sealed frame sequence before the request.
    pub submitted_sequence: u64,
    /// Presentation milestones before the request.
    pub presented_frames: u64,
}

/// A nonterminal result from observing a frame; any existing request remains open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Waiting {
    /// No checkpoint has been requested.
    NoRequest,
    /// No new moving-frame layout has been published yet.
    Observation,
    /// The frame has not satisfied the required post-request submission and presentation milestones.
    Present,
    /// The frame is presented but is not strictly intermediate.
    Intermediate,
    /// The frame is presented but has not settled at the endpoint.
    Endpoint,
    /// A held frame is waiting for a newer presentation milestone of the pinned state.
    HoldConfirmation,
    /// The held frame is confirmed and the acknowledgement is outstanding.
    Ack,
}

impl Waiting {
    /// The lowercase name used in evidence records.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::NoRequest => "no-request",
            Self::Observation => "observation",
            Self::Present => "present",
            Self::Intermediate => "intermediate",
            Self::Endpoint => "endpoint",
            Self::HoldConfirmation => "hold-confirmation",
            Self::Ack => "ack",
        }
    }
}

/// Why a capture attempt failed closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rejection {
    /// The evidence names no transition, or a request was made with no live
    /// transition to bind it to.
    MissingIdentity,
    /// The frame belongs to an earlier transition, another direction, or
    /// another capture run.
    WrongTransition,
    /// A newer transition edge started, so the pending request can never be
    /// satisfied.
    Superseded,
    /// A counter went backwards: the evidence source was replayed or reset.
    StaleFrame,
    /// The reported geometry contradicts the reported progress.
    GeometryInconsistent,
    /// Later evidence left the held state: a pinned midpoint moved at all, or
    /// an endpoint hold stopped resting at the endpoint it photographed.
    HoldDrifted,
    /// The shell is settled at the destination instead of the requested origin.
    WrongEndpoint,
    /// A request overlaps another request, or would reuse an observation already accepted for this transition.
    OutOfOrder,
    /// No qualifying frame arrived before the deadline.
    Deadline,
}

impl Rejection {
    /// The lowercase name used in evidence records.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::MissingIdentity => "missing-identity",
            Self::WrongTransition => "wrong-transition",
            Self::Superseded => "superseded",
            Self::StaleFrame => "stale-frame",
            Self::GeometryInconsistent => "geometry-inconsistent",
            Self::HoldDrifted => "hold-drifted",
            Self::WrongEndpoint => "wrong-endpoint",
            Self::OutOfOrder => "out-of-order",
            Self::Deadline => "deadline",
        }
    }
}

/// One-shot identity binding an external acknowledgement to the held frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HoldToken(u64);

impl HoldToken {
    /// Names a token read back from an acknowledgement. `None` for zero.
    #[must_use]
    pub fn new(value: u64) -> Option<Self> {
        (value != 0).then_some(Self(value))
    }

    /// The raw value, for reporting.
    #[must_use]
    pub fn get(self) -> u64 {
        self.0
    }
}

/// What a [`Session`] decided about one presented frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing to do yet.
    Wait(Waiting),
    /// This exact frame qualifies. Pin the animation clock now.
    Hold(HoldToken),
    /// A later presentation milestone accepted by the session policy confirmed the pinned state; capture it now.
    Confirmed(HoldToken),
    /// The attempt failed closed.
    Reject(Rejection),
}

/// What acknowledging a held frame did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AckOutcome {
    /// The checkpoint is recorded and the hold is released.
    Released,
    /// Nothing is held.
    NothingHeld,
    /// The token names a different frame; the hold stays.
    WrongToken,
    /// The token was already released; this is a duplicate.
    AlreadyReleased,
    /// The configured presentation milestone has not confirmed the pinned state; the hold remains active.
    Unconfirmed,
    /// The deadline passed before the acknowledgement arrived.
    Expired,
}

impl AckOutcome {
    /// Whether the checkpoint was recorded.
    #[must_use]
    pub fn released(self) -> bool {
        matches!(self, Self::Released)
    }

    /// The lowercase name used in evidence records.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Released => "released",
            Self::NothingHeld => "nothing-held",
            Self::WrongToken => "wrong-token",
            Self::AlreadyReleased => "already-released",
            Self::Unconfirmed => "unconfirmed",
            Self::Expired => "expired",
        }
    }
}

/// A frame this session is holding for capture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Held {
    /// The checkpoint being captured.
    pub checkpoint: Checkpoint,
    /// The transition it belongs to.
    pub transition: TransitionId,
    /// The token the external tool must echo back.
    pub token: HoldToken,
    /// The exact frame that is pinned.
    pub frame: FrameEvidence,
    /// Presentation counter from the later frame that confirmed the pinned state under the session policy; `None` until confirmation.
    pub confirmed_presented_frames: Option<u64>,
}

impl Held {
    /// Returns whether a later milestone accepted by the session policy confirmed the pinned state.
    #[must_use]
    pub fn confirmed(&self) -> bool {
        self.confirmed_presented_frames.is_some()
    }
}

/// One accepted checkpoint: the exact state that was captured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AcceptedCheckpoint {
    /// Which picture this is.
    pub checkpoint: Checkpoint,
    /// The transition it belongs to.
    pub transition: TransitionId,
    /// The token that was released.
    pub token: HoldToken,
    /// The frame that was held, confirmed and captured.
    pub frame: FrameEvidence,
    /// Presentation milestones observed at the moment the hold was confirmed.
    pub confirmed_presented_frames: u64,
}

impl AcceptedCheckpoint {
    /// One JSON evidence row.
    ///
    /// Written by the application next to its screenshot so a checker can join
    /// the picture to the exact transition, frame and geometry it shows.
    #[must_use]
    pub fn to_json(&self) -> String {
        format!(
            "{{\"record\":\"capture_checkpoint\",\"run\":{},\"transition_index\":{},\
\"direction\":\"{}\",\"checkpoint\":\"{}\",\"token\":{},\"observation\":{},\
\"submitted_sequence\":{},\"presented_frames\":{},\"confirmed_presented_frames\":{},\
\"presentation_window\":{},\"surface_generation\":{},\"ownership_generation\":{},\
\"presentation_owner\":\"{}\",\"scene_generation\":{},\"compositor_presented\":{},\
\"progress\":{:.6},\
\"reserved_width\":{:.3},\"resting_width\":{:.3}}}",
            self.transition.run().get(),
            self.transition.index(),
            self.transition.direction().name(),
            self.checkpoint.name(),
            self.token.get(),
            self.frame.observation,
            self.frame.submitted_sequence,
            self.frame.presented_frames,
            self.confirmed_presented_frames,
            self.frame
                .presentation
                .map_or(0, |identity| identity.window),
            self.frame
                .presentation
                .map_or(0, |identity| identity.surface_generation),
            self.frame
                .presentation
                .map_or(0, |identity| identity.ownership_generation),
            self.frame
                .presentation
                .map_or("None", |identity| identity.owner.name()),
            self.frame
                .presentation
                .map_or(0, |identity| identity.scene_generation),
            self.frame.presentation.is_some(),
            self.frame.progress,
            self.frame.reserved_width,
            self.frame.resting_width,
        )
    }
}

/// A capture attempt that failed closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failure {
    /// Which picture was being captured.
    pub checkpoint: Checkpoint,
    /// The transition the request was bound to.
    pub transition: TransitionId,
    /// Why it failed.
    pub rejection: Rejection,
}

impl Failure {
    /// One JSON evidence row.
    #[must_use]
    pub fn to_json(&self) -> String {
        format!(
            "{{\"record\":\"capture_failure\",\"run\":{},\"transition_index\":{},\
\"direction\":\"{}\",\"checkpoint\":\"{}\",\"rejection\":\"{}\"}}",
            self.transition.run().get(),
            self.transition.index(),
            self.transition.direction().name(),
            self.checkpoint.name(),
            self.rejection.name(),
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct Watermark {
    observation: u64,
    submitted_sequence: u64,
    presented_frames: u64,
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    checkpoint: Checkpoint,
    transition: TransitionId,
    floors: Floors,
    deadline: Instant,
    watermark: Watermark,
    superseded: bool,
    hold: Option<Held>,
}

/// The capture handshake for one run.
///
/// Owns the run identity, the transition counter, at most one outstanding
/// checkpoint request, and the accepted/failed record lists a report is built
/// from.
#[derive(Debug)]
pub struct Session {
    run: RunId,
    policy: PresentationPolicy,
    next_index: u64,
    next_token: u64,
    current: Option<TransitionId>,
    pending: Option<Pending>,
    released: Vec<HoldToken>,
    accepted: Vec<AcceptedCheckpoint>,
    failures: Vec<Failure>,
}

impl Session {
    /// Starts a capture session for `run`, accepting only
    /// compositor-confirmed frames.
    #[must_use]
    pub fn new(run: RunId) -> Self {
        Self::with_policy(run, PresentationPolicy::CompositorConfirmed)
    }

    /// Starts a capture session with an explicit presentation policy.
    #[must_use]
    pub fn with_policy(run: RunId, policy: PresentationPolicy) -> Self {
        Self {
            run,
            policy,
            next_index: 0,
            next_token: 0,
            current: None,
            pending: None,
            released: Vec::new(),
            accepted: Vec::new(),
            failures: Vec::new(),
        }
    }

    /// This session's run identity.
    #[must_use]
    pub fn run(&self) -> RunId {
        self.run
    }

    /// The presentation milestone this session accepts.
    #[must_use]
    pub fn presentation_policy(&self) -> PresentationPolicy {
        self.policy
    }

    /// Starts a transition edge and marks any request for the previous edge as superseded.
    pub fn begin_transition(&mut self, direction: Direction) -> TransitionId {
        self.next_index += 1;
        let id = TransitionId {
            run: self.run,
            index: self.next_index,
            direction,
        };
        self.current = Some(id);
        if let Some(pending) = self.pending.as_mut() {
            pending.superseded = true;
        }
        id
    }

    /// The transition the most recent edge started, if any.
    #[must_use]
    pub fn current(&self) -> Option<TransitionId> {
        self.current
    }

    /// The frame being held for capture, if any.
    #[must_use]
    pub fn held(&self) -> Option<Held> {
        self.pending.and_then(|pending| pending.hold)
    }

    /// The checkpoint request outstanding right now, if any.
    #[must_use]
    pub fn pending_checkpoint(&self) -> Option<(Checkpoint, TransitionId)> {
        self.pending
            .map(|pending| (pending.checkpoint, pending.transition))
    }

    /// Requests one checkpoint of the live transition.
    ///
    /// # Errors
    /// [`Rejection::MissingIdentity`] when no transition edge has been
    /// declared, and [`Rejection::OutOfOrder`] when a request is already
    /// outstanding — two concurrent captures of one shell cannot both be the
    /// frame on screen.
    pub fn request(
        &mut self,
        checkpoint: Checkpoint,
        floors: Floors,
        deadline: Instant,
    ) -> Result<TransitionId, Rejection> {
        let Some(transition) = self.current else {
            return Err(Rejection::MissingIdentity);
        };
        if self.pending.is_some() {
            return Err(Rejection::OutOfOrder);
        }
        self.pending = Some(Pending {
            checkpoint,
            transition,
            floors,
            deadline,
            watermark: Watermark {
                observation: floors.observation,
                submitted_sequence: floors.submitted_sequence,
                presented_frames: floors.presented_frames,
            },
            superseded: false,
            hold: None,
        });
        Ok(transition)
    }

    /// Judges one presented frame against the outstanding request.
    pub fn observe(&mut self, evidence: &FrameEvidence, now: Instant) -> Verdict {
        let Some(mut pending) = self.pending else {
            return Verdict::Wait(Waiting::NoRequest);
        };
        if pending.superseded {
            return self.fail(pending, Rejection::Superseded);
        }
        if now > pending.deadline {
            return self.fail(pending, Rejection::Deadline);
        }
        let Some(transition) = evidence.transition else {
            return self.fail(pending, Rejection::MissingIdentity);
        };
        if transition.run != self.run || transition.index < pending.transition.index {
            return self.fail(pending, Rejection::WrongTransition);
        }
        if transition.index > pending.transition.index {
            return self.fail(pending, Rejection::Superseded);
        }
        if transition.direction != pending.transition.direction {
            return self.fail(pending, Rejection::WrongTransition);
        }
        if !geometry_is_consistent(evidence) {
            return self.fail(pending, Rejection::GeometryInconsistent);
        }
        if evidence.observation < pending.watermark.observation
            || evidence.submitted_sequence < pending.watermark.submitted_sequence
            || evidence.presented_frames < pending.watermark.presented_frames
        {
            return self.fail(pending, Rejection::StaleFrame);
        }
        pending.watermark = Watermark {
            observation: evidence.observation,
            submitted_sequence: evidence.submitted_sequence,
            presented_frames: evidence.presented_frames,
        };

        if let Some(mut hold) = pending.hold {
            if hold.confirmed() {
                self.pending = Some(pending);
                return Verdict::Wait(Waiting::Ack);
            }
            if !held_state_kept(&hold, evidence) || !same_presentation_stream(&hold.frame, evidence)
            {
                return self.fail(pending, Rejection::HoldDrifted);
            }
            if !self.policy.accepts(evidence)
                || evidence.submitted_sequence <= hold.frame.submitted_sequence
                || evidence.presented_frames <= hold.frame.presented_frames
            {
                pending.hold = Some(hold);
                self.pending = Some(pending);
                return Verdict::Wait(Waiting::HoldConfirmation);
            }
            let token = hold.token;
            hold.confirmed_presented_frames = Some(evidence.presented_frames);
            pending.hold = Some(hold);
            self.pending = Some(pending);
            return Verdict::Confirmed(token);
        }

        if evidence.observation <= pending.floors.observation {
            self.pending = Some(pending);
            return Verdict::Wait(Waiting::Observation);
        }
        if evidence.submitted_sequence <= pending.floors.submitted_sequence
            || evidence.presented_frames <= pending.floors.presented_frames
            || !self.policy.accepts(evidence)
        {
            self.pending = Some(pending);
            return Verdict::Wait(Waiting::Present);
        }
        if self.accepted.iter().any(|accepted| {
            accepted.transition == pending.transition
                && accepted.frame.observation >= evidence.observation
        }) {
            return self.fail(pending, Rejection::OutOfOrder);
        }
        match pending.checkpoint {
            Checkpoint::Mid => {
                if !is_strictly_intermediate(evidence) {
                    self.pending = Some(pending);
                    return Verdict::Wait(Waiting::Intermediate);
                }
            }
            Checkpoint::Start => {
                let direction = pending.transition.direction;
                if !is_settled_at(evidence, direction.origin()) {
                    if is_settled_at(evidence, direction.destination()) {
                        return self.fail(pending, Rejection::WrongEndpoint);
                    }
                    self.pending = Some(pending);
                    return Verdict::Wait(Waiting::Endpoint);
                }
            }
            Checkpoint::End => {
                if !is_settled_at(evidence, pending.transition.direction.destination()) {
                    self.pending = Some(pending);
                    return Verdict::Wait(Waiting::Endpoint);
                }
            }
        }

        self.next_token += 1;
        let token = HoldToken(self.next_token);
        pending.hold = Some(Held {
            checkpoint: pending.checkpoint,
            transition: pending.transition,
            token,
            frame: *evidence,
            confirmed_presented_frames: None,
        });
        self.pending = Some(pending);
        Verdict::Hold(token)
    }

    /// Records the captured checkpoint and releases the hold.
    ///
    /// The token must name the held frame, a later milestone accepted by the session policy must have confirmed it, and the deadline must not have passed.
    pub fn acknowledge(&mut self, token: HoldToken, now: Instant) -> AckOutcome {
        if self.released.contains(&token) {
            return AckOutcome::AlreadyReleased;
        }
        let Some(pending) = self.pending else {
            return AckOutcome::NothingHeld;
        };
        let Some(hold) = pending.hold else {
            return AckOutcome::NothingHeld;
        };
        if hold.token != token {
            return AckOutcome::WrongToken;
        }
        if now > pending.deadline {
            self.fail(pending, Rejection::Deadline);
            return AckOutcome::Expired;
        }
        let Some(confirmed_presented_frames) = hold.confirmed_presented_frames else {
            return AckOutcome::Unconfirmed;
        };
        self.accepted.push(AcceptedCheckpoint {
            checkpoint: hold.checkpoint,
            transition: hold.transition,
            token,
            frame: hold.frame,
            confirmed_presented_frames,
        });
        self.released.push(token);
        self.pending = None;
        AckOutcome::Released
    }

    /// Expires an outstanding request after its deadline, including when no frame arrives.
    pub fn expire(&mut self, now: Instant) -> Option<Rejection> {
        let pending = self.pending?;
        if now <= pending.deadline {
            return None;
        }
        self.fail(pending, Rejection::Deadline);
        Some(Rejection::Deadline)
    }

    /// Every checkpoint this run captured, in order.
    #[must_use]
    pub fn accepted(&self) -> &[AcceptedCheckpoint] {
        &self.accepted
    }

    /// Every capture attempt this run failed closed on.
    #[must_use]
    pub fn failures(&self) -> &[Failure] {
        &self.failures
    }

    fn fail(&mut self, pending: Pending, rejection: Rejection) -> Verdict {
        self.pending = None;
        self.failures.push(Failure {
            checkpoint: pending.checkpoint,
            transition: pending.transition,
            rejection,
        });
        Verdict::Reject(rejection)
    }
}

fn geometry_is_consistent(evidence: &FrameEvidence) -> bool {
    let FrameEvidence {
        progress,
        reserved_width,
        resting_width,
        ..
    } = *evidence;
    if !progress.is_finite() || !reserved_width.is_finite() || !resting_width.is_finite() {
        return false;
    }
    if !(0.0..=1.0).contains(&progress) || resting_width <= 0.0 || reserved_width < 0.0 {
        return false;
    }
    if reserved_width > resting_width + geometry_tolerance(resting_width) {
        return false;
    }
    (reserved_width - progress * resting_width).abs() <= geometry_tolerance(resting_width)
}

fn is_strictly_intermediate(evidence: &FrameEvidence) -> bool {
    let margin = geometry_margin(evidence.resting_width);
    evidence.progress > MID_MARGIN
        && evidence.progress < 1.0 - MID_MARGIN
        && evidence.reserved_width > margin
        && evidence.reserved_width < evidence.resting_width - margin
}

fn is_settled_at(evidence: &FrameEvidence, endpoint: f32) -> bool {
    let margin = geometry_margin(evidence.resting_width);
    (evidence.progress - endpoint).abs() <= ENDPOINT_MARGIN
        && (evidence.reserved_width - endpoint * evidence.resting_width).abs() <= margin
}

fn same_presentation_stream(before: &FrameEvidence, after: &FrameEvidence) -> bool {
    match (before.presentation, after.presentation) {
        (Some(before), Some(after)) => {
            before.window == after.window
                && before.surface_generation == after.surface_generation
                && before.ownership_generation == after.ownership_generation
                && before.owner == after.owner
                && before.scene_generation == after.scene_generation
        }
        _ => true,
    }
}

fn pinned_state_held(pinned: &FrameEvidence, evidence: &FrameEvidence) -> bool {
    (pinned.progress - evidence.progress).abs() <= f32::EPSILON
        && (pinned.reserved_width - evidence.reserved_width).abs() <= f32::EPSILON
        && (pinned.resting_width - evidence.resting_width).abs() <= f32::EPSILON
}

// Only midpoint holds pin the clock; endpoint holds may continue converging within the settled band.
fn held_state_kept(hold: &Held, evidence: &FrameEvidence) -> bool {
    let Some(endpoint) = hold.checkpoint.endpoint(hold.transition.direction) else {
        return pinned_state_held(&hold.frame, evidence);
    };
    if (hold.frame.resting_width - evidence.resting_width).abs() > f32::EPSILON {
        return false;
    }
    if !is_settled_at(evidence, endpoint) {
        return false;
    }
    let held_distance = (hold.frame.progress - endpoint).abs();
    let frame_distance = (evidence.progress - endpoint).abs();
    frame_distance <= held_distance + f32::EPSILON
}

#[cfg(test)]
mod tests;
