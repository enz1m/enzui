use super::*;
use std::time::Duration;

const RESTING: f32 = 320.0;

fn run() -> RunId {
    RunId::new(0xC0FFEE).expect("nonzero")
}

fn deadline(now: Instant) -> Instant {
    now + Duration::from_secs(5)
}

fn floors() -> Floors {
    Floors {
        observation: 10,
        submitted_sequence: 100,
        presented_frames: 50,
    }
}

fn presentation(submitted_sequence: u64) -> PresentationIdentity {
    PresentationIdentity {
        window: 7,
        surface_generation: 3,
        ownership_generation: 2,
        owner: PresentationOwner::Ui,
        scene_generation: submitted_sequence,
        submitted_sequence,
    }
}

fn frame(transition: TransitionId, progress: f32) -> FrameEvidence {
    FrameEvidence {
        transition: Some(transition),
        observation: 11,
        submitted_sequence: 101,
        presented_frames: 51,
        presentation: Some(presentation(101)),
        progress,
        reserved_width: progress * RESTING,
        resting_width: RESTING,
    }
}

fn requested(checkpoint: Checkpoint) -> (Session, TransitionId, Instant) {
    let mut session = Session::new(run());
    let transition = session.begin_transition(Direction::Close);
    let now = Instant::now();
    session
        .request(checkpoint, floors(), deadline(now))
        .expect("a transition is live");
    (session, transition, now)
}

fn qualifying() -> (Session, TransitionId, Instant, FrameEvidence) {
    let (session, transition, now) = requested(Checkpoint::Mid);
    (session, transition, now, frame(transition, 0.5))
}

#[test]
fn foreign_transition_identity_is_rejected() {
    let other_run = TransitionId {
        run: RunId::new(0xBEEF).expect("nonzero"),
        index: 1,
        direction: Direction::Close,
    };
    let other_direction = TransitionId {
        run: run(),
        index: 1,
        direction: Direction::Open,
    };

    for foreign in [other_run, other_direction] {
        let (mut session, _, now, _) = qualifying();
        let evidence = frame(foreign, 0.5);
        assert_eq!(
            session.observe(&evidence, now),
            Verdict::Reject(Rejection::WrongTransition),
            "{foreign} must not satisfy the request"
        );
        assert_eq!(session.failures().len(), 1);
        assert_eq!(session.failures()[0].rejection, Rejection::WrongTransition);
    }

    let mut session = Session::new(run());
    let first = session.begin_transition(Direction::Close);
    let _second = session.begin_transition(Direction::Close);
    let now = Instant::now();
    session
        .request(Checkpoint::Mid, floors(), deadline(now))
        .expect("live");
    assert_eq!(
        session.observe(&frame(first, 0.5), now),
        Verdict::Reject(Rejection::WrongTransition)
    );
}
